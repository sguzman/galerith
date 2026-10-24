//! Bounded public Postimages file/gallery subset of pinned imagehosts.py.
//! Full filename, archive, metadata, account and live-site parity remain open.
use crate::{
    message::{media_metadata, queued_url, ExtractorMessage, Metadata},
    safe_filename, MediaItem,
};
use reqwest::Client;
use serde_json::Value;
use std::{collections::HashSet, error::Error};
use url::Url;

const MAX_HTML_BYTES: usize = 2 * 1024 * 1024;
const MAX_JSON_BYTES: usize = 2 * 1024 * 1024;
const MAX_GALLERY_PAGES: u16 = 4;
const MAX_GALLERY_ITEMS: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    File(String),
    Gallery(String),
}

fn valid_token(s: &str) -> bool {
    (1..=48).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric())
}

pub fn match_url(input: &str) -> Option<Target> {
    let url = Url::parse(input).ok()?;
    if url.scheme() != "https"
        || !matches!(url.host_str(), Some(
            "postimg.cc" | "www.postimg.cc" | "postimages.org" |
            "www.postimages.org" | "pixxxels.cc" | "www.pixxxels.cc"
        ))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let path: Vec<_> = url.path_segments()?.collect();
    match path.as_slice() {
        ["gallery", id] if valid_token(id) => Some(Target::Gallery((*id).into())),
        [id] | ["image", id] if valid_token(id) => Some(Target::File((*id).into())),
        _ => None,
    }
}

fn safe_media_link(raw: &str) -> Option<(Url, String)> {
    let url = Url::parse(raw).ok()?;
    if url.scheme() != "https"
        || url.host_str() != Some("i.postimg.cc")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.fragment().is_some()
        || url.query().is_some_and(|query| query != "dl=1")
    {
        return None;
    }
    let name = url.path_segments()?.next_back()?;
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    if name.is_empty() || name.len() > 240 || name.contains('%')
        || !matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif")
    {
        return None;
    }
    let safe = safe_filename(name);
    Some((url, safe))
}

fn decode_entities(value: &str) -> String {
    value
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&gt;", ">")
        .replace("&lt;", "<")
        .replace("&amp;", "&")
}

fn media_from_page(
    html: &str,
    token: &str,
) -> Result<(MediaItem, Metadata), Box<dyn Error + Send + Sync>> {
    let offset = html.find(" id=\"download\"")
        .ok_or("Postimages page has no public download anchor")?;
    let anchor = html[..offset].rfind("<a").ok_or("Postimages download marker is not on an anchor")?;
    let tag = html[anchor..].split_once('>').ok_or("Postimages download anchor is incomplete")?.0;
    let raw = tag.split_once(" href=\"")
        .and_then(|(_, rest)| rest.split_once('"'))
        .map(|(link, _)| link)
        .ok_or("Postimages download anchor has no href")?;
    let (url, fallback) = safe_media_link(raw)
        .ok_or("Postimages public media link has an unsupported origin")?;
    let ext = fallback.rsplit_once('.').map(|(_, ext)| ext).unwrap_or("jpg");
    let filename = tag
        .split_once(" alt=\"")
        .and_then(|(_, rest)| rest.split_once('"'))
        .and_then(|(alt, _)| {
            let text = decode_entities(alt);
            if text.is_empty()
                || text.len() > 230
                || text.contains('/')
                || text.contains('\\')
                || text.chars().any(char::is_control)
            {
                return None;
            }
            let has_ext = text.rsplit_once('.').is_some_and(|(_, extension)| {
                matches!(extension.to_ascii_lowercase().as_str(),
                    "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif")
            });
            let name = if has_ext { text } else { format!("{text}.{ext}") };
            Some(safe_filename(&name))
        })
        .unwrap_or(fallback);
    let item = MediaItem {
        url: url.to_string(),
        filename,
        extractor: "postimg",
        post_id: None,
    };
    let mut meta = media_metadata(&item);
    meta.insert("subcategory".into(), "image".into());
    meta.insert("token".into(), token.into());
    meta.insert("post_url".into(), format!("https://postimg.cc/{token}").into());
    Ok((item, meta))
}

