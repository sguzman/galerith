//! Guarded public Are.na channel images and attachments (pinned arena.py).
use crate::{message::{media_metadata, ExtractorMessage, Metadata}, MediaItem};
use reqwest::Client;
use serde_json::Value;
use std::{collections::HashSet, error::Error, time::Duration};
use url::Url;

const MAX_JSON: usize = 2 * 1024 * 1024;
const MAX_PAGE: usize = 100;
const MAX_PAGES: u16 = 4;
const MAX_MEDIA: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Channel(String);

fn slug(s: &str) -> bool {
    (1..=100).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub fn match_url(raw: &str) -> Option<Channel> {
    let candidate = if raw.starts_with("are.na/") || raw.starts_with("www.are.na/") {
        format!("https://{raw}")
    } else { raw.to_owned() };
    let lowered = candidate.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "/../", "/./"].iter().any(|s| lowered.contains(s)) { return None; }
    let url = Url::parse(&candidate).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !matches!(url.host_str(), Some("are.na" | "www.are.na"))
        || url.port().is_some() || !url.username().is_empty() || url.password().is_some()
        || url.query().is_some() || url.fragment().is_some() { return None; }
    let parts: Vec<_> = url.path_segments()?.collect();
    match parts.as_slice() {
        [owner, name] | [owner, name, ""] if slug(owner) && slug(name) => Some(Channel((*name).into())),
        _ => None,
    }
}

