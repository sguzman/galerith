//! Independent public S3ndpics post, user and search subset of pinned s3ndpics.py.
//! JSON and media are bounded; private/authenticated features are unsupported.
use crate::{
    message::{media_metadata, ExtractorMessage, Metadata},
    MediaItem,
};
use reqwest::{redirect::Policy, Client};
use serde_json::Value;
use std::{error::Error, time::Duration};
use url::Url;

const MAX_JSON: usize = 2 * 1024 * 1024;
const MAX_PAGES: u16 = 4;
const MAX_POSTS: usize = 128;
const MAX_FILES: usize = 160;
const MAX_ROWS_PER_PAGE: usize = 48;
const MAX_FILES_PER_POST: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Post(String),
    User(String),
    Search { key: String, value: String },
}

fn public_id(raw: &str) -> bool {
    (1..=48).contains(&raw.len())
        && raw.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

fn safe_username(raw: &str) -> bool {
    (1..=64).contains(&raw.len())
        && raw.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
}

pub fn match_url(raw: &str) -> Option<Target> {
    let lower = raw.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "/../", "/./"].iter().any(|x| lower.contains(x)) {
        return None;
    }
    let url = Url::parse(raw).ok()?;
    if !matches!(url.scheme(), "https" | "http")
        || !matches!(url.host_str(), Some("s3nd.pics" | "www.s3nd.pics"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let path: Vec<_> = url.path_segments()?.collect();
    match path.as_slice() {
        ["post", id] | ["post", id, ""] if public_id(id) && url.query().is_none() => {
            Some(Target::Post((*id).to_owned()))
        }
        ["user", user] | ["user", user, ""]
            if safe_username(user) && url.query().is_none() =>
        {
            Some(Target::User((*user).to_owned()))
        }
        ["search"] | ["search", ""] => {
            let params: Vec<_> = url.query_pairs().collect();
            if params.len() != 1 || !matches!(params[0].0.as_ref(), "q" | "tag" | "tags") {
                return None;
            }
            let value = params[0].1.to_string();
            if value.is_empty() || value.len() > 120
                || value.chars().any(char::is_control)
            {
                return None;
            }
            Some(Target::Search {
                key: params[0].0.to_string(),
                value,
            })
        }
        _ => None,
    }
}

fn valid_file_path(raw: &str) -> bool {
    if raw.is_empty() || raw.len() > 420 || raw.starts_with('/')
        || raw.contains('?') || raw.contains('#')
        || raw.contains('%') || raw.contains('\\') || raw.contains(':')
        || raw.chars().any(char::is_control)
    {
        return false;
    }
    raw.split('/').all(|part| {
        !part.is_empty() && part != "." && part != ".."
            && !part.starts_with('.')
            && part.bytes().all(|byte| byte.is_ascii_alphanumeric()
                || matches!(byte, b'.' | b'_' | b'-'))
    })
}

fn file_url(path: &str) -> Option<(String, String)> {
    if !valid_file_path(path) {
        return None;
    }
    let name = path.rsplit('/').next()?;
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    if !matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "gif" | "webp"
        | "avif" | "mp4" | "webm" | "mov" | "mp3" | "flac" | "wav")
    {
        return None;
    }
    let url = Url::parse(&format!("https://s3.s3nd.pics/s3nd-pics/{path}")).ok()?;
    if url.host_str() != Some("s3.s3nd.pics") {
        return None;
    }
    Some((url.to_string(), ext))
}

fn records_for_post(
    post: &Value,
    result: &mut Vec<(MediaItem, Metadata)>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let id = post.get("_id").and_then(Value::as_str)
        .filter(|s| public_id(s))
        .ok_or("S3ndpics public post is missing a validated ID")?;
    let files = post.get("files").and_then(Value::as_array)
        .ok_or("S3ndpics public post does not contain file entries")?;
    if files.len() > MAX_FILES_PER_POST {
        return Err("S3ndpics public post exceeds per-post file limit".into());
    }
    let mut parent = Metadata::new();
    parent.insert("category".into(), "s3ndpics".into());
    parent.insert("subcategory".into(), "post".into());
    parent.insert("id".into(), id.into());
    parent.insert("post_url".into(), format!("https://s3nd.pics/post/{id}").into());
    for (from, to) in [
        ("createdAt", "date_source"),
        ("updatedAt", "date_updated_source"),
        ("title", "title"),
    ] {
        if let Some(value) = post.get(from).and_then(Value::as_str)
            .filter(|s| s.len() <= 240 && !s.chars().any(char::is_control))
        {
            parent.insert(to.into(), value.into());
        }
    }
    if let Some(user) = post.get("userId").filter(|v| v.is_string() || v.is_object()) {
        parent.insert("user".into(), user.clone());
    }
    parent.insert("count".into(), files.len().into());
    for (num, file) in files.iter().enumerate() {
        if result.len() >= MAX_FILES {
            return Err("S3ndpics result exceeds bounded media count".into());
        }
        let Some(path) = file.get("url").and_then(Value::as_str) else {
            return Err("S3ndpics public file lacks its source path".into());
        };
        let (url, ext) = file_url(path)
            .ok_or("S3ndpics public file path is untrusted or has unsupported extension")?;
        let ordinal = num + 1;
        let item = MediaItem {
            url,
            filename: format!("{id}_{ordinal:02}.{ext}"),
            extractor: "s3ndpics",
            post_id: None,
        };
        let mut meta = media_metadata(&item);
        for (key, value) in &parent {
            meta.insert(key.clone(), value.clone());
        }
        meta.insert("num".into(), ordinal.into());
        meta.insert("extension".into(), ext.into());
        meta.insert("source_file_path".into(), path.into());
        if let Some(kind) = file.get("type").and_then(Value::as_str)
            .filter(|x| x.len() <= 80 && !x.chars().any(char::is_control))
        {
            meta.insert("type".into(), kind.into());
        }
        result.push((item, meta));
    }
    Ok(())
}

async fn get_json(
    client: &Client,
    endpoint: Url,
) -> Result<Value, Box<dyn Error + Send + Sync>> {
    let mut response = client.get(endpoint.clone()).send().await?;
    if response.url().origin() != endpoint.origin() || response.status().is_redirection() {
        return Err("S3ndpics source redirected outside approved endpoint".into());
    }
    response = response.error_for_status()?;
    if response.content_length().is_some_and(|n| n > MAX_JSON as u64) {
        return Err("S3ndpics public source JSON exceeds response limit".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_JSON.saturating_sub(bytes.len()) {
            return Err("S3ndpics public source JSON exceeds response limit".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(serde_json::from_slice(&bytes)?)
}

async fn at_base(
    client: &Client,
    base: &Url,
    target: &Target,
    max_pages: u16,
) -> Result<Vec<(MediaItem, Metadata)>, Box<dyn Error + Send + Sync>> {
    let mut records = Vec::new();
    if let Target::Post(id) = target {
        let json = get_json(client, base.join(&format!("api/posts/{id}"))?).await?;
        records_for_post(json.get("post").ok_or("S3ndpics public post response is empty")?, &mut records)?;
        return Ok(records);
    }
    let (limit, params): (&str, Vec<(String, String)>) = match target {
        Target::User(username) => {
            let json = get_json(client, base.join(&format!("api/users/username/{username}"))?).await?;
            let id = json.pointer("/user/_id").and_then(Value::as_str)
                .filter(|id| public_id(id))
                .ok_or("S3ndpics public user response lacks a validated ID")?;
            ("12", vec![("userId".into(), id.into()), ("sortBy".into(), "newest".into())])
        }
        Target::Search { key, value } => ("20", vec![(key.clone(), value.clone())]),
        Target::Post(_) => unreachable!(),
    };
    for page in 1..=max_pages.clamp(1, MAX_PAGES) {
        if page > 1 {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        let mut endpoint = base.join("api/posts")?;
        {
            let mut query = endpoint.query_pairs_mut();
            for (key, value) in &params {
                query.append_pair(key, value);
            }
            query.append_pair("limit", limit).append_pair("page", &page.to_string());
        }
        let json = get_json(client, endpoint).await?;
        let posts = json.get("posts").and_then(Value::as_array)
            .ok_or("S3ndpics public listing response lacks posts")?;
        if posts.len() > MAX_ROWS_PER_PAGE {
            return Err("S3ndpics listing exceeds bounded posts per page".into());
        }
        for post in posts {
            records_for_post(post, &mut records)?;
            if records.len() > MAX_FILES {
                return Err("S3ndpics listing exceeds bounded media count".into());
            }
        }
        let total_pages = json.pointer("/pagination/pages").and_then(Value::as_u64)
            .ok_or("S3ndpics listing lacks pagination page count")?;
        if posts.is_empty() || page as u64 >= total_pages || records.len() >= MAX_FILES {
            break;
        }
    }
    Ok(records)
}

pub async fn resolve(
    _client: &Client, target: Target, max_pages: u16,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let client = Client::builder().redirect(Policy::none()).timeout(Duration::from_secs(15)).build()?;
    let base = Url::parse("https://s3nd.pics/")?;
    Ok(at_base(&client, &base, &target, max_pages).await?
        .into_iter().map(|(item, _)| item).collect())
}

pub async fn resolve_messages(
    _client: &Client, target: Target, max_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let client = Client::builder().redirect(Policy::none()).timeout(Duration::from_secs(15)).build()?;
    let base = Url::parse("https://s3nd.pics/")?;
    let mut events = Vec::new();
    for (item, metadata) in at_base(&client, &base, &target, max_pages).await? {
        events.push(ExtractorMessage::Directory { metadata: metadata.clone() });
        events.push(ExtractorMessage::Url { url: item.url, metadata });
    }
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::TcpListener};

    #[test]
    fn public_post_user_search_patterns_reject_private_or_forged_inputs() {
        assert!(matches!(match_url("https://s3nd.pics/post/0123456789abcdef01234567"), Some(Target::Post(_))));
        assert_eq!(match_url("https://www.s3nd.pics/user/sample_1"), Some(Target::User("sample_1".into())));
        assert_eq!(match_url("https://s3nd.pics/search?q=night"), Some(Target::Search { key:"q".into(), value:"night".into() }));
        for bad in [
            "https://evil-s3nd.pics/post/0123456789abcdef01234567",
            "https://s3nd.pics/post/NOT_HEX",
            "https://s3nd.pics/user/../private",
            "https://s3nd.pics/search?q=night&token=secret",
            "https://s3nd.pics:8443/user/sample",
            "https://name:password@s3nd.pics/user/sample",
            "https://s3nd.pics/search?private=1",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn public_post_preserves_file_order_and_source_metadata_and_rejects_unsafe_paths() {
        let post=serde_json::json!({
            "_id":"0123456789abcdef01234567",
            "createdAt":"2026-05-05T00:00:00.000Z",
            "userId":{"username":"tester"},
            "files":[
                {"type":"image","url":"uploads/first.png"},
                {"type":"video","url":"uploads/second.mp4"}
            ]
        });
        let mut records=Vec::new();
        records_for_post(&post,&mut records).unwrap();
        assert_eq!(records.len(),2);
        assert_eq!(records[0].0.url,"https://s3.s3nd.pics/s3nd-pics/uploads/first.png");
        assert_eq!(records[1].0.filename,"0123456789abcdef01234567_02.mp4");
        assert_eq!(records[0].1["user"]["username"],"tester");
        assert_eq!(records[0].1["date_source"],"2026-05-05T00:00:00.000Z");
        assert_eq!(records[1].1["num"],2);
        for bad in ["../private.png","uploads/%2fsecret.jpg","uploads/evil\\path.png","https://external.test/evil.jpg","uploads/file.exe"] {
            assert!(file_url(bad).is_none(),"{bad}");
        }
    }

    #[tokio::test]
    async fn public_user_api_paginates_two_source_pages_and_retains_order() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (path, body) in [
                ("/api/users/username/tester", r#"{"user":{"_id":"0123456789abcdef01234567"}}"#),
                ("/api/posts?userId=0123456789abcdef01234567&sortBy=newest&limit=12&page=1",
                 r#"{"posts":[{"_id":"aaaa1111","files":[{"url":"2026/one.jpg"}]}],"pagination":{"pages":2,"total":2}}"#),
                ("/api/posts?userId=0123456789abcdef01234567&sortBy=newest&limit=12&page=2",
                 r#"{"posts":[{"_id":"bbbb2222","files":[{"url":"2026/two.png"}]}],"pagination":{"pages":2,"total":2}}"#),
            ] {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut buf = [0u8; 4096];
                let count = stream.read(&mut buf).await.unwrap();
                assert!(String::from_utf8_lossy(&buf[..count])
                    .starts_with(&format!("GET {path} HTTP/1.1")));
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let result = at_base(&Client::new(),&base,&Target::User("tester".into()),3).await.unwrap();
        assert_eq!(result.len(),2);
        assert_eq!(result[0].0.filename,"aaaa1111_01.jpg");
        assert_eq!(result[1].0.filename,"bbbb2222_01.png");
        server.await.unwrap();
    }
}
