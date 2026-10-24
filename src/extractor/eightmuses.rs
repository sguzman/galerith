//! Independent guarded 8muses public album preview from pinned 8muses.py.
use crate::{
    message::{media_metadata, queued_url, ExtractorMessage, Metadata},
    MediaItem,
};
use reqwest::Client;
use serde_json::Value;
use std::{collections::HashSet, error::Error, time::Duration};
use url::Url;

const MAX_HTML: usize = 2 * 1024 * 1024;
const MAX_PAGES: u16 = 4;
const MAX_MEDIA: usize = 160;
const MAX_QUEUES: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Album { path: String }

fn safe_path(s: &str) -> bool {
    (1..=230).contains(&s.len())
        && s.split('/').all(|p| !p.is_empty()
            && p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
}

pub fn match_url(input: &str) -> Option<Album> {
    let source = if input.starts_with("8muses.com/") || input.starts_with("comics.8muses.com/") {
        format!("https://{input}")
    } else { input.to_owned() };
    let lower = source.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "/../", "/./"].iter().any(|p| lower.contains(p)) {
        return None;
    }
    let url = Url::parse(&source).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !matches!(url.host_str(), Some("8muses.com" | "www.8muses.com" | "comics.8muses.com"))
        || !url.username().is_empty() || url.password().is_some()
        || url.port().is_some() || url.query().is_some() || url.fragment().is_some()
    {
        return None;
    }
    let path = url.path().strip_prefix("/comics/album/")?.trim_end_matches('/');
    safe_path(path).then(|| Album {path: path.to_owned()})
}

/// Pinned source maps printable ASCII using chr(33 + (ord(ch) + 14) % 94).
/// Decode data, never run scripts.
fn decode_payload(input: &str) -> Result<Value, Box<dyn Error + Send + Sync>> {
    if input.len() > MAX_HTML { return Err("8muses encoded payload exceeds limit".into()); }
    let stripped = input.trim_matches(|c: char| matches!(c, '\t' | '\n' | '\r' | ' ' | '!'));
    let html = stripped.replace("&quot;", "\"").replace("&#39;", "'")
        .replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&");
    let mut result = String::with_capacity(html.len());
    for c in html.chars() {
        if ('!'..='~').contains(&c) {
            result.push(char::from_u32(33 + ((c as u32 + 14) % 94))
                .ok_or("Invalid 8muses encoded character")?);
        } else { result.push(c); }
    }
    Ok(serde_json::from_str(&result)?)
}

fn payload(html: &str) -> Result<Value, Box<dyn Error + Send + Sync>> {
    let raw = html.split_once("id=\"ractive-public\" type=\"text/plain\">")
        .and_then(|(_, s)| s.split_once("</script>"))
        .map(|(s, _)| s).ok_or("Missing 8muses public Ractive payload")?;
    decode_payload(raw)
}

