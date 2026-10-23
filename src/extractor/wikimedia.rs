//! Independent Wikimedia Commons public-file API preview.
//! This uses the documented MediaWiki Action API, not HTML scraping.
//! Only public Commons File: pages are currently implemented. Article/category
//! generators, other MediaWiki hosts, revisions, full metadata and archive
//! formatting remain part of the complete gallery-dl parity gate.
use crate::{safe_filename, MediaItem};
use percent_encoding::percent_decode_str;
use reqwest::{header::USER_AGENT, Client};
use serde_json::Value;
use std::error::Error;
use url::Url;

const API: &str = "https://commons.wikimedia.org/w/api.php";
const MAX_RESPONSE: usize = 2 * 1024 * 1024;
const USER_AGENT_VALUE: &str = concat!(
    "Galerith/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/sguzman/galerith)"
);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    CommonsFile { title: String },
}

pub fn match_url(input: &str) -> Option<Target> {
    let url = Url::parse(input).ok()?;
    if url.scheme() != "https"
        || url.host_str() != Some("commons.wikimedia.org")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
    {
        return None;
    }
    let raw = url.path().strip_prefix("/wiki/File:")?;
    let file = percent_decode_str(raw).decode_utf8().ok()?;
    if file.is_empty()
        || file.len() > 180
        || file
            .chars()
            .any(|ch| ch.is_control() || matches!(ch, '/' | '\\' | '#' | '?'))
        || file == "."
        || file == ".."
    {
        return None;
    }
    Some(Target::CommonsFile {
        title: format!("File:{}", file.replace('_', " ")),
    })
}

fn media_from_api(raw: &str) -> Option<MediaItem> {
    let url = Url::parse(raw).ok()?;
    if url.scheme() != "https"
        || url.host_str() != Some("upload.wikimedia.org")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || !url.path().starts_with("/wikipedia/commons/")
    {
        return None;
    }
    let part = url.path_segments()?.next_back()?;
    let filename = percent_decode_str(part).decode_utf8().ok()?;
    if filename.is_empty() || filename.len() > 240 || filename == "." || filename == ".." {
        return None;
    }
    let filename = safe_filename(&filename);
    Some(MediaItem {
        url: url.to_string(),
        filename,
        extractor: "wikimedia",
        post_id: None,
    })
}

fn parse_page(data: &Value) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    if data.get("error").is_some() {
        return Err("Wikimedia API returned an error".into());
    }
    let pages = data
        .pointer("/query/pages")
        .and_then(Value::as_array)
        .ok_or("Wikimedia API response missing query pages")?;
    let mut media = Vec::new();
    for page in pages.iter().take(4) {
        if page.get("missing").is_some() || page.get("ns").and_then(Value::as_u64) != Some(6) {
            continue;
        }
        if let Some(raw) = page
            .get("imageinfo")
            .and_then(Value::as_array)
            .and_then(|images| images.first())
            .and_then(|image| image.get("url"))
            .and_then(Value::as_str)
        {
            if let Some(item) = media_from_api(raw) {
                media.push(item);
                break;
            }
        }
    }
    Ok(media)
}

