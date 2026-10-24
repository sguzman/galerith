//! Independent public Listal image and people-picture preview from pinned listal.py.
use crate::{
    message::{media_metadata, queued_url, ExtractorMessage, Metadata},
    safe_filename, MediaItem,
};
use reqwest::Client;
use std::{collections::HashSet, error::Error, time::Duration};
use url::Url;

const MAX_HTML: usize = 2 * 1024 * 1024;
const MAX_PAGES: u16 = 4;
const MAX_CHILDREN: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Image(String),
    People(String),
}

fn valid_id(s: &str) -> bool {
    (1..=16).contains(&s.len())
        && s.bytes().all(|b| b.is_ascii_digit())
        && s.parse::<u64>().ok().is_some_and(|n| n > 0)
}

fn valid_people(s: &str) -> bool {
    (1..=96).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

pub fn match_url(raw: &str) -> Option<Target> {
    let lower = raw.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "/../", "/./"]
        .iter()
        .any(|term| lower.contains(term))
    {
        return None;
    }
    let url = Url::parse(raw).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !matches!(url.host_str(), Some("listal.com" | "www.listal.com"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let segments: Vec<_> = url.path_segments()?.collect();
    match segments.as_slice() {
        ["viewimage", id] | ["viewimage", id, ""] if valid_id(id) => {
            Some(Target::Image((*id).to_owned()))
        }
        [person, "pictures"] | [person, "pictures", ""] if valid_people(person) => {
            Some(Target::People((*person).to_owned()))
        }
        _ => None,
    }
}

fn media_url(source: &str) -> Option<(Url, String)> {
    let mut url = if source.starts_with("//") {
        Url::parse(&format!("https:{source}")).ok()?
    } else {
        Url::parse(source).ok()?
    };
    if url.scheme() == "http" {
        url.set_scheme("https").ok()?;
    }
    let host = url.host_str()?;
    let trusted = matches!(host, "listal.com" | "lisimg.com")
        || host.ends_with(".listal.com")
        || host.ends_with(".lisimg.com");
    if !trusted
        || url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path().contains('%')
    {
        return None;
    }
    let basename = url.path_segments()?.next_back()?;
    let ext = basename.rsplit_once('.')?.1.to_ascii_lowercase();
    if !matches!(
        ext.as_str(),
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif"
    ) {
        return None;
    }
    let filename = safe_filename(basename);
    if filename.is_empty() || matches!(filename.as_str(), "." | "..") {
        return None;
    }
    Some((url, filename))
}

fn capture<'a>(source: &'a str, start: &str, end: char) -> Option<&'a str> {
    source
        .split_once(start)?
        .1
        .split_once(end)
        .map(|(value, _)| value)
}

