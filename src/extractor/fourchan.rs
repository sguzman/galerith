//! Read-only 4chan / 4channel thread-media preview.
//! Uses documented JSON endpoints; full directory templates, thumbnail fallback
//! and source metadata are not yet at upstream parity.
use crate::{
    message::{media_metadata, queued_url, ExtractorMessage, Metadata},
    safe_filename, MediaItem,
};
use reqwest::Client;
use serde_json::Value;
use std::{collections::HashSet, error::Error, time::Duration};
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thread {
    pub board: String,
    pub id: u64,
}

pub fn match_url(input: &str) -> Option<Thread> {
    let url = Url::parse(input).ok()?;
    if url.scheme() != "https"
        || !matches!(
            url.host_str(),
            Some("boards.4chan.org" | "boards.4channel.org")
        )
    {
        return None;
    }
    let mut parts = url.path_segments()?.filter(|part| !part.is_empty());
    let board = parts.next()?;
    if board.is_empty()
        || board.len() > 8
        || !board
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    {
        return None;
    }
    if parts.next()? != "thread" {
        return None;
    }
    let id = parts.next()?.parse::<u64>().ok()?;
    if id == 0 {
        return None;
    }
    // Allow an optional human-readable slug after the ID.
    if parts.next().is_some_and(|segment| segment.len() > 200) || parts.next().is_some() {
        return None;
    }
    Some(Thread {
        board: board.into(),
        id,
    })
}

/// Recognize a board listing without treating arbitrary paths as boards.
pub fn match_board_url(input: &str) -> Option<String> {
    let url = Url::parse(input).ok()?;
    if url.scheme() != "https"
        || !matches!(
            url.host_str(),
            Some("boards.4chan.org" | "boards.4channel.org")
        )
    {
        return None;
    }
    let mut parts = url.path_segments()?.filter(|part| !part.is_empty());
    let board = parts.next()?;
    if board.is_empty()
        || board.len() > 8
        || !board
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    {
        return None;
    }
    if parts.next().is_some_and(|page| page.parse::<u8>().is_err()) || parts.next().is_some() {
        return None;
    }
    Some(board.to_owned())
}

fn board_thread_ids(
    data: &Value,
    max_pages: u16,
) -> Result<Vec<u64>, Box<dyn Error + Send + Sync>> {
    let pages = data
        .as_array()
        .ok_or("4chan board API must contain pages")?;
    let mut ids = Vec::new();
    let mut seen = HashSet::new();
    for page in pages.iter().take(max_pages.max(1) as usize) {
        let threads = page
            .get("threads")
            .and_then(Value::as_array)
            .ok_or("4chan board API page missing threads")?;
        for thread in threads {
            if let Some(id) = thread.get("no").and_then(Value::as_u64) {
                if id != 0 && seen.insert(id) {
                    ids.push(id);
                }
            }
        }
    }
    Ok(ids)
}

const MAX_BOARD_QUEUES: usize = 256;
const MAX_BOARD_INDEX_BYTES: usize = 2 * 1024 * 1024;

/// Generate deferred source threads from a read-only public board listing.
/// Emitting a Queue does not fetch a thread or media file.
fn board_queue_messages(
    body: &Value,
    board: &str,
    max_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    Ok(board_thread_ids(body, max_pages)?
        .into_iter()
        .take(MAX_BOARD_QUEUES)
        .map(|id| {
            let mut metadata = Metadata::new();
            metadata.insert("category".into(), "4chan".into());
            metadata.insert("board".into(), board.into());
            metadata.insert("thread_id".into(), id.into());
            queued_url(
                format!("https://boards.4chan.org/{board}/thread/{id}"),
                metadata,
            )
        })
        .collect())
}

