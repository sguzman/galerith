//! Guarded public Koofr shared-link preview from pinned koofr.py.
//! Does not authenticate, resolve k00.fr redirects, or download protected links.
use crate::{
    message::{media_metadata, ExtractorMessage, Metadata},
    safe_filename, MediaItem,
};
use reqwest::Client;
use serde_json::Value;
use std::{
    collections::{HashSet, VecDeque},
    error::Error,
    time::Duration,
};
use url::Url;

const MAX_JSON: usize = 2 * 1024 * 1024;
const MAX_ITEMS: usize = 160;
const MAX_DIR_ENTRIES: usize = 48;
const MAX_DIRECTORIES: usize = 32;
const MAX_DEPTH: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Share {
    id: String,
}

fn safe_part(name: &str) -> bool {
    (1..=160).contains(&name.len())
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains('%')
        && !name.chars().any(char::is_control)
}

fn safe_share_id(id: &str) -> bool {
    (1..=96).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub fn match_url(raw: &str) -> Option<Share> {
    let lower = raw.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "/../", "/./"]
        .iter()
        .any(|v| lower.contains(v))
        || raw.contains('%')
    {
        return None;
    }
    let url = Url::parse(raw).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !matches!(
            url.host_str(),
            Some("koofr.net" | "koofr.eu" | "app.koofr.net" | "app.koofr.eu")
        )
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
        ["links", id] | ["links", id, ""] if safe_share_id(id) => Some(Share {
            id: (*id).to_owned(),
        }),
        _ => None,
    }
}

async fn response_json(
    client: &Client,
    url: Url,
    referer: &str,
) -> Result<Value, Box<dyn Error + Send + Sync>> {
    let response = client
        .get(url.clone())
        .header("Referer", referer)
        .header("X-Client", "newfrontend")
        .header("X-Koofr-Version", "2.1")
        .send()
        .await?
        .error_for_status()?;
    if response.url().origin() != url.origin() {
        return Err("Koofr public JSON redirected outside the source origin".into());
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_JSON as u64)
    {
        return Err("Koofr public JSON exceeded bounded response size".into());
    }
    let mut response = response;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_JSON.saturating_sub(bytes.len()) {
            return Err("Koofr public JSON exceeded bounded response size".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(serde_json::from_slice(&bytes)?)
}

fn source_download_origin(data: &Value, base: &Url) -> Result<Url, Box<dyn Error + Send + Sync>> {
    let Some(raw) = data.get("publicUrlBase").and_then(Value::as_str) else {
        return Ok(base.clone());
    };
    if raw.is_empty() {
        return Ok(base.clone());
    }
    let url = Url::parse(raw)?;
    if url.scheme() != "https"
        || !matches!(
            url.host_str(),
            Some("app.koofr.net" | "app.koofr.eu" | "koofr.net" | "koofr.eu")
        )
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err("Koofr public media base is outside guarded first-party origins".into());
    }
    Ok(url)
}

fn source_entry_name(item: &Value) -> Result<&str, Box<dyn Error + Send + Sync>> {
    let name = item
        .get("name")
        .and_then(Value::as_str)
        .ok_or("Koofr shared file lacks a source-backed name")?;
    if !safe_part(name) {
        return Err("Koofr source file name is unsafe".into());
    }
    Ok(name)
}

fn download_record(
    file: &Value,
    share: &Share,
    title: &str,
    root: &Url,
    path: &[String],
    ordinal: usize,
) -> Result<(MediaItem, Metadata), Box<dyn Error + Send + Sync>> {
    let name = source_entry_name(file)?;
    if file.get("type").and_then(Value::as_str) != Some("file") {
        return Err("Koofr shared entry is not an original file".into());
    }
    let mut url = root.clone();
    url.path_segments_mut()
        .map_err(|_| "Koofr download URL has no path segments")?
        .extend(["content", "links", &share.id, "files", "get", name]);
    let relpath = if path.is_empty() {
        "/".to_owned()
    } else {
        format!("/{}/{}", path.join("/"), name)
    };
    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair("path", &relpath);
        if path.is_empty() {
            pairs.append_pair("force", "");
        }
    }
    let basename = safe_filename(name);
    if basename.is_empty() {
        return Err("Koofr source file has no usable filename".into());
    }
    let filename = format!("{ordinal:03}_{basename}");
    let item = MediaItem {
        url: url.to_string(),
        filename,
        extractor: "koofr",
        post_id: None,
    };
    let mut metadata = media_metadata(&item);
    metadata.insert("subcategory".into(), "shared".into());
    metadata.insert("share_id".into(), share.id.clone().into());
    metadata.insert("share_title".into(), title.into());
    metadata.insert("num".into(), ordinal.into());
    metadata.insert("path".into(), path.join("/").into());
    metadata.insert("source_filename".into(), name.into());
    for field in ["id", "hash", "size", "modified", "contentType"] {
        if let Some(value) = file.get(field) {
            if value.is_number()
                || value
                    .as_str()
                    .is_some_and(|s| s.len() <= 240 && !s.chars().any(char::is_control))
            {
                metadata.insert(field.to_owned(), value.clone());
            }
        }
    }
    Ok((item, metadata))
}

