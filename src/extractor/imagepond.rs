//! Independent public ImagePond single-file page preview.
//! Album/profile queue pagination and full Chevereto metadata remain open.
use crate::{
    message::{media_metadata, ExtractorMessage},
    safe_filename, MediaItem,
};
use reqwest::Client;
use std::error::Error;
use url::Url;

const MAX_HTML_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    path: String,
    id: String,
}

fn safe_slug(value: &str) -> bool {
    (1..=180).contains(&value.len())
        && value != "."
        && value != ".."
        && !value.contains("..")
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

pub fn match_url(input: &str) -> Option<Target> {
    let url = Url::parse(input).ok()?;
    if url.scheme() != "https"
        || !matches!(url.host_str(), Some("imagepond.net" | "www.imagepond.net"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
    {
        return None;
    }
    let path: Vec<_> = url.path_segments()?.collect();
    let [route, id] = path.as_slice() else {
        return None;
    };
    if !matches!(*route, "i" | "img" | "image" | "video") || !safe_slug(id) {
        return None;
    }
    Some(Target {
        path: format!("{route}/{id}"),
        id: (*id).to_owned(),
    })
}

fn property(html: &str, name: &str) -> Option<String> {
    let needle = format!("property=\"og:{name}\" content=\"");
    let rest = html.split_once(&needle)?.1;
    let value = rest.split_once('"')?.0;
    if value.len() > 2048 || value.chars().any(char::is_control) {
        return None;
    }
    Some(
        value
            .replace("&amp;", "&")
            .replace("&quot;", "\"")
            .replace("&#39;", "'"),
    )
}

fn valid_media_link(link: &str) -> Option<(Url, String)> {
    let url = Url::parse(link).ok()?;
    let host = url.host_str()?;
    if url.scheme() != "https"
        || !(host == "imagepond.net" || host.ends_with(".imagepond.net"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let filename = url.path_segments()?.next_back()?;
    let (_, extension) = filename.rsplit_once('.')?;
    if !matches!(
        extension.to_ascii_lowercase().as_str(),
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif" | "mp4" | "webm"
    ) {
        return None;
    }
    if filename.is_empty() || filename.len() > 240 || filename.contains('%') {
        return None;
    }
    Some((url, safe_filename(filename)))
}

fn parse_page(target: &Target, html: &str) -> Result<(MediaItem, Option<String>), Box<dyn Error + Send + Sync>> {
    let kind = property(html, "type").unwrap_or_default();
    let field = if kind == "image" { "image" } else { "video" };
    let media = property(html, field).ok_or("ImagePond page has no public media OG link")?;
    let (url, filename) =
        valid_media_link(&media).ok_or("ImagePond page media link has an unsupported origin")?;
    let title = property(html, "title").filter(|v| !v.is_empty() && v.len() <= 256);
    let _ = target;
    Ok((
        MediaItem {
            url: url.to_string(),
            filename,
            extractor: "imagepond",
            post_id: None,
        },
        title,
    ))
}

async fn resolve_at_base(
    client: &Client,
    target: &Target,
    base: &Url,
) -> Result<(MediaItem, Option<String>), Box<dyn Error + Send + Sync>> {
    let endpoint = base.join(&target.path)?;
    let mut response = client.get(endpoint).send().await?.error_for_status()?;
    if response
        .content_length()
        .is_some_and(|size| size > MAX_HTML_BYTES as u64)
    {
        return Err("ImagePond HTML response exceeds byte limit".into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_HTML_BYTES.saturating_sub(body.len()) {
            return Err("ImagePond HTML response exceeds byte limit".into());
        }
        body.extend_from_slice(&chunk);
    }
    parse_page(target, std::str::from_utf8(&body)?)
}

pub async fn resolve(
    client: &Client,
    target: Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let (media, _) =
        resolve_at_base(client, &target, &Url::parse("https://www.imagepond.net/")?).await?;
    Ok(vec![media])
}

pub async fn resolve_messages(
    client: &Client,
    target: Target,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let (item, title) =
        resolve_at_base(client, &target, &Url::parse("https://www.imagepond.net/")?).await?;
    let mut metadata = media_metadata(&item);
    metadata.insert("subcategory".into(), "file".into());
    metadata.insert("file_id".into(), target.id.into());
    if let Some(title) = title {
        metadata.insert("title".into(), title.into());
    }
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
    fn matches_exact_single_file_routes() {
        for path in ["i/abc123", "img/abc123", "image/abc123", "video/some-video.abc123"] {
            assert!(match_url(&format!("https://imagepond.net/{path}")).is_some());
        }
        for url in [
            "https://imagepond.net/a/abc123",
            "https://otherimagepond.net/i/abc123",
            "http://imagepond.net/i/abc123",
            "https://imagepond.net/i/a%2Fb",
            "https://imagepond.net/i/..",
            "https://imagepond.net/i/abc123?token=secret",
            "https://user:secret@imagepond.net/i/abc123",
        ] {
            assert!(match_url(url).is_none(), "{url}");
        }
    }

    #[test]
    fn rejects_untrusted_og_media_and_keeps_safe_filename() {
        let target = match_url("https://imagepond.net/i/abc123").unwrap();
        let evil = r#"<meta property="og:type" content="image"><meta property="og:image" content="https://evil.example/p.jpg">"#;
        assert!(parse_page(&target, evil).is_err());
        let good = r#"<meta property="og:type" content="image"><meta property="og:image" content="https://cdn.imagepond.net/media/public.jpg"><meta property="og:title" content="Public image">"#;
        let (file, title) = parse_page(&target, good).unwrap();
        assert_eq!(file.url, "https://cdn.imagepond.net/media/public.jpg");
        assert_eq!(file.filename, "public.jpg");
        assert_eq!(file.post_id, None);
        assert_eq!(title.as_deref(), Some("Public image"));
    }

    #[tokio::test]
    async fn bounded_single_public_image_page_uses_only_one_request() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buffer = [0_u8; 4096];
            let n = stream.read(&mut buffer).await.unwrap();
            assert!(String::from_utf8_lossy(&buffer[..n]).starts_with("GET /i/abc123 HTTP/1.1"));
            let body = r#"<meta property="og:type" content="image"><meta property="og:image" content="https://cdn.imagepond.net/photos/example.webp">"#;
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(reply.as_bytes()).await.unwrap();
        });
        let target = match_url("https://imagepond.net/i/abc123").unwrap();
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let (file, _) = resolve_at_base(&Client::new(), &target, &base).await.unwrap();
        assert_eq!(file.filename, "example.webp");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn rejects_declared_oversized_html_before_reading_body() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buffer = [0_u8; 4096];
            let _ = stream.read(&mut buffer).await.unwrap();
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                MAX_HTML_BYTES + 1
            );
            stream.write_all(reply.as_bytes()).await.unwrap();
        });
        let target = match_url("https://imagepond.net/i/abc123").unwrap();
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let error = resolve_at_base(&Client::new(), &target, &base)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("byte limit"));
        server.await.unwrap();
    }
}
