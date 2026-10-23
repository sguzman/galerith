//! Independent public Wallhaven JSON API extractor preview.
//! Wallhaven's public API allows unauthenticated SFW image/search routes.
//! No default/shared API key, account access, private collections, or live
//! parity with upstream gallery-dl is claimed.
use crate::{
    message::{queued_url, ExtractorMessage, Metadata},
    MediaItem,
};
use reqwest::Client;
use serde_json::Value;
use std::{collections::HashSet, error::Error, time::Duration};
use tokio::time::Instant;
use url::Url;

const API_ROOT: &str = "https://wallhaven.cc/api/";
const MAX_RESPONSE: usize = 2 * 1024 * 1024;
const MAX_MEDIA: usize = 2000;
const MAX_PAGES: u16 = 100;
const MAX_COLLECTIONS: usize = 32;
const MAX_COLLECTIONS_PER_PAGE: usize = 128;
// The official public API documents a 45 request/minute limit.
const PAGE_PACING: Duration = Duration::from_millis(1400);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Image { id: String },
    Search { query: String },
    Uploads { user: String },
    Collection { user: String, collection_id: String },
    Collections { user: String },
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
        ["user", user] | ["user", user, ""] | ["user", user, "uploads"]
            if valid_user(user) && url.query().is_none() =>
        {
            Some(Target::Uploads {
                user: (*user).into(),
            })
        }
        ["user", user, "favorites"] | ["user", user, "favorites", ""]
            if valid_user(user) && url.query().is_none() =>
        {
            Some(Target::Collections {
                user: (*user).into(),
            })
        }
        ["user", user, "favorites", collection_id]
            if valid_user(user) && valid_collection_id(collection_id) && url.query().is_none() =>
        {
            Some(Target::Collection {
                user: (*user).into(),
                collection_id: (*collection_id).into(),
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
        let listed = values
            .as_array()
            .ok_or("Wallhaven listing is not an array")?;
        if listed.len() > MAX_MEDIA {
            return Err("Wallhaven page exceeded the media count limit".into());
        }
        listed.iter().collect()
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

/// Every API request across a user-collection fan-out shares the same finite
/// budget and pacing timer. Counting individual HTTP requests is important:
/// max_pages must never accidentally become max_pages * collection_count.
struct ApiBudget {
    remaining: u16,
    last_request: Option<Instant>,
}

impl ApiBudget {
    fn new(max_pages: u16) -> Self {
        Self {
            remaining: max_pages.clamp(1, MAX_PAGES),
            last_request: None,
        }
    }

    async fn get(
        &mut self,
        client: &Client,
        endpoint: Url,
    ) -> Result<Option<Value>, Box<dyn Error + Send + Sync>> {
        if self.remaining == 0 {
            return Ok(None);
        }
        if let Some(last) = self.last_request {
            tokio::time::sleep_until(last + PAGE_PACING).await;
        }
        self.remaining -= 1;
        self.last_request = Some(Instant::now());
        Ok(Some(fetch_json(client, endpoint).await?))
    }
}

fn valid_collection_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 12 && id.bytes().all(|byte| byte.is_ascii_digit())
}

fn collection_ids(data: &Value) -> Result<Vec<String>, Box<dyn Error + Send + Sync>> {
    if data.get("error").is_some() {
        return Err("Wallhaven collections API returned an error".into());
    }
    let entries = data
        .get("data")
        .and_then(Value::as_array)
        .ok_or("Wallhaven collections response is not an array")?;
    if entries.len() > MAX_COLLECTIONS_PER_PAGE {
        return Err("Wallhaven collections page exceeded the record limit".into());
    }
    let mut ids = Vec::new();
    for entry in entries {
        let Some(id) = entry.get("id") else {
            continue;
        };
        let value = match id {
            Value::String(value) => value.clone(),
            Value::Number(value) => value.to_string(),
            _ => continue,
        };
        if valid_collection_id(&value) {
            ids.push(value);
        }
    }
    Ok(ids)
}

fn list_next_page(data: &Value, page: u16) -> Result<bool, Box<dyn Error + Send + Sync>> {
    let Some(meta) = data.get("meta") else {
        return Ok(false);
    };
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
    Ok(current < last)
}

async fn append_regular(
    client: &Client,
    base: &Url,
    target: &Target,
    budget: &mut ApiBudget,
    items: &mut Vec<MediaItem>,
    seen: &mut HashSet<String>,
    max_items: usize,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    if items.len() >= max_items {
        return Ok(());
    }
    for page in 1..=MAX_PAGES {
        let mut endpoint = match target {
            Target::Image { id } => base.join(&format!("v1/w/{id}"))?,
            Target::Search { .. } | Target::Uploads { .. } => base.join("v1/search")?,
            Target::Collection {
                user,
                collection_id,
            } => base.join(&format!("v1/collections/{user}/{collection_id}"))?,
            Target::Collections { .. } => return Err("collection index needs expansion".into()),
        };
        match target {
            Target::Image { .. } => {}
            Target::Search { query } => {
                endpoint
                    .query_pairs_mut()
                    .append_pair("q", query)
                    .append_pair("page", &page.to_string());
            }
            Target::Uploads { user } => {
                endpoint
                    .query_pairs_mut()
                    .append_pair("q", &format!("@{user}"))
                    .append_pair("page", &page.to_string());
            }
            Target::Collection { .. } => {
                endpoint
                    .query_pairs_mut()
                    .append_pair("page", &page.to_string());
            }
            Target::Collections { .. } => unreachable!(),
        }
        let Some(data) = budget.get(client, endpoint).await? else {
            break;
        };
        for (id, item) in parse_items(&data, matches!(target, Target::Image { .. }))? {
            if seen.insert(id) {
                items.push(item);
                if items.len() >= max_items {
                    return Ok(());
                }
            }
        }
        if matches!(target, Target::Image { .. }) || !list_next_page(&data, page)? {
            break;
        }
    }
    Ok(())
}

async fn discover_collection_ids(
    client: &Client,
    base: &Url,
    user: &str,
    budget: &mut ApiBudget,
    reserve_media_page: bool,
) -> Result<Vec<String>, Box<dyn Error + Send + Sync>> {
    let mut ids = Vec::new();
    let mut seen_collections = HashSet::new();
    for page in 1..=MAX_PAGES {
        let mut endpoint = base.join(&format!("v1/collections/{user}"))?;
        if page > 1 {
            endpoint
                .query_pairs_mut()
                .append_pair("page", &page.to_string());
        }
        let Some(data) = budget.get(client, endpoint).await? else {
            break;
        };
        for id in collection_ids(&data)? {
            if seen_collections.insert(id.clone()) {
                ids.push(id);
                if ids.len() >= MAX_COLLECTIONS {
                    break;
                }
            }
        }
        // Flattened downloads reserve a request for media; a queue-only
        // inspection can use its whole budget on collection discovery.
        if ids.len() >= MAX_COLLECTIONS
            || (reserve_media_page && budget.remaining <= 1)
            || !list_next_page(&data, page)?
        {
            break;
        }
    }
    Ok(ids)
}

async fn append_user_collections(
    client: &Client,
    base: &Url,
    user: &str,
    budget: &mut ApiBudget,
    items: &mut Vec<MediaItem>,
    seen_media: &mut HashSet<String>,
    max_items: usize,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let ids = discover_collection_ids(client, base, user, budget, true).await?;
    // The downloader still flattens collections. The message-inspection
    // route below preserves their deferred queue boundaries instead.
    for collection_id in ids {
        if budget.remaining == 0 || items.len() >= max_items {
            break;
        }
        append_regular(
            client,
            base,
            &Target::Collection {
                user: user.to_owned(),
                collection_id,
            },
            budget,
            items,
            seen_media,
            max_items,
        )
        .await?;
    }
    Ok(())
}

/// Emit deferred child extractors for public collection indexes, without
/// fetching their files. This intentionally does not invent collection titles
/// or metadata absent from the source records.
async fn queue_collections_at_base(
    client: &Client,
    base: &Url,
    user: &str,
    max_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    if !valid_user(user) {
        return Err("invalid Wallhaven username".into());
    }
    let mut budget = ApiBudget::new(max_pages);
    let ids = discover_collection_ids(client, base, user, &mut budget, false).await?;
    Ok(ids
        .into_iter()
        .map(|id| {
            let mut metadata = Metadata::new();
            metadata.insert("category".into(), "wallhaven".into());
            metadata.insert("subcategory".into(), "collection".into());
            metadata.insert("username".into(), user.into());
            metadata.insert("collection_id".into(), id.clone().into());
            queued_url(
                format!("https://wallhaven.cc/user/{user}/favorites/{id}"),
                metadata,
            )
        })
        .collect())
}

pub async fn queued_collections(
    client: &Client,
    user: &str,
    max_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    queue_collections_at_base(client, &Url::parse(API_ROOT)?, user, max_pages).await
}

async fn resolve_at_base_limited(
    client: &Client,
    base: &Url,
    target: &Target,
    max_pages: u16,
    maximum_items: usize,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let limit = maximum_items.clamp(1, MAX_MEDIA);
    let mut items = Vec::new();
    let mut seen = HashSet::new();
    let mut budget = ApiBudget::new(max_pages);
    if matches!(target, Target::Collections { .. }) {
        // The discovery lookup is overhead, not a downloadable media page.
        // With CLI --max-pages=1, fetch the index and one media page.
        budget.remaining = budget.remaining.saturating_add(1);
    }
    match target {
        Target::Collections { user } => {
            append_user_collections(
                client,
                base,
                user,
                &mut budget,
                &mut items,
                &mut seen,
                limit,
            )
            .await?;
        }
        _ => {
            append_regular(
                client,
                base,
                target,
                &mut budget,
                &mut items,
                &mut seen,
                limit,
            )
            .await?;
        }
    }
    Ok(items)
}

/// Bounded native media extraction, stopping API pagination as soon as
/// the needed number of valid unique wallpapers has been collected.
pub async fn resolve_limited(
    client: &Client,
    target: Target,
    max_pages: u16,
    maximum_items: usize,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    resolve_at_base_limited(
        client,
        &Url::parse(API_ROOT)?,
        &target,
        max_pages,
        maximum_items,
    )
    .await
}

#[cfg(test)]
async fn resolve_at_base(
    client: &Client,
    base: &Url,
    target: &Target,
    max_pages: u16,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    resolve_at_base_limited(client, base, target, max_pages, MAX_MEDIA).await
}

pub async fn resolve(
    client: &Client,
    target: Target,
    max_pages: u16,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    resolve_at_base_limited(
        client,
        &Url::parse(API_ROOT)?,
        &target,
        max_pages,
        MAX_MEDIA,
    )
    .await
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
        assert_eq!(
            match_url("https://wallhaven.cc/user/example-1"),
            Some(Target::Uploads {
                user: "example-1".into()
            })
        );
        assert_eq!(
            match_url("https://wallhaven.cc/user/example-1/favorites"),
            Some(Target::Collections {
                user: "example-1".into()
            })
        );
        assert_eq!(
            match_url("https://wallhaven.cc/user/example-1/"),
            Some(Target::Uploads {
                user: "example-1".into()
            })
        );
        assert_eq!(
            match_url("https://wallhaven.cc/user/example-1/favorites/12345"),
            Some(Target::Collection {
                user: "example-1".into(),
                collection_id: "12345".into()
            })
        );
        for bad in [
            "http://wallhaven.cc/w/94x38z",
            "https://wallhaven.cc.evil.org/w/94x38z",
            "https://wallhaven.cc/w/94x38z?apikey=secret",
            "https://wallhaven.cc/search?q=blue&q=other",
            "https://wallhaven.cc/search?purity=111&q=blue",
            "https://wallhaven.cc/search?q=",
            "https://wallhaven.cc/user/alice/favorites/notanid",
            "https://wallhaven.cc/user/alice/favorites?api_key=secret",
            "https://wallhaven.cc/user/alice/favorites/12?apikey=secret",
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

    #[tokio::test]
    async fn finite_media_limit_avoids_fetching_later_search_pages() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 4096];
            let n = socket.read(&mut request).await.unwrap();
            let request = String::from_utf8_lossy(&request[..n]);
            assert!(request.starts_with("GET /api/v1/search?q=blue&page=1 HTTP/1.1"));
            let body = serde_json::json!({
                "data": [
                    {"id":"94x38z","path":"https://w.wallhaven.cc/full/94/wallhaven-94x38z.jpg"},
                    {"id":"ze1p56","path":"https://w.wallhaven.cc/full/ze/wallhaven-ze1p56.png"}
                ],
                "meta":{"current_page":1,"last_page":99}
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
            // Closing the listener forces an error if the extractor tries
            // to request page two despite already having enough media.
        });
        let base = Url::parse(&format!("http://{address}/api/")).unwrap();
        let items = resolve_at_base_limited(
            &Client::new(),
            &base,
            &Target::Search {
                query: "blue".into(),
            },
            5,
            1,
        )
        .await
        .unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].filename, "wallhaven-94x38z.jpg");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn finite_media_limit_counts_only_valid_unique_wallpapers() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (page, body) in [
                (
                    1,
                    serde_json::json!({
                        "data": [
                            {"id":"94x38z","path":"https://w.wallhaven.cc/full/94/wallhaven-94x38z.jpg"},
                            {"id":"94x38z","path":"https://w.wallhaven.cc/full/94/wallhaven-94x38z.jpg"},
                            {"id":"unsafe","path":"https://evil.example/full/unsafe.jpg"}
                        ],
                        "meta":{"current_page":1,"last_page":99}
                    }),
                ),
                (
                    2,
                    serde_json::json!({
                        "data": [
                            {"id":"94x38z","path":"https://w.wallhaven.cc/full/94/wallhaven-94x38z.jpg"},
                            {"id":"ze1p56","path":"https://w.wallhaven.cc/full/ze/wallhaven-ze1p56.png"}
                        ],
                        "meta":{"current_page":2,"last_page":99}
                    }),
                ),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0_u8; 4096];
                let n = socket.read(&mut request).await.unwrap();
                let request = String::from_utf8_lossy(&request[..n]);
                let url = Url::parse(&format!(
                    "http://localhost{}",
                    request
                        .lines()
                        .next()
                        .unwrap()
                        .split_whitespace()
                        .nth(1)
                        .unwrap()
                ))
                .unwrap();
                assert_eq!(url.path(), "/api/v1/search");
                assert_eq!(
                    url.query_pairs().find(|(key, _)| key == "page").unwrap().1,
                    page.to_string()
                );
                let body = body.to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
            // No third page should ever be requested to satisfy two items.
        });
        let base = Url::parse(&format!("http://{address}/api/")).unwrap();
        let items = resolve_at_base_limited(
            &Client::new(),
            &base,
            &Target::Search {
                query: "blue".into(),
            },
            5,
            2,
        )
        .await
        .unwrap();
        assert_eq!(
            items
                .iter()
                .map(|item| item.filename.as_str())
                .collect::<Vec<_>>(),
            ["wallhaven-94x38z.jpg", "wallhaven-ze1p56.png"]
        );
        server.await.unwrap();
    }

    #[test]
    fn never_silently_truncates_valid_api_image_pages() {
        let many: Vec<_> = (0..65)
            .map(|_| {
                serde_json::json!({
                    "id":"94x38z",
                    "path":"https://w.wallhaven.cc/full/94/wallhaven-94x38z.jpg"
                })
            })
            .collect();
        assert_eq!(
            parse_items(&serde_json::json!({"data":many}), false)
                .unwrap()
                .len(),
            65
        );
        let oversized = vec![serde_json::json!({"id":"94x38z"}); MAX_MEDIA + 1];
        assert!(parse_items(&serde_json::json!({"data":oversized}), false)
            .unwrap_err()
            .to_string()
            .contains("media count limit"));
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
            let url = Url::parse(&format!(
                "http://localhost{}",
                line.split_whitespace().nth(1).unwrap()
            ))
            .unwrap();
            let query: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
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

    #[tokio::test]
    async fn public_collection_uses_scoped_api_and_bounded_page_traversal() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for page in 1..=2 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                loop {
                    let mut chunk = [0_u8; 2048];
                    let n = socket.read(&mut chunk).await.unwrap();
                    assert!(n > 0);
                    request.extend_from_slice(&chunk[..n]);
                    assert!(request.len() <= 8192);
                    if request.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                let request = String::from_utf8(request).unwrap();
                let first_line = request.lines().next().unwrap();
                assert!(first_line.starts_with("GET "));
                let target_path = first_line.split_whitespace().nth(1).unwrap();
                let url = Url::parse(&format!("http://localhost{target_path}")).unwrap();
                assert_eq!(url.path(), "/api/v1/collections/example-1/12345");
                let query: std::collections::HashMap<_, _> =
                    url.query_pairs().into_owned().collect();
                assert_eq!(query.len(), 1);
                assert_eq!(
                    query.get("page").map(String::as_str),
                    Some(if page == 1 { "1" } else { "2" })
                );
                assert!(!request.to_ascii_lowercase().contains("x-api-key:"));
                let body = if page == 1 {
                    serde_json::json!({
                        "data":[{"id":"94x38z","path":"https://w.wallhaven.cc/full/94/wallhaven-94x38z.jpg"}],
                        "meta":{"current_page":1,"last_page":2}
                    })
                } else {
                    serde_json::json!({
                        "data":[
                            {"id":"94x38z","path":"https://w.wallhaven.cc/full/94/wallhaven-94x38z.jpg"},
                            {"id":"ze1p56","path":"https://w.wallhaven.cc/full/ze/wallhaven-ze1p56.png"}
                        ],
                        "meta":{"current_page":2,"last_page":2}
                    })
                };
                let body = body.to_string();
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{address}/api/")).unwrap();
        let items = resolve_at_base(
            &Client::new(),
            &base,
            &Target::Collection {
                user: "example-1".into(),
                collection_id: "12345".into(),
            },
            2,
        )
        .await
        .unwrap();
        assert_eq!(
            items
                .iter()
                .map(|x| x.filename.as_str())
                .collect::<Vec<_>>(),
            ["wallhaven-94x38z.jpg", "wallhaven-ze1p56.png"]
        );
        server.await.unwrap();
    }

    #[test]
    fn collection_ids_validate_response_and_ignore_untrusted_identifiers() {
        let response = serde_json::json!({
            "data": [
                {"id":123},
                {"id":"456"},
                {"id":"789/../../evil"},
                {"id":null},
                {"id":-5},
                {"id":1.2}
            ]
        });
        assert_eq!(
            collection_ids(&response).unwrap(),
            vec!["123".to_owned(), "456".to_owned()]
        );
        assert!(collection_ids(&serde_json::json!({"data":{}})).is_err());
        assert!(collection_ids(&serde_json::json!({"error":"denied"})).is_err());
        assert!(collection_ids(&serde_json::json!({
            "data":vec![serde_json::json!({"id":1}); MAX_COLLECTIONS_PER_PAGE + 1]
        }))
        .is_err());
    }

    #[tokio::test]
    async fn public_collection_index_flattens_two_albums_with_global_api_budget() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let urls = [
                "/api/v1/collections/example-1",
                "/api/v1/collections/example-1/12345?page=1",
                "/api/v1/collections/example-1/67890?page=1",
            ];
            let bodies = [
                serde_json::json!({"data":[{"id":12345},{"id":67890},{"id":12345},{"id":"bad/123"}]}),
                serde_json::json!({"data":[
                    {"id":"94x38z","path":"https://w.wallhaven.cc/full/94/wallhaven-94x38z.jpg"},
                    {"id":"ze1p56","path":"https://w.wallhaven.cc/full/ze/wallhaven-ze1p56.png"}
                ],"meta":{"current_page":1,"last_page":1}}),
                serde_json::json!({"data":[
                    {"id":"94x38z","path":"https://w.wallhaven.cc/full/94/wallhaven-94x38z.jpg"},
                    {"id":"a1b2c3","path":"https://w.wallhaven.cc/full/a1/wallhaven-a1b2c3.jpg"},
                    {"id":"unsafe","path":"https://malicious.invalid/bad.jpg"}
                ],"meta":{"current_page":1,"last_page":1}}),
            ];
            for (expected_path, data) in urls.into_iter().zip(bodies) {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut req = Vec::new();
                loop {
                    let mut bytes = [0_u8; 2048];
                    let count = socket.read(&mut bytes).await.unwrap();
                    assert!(count > 0, "request headers missing");
                    req.extend_from_slice(&bytes[..count]);
                    assert!(req.len() < 8192);
                    if req.windows(4).any(|part| part == b"\r\n\r\n") {
                        break;
                    }
                }
                let text = String::from_utf8(req).unwrap();
                assert!(text.starts_with(&format!("GET {expected_path} HTTP/1.1")));
                assert!(!text.to_ascii_lowercase().contains("x-api-key:"));
                let body = data.to_string();
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{address}/api/")).unwrap();
        let items = resolve_at_base(
            &Client::new(),
            &base,
            &Target::Collections {
                user: "example-1".into(),
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

    #[tokio::test]
    async fn collection_index_default_allows_one_media_page_after_discovery() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (path, body) in [
                (
                    "/api/v1/collections/example-1",
                    r#"{"data":[{"id":12345},{"id":67890}]}"#,
                ),
                (
                    "/api/v1/collections/example-1/12345?page=1",
                    r#"{"data":[{"id":"94x38z","path":"https://w.wallhaven.cc/full/94/wallhaven-94x38z.jpg"}],"meta":{"current_page":1,"last_page":2}}"#,
                ),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut req = [0_u8; 2048];
                let n = socket.read(&mut req).await.unwrap();
                assert!(
                    String::from_utf8_lossy(&req[..n]).starts_with(&format!("GET {path} HTTP/1.1"))
                );
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{address}/api/")).unwrap();
        let items = resolve_at_base(
            &Client::new(),
            &base,
            &Target::Collections {
                user: "example-1".into(),
            },
            1,
        )
        .await
        .unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].filename, "wallhaven-94x38z.jpg");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn collection_index_continuation_is_bounded_before_album_expansion() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let pages = [
                (
                    "/api/v1/collections/example-1",
                    serde_json::json!({
                        "data":[{"id":12345}],
                        "meta":{"current_page":1,"last_page":2}
                    }),
                ),
                (
                    "/api/v1/collections/example-1?page=2",
                    serde_json::json!({
                        "data":[{"id":67890},{"id":12345}],
                        "meta":{"current_page":2,"last_page":2}
                    }),
                ),
                (
                    "/api/v1/collections/example-1/12345?page=1",
                    serde_json::json!({
                        "data":[{"id":"94x38z","path":"https://w.wallhaven.cc/full/94/wallhaven-94x38z.jpg"}],
                        "meta":{"current_page":1,"last_page":1}
                    }),
                ),
                (
                    "/api/v1/collections/example-1/67890?page=1",
                    serde_json::json!({
                        "data":[{"id":"a1b2c3","path":"https://w.wallhaven.cc/full/a1/wallhaven-a1b2c3.jpg"}],
                        "meta":{"current_page":1,"last_page":1}
                    }),
                ),
            ];
            for (expected, data) in pages {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                loop {
                    let mut bytes = [0_u8; 2048];
                    let n = socket.read(&mut bytes).await.unwrap();
                    assert!(n > 0);
                    request.extend_from_slice(&bytes[..n]);
                    assert!(request.len() <= 8192);
                    if request.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                let header = String::from_utf8(request).unwrap();
                let first = header.lines().next().unwrap();
                assert!(
                    first.starts_with(&format!("GET {expected} HTTP/1.1")),
                    "{first}"
                );
                let body = data.to_string();
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
            &Target::Collections {
                user: "example-1".into(),
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
            ["wallhaven-94x38z.jpg", "wallhaven-a1b2c3.jpg"]
        );
        tokio::time::timeout(Duration::from_secs(15), server)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn collection_index_emits_deferred_queues_without_downloading() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (expected, body) in [
                (
                    "/api/v1/collections/example-1",
                    r#"{"data":[{"id":12345},{"id":"67890"},{"id":12345}],"meta":{"current_page":1,"last_page":2}}"#,
                ),
                (
                    "/api/v1/collections/example-1?page=2",
                    r#"{"data":[{"id":"45678"},{"id":"../../evil"}],"meta":{"current_page":2,"last_page":2}}"#,
                ),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                loop {
                    let mut chunk = [0_u8; 2048];
                    let n = socket.read(&mut chunk).await.unwrap();
                    assert!(n > 0, "request headers missing");
                    bytes.extend_from_slice(&chunk[..n]);
                    assert!(bytes.len() <= 8192);
                    if bytes.windows(4).any(|part| part == b"\r\n\r\n") {
                        break;
                    }
                }
                let request = String::from_utf8(bytes).unwrap();
                assert!(
                    request.starts_with(&format!("GET {expected} HTTP/1.1")),
                    "{request}"
                );
                assert!(!request.to_ascii_lowercase().contains("x-api-key:"));
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{address}/api/")).unwrap();
        let events = queue_collections_at_base(&Client::new(), &base, "example-1", 2)
            .await
            .unwrap();
        assert_eq!(events.len(), 3);
        for (index, id) in ["12345", "67890", "45678"].into_iter().enumerate() {
            let value = serde_json::to_value(&events[index]).unwrap();
            assert_eq!(value["type"], "queue");
            assert_eq!(
                value["url"],
                format!("https://wallhaven.cc/user/example-1/favorites/{id}")
            );
            assert_eq!(value["metadata"]["category"], "wallhaven");
            assert_eq!(value["metadata"]["subcategory"], "collection");
            assert_eq!(value["metadata"]["username"], "example-1");
            assert_eq!(value["metadata"]["collection_id"], id);
        }
        server.await.unwrap();
    }

    #[tokio::test]
    async fn collection_queue_budget_does_not_request_children() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut req = [0_u8; 2048];
            let n = socket.read(&mut req).await.unwrap();
            assert!(String::from_utf8_lossy(&req[..n])
                .starts_with("GET /api/v1/collections/example-1 HTTP/1.1"));
            let body =
                r#"{"data":[{"id":12345},{"id":67890}],"meta":{"current_page":1,"last_page":3}}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        let base = Url::parse(&format!("http://{address}/api/")).unwrap();
        let events = queue_collections_at_base(&Client::new(), &base, "example-1", 1)
            .await
            .unwrap();
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], ExtractorMessage::Queue { .. }));
        server.await.unwrap();
        assert!(
            queue_collections_at_base(&Client::new(), &base, "bad/name", 1)
                .await
                .is_err()
        );
    }
}
