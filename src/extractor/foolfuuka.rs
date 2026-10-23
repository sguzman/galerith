//! FoolFuuka read-only archived-thread JSON extractor preview.
//! Archive-specific redirects, board enumeration, search, galleries,
//! and original metadata/directory semantics are not yet supported.
use crate::{safe_filename, MediaItem};
use reqwest::Client;
use serde_json::Value;
use std::error::Error;
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    host: &'static str,
    category: &'static str,
    board: String,
    thread: u64,
}

fn archive(host: &str) -> Option<(&'static str, &'static str)> {
    match host {
        "archive.4plebs.org" | "4plebs.org" => Some(("archive.4plebs.org", "4plebs")),
        "archived.moe" => Some(("archived.moe", "archivedmoe")),
        "archiveofsins.com" | "www.archiveofsins.com" => {
            Some(("archiveofsins.com", "archiveofsins"))
        }
        "arch.b4k.dev" | "arch.b4k.co" => Some(("arch.b4k.dev", "b4k")),
        "desuarchive.org" => Some(("desuarchive.org", "desuarchive")),
        "boards.fireden.net" => Some(("boards.fireden.net", "fireden")),
        "archive.palanq.win" => Some(("archive.palanq.win", "palanq")),
        "rbt.asia" | "rebeccablacktech.com" | "archive.rebeccablacktech.com" => {
            Some(("rbt.asia", "rbt"))
        }
        "thebarchive.com" => Some(("thebarchive.com", "thebarchive")),
        _ => None,
    }
}

pub fn match_url(input: &str) -> Option<Target> {
    let url = Url::parse(input).ok()?;
    if url.scheme() != "https" {
        return None;
    }
    let (host, category) = archive(url.host_str()?)?;
    let mut parts = url.path_segments()?.filter(|x| !x.is_empty());
    let board = parts.next()?;
    if board.is_empty()
        || board.len() > 8
        || !board.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    {
        return None;
    }
    if parts.next()? != "thread" {
        return None;
    }
    let thread = parts.next()?.parse::<u64>().ok()?;
    if thread == 0 || parts.next().is_some() {
        return None;
    }
    Some(Target { host, category, board: board.into(), thread })
}

fn file_for_post(
    post: &Value,
    category: &'static str,
    base: &Url,
) -> Option<MediaItem> {
    let media = post.get("media")?.as_object()?;
    let raw = media.get("media_link").and_then(Value::as_str)
        .filter(|url| !url.is_empty())
        .or_else(|| media.get("remote_media_link").and_then(Value::as_str))?;
    let url = Url::parse(raw).or_else(|_| base.join(raw)).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let original = media.get("media")?.as_str()?;
    let ext = original.rsplit_once('.')?.1;
    if ext.is_empty() || ext.len() > 12 || !ext.bytes().all(|x| x.is_ascii_alphanumeric()) {
        return None;
    }
    let from_original = media.get("media_orig").and_then(Value::as_str)
        .and_then(|x| x.split_once('.').map(|(id, _)| id))
        .filter(|x| x.len() <= 32 && !x.is_empty() && x.bytes().all(|b| b.is_ascii_digit()));
    let post_id = post.get("num").and_then(Value::as_u64);
    let timestamp = from_original.map(str::to_owned)
        .or_else(|| post_id.map(|id| id.to_string()))?;
    let label = media.get("media_filename").and_then(Value::as_str)
        .and_then(|v| v.rsplit_once('.').map(|(stem, _)| stem))
        .unwrap_or("untitled");
    let stem: String = safe_filename(label).chars().take(100).collect();
    Some(MediaItem {
        url: url.to_string(),
        filename: safe_filename(&format!("{timestamp} {stem}.{ext}")),
        extractor: category,
        post_id,
    })
}

fn parse_thread(body: &Value, target: &Target, base: &Url)
    -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>>
{
    let data = body.get(target.thread.to_string()).ok_or("FoolFuuka API thread not found")?;
    let op = data.get("op").ok_or("FoolFuuka API missing original post")?;
    let mut output = Vec::new();
    if let Some(item) = file_for_post(op, target.category, base) {
        output.push(item);
    }
    if let Some(posts) = data.get("posts").and_then(Value::as_object) {
        let mut posts: Vec<_> = posts.values().collect();
        posts.sort_by_key(|post| post.get("timestamp").and_then(Value::as_u64).unwrap_or(0));
        for post in posts {
            if let Some(item) = file_for_post(post, target.category, base) {
                output.push(item);
            }
        }
    }
    Ok(output)
}

