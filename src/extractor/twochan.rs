//! Guarded Futaba / 2chan public thread-original preview from pinned 2chan.py.
use crate::{message::{media_metadata, ExtractorMessage, Metadata}, MediaItem};
use reqwest::Client;
use std::{collections::HashSet, error::Error};
use url::Url;

const MAX_HTML: usize = 2 * 1024 * 1024;
const MAX_MEDIA: usize = 160;
type Records = (Metadata, Vec<(MediaItem, Metadata)>);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thread { server: String, board: String, id: String }

fn slug(s: &str, max: usize) -> bool {
    (1..=max).contains(&s.len())
        && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_'))
}
fn numeric(s: &str) -> bool {
    (1..=18).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit())
        && s.parse::<u64>().ok().is_some_and(|n| n > 0)
}

pub fn match_url(raw: &str) -> Option<Thread> {
    let input = if !raw.contains("://") && raw.contains(".2chan.net/") {
        format!("https://{raw}")
    } else { raw.to_owned() };
    let lower = input.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "/../", "/./"].iter().any(|v| lower.contains(v)) {
        return None;
    }
    let url = Url::parse(&input).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty() || url.password().is_some() || url.port().is_some()
        || url.query().is_some() || url.fragment().is_some() {
        return None;
    }
    let server = url.host_str()?.strip_suffix(".2chan.net")?;
    if !slug(server, 36) { return None }
    let parts: Vec<_> = url.path_segments()?.collect();
    match parts.as_slice() {
        [board, "res", id] | [board, "res", id, ""] if slug(board, 24) => {
            let id = id.strip_suffix(".htm").unwrap_or(id);
            numeric(id).then(|| Thread { server: server.into(), board: (*board).into(), id: id.into() })
        }
        _ => None,
    }
}

fn extract<'a>(page: &'a str, start: &str, end: &str) -> Option<&'a str> {
    page.split_once(start)?.1.split_once(end).map(|(value, _)| value)
}
fn text_only(value: &str) -> String {
    let mut output = String::new();
    let mut tag = false;
    for ch in value.chars().take(2000) {
        match ch {
            '<' => tag = true,
            '>' => tag = false,
            _ if !tag => output.push(ch),
            _ => {}
        }
    }
    output.replace("&amp;", "&").replace("&quot;", "\"").replace("&lt;", "<")
        .replace("&gt;", ">").replace("&#39;", "'").trim().chars().take(240).collect()
}

fn original(root: &Url, board: &str, source: &str) -> Option<(Url, String, String)> {
    if !source.starts_with('/') || source.contains('%') || source.contains('\\')
        || source.split('/').any(|part| matches!(part, "." | "..")) {
        return None;
    }
    let url = root.join(source).ok()?;
    if url.origin() != root.origin() || !url.username().is_empty() || url.password().is_some()
        || url.query().is_some() || url.fragment().is_some() { return None }
    let parts: Vec<_> = url.path_segments()?.collect();
    let [file_board, "src", name] = parts.as_slice() else { return None };
    if *file_board != board { return None }
    let (tim, ext) = name.rsplit_once('.')?;
    let ext = ext.to_ascii_lowercase();
    if !numeric(tim) || !matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "gif" | "webp" | "webm" | "mp4") {
        return None;
    }
    Some((url, tim.into(), ext))
}

fn parse(html: &str, target: &Thread, root: &Url) -> Result<Records, Box<dyn Error + Send + Sync>> {
    let title = extract(html, "<title>", "</title>").ok_or("2chan public page lacks title")?;
    let (title, board_name) = title.rsplit_once(" - ")
        .ok_or("2chan public title lacks source board separator")?;
    let mut directory = Metadata::new();
    for (key, value) in [
        ("category", "2chan"), ("subcategory", "thread"),
        ("server", target.server.as_str()), ("board", target.board.as_str()),
        ("thread", target.id.as_str()),
    ] { directory.insert(key.into(), value.into()); }
    directory.insert("title".into(), text_only(title).into());
    directory.insert("board_name".into(), text_only(board_name).into());
    let posts = extract(html, "<div class=\"thre\"", "<div style=\"clear:left\"></div>")
        .ok_or("2chan thread lacks bounded public post area")?;
    let mut files = Vec::new();
    let mut seen = HashSet::new();
    for post in posts.split("<table border=0>") {
        let post_id = extract(post, "class=\"cno\">No.", "<").and_then(|x| x.parse::<u64>().ok());
        let post_name = extract(post, "class=\"cnm\">", "<").map(text_only);
        let post_date = extract(post, "class=\"cnw\">", "<").map(text_only);
        let comment = extract(post, "<blockquote", "</blockquote>")
            .and_then(|x| x.split_once('>').map(|(_, y)| text_only(y)));
        for link in post.split("<a href=\"").skip(1).take(64) {
            let Some((href, after)) = link.split_once('"') else { continue };
            if href.starts_with("/bin/jump") { continue }
            let Some((url, tim, ext)) = original(root, &target.board, href) else { continue };
            if !seen.insert(url.to_string()) { continue }
            if files.len() >= MAX_MEDIA { return Err("2chan thread exceeded public media cap".into()) }
            let item = MediaItem { url: url.to_string(), filename: format!("{tim}.{ext}"),
                extractor: "2chan", post_id };
            let mut metadata = media_metadata(&item);
            for (key, value) in &directory { metadata.insert(key.clone(), value.clone()); }
            metadata.insert("tim".into(), tim.clone().into());
            metadata.insert("time_source".into(), tim.get(..tim.len().saturating_sub(3)).unwrap_or("").into());
            metadata.insert("extension".into(), ext.into());
            if let Some(ref value) = post_name { metadata.insert("name".into(), value.clone().into()); }
            if let Some(ref value) = post_date { metadata.insert("now".into(), value.clone().into()); }
            if let Some(ref value) = comment { metadata.insert("com".into(), value.clone().into()); }
            if let Some(value) = extract(after, ">", "<").filter(|x| x.len() <= 240) {
                metadata.insert("source_attachment_text".into(), text_only(value).into());
            }
            files.push((item, metadata));
        }
    }
    Ok((directory, files))
}

