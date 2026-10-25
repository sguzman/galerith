//! Bounded 94chan jschan public thread attachments and board queues.
//! Independent parser of the gallery-dl pinned jschan.py contract.
use crate::{
    message::{media_metadata, queued_url, ExtractorMessage, Metadata},
    safe_filename, MediaItem,
};
use reqwest::Client;
use serde_json::Value;
use std::{collections::HashSet, error::Error};
use url::Url;

const MAX_JSON: usize = 2 * 1024 * 1024;
const MAX_POSTS: usize = 256;
const MAX_FILES: usize = 160;
const MAX_FILE_PER_POST: usize = 12;
const MAX_CATALOG: usize = 128;
type MediaRecords = Vec<(MediaItem, Metadata)>;


#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Target {
    Thread { board: String, id: u64 },
    Board { board: String },
}

fn valid_board(board: &str) -> bool {
    (1..=16).contains(&board.len())
        && board
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn valid_id(raw: &str) -> Option<u64> {
    if raw.len() > 18 || raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    raw.parse().ok().filter(|v| *v > 0)
}

pub fn match_url(raw: &str) -> Option<Target> {
    let lower = raw.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "/../", "/./"]
        .iter()
        .any(|part| lower.contains(part))
    {
        return None;
    }
    let url = Url::parse(raw).ok()?;
    if !matches!(url.scheme(), "https" | "http")
        || url.host_str() != Some("94chan.org")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let parts: Vec<_> = url.path_segments()?.collect();
    let board = *parts.first()?;
    if !valid_board(board) {
        return None;
    }
    match parts.as_slice() {
        [_, "thread", filename] if filename.ends_with(".html") => Some(Target::Thread {
            board: board.into(),
            id: valid_id(filename.strip_suffix(".html")?)?,
        }),
        [_] | [_, ""] | [_, "index.html"] | [_, "catalog.html"] => Some(Target::Board {
            board: board.into(),
        }),
        [_, page] if page.ends_with(".html") && valid_id(page.strip_suffix(".html")?).is_some() => {
            Some(Target::Board {
                board: board.into(),
            })
        }
        _ => None,
    }
}

fn api_path(target: &Target) -> String {
    match target {
        Target::Thread { board, id } => format!("{board}/thread/{id}.json"),
        Target::Board { board } => format!("{board}/catalog.json"),
    }
}

fn scalar_id(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(valid_id))
        .filter(|id| *id > 0)
}

fn file_name(value: &str) -> Option<String> {
    if value.is_empty()
        || value.len() > 240
        || value == "."
        || value == ".."
        || value.contains("..")
        || value.contains('/')
        || value.contains('\\')
        || value.contains('%')
        || value.chars().any(char::is_control)
    {
        return None;
    }
    let ext = value.rsplit_once('.')?.1.to_ascii_lowercase();
    if !matches!(
        ext.as_str(),
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "webm" | "mp4"
    ) {
        return None;
    }
    let safe = safe_filename(value);
    (!safe.is_empty()).then_some(safe)
}

fn thread_meta(board: &str, root: &Value, thread_id: u64) -> Metadata {
    let mut data = Metadata::new();
    data.insert("category".into(), "jschan".into());
    data.insert("subcategory".into(), "thread".into());
    data.insert("board".into(), board.into());
    data.insert("threadId".into(), thread_id.into());
    data.insert(
        "post_url".into(),
        format!("https://94chan.org/{board}/thread/{thread_id}.html").into(),
    );
    if let Some(subject) = root
        .get("subject")
        .and_then(Value::as_str)
        .filter(|s| s.len() <= 240 && !s.chars().any(char::is_control))
    {
        data.insert("subject".into(), subject.into());
    }
    data
}

