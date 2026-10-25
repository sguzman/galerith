//! Bounded public Audiochan single-audio originals and user queues.
//! Pinned upstream: gallery_dl/extractor/audiochan.py. No segmented streaming yet.
use crate::{
    message::{media_metadata, queued_url, ExtractorMessage, Metadata},
    safe_filename, MediaItem,
};
use reqwest::Client;
use serde_json::Value;
use std::{collections::HashSet, error::Error, time::Duration};
use url::Url;

const MAX_JSON: usize = 2 * 1024 * 1024;
const MAX_PAGE: usize = 12;
const MAX_PAGES: u16 = 4;
const MAX_CHILDREN: usize = 48;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Audio(String),
    User(String),
}

fn valid_slug(s: &str) -> bool {
    (1..=96).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

pub fn match_url(raw: &str) -> Option<Target> {
    let lower = raw.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "/../", "/./"]
        .iter()
        .any(|needle| lower.contains(needle))
    {
        return None;
    }
    let url = Url::parse(raw).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !matches!(url.host_str(), Some("audiochan.com" | "www.audiochan.com"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let parts: Vec<_> = url.path_segments()?.collect();
    match parts.as_slice() {
        ["a", slug] | ["a", slug, ""] if valid_slug(slug) => {
            Some(Target::Audio((*slug).to_owned()))
        }
        ["u", slug] | ["u", slug, ""] if valid_slug(slug) => Some(Target::User((*slug).to_owned())),
        _ => None,
    }
}

fn audio_url(raw: &str) -> Option<(Url, String)> {
    let url = Url::parse(raw).ok()?;
    let host = url.host_str()?;
    if url.scheme() != "https"
        || !(host == "audiochan.com" || host.ends_with(".audiochan.com"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path().contains('%')
    {
        return None;
    }
    let basename = url.path_segments()?.next_back()?;
    let ext = basename.rsplit_once('.')?.1.to_ascii_lowercase();
    if !matches!(
        ext.as_str(),
        "mp3" | "ogg" | "opus" | "wav" | "flac" | "m4a" | "webm" | "aac"
    ) {
        return None;
    }
    let filename = safe_filename(basename);
    (!filename.is_empty() && !matches!(filename.as_str(), "." | "..")).then_some((url, filename))
}

fn flatten_description(root: &Value, depth: usize, texts: &mut Vec<String>) {
    if depth > 12 || texts.len() >= 64 {
        return;
    }
    if let Some(s) = root.get("text").and_then(Value::as_str) {
        if s.len() <= 2048 && !s.chars().any(char::is_control) {
            texts.push(s.to_owned());
        }
    } else if let Some(children) = root.get("content").and_then(Value::as_array) {
        for child in children.iter().take(64) {
            flatten_description(child, depth + 1, texts);
        }
    }
}

fn media_record(
    post: &Value,
    expected_slug: &str,
) -> Result<(MediaItem, Metadata), Box<dyn Error + Send + Sync>> {
    if post.get("slug").and_then(Value::as_str) != Some(expected_slug) {
        return Err("Audiochan public work slug differs from requested source".into());
    }
    let audio = post
        .get("audioFile")
        .ok_or("Audiochan post lacks an audioFile")?;
    let original = audio
        .get("url")
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
        .ok_or("Audiochan media requires unsupported segmented stream fallback")?;
    let (url, basename) = audio_url(original)
        .ok_or("Audiochan direct audio original is not on a trusted HTTPS host")?;
    let name = audio
        .get("filename")
        .and_then(Value::as_str)
        .filter(|s| {
            (1..=220).contains(&s.len())
                && !s.contains('/')
                && !s.contains('\\')
                && !s.contains("..")
                && !s.chars().any(char::is_control)
        })
        .unwrap_or(&basename);
    let ext = name
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .ok_or("Audiochan filename missing a source extension")?;
    if !matches!(
        ext.as_str(),
        "mp3" | "ogg" | "opus" | "wav" | "flac" | "m4a" | "webm" | "aac"
    ) {
        return Err("Audiochan source filename has unsupported audio extension".into());
    }
    let title = post
        .get("title")
        .and_then(Value::as_str)
        .filter(|s| s.len() <= 200 && !s.chars().any(char::is_control))
        .unwrap_or(expected_slug);
    let filename = safe_filename(&format!("{title} ({expected_slug}).{ext}"));
    let post_id = audio
        .get("id")
        .and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse::<u64>().ok()));
    let item = MediaItem {
        url: url.to_string(),
        filename,
        extractor: "audiochan",
        post_id,
    };
    let mut metadata = media_metadata(&item);
    metadata.insert("subcategory".into(), "audio".into());
    metadata.insert("slug".into(), expected_slug.to_owned().into());
    metadata.insert("title".into(), title.into());
    metadata.insert("extension".into(), ext.into());
    metadata.insert("source_filename".into(), name.into());
    metadata.insert(
        "post_url".into(),
        format!("https://audiochan.com/a/{expected_slug}").into(),
    );
    for (src, dest) in [
        ("created_at", "date_source"),
        ("updated_at", "date_updated_source"),
    ] {
        if let Some(date) = audio
            .get(src)
            .and_then(Value::as_str)
            .filter(|s| s.len() <= 60 && !s.chars().any(char::is_control))
        {
            metadata.insert(dest.into(), date.into());
        }
    }
    if let Some(tags) = post.get("tags").and_then(Value::as_array) {
        let labels = tags
            .iter()
            .take(80)
            .filter_map(|wrapper| {
                let tag = wrapper.get("tag").unwrap_or(wrapper);
                let category = tag.get("category")?.as_str()?;
                let name = tag.get("name")?.as_str()?;
                ((1..=64).contains(&category.len())
                    && (1..=120).contains(&name.len())
                    && !category.chars().any(char::is_control)
                    && !name.chars().any(char::is_control))
                .then(|| Value::from(format!("{category}:{name}")))
            })
            .collect::<Vec<_>>();
        metadata.insert("tags".into(), Value::Array(labels));
    }
    if let Some(description) = post.get("description") {
        let mut texts = Vec::new();
        flatten_description(description, 0, &mut texts);
        metadata.insert("description".into(), serde_json::to_value(texts)?);
    }
    if let Some(credits) = post.get("credits").and_then(Value::as_array) {
        if let Some(user) = credits.first().and_then(|credit| credit.get("user")) {
            metadata.insert("user".into(), user.clone());
        }
    }
    Ok((item, metadata))
}

async fn api_json(client: &Client, endpoint: Url) -> Result<Value, Box<dyn Error + Send + Sync>> {
    let mut response = client
        .get(endpoint.clone())
        .header("Content-Type", "application/json")
        .header("Origin", "https://audiochan.com")
        .header("Sec-Fetch-Dest", "empty")
        .header("Sec-Fetch-Mode", "cors")
        .header("Sec-Fetch-Site", "same-site")
        .send()
        .await?
        .error_for_status()?;
    if response.url().origin() != endpoint.origin() {
        return Err("Audiochan API redirected outside first-party origin".into());
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_JSON as u64)
    {
        return Err("Audiochan JSON response exceeds 2 MiB".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_JSON.saturating_sub(bytes.len()) {
            return Err("Audiochan JSON response exceeds 2 MiB".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(serde_json::from_slice(&bytes)?)
}

async fn single_at_base(
    client: &Client,
    api: &Url,
    slug: &str,
) -> Result<(MediaItem, Metadata), Box<dyn Error + Send + Sync>> {
    let post = api_json(client, api.join(&format!("audios/slug/{slug}"))?).await?;
    media_record(&post, slug)
}

async fn user_at_base(
    client: &Client,
    api: &Url,
    user: &str,
    max_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let profile = api_json(client, api.join(&format!("users/{user}"))?).await?;
    if let Some(claimed) = profile.pointer("/data/username").and_then(Value::as_str) {
        if claimed != user {
            return Err("Audiochan user profile identity differs from requested username".into());
        }
    }
    let mut seen = HashSet::new();
    let mut events = Vec::new();
    for page in 1..=max_pages.clamp(1, MAX_PAGES) {
        if page > 1 {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        let mut url = api.join(&format!("users/{user}/audios"))?;
        url.query_pairs_mut()
            .append_pair("sfw_only", "false")
            .append_pair("sort", "new")
            .append_pair("page", &page.to_string())
            .append_pair("limit", "12");
        let response = api_json(client, url).await?;
        let rows = response
            .get("data")
            .and_then(Value::as_array)
            .ok_or("Audiochan user API lacks a source audio list")?;
        if rows.len() > MAX_PAGE {
            return Err("Audiochan source exceeds 12 audio records per page".into());
        }
        for post in rows {
            if events.len() >= MAX_CHILDREN {
                break;
            }
            let Some(slug) = post.get("slug").and_then(Value::as_str) else {
                continue;
            };
            if !valid_slug(slug) || !seen.insert(slug.to_owned()) {
                continue;
            }
            let mut meta = Metadata::new();
            meta.insert("category".into(), "audiochan".into());
            meta.insert("subcategory".into(), "user".into());
            meta.insert("username".into(), user.into());
            meta.insert("slug".into(), slug.into());
            events.push(queued_url(format!("https://audiochan.com/a/{slug}"), meta));
        }
        if events.len() >= MAX_CHILDREN
            || !response
                .get("has_more")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        {
            break;
        }
    }
    Ok(events)
}

pub async fn resolve(
    client: &Client,
    target: Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let api = Url::parse("https://api.audiochan.com/")?;
    match target {
        Target::Audio(slug) => Ok(vec![single_at_base(client, &api, &slug).await?.0]),
        Target::User(_) => {
            Err("Audiochan user listings require --dump-messages or --follow-queues".into())
        }
    }
}

pub async fn resolve_messages(
    client: &Client,
    target: Target,
    max_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let api = Url::parse("https://api.audiochan.com/")?;
    match target {
        Target::Audio(slug) => {
            let (item, metadata) = single_at_base(client, &api, &slug).await?;
            Ok(vec![
                ExtractorMessage::Directory {
                    metadata: metadata.clone(),
                },
                ExtractorMessage::Url {
                    url: item.url,
                    metadata,
                },
            ])
        }
        Target::User(user) => user_at_base(client, &api, &user, max_pages).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    #[test]
    fn public_audio_and_user_routes_reject_bad_origins_and_queries() {
        assert_eq!(
            match_url("https://audiochan.com/a/hello"),
            Some(Target::Audio("hello".into()))
        );
        assert_eq!(
            match_url("https://www.audiochan.com/u/alice"),
            Some(Target::User("alice".into()))
        );
        for bad in [
            "https://evilaudiochan.com/a/hello",
            "https://audiochan.com/a/hello?private=true",
            "https://audiochan.com/a/../../private",
            "https://audiochan.com:8443/a/hello",
            "https://user:secret@audiochan.com/a/hello",
            "https://audiochan.com/a/hello%2fprivate",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn media_original_and_nested_text_tags_preserve_source_identity() {
        let source = serde_json::json!({
            "slug":"hello","title":"Hello Audio",
            "audioFile":{"id":42,"url":"https://cdn.audiochan.com/media/a.mp3","filename":"recording.mp3",
                "created_at":"2026-01-02T03:00:00Z","updated_at":"2026-01-03T03:00:00Z"},
            "tags":[{"tag":{"category":"mood","name":"calm"}},{"category":"type","name":"music"}],
            "description":{"content":[{"text":"hello"},{"content":[{"text":"world"}]}]},
            "credits":[{"user":{"display_name":"Alice"}}]
        });
        let (media, meta) = media_record(&source, "hello").unwrap();
        assert_eq!(media.url, "https://cdn.audiochan.com/media/a.mp3");
        assert_eq!(media.filename, "Hello Audio (hello).mp3");
        assert_eq!(media.post_id, Some(42));
        assert_eq!(meta["tags"], serde_json::json!(["mood:calm", "type:music"]));
        assert_eq!(meta["description"], serde_json::json!(["hello", "world"]));
        assert_eq!(meta["user"]["display_name"], "Alice");
        let mut wrong = source.clone();
        wrong["slug"] = "different".into();
        assert!(media_record(&wrong, "hello").is_err());
        wrong = source.clone();
        wrong["audioFile"]["url"] = "https://evil-audiochan.com/a.mp3".into();
        assert!(media_record(&wrong, "hello").is_err());
        wrong["audioFile"]["url"] = "".into();
        assert!(media_record(&wrong, "hello").is_err());
    }

    #[tokio::test]
    async fn source_user_pagination_follows_has_more_and_deduplicates_audio_slugs() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (path, body) in [
                ("/users/alice", r#"{"data":{"username":"alice"}}"#),
                (
                    "/users/alice/audios?sfw_only=false&sort=new&page=1&limit=12",
                    r#"{"data":[{"slug":"first"},{"slug":"second"}],"has_more":true}"#,
                ),
                (
                    "/users/alice/audios?sfw_only=false&sort=new&page=2&limit=12",
                    r#"{"data":[{"slug":"second"},{"slug":"third"}],"has_more":false}"#,
                ),
            ] {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = [0_u8; 8192];
                let n = stream.read(&mut request).await.unwrap();
                assert!(String::from_utf8_lossy(&request[..n])
                    .starts_with(&format!("GET {path} HTTP/1.1")));
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let jobs = user_at_base(&Client::new(), &base, "alice", 4).await.unwrap();
        let urls = jobs.iter().map(|message| {
            serde_json::to_value(message).unwrap()["url"].as_str().unwrap().to_owned()
        }).collect::<Vec<_>>();
        assert_eq!(urls, [
            "https://audiochan.com/a/first",
            "https://audiochan.com/a/second",
            "https://audiochan.com/a/third",
        ]);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn public_audio_get_and_user_queues_preserve_pinned_api_headers() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (path, body) in [
                (
                    "/audios/slug/hello",
                    r#"{"slug":"hello","title":"Hello Audio","audioFile":{"id":42,"url":"https://cdn.audiochan.com/media/a.mp3","filename":"a.mp3"}}"#,
                ),
                ("/users/alice", r#"{"data":{"username":"alice"}}"#),
                (
                    "/users/alice/audios?sfw_only=false&sort=new&page=1&limit=12",
                    r#"{"data":[{"slug":"hello"},{"slug":"hello"},{"slug":"other"}],"has_more":false}"#,
                ),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0u8; 8192];
                let n = socket.read(&mut request).await.unwrap();
                let head = String::from_utf8_lossy(&request[..n]);
                assert!(head.starts_with(&format!("GET {path} HTTP/1.1")));
                assert!(head
                    .to_ascii_lowercase()
                    .contains("origin: https://audiochan.com"));
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        assert_eq!(
            single_at_base(&Client::new(), &base, "hello")
                .await
                .unwrap()
                .0
                .post_id,
            Some(42)
        );
        let events = user_at_base(&Client::new(), &base, "alice", 4)
            .await
            .unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(
            serde_json::to_value(&events[1]).unwrap()["url"],
            "https://audiochan.com/a/other"
        );
        server.await.unwrap();
    }
}
