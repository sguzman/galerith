//! Independent Adobe Lightroom public shared-album extractor preview.
//! Shared private/authenticated flows, non-Adobe asset hosts, rich metadata,
//! per-site archive/filename templates and live upstream parity are unfinished.
use crate::{safe_filename, MediaItem};
use reqwest::Client;
use serde_json::Value;
use std::{collections::HashSet, error::Error};
use url::Url;

const ROOT: &str = "https://lightroom.adobe.com/";
const MAX_DOCUMENT: usize = 2 * 1024 * 1024;
const MAX_ASSETS: usize = 2000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Share { id: String },
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub fn match_url(raw: &str) -> Option<Target> {
    if raw.to_ascii_lowercase().contains("%2e") {
        return None;
    }
    let url = Url::parse(raw).ok()?;
    if url.scheme() != "https" || url.host_str() != Some("lightroom.adobe.com") {
        return None;
    }
    match url.path_segments()?.collect::<Vec<_>>().as_slice() {
        ["shares", id] if valid_id(id) => Some(Target::Share {
            id: (*id).to_owned(),
        }),
        _ => None,
    }
}

fn adobe_host(host: &str) -> bool {
    ["adobe.com", "adobe.io", "adobe.net", "adobecdn.com"]
        .iter()
        .any(|base| host == *base || host.ends_with(&format!(".{base}")))
}

fn allowed_endpoint(url: &Url, test_base: &Url) -> bool {
    (url.scheme() == "https" && url.host_str().is_some_and(adobe_host))
        || (url.origin() == test_base.origin() && url.path().starts_with('/'))
}

fn scoped_url(raw: &str, base: &Url, test_base: &Url) -> Option<Url> {
    let target = base.join(raw).ok()?;
    allowed_endpoint(&target, test_base).then_some(target)
}

fn album_data(html: &str) -> Option<Value> {
    let json = html.split_once("albumAttributes: ")?.1.split_once('\n')?.0;
    (json.len() <= MAX_DOCUMENT)
        .then(|| serde_json::from_str::<Value>(json).ok())
        .flatten()
}

fn strip_xssi(bytes: &[u8]) -> &[u8] {
    // Adobe album page data can carry a JavaScript-loop prelude on line one.
    let src = match bytes.first().copied() {
        Some(b'{') | Some(b'[') => bytes,
        _ => bytes
            .iter()
            .position(|b| *b == b'\n')
            .map_or(bytes, |pos| &bytes[pos + 1..]),
    };
    src
}

fn rendition_url(asset: &Value, base: &Url, test_base: &Url) -> Option<Url> {
    let links = asset.get("links")?.as_object()?;
    links
        .iter()
        .filter_map(|(type_name, rendition)| {
            let resolution = type_name
                .strip_prefix("/rels/rendition_type/")?
                .parse::<u64>()
                .ok()?;
            let href = rendition.get("href")?.as_str()?;
            Some((resolution, scoped_url(href, base, test_base)?))
        })
        .filter(|(resolution, _)| *resolution > 0)
        .max_by_key(|(resolution, _)| *resolution)
        .map(|(_, url)| url)
}

fn media_filename(url: &Url, id: &str, index: usize) -> Option<String> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return None;
    }
    let basename = url.path_segments()?.next_back()?;
    let extension = basename.rsplit_once('.')?.1;
    if extension.is_empty()
        || extension.len() > 12
        || !extension.bytes().all(|b| b.is_ascii_alphanumeric())
    {
        return None;
    }
    let clean = safe_filename(id);
    (!matches!(clean.as_str(), "" | "." | "..")).then(|| format!("{index:04}_{clean}.{extension}"))
}

fn page_items(
    page: &Value,
    album_base: &Url,
    test_base: &Url,
    start_index: usize,
    seen: &mut HashSet<String>,
) -> Vec<MediaItem> {
    let Some(resources) = page.get("resources").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for resource in resources.iter().take(MAX_ASSETS) {
        let Some(asset) = resource.get("asset") else {
            continue;
        };
        let Some(id) = asset.get("id").and_then(Value::as_str) else {
            continue;
        };
        let Some(url) = rendition_url(asset, album_base, test_base) else {
            continue;
        };
        let Some(filename) = media_filename(&url, id, start_index + out.len()) else {
            continue;
        };
        if seen.insert(id.to_owned()) {
            out.push(MediaItem {
                url: url.to_string(),
                filename,
                extractor: "lightroom",
                post_id: id.parse::<u64>().ok(),
            });
        }
    }
    out
}

