//! Guarded public Piczel gallery, folder and image preview from pinned piczel.py.
use crate::{
    message::{media_metadata, ExtractorMessage, Metadata},
    safe_filename, MediaItem,
};
use percent_encoding::percent_decode_str;
use reqwest::Client;
use serde_json::Value;
use std::{error::Error, time::Duration};
use url::Url;

const MAX_JSON: usize = 2 * 1024 * 1024;
const MAX_PAGES: u16 = 4;
const MAX_POSTS_PER_PAGE: usize = 48;
const MAX_MULTI_IMAGES: usize = 16;
const MAX_MEDIA: usize = 160;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    User(String),
    Folder { user: String, id: String },
    Image(String),
}

fn safe_user(s: &str) -> bool {
    (1..=96).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
fn safe_id(s: &str) -> bool {
    (1..=16).contains(&s.len())
        && s.bytes().all(|b| b.is_ascii_digit())
        && s.parse::<u64>().ok().is_some_and(|n| n > 0)
}

pub fn match_url(raw: &str) -> Option<Target> {
    let lower = raw.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "/../", "/./"]
        .iter()
        .any(|v| lower.contains(v))
        || raw.contains('%')
    {
        return None;
    }
    let url = Url::parse(raw).ok()?;
    if !matches!(url.scheme(), "https" | "http")
        || !matches!(url.host_str(), Some("piczel.tv" | "www.piczel.tv"))
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
        ["gallery", "image", id] | ["gallery", "image", id, ""] if safe_id(id) => {
            Some(Target::Image((*id).into()))
        }
        ["gallery", user, id] | ["gallery", user, id, ""] if safe_user(user) && safe_id(id) => {
            Some(Target::Folder {
                user: (*user).into(),
                id: (*id).into(),
            })
        }
        ["gallery", user] | ["gallery", user, ""] if safe_user(user) => {
            Some(Target::User((*user).into()))
        }
        _ => None,
    }
}

fn source_id(value: &Value) -> Option<String> {
    value.as_u64().map(|v| v.to_string()).or_else(|| {
        value
            .as_str()
            .filter(|s| {
                (1..=64).contains(&s.len())
                    && s.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
            })
            .map(ToOwned::to_owned)
    })
}

fn source_text<'a>(v: &'a Value, size: usize) -> Option<&'a str> {
    v.as_str()
        .filter(|s| s.len() <= size && !s.chars().any(char::is_control))
}

