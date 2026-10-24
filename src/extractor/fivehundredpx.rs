//! Guarded 500px public single-photo original preview from pinned 500px.py.
use crate::{
    message::{media_metadata, ExtractorMessage, Metadata},
    safe_filename, MediaItem,
};
use reqwest::Client;
use serde_json::Value;
use std::error::Error;
use url::Url;

const MAX_JSON: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Photo {
    id: u64,
}

pub fn match_url(raw: &str) -> Option<Photo> {
    let lower = raw.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "/../", "/./"]
        .iter()
        .any(|x| lower.contains(x))
    {
        return None;
    }
    let url = Url::parse(raw).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !matches!(url.host_str(), Some("500px.com" | "web.500px.com"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let parts: Vec<_> = url.path_segments()?.collect();
    let id = match parts.as_slice() {
        ["photo", id] | ["photo", id, ""] | ["photo", id, _] | ["photo", id, _, ""] => *id,
        _ => return None,
    };
    if id.is_empty() || id.len() > 18 || !id.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let value = id.parse::<u64>().ok()?;
    (value > 0).then_some(Photo { id: value })
}

fn media_url(raw: &str) -> Option<String> {
    let url = Url::parse(raw).ok()?;
    let host = url.host_str()?;
    let origin_ok = ["500px.com", "500px.org", "500pxcdn.com"]
        .iter()
        .any(|root| host == *root || host.ends_with(&format!(".{root}")));
    if !origin_ok
        || url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.path().contains("..")
    {
        return None;
    }
    Some(url.into())
}

fn record(
    body: &Value,
    photo: &Photo,
) -> Result<(MediaItem, Metadata), Box<dyn Error + Send + Sync>> {
    let id = photo.id.to_string();
    let source = body
        .get("photos")
        .and_then(|p| p.get(&id))
        .ok_or("500px expanded photo API response lacks requested photo")?;
    let source_id = source
        .get("id")
        .and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok()))
        .ok_or("500px expanded photo lacks validated numeric ID")?;
    if source_id != photo.id {
        return Err("500px expanded photo ID does not match request".into());
    }
    let images = source
        .get("images")
        .and_then(Value::as_array)
        .ok_or("500px expanded photo lacks image renditions")?;
    if images.is_empty() || images.len() > 32 {
        return Err("500px expanded image list is outside bounds".into());
    }
    let original = images
        .last()
        .and_then(|v| v.get("url"))
        .and_then(Value::as_str)
        .and_then(media_url)
        .ok_or("500px original must use a trusted HTTPS image host")?;
    let ext = source
        .get("image_format")
        .and_then(Value::as_str)
        .ok_or("500px original has no image format")?
        .to_ascii_lowercase();
    if !(2..=5).contains(&ext.len()) || !ext.bytes().all(|c| c.is_ascii_alphanumeric()) {
        return Err("500px source image format invalid".into());
    }
    let name = source
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty() && name.len() <= 240 && !name.chars().any(char::is_control))
        .unwrap_or(&id);
    let cleaned = safe_filename(name);
    let item = MediaItem {
        url: original,
        filename: safe_filename(&format!("{id}_{cleaned}.{ext}")),
        extractor: "500px",
        post_id: Some(photo.id),
    };
    let mut metadata = media_metadata(&item);
    metadata.insert("subcategory".into(), "image".into());
    metadata.insert("id".into(), photo.id.into());
    metadata.insert("name".into(), cleaned.into());
    metadata.insert("extension".into(), ext.into());
    metadata.insert(
        "post_url".into(),
        format!("https://500px.com/photo/{id}").into(),
    );
    if let Some(username) = source
        .pointer("/user/username")
        .and_then(Value::as_str)
        .filter(|s| s.len() <= 100 && !s.chars().any(char::is_control))
    {
        metadata.insert("username".into(), username.into());
    }
    if let Some(tags) = source.get("tags").and_then(Value::as_array) {
        if tags.len() <= 80 {
            metadata.insert("tags".into(), Value::Array(tags.clone()));
        }
    }
    for key in ["width", "height", "views"] {
        if let Some(number) = source.get(key).and_then(Value::as_u64) {
            metadata.insert(key.into(), number.into());
        }
    }
    Ok((item, metadata))
}