async fn request_json(client: &Client, url: Url) -> Result<Value, Box<dyn Error + Send + Sync>> {
    let mut response = client.get(url.clone()).send().await?.error_for_status()?;
    if response.url().origin() != url.origin() { return Err("Are.na API left its source origin".into()); }
    if response.content_length().is_some_and(|n| n > MAX_JSON as u64) {
        return Err("Are.na public response exceeds JSON cap".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_JSON.saturating_sub(bytes.len()) {
            return Err("Are.na public response exceeds JSON cap".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(serde_json::from_slice(&bytes)?)
}

fn source_media(raw: &str) -> Option<(String, String)> {
    let low = raw.to_ascii_lowercase();
    if ["/../", "/./", "%2f", "%5c", "%00"].iter().any(|bad| low.contains(bad)) { return None; }
    let url = Url::parse(raw).ok()?;
    let host = url.host_str()?;
    if url.scheme() != "https" || !(host == "are.na" || host.ends_with(".are.na"))
        || url.port().is_some() || !url.username().is_empty() || url.password().is_some()
        || url.query().is_some() || url.fragment().is_some() { return None; }
    let name = url.path_segments()?.next_back()?;
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    if !matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif" | "pdf" | "mp3" | "mp4" | "zip") { return None; }
    Some((url.to_string(), ext))
}

fn candidate(block: &Value) -> Option<&str> {
    if let Some(attachment) = block.get("attachment").filter(|v| !v.is_null()) {
        return attachment.get("url").and_then(Value::as_str);
    }
    let image = block.get("image")?;
    for format in ["original", "display", "large"] {
        if let Some(v) = image.get(format).filter(|v| !v.is_null()) {
            return v.get("url").and_then(Value::as_str);
        }
    }
    None
}

fn append_contents(
    data: &Value, channel: &Channel, details: &Value, seen: &mut HashSet<String>,
    output: &mut Vec<(MediaItem, Metadata)>, ordinal: &mut usize,
) -> Result<usize, Box<dyn Error + Send + Sync>> {
    let blocks = data.get("contents").and_then(Value::as_array).ok_or("Are.na contents array missing")?;
    if blocks.len() > MAX_PAGE { return Err("Are.na contents exceeds pinned page size".into()); }
    for block in blocks {
        let num = *ordinal;
        *ordinal += 1;
        let Some((url, ext)) = candidate(block).and_then(source_media) else { continue };
        if !seen.insert(url.clone()) { continue; }
        if output.len() >= MAX_MEDIA { return Err("Are.na public channel media cap exceeded".into()); }
        let block_id = block.get("id").and_then(Value::as_u64);
        let filename = match block_id {
            Some(id) => format!("{num:03}_{id}.{ext}"),
            None => format!("{num:03}.{ext}"),
        };
        let item = MediaItem { url, filename, extractor: "arena", post_id: block_id };
        let mut metadata = media_metadata(&item);
        metadata.insert("subcategory".into(), "channel".into());
        metadata.insert("channel_slug".into(), channel.0.clone().into());
        metadata.insert("num".into(), num.into());
        metadata.insert("extension".into(), ext.into());
        metadata.insert("block".into(), block.clone());
        if let Some(source) = block.get("source") {
            metadata.insert("source".into(), source.clone());
        }
        if let Some(id) = details.get("id").and_then(Value::as_u64) {
            metadata.insert("channel_id".into(), id.into());
        }
        if let Some(total) = details.get("length").and_then(Value::as_u64) {
            metadata.insert("count".into(), total.into());
        }
        for key in ["user", "owner"] {
            if let Some(v) = details.get(key) { metadata.insert(key.into(), v.clone()); }
        }
        let mut channel_detail = details.clone();
        if let Some(object) = channel_detail.as_object_mut() {
            object.remove("contents");
            object.remove("user");
            object.remove("owner");
        }
        metadata.insert("channel".into(), channel_detail);
        output.push((item, metadata));
    }
    Ok(blocks.len())
}

async fn records_at(
    client: &Client, api: &Url, channel: &Channel, max_pages: u16,
) -> Result<Vec<(MediaItem, Metadata)>, Box<dyn Error + Send + Sync>> {
    let details = request_json(client, api.join(&format!("channels/{}", channel.0))?).await?;
    if details.get("id").and_then(Value::as_u64).is_none() {
        return Err("Are.na public channel ID missing".into());
    }
    let mut seen = HashSet::new();
    let mut output = Vec::new();
    let mut ordinal = 1_usize;
    for page in 1..=max_pages.clamp(1, MAX_PAGES) {
        if page > 1 { tokio::time::sleep(Duration::from_secs(1)).await; }
        let mut endpoint = api.join(&format!("channels/{}/contents", channel.0))?;
        endpoint.query_pairs_mut().append_pair("page", &page.to_string()).append_pair("per", "100");
        let data = request_json(client, endpoint).await?;
        let count = append_contents(&data, channel, &details, &mut seen, &mut output, &mut ordinal)?;
        if count < MAX_PAGE { break; }
    }
    Ok(output)
}

pub async fn resolve(client: &Client, channel: Channel, max_pages: u16)
    -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    Ok(records_at(client, &Url::parse("https://api.are.na/v2/")?, &channel, max_pages)
        .await?.into_iter().map(|(item, _)| item).collect())
}

pub async fn resolve_messages(client: &Client, channel: Channel, max_pages: u16)
    -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let mut result = Vec::new();
    for (item, metadata) in records_at(client, &Url::parse("https://api.are.na/v2/")?, &channel, max_pages).await? {
        result.push(ExtractorMessage::Directory { metadata: metadata.clone() });
        result.push(ExtractorMessage::Url { url: item.url, metadata });
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::TcpListener};

    #[test]
    fn routes_reject_foreign_hosts_queries_and_path_escapes() {
        assert_eq!(match_url("https://are.na/alice/cassette-futurism").unwrap().0, "cassette-futurism");
        assert_eq!(match_url("www.are.na/alice/cassette-futurism/").unwrap().0, "cassette-futurism");
        for bad in ["https://fakeare.na/alice/cassette-futurism",
            "https://are.na/alice/cassette-futurism?secret=1",
            "https://are.na:8443/alice/cassette-futurism",
            "https://are.na/alice/../secret",
            "https://bob:pass@are.na/alice/cassette-futurism"] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn source_blocks_choose_attachments_then_image_renditions_and_deduplicate() {
        let channel = Channel("collection".into());
        let details = serde_json::json!({"id":42,"length":5,"title":"Archive","user":{"id":3}});
        let data = serde_json::json!({"contents":[
            {"id":10,"attachment":{"url":"https://attachments.are.na/10/book.pdf"},"image":{"original":{"url":"https://images.are.na/10/ignored.jpg"}}},
            {"id":11,"image":{"display":{"url":"https://images.are.na/11/large.png"}}},
            {"id":12,"image":{"large":{"url":"https://images.are.na/12/last.jpg"}}},
            {"id":13,"image":null},
            {"id":14,"attachment":{"url":"https://evil-are.na/14/bad.zip"}},
            {"id":15,"attachment":{"url":"https://attachments.are.na/10/book.pdf"}}
        ]});
        let mut output=Vec::new();
        let mut ordinal=1;
        assert_eq!(append_contents(&data,&channel,&details,&mut HashSet::new(),&mut output,&mut ordinal).unwrap(),6);
        assert_eq!(output.len(),3);
        assert_eq!(output[0].0.filename,"001_10.pdf");
        assert_eq!(output[1].0.filename,"002_11.png");
        assert_eq!(output[2].1["num"],3);
        assert_eq!(output[0].1["channel"]["title"],"Archive");
        assert_eq!(output[0].1["user"]["id"],3);
        assert_eq!(output[0].1["block"]["id"],10);
        assert!(source_media("https://169.254.169.254/private.jpg").is_none());
    }

    #[tokio::test]
    async fn public_channel_and_contents_make_two_bounded_api_requests() {
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr=listener.local_addr().unwrap();
        let server=tokio::spawn(async move {
            for (route,body) in [
                ("/v2/channels/collection",r#"{"id":42,"title":"Archive","length":1,"user":{"full_name":"Alice"}}"#),
                ("/v2/channels/collection/contents?page=1&per=100",r#"{"contents":[{"id":13,"image":{"original":{"url":"https://images.are.na/13/original.jpg"}}}]}"#)
            ] {
                let (mut socket,_)=listener.accept().await.unwrap();
                let mut req=[0u8;4096];
                let n=socket.read(&mut req).await.unwrap();
                assert!(String::from_utf8_lossy(&req[..n]).starts_with(&format!("GET {route} HTTP/1.1")));
                let reply=format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let api=Url::parse(&format!("http://{addr}/v2/")).unwrap();
        let result=records_at(&Client::new(),&api,&Channel("collection".into()),4).await.unwrap();
        assert_eq!(result.len(),1);
        assert_eq!(result[0].0.filename,"001_13.jpg");
        assert_eq!(result[0].1["user"]["full_name"],"Alice");
        server.await.unwrap();
    }
}
