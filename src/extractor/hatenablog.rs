//! Independent guarded public Hatena Blog article images and archive discovery.
//! Only source-backed public entry, home, archive and search URLs are supported.
use crate::{
    message::{media_metadata, queued_url, ExtractorMessage, Metadata},
    safe_filename, MediaItem,
};
use reqwest::Client;
use std::{collections::HashSet, error::Error, time::Duration};
use url::Url;

const MAX_HTML: usize = 2 * 1024 * 1024;
const MAX_PAGES: u16 = 4;
const MAX_MEDIA: usize = 160;
const MAX_CHILDREN: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Entry {
        host: String,
        path: String,
    },
    Listing {
        host: String,
        path: String,
        query: Option<String>,
    },
}

fn safe_blog_host(host: &str) -> bool {
    [
        "hatenablog.com",
        "hatenablog.jp",
        "hatenadiary.com",
        "hateblo.jp",
    ]
    .iter()
    .any(|suffix| {
        host.strip_suffix(&format!(".{suffix}"))
            .is_some_and(|name| {
                (1..=64).contains(&name.len())
                    && name
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            })
    })
}

fn safe_entry_path(path: &str) -> bool {
    path.starts_with("/entry/")
        && path.len() <= 240
        && !path.contains('%')
        && !path.contains("..")
        && path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'_'))
}

fn safe_archive_path(path: &str) -> bool {
    if path == "/" || path == "/archive" || path == "/search" {
        return true;
    }
    if let Some(suffix) = path.strip_prefix("/archive/") {
        if let Some(category) = suffix.strip_prefix("category/") {
            return (1..=96).contains(&category.len())
                && category
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        }
        let parts = suffix.split('/').collect::<Vec<_>>();
        return (1..=3).contains(&parts.len())
            && parts
                .iter()
                .all(|p| (1..=4).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_digit()));
    }
    false
}

pub fn match_url(raw: &str) -> Option<Target> {
    let lower = raw.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "/../", "/./"]
        .iter()
        .any(|v| lower.contains(v))
        || lower.ends_with("/..")
        || lower.ends_with("/.")
    {
        return None;
    }
    let url = Url::parse(raw).ok()?;
    let host = url.host_str()?;
    if !matches!(url.scheme(), "https" | "http")
        || !safe_blog_host(host)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let path = url.path();
    if safe_entry_path(path) && url.query().is_none() {
        return Some(Target::Entry {
            host: host.to_owned(),
            path: path.to_owned(),
        });
    }
    if !safe_archive_path(path) {
        return None;
    }
    if let Some(query) = url.query() {
        if query.len() > 180 {
            return None;
        }
        let pairs: Vec<_> = url.query_pairs().collect();
        if pairs.len() > 2
            || pairs.is_empty()
            || pairs.iter().any(|(key, value)| {
                !matches!(key.as_ref(), "q" | "page") || value.is_empty() || value.len() > 120
            })
            || pairs.iter().filter(|(k, _)| k == "page").count() > 1
            || pairs.iter().filter(|(k, _)| k == "q").count() > 1
            || (path != "/search" && pairs.iter().any(|(k, _)| k == "q"))
            || pairs
                .iter()
                .any(|(k, v)| k == "page" && !v.bytes().all(|b| b.is_ascii_digit()))
        {
            return None;
        }
    }
    Some(Target::Listing {
        host: host.to_owned(),
        path: path.to_owned(),
        query: url.query().map(ToOwned::to_owned),
    })
}

fn text_between<'a>(s: &'a str, start: &str, end: &str) -> Option<&'a str> {
    s.split_once(start)?.1.split_once(end).map(|(text, _)| text)
}

fn decode_text(raw: &str) -> Option<String> {
    if raw.len() > 240 || raw.chars().any(char::is_control) || raw.contains('<') {
        return None;
    }
    Some(
        raw.replace("&amp;", "&")
            .replace("&quot;", "\"")
            .replace("&#39;", "'"),
    )
}

fn original(raw: &str, base: &Url) -> Option<(Url, String)> {
    let mut url = base.join(raw).ok()?;
    if url.scheme() == "http" {
        url.set_scheme("https").ok()?;
    }
    let host = url.host_str()?;
    if !(host.ends_with(".st-hatena.com") || host == "f.hatena.ne.jp")
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
    let name = url.path_segments()?.next_back()?;
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    if !matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "gif" | "webp") {
        return None;
    }
    let name = safe_filename(name);
    (!name.is_empty()).then_some((url, name))
}