fn original(raw: &str) -> Option<(String, String)> {
    let url = Url::parse(raw).ok()?;
    let host = url.host_str()?;
    if raw.len() > 2048
        || url.scheme() != "https"
        || !(host == "piczel.tv" || host.ends_with(".piczel.tv"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let mut last = None;
    for segment in url.path_segments()? {
        let part = percent_decode_str(segment).decode_utf8().ok()?;
        if part == "."
            || part == ".."
            || part.contains('/')
            || part.contains('\\')
            || part.contains('%')
            || part.chars().any(char::is_control)
        {
            return None;
        }
        last = Some(part.into_owned());
    }
    let name = last?;
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    if !matches!(
        ext.as_str(),
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif" | "mp4" | "webm"
    ) {
        return None;
    }
    let safe = safe_filename(&name);
    if safe.is_empty() || matches!(safe.as_str(), "." | "..") {
        return None;
    }
    Some((url.to_string(), ext))
}

enum Record {
    Directory(Metadata),
    Media(MediaItem, Metadata),
}

fn post_meta(post: &Value, category: &str) -> Result<Metadata, Box<dyn Error + Send + Sync>> {
    let id = post
        .get("id")
        .and_then(source_id)
        .ok_or("Piczel public post has no source-backed id")?;
    let mut meta = Metadata::new();
    meta.insert("category".into(), "piczel".into());
    meta.insert("subcategory".into(), category.into());
    meta.insert("id".into(), id.into());
    if let Some(title) = post.get("title").and_then(|v| source_text(v, 200)) {
        meta.insert("title".into(), title.into());
    }
    if let Some(date) = post.get("created_at").and_then(|v| source_text(v, 60)) {
        meta.insert("date_source".into(), date.into());
    }
    if let Some(user) = post
        .pointer("/user/username")
        .and_then(|v| source_text(v, 96))
    {
        meta.insert("username".into(), user.into());
    }
    if let Some(folder) = post.get("folder") {
        if let Some(id) = folder.get("id").and_then(source_id) {
            meta.insert("folder_id".into(), id.into());
        }
        if let Some(name) = folder.get("name").and_then(|v| source_text(v, 200)) {
            meta.insert("folder_name".into(), name.into());
        }
    }
    if let Some(tags) = post.get("tags").and_then(Value::as_array) {
        if tags.len() > 64 {
            return Err("Piczel post has too many source tags".into());
        }
        let names: Vec<_> = tags
            .iter()
            .filter_map(|tag| tag.get("title").and_then(|v| source_text(v, 80)))
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned)
            .collect();
        meta.insert("tags".into(), Value::from(names));
    }
    Ok(meta)
}

fn append_media(
    post: &Value,
    variant: Option<&Value>,
    index: usize,
    base: &Metadata,
    out: &mut Vec<Record>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let object = variant.unwrap_or(post);
    let raw = object
        .pointer("/image/url")
        .and_then(Value::as_str)
        .ok_or("Piczel post variation lacks original URL")?;
    let (url, extension) =
        original(raw).ok_or("Piczel original was not a trusted first-party media file")?;
    let id = base
        .get("id")
        .and_then(Value::as_str)
        .ok_or("Piczel source post id missing from metadata")?;
    let title = base.get("title").and_then(Value::as_str).unwrap_or(id);
    if out
        .iter()
        .filter(|v| matches!(v, Record::Media(..)))
        .count()
        >= MAX_MEDIA
    {
        return Err("Piczel gallery exceeds bounded media count".into());
    }
    let filename = safe_filename(&format!("piczel_{id}_{title}_{index:02}.{extension}"));
    let item = MediaItem {
        url,
        filename,
        extractor: "piczel",
        post_id: id.parse().ok(),
    };
    let mut metadata = media_metadata(&item);
    for (key, value) in base {
        metadata.insert(key.clone(), value.clone());
    }
    metadata.insert("num".into(), index.into());
    metadata.insert("extension".into(), extension.into());
    if let Some(variant) = variant {
        if let Some(v_id) = variant.get("id").and_then(source_id) {
            metadata.insert("variation_id".into(), v_id.into());
        }
    }
    out.push(Record::Media(item, metadata));
    Ok(())
}

fn append_post(
    post: &Value,
    category: &str,
    out: &mut Vec<Record>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut meta = post_meta(post, category)?;
    let multiple = post.get("multi").and_then(Value::as_bool).unwrap_or(false);
    let variants = if multiple {
        let images = post
            .get("images")
            .and_then(Value::as_array)
            .ok_or("Piczel multi-image post lacks its images array")?;
        if images.len() > MAX_MULTI_IMAGES {
            return Err("Piczel post exceeds multi-image cap".into());
        }
        Some(images)
    } else {
        None
    };
    meta.insert("count".into(), variants.map_or(1_usize, Vec::len).into());
    out.push(Record::Directory(meta.clone()));
    if let Some(images) = variants {
        for (num, variant) in images.iter().enumerate() {
            append_media(post, Some(variant), num, &meta, out)?;
        }
    } else {
        append_media(post, None, 0, &meta, out)?;
    }
    Ok(())
}

fn append_page(
    root: &Value,
    category: &str,
    out: &mut Vec<Record>,
    current: u16,
) -> Result<Option<u16>, Box<dyn Error + Send + Sync>> {
    let posts = root
        .get("data")
        .and_then(Value::as_array)
        .ok_or("Piczel paginated response lacks source data array")?;
    if posts.len() > MAX_POSTS_PER_PAGE {
        return Err("Piczel page exceeds bounded post count".into());
    }
    for post in posts {
        append_post(post, category, out)?;
    }
    let next = root
        .pointer("/meta/next_page")
        .filter(|v| !v.is_null())
        .and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok()));
    match next {
        None => Ok(None),
        Some(page) if page == u64::from(current + 1) && page <= u64::from(MAX_PAGES) => {
            Ok(Some(page as u16))
        }
        _ => Err("Piczel source API pagination skipped or exceeded bound".into()),
    }
}