async fn read_html(client: &Client, endpoint: Url) -> Result<String, Box<dyn Error + Send + Sync>> {
    let mut response = client.get(endpoint).send().await?.error_for_status()?;
    if response.content_length().is_some_and(|v| v > MAX_HTML_BYTES as u64) {
        return Err("Postimages public HTML exceeded byte limit".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_HTML_BYTES.saturating_sub(bytes.len()) {
            return Err("Postimages public HTML exceeded byte limit".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8(bytes)?)
}

async fn file_at_base(
    client: &Client,
    token: &str,
    base: &Url,
) -> Result<(MediaItem, Metadata), Box<dyn Error + Send + Sync>> {
    let html = read_html(client, base.join(token)?).await?;
    media_from_page(&html, token)
}

fn parse_gallery_page(
    value: &Value,
    gallery: &str,
    seen: &mut HashSet<String>,
    events: &mut Vec<ExtractorMessage>,
) -> Result<bool, Box<dyn Error + Send + Sync>> {
    let images = value.get("images").and_then(Value::as_array)
        .ok_or("Postimages gallery API has no images array")?;
    if images.len() > MAX_GALLERY_ITEMS {
        return Err("Postimages gallery page exceeds row limit".into());
    }
    for entry in images {
        let Some(fields) = entry.as_array() else { continue };
        let Some(token) = fields.get(1).and_then(Value::as_str) else { continue };
        if !valid_token(token) || !seen.insert(token.to_owned()) {
            continue;
        }
        let mut meta = Metadata::new();
        meta.insert("category".into(), "postimg".into());
        meta.insert("subcategory".into(), "image".into());
        meta.insert("gallery_id".into(), gallery.into());
        meta.insert("token".into(), token.into());
        for (key, index) in [("width", 4), ("height", 5)] {
            if let Some(value) = fields.get(index).and_then(Value::as_u64)
                .filter(|v| (1..=32768).contains(v))
            {
                meta.insert(key.into(), value.into());
            }
        }
        if let Some(name) = fields.get(2).and_then(Value::as_str)
            .filter(|s| !s.is_empty() && s.len() <= 230
                && !s.contains('/') && !s.contains('\\')
                && !s.chars().any(char::is_control))
        {
            meta.insert("filename_hint".into(), name.into());
        }
        events.push(queued_url(format!("https://postimg.cc/{token}"), meta));
        if events.len() >= MAX_GALLERY_ITEMS {
            return Ok(false);
        }
    }
    Ok(value.get("has_page_next").and_then(Value::as_bool).unwrap_or(false))
}

async fn gallery_at_base(
    client: &Client,
    id: &str,
    max_pages: u16,
    base: &Url,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let mut seen = HashSet::new();
    let mut events = Vec::new();
    for page in 1..=max_pages.clamp(1, MAX_GALLERY_PAGES) {
        let mut endpoint = base.join("json")?;
        endpoint.query_pairs_mut()
            .append_pair("action", "list")
            .append_pair("page", &page.to_string())
            .append_pair("album", id);
        let result = super::http_json::fetch_json_limited(client, endpoint, MAX_JSON_BYTES).await?;
        if !parse_gallery_page(&result, id, &mut seen, &mut events)? {
            break;
        }
        if page < max_pages.min(MAX_GALLERY_PAGES) {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    }
    Ok(events)
}

pub async fn resolve(
    client: &Client,
    target: Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    match target {
        Target::File(token) => {
            let (item, _) = file_at_base(client, &token, &Url::parse("https://postimg.cc/")?).await?;
            Ok(vec![item])
        }
        Target::Gallery(_) => {
            Err("Postimages gallery needs --dump-messages or --follow-queues".into())
        }
    }
}

pub async fn resolve_messages(
    client: &Client,
    target: Target,
    max_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    match target {
        Target::File(token) => {
            let (item, metadata) =
                file_at_base(client, &token, &Url::parse("https://postimg.cc/")?).await?;
            Ok(vec![
                ExtractorMessage::Directory { metadata: metadata.clone() },
                ExtractorMessage::Url { url: item.url, metadata },
            ])
        }
        Target::Gallery(id) => {
            gallery_at_base(client, &id, max_pages, &Url::parse("https://postimg.cc/")?).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::TcpListener};

    #[test]
    fn file_and_gallery_routes_match_pinned_public_aliases() {
        assert_eq!(
            match_url("https://postimages.org/Wtn2b3hC"),
            Some(Target::File("Wtn2b3hC".into()))
        );
        assert_eq!(
            match_url("https://postimg.cc/gallery/wxpDLgX"),
            Some(Target::Gallery("wxpDLgX".into()))
        );
        for bad in [
            "http://postimg.cc/Wtn2b3hC",
            "https://evilpostimg.cc/Wtn2b3hC",
            "https://postimg.cc/Wtn2b3hC?token=secret",
            "https://postimg.cc/gallery/a%2Fb",
            "https://postimg.cc/gallery/../bad",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn image_download_anchor_accepts_pinned_url_and_rejects_foreign_file() {
        let html = r#"<a id="download" href="https://i.postimg.cc/PhJZt1Rw/test-tesuto.png?dl=1" alt="Test &amp; Art">Download</a>"#;
        let (file, meta) = media_from_page(html, "Wtn2b3hC").unwrap();
        assert_eq!(file.url, "https://i.postimg.cc/PhJZt1Rw/test-tesuto.png?dl=1");
        assert_eq!(file.filename, "Test & Art.png");
        assert_eq!(meta["token"], "Wtn2b3hC");
        assert!(media_from_page("<html></html>", "Wtn2b3hC").is_err());
        for bad in [
            "https://attacker.example/evil.png",
            "http://i.postimg.cc/a.png",
            "https://i.postimg.cc/a.png?token=x",
            "https://i.postimg.cc/a.exe",
            "https://user:pass@i.postimg.cc/a.jpg",
        ] {
            assert!(safe_media_link(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn gallery_queue_validates_tokens_deduplicates_and_bounds_records() {
        let data = json!({"images":[
            ["x","Wtn2b3hC","name","png",720,540,null,null,null,null,null],
            ["y","Wtn2b3hC","name","png",720,540,null,null,null,null,null],
            ["z","https://evil.example/i","bad","png",500,500,null,null,null,null,null],
            ["y","Z9Y6srnT","second","jpg",400,300,null,null,null,null,null]
        ],"has_page_next":false});
        let mut seen = HashSet::new();
        let mut events = Vec::new();
        assert!(!parse_gallery_page(&data, "wxpDLgX", &mut seen, &mut events).unwrap());
        assert_eq!(events.len(), 2);
        assert_eq!(serde_json::to_value(&events[0]).unwrap()["url"], "https://postimg.cc/Wtn2b3hC");
        assert_eq!(serde_json::to_value(&events[1]).unwrap()["metadata"]["width"], 400);
    }

    #[tokio::test]
    async fn first_party_public_file_fetches_one_bounded_html_request() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0_u8; 4096];
            let n = stream.read(&mut buf).await.unwrap();
            assert!(String::from_utf8_lossy(&buf[..n]).starts_with("GET /Wtn2b3hC HTTP/1.1"));
            let html = r#"<a id="download" href="https://i.postimg.cc/PhJZt1Rw/test.png?dl=1">Download</a>"#;
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
                html.len()
            );
            stream.write_all(reply.as_bytes()).await.unwrap();
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let (media, _) = file_at_base(&Client::new(), "Wtn2b3hC", &base).await.unwrap();
        assert_eq!(media.filename, "test.png");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn gallery_follows_two_bounded_api_pages_and_deduplicates() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (page, token, more) in [(1,"Wtn2b3hC",true),(2,"Z9Y6srnT",false)] {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut buf = [0_u8; 4096];
                let n = stream.read(&mut buf).await.unwrap();
                let req = String::from_utf8_lossy(&buf[..n]);
                assert!(req.starts_with("GET /json?action=list"), "{req}");
                assert!(req.contains(&format!("page={page}")), "{req}");
                assert!(req.contains("album=wxpDLgX"), "{req}");
                let body = json!({"images":[["x",token,"photo","jpg",100,200,null,null,null,null,null]],"has_page_next":more}).to_string();
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let events = gallery_at_base(&Client::new(), "wxpDLgX", 3, &base).await.unwrap();
        assert_eq!(events.len(), 2);
        server.await.unwrap();
    }
}
