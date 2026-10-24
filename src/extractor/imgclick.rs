//! Public Imgclick single original-image subset of pinned imagehosts.py.
//! Full complex form-posts, authenticated file behavior and live parity remain open.
use crate::{
    message::{media_metadata, ExtractorMessage, Metadata},
    safe_filename, MediaItem,
};
use reqwest::Client;
use std::error::Error;
use url::Url;

const MAX_HTML_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    token: String,
    host: String,
}

fn valid_token(value: &str) -> bool {
    (1..=80).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

fn valid_hint(value: &str) -> bool {
    (6..=240).contains(&value.len())
        && !value.contains("..")
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
        && value.to_ascii_lowercase().ends_with(".html")
}

pub fn match_url(input: &str) -> Option<Target> {
    let lower = input.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "/../", "/./"]
        .iter()
        .any(|word| lower.contains(word))
    {
        return None;
    }
    // The pinned route accepts an omitted scheme. Canonicalize only the
    // literal Imgclick host forms before applying strict URL validation.
    let source = if input.starts_with("imgclick.net/")
        || input.starts_with("www.imgclick.net/")
    {
        format!("https://{input}")
    } else {
        input.to_owned()
    };
    let url = Url::parse(&source).ok()?;
    let host = url.host_str()?;
    if !matches!(url.scheme(), "https" | "http")
        || !matches!(host, "imgclick.net" | "www.imgclick.net")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let parts: Vec<_> = url.path_segments()?.collect();
    let token = match parts.as_slice() {
        [token] if valid_token(token) => *token,
        [token, hint] if valid_token(token) && valid_hint(hint) => *token,
        _ => return None,
    };
    Some(Target {
        token: token.to_owned(),
        host: host.to_owned(),
    })
}

