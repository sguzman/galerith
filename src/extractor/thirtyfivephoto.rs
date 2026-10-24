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
    User(String),
}

fn numeric(s: &str) -> bool {
    (1..=18).contains(&s.len())
        && s.bytes().all(|b| b.is_ascii_digit())
        && s.parse::<u64>().ok().is_some_and(|n| n > 0)
}
fn slug(s: &str) -> bool {
    (1..=90).contains(&s.len())
        && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}
pub fn match_url(raw: &str) -> Option<Target> {
    let candidate = if raw.starts_with("35photo.pro/") {
        format!("https://{raw}")
    } else {raw.to_owned()};
    let lower = candidate.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "/../", "/./"].iter().any(|s| lower.contains(s)) {
        return None;
    }
    let url = Url::parse(&candidate).ok()?;
    let host = url.host_str()?;
    let recognized = host == "35photo.pro"
        || host.strip_suffix(".35photo.pro").is_some_and(|sub| (1..=24).contains(&sub.len())
            && sub.bytes().all(|b| b.is_ascii_lowercase()));
    if !recognized || !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty() || url.password().is_some()
        || url.port().is_some() || url.query().is_some() || url.fragment().is_some()
    {return None;}
    let segs: Vec<_> = url.path_segments()?.collect();
    match segs.as_slice() {
        [p] | [p, ""] if p.strip_prefix("photo_").is_some_and(numeric) =>
            Some(Target::Image(p.trim_start_matches("photo_").into())),
        ["tags", tag] | ["tags", tag, ""] if slug(tag) =>
            Some(Target::Tag((*tag).into())),
        [p] | [p, ""] if p.strip_prefix("genre_").is_some_and(numeric) =>
            Some(Target::Genre(p.trim_start_matches("genre_").into())),
        [user] | [user, ""] if slug(user)
            && !matches!(*user, "rating" | "tags" | "genre" | "photo" | "photoUser") =>
                Some(Target::User((*user).into())),
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
    if response.content_length().is_some_and(|n| n > MAX_BODY as u64) {
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
        || !url.username().is_empty() || url.password().is_some()
        || url.port().is_some() || url.query().is_some() || url.fragment().is_some()
        || url.path().contains('%') || url.path().contains("..")
    {return None;}
    let file = url.path_segments()?.next_back()?;
    let ext = file.rsplit_once('.')?.1.to_ascii_lowercase();
    if !matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "webp" | "gif") {return None;}
    Some((url.to_string(), ext))
}

fn photo_records(
    value: &Value, id: &str,
) -> Result<Vec<(MediaItem, Metadata)>, Box<dyn Error + Send + Sync>> {
    let source = value.get("data").and_then(|d| d.get(id))
        .ok_or("35photo API missing requested public photo ID")?;
    let source_id = source.get("photo_id")
        .and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok()))
        .ok_or("35photo source photo ID invalid")?;
    if source_id.to_string() != id {
        return Err("35photo API returned a mismatched image ID".into());
    }
    let mut common = Metadata::new();
    common.insert("category".into(), "35photo".into());
    common.insert("subcategory".into(), "image".into());
    common.insert("id".into(), source_id.into());
    common.insert("post_url".into(), format!("https://35photo.pro/photo_{id}/").into());
    for (field, key) in [
        ("photo_name", "title"), ("photo_desc", "description"), ("photo_type", "type"),
        ("timeAdd", "date_source"), ("user_login", "user"), ("user_name", "user_name"),
    ] {
        if let Some(s) = source.get(field).and_then(Value::as_str)
            .filter(|s| s.len() <= 500 && !s.chars().any(char::is_control))
        {common.insert(key.into(), s.into());}
    }
    for (field, key) in [
        ("photo_see", "views"), ("photo_fav", "favorites"),
        ("photo_rating", "score"), ("user_id", "user_id"),
    ] {
        if let Some(n) = source.get(field)
            .and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok()))
        {common.insert(key.into(), n.into());}
    }
    if let Some(tags) = source.get("tags").and_then(Value::as_array) {
        if tags.len() <= 100 {
            common.insert("tags".into(), Value::Array(tags.clone()));
        }
    }
    let mut out = Vec::new();
    if let Some(series) = source.get("series").and_then(Value::as_array) {
        if series.len() > MAX_MEDIA {return Err("35photo series exceeds public image cap".into());}
        for (i, photo) in series.iter().enumerate() {
            let Some(raw) = photo.get("src").and_then(Value::as_str) else {continue};
            let Some((url, ext)) = media_source(raw) else {continue};
            let index = i + 1;
            let item = MediaItem {
                url,
                filename: format!("{id}_{index:02}.{ext}"),
                extractor: "35photo",
                post_id: Some(source_id),
            };
            let mut meta = media_metadata(&item);
            for (key, value) in &common {meta.insert(key.clone(), value.clone());}
            meta.insert("num".into(), index.into());
            if let Some(series_id) = photo.get("id")
                .and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok()))
            {meta.insert("id_series".into(), series_id.into());}
            if let Some(title) = photo.get("title").and_then(Value::as_str)
                .filter(|s| s.len() <= 240 && !s.chars().any(char::is_control))
            {meta.insert("title_series".into(), title.into());}
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
            for (key, value) in &common {meta.insert(key.clone(), value.clone());}
            meta.insert("num".into(), 1.into());
            out.push((item, meta));
        }
    }
    if out.is_empty() {return Err("35photo returned no trusted public original".into());}
    Ok(out)
}

