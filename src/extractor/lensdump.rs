//! Independent guarded public Lensdump image and album subset of pinned lensdump.py.
pub mod user;
use crate::{
    message::{media_metadata, ExtractorMessage, Metadata},
    safe_filename, MediaItem,
};
use reqwest::Client;
use serde_json::Value;
use std::{collections::HashSet, error::Error, time::Duration};
use url::Url;

const MAX_HTML_BYTES: usize = 2 * 1024 * 1024;
const MAX_ALBUM_IMAGES: usize = 160;
const MAX_ALBUM_PAGES: u16 = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Image(String),
    Album(String),
}

fn safe_id(id: &str) -> bool {
    (1..=80).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

pub fn match_url(raw: &str) -> Option<Target> {
    let lower = raw.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "/../", "/./"]
        .iter()
        .any(|bad| lower.contains(bad))
    {
        return None;
    }
    let url = Url::parse(raw).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let host = url.host_str()?;
    let parts: Vec<_> = url.path_segments()?.collect();
    if host == "lensdump.com" {
        return match parts.as_slice() {
            ["i", id] | ["i", id, ""] if safe_id(id) => Some(Target::Image((*id).into())),
            ["a", id] | ["a", id, ""] if safe_id(id) => Some(Target::Album((*id).into())),
            _ => None,
        };
    }
    let cdn_image = host
        .strip_suffix(".lensdump.com")
        .is_some_and(|sub| sub == "i" || (
            sub.len() == 2
                && sub.starts_with('i')
                && sub.as_bytes()[1].is_ascii_digit()
        ));
    let cdn_short = host
        .strip_suffix(".l3n.co")
        .is_some_and(|sub| sub.len() == 1 && sub.as_bytes()[0].is_ascii_alphanumeric());
    if !(cdn_image || cdn_short) {
        return None;
    }
    match parts.as_slice() {
        [id] | ["i", id] | [id, ""] if safe_id(id) => Some(Target::Image((*id).into())),
        _ => None,
    }
}

fn extract<'a>(html: &'a str, start: &str, end: char) -> Option<&'a str> {
    html.split_once(start)?.1.split_once(end).map(|(s, _)| s)
}

