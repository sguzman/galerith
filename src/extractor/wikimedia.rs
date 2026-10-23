//! Independent Wikimedia Commons public-file API preview.
//! This uses the documented MediaWiki Action API, not HTML scraping.
//! Public Commons File:, ordinary article and Category: routes use Action API.
//! Other MediaWiki hosts, recursive subcategories, revisions, full metadata
//! and archive formatting remain part of the complete gallery-dl parity gate.
use crate::{safe_filename, MediaItem};
use percent_encoding::percent_decode_str;
use reqwest::{header::USER_AGENT, Client};
use serde_json::Value;
use std::{collections::HashSet, error::Error, time::Duration};
use url::Url;

const API: &str = "https://commons.wikimedia.org/w/api.php";
const MAX_RESPONSE: usize = 2 * 1024 * 1024;
const MAX_MEDIA: usize = 2000;
const MAX_API_PAGES: u16 = 100;
const USER_AGENT_VALUE: &str = concat!(
    "Galerith/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/sguzman/galerith)"
);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    CommonsFile { title: String },
    CommonsArticle { title: String },
    CommonsCategory { title: String },
}

impl Target {
    fn title(&self) -> &str {
        match self {
            Self::CommonsFile { title }
            | Self::CommonsArticle { title }
            | Self::CommonsCategory { title } => title,
        }
    }

    fn continuation_field(&self) -> Option<&'static str> {
        match self {
            Self::CommonsFile { .. } => None,
            Self::CommonsArticle { .. } => Some("gimcontinue"),
            Self::CommonsCategory { .. } => Some("gcmcontinue"),
        }
    }
}

pub fn match_url(input: &str) -> Option<Target> {
    let url = Url::parse(input).ok()?;
    if url.scheme() != "https"
        || url.host_str() != Some("commons.wikimedia.org")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let raw = url.path().strip_prefix("/wiki/")?;
    let title = percent_decode_str(raw).decode_utf8().ok()?;
    if title.is_empty()
        || title.len() > 180
        || title
            .chars()
            .any(|ch| ch.is_control() || matches!(ch, '/' | '\\' | '#' | '?'))
        || title == "."
        || title == ".."
    {
        return None;
    }
    let title = title.replace('_', " ");
    if title.starts_with("File:") && title.len() > 5 {
        Some(Target::CommonsFile { title })
    } else if title.starts_with("Category:") && title.len() > 9 {
        Some(Target::CommonsCategory { title })
    } else if !title.contains(':') {
        Some(Target::CommonsArticle { title })
    } else {
        None
    }
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
    for page in pages.iter().take(MAX_MEDIA) {
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
            }
        }
    }
    Ok(media)
}

fn continuation(
    data: &Value,
    target: &Target,
) -> Result<Option<(String, String)>, Box<dyn Error + Send + Sync>> {
    let Some(expected) = target.continuation_field() else {
        return Ok(None);
    };
    let Some(object) = data.get("continue") else {
        return Ok(None);
    };
    let object = object
        .as_object()
        .ok_or("Wikimedia API returned invalid continuation object")?;
    let Some(token) = object.get(expected) else {
        return Ok(None);
    };
    let token = token.as_str().ok_or("Wikimedia continuation is not text")?;
    if token.is_empty() || token.len() > 512 || token.chars().any(char::is_control) {
        return Err("Wikimedia continuation token is invalid".into());
    }
    let generic = object
        .get("continue")
        .and_then(Value::as_str)
        .unwrap_or("-||");
    if generic.is_empty() || generic.len() > 512 || generic.chars().any(char::is_control) {
        return Err("Wikimedia generic continuation is invalid".into());
    }
    Ok(Some((generic.to_owned(), token.to_owned())))
}

async fn request_api_page(
    client: &Client,
    url: Url,
) -> Result<Value, Box<dyn Error + Send + Sync>> {
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
    Ok(serde_json::from_slice(&bytes)?)
}

