//! Guarded public Foriio image work and user queue subset of pinned foriio.py.
use crate::{
    message::{media_metadata, queued_url, ExtractorMessage, Metadata},
    MediaItem,
};
use reqwest::Client;
use serde_json::Value;
use std::{collections::HashSet, error::Error, time::Duration};
use url::Url;

const MAX_JSON: usize = 2 * 1024 * 1024;
const MAX_IMAGES: usize = 128;
const MAX_PAGES: u16 = 4;
const MAX_CHILDREN: usize = 80;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Work(u64),
    User(String),
}

fn safe_slug(s: &str) -> bool {
    (1..=80).contains(&s.len())
        && s != "works"
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub fn match_url(raw: &str) -> Option<Target> {
    let lower = raw.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "/../", "/./"]
        .iter()
        .any(|x| lower.contains(x))
    {
        return None;
    }
    let url = Url::parse(raw).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !matches!(
            url.host_str(),
            Some("foriio.com" | "www.foriio.com" | "fori.io" | "www.fori.io")
        )
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let paths: Vec<_> = url.path_segments()?.collect();
    match paths.as_slice() {
        ["works", id] | ["works", id, ""]
            if (1..=18).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_digit()) =>
        {
            let id = id.parse::<u64>().ok()?;
            (id > 0).then_some(Target::Work(id))
        }
        [user] | [user, ""] if safe_slug(user) => Some(Target::User((*user).to_owned())),
        _ => None,
    }
}

