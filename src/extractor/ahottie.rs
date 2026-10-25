//! Guarded public Ahottie galleries, tags and searches from pinned ahottie.py.
use crate::{
    message::{media_metadata, queued_url, ExtractorMessage, Metadata},
    MediaItem,
};
use reqwest::Client;
use std::{collections::HashSet, error::Error, time::Duration};
use url::Url;

const MAX_HTML: usize = 2 * 1024 * 1024;
const MAX_PAGES: u16 = 4;
const MAX_MEDIA: usize = 160;
const MAX_CHILDREN: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Gallery(String),
    Tag { tag: String, start: u16 },
    Search { query: String, start: u16 },
}

fn safe_slug(s: &str) -> bool {
    (1..=100).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub fn match_url(raw: &str) -> Option<Target> {
    let candidate = if raw.starts_with("ahottie.top/") || raw.starts_with("www.ahottie.top/") {
        format!("https://{raw}")
    } else {
        raw.to_owned()
    };
    let low = candidate.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "/../", "/./"]
        .iter()
        .any(|bad| low.contains(bad))
    {
        return None;
    }
    let u = Url::parse(&candidate).ok()?;
    if !matches!(u.scheme(), "https" | "http")
        || !matches!(u.host_str(), Some("ahottie.top" | "www.ahottie.top"))
        || u.port().is_some()
        || !u.username().is_empty()
        || u.password().is_some()
        || u.fragment().is_some()
    {
        return None;
    }
    let parts: Vec<_> = u.path_segments()?.collect();
    match parts.as_slice() {
        ["albums", id] | ["albums", id, ""] if safe_slug(id) && u.query().is_none() => {
            Some(Target::Gallery((*id).into()))
        }
        ["tags", tag] | ["tags", tag, ""] if safe_slug(tag) => {
            let page = optional_page(&u)?;
            Some(Target::Tag {
                tag: (*tag).into(),
                start: page,
            })
        }
        ["search"] | ["search", ""] => {
            let mut query = None;
            let mut page = 1;
            let mut page_seen = false;
            for (name, value) in u.query_pairs() {
                match name.as_ref() {
                    "kw" if query.is_none()
                        && !value.is_empty()
                        && value.len() <= 150
                        && !value.chars().any(char::is_control) =>
                    {
                        query = Some(value.into_owned())
                    }
                    "page" if !page_seen => {
                        page_seen = true;
                        page = value.parse().ok()?;
                    }
                    _ => return None,
                }
            }
            (1..=MAX_PAGES).contains(&page).then_some(Target::Search {
                query: query?,
                start: page,
            })
        }
        _ => None,
    }
}

fn optional_page(url: &Url) -> Option<u16> {
    let mut page = 1;
    if url.query().is_some() {
        let params: Vec<_> = url.query_pairs().collect();
        if params.len() != 1 || params[0].0 != "page" {
            return None;
        }
        page = params[0].1.parse().ok()?;
    }
    (1..=MAX_PAGES).contains(&page).then_some(page)
}

async fn read_html(client: &Client, url: Url) -> Result<String, Box<dyn Error + Send + Sync>> {
    let mut response = client.get(url.clone()).send().await?.error_for_status()?;
    if response.url().origin() != url.origin() {
        return Err("Ahottie page redirected outside origin".into());
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_HTML as u64)
    {
        return Err("Ahottie public HTML exceeds byte cap".into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_HTML.saturating_sub(body.len()) {
            return Err("Ahottie public HTML exceeds byte cap".into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8(body)?)
}

fn between<'a>(s: &'a str, left: &str, right: &str) -> Option<&'a str> {
    s.split_once(left)?.1.split_once(right).map(|(v, _)| v)
}

fn safe_image(raw: &str) -> Option<(String, String)> {
    let low = raw.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "/../", "/./"]
        .iter()
        .any(|bad| low.contains(bad))
    {
        return None;
    }
    let u = Url::parse(raw).ok()?;
    let host = u.host_str()?;
    if u.scheme() != "https"
        || !(host == "ahottie.top" || host.ends_with(".ahottie.top"))
        || u.port().is_some()
        || !u.username().is_empty()
        || u.password().is_some()
        || u.query().is_some()
        || u.fragment().is_some()
    {
        return None;
    }
    let filename = u.path_segments()?.next_back()?;
    let ext = filename.rsplit_once('.')?.1.to_ascii_lowercase();
    if !matches!(
        ext.as_str(),
        "jpg" | "jpeg" | "png" | "webp" | "avif" | "gif"
    ) {
        return None;
    }
    Some((u.to_string(), ext))
}