fn list_ids(
    html: &str, seen: &mut HashSet<String>, out: &mut Vec<ExtractorMessage>,
    scope: &str,
) {
    let base = Url::parse("https://35photo.pro/").unwrap();
    for link in html.split("<a ").skip(1).take(4000) {
        if out.len() >= MAX_CHILDREN { break; }
        let Some((attrs, _)) = link.split_once('>') else { continue };
        let href = attrs.split_once("href=\"")
            .and_then(|(_, after)| after.split_once('"').map(|(value, _)| value))
            .or_else(|| attrs.split_once("href='")
                .and_then(|(_, after)| after.split_once('\'').map(|(value, _)| value)));
        let Some(href) = href else { continue };
        let Ok(url) = base.join(href) else { continue };
        let Some(host) = url.host_str() else { continue };
        if !matches!(url.scheme(), "https" | "http")
            || !(host == "35photo.pro" || host.ends_with(".35photo.pro"))
            || !url.username().is_empty() || url.password().is_some()
            || url.port().is_some() || url.query().is_some() || url.fragment().is_some()
        { continue; }
        let Some(paths) = url.path_segments() else { continue };
        let parts: Vec<_> = paths.collect();
        let [route] | [route, ""] = parts.as_slice() else { continue };
        let Some(id) = route.strip_prefix("photo_") else { continue };
        if !numeric(id) || !seen.insert(id.into()) { continue; }
        let mut meta = Metadata::new();
        meta.insert("category".into(), "35photo".into());
        meta.insert("subcategory".into(), "listing".into());
        meta.insert("source_scope".into(), scope.into());
        meta.insert("id".into(), id.parse::<u64>().unwrap().into());
        out.push(queued_url(format!("https://35photo.pro/photo_{id}/"), meta));
    }
}

async fn at_base(
    client: &Client, page_base: &Url, api_base: &Url, target: Target, max_pages: u16,
) -> Result<(Vec<(MediaItem, Metadata)>, Vec<ExtractorMessage>), Box<dyn Error + Send + Sync>> {
    match target {
        Target::Image(id) => {
            let mut url = api_base.clone();
            url.query_pairs_mut().append_pair("method", "photo.getData").append_pair("photoId", &id);
            let bytes = read_bounded(client.get(url.clone()).send().await?, &url).await?;
            let source: Value = serde_json::from_slice(&bytes)?;
            Ok((photo_records(&source, &id)?, Vec::new()))
        }
        listing => {
            let (scope, base_path) = match listing {
                Target::Tag(name) => (format!("tag:{name}"), format!("tags/{name}/list")),
                Target::Genre(id) => (format!("genre:{id}"), format!("genre_{id}")),
                Target::User(name) => (format!("user:{name}"), name),
                Target::Image(_) => unreachable!(),
            };
            let mut seen = HashSet::new();
            let mut output = Vec::new();
            for p in 1..=max_pages.clamp(1, MAX_LIST_PAGES) {
                if p > 1 {tokio::time::sleep(Duration::from_secs(1)).await;}
                let path = if scope.starts_with("tag:") {
                    format!("{base_path}_{p}/")
                } else if p == 1 {
                    format!("{base_path}/")
                } else {
                    break;
                };
                let url = page_base.join(&path)?;
                let bytes = read_bounded(client.get(url.clone()).send().await?, &url).await?;
                let text = String::from_utf8(bytes)?;
                let previous = output.len();
                list_ids(&text, &mut seen, &mut output, &scope);
                if output.len() == previous || output.len() >= MAX_CHILDREN {break;}
            }
            Ok((Vec::new(), output))
        }
    }
}

pub async fn resolve(
    client: &Client, target: Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse("https://35photo.pro/")?;
    let api = Url::parse("https://api.35photo.pro/")?;
    let (media, queues) = at_base(client, &base, &api, target, MAX_LIST_PAGES).await?;
    if media.is_empty() && !queues.is_empty() {
        return Err("35photo listing feeds require --dump-messages or --follow-queues".into());
    }
    Ok(media.into_iter().map(|(item, _)|item).collect())
}

