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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Navigation {
    Linked,
    NextId,
    Needle(&'static str),
    Lastblood,
    Brawl,
}

fn href_before<'a>(html: &'a str, needle: &str) -> Option<&'a str> {
    let (prefix, _) = html.split_once(needle)?;
    let attrs = prefix.rsplit_once("<a ")?.1;
    snippet(attrs, "href=\"", "\"")
}

fn href_after<'a>(html: &'a str, needle: &str) -> Option<&'a str> {
    let (_, trailing) = html.split_once(needle)?;
    snippet(trailing, "href=\"", "\"")
}

fn comic_start(html: &str, comic: &Comic) -> Option<(String, Navigation, &'static str)> {
    if comic.comic == "brawlinthefamily" {
        return Some((
            "/comic/theshowdown/".to_owned(),
            Navigation::Brawl,
            "<div id=\"comic\">",
        ));
    }
    if let Some(first) = source_link(html, true) {
        return Some((
            first.to_owned(),
            if comic.comic == "porcelain" {
                Navigation::Needle("id=\"porArchivetop_\"")
            } else {
                Navigation::Linked
            },
            "class=\"ksc\"",
        ));
    }
    if let Some(first) = href_before(html, "id=\"first_day1\"") {
        return Some((first.to_owned(), Navigation::NextId, "class=\"ksc\""));
    }
    if let Some(first) = href_before(html, ">FIRST PAGE<") {
        let special = comic.comic == "lastblood";
        return Some((
            first.to_owned(),
            if special {
                Navigation::Lastblood
            } else {
                Navigation::NextId
            },
            if special {
                "<div id=\"comic\">"
            } else {
                "class=\"ksc\""
            },
        ));
    }
    if let Some(first) = href_after(html, "<div id=\"kscomicpart\"") {
        return Some((
            first.to_owned(),
            Navigation::Needle("<a href=\"/archive.html"),
            "class=\"ksc\"",
        ));
    }
    if let Some(first) = href_before(html, ">First Comic<") {
        return Some((
            first.to_owned(),
            Navigation::Needle("class=\"navarchive\""),
            "</header>",
        ));
    }
    if let Some(first) = href_before(html, "id=\"flip_FirstDay\"") {
        return Some((
            first.to_owned(),
            Navigation::Needle("id=\"flip_ArcButton\""),
            "class=\"flip_Pages ksc\"",
        ));
    }
    None
}

fn comic_next<'a>(html: &'a str, mode: Navigation) -> Option<&'a str> {
    match mode {
        Navigation::Linked => source_link(html, false),
        Navigation::NextId => href_before(html, "id=\"next_"),
        Navigation::Needle(needle) => href_after(html, needle),
        Navigation::Lastblood => {
            let (_, tail) = html.split_once("link rel='next'")?;
            snippet(tail, "href='", "'")
        }
        Navigation::Brawl => {
            href_before(html, "comic-nav-next").filter(|href| !href.contains("?random"))
        }
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
    marker: &str,
    seen: &mut HashSet<String>,
    result: &mut Vec<(MediaItem, Metadata)>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    for image in html.split(marker).skip(1).take(500) {
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
    let choice = comic_start(&initial, comic);
    let (first, navigator, marker) =
        choice.unwrap_or((String::new(), Navigation::Linked, "class=\"ksc\""));
    let start = if let Some(path) = &comic.path {
        scoped_page(base, path)
    } else {
        scoped_page(base, &first)
    }
    .filter(|_| !first.is_empty() || comic.path.is_some())
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
        page_records(&content, comic, &page, marker, &mut originals, &mut files)?;
        page_count += 1;
        current = match comic_next(&content, navigator) {
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
        page_records(html, &comic, &page, "class=\"ksc\"", &mut seen, &mut result).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].0.filename, "story_001_page one.png");
        assert_eq!(result[1].1["comic"], "story");
        assert_eq!(
            result[0].1["page_url"],
            "https://story.keenspot.com/comic/one/"
        );
    }

    #[test]
    fn pinned_alternate_first_and_next_navigators_are_selected() {
        let sample = sample_comic();
        let (first, mode, marker) = comic_start(
            r#"<a href="/comic/first/" id="first_day1">First</a>"#,
            &sample,
        )
        .unwrap();
        assert_eq!(first, "/comic/first/");
        assert_eq!(mode, Navigation::NextId);
        assert_eq!(marker, "class=\"ksc\"");
        assert_eq!(
            comic_next(r#"<a href="/comic/second/" id="next_day">Next</a>"#, mode),
            Some("/comic/second/")
        );
        let porcelain = match_url("https://porcelain.keenspot.com/").unwrap();
        let (_, mode, _) =
            comic_start(r#"<link rel="first" href="/comic/first/">"#, &porcelain).unwrap();
        assert_eq!(mode, Navigation::Needle("id=\"porArchivetop_\""));
        assert_eq!(
            comic_next(
                r#"<div id="porArchivetop_"><a href="/comic/next/">More</a>"#,
                mode
            ),
            Some("/comic/next/")
        );
        let lastblood = match_url("https://lastblood.keenspot.com/").unwrap();
        let (_, mode, marker) =
            comic_start(r#"<a href="/comic/first/">FIRST PAGE</a>"#, &lastblood).unwrap();
        assert_eq!(mode, Navigation::Lastblood);
        assert_eq!(marker, "<div id=\"comic\">");
        assert_eq!(
            comic_next(r#"<link rel='next' href='/comic/two/'>"#, mode),
            Some("/comic/two/")
        );
        let brawl = match_url("https://brawlinthefamily.keenspot.com/").unwrap();
        let (first, mode, _) = comic_start("", &brawl).unwrap();
        assert_eq!(first, "/comic/theshowdown/");
        assert_eq!(
            comic_next(r#"<a href="/comic/next/" class="comic-nav-next">"#, mode),
            Some("/comic/next/")
        );
        assert!(comic_next(
            r#"<a href="/comic/next/?random" class="comic-nav-next">"#,
            mode
        )
        .is_none());
        let twokinds = match_url("https://twokinds.keenspot.com/").unwrap();
        let (_, nav, marker) =
            comic_start(r#"<a href="/comic/first/">First Comic</a>"#, &twokinds).unwrap();
        assert_eq!(nav, Navigation::Needle("class=\"navarchive\""));
        assert_eq!(marker, "</header>");
        let flip = match_url("https://flipside.keenspot.com/").unwrap();
        let (_, nav, marker) = comic_start(
            r#"<a href="/comic/first/" id="flip_FirstDay">First</a>"#,
            &flip,
        )
        .unwrap();
        assert_eq!(nav, Navigation::Needle("id=\"flip_ArcButton\""));
        assert_eq!(marker, "class=\"flip_Pages ksc\"");
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
