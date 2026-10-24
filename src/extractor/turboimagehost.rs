//! Pinned Turboimagehost public single-image preview (imagehosts.py).
use crate::{
    message::{media_metadata, queued_url, ExtractorMessage, Metadata},
    safe_filename, MediaItem,
};
use reqwest::Client;
use std::{collections::HashSet, error::Error, time::Duration};
use url::Url;

const MAX_HTML_BYTES: usize = 2 * 1024 * 1024;
const MAX_PAGES: u16 = 4;
const MAX_CHILDREN: usize = 160;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    id: String,
    path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Album {
    id: String,
    path: String,
}

pub fn match_album_url(input: &str) -> Option<Album> {
    let lower = input.to_ascii_lowercase();
    if lower.contains("/../") || lower.contains("/./") || lower.contains("/%2e")
        || lower.contains("%2f") || lower.contains("%5c")
    {
        return None;
    }
    let url = Url::parse(input).ok()?;
    if url.scheme() != "https"
        || !matches!(url.host_str(), Some("turboimagehost.com" | "www.turboimagehost.com"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let parts: Vec<_> = url.path_segments()?.collect();
    let ["album", id, name] = parts.as_slice() else { return None };
    if !(1..=24).contains(&id.len())
        || !id.bytes().all(|b| b.is_ascii_digit())
        || name.is_empty() || name.len() > 180
        || !name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
    {
        return None;
    }
    Some(Album { id: (*id).into(), path: format!("album/{id}/{name}") })
}

pub fn match_url(input: &str) -> Option<Target> {
    let raw = input.to_ascii_lowercase();
    if raw.contains("/../") || raw.contains("/./") || raw.contains("/%2e")
        || raw.contains("%2f") || raw.contains("%5c")
    {
        return None;
    }
    let url = Url::parse(input).ok()?;
    if url.scheme() != "https"
        || !matches!(url.host_str(), Some("turboimagehost.com" | "www.turboimagehost.com"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let parts: Vec<_> = url.path_segments()?.collect();
    let ["p", id, file] = parts.as_slice() else {
        return None;
    };
    if !(1..=24).contains(&id.len()) || !id.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let name = file.strip_suffix(".html")?;
    if name.is_empty() || name.len() > 180
        || !name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return None;
    }
    Some(Target {
        id: (*id).to_owned(),
        path: format!("p/{id}/{file}"),
    })
}

fn trusted_media(raw: &str) -> Option<(Url, String)> {
    let url = Url::parse(raw).ok()?;
    let host = url.host_str()?;
    if url.scheme() != "https"
        || !(host == "turboimagehost.com" || host.ends_with(".turboimagehost.com"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let name = url.path_segments()?.next_back()?.to_owned();
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    if name.is_empty() || name.len() > 240 || name.contains('%')
        || !matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif")
    {
        return None;
    }
    let filename = safe_filename(&name);
    Some((url, filename))
}

fn parse_page(html: &str, target: &Target) -> Result<(MediaItem, Metadata), Box<dyn Error + Send + Sync>> {
    let tag = html.split_once("<img ").map(|(_, v)| v)
        .and_then(|v| v.split_once('>').map(|(tag, _)| tag))
        .ok_or("Turboimagehost public image is absent")?;
    let link = tag.split_once("src=\"")
        .and_then(|(_, v)| v.split_once('"').map(|(url, _)| url))
        .ok_or("Turboimagehost image has no src")?;
    let (url, filename) = trusted_media(link)
        .ok_or("Turboimagehost media origin is unsupported")?;
    let item = MediaItem {
        url: url.to_string(), filename, extractor: "turboimagehost", post_id: None,
    };
    let mut metadata = media_metadata(&item);
    metadata.insert("subcategory".into(), "image".into());
    metadata.insert("token".into(), target.id.clone().into());
    metadata.insert(
        "post_url".into(),
        format!("https://www.turboimagehost.com/{}", target.path).into(),
    );
    Ok((item, metadata))
}

async fn read_html(
    client: &Client,
    url: Url,
) -> Result<String, Box<dyn Error + Send + Sync>> {
    let mut response = client.get(url).send().await?.error_for_status()?;
    if response.content_length().is_some_and(|n| n > MAX_HTML_BYTES as u64) {
        return Err("Turboimagehost public HTML exceeds response cap".into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_HTML_BYTES.saturating_sub(body.len()) {
            return Err("Turboimagehost public HTML exceeds response cap".into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8(body)?)
}

async fn at_base(
    client: &Client, target: &Target, base: &Url,
) -> Result<(MediaItem, Metadata), Box<dyn Error + Send + Sync>> {
    parse_page(&read_html(client, base.join(&target.path)?).await?, target)
}

fn album_links(
    html: &str,
    album_id: &str,
    seen: &mut HashSet<String>,
    out: &mut Vec<ExtractorMessage>,
) -> usize {
    let origin = Url::parse("https://www.turboimagehost.com/").expect("constant root");
    let before = out.len();
    for (_, after) in html.match_indices("<a ") {
        let Some((attrs, _)) = after["<a ".len()..].split_once('>') else { continue };
        let Some((prefix, rest)) = attrs.split_once("href=\"") else { continue };
        if !prefix.is_empty() && !prefix.ends_with(char::is_whitespace) { continue; }
        let Some((href, _)) = rest.split_once('"') else { continue };
        if href.len() > 2048 { continue; }
        let Ok(url) = origin.join(href) else { continue };
        let Some(child) = match_url(url.as_str()) else { continue };
        let canonical = format!("https://www.turboimagehost.com/{}", child.path);
        if !seen.insert(canonical.clone()) { continue; }
        let mut metadata = Metadata::new();
        metadata.insert("category".into(), "turboimagehost".into());
        metadata.insert("subcategory".into(), "image".into());
        metadata.insert("album_id".into(), album_id.into());
        metadata.insert("token".into(), child.id.into());
        out.push(queued_url(canonical, metadata));
        if out.len() >= MAX_CHILDREN { break; }
    }
    out.len() - before
}

async fn album_at_base(
    client: &Client, album: &Album, base: &Url, max_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for page in 1..=max_pages.clamp(1, MAX_PAGES) {
        let mut endpoint = base.join(&album.path)?;
        endpoint.query_pairs_mut().append_pair("p", &page.to_string());
        let html = read_html(client, endpoint).await?;
        if page == 1 && html.contains("Requested gallery don") {
            return Err("Turboimagehost public gallery not found".into());
        }
        if album_links(&html, &album.id, &mut seen, &mut result) == 0
            || result.len() >= MAX_CHILDREN
        {
            break;
        }
        if page < max_pages.min(MAX_PAGES) {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
    Ok(result)
}

pub async fn resolve_album_messages(
    client: &Client, album: Album, max_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    album_at_base(client, &album, &Url::parse("https://www.turboimagehost.com/")?, max_pages).await
}


pub async fn resolve(
    client: &Client, target: Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let (item, _) = at_base(
        client, &target, &Url::parse("https://www.turboimagehost.com/")?,
    ).await?;
    Ok(vec![item])
}

pub async fn resolve_messages(
    client: &Client, target: Target,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let (item, metadata) = at_base(
        client, &target, &Url::parse("https://www.turboimagehost.com/")?,
    ).await?;
    Ok(vec![
        ExtractorMessage::Directory { metadata: metadata.clone() },
        ExtractorMessage::Url { url: item.url, metadata },
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::TcpListener};

    #[test]
    fn matches_only_public_image_url_shapes() {
        assert_eq!(
            match_url("https://www.turboimagehost.com/p/39078423/test--.png.html"),
            Some(Target {
                id: "39078423".into(),
                path: "p/39078423/test--.png.html".into()
            })
        );
        for bad in [
            "http://turboimagehost.com/p/39078423/file.jpg.html",
            "https://evilturboimagehost.com/p/39078423/file.jpg.html",
            "https://turboimagehost.com/p/not-digits/file.jpg.html",
            "https://turboimagehost.com/p/39078423/file.jpg.html?token=x",
            "https://turboimagehost.com/album/344597/example",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn album_urls_and_deferred_children_are_strict_and_ordered() {
        let target = match_album_url("https://www.turboimagehost.com/album/344597/testimagegallery").unwrap();
        assert_eq!(target.id, "344597");
        for bad in [
            "http://www.turboimagehost.com/album/344597/testimagegallery",
            "https://evilturboimagehost.com/album/344597/gallery",
            "https://www.turboimagehost.com/album/344597/test?token=x",
            "https://www.turboimagehost.com/album/bad/gallery",
        ] {
            assert!(match_album_url(bad).is_none(), "{bad}");
        }
        let html = r#"<a href="/p/1/one.jpg.html">First</a><a href="/p/1/one.jpg.html">Duplicate</a><a href="https://www.turboimagehost.com/p/2/two.png.html">Second</a><a data-href="/p/3/forged.jpg.html">No</a><script>const x='href="/p/4/script.png.html"';</script><a href="https://evil.example/p/5/evil.jpg.html">Bad</a>"#;
        let mut seen = HashSet::new();
        let mut queue = Vec::new();
        assert_eq!(album_links(html, "344597", &mut seen, &mut queue), 2);
        assert_eq!(serde_json::to_value(&queue[0]).unwrap()["url"], "https://www.turboimagehost.com/p/1/one.jpg.html");
        assert_eq!(serde_json::to_value(&queue[1]).unwrap()["metadata"]["album_id"], "344597");
    }

    #[tokio::test]
    async fn bounded_album_pages_deduplicate_and_stop_on_empty_page() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (page, html) in [
                (1, r#"<a href="/p/1/one.jpg.html">One</a>"#),
                (2, r#"<a href="/p/1/one.jpg.html">Dup</a><a href="/p/2/two.png.html">Two</a>"#),
                (3, "<div>Empty</div>"),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buf = [0_u8; 4096];
                let len = socket.read(&mut buf).await.unwrap();
                let request = String::from_utf8_lossy(&buf[..len]);
                assert!(request.starts_with("GET /album/344597/gallery?p="), "{request}");
                assert!(request.contains(&format!("p={page}")), "{request}");
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
                    html.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let album = match_album_url("https://www.turboimagehost.com/album/344597/gallery").unwrap();
        let events = album_at_base(&Client::new(), &album, &base, 4).await.unwrap();
        assert_eq!(events.len(), 2);
        server.await.unwrap();
    }

    #[test]
    fn first_public_image_uses_trusted_media_and_real_id() {
        let target = match_url("https://turboimagehost.com/p/39078423/test--.png.html").unwrap();
        let html = r#"<img src="https://i1.turboimagehost.com/files/photo.png">"#;
        let (item, metadata) = parse_page(html, &target).unwrap();
        assert_eq!(item.filename, "photo.png");
        assert_eq!(metadata["token"], "39078423");
        assert_eq!(metadata["post_url"], "https://www.turboimagehost.com/p/39078423/test--.png.html");
        for bad in [
            "http://i1.turboimagehost.com/files/photo.jpg",
            "https://evil.example/photo.jpg",
            "https://evilturboimagehost.com/photo.jpg",
            "https://i1.turboimagehost.com/photo.jpg?token=x",
        ] {
            assert!(trusted_media(bad).is_none(), "{bad}");
        }
    }

    #[tokio::test]
    async fn public_image_fetches_one_bounded_html_response() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 4096];
            let n = socket.read(&mut request).await.unwrap();
            assert!(String::from_utf8_lossy(&request[..n])
                .starts_with("GET /p/39078423/test--.png.html HTTP/1.1"));
            let html = r#"<img src="https://img.turboimagehost.com/files/photo.png">"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
                html.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        let base = Url::parse(&format!("http://{address}/")).unwrap();
        let target = match_url("https://www.turboimagehost.com/p/39078423/test--.png.html").unwrap();
        let (item, _) = at_base(&Client::new(), &target, &base).await.unwrap();
        assert_eq!(item.filename, "photo.png");
        server.await.unwrap();
    }
}
