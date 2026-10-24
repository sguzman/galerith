//! Guarded public Keenspot webcomic preview against pinned keenspot.py.
//! This covers standard first/next-link pages; quirky site-specific
//! navigators and embedded video delegation remain explicitly unsupported.
use crate::{
    message::{media_metadata, ExtractorMessage, Metadata},
    safe_filename, MediaItem,
};
use percent_encoding::percent_decode_str;
use reqwest::Client;
use std::{collections::HashSet, error::Error, time::Duration};
use url::Url;

const MAX_HTML: usize = 2 * 1024 * 1024;
const MAX_COMIC_PAGES: usize = 32;
const MAX_MEDIA: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comic {
    host: String,
    comic: String,
    path: Option<String>,
}

pub fn match_url(raw: &str) -> Option<Comic> {
    let lower = raw.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "/../", "/./"]
        .iter()
        .any(|term| lower.contains(term))
    {
        return None;
    }
    let url = Url::parse(raw).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let host = url.host_str()?;
    let comic = host.strip_suffix(".keenspot.com")?;
    if matches!(comic, "www" | "forums")
        || !(1..=64).contains(&comic.len())
        || !comic
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return None;
    }
    let path = match url.path() {
        "/" => None,
        value if value.len() <= 240 && value.starts_with('/') => Some(value.to_owned()),
        _ => return None,
    };
    Some(Comic {
        host: host.to_owned(),
        comic: comic.to_owned(),
        path,
    })
}

fn snippet<'a>(text: &'a str, before: &str, after: &str) -> Option<&'a str> {
    text.split_once(before)?
        .1
        .split_once(after)
        .map(|(found, _)| found)
}

fn source_link(html: &str, first: bool) -> Option<&str> {
    if first {
        snippet(html, "<link rel=\"first\" href=\"", "\"")
    } else {
        snippet(html, "<link rel=\"next\" href=\"", "\"")
    }
}

fn scoped_page(base: &Url, raw: &str) -> Option<Url> {
    let url = base.join(raw).ok()?;
    if url.origin() != base.origin()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path().contains('%')
        || url
            .path_segments()?
            .any(|piece| piece == "." || piece == "..")
    {
        return None;
    }
    Some(url)
}