fn thread_records(
    root: &Value,
    board: &str,
    requested: u64,
) -> Result<(Metadata, MediaRecords), Box<dyn Error + Send + Sync>> {
    let id = root
        .get("postId")
        .and_then(scalar_id)
        .ok_or("Jschan thread root lacks numeric postId")?;
    if id != requested {
        return Err("Jschan thread identity mismatches requested URL".into());
    }
    let replies = root.get("replies").and_then(Value::as_array);
    if replies.is_some_and(|a| a.len() >= MAX_POSTS) {
        return Err("Jschan thread exceeds supported post bound".into());
    }
    let metadata = thread_meta(board, root, id);
    let mut output = Vec::new();
    for post in std::iter::once(root).chain(replies.into_iter().flatten()) {
        let Some(post_id) = post.get("postId").and_then(scalar_id) else {
            continue;
        };
        let Some(files) = post.get("files").and_then(Value::as_array) else {
            continue;
        };
        if files.len() > MAX_FILE_PER_POST {
            return Err("Jschan post exceeds attachment limit".into());
        }
        for (num, file) in files.iter().enumerate() {
            let filename = file
                .get("filename")
                .and_then(Value::as_str)
                .and_then(file_name)
                .ok_or("Jschan file path is invalid or unsupported")?;
            let original = file
                .get("originalFilename")
                .and_then(Value::as_str)
                .and_then(file_name)
                .ok_or("Jschan original filename is missing or invalid")?;
            if output.len() >= MAX_FILES {
                return Err("Jschan thread exceeds media cap".into());
            }
            let mut endpoint = Url::parse("https://94chan.org/")?;
            endpoint
                .path_segments_mut()
                .map_err(|()| "Invalid Jschan file endpoint")?
                .push("file")
                .push(&filename);
            let item = MediaItem {
                url: endpoint.into(),
                filename: format!(
                    "{post_id}{sep}{original}",
                    sep = if num > 0 {
                        format!("-{num}")
                    } else {
                        String::new()
                    }
                ),
                extractor: "jschan",
                post_id: Some(post_id),
            };
            let mut data = media_metadata(&item);
            for (key, value) in &metadata {
                data.insert(key.clone(), value.clone());
            }
            data.insert("postId".into(), post_id.into());
            data.insert("num".into(), num.into());
            data.insert("count".into(), files.len().into());
            data.insert("siteFilename".into(), filename.into());
            data.insert("originalFilename".into(), original.into());
            output.push((item, data));
        }
    }
    Ok((metadata, output))
}

fn board_queues(
    catalog: &Value,
    board: &str,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let entries = catalog
        .as_array()
        .ok_or("Jschan catalog must be an array")?;
    if entries.len() > MAX_CATALOG {
        return Err("Jschan board catalog exceeds bounded entry count".into());
    }
    let mut seen = HashSet::new();
    let mut events = Vec::new();
    for entry in entries {
        let Some(id) = entry.get("postId").and_then(scalar_id) else {
            continue;
        };
        if !seen.insert(id) {
            continue;
        }
        let mut metadata = Metadata::new();
        metadata.insert("category".into(), "jschan".into());
        metadata.insert("subcategory".into(), "board".into());
        metadata.insert("board".into(), board.into());
        metadata.insert("threadId".into(), id.into());
        events.push(queued_url(
            format!("https://94chan.org/{board}/thread/{id}.html"),
            metadata,
        ));
    }
    Ok(events)
}

