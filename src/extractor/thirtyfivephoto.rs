//! Guarded public 35photo original-image and listing preview from pinned 35photo.py.
use crate::{
    message::{media_metadata, queued_url, ExtractorMessage, Metadata},
    MediaItem,
};
use reqwest::Client;
use serde_json::Value;
use std::{collections::HashSet, error::Error, time::Duration};
use url::Url;

const MAX_BODY: usize = 2 * 1024 * 1024;
const MAX_MEDIA: usize = 32;
const MAX_LIST_PAGES: u16 = 4;
const MAX_CHILDREN: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Image(String),
    Tag(String),
    Genre(String),
    GenreNew(String),
    User(String),
}

fn numeric(s: &str) -> bool {
    (1..=18).contains(&s.len())
        && s.bytes().all(|b| b.is_ascii_digit())
        && s.parse::<u64>().ok().is_some_and(|n| n > 0)
}
fn slug(s: &str) -> bool {
    (1..=90).contains(&s.len())
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}
pub fn match_url(raw: &str) -> Option<Target> {
    let candidate = if raw.starts_with("35photo.pro/") {
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
    let host = url.host_str()?;
    let recognized = host == "35photo.pro"
        || host.strip_suffix(".35photo.pro").is_some_and(|sub| {
            (1..=24).contains(&sub.len()) && sub.bytes().all(|b| b.is_ascii_lowercase())
        });
    if !recognized
        || !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let segs: Vec<_> = url.path_segments()?.collect();
    match segs.as_slice() {
        [p] | [p, ""] if p.strip_prefix("photo_").is_some_and(numeric) => {
            Some(Target::Image(p.trim_start_matches("photo_").into()))
        }
        ["tags", tag] | ["tags", tag, ""] if slug(tag) => Some(Target::Tag((*tag).into())),
        [p] | [p, ""] if p.strip_prefix("genre_").is_some_and(numeric) => {
            Some(Target::Genre(p.trim_start_matches("genre_").into()))
        }
        [p, "new"] | [p, "new", ""] if p.strip_prefix("genre_").is_some_and(numeric) => {
            Some(Target::GenreNew(p.trim_start_matches("genre_").into()))
        }
        [user] | [user, ""]
            if slug(user)
                && !user.starts_with("photo_")
                && !user.starts_with("genre_")
                && !matches!(*user, "rating" | "tags" | "genre" | "photo" | "photoUser") =>
        {
            Some(Target::User((*user).into()))
        }
        _ => None,
    }
}

async fn read_bounded(
    response: reqwest::Response,
    request: &Url,
) -> Result<Vec<u8>, Box<dyn Error + Send + Sync>> {
    let mut response = response.error_for_status()?;
    if response.url().origin() != request.origin() {
        return Err("35photo request redirected outside origin".into());
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_BODY as u64)
    {
        return Err("35photo public response exceeds limit".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_BODY.saturating_sub(bytes.len()) {
            return Err("35photo public response exceeds limit".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn media_source(raw: &str) -> Option<(String, String)> {
    let url = Url::parse(raw).ok()?;
    let host = url.host_str()?;
    if url.scheme() != "https"
        || !(host == "35photo.pro" || host.ends_with(".35photo.pro"))
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
    let file = url.path_segments()?.next_back()?;
    let ext = file.rsplit_once('.')?.1.to_ascii_lowercase();
    if !matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "webp" | "gif") {
        return None;
    }
    Some((url.to_string(), ext))
}

fn photo_records(
    value: &Value,
    id: &str,
) -> Result<Vec<(MediaItem, Metadata)>, Box<dyn Error + Send + Sync>> {
    let source = value
        .get("data")
        .and_then(|d| d.get(id))
        .ok_or("35photo API missing requested public photo ID")?;
    let source_id = source
        .get("photo_id")
        .and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok()))
        .ok_or("35photo source photo ID invalid")?;
    if source_id.to_string() != id {
        return Err("35photo API returned a mismatched image ID".into());
    }
    let mut common = Metadata::new();
    common.insert("category".into(), "35photo".into());
    common.insert("subcategory".into(), "image".into());
    common.insert("id".into(), source_id.into());
    common.insert(
        "post_url".into(),
        format!("https://35photo.pro/photo_{id}/").into(),
    );
    for (field, key) in [
        ("photo_name", "title"),
        ("photo_desc", "description"),
        ("photo_type", "type"),
        ("timeAdd", "date_source"),
        ("user_login", "user"),
        ("user_name", "user_name"),
    ] {
        if let Some(s) = source
            .get(field)
            .and_then(Value::as_str)
            .filter(|s| s.len() <= 500 && !s.chars().any(char::is_control))
        {
            common.insert(key.into(), s.into());
        }
    }
    for (field, key) in [
        ("photo_see", "views"),
        ("photo_fav", "favorites"),
        ("photo_rating", "score"),
        ("user_id", "user_id"),
    ] {
        if let Some(n) = source
            .get(field)
            .and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok()))
        {
            common.insert(key.into(), n.into());
        }
    }
    if let Some(tags) = source.get("tags").and_then(Value::as_array) {
        if tags.len() <= 100 {
            common.insert("tags".into(), Value::Array(tags.clone()));
        }
    }
    let mut out = Vec::new();
    if let Some(series) = source.get("series").and_then(Value::as_array) {
        if series.len() > MAX_MEDIA {
            return Err("35photo series exceeds public image cap".into());
        }
        for (i, photo) in series.iter().enumerate() {
            let Some(raw) = photo.get("src").and_then(Value::as_str) else {
                continue;
            };
            let Some((url, ext)) = media_source(raw) else {
                continue;
            };
            let index = i + 1;
            let item = MediaItem {
                url,
                filename: format!("{id}_{index:02}.{ext}"),
                extractor: "35photo",
                post_id: Some(source_id),
            };
            let mut meta = media_metadata(&item);
            for (key, value) in &common {
                meta.insert(key.clone(), value.clone());
            }
            meta.insert("num".into(), index.into());
            if let Some(series_id) = photo
                .get("id")
                .and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok()))
            {
                meta.insert("id_series".into(), series_id.into());
            }
            if let Some(title) = photo
                .get("title")
                .and_then(Value::as_str)
                .filter(|s| s.len() <= 240 && !s.chars().any(char::is_control))
            {
                meta.insert("title_series".into(), title.into());
            }
            out.push((item, meta));
        }
    } else if let Some(raw) = source.get("src").and_then(Value::as_str) {
        if let Some((url, ext)) = media_source(raw) {
            let item = MediaItem {
                url,
                filename: format!("{id}_01.{ext}"),
                extractor: "35photo",
                post_id: Some(source_id),
            };
            let mut meta = media_metadata(&item);
            for (key, value) in &common {
                meta.insert(key.clone(), value.clone());
            }
            meta.insert("num".into(), 1.into());
            out.push((item, meta));
        }
    }
    if out.is_empty() {
        return Err("35photo returned no trusted public original".into());
    }
    Ok(out)
}