async fn resolve_at_api(
    client: &Client,
    endpoint: &Url,
    target: &Target,
    max_pages: u16,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let mut media = Vec::new();
    let mut seen_media = HashSet::new();
    let mut seen_tokens = HashSet::new();
    let mut next: Option<(String, String)> = None;
    let pages = if matches!(target, Target::CommonsFile { .. }) {
        1
    } else {
        max_pages.clamp(1, MAX_API_PAGES)
    };
    for index in 0..pages {
        if index > 0 {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        let mut url = endpoint.clone();
        {
            let mut params = url.query_pairs_mut();
            params
                .append_pair("action", "query")
                .append_pair("format", "json")
                .append_pair("formatversion", "2")
                .append_pair("prop", "imageinfo")
                .append_pair("iiprop", "url")
                .append_pair("iilimit", "1")
                .append_pair("maxlag", "5");
            match target {
                Target::CommonsFile { .. } => {
                    params.append_pair("titles", target.title());
                }
                Target::CommonsArticle { .. } => {
                    params
                        .append_pair("generator", "images")
                        .append_pair("gimlimit", "50")
                        .append_pair("titles", target.title());
                }
                Target::CommonsCategory { .. } => {
                    params
                        .append_pair("generator", "categorymembers")
                        .append_pair("gcmlimit", "50")
                        .append_pair("gcmtype", "file")
                        .append_pair("gcmtitle", target.title());
                }
            }
            if let (Some((generic, token)), Some(field)) =
                (next.as_ref(), target.continuation_field())
            {
                params
                    .append_pair("continue", generic)
                    .append_pair(field, token);
            }
        }
        let data = request_api_page(client, url).await?;
        let images = parse_page(&data)?;
        for item in images {
            if seen_media.insert(item.url.clone()) {
                media.push(item);
                if media.len() >= MAX_MEDIA {
                    return Ok(media);
                }
            }
        }
        next = continuation(&data, target)?;
        let Some(ref tokens) = next else {
            break;
        };
        if !seen_tokens.insert(tokens.clone()) {
            break;
        }
    }
    Ok(media)
}

pub async fn resolve(
    client: &Client,
    target: Target,
    max_pages: u16,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    resolve_at_api(client, &Url::parse(API)?, &target, max_pages).await
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
        assert_eq!(
            match_url("https://commons.wikimedia.org/wiki/Category:Night_sky"),
            Some(Target::CommonsCategory {
                title: "Category:Night sky".into()
            })
        );
        assert_eq!(
            match_url("https://commons.wikimedia.org/wiki/Featured_pictures"),
            Some(Target::CommonsArticle {
                title: "Featured pictures".into()
            })
        );
        for bad in [
            "http://commons.wikimedia.org/wiki/File:Blue_sky.png",
            "https://commons.wikimedia.org.evil.test/wiki/File:Blue_sky.png",
            "https://commons.wikimedia.org/wiki/Category:",
            "https://commons.wikimedia.org/wiki/Special:RecentChanges",
            "https://commons.wikimedia.org/wiki/File:",
            "https://commons.wikimedia.org/wiki/File:Bad%2Fname.jpg",
            "https://commons.wikimedia.org/wiki/Category:Bad%2Fname",
            "https://commons.wikimedia.org/wiki/Featured_pictures?oldid=2",
            "https://commons.wikimedia.org/wiki/File:Blue_sky.png#section",
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
            1,
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
            1,
        )
        .await;
        assert!(response.unwrap_err().to_string().contains("2 MiB"));
        server.await.unwrap();
    }

    async fn collection_fixture(target: Target, generator: &str, expected_title: &str) {
        use std::collections::HashMap;
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let generator = generator.to_owned();
        let expected_title = expected_title.to_owned();
        let server = tokio::spawn(async move {
            for index in 0..2 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                loop {
                    let mut chunk = [0_u8; 2048];
                    let n = socket.read(&mut chunk).await.unwrap();
                    assert!(n > 0, "GET headers missing");
                    request.extend_from_slice(&chunk[..n]);
                    assert!(request.len() <= 8192);
                    if request.windows(4).any(|x| x == b"\r\n\r\n") {
                        break;
                    }
                }
                let request = String::from_utf8(request).unwrap();
                let path = request.lines().next().unwrap().split_whitespace().nth(1).unwrap();
                let url = Url::parse(&format!("http://localhost{path}")).unwrap();
                let query: HashMap<String, String> = url.query_pairs().into_owned().collect();
                assert_eq!(query.get("generator"), Some(&generator));
                assert_eq!(
                    query.get(if generator == "images" { "titles" } else { "gcmtitle" }),
                    Some(&expected_title)
                );
                assert_eq!(query.get("prop").map(String::as_str), Some("imageinfo"));
                assert_eq!(query.get("iiprop").map(String::as_str), Some("url"));
                assert_eq!(query.get("iilimit").map(String::as_str), Some("1"));
                assert_eq!(query.get("maxlag").map(String::as_str), Some("5"));
                if generator == "categorymembers" {
                    assert_eq!(query.get("gcmtype").map(String::as_str), Some("file"));
                    assert_eq!(query.get("gcmlimit").map(String::as_str), Some("50"));
                } else {
                    assert_eq!(query.get("gimlimit").map(String::as_str), Some("50"));
                }
                let field = if generator == "images" {
                    "gimcontinue"
                } else {
                    "gcmcontinue"
                };
                if index == 0 {
                    assert!(!query.contains_key(field));
                    assert!(!query.contains_key("continue"));
                } else {
                    assert_eq!(query.get(field).map(String::as_str), Some("next-page"));
                    assert_eq!(query.get("continue").map(String::as_str), Some("-||"));
                }
                let first = serde_json::json!({
                    "query":{"pages":[
                        {"ns":6,"title":"File:A.jpg","imageinfo":[{"url":"https://upload.wikimedia.org/wikipedia/commons/a/a0/A.jpg"}]},
                        {"ns":6,"title":"File:B.jpg","imageinfo":[{"url":"https://upload.wikimedia.org/wikipedia/commons/b/b0/B.jpg"}]}
                    ]},
                    "continue":{"continue":"-||"}
                });
                let mut first = first;
                first["continue"][field] = serde_json::json!("next-page");
                let second = serde_json::json!({
                    "query":{"pages":[
                        {"ns":6,"title":"File:B.jpg","imageinfo":[{"url":"https://upload.wikimedia.org/wikipedia/commons/b/b0/B.jpg"}]},
                        {"ns":6,"title":"File:C.jpg","imageinfo":[{"url":"https://upload.wikimedia.org/wikipedia/commons/c/c0/C.jpg"}]},
                        {"ns":6,"title":"File:Malicious.jpg","imageinfo":[{"url":"https://untrusted.invalid/bad.jpg"}]}
                    ]}
                });
                let body = if index == 0 { first.to_string() } else { second.to_string() };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let endpoint = Url::parse(&format!("http://{address}/w/api.php")).unwrap();
        let media = resolve_at_api(&Client::new(), &endpoint, &target, 2)
            .await
            .unwrap();
        server.await.unwrap();
        assert_eq!(
            media.iter().map(|item| item.filename.as_str()).collect::<Vec<_>>(),
            ["A.jpg", "B.jpg", "C.jpg"]
        );
    }

    #[tokio::test]
    async fn article_image_generator_uses_safe_bounded_api_continuation() {
        collection_fixture(
            Target::CommonsArticle {
                title: "Featured pictures".into(),
            },
            "images",
            "Featured pictures",
        )
        .await;
    }

    #[tokio::test]
    async fn category_file_generator_uses_safe_bounded_api_continuation() {
        collection_fixture(
            Target::CommonsCategory {
                title: "Category:Night sky".into(),
            },
            "categorymembers",
            "Category:Night sky",
        )
        .await;
    }

    #[test]
    fn continuation_rejects_malformed_tokens_and_wrong_fields() {
        let target = Target::CommonsCategory {
            title: "Category:Night sky".into(),
        };
        assert_eq!(
            continuation(&serde_json::json!({"continue":{"continue":"-||","gcmcontinue":"next"}}), &target).unwrap(),
            Some(("-||".into(), "next".into()))
        );
        assert!(
            continuation(&serde_json::json!({"continue":{"gcmcontinue":"bad\u{0000}next"}}), &target)
                .is_err()
        );
        assert!(
            continuation(&serde_json::json!({"continue":{"gcmcontinue":"x".repeat(513)}}), &target)
                .is_err()
        );
        assert!(
            continuation(&serde_json::json!({"continue":"invalid"}), &target).is_err()
        );
        assert_eq!(
            continuation(&serde_json::json!({"continue":{"gimcontinue":"wrong-generator"}}), &target)
                .unwrap(),
            None
        );
    }
}