async fn at_base(
    client: &Client,
    photo: &Photo,
    base: &Url,
) -> Result<(MediaItem, Metadata), Box<dyn Error + Send + Sync>> {
    let mut url = base.join("v1/photos")?;
    let id = photo.id.to_string();
    {
        let mut params = url.query_pairs_mut();
        for (key, value) in [
            ("expanded_user_info", "true"),
            ("include_tags", "true"),
            ("include_geo", "true"),
            ("include_equipment_info", "true"),
            ("vendor_photos", "true"),
            ("include_licensing", "true"),
            ("include_releases", "true"),
            ("liked_by", "1"),
            ("following_sample", "100"),
            ("image_size", "4096"),
            ("ids", id.as_str()),
        ] {
            params.append_pair(key, value);
        }
    }
    let mut response = client
        .get(url.clone())
        .header("Origin", "https://500px.com")
        .send()
        .await?
        .error_for_status()?;
    if response.url().origin() != url.origin() {
        return Err("500px expanded photo API redirected outside its origin".into());
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_JSON as u64)
    {
        return Err("500px API JSON exceeds response limit".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_JSON.saturating_sub(bytes.len()) {
            return Err("500px API JSON exceeds response limit".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    record(&serde_json::from_slice::<Value>(&bytes)?, photo)
}

pub async fn resolve(
    client: &Client,
    photo: Photo,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse("https://api.500px.com/")?;
    Ok(vec![at_base(client, &photo, &base).await?.0])
}

pub async fn resolve_messages(
    client: &Client,
    photo: Photo,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse("https://api.500px.com/")?;
    let (item, metadata) = at_base(client, &photo, &base).await?;
    Ok(vec![
        ExtractorMessage::Directory {
            metadata: metadata.clone(),
        },
        ExtractorMessage::Url {
            url: item.url,
            metadata,
        },
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    #[test]
    fn matches_only_public_single_photos_and_pinned_origins() {
        assert_eq!(
            match_url("https://500px.com/photo/12345/Golden-Landscape")
                .unwrap()
                .id,
            12345
        );
        assert_eq!(
            match_url("https://web.500px.com/photo/12345").unwrap().id,
            12345
        );
        for bad in [
            "https://fake500px.com/photo/12345",
            "https://500px.com/photo/0",
            "https://500px.com/photo/abc",
            "https://500px.com/photo/12345?secret=1",
            "https://500px.com:8443/photo/12345",
            "https://name:pass@500px.com/photo/12345",
            "https://500px.com/photo/12345/../../private",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn last_expanded_image_is_selected_with_source_backed_metadata() {
        let photo = match_url("https://500px.com/photo/12345").unwrap();
        let body = serde_json::json!({"photos":{"12345":{
            "id":12345,"name":"Amber","image_format":"jpg",
            "user":{"username":"alice"},"tags":["gold","light"],
            "images":[
                {"url":"https://img.500px.com/low.jpg"},
                {"url":"https://drscdn.500px.org/original.jpg"}
            ]
        }}});
        let (item, meta) = record(&body, &photo).unwrap();
        assert_eq!(item.filename, "12345_Amber.jpg");
        assert_eq!(item.url, "https://drscdn.500px.org/original.jpg");
        assert_eq!(meta["username"], "alice");
        assert_eq!(meta["tags"][0], "gold");
        let foreign = serde_json::json!({"photos":{"12345":{"id":12345,"image_format":"jpg","images":[{"url":"https://evil500px.com/secret.jpg"}]}}});
        assert!(record(&foreign, &photo).is_err());
    }

    #[tokio::test]
    async fn bounded_expanded_photo_api_fetches_one_local_source_request() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut input = [0u8; 8192];
            let n = socket.read(&mut input).await.unwrap();
            let req = String::from_utf8_lossy(&input[..n]).to_ascii_lowercase();
            assert!(req.starts_with("get /v1/photos?"));
            assert!(req.contains("ids=12345"));
            assert!(req.contains("image_size=4096"));
            assert!(req.contains("origin: https://500px.com"));
            let body = r#"{"photos":{"12345":{"id":12345,"name":"Sunset","image_format":"jpeg","images":[{"url":"https://img.500px.com/full.jpg"}]}}}"#;
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(reply.as_bytes()).await.unwrap();
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let photo = match_url("https://500px.com/photo/12345").unwrap();
        let (item, _) = at_base(&Client::new(), &photo, &base).await.unwrap();
        assert_eq!(item.filename, "12345_Sunset.jpeg");
        server.await.unwrap();
    }
}
