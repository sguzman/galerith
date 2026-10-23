//! Independent e621-style JSON extractor (initial post and tag-search subset).
//! E621's upstream extractor includes pools, artists, metadata and auth
//! variants that are NOT supported by this preview.
use crate::{safe_filename, MediaItem};
use reqwest::Client;
use serde_json::Value;
use std::{error::Error, time::Duration};
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
    Popular {
        host: String,
        parameters: Vec<(String, String)>,
    },
    Favorites {
        host: String,
        parameters: Vec<(String, String)>,
    },
}

pub fn match_url(input: &str) -> Option<Target> {
    let url = Url::parse(input).ok()?;
    if url.scheme() != "https" {
        return None;
    }
    let host = url.host_str()?;
    if !matches!(host, "e621.net" | "e926.net" | "e6ai.net") {
        return None;
    }
    let host = host.to_owned();
    let parts: Vec<_> = url.path_segments()?.collect();
    match parts.as_slice() {
        ["posts", id] => {
            let id = id.strip_suffix(".json").unwrap_or(id).parse::<u64>().ok()?;
            (id > 0).then_some(Target::Post { host, id })
        }
        ["explore", "posts", "popular"] | ["favorites"] => {
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
                        || (key == "page"
                            && value.parse::<u16>().ok().filter(|page| *page > 0).is_none())
                })
            {
                return None;
            }
            if parts.as_slice() == ["favorites"] {
                Some(Target::Favorites { host, parameters })
            } else {
                Some(Target::Popular { host, parameters })
            }
        }
        ["posts"] | ["posts.json"] => {
            let tags = url
                .query_pairs()
                .find(|(key, _)| key == "tags")
                .map(|(_, value)| value.into_owned())
                .unwrap_or_default();
            Some(Target::Search { host, tags })
        }
        _ => None,
    }
}

fn parse_post(post: &Value, base: &Url) -> Option<MediaItem> {
    let id = post.get("id")?.as_u64()?;
    let file = post.get("file")?;
    let ext = file.get("ext")?.as_str()?;
    if ext.is_empty() || ext.len() > 16 || !ext.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    let md5 = file
        .get("md5")
        .and_then(Value::as_str)
        .filter(|value| value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit()));
    let media_url = match file
        .get("url")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        Some(raw) => Url::parse(raw).or_else(|_| base.join(raw)).ok()?,
        None => {
            let md5 = md5?;
            let host = base.host_str()?;
            if !matches!(host, "e621.net" | "e926.net" | "e6ai.net") {
                return None;
            }
            let path = format!(
                "https://static1.{host}/data/{}/{}/{}.{}",
                &md5[..2],
                &md5[2..4],
                md5,
                ext
            );
            Url::parse(&path).ok()?
        }
    };
    if !matches!(media_url.scheme(), "https" | "http") {
        return None;
    }
    let extractor = if base.host_str() == Some("e6ai.net") {
        "e6ai"
    } else {
        "e621"
    };
    let stem = md5.map(str::to_owned).unwrap_or_else(|| id.to_string());
    Some(MediaItem {
        url: media_url.into(),
        filename: safe_filename(&format!("{stem}.{ext}")),
        extractor,
        post_id: Some(id),
    })
}

fn parse_posts(data: &Value, base: &Url) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let posts = data
        .get("posts")
        .and_then(Value::as_array)
        .ok_or("unexpected e621 posts list response")?;
    Ok(posts
        .iter()
        .filter_map(|post| parse_post(post, base))
        .collect())
}

pub async fn resolve(
    client: &Client,
    target: Target,
    max_pages: u16,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let host = match &target {
        Target::Post { host, .. }
        | Target::Search { host, .. }
        | Target::Popular { host, .. }
        | Target::Favorites { host, .. } => host,
    };
    let base = Url::parse(&format!("https://{host}/"))?;
    resolve_at_base(client, target, max_pages, &base).await
}

