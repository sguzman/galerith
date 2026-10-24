//! Guarded pinned public 2chen board catalog and thread media HTML preview.
use crate::{
    message::{media_metadata, queued_url, ExtractorMessage, Metadata},
    safe_filename, MediaItem,
};
use reqwest::Client;
use std::{collections::HashSet, error::Error};
use url::Url;

const MAX_HTML: usize = 2 * 1024 * 1024;
const MAX_MEDIA: usize = 128;
const MAX_CHILDREN: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Thread {
        root: String,
        board: String,
        id: String,
    },
    Board {
        root: String,
        board: String,
    },
}
type ThreadRecords = (Metadata, Vec<(MediaItem, Metadata)>);

fn valid_board(input: &str) -> bool {
    (1..=32).contains(&input.len())
        && input
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
}

fn valid_id(input: &str) -> bool {
    (1..=18).contains(&input.len())
        && input.bytes().all(|c| c.is_ascii_digit())
        && input.parse::<u64>().ok().is_some_and(|num| num > 0)
}

pub fn match_url(raw: &str) -> Option<Target> {
    let candidate = if raw.starts_with("sturdychan.help/")
        || raw.starts_with("2chen.moe/")
        || raw.starts_with("2chen.club/")
        || raw.starts_with("schan.help/")
    {
        format!("https://{raw}")
    } else {
        raw.to_owned()
    };
    let lower = candidate.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "/../", "/./"]
        .iter()
        .any(|term| lower.contains(term))
    {
        return None;
    }
    let url = Url::parse(&candidate).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let root = match url.host_str()? {
        "sturdychan.help" | "2chen.moe" | "2chen.club" => "https://sturdychan.help",
        "schan.help" => "https://schan.help",
        _ => return None,
    }
    .to_owned();
    let parts: Vec<_> = url.path_segments()?.collect();
    match parts.as_slice() {
        [board] | [board, ""] | [board, "catalog"] | [board, "catalog", ""]
            if valid_board(board) =>
        {
            Some(Target::Board {
                root,
                board: (*board).to_owned(),
            })
        }
        [board, id] | [board, id, ""] if valid_board(board) && valid_id(id) => {
            Some(Target::Thread {
                root,
                board: (*board).to_owned(),
                id: (*id).to_owned(),
            })
        }
        _ => None,
    }
}

fn field<'a>(text: &'a str, start: &str, end: &str) -> Option<&'a str> {
    text.split_once(start)?.1.split_once(end).map(|(s, _)| s)
}

fn visible(text: &str) -> Option<String> {
    if text.len() > 512 || text.chars().any(char::is_control) || text.contains('<') {
        return None;
    }
    Some(
        text.replace("&amp;", "&")
            .replace("&quot;", "\"")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&#39;", "'"),
    )
}

fn scoped_url(root: &Url, raw: &str) -> Option<Url> {
    if raw.len() > 1600
        || raw.contains('\\')
        || raw.contains('%')
        || raw.split('/').any(|part| part == "." || part == "..")
    {
        return None;
    }
    let url = root.join(raw).ok()?;
    if url.origin() != root.origin()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    Some(url)
}

