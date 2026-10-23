//! Independent e621-style JSON extractor (initial post and tag-search subset).
//! E621's upstream extractor includes pools, artists, favorites, metadata
//! and auth variants that are NOT supported by this preview.
use crate::{safe_filename, MediaItem};
use reqwest::Client;
use serde_json::Value;
use std::{error::Error, time::Duration};
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Post { host: String, id: u64 },
    Search { host: String, tags: String },
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
        ["posts"] | ["posts.json"] => {
            let tags = url.query_pairs()
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
    let raw = file.get("url")?.as_str()?;
    let ext = file.get("ext")?.as_str()?;
    if ext.is_empty() || ext.len() > 16 || !ext.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    let url = Url::parse(raw).or_else(|_| base.join(raw)).ok()?;
    if !matches!(url.scheme(), "https" | "http") {
        return None;
    }
    let extractor = if base.host_str() == Some("e6ai.net") { "e6ai" } else { "e621" };
    Some(MediaItem {
        url: url.into(),
        filename: safe_filename(&format!("{id}.{ext}")),
        extractor,
        post_id: Some(id),
    })
}

fn parse_posts(data: &Value, base: &Url) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let posts = data.get("posts")
        .and_then(Value::as_array)
        .ok_or("unexpected e621 posts list response")?;
    Ok(posts.iter().filter_map(|post| parse_post(post, base)).collect())
}

pub async fn resolve(client: &Client, target: Target, max_pages: u16)
    -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>>
{
    let host = match &target {
        Target::Post { host, .. } | Target::Search { host, .. } => host,
    };
    let base = Url::parse(&format!("https://{host}/"))?;
    resolve_at_base(client, target, max_pages, &base).await
}

/// Testing seam: the public router accepts only known upstream hostnames.
async fn resolve_at_base(client: &Client, target: Target, max_pages: u16, base: &Url)
    -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>>
{
    match target {
        Target::Post { id, .. } => {
            let endpoint = base.join(&format!("posts/{id}.json"))?;
            let data: Value = client.get(endpoint).send().await?.error_for_status()?.json().await?;
            let post = data.get("post").ok_or("missing e621 post object")?;
            Ok(parse_post(post, base).into_iter().collect())
        }
        Target::Search { tags, .. } => {
            let mut media = Vec::new();
            for page in 1..=max_pages.max(1) {
                let mut endpoint = base.join("posts.json")?;
                endpoint.query_pairs_mut()
                    .append_pair("tags", &tags)
                    .append_pair("limit", "75")
                    .append_pair("page", &page.to_string());
                let data: Value = client.get(endpoint).send().await?.error_for_status()?.json().await?;
                let empty = data.get("posts")
                    .and_then(Value::as_array)
                    .ok_or("unexpected e621 posts list response")?
                    .is_empty();
                if empty {
                    break;
                }
                media.extend(parse_posts(&data, base)?);
                if page < max_pages {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
            Ok(media)
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
            Some(Target::Post { host: "e621.net".into(), id: 90 }),
        );
        assert_eq!(
            match_url("https://e926.net/posts?tags=blue+sky"),
            Some(Target::Search { host: "e926.net".into(), tags: "blue sky".into() }),
        );
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
        assert!(parse_posts(&json!({"posts":{}}), &base).is_err());
    }

    #[tokio::test]
    async fn resolves_single_post_using_local_fixture() {
        use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::TcpListener};
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut connection, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 2048];
            let count = connection.read(&mut buf).await.unwrap();
            assert!(String::from_utf8_lossy(&buf[..count]).starts_with("GET /posts/77.json HTTP/1.1"));
            let body = r#"{"post":{"id":77,"file":{"url":"https://cdn.example.org/77.jpg","ext":"jpg"}}}"#;
            let reply = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}", body.len());
            connection.write_all(reply.as_bytes()).await.unwrap();
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let target = Target::Post { host: "e621.net".into(), id: 77 };
        let items = resolve_at_base(&Client::new(), target, 1, &base).await.unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].filename, "77.jpg");
        server.await.unwrap();
    }
}
