//! Independent public Imgur image/album/gallery preview from pinned imgur.py.
//! Private sessions, complete archive configuration and live differential
//! parity remain outside this conservative public Rust subset.
use crate::{
    message::{media_metadata, queued_url, ExtractorMessage, Metadata},
    MediaItem,
};
mod listing;
use reqwest::Client;
use serde_json::Value;
use std::error::Error;
use url::Url;

const MAX_JSON_BYTES: usize = 2 * 1024 * 1024;
const MAX_ALBUM_MEDIA: usize = 160;
const PUBLIC_CLIENT_ID: &str = "546c25a59c58ad7";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Image(String),
    Album(String),
    Gallery(String),
    Listing(listing::Listing),
}

fn valid_id(s: &str) -> bool {
    matches!(s.len(), 5 | 7) && s.bytes().all(|c| c.is_ascii_alphanumeric())
}

fn id_suffix(s: &str) -> Option<String> {
    let ending = s.rsplit('-').next()?;
    let raw = if valid_id(ending) {
        ending
    } else if ending.len() > 1 && b"sbtmlh".contains(&ending.as_bytes()[ending.len() - 1]) {
        &ending[..ending.len() - 1]
    } else {
        return None;
    };
    valid_id(raw).then(|| raw.to_owned())
}

pub fn match_url(raw: &str) -> Option<Target> {
    let lower = raw.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "/../", "/./"]
        .iter()
        .any(|s| lower.contains(s))
    {
        return None;
    }
    let url = Url::parse(raw).ok()?;
    let host = url.host_str()?;
    if !matches!(url.scheme(), "http" | "https")
        || !matches!(
            host,
            "imgur.com" | "www.imgur.com" | "i.imgur.com" | "m.imgur.com" | "imgur.io"
        )
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    if let Some(listing) = listing::match_url(&url) {
        return Some(Target::Listing(listing));
    }
    if url.query().is_some() {
        return None;
    }
    let segments: Vec<_> = url.path_segments()?.collect();
    if host == "i.imgur.com" {
        let [name] = segments.as_slice() else {
            return None;
        };
        let (id, ext) = name.rsplit_once('.')?;
        if !matches!(
            ext.to_ascii_lowercase().as_str(),
            "jpg" | "jpeg" | "png" | "gif" | "webp" | "mp4"
        ) {
            return None;
        }
        return id_suffix(id).map(Target::Image);
    }
    match segments.as_slice() {
        ["a", album] => id_suffix(album).map(Target::Album),
        ["gallery", id] | ["t", _, id] => id_suffix(id).map(Target::Gallery),
        ["r", _, id] | [id] => id_suffix(id).map(Target::Image),
        _ => None,
    }
}

fn media_record(
    value: &Value,
    album: Option<(&str, usize, usize)>,
) -> Option<(MediaItem, Metadata)> {
    let id = value.get("id")?.as_str()?;
    if !valid_id(id) {
        return None;
    }
    let ext = value.get("ext")?.as_str()?.to_ascii_lowercase();
    let ext = match ext.as_str() {
        "jpeg" => "jpg",
        "gif" if value.get("is_animated").and_then(Value::as_bool) == Some(true) => "mp4",
        "jpg" | "png" | "gif" | "webp" | "mp4" => ext.as_str(),
        _ => return None,
    };
    let url = format!("https://i.imgur.com/{id}.{ext}");
    let filename = if let Some((album_id, number, _)) = album {
        format!("imgur_{album_id}_{number:03}_{id}.{ext}")
    } else {
        format!("imgur_{id}.{ext}")
    };
    let item = MediaItem {
        url,
        filename,
        extractor: "imgur",
        post_id: None,
    };
    let mut meta = media_metadata(&item);
    meta.insert(
        "subcategory".into(),
        if album.is_some() { "album" } else { "image" }.into(),
    );
    meta.insert("media_id".into(), id.to_owned().into());
    meta.insert("extension".into(), ext.to_owned().into());
    if let Some((album_id, number, count)) = album {
        meta.insert("album_id".into(), album_id.to_owned().into());
        meta.insert("num".into(), number.into());
        meta.insert("count".into(), count.into());
    }
    Some((item, meta))
}

fn unwrap_data(value: &Value) -> &Value {
    value.get("data").unwrap_or(value)
}

fn decode_image(
    value: &Value,
    id: &str,
) -> Result<Vec<(MediaItem, Metadata)>, Box<dyn Error + Send + Sync>> {
    let root = unwrap_data(value);
    let primary = root
        .get("media")
        .and_then(Value::as_array)
        .and_then(|images| images.first())
        .unwrap_or(root);
    let record = media_record(primary, None)
        .ok_or("Imgur public image metadata lacks a supported original")?;
    if record.1.get("media_id").and_then(Value::as_str) != Some(id) {
        return Err("Imgur public image response did not match requested ID".into());
    }
    Ok(vec![record])
}