fn original_image(page_url: &Url, raw: &str) -> Option<(Url, String)> {
    let mut url = page_url.join(raw).ok()?;
    if url.scheme() == "http" {
        url.set_scheme("https").ok()?;
    }
    let host = url.host_str()?;
    if url.scheme() != "https"
        || !(host == "keenspot.com" || host.ends_with(".keenspot.com"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let mut basename = None;
    for part in url.path_segments()? {
        let decoded = percent_decode_str(part).decode_utf8().ok()?;
        if decoded == "."
            || decoded == ".."
            || decoded.contains('/')
            || decoded.contains('\\')
            || decoded.chars().any(char::is_control)
        {
            return None;
        }
        basename = Some(decoded.into_owned());
    }
    let filename = basename?;
    let extension = filename.rsplit_once('.')?.1.to_ascii_lowercase();
    if !matches!(
        extension.as_str(),
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif"
    ) {
        return None;
    }
    let name = safe_filename(&filename);
    (!name.is_empty() && !matches!(name.as_str(), "." | "..")).then_some((url, name))
}

fn page_records(
    html: &str,
    comic: &Comic,
    page: &Url,
    seen: &mut HashSet<String>,
    result: &mut Vec<(MediaItem, Metadata)>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    for image in html.split("class=\"ksc\"").skip(1).take(500) {
        let Some(src) = snippet(image, "src=\"", "\"") else {
            continue;
        };
        if src.ends_with(".js") {
            continue;
        }
        let Some((url, name)) = original_image(page, src) else {
            continue;
        };
        let canonical = url.to_string();
        if !seen.insert(canonical.clone()) {
            continue;
        }
        if result.len() >= MAX_MEDIA {
            return Err("Keenspot public comic exceeds bounded media count".into());
        }
        let ordinal = result.len() + 1;
        let item = MediaItem {
            url: canonical,
            filename: format!("{}_{ordinal:03}_{name}", comic.comic),
            extractor: "keenspot",
            post_id: None,
        };
        let mut metadata = media_metadata(&item);
        metadata.insert("subcategory".into(), "comic".into());
        metadata.insert("comic".into(), comic.comic.clone().into());
        metadata.insert("num".into(), ordinal.into());
        metadata.insert("page_url".into(), page.to_string().into());
        result.push((item, metadata));
    }
    Ok(())
}

async fn html(
    client: &Client,
    page: Url,
    base: &Url,
) -> Result<String, Box<dyn Error + Send + Sync>> {
    let mut response = client.get(page).send().await?.error_for_status()?;
    if response.url().origin() != base.origin() {
        return Err("Keenspot page redirected outside the comic origin".into());
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_HTML as u64)
    {
        return Err("Keenspot source HTML exceeds bounded response".into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_HTML.saturating_sub(body.len()) {
            return Err("Keenspot source HTML exceeds bounded response".into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8(body)?)
}

async fn at_base(
    client: &Client,
    comic: &Comic,
    base: &Url,
) -> Result<Vec<(MediaItem, Metadata)>, Box<dyn Error + Send + Sync>> {
    let initial = html(client, base.clone(), base).await?;
    let start = if let Some(path) = &comic.path {
        scoped_page(base, path)
    } else {
        source_link(&initial, true).and_then(|href| scoped_page(base, href))
    }
    .ok_or("Keenspot root lacks a supported public first-comic link")?;
    let mut current = Some(start);
    let mut pages = HashSet::new();
    let mut originals = HashSet::new();
    let mut files = Vec::new();
    let mut page_count = 0;
    while let Some(page) = current {
        if !pages.insert(page.to_string()) {
            break;
        }
        if page_count >= MAX_COMIC_PAGES {
            return Err("Keenspot public comic exceeds bounded page traversal".into());
        }
        if page_count > 0 {
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        let content = html(client, page.clone(), base).await?;
        page_records(&content, comic, &page, &mut originals, &mut files)?;
        page_count += 1;
        current = match source_link(&content, false) {
            Some(next) => Some(
                scoped_page(base, next)
                    .ok_or("Keenspot next-comic link escaped the source origin")?,
            ),
            None => None,
        };
    }
    if files.is_empty() {
        return Err("Keenspot comic exposes no supported public originals".into());
    }
    Ok(files)
}

pub async fn resolve(
    client: &Client,
    comic: Comic,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse(&format!("https://{}/", comic.host))?;
    Ok(at_base(client, &comic, &base)
        .await?
        .into_iter()
        .map(|(item, _)| item)
        .collect())
}

pub async fn resolve_messages(
    client: &Client,
    comic: Comic,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse(&format!("https://{}/", comic.host))?;
    let mut events = Vec::new();
    for (item, metadata) in at_base(client, &comic, &base).await? {
        events.push(ExtractorMessage::Directory {
            metadata: metadata.clone(),
        });
        events.push(ExtractorMessage::Url {
            url: item.url,
            metadata,
        });
    }
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    fn sample_comic() -> Comic {
        match_url("https://story.keenspot.com/").unwrap()
    }

    #[test]
    fn recognized_subdomains_exclude_forums_and_untrusted_paths() {
        assert_eq!(sample_comic().comic, "story");
        assert!(match_url("https://story.keenspot.com/comic/1/")
            .unwrap()
            .path
            .is_some());
        for bad in [
            "https://www.keenspot.com/",
            "https://forums.keenspot.com/",
            "https://evilkeenspot.com/",
            "https://story.keenspot.com/comic/../../private",
            "https://story.keenspot.com:8443/",
            "https://user:pass@story.keenspot.com/",
            "https://story.keenspot.com/?random",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn comic_page_extracts_first_party_originals_and_source_metadata() {
        let comic = sample_comic();
        let page = Url::parse("https://story.keenspot.com/comic/one/").unwrap();
        let html = r#"<img class="ksc" src="/images/page%20one.png"><img class="ksc" src="https://static.keenspot.com/assets/two.jpg"><img class="ksc" src="https://evil-keenspot.com/three.jpg"><img class="ksc" src="/scripts/skip.js">"#;
        let mut seen = HashSet::new();
        let mut result = Vec::new();
        page_records(html, &comic, &page, &mut seen, &mut result).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].0.filename, "story_001_page one.png");
        assert_eq!(result[1].1["comic"], "story");
        assert_eq!(
            result[0].1["page_url"],
            "https://story.keenspot.com/comic/one/"
        );
    }

    #[tokio::test]
    async fn bounded_two_comic_page_traversal_stays_on_origin_and_keeps_order() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (path, body) in [
                ("/", r#"<link rel="first" href="/comic/one/">"#),
                (
                    "/comic/one/",
                    r#"<img class="ksc" src="https://images.keenspot.com/1.png"><link rel="next" href="/comic/two/">"#,
                ),
                (
                    "/comic/two/",
                    r#"<img class="ksc" src="https://images.keenspot.com/2.png">"#,
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
        let results = at_base(&Client::new(), &sample_comic(), &base)
            .await
            .unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0.url, "https://images.keenspot.com/1.png");
        assert_eq!(results[1].0.filename, "story_002_2.png");
        server.await.unwrap();
    }

    #[test]
    fn next_comic_urls_reject_outside_origins_and_bad_queries() {
        let base = Url::parse("https://story.keenspot.com/").unwrap();
        assert!(scoped_page(&base, "/comic/one/").is_some());
        assert!(scoped_page(&base, "https://evil-keenspot.com/comic/one/").is_none());
        assert!(scoped_page(&base, "/comic/one/?private=1").is_none());
    }
}
