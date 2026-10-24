//! Guarded public 4archive thread originals and bounded board queues.
//! This is an independent partial port of pinned gallery-dl 4archive.py.
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
const MAX_BOARD_PAGES: u16 = 4;
const MAX_BOARD_CHILDREN: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Thread { board: String, id: String },
    Board { board: String, start: u16 },
}

fn board_slug(s: &str) -> bool {
    (1..=32).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}
fn thread_id(s: &str) -> bool {
    (1..=18).contains(&s.len())
        && s.bytes().all(|b| b.is_ascii_digit())
        && s.parse::<u64>().ok().is_some_and(|id| id > 0)
}

pub fn match_url(raw: &str) -> Option<Target> {
    let url_text = if raw.starts_with("4archive.org/") {
        format!("https://{raw}")
    } else {
        raw.to_owned()
    };
    let lower = url_text.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "/../", "/./"]
        .iter()
        .any(|s| lower.contains(s))
    {
        return None;
    }
    let url = Url::parse(&url_text).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str() != Some("4archive.org")
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
        ["board", board, "thread", id] | ["board", board, "thread", id, ""]
            if board_slug(board) && thread_id(id) =>
        {
            Some(Target::Thread {
                board: (*board).into(),
                id: (*id).into(),
            })
        }
        ["board", board] | ["board", board, ""] if board_slug(board) => Some(Target::Board {
            board: (*board).into(),
            start: 1,
        }),
        ["board", board, page] | ["board", board, page, ""] if board_slug(board) => {
            let page = page.parse::<u16>().ok()?;
            (1..=MAX_BOARD_PAGES)
                .contains(&page)
                .then(|| Target::Board {
                    board: (*board).into(),
                    start: page,
                })
        }
        _ => None,
    }
}

fn extract<'a>(html: &'a str, start: &str, end: &str) -> Option<&'a str> {
    html.split_once(start)?.1.split_once(end).map(|(s, _)| s)
}
fn plain_text(html: &str) -> String {
    let mut result = String::new();
    let mut tag = false;
    for ch in html.chars().take(4000) {
        match ch {
            '<' => tag = true,
            '>' => tag = false,
            _ if !tag && !ch.is_control() => result.push(ch),
            _ => {}
        }
    }
    result
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .trim()
        .to_owned()
}

fn trusted_original(base: &Url, raw: &str) -> Option<(Url, String)> {
    let url = base.join(raw).ok()?;
    let host = url.host_str()?;
    let allowed = host == "4archive.org"
        || host.ends_with(".4archive.org")
        || host == "4cdn.org"
        || host.ends_with(".4cdn.org");
    if !allowed
        || url.scheme() != "https"
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
    let filename = url.path_segments()?.next_back()?;
    let extension = filename.rsplit_once('.')?.1.to_ascii_lowercase();
    if !matches!(
        extension.as_str(),
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "webm" | "mp4"
    ) {
        return None;
    }
    let name = safe_filename(filename);
    (!name.is_empty() && name != "." && name != "..").then_some((url, name))
}

type Records = (Metadata, Vec<(MediaItem, Metadata)>);