fn source_entry_url(host: &str, raw: &str) -> Option<Url> {
    // Validate the unnormalized input too: Url::join would remove raw
    // dot-segments before the canonical entry-path check.
    let lower = raw.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "/../", "/./"]
        .iter()
        .any(|part| lower.contains(part))
        || lower.ends_with("/.")
        || lower.ends_with("/..")
    {
        return None;
    }
    let base = Url::parse(&format!("https://{host}/")).ok()?;
    let url = base.join(raw).ok()?;
    (url.origin() == base.origin()
        && safe_entry_path(url.path())
        && url.query().is_none()
        && url.fragment().is_none()
        && url.username().is_empty()
        && url.password().is_none())
    .then_some(url)
}

/// Preserve source RFC3339 timezone information, but never normalize
/// an impossible Gregorian calendar date or fabricate a missing zone.
fn source_iso_date(raw: &str) -> Option<String> {
    if raw.len() < 20 || raw.len() > 40 || !raw.is_ascii() {
        return None;
    }
    let (date, clock_and_zone) = raw.split_once('T')?;
    let date_parts = date.split('-').collect::<Vec<_>>();
    let [year, month, day] = date_parts.as_slice() else {
        return None;
    };
    if year.len() != 4 || month.len() != 2 || day.len() != 2 {
        return None;
    }
    let year = year.parse::<u32>().ok()?;
    let month = month.parse::<u32>().ok()?;
    let day = day.parse::<u32>().ok()?;
    if year == 0 || month == 0 || month > 12 {
        return None;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if day == 0 || day > days {
        return None;
    }
    let (clock, zone) = if let Some(time) = clock_and_zone.strip_suffix('Z') {
        (time, "Z")
    } else {
        let index = clock_and_zone.find(['+', '-'])?;
        (&clock_and_zone[..index], &clock_and_zone[index..])
    };
    let fields = clock.split(':').collect::<Vec<_>>();
    let [hours, minutes, seconds] = fields.as_slice() else {
        return None;
    };
    if hours.len() != 2 || minutes.len() != 2 {
        return None;
    }
    let hours = hours.parse::<u32>().ok()?;
    let minutes = minutes.parse::<u32>().ok()?;
    let seconds_value = seconds.split_once('.').map_or(*seconds, |(head, _)| head);
    if seconds_value.len() != 2 {
        return None;
    }
    let seconds_number = seconds_value.parse::<u32>().ok()?;
    if let Some((_, fraction)) = seconds.split_once('.') {
        if fraction.is_empty()
            || fraction.len() > 9
            || !fraction.bytes().all(|v| v.is_ascii_digit())
        {
            return None;
        }
    }
    if hours > 23 || minutes > 59 || seconds_number > 59 {
        return None;
    }
    if zone != "Z" {
        let offset = &zone[1..];
        let zone_parts = offset.split(':').collect::<Vec<_>>();
        let [zone_hour, zone_minute] = zone_parts.as_slice() else {
            return None;
        };
        if zone_hour.len() != 2
            || zone_minute.len() != 2
            || zone_hour.parse::<u32>().ok()? > 23
            || zone_minute.parse::<u32>().ok()? > 59
        {
            return None;
        }
    }
    Some(raw.to_owned())
}

fn image_records(
    article: &str,
    host: &str,
    fallback_path: &str,
    seen: &mut HashSet<String>,
) -> Vec<(MediaItem, Metadata)> {
    let page = Url::parse(&format!("https://{host}/")).unwrap();
    let link =
        text_between(article, "<a href=\"", "\"").and_then(|raw| source_entry_url(host, raw));
    let path = link.as_ref().map_or(fallback_path, Url::path);
    let entry = path.strip_prefix("/entry/").unwrap_or("").to_owned();
    let title = text_between(article, "<a href=\"", "</a>")
        .and_then(|text| text.split_once('>').map(|(_, body)| body))
        .and_then(decode_text);
    let published = text_between(article, "<time datetime=\"", "\"")
        .filter(|value| value.len() <= 64 && !value.chars().any(char::is_control));
    let content = text_between(
        article,
        "<div class=\"entry-content hatenablog-entry\">",
        "</div>",
    )
    .unwrap_or("");
    let mut files = Vec::new();
    for chunk in content.split("<img ").skip(1).take(2000) {
        let Some((attrs, _)) = chunk.split_once('>') else {
            continue;
        };
        if !attrs.contains("class=\"hatena-fotolife\"") {
            continue;
        }
        let Some(src) = text_between(attrs, "src=\"", "\"") else {
            continue;
        };
        let Some((url, name)) = original(src, &page) else {
            continue;
        };
        if !seen.insert(url.to_string()) {
            continue;
        }
        let number = files.len() + 1;
        let item = MediaItem {
            url: url.to_string(),
            filename: format!("hatenablog_{number:03}_{name}"),
            extractor: "hatenablog",
            post_id: None,
        };
        let mut metadata = media_metadata(&item);
        metadata.insert("subcategory".into(), "entry".into());
        metadata.insert("domain".into(), host.into());
        metadata.insert("entry".into(), entry.clone().into());
        metadata.insert("num".into(), number.into());
        metadata.insert("post_url".into(), format!("https://{host}{path}").into());
        if let Some(title) = &title {
            metadata.insert("title".into(), title.clone().into());
        }
        if let Some(date) = published {
            metadata.insert("date_source".into(), date.into());
            if let Some(validated) = source_iso_date(date) {
                metadata.insert("date".into(), validated.into());
            }
        }
        files.push((item, metadata));
    }
    files
}

fn article_events(
    page: &str,
    host: &str,
    fallback: &str,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let mut events = Vec::new();
    let mut media_count = 0usize;
    let mut rest = page;
    while let Some((_, tail)) = rest.split_once("<article ") {
        let Some((attrs, following)) = tail.split_once('>') else {
            break;
        };
        let Some((body, remainder)) = following.split_once("</article>") else {
            break;
        };
        rest = remainder;
        if attrs.contains("no-entry") {
            continue;
        }
        // Preserve independently published images even when two posts
        // reference the same original. Only deduplicate inside each article.
        let mut seen = HashSet::new();
        let records = image_records(body, host, fallback, &mut seen);
        media_count += records.len();
        if media_count > MAX_MEDIA || events.len() >= MAX_MEDIA * 2 {
            return Err("Hatena Blog page exceeds bounded public media count".into());
        }
        // The pinned extractor emits exactly one directory per article,
        // including image-less articles, then its ordered URL events.
        let mut directory = if let Some((_, meta)) = records.first() {
            meta.clone()
        } else {
            let mut metadata = Metadata::new();
            metadata.insert("category".into(), "hatenablog".into());
            metadata.insert("subcategory".into(), "entry".into());
            metadata.insert("domain".into(), host.into());
            let path = text_between(body, "<a href=\"", "\"")
                .and_then(|href| source_entry_url(host, href))
                .map_or(fallback.to_owned(), |url| url.path().to_owned());
            metadata.insert("entry".into(), path.trim_start_matches("/entry/").into());
            metadata.insert("post_url".into(), format!("https://{host}{path}").into());
            if let Some(title) = text_between(body, "<a href=\"", "</a>")
                .and_then(|text| text.split_once('>').map(|(_, content)| content))
                .and_then(decode_text)
            {
                metadata.insert("title".into(), title.into());
            }
            if let Some(date) = text_between(body, "<time datetime=\"", "\"")
                .filter(|date| date.len() <= 64 && !date.chars().any(char::is_control))
            {
                metadata.insert("date_source".into(), date.into());
                if let Some(validated) = source_iso_date(date) {
                    metadata.insert("date".into(), validated.into());
                }
            }
            metadata
        };
        directory.remove("filename");
        let article_count = records.len();
        directory.insert("count".into(), article_count.into());
        events.push(ExtractorMessage::Directory {
            metadata: directory,
        });
        for (file, mut metadata) in records {
            metadata.insert("count".into(), article_count.into());
            events.push(ExtractorMessage::Url {
                url: file.url,
                metadata,
            });
        }
    }
    Ok(events)
}

fn queue_entries(
    page: &str,
    host: &str,
    seen: &mut HashSet<String>,
    result: &mut Vec<ExtractorMessage>,
) {
    for segment in page
        .split("<section class=\"archive-entry")
        .skip(1)
        .take(1200)
    {
        if result.len() >= MAX_CHILDREN {
            break;
        }
        let Some((section, _)) = segment.split_once("</section>") else {
            continue;
        };
        let Some(url) = text_between(section, "<a class=\"entry-title-link\" href=\"", "\"")
            .and_then(|u| source_entry_url(host, u))
        else {
            continue;
        };
        if !seen.insert(url.to_string()) {
            continue;
        }
        let mut meta = Metadata::new();
        meta.insert("category".into(), "hatenablog".into());
        meta.insert("subcategory".into(), "archive".into());
        meta.insert("domain".into(), host.into());
        meta.insert(
            "entry".into(),
            url.path().trim_start_matches("/entry/").into(),
        );
        result.push(queued_url(url.to_string(), meta));
    }
}

fn valid_continuation_query(url: &Url) -> bool {
    let Some(query) = url.query() else {
        return true;
    };
    if query.len() > 180 {
        return false;
    }
    let pairs = url.query_pairs().collect::<Vec<_>>();
    if pairs.is_empty() || pairs.len() > 2 {
        return false;
    }
    let mut page = false;
    let mut search = false;
    for (key, value) in pairs {
        if value.is_empty() || value.len() > 120 {
            return false;
        }
        match key.as_ref() {
            "page" if !page && value.bytes().all(|b| b.is_ascii_digit()) => page = true,
            "q" if !search && url.path() == "/search" => search = true,
            _ => return false,
        }
    }
    true
}

fn next_page(html: &str, current: &Url) -> Option<Url> {
    let raw = text_between(html, " class=\"pager-next\">", "</a>")?;
    let href = text_between(raw, "<a href=\"", "\"")?;
    let lower = href.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "/../", "/./"]
        .iter()
        .any(|v| lower.contains(v))
        || lower.ends_with("/..")
        || lower.ends_with("/.")
    {
        return None;
    }
    let url = current.join(href).ok()?;
    if url.origin() != current.origin()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || !safe_archive_path(url.path())
        || !valid_continuation_query(&url)
    {
        return None;
    }
    Some(url)
}