async fn at_base(
    client: &Client,
    share: &Share,
    base: &Url,
) -> Result<Vec<(MediaItem, Metadata)>, Box<dyn Error + Send + Sync>> {
    let endpoint = base.join(&format!("api/v2/public/links/{}", share.id))?;
    let referer = base.join(&format!("links/{}", share.id))?.to_string();
    let mut index_url = endpoint.clone();
    index_url.query_pairs_mut().append_pair("password", "");
    let data = response_json(client, index_url, &referer).await?;
    let title = data
        .get("name")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && s.len() <= 240 && !s.chars().any(char::is_control))
        .ok_or("Koofr share lacks a supported public title")?;
    let source_id = data
        .get("id")
        .and_then(Value::as_str)
        .ok_or("Koofr share lacks a source ID")?;
    if !safe_share_id(source_id) {
        return Err("Koofr share source ID is malformed".into());
    }
    let download_origin = source_download_origin(&data, base)?;
    let root = data
        .get("file")
        .ok_or("Koofr shared response lacks root file")?;
    let typ = root
        .get("type")
        .and_then(Value::as_str)
        .ok_or("Koofr shared root lacks a type")?;
    if typ != "dir" && typ != "file" {
        return Err("Koofr shared root has an unsupported type".into());
    }
    let mut result = Vec::new();
    if typ == "file" {
        result.push(download_record(
            root,
            share,
            title,
            &download_origin,
            &[],
            1,
        )?);
        return Ok(result);
    }
    let mut queue = VecDeque::from([(Vec::<String>::new(), 0_usize)]);
    let mut visited = HashSet::new();
    while let Some((path, depth)) = queue.pop_front() {
        if !visited.insert(path.clone()) || visited.len() > MAX_DIRECTORIES {
            return Err("Koofr shared directory exceeded bounded traversal".into());
        }
        if depth > 0 {
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        let mut bundle = base.join(&format!("api/v2/public/links/{}/bundle", share.id))?;
        bundle
            .query_pairs_mut()
            .append_pair("password", "")
            .append_pair("path", &format!("/{}", path.join("/")));
        let data = response_json(client, bundle, &referer).await?;
        let files = data
            .get("files")
            .and_then(Value::as_array)
            .ok_or("Koofr directory response lacks file entries")?;
        if files.len() > MAX_DIR_ENTRIES {
            return Err("Koofr directory response exceeds bounded item count".into());
        }
        for file in files {
            let name = source_entry_name(file)?;
            match file.get("type").and_then(Value::as_str) {
                Some("dir") => {
                    if depth >= MAX_DEPTH {
                        return Err("Koofr nested directory exceeds depth cap".into());
                    }
                    let mut child = path.clone();
                    child.push(name.to_owned());
                    queue.push_back((child, depth + 1));
                }
                Some("file") => {
                    if result.len() >= MAX_ITEMS {
                        return Err("Koofr public directory exceeds bounded media count".into());
                    }
                    result.push(download_record(
                        file,
                        share,
                        title,
                        &download_origin,
                        &path,
                        result.len() + 1,
                    )?);
                }
                _ => return Err("Koofr directory includes unsupported entry type".into()),
            }
        }
    }
    Ok(result)
}

pub async fn resolve(
    client: &Client,
    share: Share,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse("https://app.koofr.net/")?;
    Ok(at_base(client, &share, &base)
        .await?
        .into_iter()
        .map(|(file, _)| file)
        .collect())
}