async fn paginate_posts(
    client: &Client,
    base: &Url,
    api_path: &str,
    parameters: &[(String, String)],
    max_pages: u16,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let start = parameters
        .iter()
        .find(|(key, _)| key == "page")
        .map(|(_, value)| value.parse::<u16>())
        .transpose()?
        .unwrap_or(1);
    let mut media = Vec::new();
    for offset in 0..max_pages.max(1) {
        let Some(page) = start.checked_add(offset) else {
            break;
        };
        let mut endpoint = base.join(api_path)?;
        {
            let mut query = endpoint.query_pairs_mut();
            for (key, value) in parameters {
                if key != "page" && key != "limit" {
                    query.append_pair(key, value);
                }
            }
            query
                .append_pair("limit", "75")
                .append_pair("page", &page.to_string());
        }
        let data: Value = client
            .get(endpoint)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let empty = data
            .get("posts")
            .and_then(Value::as_array)
            .ok_or("unexpected e621 posts list response")?
            .is_empty();
        if empty {
            break;
        }
        media.extend(parse_posts(&data, base)?);
        if offset + 1 < max_pages {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
    Ok(media)
}

/// Testing seam: the public router accepts only known upstream hostnames.
async fn resolve_at_base(
    client: &Client,
    target: Target,
    max_pages: u16,
    base: &Url,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    match target {
        Target::Post { id, .. } => {
            let endpoint = base.join(&format!("posts/{id}.json"))?;
            let data: Value = client
                .get(endpoint)
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            let post = data.get("post").ok_or("missing e621 post object")?;
            Ok(parse_post(post, base).into_iter().collect())
        }
        Target::Search { tags, .. } => {
            paginate_posts(
                client,
                base,
                "posts.json",
                &[("tags".into(), tags)],
                max_pages,
            )
            .await
        }
        Target::Popular { parameters, .. } => {
            paginate_posts(client, base, "popular.json", &parameters, max_pages).await
        }
        Target::Favorites { parameters, .. } => {
            paginate_posts(client, base, "favorites.json", &parameters, max_pages).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn matches_known_host_and_explicit_supported_paths() {
        assert_eq!(
            match_url("https://e621.net/posts/90"),
            Some(Target::Post {
                host: "e621.net".into(),
                id: 90
            }),
        );
        assert_eq!(
            match_url("https://e926.net/posts?tags=blue+sky"),
            Some(Target::Search {
                host: "e926.net".into(),
                tags: "blue sky".into()
            }),
        );
        assert_eq!(
            match_url("https://e621.net/explore/posts/popular?scale=week"),
            Some(Target::Popular {
                host: "e621.net".into(),
                parameters: vec![("scale".into(), "week".into())]
            })
        );
        assert_eq!(
            match_url("https://e926.net/favorites?user_id=42&page=3"),
            Some(Target::Favorites {
                host: "e926.net".into(),
                parameters: vec![("user_id".into(), "42".into()), ("page".into(), "3".into())]
            })
        );
        assert_eq!(match_url("https://e621.net/favorites?page=0"), None);
        assert_eq!(match_url("https://e621.net/pools/123"), None);
        assert_eq!(match_url("https://not-e621.net/posts/90"), None);
        assert_eq!(match_url("http://e621.net/posts/90"), None);
    }

    #[test]
    fn parses_media_and_skips_unavailable_files() {
        let base = Url::parse("https://e621.net/").unwrap();
        let input = json!({
            "posts": [
                {"id": 1, "file": {"url": "https://static1.e621.net/1.png", "ext": "png"}},
                {"id": 2, "file": {"url": null, "ext": "jpg"}},
                {"id": 3, "file": {"url": "javascript:bad()", "ext": "gif"}},
                {"id": 4, "file": {"url": "/data/4.webp", "ext": "webp"}}
            ]
        });
        let items = parse_posts(&input, &base).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].filename, "1.png");
        assert_eq!(items[1].url, "https://e621.net/data/4.webp");
        assert_eq!(items[1].post_id, Some(4));
        let fallback = json!({
            "posts": [{"id": 19, "file": {
                "url": null,
                "md5": "0123456789abcdef0123456789abcdef",
                "ext": "jpg"
            }}]
        });
        let recovered = parse_posts(&fallback, &base).unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(
            recovered[0].url,
            "https://static1.e621.net/data/01/23/0123456789abcdef0123456789abcdef.jpg"
        );
        assert_eq!(
            recovered[0].filename,
            "0123456789abcdef0123456789abcdef.jpg"
        );
        assert!(parse_posts(&json!({"posts":{}}), &base).is_err());
    }

    #[tokio::test]
    async fn resolves_single_post_using_local_fixture() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut connection, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 2048];
            let count = connection.read(&mut buf).await.unwrap();
            assert!(
                String::from_utf8_lossy(&buf[..count]).starts_with("GET /posts/77.json HTTP/1.1")
            );
            let body =
                r#"{"post":{"id":77,"file":{"url":"https://cdn.example.org/77.jpg","ext":"jpg"}}}"#;
            let reply = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}", body.len());
            connection.write_all(reply.as_bytes()).await.unwrap();
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let target = Target::Post {
            host: "e621.net".into(),
            id: 77,
        };
        let items = resolve_at_base(&Client::new(), target, 1, &base)
            .await
            .unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].filename, "77.jpg");
        server.await.unwrap();
    }
    #[tokio::test]
    async fn local_tag_search_fetches_two_pages_and_stops_at_empty_page() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for page in 1..=2 {
                let (mut connection, _) = listener.accept().await.unwrap();
                let mut buf = [0u8; 4096];
                let count = connection.read(&mut buf).await.unwrap();
                let request = String::from_utf8_lossy(&buf[..count]);
                assert!(request.starts_with("GET /posts.json?"), "{request}");
                assert!(request.contains("tags=blue+sky"), "{request}");
                assert!(request.contains(&format!("page={page}")), "{request}");
                let body = if page == 1 {
                    r#"{"posts":[{"id":5,"file":{"url":"/media/5.png","ext":"png"}},{"id":6,"file":{"url":null,"ext":"jpg"}}]}"#
                } else {
                    r#"{"posts":[]}"#
                };
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()
                );
                connection.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let target = Target::Search {
            host: "e621.net".into(),
            tags: "blue sky".into(),
        };
        let items = resolve_at_base(&Client::new(), target, 5, &base)
            .await
            .unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].filename, "5.png");
        assert_eq!(items[0].post_id, Some(5));
        server.await.unwrap();
    }
    #[tokio::test]
    async fn popular_and_favorites_use_documented_json_endpoints() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (route, parameter, id) in [
                ("/popular.json?", "scale=week", 91),
                ("/favorites.json?", "user_id=42", 92),
            ] {
                let (mut connection, _) = listener.accept().await.unwrap();
                let mut buf = [0_u8; 4096];
                let n = connection.read(&mut buf).await.unwrap();
                let request = String::from_utf8_lossy(&buf[..n]);
                assert!(request.starts_with(&format!("GET {route}")), "{request}");
                assert!(request.contains(parameter), "{request}");
                assert!(request.contains("page=2"), "{request}");
                let body = format!(
                    r#"{{"posts":[{{"id":{id},"file":{{"url":"https://cdn.example.org/{id}.jpg","ext":"jpg"}}}}]}}"#
                );
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                connection.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let api = Url::parse(&format!("http://{addr}/")).unwrap();
        let popular = Target::Popular {
            host: "e621.net".into(),
            parameters: vec![("scale".into(), "week".into()), ("page".into(), "2".into())],
        };
        let favorites = Target::Favorites {
            host: "e621.net".into(),
            parameters: vec![("user_id".into(), "42".into()), ("page".into(), "2".into())],
        };
        let a = resolve_at_base(&Client::new(), popular, 1, &api)
            .await
            .unwrap();
        let b = resolve_at_base(&Client::new(), favorites, 1, &api)
            .await
            .unwrap();
        assert_eq!(a[0].filename, "91.jpg");
        assert_eq!(b[0].filename, "92.jpg");
        server.await.unwrap();
    }
}
