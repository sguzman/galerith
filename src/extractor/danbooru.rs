//! Partial Danbooru read-only post, tag, pool and favorite-group extraction.
//! API reference: https://safebooru.donmai.us/wiki_pages/api%3Aposts
//! Module status is PARTIAL until gallery-dl parity is verified.
use crate::{safe_filename, MediaItem};
use reqwest::Client;
use serde_json::Value;
use std::{collections::HashMap, error::Error, time::Duration};
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Post {
        host: String,
        id: u64,
    },
    Search {
        host: String,
        tags: String,
    },
    Collection {
        host: String,
        id: u64,
        kind: CollectionKind,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectionKind {
    Pool,
    FavoriteGroup,
}

impl CollectionKind {
    fn api_path(self) -> &'static str {
        match self {
            Self::Pool => "pools",
            Self::FavoriteGroup => "favorite_groups",
        }
    }

    fn ordered_tag(self) -> &'static str {
        match self {
            Self::Pool => "ordpool",
            Self::FavoriteGroup => "ordfavgroup",
        }
    }
}

pub fn match_url(input: &str) -> Option<Target> {
    let url = Url::parse(input).ok()?;
    if url.scheme() != "https" {
        return None;
    }
    let host = url.host_str()?;
    if !matches!(host, "danbooru.donmai.us" | "safebooru.donmai.us") {
        return None;
    }
    let parts: Vec<&str> = url.path_segments()?.collect();
    let host = host.to_owned();
    match parts.as_slice() {
        ["posts", id] => {
            let id = id.strip_suffix(".json").unwrap_or(id).parse::<u64>().ok()?;
            (id != 0).then_some(Target::Post { host, id })
        }
        ["pools", id] | ["favorite_groups", id] => {
            let kind = if parts[0] == "pools" {
                CollectionKind::Pool
            } else {
                CollectionKind::FavoriteGroup
            };
            let id = id.parse::<u64>().ok()?;
            (id > 0).then_some(Target::Collection { host, id, kind })
        }
        ["posts"] | ["posts.json"] => {
            let tags = url
                .query_pairs()
                .find(|(name, _)| name == "tags")
                .map(|(_, value)| value.into_owned())
                .unwrap_or_default();
            Some(Target::Search { host, tags })
        }
        _ => None,
    }
}

fn post_items(body: &Value, base: &Url) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let posts: Vec<&Value> = match body {
        Value::Array(posts) => posts.iter().collect(),
        Value::Object(_) => vec![body],
        _ => return Err("unexpected Danbooru API response".into()),
    };
    let mut items = Vec::new();
    for post in posts {
        let Some(id) = post.get("id").and_then(Value::as_u64) else {
            continue;
        };
        let Some(raw_url) = post.get("file_url").and_then(Value::as_str) else {
            continue;
        };
        let Some(ext) = post.get("file_ext").and_then(Value::as_str) else {
            continue;
        };
        if ext.is_empty() || ext.len() > 16 || !ext.chars().all(|c| c.is_ascii_alphanumeric()) {
            continue;
        }
        let Ok(media_url) = Url::parse(raw_url).or_else(|_| base.join(raw_url)) else {
            continue;
        };
        if !matches!(media_url.scheme(), "http" | "https") {
            continue;
        }
        items.push(MediaItem {
            url: media_url.to_string(),
            filename: safe_filename(&format!("{id}.{ext}")),
            extractor: "danbooru",
            post_id: Some(id),
        });
    }
    Ok(items)
}

pub async fn resolve(
    client: &Client,
    target: Target,
    max_pages: u16,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let host = match &target {
        Target::Post { host, .. }
        | Target::Search { host, .. }
        | Target::Collection { host, .. } => host,
    };
    let base = Url::parse(&format!("https://{host}/"))?;
    resolve_at_base(client, target, max_pages, &base).await
}