async fn at_base(client: &Client, target: &Thread, base: &Url) -> Result<Records, Box<dyn Error + Send + Sync>> {
    let url = base.join(&format!("{}/res/{}.htm", target.board, target.id))?;
    let mut response = client.get(url.clone()).send().await?.error_for_status()?;
    if response.url().origin() != url.origin() {
        return Err("2chan thread redirect left first-party source origin".into());
    }
    if response.content_length().is_some_and(|n| n > MAX_HTML as u64) {
        return Err("2chan HTML exceeds size limit".into());
    }
    let mut content = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_HTML.saturating_sub(content.len()) {
            return Err("2chan HTML exceeds streamed size limit".into());
        }
        content.extend_from_slice(&chunk);
    }
    parse(std::str::from_utf8(&content)?, target, base)
}

pub async fn resolve(client: &Client, target: Thread) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse(&format!("https://{}.2chan.net/", target.server))?;
    Ok(at_base(client, &target, &base).await?.1.into_iter().map(|(item, _)| item).collect())
}

pub async fn resolve_messages(client: &Client, target: Thread)
    -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse(&format!("https://{}.2chan.net/", target.server))?;
    let (metadata, files) = at_base(client, &target, &base).await?;
    let mut output = vec![ExtractorMessage::Directory { metadata }];
    output.extend(files.into_iter().map(|(item, metadata)| ExtractorMessage::Url {
        url: item.url, metadata,
    }));
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::TcpListener};

    fn fixture() -> &'static str {
        r#"<title>Art &amp; discussion - Board</title><div class="thre"><table border=0><span class="cnm">Anon</span><span class="cno">No.12345</span><blockquote>Hello &amp; friend</blockquote><a href="/12/src/178000001.jpg" target="_blank">178000001.jpg</a><table border=0><span class="cnm">Reply</span><span class="cno">No.12346</span><a href="/12/src/178000002.png" target="_blank">178000002.png</a><div style="clear:left"></div>"#
    }

    #[test]
    fn only_trusted_public_thread_routes_are_recognized() {
        assert_eq!(match_url("dec.2chan.net/12/res/12345.htm"), Some(Thread {
            server:"dec".into(),board:"12".into(),id:"12345".into()
        }));
        for invalid in [
            "https://evil2chan.net/12/res/12345.htm",
            "https://dec.2chan.net/12/res/0.htm",
            "https://dec.2chan.net/12/res/12345.htm?private=1",
            "https://dec.2chan.net:8443/12/res/12345.htm",
            "https://user:pw@dec.2chan.net/12/res/12345.htm",
            "https://dec.2chan.net/12/../../secret",
        ] { assert!(match_url(invalid).is_none(), "{invalid}"); }
    }

    #[test]
    fn pinned_post_attachments_keep_original_names_and_post_provenance() {
        let target = match_url("https://dec.2chan.net/12/res/12345.htm").unwrap();
        let base = Url::parse("https://dec.2chan.net/").unwrap();
        let (dir, files) = parse(fixture(), &target, &base).unwrap();
        assert_eq!(dir["title"], "Art & discussion");
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].0.filename, "178000001.jpg");
        assert_eq!(files[0].0.url, "https://dec.2chan.net/12/src/178000001.jpg");
        assert_eq!(files[0].0.post_id, Some(12345));
        assert_eq!(files[0].1["com"], "Hello & friend");
        let forged = fixture().replace("/12/src/178000001.jpg", "https://evil.dec.2chan.net/12/src/178000001.jpg");
        assert_eq!(parse(&forged, &target, &base).unwrap().1.len(), 1);
    }

    #[tokio::test]
    async fn bounded_thread_uses_one_local_http_get_and_two_ordered_files() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 4096];
            let n = socket.read(&mut request).await.unwrap();
            assert!(String::from_utf8_lossy(&request[..n]).starts_with("GET /12/res/12345.htm HTTP/1.1"));
            let body = fixture();
            let response = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        let target = match_url("https://dec.2chan.net/12/res/12345.htm").unwrap();
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let (_, files) = at_base(&Client::new(), &target, &base).await.unwrap();
        assert_eq!(files.len(), 2);
        server.await.unwrap();
    }
}