fn public_segment(path: &str) -> bool {
    (1..=220).contains(&path.len())
        && path.split('/').all(|p| !p.is_empty()
            && p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
}

fn decode_album(
    data: &Value, path: &str,
    image_ids: &mut HashSet<String>, child_ids: &mut HashSet<String>,
    media: &mut Vec<(MediaItem, Metadata)>, children: &mut Vec<ExtractorMessage>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut common = Metadata::new();
    common.insert("category".into(), "8muses".into());
    common.insert("subcategory".into(), "album".into());
    common.insert("album_path".into(), path.into());
    if let Some(album) = data.get("album") {
        if let Some(id) = album.get("id").and_then(Value::as_u64) {
            common.insert("album_id".into(), id.into());
        }
        for (source, field) in [("name", "album_title"), ("updatedAt", "date_source")] {
            if let Some(name) = album.get(source).and_then(Value::as_str)
                .filter(|s| s.len() <= 240 && !s.chars().any(char::is_control))
            { common.insert(field.into(), name.into()); }
        }
        if let Some(n) = album.get("numberViews")
            .and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse::<u64>().ok()))
        { common.insert("views".into(), n.into()); }
    }
    if let Some(pictures) = data.get("pictures").and_then(Value::as_array) {
        if pictures.len() > MAX_MEDIA { return Err("8muses picture page too large".into()); }
        for entry in pictures {
            let Some(uri) = entry.get("publicUri").and_then(Value::as_str) else { continue };
            if !public_segment(uri) || !image_ids.insert(uri.into()) { continue; }
            if media.len() >= MAX_MEDIA { return Err("8muses album media cap reached".into()); }
            let index = media.len() + 1;
            let item = MediaItem {
                url: format!("https://comics.8muses.com/image/fl/{uri}"),
                filename: format!("{index:03}.jpg"),
                extractor: "8muses",
                post_id: None,
            };
            let mut meta = media_metadata(&item);
            for (key, value) in &common {meta.insert(key.clone(), value.clone());}
            meta.insert("page".into(), index.into());
            meta.insert("hash".into(), uri.into());
            meta.insert("extension".into(), "jpg".into());
            media.push((item, meta));
        }
    }
    if let Some(albums) = data.get("albums").and_then(Value::as_array) {
        if albums.len() > MAX_QUEUES { return Err("8muses child album page too large".into()); }
        for album in albums {
            if album.get("isPrivate").and_then(Value::as_bool).unwrap_or(false) {continue;}
            let Some(slug) = album.get("permalink").and_then(Value::as_str) else {continue};
            if !safe_path(slug) || !child_ids.insert(slug.into()) {continue;}
            if children.len() >= MAX_QUEUES {return Err("8muses album queue cap reached".into());}
            let mut meta = Metadata::new();
            meta.insert("category".into(), "8muses".into());
            meta.insert("subcategory".into(), "album".into());
            meta.insert("album_path".into(), slug.into());
            if let Some(name) = album.get("name").and_then(Value::as_str)
                .filter(|n| n.len() <= 240 && !n.chars().any(char::is_control))
            { meta.insert("title".into(), name.into()); }
            children.push(queued_url(
                format!("https://comics.8muses.com/comics/album/{slug}"), meta,
            ));
        }
    }
    Ok(())
}