async fn read_json(
    client: &Client,
    base: &Url,
    path: &str,
    list: bool,
) -> Result<Value, Box<dyn Error + Send + Sync>> {
    let url = base.join(path)?;
    let mut request = client.get(url);
    if list {
        request = request
            .header("Origin", "https://foriio.com")
            .header("Referer", "https://foriio.com/");
    }
    let mut response = request.send().await?.error_for_status()?;
    if response.url().origin() != base.origin() {
        return Err("Foriio API redirected off its exact origin".into());
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_JSON as u64)
    {
        return Err("Foriio API response exceeds byte limit".into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_JSON.saturating_sub(body.len()) {
            return Err("Foriio API response exceeds byte limit".into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(serde_json::from_slice(&body)?)
}

fn scalar_id(v: &Value) -> Option<String> {
    let s = v
        .as_str()
        .map(ToOwned::to_owned)
        .or_else(|| v.as_u64().map(|n| n.to_string()))?;
    ((1..=96).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'))
    .then_some(s)
}

fn original(source: &str) -> Option<(String, String)> {
    let url = Url::parse(source).ok()?;
    if url.scheme() != "https"
        || url.host_str() != Some("foriio.imgix.net")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let name = url.path_segments()?.next_back()?;
    if !(1..=180).contains(&name.len())
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        || name.contains("..")
    {
        return None;
    }
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    if !matches!(
        ext.as_str(),
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif"
    ) {
        return None;
    }
    Some((format!("https://foriio.imgix.net/store/{name}"), ext))
}

fn decode_work(
    root: &Value,
    id: u64,
) -> Result<Vec<(MediaItem, Metadata)>, Box<dyn Error + Send + Sync>> {
    let work = root.get("work").ok_or("Foriio work object absent")?;
    if work
        .get("id")
        .and_then(Value::as_u64)
        .is_some_and(|actual| actual != id)
    {
        return Err("Foriio work ID differs from requested work".into());
    }
    let files = work
        .get("images")
        .and_then(Value::as_array)
        .ok_or("Foriio video/audio/external work type not supported by public image adapter")?;
    if files.is_empty() || files.len() > MAX_IMAGES {
        return Err("Foriio public image work is empty or exceeds media cap".into());
    }
    let mut out = Vec::new();
    for (index, file) in files.iter().enumerate() {
        let (url, ext) = file
            .pointer("/urls/list")
            .and_then(Value::as_str)
            .and_then(original)
            .ok_or("Foriio image URL is not a trusted source thumbnail")?;
        let file_id = file
            .get("id")
            .and_then(scalar_id)
            .ok_or("Foriio image file ID invalid")?;
        let item = MediaItem {
            url,
            filename: format!("{:02} {file_id}.{ext}", index + 1),
            extractor: "foriio",
            post_id: Some(id),
        };
        let mut metadata = media_metadata(&item);
        metadata.insert("subcategory".into(), "work".into());
        metadata.insert("work_id".into(), id.into());
        metadata.insert("file_id".into(), file_id.into());
        metadata.insert("num".into(), (index + 1).into());
        metadata.insert("count".into(), files.len().into());
        metadata.insert("extension".into(), ext.into());
        metadata.insert(
            "post_url".into(),
            format!("https://foriio.com/works/{id}").into(),
        );
        for key in ["title", "published_at", "type"] {
            if let Some(s) = work
                .get(key)
                .and_then(Value::as_str)
                .filter(|s| s.len() <= 240 && !s.chars().any(char::is_control))
            {
                metadata.insert(key.into(), s.into());
            }
        }
        if let Some(author) = work.get("author").filter(|value| value.is_object()) {
            metadata.insert("author".into(), author.clone());
        }
        for key in ["width", "height"] {
            if let Some(value) = file
                .get(key)
                .and_then(Value::as_u64)
                .filter(|value| (1..=100_000).contains(value))
            {
                metadata.insert(key.into(), value.into());
            }
        }
        out.push((item, metadata));
    }
    Ok(out)
}

fn decode_user(
    root: &Value,
    page: u16,
    user: &str,
    seen: &mut HashSet<u64>,
    out: &mut Vec<ExtractorMessage>,
) -> Result<bool, Box<dyn Error + Send + Sync>> {
    let works = root
        .get("works")
        .and_then(Value::as_array)
        .ok_or("Foriio user works array absent")?;
    if works.len() > 20 {
        return Err("Foriio user API exceeds pinned 20-work page".into());
    }
    let meta = root
        .get("meta")
        .ok_or("Foriio pagination metadata absent")?;
    let current = meta
        .get("current_page")
        .and_then(Value::as_u64)
        .ok_or("No current page")?;
    let total = meta
        .get("total_pages")
        .and_then(Value::as_u64)
        .ok_or("No total pages")?;
    if current != u64::from(page) || total == 0 || current > total {
        return Err("Foriio user page metadata inconsistent with requested page".into());
    }
    for work in works {
        if out.len() >= MAX_CHILDREN {
            break;
        }
        let Some(id) = work.get("id").and_then(Value::as_u64).filter(|n| *n > 0) else {
            continue;
        };
        if !seen.insert(id) {
            continue;
        }
        let mut metadata = Metadata::new();
        metadata.insert("category".into(), "foriio".into());
        metadata.insert("subcategory".into(), "user".into());
        metadata.insert("username".into(), user.into());
        metadata.insert("work_id".into(), id.into());
        if let Some(kind) = work
            .get("type")
            .and_then(Value::as_str)
            .filter(|s| s.len() <= 80 && !s.chars().any(char::is_control))
        {
            metadata.insert("type".into(), kind.into());
        }
        out.push(queued_url(
            format!("https://foriio.com/works/{id}"),
            metadata,
        ));
    }
    Ok(current < total)
}

async fn at_work(
    client: &Client,
    base: &Url,
    id: u64,
) -> Result<Vec<(MediaItem, Metadata)>, Box<dyn Error + Send + Sync>> {
    decode_work(
        &read_json(client, base, &format!("v1/works/{id}"), false).await?,
        id,
    )
}

async fn at_user(
    client: &Client,
    base: &Url,
    user: &str,
    max_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    for page in 1..=max_pages.clamp(1, MAX_PAGES) {
        if page > 1 {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        let query = format!("v1/users/{user}/works?page={page}&per_page=20");
        let data = read_json(client, base, &query, true).await?;
        let more = decode_user(&data, page, user, &mut seen, &mut result)?;
        if !more || result.len() >= MAX_CHILDREN {
            break;
        }
    }
    Ok(result)
}

pub async fn resolve(
    client: &Client,
    target: Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse("https://api.foriio.com/api/")?;
    match target {
        Target::Work(id) => Ok(at_work(client, &base, id)
            .await?
            .into_iter()
            .map(|(item, _)| item)
            .collect()),
        Target::User(_) => {
            Err("Foriio user listing requires --dump-messages or --follow-queues".into())
        }
    }
}

pub async fn resolve_messages(
    client: &Client,
    target: Target,
    max_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse("https://api.foriio.com/api/")?;
    match target {
        Target::User(user) => at_user(client, &base, &user, max_pages).await,
        Target::Work(id) => {
            let mut result = Vec::new();
            for (item, metadata) in at_work(client, &base, id).await? {
                result.push(ExtractorMessage::Directory {
                    metadata: metadata.clone(),
                });
                result.push(ExtractorMessage::Url {
                    url: item.url,
                    metadata,
                });
            }
            Ok(result)
        }
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
    fn routes_accept_pinned_work_and_user_forms_only() {
        assert_eq!(
            match_url("https://foriio.com/works/12345"),
            Some(Target::Work(12345))
        );
        assert_eq!(
            match_url("https://www.fori.io/artist"),
            Some(Target::User("artist".into()))
        );
        for bad in [
            "https://fakeforiio.com/works/1",
            "https://foriio.com/works/0",
            "https://foriio.com/works/../private",
            "https://foriio.com/works/3?private=1",
            "https://foriio.com:8443/works/3",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn images_preserve_source_ids_and_reject_foreign_media() {
        let api = serde_json::json!({"work":{"id":12345,"title":"Art",
        "author":{"id":8,"screen_name":"creator"},"images":[
            {"id":10,"width":800,"urls":{"list":"https://foriio.imgix.net/store/first.png?resize=1"}},
            {"id":20,"width":900,"urls":{"list":"https://foriio.imgix.net/store/second.jpg"}}
        ]}});
        let media = decode_work(&api, 12345).unwrap();
        assert_eq!(media.len(), 2);
        assert_eq!(media[0].0.url, "https://foriio.imgix.net/store/first.png");
        assert_eq!(media[0].0.filename, "01 10.png");
        assert_eq!(media[1].1["work_id"], 12345);
        assert_eq!(media[0].1["author"]["screen_name"], "creator");
        assert!(decode_work(&api, 54321).is_err());
        let evil = serde_json::json!({"work":{"images":[{"id":1,"urls":{"list":"https://evilforiio.imgix.net/store/one.jpg"}}]}});
        assert!(decode_work(&evil, 12345).is_err());
    }

    #[test]
    fn user_queue_deduplicates_and_validates_pagination_identity() {
        let value = serde_json::json!({"works":[{"id":1,"type":"image"},{"id":1,"type":"image"},{"id":2,"type":"video"}],
            "meta":{"current_page":1,"total_pages":2}});
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        assert!(decode_user(&value, 1, "artist", &mut seen, &mut out).unwrap());
        assert_eq!(out.len(), 2);
        assert_eq!(
            serde_json::to_value(&out[1]).unwrap()["url"],
            "https://foriio.com/works/2"
        );
        assert!(decode_user(&value, 2, "artist", &mut seen, &mut out).is_err());
    }

    #[tokio::test]
    async fn bounded_user_api_follows_two_pages_with_first_seen_work_ids() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (path, body) in [
                (
                    "/api/v1/users/artist/works?page=1&per_page=20",
                    r#"{"works":[{"id":12,"type":"image"}],"meta":{"current_page":1,"total_pages":2}}"#,
                ),
                (
                    "/api/v1/users/artist/works?page=2&per_page=20",
                    r#"{"works":[{"id":12,"type":"image"},{"id":34,"type":"image"}],"meta":{"current_page":2,"total_pages":2}}"#,
                ),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buf = [0u8; 4096];
                let n = socket.read(&mut buf).await.unwrap();
                assert!(
                    String::from_utf8_lossy(&buf[..n]).starts_with(&format!("GET {path} HTTP/1.1"))
                );
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{addr}/api/")).unwrap();
        let children = at_user(&Client::new(), &base, "artist", 4).await.unwrap();
        let urls = children
            .iter()
            .map(|value| {
                serde_json::to_value(value).unwrap()["url"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            urls,
            vec!["https://foriio.com/works/12", "https://foriio.com/works/34"]
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn public_work_and_user_queue_read_bounded_local_api() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (path, body) in [
                (
                    "/api/v1/works/12345",
                    r#"{"work":{"id":12345,"images":[{"id":10,"urls":{"list":"https://foriio.imgix.net/store/a.jpg"}}]}}"#,
                ),
                (
                    "/api/v1/users/artist/works?page=1&per_page=20",
                    r#"{"works":[{"id":12345,"type":"image"}],"meta":{"current_page":1,"total_pages":1}}"#,
                ),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut data = [0u8; 4096];
                let count = socket.read(&mut data).await.unwrap();
                assert!(String::from_utf8_lossy(&data[..count])
                    .starts_with(&format!("GET {path} HTTP/1.1")));
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{addr}/api/")).unwrap();
        assert_eq!(
            at_work(&Client::new(), &base, 12345).await.unwrap().len(),
            1
        );
        assert_eq!(
            at_user(&Client::new(), &base, "artist", 4)
                .await
                .unwrap()
                .len(),
            1
        );
        server.await.unwrap();
    }
}