fn thread_records(
    html: &str,
    board: &str,
    thread: &str,
) -> Result<Records, Box<dyn Error + Send + Sync>> {
    let base = Url::parse("https://4archive.org/")?;
    let mut common = Metadata::new();
    common.insert("category".into(), "4archive".into());
    common.insert("subcategory".into(), "thread".into());
    common.insert("board".into(), board.into());
    common.insert("thread".into(), thread.parse::<u64>()?.into());
    common.insert(
        "post_url".into(),
        format!("https://4archive.org/board/{board}/thread/{thread}").into(),
    );
    let mut title = extract(html, "class=\"subject\">", "</span>")
        .map(plain_text)
        .unwrap_or_default();
    let post_blocks: Vec<_> = html.split("class=\"postContainer").skip(1).collect();
    if post_blocks.is_empty() || post_blocks.len() > MAX_POSTS {
        return Err("4archive thread lacks a bounded set of public posts".into());
    }
    if title.is_empty() {
        title = post_blocks
            .iter()
            .find_map(|block| {
                extract(block, "<blockquote ", "</blockquote>")
                    .and_then(|q| q.split_once('>'))
                    .map(|(_, text)| plain_text(text))
                    .filter(|s| !s.is_empty())
            })
            .unwrap_or_default()
            .chars()
            .take(50)
            .collect();
    }
    if title.len() > 240 {
        title = title.chars().take(240).collect();
    }
    common.insert("title".into(), title.into());
    let mut media = Vec::new();
    let mut seen = HashSet::new();
    for block in post_blocks {
        let Some(post) = extract(block, ">Post No.", "<").filter(|n| thread_id(n)) else {
            continue;
        };
        if !block.contains("class=\"file\"") {
            continue;
        }
        let Some(raw) = block
            .split_once("class=\"fileText\"")
            .and_then(|(_, tail)| tail.split_once("</span>").or_else(|| tail.split_once("</div>")))
            .map(|(raw, _)| raw)
        else {
            continue;
        };
        let Some(link) = extract(raw, "href=\"", "\"") else {
            continue;
        };
        let Some((url, basename)) = trusted_original(&base, link) else {
            continue;
        };
        if !seen.insert(url.to_string()) {
            continue;
        }
        if media.len() >= MAX_MEDIA {
            return Err("4archive thread exceeds supported media bound".into());
        }
        let filename = extract(raw, "alt=\"Image: ", "\"")
            .filter(|v| !v.is_empty() && v.len() <= 200 && !v.contains('/') && !v.contains('\\'))
            .map(safe_filename)
            .filter(|v| !v.is_empty())
            .unwrap_or(basename);
        let item = MediaItem {
            url: url.to_string(),
            filename: safe_filename(&format!("{post} {filename}")),
            extractor: "4archive",
            post_id: post.parse::<u64>().ok(),
        };
        let mut meta = media_metadata(&item);
        for (k, v) in &common {
            meta.insert(k.clone(), v.clone());
        }
        meta.insert("no".into(), post.parse::<u64>()?.into());
        if let Some(name) = extract(block, "class=\"name\">", "</span>") {
            let name = plain_text(name);
            if name.len() <= 120 {
                meta.insert("name".into(), name.into());
            }
        }
        if let Some(date) = extract(block, "class=\"dateTime\">", "<")
            .or_else(|| extract(block, "class=\"dateTime postNum\" >", "<"))
            .map(str::trim)
            .filter(|d| d.len() <= 80 && !d.chars().any(char::is_control))
        {
            meta.insert("date_source".into(), date.into());
        }
        if let Some(comment) = extract(block, "<blockquote ", "</blockquote>")
            .and_then(|q| q.split_once('>'))
            .map(|(_, q)| plain_text(q))
        {
            meta.insert("com".into(), comment.into());
        }
        media.push((item, meta));
    }
    Ok((common, media))
}

fn board_children(
    html: &str,
    board: &str,
    seen: &mut HashSet<String>,
    output: &mut Vec<ExtractorMessage>,
) -> bool {
    let mut found = false;
    for tail in html.split("class=\"thread\" id=\"t").skip(1).take(1000) {
        let Some(id) = tail.split_once('"').map(|(id, _)| id) else {
            continue;
        };
        if !thread_id(id) {
            continue;
        }
        found = true;
        if output.len() >= MAX_BOARD_CHILDREN {
            break;
        }
        if !seen.insert(id.into()) {
            continue;
        }
        let target = format!("https://4archive.org/board/{board}/thread/{id}");
        let mut metadata = Metadata::new();
        metadata.insert("category".into(), "4archive".into());
        metadata.insert("subcategory".into(), "board".into());
        metadata.insert("board".into(), board.into());
        metadata.insert("thread".into(), id.parse::<u64>().unwrap().into());
        output.push(queued_url(target, metadata));
    }
    found
}

async fn read_html(client: &Client, endpoint: Url) -> Result<String, Box<dyn Error + Send + Sync>> {
    let mut response = client
        .get(endpoint.clone())
        .send()
        .await?
        .error_for_status()?;
    if response.url().origin() != endpoint.origin() {
        return Err("4archive source redirected to a different origin".into());
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_HTML as u64)
    {
        return Err("4archive source HTML exceeds 2 MiB limit".into());
    }
    let mut content = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_HTML.saturating_sub(content.len()) {
            return Err("4archive source HTML exceeds 2 MiB limit".into());
        }
        content.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8(content)?)
}

async fn thread_at_base(
    client: &Client,
    base: &Url,
    board: &str,
    thread: &str,
) -> Result<Records, Box<dyn Error + Send + Sync>> {
    let url = base.join(&format!("board/{board}/thread/{thread}"))?;
    thread_records(&read_html(client, url).await?, board, thread)
}

