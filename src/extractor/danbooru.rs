//! Partial Danbooru read-only post, tag, pool, favorite-group, media-asset, artist and popular extraction.
//! API reference: https://safebooru.donmai.us/wiki_pages/api%3Aposts
//! Module status is PARTIAL until gallery-dl parity is verified.
use crate::{safe_filename, MediaItem};
use reqwest::Client;
use serde_json::Value;
use std::{collections::HashMap, error::Error, time::Duration};
use url::Url;

const DANBOORU_PER_PAGE: usize = 200;
const DANBOORU_PAGE_THRESHOLD: usize = DANBOORU_PER_PAGE - 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Post {
        host: String,
        id: u64,
    },
    Random {
        host: String,
        tags: String,
    },
    Search {
        host: String,
        tags: String,
        page: u16,
    },
    Collection {
        host: String,
        id: u64,
        kind: CollectionKind,
    },
    MediaAsset {
        host: String,
        id: u64,
    },
    Artist {
        host: String,
        id: u64,
    },
    Popular {
        host: String,
        parameters: Vec<(String, String)>,
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
        ["posts", "random"] | ["posts", "random.json"] => {
            let tags = url
                .query_pairs()
                .find(|(key, _)| key == "tags")
                .map(|(_, value)| value.into_owned())
                .unwrap_or_default();
            Some(Target::Random { host, tags })
        }
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
        ["artists", id] => {
            let id = id.parse::<u64>().ok()?;
            (id > 0).then_some(Target::Artist { host, id })
        }
        ["media_assets", id] => {
            let id = id.parse::<u64>().ok()?;
            (id > 0).then_some(Target::MediaAsset { host, id })
        }
        ["popular"] | ["explore", "posts", "popular"] => {
            let parameters: Vec<(String, String)> = url
                .query_pairs()
                .map(|(key, value)| (key.into_owned(), value.into_owned()))
                .collect();
            if parameters.len() > 12
                || parameters.iter().any(|(key, value)| {
                    key.is_empty()
                        || key.len() > 32
                        || value.len() > 128
                        || !key
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
                        || (key == "page" && value.parse::<u16>().ok().filter(|n| *n > 0).is_none())
                })
            {
                return None;
            }
            Some(Target::Popular { host, parameters })
        }
        ["posts"] | ["posts.json"] => {
            let mut tags = String::new();
            let mut page = 1;
            for (key, value) in url.query_pairs() {
                match key.as_ref() {
                    "tags" => tags = value.into_owned(),
                    "page" => page = value.parse::<u16>().ok().filter(|p| *p > 0)?,
                    _ => {}
                }
            }
            Some(Target::Search { host, tags, page })
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
        | Target::Random { host, .. }
        | Target::Search { host, .. }
        | Target::Collection { host, .. }
        | Target::MediaAsset { host, .. }
        | Target::Artist { host, .. }
        | Target::Popular { host, .. } => host,
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
            .append_pair("limit", &DANBOORU_PER_PAGE.to_string())
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
        if posts.len() < DANBOORU_PAGE_THRESHOLD {
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

async fn resolve_media_asset(
    client: &Client,
    base: &Url,
    id: u64,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let endpoint = base.join(&format!("media_assets/{id}.json"))?;
    let body: Value = client
        .get(endpoint)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let variants = body
        .get("variants")
        .and_then(Value::as_array)
        .ok_or("Danbooru media asset response missing variants")?;
    let raw = variants
        .last()
        .and_then(|item| item.get("url"))
        .and_then(Value::as_str)
        .ok_or("Danbooru media asset has no original media URL")?;
    let url = Url::parse(raw).or_else(|_| base.join(raw))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("unsafe Danbooru media asset URL".into());
    }
    let ext = url
        .path()
        .rsplit_once('.')
        .map(|(_, ext)| ext)
        .ok_or("Danbooru media asset missing file extension")?;
    if ext.is_empty() || ext.len() > 16 || !ext.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err("Danbooru media asset extension is invalid".into());
    }
    let post = serde_json::json!({
        "id": id,
        "file_url": url.to_string(),
        "file_ext": ext,
    });
    post_items(&post, base)
}

async fn resolve_popular(
    client: &Client,
    base: &Url,
    parameters: &[(String, String)],
    max_pages: u16,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let start = parameters
        .iter()
        .find(|(key, _)| key == "page")
        .map(|(_, value)| value.parse::<u16>())
        .transpose()?
        .unwrap_or(1);
    let mut items = Vec::new();
    for offset in 0..max_pages.max(1) {
        let Some(page) = start.checked_add(offset) else {
            break;
        };
        let mut endpoint = base.join("explore/posts/popular.json")?;
        {
            let mut query = endpoint.query_pairs_mut();
            for (key, value) in parameters {
                if key != "page" && key != "limit" {
                    query.append_pair(key, value);
                }
            }
            query
                .append_pair("limit", &DANBOORU_PER_PAGE.to_string())
                .append_pair("page", &page.to_string());
        }
        let body: Value = client
            .get(endpoint)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let count = body
            .as_array()
            .ok_or("invalid Danbooru popular posts response")?
            .len();
        if count == 0 {
            break;
        }
        items.extend(post_items(&body, base)?);
        if count < DANBOORU_PAGE_THRESHOLD {
            break;
        }
        if offset + 1 < max_pages {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
    Ok(items)
}

async fn resolve_tag_posts(
    client: &Client,
    base: &Url,
    tags: &str,
    start_page: u16,
    max_pages: u16,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let mut items = Vec::new();
    for offset in 0..max_pages.max(1) {
        let Some(page) = start_page.checked_add(offset) else {
            break;
        };
        let mut endpoint = base.join("posts.json")?;
        endpoint
            .query_pairs_mut()
            .append_pair("tags", tags)
            .append_pair("limit", &DANBOORU_PER_PAGE.to_string())
            .append_pair("page", &page.to_string());
        let body: Value = client
            .get(endpoint)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let count = body
            .as_array()
            .ok_or("unexpected Danbooru posts response")?
            .len();
        if count == 0 {
            break;
        }
        items.extend(post_items(&body, base)?);
        if count < DANBOORU_PAGE_THRESHOLD {
            break;
        }
        if offset + 1 < max_pages {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
    Ok(items)
}

async fn resolve_artist(
    client: &Client,
    base: &Url,
    id: u64,
    max_pages: u16,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let endpoint = base.join(&format!("artists/{id}.json"))?;
    let artist: Value = client
        .get(endpoint)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let name = artist
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty() && name.len() <= 512)
        .ok_or("Danbooru artist response missing a usable name")?;
    resolve_tag_posts(client, base, name, 1, max_pages).await
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
        Target::MediaAsset { id, .. } => resolve_media_asset(client, base, id).await,
        Target::Artist { id, .. } => resolve_artist(client, base, id, max_pages).await,
        Target::Popular { parameters, .. } => {
            resolve_popular(client, base, &parameters, max_pages).await
        }
        Target::Collection { id, kind, .. } => {
            resolve_collection(client, base, id, kind, max_pages).await
        }
        Target::Random { tags, .. } => {
            let mut endpoint = base.join("posts/random.json")?;
            if !tags.is_empty() {
                endpoint.query_pairs_mut().append_pair("tags", &tags);
            }
            let body: Value = client
                .get(endpoint)
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            post_items(&body, base)
        }
        Target::Post { id, .. } => {
            let endpoint = base.join(&format!("posts/{id}.json"))?;
            let response = client.get(endpoint).send().await?.error_for_status()?;
            let body: Value = response.json().await?;
            post_items(&body, base)
        }
        Target::Search { tags, page, .. } => {
            resolve_tag_posts(client, base, &tags, page, max_pages).await
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
                tags: "blue sky".into(),
                page: 1
            })
        );
        assert_eq!(
            match_url("https://danbooru.donmai.us/posts?tags=fox&page=2"),
            Some(Target::Search {
                host: "danbooru.donmai.us".into(),
                tags: "fox".into(),
                page: 2
            })
        );
        assert!(match_url("https://danbooru.donmai.us/posts?tags=fox&page=0").is_none());
        assert!(match_url("https://danbooru.donmai.us/posts?tags=fox&page=bogus").is_none());
        assert_eq!(
            match_url("https://danbooru.donmai.us/posts/random?tags=blue+fox"),
            Some(Target::Random {
                host: "danbooru.donmai.us".into(),
                tags: "blue fox".into()
            })
        );
        assert_eq!(
            match_url("https://safebooru.donmai.us/posts/random.json"),
            Some(Target::Random {
                host: "safebooru.donmai.us".into(),
                tags: String::new()
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
                    "GET /posts.json?tags=ordpool%3A42&limit=200&page=1 HTTP/1.1",
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
                    "GET /posts.json?tags=ordfavgroup%3A71&limit=200&page=1 HTTP/1.1",
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
    fn media_assets_and_popular_urls_are_strictly_matched() {
        assert_eq!(
            match_url("https://danbooru.donmai.us/media_assets/22"),
            Some(Target::MediaAsset {
                host: "danbooru.donmai.us".into(),
                id: 22
            })
        );
        assert_eq!(
            match_url("https://safebooru.donmai.us/explore/posts/popular?scale=week"),
            Some(Target::Popular {
                host: "safebooru.donmai.us".into(),
                parameters: vec![("scale".into(), "week".into())],
            })
        );
        assert!(match_url("https://danbooru.donmai.us/media_assets/0").is_none());
        assert!(match_url("https://danbooru.donmai.us/explore/posts/popular?page=0").is_none());
        assert!(match_url("https://imposter.donmai.us/media_assets/22").is_none());
        assert_eq!(
            match_url("https://danbooru.donmai.us/artists/33"),
            Some(Target::Artist {
                host: "danbooru.donmai.us".into(),
                id: 33
            })
        );
        assert!(match_url("https://safebooru.donmai.us/artists/0").is_none());
    }

    #[tokio::test]
    async fn media_asset_uses_last_variant_from_json_api() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let n = socket.read(&mut buf).await.unwrap();
            let req = String::from_utf8_lossy(&buf[..n]);
            assert!(
                req.starts_with("GET /media_assets/22.json HTTP/1.1"),
                "{req}"
            );
            let body = r#"{"variants":[{"url":"https://cdn.example.org/preview.jpg"},{"url":"https://cdn.example.org/original.webp?token=abc"}]}"#;
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(reply.as_bytes()).await.unwrap();
        });
        let base = Url::parse(&format!("http://{address}/")).unwrap();
        let media = resolve_media_asset(&Client::new(), &base, 22)
            .await
            .unwrap();
        assert_eq!(media.len(), 1);
        assert_eq!(media[0].filename, "22.webp");
        assert_eq!(
            media[0].url,
            "https://cdn.example.org/original.webp?token=abc"
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn popular_feed_uses_expected_query_and_json_api_path() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let n = socket.read(&mut buf).await.unwrap();
            let req = String::from_utf8_lossy(&buf[..n]);
            assert!(req.starts_with("GET /explore/posts/popular.json?"), "{req}");
            assert!(req.contains("scale=week"), "{req}");
            assert!(req.contains("page=3"), "{req}");
            let body =
                r#"[{"id":60,"file_url":"https://cdn.example.org/60.jpg","file_ext":"jpg"}]"#;
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(reply.as_bytes()).await.unwrap();
        });
        let base = Url::parse(&format!("http://{address}/")).unwrap();
        let params = [
            ("scale".to_owned(), "week".to_owned()),
            ("page".to_owned(), "3".to_owned()),
        ];
        let media = resolve_popular(&Client::new(), &base, &params, 1)
            .await
            .unwrap();
        assert_eq!(media.len(), 1);
        assert_eq!(media[0].filename, "60.jpg");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn artist_record_resolves_into_a_bounded_tag_query() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (needle, body) in [
                (
                    "GET /artists/33.json HTTP/1.1",
                    r#"{"id":33,"name":"blue_fox"}"#,
                ),
                (
                    "GET /posts.json?tags=blue_fox&limit=200&page=1 HTTP/1.1",
                    r#"[{"id":15,"file_ext":"png","file_url":"https://cdn.example.org/15.png"}]"#,
                ),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = [0_u8; 4096];
                let n = socket.read(&mut bytes).await.unwrap();
                let request = String::from_utf8_lossy(&bytes[..n]);
                assert!(request.starts_with(needle), "{request}");
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{address}/")).unwrap();
        let result = resolve_artist(&Client::new(), &base, 33, 1).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].filename, "15.png");
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
    async fn random_post_uses_native_json_api_with_optional_tags() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (route, body) in [
                (
                    "GET /posts/random.json?tags=blue+fox HTTP/1.1",
                    r#"{"id":71,"file_url":"https://cdn.example.org/71.jpg","file_ext":"jpg"}"#,
                ),
                (
                    "GET /posts/random.json HTTP/1.1",
                    r#"[{"id":72,"file_url":"https://cdn.example.org/72.png","file_ext":"png"}]"#,
                ),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buf = [0_u8; 4096];
                let n = socket.read(&mut buf).await.unwrap();
                let request = String::from_utf8_lossy(&buf[..n]);
                assert!(request.starts_with(route), "{request}");
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{address}/")).unwrap();
        for (tags, expected_id) in [("blue fox", 71), ("", 72)] {
            let target = Target::Random {
                host: "danbooru.donmai.us".into(),
                tags: tags.into(),
            };
            let items = resolve_at_base(&Client::new(), target, 1, &base)
                .await
                .unwrap();
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].post_id, Some(expected_id));
        }
        server.await.unwrap();
    }

    #[tokio::test]
    async fn short_tag_and_popular_pages_do_not_trigger_extra_requests() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for route in [
                "GET /posts.json?tags=fox&limit=200&page=1 HTTP/1.1",
                "GET /explore/posts/popular.json?limit=200&page=1 HTTP/1.1",
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buf = [0_u8; 4096];
                let n = socket.read(&mut buf).await.unwrap();
                let request = String::from_utf8_lossy(&buf[..n]);
                assert!(request.starts_with(route), "{request}");
                let body =
                    r#"[{"id":78,"file_url":"https://cdn.example.org/78.jpg","file_ext":"jpg"}]"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{address}/")).unwrap();
        let tags = resolve_tag_posts(&Client::new(), &base, "fox", 1, 5)
            .await
            .unwrap();
        let popular = resolve_popular(&Client::new(), &base, &[], 5)
            .await
            .unwrap();
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].post_id, Some(78));
        assert_eq!(popular.len(), 1);
        assert_eq!(popular[0].post_id, Some(78));
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
            for page in 2..=3 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut data = [0_u8; 4096];
                let read = socket.read(&mut data).await.unwrap();
                let request = String::from_utf8_lossy(&data[..read]);
                assert!(request.starts_with("GET /posts.json?"), "{request}");
                assert!(request.contains("tags=blue+sky"), "{request}");
                assert!(request.contains(&format!("page={page}")), "{request}");
                assert!(request.contains("limit=200"), "{request}");
                let body = if page == 2 {
                    let posts: Vec<_> = (1..=DANBOORU_PER_PAGE)
                        .map(|id| {
                            json!({
                                "id": id,
                                "file_url": format!("https://cdn.example.org/{id}.jpg"),
                                "file_ext": "jpg"
                            })
                        })
                        .collect();
                    json!(posts).to_string()
                } else {
                    "[]".to_owned()
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
            page: 2,
        };
        let result = resolve_at_base(&Client::new(), target, 5, &base)
            .await
            .unwrap();
        assert_eq!(result.len(), DANBOORU_PER_PAGE);
        assert_eq!(result[0].filename, "1.jpg");
        assert_eq!(result[0].post_id, Some(1));
        assert_eq!(result[DANBOORU_PER_PAGE - 1].post_id, Some(200));
        server.await.unwrap();
    }
}
