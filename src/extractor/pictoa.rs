//! Public, bounded Pictoa image originals and album child queues from pinned pictoa.py.
use crate::{
    message::{media_metadata, queued_url, ExtractorMessage, Metadata},
    safe_filename, MediaItem,
};
use percent_encoding::percent_decode_str;
use reqwest::Client;
use serde_json::Value;
use std::{collections::HashSet, error::Error, time::Duration};
use url::Url;

const MAX_HTML: usize = 2 * 1024 * 1024;
const MAX_ALBUM_PAGES: u16 = 4;
const MAX_CHILDREN: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Image { album_id: String, image_id: String },
    Album { album_id: String },
}

fn valid_id(value: &str) -> bool {
    (1..=16).contains(&value.len())
        && value.bytes().all(|b| b.is_ascii_digit())
        && value.parse::<u64>().ok().is_some_and(|n| n > 0)
}

fn album_id(value: &str) -> Option<&str> {
    let last = value.rsplit_once('-').map_or(value, |(_, id)| id);
    valid_id(last).then_some(last)
}

fn first_party(host: &str) -> bool {
    matches!(
        host,
        "pictoa.com" | "www.pictoa.com" | "pictoa.com.de" | "www.pictoa.com.de"
    )
}

pub fn match_url(raw: &str) -> Option<Target> {
    let lower = raw.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "/../", "/./"]
        .iter()
        .any(|part| lower.contains(part))
        || raw.contains('%')
    {
        return None;
    }
    let url = Url::parse(raw).ok()?;
    if !matches!(url.scheme(), "https" | "http")
        || !first_party(url.host_str()?)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let segments: Vec<_> = url.path_segments()?.collect();
    match segments.as_slice() {
        ["albums", filename] => {
            let base = filename.strip_suffix(".html")?;
            Some(Target::Album { album_id: album_id(base)?.to_owned() })
        }
        ["albums", folder, file] => {
            let id = file.strip_suffix(".html")?;
            if !valid_id(id) {
                return None;
            }
            Some(Target::Image {
                album_id: album_id(folder)?.into(),
                image_id: id.to_owned(),
            })
        }
        _ => None,
    }
}

fn field<'a>(html: &'a str, start: &str, end: &str) -> Option<&'a str> {
    html.split_once(start)?.1.split_once(end).map(|(v, _)| v)
}