pub async fn resolve_messages(
    client: &Client, target: Target, max_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse("https://35photo.pro/")?;
    let api = Url::parse("https://api.35photo.pro/")?;
    let (records, mut queue) = at_base(client, &base, &api, target, max_pages).await?;
    let mut events = Vec::new();
    for (item, meta) in records {
        events.push(ExtractorMessage::Directory {metadata: meta.clone()});
        events.push(ExtractorMessage::Url {url: item.url, metadata: meta});
    }
    events.append(&mut queue);
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::TcpListener};

    #[test]
    fn routes_accept_pinned_images_tags_genres_and_users_only() {
        assert_eq!(match_url("https://35photo.pro/photo_12345/"),Some(Target::Image("12345".into())));
        assert_eq!(match_url("https://35photo.pro/tags/sunset/"),Some(Target::Tag("sunset".into())));
        assert_eq!(match_url("https://35photo.pro/genre_12345/"),Some(Target::Genre("12345".into())));
        assert_eq!(match_url("https://35photo.pro/alice/"),Some(Target::User("alice".into())));
        for bad in [
            "https://evil35photo.pro/photo_12345",
            "https://35photo.pro/photo_0",
            "https://35photo.pro/photo_12345?private=1",
            "https://name:pw@35photo.pro/photo_123",
            "https://35photo.pro:8443/photo_123",
            "https://35photo.pro/photo_123/../../private",
        ] {assert!(match_url(bad).is_none(),"{bad}");}
    }

    #[test]
    fn source_photo_json_yields_validated_series_originals_and_metadata() {
        let source=serde_json::json!({"data":{"12345":{
            "photo_id":12345,"photo_name":"Autumn","photo_desc":"Golden light",
            "user_login":"alice","photo_see":"50","tags":["gold","leaves"],
            "series":[
                {"src":"https://img.35photo.pro/one.jpg","id":"1","title":"One"},
                {"src":"https://img.35photo.pro/two.png","id":2,"title":"Two"}
            ]
        }}});
        let records=photo_records(&source,"12345").unwrap();
        assert_eq!(records.len(),2);
        assert_eq!(records[0].0.filename,"12345_01.jpg");
        assert_eq!(records[1].0.filename,"12345_02.png");
        assert_eq!(records[0].1["views"],50);
        assert_eq!(records[1].1["id_series"],2);
        let forged=serde_json::json!({"data":{"12345":{"photo_id":12345,"src":"https://evil35photo.pro/secret.jpg"}}});
        assert!(photo_records(&forged,"12345").is_err());
    }

    #[test]
    fn tag_page_queues_first_seen_numeric_public_photos() {
        let mut seen=HashSet::new();let mut output=Vec::new();
        list_ids(r#"<a href="https://35photo.pro/photo_10/">One</a><a href="https://35photo.pro/photo_10/">Duplicate</a><a href="https://35photo.pro/photo_20/">Two</a><a href="https://evil35photo.pro/photo_30/">Forged</a>"#,
            &mut seen, &mut output, "tag:sunset");
        assert_eq!(output.len(),2);
        assert_eq!(serde_json::to_value(&output[1]).unwrap()["url"],
            "https://35photo.pro/photo_20/");
    }

    #[tokio::test]
    async fn api_and_two_tag_pages_resolve_public_source_requests() {
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr=listener.local_addr().unwrap();
        let server=tokio::spawn(async move {
            for (path,body) in [
                ("/?method=photo.getData&photoId=12345",r#"{"data":{"12345":{"photo_id":12345,"src":"https://img.35photo.pro/photo.jpg"}}}"#),
                ("/tags/sunset/list_1/",r#"<a href="https://35photo.pro/photo_10/">One</a>"#),
                ("/tags/sunset/list_2/",r#"<a href="https://35photo.pro/photo_20/">Two</a>"#),
                ("/tags/sunset/list_3/","")
            ] {
                let (mut socket,_)=listener.accept().await.unwrap();
                let mut request=[0u8;4096];
                let n=socket.read(&mut request).await.unwrap();
                assert!(String::from_utf8_lossy(&request[..n]).starts_with(&format!("GET {path} HTTP/1.1")));
                let reply=format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let base=Url::parse(&format!("http://{addr}/")).unwrap();
        let (records,_) = at_base(&Client::new(),&base,&base,Target::Image("12345".into()),4).await.unwrap();
        assert_eq!(records.len(),1);
        let (_,queued) = at_base(&Client::new(),&base,&base,Target::Tag("sunset".into()),4).await.unwrap();
        assert_eq!(queued.len(),2);
        server.await.unwrap();
    }
}