/// Parse the pinned Listal "<day> <English month> <year> <HH:MM>"
/// timestamp as a timezone-free local date. Never invent a UTC offset.
fn parse_source_date(raw: &str) -> Option<String> {
    let mut words = raw.split_whitespace();
    let day = words.next()?;
    let month = words.next()?;
    let year = words.next()?;
    let clock = words.next()?;
    if words.next().is_some() || year.len() != 4 {
        return None;
    }
    let day: u32 = day.parse().ok()?;
    let year: u32 = year.parse().ok()?;
    if !(1..=9999).contains(&year) {
        return None;
    }
    let month = match month {
        "January" => 1,
        "February" => 2,
        "March" => 3,
        "April" => 4,
        "May" => 5,
        "June" => 6,
        "July" => 7,
        "August" => 8,
        "September" => 9,
        "October" => 10,
        "November" => 11,
        "December" => 12,
        _ => return None,
    };
    let mut hm = clock.split(':');
    let hours: u32 = hm.next()?.parse().ok()?;
    let minutes: u32 = hm.next()?.parse().ok()?;
    if hm.next().is_some() || hours > 23 || minutes > 59 {
        return None;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let max_day = match month {
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => 31,
    };
    if day == 0 || day > max_day {
        return None;
    }
    Some(format!(
        "{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:00"
    ))
}

fn parse_image(
    html: &str,
    id: &str,
) -> Result<(MediaItem, Metadata), Box<dyn Error + Send + Sync>> {
    let source = capture(html, "<div><center><img src='", '\'')
        .ok_or("Listal public image page missing original media anchor")?;
    let (url, source_name) =
        media_url(source).ok_or("Listal original media URL is not a trusted image")?;
    let item = MediaItem {
        url: url.to_string(),
        filename: format!("{id}_{source_name}"),
        extractor: "listal",
        post_id: None,
    };
    let mut metadata = media_metadata(&item);
    metadata.insert("subcategory".into(), "image".into());
    metadata.insert("image_id".into(), id.parse::<u64>()?.into());
    metadata.insert(
        "post_url".into(),
        format!("https://www.listal.com/viewimage/{id}h").into(),
    );
    if let Some(title) = capture(html, "title=\"", '"')
        .filter(|text| text.len() <= 200 && !text.chars().any(char::is_control))
    {
        metadata.insert("title".into(), title.replace("&amp;", "&").into());
    }
    for (key, start) in [("width", "width='"), ("height", "height='")] {
        if let Some(value) = capture(html, start, '\'') {
            if let Ok(number) = value.parse::<u32>() {
                if (1..=50000).contains(&number) {
                    metadata.insert(key.into(), number.into());
                }
            }
        }
    }
    if let Some(author_part) = html.split_once("Added by <a href='").map(|(_, tail)| tail) {
        if let Some(path) = author_part
            .split_once('\'')
            .map(|(path, _)| path)
            .filter(|path| path.starts_with('/') && path.len() <= 100 && !path.contains(".."))
        {
            metadata.insert(
                "author_url".into(),
                format!("https://www.listal.com{path}").into(),
            );
        }
        if let Some(name) = author_part
            .split_once('>')
            .and_then(|(_, tail)| tail.split_once('<'))
            .map(|(text, _)| text)
            .filter(|text| text.len() <= 120 && !text.chars().any(char::is_control))
        {
            metadata.insert("author".into(), name.replace("&amp;", "&").into());
        }
    }
    if let Some(date) = capture(html, " ago on ", '<')
        .filter(|text| text.len() <= 90 && !text.chars().any(char::is_control))
    {
        metadata.insert("date_source".into(), date.into());
        if let Some(parsed) = parse_source_date(date) {
            metadata.insert("date".into(), parsed.into());
        }
    }
    Ok((item, metadata))
}

/// Scope HTML links to parsed first-party Listal URLs. Domain-name substring
/// matching would incorrectly accept lookalike hosts.
fn public_anchor(attrs: &str) -> Option<Url> {
    let raw = attrs
        .split_once("href='")
        .and_then(|(_, suffix)| suffix.split_once('\''))
        .or_else(|| {
            attrs
                .split_once("href=\"")
                .and_then(|(_, suffix)| suffix.split_once('"'))
        })
        .map(|(href, _)| href)?;
    let lower = raw.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c"]
        .iter()
        .any(|needle| lower.contains(needle))
    {
        return None;
    }
    let base = Url::parse("https://www.listal.com/").ok()?;
    let url = base.join(raw).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !matches!(url.host_str(), Some("listal.com" | "www.listal.com"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    Some(url)
}

fn listing_ids(
    html: &str,
    found: &mut HashSet<String>,
    out: &mut Vec<ExtractorMessage>,
    person: &str,
) {
    for anchor in html.split("<a ").skip(1).take(4000) {
        if out.len() >= MAX_CHILDREN {
            break;
        }
        let Some((attrs, _)) = anchor.split_once('>') else {
            continue;
        };
        let Some(url) = public_anchor(attrs) else {
            continue;
        };
        let Some(parts) = url.path_segments() else {
            continue;
        };
        let segments: Vec<_> = parts.collect();
        let ["viewimage", id] = segments.as_slice() else {
            continue;
        };
        if !valid_id(id) || !found.insert((*id).to_owned()) {
            continue;
        }
        let mut meta = Metadata::new();
        meta.insert("category".into(), "listal".into());
        meta.insert("subcategory".into(), "people".into());
        meta.insert("person".into(), person.into());
        meta.insert("image_id".into(), id.parse::<u64>().unwrap().into());
        out.push(queued_url(
            format!("https://www.listal.com/viewimage/{id}"),
            meta,
        ));
    }
}

fn next_page(html: &str, person: &str, current: u16) -> bool {
    let expected_path = format!("/{person}/pictures/{}", current + 1);
    for anchor in html.split("<a ").skip(1).take(4000) {
        let Some((attrs, body)) = anchor.split_once('>') else {
            continue;
        };
        if !(attrs.contains("nextprev") || body.trim_start().starts_with("Next")) {
            continue;
        }
        if let Some(url) = public_anchor(attrs) {
            if url.path() == expected_path {
                return true;
            }
        }
    }
    false
}

async fn read_html(client: &Client, endpoint: Url) -> Result<String, Box<dyn Error + Send + Sync>> {
    let mut response = client
        .get(endpoint.clone())
        .send()
        .await?
        .error_for_status()?;
    if response.url().origin() != endpoint.origin() {
        return Err("Listal public page redirected outside its origin".into());
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_HTML as u64)
    {
        return Err("Listal public HTML exceeds size bound".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_HTML.saturating_sub(bytes.len()) {
            return Err("Listal public HTML exceeds size bound".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8(bytes)?)
}

async fn image_at_base(
    client: &Client,
    base: &Url,
    id: &str,
) -> Result<(MediaItem, Metadata), Box<dyn Error + Send + Sync>> {
    let page = read_html(client, base.join(&format!("viewimage/{id}h"))?).await?;
    parse_image(&page, id)
}

async fn people_at_base(
    client: &Client,
    base: &Url,
    person: &str,
    max_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for page in 1..=max_pages.clamp(1, MAX_PAGES) {
        if page > 1 {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        let path = if page == 1 {
            format!("{person}/pictures")
        } else {
            format!("{person}/pictures/{page}")
        };
        let body = read_html(client, base.join(&path)?).await?;
        listing_ids(&body, &mut seen, &mut result, person);
        if result.len() >= MAX_CHILDREN || page == MAX_PAGES || !next_page(&body, person, page) {
            break;
        }
    }
    Ok(result)
}

pub async fn resolve(
    client: &Client,
    target: Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse("https://www.listal.com/")?;
    match target {
        Target::Image(id) => Ok(vec![image_at_base(client, &base, &id).await?.0]),
        Target::People(_) => {
            Err("Listal people collections require --dump-messages or --follow-queues".into())
        }
    }
}

pub async fn resolve_messages(
    client: &Client,
    target: Target,
    max_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse("https://www.listal.com/")?;
    match target {
        Target::Image(id) => {
            let (item, metadata) = image_at_base(client, &base, &id).await?;
            Ok(vec![
                ExtractorMessage::Directory {
                    metadata: metadata.clone(),
                },
                ExtractorMessage::Url {
                    url: item.url,
                    metadata,
                },
            ])
        }
        Target::People(person) => people_at_base(client, &base, &person, max_pages).await,
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
    fn pinned_public_url_families_validate_id_and_people_slug() {
        assert_eq!(
            match_url("https://www.listal.com/viewimage/12345678"),
            Some(Target::Image("12345678".into()))
        );
        assert_eq!(
            match_url("https://listal.com/alice/pictures"),
            Some(Target::People("alice".into()))
        );
        for bad in [
            "https://evillistal.com/viewimage/12345678",
            "https://listal.com/viewimage/0",
            "https://listal.com/viewimage/notanid",
            "https://listal.com/alice/pictures?secret=1",
            "https://name:password@listal.com/viewimage/123",
            "https://listal.com/alice/../../private",
            "https://listal.com:8443/viewimage/123",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn single_original_preserves_source_image_dimensions_and_author() {
        let html = r#"<div><center><img src='https://image.listal.com/pics/full.png' title="Portrait" width='800' height='1200'></center></div>Added by <a href='/authors/sam'>Sam</a> ago on 12 April 2020 16:30<"#;
        let (media, meta) = parse_image(html, "12345678").unwrap();
        assert_eq!(media.url, "https://image.listal.com/pics/full.png");
        assert_eq!(media.filename, "12345678_full.png");
        assert_eq!(meta["image_id"], 12345678);
        assert_eq!(meta["title"], "Portrait");
        assert_eq!(meta["width"], 800);
        assert_eq!(meta["author"], "Sam");
        assert_eq!(meta["date_source"], "12 April 2020 16:30");
        assert_eq!(meta["date"], "2020-04-12T16:30:00");
        assert!(media.post_id.is_none());
        let forged = html.replace("image.listal.com", "evil-listal.com");
        assert!(parse_image(&forged, "12345678").is_err());
    }

    #[test]
    fn source_date_parser_rejects_impossible_calendar_dates_and_offsets() {
        assert_eq!(
            parse_source_date("29 February 2024 23:59"),
            Some("2024-02-29T23:59:00".into())
        );
        assert_eq!(
            parse_source_date("29 February 2000 00:00"),
            Some("2000-02-29T00:00:00".into())
        );
        for bad in [
            "29 February 2023 12:00",
            "29 February 1900 12:00",
            "31 April 2026 12:00",
            "0 December 2026 12:00",
            "10 January 2026 24:00",
            "10 January 2026 20:60",
            "10 Unknown 2026 12:00",
            "10 January 2026 12:00 UTC",
            "10 January 0000 12:00",
        ] {
            assert_eq!(parse_source_date(bad), None, "{bad}");
        }
    }

    #[test]
    fn people_images_queue_first_seen_numeric_ids_without_external_links() {
        let mut seen = HashSet::new();
        let mut output = Vec::new();
        listing_ids(
            r#"<a href='https://www.listal.com/viewimage/1234'>A</a><a href='https://www.listal.com/viewimage/1234'>Again</a><a href='http://www.listal.com/viewimage/5678'>B</a>"#,
            &mut seen,
            &mut output,
            "alice",
        );
        assert_eq!(output.len(), 2);
        assert_eq!(
            serde_json::to_value(&output[1]).unwrap()["url"],
            "https://www.listal.com/viewimage/5678"
        );
        listing_ids(
            "<a href='https://evil-listal.com/viewimage/9999'>forged</a><a href='https://www.listal.com.evil.test/viewimage/8888'>forged</a>",
            &mut seen,
            &mut output,
            "alice",
        );
        assert_eq!(output.len(), 2);
        assert!(next_page(
            "<a href='/alice/pictures/2'>Next</a>",
            "alice",
            1
        ));
        assert!(!next_page(
            "<a href='https://foreign.test/alice/pictures/2'>No</a>",
            "alice",
            1
        ));
        assert!(!next_page(
            "<a href='https://foreign.test/alice/pictures/2'>Next</a>",
            "alice",
            1
        ));
    }

    #[tokio::test]
    async fn bounded_people_listing_follows_valid_next_page_and_deduplicates_ids() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (path, html) in [
                (
                    "/alice/pictures",
                    "<a href='https://www.listal.com/viewimage/1234'>First</a><a href='/alice/pictures/2'>Next</a>",
                ),
                (
                    "/alice/pictures/2",
                    "<a href='https://www.listal.com/viewimage/1234'>Dup</a><a href='https://www.listal.com/viewimage/5678'>Second</a><a href='https://evil-listal.com/viewimage/9999'>Bad</a>",
                ),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0u8; 4096];
                let n = socket.read(&mut request).await.unwrap();
                assert!(
                    String::from_utf8_lossy(&request[..n])
                        .starts_with(&format!("GET {path} HTTP/1.1"))
                );
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
                    html.len()
                );
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let events = people_at_base(&Client::new(), &base, "alice", 4)
            .await
            .unwrap();
        assert_eq!(events.len(), 2);
        let urls = events
            .iter()
            .map(|event| {
                serde_json::to_value(event).unwrap()["url"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            urls,
            [
                "https://www.listal.com/viewimage/1234",
                "https://www.listal.com/viewimage/5678"
            ]
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn bounded_public_image_get_uses_one_source_request() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let n = socket.read(&mut buf).await.unwrap();
            assert!(String::from_utf8_lossy(&buf[..n]).starts_with("GET /viewimage/1234h HTTP/1.1"));
            let html =
                r#"<div><center><img src='https://image.listal.com/pics/full.jpg'></center></div>"#;
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
                html.len()
            );
            socket.write_all(reply.as_bytes()).await.unwrap();
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let record = image_at_base(&Client::new(), &base, "1234").await.unwrap();
        assert_eq!(record.0.filename, "1234_full.jpg");
        server.await.unwrap();
    }
}