async fn board_at_base(
    client: &Client,
    base: &Url,
    board: &str,
    start: u16,
    count: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let mut seen = HashSet::new();
    let mut output = Vec::new();
    let end = start
        .saturating_add(count.clamp(1, MAX_BOARD_PAGES))
        .min(MAX_BOARD_PAGES + 1);
    for page in start..end {
        if page > start {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        let html = read_html(client, base.join(&format!("board/{board}/{page}"))?).await?;
        if !board_children(&html, board, &mut seen, &mut output)
            || output.len() >= MAX_BOARD_CHILDREN
        {
            break;
        }
    }
    Ok(output)
}

pub async fn resolve(
    client: &Client,
    target: Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse("https://4archive.org/")?;
    match target {
        Target::Thread { board, id } => Ok(thread_at_base(client, &base, &board, &id)
            .await?
            .1
            .into_iter()
            .map(|(item, _)| item)
            .collect()),
        Target::Board { .. } => {
            Err("4archive board feeds require --dump-messages or --follow-queues".into())
        }
    }
}

pub async fn resolve_messages(
    client: &Client,
    target: Target,
    max_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse("https://4archive.org/")?;
    match target {
        Target::Thread { board, id } => {
            let (common, records) = thread_at_base(client, &base, &board, &id).await?;
            let mut events = vec![ExtractorMessage::Directory { metadata: common }];
            for (media, meta) in records {
                events.push(ExtractorMessage::Url {
                    url: media.url,
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
    fn pinned_board_and_thread_routes_enforce_first_party_and_page_bounds() {
        assert_eq!(
            match_url("https://4archive.org/board/a/thread/12345"),
            Some(Target::Thread {
                board: "a".into(),
                id: "12345".into()
            })
        );
        assert_eq!(
            match_url("https://4archive.org/board/g/2"),
            Some(Target::Board {
                board: "g".into(),
                start: 2
            })
        );
        assert_eq!(
            match_url("4archive.org/board/g/"),
            Some(Target::Board {
                board: "g".into(),
                start: 1
            })
        );
        for bad in [
            "https://evil4archive.org/board/a/thread/12345",
            "https://4archive.org/board/a/thread/0",
            "https://4archive.org/board/a/5",
            "https://4archive.org/board/a/thread/12345?private=1",
            "https://name:pw@4archive.org/board/a/thread/12345",
            "https://4archive.org:8443/board/a",
            "https://4archive.org/board/a/../../private",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn source_post_originals_keep_title_author_comment_and_safe_media() {
        let page = r#"<span class="subject">Source &amp; Story</span><div class="postContainer"><span class="name">Anonymous</span><span class="dateTime">2026-01-12T11:30:00</span><span>Post No.1001</span><div class="file"><span class="fileText">File: <a href="https://media.4archive.org/abc.jpg"><img alt="Image: picture.jpg"></a></span></div><blockquote class="postMessage">A &amp; B</blockquote></div><div class="postContainer"><span>Post No.1002</span><div class="file"><span class="fileText">File: <a href="https://evil4archive.org/bad.jpg"><img alt="Image: bad.jpg"></a></span></div></div>"#;
        let (common, records) = thread_records(page, "a", "999").unwrap();
        assert_eq!(common["title"], "Source & Story");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].0.filename, "1001 picture.jpg");
        assert_eq!(records[0].1["name"], "Anonymous");
        assert_eq!(records[0].1["date_source"], "2026-01-12T11:30:00");
        assert_eq!(records[0].1["com"], "A & B");
    }

    #[test]
    fn board_deduplicates_scoped_numeric_thread_ids() {
        let mut seen = HashSet::new();
        let mut events = Vec::new();
        let page = r#"<div class="thread" id="t1001"></div><div class="thread" id="t1001"></div><div class="thread" id="t1002"></div><div class="thread" id="t../../bad"></div>"#;
        assert!(board_children(page, "g", &mut seen, &mut events));
        assert_eq!(events.len(), 2);
        assert_eq!(
            serde_json::to_value(&events[1]).unwrap()["url"],
            "https://4archive.org/board/g/thread/1002"
        );
    }

    #[tokio::test]
    async fn thread_and_board_bounded_http_requests_use_first_party_routes() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (path, body) in [
                (
                    "/board/a/thread/100",
                    r#"<div class="postContainer"><span>Post No.200</span><div class="file"><span class="fileText">File: <a href="https://files.4archive.org/image.png"><img alt="Image: picture.png"></a></span></div></div>"#,
                ),
                ("/board/a/1", r#"<div class="thread" id="t100"></div>"#),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut req = [0u8; 4096];
                let n = socket.read(&mut req).await.unwrap();
                assert!(
                    String::from_utf8_lossy(&req[..n]).starts_with(&format!("GET {path} HTTP/1.1"))
                );
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let records = thread_at_base(&Client::new(), &base, "a", "100")
            .await
            .unwrap();
        assert_eq!(records.1.len(), 1);
        let boards = board_at_base(&Client::new(), &base, "a", 1, 1)
            .await
            .unwrap();
        assert_eq!(boards.len(), 1);
        server.await.unwrap();
    }
}