fn gallery_page(
    html: &str,
    id: &str,
    meta: &mut Metadata,
    seen: &mut HashSet<String>,
    output: &mut Vec<(MediaItem, Metadata)>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    if !meta.contains_key("gallery_id") {
        meta.insert("gallery_id".into(), id.into());
        meta.insert("subcategory".into(), "gallery".into());
        if let Some(title) = between(html, "<title>", "<")
            .and_then(|s| s.split_once(" | ").map(|(v, _)| v).or(Some(s)))
            .filter(|s| s.len() <= 240 && !s.chars().any(char::is_control))
        {
            meta.insert("title".into(), title.replace("&amp;", "&").into());
        }
        if let Some(date) = between(html, "datetime=\"", "\"")
            .filter(|s| s.len() <= 64 && !s.chars().any(char::is_control))
        {
            meta.insert("date_source".into(), date.into());
        }
    }
    let section = html
        .split_once("<time ")
        .map(|(_, rest)| rest)
        .unwrap_or(html);
    for block in section.split("\" src=\"").skip(1).take(2000) {
        let Some(url_raw) = block.split_once('"').map(|(u, _)| u) else {
            continue;
        };
        let Some((url, ext)) = safe_image(url_raw) else {
            continue;
        };
        if !seen.insert(url.clone()) {
            continue;
        }
        if output.len() >= MAX_MEDIA {
            return Err("Ahottie gallery exceeds image count".into());
        }
        let num = output.len() + 1;
        let item = MediaItem {
            url,
            filename: format!("{num:03}.{ext}"),
            extractor: "ahottie",
            post_id: None,
        };
        let mut fields = media_metadata(&item);
        for (key, v) in meta.iter() {
            fields.insert(key.clone(), v.clone());
        }
        fields.insert("num".into(), num.into());
        fields.insert(
            "post_url".into(),
            format!("https://ahottie.top/albums/{id}").into(),
        );
        output.push((item, fields));
    }
    Ok(())
}

fn next_link(html: &str, base: &Url) -> Option<Url> {
    for anchor in html.split("<a ").skip(1).take(4000) {
        let (attrs, _) = anchor.split_once('>')?;
        if !attrs.contains("rel=\"next\"") {
            continue;
        }
        let href = between(attrs, "href=\"", "\"")?;
        let url = base.join(href).ok()?;
        if url.origin() == base.origin()
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none()
        {
            return Some(url);
        }
    }
    None
}

