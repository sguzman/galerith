//! Conservative public Imagetwist/Imagehaha original-image subset of imagehosts.py.
use crate::{
    message::{media_metadata, queued_url, ExtractorMessage, Metadata},
    safe_filename, MediaItem,
};
use reqwest::Client;
use std::{collections::HashSet, error::Error};
use url::Url;

const MAX_HTML_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    token: String,
    host: String,
}

fn valid_host(host: &str) -> bool {
    matches!(
        host,
        "imagetwist.com"
            | "www.imagetwist.com"
            | "phun.imagetwist.com"
            | "imagehaha.com"
            | "www.imagehaha.com"
            | "phun.imagehaha.com"
    )
}

fn valid_token(value: &str) -> bool {
    value.len() == 12
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

pub fn match_url(input: &str) -> Option<Image> {
    let lower = input.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "/../", "/./"]
        .iter()
        .any(|pattern| lower.contains(pattern))
    {
        return None;
    }
    let parsed = Url::parse(input).ok()?;
    let host = parsed.host_str()?;
    if parsed.scheme() != "https"
        || !valid_host(host)
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.port().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return None;
    }
    let parts: Vec<_> = parsed.path_segments()?.collect();
    let token = match parts.as_slice() {
        [token] if valid_token(token) => *token,
        [token, filename] if valid_token(token) && safe_hint(filename) => *token,
        _ => return None,
    };
    Some(Image {
        token: token.to_owned(),
        host: host.to_owned(),
    })
}

fn safe_hint(value: &str) -> bool {
    (5..=220).contains(&value.len())
        && !value.contains("..")
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
        && value.rsplit_once('.').is_some_and(|(_, extension)| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif"
            )
        })
}

