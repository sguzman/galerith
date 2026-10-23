//! Independent public Vogue PhotoVogue photographer-feed extractor preview.
//! The pinned upstream API exposes paginated photographer photos; complete
//! metadata, archive naming, CDN variations and live parity are outstanding.
use crate::{safe_filename, MediaItem};
use reqwest::Client;
use serde_json::Value;
use std::{collections::HashSet, error::Error};
use url::Url;

const MAX_JSON_BYTES: usize = 2 * 1024 * 1024;
const MAX_ITEMS_PER_PAGE: usize = 50;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Photographer { id: String },
}

pub fn match_url(input: &str) -> Option<Target> {
    let url = Url::parse(input).ok()?;
    if url.scheme() != "https" || !matches!(url.host_str(), Some("vogue.com" | "www.vogue.com")) {
        return None;
    }
    match url.path_segments()?.collect::<Vec<_>>().as_slice() {
        ["photovogue", "photographers", id]
            if !id.is_empty()
                && id.len() <= 18
                && id.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            Some(Target::Photographer {
                id: (*id).to_owned(),
            })
        }
        _ => None,
    }
}

fn photo_item(value: &Value) -> Option<(u64, MediaItem)> {
    let id = value
        .get("id")
        .and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse::<u64>().ok()))?;
    let raw = value.get("gallery_image")?.as_str()?;
    let url = Url::parse(raw).ok()?;
    let host = url.host_str()?;
    if url.scheme() != "https" || !(host == "vogue.com" || host.ends_with(".vogue.com")) {
        return None;
    }
    let original = url.path_segments()?.next_back()?;
    let extension = original.rsplit_once('.')?.1;
    if extension.is_empty()
        || extension.len() > 12
        || !extension.bytes().all(|b| b.is_ascii_alphanumeric())
    {
        return None;
    }
    let title = value.get("title").and_then(Value::as_str).unwrap_or("");
    if title.len() > 240 {
        return None;
    }
    let filename = if title.trim().is_empty() {
        format!("{id}.{extension}")
    } else {
        safe_filename(&format!("{id} {}.{extension}", title.trim()))
    };
    if matches!(filename.as_str(), "" | "." | "..") {
        return None;
    }
    Some((
        id,
        MediaItem {
            url: url.to_string(),
            filename,
            extractor: "photovogue",
            post_id: Some(id),
        },
    ))
}

async fn read_json(client: &Client, url: Url) -> Result<Value, Box<dyn Error + Send + Sync>> {
    let mut response = client.get(url).send().await?.error_for_status()?;
    if response
        .content_length()
        .is_some_and(|size| size > MAX_JSON_BYTES as u64)
    {
        return Err("PhotoVogue API JSON exceeds the response limit".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len().saturating_add(chunk.len()) > MAX_JSON_BYTES {
            return Err("PhotoVogue API JSON exceeds the response limit".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(serde_json::from_slice(&bytes)?)
}

async fn resolve_at_api_base(
    client: &Client,
    base: &Url,
    target: Target,
    max_pages: u16,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    match target {
        Target::Photographer { id } => {
            let mut results = Vec::new();
            let mut seen = HashSet::new();
            for page in 0..max_pages.clamp(1, 100) {
                if page > 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                }
                let mut endpoint = base.join("production/photos")?;
                endpoint
                    .query_pairs_mut()
                    .append_pair("count", "50")
                    .append_pair("order_by", "DESC")
                    .append_pair("page", &page.to_string())
                    .append_pair("photographer_id", &id);
                let response = read_json(client, endpoint).await?;
                let posts = response
                    .get("items")
                    .and_then(Value::as_array)
                    .ok_or("PhotoVogue response does not contain a photo list")?;
                if posts.len() > MAX_ITEMS_PER_PAGE {
                    return Err("PhotoVogue response contains more than 50 images per page".into());
                }
                for photo in posts {
                    if let Some((id, item)) = photo_item(photo) {
                        if seen.insert(id) {
                            results.push(item);
                        }
                    }
                }
                let has_next = response
                    .get("has_next")
                    .and_then(Value::as_bool)
                    .ok_or("PhotoVogue response is missing has_next")?;
                if !has_next || posts.is_empty() {
                    break;
                }
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
    resolve_at_api_base(client, &Url::parse("https://api.vogue.com/")?, target, max_pages).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_only_public_photovogue_photographer_identifiers() {
        assert_eq!(
            match_url("https://www.vogue.com/photovogue/photographers/12345"),
            Some(Target::Photographer {
                id: "12345".to_owned()
            })
        );
        for raw in [
            "http://vogue.com/photovogue/photographers/123",
            "https://vogue.com.evil.test/photovogue/photographers/123",
            "https://www.vogue.com/photovogue/photographers/abc",
            "https://www.vogue.com/photovogue/photographers/123/photos",
            "https://www.vogue.com/photovogue/photographers/",
        ] {
            assert!(match_url(raw).is_none(), "{raw}");
        }
    }

    #[test]
    fn photographer_media_require_valid_id_and_official_cdn() {
        let item = serde_json::json!({
            "id": 145, "title": "  Autumn Sky  ",
            "gallery_image": "https://assets.vogue.com/photos/abc/autumn.webp"
        });
        let (id, media) = photo_item(&item).unwrap();
        assert_eq!(id, 145);
        assert_eq!(media.post_id, Some(145));
        assert_eq!(media.filename, "145 Autumn Sky.webp");
        assert_eq!(media.url, "https://assets.vogue.com/photos/abc/autumn.webp");
        assert!(photo_item(&serde_json::json!({
            "id": 145, "gallery_image": "https://assets.vogue.com.evil.org/a.jpg"
        }))
        .is_none());
        assert!(photo_item(&serde_json::json!({
            "id": 145, "gallery_image": "file:///etc/private.jpg"
        }))
        .is_none());
        assert!(photo_item(&serde_json::json!({
            "id": "not-a-number", "gallery_image": "https://assets.vogue.com/a.jpg"
        }))
        .is_none());
    }

    #[tokio::test]
    async fn paginates_public_photographer_json_and_deduplicates_photo_ids() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (route, body) in [
                (
                    "GET /production/photos?count=50&order_by=DESC&page=0&photographer_id=123 HTTP/1.1",
                    r#"{"has_next":true,"items":[{"id":101,"title":"First","gallery_image":"https://assets.vogue.com/img/first.jpg"},{"id":102,"title":"Second","gallery_image":"https://assets.vogue.com/img/second.png"}]}"#,
                ),
                (
                    "GET /production/photos?count=50&order_by=DESC&page=1&photographer_id=123 HTTP/1.1",
                    r#"{"has_next":false,"items":[{"id":101,"title":"Duplicate","gallery_image":"https://assets.vogue.com/img/first.jpg"},{"id":103,"title":"Third","gallery_image":"https://media.vogue.com/img/third.webp"}]}"#,
                ),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buffer = [0_u8; 4096];
                let read = socket.read(&mut buffer).await.unwrap();
                let request = String::from_utf8_lossy(&buffer[..read]);
                assert!(request.starts_with(route), "{request}");
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{address}/")).unwrap();
        let media = resolve_at_api_base(
            &Client::new(),
            &base,
            Target::Photographer { id: "123".into() },
            2,
        )
        .await
        .unwrap();
        assert_eq!(media.len(), 3);
        assert_eq!(media[0].filename, "101 First.jpg");
        assert_eq!(media[1].post_id, Some(102));
        assert_eq!(media[2].filename, "103 Third.webp");
        server.await.unwrap();
    }
}
