//! Guarded public Picazor user-file preview from pinned picazor.py.
use crate::{
    message::{media_metadata, ExtractorMessage, Metadata},
    safe_filename, MediaItem,
};
use percent_encoding::percent_decode_str;
use reqwest::Client;
use serde_json::Value;
use std::{error::Error, time::Duration};
use url::Url;

const MAX_JSON: usize = 2 * 1024 * 1024;
const MAX_PAGES: u16 = 4;
const MAX_FILES_PER_PAGE: usize = 48;
const MAX_MEDIA: usize = 160;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    name: String,
}

fn safe_user(name: &str) -> bool {
    (1..=96).contains(&name.len())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

pub fn match_url(raw: &str) -> Option<User> {
    let lower = raw.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "/../", "/./"].iter().any(|bad| lower.contains(bad))
        || raw.contains('%')
    {
        return None;
    }
    let url = Url::parse(raw).ok()?;
    if !matches!(url.scheme(), "https" | "http")
        || !matches!(url.host_str(), Some("picazor.com" | "www.picazor.com"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let segments: Vec<_> = url.path_segments()?.collect();
    let (lang, user) = match segments.as_slice() {
        [lang, user] | [lang, user, ""] => (*lang, *user),
        _ => return None,
    };
    if lang.len() != 2 || !lang.bytes().all(|b| b.is_ascii_lowercase()) || !safe_user(user) {
        return None;
    }
    Some(User { name: user.into() })
}

fn source_id(value: &Value) -> Option<String> {
    value.as_u64().map(|n| n.to_string()).or_else(|| {
        value
            .as_str()
            .filter(|id| {
                (1..=64).contains(&id.len())
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
            })
            .map(ToOwned::to_owned)
    })
}

fn original(root: &Url, raw: &str) -> Option<(String, String, String)> {
    if !raw.starts_with('/') || raw.starts_with("//")
        || raw.len() > 1024 || raw.contains('\\')
        || raw.contains('?') || raw.contains('#')
        || raw.chars().any(char::is_control)
    {
        return None;
    }
    let url = root.join(raw).ok()?;
    if url.origin() != root.origin() || !url.username().is_empty()
        || url.password().is_some() || url.query().is_some() || url.fragment().is_some()
    {
        return None;
    }
    let mut last = None;
    for segment in url.path_segments()? {
        let decoded = percent_decode_str(segment).decode_utf8().ok()?;
        if decoded == "." || decoded == ".." || decoded.contains('/') || decoded.contains('\\')
            || decoded.chars().any(char::is_control)
        {
            return None;
        }
        last = Some(decoded.into_owned());
    }
    let last = last?;
    let extension = last.rsplit_once('.')?.1.to_ascii_lowercase();
    if !matches!(
        extension.as_str(),
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif" | "mp4" | "webm"
    ) {
        return None;
    }
    let safe = safe_filename(&last);
    if safe.is_empty() || matches!(safe.as_str(), "." | "..") {
        return None;
    }
    Some((url.to_string(), extension, safe))
}

fn decode_page(
    posts: &Value,
    user: &User,
    root: &Url,
    output: &mut Vec<(MediaItem, Metadata)>,
    directory: &mut Option<Metadata>,
) -> Result<usize, Box<dyn Error + Send + Sync>> {
    let posts = posts
        .as_array()
        .ok_or("Picazor public files API response is not an array")?;
    if posts.len() > MAX_FILES_PER_PAGE {
        return Err("Picazor public files page exceeds the bounded size".into());
    }
    for post in posts {
        let Some(raw) = post.get("path").and_then(Value::as_str).filter(|s| !s.is_empty()) else {
            continue;
        };
        let (url, extension, _) = original(root, raw)
            .ok_or("Picazor file path is unsafe or outside supported original media")?;
        let id = post.get("id").and_then(source_id)
            .ok_or("Picazor original lacks a source-backed file id")?;
        let num = post
            .get("order")
            .and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse::<u64>().ok()))
            .filter(|n| *n <= 999_999)
            .ok_or("Picazor original has no bounded source order")?;
        if output.len() >= MAX_MEDIA {
            return Err("Picazor user gallery exceeds bounded media cap".into());
        }
        let filename = safe_filename(&format!("{id}_{num:03}.{extension}"));
        let item = MediaItem {
            url,
            filename,
            extractor: "picazor",
            post_id: None,
        };
        let mut meta = media_metadata(&item);
        meta.insert("subcategory".into(), "user".into());
        meta.insert("user".into(), user.name.clone().into());
        meta.insert("id".into(), id.into());
        meta.insert("num".into(), num.into());
        meta.insert("extension".into(), extension.into());
        meta.insert("source_path".into(), raw.into());
        if let Some(subject) = post.get("subject").and_then(Value::as_str)
            .filter(|v| v.len() <= 240 && !v.chars().any(char::is_control))
        {
            meta.insert("subject".into(), subject.into());
        }
        if directory.is_none() {
            let mut dir = Metadata::new();
            dir.insert("category".into(), "picazor".into());
            dir.insert("subcategory".into(), "user".into());
            dir.insert("user".into(), user.name.clone().into());
            dir.insert("count".into(), num.into());
            if let Some(subject) = meta.get("subject") {
                dir.insert("subject".into(), subject.clone());
            }
            *directory = Some(dir);
        }
        output.push((item, meta));
    }
    Ok(posts.len())
}

