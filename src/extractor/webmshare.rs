//! Independent public Webmshare video-page extractor preview.
//! Complete upstream metadata, filename option formatting, authentication,
//! CDN changes and opt-in live conformance remain unsupported.
use crate::{safe_filename, MediaItem};
use reqwest::Client;
use std::error::Error;
use url::Url;

const MAX_HTML_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Video { id: String },
}

fn valid_host(host: &str) -> bool {
    matches!(host, "webmshare.com" | "www.webmshare.com")
        || host
            .strip_prefix('s')
            .and_then(|rest| rest.strip_suffix(".webmshare.com"))
            .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
}

fn valid_id(id: &str) -> bool {
    (3..=128).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

pub fn match_url(input: &str) -> Option<Target> {
    if input.to_ascii_lowercase().contains("%2e") {
        return None;
    }
    let url = Url::parse(input).ok()?;
    if url.scheme() != "https" || !valid_host(url.host_str()?) {
        return None;
    }
    let parts: Vec<_> = url.path_segments()?.collect();
    let id = match parts.as_slice() {
        [id] | ["play", id] | ["download-webm", id] if valid_id(id) => *id,
        _ => return None,
    };
    Some(Target::Video { id: id.to_owned() })
}

fn meta(html: &str, property: &str) -> Option<String> {
    let marker = format!("property=\"{property}\" content=\"");
    let value = html.split_once(&marker)?.1.split_once('"')?.0;
    (value.len() <= 2048).then(|| value.to_owned())
}

fn video_item(html: &str, id: &str) -> Option<MediaItem> {
    let raw = meta(html, "og:video")?;
    let video_url = if raw.starts_with("//") {
        format!("https:{raw}")
    } else {
        raw
    };
    let url = Url::parse(&video_url).ok()?;
    if url.scheme() != "https" || !valid_host(url.host_str()?) {
        return None;
    }
    // Pinned upstream treats this extractor's download media as WebM.
    let mut title = meta(html, "og:title").unwrap_or_default();
    if let Some((left, _)) = title.rsplit_once(" — ") {
        title = left.to_owned();
    }
    let title = title.trim();
    let filename = if title.is_empty() || title.eq_ignore_ascii_case("webmshare") {
        format!("{id}.webm")
    } else if title.len() <= 180 {
        safe_filename(&format!("{id} {title}.webm"))
    } else {
        format!("{id}.webm")
    };
    if matches!(filename.as_str(), "" | "." | "..") {
        return None;
    }
    Some(MediaItem {
        url: url.to_string(),
        filename,
        extractor: "webmshare",
        post_id: id.parse::<u64>().ok(),
    })
}

async fn read_html(client: &Client, url: Url) -> Result<String, Box<dyn Error + Send + Sync>> {
    let mut response = client.get(url).send().await?.error_for_status()?;
    if response
        .content_length()
        .is_some_and(|size| size > MAX_HTML_BYTES as u64)
    {
        return Err("Webmshare page exceeds the configured HTML bound".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len().saturating_add(chunk.len()) > MAX_HTML_BYTES {
            return Err("Webmshare page exceeds the configured HTML bound".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8(bytes)?)
}

async fn resolve_at_base(
    client: &Client,
    base: &Url,
    target: Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    match target {
        Target::Video { id } => {
            let html = read_html(client, base.join(&id)?).await?;
            let item = video_item(&html, &id).ok_or("Webmshare page missing safe OG video media")?;
            Ok(vec![item])
        }
    }
}

pub async fn resolve(
    client: &Client,
    target: Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    resolve_at_base(client, &Url::parse("https://webmshare.com/")?, target).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_pinned_public_video_and_download_routes_only() {
        assert_eq!(
            match_url("https://webmshare.com/play/ABc_123"),
            Some(Target::Video {
                id: "ABc_123".into(),
            })
        );
        assert_eq!(
            match_url("https://s4.webmshare.com/download-webm/video99"),
            Some(Target::Video {
                id: "video99".into(),
            })
        );
        for url in [
            "http://webmshare.com/abc123",
            "https://fakewebmshare.com/abc123",
            "https://sbad.webmshare.com/abc123",
            "https://webmshare.com/xy",
            "https://webmshare.com/play/abc/extra",
            "https://webmshare.com/play/bad%2Fid",
        ] {
            assert!(match_url(url).is_none(), "{url}");
        }
    }

    #[test]
    fn extracts_protocol_relative_video_and_filename_with_sanitized_title() {
        let html = concat!(
            "<meta property=\"og:title\" content=\"Birds in Flight — webmshare\">",
            "<meta property=\"og:video\" content=\"//s4.webmshare.com/media/Birds.webm\">"
        );
        let item = video_item(html, "ABC123").unwrap();
        assert_eq!(item.filename, "ABC123 Birds in Flight.webm");
        assert_eq!(item.url, "https://s4.webmshare.com/media/Birds.webm");
        assert_eq!(item.extractor, "webmshare");
        assert!(video_item(
            "<meta property=\"og:video\" content=\"https://webmshare.com.evil.test/malware.webm\">",
            "abc"
        )
        .is_none());
        assert!(video_item(
            "<meta property=\"og:video\" content=\"file:///etc/private\">",
            "abc"
        )
        .is_none());
    }

    #[tokio::test]
    async fn public_video_resolves_metadata_via_bounded_local_http() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = [0_u8; 4096];
            let read = socket.read(&mut bytes).await.unwrap();
            let request = String::from_utf8_lossy(&bytes[..read]);
            assert!(request.starts_with("GET /abc123 HTTP/1.1"), "{request}");
            let body = "<meta property=\"og:video\" content=\"//s3.webmshare.com/abc123.webm\">";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        let base = Url::parse(&format!("http://{address}/")).unwrap();
        let items = resolve_at_base(
            &Client::new(),
            &base,
            Target::Video {
                id: "abc123".to_owned(),
            },
        )
        .await
        .unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].filename, "abc123.webm");
        assert_eq!(items[0].url, "https://s3.webmshare.com/abc123.webm");
        server.await.unwrap();
    }
}
