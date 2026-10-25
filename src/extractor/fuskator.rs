//! Guarded public Fuskator gallery and search subset of pinned fuskator.py.
use crate::{
    message::{media_metadata, queued_url, ExtractorMessage, Metadata},
    safe_filename, MediaItem,
};
use reqwest::Client;
use serde_json::Value;
use std::{
    collections::HashSet,
    error::Error,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use url::Url;

const MAX_HTML: usize = 2 * 1024 * 1024;
const MAX_MEDIA: usize = 160;
const MAX_PAGES: u16 = 4;
const MAX_CHILDREN: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Gallery(String),
    Search(String),
}

fn valid_slug(value: &str) -> bool {
    (1..=96).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub fn match_url(input: &str) -> Option<Target> {
    let low = input.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "/../", "/./"]
        .iter()
        .any(|bad| low.contains(bad))
    {
        return None;
    }
    let url = Url::parse(input).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str() != Some("fuskator.com")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let pieces: Vec<_> = url.path_segments()?.collect();
    match pieces.as_slice() {
        ["thumbs" | "expanded", hash] | ["thumbs" | "expanded", hash, ""] if valid_slug(hash) => {
            Some(Target::Gallery((*hash).to_owned()))
        }
        ["search" | "page", tag] | ["search" | "page", tag, ""] if valid_slug(tag) => {
            Some(Target::Search(url.path().trim_end_matches('/').to_owned()))
        }
        _ => None,
    }
}