async fn html(
    client: &Client,
    endpoint: Url,
    origin: &Url,
) -> Result<String, Box<dyn Error + Send + Sync>> {
    let mut response = client.get(endpoint).send().await?.error_for_status()?;
    if response.url().origin() != origin.origin() {
        return Err("Hatena Blog public page redirected outside its source origin".into());
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_HTML as u64)
    {
        return Err("Hatena Blog HTML exceeds bounded source size".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_HTML.saturating_sub(bytes.len()) {
            return Err("Hatena Blog HTML exceeds bounded source size".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8(bytes)?)
}

async fn at_base(
    client: &Client,
    base: &Url,
    target: &Target,
    max_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let (path, query, host, is_entry) = match target {
        Target::Entry { path, host } => (path.as_str(), None, host.as_str(), true),
        Target::Listing { path, query, host } => {
            (path.as_str(), query.as_deref(), host.as_str(), false)
        }
    };
    let mut endpoint = base.join(path.trim_start_matches('/'))?;
    if let Some(query) = query {
        endpoint.set_query(Some(query));
    }
    let mut visited = HashSet::new();
    let mut queues = HashSet::new();
    let mut events = Vec::new();
    for count in 0..(if is_entry {
        1
    } else {
        max_pages.clamp(1, MAX_PAGES)
    }) {
        if !visited.insert(endpoint.to_string()) {
            return Err("Hatena Blog pagination revisited a prior page".into());
        }
        if count > 0 {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        let body = html(client, endpoint.clone(), base).await?;
        if body.contains("page-archive") && !is_entry {
            queue_entries(&body, host, &mut queues, &mut events);
        } else {
            events.extend(article_events(&body, host, path)?);
        }
        if events.len() > MAX_MEDIA * 2 {
            return Err("Hatena Blog listing exceeds total media event cap".into());
        }
        if is_entry || queues.len() >= MAX_CHILDREN {
            break;
        }
        let Some(next) = next_page(&body, &endpoint) else {
            break;
        };
        endpoint = next;
    }
    Ok(events)
}

pub async fn resolve_messages(
    client: &Client,
    target: Target,
    max_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let host = match &target {
        Target::Entry { host, .. } | Target::Listing { host, .. } => host,
    };
    at_base(
        client,
        &Url::parse(&format!("https://{host}/"))?,
        &target,
        max_pages,
    )
    .await
}

pub async fn resolve(
    client: &Client,
    target: Target,
    max_pages: u16,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let events = resolve_messages(client, target, max_pages).await?;
    let mut files = Vec::new();
    for message in events {
        match message {
            ExtractorMessage::Url { url, metadata } => {
                let filename = metadata
                    .get("filename")
                    .and_then(|v| v.as_str())
                    .ok_or("Hatena Blog original lacks source-backed filename")?;
                files.push(MediaItem {
                    url,
                    filename: filename.into(),
                    extractor: "hatenablog",
                    post_id: None,
                });
            }
            ExtractorMessage::Queue { .. } => {
                return Err(
                    "Hatena Blog archive discovery requires --dump-messages or --follow-queues"
                        .into(),
                )
            }
            ExtractorMessage::Directory { .. } => {}
        }
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    #[test]
    fn routes_scoped_to_public_hatena_blog_domains_and_source_paths() {
        assert!(matches!(
            match_url("https://author.hatenablog.com/entry/2026/10/25/hello"),
            Some(Target::Entry { .. })
        ));
        assert!(matches!(
            match_url("https://author.hateblo.jp/archive/2026/10"),
            Some(Target::Listing { .. })
        ));
        assert!(matches!(
            match_url("https://author.hatenablog.jp/search?q=rust"),
            Some(Target::Listing { .. })
        ));
        for bad in [
            "https://evil-hatenablog.com/entry/123",
            "https://author.hatenablog.com:8080/archive",
            "https://name:pass@author.hatenablog.com/archive",
            "https://author.hatenablog.com/entry/../../secret",
            "https://author.hatenablog.com/archive/..",
            "https://author.hatenablog.com/archive/.",
            "https://author.hatenablog.com/archive?password=secret",
            "https://author.hatenablog.com/search?q=rust&secret=1",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn article_media_are_source_scoped_and_keep_entry_metadata() {
        let article = r#"<a href="/entry/2026/10/25/test">A &amp; B</a><time datetime="2026-10-25T12:00:00+09:00"></time><div class="entry-content hatenablog-entry"><img class="hatena-fotolife" src="https://cdn-ak.f.st-hatena.com/images/fotolife/u/author/20261025/original.jpg"><img class="other" src="https://cdn-ak.f.st-hatena.com/images/fotolife/u/author/20261025/ignored.jpg"><img class="hatena-fotolife" src="https://evil-st-hatena.com/forged.jpg"></div>"#;
        let mut seen = HashSet::new();
        let records = image_records(
            article,
            "author.hatenablog.com",
            "/entry/2026/10/25/test",
            &mut seen,
        );
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].1["entry"], "2026/10/25/test");
        assert_eq!(records[0].1["title"], "A & B");
        assert_eq!(records[0].1["date_source"], "2026-10-25T12:00:00+09:00");
        assert_eq!(records[0].1["date"], "2026-10-25T12:00:00+09:00");
        assert!(records[0].0.url.ends_with("/original.jpg"));
    }

    #[test]
    fn source_iso_dates_validate_gregorian_calendar_without_losing_timezone() {
        for good in [
            "2024-02-29T12:34:56Z",
            "2000-02-29T00:00:00+09:00",
            "2026-10-25T23:59:59.123-06:00",
        ] {
            assert_eq!(source_iso_date(good), Some(good.into()), "{good}");
        }
        for bad in [
            "2023-02-29T12:00:00Z",
            "1900-02-29T12:00:00Z",
            "2026-04-31T12:00:00Z",
            "2026-10-25T24:00:00Z",
            "2026-10-25T12:60:00Z",
            "2026-10-25T12:00:60Z",
            "2026-10-25T12:00:00",
            "2026-10-25T12:00:00+99:00",
            "2026-10-25T12:00:00-06:99",
            "2026-10-25T12:00:00.1234567890Z",
        ] {
            assert!(source_iso_date(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn archived_entry_queues_exclude_forged_hosts_and_duplicates() {
        let page = r#"<section class="archive-entry foo"><a class="entry-title-link" href="/entry/2026/10/25/first">First</a></section><section class="archive-entry foo"><a class="entry-title-link" href="/entry/2026/10/25/first">Duplicate</a></section><section class="archive-entry foo"><a class="entry-title-link" href="https://evil-hatenablog.com/entry/2026/10/25/forged">Bad</a></section>"#;
        let mut queues = Vec::new();
        queue_entries(
            page,
            "author.hatenablog.com",
            &mut HashSet::new(),
            &mut queues,
        );
        assert_eq!(queues.len(), 1);
        assert_eq!(
            serde_json::to_value(&queues[0]).unwrap()["url"],
            "https://author.hatenablog.com/entry/2026/10/25/first"
        );
    }

    #[test]
    fn source_entry_queues_reject_raw_traversal_before_url_normalization() {
        let host = "author.hatenablog.com";
        for raw in [
            "/entry/2026/../2025/first",
            "/entry/2026/%2e%2e/2025/first",
            "/entry/2026/%2fsecret",
            "/entry/2026/.",
            "https://evil-hatenablog.com/entry/2026/first",
            "https://author.hatenablog.com/entry/2026/first?secret=1",
        ] {
            assert!(source_entry_url(host, raw).is_none(), "{raw}");
        }
        assert!(source_entry_url(host, "/entry/2026/10/25/first").is_some());
    }

    #[test]
    fn full_article_listing_keeps_distinct_posts_sharing_a_media_original() {
        let page = r#"<article class="entry"><a href="/entry/2026/10/25/first">First</a><div class="entry-content hatenablog-entry"><img class="hatena-fotolife" src="https://cdn-ak.f.st-hatena.com/shared.jpg"></div></article><article class="entry"><a href="/entry/2026/10/25/second">Second</a><div class="entry-content hatenablog-entry"><img class="hatena-fotolife" src="https://cdn-ak.f.st-hatena.com/shared.jpg"></div></article>"#;
        let events = article_events(page, "author.hatenablog.com", "/").unwrap();
        assert_eq!(events.len(), 4);
        let first = serde_json::to_value(&events[1]).unwrap();
        let second = serde_json::to_value(&events[3]).unwrap();
        assert_eq!(first["metadata"]["entry"], "2026/10/25/first");
        assert_eq!(second["metadata"]["entry"], "2026/10/25/second");
        assert_eq!(first["url"], second["url"]);
    }

    #[test]
    fn imageless_article_emits_directory_and_multi_image_article_has_one_directory() {
        let page = r#"<article class="entry"><a href="/entry/2026/10/25/empty">Empty</a><div class="entry-content hatenablog-entry"></div></article><article class="entry"><a href="/entry/2026/10/25/two">Two</a><div class="entry-content hatenablog-entry"><img class="hatena-fotolife" src="https://cdn-ak.f.st-hatena.com/one.jpg"><img class="hatena-fotolife" src="https://cdn-ak.f.st-hatena.com/two.png"></div></article>"#;
        let events = article_events(page, "author.hatenablog.com", "/").unwrap();
        assert_eq!(events.len(), 4);
        let empty = serde_json::to_value(&events[0]).unwrap();
        let populated = serde_json::to_value(&events[1]).unwrap();
        assert_eq!(empty["metadata"]["entry"], "2026/10/25/empty");
        assert_eq!(empty["metadata"]["count"], 0);
        assert_eq!(populated["metadata"]["count"], 2);
        assert_eq!(
            serde_json::to_value(&events[2]).unwrap()["metadata"]["count"],
            2
        );
        assert_eq!(
            serde_json::to_value(&events[3]).unwrap()["metadata"]["num"],
            2
        );
    }

    #[tokio::test]
    async fn public_entry_uses_one_bounded_html_request() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 4096];
            let n = socket.read(&mut request).await.unwrap();
            assert!(String::from_utf8_lossy(&request[..n])
                .starts_with("GET /entry/2026/10/25/test HTTP/1.1"));
            let body = r#"<article class="entry"><a href="/entry/2026/10/25/test">Test</a><div class="entry-content hatenablog-entry"><img class="hatena-fotolife" src="https://cdn-ak.f.st-hatena.com/images/fotolife/u/author/20261025/one.png"></div></article>"#;
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(reply.as_bytes()).await.unwrap();
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let target = match_url("https://author.hatenablog.com/entry/2026/10/25/test").unwrap();
        let events = at_base(&Client::new(), &base, &target, 4).await.unwrap();
        assert_eq!(events.len(), 2);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn two_archive_pages_queue_first_party_entries_once() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (path, body) in [
                (
                    "/archive",
                    r#"<body class="page-archive"><section class="archive-entry foo"><a class="entry-title-link" href="/entry/2026/10/25/one">One</a></section><div class="pager-next"><a href="/archive?page=2">Next</a></div>"#,
                ),
                (
                    "/archive?page=2",
                    r#"<body class="page-archive"><section class="archive-entry foo"><a class="entry-title-link" href="/entry/2026/10/25/one">Dup</a></section><section class="archive-entry foo"><a class="entry-title-link" href="/entry/2026/10/25/two">Two</a></section>"#,
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
        let target = match_url("https://author.hatenablog.com/archive").unwrap();
        let events = at_base(&Client::new(), &base, &target, 4).await.unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(
            serde_json::to_value(&events[1]).unwrap()["url"],
            "https://author.hatenablog.com/entry/2026/10/25/two"
        );
        server.await.unwrap();
    }
}
