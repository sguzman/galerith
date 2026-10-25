//! Guarded public Fapachi post originals and deferred user-page queues.
//! Independent bounded subset of pinned gallery-dl fapachi.py.
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
    Post { user: String, id: u64 },
    User { user: String, start: u16 },
}

fn valid_user(raw: &str) -> bool {
    (1..=80).contains(&raw.len())
        && raw
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        && raw != "search"
}

fn numeric(raw: &str) -> Option<u64> {
    if !(1..=18).contains(&raw.len()) || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    raw.parse::<u64>().ok().filter(|n| *n > 0)
}

pub fn match_url(raw: &str) -> Option<Target> {
    let lower = raw.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "/../", "/./"]
        .iter()
        .any(|s| lower.contains(s))
    {
        return None;
    }
    let url = Url::parse(raw).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !matches!(url.host_str(), Some("fapachi.com" | "www.fapachi.com"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let parts: Vec<_> = url.path_segments()?.collect();
    let user = *parts.first()?;
    if !valid_user(user) {
        return None;
    }
    match parts.as_slice() {
        [_, "media", id] | [_, "media", id, ""] => Some(Target::Post {
            user: user.into(),
            id: numeric(id)?,
        }),
        [_] | [_, ""] => Some(Target::User {
            user: user.into(),
            start: 1,
        }),
        [_, "page", page] | [_, "page", page, ""] => {
            let start = page.parse::<u16>().ok()?;
            (1..=MAX_PAGES).contains(&start).then_some(Target::User {
                user: user.into(),
                start,
            })
        }
        _ => None,
    }
}

fn same_origin(url: &Url, base: &Url) -> bool {
    url.origin() == base.origin()
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
}

fn trusted_link(root: &Url, raw: &str) -> Option<Url> {
    if raw.is_empty()
        || raw.starts_with("//")
        || raw.contains('%')
        || raw.contains('\\')
        || raw.contains("..")
    {
        return None;
    }
    let url = root.join(raw).ok()?;
    (same_origin(&url, root) && !url.path().contains('%')).then_some(url)
}

fn parse_post(
    html: &str,
    user: &str,
    id: u64,
) -> Result<(MediaItem, Metadata), Box<dyn Error + Send + Sync>> {
    let (_, source) = html
        .split_once("class=\"media-img")
        .ok_or("Fapachi public post lacks original image region")?;
    let raw = source
        .split_once("data-src=\"")
        .and_then(|(_, text)| text.split_once('"'))
        .map(|(url, _)| url)
        .ok_or("Fapachi original media path unavailable")?;
    let root = Url::parse("https://fapachi.com/")?;
    let url = trusted_link(&root, raw).ok_or("Fapachi original media path is untrusted")?;
    let basename = url
        .path_segments()
        .and_then(|mut x| x.next_back())
        .ok_or("Fapachi original media filename missing")?;
    let ext = basename
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .ok_or("Fapachi media file extension missing")?;
    if !matches!(
        ext.as_str(),
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "mp4" | "webm"
    ) {
        return Err("Unsupported Fapachi original extension".into());
    }
    let filename = safe_filename(&format!("{user}_{id}.{ext}"));
    let item = MediaItem {
        url: url.to_string(),
        filename,
        extractor: "fapachi",
        post_id: Some(id),
    };
    let mut metadata = media_metadata(&item);
    metadata.insert("subcategory".into(), "post".into());
    metadata.insert("user".into(), user.into());
    metadata.insert("id".into(), id.into());
    metadata.insert("extension".into(), ext.into());
    metadata.insert(
        "post_url".into(),
        format!("https://fapachi.com/{user}/media/{id}").into(),
    );
    Ok((item, metadata))
}

fn anchor_href(attrs: &str) -> Option<&str> {
    attrs
        .split_once("href=\"")?
        .1
        .split_once('"')
        .map(|(href, _)| href)
}

fn parse_listing(
    html: &str,
    user: &str,
    seen: &mut HashSet<u64>,
    output: &mut Vec<ExtractorMessage>,
) {
    for snippet in html.split("model-media-prew\">").skip(1).take(4000) {
        if output.len() >= MAX_CHILDREN {
            break;
        }
        let Some((body, _)) = snippet.split_once("</a>") else {
            continue;
        };
        let Some(href) = body.split_once("<a ").and_then(|(_, x)| anchor_href(x)) else {
            continue;
        };
        let Ok(base) = Url::parse("https://fapachi.com/") else {
            continue;
        };
        let Some(url) = trusted_link(&base, href) else {
            continue;
        };
        let Some(Target::Post {
            user: candidate,
            id,
        }) = match_url(url.as_str())
        else {
            continue;
        };
        if candidate != user || !seen.insert(id) {
            continue;
        }
        let mut metadata = Metadata::new();
        metadata.insert("category".into(), "fapachi".into());
        metadata.insert("subcategory".into(), "user".into());
        metadata.insert("user".into(), user.into());
        metadata.insert("id".into(), id.into());
        output.push(queued_url(
            format!("https://fapachi.com/{user}/media/{id}"),
            metadata,
        ));
    }
}

fn next_page(html: &str, user: &str, page: u16) -> bool {
    let expected = format!("/{user}/page/{}", page + 1);
    let Ok(base) = Url::parse("https://fapachi.com/") else {
        return false;
    };
    for chunk in html.split("<a ").skip(1).take(4000) {
        let Some((attrs, _)) = chunk.split_once("</a>") else {
            continue;
        };
        if !attrs.contains(">Next page") {
            continue;
        }
        let Some(href) = anchor_href(attrs) else {
            continue;
        };
        if trusted_link(&base, href).is_some_and(|url| url.path() == expected) {
            return true;
        }
    }
    false
}

async fn read_html(client: &Client, url: Url) -> Result<String, Box<dyn Error + Send + Sync>> {
    let mut response = client.get(url.clone()).send().await?.error_for_status()?;
    if response.url().origin() != url.origin() {
        return Err("Fapachi page redirected outside its origin".into());
    }
    if response
        .content_length()
        .is_some_and(|size| size > MAX_HTML as u64)
    {
        return Err("Fapachi response exceeds public byte limit".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_HTML.saturating_sub(bytes.len()) {
            return Err("Fapachi response exceeds public byte limit".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8(bytes)?)
}

async fn post_at_base(
    client: &Client,
    base: &Url,
    user: &str,
    id: u64,
) -> Result<(MediaItem, Metadata), Box<dyn Error + Send + Sync>> {
    let html = read_html(client, base.join(&format!("{user}/media/{id}"))?).await?;
    parse_post(&html, user, id)
}

async fn listing_at_base(
    client: &Client,
    base: &Url,
    user: &str,
    start: u16,
    max_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let mut seen = HashSet::new();
    let mut output = Vec::new();
    let end = start
        .saturating_add(max_pages.clamp(1, MAX_PAGES))
        .min(MAX_PAGES + 1);
    for page in start..end {
        if page > start {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        let body = read_html(client, base.join(&format!("{user}/page/{page}"))?).await?;
        parse_listing(&body, user, &mut seen, &mut output);
        if output.len() >= MAX_CHILDREN || !next_page(&body, user, page) {
            break;
        }
    }
    Ok(output)
}

pub async fn resolve(
    client: &Client,
    target: Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    match target {
        Target::Post { user, id } => Ok(vec![
            post_at_base(client, &Url::parse("https://fapachi.com/")?, &user, id)
                .await?
                .0,
        ]),
        Target::User { .. } => {
            Err("Fapachi user feeds require --dump-messages or --follow-queues".into())
        }
    }
}

pub async fn resolve_messages(
    client: &Client,
    target: Target,
    max_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse("https://fapachi.com/")?;
    match target {
        Target::Post { user, id } => {
            let (item, metadata) = post_at_base(client, &base, &user, id).await?;
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
        Target::User { user, start } => {
            listing_at_base(client, &base, &user, start, max_pages).await
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
    fn pins_first_party_posts_and_user_feed_routes_only() {
        assert_eq!(
            match_url("https://fapachi.com/model/media/12345"),
            Some(Target::Post {
                user: "model".into(),
                id: 12345
            })
        );
        assert_eq!(
            match_url("https://www.fapachi.com/model/page/2"),
            Some(Target::User {
                user: "model".into(),
                start: 2
            })
        );
        for bad in [
            "https://fakefapachi.com/model/media/12345",
            "https://fapachi.com/model/media/0",
            "https://fapachi.com/search/media/12345",
            "https://fapachi.com/model/page/5",
            "https://fapachi.com/model?private=1",
            "https://name:password@fapachi.com/model",
            "https://fapachi.com:8443/model",
            "https://fapachi.com/model/../../secret",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn post_original_preserves_id_filename_and_rejects_hostile_paths() {
        let html = r#"<div class="media-img" data-src="/uploads/model/photo.jpg"></div>"#;
        let (item, data) = parse_post(html, "model", 12345).unwrap();
        assert_eq!(item.filename, "model_12345.jpg");
        assert_eq!(item.url, "https://fapachi.com/uploads/model/photo.jpg");
        assert_eq!(data["id"], 12345);
        assert_eq!(data["user"], "model");
        assert_eq!(data["subcategory"], "post");
        for bad in [
            r#"<div class="media-img" data-src="//evil.test/private.png">"#,
            r#"<div class="media-img" data-src="https://fakefapachi.com/private.png">"#,
            r#"<div class="media-img" data-src="/uploads/../secret.png">"#,
            r#"<div class="media-img" data-src="/uploads/%2e%2e/secret.png">"#,
        ] {
            assert!(parse_post(bad, "model", 12345).is_err(), "{bad}");
        }
    }

    #[test]
    fn listing_queues_first_seen_same_user_original_posts() {
        let html = r#"<div class="model-media-prew"><a href="/model/media/12345">a</a></div><div class="model-media-prew"><a href="/model/media/12345">dup</a></div><div class="model-media-prew"><a href="/other/media/12346">foreign</a></div><div class="model-media-prew"><a href="https://fakefapachi.com/model/media/999">bad</a></div><div class="model-media-prew"><a href="/model/media/12347">b</a></div><a href="/model/page/2">Next page</a>"#;
        let mut found = HashSet::new();
        let mut output = Vec::new();
        parse_listing(html, "model", &mut found, &mut output);
        assert_eq!(output.len(), 2);
        assert_eq!(
            serde_json::to_value(&output[0]).unwrap()["url"],
            "https://fapachi.com/model/media/12345"
        );
        assert!(next_page(html, "model", 1));
        assert!(!next_page(
            r#"<a href="https://fakefapachi.com/model/page/2">Next page</a>"#,
            "model",
            1
        ));
    }

    #[tokio::test]
    async fn bounded_post_and_two_user_pages_use_three_loopback_gets() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (route, body) in [
                (
                    "/model/media/12345",
                    r#"<div class="media-img" data-src="/uploads/original.jpg">"#,
                ),
                (
                    "/model/page/1",
                    r#"<div class="model-media-prew"><a href="/model/media/12345">first</a></div><a href="/model/page/2">Next page</a>"#,
                ),
                (
                    "/model/page/2",
                    r#"<div class="model-media-prew"><a href="/model/media/12345">dup</a></div><div class="model-media-prew"><a href="/model/media/12347">second</a></div>"#,
                ),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0u8; 4096];
                let count = socket.read(&mut request).await.unwrap();
                assert!(String::from_utf8_lossy(&request[..count])
                    .starts_with(&format!("GET {route} HTTP/1.1")));
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{address}/")).unwrap();
        let post = post_at_base(&Client::new(), &base, "model", 12345)
            .await
            .unwrap();
        assert_eq!(post.0.filename, "model_12345.jpg");
        let queues = listing_at_base(&Client::new(), &base, "model", 1, 4)
            .await
            .unwrap();
        assert_eq!(queues.len(), 2);
        assert_eq!(
            serde_json::to_value(&queues[1]).unwrap()["url"],
            "https://fapachi.com/model/media/12347"
        );
        server.await.unwrap();
    }
}