async fn bounded_json(
    client: &Client,
    endpoint: Url,
) -> Result<Value, Box<dyn Error + Send + Sync>> {
    let mut response = client
        .get(endpoint.clone())
        .send()
        .await?
        .error_for_status()?;
    if response.url().origin() != endpoint.origin() {
        return Err("Jschan JSON redirected outside its source origin".into());
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_JSON as u64)
    {
        return Err("Jschan JSON exceeds response limit".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_JSON.saturating_sub(bytes.len()) {
            return Err("Jschan JSON exceeds response limit".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(serde_json::from_slice(&bytes)?)
}

async fn messages_at_base(
    client: &Client,
    target: Target,
    base: &Url,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let endpoint = base.join(&api_path(&target))?;
    let body = bounded_json(client, endpoint).await?;
    match target {
        Target::Board { board } => board_queues(&body, &board),
        Target::Thread { board, id } => {
            let (metadata, files) = thread_records(&body, &board, id)?;
            let mut events = Vec::with_capacity(1 + files.len());
            events.push(ExtractorMessage::Directory { metadata });
            for (item, metadata) in files {
                events.push(ExtractorMessage::Url {
                    url: item.url,
                    metadata,
                });
            }
            Ok(events)
        }
    }
}

pub async fn resolve_messages(
    client: &Client,
    target: Target,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    messages_at_base(client, target, &Url::parse("https://94chan.org/")?).await
}

pub async fn resolve(
    client: &Client,
    target: Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let Target::Thread { board, id } = target else {
        return Err("Jschan board catalog requires --dump-messages or --follow-queues".into());
    };
    let base = Url::parse("https://94chan.org/")?;
    let body = bounded_json(client, base.join(&format!("{board}/thread/{id}.json"))?).await?;
    Ok(thread_records(&body, &board, id)?
        .1
        .into_iter()
        .map(|(item, _)| item)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    fn thread_data() -> Value {
        serde_json::json!({
            "postId":12345, "subject":"Pinned public test",
            "files":[{"filename":"upload-1.png","originalFilename":"Cover.png"}],
            "replies":[
                {"postId":12346},
                {"postId":12347,"files":[
                    {"filename":"upload-2.jpg","originalFilename":"Sketch.jpg"},
                    {"filename":"upload-3.webm","originalFilename":"Clip.webm"}
                ]}
            ]
        })
    }

    #[test]
    fn pinned_thread_and_board_paths_are_scoped() {
        assert_eq!(
            match_url("https://94chan.org/a/thread/12345.html"),
            Some(Target::Thread {
                board: "a".into(),
                id: 12345
            })
        );
        assert_eq!(
            match_url("https://94chan.org/a/catalog.html"),
            Some(Target::Board { board: "a".into() })
        );
        assert_eq!(
            match_url("https://94chan.org/a/2.html"),
            Some(Target::Board { board: "a".into() })
        );
        for bad in [
            "https://fake94chan.org/a/thread/12345.html",
            "https://94chan.org/a/thread/0.html",
            "https://94chan.org/a/thread/12345.json",
            "https://94chan.org/a/thread/12345.html?password=1",
            "https://name:pass@94chan.org/a/thread/12345.html",
            "https://94chan.org:8443/a/",
            "https://94chan.org/a/../../private",
            "https://94chan.org/a/thread/123%2fprivate.html",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn thread_media_records_keep_post_identity_and_attachment_order() {
        let (scope, media) = thread_records(&thread_data(), "a", 12345).unwrap();
        assert_eq!(scope["threadId"], 12345);
        assert_eq!(media.len(), 3);
        assert_eq!(media[0].0.url, "https://94chan.org/file/upload-1.png");
        assert_eq!(media[0].0.filename, "12345Cover.png");
        assert_eq!(media[1].0.filename, "12347Sketch.jpg");
        assert_eq!(media[2].0.filename, "12347-1Clip.webm");
        assert_eq!(media[2].1["postId"], 12347);
        assert_eq!(media[2].1["num"], 1);
        assert_eq!(media[1].1["count"], 2);
        assert_eq!(media[0].1["siteFilename"], "upload-1.png");
        assert!(thread_records(&thread_data(), "a", 999).is_err());
        let mut bad = thread_data();
        bad["files"][0]["filename"] = Value::from("../private.png");
        assert!(thread_records(&bad, "a", 12345).is_err());
    }

    #[test]
    fn catalog_queues_are_deferred_unique_canonical_threads() {
        let catalog = serde_json::json!([
            {"postId":12345},{"postId":12345},{"postId":12347},{"postId":"bad"}
        ]);
        let result = board_queues(&catalog, "a").unwrap();
        assert_eq!(result.len(), 2);
        let first = serde_json::to_value(&result[0]).unwrap();
        assert_eq!(first["type"], "queue");
        assert_eq!(first["url"], "https://94chan.org/a/thread/12345.html");
        assert_eq!(first["metadata"]["board"], "a");
    }

    #[tokio::test]
    async fn public_thread_and_board_json_use_bounded_loopback_http() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (path, body) in [
                ("/a/thread/12345.json", thread_data().to_string()),
                (
                    "/a/catalog.json",
                    r#"[{"postId":12345},{"postId":12347}]"#.into(),
                ),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0u8; 4096];
                let n = socket.read(&mut request).await.unwrap();
                assert!(String::from_utf8_lossy(&request[..n])
                    .starts_with(&format!("GET {path} HTTP/1.1")));
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let thread = messages_at_base(
            &Client::new(),
            Target::Thread {
                board: "a".into(),
                id: 12345,
            },
            &base,
        )
        .await
        .unwrap();
        assert_eq!(thread.len(), 4);
        let board = messages_at_base(&Client::new(), Target::Board { board: "a".into() }, &base)
            .await
            .unwrap();
        assert_eq!(board.len(), 2);
        server.await.unwrap();
    }
}
