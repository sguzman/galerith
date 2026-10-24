//! Pinned imagehosts.py Imxto public file/gallery subset in independent Rust.
use crate::{
    message::{media_metadata, queued_url, ExtractorMessage, Metadata},
    safe_filename, MediaItem,
};
use reqwest::Client;
use std::{collections::HashSet, error::Error};
use url::Url;

const MAX_HTML_BYTES: usize = 2 * 1024 * 1024;
const MAX_GALLERY_PAGES: u16 = 4;
const MAX_GALLERY_CHILDREN: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Image { path: String, token: String },
    Gallery(String),
}

fn safe_token(value: &str) -> bool {
    (1..=80).contains(&value.len())
        && value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

pub fn match_url(raw: &str) -> Option<Target> {
    let lower = raw.to_ascii_lowercase();
    if lower.contains("/../")
        || lower.contains("/./")
        || lower.contains("/%2e%2e/")
        || lower.contains("/%2e/")
    {
        return None;
    }
    let url = Url::parse(raw).ok()?;
    if url.scheme() != "https"
        || !matches!(url.host_str(), Some("imx.to" | "www.imx.to" | "img.yt" | "www.img.yt"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let parts: Vec<_> = url.path_segments()?.collect();
    match parts.as_slice() {
        ["i", token] if safe_token(token) => Some(Target::Image {
            path: format!("i/{token}"),
            token: (*token).into(),
        }),
        [filename] if filename.starts_with("img-") && filename.ends_with(".html") => {
            let token = filename.strip_prefix("img-")?.strip_suffix(".html")?;
            if !safe_token(token) {
                return None;
            }
            Some(Target::Image {
                path: format!("img-{token}.html"),
                token: token.into(),
            })
        }
        ["g", id] if safe_token(id) => Some(Target::Gallery((*id).into())),
        _ => None,
    }
}

fn valid_media(raw: &str) -> Option<(Url, String)> {
    let url = Url::parse(raw).ok()?;
    if url.scheme() != "https"
        || url.host_str() != Some("image.imx.to")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let filename = url.path_segments()?.next_back()?;
    let ext = filename.rsplit_once('.')?.1.to_ascii_lowercase();
    if filename.is_empty() || filename.len() > 240 || filename.contains('%')
        || !matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif")
    {
        return None;
    }
    let name = safe_filename(filename);
    Some((url, name))
}

fn image_from_html(
    html: &str,
    token: &str,
) -> Result<(MediaItem, Metadata), Box<dyn Error + Send + Sync>> {
    let source = html
        .split_once("<div style=\"text-align:center;\"><a href=\"")
        .map(|(_, rest)| rest)
        .ok_or("Imxto public page lacks expected image anchor")?;
    let (media, after) = source.split_once('"').ok_or("Imxto media anchor is incomplete")?;
    let (url, fallback) = valid_media(media)
        .ok_or("Imxto original image link has unsupported origin")?;
    let filename = after
        .split_once(" title=\"")
        .and_then(|(_, rest)| rest.split_once('"'))
        .and_then(|(name, _)| {
            if name.is_empty() || name.len() > 230
                || name.contains('/') || name.contains('\\')
                || name.chars().any(char::is_control)
            {
                return None;
            }
            let suffix = fallback.rsplit_once('.').map(|(_, ext)| ext).unwrap_or("jpg");
            let name = if name.rsplit_once('.').is_some_and(|(_, ext)| {
                matches!(ext.to_ascii_lowercase().as_str(),
                    "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif")
            }) {
                name.to_string()
            } else {
                format!("{name}.{suffix}")
            };
            Some(safe_filename(&name))
        })
        .unwrap_or(fallback);
    let item = MediaItem {
        url: url.to_string(),
        filename,
        extractor: "imxto",
        post_id: None,
    };
    let mut meta = media_metadata(&item);
    meta.insert("subcategory".into(), "image".into());
    meta.insert("token".into(), token.into());
    Ok((item, meta))
}

async fn read_limited_html(
    response: reqwest::Response,
) -> Result<String, Box<dyn Error + Send + Sync>> {
    let mut response = response.error_for_status()?;
    if response.content_length().is_some_and(|v| v > MAX_HTML_BYTES as u64) {
        return Err("Imxto public HTML exceeds byte limit".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_HTML_BYTES.saturating_sub(bytes.len()) {
            return Err("Imxto public HTML exceeds byte limit".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8(bytes)?)
}

async fn image_at_base(
    client: &Client,
    path: &str,
    token: &str,
    base: &Url,
) -> Result<(MediaItem, Metadata), Box<dyn Error + Send + Sync>> {
    let endpoint = base.join(path)?;
    let response = client
        .post(endpoint)
        .form(&[("imgContinue", "Continue+to+image+...+")])
        .send()
        .await?;
    let html = read_limited_html(response).await?;
    image_from_html(&html, token)
}

fn gallery_children(
    html: &str,
    gallery: &str,
    seen: &mut HashSet<String>,
    messages: &mut Vec<ExtractorMessage>,
) -> bool {
    let root = Url::parse("https://imx.to/").expect("fixed Imxto origin");
    for (position, _) in html.match_indices("<a") {
        let Some(after) = html.get(position + 2..) else { continue };
        let Some((attrs, _)) = after.split_once('>') else { continue };
        if !attrs.starts_with(char::is_whitespace) { continue; }
        let Some((prefix, href)) = attrs.split_once("href=\"") else { continue };
        if !prefix.ends_with(char::is_whitespace) { continue; }
        let Some((raw, _)) = href.split_once('"') else { continue };
        let Ok(link) = root.join(raw) else { continue };
        let Some(Target::Image { path, token }) = match_url(link.as_str()) else {
            continue;
        };
        let canonical = format!("https://imx.to/{path}");
        if !seen.insert(canonical.clone()) { continue; }
        let mut meta = Metadata::new();
        meta.insert("category".into(), "imxto".into());
        meta.insert("subcategory".into(), "image".into());
        meta.insert("gallery_id".into(), gallery.into());
        meta.insert("token".into(), token.into());
        messages.push(queued_url(canonical, meta));
        if messages.len() >= MAX_GALLERY_CHILDREN {
            return false;
        }
    }
    html.contains("class=\"pagination") && !html.contains("class=\"disabled\">Last")
}

async fn gallery_at_base(
    client: &Client,
    id: &str,
    max_pages: u16,
    base: &Url,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let mut messages = Vec::new();
    let mut seen = HashSet::new();
    for page in 1..=max_pages.clamp(1, MAX_GALLERY_PAGES) {
        let mut endpoint = base.join(&format!("g/{id}"))?;
        endpoint.query_pairs_mut().append_pair("page", &page.to_string());
        let response = client.get(endpoint).send().await?;
        let html = read_limited_html(response).await?;
        if !gallery_children(&html, id, &mut seen, &mut messages) {
            break;
        }
        if page < max_pages.min(MAX_GALLERY_PAGES) {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    }
    Ok(messages)
}

pub async fn resolve(
    client: &Client,
    target: Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    match target {
        Target::Image { path, token } => {
            let (item, _) =
                image_at_base(client, &path, &token, &Url::parse("https://imx.to/")?).await?;
            Ok(vec![item])
        }
        Target::Gallery(_) => Err("Imxto gallery requires --dump-messages or --follow-queues".into()),
    }
}

pub async fn resolve_messages(
    client: &Client,
    target: Target,
    max_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    match target {
        Target::Image { path, token } => {
            let (item, metadata) =
                image_at_base(client, &path, &token, &Url::parse("https://imx.to/")?).await?;
            Ok(vec![
                ExtractorMessage::Directory { metadata: metadata.clone() },
                ExtractorMessage::Url { url: item.url, metadata },
            ])
        }
        Target::Gallery(id) => {
            gallery_at_base(client, &id, max_pages, &Url::parse("https://imx.to/")?).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::TcpListener};

    #[test]
    fn pinned_imxto_and_imgyt_image_gallery_routes_are_safe() {
        assert_eq!(
            match_url("https://imx.to/i/1qdeva"),
            Some(Target::Image { path: "i/1qdeva".into(), token: "1qdeva".into() })
        );
        assert_eq!(
            match_url("https://img.yt/img-57a2050547b97.html"),
            Some(Target::Image {
                path: "img-57a2050547b97.html".into(),
                token: "57a2050547b97".into()
            })
        );
        assert_eq!(match_url("https://imx.to/g/ozdy"), Some(Target::Gallery("ozdy".into())));
        for bad in [
            "http://imx.to/i/1qdeva", "https://evilimx.to/i/1qdeva",
            "https://imx.to/i/1qdeva?secret=1", "https://imx.to/i/a%2Fb",
            "https://imx.to/g/../i/1qdeva",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn pinned_original_image_and_filename_are_projected_safely() {
        let html = r#"<div style="text-align:center;"><a href="https://image.imx.to/u/i/2018/04/09/1qdeva.png" title="test">View</a></div>"#;
        let (item, metadata) = image_from_html(html, "1qdeva").unwrap();
        assert_eq!(item.url, "https://image.imx.to/u/i/2018/04/09/1qdeva.png");
        assert_eq!(item.filename, "test.png");
        assert_eq!(metadata["token"], "1qdeva");
        assert!(valid_media("https://evil.example/attack.png").is_none());
        assert!(valid_media("http://image.imx.to/example.png").is_none());
        assert!(valid_media("https://image.imx.to/a.png?secret=1").is_none());
    }

    #[test]
    fn gallery_queues_only_canonical_safe_image_anchors_in_order() {
        let html = r#"<a href="/i/one123">one</a><a href="https://imx.to/img-two456.html">two</a><script>let fake='href="/i/ghost123"'</script><a data-href="/i/hidden123">not link</a><a href="//evil.example/i/bad123">foreign</a>"#;
        let mut seen = HashSet::new();
        let mut events = Vec::new();
        assert!(!gallery_children(html, "ozdy", &mut seen, &mut events));
        assert_eq!(events.len(), 2);
        assert_eq!(serde_json::to_value(&events[0]).unwrap()["url"], "https://imx.to/i/one123");
        assert_eq!(serde_json::to_value(&events[1]).unwrap()["url"], "https://imx.to/img-two456.html");
    }

    #[tokio::test]
    async fn public_image_request_uses_one_bounded_post_form() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = [0_u8; 4096];
            let n = socket.read(&mut buffer).await.unwrap();
            let request = String::from_utf8_lossy(&buffer[..n]);
            assert!(request.starts_with("POST /i/1qdeva HTTP/1.1"), "{request}");
            let body = r#"<div style="text-align:center;"><a href="https://image.imx.to/u/i/2018/04/09/1qdeva.png" title="test">View</a></div>"#;
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(reply.as_bytes()).await.unwrap();
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let (file, _) = image_at_base(&Client::new(), "i/1qdeva", "1qdeva", &base).await.unwrap();
        assert_eq!(file.filename, "test.png");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn gallery_pagination_is_capped_and_deduplicates_across_two_pages() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (page, html) in [
                (1, r#"<a href="/i/one123">one</a><span class="pagination">More</span>"#),
                (2, r#"<a href="/i/one123">duplicate</a><a href="/i/two456">two</a>"#),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buffer = [0_u8; 4096];
                let n = socket.read(&mut buffer).await.unwrap();
                let request = String::from_utf8_lossy(&buffer[..n]);
                assert!(request.starts_with("GET /g/ozdy?page="), "{request}");
                assert!(request.contains(&format!("page={page}")), "{request}");
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
                    html.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let events = gallery_at_base(&Client::new(), "ozdy", 3, &base).await.unwrap();
        assert_eq!(events.len(), 2);
        server.await.unwrap();
    }
}
