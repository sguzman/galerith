//! Read-only 4chan / 4channel thread-media preview.
//! Uses documented JSON endpoints; board-wide recursion, directory templates,
//! thumbnail fallback and source metadata are not yet at upstream parity.
use crate::{safe_filename, MediaItem};
use reqwest::Client;
use serde_json::Value;
use std::error::Error;
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

fn files(body: &Value, board: &str) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let posts = body
        .get("posts")
        .and_then(Value::as_array)
        .ok_or("4chan API response missing posts")?;
    let mut media = Vec::new();
    for post in posts {
        let Some(tim) = post.get("tim").and_then(Value::as_u64) else {
            continue;
        };
        let Some(ext) = post.get("ext").and_then(Value::as_str) else {
            continue;
        };
        let Some(original) = post.get("filename").and_then(Value::as_str) else {
            continue;
        };
        let Some(ext) = ext.strip_prefix('.') else {
            continue;
        };
        if ext.is_empty() || ext.len() > 10 || !ext.bytes().all(|x| x.is_ascii_alphanumeric()) {
            continue;
        }
        let name = safe_filename(original);
        let name: String = name.chars().take(100).collect();
        let filename = safe_filename(&format!("{tim} {name}.{ext}"));
        let url = format!("https://i.4cdn.org/{board}/{tim}.{ext}");
        media.push(MediaItem {
            url,
            filename,
            extractor: "4chan",
            post_id: Some(tim),
        });
    }
    Ok(media)
}

pub async fn resolve(
    client: &Client,
    target: Thread,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let api = Url::parse("https://a.4cdn.org/")?;
    resolve_at_base(client, target, &api).await
}

async fn resolve_at_base(
    client: &Client,
    target: Thread,
    api: &Url,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let endpoint = api.join(&format!("{}/thread/{}.json", target.board, target.id))?;
    let body: Value = client
        .get(endpoint)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    files(&body, &target.board)
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
