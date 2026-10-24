//! Guarded public 4chanarchives threads and board queues from pinned gallery-dl.
use crate::{
    message::{media_metadata, queued_url, ExtractorMessage, Metadata},
    safe_filename, MediaItem,
};
use reqwest::Client;
use std::{collections::HashSet, error::Error, time::Duration};
use url::Url;

const MAX_HTML: usize = 2 * 1024 * 1024;
const MAX_POSTS: usize = 256;
const MAX_MEDIA: usize = 160;
const MAX_PAGES: u16 = 4;
const MAX_CHILDREN: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Thread { board: String, id: String },
    Board { board: String, start: u16 },
}

fn board_name(s: &str) -> bool {
    (1..=32).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}
fn numeric(s: &str) -> bool {
    (1..=18).contains(&s.len())
        && s.bytes().all(|b| b.is_ascii_digit())
        && s.parse::<u64>().ok().is_some_and(|n| n > 0)
}

pub fn match_url(raw: &str) -> Option<Target> {
    let candidate = if raw.starts_with("4chanarchives.com/") {
        format!("https://{raw}")
    } else {
        raw.to_owned()
    };
    let lower = candidate.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "/../", "/./"]
        .iter()
        .any(|s| lower.contains(s))
    {
        return None;
    }
    let url = Url::parse(&candidate).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str() != Some("4chanarchives.com")
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
        ["board", board, "thread", id] | ["board", board, "thread", id, ""]
            if board_name(board) && numeric(id) =>
        {
            Some(Target::Thread {
                board: (*board).into(),
                id: (*id).into(),
            })
        }
        ["board", board] | ["board", board, ""] if board_name(board) => Some(Target::Board {
            board: (*board).into(),
            start: 1,
        }),
        ["board", board, p] | ["board", board, p, ""] if board_name(board) => {
            let start = p.parse::<u16>().ok()?;
            (1..=MAX_PAGES).contains(&start).then(|| Target::Board {
                board: (*board).into(),
                start,
            })
        }
        _ => None,
    }
}

fn between<'a>(s: &'a str, begin: &str, end: &str) -> Option<&'a str> {
    s.split_once(begin)?.1.split_once(end).map(|(v, _)| v)
}
fn plain_text(html: &str) -> String {
    let mut output = String::new();
    let mut tag = false;
    for ch in html.chars().take(4000) {
        match ch {
            '<' => tag = true,
            '>' => tag = false,
            _ if !tag && !ch.is_control() => output.push(ch),
            _ => {}
        }
    }
    output
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .trim()
        .into()
}