fn trusted_original(raw: &str) -> Option<(String, String)> {
    if raw.len() > 1024 || raw.contains('\\') || raw.contains('?') || raw.contains('#')
        || raw.chars().any(char::is_control)
    {
        return None;
    }
    let url = Url::parse(raw).ok()?;
    let host = url.host_str()?;
    if url.scheme() != "https"
        || !(host == "pictoa.com" || host.ends_with(".pictoa.com")
            || host == "pictoa.com.de" || host.ends_with(".pictoa.com.de"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let mut last = None;
    for part in url.path_segments()? {
        let name = percent_decode_str(part).decode_utf8().ok()?;
        if name == "." || name == ".." || name.contains('/')
            || name.contains('\\') || name.contains('%')
            || name.chars().any(char::is_control)
        {
            return None;
        }
        last = Some(name.into_owned());
    }
    let name = last?;
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    if !matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif") {
        return None;
    }
    let name = safe_filename(&name);
    if name.is_empty() || matches!(name.as_str(), "." | "..") {
        return None;
    }
    Some((url.to_string(), ext))
}

fn image_record(
    html: &str, album_id: &str, image_id: &str,
) -> Result<(MediaItem, Metadata), Box<dyn Error + Send + Sync>> {
    let title = field(html, "property=\"og:title\" content=\"", "\"")
        .ok_or("Pictoa public image page lacks title metadata")?;
    let image = field(html, "property=\"og:image\" content=\"", "\"")
        .ok_or("Pictoa public image page lacks original metadata")?;
    let (url, extension) = trusted_original(image)
        .ok_or("Pictoa public original points outside approved media hosts")?;
    let album_title = title.rsplit_once(" #").map_or(title, |(text, _)| text);
    if album_title.len() > 240 || album_title.chars().any(char::is_control) {
        return Err("Pictoa original album title exceeds source bounds".into());
    }
    let item = MediaItem {
        url,
        filename: format!("{image_id}.{extension}"),
        extractor: "pictoa",
        post_id: image_id.parse::<u64>().ok(),
    };
    let mut metadata = media_metadata(&item);
    metadata.insert("subcategory".into(), "image".into());
    metadata.insert("id".into(), image_id.into());
    metadata.insert("album_id".into(), album_id.into());
    metadata.insert("album_title".into(), album_title.into());
    metadata.insert("extension".into(), extension.into());
    metadata.insert("post_url".into(), format!("https://pictoa.com/albums/{album_id}/{image_id}.html").into());
    Ok((item, metadata))
}

fn album_metadata(html: &str, album_id: &str) -> Metadata {
    let mut meta = Metadata::new();
    meta.insert("category".into(), "pictoa".into());
    meta.insert("subcategory".into(), "album".into());
    meta.insert("album_id".into(), album_id.into());
    if let Some(title) = field(html, "<h1>", "<")
        .filter(|s| s.len() <= 240 && !s.chars().any(char::is_control))
    {
        meta.insert("album_title".into(), title.into());
    }
    let mut tags = Vec::new();
    if let Some(rest) = html.split_once("<ol class=\"related-categories") {
        if let Some((body, _)) = rest.1.split_once("</ol>") {
            for entry in body.split("<li").skip(1).take(24) {
                let Some((_, text)) = entry.split_once("<a ") else { continue };
                let Some(name) = field(text, ">", "<") else { continue };
                if !name.is_empty() && name.len() <= 80 && !name.chars().any(char::is_control) {
                    tags.push(name.to_owned());
                }
            }
        }
    }
    if !tags.is_empty() {
        meta.insert("tags".into(), Value::from(tags));
    }
    meta
}

fn album_children(
    html: &str,
    album_id: &str,
    meta: &Metadata,
    seen: &mut HashSet<String>,
    result: &mut Vec<ExtractorMessage>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let body = field(html, "<main>", "<span id=\"flag\" >")
        .ok_or("Pictoa album page lacks a supported source image container")?;
    let base = Url::parse("https://pictoa.com/")?;
    for block in body.split("<a rel=\"nofollow\" href=\"").skip(1).take(3000) {
        let Some((href, _)) = block.split_once('"') else { continue };
        // Raw encoded traversal must be rejected before Url normalizes it.
        let lowercase = href.to_ascii_lowercase();
        if ["%2e", "%2f", "%5c", "/../", "/./"].iter().any(|term| lowercase.contains(term)) {
            continue;
        }
        let Ok(link) = base.join(href) else { continue };
        let Some(Target::Image { album_id: child_album, image_id }) =
            match_url(link.as_str()) else { continue };
        if child_album != album_id {
            continue;
        }
        let target = format!("https://pictoa.com/albums/{album_id}/{image_id}.html");
        if !seen.insert(target.clone()) {
            continue;
        }
        if result.len() >= MAX_CHILDREN {
            return Err("Pictoa album exceeds bounded source image count".into());
        }
        let mut metadata = meta.clone();
        metadata.insert("id".into(), image_id.into());
        result.push(queued_url(target, metadata));
    }
    Ok(())
}

fn continuation(base: &Url, album_id: &str, page: u16, html: &str) -> Option<Url> {
    let raw = field(html, "<link rel=\"next\" href=\"", "\"")?;
    let url = base.join(raw).ok()?;
    if url.origin() != base.origin()
        || url.path() != format!("/albums/{album_id}.html")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let query: Vec<_> = url.query_pairs().collect();
    if query.len() != 1 || query[0].0 != "page" {
        return None;
    }
    let next = query[0].1.parse::<u16>().ok()?;
    (next == page + 1 && next <= MAX_ALBUM_PAGES).then_some(url)
}

async fn source_html(
    client: &Client, endpoint: Url, root: &Url,
) -> Result<String, Box<dyn Error + Send + Sync>> {
    let mut response = client.get(endpoint).send().await?.error_for_status()?;
    if response.url().origin() != root.origin() {
        return Err("Pictoa page redirected outside canonical first-party origin".into());
    }
    if response.content_length().is_some_and(|n| n > MAX_HTML as u64) {
        return Err("Pictoa public HTML exceeds response limit".into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_HTML.saturating_sub(body.len()) {
            return Err("Pictoa public HTML exceeds response limit".into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8(body)?)
}

async fn image_at(
    client: &Client, root: &Url, album_id: &str, image_id: &str,
) -> Result<(MediaItem, Metadata), Box<dyn Error + Send + Sync>> {
    let page = root.join(&format!("albums/{album_id}/{image_id}.html"))?;
    image_record(&source_html(client, page, root).await?, album_id, image_id)
}

async fn album_at(
    client: &Client, root: &Url, album_id: &str, max_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let mut page = root.join(&format!("albums/{album_id}.html"))?;
    let mut seen = HashSet::new();
    let mut events = Vec::new();
    for index in 1..=max_pages.clamp(1, MAX_ALBUM_PAGES) {
        if index > 1 {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        let html = source_html(client, page.clone(), root).await?;
        let meta = album_metadata(&html, album_id);
        album_children(&html, album_id, &meta, &mut seen, &mut events)?;
        if events.len() >= MAX_CHILDREN {
            break;
        }
        let Some(next) = continuation(root, album_id, index, &html) else { break };
        page = next;
    }
    Ok(events)
}

pub async fn resolve(
    client: &Client, target: Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let root = Url::parse("https://pictoa.com/")?;
    match target {
        Target::Image {album_id, image_id} =>
            Ok(vec![image_at(client, &root, &album_id, &image_id).await?.0]),
        Target::Album { .. } =>
            Err("Pictoa album discovery requires typed --dump-messages or --follow-queues".into()),
    }
}

pub async fn resolve_messages(
    client: &Client, target: Target, max_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let root = Url::parse("https://pictoa.com/")?;
    match target {
        Target::Image {album_id, image_id} => {
            let (item, metadata) = image_at(client, &root, &album_id, &image_id).await?;
            Ok(vec![
                ExtractorMessage::Directory { metadata: metadata.clone() },
                ExtractorMessage::Url { url: item.url, metadata },
            ])
        }
        Target::Album { album_id } => album_at(client, &root, &album_id, max_pages).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::TcpListener};

    #[test]
    fn pinned_album_and_single_image_routes_reject_host_forgeries() {
        assert_eq!(match_url("https://www.pictoa.com/albums/Art-12345.html"),
            Some(Target::Album {album_id:"12345".into()}));
        assert_eq!(match_url("https://pictoa.com.de/albums/Art-12345/678.html"),
            Some(Target::Image {album_id:"12345".into(), image_id:"678".into()}));
        for bad in [
            "https://evilpictoa.com/albums/12345.html",
            "https://pictoa.com/albums/0.html",
            "https://pictoa.com/albums/12345/notanimage.html",
            "https://pictoa.com/albums/../private/678.html",
            "https://user:password@pictoa.com/albums/12345.html",
            "https://pictoa.com/albums/12345.html?key=secret",
        ] { assert!(match_url(bad).is_none(),"{bad}"); }
    }

    #[test]
    fn image_og_metadata_preserves_album_id_and_safe_first_party_originals() {
        let html = r#"<meta property="og:title" content="Art Gallery #42"><meta property="og:image" content="https://cdn.pictoa.com/photos/flower.jpg">"#;
        let (item, meta) = image_record(html, "12345", "678").unwrap();
        assert_eq!(item.url, "https://cdn.pictoa.com/photos/flower.jpg");
        assert_eq!(item.filename, "678.jpg");
        assert_eq!(meta["album_id"], "12345");
        assert_eq!(meta["album_title"], "Art Gallery");
        let forged=html.replace("cdn.pictoa.com","evil-pictoa.com");
        assert!(image_record(&forged,"12345","678").is_err());
    }

    #[test]
    fn album_queues_first_party_children_with_dedup_and_source_tags() {
        let html = r#"<h1>Art Gallery</h1><ol class="related-categories"><li><a href="/tags/flowers">Flowers</a></li></ol><main><a rel="nofollow" href="/albums/Art-12345/678.html">First</a><a rel="nofollow" href="/albums/12345/678.html">Duplicate</a><a rel="nofollow" href="https://evilpictoa.com/albums/12345/999.html">Forged</a><span id="flag" >"#;
        let meta = album_metadata(html, "12345");
        let mut seen=HashSet::new();
        let mut events=Vec::new();
        album_children(html,"12345",&meta,&mut seen,&mut events).unwrap();
        assert_eq!(events.len(),1);
        let first=serde_json::to_value(&events[0]).unwrap();
        assert_eq!(first["url"],"https://pictoa.com/albums/12345/678.html");
        assert_eq!(first["metadata"]["album_title"],"Art Gallery");
        assert_eq!(first["metadata"]["tags"][0],"Flowers");
    }

    #[test]
    fn album_continuation_rejects_forged_origins_and_page_jumps() {
        let root=Url::parse("https://pictoa.com/").unwrap();
        assert!(continuation(&root,"12345",1,r#"<link rel="next" href="/albums/12345.html?page=2">"#).is_some());
        assert!(continuation(&root,"12345",1,r#"<link rel="next" href="https://evilpictoa.com/albums/12345.html?page=2">"#).is_none());
        assert!(continuation(&root,"12345",1,r#"<link rel="next" href="/albums/12345.html?page=4">"#).is_none());
    }

    #[tokio::test]
    async fn original_image_uses_one_get_from_canonical_source() {
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr=listener.local_addr().unwrap();
        let server=tokio::spawn(async move {
            let (mut socket,_)=listener.accept().await.unwrap();
            let mut request=[0u8;4096];
            let n=socket.read(&mut request).await.unwrap();
            assert!(String::from_utf8_lossy(&request[..n]).starts_with("GET /albums/12345/678.html HTTP/1.1"));
            let html=r#"<meta property="og:title" content="Art Gallery #42"><meta property="og:image" content="https://cdn.pictoa.com/photos/flower.jpg">"#;
            let reply=format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",html.len());
            socket.write_all(reply.as_bytes()).await.unwrap();
        });
        let root=Url::parse(&format!("http://{addr}/")).unwrap();
        let (item,meta)=image_at(&Client::new(),&root,"12345","678").await.unwrap();
        assert_eq!(item.filename,"678.jpg");
        assert_eq!(meta["album_title"],"Art Gallery");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn two_public_album_pages_yield_ordered_deduplicated_child_queues() {
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr=listener.local_addr().unwrap();
        let server=tokio::spawn(async move {
            for (path,html) in [
                ("/albums/12345.html",r#"<h1>Art</h1><main><a rel="nofollow" href="/albums/12345/111.html">One</a><span id="flag" ><link rel="next" href="/albums/12345.html?page=2">"#),
                ("/albums/12345.html?page=2",r#"<h1>Art</h1><main><a rel="nofollow" href="/albums/12345/111.html">Dup</a><a rel="nofollow" href="/albums/12345/222.html">Two</a><span id="flag" >"#)
            ] {
                let (mut socket,_)=listener.accept().await.unwrap();
                let mut request=[0u8;4096];
                let n=socket.read(&mut request).await.unwrap();
                assert!(String::from_utf8_lossy(&request[..n]).starts_with(&format!("GET {path} HTTP/1.1")));
                let reply=format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",html.len());
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let root=Url::parse(&format!("http://{addr}/")).unwrap();
        let events=album_at(&Client::new(),&root,"12345",4).await.unwrap();
        assert_eq!(events.len(),2);
        assert_eq!(serde_json::to_value(&events[1]).unwrap()["url"],"https://pictoa.com/albums/12345/222.html");
        server.await.unwrap();
    }
}
