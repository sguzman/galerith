//! Guarded public 2ch thread media and board-to-thread queues.
//! Independent partial implementation against pinned gallery-dl 2ch.py.
use crate::{
    message::{media_metadata, queued_url, ExtractorMessage, Metadata},
    safe_filename, MediaItem,
};
use reqwest::Client;
use serde_json::Value;
use std::{collections::HashSet, error::Error, time::Duration};
use url::Url;

const MAX_JSON: usize = 2 * 1024 * 1024;
const MAX_FILES: usize = 160;
const MAX_CHILDREN: usize = 128;
const MAX_BOARD_PAGES: u16 = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Thread { root: String, board: String, id: String },
    Board { root: String, board: String },
}

fn valid_board(board: &str) -> bool {
    (1..=24).contains(&board.len())
        && board.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
}

fn valid_id(id: &str) -> bool {
    (1..=18).contains(&id.len())
        && id.bytes().all(|c| c.is_ascii_digit())
        && id.parse::<u64>().ok().is_some_and(|n| n > 0)
}

pub fn match_url(raw: &str) -> Option<Target> {
    let candidate = if raw.starts_with("2ch.") {
        format!("https://{raw}")
    } else {
        raw.to_owned()
    };
    let lower = candidate.to_ascii_lowercase();
    if ["%2f", "%5c", "%2e", "/../", "/./"].iter().any(|x| lower.contains(x)) {
        return None;
    }
    let url = Url::parse(&candidate).ok()?;
    if !matches!(url.scheme(), "https" | "http")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let root = match url.host_str()? {
        "2ch.org" | "2ch.hk" => "https://2ch.org",
        "2ch.su" => "https://2ch.su",
        "2ch.life" => "https://2ch.life",
        _ => return None,
    }.to_owned();
    let paths: Vec<_> = url.path_segments()?.collect();
    match paths.as_slice() {
        [board] | [board, ""] if valid_board(board) =>
            Some(Target::Board { root, board: (*board).to_owned() }),
        [board, "res", thread] | [board, "res", thread, ""]
            if valid_board(board) =>
        {
            let id = thread.strip_suffix(".html").unwrap_or(thread);
            valid_id(id).then(|| Target::Thread {
                root,
                board: (*board).to_owned(),
                id: id.to_owned(),
            })
        }
        _ => None,
    }
}

fn strip_html(input: &str) -> String {
    let mut result = String::new();
    let mut tag = false;
    for ch in input.chars().take(2000) {
        match ch {
            '<' => tag = true,
            '>' => tag = false,
            _ if !tag => result.push(ch),
            _ => {}
        }
    }
    result.replace("&amp;", "&").replace("&quot;", "\"")
        .replace("&lt;", "<").replace("&gt;", ">").replace("&#39;", "'")
        .trim().chars().take(50).collect()
}

fn first_party_path(root: &Url, raw: &str) -> Option<Url> {
    if raw.len() > 1200 || !raw.starts_with('/')
        || raw.contains('%') || raw.contains('\\')
        || raw.split('/').any(|part| part == "." || part == "..")
    {
        return None;
    }
    let url = root.join(raw).ok()?;
    if url.origin() != root.origin()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path_segments()?.any(|s| s == "." || s == "..")
    {
        return None;
    }
    Some(url)
}