async fn bounded(
    mut response: reqwest::Response,
    origin: &Url,
) -> Result<Vec<u8>, Box<dyn Error + Send + Sync>> {
    if response.url().origin() != origin.origin() {
        return Err("Fuskator public source redirected outside its origin".into());
    }
    response = response.error_for_status()?;
    if response
        .content_length()
        .is_some_and(|n| n > MAX_HTML as u64)
    {
        return Err("Fuskator source response exceeds HTML/JSON byte limit".into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_HTML.saturating_sub(body.len()) {
            return Err("Fuskator source exceeds HTML/JSON byte limit".into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn original_media(raw: &str) -> Option<(String, String)> {
    let low = raw.to_ascii_lowercase();
    if low.contains("..") || ["%2e", "%2f", "%5c"].iter().any(|term| low.contains(term)) {
        return None;
    }
    let source = if raw.starts_with("//") {
        format!("https:{raw}")
    } else {
        raw.to_owned()
    };
    let url = Url::parse(&source).ok()?;
    let host = url.host_str()?;
    if url.scheme() != "https"
        || !(host == "fuskator.com" || host.ends_with(".fuskator.com"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let basename = url.path_segments()?.next_back()?;
    let ext = basename.rsplit_once('.')?.1.to_ascii_lowercase();
    if !matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "webp" | "gif") {
        return None;
    }
    let name = safe_filename(basename);
    if name.is_empty() {
        return None;
    }
    Some((url.to_string(), name))
}

fn gallery_records(
    html: &str,
    json: &Value,
    hash: &str,
) -> Result<Vec<(MediaItem, Metadata)>, Box<dyn Error + Send + Sync>> {
    let images = json
        .get("images")
        .and_then(Value::as_array)
        .ok_or("Fuskator public gallery JSON lacks image list")?;
    if images.is_empty() || images.len() > MAX_MEDIA {
        return Err("Fuskator public image count is outside bounded range".into());
    }
    let title = html
        .split_once("<title>")
        .and_then(|(_, rest)| rest.split_once("</title>"))
        .map(|(text, _)| text.trim().to_owned())
        .filter(|text| text.len() <= 240 && !text.chars().any(char::is_control))
        .unwrap_or_else(|| hash.to_owned());
    let gallery_id = title
        .rsplit_once('#')
        .and_then(|(_, id)| id.trim().parse::<u64>().ok());
    let tags = json
        .get("tags")
        .and_then(Value::as_str)
        .filter(|s| s.len() <= 2000 && !s.chars().any(char::is_control))
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
        .take(64)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let mut output = Vec::new();
    for (index, entry) in images.iter().enumerate() {
        let raw = entry
            .get("imageUrl")
            .and_then(Value::as_str)
            .ok_or("Fuskator image lacks an original source URL")?;
        let (url, name) = original_media(raw)
            .ok_or("Fuskator image original has an unsupported or untrusted URL")?;
        let item = MediaItem {
            url,
            filename: safe_filename(&format!("{hash}_{:03}_{name}", index + 1)),
            extractor: "fuskator",
            post_id: gallery_id,
        };
        let mut metadata = media_metadata(&item);
        metadata.insert("subcategory".into(), "gallery".into());
        metadata.insert("gallery_hash".into(), hash.into());
        metadata.insert("title".into(), title.clone().into());
        metadata.insert("num".into(), (index + 1).into());
        metadata.insert("count".into(), images.len().into());
        metadata.insert("tags".into(), serde_json::to_value(&tags)?);
        metadata.insert(
            "post_url".into(),
            format!("https://fuskator.com/thumbs/{hash}/").into(),
        );
        if let Some(id) = gallery_id {
            metadata.insert("gallery_id".into(), id.into());
        }
        for key in ["hits", "rating"] {
            if let Some(number) = json.get(key).and_then(Value::as_i64) {
                metadata.insert(key.into(), number.into());
            }
        }
        output.push((item, metadata));
    }
    Ok(output)
}

async fn gallery_at_base(
    client: &Client,
    base: &Url,
    hash: &str,
) -> Result<Vec<(MediaItem, Metadata)>, Box<dyn Error + Send + Sync>> {
    let gallery = base.join(&format!("thumbs/{hash}/index.html"))?;
    let html =
        String::from_utf8(bounded(client.get(gallery.clone()).send().await?, &gallery).await?)?;
    let api = base.join("ajax/auth.aspx")?;
    let token = String::from_utf8(
        bounded(
            client
                .post(api.clone())
                .header("Referer", gallery.as_str())
                .header("X-Requested-With", "XMLHttpRequest")
                .send()
                .await?,
            &api,
        )
        .await?,
    )?;
    let token = token.trim();
    if token.is_empty() || token.len() > 256 || token.chars().any(char::is_whitespace) {
        return Err("Fuskator public gallery supplied an invalid anonymous auth token".into());
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let mut details = base.join("ajax/gal.aspx")?;
    details
        .query_pairs_mut()
        .append_pair("X-Auth", token)
        .append_pair("hash", hash)
        .append_pair("_", &now.to_string());
    let data: Value = serde_json::from_slice(
        &bounded(
            client
                .get(details.clone())
                .header("Referer", gallery.as_str())
                .header("X-Requested-With", "XMLHttpRequest")
                .send()
                .await?,
            &details,
        )
        .await?,
    )?;
    gallery_records(&html, &data, hash)
}

fn listing_urls(html: &str, seen: &mut HashSet<String>, out: &mut Vec<ExtractorMessage>) {
    for section in html
        .split("class=\"pic_pad\"><a href=\"")
        .skip(1)
        .take(4000)
    {
        if out.len() >= MAX_CHILDREN {
            break;
        }
        let Some((link, _)) = section.split_once('"') else {
            continue;
        };
        let Ok(base) = Url::parse("https://fuskator.com/") else {
            continue;
        };
        let Ok(url) = base.join(link) else { continue };
        let Some(Target::Gallery(hash)) = match_url(url.as_str()) else {
            continue;
        };
        let canonical = format!("https://fuskator.com/thumbs/{hash}/");
        if !seen.insert(canonical.clone()) {
            continue;
        }
        let mut metadata = Metadata::new();
        metadata.insert("category".into(), "fuskator".into());
        metadata.insert("subcategory".into(), "search".into());
        metadata.insert("gallery_hash".into(), hash.into());
        out.push(queued_url(canonical, metadata));
    }
}

fn next_listing(html: &str, current: &Url) -> Option<Url> {
    let (_, tail) = html.split_once("class=\"pages\"><span>")?;
    let (pages, _) = tail.split_once(">&gt;&gt;<")?;
    let href = pages.rsplit_once("href=\"")?.1.split_once('"')?.0;
    let next = current.join(href).ok()?;
    if next.origin() != current.origin()
        || next.path() == current.path()
        || !matches!(match_url(next.as_str()), Some(Target::Search(_)))
    {
        return None;
    }
    Some(next)
}

async fn search_at_base(
    client: &Client,
    base: &Url,
    path: &str,
    max_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let mut current = base.join(path.trim_start_matches('/'))?;
    let mut seen_urls = HashSet::new();
    let mut seen_galleries = HashSet::new();
    let mut output = Vec::new();
    for page in 0..max_pages.clamp(1, MAX_PAGES) {
        if !seen_urls.insert(current.to_string()) {
            break;
        }
        if page > 0 {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        let html =
            String::from_utf8(bounded(client.get(current.clone()).send().await?, &current).await?)?;
        listing_urls(&html, &mut seen_galleries, &mut output);
        if output.len() >= MAX_CHILDREN {
            break;
        }
        let Some(next) = next_listing(&html, &current) else {
            break;
        };
        current = if base.scheme() == "http" {
            base.join(next.path().trim_start_matches('/'))?
        } else {
            next
        };
    }
    Ok(output)
}

pub async fn resolve(
    client: &Client,
    target: Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    match target {
        Target::Gallery(hash) => {
            let base = Url::parse("https://fuskator.com/")?;
            Ok(gallery_at_base(client, &base, &hash)
                .await?
                .into_iter()
                .map(|(item, _)| item)
                .collect())
        }
        Target::Search(_) => {
            Err("Fuskator search results require --dump-messages or --follow-queues".into())
        }
    }
}

pub async fn resolve_messages(
    client: &Client,
    target: Target,
    max_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse("https://fuskator.com/")?;
    match target {
        Target::Gallery(hash) => {
            let mut output = Vec::new();
            for (item, metadata) in gallery_at_base(client, &base, &hash).await? {
                output.push(ExtractorMessage::Directory {
                    metadata: metadata.clone(),
                });
                output.push(ExtractorMessage::Url {
                    url: item.url,
                    metadata,
                });
            }
            Ok(output)
        }
        Target::Search(path) => search_at_base(client, &base, &path, max_pages).await,
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
    fn public_fuskator_gallery_and_search_urls_reject_host_spoofing() {
        assert_eq!(
            match_url("https://fuskator.com/expanded/Abc_12/"),
            Some(Target::Gallery("Abc_12".into()))
        );
        assert_eq!(
            match_url("https://fuskator.com/search/cats/"),
            Some(Target::Search("/search/cats".into()))
        );
        for invalid in [
            "https://evilfuskator.com/thumbs/a",
            "https://www.fuskator.com/thumbs/a",
            "https://fuskator.com/thumbs/..",
            "https://fuskator.com/search/cats?private=1",
            "https://person:secret@fuskator.com/thumbs/a",
            "https://fuskator.com:8443/thumbs/a",
        ] {
            assert!(match_url(invalid).is_none(), "{invalid}");
        }
    }

    #[test]
    fn trusted_gallery_json_retains_source_metadata_and_ordered_images() {
        let html = "<title>Demo Gallery #123</title>";
        let payload = serde_json::json!({"hits":42,"rating":5,"tags":"a, b, c","images":[
            {"imageUrl":"//cdn.fuskator.com/media/one.jpg"},
            {"imageUrl":"//cdn.fuskator.com/media/two.png"}
        ]});
        let files = gallery_records(html, &payload, "Abc12").unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].0.url, "https://cdn.fuskator.com/media/one.jpg");
        assert_eq!(files[1].0.filename, "Abc12_002_two.png");
        assert_eq!(files[0].1["gallery_id"], 123);
        assert_eq!(files[0].1["tags"][1], "b");
        assert_eq!(files[0].1["hits"], 42);
        let bad = serde_json::json!({"images":[{"imageUrl":"//evil-fuskator.com/media/one.jpg"}]});
        assert!(gallery_records(html, &bad, "Abc12").is_err());
    }

    #[test]
    fn bounded_search_deduplicates_scoped_gallery_anchors() {
        let html = r#"<div class="pic_pad"><a href="/thumbs/Abc12">A</a><div class="pic_pad"><a href="/expanded/Abc12">again</a><div class="pic_pad"><a href="https://evilfuskator.com/thumbs/fake">Forged</a><div class="pic_pad"><a href="/thumbs/Def34">B</a>"#;
        let mut ids = HashSet::new();
        let mut events = Vec::new();
        listing_urls(html, &mut ids, &mut events);
        assert_eq!(events.len(), 2);
        assert_eq!(
            serde_json::to_value(&events[1]).unwrap()["url"],
            "https://fuskator.com/thumbs/Def34/"
        );
    }

    #[test]
    fn bounded_search_next_page_uses_last_link_and_rejects_foreign_hosts() {
        let current = Url::parse("https://fuskator.com/search/cats").unwrap();
        let markup = r#"<div class="pages"><span><a href="/page/one">1</a><a href="/page/two">2</a><a href="/page/three">&gt;&gt;</a>"#;
        assert_eq!(
            next_listing(markup, &current).unwrap().path(),
            "/page/three"
        );
        assert!(next_listing(
            r#"<div class="pages"><span><a href="https://evil-fuskator.com/page/two">&gt;&gt;</a>"#,
            &current
        )
        .is_none());
    }

    #[tokio::test]
    async fn album_html_anonymous_token_and_json_make_three_local_requests() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (route, body) in [
                (
                    "GET /thumbs/Abc12/index.html",
                    "<title>My Album #123</title>",
                ),
                ("POST /ajax/auth.aspx", "token123"),
                (
                    "GET /ajax/gal.aspx?",
                    r#"{"images":[{"imageUrl":"//cdn.fuskator.com/a.jpg"}],"hits":5,"rating":2}"#,
                ),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = [0u8; 4096];
                let size = socket.read(&mut bytes).await.unwrap();
                assert!(String::from_utf8_lossy(&bytes[..size]).starts_with(route));
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{address}/")).unwrap();
        let files = gallery_at_base(&Client::new(), &base, "Abc12")
            .await
            .unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].0.filename, "Abc12_001_a.jpg");
        server.await.unwrap();
    }
}
