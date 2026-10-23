//! FoolFuuka read-only archived-thread JSON extractor preview.
//! Archive-specific redirects, search, galleries, and original metadata /
//! directory semantics are not yet supported.
use crate::{safe_filename, MediaItem};
use reqwest::Client;
use serde_json::Value;
use std::{
    collections::{BTreeSet, HashSet},
    error::Error,
    time::Duration,
};
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
        || !board
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
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
    Some(Target {
        host,
        category,
        board: board.into(),
        thread,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Board {
    host: &'static str,
    category: &'static str,
    board: String,
    first_page: u16,
    explicit_page: bool,
}

pub fn match_board_url(input: &str) -> Option<Board> {
    let url = Url::parse(input).ok()?;
    if url.scheme() != "https" {
        return None;
    }
    let (host, category) = archive(url.host_str()?)?;
    let mut segments = url.path_segments()?.filter(|x| !x.is_empty());
    let board = segments.next()?;
    if board.is_empty()
        || board.len() > 8
        || !board
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    {
        return None;
    }
    let (first_page, explicit_page) = match segments.next() {
        None => (1, false),
        Some("page") => {
            let page = segments.next()?.parse::<u16>().ok()?;
            if page == 0 {
                return None;
            }
            (page, true)
        }
        Some(page) => {
            let page = page.parse::<u16>().ok()?;
            if page == 0 {
                return None;
            }
            (page, true)
        }
    };
    if segments.next().is_some() {
        return None;
    }
    Some(Board {
        host,
        category,
        board: board.into(),
        first_page,
        explicit_page,
    })
}

fn board_thread_ids(value: &Value) -> Result<Vec<u64>, Box<dyn Error + Send + Sync>> {
    let threads = value
        .as_object()
        .ok_or("FoolFuuka index API must return a thread object")?;
    let mut ids = BTreeSet::new();
    for raw_id in threads.keys() {
        if let Ok(id) = raw_id.parse::<u64>() {
            if id > 0 {
                ids.insert(id);
            }
        }
    }
    Ok(ids.into_iter().collect())
}

pub async fn resolve_board(
    client: &Client,
    board: Board,
    max_pages: u16,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let api = Url::parse(&format!("https://{}/", board.host))?;
    resolve_board_at_base(client, board, max_pages, &api).await
}

async fn resolve_board_at_base(
    client: &Client,
    board: Board,
    max_pages: u16,
    base: &Url,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let count = if board.explicit_page {
        1
    } else {
        max_pages.max(1)
    };
    let mut files = Vec::new();
    let mut visited = BTreeSet::new();
    let mut requested = false;
    for offset in 0..count {
        let Some(page) = board.first_page.checked_add(offset) else {
            break;
        };
        if requested {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        let mut endpoint = base.join("_/api/chan/index/")?;
        endpoint
            .query_pairs_mut()
            .append_pair("board", &board.board)
            .append_pair("page", &page.to_string());
        let response: Value = client
            .get(endpoint)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        if response.is_null() {
            break;
        }
        let threads = board_thread_ids(&response)?;
        if threads.is_empty() {
            break;
        }
        for id in threads {
            if !visited.insert(id) {
                continue;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
            files.extend(
                resolve_at_base(
                    client,
                    Target {
                        host: board.host,
                        category: board.category,
                        board: board.board.clone(),
                        thread: id,
                    },
                    base,
                )
                .await?,
            );
        }
        requested = true;
    }
    Ok(files)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gallery {
    host: &'static str,
    category: &'static str,
    board: String,
    first_page: u16,
    explicit_page: bool,
}

pub fn match_gallery_url(input: &str) -> Option<Gallery> {
    let url = Url::parse(input).ok()?;
    if url.scheme() != "https" {
        return None;
    }
    let (host, category) = archive(url.host_str()?)?;
    let mut parts = url.path_segments()?.filter(|s| !s.is_empty());
    let board = parts.next()?;
    if board.is_empty()
        || board.len() > 8
        || !board
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        || parts.next()? != "gallery"
    {
        return None;
    }
    let (first_page, explicit_page) = match parts.next() {
        None => (1, false),
        Some(page) => {
            let page = page.parse::<u16>().ok()?;
            if page == 0 {
                return None;
            }
            (page, true)
        }
    };
    if parts.next().is_some() {
        return None;
    }
    Some(Gallery {
        host,
        category,
        board: board.to_owned(),
        first_page,
        explicit_page,
    })
}

pub async fn resolve_gallery(
    client: &Client,
    gallery: Gallery,
    max_pages: u16,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse(&format!("https://{}/", gallery.host))?;
    resolve_gallery_at_base(client, gallery, max_pages, &base).await
}

async fn resolve_gallery_at_base(
    client: &Client,
    gallery: Gallery,
    max_pages: u16,
    base: &Url,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let count = if gallery.explicit_page {
        1
    } else {
        max_pages.max(1)
    };
    let mut items = Vec::new();
    let mut seen = HashSet::new();
    for offset in 0..count {
        let Some(page) = gallery.first_page.checked_add(offset) else {
            break;
        };
        if offset > 0 {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        let mut endpoint = base.join("_/api/chan/gallery/")?;
        endpoint
            .query_pairs_mut()
            .append_pair("board", &gallery.board)
            .append_pair("page", &page.to_string());
        let data: Value = client
            .get(endpoint)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        if data.is_null() {
            break;
        }
        let posts = data
            .as_array()
            .ok_or("FoolFuuka gallery API must return a post array")?;
        if posts.is_empty() {
            break;
        }
        for post in posts {
            if let Some(item) = file_for_post(post, gallery.category, base) {
                if seen.insert(item.url.clone()) {
                    items.push(item);
                }
            }
        }
    }
    Ok(items)
}

fn file_for_post(post: &Value, category: &'static str, base: &Url) -> Option<MediaItem> {
    let media = post.get("media")?.as_object()?;
    let raw = media
        .get("media_link")
        .and_then(Value::as_str)
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
    let from_original = media
        .get("media_orig")
        .and_then(Value::as_str)
        .and_then(|x| x.split_once('.').map(|(id, _)| id))
        .filter(|x| x.len() <= 32 && !x.is_empty() && x.bytes().all(|b| b.is_ascii_digit()));
    let post_id = post.get("num").and_then(Value::as_u64);
    let timestamp = from_original
        .map(str::to_owned)
        .or_else(|| post_id.map(|id| id.to_string()))?;
    let label = media
        .get("media_filename")
        .and_then(Value::as_str)
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

fn parse_thread(
    body: &Value,
    target: &Target,
    base: &Url,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let data = body
        .get(target.thread.to_string())
        .ok_or("FoolFuuka API thread not found")?;
    let op = data
        .get("op")
        .ok_or("FoolFuuka API missing original post")?;
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

pub async fn resolve(
    client: &Client,
    target: Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse(&format!("https://{}/", target.host))?;
    resolve_at_base(client, target, &base).await
}

async fn resolve_at_base(
    client: &Client,
    target: Target,
    base: &Url,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let mut api = base.join("_/api/chan/thread/")?;
    api.query_pairs_mut()
        .append_pair("board", &target.board)
        .append_pair("num", &target.thread.to_string());
    let response: Value = client
        .get(api)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
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
                host: "archive.4plebs.org",
                category: "4plebs",
                board: "tg".into(),
                thread: 123
            })
        );
        assert_eq!(
            match_url("https://arch.b4k.co/g/thread/456"),
            Some(Target {
                host: "arch.b4k.dev",
                category: "b4k",
                board: "g".into(),
                thread: 456
            })
        );
        assert!(match_url("https://desuarchive.org/g/search/text/test").is_none());
        assert!(match_url("https://desuarchive.org/g/thread/0").is_none());
        assert!(match_url("http://archive.4plebs.org/tg/thread/1").is_none());
        assert!(match_url("https://notarchive.4plebs.org/tg/thread/1").is_none());
    }

    #[test]
    fn recognizes_board_indices_and_explicit_pages() {
        assert_eq!(
            match_board_url("https://archived.moe/a/"),
            Some(Board {
                host: "archived.moe",
                category: "archivedmoe",
                board: "a".into(),
                first_page: 1,
                explicit_page: false
            })
        );
        assert_eq!(
            match_board_url("https://desuarchive.org/g/page/4"),
            Some(Board {
                host: "desuarchive.org",
                category: "desuarchive",
                board: "g".into(),
                first_page: 4,
                explicit_page: true
            })
        );
        assert!(match_board_url("https://archived.moe/a/thread/123").is_none());
        assert!(match_board_url("https://archived.moe/a/page/0").is_none());
        assert!(match_board_url("https://archived.moe/a/other/2").is_none());
    }

    #[test]
    fn validates_thread_index_keys_and_sorts_ids() {
        assert_eq!(
            board_thread_ids(&json!({"9": {}, "2": {}, "bad": {}})).unwrap(),
            vec![2, 9]
        );
        assert!(board_thread_ids(&json!([1, 2, 3])).is_err());
        assert_eq!(board_thread_ids(&json!({})).unwrap(), Vec::<u64>::new());
    }

    #[tokio::test]
    async fn local_board_index_recurses_into_single_thread() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (needle, body) in [
                (
                    "GET /_/api/chan/index/?board=a&page=1 HTTP/1.1",
                    r#"{"123":{"num":123}}"#,
                ),
                (
                    "GET /_/api/chan/thread/?board=a&num=123 HTTP/1.1",
                    r#"{"123":{"op":{"num":123,"media":{"media_link":"/files/9.png","media":"9.png","media_filename":"art.png","media_orig":"9000.png"}}}}"#,
                ),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = [0_u8; 4096];
                let count = socket.read(&mut bytes).await.unwrap();
                let request = String::from_utf8_lossy(&bytes[..count]);
                assert!(request.starts_with(needle), "{request}");
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{address}/")).unwrap();
        let board = Board {
            host: "archived.moe",
            category: "archivedmoe",
            board: "a".into(),
            first_page: 1,
            explicit_page: false,
        };
        let items = resolve_board_at_base(&Client::new(), board, 1, &base)
            .await
            .unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].filename, "9000 art.png");
        server.await.unwrap();
    }

    #[test]
    fn recognizes_archive_gallery_paths_only() {
        assert_eq!(
            match_gallery_url("https://desuarchive.org/g/gallery"),
            Some(Gallery {
                host: "desuarchive.org",
                category: "desuarchive",
                board: "g".into(),
                first_page: 1,
                explicit_page: false
            })
        );
        assert_eq!(
            match_gallery_url("https://archived.moe/a/gallery/4"),
            Some(Gallery {
                host: "archived.moe",
                category: "archivedmoe",
                board: "a".into(),
                first_page: 4,
                explicit_page: true
            })
        );
        assert!(match_gallery_url("https://archived.moe/a/thread/1").is_none());
        assert!(match_gallery_url("https://archived.moe/a/gallery/0").is_none());
        assert!(match_gallery_url("https://archived.moe/a/gallery/2/extra").is_none());
        assert!(match_gallery_url("http://archived.moe/a/gallery").is_none());
    }

    #[tokio::test]
    async fn gallery_reads_media_from_local_fixture() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = [0u8; 4096];
            let n = socket.read(&mut bytes).await.unwrap();
            let request = String::from_utf8_lossy(&bytes[..n]);
            assert!(
                request.starts_with("GET /_/api/chan/gallery/?board=a&page=4 HTTP/1.1"),
                "{request}"
            );
            let body = r#"[{"num":123,"media":{"media_link":"/files/123.jpg","media":"123.jpg","media_filename":"fixture.jpg","media_orig":"990.jpg"}},{"num":124,"media":null}]"#;
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(reply.as_bytes()).await.unwrap();
        });
        let api = Url::parse(&format!("http://{addr}/")).unwrap();
        let gallery = Gallery {
            host: "archived.moe",
            category: "archivedmoe",
            board: "a".into(),
            first_page: 4,
            explicit_page: true,
        };
        let items = resolve_gallery_at_base(&Client::new(), gallery, 3, &api)
            .await
            .unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].filename, "990 fixture.jpg");
        server.await.unwrap();
    }

    #[test]
    fn orders_replies_and_rejects_unsafe_media() {
        let base = Url::parse("https://archived.moe/").unwrap();
        let target = Target {
            host: "archived.moe",
            category: "archivedmoe",
            board: "a".into(),
            thread: 123,
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
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
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
            host: "archived.moe",
            category: "archivedmoe",
            board: "a".into(),
            thread: 123,
        };
        let output = resolve_at_base(&Client::new(), target, &api).await.unwrap();
        assert_eq!(output.len(), 1);
        assert_eq!(output[0].filename, "99 fixture.jpg");
        server.await.unwrap();
    }
}