fn decode_album(
    value: &Value,
    id: &str,
) -> Result<Vec<(MediaItem, Metadata)>, Box<dyn Error + Send + Sync>> {
    let root = unwrap_data(value);
    if root
        .get("id")
        .and_then(Value::as_str)
        .is_some_and(|actual| actual != id)
    {
        return Err("Imgur public album identity mismatch".into());
    }
    let images = root
        .get("media")
        .and_then(Value::as_array)
        .ok_or("Imgur public album has no media collection")?;
    if images.len() > MAX_ALBUM_MEDIA {
        return Err("Imgur album exceeds the bounded public media limit".into());
    }
    let count = images.len();
    Ok(images
        .iter()
        .enumerate()
        .filter_map(|(i, image)| media_record(image, Some((id, i + 1, count))))
        .collect())
}

fn decode_gallery(
    value: &Value,
    id: &str,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let root = unwrap_data(value);
    if root
        .get("id")
        .and_then(Value::as_str)
        .is_some_and(|actual| actual != id)
    {
        return Err("Imgur public gallery identity mismatch".into());
    }
    let album = root
        .get("is_album")
        .and_then(Value::as_bool)
        .ok_or("Imgur gallery has no public album classification")?;
    let mut meta = Metadata::new();
    meta.insert("category".into(), "imgur".into());
    meta.insert("gallery_id".into(), id.to_owned().into());
    meta.insert("subcategory".into(), "gallery".into());
    let url = if album {
        format!("https://imgur.com/a/{id}")
    } else {
        format!("https://imgur.com/{id}")
    };
    Ok(vec![queued_url(url, meta)])
}

