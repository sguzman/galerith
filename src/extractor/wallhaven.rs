//! Independent public Wallhaven JSON API extractor preview.
//! Wallhaven's public API allows unauthenticated SFW image/search routes.
//! No default/shared API key, account access, private collections, or live
//! parity with upstream gallery-dl is claimed.
use crate::MediaItem;
use reqwest::Client;
use serde_json::Value;
use std::{collections::HashSet, error::Error, time::Duration};
use url::Url;

const API_ROOT: &str = "https://wallhaven.cc/api/";
const MAX_RESPONSE: usize = 2 * 1024 * 1024;
const MAX_MEDIA: usize = 2000;
const MAX_PAGES: u16 = 100;
// The official public API documents a 45 request/minute limit.
const PAGE_PACING: Duration = Duration::from_millis(1400);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Image { id: String },
    Search { query: String },
    Uploads { user: String },
}

fn valid_id(id: &str) -> bool {
    (6..=32).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

fn valid_user(user: &str) -> bool {
    (1..=64).contains(&user.len())
        && user
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

pub fn match_url(input: &str) -> Option<Target> {
    let url = Url::parse(input).ok()?;
    if url.scheme() != "https"
        || url.host_str() != Some("wallhaven.cc")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let parts: Vec<_> = url.path_segments()?.collect();
    match parts.as_slice() {
        ["w", id] if valid_id(id) && url.query().is_none() => {
            Some(Target::Image { id: (*id).into() })
        }
        ["user", user, "uploads"] if valid_user(user) && url.query().is_none() => {
            Some(Target::Uploads {
                user: (*user).into(),
            })
        }
        ["search"] | ["search", ""] => {
            let mut pairs = url.query_pairs();
            let (key, value) = pairs.next()?;
            if key != "q"
                || pairs.next().is_some()
                || value.trim().is_empty()
                || value.len() > 240
                || value.chars().any(char::is_control)
            {
                return None;
            }
            Some(Target::Search {
                query: value.into_owned(),
            })
        }
        _ => None,
    }
}

fn image_from_json(wp: &Value) -> Option<(String, MediaItem)> {
    let id = wp.get("id")?.as_str()?;
    if !valid_id(id) {
        return None;
    }
    let url = Url::parse(wp.get("path")?.as_str()?).ok()?;
    if url.scheme() != "https"
        || url.host_str() != Some("w.wallhaven.cc")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let segments: Vec<_> = url.path_segments()?.collect();
    let (prefix, filename) = match segments.as_slice() {
        ["full", prefix, filename] => (*prefix, *filename),
        [prefix, filename] => (*prefix, *filename),
        _ => return None,
    };
    if prefix.len() != 2 || !prefix.bytes().all(|x| x.is_ascii_alphanumeric()) {
        return None;
    }
    let filename_root = format!("wallhaven-{id}.");
    let ext = filename.strip_prefix(&filename_root)?;
    if !matches!(ext, "jpg" | "jpeg" | "png" | "webp") {
        return None;
    }
    Some((
        id.to_owned(),
        MediaItem {
            url: url.to_string(),
            filename: filename.to_owned(),
            extractor: "wallhaven",
            post_id: None,
        },
    ))
}

fn parse_items(
    data: &Value,
    single: bool,
) -> Result<Vec<(String, MediaItem)>, Box<dyn Error + Send + Sync>> {
    if data.get("error").is_some() {
        return Err("Wallhaven API returned an error".into());
    }
    let Some(values) = data.get("data") else {
        return Err("Wallhaven API is missing data".into());
    };
    let wallpapers: Vec<&Value> = if single {
        vec![values]
    } else {
        values
            .as_array()
            .ok_or("Wallhaven listing is not an array")?
            .iter()
            .take(64)
            .collect()
    };
    Ok(wallpapers.into_iter().filter_map(image_from_json).collect())
}

async fn fetch_json(client: &Client, url: Url) -> Result<Value, Box<dyn Error + Send + Sync>> {
    let mut response = client
        .get(url)
        .header("Accept", "application/json")
        .send()
        .await?
        .error_for_status()?;
    if response
        .content_length()
        .is_some_and(|len| len > MAX_RESPONSE as u64)
    {
        return Err("Wallhaven JSON exceeded the response limit".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE {
            return Err("Wallhaven JSON exceeded the response limit".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(serde_json::from_slice(&bytes)?)
}

async fn resolve_at_base(
    client: &Client,
    base: &Url,
    target: &Target,
    max_pages: u16,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let mut items = Vec::new();
    let mut seen = HashSet::new();
    let limit = if matches!(target, Target::Image { .. }) {
        1
    } else {
        max_pages.clamp(1, MAX_PAGES)
    };
    for page in 1..=limit {
        if page > 1 {
            tokio::time::sleep(PAGE_PACING).await;
        }
        let mut endpoint = match target {
            Target::Image { id } => base.join(&format!("v1/w/{id}"))?,
            Target::Search { .. } | Target::Uploads { .. } => base.join("v1/search")?,
        };
        if !matches!(target, Target::Image { .. }) {
            let q = match target {
                Target::Search { query } => query.clone(),
                Target::Uploads { user } => format!("@{user}"),
                Target::Image { .. } => unreachable!(),
            };
            endpoint
                .query_pairs_mut()
                .append_pair("q", &q)
                .append_pair("page", &page.to_string());
        }
        let data = fetch_json(client, endpoint).await?;
        let one = matches!(target, Target::Image { .. });
        for (id, item) in parse_items(&data, one)? {
            if seen.insert(id) {
                items.push(item);
                if items.len() >= MAX_MEDIA {
                    return Ok(items);
                }
            }
        }
        if one {
            break;
        }
        let meta = data
            .get("meta")
            .ok_or("Wallhaven listing lacks pagination metadata")?;
        let current = meta
            .get("current_page")
            .and_then(Value::as_u64)
            .ok_or("Wallhaven page index missing")?;
        let last = meta
            .get("last_page")
            .and_then(Value::as_u64)
            .ok_or("Wallhaven last page missing")?;
        if current == 0 || current != page as u64 || last < current {
            return Err("Wallhaven pagination metadata was inconsistent".into());
        }
        if current >= last {
            break;
        }
    }
    Ok(items)
}

pub async fn resolve(
    client: &Client,
    target: Target,
    max_pages: u16,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    resolve_at_base(client, &Url::parse(API_ROOT)?, &target, max_pages).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_only_safe_public_image_search_and_upload_urls() {
        assert_eq!(
            match_url("https://wallhaven.cc/w/94x38z"),
            Some(Target::Image {
                id: "94x38z".into()
            })
        );
        assert_eq!(
            match_url("https://wallhaven.cc/search?q=blue+sky"),
            Some(Target::Search {
                query: "blue sky".into()
            })
        );
        assert_eq!(
            match_url("https://wallhaven.cc/user/example-1/uploads"),
            Some(Target::Uploads {
                user: "example-1".into()
            })
        );
        for bad in [
            "http://wallhaven.cc/w/94x38z",
            "https://wallhaven.cc.evil.org/w/94x38z",
            "https://wallhaven.cc/w/94x38z?apikey=secret",
            "https://wallhaven.cc/search?q=blue&q=other",
            "https://wallhaven.cc/search?purity=111&q=blue",
            "https://wallhaven.cc/search?q=",
            "https://wallhaven.cc/user/alice/favorites/1",
            "https://wallhaven.cc/w/a",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn original_image_urls_must_match_known_wallhaven_cdn_and_ids() {
        let wp = serde_json::json!({
            "id":"94x38z",
            "path":"https://w.wallhaven.cc/full/94/wallhaven-94x38z.jpg"
        });
        let (id, media) = image_from_json(&wp).unwrap();
        assert_eq!(id, "94x38z");
        assert_eq!(media.filename, "wallhaven-94x38z.jpg");
        assert!(image_from_json(&serde_json::json!({
            "id":"94x38z",
            "path":"https://w.wallhaven.cc/94/wallhaven-94x38z.jpg"
        }))
        .is_some());
        for path in [
            "http://w.wallhaven.cc/full/94/wallhaven-94x38z.jpg",
            "https://w.wallhaven.cc.evil.org/full/94/wallhaven-94x38z.jpg",
            "https://w.wallhaven.cc/full/94/wallhaven-evil.jpg",
            "https://w.wallhaven.cc/full/94/wallhaven-94x38z.svg",
            "https://w.wallhaven.cc/other/path/file.jpg",
        ] {
            assert!(
                image_from_json(&serde_json::json!({"id":"94x38z","path":path})).is_none(),
                "{path}"
            );
        }
    }

    #[tokio::test]
    async fn single_wallpaper_resolves_through_public_json_api() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut req = [0_u8; 4096];
            let n = socket.read(&mut req).await.unwrap();
            let request = String::from_utf8_lossy(&req[..n]);
            assert!(request.starts_with("GET /api/v1/w/94x38z HTTP/1.1"));
            let body = r#"{"data":{"id":"94x38z","path":"https://w.wallhaven.cc/full/94/wallhaven-94x38z.jpg"}}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        let base = Url::parse(&format!("http://{address}/api/")).unwrap();
        let items = resolve_at_base(
            &Client::new(),
            &base,
            &Target::Image {
                id: "94x38z".into(),
            },
            2,
        )
        .await
        .unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].filename, "wallhaven-94x38z.jpg");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn search_paginates_deduplicates_and_stops_at_api_last_page() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for page in 1..=2 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut req = [0_u8; 4096];
                let n = socket.read(&mut req).await.unwrap();
                let request = String::from_utf8_lossy(&req[..n]);
                let first = request.lines().next().unwrap();
                let path = first.split_whitespace().nth(1).unwrap();
                let url = Url::parse(&format!("http://localhost{path}")).unwrap();
                assert_eq!(url.path(), "/api/v1/search");
                let pairs: std::collections::HashMap<_, _> =
                    url.query_pairs().into_owned().collect();
                assert_eq!(pairs.get("q").map(String::as_str), Some("blue sky"));
                assert_eq!(
                    pairs.get("page").map(String::as_str),
                    Some(if page == 1 { "1" } else { "2" })
                );
                let first_page = serde_json::json!({
                    "data":[
                        {"id":"94x38z","path":"https://w.wallhaven.cc/full/94/wallhaven-94x38z.jpg"},
                        {"id":"ze1p56","path":"https://w.wallhaven.cc/full/ze/wallhaven-ze1p56.png"}
                    ],"meta":{"current_page":1,"last_page":2}
                });
                let last_page = serde_json::json!({
                    "data":[
                        {"id":"94x38z","path":"https://w.wallhaven.cc/full/94/wallhaven-94x38z.jpg"},
                        {"id":"a1b2c3","path":"https://w.wallhaven.cc/full/a1/wallhaven-a1b2c3.jpg"},
                        {"id":"unsafe","path":"https://evil.test/unsafe.jpg"}
                    ],"meta":{"current_page":2,"last_page":2}
                });
                let body = if page == 1 {
                    first_page.to_string()
                } else {
                    last_page.to_string()
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{address}/api/")).unwrap();
        let items = resolve_at_base(
            &Client::new(),
            &base,
            &Target::Search {
                query: "blue sky".into(),
            },
            3,
        )
        .await
        .unwrap();
        assert_eq!(
            items
                .iter()
                .map(|x| x.filename.as_str())
                .collect::<Vec<_>>(),
            [
                "wallhaven-94x38z.jpg",
                "wallhaven-ze1p56.png",
                "wallhaven-a1b2c3.jpg"
            ]
        );
        server.await.unwrap();
    }

    #[test]
    fn rejects_bad_listing_shapes_and_error_responses() {
        assert!(parse_items(&serde_json::json!({"data":{}}), false).is_err());
        assert!(parse_items(&serde_json::json!({"error":"rate-limited"}), true).is_err());
        assert!(parse_items(&serde_json::json!({}), true).is_err());
    }

    #[tokio::test]
    async fn user_uploads_query_is_scoped_and_read_only() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            loop {
                let mut chunk = [0_u8; 2048];
                let n = socket.read(&mut chunk).await.unwrap();
                assert!(n > 0);
                request.extend_from_slice(&chunk[..n]);
                assert!(request.len() <= 8192);
                if request.windows(4).any(|x| x == b"\r\n\r\n") {
                    break;
                }
            }
            let req = String::from_utf8(request).unwrap();
            let line = req.lines().next().unwrap();
            assert!(line.starts_with("GET "));
            let url = Url::parse(&format!("http://localhost{}", line.split_whitespace().nth(1).unwrap())).unwrap();
            let query: std::collections::HashMap<_, _> =
                url.query_pairs().into_owned().collect();
            assert_eq!(url.path(), "/api/v1/search");
            assert_eq!(query.get("q").map(String::as_str), Some("@example-1"));
            assert_eq!(query.get("page").map(String::as_str), Some("1"));
            assert!(!req.contains("X-API-Key:"));
            let body = r#"{"data":[],"meta":{"current_page":1,"last_page":1}}"#;
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(reply.as_bytes()).await.unwrap();
        });
        let base = Url::parse(&format!("http://{address}/api/")).unwrap();
        let items = resolve_at_base(
            &Client::new(),
            &base,
            &Target::Uploads {
                user: "example-1".into(),
            },
            3,
        )
        .await
        .unwrap();
        assert!(items.is_empty());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn rejects_oversized_public_api_response_without_reading_body() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            loop {
                let mut chunk = [0_u8; 2048];
                let n = socket.read(&mut chunk).await.unwrap();
                assert!(n > 0);
                request.extend_from_slice(&chunk[..n]);
                assert!(request.len() <= 8192);
                if request.windows(4).any(|x| x == b"\r\n\r\n") {
                    break;
                }
            }
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                MAX_RESPONSE + 1
            );
            socket.write_all(reply.as_bytes()).await.unwrap();
        });
        let base = Url::parse(&format!("http://{address}/api/")).unwrap();
        let error = resolve_at_base(
            &Client::new(),
            &base,
            &Target::Image {
                id: "94x38z".into(),
            },
            1,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("response limit"));
        server.await.unwrap();
    }
}
