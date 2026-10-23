//! Independent public Lexica search extractor preview.
//! The pinned upstream uses cursor-based JSON POST searches. Complete prompt
//! metadata, archive formats and live API parity are not yet accepted.
use crate::MediaItem;
use reqwest::Client;
use serde_json::{json, Value};
use std::{collections::HashSet, error::Error};
use url::Url;

const ROOT: &str = "https://lexica.art/";
const API: &str = "api/infinite-prompts";
const IMAGE_ROOT: &str = "https://lexica-serve-encoded-images2.sharif.workers.dev/full_jpg/";
const MAX_RESPONSE: usize = 2 * 1024 * 1024;
const MAX_IMAGES: usize = 2000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Search { query: String },
}

pub fn match_url(input: &str) -> Option<Target> {
    let url = Url::parse(input).ok()?;
    if url.scheme() != "https" || url.host_str() != Some("lexica.art") || url.path() != "/" {
        return None;
    }
    let mut values = url.query_pairs().filter(|(key, _)| key == "q");
    let first = values.next()?.1;
    if values.next().is_some()
        || first.trim().is_empty()
        || first.len() > 240
        || first.chars().any(char::is_control)
    {
        return None;
    }
    Some(Target::Search {
        query: first.into_owned(),
    })
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

fn images(data: &Value, seen: &mut HashSet<String>, limit: usize) -> Vec<MediaItem> {
    let mut result = Vec::new();
    let Some(items) = data.get("images").and_then(Value::as_array) else {
        return result;
    };
    for image in items.iter().take(limit) {
        let Some(id) = image.get("id").and_then(Value::as_str) else {
            continue;
        };
        if !valid_id(id) || !seen.insert(id.to_owned()) {
            continue;
        }
        result.push(MediaItem {
            url: format!("{IMAGE_ROOT}{id}"),
            filename: format!("{id}.jpg"),
            extractor: "lexica",
            post_id: id.parse::<u64>().ok(),
        });
    }
    result
}

fn next_cursor(data: &Value) -> Option<Value> {
    match data.get("nextCursor")? {
        Value::String(s)
            if !s.trim().is_empty()
                && s.len() <= 128
                && !s.chars().any(char::is_control) =>
        {
            Some(Value::String(s.clone()))
        },
        Value::Number(n) if n.as_u64().is_some_and(|n| n > 0) => Some(Value::Number(n.clone())),
        _ => None,
    }
}

async fn request_page(
    client: &Client,
    endpoint: &Url,
    query: &str,
    cursor: &Value,
) -> Result<Value, Box<dyn Error + Send + Sync>> {
    let referer = {
        let mut url = Url::parse(ROOT)?;
        url.query_pairs_mut().append_pair("q", query);
        url.to_string()
    };
    let payload = json!({
        "text": query,
        "searchMode": "images",
        "source": "search",
        "cursor": cursor,
        "model": "lexica-aperture-v2"
    });
    let mut response = client
        .post(endpoint.clone())
        .header("Accept", "application/json, text/plain, */*")
        .header("Referer", referer)
        .json(&payload)
        .send()
        .await?
        .error_for_status()?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE as u64)
    {
        return Err("Lexica API JSON exceeded the response limit".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE {
            return Err("Lexica API JSON exceeded the response limit".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(serde_json::from_slice(&bytes)?)
}

async fn resolve_at_base(
    client: &Client,
    root: &Url,
    target: Target,
    max_pages: u16,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let Target::Search { query } = target;
    let endpoint = root.join(API)?;
    let mut result = Vec::new();
    let mut seen_images = HashSet::new();
    let mut seen_cursors = HashSet::new();
    let mut cursor = json!(0);
    for page in 0..max_pages.clamp(1, 100) {
        let cursor_id = serde_json::to_string(&cursor)?;
        if !seen_cursors.insert(cursor_id) {
            break;
        }
        if page > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
        let data = request_page(client, &endpoint, &query, &cursor).await?;
        let items = data
            .get("images")
            .and_then(Value::as_array)
            .ok_or("Lexica API result missing images array")?;
        if items.len() > MAX_IMAGES {
            return Err("Lexica page exceeded the image count bound".into());
        }
        result.extend(images(&data, &mut seen_images, MAX_IMAGES - result.len()));
        if result.len() >= MAX_IMAGES {
            break;
        }
        let Some(next) = next_cursor(&data) else {
            break;
        };
        cursor = next;
    }
    Ok(result)
}

pub async fn resolve(
    client: &Client,
    target: Target,
    max_pages: u16,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    resolve_at_base(client, &Url::parse(ROOT)?, target, max_pages).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_url_is_root_only_and_preserves_query_spaces() {
        assert_eq!(
            match_url("https://lexica.art/?q=blue+sky"),
            Some(Target::Search {
                query: "blue sky".into()
            })
        );
        for bad in [
            "http://lexica.art/?q=blue",
            "https://lexica.art.evil.test/?q=blue",
            "https://lexica.art/api?q=blue",
            "https://lexica.art/?q=",
            "https://lexica.art/?q=blue&q=other",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn public_image_ids_are_bounded_and_duplicate_safe() {
        let page = json!({
            "images":[
                {"id":"abc123"},
                {"id":"abc123"},
                {"id":"safe_2"},
                {"id":"../unsafe"},
                {"id":"https://evil.org/malware.jpg"}
            ]
        });
        let mut seen = HashSet::new();
        let items = images(&page, &mut seen, 20);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].filename, "abc123.jpg");
        assert_eq!(items[0].url, format!("{IMAGE_ROOT}abc123"));
        assert_eq!(items[1].filename, "safe_2.jpg");
        assert_eq!(next_cursor(&json!({"nextCursor": 5})), Some(json!(5)));
        assert!(next_cursor(&json!({"nextCursor": 0})).is_none());
        assert!(next_cursor(&json!({"nextCursor": {}})).is_none());
        assert!(next_cursor(&json!({"nextCursor": "  "})).is_none());
        assert!(next_cursor(&json!({"nextCursor": "bad\\u0000cursor"})).is_none());
        assert!(next_cursor(&json!({"nextCursor": "x".repeat(129)})).is_none());
    }

    #[tokio::test]
    async fn search_posts_cursor_pages_and_stops_on_cycle_with_local_http() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (expected_cursor, body) in [
                (
                    json!(0),
                    r#"{"images":[{"id":"image1"},{"id":"image2"}],"prompts":[],"nextCursor":123}"#,
                ),
                (
                    json!(123),
                    r#"{"images":[{"id":"image1"},{"id":"image3"}],"prompts":[],"nextCursor":123}"#,
                ),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let header_end = loop {
                    let mut chunk = [0_u8; 2048];
                    let n = socket.read(&mut chunk).await.unwrap();
                    assert!(n > 0, "request ended before headers");
                    request.extend_from_slice(&chunk[..n]);
                    assert!(request.len() <= 16384, "request exceeded test bound");
                    if let Some(index) = request.windows(4).position(|x| x == b"\\r\\n\\r\\n") {
                        break index + 4;
                    }
                };
                let header = String::from_utf8_lossy(&request[..header_end]);
                assert!(
                    header.starts_with("POST /api/infinite-prompts HTTP/1.1"),
                    "{header}"
                );
                let length: usize = header
                    .lines()
                    .find_map(|line| {
                        let (key, value) = line.split_once(':')?;
                        key.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                    .expect("JSON request must have Content-Length");
                assert!(length <= 8192, "unexpectedly large JSON request");
                while request.len() - header_end < length {
                    let mut chunk = [0_u8; 2048];
                    let n = socket.read(&mut chunk).await.unwrap();
                    assert!(n > 0, "request ended before JSON body");
                    request.extend_from_slice(&chunk[..n]);
                }
                let payload: Value =
                    serde_json::from_slice(&request[header_end..header_end + length]).unwrap();
                assert_eq!(payload["cursor"], expected_cursor);
                assert_eq!(payload["text"], "blue sky");
                assert_eq!(payload["searchMode"], "images");
                assert_eq!(payload["source"], "search");
                assert_eq!(payload["model"], "lexica-aperture-v2");
                assert!(
                    header.to_ascii_lowercase().contains("referer: https://lexica.art/?q=blue+sky"),
                    "{header}"
                );
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{address}/")).unwrap();
        let media = resolve_at_base(
            &Client::new(),
            &base,
            Target::Search {
                query: "blue sky".into(),
            },
            2,
        )
        .await
        .unwrap();
        assert_eq!(media.len(), 3);
        assert_eq!(media[0].filename, "image1.jpg");
        assert_eq!(media[2].filename, "image3.jpg");
        server.await.unwrap();
    }
}