fn original(raw: &str) -> Option<(Url, String)> {
    let root = Url::parse("https://4chanarchives.com/").ok()?;
    let url = root.join(raw).ok()?;
    let host = url.host_str()?;
    if url.scheme() != "https"
        || !(host == "4chanarchives.com"
            || host.ends_with(".4chanarchives.com")
            || host == "4cdn.org"
            || host.ends_with(".4cdn.org"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path().contains('%')
        || url.path().contains("..")
    {
        return None;
    }
    let last = url.path_segments()?.next_back()?;
    let ext = last.rsplit_once('.')?.1.to_ascii_lowercase();
    if !matches!(
        ext.as_str(),
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "webm" | "mp4"
    ) {
        return None;
    }
    let name = safe_filename(last);
    if name.is_empty() || name == "." || name == ".." {
        return None;
    }
    Some((url, name))
}

type Records = (Metadata, Vec<(MediaItem, Metadata)>);

fn thread_records(
    html: &str,
    board: &str,
    thread: &str,
) -> Result<Records, Box<dyn Error + Send + Sync>> {
    let blocks: Vec<_> = html.split("id=\"pc").skip(1).take(MAX_POSTS + 1).collect();
    if blocks.is_empty() || blocks.len() > MAX_POSTS {
        return Err("4chanarchives thread exceeds bounded public post count".into());
    }
    let mut common = Metadata::new();
    common.insert("category".into(), "4chanarchives".into());
    common.insert("subcategory".into(), "thread".into());
    common.insert("board".into(), board.into());
    common.insert("thread".into(), thread.parse::<u64>()?.into());
    common.insert(
        "post_url".into(),
        format!("https://4chanarchives.com/board/{board}/thread/{thread}").into(),
    );
    let mut title = between(html, "property=\"og:title\" content=\"", "\"")
        .map(plain_text)
        .unwrap_or_default();
    if title.is_empty() {
        title = blocks
            .iter()
            .find_map(|b| {
                b.split_once("<blockquote")
                    .and_then(|(_, tail)| tail.split_once('>'))
                    .map(|(_, com)| {
                        plain_text(
                            com.split_once("</blockquote>")
                                .map(|(s, _)| s)
                                .unwrap_or(com),
                        )
                    })
                    .filter(|s| !s.is_empty())
            })
            .unwrap_or_default();
    }
    common.insert(
        "title".into(),
        title.chars().take(240).collect::<String>().into(),
    );
    let mut seen = HashSet::new();
    let mut items = Vec::new();
    for block in blocks {
        let Some(no) = block.split_once('"').map(|(n, _)| n).filter(|n| numeric(n)) else {
            continue;
        };
        let Some(file) = block.split_once(">File: <").map(|(_, tail)| tail) else {
            continue;
        };
        let Some(href) = between(file, "href=\"", "\"") else {
            continue;
        };
        let Some((url, basename)) = original(href) else {
            continue;
        };
        if !seen.insert(url.to_string()) {
            continue;
        }
        if items.len() >= MAX_MEDIA {
            return Err("4chanarchives thread exceeds supported attachment bound".into());
        }
        let filename = between(file, ">", "<")
            .map(plain_text)
            .filter(|name| {
                !name.is_empty() && name.len() <= 200 && !name.contains('/') && !name.contains('\\')
            })
            .map(|name| safe_filename(&name))
            .filter(|name| !name.is_empty())
            .unwrap_or(basename);
        let item = MediaItem {
            url: url.to_string(),
            filename: safe_filename(&format!("{no}-{filename}")),
            extractor: "4chanarchives",
            post_id: no.parse::<u64>().ok(),
        };
        let mut meta = media_metadata(&item);
        for (key, value) in &common {
            meta.insert(key.clone(), value.clone());
        }
        meta.insert("no".into(), no.parse::<u64>()?.into());
        if let Some(name) = between(block, "class=\"name\">", "<") {
            let name = plain_text(name);
            if name.len() <= 120 {
                meta.insert("name".into(), name.into());
            }
        }
        if let Some(time) = between(block, "class=\"dateTime postNum\" >", "<")
            .map(str::trim)
            .filter(|s| s.len() <= 80 && !s.chars().any(char::is_control))
        {
            meta.insert("time_source".into(), time.into());
        }
        if let Some(comment) = block
            .split_once("<blockquote")
            .and_then(|(_, tail)| tail.split_once('>'))
            .map(|(_, text)| {
                text.split_once("</blockquote>")
                    .map(|(s, _)| s)
                    .unwrap_or(text)
            })
        {
            meta.insert("com".into(), plain_text(comment).into());
        }
        items.push((item, meta));
    }
    Ok((common, items))
}

fn board_children(
    html: &str,
    board: &str,
    seen: &mut HashSet<String>,
    events: &mut Vec<ExtractorMessage>,
) -> bool {
    let root = Url::parse("https://4chanarchives.com/").unwrap();
    let mut any = false;
    for fragment in html.split("class=\"postNum desktop\"").skip(1).take(2000) {
        let Some(href) = between(fragment, "href=\"", "\"") else {
            continue;
        };
        let Ok(url) = root.join(href) else { continue };
        if url.scheme() != "https"
            || url.host_str() != Some("4chanarchives.com")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            continue;
        }
        let Some(paths) = url.path_segments() else {
            continue;
        };
        let parts: Vec<_> = paths.collect();
        let (["board", actual, "thread", id] | ["board", actual, "thread", id, ""]) =
            parts.as_slice()
        else {
            continue;
        };
        if *actual != board || !numeric(id) {
            continue;
        }
        any = true;
        if events.len() >= MAX_CHILDREN {
            break;
        }
        if !seen.insert((*id).into()) {
            continue;
        }
        let mut meta = Metadata::new();
        meta.insert("category".into(), "4chanarchives".into());
        meta.insert("subcategory".into(), "board".into());
        meta.insert("board".into(), board.into());
        meta.insert("thread".into(), id.parse::<u64>().unwrap().into());
        events.push(queued_url(
            format!("https://4chanarchives.com/board/{board}/thread/{id}"),
            meta,
        ));
    }
    any
}

async fn read_html(client: &Client, endpoint: Url) -> Result<String, Box<dyn Error + Send + Sync>> {
    let mut response = client
        .get(endpoint.clone())
        .send()
        .await?
        .error_for_status()?;
    if response.url().origin() != endpoint.origin() {
        return Err("4chanarchives source redirected outside source origin".into());
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_HTML as u64)
    {
        return Err("4chanarchives public HTML exceeds byte cap".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_HTML.saturating_sub(bytes.len()) {
            return Err("4chanarchives public HTML exceeds byte cap".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8(bytes)?)
}

async fn thread_at_base(
    client: &Client,
    base: &Url,
    board: &str,
    thread: &str,
) -> Result<Records, Box<dyn Error + Send + Sync>> {
    let html = read_html(
        client,
        base.join(&format!("board/{board}/thread/{thread}"))?,
    )
    .await?;
    thread_records(&html, board, thread)
}

async fn board_at_base(
    client: &Client,
    base: &Url,
    board: &str,
    start: u16,
    max: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let mut seen = HashSet::new();
    let mut events = Vec::new();
    let end = start
        .saturating_add(max.clamp(1, MAX_PAGES))
        .min(MAX_PAGES + 1);
    for page in start..end {
        if page > start {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        let body = read_html(client, base.join(&format!("board/{board}/{page}"))?).await?;
        if !board_children(&body, board, &mut seen, &mut events) || events.len() >= MAX_CHILDREN {
            break;
        }
    }
    Ok(events)
}

pub async fn resolve(
    client: &Client,
    target: Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse("https://4chanarchives.com/")?;
    match target {
        Target::Thread { board, id } => Ok(thread_at_base(client, &base, &board, &id)
            .await?
            .1
            .into_iter()
            .map(|(item, _)| item)
            .collect()),
        Target::Board { .. } => {
            Err("4chanarchives board feeds require --dump-messages or --follow-queues".into())
        }
    }
}

pub async fn resolve_messages(
    client: &Client,
    target: Target,
    max_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse("https://4chanarchives.com/")?;
    match target {
        Target::Thread { board, id } => {
            let (common, files) = thread_at_base(client, &base, &board, &id).await?;
            let mut events = vec![ExtractorMessage::Directory { metadata: common }];
            for (file, meta) in files {
                events.push(ExtractorMessage::Url {
                    url: file.url,
                    metadata: meta,
                });
            }
            Ok(events)
        }
        Target::Board { board, start } => {
            board_at_base(client, &base, &board, start, max_pages).await
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
    fn public_archive_thread_and_board_routes_reject_unsafe_inputs() {
        assert_eq!(
            match_url("https://4chanarchives.com/board/a/thread/12345"),
            Some(Target::Thread {
                board: "a".into(),
                id: "12345".into()
            })
        );
        assert_eq!(
            match_url("https://4chanarchives.com/board/a/2"),
            Some(Target::Board {
                board: "a".into(),
                start: 2
            })
        );
        for bad in [
            "https://fake4chanarchives.com/board/a/thread/12345",
            "https://4chanarchives.com/board/a/thread/0",
            "https://4chanarchives.com/board/a/5",
            "https://4chanarchives.com/board/a/thread/123?private=1",
            "https://name:pw@4chanarchives.com/board/a/thread/123",
            "https://4chanarchives.com:8443/board/a",
            "https://4chanarchives.com/board/a/../../private",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn archived_thread_original_preserves_title_name_date_and_post_comment() {
        let content = r#"<meta property="og:title" content="Thread &amp; Title"><div id="pc1001"><span class="name">Anonymous</span><span class="dateTime postNum" >2026-06-01T12:30:00</span>File: <a href="https://images.4chanarchives.com/pic.png">Picture.png</a><blockquote>A &amp; B</blockquote></div><div id="pc1002">File: <a href="https://fake4chanarchives.com/bad.png">Bad.png</a><blockquote>Bad</blockquote></div>"#;
        let (common, files) = thread_records(content, "a", "900").unwrap();
        assert_eq!(common["title"], "Thread & Title");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].0.filename, "1001-Picture.png");
        assert_eq!(files[0].1["name"], "Anonymous");
        assert_eq!(files[0].1["time_source"], "2026-06-01T12:30:00");
        assert_eq!(files[0].1["com"], "A & B");
    }

    #[test]
    fn board_queues_only_same_board_first_party_numeric_threads() {
        let mut seen = HashSet::new();
        let mut events = Vec::new();
        let page = r#"<span class="postNum desktop"><span><a href="/board/a/thread/1001">1</a></span><span class="postNum desktop"><span><a href="/board/a/thread/1001">Dup</a></span><span class="postNum desktop"><span><a href="/board/a/thread/1002">2</a></span><span class="postNum desktop"><span><a href="/board/b/thread/1003">Wrong board</a></span><span class="postNum desktop"><span><a href="https://fake4chanarchives.com/board/a/thread/1004">Forged</a></span>"#;
        assert!(board_children(page, "a", &mut seen, &mut events));
        assert_eq!(events.len(), 2);
        assert_eq!(
            serde_json::to_value(&events[1]).unwrap()["url"],
            "https://4chanarchives.com/board/a/thread/1002"
        );
    }

    #[tokio::test]
    async fn archived_thread_and_board_use_two_bounded_html_gets() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (path, html) in [
                (
                    "/board/a/thread/100",
                    r#"<div id="pc200">File: <a href="https://images.4chanarchives.com/one.jpg">one.jpg</a><blockquote>Hi</blockquote>"#,
                ),
                (
                    "/board/a/1",
                    r#"<span class="postNum desktop"><span><a href="/board/a/thread/100">100</a></span>"#,
                ),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buffer = [0u8; 4096];
                let n = socket.read(&mut buffer).await.unwrap();
                assert!(String::from_utf8_lossy(&buffer[..n])
                    .starts_with(&format!("GET {path} HTTP/1.1")));
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
                    html.len()
                );
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let files = thread_at_base(&Client::new(), &base, "a", "100")
            .await
            .unwrap();
        assert_eq!(files.1.len(), 1);
        let events = board_at_base(&Client::new(), &base, "a", 1, 1)
            .await
            .unwrap();
        assert_eq!(events.len(), 1);
        server.await.unwrap();
    }
}