async fn read_json(
    client: &Client,
    endpoint: Url,
    referer: &str,
) -> Result<Value, Box<dyn Error + Send + Sync>> {
    let mut response = client
        .get(endpoint.clone())
        .header(reqwest::header::REFERER, referer)
        .send()
        .await?
        .error_for_status()?;
    if response.url().origin() != endpoint.origin() {
        return Err("Picazor public API redirected outside its origin".into());
    }
    if response.content_length().is_some_and(|n| n > MAX_JSON as u64) {
        return Err("Picazor public API response exceeds the JSON cap".into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_JSON.saturating_sub(body.len()) {
            return Err("Picazor public API response exceeds the JSON cap".into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(serde_json::from_slice(&body)?)
}

async fn at_root(
    client: &Client,
    root: &Url,
    user: &User,
    max_pages: u16,
) -> Result<(Option<Metadata>, Vec<(MediaItem, Metadata)>), Box<dyn Error + Send + Sync>> {
    let mut output = Vec::new();
    let mut directory = None;
    let referer = format!("https://picazor.com/en/{}", user.name);
    for page in 1..=max_pages.clamp(1, MAX_PAGES) {
        if page > 1 {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        let mut endpoint = root.join(&format!("api/files/{}/sfiles", user.name))?;
        endpoint.query_pairs_mut().append_pair("page", &page.to_string());
        let posts = read_json(client, endpoint, &referer).await?;
        if decode_page(&posts, user, root, &mut output, &mut directory)? == 0 {
            break;
        }
    }
    Ok((directory, output))
}

pub async fn resolve(
    client: &Client,
    user: User,
    max_pages: u16,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let (_, media) = at_root(client, &Url::parse("https://picazor.com/")?, &user, max_pages).await?;
    Ok(media.into_iter().map(|(item, _)| item).collect())
}

pub async fn resolve_messages(
    client: &Client,
    user: User,
    max_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let (directory, media) =
        at_root(client, &Url::parse("https://picazor.com/")?, &user, max_pages).await?;
    let mut events = Vec::new();
    if let Some(metadata) = directory {
        events.push(ExtractorMessage::Directory { metadata });
    }
    events.extend(media.into_iter().map(|(item, metadata)| {
        ExtractorMessage::Url { url: item.url, metadata }
    }));
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::TcpListener};

    #[test]
    fn pinned_languages_and_users_reject_foreign_hosts_or_private_urls() {
        assert_eq!(match_url("https://picazor.com/en/example").unwrap().name, "example");
        assert_eq!(match_url("https://www.picazor.com/de/example/").unwrap().name, "example");
        for bad in [
            "https://fakepicazor.com/en/example",
            "https://picazor.com/english/example",
            "https://picazor.com/en/example?secret=1",
            "https://user:pass@picazor.com/en/example",
            "https://picazor.com:8443/en/example",
            "https://picazor.com/en/%2e%2e",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn original_files_preserve_source_order_and_skip_missing_paths() {
        let user = match_url("https://picazor.com/en/example").unwrap();
        let root = Url::parse("https://picazor.com/").unwrap();
        let data = serde_json::json!([
            {"id":123,"order":7,"path":"/files/a%20b.png","subject":"Profile"},
            {"id":124,"order":8,"path":""},
            {"id":"p125","order":"9","path":"/files/clip.webm"}
        ]);
        let mut out = Vec::new();
        let mut directory = None;
        assert_eq!(decode_page(&data, &user, &root, &mut out, &mut directory).unwrap(), 3);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].0.url, "https://picazor.com/files/a%20b.png");
        assert_eq!(out[0].0.filename, "123_007.png");
        assert_eq!(out[1].0.filename, "p125_009.webm");
        assert_eq!(out[0].1["subject"], "Profile");
        assert_eq!(directory.unwrap()["count"], 7);
    }

    #[test]
    fn file_origins_and_encoded_path_escapes_are_rejected() {
        let root = Url::parse("https://picazor.com/").unwrap();
        for bad in [
            "//evil.test/a.png",
            "https://evil.test/a.png",
            "/files/%2fsecret.jpg",
            "/files/%5csecret.jpg",
            "/files/%2e%2e/private.jpg",
            "/files/%00nul.png",
            "/files/anything.php",
            "/files/photo.png?token=secret",
        ] {
            assert!(original(&root, bad).is_none(), "{bad}");
        }
    }

    #[tokio::test]
    async fn bounded_user_file_api_fetches_source_order_with_pinned_referer() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (path, data) in [
                ("/api/files/example/sfiles?page=1", r#"[{"id":100,"order":1,"path":"/files/first.jpg","subject":"User"}]"#),
                ("/api/files/example/sfiles?page=2", r#"[]"#),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = [0u8; 8192];
                let count = socket.read(&mut bytes).await.unwrap();
                let request = String::from_utf8_lossy(&bytes[..count]);
                assert!(request.starts_with(&format!("GET {path} HTTP/1.1")));
                assert!(request.to_ascii_lowercase().contains("referer: https://picazor.com/en/example"));
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{data}",
                    data.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let user = match_url("https://picazor.com/en/example").unwrap();
        let root = Url::parse(&format!("http://{addr}/")).unwrap();
        let (directory, media) = at_root(&Client::new(), &root, &user, 4).await.unwrap();
        assert_eq!(directory.unwrap()["subject"], "User");
        assert_eq!(media.len(), 1);
        assert_eq!(media[0].0.filename, "100_001.jpg");
        server.await.unwrap();
    }
}
