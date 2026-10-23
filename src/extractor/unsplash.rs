//! Independent public Unsplash photo and collection API extractor preview.
//! The pinned upstream defines single photos, user photos/likes, collections,
//! and search; live API changes, full metadata and authentication are unverified.
use crate::MediaItem;
use reqwest::Client;
use serde_json::Value;
use std::{collections::HashSet, error::Error};
use url::Url;

const ROOT: &str = "https://unsplash.com/";
const MAX_JSON_BYTES: usize = 2 * 1024 * 1024;
const PAGE_SIZE: usize = 20;
const MAX_PHOTOS: usize = 2000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Image {
        id: String,
    },
    User {
        username: String,
    },
    Likes {
        username: String,
    },
    Collection {
        id: String,
    },
    Search {
        query: String,
        orientation: Option<String>,
    },
}

fn valid_segment(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

pub fn match_url(input: &str) -> Option<Target> {
    if input.to_ascii_lowercase().contains("%2e") {
        return None;
    }
    let url = Url::parse(input).ok()?;
    if url.scheme() != "https" || url.host_str() != Some("unsplash.com") {
        return None;
    }
    let segments: Vec<_> = url.path_segments()?.collect();
    match segments.as_slice() {
        ["photos", id] if valid_segment(id) => Some(Target::Image {
            id: (*id).to_owned(),
        }),
        [user] if user.starts_with('@') && valid_segment(&user[1..]) => Some(Target::User {
            username: user[1..].to_owned(),
        }),
        [user, "likes"] if user.starts_with('@') && valid_segment(&user[1..]) => {
            Some(Target::Likes {
                username: user[1..].to_owned(),
            })
        }
        ["collections", id] | ["collections", id, _] if valid_segment(id) => {
            Some(Target::Collection {
                id: (*id).to_owned(),
            })
        }
        ["s", "photos", slug] if valid_segment(slug) => {
            let orientation = url
                .query_pairs()
                .find(|(key, _)| key == "orientation")
                .and_then(|(_, val)| {
                    matches!(val.as_ref(), "landscape" | "portrait" | "squarish")
                        .then(|| val.into_owned())
                });
            Some(Target::Search {
                query: slug.replace('-', " "),
                orientation,
            })
        }
        _ => None,
    }
}

fn photo_item(photo: &Value) -> Option<(String, MediaItem)> {
    let id = photo.get("id")?.as_str()?;
    if !valid_segment(id) {
        return None;
    }
    let raw = photo.pointer("/urls/raw")?.as_str()?;
    let url = Url::parse(raw).ok()?;
    if url.scheme() != "https"
        || !matches!(
            url.host_str(),
            Some("images.unsplash.com" | "plus.unsplash.com")
        )
    {
        return None;
    }
    Some((
        id.to_owned(),
        MediaItem {
            url: url.to_string(),
            filename: format!("{id}.jpg"),
            extractor: "unsplash",
            post_id: id.parse::<u64>().ok(),
        },
    ))
}

async fn read_json(client: &Client, url: Url) -> Result<Value, Box<dyn Error + Send + Sync>> {
    let mut response = client.get(url).send().await?.error_for_status()?;
    if response
        .content_length()
        .is_some_and(|n| n > MAX_JSON_BYTES as u64)
    {
        return Err("Unsplash response exceeds the 2 MiB JSON limit".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len().saturating_add(chunk.len()) > MAX_JSON_BYTES {
            return Err("Unsplash response exceeds the 2 MiB JSON limit".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(serde_json::from_slice(&bytes)?)
}

async fn resolve_at_base(
    client: &Client,
    base: &Url,
    target: Target,
    max_pages: u16,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    if let Target::Image { id } = target {
        let photo = read_json(client, base.join(&format!("napi/photos/{id}"))?).await?;
        return Ok(photo_item(&photo)
            .into_iter()
            .map(|(_, item)| item)
            .collect());
    }
    let (route, params, search) = match target {
        Target::User { username } => (
            format!("napi/users/{username}/photos"),
            vec![("order_by", "latest".to_owned())],
            false,
        ),
        Target::Likes { username } => (
            format!("napi/users/{username}/likes"),
            vec![("order_by", "latest".to_owned())],
            false,
        ),
        Target::Collection { id } => (
            format!("napi/collections/{id}/photos"),
            vec![("order_by", "latest".to_owned())],
            false,
        ),
        Target::Search { query, orientation } => {
            let mut params = vec![("query", query)];
            if let Some(orientation) = orientation {
                params.push(("orientation", orientation));
            }
            ("napi/search/photos".to_owned(), params, true)
        }
        Target::Image { .. } => unreachable!(),
    };
    let mut media = Vec::new();
    let mut seen = HashSet::new();
    for page in 1..=max_pages.clamp(1, 100) {
        if page > 1 {
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
        let mut url = base.join(&route)?;
        {
            let mut query = url.query_pairs_mut();
            for (key, value) in &params {
                query.append_pair(key, value);
            }
            query.append_pair("per_page", "20");
            query.append_pair("page", &page.to_string());
        }
        let response = read_json(client, url).await?;
        let photos = if search {
            response.get("results").and_then(Value::as_array)
        } else {
            response.as_array()
        }
        .ok_or("Unsplash API response does not contain expected image array")?;
        if photos.len() > PAGE_SIZE {
            return Err("Unsplash API returned more than 20 images in one page".into());
        }
        for photo in photos {
            if let Some((id, item)) = photo_item(photo) {
                if seen.insert(id) {
                    media.push(item);
                }
            }
        }
        if media.len() >= MAX_PHOTOS || photos.len() < PAGE_SIZE {
            break;
        }
    }
    media.truncate(MAX_PHOTOS);
    Ok(media)
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
    use serde_json::json;

    #[test]
    fn matches_five_pinned_public_unsplash_url_families() {
        assert_eq!(
            match_url("https://unsplash.com/photos/abc-123"),
            Some(Target::Image {
                id: "abc-123".into()
            })
        );
        assert_eq!(
            match_url("https://unsplash.com/@photographer"),
            Some(Target::User {
                username: "photographer".into()
            })
        );
        assert_eq!(
            match_url("https://unsplash.com/@photographer/likes"),
            Some(Target::Likes {
                username: "photographer".into()
            })
        );
        assert_eq!(
            match_url("https://unsplash.com/collections/12345/nature"),
            Some(Target::Collection { id: "12345".into() })
        );
        assert_eq!(
            match_url("https://unsplash.com/s/photos/night-sky?orientation=landscape"),
            Some(Target::Search {
                query: "night sky".into(),
                orientation: Some("landscape".into()),
            })
        );
        for bad in [
            "https://evil.unsplash.com/@photographer",
            "http://unsplash.com/photos/abc",
            "https://unsplash.com/@photographer/collections",
            "https://unsplash.com/photos/bad%2Fid",
            "https://unsplash.com/s/photos/nature/extra",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn media_require_safe_unsplash_cdn_and_valid_id() {
        let photo = json!({
            "id":"photo_abc",
            "urls":{"raw":"https://images.unsplash.com/photo-123?ixid=abc"}
        });
        let (_, item) = photo_item(&photo).unwrap();
        assert_eq!(item.filename, "photo_abc.jpg");
        assert_eq!(item.url, "https://images.unsplash.com/photo-123?ixid=abc");
        assert!(photo_item(&json!({
            "id":"photo_abc",
            "urls":{"raw":"https://images.unsplash.com.evil.test/file"}
        }))
        .is_none());
        assert!(photo_item(&json!({
            "id":"../unsafe",
            "urls":{"raw":"https://images.unsplash.com/file"}
        }))
        .is_none());
    }

    #[tokio::test]
    async fn public_single_photo_and_search_pages_resolve_via_local_http() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (route, body) in [
                (
                    "GET /napi/photos/abc123 HTTP/1.1",
                    r#"{"id":"abc123","urls":{"raw":"https://images.unsplash.com/photo-first"}}"#,
                ),
                (
                    "GET /napi/search/photos?query=blue+sky&orientation=landscape&per_page=20&page=1 HTTP/1.1",
                    r#"{"results":[{"id":"xyz789","urls":{"raw":"https://images.unsplash.com/photo-second"}}]}"#,
                ),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buf = [0_u8; 4096];
                let n = socket.read(&mut buf).await.unwrap();
                let request = String::from_utf8_lossy(&buf[..n]);
                assert!(request.starts_with(route), "{request}");
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{address}/")).unwrap();
        let single = resolve_at_base(
            &Client::new(),
            &base,
            Target::Image {
                id: "abc123".into(),
            },
            1,
        )
        .await
        .unwrap();
        assert_eq!(single.len(), 1);
        assert_eq!(single[0].filename, "abc123.jpg");
        let search = resolve_at_base(
            &Client::new(),
            &base,
            Target::Search {
                query: "blue sky".into(),
                orientation: Some("landscape".into()),
            },
            1,
        )
        .await
        .unwrap();
        assert_eq!(search.len(), 1);
        assert_eq!(search[0].filename, "xyz789.jpg");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn public_likes_and_collections_use_their_separate_endpoints() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (route, body) in [
                (
                    "GET /napi/users/alice/likes?order_by=latest&per_page=20&page=1 HTTP/1.1",
                    r#"[{"id":"liked01","urls":{"raw":"https://images.unsplash.com/liked"}}]"#,
                ),
                (
                    "GET /napi/collections/123/photos?order_by=latest&per_page=20&page=1 HTTP/1.1",
                    r#"[{"id":"collect01","urls":{"raw":"https://images.unsplash.com/collected"}}]"#,
                ),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buf = [0_u8; 4096];
                let n = socket.read(&mut buf).await.unwrap();
                let request = String::from_utf8_lossy(&buf[..n]);
                assert!(request.starts_with(route), "{request}");
                let answer = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(answer.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{address}/")).unwrap();
        let likes = resolve_at_base(
            &Client::new(),
            &base,
            Target::Likes {
                username: "alice".into(),
            },
            1,
        )
        .await
        .unwrap();
        assert_eq!(likes.len(), 1);
        assert_eq!(likes[0].filename, "liked01.jpg");
        let collection = resolve_at_base(
            &Client::new(),
            &base,
            Target::Collection { id: "123".into() },
            1,
        )
        .await
        .unwrap();
        assert_eq!(collection.len(), 1);
        assert_eq!(collection[0].filename, "collect01.jpg");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn user_photo_feed_paginates_full_page_and_deduplicates() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let first: Vec<_> = (0..20)
                .map(|i| {
                    json!({
                        "id":format!("picture{i}"),
                        "urls":{"raw":format!("https://images.unsplash.com/photo-{i}")}
                    })
                })
                .collect();
            let first = serde_json::to_string(&first).unwrap();
            let second = serde_json::to_string(&vec![
                json!({"id":"picture0","urls":{"raw":"https://images.unsplash.com/duplicate"}}),
                json!({"id":"lastone","urls":{"raw":"https://images.unsplash.com/last"}}),
            ])
            .unwrap();
            for (route, body) in [
                (
                    "GET /napi/users/alice/photos?order_by=latest&per_page=20&page=1 HTTP/1.1",
                    first,
                ),
                (
                    "GET /napi/users/alice/photos?order_by=latest&per_page=20&page=2 HTTP/1.1",
                    second,
                ),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buf = [0_u8; 4096];
                let n = socket.read(&mut buf).await.unwrap();
                let request = String::from_utf8_lossy(&buf[..n]);
                assert!(request.starts_with(route), "{request}");
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{address}/")).unwrap();
        let result = resolve_at_base(
            &Client::new(),
            &base,
            Target::User {
                username: "alice".into(),
            },
            2,
        )
        .await
        .unwrap();
        assert_eq!(result.len(), 21);
        assert_eq!(result[0].filename, "picture0.jpg");
        assert_eq!(result[20].filename, "lastone.jpg");
        server.await.unwrap();
    }
}