fn media_url(raw: &str) -> Option<(Url, String)> {
    let url = Url::parse(raw).ok()?;
    let host = url.host_str()?;
    if url.scheme() != "https"
        || !(host == "imagetwist.com"
            || host.ends_with(".imagetwist.com")
            || host == "imagehaha.com"
            || host.ends_with(".imagehaha.com"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path().starts_with("/imgs/")
    {
        return None;
    }
    let filename = url.path_segments()?.next_back()?;
    let ext = filename.rsplit_once('.')?.1.to_ascii_lowercase();
    if filename.is_empty()
        || filename.len() > 240
        || filename.contains('%')
        || !matches!(
            ext.as_str(),
            "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif"
        )
    {
        return None;
    }
    let name = safe_filename(filename);
    Some((url, name))
}

fn alt_filename(alt: &str, ext: &str) -> Option<String> {
    let decoded = alt
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    if decoded.is_empty()
        || decoded.len() > 225
        || decoded.contains('/')
        || decoded.contains('\\')
        || decoded.chars().any(char::is_control)
    {
        return None;
    }
    let name = if decoded.rsplit_once('.').is_some_and(|(_, extension)| {
        matches!(
            extension.to_ascii_lowercase().as_str(),
            "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif"
        )
    }) {
        decoded
    } else {
        format!("{decoded}.{ext}")
    };
    Some(safe_filename(&name))
}

fn parse_page(
    html: &str,
    image: &Image,
) -> Result<(MediaItem, Metadata), Box<dyn Error + Send + Sync>> {
    let mut original = None;
    for node in html.split("<img").skip(1).take(12) {
        let Some((attrs, _)) = node.split_once('>') else {
            continue;
        };
        let Some(link) = attrs
            .split_once("src=\"")
            .and_then(|(_, s)| s.split_once('"'))
            .map(|(link, _)| link)
        else {
            continue;
        };
        if link.starts_with("/imgs/") {
            return Err("Imagetwist reports an unavailable original image".into());
        }
        if let Some((url, fallback)) = media_url(link) {
            let ext = fallback
                .rsplit_once('.')
                .map(|(_, ext)| ext)
                .unwrap_or("jpg");
            let filename = attrs
                .split_once("alt=\"")
                .and_then(|(_, s)| s.split_once('"'))
                .and_then(|(alt, _)| alt_filename(alt, ext))
                .unwrap_or(fallback);
            original = Some((url, filename));
            break;
        }
    }
    let (url, filename) = original.ok_or("Imagetwist page has no validated original")?;
    let file = MediaItem {
        url: url.to_string(),
        filename,
        extractor: "imagetwist",
        post_id: None,
    };
    let mut metadata = media_metadata(&file);
    metadata.insert("subcategory".into(), "image".into());
    metadata.insert("token".into(), image.token.clone().into());
    metadata.insert(
        "post_url".into(),
        format!("https://{}/{}", image.host, image.token).into(),
    );
    Ok((file, metadata))
}

async fn resolve_at_base(
    client: &Client,
    image: &Image,
    base: &Url,
) -> Result<(MediaItem, Metadata), Box<dyn Error + Send + Sync>> {
    let mut response = client
        .get(base.join(&image.token)?)
        .send()
        .await?
        .error_for_status()?;
    if response
        .content_length()
        .is_some_and(|n| n > MAX_HTML_BYTES as u64)
    {
        return Err("Imagetwist HTML exceeds preview byte bound".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_HTML_BYTES.saturating_sub(bytes.len()) {
            return Err("Imagetwist HTML exceeds preview byte bound".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    parse_page(std::str::from_utf8(&bytes)?, image)
}

pub async fn resolve(
    client: &Client,
    image: Image,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse(&format!("https://{}/", image.host))?;
    let (file, _) = resolve_at_base(client, &image, &base).await?;
    Ok(vec![file])
}

pub async fn resolve_messages(
    client: &Client,
    image: Image,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse(&format!("https://{}/", image.host))?;
    let (file, metadata) = resolve_at_base(client, &image, &base).await?;
    Ok(vec![
        ExtractorMessage::Directory {
            metadata: metadata.clone(),
        },
        ExtractorMessage::Url {
            url: file.url,
            metadata,
        },
    ])
}

const MAX_GALLERY_PAGES: u16 = 4;
const MAX_GALLERY_CHILDREN: usize = 160;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gallery {
    host: String,
    user: String,
    id: String,
    title: Option<String>,
    path: String,
}

fn safe_segment(value: &str) -> bool {
    (1..=80).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

fn gallery_id(value: &str) -> bool {
    (1..=18).contains(&value.len()) && value.bytes().all(|b| b.is_ascii_digit())
}

pub fn match_gallery_url(input: &str) -> Option<Gallery> {
    let lower = input.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "/../", "/./"]
        .iter()
        .any(|p| lower.contains(p))
    {
        return None;
    }
    let url = Url::parse(input).ok()?;
    let host = url.host_str()?;
    if url.scheme() != "https"
        || !valid_host(host)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let parts: Vec<_> = url.path_segments()?.collect();
    let (user, id, title) = match parts.as_slice() {
        ["p", user, id] if safe_segment(user) && gallery_id(id) => (*user, *id, None),
        ["p", user, id, title] if safe_segment(user) && gallery_id(id) && safe_segment(title) => {
            (*user, *id, Some((*title).to_owned()))
        }
        _ => return None,
    };
    Some(Gallery {
        host: host.to_owned(),
        user: user.to_owned(),
        id: id.to_owned(),
        title,
        path: url.path().trim_start_matches('/').to_owned(),
    })
}

fn gallery_title(page: &str, fallback: Option<&str>) -> Option<String> {
    let title = page
        .split_once("page_main_title\">")
        .and_then(|(_, rest)| rest.split_once('<'))
        .map(|(text, _)| text)
        .or(fallback)?;
    if title.is_empty() || title.len() > 256 || title.chars().any(char::is_control) {
        return None;
    }
    let safe = title.replace("&amp;", "&").replace("&quot;", "\"");
    if safe.chars().any(char::is_control) {
        return None;
    }
    Some(safe)
}

fn gallery_child_events(
    html: &str,
    gallery: &Gallery,
    title: Option<&str>,
    seen: &mut HashSet<String>,
    events: &mut Vec<ExtractorMessage>,
) {
    let Some(section) = html.split_once("class=\"gallerys").map(|(_, rest)| rest) else {
        return;
    };
    let section = section
        .split_once("</div")
        .map(|(part, _)| part)
        .unwrap_or(section);
    for part in section.split("<a").skip(1) {
        let Some((attrs, _)) = part.split_once('>') else {
            continue;
        };
        if !attrs.starts_with(char::is_whitespace) {
            continue;
        }
        let Some((prefix, rest)) = attrs.split_once("href=\"") else {
            continue;
        };
        if !prefix.ends_with(char::is_whitespace) {
            continue;
        }
        let Some((raw, _)) = rest.split_once('"') else {
            continue;
        };
        let Ok(link) =
            Url::parse(&format!("https://{}/", gallery.host)).and_then(|root| root.join(raw))
        else {
            continue;
        };
        let Some(image) = match_url(link.as_str()) else {
            continue;
        };
        let child = format!("https://{}/{}", gallery.host, image.token);
        if !seen.insert(child.clone()) {
            continue;
        }
        let mut metadata = Metadata::new();
        metadata.insert("category".into(), "imagetwist".into());
        metadata.insert("subcategory".into(), "image".into());
        metadata.insert("gallery_id".into(), gallery.id.clone().into());
        metadata.insert("username".into(), gallery.user.clone().into());
        if let Some(name) = title {
            metadata.insert("gallery_title".into(), name.into());
        }
        events.push(queued_url(child, metadata));
        if events.len() >= MAX_GALLERY_CHILDREN {
            break;
        }
    }
}

fn next_gallery_url(
    html: &str,
    current: &Url,
    gallery: &Gallery,
    last_page: u16,
    max_pages: u16,
) -> Result<Option<Url>, Box<dyn Error + Send + Sync>> {
    for part in html.split("<a").skip(1) {
        let Some((attrs, content)) = part.split_once('>') else {
            continue;
        };
        if !content.starts_with("&#187;</a>") {
            continue;
        }
        let raw = attrs
            .split_once("href='")
            .and_then(|(_, rest)| rest.split_once('\''))
            .map(|(href, _)| href)
            .ok_or("Imagetwist pagination has no href")?;
        let decoded = raw.replace("&amp;", "&");
        let next = current.join(&decoded)?;
        if next.origin() != current.origin()
            || !matches!(next.path(), "/" | "") && next.path() != current.path()
            || !next.username().is_empty()
            || next.password().is_some()
            || next.fragment().is_some()
        {
            return Err("Imagetwist gallery next page escaped its source".into());
        }
        let pairs: Vec<_> = next.query_pairs().collect();
        if pairs.len() > 5
            || pairs.iter().any(|(key, _)| {
                !matches!(
                    key.as_ref(),
                    "op" | "fld_id" | "page" | "per_page" | "usr_login"
                )
            })
        {
            return Err("Imagetwist gallery next URL has unsupported query fields".into());
        }
        let values = |key: &str| {
            pairs
                .iter()
                .filter(|(k, _)| k == key)
                .map(|(_, v)| v.as_ref())
                .collect::<Vec<_>>()
        };
        let page = values("page");
        if page.len() != 1
            || values("fld_id") != [gallery.id.as_str()]
            || values("op") != ["user_public"]
            || values("usr_login").iter().any(|u| *u != gallery.user)
        {
            return Err("Imagetwist gallery continuation has invalid identity".into());
        }
        let number = page[0].parse::<u16>()?;
        if number <= last_page {
            return Err("Imagetwist gallery page must advance".into());
        }
        if number > max_pages.clamp(1, MAX_GALLERY_PAGES) {
            return Ok(None);
        }
        return Ok(Some(next));
    }
    Ok(None)
}

async fn gallery_at_base(
    client: &Client,
    gallery: &Gallery,
    max_pages: u16,
    base: &Url,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let mut current = base.join(&gallery.path)?;
    let mut events = Vec::new();
    let mut seen = HashSet::new();
    let mut inherited_title = gallery.title.clone();
    let mut last_page = 1u16;
    for page in 0..max_pages.clamp(1, MAX_GALLERY_PAGES) {
        let mut response = client
            .get(current.clone())
            .send()
            .await?
            .error_for_status()?;
        if response
            .content_length()
            .is_some_and(|n| n > MAX_HTML_BYTES as u64)
        {
            return Err("Imagetwist gallery exceeds HTML byte limit".into());
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if chunk.len() > MAX_HTML_BYTES.saturating_sub(bytes.len()) {
                return Err("Imagetwist gallery exceeds HTML byte limit".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        let html = std::str::from_utf8(&bytes)?;
        if let Some(title) = gallery_title(html, inherited_title.as_deref()) {
            inherited_title = Some(title);
        }
        gallery_child_events(
            html,
            gallery,
            inherited_title.as_deref(),
            &mut seen,
            &mut events,
        );
        if events.len() >= MAX_GALLERY_CHILDREN {
            break;
        }
        let Some(next) = next_gallery_url(html, &current, gallery, last_page, max_pages)? else {
            break;
        };
        last_page = next
            .query_pairs()
            .find(|(key, _)| key == "page")
            .and_then(|(_, value)| value.parse::<u16>().ok())
            .ok_or("Invalid Imagetwist page index")?;
        current = next;
        if page + 1 < max_pages.min(MAX_GALLERY_PAGES) {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    }
    Ok(events)
}

pub async fn resolve_gallery_messages(
    client: &Client,
    gallery: Gallery,
    max_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse(&format!("https://{}/", gallery.host))?;
    gallery_at_base(client, &gallery, max_pages, &base).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    #[test]
    fn pinned_image_hosts_and_token_routes() {
        for host in [
            "imagetwist.com",
            "www.imagetwist.com",
            "phun.imagetwist.com",
            "imagehaha.com",
        ] {
            assert!(match_url(&format!("https://{host}/f1i2s4vhvbrq/test.png")).is_some());
        }
        for bad in [
            "http://imagetwist.com/f1i2s4vhvbrq",
            "https://evilimagetwist.com/f1i2s4vhvbrq",
            "https://imagetwist.com/f1i2s4vhvbrq?private=1",
            "https://imagetwist.com/abc/filename.jpg",
            "https://imagetwist.com/f1i2s4vhvbrq/../../tmp",
            "https://imagetwist.com/f1i2s4vhvbrq/file.exe",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn pinned_gallery_routes_and_ordered_child_queue_filter() {
        let gallery = match_gallery_url("https://imagetwist.com/p/gdldev/747223/digits").unwrap();
        assert_eq!(gallery.id, "747223");
        for bad in [
            "http://imagetwist.com/p/gdldev/747223/digits",
            "https://imagetwist.com/p/gdldev/notanumber/title",
            "https://imagetwist.com/p/gdldev/747223/digits?private=1",
            "https://evil.example/p/gdldev/747223/digits",
        ] {
            assert!(match_gallery_url(bad).is_none(), "{bad}");
        }
        let html = r#"<h1 class="page_main_title">digits</h1><div class="gallerys"><a href="/j6eu91sbl9bs">one</a><a href="/j6eu91sbl9bs">duplicate</a><a href="https://evil.example/j6eu91sbl9bs">foreign</a><a href="/vx4oh119izyr">two</a></div>"#;
        let mut seen = HashSet::new();
        let mut events = Vec::new();
        gallery_child_events(html, &gallery, Some("digits"), &mut seen, &mut events);
        assert_eq!(events.len(), 2);
        let item = serde_json::to_value(&events[0]).unwrap();
        assert_eq!(item["url"], "https://imagetwist.com/j6eu91sbl9bs");
        assert_eq!(item["metadata"]["gallery_id"], "747223");
        assert_eq!(item["metadata"]["gallery_title"], "digits");
    }

    #[test]
    fn gallery_next_page_rejects_external_and_nonadvancing_urls() {
        let gallery = match_gallery_url("https://imagetwist.com/p/gdldev/747223/digits").unwrap();
        let current = Url::parse("https://imagetwist.com/p/gdldev/747223/digits").unwrap();
        let good = r#"<a href='/?op=user_public&fld_id=747223&usr_login=gdldev&page=2'>&#187;</a>"#;
        assert_eq!(
            next_gallery_url(good, &current, &gallery, 1, 4)
                .unwrap()
                .unwrap()
                .query_pairs()
                .find(|(key, _)| key == "page")
                .unwrap()
                .1,
            "2"
        );
        for bad in [
            r#"<a href='https://evil.example/?op=user_public&fld_id=747223&page=2'>&#187;</a>"#,
            r#"<a href='/?op=user_public&fld_id=747223&page=1'>&#187;</a>"#,
            r#"<a href='/?op=user_public&fld_id=1&page=2'>&#187;</a>"#,
            r#"<a href='/?op=user_public&fld_id=747223&page=2&token=secret'>&#187;</a>"#,
        ] {
            assert!(
                next_gallery_url(bad, &current, &gallery, 1, 4).is_err(),
                "{bad}"
            );
        }
    }

    #[tokio::test]
    async fn bounded_two_page_gallery_preserves_first_seen_child_order() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (number, html) in [
                (
                    1,
                    r#"<div class="page_main_title">digits</div><div class="gallerys"><a href="/j6eu91sbl9bs">One</a></div><a href='/?op=user_public&fld_id=747223&usr_login=gdldev&page=2'>&#187;</a>"#,
                ),
                (
                    2,
                    r#"<div class="gallerys"><a href="/j6eu91sbl9bs">Again</a><a href="/vx4oh119izyr">Two</a></div>"#,
                ),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buffer = [0u8; 4096];
                let n = socket.read(&mut buffer).await.unwrap();
                let request = String::from_utf8_lossy(&buffer[..n]);
                if number == 1 {
                    assert!(request.starts_with("GET /p/gdldev/747223/digits HTTP/1.1"));
                } else {
                    assert!(request.contains("page=2"), "{request}");
                }
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
                    html.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let gallery = match_gallery_url("https://imagetwist.com/p/gdldev/747223/digits").unwrap();
        let events = gallery_at_base(&Client::new(), &gallery, 3, &base)
            .await
            .unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(
            serde_json::to_value(&events[1]).unwrap()["url"],
            "https://imagetwist.com/vx4oh119izyr"
        );
        server.await.unwrap();
    }

    #[test]
    fn original_media_keeps_safe_filename_and_rejects_placeholder() {
        let image = match_url("https://imagetwist.com/f1i2s4vhvbrq").unwrap();
        let html =
            r#"<img src="https://img.imagetwist.com/files/original.png" alt="test &amp; image">"#;
        let (file, metadata) = parse_page(html, &image).unwrap();
        assert_eq!(file.filename, "test & image.png");
        assert_eq!(metadata["token"], "f1i2s4vhvbrq");
        assert!(parse_page(r#"<img src="/imgs/noimage.jpg">"#, &image).is_err());
        for bad in [
            "http://img.imagetwist.com/files/a.jpg",
            "https://evilimagetwist.com/files/a.jpg",
            "https://img.imagetwist.com/imgs/blank.png",
            "https://img.imagetwist.com/files/a.jpg?token=secret",
        ] {
            assert!(media_url(bad).is_none(), "{bad}");
        }
    }

    #[tokio::test]
    async fn fetches_one_bounded_public_page_without_downloading_media() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = [0u8; 4096];
            let n = socket.read(&mut bytes).await.unwrap();
            assert!(String::from_utf8_lossy(&bytes[..n]).starts_with("GET /f1i2s4vhvbrq HTTP/1.1"));
            let html = r#"<img src="https://img.imagetwist.com/files/original.png" alt="Public">"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
                html.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let image = match_url("https://imagetwist.com/f1i2s4vhvbrq").unwrap();
        let (file, _) = resolve_at_base(&Client::new(), &image, &base)
            .await
            .unwrap();
        assert_eq!(file.filename, "Public.png");
        server.await.unwrap();
    }
}
