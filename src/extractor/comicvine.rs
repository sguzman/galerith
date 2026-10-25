//! Bounded public ComicVine tag galleries from pinned comicvine.py.
use crate::{message::{media_metadata, ExtractorMessage, Metadata}, safe_filename, MediaItem};
use percent_encoding::percent_decode_str;
use reqwest::Client;
use serde_json::Value;
use std::{collections::HashSet, error::Error, time::Duration};
use url::Url;

const MAX_BODY: usize = 2 * 1024 * 1024;
const PER_PAGE: usize = 1000;
const MAX_PAGES: u16 = 3;
const MAX_MEDIA: usize = 3000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gallery { tag: String, object: String, path: String }

fn valid_tag(s: &str) -> bool {
    (1..=100).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

fn valid_object(s: &str) -> bool {
    let Some((a, b)) = s.split_once('-') else { return false };
    [a,b].iter().all(|p| (1..=18).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_digit()))
}

pub fn match_url(raw: &str) -> Option<Gallery> {
    let lower = raw.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "/../", "/./"].iter().any(|bad| lower.contains(bad)) { return None; }
    let url = Url::parse(raw).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str() != Some("comicvine.gamespot.com")
        || !url.username().is_empty() || url.password().is_some()
        || url.port().is_some() || url.query().is_some() || url.fragment().is_some()
    { return None; }
    let parts: Vec<_> = url.path_segments()?.collect();
    let [tag, id, "images", rest @ ..] = parts.as_slice() else { return None };
    if !valid_tag(tag) || !valid_object(id) || rest.len() > 3
        || rest.iter().any(|p| !p.is_empty() && !valid_tag(p)) { return None; }
    Some(Gallery { tag: (*tag).into(), object: (*id).into(), path: url.path().into() })
}

fn gallery_id(html: &str) -> Option<String> {
    let id = html.split_once("data-gallery-id=\"")?.1.split_once('"')?.0;
    ((1..=100).contains(&id.len()) &&
        id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))).then(|| id.into())
}

fn trusted_original(raw: &str) -> Option<(String, String)> {
    let url = Url::parse(raw).ok()?;
    if url.scheme() != "https"
        || !matches!(url.host_str(), Some("comicvine.gamespot.com" | "static.comicvine.com" | "comicvine1.cbsistatic.com"))
        || !url.username().is_empty() || url.password().is_some()
        || url.port().is_some() || url.query().is_some() || url.fragment().is_some() { return None; }
    let mut name = None;
    for segment in url.path_segments()? {
        let decoded = percent_decode_str(segment).decode_utf8().ok()?;
        if matches!(decoded.as_ref(), "." | "..") || decoded.contains('/')
            || decoded.contains('\\') || decoded.chars().any(char::is_control) { return None; }
        name = Some(decoded.into_owned());
    }
    let name = name?;
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    if !matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "gif" | "webp") { return None; }
    let safe = safe_filename(&name);
    (!safe.is_empty() && !matches!(safe.as_str(), "." | "..")).then(|| (url.to_string(), safe))
}

fn read_posts(
    root: &Value, gallery: &Gallery, seen: &mut HashSet<String>,
    records: &mut Vec<(MediaItem, Metadata)>
) -> Result<usize, Box<dyn Error + Send + Sync>> {
    let posts = root.get("images").and_then(Value::as_array).ok_or("ComicVine public JSON lacks images")?;
    if posts.len() > PER_PAGE { return Err("ComicVine API exceeded requested page length".into()); }
    for post in posts {
        let Some(raw) = post.get("original").and_then(Value::as_str) else { continue };
        let Some((url, basename)) = trusted_original(raw) else { continue };
        if !seen.insert(url.clone()) { continue; }
        if records.len() >= MAX_MEDIA { return Err("ComicVine gallery exceeds bounded image total".into()); }
        let post_id = post.get("id").and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse::<u64>().ok()));
        let filename = post_id.map_or_else(|| basename.clone(), |id| format!("{id}_{basename}"));
        let item = MediaItem { url, filename, extractor: "comicvine", post_id };
        let mut metadata = media_metadata(&item);
        metadata.insert("subcategory".into(), "tag".into());
        metadata.insert("tag".into(), gallery.tag.clone().into());
        metadata.insert("object_id".into(), gallery.object.clone().into());
        metadata.insert("num".into(), (records.len()+1).into());
        if let Some(date) = post.get("dateCreated").and_then(Value::as_str)
            .filter(|s| s.len() <= 80 && !s.chars().any(char::is_control)) {
            metadata.insert("date_source".into(), date.into());
        }
        if let Some(tags) = post.get("tags").and_then(Value::as_array) {
            let labels = tags.iter().take(80).filter_map(|v| v.get("name").and_then(Value::as_str))
                .filter(|s| !s.is_empty() && s.len() <= 120 && !s.chars().any(char::is_control))
                .map(Value::from).collect::<Vec<_>>();
            metadata.insert("tags".into(), Value::Array(labels));
        }
        records.push((item, metadata));
    }
    Ok(posts.len())
}