fn parse_thread(
    html: &str,
    board: &str,
    thread_id: &str,
    base: &Url,
) -> Result<ThreadRecords, Box<dyn Error + Send + Sync>> {
    let source_board = field(html, "class=\"board\">/", "/<")
        .filter(|s| valid_board(s) && *s == board)
        .ok_or("2chen public board identity missing or mismatched")?;
    let title = field(html, "<h3>", "</h3>")
        .and_then(visible)
        .ok_or("2chen thread lacks a supported public title")?;
    let mut directory = Metadata::new();
    directory.insert("category".into(), "2chen".into());
    directory.insert("subcategory".into(), "thread".into());
    directory.insert("board".into(), source_board.into());
    directory.insert("thread".into(), thread_id.into());
    directory.insert("title".into(), title.into());
    let mut files = Vec::new();
    let mut seen = HashSet::new();
    for segment in html.split("class=\"glass media").skip(1).take(500) {
        let Some((article, _)) = segment.split_once("</article>") else {
            continue;
        };
        let Some(figure) = field(article, "<figure>", "</") else {
            continue;
        };
        let Some(href) = field(figure, "href=\"", "\"") else {
            continue;
        };
        let Some(url) = scoped_url(base, href.split_once('?').map_or(href, |(path, _)| path))
        else {
            return Err("2chen post media escaped trusted site origin".into());
        };
        if !seen.insert(url.to_string()) {
            continue;
        }
        if files.len() >= MAX_MEDIA {
            return Err("2chen thread exceeds bounded media limit".into());
        }
        let Some(raw_filename) = field(article, "download=\"", "\"") else {
            continue;
        };
        if raw_filename.len() > 240
            || raw_filename.chars().any(char::is_control)
            || raw_filename.contains('/')
            || raw_filename.contains('\\')
        {
            return Err("2chen source download filename unsafe".into());
        }
        let (filename_stem, extension) = raw_filename
            .rsplit_once('.')
            .ok_or("2chen source download filename lacks extension")?;
        let extension = extension.to_ascii_lowercase();
        if !matches!(
            extension.as_str(),
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "webm" | "mp4" | "mp3"
        ) {
            continue;
        }
        let filename = safe_filename(raw_filename);
        if filename_stem.is_empty() || filename.is_empty() {
            return Err("2chen source download filename empty".into());
        }
        let post_id = field(article, "href=\"#p", "\"")
            .filter(|id| valid_id(id))
            .and_then(|s| s.parse::<u64>().ok());
        let post_name = field(article, "<span>", "</span>").and_then(visible);
        let date_source = field(article, "<time", ">")
            .and_then(|_| field(article, "<time", "</time>"))
            .and_then(|text| text.split_once('>').map(|(_, date)| date))
            .and_then(visible);
        let hash = field(article, "data-hash=\"", "\"").and_then(|s| {
            (s.len() <= 160 && s.bytes().all(|b| b.is_ascii_alphanumeric())).then_some(s)
        });
        let item = MediaItem {
            url: url.to_string(),
            filename,
            extractor: "2chen",
            post_id,
        };
        let mut meta = media_metadata(&item);
        for (key, value) in &directory {
            meta.insert(key.clone(), value.clone());
        }
        meta.insert("extension".into(), extension.into());
        meta.insert("source_filename".into(), raw_filename.into());
        if let Some(name) = post_name {
            meta.insert("name".into(), name.into());
        }
        if let Some(date) = date_source {
            meta.insert("date_source".into(), date.into());
        }
        if let Some(hash) = hash {
            meta.insert("hash".into(), hash.into());
        }
        files.push((item, meta));
    }
    Ok((directory, files))
}

fn parse_catalog(
    html: &str,
    base: &Url,
    board: &str,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let mut seen = HashSet::new();
    let mut events = Vec::new();
    for chunk in html.split("<figure><a href=\"").skip(1).take(2000) {
        let Some((raw, _)) = chunk.split_once('"') else {
            continue;
        };
        let Some(url) = scoped_url(base, raw) else {
            continue;
        };
        let segments: Vec<_> = url
            .path_segments()
            .ok_or("2chen catalog URL missing path")?
            .collect();
        let ([source_board, thread_id] | [source_board, thread_id, ""]) = segments.as_slice()
        else {
            continue;
        };
        if *source_board != board || !valid_id(thread_id) || !seen.insert((*thread_id).to_owned()) {
            continue;
        }
        if events.len() >= MAX_CHILDREN {
            return Err("2chen catalog exceeds child queue limit".into());
        }
        let mut meta = Metadata::new();
        meta.insert("category".into(), "2chen".into());
        meta.insert("subcategory".into(), "board".into());
        meta.insert("board".into(), board.into());
        meta.insert("thread".into(), (*thread_id).into());
        events.push(queued_url(format!("{base}{board}/{thread_id}"), meta));
    }
    Ok(events)
}