async fn html(client: &Client, url: Url) -> Result<String, Box<dyn Error + Send + Sync>> {
    let mut response = client.get(url.clone()).send().await?.error_for_status()?;
    if response.url().origin() != url.origin() {
        return Err("8muses public source redirected outside origin".into());
    }
    if response.content_length().is_some_and(|n| n > MAX_HTML as u64) {
        return Err("8muses HTML exceeds size cap".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_HTML.saturating_sub(bytes.len()) {
            return Err("8muses HTML exceeds size cap".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8(bytes)?)
}

async fn records_at_base(
    client: &Client, base: &Url, album: &Album, max_pages: u16,
) -> Result<(Vec<(MediaItem, Metadata)>, Vec<ExtractorMessage>), Box<dyn Error + Send + Sync>> {
    let mut media = Vec::new();
    let mut queues = Vec::new();
    let mut images = HashSet::new();
    let mut albums = HashSet::new();
    for page in 1..=max_pages.clamp(1, MAX_PAGES) {
        if page > 1 {tokio::time::sleep(Duration::from_secs(1)).await;}
        let suffix = if page == 1 {String::new()} else {format!("/{page}")};
        let url = base.join(&format!("comics/album/{}{suffix}", album.path))?;
        let data = payload(&html(client, url).await?)?;
        let current = data.get("page").and_then(Value::as_u64)
            .ok_or("8muses current page missing")?;
        let last = data.get("pages").and_then(Value::as_u64)
            .ok_or("8muses page count missing")?;
        if current != u64::from(page) || last < current || last == 0 {
            return Err("8muses page sequence invalid".into());
        }
        decode_album(&data, &album.path, &mut images, &mut albums, &mut media, &mut queues)?;
        if current >= last { break; }
    }
    Ok((media, queues))
}

pub async fn resolve(
    client: &Client, target: Album,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse("https://comics.8muses.com/")?;
    let (images, children) = records_at_base(client, &base, &target, MAX_PAGES).await?;
    if images.is_empty() && !children.is_empty() {
        return Err("8muses child album queues require --dump-messages or --follow-queues".into());
    }
    Ok(images.into_iter().map(|(item, _)|item).collect())
}

pub async fn resolve_messages(
    client: &Client, target: Album, max_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse("https://comics.8muses.com/")?;
    let (images, mut children) = records_at_base(client, &base, &target, max_pages).await?;
    let mut events = Vec::new();
    for (item, meta) in images {
        events.push(ExtractorMessage::Directory {metadata: meta.clone()});
        events.push(ExtractorMessage::Url {url: item.url, metadata: meta});
    }
    events.append(&mut children);
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::TcpListener};

    fn encode(json: &str) -> String {
        json.chars().map(|c| {
            if ('!'..='~').contains(&c) {
                char::from_u32(33 + ((c as u32 + 94 - 80) % 94)).unwrap()
            } else { c }
        }).collect()
    }
    fn source_page(current: u64, last: u64) -> String {
        let data = serde_json::json!({
            "page":current,"pages":last,
            "album":{"id":123,"name":"Sample Album","updatedAt":"2026-05-10T12:00:00Z","numberViews":"42"},
            "pictures":[{"publicUri":format!("hash{current}")}],
            "albums":[
                {"permalink":"Artist/Child","name":"Child","isPrivate":false},
                {"permalink":"Artist/Hidden","name":"Private","isPrivate":true}
            ]
        });
        format!(r#"<script id="ractive-public" type="text/plain">{}</script>"#,
            encode(&data.to_string()))
    }

    #[test]
    fn public_routes_reject_unsafe_album_slugs_and_lookalikes() {
        assert_eq!(match_url("https://comics.8muses.com/comics/album/Artist/Album").unwrap().path,"Artist/Album");
        for bad in [
            "https://evil8muses.com/comics/album/Artist/Album",
            "https://comics.8muses.com/comics/album/Artist/../Hidden",
            "https://comics.8muses.com/comics/album/Artist/Album?private=1",
            "https://name:pw@comics.8muses.com/comics/album/Artist/Album",
            "https://comics.8muses.com:8443/comics/album/Artist/Album"
        ] {assert!(match_url(bad).is_none(),"{bad}");}
    }

    #[test]
    fn encoded_public_ractive_json_produces_source_backed_images_and_child_queues() {
        let decoded = payload(&source_page(1, 1)).unwrap();
        assert_eq!(decoded["album"]["name"],"Sample Album");
        let mut seen = HashSet::new();let mut visited = HashSet::new();
        let mut media = Vec::new();let mut queued = Vec::new();
        decode_album(&decoded,"Artist/Album",&mut seen,&mut visited,&mut media,&mut queued).unwrap();
        assert_eq!(media.len(),1);
        assert_eq!(media[0].0.filename,"001.jpg");
        assert_eq!(media[0].1["views"],42);
        assert_eq!(queued.len(),1);
        assert_eq!(serde_json::to_value(&queued[0]).unwrap()["url"],
            "https://comics.8muses.com/comics/album/Artist/Child");
        decode_album(&decoded,"Artist/Album",&mut seen,&mut visited,&mut media,&mut queued).unwrap();
        assert_eq!(media.len(),1);
        assert_eq!(queued.len(),1);
    }

    #[tokio::test]
    async fn two_bounded_encoded_album_pages_use_source_order_and_dedup() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (path, page) in [
                ("/comics/album/Artist/Album",1),
                ("/comics/album/Artist/Album/2",2)
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0u8; 4096];
                let count = socket.read(&mut request).await.unwrap();
                assert!(String::from_utf8_lossy(&request[..count]).starts_with(&format!("GET {path} HTTP/1.1")));
                let body = source_page(page, 2);
                let reply = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let album = match_url("https://comics.8muses.com/comics/album/Artist/Album").unwrap();
        let (records, queued) = records_at_base(&Client::new(),&base,&album,4).await.unwrap();
        assert_eq!(records.len(),2);
        assert_eq!(records[0].0.filename,"001.jpg");
        assert_eq!(records[1].0.filename,"002.jpg");
        assert_eq!(queued.len(),1);
        server.await.unwrap();
    }
}