async fn resolve_collection(
    client: &Client,
    base: &Url,
    id: u64,
    kind: CollectionKind,
    max_pages: u16,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let endpoint = base.join(&format!("{}/{id}.json", kind.api_path()))?;
    let metadata: Value = client
        .get(endpoint)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let ordered_ids = metadata
        .get("post_ids")
        .and_then(Value::as_array)
        .ok_or("Danbooru collection response missing post_ids")?;
    let mut order = Vec::with_capacity(ordered_ids.len());
    for id in ordered_ids {
        order.push(id.as_u64().ok_or("invalid collection post id")?);
    }
    if order.is_empty() {
        return Ok(Vec::new());
    }

    let mut available: HashMap<u64, MediaItem> = HashMap::new();
    for page in 1..=max_pages.max(1) {
        let mut endpoint = base.join("posts.json")?;
        endpoint
            .query_pairs_mut()
            .append_pair("tags", &format!("{}:{id}", kind.ordered_tag()))
            .append_pair("limit", "50")
            .append_pair("page", &page.to_string());
        let response: Value = client
            .get(endpoint)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let posts = response
            .as_array()
            .ok_or("invalid Danbooru collection posts")?;
        for media in post_items(&response, base)? {
            if let Some(id) = media.post_id {
                available.entry(id).or_insert(media);
            }
        }
        if posts.len() < 50 {
            break;
        }
        if page < max_pages {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
    Ok(order
        .into_iter()
        .filter_map(|id| available.get(&id).cloned())
        .collect())
}

/// Private injection seam for deterministic synthetic server tests. Public
/// dispatch still restricts real extraction to recognized Danbooru hosts.
async fn resolve_at_base(
    client: &Client,
    target: Target,
    max_pages: u16,
    base: &Url,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    match target {
        Target::Collection { id, kind, .. } => {
            resolve_collection(client, base, id, kind, max_pages).await
        }
        Target::Post { id, .. } => {
            let endpoint = base.join(&format!("posts/{id}.json"))?;
            let response = client.get(endpoint).send().await?.error_for_status()?;
            let body: Value = response.json().await?;
            post_items(&body, base)
        }
        Target::Search { tags, .. } => {
            let mut items = Vec::new();
            for page in 1..=max_pages.max(1) {
                let mut endpoint = base.join("posts.json")?;
                endpoint
                    .query_pairs_mut()
                    .append_pair("tags", &tags)
                    .append_pair("limit", "50")
                    .append_pair("page", &page.to_string());
                let response = client.get(endpoint).send().await?.error_for_status()?;
                let body: Value = response.json().await?;
                let page_is_empty = body
                    .as_array()
                    .ok_or("unexpected Danbooru posts response")?
                    .is_empty();
                if page_is_empty {
                    break;
                }
                items.extend(post_items(&body, base)?);
                if page < max_pages {
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                }
            }
            Ok(items)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn post_routes_and_search_query_are_explicit() {
        assert_eq!(
            match_url("https://danbooru.donmai.us/posts/42.json"),
            Some(Target::Post {
                host: "danbooru.donmai.us".into(),
                id: 42
            })
        );
        assert_eq!(
            match_url("https://safebooru.donmai.us/posts?tags=blue+sky"),
            Some(Target::Search {
                host: "safebooru.donmai.us".into(),
                tags: "blue sky".into()
            })
        );
        assert_eq!(match_url("https://evil.example/posts/42"), None);
        assert_eq!(match_url("http://danbooru.donmai.us/posts/42"), None);
    }

    #[test]
    fn recognizes_collection_routes_and_rejects_unknown_hosts() {
        assert_eq!(
            match_url("https://danbooru.donmai.us/pools/42"),
            Some(Target::Collection {
                host: "danbooru.donmai.us".into(),
                id: 42,
                kind: CollectionKind::Pool,
            })
        );
        assert_eq!(
            match_url("https://safebooru.donmai.us/favorite_groups/71"),
            Some(Target::Collection {
                host: "safebooru.donmai.us".into(),
                id: 71,
                kind: CollectionKind::FavoriteGroup,
            })
        );
        assert!(match_url("https://danbooru.donmai.us/pools/0").is_none());
        assert!(match_url("https://danbooru.donmai.us/pools/42/edit").is_none());
        assert!(match_url("https://evil.donmai.us/pools/42").is_none());
    }

    #[tokio::test]
    async fn pool_uses_collection_post_ids_not_api_reply_order() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (needle, body) in [
                ("GET /pools/42.json HTTP/1.1", r#"{"post_ids":[8,7,9]}"#),
                (
                    "GET /posts.json?tags=ordpool%3A42&limit=50&page=1 HTTP/1.1",
                    r#"[{"id":7,"file_ext":"jpg","file_url":"https://img.example/7.jpg"},{"id":8,"file_ext":"png","file_url":"https://img.example/8.png"},{"id":9,"file_ext":"jpg","file_url":null}]"#,
                ),
            ] {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).await.unwrap();
                let req = String::from_utf8_lossy(&buf[..n]);
                assert!(req.starts_with(needle), "{req}");
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let items = resolve_collection(&Client::new(), &base, 42, CollectionKind::Pool, 4)
            .await
            .unwrap();
        assert_eq!(
            items.iter().map(|it| it.post_id).collect::<Vec<_>>(),
            vec![Some(8), Some(7)]
        );
        assert_eq!(items[0].filename, "8.png");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn favorite_group_preserves_order_and_skips_missing_items() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (needle, body) in [
                (
                    "GET /favorite_groups/71.json HTTP/1.1",
                    r#"{"post_ids":[55,54]}"#,
                ),
                (
                    "GET /posts.json?tags=ordfavgroup%3A71&limit=50&page=1 HTTP/1.1",
                    r#"[{"id":54,"file_ext":"webm","file_url":"https://img.example/54.webm"},{"id":55,"file_ext":"jpg","file_url":"https://img.example/55.jpg"}]"#,
                ),
            ] {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).await.unwrap();
                let req = String::from_utf8_lossy(&buf[..n]);
                assert!(req.starts_with(needle), "{req}");
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let items = resolve_collection(&Client::new(), &base, 71, CollectionKind::FavoriteGroup, 1)
            .await
            .unwrap();
        assert_eq!(
            items.iter().map(|it| it.post_id).collect::<Vec<_>>(),
            vec![Some(55), Some(54)]
        );
        server.await.unwrap();
    }

    #[test]
    fn parse_posts_skip_inaccessible_file_urls() {
        let body = json!([
            {"id": 7, "file_ext": "png", "file_url": "https://cdn.example/file.png"},
            {"id": 8, "file_ext": "jpg", "file_url": null},
            {"id": 9, "file_ext": "../bad", "file_url": "https://cdn.example/x"}
        ]);
        let base = Url::parse("https://danbooru.donmai.us/").unwrap();
        let items = post_items(&body, &base).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].filename, "7.png");
        assert_eq!(items[0].post_id, Some(7));
    }

    #[test]
    fn resolves_relative_media_urls() {
        let base = Url::parse("https://safebooru.donmai.us/").unwrap();
        let posts = json!({"id": 23, "file_ext": "webp", "file_url": "/data/abc.webp"});
        let items = post_items(&posts, &base).unwrap();
        assert_eq!(items[0].url, "https://safebooru.donmai.us/data/abc.webp");
    }
    #[tokio::test]
    async fn end_to_end_post_fixture_uses_correct_api_path() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut data = [0_u8; 4096];
            let read = socket.read(&mut data).await.unwrap();
            let request = String::from_utf8_lossy(&data[..read]);
            assert!(
                request.starts_with("GET /posts/56.json HTTP/1.1"),
                "{request}"
            );
            let body = r#"{"id":56,"file_url":"https://cdn.example.org/p.png","file_ext":"png"}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(), body
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        let base = Url::parse(&format!("http://{address}/")).unwrap();
        let target = Target::Post {
            host: "safebooru.donmai.us".into(),
            id: 56,
        };
        let result = resolve_at_base(&Client::new(), target, 1, &base)
            .await
            .unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].filename, "56.png");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn end_to_end_tag_search_stops_at_empty_page() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for page in 1..=2 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut data = [0_u8; 4096];
                let read = socket.read(&mut data).await.unwrap();
                let request = String::from_utf8_lossy(&data[..read]);
                assert!(request.starts_with("GET /posts.json?"), "{request}");
                assert!(request.contains("tags=blue+sky"), "{request}");
                assert!(request.contains(&format!("page={page}")), "{request}");
                let body = if page == 1 {
                    r#"[{"id":123,"file_url":"https://cdn.example.org/one.jpg","file_ext":"jpg"},{"id":124,"file_url":null,"file_ext":"png"}]"#
                } else {
                    "[]"
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(), body
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{address}/")).unwrap();
        let target = Target::Search {
            host: "safebooru.donmai.us".into(),
            tags: "blue sky".into(),
        };
        let result = resolve_at_base(&Client::new(), target, 5, &base)
            .await
            .unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].filename, "123.jpg");
        server.await.unwrap();
    }
}