async fn queue_board_at_base(
    client: &Client,
    board: &str,
    max_pages: u16,
    api: &Url,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let endpoint = api.join(&format!("{board}/threads.json"))?;
    let mut response = client.get(endpoint).send().await?.error_for_status()?;
    if response
        .content_length()
        .is_some_and(|size| size > MAX_BOARD_INDEX_BYTES as u64)
    {
        return Err("4chan board JSON exceeded the response limit".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len().saturating_add(chunk.len()) > MAX_BOARD_INDEX_BYTES {
            return Err("4chan board JSON exceeded the response limit".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    board_queue_messages(&serde_json::from_slice(&bytes)?, board, max_pages)
}

/// Native read-only board index queue preview. Ordinary downloads keep their
/// legacy media traversal unless the explicit queue-job mode is selected.
pub async fn resolve_board_messages(
    client: &Client,
    board: &str,
    max_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    queue_board_at_base(
        client,
        board,
        max_pages,
        &Url::parse("https://a.4cdn.org/")?,
    )
    .await
}

pub async fn resolve_board(
    client: &Client,
    board: &str,
    max_pages: u16,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let api = Url::parse("https://a.4cdn.org/")?;
    resolve_board_at_base(client, board, max_pages, &api).await
}

async fn resolve_board_at_base(
    client: &Client,
    board: &str,
    max_pages: u16,
    api: &Url,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let endpoint = api.join(&format!("{board}/threads.json"))?;
    let pages: Value = client
        .get(endpoint)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let threads = board_thread_ids(&pages, max_pages)?;
    let mut items = Vec::new();
    for (index, id) in threads.into_iter().enumerate() {
        if index > 0 {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        items.extend(
            resolve_at_base(
                client,
                Thread {
                    board: board.to_owned(),
                    id,
                },
                api,
            )
            .await?,
        );
    }
    Ok(items)
}

fn media_from_post(post: &Value, board: &str) -> Option<MediaItem> {
    let tim = post.get("tim")?.as_u64()?;
    let ext = post.get("ext")?.as_str()?.strip_prefix('.')?;
    let original = post.get("filename")?.as_str()?;
    if ext.is_empty() || ext.len() > 10 || !ext.bytes().all(|x| x.is_ascii_alphanumeric()) {
        return None;
    }
    let name = safe_filename(original);
    let name: String = name.chars().take(100).collect();
    let filename = safe_filename(&format!("{tim} {name}.{ext}"));
    let url = format!("https://i.4cdn.org/{board}/{tim}.{ext}");
    Some(MediaItem {
        url,
        filename,
        extractor: "4chan",
        post_id: Some(tim),
    })
}

fn files(body: &Value, board: &str) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let posts = body
        .get("posts")
        .and_then(Value::as_array)
        .ok_or("4chan API response missing posts")?;
    Ok(posts
        .iter()
        .filter_map(|post| media_from_post(post, board))
        .collect())
}

/// Preserve real API post boundaries, including text-only posts, without
/// changing the legacy MediaItem download/archive identifier (the media tim).
/// source_post_id is the API post number, distinct from post_id in the
/// current MediaItem projection, which is the file's CDN timestamp.
fn thread_messages(
    body: &Value,
    board: &str,
    thread_id: u64,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let posts = body
        .get("posts")
        .and_then(Value::as_array)
        .ok_or("4chan API response missing posts")?;
    let mut output = Vec::new();
    for post in posts {
        let media = media_from_post(post, board);
        let source_post = post.get("no").and_then(Value::as_u64).filter(|id| *id > 0);
        if source_post.is_none() && media.is_none() {
            continue;
        }
        let mut metadata = Metadata::new();
        metadata.insert("category".into(), "4chan".into());
        metadata.insert("board".into(), board.into());
        metadata.insert("thread_id".into(), thread_id.into());
        if let Some(post_id) = source_post {
            metadata.insert("source_post_id".into(), post_id.into());
        }
        output.push(ExtractorMessage::Directory {
            metadata: metadata.clone(),
        });
        if let Some(item) = media {
            let mut fields = media_metadata(&item);
            for (key, value) in metadata {
                fields.entry(key).or_insert(value);
            }
            output.push(ExtractorMessage::Url {
                url: item.url,
                metadata: fields,
            });
        }
    }
    Ok(output)
}

pub async fn resolve(
    client: &Client,
    target: Thread,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let api = Url::parse("https://a.4cdn.org/")?;
    resolve_at_base(client, target, &api).await
}

async fn thread_json(
    client: &Client,
    target: &Thread,
    api: &Url,
) -> Result<Value, Box<dyn Error + Send + Sync>> {
    let endpoint = api.join(&format!("{}/thread/{}.json", target.board, target.id))?;
    Ok(client
        .get(endpoint)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

async fn resolve_at_base(
    client: &Client,
    target: Thread,
    api: &Url,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    files(&thread_json(client, &target, api).await?, &target.board)
}

async fn resolve_messages_at_base(
    client: &Client,
    target: Thread,
    api: &Url,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    thread_messages(
        &thread_json(client, &target, api).await?,
        &target.board,
        target.id,
    )
}

/// Public typed 4chan thread preview with API-backed post grouping.
pub async fn resolve_messages(
    client: &Client,
    target: Thread,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    resolve_messages_at_base(client, target, &Url::parse("https://a.4cdn.org/")?).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn accepts_only_known_thread_urls_and_valid_board_names() {
        assert_eq!(
            match_url("https://boards.4channel.org/g/thread/123/title"),
            Some(Thread {
                board: "g".into(),
                id: 123
            })
        );
        assert_eq!(
            match_url("https://boards.4chan.org/a/thread/456/"),
            Some(Thread {
                board: "a".into(),
                id: 456
            })
        );
        assert_eq!(match_url("https://boards.4chan.org/a/"), None);
        assert_eq!(match_url("https://boards.4chan.org/../../thread/123"), None);
        assert_eq!(match_url("https://boards.4chan.org/a/thread/0"), None);
        assert_eq!(match_url("https://evil.example/a/thread/123"), None);
    }

    #[test]
    fn board_listing_matches_only_bounded_board_paths() {
        assert_eq!(
            match_board_url("https://boards.4chan.org/g/"),
            Some("g".into())
        );
        assert_eq!(
            match_board_url("https://boards.4channel.org/a/4"),
            Some("a".into())
        );
        assert_eq!(match_board_url("https://boards.4chan.org/a/thread/1"), None);
        assert_eq!(
            match_board_url("https://boards.4chan.org/a/not-a-page"),
            None
        );
        assert_eq!(match_board_url("https://evil.example/g/"), None);
    }

    #[test]
    fn board_queue_preview_preserves_source_thread_ids_without_fetching_threads() {
        let data = json!([
            {"page":1,"threads":[{"no":123},{"no":456},{"no":123},{"no":0}]},
            {"page":2,"threads":[{"no":456},{"no":789}]}
        ]);
        let first = board_queue_messages(&data, "g", 1).unwrap();
        assert_eq!(first.len(), 2);
        let all = board_queue_messages(&data, "g", 2).unwrap();
        assert_eq!(all.len(), 3);
        let value = serde_json::to_value(&all[2]).unwrap();
        assert_eq!(value["type"], "queue");
        assert_eq!(value["url"], "https://boards.4chan.org/g/thread/789");
        assert_eq!(value["metadata"]["board"], "g");
        assert_eq!(value["metadata"]["thread_id"], 789);
        assert!(board_queue_messages(&json!({"posts":[]}), "g", 1).is_err());
        let oversized = json!([{
            "page":1,
            "threads":(1..=400).map(|id| json!({"no":id})).collect::<Vec<_>>()
        }]);
        assert_eq!(
            board_queue_messages(&oversized, "g", 1).unwrap().len(),
            MAX_BOARD_QUEUES
        );
    }

    #[tokio::test]
    async fn filtered_board_queues_feed_true_thread_scopes_and_media_projection() {
        use crate::{
            extractor::media_from_messages,
            message::{traverse_queue_job_filtered_with, JobSelection},
            range::IndexRange,
        };

        let queues = board_queue_messages(
            &json!([{"page":1,"threads":[{"no":123},{"no":456}]}]),
            "g",
            1,
        )
        .unwrap();
        let selected = JobSelection {
            files: None,
            children: Some(IndexRange::parse("2").unwrap()),
            directories: None,
        };
        let events = traverse_queue_job_filtered_with(queues, 1, 8, &selected, |url| async move {
            assert_eq!(url, "https://boards.4chan.org/g/thread/456");
            thread_messages(
                &json!({"posts":[
                    {"no":456,"tim":90001,"filename":"picture","ext":".jpg"},
                    {"no":457,"com":"text only"}
                ]}),
                "g",
                456,
            )
        })
        .await
        .unwrap();
        assert_eq!(events.len(), 5);
        assert_eq!(
            serde_json::to_value(&events[0]).unwrap()["type"],
            "queue"
        );
        assert_eq!(
            serde_json::to_value(&events[2]).unwrap()["metadata"]["source_post_id"],
            456
        );
        let files = media_from_messages(events).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].filename, "90001 picture.jpg");
        assert_eq!(files[0].post_id, Some(90001));
    }

    #[tokio::test]
    async fn board_queue_fetches_only_the_bounded_index_not_child_threads() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0_u8; 4096];
            let count = socket.read(&mut buf).await.unwrap();
            assert!(
                String::from_utf8_lossy(&buf[..count]).starts_with("GET /g/threads.json HTTP/1.1")
            );
            let body = r#"[{"page":1,"threads":[{"no":123},{"no":456}]}]"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
            // The listening socket closes; a child GET would fail instead
            // of silently succeeding against this synthetic fixture.
        });
        let api = Url::parse(&format!("http://{address}/")).unwrap();
        let events = queue_board_at_base(&Client::new(), "g", 1, &api)
            .await
            .unwrap();
        assert_eq!(events.len(), 2);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn board_queue_rejects_oversize_response_before_reading_body() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0_u8; 2048];
            assert!(socket.read(&mut buf).await.unwrap() > 0);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                MAX_BOARD_INDEX_BYTES + 1
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        let api = Url::parse(&format!("http://{address}/")).unwrap();
        let error = queue_board_at_base(&Client::new(), "g", 1, &api)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("response limit"));
        server.await.unwrap();
    }

    #[test]
    fn board_page_selection_and_thread_deduplication() {
        let data = json!([
            {"page": 1, "threads": [{"no": 123}, {"no": 456}]},
            {"page": 2, "threads": [{"no": 456}, {"no": 789}]}
        ]);
        assert_eq!(board_thread_ids(&data, 1).unwrap(), vec![123, 456]);
        assert_eq!(board_thread_ids(&data, 2).unwrap(), vec![123, 456, 789]);
        assert!(board_thread_ids(&json!({"bad": true}), 1).is_err());
    }

    #[tokio::test]
    async fn board_listing_recurses_into_thread_using_synthetic_api() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (url, response) in [
                ("/a/threads.json", r#"[{"page":1,"threads":[{"no":123}]}]"#),
                (
                    "/a/thread/123.json",
                    r#"{"posts":[{"no":123,"tim":4455,"filename":"art","ext":".png"}]}"#,
                ),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buf = [0_u8; 4096];
                let length = socket.read(&mut buf).await.unwrap();
                let request = String::from_utf8_lossy(&buf[..length]);
                assert!(
                    request.starts_with(&format!("GET {url} HTTP/1.1")),
                    "{request}"
                );
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{response}",
                    response.len()
                );
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let items = resolve_board_at_base(&Client::new(), "a", 1, &base)
            .await
            .unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].filename, "4455 art.png");
        server.await.unwrap();
    }

    #[test]
    fn typed_thread_events_preserve_actual_api_post_scopes_including_text() {
        let data = json!({"posts": [
            {"no": 120, "tim": 90001, "filename":"first", "ext":".png"},
            {"no": 121, "com":"text-only, no attachment"},
            {"no": 122, "tim": 90002, "filename":"second", "ext":".webm"},
            {"no": 123, "tim": 90003, "filename":"bad", "ext":"../jpg"}
        ]});
        let events = thread_messages(&data, "g", 120).unwrap();
        assert_eq!(events.len(), 6);
        let values: Vec<Value> = events
            .iter()
            .map(|event| serde_json::to_value(event).unwrap())
            .collect();
        assert_eq!(values[0]["type"], "directory");
        assert_eq!(values[0]["metadata"]["source_post_id"], 120);
        assert_eq!(values[0]["metadata"]["thread_id"], 120);
        assert_eq!(values[0]["metadata"]["board"], "g");
        assert_eq!(values[1]["type"], "url");
        assert_eq!(values[1]["metadata"]["source_post_id"], 120);
        assert_eq!(values[1]["metadata"]["post_id"], 90001);
        assert_eq!(values[2]["type"], "directory");
        assert_eq!(values[2]["metadata"]["source_post_id"], 121);
        assert_eq!(values[3]["type"], "directory");
        assert_eq!(values[3]["metadata"]["source_post_id"], 122);
        assert_eq!(values[4]["metadata"]["filename"], "90002 second.webm");
        assert_eq!(values[5]["metadata"]["source_post_id"], 123);
        let legacy = files(&data, "g").unwrap();
        assert_eq!(legacy.len(), 2);
        assert_eq!(legacy[0].post_id, Some(90001));
        assert_eq!(legacy[1].post_id, Some(90002));
        assert!(thread_messages(&json!({"posts": {}}), "g", 120).is_err());
    }

    #[tokio::test]
    async fn typed_thread_api_uses_single_read_for_real_post_boundary_events() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0_u8; 4096];
            let count = socket.read(&mut buf).await.unwrap();
            assert!(String::from_utf8_lossy(&buf[..count])
                .starts_with("GET /g/thread/120.json HTTP/1.1"));
            let body = json!({"posts":[
                {"no":120,"tim":90001,"filename":"first","ext":".png"},
                {"no":121,"com":"text only"}
            ]})
            .to_string();
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(reply.as_bytes()).await.unwrap();
        });
        let api = Url::parse(&format!("http://{address}/")).unwrap();
        let events = resolve_messages_at_base(
            &Client::new(),
            Thread {
                board: "g".into(),
                id: 120,
            },
            &api,
        )
        .await
        .unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(
            serde_json::to_value(&events[2]).unwrap()["metadata"]["source_post_id"],
            121
        );
        server.await.unwrap();
    }

    #[test]
    fn skips_text_posts_and_unsafe_extensions() {
        let body = json!({"posts": [
            {"no": 100, "tim": 98765, "filename": "hello/world", "ext": ".png"},
            {"no": 101, "com": "text-only"},
            {"no": 102, "tim": 98766, "filename": "bad", "ext": "../jpg"},
            {"no": 103, "tim": 98767, "filename": "movie", "ext": ".webm"}
        ]});
        let output = files(&body, "g").unwrap();
        assert_eq!(output.len(), 2);
        assert_eq!(output[0].filename, "98765 hello_world.png");
        assert_eq!(output[0].url, "https://i.4cdn.org/g/98765.png");
        assert_eq!(output[1].filename, "98767 movie.webm");
        assert!(files(&json!({"posts":{}}), "g").is_err());
    }

    #[tokio::test]
    async fn synthetic_thread_api_contract_is_correct() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0_u8; 2048];
            let count = socket.read(&mut buf).await.unwrap();
            let request = String::from_utf8_lossy(&buf[..count]);
            assert!(
                request.starts_with("GET /g/thread/123.json HTTP/1.1"),
                "{request}"
            );
            let data = r#"{"posts":[{"no":123,"tim":555,"filename":"fixture","ext":".jpg"}]}"#;
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{data}",
                data.len()
            );
            socket.write_all(reply.as_bytes()).await.unwrap();
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let target = Thread {
            board: "g".into(),
            id: 123,
        };
        let output = resolve_at_base(&Client::new(), target, &base)
            .await
            .unwrap();
        assert_eq!(output.len(), 1);
        assert_eq!(output[0].filename, "555 fixture.jpg");
        server.await.unwrap();
    }
}