fn list_ids(html: &str, seen: &mut HashSet<String>, out: &mut Vec<ExtractorMessage>, scope: &str) {
    let base = Url::parse("https://35photo.pro/").unwrap();
    for link in html.split("<a ").skip(1).take(4000) {
        if out.len() >= MAX_CHILDREN {
            break;
        }
        let Some((attrs, _)) = link.split_once('>') else {
            continue;
        };
        let href = attrs
            .split_once("href=\"")
            .and_then(|(_, after)| after.split_once('"').map(|(value, _)| value))
            .or_else(|| {
                attrs
                    .split_once("href='")
                    .and_then(|(_, after)| after.split_once('\'').map(|(value, _)| value))
            });
        let Some(href) = href else { continue };
        let Ok(url) = base.join(href) else { continue };
        let Some(host) = url.host_str() else { continue };
        if !matches!(url.scheme(), "https" | "http")
            || !(host == "35photo.pro" || host.ends_with(".35photo.pro"))
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
        let ([route] | [route, ""]) = parts.as_slice() else {
            continue;
        };
        let Some(id) = route.strip_prefix("photo_") else {
            continue;
        };
        if !numeric(id) || !seen.insert(id.into()) {
            continue;
        }
        let mut meta = Metadata::new();
        meta.insert("category".into(), "35photo".into());
        meta.insert("subcategory".into(), "listing".into());
        meta.insert("source_scope".into(), scope.into());
        meta.insert("id".into(), id.parse::<u64>().unwrap().into());
        out.push(queued_url(format!("https://35photo.pro/photo_{id}/"), meta));
    }
}

