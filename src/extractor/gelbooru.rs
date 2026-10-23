//! A conservative Gelbooru API v0.2 subset for post and tag-search URLs.
//! Pools, favorites, redirects, media fallbacks, auth, and full metadata
//! remain unimplemented; do not mistake this preview for source parity.
use crate::{safe_filename, MediaItem};
use reqwest::Client;
use serde_json::Value;
use std::{error::Error, time::Duration};
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Post(u64),
    Search(String),
}

pub fn match_url(input: &str) -> Option<Target> {
    let url = Url::parse(input).ok()?;
    if url.scheme() != "https"
        || !matches!(url.host_str(), Some("gelbooru.com" | "www.gelbooru.com"))
    {
        return None;
    }
    if url.path() != "/index.php" && url.path() != "/" {
        return None;
    }
    let query: std::collections::HashMap<String, String> = url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    if query.get("page")?.as_str() != "post" {
        return None;
    }
    match query.get("s")?.as_str() {
        "view" => {
            let id = query.get("id")?.parse::<u64>().ok()?;
            (id > 0).then_some(Target::Post(id))
        }
        "list" => Some(Target::Search(query.get("tags")?.clone())),
        _ => None,
    }
}

fn parse_media(value: &Value, base: &Url) -> Option<MediaItem> {
    let id = value.get("id")?.as_u64()?;
    let raw = value.get("file_url")?.as_str()?;
    if raw.is_empty() {
        return None;
    }
    let url = Url::parse(raw).or_else(|_| base.join(raw)).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let extension = url.path().rsplit_once('.')?.1;
    if extension.is_empty()
        || extension.len() > 12
        || !extension.bytes().all(|b| b.is_ascii_alphanumeric())
    {
        return None;
    }
    Some(MediaItem {
        url: url.to_string(),
        filename: safe_filename(&format!("{id}.{extension}")),
        extractor: "gelbooru",
        post_id: Some(id),
    })
}

/// API pagination counts source posts, including posts with no usable file.
fn source_post_count(value: &Value) -> Result<usize, Box<dyn Error + Send + Sync>> {
    match value.get("post") {
        Some(Value::Array(posts)) => Ok(posts.len()),
        Some(Value::Object(_)) => Ok(1),
        Some(Value::Null) => Ok(0),
        Some(_) => Err("unsupported Gelbooru API post representation".into()),
        None => Err("missing post key in Gelbooru API response".into()),
    }
}

fn parse_posts(value: &Value, base: &Url) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let Some(data) = value.get("post") else {
        return Err("missing post key in Gelbooru API response".into());
    };
    let values = match data {
        Value::Array(posts) => posts.iter().collect::<Vec<_>>(),
        Value::Object(_) => vec![data],
        Value::Null => vec![],
        _ => return Err("unsupported Gelbooru API post representation".into()),
    };
    Ok(values
        .into_iter()
        .filter_map(|post| parse_media(post, base))
        .collect())
}

pub async fn resolve(
    client: &Client,
    target: Target,
    max_pages: u16,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse("https://gelbooru.com/")?;
    resolve_at_base(client, target, max_pages, &base).await
}

/// Apply a conservative cap to *valid* media items for selected file ranges.
/// The API can return posts without downloadable files; do not use raw API
/// post counts as the accepted media ordinal. Zero is clamped to one.
pub async fn resolve_limited(
    client: &Client,
    target: Target,
    max_pages: u16,
    max_media: usize,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse("https://gelbooru.com/")?;
    resolve_at_base_limited(client, target, max_pages, &base, max_media).await
}

async fn resolve_at_base(
    client: &Client,
    target: Target,
    max_pages: u16,
    base: &Url,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    resolve_at_base_limited(client, target, max_pages, base, usize::MAX).await
}

async fn resolve_at_base_limited(
    client: &Client,
    target: Target,
    max_pages: u16,
    base: &Url,
    max_media: usize,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let mut items = Vec::new();
    let media_limit = max_media.max(1);
    let limit = match target {
        Target::Post(_) => 1_u16,
        Target::Search(_) => max_pages.max(1),
    };
    for page in 0..limit {
        let mut url = base.join("index.php")?;
        {
            let mut qp = url.query_pairs_mut();
            qp.append_pair("page", "dapi")
                .append_pair("s", "post")
                .append_pair("q", "index")
                .append_pair("json", "1");
            match &target {
                Target::Post(id) => {
                    qp.append_pair("id", &id.to_string());
                }
                Target::Search(tags) => {
                    qp.append_pair("tags", tags)
                        .append_pair("limit", "100")
                        .append_pair("pid", &page.to_string());
                }
            }
        }
        let response = client.get(url).send().await?.error_for_status()?;
        let body: Value = response.json().await?;
        let source_count = source_post_count(&body)?;
        let fresh = parse_posts(&body, base)?;
        let done = matches!(target, Target::Post(_)) || source_count < 100;
        let remaining = media_limit.saturating_sub(items.len());
        items.extend(fresh.into_iter().take(remaining));
        // In particular, never schedule another API page after the requested
        // finite file ordinal is already available.
        if done || items.len() >= media_limit {
            break;
        }
        if page + 1 < limit {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn query_parameters_match_any_order_and_reject_other_pages() {
        assert_eq!(
            match_url("https://gelbooru.com/index.php?id=45&s=view&page=post"),
            Some(Target::Post(45))
        );
        assert_eq!(
            match_url("https://www.gelbooru.com/index.php?tags=blue+sky&s=list&page=post"),
            Some(Target::Search("blue sky".into()))
        );
        assert_eq!(
            match_url("https://gelbooru.com/index.php?page=pool&s=show&id=45"),
            None
        );
        assert_eq!(
            match_url("https://othergelbooru.com/index.php?page=post&s=view&id=45"),
            None
        );
    }

    #[test]
    fn parses_post_arrays_and_single_objects() {
        let base = Url::parse("https://gelbooru.com/").unwrap();
        let data = json!({"post":[
            {"id": 13, "file_url": "https://img.example.test/pic.webp"},
            {"id": 14, "file_url": null},
            {"id": 15, "file_url": "javascript:alert(1)"},
            {"id": 16, "file_url": "/images/16.png"}
        ]});
        let files = parse_posts(&data, &base).unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].filename, "13.webp");
        assert_eq!(files[1].url, "https://gelbooru.com/images/16.png");
        assert_eq!(
            parse_posts(&json!({"post": {"id": 17, "file_url": "/a.jpg"}}), &base)
                .unwrap()
                .len(),
            1
        );
        assert!(parse_posts(&json!({"unrelated": []}), &base).is_err());
    }

    #[tokio::test]
    async fn single_post_api_is_exercised_against_local_fixture() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0_u8; 4096];
            let read = socket.read(&mut buf).await.unwrap();
            let request = String::from_utf8_lossy(&buf[..read]);
            assert!(request.starts_with("GET /index.php?"), "{request}");
            assert!(request.contains("page=dapi"), "{request}");
            assert!(request.contains("id=91"), "{request}");
            let data = r#"{"post":[{"id":91,"file_url":"https://cdn.example.test/image.jpg"}]}"#;
            let response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{data}", data.len());
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let items = resolve_at_base(&Client::new(), Target::Post(91), 1, &base)
            .await
            .unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].post_id, Some(91));
        server.await.unwrap();
    }
}