fn queue_gallery_links(
    html: &str,
    seen: &mut HashSet<String>,
    out: &mut Vec<ExtractorMessage>,
    source: &str,
) {
    for chunk in html.split("<div class=\"relative\">").skip(1).take(3200) {
        if out.len() >= MAX_CHILDREN {
            break;
        }
        let Some(section) = chunk.split_once("</div>").map(|(s, _)| s) else {
            continue;
        };
        let Some(link) = between(section, " href=\"", "\"") else {
            continue;
        };
        let Ok(url) = Url::parse("https://ahottie.top/").and_then(|base| base.join(link)) else {
            continue;
        };
        if !matches!(match_url(url.as_str()), Some(Target::Gallery(_))) {
            continue;
        }
        let Some(id) = url
            .path_segments()
            .and_then(|mut p| p.nth(1))
            .map(ToOwned::to_owned)
        else {
            continue;
        };
        let canonical = format!("https://ahottie.top/albums/{id}");
        if !seen.insert(canonical.clone()) {
            continue;
        }
        let mut meta = Metadata::new();
        meta.insert("category".into(), "ahottie".into());
        meta.insert("subcategory".into(), source.into());
        meta.insert("gallery_id".into(), id.into());
        if let Some(title) = between(section, " alt=\"", "\"")
            .filter(|t| t.len() <= 240 && !t.chars().any(char::is_control))
        {
            meta.insert("title".into(), title.replace("&amp;", "&").into());
        }
        if let Some(date) = between(section, " datetime=\"", "\"")
            .filter(|t| t.len() <= 64 && !t.chars().any(char::is_control))
        {
            meta.insert("date_source".into(), date.into());
        }
        out.push(queued_url(canonical, meta));
    }
}