pub async fn resolve(client: &Client, target: Target)
    -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>>
{
    let base = Url::parse(&format!("https://{}/", target.host))?;
    resolve_at_base(client, target, &base).await
}

async fn resolve_at_base(client: &Client, target: Target, base: &Url)
    -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>>
{
    let mut api = base.join("_/api/chan/thread/")?;
    api.query_pairs_mut()
        .append_pair("board", &target.board)
        .append_pair("num", &target.thread.to_string());
    let response: Value = client.get(api).send().await?.error_for_status()?.json().await?;
    parse_thread(&response, &target, base)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn identifies_pinned_archive_hosts_and_thread_urls() {
        assert_eq!(
            match_url("https://archive.4plebs.org/tg/thread/123"),
            Some(Target {
                host: "archive.4plebs.org", category: "4plebs",
                board: "tg".into(), thread: 123
            })
        );
        assert_eq!(
            match_url("https://arch.b4k.co/g/thread/456"),
            Some(Target {
                host: "arch.b4k.dev", category: "b4k",
                board: "g".into(), thread: 456
            })
        );
        assert!(match_url("https://desuarchive.org/g/search/text/test").is_none());
        assert!(match_url("https://desuarchive.org/g/thread/0").is_none());
        assert!(match_url("http://archive.4plebs.org/tg/thread/1").is_none());
        assert!(match_url("https://notarchive.4plebs.org/tg/thread/1").is_none());
    }

    #[test]
    fn orders_replies_and_rejects_unsafe_media() {
        let base = Url::parse("https://archived.moe/").unwrap();
        let target = Target {
            host: "archived.moe", category: "archivedmoe",
            board: "a".into(), thread: 123
        };
        let body = json!({"123": {
            "op": {"num":123, "media": {
                "media_link":"/a/image/123.jpg", "media":"123.jpg",
                "media_filename":"chapter.jpg", "media_orig":"1000.jpg"
            }},
            "posts": {
                "501":{"num":501,"timestamp":20,"media":{
                    "media_link":"javascript:unsafe", "media":"501.png",
                    "media_filename":"bad.png", "media_orig":"1002.png"
                }},
                "500":{"num":500,"timestamp":10,"media":{
                    "media_link":"https://cdn.example.org/500.png", "media":"500.png",
                    "media_filename":"panel.png", "media_orig":"1001.png"
                }}
            }
        }});
        let files = parse_thread(&body, &target, &base).unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].filename, "1000 chapter.jpg");
        assert_eq!(files[1].filename, "1001 panel.png");
        assert_eq!(files[0].url, "https://archived.moe/a/image/123.jpg");
        assert_eq!(files[1].post_id, Some(500));
    }

    #[tokio::test]
    async fn local_thread_endpoint_extracts_synthetic_attachment() {
        use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::TcpListener};
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut data = [0_u8; 4096];
            let length = socket.read(&mut data).await.unwrap();
            let request = String::from_utf8_lossy(&data[..length]);
            assert!(request.starts_with("GET /_/api/chan/thread/?"), "{request}");
            assert!(request.contains("board=a"), "{request}");
            assert!(request.contains("num=123"), "{request}");
            let body = r#"{"123":{"op":{"num":123,"media":{"media_link":"/images/1.jpg","media":"1.jpg","media_filename":"fixture.jpg","media_orig":"99.jpg"}}}}"#;
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(reply.as_bytes()).await.unwrap();
        });
        let api = Url::parse(&format!("http://{address}/")).unwrap();
        let target = Target {
            host: "archived.moe", category: "archivedmoe",
            board: "a".into(), thread: 123
        };
        let output = resolve_at_base(&Client::new(), target, &api).await.unwrap();
        assert_eq!(output.len(), 1);
        assert_eq!(output[0].filename, "99 fixture.jpg");
        server.await.unwrap();
    }
}