async fn public_json(
    client: &Client,
    base: &Url,
    path: &str,
    include: bool,
) -> Result<Value, Box<dyn Error + Send + Sync>> {
    let mut endpoint = base.join(path)?;
    if include {
        endpoint
            .query_pairs_mut()
            .append_pair("include", "media,tags,account");
    }
    let mut response = client
        .get(endpoint)
        .header("Authorization", format!("Client-ID {PUBLIC_CLIENT_ID}"))
        .send()
        .await?
        .error_for_status()?;
    if response
        .content_length()
        .is_some_and(|n| n > MAX_JSON_BYTES as u64)
    {
        return Err("Imgur public JSON exceeds the response limit".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_JSON_BYTES.saturating_sub(bytes.len()) {
            return Err("Imgur public JSON exceeds the response limit".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(serde_json::from_slice(&bytes)?)
}

async fn records_at_base(
    client: &Client,
    target: &Target,
    base: &Url,
) -> Result<Vec<(MediaItem, Metadata)>, Box<dyn Error + Send + Sync>> {
    match target {
        Target::Image(id) => {
            let value = public_json(client, base, &format!("post/v1/media/{id}"), true).await?;
            decode_image(&value, id)
        }
        Target::Album(id) => {
            let value = public_json(client, base, &format!("post/v1/albums/{id}"), true).await?;
            decode_album(&value, id)
        }
        Target::Gallery(_) => Err("Imgur gallery requires deferred typed child queue".into()),
        Target::Listing(_) => {
            Err("Imgur public listing requires deferred typed child queues".into())
        }
    }
}

fn messages_from_records(records: Vec<(MediaItem, Metadata)>) -> Vec<ExtractorMessage> {
    let mut events = Vec::with_capacity(records.len() * 2);
    for (item, meta) in records {
        events.push(ExtractorMessage::Directory {
            metadata: meta.clone(),
        });
        events.push(ExtractorMessage::Url {
            url: item.url,
            metadata: meta,
        });
    }
    events
}

pub async fn resolve(
    client: &Client,
    target: Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse("https://api.imgur.com/")?;
    Ok(records_at_base(client, &target, &base)
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
    let base = Url::parse("https://api.imgur.com/")?;
    resolve_messages_at_base(client, target, max_pages, &base).await
}

async fn resolve_messages_at_base(
    client: &Client,
    target: Target,
    max_pages: u16,
    base: &Url,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    match target {
        Target::Gallery(id) => {
            let value = public_json(client, &base, &format!("post/v1/posts/{id}"), false).await?;
            decode_gallery(&value, &id)
        }
        Target::Listing(listing) => {
            listing::resolve_messages(client, &base, listing, max_pages).await
        }
        other => Ok(messages_from_records(
            records_at_base(client, &other, &base).await?,
        )),
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
    fn pinned_public_url_families_have_strict_origins() {
        assert_eq!(
            match_url("https://imgur.com/abcde"),
            Some(Target::Image("abcde".into()))
        );
        assert_eq!(
            match_url("https://i.imgur.com/abcdefg.jpg"),
            Some(Target::Image("abcdefg".into()))
        );
        assert_eq!(
            match_url("https://imgur.com/a/abcde"),
            Some(Target::Album("abcde".into()))
        );
        assert_eq!(
            match_url("https://imgur.com/gallery/abcdefg"),
            Some(Target::Gallery("abcdefg".into()))
        );
        assert_eq!(
            match_url("https://imgur.com/r/pics/abcdefg"),
            Some(Target::Image("abcdefg".into()))
        );
        for invalid in [
            "https://evilimgur.com/abcdefg",
            "https://imgur.com/a/../../../private",
            "https://imgur.com/abcdefg?token=x",
            "https://user:pass@imgur.com/abcdefg",
            "https://imgur.com/a/abcd",
            "https://imgur.com/user/me",
            "https://i.imgur.com/abcdefg.exe",
        ] {
            assert!(match_url(invalid).is_none(), "{invalid}");
        }
    }

    #[test]
    fn pinned_public_images_and_albums_preserve_id_and_order() {
        let v = serde_json::json!({"id":"abcde", "media":[{"id":"abcde", "ext":"jpeg", "is_animated":false}]});
        let images = decode_image(&v, "abcde").unwrap();
        assert_eq!(images[0].0.url, "https://i.imgur.com/abcde.jpg");
        assert_eq!(images[0].1["media_id"], "abcde");
        let a = serde_json::json!({"id":"abcdefg","media":[
            {"id":"abcde","ext":"png"},
            {"id":"xyz12","ext":"gif","is_animated":true},
            {"id":"bogus","ext":"exe"}
        ]});
        let output = decode_album(&a, "abcdefg").unwrap();
        assert_eq!(output.len(), 2);
        assert_eq!(output[0].0.filename, "imgur_abcdefg_001_abcde.png");
        assert_eq!(output[1].0.url, "https://i.imgur.com/xyz12.mp4");
        assert_eq!(output[1].1["count"], 3);
        assert!(decode_album(&a, "wrong").is_err());
    }

    #[test]
    fn pinned_public_gallery_is_deferred_to_image_or_album() {
        let album =
            decode_gallery(&serde_json::json!({"id":"abcde", "is_album":true}), "abcde").unwrap();
        assert_eq!(
            serde_json::to_value(&album[0]).unwrap()["url"],
            "https://imgur.com/a/abcde"
        );
        let image = decode_gallery(
            &serde_json::json!({"id":"abcde", "is_album":false}),
            "abcde",
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(&image[0]).unwrap()["url"],
            "https://imgur.com/abcde"
        );
        assert!(decode_gallery(&serde_json::json!({"id":"abcde"}), "abcde").is_err());
    }

    #[tokio::test]
    async fn gallery_queue_traversal_fetches_public_album_and_preserves_media_order() {
        use crate::message::traverse_queue_job_with;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (path, json) in [
                (
                    "/post/v1/posts/abcdefg",
                    r#"{"data":{"id":"abcdefg","is_album":true}}"#,
                ),
                (
                    "/post/v1/albums/abcdefg?include=",
                    r#"{"data":{"id":"abcdefg","media":[{"id":"abcde","ext":"jpg"},{"id":"xyz12","ext":"png"}]}}"#,
                ),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buffer = [0u8; 4096];
                let count = socket.read(&mut buffer).await.unwrap();
                let request = String::from_utf8_lossy(&buffer[..count]);
                assert!(request.starts_with(&format!("GET {path}")), "{request}");
                assert!(
                    request.to_ascii_lowercase().contains("authorization: client-id "),
                    "{request}"
                );
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json}",
                    json.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });

        let base = Url::parse(&format!("http://{address}/")).unwrap();
        let client = Client::new();
        let parent = resolve_messages_at_base(
            &client,
            Target::Gallery("abcdefg".into()),
            1,
            &base,
        )
        .await
        .unwrap();
        assert!(matches!(&parent[0], ExtractorMessage::Queue { .. }));
        let expanded = traverse_queue_job_with(parent, 1, 4, |url| {
            let target = match_url(&url).expect("validated gallery child");
            resolve_messages_at_base(&client, target, 1, &base)
        })
        .await
        .unwrap();
        let urls = expanded
            .iter()
            .filter_map(|message| {
                if let ExtractorMessage::Url { url, .. } = message {
                    Some(url.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        assert_eq!(
            urls,
            ["https://i.imgur.com/abcde.jpg", "https://i.imgur.com/xyz12.png"]
        );
        assert_eq!(expanded.len(), 5);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn bounded_single_image_makes_one_authenticated_public_api_request() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let n = socket.read(&mut buf).await.unwrap();
            let request = String::from_utf8_lossy(&buf[..n]);
            assert!(
                request.starts_with("GET /post/v1/media/abcde?include="),
                "{request}"
            );
            assert!(
                request.contains("Authorization: Client-ID 546c25a59c58ad7")
                    || request.contains("authorization: Client-ID 546c25a59c58ad7"),
                "{request}"
            );
            let json = r#"{"id":"abcde","media":[{"id":"abcde","ext":"jpeg"}]}"#;
            let reply = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json}", json.len());
            socket.write_all(reply.as_bytes()).await.unwrap();
        });
        let root = Url::parse(&format!("http://{address}/")).unwrap();
        let result = records_at_base(&Client::new(), &Target::Image("abcde".into()), &root)
            .await
            .unwrap();
        assert_eq!(result[0].0.filename, "imgur_abcde.jpg");
        server.await.unwrap();
    }
}