fn parse_thread(
    data: &Value,
    board: &str,
    id: &str,
    root: &Url,
) -> Result<(Metadata, Vec<(MediaItem, Metadata)>), Box<dyn Error + Send + Sync>> {
    let posts = data.pointer("/threads/0/posts").and_then(Value::as_array)
        .ok_or("2ch public JSON lacks thread posts")?;
    if posts.is_empty() || posts.len() > 1000 {
        return Err("2ch thread post count unsupported or exceeds bounded limit".into());
    }
    let op = &posts[0];
    let title = op.get("subject").and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .or_else(|| op.get("comment").and_then(Value::as_str))
        .map(strip_html).unwrap_or_default();
    let mut thread = Metadata::new();
    thread.insert("category".into(), "2ch".into());
    thread.insert("subcategory".into(), "thread".into());
    thread.insert("board".into(), board.into());
    thread.insert("thread".into(), id.into());
    thread.insert("title".into(), title.into());
    let mut result = Vec::new();
    for post in posts {
        let Some(files) = post.get("files").and_then(Value::as_array) else { continue };
        let post_name = post.get("name").and_then(Value::as_str).unwrap_or("");
        let timestamp = post.get("timestamp").and_then(Value::as_i64);
        let post_id = post.get("num").and_then(Value::as_u64);
        for file in files {
            if result.len() >= MAX_FILES {
                return Err("2ch public thread exceeds media cap".into());
            }
            let Some(path) = file.get("path").and_then(Value::as_str) else { continue };
            let Some(url) = first_party_path(root, path) else {
                return Err("2ch post file path escapes its first-party origin".into());
            };
            let Some(file_name) = file.get("name").and_then(Value::as_str) else { continue };
            let Some((tim, ext)) = file_name.rsplit_once('.') else { continue };
            let ext = ext.to_ascii_lowercase();
            if !(1..=32).contains(&tim.len())
                || !tim.bytes().all(|b| b.is_ascii_digit())
                || !matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "gif" | "webp" | "mp4" | "webm")
            {
                continue;
            }
            let original = file.get("fullname").and_then(Value::as_str).unwrap_or(file_name);
            if original.len() > 240 || original.contains('/') || original.contains('\\')
                || original.chars().any(char::is_control)
            {
                return Err("2ch original filename contains an unsafe path".into());
            }
            let stem = original.rsplit_once('.').map_or(original, |(stem, _)| stem);
            let safe_stem = safe_filename(stem);
            let filename = if safe_stem.is_empty() {
                format!("{tim}.{ext}")
            } else {
                format!("{tim}_{safe_stem}.{ext}")
            };
            let item = MediaItem {
                url: url.to_string(),
                filename,
                extractor: "2ch",
                post_id,
            };
            let mut meta = media_metadata(&item);
            for (k, v) in &thread {
                meta.insert(k.clone(), v.clone());
            }
            if let Some(fields) = file.as_object() {
                for (k, v) in fields {
                    if !matches!(k.as_str(), "path" | "name" | "fullname"
                        | "filename" | "extractor" | "category" | "subcategory"
                        | "post_id" | "board" | "thread" | "title" | "tim"
                        | "extension" | "source_filename" | "post_name"
                        | "url" | "timestamp") {
                        meta.insert(k.clone(), v.clone());
                    }
                }
            }
            if let Some(fields) = post.as_object() {
                for (k, v) in fields {
                    if !matches!(k.as_str(), "files" | "name" | "comment"
                        | "filename" | "extractor" | "category" | "subcategory"
                        | "post_id" | "board" | "thread" | "title" | "tim"
                        | "extension" | "source_filename" | "post_name"
                        | "url" | "timestamp") {
                        meta.insert(k.clone(), v.clone());
                    }
                }
            }
            meta.insert("post_name".into(), post_name.into());
            meta.insert("tim".into(), tim.into());
            meta.insert("extension".into(), ext.into());
            meta.insert("source_filename".into(), original.into());
            if let Some(timestamp) = timestamp {
                meta.insert("timestamp".into(), timestamp.into());
            }
            result.push((item, meta));
        }
    }
    Ok((thread, result))
}

fn board_queues(
    root: &Value, board: &str, base: &Url, seen: &mut HashSet<String>,
    output: &mut Vec<ExtractorMessage>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let threads = root.get("threads").and_then(Value::as_array)
        .ok_or("2ch public board JSON lacks thread list")?;
    for thread in threads {
        let id = thread.get("thread_num")
            .and_then(|v| v.as_str().map(ToOwned::to_owned)
                .or_else(|| v.as_u64().map(|n| n.to_string())));
        let Some(id) = id else { continue };
        if !valid_id(&id) || !seen.insert(id.clone()) {
            continue;
        }
        if output.len() >= MAX_CHILDREN {
            return Err("2ch board exceeds bounded child queue cap".into());
        }
        let mut meta = Metadata::new();
        meta.insert("category".into(), "2ch".into());
        meta.insert("subcategory".into(), "board".into());
        meta.insert("board".into(), board.into());
        meta.insert("thread".into(), id.clone().into());
        output.push(queued_url(format!("{base}{board}/res/{id}.html"), meta));
    }
    Ok(())
}