async fn read_json(client: &Client, endpoint: Url) -> Result<Value, Box<dyn Error + Send + Sync>> {
    let mut response = client
        .get(endpoint.clone())
        .send()
        .await?
        .error_for_status()?;
    if response.url().origin() != endpoint.origin() {
        return Err("Piczel source API redirected outside its origin".into());
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_JSON as u64)
    {
        return Err("Piczel source JSON exceeds bounded response".into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_JSON.saturating_sub(body.len()) {
            return Err("Piczel source JSON exceeds bounded response".into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(serde_json::from_slice(&body)?)
}

async fn records_at(
    client: &Client,
    api: &Url,
    target: &Target,
    max_pages: u16,
) -> Result<Vec<Record>, Box<dyn Error + Send + Sync>> {
    let (url_path, category, paginated) = match target {
        Target::User(user) => (format!("api/users/{user}/gallery"), "user", true),
        Target::Folder { id, .. } => (format!("api/gallery/folder/{id}"), "folder", true),
        Target::Image(id) => (format!("api/gallery/{id}"), "image", false),
    };
    let mut output = Vec::new();
    if !paginated {
        append_post(
            &read_json(client, api.join(&url_path)?).await?,
            category,
            &mut output,
        )?;
    } else {
        let mut page = 1;
        for iteration in 0..max_pages.clamp(1, MAX_PAGES) {
            if iteration > 0 {
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            let mut endpoint = api.join(&url_path)?;
            endpoint
                .query_pairs_mut()
                .append_pair("page", &page.to_string());
            let root = read_json(client, endpoint).await?;
            let Some(next) = append_page(&root, category, &mut output, page)? else {
                break;
            };
            page = next;
        }
    }
    Ok(output)
}

pub async fn resolve(
    client: &Client,
    target: Target,
    max_pages: u16,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let records = records_at(
        client,
        &Url::parse("https://piczel.tv/")?,
        &target,
        max_pages,
    )
    .await?;
    Ok(records
        .into_iter()
        .filter_map(|r| match r {
            Record::Media(item, _) => Some(item),
            _ => None,
        })
        .collect())
}

pub async fn resolve_messages(
    client: &Client,
    target: Target,
    max_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let records = records_at(
        client,
        &Url::parse("https://piczel.tv/")?,
        &target,
        max_pages,
    )
    .await?;
    Ok(records
        .into_iter()
        .map(|record| match record {
            Record::Directory(metadata) => ExtractorMessage::Directory { metadata },
            Record::Media(item, metadata) => ExtractorMessage::Url {
                url: item.url,
                metadata,
            },
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    #[test]
    fn matches_pinned_public_user_folder_and_single_post_url_forms() {
        assert_eq!(
            match_url("https://piczel.tv/gallery/artist"),
            Some(Target::User("artist".into()))
        );
        assert_eq!(
            match_url("https://www.piczel.tv/gallery/artist/123"),
            Some(Target::Folder {
                user: "artist".into(),
                id: "123".into()
            })
        );
        assert_eq!(
            match_url("https://piczel.tv/gallery/image/456"),
            Some(Target::Image("456".into()))
        );
        for bad in [
            "https://fakepiczel.tv/gallery/artist",
            "https://piczel.tv/gallery/image/notanid",
            "https://piczel.tv/gallery/artist/0",
            "https://piczel.tv/gallery/../private",
            "https://user:pass@piczel.tv/gallery/artist",
            "https://piczel.tv:8443/gallery/artist",
            "https://piczel.tv/gallery/artist?token=secret",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn source_multi_post_directory_and_url_events_keep_order_and_identity() {
        let post = serde_json::json!({
            "id":1001,"multi":true,"title":"Aurora","created_at":"2026-02-03T08:00:00Z",
            "user":{"username":"artist"},"folder":{"id":5,"name":"Sketchbook"},
            "tags":[{"title":"night"},{"title":""},{"title":"landscape"}],
            "images":[
                {"id":3001,"image":{"url":"https://cdn.piczel.tv/1.jpg"}},
                {"id":3002,"image":{"url":"https://cdn.piczel.tv/2.png"}}
            ]
        });
        let mut records = Vec::new();
        append_post(&post, "folder", &mut records).unwrap();
        assert_eq!(records.len(), 3);
        let Record::Directory(dir) = &records[0] else {
            panic!("missing directory")
        };
        assert_eq!(dir["count"], 2);
        assert_eq!(dir["folder_name"], "Sketchbook");
        assert_eq!(dir["tags"][1], "landscape");
        let Record::Media(first, meta) = &records[1] else {
            panic!("missing source first image")
        };
        assert_eq!(first.filename, "piczel_1001_Aurora_00.jpg");
        assert_eq!(meta["num"], 0);
        assert_eq!(meta["variation_id"], "3001");
        assert_eq!(meta["id"], "1001");
        let Record::Media(second, _) = &records[2] else {
            panic!("missing source second image")
        };
        assert_eq!(second.url, "https://cdn.piczel.tv/2.png");
    }

    #[test]
    fn single_image_preserves_date_and_rejects_foreign_media() {
        let post = serde_json::json!({"id":9,"title":"Painting","multi":false,"image":{"url":"https://cdn.piczel.tv/9.png"},"created_at":"2026-10-01T00:00:00Z"});
        let mut result = Vec::new();
        append_post(&post, "image", &mut result).unwrap();
        assert_eq!(result.len(), 2);
        let Record::Media(_, metadata) = &result[1] else {
            panic!("no file")
        };
        assert_eq!(metadata["date_source"], "2026-10-01T00:00:00Z");
        assert_eq!(metadata["count"], 1);
        let forged = serde_json::json!({"id":9,"multi":false,"image":{"url":"https://evil-piczel.tv/9.png"}});
        assert!(append_post(&forged, "image", &mut Vec::new()).is_err());
        let encoded = serde_json::json!({"id":9,"multi":false,"image":{"url":"https://cdn.piczel.tv/%2fsecret.png"}});
        assert!(append_post(&encoded, "image", &mut Vec::new()).is_err());
    }

    #[test]
    fn pagination_rejects_cursor_skips_and_bounds() {
        let mut records = Vec::new();
        let empty = serde_json::json!({"data":[],"meta":{"next_page":2}});
        assert_eq!(
            append_page(&empty, "user", &mut records, 1).unwrap(),
            Some(2)
        );
        let jump = serde_json::json!({"data":[],"meta":{"next_page":3}});
        assert!(append_page(&jump, "user", &mut records, 1).is_err());
        let bound = serde_json::json!({"data":[],"meta":{"next_page":5}});
        assert!(append_page(&bound, "user", &mut records, 4).is_err());
    }

    #[tokio::test]
    async fn public_user_gallery_two_json_pages_keep_multi_media_order() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (route, body) in [
                (
                    "/api/users/artist/gallery?page=1",
                    r#"{"data":[{"id":10,"title":"A","multi":false,"image":{"url":"https://cdn.piczel.tv/10.jpg"}}],"meta":{"next_page":2}}"#,
                ),
                (
                    "/api/users/artist/gallery?page=2",
                    r#"{"data":[{"id":11,"title":"B","multi":true,"images":[{"image":{"url":"https://cdn.piczel.tv/11a.jpg"}},{"image":{"url":"https://cdn.piczel.tv/11b.png"}}]}],"meta":{"next_page":null}}"#,
                ),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0u8; 4096];
                let n = socket.read(&mut request).await.unwrap();
                assert!(String::from_utf8_lossy(&request[..n])
                    .starts_with(&format!("GET {route} HTTP/1.1")));
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let records = records_at(&Client::new(), &base, &Target::User("artist".into()), 4)
            .await
            .unwrap();
        assert_eq!(records.len(), 5);
        let urls = records
            .iter()
            .filter_map(|r| match r {
                Record::Media(item, _) => Some(item.url.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            urls,
            [
                "https://cdn.piczel.tv/10.jpg",
                "https://cdn.piczel.tv/11a.jpg",
                "https://cdn.piczel.tv/11b.png"
            ]
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn folder_and_image_modes_fetch_exact_first_party_endpoints() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (path, body) in [
                (
                    "/api/gallery/folder/55?page=1",
                    r#"{"data":[{"id":91,"title":"Folder","multi":false,"image":{"url":"https://cdn.piczel.tv/91.png"}}],"meta":{"next_page":null}}"#,
                ),
                (
                    "/api/gallery/77",
                    r#"{"id":77,"title":"Single","multi":false,"image":{"url":"https://cdn.piczel.tv/77.jpg"}}"#,
                ),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buffer = [0u8; 4096];
                let n = socket.read(&mut buffer).await.unwrap();
                assert!(String::from_utf8_lossy(&buffer[..n])
                    .starts_with(&format!("GET {path} HTTP/1.1")));
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        assert_eq!(
            records_at(
                &Client::new(),
                &base,
                &Target::Folder {
                    user: "artist".into(),
                    id: "55".into()
                },
                2
            )
            .await
            .unwrap()
            .len(),
            2
        );
        assert_eq!(
            records_at(&Client::new(), &base, &Target::Image("77".into()), 2)
                .await
                .unwrap()
                .len(),
            2
        );
        server.await.unwrap();
    }
}
