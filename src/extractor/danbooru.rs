//! Preliminary Danbooru read-only post and tag query support.
//! API reference: https://safebooru.donmai.us/wiki_pages/api%3Aposts
//! Module status is PARTIAL until gallery-dl parity is verified.
use crate::{safe_filename, MediaItem};
use reqwest::Client;
use serde_json::Value;
use std::error::Error;
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
        ["posts"] | ["posts.json"] => {
            let tags = url.query_pairs()
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
        let media_url = Url::parse(raw_url).or_else(|_| base.join(raw_url))?;
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
        Target::Post { host, .. } | Target::Search { host, .. } => host,
    };
    let base = Url::parse(&format!("https://{host}/"))?;
    match target {
        Target::Post { id, .. } => {
            let endpoint = base.join(&format!("posts/{id}.json"))?;
            let response = client.get(endpoint).send().await?.error_for_status()?;
            let body: Value = response.json().await?;
            post_items(&body, &base)
        }
        Target::Search { tags, .. } => {
            let mut items = Vec::new();
            for page in 1..=max_pages.max(1) {
                let mut endpoint = base.join("posts.json")?;
                endpoint.query_pairs_mut()
                    .append_pair("tags", &tags)
                    .append_pair("limit", "50")
                    .append_pair("page", &page.to_string());
                let response = client.get(endpoint).send().await?.error_for_status()?;
                let body: Value = response.json().await?;
                let page_is_empty = body.as_array().ok_or("unexpected Danbooru posts response")?.is_empty();
                if page_is_empty {
                    break;
                }
                items.extend(post_items(&body, &base)?);
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
            Some(Target::Post { host: "danbooru.donmai.us".into(), id: 42 })
        );
        assert_eq!(
            match_url("https://safebooru.donmai.us/posts?tags=blue+sky"),
            Some(Target::Search { host: "safebooru.donmai.us".into(), tags: "blue sky".into() })
        );
        assert_eq!(match_url("https://evil.example/posts/42"), None);
        assert_eq!(match_url("http://danbooru.donmai.us/posts/42"), None);
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
}