fn media(raw: &str) -> Option<(Url, String, String)> {
    let mut url = if raw.starts_with("//") {
        Url::parse(&format!("https:{raw}")).ok()?
    } else {
        Url::parse(raw).ok()?
    };
    if url.scheme() == "http" {
        url.set_scheme("https").ok()?;
    }
    let host = url.host_str()?;
    if url.scheme() != "https"
        || !(matches!(host, "lensdump.com" | "l3n.co")
            || host.ends_with(".lensdump.com")
            || host.ends_with(".l3n.co"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path().contains('%')
    {
        return None;
    }
    let name = url.path_segments()?.next_back()?;
    let extension = name.rsplit_once('.')?.1.to_ascii_lowercase();
    if !matches!(
        extension.as_str(),
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif"
    ) {
        return None;
    }
    let safe = safe_filename(name);
    if safe.is_empty() || matches!(safe.as_str(), "." | "..") {
        return None;
    }
    Some((url, safe, extension))
}

fn image_record(
    html: &str,
    id: &str,
) -> Result<(MediaItem, Metadata), Box<dyn Error + Send + Sync>> {
    let raw = extract(html, "property=\"og:image\" content=\"", '"')
        .ok_or("Lensdump image page has no original media marker")?;
    let (url, name, extension) =
        media(raw).ok_or("Lensdump image media origin or type is not supported")?;
    let item = MediaItem {
        url: url.to_string(),
        filename: format!("lensdump_{id}_{name}"),
        extractor: "lensdump",
        post_id: None,
    };
    let mut meta = media_metadata(&item);
    meta.insert("subcategory".into(), "image".into());
    meta.insert("image_id".into(), id.into());
    meta.insert("extension".into(), extension.into());
    meta.insert(
        "post_url".into(),
        format!("https://lensdump.com/i/{id}").into(),
    );
    if let Some(title) = extract(html, "property=\"og:title\" content=\"", '"')
        .filter(|s| s.len() <= 240 && !s.chars().any(char::is_control))
    {
        meta.insert("title".into(), title.replace("&amp;", "&").into());
    }
    for (field, marker) in [
        ("width", "property=\"image:width\" content=\""),
        ("height", "property=\"image:height\" content=\""),
    ] {
        if let Some(number) = extract(html, marker, '"')
            .and_then(|s| s.parse::<u32>().ok())
            .filter(|v| (1..=50000).contains(v))
        {
            meta.insert(field.into(), number.into());
        }
    }
    if let Some(date) = extract(html, "<span title=\"", '"')
        .filter(|s| s.len() <= 48 && !s.chars().any(char::is_control))
    {
        meta.insert("date_source".into(), date.into());
    }
    Ok((item, meta))
}

fn album_records(
    html: &str,
    id: &str,
    seen: &mut HashSet<String>,
    result: &mut Vec<(MediaItem, Metadata)>,
) {
    let title = extract(html, "property=\"og:title\" content=\"", '"')
        .map(|s| s.replace("&amp;", "&"))
        .filter(|s| s.len() <= 240 && !s.chars().any(char::is_control));
    for chunk in html.split(" class=\"list-item ").skip(1).take(2400) {
        if result.len() >= MAX_ALBUM_IMAGES {
            break;
        }
        let attrs = chunk.split_once('>').map(|(attrs, _)| attrs).unwrap_or("");
        let Some(json) =
            extract(attrs, "data-object='", '\'').or_else(|| extract(attrs, "data-object=\"", '"'))
        else {
            continue;
        };
        let decoded = json.replace("&quot;", "\"").replace("&amp;", "&");
        let Ok(data) = serde_json::from_str::<Value>(&decoded) else {
            continue;
        };
        let Some(image_id) = data
            .get("name")
            .and_then(Value::as_str)
            .filter(|s| safe_id(s))
        else {
            continue;
        };
        let Some(raw_url) = data.get("url").and_then(Value::as_str) else {
            continue;
        };
        let Some((url, name, extension)) = media(raw_url) else {
            continue;
        };
        if !seen.insert(image_id.to_owned()) {
            continue;
        };
        let item = MediaItem {
            url: url.to_string(),
            filename: format!("{id}_{image_id}_{name}"),
            extractor: "lensdump",
            post_id: None,
        };
        let mut meta = media_metadata(&item);
        meta.insert("subcategory".into(), "album".into());
        meta.insert("gallery_id".into(), id.into());
        meta.insert("image_id".into(), image_id.into());
        meta.insert("extension".into(), extension.into());
        meta.insert("num".into(), (result.len() + 1).into());
        if let Some(title) = &title {
            meta.insert("album_title".into(), title.clone().into());
        }
        if let Some(title) = data
            .get("title")
            .and_then(Value::as_str)
            .filter(|s| s.len() <= 200 && !s.chars().any(char::is_control))
        {
            meta.insert("title".into(), title.replace("&amp;", "&").into());
        }
        for field in ["width", "height"] {
            let n = data
                .get(field)
                .and_then(|value| {
                    value
                        .as_u64()
                        .or_else(|| value.as_str()?.parse::<u64>().ok())
                })
                .filter(|n| (1..=50000).contains(n));
            if let Some(n) = n {
                meta.insert(field.into(), n.into());
            }
        }
        result.push((item, meta));
    }
}

fn next_album_page(html: &str, current: &Url, id: &str, number: u16) -> Option<Url> {
    let attrs = html
        .split_once(" data-pagination=\"next\"")?
        .1
        .split_once('>')?
        .0;
    let path = extract(attrs, "href=\"", '"')?;
    let next = current.join(path).ok()?;
    if next.origin() != current.origin()
        || next.path() != format!("/a/{id}/")
        || !next.username().is_empty()
        || next.password().is_some()
        || next.fragment().is_some()
    {
        return None;
    }
    let query: Vec<_> = next.query_pairs().collect();
    if query.len() != 1 || query[0].0 != "page" {
        return None;
    }
    let page = query[0].1.parse::<u16>().ok()?;
    (page == number + 1 && page <= MAX_ALBUM_PAGES).then_some(next)
}

async fn source_html(
    client: &Client,
    endpoint: Url,
) -> Result<String, Box<dyn Error + Send + Sync>> {
    let mut response = client
        .get(endpoint.clone())
        .send()
        .await?
        .error_for_status()?;
    if response.url().origin() != endpoint.origin() {
        return Err("Lensdump page redirected outside its first-party origin".into());
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_HTML_BYTES as u64)
    {
        return Err("Lensdump HTML exceeds size limit".into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_HTML_BYTES.saturating_sub(body.len()) {
            return Err("Lensdump HTML exceeds size limit".into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8(body)?)
}

async fn at_base(
    client: &Client,
    base: &Url,
    target: &Target,
    max_pages: u16,
) -> Result<Vec<(MediaItem, Metadata)>, Box<dyn Error + Send + Sync>> {
    match target {
        Target::Image(id) => {
            let page = source_html(client, base.join(&format!("i/{id}"))?).await?;
            Ok(vec![image_record(&page, id)?])
        }
        Target::Album(id) => {
            let mut url = base.join(&format!("a/{id}"))?;
            let mut seen = HashSet::new();
            let mut results = Vec::new();
            for page in 1..=max_pages.clamp(1, MAX_ALBUM_PAGES) {
                if page > 1 {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                let html = source_html(client, url.clone()).await?;
                album_records(&html, id, &mut seen, &mut results);
                if results.len() >= MAX_ALBUM_IMAGES {
                    break;
                }
                let Some(next) = next_album_page(&html, &url, id, page) else {
                    break;
                };
                url = next;
            }
            Ok(results)
        }
    }
}

pub async fn resolve(
    client: &Client,
    target: Target,
    max_pages: u16,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse("https://lensdump.com/")?;
    Ok(at_base(client, &base, &target, max_pages)
        .await?
        .into_iter()
        .map(|(item, _)| item)
        .collect())
}

pub async fn resolve_messages(
    client: &Client,
    target: Target,
    max_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse("https://lensdump.com/")?;
    let mut result = Vec::new();
    for (item, metadata) in at_base(client, &base, &target, max_pages).await? {
        result.push(ExtractorMessage::Directory {
            metadata: metadata.clone(),
        });
        result.push(ExtractorMessage::Url {
            url: item.url,
            metadata,
        });
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    #[test]
    fn source_matcher_accepts_pinned_public_images_and_albums_only() {
        assert_eq!(
            match_url("https://lensdump.com/i/Abc12"),
            Some(Target::Image("Abc12".into()))
        );
        assert_eq!(
            match_url("https://lensdump.com/a/Abc12/"),
            Some(Target::Album("Abc12".into()))
        );
        assert_eq!(
            match_url("https://i1.lensdump.com/Abc12"),
            Some(Target::Image("Abc12".into()))
        );
        assert_eq!(
            match_url("https://x.l3n.co/Abc12"),
            Some(Target::Image("Abc12".into()))
        );
        for bad in [
            "https://evil-lensdump.com/i/Abc12",
            "https://i10.lensdump.com/Abc12",
            "https://xx.l3n.co/Abc12",
            "https://i1.lensdump.com/Abc12?private=1",
            "https://lensdump.com/i/../../private",
            "https://lensdump.com/i/Abc12?secret=1",
            "https://lensdump.com/i/unsafe-123",
            "https://name:password@lensdump.com/i/Abc12",
            "https://lensdump.com:8443/a/Abc12",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn public_image_and_album_records_preserve_trusted_media_order_and_metadata() {
        let html = r#"<meta property="og:title" content="Cover &amp; Detail"><meta property="og:image" content="https://i1.lensdump.com/cover.png"><meta property="image:width" content="800">"#;
        let (item, meta) = image_record(html, "Abc12").unwrap();
        assert_eq!(item.url, "https://i1.lensdump.com/cover.png");
        assert_eq!(meta["title"], "Cover & Detail");
        assert_eq!(meta["width"], 800);
        let unsafe_html = html.replace("i1.lensdump.com", "evil-lensdump.com");
        assert!(image_record(&unsafe_html, "Abc12").is_err());
        let album = r#"<meta property="og:title" content="Album"><div class="list-item row" data-object='{"name":"Img1","url":"https://i2.lensdump.com/one.jpg","width":"600"}'></div><div class="list-item row" data-object='{"name":"Img2","url":"https://i2.lensdump.com/two.png"}'></div><div class="list-item row" data-object='{"name":"Img1","url":"https://i2.lensdump.com/one.jpg"}'></div><div class="list-item row" data-object='{"name":"Forged","url":"https://evil-lensdump.com/image.png"}'></div>"#;
        let mut seen = HashSet::new();
        let mut records = Vec::new();
        album_records(album, "Abc12", &mut seen, &mut records);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].1["width"], 600);
        assert_eq!(records[1].1["num"], 2);
        assert!(records.iter().all(|(item, _)| item.post_id.is_none()));
    }

    #[tokio::test]
    async fn public_single_original_reads_one_bounded_html_page() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let count = socket.read(&mut buf).await.unwrap();
            assert!(String::from_utf8_lossy(&buf[..count]).starts_with("GET /i/Abc12 HTTP/1.1"));
            let html = r#"<meta property="og:image" content="https://i1.lensdump.com/one.jpg">"#;
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
                html.len()
            );
            socket.write_all(reply.as_bytes()).await.unwrap();
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let files = at_base(&Client::new(), &base, &Target::Image("Abc12".into()), 1)
            .await
            .unwrap();
        assert_eq!(files[0].0.filename, "lensdump_Abc12_one.jpg");
        server.await.unwrap();
    }
}