pub async fn resolve_messages(
    client: &Client,
    share: Share,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse("https://app.koofr.net/")?;
    let mut messages = Vec::new();
    for (file, metadata) in at_base(client, &share, &base).await? {
        messages.push(ExtractorMessage::Directory {
            metadata: metadata.clone(),
        });
        messages.push(ExtractorMessage::Url {
            url: file.url,
            metadata,
        });
    }
    Ok(messages)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    fn share() -> Share {
        match_url("https://app.koofr.net/links/1234-abc").unwrap()
    }

    #[test]
    fn pinned_public_share_origins_only() {
        for valid in [
            "https://app.koofr.net/links/1234-abc",
            "https://koofr.net/links/1234-abc",
            "https://app.koofr.eu/links/1234-abc",
        ] {
            assert_eq!(match_url(valid), Some(share()), "{valid}");
        }
        for bad in [
            "https://evilkoofr.net/links/1234-abc",
            "https://app.koofr.net/links/1234-abc?password=secret",
            "https://app.koofr.net/links/../secret",
            "https://name:pass@app.koofr.net/links/1234-abc",
            "https://app.koofr.net:8443/links/1234-abc",
            "https://app.koofr.net/links/abc%2fsecret",
            "https://app.koofr.net/private/1234-abc",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn strict_origin_and_filename_guards_reject_malicious_content() {
        let base = Url::parse("https://app.koofr.net/").unwrap();
        assert!(source_download_origin(
            &serde_json::json!({"publicUrlBase":"https://evilkoofr.net/"}),
            &base
        )
        .is_err());
        assert!(source_download_origin(
            &serde_json::json!({"publicUrlBase":"https://app.koofr.net/" }),
            &base
        )
        .is_ok());
        for name in [
            "../secret.jpg",
            "folder/path.png",
            "%2fprivate.pdf",
            ".",
            "line\nbreak.txt",
        ] {
            assert!(!safe_part(name));
        }
        let file = serde_json::json!({"name": "../escape.jpg", "type": "file"});
        assert!(download_record(&file, &share(), "Open", &base, &[], 1).is_err());
    }

    #[tokio::test]
    async fn one_public_shared_file_uses_only_public_index_endpoint() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 4096];
            let n = stream.read(&mut request).await.unwrap();
            assert!(String::from_utf8_lossy(&request[..n])
                .starts_with("GET /api/v2/public/links/1234-abc?password= HTTP/1.1"));
            let body = r#"{"id":"1234-abc","name":"Public files","file":{"type":"file","name":"Read Me.pdf","hash":"abc123","size":10,"modified":1710000000000}}"#;
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(reply.as_bytes()).await.unwrap();
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let files = at_base(&Client::new(), &share(), &base).await.unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].0.filename, "001_Read Me.pdf");
        assert!(files[0]
            .0
            .url
            .contains("/content/links/1234-abc/files/get/Read%20Me.pdf"));
        assert_eq!(files[0].1["hash"], "abc123");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn bounded_two_directory_bundle_pages_keep_source_paths_and_order() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (prefix, body) in [
                (
                    "GET /api/v2/public/links/1234-abc?password=",
                    r#"{"id":"1234-abc","name":"Public files","file":{"type":"dir","name":"Root","modified":1710000000000}}"#,
                ),
                (
                    "GET /api/v2/public/links/1234-abc/bundle?password=",
                    r#"{"files":[{"type":"file","name":"one.jpg","hash":"h1"},{"type":"dir","name":"More"}]}"#,
                ),
                (
                    "GET /api/v2/public/links/1234-abc/bundle?password=",
                    r#"{"files":[{"type":"file","name":"two.png","hash":"h2"}]}"#,
                ),
            ] {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = [0u8; 4096];
                let n = stream.read(&mut request).await.unwrap();
                assert!(String::from_utf8_lossy(&request[..n]).starts_with(prefix));
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let files = at_base(&Client::new(), &share(), &base).await.unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].1["path"], "");
        assert_eq!(files[1].1["path"], "More");
        assert_eq!(files[1].0.filename, "002_two.png");
        assert!(files[1].0.url.contains("path=%2FMore%2Ftwo.png"));
        server.await.unwrap();
    }
}
