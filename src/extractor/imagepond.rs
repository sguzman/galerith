//! Independent public ImagePond single-file page preview.
//! Album/profile queue pagination and full Chevereto metadata remain open.
use crate::{
    message::{media_metadata, queued_url, ExtractorMessage, Metadata},
    safe_filename, MediaItem,
};
use reqwest::Client;
use std::{collections::HashSet, error::Error};
use url::Url;

const MAX_HTML_BYTES: usize = 2 * 1024 * 1024;
const MAX_QUEUE_CHILDREN: usize = 48;
const MAX_QUEUE_PAGES: u16 = 4;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueTarget {
    Album(String),
    User(String),
}

/// Bounded public album and explicit /user/ profile routes. Upstream
/// supports more aliases, auth and metadata than this conservative preview.
pub fn match_queue_url(input: &str) -> Option<QueueTarget> {
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
    match path.as_slice() {
        ["a" | "album", id] if safe_slug(id) => Some(QueueTarget::Album((*id).into())),
        ["user", name] if safe_slug(name) => Some(QueueTarget::User((*name).into())),
        _ => None,
    }
}

fn collect_queue_messages(html: &str, target: &QueueTarget) -> Vec<ExtractorMessage> {
    // Only actual HTML href attributes qualify; substrings of foreign links,
    // scripts and plain text must never turn into first-party child requests.
    let root = Url::parse("https://www.imagepond.net/").expect("fixed public origin");
    let mut seen = HashSet::new();
    let mut messages = Vec::new();
    for (position, _) in html.match_indices("href=\"") {
        let suffix = &html[position + "href=\"".len()..];
        let Some(raw_href) = suffix.split_once('"').map(|(href, _)| href) else {
            continue;
        };
        let Ok(url) = root.join(&raw_href.replace("&amp;", "&")) else {
            continue;
        };
        if url.scheme() != "https"
            || !matches!(url.host_str(), Some("imagepond.net" | "www.imagepond.net"))
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            continue;
        }
        let Some(parts) = url
            .path_segments()
            .map(|segments| segments.collect::<Vec<_>>())
        else {
            continue;
        };
        let [route, id] = parts.as_slice() else {
            continue;
        };
        if !matches!(*route, "i" | "img" | "image" | "video") || !safe_slug(id) {
            continue;
        }
        let child = format!("https://www.imagepond.net/{route}/{id}");
        if !seen.insert(child.clone()) {
            continue;
        }
        let mut metadata = Metadata::new();
        metadata.insert("category".into(), "imagepond".into());
        metadata.insert("subcategory".into(), "file".into());
        metadata.insert("file_id".into(), (*id).into());
        match target {
            QueueTarget::Album(id) => {
                metadata.insert("album_id".into(), id.clone().into());
            }
            QueueTarget::User(name) => {
                metadata.insert("username".into(), name.clone().into());
            }
        }
        messages.push(queued_url(child, metadata));
        if messages.len() >= MAX_QUEUE_CHILDREN {
            break;
        }
    }
    messages
}

fn next_listing_page(
    html: &str,
    current: &Url,
    max_pages: u16,
) -> Result<Option<Url>, Box<dyn Error + Send + Sync>> {
    for part in html.split("<a").skip(1) {
        let Some((attributes, _)) = part.split_once('>') else {
            continue;
        };
        if !attributes.contains("rel=\"next\"") {
            continue;
        }
        let href = attributes
            .split_once("href=\"")
            .and_then(|(_, suffix)| suffix.split_once('"'))
            .map(|(href, _)| href)
            .ok_or("ImagePond next page has no safe href")?;
        let next = current.join(href)?;
        if next.origin() != current.origin()
            || next.path() != current.path()
            || next.username() != current.username()
            || next.password().is_some()
            || next.fragment().is_some()
        {
            return Err("ImagePond next page has an unsafe origin or path".into());
        }
        let query: Vec<_> = next.query_pairs().collect();
        if query.len() != 1 || query[0].0 != "page" {
            return Err("ImagePond next page has unsupported query parameters".into());
        }
        let number = query[0].1.parse::<u16>()?;
        let current_page = current
            .query_pairs()
            .find(|(key, _)| key == "page")
            .map(|(_, value)| value.parse::<u16>())
            .transpose()?
            .unwrap_or(1);
        if number <= current_page {
            return Err("ImagePond next page must advance monotonically".into());
        }
        if number > max_pages.clamp(1, MAX_QUEUE_PAGES) {
            return Ok(None);
        }
        return Ok(Some(next));
    }
    Ok(None)
}

