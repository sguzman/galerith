//! Philomena JSON API extraction preview for Derpibooru, Ponybooru, Furbooru.
//! The full upstream implementation includes filter configuration, accounts,
//! search field semantics, tag pages, SVG conversions and rich metadata.
use crate::{safe_filename, MediaItem};
use reqwest::Client;
use serde_json::Value;
use std::{error::Error, time::Duration};
use url::Url;

const PHILOMENA_JSON_RESPONSE_LIMIT: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Query {
    Image(u64),
    Search(String),
    Gallery(u64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    host: &'static str,
    category: &'static str,
    filter_id: &'static str,
    query: Query,
}

fn host_info(host: &str) -> Option<(&'static str, &'static str, &'static str)> {
    match host {
        "derpibooru.org" | "www.derpibooru.org" => Some(("derpibooru.org", "derpibooru", "56027")),
        "ponybooru.org" | "www.ponybooru.org" => Some(("ponybooru.org", "ponybooru", "3")),
        "furbooru.org" => Some(("furbooru.org", "furbooru", "2")),
        _ => None,
    }
}

pub fn match_url(input: &str) -> Option<Target> {
    let url = Url::parse(input).ok()?;
    if url.scheme() != "https" {
        return None;
    }
    let (host, category, filter_id) = host_info(url.host_str()?)?;
    let segments: Vec<_> = url.path_segments()?.filter(|x| !x.is_empty()).collect();
    let query = match segments.as_slice() {
        ["tags", slug] => {
            let decoded = percent_encoding::percent_decode_str(slug)
                .decode_utf8()
                .ok()?;
            let mut query = decoded.replace('+', " ");
            for (from, to) in [
                ("-colon-", ":"),
                ("-dash-", "-"),
                ("-dot-", "."),
                ("-plus-", "+"),
                ("-fwslash-", "/"),
                ("-bwslash-", "\\"),
            ] {
                query = query.replace(from, to);
            }
            if query.is_empty() || query.len() > 512 {
                return None;
            }
            Query::Search(query)
        }
        ["search"] => Query::Search(
            url.query_pairs()
                .find(|(k, _)| k == "q")
                .map(|(_, v)| v.into_owned())?,
        ),
        ["images", id] | [id] => {
            let id = id.parse::<u64>().ok()?;
            if id == 0 {
                return None;
            }
            Query::Image(id)
        }
        ["galleries", id] => {
            let id = id.parse::<u64>().ok()?;
            if id == 0 {
                return None;
            }
            Query::Gallery(id)
        }
        _ => return None,
    };
    Some(Target {
        host,
        category,
        filter_id,
        query,
    })
}

fn parse_image(item: &Value, category: &'static str) -> Option<MediaItem> {
    let id = item.get("id")?.as_u64()?;
    let raw = item
        .get("representations")
        .and_then(|v| v.get("full"))
        .and_then(Value::as_str)
        .or_else(|| item.get("view_url").and_then(Value::as_str))?;
    let mut url = Url::parse(raw).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    if item.get("format").and_then(Value::as_str) == Some("svg") {
        let (stem, _) = url.path().rsplit_once('.')?;
        let svg_path = format!("{stem}.svg");
        url.set_path(&svg_path);
    }
    let ext = url.path().rsplit_once('.')?.1;
    if ext.is_empty() || ext.len() > 12 || !ext.bytes().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    Some(MediaItem {
        url: url.to_string(),
        filename: safe_filename(&format!("{id}.{ext}")),
        extractor: category,
        post_id: Some(id),
    })
}

fn images(
    data: &Value,
    category: &'static str,
) -> Result<(Vec<MediaItem>, usize), Box<dyn Error + Send + Sync>> {
    let posts = data
        .get("images")
        .and_then(Value::as_array)
        .ok_or("Philomena search response missing image array")?;
    Ok((
        posts
            .iter()
            .filter_map(|item| parse_image(item, category))
            .collect(),
        posts.len(),
    ))
}

pub async fn resolve(
    client: &Client,
    target: Target,
    max_pages: u16,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let api = Url::parse(&format!("https://{}/api/v1/json/", target.host))?;
    resolve_at_base(client, target, max_pages, &api).await
}

/// Safe file-only finite selector pushdown for native image/search/gallery
/// requests. Source pages are still counted by their raw image records.
pub async fn resolve_limited(
    client: &Client,
    target: Target,
    max_pages: u16,
    max_items: usize,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let api = Url::parse(&format!("https://{}/api/v1/json/", target.host))?;
    resolve_at_base_limited(client, target, max_pages, &api, max_items.max(1)).await
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
    max_items: usize,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    if let Query::Image(id) = &target.query {
        let endpoint = base.join(&format!("images/{id}"))?;
        let body: Value =
            super::http_json::fetch_json_limited(client, endpoint, PHILOMENA_JSON_RESPONSE_LIMIT)
                .await?;
        let image = body
            .get("image")
            .ok_or("Philomena API did not return an image")?;
        return Ok(parse_image(image, target.category).into_iter().collect());
    }
    let query = match &target.query {
        Query::Search(tags) => tags.clone(),
        Query::Gallery(id) => format!("gallery_id:{id}"),
        Query::Image(_) => unreachable!(),
    };
    let mut results = Vec::new();
    for page in 1..=max_pages.max(1) {
        let mut endpoint = base.join("search/images")?;
        endpoint
            .query_pairs_mut()
            .append_pair("q", &query)
            .append_pair("filter_id", target.filter_id)
            .append_pair("per_page", "50")
            .append_pair("page", &page.to_string());
        let body: Value =
            super::http_json::fetch_json_limited(client, endpoint, PHILOMENA_JSON_RESPONSE_LIMIT)
                .await?;
        let (fresh, count) = images(&body, target.category)?;
        for item in fresh {
            results.push(item);
            if results.len() >= max_items {
                return Ok(results);
            }
        }
        if count < 50 {
            break;
        }
        if page < max_pages {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn recognizes_only_supported_instances_and_explicit_routes() {
        assert_eq!(
            match_url("https://www.derpibooru.org/images/77"),
            Some(Target {
                host: "derpibooru.org",
                category: "derpibooru",
                filter_id: "56027",
                query: Query::Image(77)
            })
        );
        assert_eq!(
            match_url("https://ponybooru.org/search?q=morning+light"),
            Some(Target {
                host: "ponybooru.org",
                category: "ponybooru",
                filter_id: "3",
                query: Query::Search("morning light".into())
            })
        );
        assert_eq!(
            match_url("https://furbooru.org/galleries/50"),
            Some(Target {
                host: "furbooru.org",
                category: "furbooru",
                filter_id: "2",
                query: Query::Gallery(50)
            })
        );
        assert_eq!(
            match_url("https://derpibooru.org/tags/hello-colon-world-dash-blue"),
            Some(Target {
                host: "derpibooru.org",
                category: "derpibooru",
                filter_id: "56027",
                query: Query::Search("hello:world-blue".into()),
            })
        );
        assert!(match_url("https://derpibooru.org/tags/%FF").is_none());
        assert!(match_url("http://derpibooru.org/images/77").is_none());
        assert!(match_url("https://otherderpibooru.org/images/77").is_none());
    }

    #[test]
    fn parses_images_with_representations_and_view_fallback() {
        let data = json!({"images": [
            {"id": 5, "representations": {"full": "https://img.example.org/5.jpg"}},
            {"id": 6, "view_url": "https://img.example.org/6.png"},
            {"id": 7, "view_url": "javascript:bad"},
            {"id": 8, "view_url": null},
            {"id": 9, "format": "svg", "representations": {"full": "https://img.example.org/9.png"}}
        ]});
        let (files, count) = images(&data, "derpibooru").unwrap();
        assert_eq!(count, 5);
        assert_eq!(files.len(), 3);
        assert_eq!(files[0].filename, "5.jpg");
        assert_eq!(files[1].filename, "6.png");
        assert_eq!(files[2].filename, "9.svg");
        assert_eq!(files[2].url, "https://img.example.org/9.svg");
        assert!(images(&json!({"other": []}), "derpibooru").is_err());
    }

    #[tokio::test]
    async fn local_single_image_endpoint_and_returned_media() {
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
            assert!(request.starts_with("GET /images/44 HTTP/1.1"), "{request}");
            let body = r#"{"image":{"id":44,"representations":{"full":"https://img.example.org/44.jpg"}}}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let target = Target {
            host: "derpibooru.org",
            category: "derpibooru",
            filter_id: "56027",
            query: Query::Image(44),
        };
        let results = resolve_at_base(&Client::new(), target, 1, &base)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].filename, "44.jpg");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn gallery_search_uses_filter_and_pagination_fields() {
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
            assert!(request.starts_with("GET /search/images?"), "{request}");
            assert!(request.contains("q=gallery_id%3A100"), "{request}");
            assert!(request.contains("filter_id=2"), "{request}");
            assert!(request.contains("per_page=50"), "{request}");
            let body = r#"{"images":[{"id":50,"view_url":"https://cdn.example.org/50.webp"}]}"#;
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(reply.as_bytes()).await.unwrap();
        });
        let api = Url::parse(&format!("http://{addr}/")).unwrap();
        let target = Target {
            host: "furbooru.org",
            category: "furbooru",
            filter_id: "2",
            query: Query::Gallery(100),
        };
        let results = resolve_at_base(&Client::new(), target, 5, &api)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].filename, "50.webp");
        server.await.unwrap();
    }
    #[tokio::test]
    async fn finite_gallery_limit_skips_later_full_api_pages() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 4096];
            let n = socket.read(&mut request).await.unwrap();
            let text = String::from_utf8_lossy(&request[..n]);
            assert!(text.contains("q=gallery_id%3A100"), "{text}");
            assert!(text.contains("page=1"), "{text}");
            let image_rows: Vec<_> = (1..=50)
                .map(|id| json!({"id":id,"view_url":format!("https://cdn.example.org/{id}.jpg")}))
                .collect();
            let body = json!({"images":image_rows}).to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let target = Target {
            host: "furbooru.org",
            category: "furbooru",
            filter_id: "2",
            query: Query::Gallery(100),
        };
        let images = resolve_at_base_limited(&Client::new(), target, 3, &base, 2)
            .await
            .unwrap();
        assert_eq!(images.len(), 2);
        assert_eq!(images[0].post_id, Some(1));
        assert_eq!(images[1].post_id, Some(2));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn finite_search_limit_counts_only_usable_media_across_full_pages() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for page in [1, 2] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0_u8; 4096];
                let n = socket.read(&mut request).await.unwrap();
                let text = String::from_utf8_lossy(&request[..n]);
                assert!(text.contains(&format!("page={page}")), "{text}");
                let posts: Vec<_> = if page == 1 {
                    (1..=50)
                        .map(|id| {
                            if id == 1 {
                                json!({"id":id,"view_url":"https://cdn.example.org/1.jpg"})
                            } else {
                                json!({"id":id})
                            }
                        })
                        .collect()
                } else {
                    vec![json!({"id":51,"view_url":"https://cdn.example.org/51.jpg"})]
                };
                let body = json!({"images":posts}).to_string();
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let target = Target {
            host: "derpibooru.org",
            category: "derpibooru",
            filter_id: "56027",
            query: Query::Search("fox".into()),
        };
        let images = resolve_at_base_limited(&Client::new(), target, 3, &base, 2)
            .await
            .unwrap();
        assert_eq!(images.len(), 2);
        assert_eq!(images[0].post_id, Some(1));
        assert_eq!(images[1].post_id, Some(51));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn tag_slug_resolves_to_search_query_in_local_api() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = [0_u8; 4096];
            let read = socket.read(&mut bytes).await.unwrap();
            let request = String::from_utf8_lossy(&bytes[..read]);
            assert!(request.starts_with("GET /search/images?"), "{request}");
            assert!(request.contains("q=hello%3Aworld"), "{request}");
            assert!(request.contains("filter_id=56027"), "{request}");
            let body = r#"{"images":[{"id":66,"view_url":"https://img.example.org/66.png"}]}"#;
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(reply.as_bytes()).await.unwrap();
        });
        let api = Url::parse(&format!("http://{address}/")).unwrap();
        let target = match_url("https://derpibooru.org/tags/hello-colon-world").unwrap();
        let result = resolve_at_base(&Client::new(), target, 3, &api)
            .await
            .unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].filename, "66.png");
        server.await.unwrap();
    }
}
