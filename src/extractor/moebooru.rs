//! Native Moebooru JSON API preview for pinned host families.
//! Post, tag-search and pool-file enumeration are implemented independently.
//! Authentication, full metadata and formatting remain pending.
use crate::{safe_filename, MediaItem};
use reqwest::Client;
use serde_json::Value;
use std::{error::Error, time::Duration};
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Query {
    Post(u64),
    Tags(String),
    Pool(u64),
    Popular { feed: &'static str, parameters: Vec<(String, String)> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    host: &'static str,
    category: &'static str,
    query: Query,
}

fn site(host: &str) -> Option<(&'static str, &'static str)> {
    match host {
        "yande.re" => Some(("yande.re", "yandere")),
        "konachan.com" | "konachan.net" => Some(("konachan.com", "konachan")),
        "sakugabooru.com" | "www.sakugabooru.com" => Some(("www.sakugabooru.com", "sakugabooru")),
        "lolibooru.moe" => Some(("lolibooru.moe", "lolibooru")),
        _ => None,
    }
}

pub fn match_url(input: &str) -> Option<Target> {
    let url = Url::parse(input).ok()?;
    if url.scheme() != "https" {
        return None;
    }
    let (host, category) = site(url.host_str()?)?;
    let mut path = url.path_segments()?.filter(|p| !p.is_empty());
    let query = match (path.next()?, path.next(), path.next(), path.next()) {
        ("post", Some("show"), Some(id), None) => {
            let id = id.parse::<u64>().ok()?;
            if id == 0 {
                return None;
            }
            Query::Post(id)
        }
        ("pool", Some("show"), Some(id), None) => {
            let id = id.parse::<u64>().ok()?;
            if id == 0 {
                return None;
            }
            Query::Pool(id)
        }
        ("post", Some(feed), None, None) => {
            let feed = match feed {
                "popular_by_day" => "popular_by_day",
                "popular_by_week" => "popular_by_week",
                "popular_by_month" => "popular_by_month",
                "popular_recent" => "popular_recent",
                _ => return None,
            };
            let parameters: Vec<(String, String)> = url.query_pairs()
                .map(|(key, value)| (key.into_owned(), value.into_owned()))
                .collect();
            if parameters.len() > 12 || parameters.iter().any(|(key, value)| {
                key.is_empty() || key.len() > 32 || value.len() > 128
                    || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            }) {
                return None;
            }
            Query::Popular { feed, parameters }
        }
        ("post", None, None, None) => {
            let tags = url
                .query_pairs()
                .find(|(k, _)| k == "tags")
                .map(|(_, v)| v.into_owned())?;
            Query::Tags(tags)
        }
        _ => return None,
    };
    Some(Target {
        host,
        category,
        query,
    })
}

fn parse_items(
    value: &Value,
    category: &'static str,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let posts = value
        .as_array()
        .ok_or("Moebooru JSON API must return an array")?;
    let mut items = Vec::new();
    for post in posts {
        let Some(id) = post.get("id").and_then(Value::as_u64) else {
            continue;
        };
        let Some(raw) = post.get("file_url").and_then(Value::as_str) else {
            continue;
        };
        let Ok(url) = Url::parse(raw) else { continue };
        if !matches!(url.scheme(), "http" | "https") {
            continue;
        }
        let Some(ext) = url.path().rsplit_once('.').map(|(_, ext)| ext) else {
            continue;
        };
        if ext.is_empty() || ext.len() > 12 || !ext.bytes().all(|c| c.is_ascii_alphanumeric()) {
            continue;
        }
        let md5 = post
            .get("md5")
            .and_then(Value::as_str)
            .filter(|s| s.len() == 32 && s.bytes().all(|byte| byte.is_ascii_hexdigit()));
        let filename = match md5 {
            Some(md5) => format!("{category}_{id}_{md5}.{ext}"),
            None => format!("{category}_{id}.{ext}"),
        };
        items.push(MediaItem {
            url: url.to_string(),
            filename: safe_filename(&filename),
            extractor: category,
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
    let base = Url::parse(&format!("https://{}/", target.host))?;
    resolve_at_base(client, target, max_pages, &base).await
}

async fn resolve_at_base(
    client: &Client,
    target: Target,
    max_pages: u16,
    base: &Url,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    if let Query::Popular { feed, parameters } = &target.query {
        let mut endpoint = base.join(&format!("post/{feed}.json"))?;
        {
            let mut query = endpoint.query_pairs_mut();
            for (key, value) in parameters {
                query.append_pair(key, value);
            }
        }
        let body: Value = client.get(endpoint).send().await?.error_for_status()?.json().await?;
        return parse_items(&body, target.category);
    }
    let tags = match &target.query {
        Query::Post(id) => format!("id:{id}"),
        Query::Tags(tags) => tags.clone(),
        Query::Pool(id) => format!("pool:{id}"),
        Query::Popular { .. } => unreachable!("popular queries return above"),
    };
    let pages = if matches!(target.query, Query::Post(_)) {
        1
    } else {
        max_pages.max(1)
    };
    let mut output = Vec::new();
    for page in 1..=pages {
        let mut endpoint = base.join("post.json")?;
        endpoint
            .query_pairs_mut()
            .append_pair("tags", &tags)
            .append_pair("limit", "100")
            .append_pair("page", &page.to_string());
        let response = client.get(endpoint).send().await?.error_for_status()?;
        let body: Value = response.json().await?;
        let short_page = body
            .as_array()
            .ok_or("Moebooru API returned non-array JSON")?
            .len()
            < 100;
        output.extend(parse_items(&body, target.category)?);
        if short_page || matches!(target.query, Query::Post(_)) {
            break;
        }
        if page < pages {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn matches_only_known_hosts_and_supported_routes() {
        assert_eq!(
            match_url("https://konachan.net/post/show/123"),
            Some(Target {
                host: "konachan.com",
                category: "konachan",
                query: Query::Post(123)
            })
        );
        assert_eq!(
            match_url("https://yande.re/post?tags=sky+blue"),
            Some(Target {
                host: "yande.re",
                category: "yandere",
                query: Query::Tags("sky blue".into())
            })
        );
        assert_eq!(
            match_url("https://www.sakugabooru.com/pool/show/25"),
            Some(Target {
                host: "www.sakugabooru.com",
                category: "sakugabooru",
                query: Query::Pool(25)
            })
        );
        assert!(match_url("https://lolibooru.moe/post/show/0").is_none());
        assert_eq!(
            match_url("https://yande.re/post/popular_by_day?year=2025&month=10"),
            Some(Target {
                host: "yande.re", category: "yandere",
                query: Query::Popular {
                    feed: "popular_by_day",
                    parameters: vec![("year".into(), "2025".into()), ("month".into(), "10".into())]
                }
            })
        );
        assert!(match_url("https://yande.re/post/popular_by_hour").is_none());
        assert!(match_url("http://yande.re/post/show/1").is_none());
        assert!(match_url("https://untrusted.example/post/show/1").is_none());
    }

    #[test]
    fn parses_valid_media_and_skips_bad_records() {
        let posts = json!([
            {"id": 9, "md5": "0123456789abcdef0123456789abcdef",
             "file_url": "https://files.example.org/item.jpg?signature=not-in-filename"},
            {"id": 10, "file_url": "https://files.example.org/animation.webm"},
            {"id": 11, "file_url": "javascript:invalid"},
            {"id": 12, "file_url": null}
        ]);
        let files = parse_items(&posts, "konachan").unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(
            files[0].filename,
            "konachan_9_0123456789abcdef0123456789abcdef.jpg"
        );
        assert_eq!(files[1].filename, "konachan_10.webm");
        assert!(parse_items(&json!({"posts": []}), "konachan").is_err());
    }

    #[tokio::test]
    async fn local_post_api_uses_expected_id_filter() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = [0_u8; 4096];
            let n = socket.read(&mut bytes).await.unwrap();
            let request = String::from_utf8_lossy(&bytes[..n]);
            assert!(request.starts_with("GET /post.json?"), "{request}");
            assert!(request.contains("tags=id%3A44"), "{request}");
            assert!(request.contains("page=1"), "{request}");
            let payload = r#"[{"id":44,"file_url":"https://img.example.org/test.webp"}]"#;
            let answer = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{payload}",
                payload.len()
            );
            socket.write_all(answer.as_bytes()).await.unwrap();
        });
        let api = Url::parse(&format!("http://{addr}/")).unwrap();
        let target = Target {
            host: "yande.re",
            category: "yandere",
            query: Query::Post(44),
        };
        let files = resolve_at_base(&Client::new(), target, 1, &api)
            .await
            .unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].filename, "yandere_44.webp");
        server.await.unwrap();
    }
    #[tokio::test]
    async fn pool_query_resolves_bounded_page_and_skips_missing_media() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = [0_u8; 4096];
            let n = socket.read(&mut bytes).await.unwrap();
            let request = String::from_utf8_lossy(&bytes[..n]);
            assert!(request.starts_with("GET /post.json?"), "{request}");
            assert!(request.contains("tags=pool%3A25"), "{request}");
            assert!(request.contains("page=1"), "{request}");
            let payload = r#"[{"id":44,"file_url":"https://img.example.org/test.png"},{"id":45,"file_url":null}]"#;
            let answer = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{payload}",
                payload.len()
            );
            socket.write_all(answer.as_bytes()).await.unwrap();
        });
        let api = Url::parse(&format!("http://{addr}/")).unwrap();
        let target = Target {
            host: "konachan.com",
            category: "konachan",
            query: Query::Pool(25),
        };
        let files = resolve_at_base(&Client::new(), target, 3, &api)
            .await
            .unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].filename, "konachan_44.png");
        server.await.unwrap();
    }
    #[tokio::test]
    async fn local_popular_feed_matches_upstream_json_endpoint() {
        use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::TcpListener};
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = [0_u8; 4096];
            let count = socket.read(&mut bytes).await.unwrap();
            let request = String::from_utf8_lossy(&bytes[..count]);
            assert!(request.starts_with("GET /post/popular_by_week.json?"), "{request}");
            assert!(request.contains("year=2025"), "{request}");
            assert!(request.contains("month=10"), "{request}");
            let payload = r#"[{"id":84,"file_url":"https://img.example.org/pic.jpg"}]"#;
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                payload.len()
            );
            socket.write_all(reply.as_bytes()).await.unwrap();
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let target = Target {
            host: "yande.re",
            category: "yandere",
            query: Query::Popular {
                feed: "popular_by_week",
                parameters: vec![("year".into(), "2025".into()), ("month".into(), "10".into())],
            },
        };
        let items = resolve_at_base(&Client::new(), target, 9, &base).await.unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].filename, "yandere_84.jpg");
        server.await.unwrap();
    }

}