async fn html_at(client: &Client, endpoint: Url) -> Result<String, Box<dyn Error + Send + Sync>> {
    let mut response = client
        .get(endpoint.clone())
        .send()
        .await?
        .error_for_status()?;
    if response.url().origin() != endpoint.origin() {
        return Err("2chen public page redirected outside site origin".into());
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_HTML as u64)
    {
        return Err("2chen source HTML exceeds 2 MiB".into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_HTML.saturating_sub(body.len()) {
            return Err("2chen source HTML exceeds 2 MiB".into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8(body)?)
}

async fn thread_at(
    client: &Client,
    base: &Url,
    board: &str,
    id: &str,
) -> Result<ThreadRecords, Box<dyn Error + Send + Sync>> {
    let text = html_at(client, base.join(&format!("{board}/{id}"))?).await?;
    parse_thread(&text, board, id, base)
}

async fn catalog_at(
    client: &Client,
    base: &Url,
    board: &str,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let text = html_at(client, base.join(&format!("{board}/catalog"))?).await?;
    parse_catalog(&text, base, board)
}

pub async fn resolve(
    client: &Client,
    target: Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    match target {
        Target::Thread { root, board, id } => {
            Ok(
                thread_at(client, &Url::parse(&format!("{root}/"))?, &board, &id)
                    .await?
                    .1
                    .into_iter()
                    .map(|(item, _)| item)
                    .collect(),
            )
        }
        Target::Board { .. } => {
            Err("2chen board catalog requires --dump-messages or --follow-queues".into())
        }
    }
}

pub async fn resolve_messages(
    client: &Client,
    target: Target,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    match target {
        Target::Thread { root, board, id } => {
            let (directory, files) =
                thread_at(client, &Url::parse(&format!("{root}/"))?, &board, &id).await?;
            let mut events = vec![ExtractorMessage::Directory {
                metadata: directory,
            }];
            events.extend(
                files
                    .into_iter()
                    .map(|(item, metadata)| ExtractorMessage::Url {
                        url: item.url,
                        metadata,
                    }),
            );
            Ok(events)
        }
        Target::Board { root, board } => {
            catalog_at(client, &Url::parse(&format!("{root}/"))?, &board).await
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

    fn source_fixture() -> &'static str {
        r##"<span class="board">/a/<h3>Gallery &amp; discussion</h3><article class="glass media a"><span>Author</span><time>14 Oct 2026 (Wed) 22:33:44</time><a href="#p12345">Post</a><a download="drawing one.png">Download</a><figure><a href="/files/12345.png?thumb=1">Media</a></figure><a data-hash="abc123">Hash</a></article><article class="glass media b"><span>Other</span><a href="#p12346">Post</a><a download="sketch.jpg">Download</a><figure><a href="/files/12346.jpg">Media</a></figure></article>"##
    }

    #[test]
    fn canonical_sturdychan_schan_and_2chen_aliases_are_scoped() {
        assert_eq!(
            match_url("2chen.moe/a/12345/"),
            Some(Target::Thread {
                root: "https://sturdychan.help".into(),
                board: "a".into(),
                id: "12345".into()
            })
        );
        assert_eq!(
            match_url("https://schan.help/a/catalog"),
            Some(Target::Board {
                root: "https://schan.help".into(),
                board: "a".into()
            })
        );
        for bad in [
            "https://evilsturdychan.help/a/12345",
            "https://sturdychan.help/a/0",
            "https://sturdychan.help/a/12345?private=1",
            "https://sturdychan.help/a/%2e%2e/12345",
            "https://user:password@sturdychan.help/a/12345",
            "https://sturdychan.help:8443/a/12345",
            "https://sturdychan.help/a/12345/another",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn thread_html_produces_source_order_post_ids_and_safe_download_names() {
        let root = Url::parse("https://sturdychan.help/").unwrap();
        let (dir, files) = parse_thread(source_fixture(), "a", "12345", &root).unwrap();
        assert_eq!(dir["title"], "Gallery & discussion");
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].0.url, "https://sturdychan.help/files/12345.png");
        assert_eq!(files[0].0.filename, "drawing one.png");
        assert_eq!(files[0].0.post_id, Some(12345));
        assert_eq!(files[0].1["hash"], "abc123");
        assert_eq!(files[0].1["date_source"], "14 Oct 2026 (Wed) 22:33:44");
        let evil = source_fixture().replace(
            "/files/12345.png?thumb=1",
            "https://evil.sturdychan.help/file.png",
        );
        assert!(parse_thread(&evil, "a", "12345", &root).is_err());
    }

    #[test]
    fn catalog_queues_exact_first_party_thread_urls_and_deduplicates() {
        let root = Url::parse("https://sturdychan.help/").unwrap();
        let html = r#"<figure><a href="/a/12345">One</a></figure><figure><a href="/a/12345">Dup</a></figure><figure><a href="/a/12346">Two</a></figure><figure><a href="https://evil.sturdychan.help/a/99999">Bad</a></figure>"#;
        let events = parse_catalog(html, &root, "a").unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(
            serde_json::to_value(&events[1]).unwrap()["url"],
            "https://sturdychan.help/a/12346"
        );
    }

    #[tokio::test]
    async fn public_thread_request_returns_two_safe_originals() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 4096];
            let n = socket.read(&mut request).await.unwrap();
            assert!(String::from_utf8_lossy(&request[..n]).starts_with("GET /a/12345 HTTP/1.1"));
            let body = source_fixture();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let (dir, files) = thread_at(&Client::new(), &base, "a", "12345")
            .await
            .unwrap();
        assert_eq!(dir["board"], "a");
        assert_eq!(files.len(), 2);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn public_catalog_request_queues_two_thread_children() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 4096];
            let n = socket.read(&mut request).await.unwrap();
            assert!(String::from_utf8_lossy(&request[..n]).starts_with("GET /a/catalog HTTP/1.1"));
            let body = r#"<figure><a href="/a/12345">One</a></figure><figure><a href="/a/12346">Two</a></figure>"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let events = catalog_at(&Client::new(), &base, "a").await.unwrap();
        assert_eq!(events.len(), 2);
        server.await.unwrap();
    }
}