/// Matches pinned _35photoExtractor._photo_ids: numeric source IDs
/// descending, with duplicates removed. Never follow source HTML links.
fn parse_ajax_ids(markup: &str) -> Vec<String> {
    let mut ids = HashSet::new();
    for block in markup.split("/photo_").skip(1).take(4000) {
        let raw = block
            .bytes()
            .take_while(u8::is_ascii_digit)
            .collect::<Vec<_>>();
        let Ok(id) = std::str::from_utf8(&raw) else {
            continue;
        };
        if !numeric(id) || !block[raw.len()..].starts_with('/') {
            continue;
        }
        ids.insert(id.parse::<u64>().unwrap());
    }
    let mut values = ids.into_iter().collect::<Vec<_>>();
    values.sort_unstable_by(|left, right| right.cmp(left));
    values.into_iter().map(|id| id.to_string()).collect()
}

fn enqueue_ids(
    ids: impl IntoIterator<Item = String>,
    scope: &str,
    seen: &mut HashSet<String>,
    queue: &mut Vec<ExtractorMessage>,
) {
    for id in ids {
        if queue.len() >= MAX_CHILDREN {
            break;
        }
        if !seen.insert(id.clone()) {
            continue;
        }
        let mut meta = Metadata::new();
        meta.insert("category".into(), "35photo".into());
        meta.insert("subcategory".into(), "listing".into());
        meta.insert("source_scope".into(), scope.into());
        meta.insert("id".into(), id.parse::<u64>().unwrap().into());
        queue.push(queued_url(format!("https://35photo.pro/photo_{id}/"), meta));
    }
}

fn numeric_field(value: &Value) -> Option<String> {
    let val = value
        .as_str()
        .map(ToOwned::to_owned)
        .or_else(|| value.as_u64().map(|n| n.to_string()))?;
    numeric(&val).then_some(val)
}