async fn read_limited(client: &Client, url: Url) -> Result<Vec<u8>, Box<dyn Error + Send + Sync>> {
    let mut response = client.get(url).send().await?.error_for_status()?;
    if response
        .content_length()
        .is_some_and(|len| len > MAX_DOCUMENT as u64)
    {
        return Err("Lightroom gallery response exceeds 2 MiB".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len().saturating_add(chunk.len()) > MAX_DOCUMENT {
            return Err("Lightroom gallery response exceeds 2 MiB".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

async fn resolve_at_base(
    client: &Client,
    root: &Url,
    target: Target,
    max_pages: u16,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let Target::Share { id } = target;
    let html = String::from_utf8(read_limited(client, root.join(&format!("shares/{id}"))?).await?)?;
    let album = album_data(&html).ok_or("Lightroom public share missing albumAttributes JSON")?;
    let mut base = album
        .get("base")
        .and_then(Value::as_str)
        .and_then(|raw| scoped_url(raw, root, root))
        .ok_or("Lightroom album returned an invalid API base")?;
    let first = album
        .pointer("/links/~1rels~1space_album_images_videos/href")
        .and_then(Value::as_str)
        .ok_or("Lightroom album missing images/videos API link")?;
    let mut next =
        scoped_url(first, &base, root).ok_or("Lightroom image-list link has an unapproved host")?;
    let mut result = Vec::new();
    let mut seen_assets = HashSet::new();
    let mut visited = HashSet::new();
    for index in 0..max_pages.clamp(1, 100) {
        if !visited.insert(next.to_string()) {
            break;
        }
        if index > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
        let bytes = read_limited(client, next.clone()).await?;
        let page: Value = serde_json::from_slice(strip_xssi(&bytes))?;
        let resources = page
            .get("resources")
            .and_then(Value::as_array)
            .ok_or("Lightroom image-list response missing resources")?;
        if resources.len() > MAX_ASSETS {
            return Err("Lightroom image-list page exceeds the media cap".into());
        }
        if let Some(new_base) = page.get("base").and_then(Value::as_str) {
            base = scoped_url(new_base, &base, root)
                .ok_or("Lightroom API page returned an unapproved base")?;
        }
        result.extend(page_items(
            &page,
            &base,
            root,
            result.len() + 1,
            &mut seen_assets,
        ));
        if result.len() >= MAX_ASSETS || resources.is_empty() {
            break;
        }
        let Some(link) = page.pointer("/links/next/href").and_then(Value::as_str) else {
            break;
        };
        let Some(candidate) = scoped_url(link, &base, root) else {
            break;
        };
        next = candidate;
    }
    result.truncate(MAX_ASSETS);
    Ok(result)
}

pub async fn resolve(
    client: &Client,
    target: Target,
    max_pages: u16,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    resolve_at_base(client, &Url::parse(ROOT)?, target, max_pages).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn exact_share_routes_and_off_host_media_rejected() {
        assert_eq!(
            match_url("https://lightroom.adobe.com/shares/0123456789abcdef"),
            Some(Target::Share {
                id: "0123456789abcdef".into(),
            })
        );
        for input in [
            "http://lightroom.adobe.com/shares/abc",
            "https://evil.lightroom.adobe.com/shares/abc",
            "https://lightroom.adobe.com/shares/GGG",
            "https://lightroom.adobe.com/shares/abc/extra",
            "https://lightroom.adobe.com/shares/a%2Fb",
        ] {
            assert!(match_url(input).is_none(), "{input}");
        }
        let base = Url::parse(ROOT).unwrap();
        assert!(scoped_url("https://files.adobe.com/x.jpg", &base, &base).is_some());
        assert!(scoped_url("https://adobe.com.evil.test/x.jpg", &base, &base).is_none());
        assert!(scoped_url("file:///etc/passwd", &base, &base).is_none());
    }

    #[test]
    fn choose_largest_available_adobe_rendition_and_ignore_duplicates() {
        let page = json!({"resources":[
            {"asset":{"id":"image1","links":{
                "/rels/rendition_type/320":{"href":"/image-small.jpg"},
                "/rels/rendition_type/2560":{"href":"/image-large.webp"}}}},
            {"asset":{"id":"image1","links":{
                "/rels/rendition_type/3000":{"href":"/image-duplicate.png"}}}},
            {"asset":{"id":"image2","links":{
                "/rels/rendition_type/1024":{"href":"https://evil.example.org/bad.jpg"}}}},
            {"asset":{"id":"image3","links":{
                "/rels/rendition_type/1280":{"href":"https://assets.adobe.io/photo.png"}}}}
        ]});
        let base = Url::parse(ROOT).unwrap();
        let mut seen = HashSet::new();
        let images = page_items(&page, &base, &base, 1, &mut seen);
        assert_eq!(images.len(), 2);
        assert_eq!(images[0].filename, "0001_image1.webp");
        assert_eq!(
            images[0].url,
            "https://lightroom.adobe.com/image-large.webp"
        );
        assert_eq!(images[1].filename, "0002_image3.png");
    }

    #[tokio::test]
    async fn shared_album_resolves_two_paginated_pages_via_local_http() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let base = format!("http://{address}/");
            let album = format!(
                r#"<script>albumAttributes: {{"base":"{base}","links":{{"/rels/space_album_images_videos":{{"href":"/album/page1"}}}},"payload":{{"name":"Nature"}}}}
</script>"#,
            );
            let first = r#"while(1);
{"base":"https://lightroom.adobe.com/","resources":[{"asset":{"id":"first","links":{"/rels/rendition_type/400":{"href":"https://assets.adobe.io/first.jpg"},"/rels/rendition_type/1200":{"href":"https://assets.adobe.io/first-large.jpg"}}}}],"links":{"next":{"href":"NEXT_PAGE_MARKER"}}}"#;
            let second = r#"while(1);
{"resources":[{"asset":{"id":"second","links":{"/rels/rendition_type/800":{"href":"https://assets.adobe.io/second.webp"}}}}],"links":{}}"#;
            for (expected, body) in [
                ("GET /shares/abcdef HTTP/1.1", album),
                (
                    "GET /album/page1 HTTP/1.1",
                    first.replace("NEXT_PAGE_MARKER", &format!("{base}album/page2")),
                ),
                ("GET /album/page2 HTTP/1.1", second.to_owned()),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut req = [0_u8; 4096];
                let count = socket.read(&mut req).await.unwrap();
                let request = String::from_utf8_lossy(&req[..count]);
                assert!(request.starts_with(expected), "{request}");
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let root = Url::parse(&format!("http://{address}/")).unwrap();
        let result = resolve_at_base(
            &Client::new(),
            &root,
            Target::Share {
                id: "abcdef".into(),
            },
            2,
        )
        .await
        .unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].filename, "0001_first.jpg");
        assert_eq!(
            result[0].url,
            "https://assets.adobe.io/first-large.jpg"
        );
        assert_eq!(result[1].filename, "0002_second.webp");
        server.await.unwrap();
    }
}