async fn json_at(client: &Client, endpoint: Url) -> Result<Value, Box<dyn Error + Send + Sync>> {
    let mut response = client.get(endpoint.clone()).send().await?.error_for_status()?;
    if response.url().origin() != endpoint.origin() {
        return Err("2ch public API redirected outside trusted origin".into());
    }
    if response.content_length().is_some_and(|n| n > MAX_JSON as u64) {
        return Err("2ch source JSON exceeds 2 MiB".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_JSON.saturating_sub(bytes.len()) {
            return Err("2ch source JSON exceeds 2 MiB".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(serde_json::from_slice(&bytes)?)
}

async fn thread_at(
    client: &Client, base: &Url, board: &str, id: &str,
) -> Result<(Metadata, Vec<(MediaItem, Metadata)>), Box<dyn Error + Send + Sync>> {
    let data = json_at(client, base.join(&format!("{board}/res/{id}.json"))?).await?;
    parse_thread(&data, board, id, base)
}

async fn board_at(
    client: &Client, base: &Url, board: &str, max_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let index = json_at(client, base.join(&format!("{board}/index.json"))?).await?;
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    board_queues(&index, board, base, &mut seen, &mut result)?;
    let pages = index.get("pages").and_then(Value::as_array)
        .ok_or("2ch public board lacks available page indexes")?;
    let mut seen_pages = HashSet::new();
    for number in pages {
        let page = number.as_u64().or_else(|| number.as_str()?.parse().ok());
        let Some(page) = page else { continue };
        if page == 0 || page > MAX_BOARD_PAGES as u64 || !seen_pages.insert(page) {
            continue;
        }
        if seen_pages.len() >= max_pages.clamp(1, MAX_BOARD_PAGES) as usize {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
        let value = json_at(client, base.join(&format!("{board}/{page}.json"))?).await?;
        board_queues(&value, board, base, &mut seen, &mut result)?;
    }
    Ok(result)
}

pub async fn resolve(
    client: &Client, target: Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    match target {
        Target::Thread { root, board, id } => {
            let (_, items) = thread_at(client, &Url::parse(&format!("{root}/"))?, &board, &id).await?;
            Ok(items.into_iter().map(|(item, _)| item).collect())
        }
        Target::Board { .. } => Err("2ch public boards require --dump-messages or --follow-queues".into()),
    }
}

pub async fn resolve_messages(
    client: &Client, target: Target, max_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    match target {
        Target::Thread { root, board, id } => {
            let (dir, items) = thread_at(client, &Url::parse(&format!("{root}/"))?, &board, &id).await?;
            let mut events = vec![ExtractorMessage::Directory { metadata: dir }];
            for (item, meta) in items {
                events.push(ExtractorMessage::Url { url: item.url, metadata: meta });
            }
            Ok(events)
        }
        Target::Board { root, board } =>
            board_at(client, &Url::parse(&format!("{root}/"))?, &board, max_pages).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::TcpListener};

    #[test]
    fn pinned_domain_mirrors_and_thread_board_paths_are_exact() {
        assert_eq!(match_url("2ch.hk/a/res/12345.html"),
            Some(Target::Thread { root: "https://2ch.org".into(), board: "a".into(), id: "12345".into() }));
        assert_eq!(match_url("https://2ch.su/g/"),
            Some(Target::Board { root: "https://2ch.su".into(), board: "g".into() }));
        for bad in [
            "https://evil2ch.org/a/res/12345",
            "https://2ch.org/a/res/0.html",
            "https://2ch.org/a/res/12345.html?admin=1",
            "https://2ch.org/a/../../private",
            "https://2ch.org:8443/a/res/12345",
            "https://user:pw@2ch.org/a/res/12345",
            "https://2ch.org/a/res/123%2f45.html",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    fn thread_fixture() -> Value {
        serde_json::json!({"threads":[{"posts":[
            {"num":12345,"subject":"Review &amp; discussion","name":"OP",
             "timestamp":1760000000,
             "files":[{"name":"157000001.jpg","fullname":"original art.jpg","path":"/a/src/157000001.jpg","size":451}]},
            {"num":12346,"name":"Reply","timestamp":1760000100,
             "files":[{"name":"157000002.png","fullname":"sketch.png","path":"/a/src/157000002.png"}]},
            {"num":12347,"name":"Empty","files":[]}
        ]}]})
    }

    #[test]
    fn thread_json_preserves_order_source_title_and_file_post_fields() {
        let base = Url::parse("https://2ch.org/").unwrap();
        let (dir, records) = parse_thread(&thread_fixture(), "a", "12345", &base).unwrap();
        assert_eq!(dir["title"], "Review & discussion");
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].0.url, "https://2ch.org/a/src/157000001.jpg");
        assert_eq!(records[0].0.filename, "157000001_original art.jpg");
        assert_eq!(records[0].1["timestamp"], 1760000000);
        assert_eq!(records[0].1["post_name"], "OP");
        assert_eq!(records[0].1["size"], 451);
        assert_eq!(records[1].0.post_id, Some(12346));
        let mut evil = thread_fixture();
        evil["threads"][0]["posts"][0]["files"][0]["path"] =
            "https://evil2ch.org/secret.jpg".into();
        assert!(parse_thread(&evil, "a", "12345", &base).is_err());
        evil["threads"][0]["posts"][0]["files"][0]["path"] =
            "/a/src/../../private.jpg".into();
        assert!(parse_thread(&evil, "a", "12345", &base).is_err());
        let mut forged = thread_fixture();
        forged["threads"][0]["posts"][0]["filename"] = "../../secret.jpg".into();
        forged["threads"][0]["posts"][0]["files"][0]["extractor"] = "directlink".into();
        let (_, records) = parse_thread(&forged, "a", "12345", &base).unwrap();
        assert_eq!(records[0].1["extractor"], "2ch");
        assert_eq!(records[0].1["filename"], "157000001_original art.jpg");
    }

    #[test]
    fn board_queues_deduplicate_first_seen_numeric_thread_ids() {
        let base = Url::parse("https://2ch.org/").unwrap();
        let mut seen = HashSet::new();
        let mut output = Vec::new();
        let data = serde_json::json!({"threads":[
            {"thread_num":"12345"},{"thread_num":"12345"},
            {"thread_num":12346},{"thread_num":"0"},{"thread_num":"bad"}
        ]});
        board_queues(&data, "a", &base, &mut seen, &mut output).unwrap();
        assert_eq!(output.len(), 2);
        assert_eq!(serde_json::to_value(&output[0]).unwrap()["url"],
            "https://2ch.org/a/res/12345.html");
        assert_eq!(serde_json::to_value(&output[1]).unwrap()["metadata"]["thread"], "12346");
    }

    #[tokio::test]
    async fn thread_api_reads_single_bounded_json_request() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 4096];
            let n = socket.read(&mut request).await.unwrap();
            assert!(String::from_utf8_lossy(&request[..n])
                .starts_with("GET /a/res/12345.json HTTP/1.1"));
            let body = thread_fixture().to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let (_, files) = thread_at(&Client::new(), &base, "a", "12345").await.unwrap();
        assert_eq!(files.len(), 2);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn board_index_and_page_follow_ordered_deferred_children() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (path, body) in [
                ("/a/index.json", r#"{"threads":[{"thread_num":12345}],"pages":[0,1]}"#),
                ("/a/1.json", r#"{"threads":[{"thread_num":12345},{"thread_num":12346}]}"#),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0u8; 4096];
                let n = socket.read(&mut request).await.unwrap();
                assert!(String::from_utf8_lossy(&request[..n])
                    .starts_with(&format!("GET {path} HTTP/1.1")));
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let queues = board_at(&Client::new(), &base, "a", 3).await.unwrap();
        assert_eq!(queues.len(), 2);
        assert_eq!(serde_json::to_value(&queues[1]).unwrap()["url"],
            format!("{base}a/res/12346.html"));
        server.await.unwrap();
    }
}