/// Uses pinned XHR request parameters and strictly monotonically descending
/// numeric cursors. Bounded pages prevent unbounded source-side workloads.
async fn ajax_listing(
    client: &Client,
    page_base: &Url,
    scope: &str,
    mode: &str,
    key: &str,
    rating: &str,
    first_ids: Vec<String>,
    max_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let mut queue = Vec::new();
    let mut seen_ids = HashSet::new();
    let mut cursor = if mode == "genre" {
        let Some(last) = first_ids.last() else {
            return Ok(queue);
        };
        last.clone()
    } else {
        "999999999".to_owned()
    };
    enqueue_ids(first_ids, scope, &mut seen_ids, &mut queue);
    let mut visited_cursors = HashSet::new();
    for page in 0..max_pages.clamp(1, MAX_LIST_PAGES) {
        if queue.len() >= MAX_CHILDREN {
            break;
        }
        if !visited_cursors.insert(cursor.clone()) {
            return Err("35photo AJAX pagination repeated a cursor".into());
        }
        if page > 0 {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        let mut url = page_base.join("show_block.php")?;
        {
            let mut pairs = url.query_pairs_mut();
            pairs
                .append_pair("type", "getNextPageData")
                .append_pair("page", mode);
            if mode == "photoUser" {
                pairs.append_pair("user_id", key);
            } else {
                pairs
                    .append_pair("community_id", key)
                    .append_pair("photo_rating", rating);
            }
            pairs.append_pair("lastId", &cursor);
        }
        let response = client
            .get(url.clone())
            .header("Referer", "https://35photo.pro")
            .header("X-Requested-With", "XMLHttpRequest")
            .send()
            .await?;
        let body = read_bounded(response, &url).await?;
        let data: Value = serde_json::from_slice(&body)?;
        let markup = data
            .get("data")
            .and_then(Value::as_str)
            .ok_or("35photo AJAX response missing public data HTML")?;
        if markup.len() > MAX_BODY {
            return Err("35photo AJAX HTML exceeds bounded response".into());
        }
        enqueue_ids(parse_ajax_ids(markup), scope, &mut seen_ids, &mut queue);
        match data.get("lastId") {
            Some(Value::String(last)) if last.is_empty() => break,
            Some(Value::Number(n)) if n.as_u64() == Some(0) => break,
            Some(next) => {
                let next = numeric_field(next)
                    .ok_or("35photo AJAX returned an invalid numeric continuation")?;
                if next.parse::<u64>()? >= cursor.parse::<u64>()? {
                    return Err("35photo AJAX cursor did not decrease".into());
                }
                cursor = next;
            }
            None => return Err("35photo AJAX response missing continuation cursor".into()),
        }
    }
    Ok(queue)
}

async fn at_base(
    client: &Client,
    page_base: &Url,
    api_base: &Url,
    target: Target,
    max_pages: u16,
) -> Result<(Vec<(MediaItem, Metadata)>, Vec<ExtractorMessage>), Box<dyn Error + Send + Sync>> {
    match target {
        Target::Image(id) => {
            let mut url = api_base.clone();
            url.query_pairs_mut()
                .append_pair("method", "photo.getData")
                .append_pair("photoId", &id);
            let bytes = read_bounded(client.get(url.clone()).send().await?, &url).await?;
            let source: Value = serde_json::from_slice(&bytes)?;
            Ok((photo_records(&source, &id)?, Vec::new()))
        }
        Target::Tag(name) => {
            let scope = format!("tag:{name}");
            let mut seen = HashSet::new();
            let mut output = Vec::new();
            for page in 1..=max_pages.clamp(1, MAX_LIST_PAGES) {
                if page > 1 {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                let url = page_base.join(&format!("tags/{name}/list_{page}/"))?;
                let body = read_bounded(client.get(url.clone()).send().await?, &url).await?;
                let text = String::from_utf8(body)?;
                let previous = output.len();
                list_ids(&text, &mut seen, &mut output, &scope);
                if output.len() == previous || output.len() >= MAX_CHILDREN {
                    break;
                }
            }
            Ok((Vec::new(), output))
        }
        Target::User(name) => {
            let url = page_base.join(&format!("{name}/"))?;
            let body = read_bounded(client.get(url.clone()).send().await?, &url).await?;
            let html = String::from_utf8(body)?;
            let (_, tail) = html
                .split_once("/user_")
                .ok_or("35photo public user profile lacks a source user ID")?;
            let raw = tail
                .split_once(".xml")
                .map(|(first, _)| first)
                .ok_or("35photo public user profile lacks source XML identifier")?;
            if !numeric(raw) {
                return Err("35photo public user source ID invalid".into());
            }
            let queue = ajax_listing(
                client,
                page_base,
                &format!("user:{name}"),
                "photoUser",
                raw,
                "50",
                Vec::new(),
                max_pages,
            )
            .await?;
            Ok((Vec::new(), queue))
        }
        listing @ (Target::Genre(_) | Target::GenreNew(_)) => {
            let (id, recent) = match listing {
                Target::Genre(id) => (id, false),
                Target::GenreNew(id) => (id, true),
                _ => unreachable!(),
            };
            let path = if recent {
                format!("genre_{id}/new/")
            } else {
                format!("genre_{id}/")
            };
            let url = page_base.join(&path)?;
            let body = read_bounded(client.get(url.clone()).send().await?, &url).await?;
            let html = String::from_utf8(body)?;
            let markup = html
                .split_once(" class=\"photo")
                .map(|(_, text)| text)
                .unwrap_or("");
            let first = parse_ajax_ids(markup);
            let scope = if recent {
                format!("genre_new:{id}")
            } else {
                format!("genre:{id}")
            };
            let rating = if recent { "0" } else { "50" };
            let queue = ajax_listing(
                client, page_base, &scope, "genre", &id, rating, first, max_pages,
            )
            .await?;
            Ok((Vec::new(), queue))
        }
    }
}

pub async fn resolve(
    client: &Client,
    target: Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse("https://35photo.pro/")?;
    let api = Url::parse("https://api.35photo.pro/")?;
    let (media, queues) = at_base(client, &base, &api, target, MAX_LIST_PAGES).await?;
    if media.is_empty() && !queues.is_empty() {
        return Err("35photo listing feeds require --dump-messages or --follow-queues".into());
    }
    Ok(media.into_iter().map(|(item, _)| item).collect())
}

pub async fn resolve_messages(
    client: &Client,
    target: Target,
    max_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse("https://35photo.pro/")?;
    let api = Url::parse("https://api.35photo.pro/")?;
    let (records, mut queue) = at_base(client, &base, &api, target, max_pages).await?;
    let mut events = Vec::new();
    for (item, meta) in records {
        events.push(ExtractorMessage::Directory {
            metadata: meta.clone(),
        });
        events.push(ExtractorMessage::Url {
            url: item.url,
            metadata: meta,
        });
    }
    events.append(&mut queue);
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    #[test]
    fn routes_accept_pinned_images_tags_genres_and_users_only() {
        assert_eq!(
            match_url("https://35photo.pro/photo_12345/"),
            Some(Target::Image("12345".into()))
        );
        assert_eq!(
            match_url("https://35photo.pro/tags/sunset/"),
            Some(Target::Tag("sunset".into()))
        );
        assert_eq!(
            match_url("https://35photo.pro/genre_12345/"),
            Some(Target::Genre("12345".into()))
        );
        assert_eq!(
            match_url("https://35photo.pro/genre_12345/new/"),
            Some(Target::GenreNew("12345".into()))
        );
        assert_eq!(
            match_url("https://35photo.pro/alice/"),
            Some(Target::User("alice".into()))
        );
        for bad in [
            "https://evil35photo.pro/photo_12345",
            "https://35photo.pro/photo_0",
            "https://35photo.pro/photo_12345?private=1",
            "https://name:pw@35photo.pro/photo_123",
            "https://35photo.pro:8443/photo_123",
            "https://35photo.pro/photo_123/../../private",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn source_photo_json_yields_validated_series_originals_and_metadata() {
        let source = serde_json::json!({"data":{"12345":{
            "photo_id":12345,"photo_name":"Autumn","photo_desc":"Golden light",
            "user_login":"alice","photo_see":"50","tags":["gold","leaves"],
            "series":[
                {"src":"https://img.35photo.pro/one.jpg","id":"1","title":"One"},
                {"src":"https://img.35photo.pro/two.png","id":2,"title":"Two"}
            ]
        }}});
        let records = photo_records(&source, "12345").unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].0.filename, "12345_01.jpg");
        assert_eq!(records[1].0.filename, "12345_02.png");
        assert_eq!(records[0].1["views"], 50);
        assert_eq!(records[1].1["id_series"], 2);
        let forged = serde_json::json!({"data":{"12345":{"photo_id":12345,"src":"https://evil35photo.pro/secret.jpg"}}});
        assert!(photo_records(&forged, "12345").is_err());
    }

    #[test]
    fn tag_page_queues_first_seen_numeric_public_photos() {
        let mut seen = HashSet::new();
        let mut output = Vec::new();
        list_ids(
            r#"<a href="https://35photo.pro/photo_10/">One</a><a href="https://35photo.pro/photo_10/">Duplicate</a><a href="https://35photo.pro/photo_20/">Two</a><a href="https://evil35photo.pro/photo_30/">Forged</a>"#,
            &mut seen,
            &mut output,
            "tag:sunset",
        );
        assert_eq!(output.len(), 2);
        assert_eq!(
            serde_json::to_value(&output[1]).unwrap()["url"],
            "https://35photo.pro/photo_20/"
        );
    }

    #[test]
    fn ajax_photo_identifiers_sort_numerically_and_reject_truncated_paths() {
        assert_eq!(
            parse_ajax_ids(
                r#"/photo_9/ /photo_23/ /photo_23/ /photo_7/ /photo_0/ /photo_35fake/ /photo_22"#
            ),
            vec!["23", "9", "7"]
        );
    }

    #[tokio::test]
    async fn user_and_genre_ajax_queues_follow_pinned_numeric_cursors() {
        let server = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let backend = tokio::spawn(async move {
            for (expected, body) in [
                ("/alice/", r#"<link href="/user_700.xml">"#),
                ("/show_block.php?type=getNextPageData&page=photoUser&user_id=700&lastId=999999999",
                    r#"{"data":"<a href='/photo_90/'>One</a><a href='/photo_80/'>Two</a>","lastId":"80"}"#),
                ("/show_block.php?type=getNextPageData&page=photoUser&user_id=700&lastId=80",
                    r#"{"data":"<a href='/photo_70/'>Three</a>","lastId":""}"#),
                ("/genre_12/", r#"<div class="photo item"><a href="/photo_45/">A</a><a href="/photo_30/">B</a></div>"#),
                ("/show_block.php?type=getNextPageData&page=genre&community_id=12&photo_rating=50&lastId=30",
                    r#"{"data":"<a href='/photo_20/'>C</a><a href='/photo_20/'>Dup</a>","lastId":""}"#),
            ] {
                let (mut stream, _) = server.accept().await.unwrap();
                let mut request = [0u8; 8192];
                let n = stream.read(&mut request).await.unwrap();
                assert!(
                    String::from_utf8_lossy(&request[..n])
                        .starts_with(&format!("GET {expected} HTTP/1.1")),
                    "request mismatch for {expected}"
                );
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{address}/")).unwrap();
        let (_, user) = at_base(
            &Client::new(),
            &base,
            &base,
            Target::User("alice".into()),
            2,
        )
        .await
        .unwrap();
        assert_eq!(user.len(), 3);
        let (_, genre) = at_base(&Client::new(), &base, &base, Target::Genre("12".into()), 2)
            .await
            .unwrap();
        assert_eq!(genre.len(), 3);
        assert_eq!(
            serde_json::to_value(&genre[2]).unwrap()["url"],
            "https://35photo.pro/photo_20/"
        );
        backend.await.unwrap();
    }

    #[tokio::test]
    async fn recent_genre_uses_zero_rating_in_pinned_ajax_protocol() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (path, html) in [
                ("/genre_12/new/", r#"<div class="photo item"><a href="/photo_45/">First</a></div>"#),
                (
                    "/show_block.php?type=getNextPageData&page=genre&community_id=12&photo_rating=0&lastId=45",
                    r#"{"data":"<a href='/photo_20/'>Next</a>","lastId":""}"#,
                ),
            ] {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = [0u8; 8192];
                let len = stream.read(&mut request).await.unwrap();
                assert!(
                    String::from_utf8_lossy(&request[..len])
                        .starts_with(&format!("GET {path} HTTP/1.1"))
                );
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
                    html.len()
                );
                stream.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{address}/")).unwrap();
        let (_, queue) = at_base(
            &Client::new(),
            &base,
            &base,
            Target::GenreNew("12".into()),
            2,
        )
        .await
        .unwrap();
        assert_eq!(queue.len(), 2);
        assert_eq!(
            serde_json::to_value(&queue[0]).unwrap()["metadata"]["source_scope"],
            "genre_new:12"
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn api_and_two_tag_pages_resolve_public_source_requests() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (path, body) in [
                (
                    "/?method=photo.getData&photoId=12345",
                    r#"{"data":{"12345":{"photo_id":12345,"src":"https://img.35photo.pro/photo.jpg"}}}"#,
                ),
                (
                    "/tags/sunset/list_1/",
                    r#"<a href="https://35photo.pro/photo_10/">One</a>"#,
                ),
                (
                    "/tags/sunset/list_2/",
                    r#"<a href="https://35photo.pro/photo_20/">Two</a>"#,
                ),
                ("/tags/sunset/list_3/", ""),
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
        let (records, _) = at_base(
            &Client::new(),
            &base,
            &base,
            Target::Image("12345".into()),
            4,
        )
        .await
        .unwrap();
        assert_eq!(records.len(), 1);
        let (_, queued) = at_base(
            &Client::new(),
            &base,
            &base,
            Target::Tag("sunset".into()),
            4,
        )
        .await
        .unwrap();
        assert_eq!(queued.len(), 2);
        server.await.unwrap();
    }
}