fn media_url(raw: &str) -> Option<(Url, String)> {
    let mut url = Url::parse(raw).ok()?;
    // The pinned host normalizes public HTTP image links to HTTPS before
    // downloading. Never permit cleartext media retrieval.
    if url.scheme() == "http" {
        url.set_scheme("https").ok()?;
    }
    let host = url.host_str()?;
    if url.scheme() != "https"
        || !(host == "imgclick.net" || host.ends_with(".imgclick.net"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let name = url.path_segments()?.next_back()?;
    if name.is_empty() || name.len() > 240 || name.contains('%') {
        return None;
    }
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    if !matches!(
        ext.as_str(),
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif"
    ) {
        return None;
    }
    let safe = safe_filename(name);
    Some((url, safe))
}

fn safe_alt(raw: &str, ext: &str) -> Option<String> {
    let title = raw
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    if title.is_empty()
        || title.len() > 225
        || title.contains('/')
        || title.contains('\\')
        || title.chars().any(char::is_control)
    {
        return None;
    }
    let name = if title.rsplit_once('.').is_some_and(|(_, suffix)| {
        matches!(
            suffix.to_ascii_lowercase().as_str(),
            "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif"
        )
    }) {
        title
    } else {
        format!("{title}.{ext}")
    };
    Some(safe_filename(&name))
}

fn parse_page(
    html: &str,
    target: &Target,
) -> Result<(MediaItem, Metadata), Box<dyn Error + Send + Sync>> {
    let from = html
        .split_once("<br><img src=\"")
        .map(|(_, after)| after)
        .ok_or("Imgclick original public image marker is missing")?;
    let (raw, rest) = from
        .split_once('"')
        .ok_or("Imgclick source image attribute is incomplete")?;
    let (url, fallback) = media_url(raw).ok_or("Imgclick original media origin is unsafe")?;
    let attrs = rest
        .split_once('>')
        .map(|(attrs, _)| attrs)
        .ok_or("Imgclick original media element is incomplete")?;
    let ext = fallback
        .rsplit_once('.')
        .map(|(_, extension)| extension)
        .unwrap_or("jpg");
    let name = attrs
        .split_once("alt=\"")
        .and_then(|(_, rest)| rest.split_once('"'))
        .and_then(|(alt, _)| safe_alt(alt, ext))
        .unwrap_or(fallback);
    let item = MediaItem {
        url: url.to_string(),
        filename: name,
        extractor: "imgclick",
        post_id: None,
    };
    let mut metadata = media_metadata(&item);
    metadata.insert("subcategory".into(), "image".into());
    metadata.insert("token".into(), target.token.clone().into());
    metadata.insert(
        "post_url".into(),
        format!("https://{}/{}", target.host, target.token).into(),
    );
    Ok((item, metadata))
}

async fn resolve_at_base(
    client: &Client,
    target: &Target,
    base: &Url,
) -> Result<(MediaItem, Metadata), Box<dyn Error + Send + Sync>> {
    // Pinned imagehosts.py uses the shared "complex" continue form for Imgclick.
    // Build its fields from the validated token, never from the input URL's query.
    let mut response = client
        .post(base.join(&target.token)?)
        .form(&[
            ("op", "view"),
            ("id", target.token.as_str()),
            ("pre", "1"),
            ("adb", "1"),
            ("next", "Continue+to+image+...+"),
        ])
        .send()
        .await?
        .error_for_status()?;
    if response
        .content_length()
        .is_some_and(|n| n > MAX_HTML_BYTES as u64)
    {
        return Err("Imgclick public HTML exceeds byte limit".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_HTML_BYTES.saturating_sub(bytes.len()) {
            return Err("Imgclick public HTML exceeds byte limit".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    parse_page(std::str::from_utf8(&bytes)?, target)
}

pub async fn resolve(
    client: &Client,
    target: Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse(&format!("https://{}/", target.host))?;
    let (item, _) = resolve_at_base(client, &target, &base).await?;
    Ok(vec![item])
}

pub async fn resolve_messages(
    client: &Client,
    target: Target,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse(&format!("https://{}/", target.host))?;
    let (item, metadata) = resolve_at_base(client, &target, &base).await?;
    Ok(vec![
        ExtractorMessage::Directory {
            metadata: metadata.clone(),
        },
        ExtractorMessage::Url {
            url: item.url,
            metadata,
        },
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    #[test]
    fn pinned_imgclick_token_and_safe_source_names() {
        let source = match_url("https://imgclick.net/4tbrre1oxew9/test-_-_.png.html").unwrap();
        assert_eq!(source.token, "4tbrre1oxew9");
        assert!(match_url("https://www.imgclick.net/4tbrre1oxew9").is_some());
        assert!(match_url("http://imgclick.net/4tbrre1oxew9").is_some());
        assert!(match_url("imgclick.net/4tbrre1oxew9").is_some());
        assert!(match_url("www.imgclick.net/4tbrre1oxew9/sample.png.html").is_some());
        for bad in [
            "evilimgclick.net/4tbrre1oxew9",
            "imgclick.net/4tbrre1oxew9?private=1",
            "https://evilimgclick.net/4tbrre1oxew9",
            "https://imgclick.net/4tbrre1oxew9?token=private",
            "https://imgclick.net/4tbrre1oxew9/test.exe",
            "https://imgclick.net/4tbrre1oxew9/../../etc",
            "https://imgclick.net/a%2fb",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn public_imgclick_image_marker_extracts_only_safe_originals() {
        let target = match_url("https://imgclick.net/4tbrre1oxew9").unwrap();
        let html = r#"<div>header</div><br><img src="https://img1.imgclick.net/data/test.png" alt="sample &amp; image">"#;
        let (file, metadata) = parse_page(html, &target).unwrap();
        assert_eq!(file.filename, "sample & image.png");
        assert_eq!(metadata["token"], "4tbrre1oxew9");
        assert_eq!(file.post_id, None);
        assert!(parse_page("<html>missing</html>", &target).is_err());
        let (upgraded, _) = media_url("http://img1.imgclick.net/file.jpg").unwrap();
        assert_eq!(upgraded.as_str(), "https://img1.imgclick.net/file.jpg");
        for bad in [
            "http://evilimgclick.net/file.jpg",
            "https://evilimgclick.net/file.jpg",
            "https://user:secret@img1.imgclick.net/file.jpg",
            "https://img1.imgclick.net/file.jpg?secret=1",
            "https://img1.imgclick.net/file.exe",
        ] {
            assert!(media_url(bad).is_none(), "{bad}");
        }
    }

    #[tokio::test]
    async fn loopback_single_public_image_is_bounded() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            // Consume the complete form body rather than assuming it arrives with
            // the HTTP headers in a single read.
            loop {
                let mut chunk = [0u8; 1024];
                let count = stream.read(&mut chunk).await.unwrap();
                assert!(count > 0, "request ended before form was complete");
                request.extend_from_slice(&chunk[..count]);
                assert!(request.len() < 16 * 1024, "unexpected oversized request");
                let raw = String::from_utf8_lossy(&request);
                if let Some((headers, body)) = raw.split_once("\r\n\r\n") {
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length: ")
                                .and_then(|value| value.parse::<usize>().ok())
                        })
                        .unwrap();
                    if body.len() >= length {
                        break;
                    }
                }
            }
            let raw = String::from_utf8_lossy(&request);
            assert!(raw.starts_with("POST /4tbrre1oxew9 HTTP/1.1"));
            assert!(raw.contains("application/x-www-form-urlencoded"));
            let body = raw.split_once("\r\n\r\n").unwrap().1;
            let params: std::collections::HashMap<String, String> =
                url::form_urlencoded::parse(body.as_bytes())
                    .into_owned()
                    .collect();
            assert_eq!(params.len(), 5);
            assert_eq!(params["op"], "view");
            assert_eq!(params["id"], "4tbrre1oxew9");
            assert_eq!(params["pre"], "1");
            assert_eq!(params["adb"], "1");
            assert_eq!(params["next"], "Continue+to+image+...+");
            let html = r#"<br><img src="https://img1.imgclick.net/data/file.png" alt="Public">"#;
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
                html.len()
            );
            stream.write_all(reply.as_bytes()).await.unwrap();
        });
        let target = match_url("https://imgclick.net/4tbrre1oxew9").unwrap();
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let (file, _) = resolve_at_base(&Client::new(), &target, &base)
            .await
            .unwrap();
        assert_eq!(file.filename, "Public.png");
        server.await.unwrap();
    }
}