async fn gallery_at(
    client: &Client,
    base: &Url,
    id: &str,
    max_pages: u16,
) -> Result<Vec<(MediaItem, Metadata)>, Box<dyn Error + Send + Sync>> {
    let mut page = base.join(&format!("albums/{id}"))?;
    let mut visited = HashSet::new();
    let mut seen = HashSet::new();
    let mut meta = Metadata::new();
    let mut out = Vec::new();
    for count in 0..max_pages.clamp(1, MAX_PAGES) {
        if !visited.insert(page.to_string()) {
            break;
        }
        if count > 0 {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        let html = read_html(client, page.clone()).await?;
        gallery_page(&html, id, &mut meta, &mut seen, &mut out)?;
        let Some(next) = next_link(&html, &page) else {
            break;
        };
        let scoped = format!("/albums/{id}");
        if next.path() != scoped && next.path() != format!("{scoped}/") {
            return Err("Ahottie gallery pagination left its exact album".into());
        }
        page = next;
    }
    Ok(out)
}

async fn listing_at(
    client: &Client,
    base: &Url,
    target: Target,
    max_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let (path, source, start) = match target {
        Target::Tag { tag, start } => (format!("tags/{tag}"), "tag", start),
        Target::Search { query, start } => (
            format!(
                "search?kw={}",
                url::form_urlencoded::byte_serialize(query.as_bytes()).collect::<String>()
            ),
            "search",
            start,
        ),
        Target::Gallery(_) => return Err("Expected Ahottie listing".into()),
    };
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for page in start
        ..start
            .saturating_add(max_pages.clamp(1, MAX_PAGES))
            .min(MAX_PAGES + 1)
    {
        if page > start {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        let mut url = base.join(&path)?;
        url.query_pairs_mut().append_pair("page", &page.to_string());
        let html = read_html(client, url).await?;
        queue_gallery_links(&html, &mut seen, &mut out, source);
        if out.len() >= MAX_CHILDREN || !html.contains("rel=\"next\"") {
            break;
        }
    }
    Ok(out)
}

pub async fn resolve(
    client: &Client,
    target: Target,
    max_pages: u16,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse("https://ahottie.top/")?;
    match target {
        Target::Gallery(id) => Ok(gallery_at(client, &base, &id, max_pages)
            .await?
            .into_iter()
            .map(|(m, _)| m)
            .collect()),
        _ => Err("Ahottie tag/search listings require --dump-messages or --follow-queues".into()),
    }
}

pub async fn resolve_messages(
    client: &Client,
    target: Target,
    max_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse("https://ahottie.top/")?;
    match target {
        Target::Gallery(id) => {
            let mut out = Vec::new();
            for (item, metadata) in gallery_at(client, &base, &id, max_pages).await? {
                out.push(ExtractorMessage::Directory {
                    metadata: metadata.clone(),
                });
                out.push(ExtractorMessage::Url {
                    url: item.url,
                    metadata,
                });
            }
            Ok(out)
        }
        t => listing_at(client, &base, t, max_pages).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    #[test]
    fn pinned_public_album_tag_and_search_routes() {
        assert_eq!(
            match_url("https://ahottie.top/albums/Album12"),
            Some(Target::Gallery("Album12".into()))
        );
        assert_eq!(
            match_url("https://www.ahottie.top/tags/landscape"),
            Some(Target::Tag {
                tag: "landscape".into(),
                start: 1
            })
        );
        assert_eq!(
            match_url("https://ahottie.top/search?kw=landscape"),
            Some(Target::Search {
                query: "landscape".into(),
                start: 1
            })
        );
        for bad in [
            "https://fakeahottie.top/albums/Album12",
            "https://ahottie.top/albums/../private",
            "https://ahottie.top/search?kw=x&password=1",
            "https://ahottie.top/albums/Album12?x=1",
            "https://user:pass@ahottie.top/albums/Album12",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn public_gallery_preserves_image_order_and_source_metadata() {
        let html = r#"<title>Landscape | Ahottie</title><time datetime="2025-09-10T10:00:00Z"></time><div>" src="https://images.ahottie.top/1.jpg">" src="https://images.ahottie.top/2.png">" src="https://evil-ahottie.top/fake.jpg"></div>"#;
        let mut out = Vec::new();
        gallery_page(
            html,
            "Gallery1",
            &mut Metadata::new(),
            &mut HashSet::new(),
            &mut out,
        )
        .unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].0.filename, "001.jpg");
        assert_eq!(out[1].1["gallery_id"], "Gallery1");
        assert_eq!(out[0].1["title"], "Landscape");
        assert_eq!(out[0].1["date_source"], "2025-09-10T10:00:00Z");
    }

    #[test]
    fn album_queues_dedup_first_party_links() {
        let html = r#"<div class="relative"><a href="/albums/One" alt="One" datetime="2025-01-01"></a></div><div class="relative"><a href="/albums/One"></a></div><div class="relative"><a href="https://evil-ahottie.top/albums/Bad"></a></div><div class="relative"><a href="/albums/Two"></a></div>"#;
        let mut out = Vec::new();
        queue_gallery_links(html, &mut HashSet::new(), &mut out, "tag");
        assert_eq!(out.len(), 2);
        assert_eq!(
            serde_json::to_value(&out[0]).unwrap()["url"],
            "https://ahottie.top/albums/One"
        );
    }

    #[tokio::test]
    async fn gallery_continuation_rejects_prefix_matched_foreign_albums() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 4096];
            let size = socket.read(&mut request).await.unwrap();
            assert!(
                String::from_utf8_lossy(&request[..size]).starts_with("GET /albums/One HTTP/1.1")
            );
            let html = r#"<time datetime="2025-01-01"></time><a href="/albums/OneTwo?page=2" rel="next">Next</a>"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
                html.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        let base = Url::parse(&format!("http://{address}/")).unwrap();
        let result = gallery_at(&Client::new(), &base, "One", 2).await;
        assert!(result.is_err());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn public_album_and_tag_queue_use_bounded_loopback_html() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (route, body) in [
                (
                    "/albums/One",
                    r#"<title>One | Ahottie</title><time datetime="2025-01-01"></time><div>" src="https://images.ahottie.top/one.jpg"></div>"#,
                ),
                (
                    "/tags/landscape?page=1",
                    r#"<div class="relative"><a href="/albums/One"></a></div>"#,
                ),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut data = [0u8; 4096];
                let size = socket.read(&mut data).await.unwrap();
                assert!(String::from_utf8_lossy(&data[..size])
                    .starts_with(&format!("GET {route} HTTP/1.1")));
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        assert_eq!(
            gallery_at(&Client::new(), &base, "One", 4)
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            listing_at(
                &Client::new(),
                &base,
                Target::Tag {
                    tag: "landscape".into(),
                    start: 1
                },
                4
            )
            .await
            .unwrap()
            .len(),
            1
        );
        server.await.unwrap();
    }
}