async fn resolve_at_api(
    client: &Client,
    endpoint: &Url,
    target: &Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let Target::CommonsFile { title } = target;
    let mut url = endpoint.clone();
    url.query_pairs_mut()
        .append_pair("action", "query")
        .append_pair("format", "json")
        .append_pair("formatversion", "2")
        .append_pair("prop", "imageinfo")
        .append_pair("iiprop", "url")
        .append_pair("titles", title)
        .append_pair("maxlag", "5");
    let mut response = client
        .get(url)
        .header(USER_AGENT, USER_AGENT_VALUE)
        .header("Accept", "application/json")
        .send()
        .await?
        .error_for_status()?;
    if response
        .content_length()
        .is_some_and(|size| size > MAX_RESPONSE as u64)
    {
        return Err("Wikimedia API response exceeded 2 MiB".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE {
            return Err("Wikimedia API response exceeded 2 MiB".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    parse_page(&serde_json::from_slice(&bytes)?)
}

pub async fn resolve(
    client: &Client,
    target: Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    resolve_at_api(client, &Url::parse(API)?, &target).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_public_commons_file_pages() {
        assert_eq!(
            match_url("https://commons.wikimedia.org/wiki/File:Blue_sky.png"),
            Some(Target::CommonsFile {
                title: "File:Blue sky.png".into()
            })
        );
        for bad in [
            "http://commons.wikimedia.org/wiki/File:Blue_sky.png",
            "https://commons.wikimedia.org.evil.test/wiki/File:Blue_sky.png",
            "https://commons.wikimedia.org/wiki/Category:Photos",
            "https://commons.wikimedia.org/wiki/File:",
            "https://commons.wikimedia.org/wiki/File:Bad%2Fname.jpg",
            "https://commons.wikimedia.org/wiki/File:Blue.png?oldid=2",
            "https://en.wikipedia.org/wiki/File:Blue_sky.png",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn validates_official_original_media_urls_and_filenames() {
        let photo =
            media_from_api("https://upload.wikimedia.org/wikipedia/commons/5/5f/Blue_sky.png")
                .unwrap();
        assert_eq!(photo.filename, "Blue_sky.png");
        assert_eq!(photo.extractor, "wikimedia");
        assert_eq!(photo.post_id, None);
        for bad in [
            "http://upload.wikimedia.org/wikipedia/commons/5/5f/A.png",
            "https://upload.wikimedia.org.evil.test/wikipedia/commons/A.png",
            "https://upload.wikimedia.org/wikipedia/en/A.png",
            "https://upload.wikimedia.org/wikipedia/commons/%2E%2E",
        ] {
            assert!(media_from_api(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn accepts_imageinfo_and_ignores_missing_or_unapproved_files() {
        let data = serde_json::json!({
            "query":{"pages":[
                {"ns":6,"missing":true,"title":"File:Missing.png"},
                {"ns":0,"imageinfo":[{"url":"https://upload.wikimedia.org/wikipedia/commons/a/A.png"}]},
                {"ns":6,"title":"File:Other.png","imageinfo":[{"url":"https://evil.test/Other.png"}]},
                {"ns":6,"title":"File:Blue sky.png","imageinfo":[{"url":"https://upload.wikimedia.org/wikipedia/commons/5/5f/Blue_sky.png"}]}
            ]}
        });
        let items = parse_page(&data).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].filename, "Blue_sky.png");
        assert!(parse_page(&serde_json::json!({"error":{"code":"maxlag"}})).is_err());
        assert!(parse_page(&serde_json::json!({"query":{}})).is_err());
    }

    #[tokio::test]
    async fn uses_public_action_api_with_identified_read_only_get() {
        use std::collections::HashMap;
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            loop {
                let mut chunk = [0_u8; 2048];
                let n = socket.read(&mut chunk).await.unwrap();
                assert!(n > 0, "incomplete request headers");
                request.extend_from_slice(&chunk[..n]);
                assert!(request.len() < 8192, "request headers too large");
                if request.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let request = String::from_utf8(request).unwrap();
            let line = request.lines().next().unwrap();
            let path = line.split_whitespace().nth(1).unwrap();
            let req_url = Url::parse(&format!("http://localhost{path}")).unwrap();
            assert_eq!(req_url.path(), "/w/api.php");
            assert!(line.starts_with("GET "));
            let pairs: HashMap<String, String> = req_url.query_pairs().into_owned().collect();
            assert_eq!(pairs.get("action").map(String::as_str), Some("query"));
            assert_eq!(pairs.get("format").map(String::as_str), Some("json"));
            assert_eq!(pairs.get("formatversion").map(String::as_str), Some("2"));
            assert_eq!(pairs.get("prop").map(String::as_str), Some("imageinfo"));
            assert_eq!(
                pairs.get("titles").map(String::as_str),
                Some("File:Blue sky.png")
            );
            assert_eq!(pairs.get("maxlag").map(String::as_str), Some("5"));
            assert!(request
                .to_ascii_lowercase()
                .contains("user-agent: galerith/"));
            assert!(request.contains("github.com/sguzman/galerith"));
            let body = r#"{"query":{"pages":[{"ns":6,"title":"File:Blue sky.png","imageinfo":[{"url":"https://upload.wikimedia.org/wikipedia/commons/5/5f/Blue_sky.png"}]}]}}"#;
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(reply.as_bytes()).await.unwrap();
        });
        let endpoint = Url::parse(&format!("http://{address}/w/api.php")).unwrap();
        let items = resolve_at_api(
            &Client::new(),
            &endpoint,
            &Target::CommonsFile {
                title: "File:Blue sky.png".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].filename, "Blue_sky.png");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn refuses_oversized_api_response_from_content_length() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            loop {
                let mut chunk = [0_u8; 1024];
                let n = socket.read(&mut chunk).await.unwrap();
                assert!(n > 0, "GET headers missing");
                request.extend_from_slice(&chunk[..n]);
                assert!(request.len() <= 8192);
                if request.windows(4).any(|x| x == b"\r\n\r\n") {
                    break;
                }
            }
            let headers = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                MAX_RESPONSE + 1
            );
            socket.write_all(headers.as_bytes()).await.unwrap();
        });
        let api = Url::parse(&format!("http://{address}/w/api.php")).unwrap();
        let response = resolve_at_api(
            &Client::new(),
            &api,
            &Target::CommonsFile {
                title: "File:Blue sky.png".into(),
            },
        )
        .await;
        assert!(response.unwrap_err().to_string().contains("2 MiB"));
        server.await.unwrap();
    }
}