async fn bounded_get(client: &Client, endpoint: Url) -> Result<Vec<u8>, Box<dyn Error + Send + Sync>> {
    let mut response = client.get(endpoint.clone()).send().await?.error_for_status()?;
    if response.url().origin() != endpoint.origin() { return Err("ComicVine HTTP redirect left source origin".into()); }
    if response.content_length().is_some_and(|n| n > MAX_BODY as u64) {
        return Err("ComicVine source exceeds 2 MiB response cap".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_BODY.saturating_sub(bytes.len()) {
            return Err("ComicVine source exceeds 2 MiB response cap".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

async fn at_base(client: &Client, base: &Url, gallery: &Gallery)
    -> Result<Vec<(MediaItem, Metadata)>, Box<dyn Error + Send + Sync>>
{
    let html = String::from_utf8(bounded_get(client, base.join(gallery.path.trim_start_matches('/'))?).await?)?;
    let gallery_id = gallery_id(&html).ok_or("ComicVine source HTML has no supported public gallery ID")?;
    let mut seen = HashSet::new();
    let mut records = Vec::new();
    for page in 0..MAX_PAGES {
        if page > 0 { tokio::time::sleep(Duration::from_secs(1)).await; }
        let mut url = base.join("js/image-data.json")?;
        url.query_pairs_mut()
            .append_pair("images", &gallery_id)
            .append_pair("start", &(usize::from(page) * PER_PAGE).to_string())
            .append_pair("count", &PER_PAGE.to_string())
            .append_pair("object", &gallery.object);
        let root: Value = serde_json::from_slice(&bounded_get(client, url).await?)?;
        let count = read_posts(&root, gallery, &mut seen, &mut records)?;
        if count < PER_PAGE { break; }
    }
    if records.is_empty() { return Err("ComicVine gallery has no trusted image originals".into()); }
    Ok(records)
}

pub async fn resolve(client: &Client, gallery: Gallery)
    -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>>
{
    let base = Url::parse("https://comicvine.gamespot.com/")?;
    Ok(at_base(client, &base, &gallery).await?.into_iter().map(|(m,_)| m).collect())
}

pub async fn resolve_messages(client: &Client, gallery: Gallery)
    -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>>
{
    let base = Url::parse("https://comicvine.gamespot.com/")?;
    let mut out = Vec::new();
    for (item, metadata) in at_base(client, &base, &gallery).await? {
        out.push(ExtractorMessage::Directory { metadata: metadata.clone() });
        out.push(ExtractorMessage::Url { url: item.url, metadata });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::TcpListener};

    #[test]
    fn gallery_routes_validate_public_origin_and_id() {
        assert_eq!(match_url("https://comicvine.gamespot.com/covers/123-45/images/").unwrap().object,"123-45");
        for bad in [
            "https://evilcomicvine.gamespot.com/covers/123-45/images/",
            "https://comicvine.gamespot.com/covers/123/images/",
            "https://comicvine.gamespot.com/covers/123-45/images/?token=1",
            "https://comicvine.gamespot.com/covers/123-45/../secret",
            "https://user:pass@comicvine.gamespot.com/covers/123-45/images/",
            "https://comicvine.gamespot.com:8443/covers/123-45/images/",
        ] { assert!(match_url(bad).is_none(), "{bad}"); }
    }

    #[test]
    fn source_tags_dates_media_and_duplicate_images_are_bounded() {
        let gallery = match_url("https://comicvine.gamespot.com/covers/123-45/images/").unwrap();
        let data = serde_json::json!({"images":[
            {"id":42,"original":"https://comicvine1.cbsistatic.com/uploads/original/1/42/cover%20one.jpg","dateCreated":"Mon, Jan 01 2024","tags":[{"name":"Cover"},{"name":""}]},
            {"id":42,"original":"https://comicvine1.cbsistatic.com/uploads/original/1/42/cover%20one.jpg"},
            {"id":43,"original":"https://comicvine.gamespot.com/a/uploads/original/2/43/next.png"},
            {"id":44,"original":"https://evilcomicvine.com/a/uploads/original/bad.png"}
        ]});
        let mut records=Vec::new();
        assert_eq!(read_posts(&data, &gallery, &mut HashSet::new(), &mut records).unwrap(),4);
        assert_eq!(records.len(),2);
        assert_eq!(records[0].0.filename,"42_cover one.jpg");
        assert_eq!(records[0].1["tags"],serde_json::json!(["Cover"]));
        assert_eq!(records[0].1["date_source"],"Mon, Jan 01 2024");
        assert_eq!(records[1].1["num"],2);
        for bad in [
            "https://comicvine.gamespot.com/a/uploads/original/%2fsecret.jpg",
            "https://comicvine.gamespot.com/a/uploads/original/%5csecret.jpg",
            "https://comicvine.gamespot.com/a/uploads/original/%FF.jpg",
        ] { assert!(trusted_original(bad).is_none(),"{bad}"); }
    }

    #[tokio::test]
    async fn public_gallery_and_api_use_two_bounded_local_gets() {
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr=listener.local_addr().unwrap();
        let server=tokio::spawn(async move {
            for (path,body) in [
                ("/covers/123-45/images/",r#"<div data-gallery-id="g12">"#),
                ("/js/image-data.json?images=g12&start=0&count=1000&object=123-45",
                    r#"{"images":[{"id":101,"original":"https://comicvine.gamespot.com/a/uploads/original/101.jpg"}]}"#),
            ] {
                let (mut stream,_)=listener.accept().await.unwrap();
                let mut request=[0u8;4096];
                let n=stream.read(&mut request).await.unwrap();
                assert!(String::from_utf8_lossy(&request[..n]).starts_with(&format!("GET {path} HTTP/1.1")));
                let reply=format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());
                stream.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let base=Url::parse(&format!("http://{addr}/")).unwrap();
        let gallery=match_url("https://comicvine.gamespot.com/covers/123-45/images/").unwrap();
        let images=at_base(&Client::new(),&base,&gallery).await.unwrap();
        assert_eq!(images.len(),1);
        assert_eq!(images[0].0.filename,"101_101.jpg");
        server.await.unwrap();
    }
}