async fn queue_at_base(
    client: &Client,
    target: &QueueTarget,
    max_pages: u16,
    base: &Url,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let route = match target {
        QueueTarget::Album(id) => format!("a/{id}"),
        QueueTarget::User(name) => format!("user/{name}"),
    };
    let mut endpoint = base.join(&route)?;
    let mut seen = HashSet::new();
    let mut messages = Vec::new();
    for page in 0..max_pages.clamp(1, MAX_QUEUE_PAGES) {
        let mut response = client
            .get(endpoint.clone())
            .send()
            .await?
            .error_for_status()?;
        if response
            .content_length()
            .is_some_and(|size| size > MAX_HTML_BYTES as u64)
        {
            return Err("ImagePond listing HTML exceeds byte limit".into());
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if chunk.len() > MAX_HTML_BYTES.saturating_sub(body.len()) {
                return Err("ImagePond listing HTML exceeds byte limit".into());
            }
            body.extend_from_slice(&chunk);
        }
        let html = std::str::from_utf8(&body)?;
        for event in collect_queue_messages(html, target) {
            let ExtractorMessage::Queue { url, .. } = &event else {
                continue;
            };
            if seen.insert(url.clone()) {
                messages.push(event);
            }
            if messages.len() >= MAX_QUEUE_CHILDREN {
                return Ok(messages);
            }
        }
        let Some(next) = next_listing_page(html, &endpoint, max_pages)? else {
            break;
        };
        endpoint = next;
        if page + 1 < max_pages.min(MAX_QUEUE_PAGES) {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    }
    Ok(messages)
}

pub async fn resolve_queue_messages(
    client: &Client,
    target: QueueTarget,
    max_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    queue_at_base(
        client,
        &target,
        max_pages,
        &Url::parse("https://www.imagepond.net/")?,
    )
    .await
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
    let safe_name = safe_filename(filename);
    Some((url, safe_name))
}

fn parse_page(html: &str) -> Result<(MediaItem, Metadata), Box<dyn Error + Send + Sync>> {
    let kind = property(html, "type").unwrap_or_default();
    let is_image = kind == "image";
    let field = if is_image { "image" } else { "video" };
    let media = property(html, field).ok_or("ImagePond page has no public media OG link")?;
    let (url, filename) =
        valid_media_link(&media).ok_or("ImagePond page media link has an unsupported origin")?;
    let item = MediaItem {
        url: url.to_string(),
        filename,
        extractor: "imagepond",
        post_id: None,
    };
    let mut metadata = media_metadata(&item);
    metadata.insert("subcategory".into(), "file".into());
    metadata.insert(
        "media_type".into(),
        if is_image { "image" } else { "video" }.into(),
    );
    if let Some(title) = property(html, "title").filter(|v| !v.is_empty() && v.len() <= 256) {
        metadata.insert("title".into(), title.into());
    }
    for axis in ["width", "height"] {
        let source = format!("{field}:{axis}");
        let value = property(html, &source)
            .and_then(|value| value.parse::<u32>().ok())
            .filter(|value| (1..=32_768).contains(value));
        if let Some(value) = value {
            metadata.insert(axis.into(), value.into());
        }
    }
    if !is_image {
        if let Some(mime) = property(html, "video:type")
            .filter(|v| matches!(v.as_str(), "video/mp4" | "video/webm" | "video/ogg"))
        {
            metadata.insert("mime".into(), mime.into());
        }
        if let Some(thumbnail) = property(html, "image")
            .and_then(|link| valid_media_link(&link).map(|(url, _)| url.to_string()))
            .filter(|link| {
                link.ends_with(".jpg")
                    || link.ends_with(".jpeg")
                    || link.ends_with(".png")
                    || link.ends_with(".webp")
            })
        {
            metadata.insert("thumbnail".into(), thumbnail.into());
        }
    }
    Ok((item, metadata))
}

async fn resolve_at_base(
    client: &Client,
    target: &Target,
    base: &Url,
) -> Result<(MediaItem, Metadata), Box<dyn Error + Send + Sync>> {
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
    parse_page(std::str::from_utf8(&body)?)
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
    let (item, mut metadata) =
        resolve_at_base(client, &target, &Url::parse("https://www.imagepond.net/")?).await?;
    metadata.insert("file_id".into(), target.id.into());
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
        for path in [
            "i/abc123",
            "img/abc123",
            "image/abc123",
            "video/some-video.abc123",
        ] {
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
    fn album_user_routes_and_deferred_media_queues_are_bounded() {
        assert_eq!(
            match_queue_url("https://imagepond.net/a/gallery123"),
            Some(QueueTarget::Album("gallery123".into()))
        );
        assert_eq!(
            match_queue_url("https://www.imagepond.net/user/public-user"),
            Some(QueueTarget::User("public-user".into()))
        );
        for bad in [
            "http://imagepond.net/a/gallery123",
            "https://imagepond.net/a/gallery123?token=x",
            "https://imagepond.net/user/a%2Fb",
            "https://evilimagepond.net/a/gallery123",
        ] {
            assert!(match_queue_url(bad).is_none(), "{bad}");
        }
        let html = r#"<a href="/i/a1">One</a><a href="/video/b2">Video</a><a href="/i/a1">Duplicate</a><a href="/i/../secret">Bad</a>"#;
        let events = collect_queue_messages(html, &QueueTarget::Album("gallery123".into()));
        assert_eq!(events.len(), 2);
        let a = serde_json::to_value(&events[0]).unwrap();
        let b = serde_json::to_value(&events[1]).unwrap();
        assert_eq!(a["url"], "https://www.imagepond.net/i/a1");
        assert_eq!(b["url"], "https://www.imagepond.net/video/b2");
        assert_eq!(a["metadata"]["album_id"], "gallery123");
        assert_eq!(b["metadata"]["file_id"], "b2");
    }

    #[test]
    fn queue_scanner_never_rewrites_foreign_link_or_plain_text_into_child() {
        let html = r#"<a href="https://malicious.example/i/fake123">External</a><script>let example="/video/ghost123";</script><a href="/i/good123">Valid</a><a href="//evil.example/video/bad123">Not ours</a><a href="/i/../hidden">Traversal</a><a href="/img/picture123?token=secret">Token</a><a href="https://imagepond.net/video/real456">Valid video</a>"#;
        let events = collect_queue_messages(html, &QueueTarget::User("alice".into()));
        let urls: Vec<_> = events
            .iter()
            .map(|event| {
                serde_json::to_value(event).unwrap()["url"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        assert_eq!(
            urls,
            [
                "https://www.imagepond.net/i/good123",
                "https://www.imagepond.net/video/real456"
            ]
        );
    }

    #[tokio::test]
    async fn local_first_page_user_listing_emits_safe_deferred_queues() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 4096];
            let n = stream.read(&mut request).await.unwrap();
            assert!(String::from_utf8_lossy(&request[..n]).starts_with("GET /user/alice HTTP/1.1"));
            let html = r#"<a href="/i/one123">Image</a><a href="/video/two456">Video</a>"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
                html.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let events = queue_at_base(&Client::new(), &QueueTarget::User("alice".into()), 3, &base)
            .await
            .unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(
            serde_json::to_value(&events[0]).unwrap()["metadata"]["username"],
            "alice"
        );
        server.await.unwrap();
    }

    #[test]
    fn rejects_foreign_and_nonprogressing_listing_continuations() {
        let current = Url::parse("https://www.imagepond.net/a/gallery123").unwrap();
        for bad in [
            r#"<a href="https://untrusted.example/a/gallery123?page=2" rel="next">"#,
            r#"<a href="/a/other?page=2" rel="next">"#,
            r#"<a href="/a/gallery123?page=1" rel="next">"#,
            r#"<a href="/a/gallery123?page=2&token=secret" rel="next">"#,
        ] {
            assert!(next_listing_page(bad, &current, 4).is_err(), "{bad}");
        }
        let good = r#"<a href="?page=2" rel="next">Next</a>"#;
        let second = next_listing_page(good, &current, 4).unwrap().unwrap();
        assert_eq!(
            second.as_str(),
            "https://www.imagepond.net/a/gallery123?page=2"
        );
        assert!(next_listing_page(good, &current, 1).unwrap().is_none());
    }

    #[tokio::test]
    async fn album_pagination_preserves_first_seen_media_and_caps_pages() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (page, body) in [
                (
                    1,
                    r#"<a href="/i/one123">Image</a><a href="?page=2" rel="next">Next</a>"#,
                ),
                (
                    2,
                    r#"<a href="/i/one123">Duplicate</a><a href="/video/two456">Video</a>"#,
                ),
            ] {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut buffer = [0_u8; 4096];
                let n = stream.read(&mut buffer).await.unwrap();
                let request = String::from_utf8_lossy(&buffer[..n]);
                assert!(request.starts_with("GET /a/gallery123"), "{request}");
                if page == 2 {
                    assert!(request.contains("page=2"), "{request}");
                }
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let events = queue_at_base(
            &Client::new(),
            &QueueTarget::Album("gallery123".into()),
            3,
            &base,
        )
        .await
        .unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(
            serde_json::to_value(&events[0]).unwrap()["url"],
            "https://www.imagepond.net/i/one123"
        );
        assert_eq!(
            serde_json::to_value(&events[1]).unwrap()["url"],
            "https://www.imagepond.net/video/two456"
        );
        server.await.unwrap();
    }

    #[test]
    fn rejects_untrusted_og_media_and_keeps_safe_filename() {
        let evil = r#"<meta property="og:type" content="image"><meta property="og:image" content="https://evil.example/p.jpg">"#;
        assert!(parse_page(evil).is_err());
        let good = r#"<meta property="og:type" content="image"><meta property="og:image" content="https://cdn.imagepond.net/media/public.jpg"><meta property="og:title" content="Public image">"#;
        let (file, title) = parse_page(good).unwrap();
        assert_eq!(file.url, "https://cdn.imagepond.net/media/public.jpg");
        assert_eq!(file.filename, "public.jpg");
        assert_eq!(file.post_id, None);
        assert_eq!(
            title.get("title").and_then(serde_json::Value::as_str),
            Some("Public image")
        );
    }

    #[test]
    fn public_video_og_link_is_preferred_over_thumbnail() {
        let html = r#"<meta property="og:type" content="video"><meta property="og:video" content="https://cdn.imagepond.net/media/clip.mp4"><meta property="og:image" content="https://cdn.imagepond.net/thumb/clip.jpg"><meta property="og:title" content="Public clip">"#;
        let (media, title) = parse_page(html).unwrap();
        assert_eq!(media.url, "https://cdn.imagepond.net/media/clip.mp4");
        assert_eq!(media.filename, "clip.mp4");
        assert_eq!(media.extractor, "imagepond");
        assert_eq!(
            title.get("title").and_then(serde_json::Value::as_str),
            Some("Public clip")
        );
        for bad in [
            "http://cdn.imagepond.net/media/clip.mp4",
            "https://cdn.imagepond.net/media/clip.mp4?token=secret",
            "https://user:password@cdn.imagepond.net/media/clip.mp4",
            "https://evilimagepond.net/media/clip.mp4",
            "https://cdn.imagepond.net/media/clip.exe",
        ] {
            assert!(valid_media_link(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn typed_og_details_preserve_numeric_dimensions_and_safe_mime_thumbnail() {
        let page = r#"<meta property="og:type" content="video"><meta property="og:video" content="https://cdn.imagepond.net/media/clip.mp4"><meta property="og:video:width" content="1920"><meta property="og:video:height" content="1080"><meta property="og:video:type" content="video/mp4"><meta property="og:image" content="https://cdn.imagepond.net/thumb/clip.jpg">"#;
        let (file, meta) = parse_page(page).unwrap();
        assert_eq!(file.filename, "clip.mp4");
        assert_eq!(meta["media_type"], "video");
        assert_eq!(meta["width"], 1920);
        assert_eq!(meta["height"], 1080);
        assert_eq!(meta["mime"], "video/mp4");
        assert_eq!(
            meta["thumbnail"],
            "https://cdn.imagepond.net/thumb/clip.jpg"
        );
    }

    #[test]
    fn typed_og_details_drop_overflow_dimensions_and_untrusted_thumbnail() {
        let page = r#"<meta property="og:type" content="video"><meta property="og:video" content="https://cdn.imagepond.net/media/clip.webm"><meta property="og:video:width" content="99999999999"><meta property="og:video:height" content="-22"><meta property="og:video:type" content="application/octet-stream"><meta property="og:image" content="https://evil.example/thumb.jpg">"#;
        let (_, meta) = parse_page(page).unwrap();
        for key in ["width", "height", "mime", "thumbnail"] {
            assert!(meta.get(key).is_none(), "{key}");
        }
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
        let (file, _) = resolve_at_base(&Client::new(), &target, &base)
            .await
            .unwrap();
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
