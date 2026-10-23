//! Independent public ImgBB/ibb.co image and first-page album preview.
//! Album pagination, user accounts, authenticated views, and upstream metadata,
//! filename/archive formatting and live parity are not implemented.
use crate::{safe_filename, MediaItem};
use reqwest::Client;
use serde_json::Value;
use std::{collections::HashSet, error::Error};
use url::Url;

const MAX_HTML_BYTES: usize = 2 * 1024 * 1024;
const MAX_ALBUM_IMAGES: usize = 2000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Image { id: String },
    Album { id: String },
}

fn valid_id(id: &str) -> bool {
    (3..=128).contains(&id.len())
        && id
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'-' | b'_'))
}

pub fn match_url(input: &str) -> Option<Target> {
    let url = Url::parse(input).ok()?;
    if url.scheme() != "https" || url.host_str() != Some("ibb.co") {
        return None;
    }
    let parts: Vec<_> = url.path_segments()?.collect();
    match parts.as_slice() {
        ["album", id] if valid_id(id) => Some(Target::Album { id: (*id).into() }),
        [id] if valid_id(id) && !matches!(*id, "login" | "signup" | "album" | "about") => {
            Some(Target::Image { id: (*id).into() })
        }
        _ => None,
    }
}

fn public_image(raw: &str, preferred_name: Option<&str>) -> Option<MediaItem> {
    let url = Url::parse(raw).ok()?;
    if url.scheme() != "https" || !matches!(url.host_str(), Some("i.ibb.co" | "image.ibb.co")) {
        return None;
    }
    let extension = url.path().rsplit_once('.')?.1;
    if extension.is_empty()
        || extension.len() > 12
        || !extension.bytes().all(|b| b.is_ascii_alphanumeric())
    {
        return None;
    }
    let basename = url.path_segments()?.next_back()?;
    let name = preferred_name
        .filter(|s| !s.is_empty() && s.len() <= 240)
        .unwrap_or(basename);
    let mut filename = safe_filename(name);
    if matches!(filename.as_str(), "" | "." | "..") {
        return None;
    }
    if !filename.contains('.') {
        filename.push('.');
        filename.push_str(extension);
    }
    Some(MediaItem {
        url: url.to_string(),
        filename,
        extractor: "imgbb",
        post_id: None,
    })
}

fn image_page(html: &str) -> Option<MediaItem> {
    for marker in [
        "property=\"og:image\" content=\"",
        "\"og:image\" content=\"",
    ] {
        if let Some((_, right)) = html.split_once(marker) {
            if let Some((raw, _)) = right.split_once('"') {
                if let Some(item) = public_image(raw, None) {
                    return Some(item);
                }
            }
        }
    }
    None
}

fn album_page(html: &str) -> Vec<MediaItem> {
    let mut images = Vec::new();
    let mut seen = HashSet::new();
    for part in html.split("data-object='").skip(1).take(MAX_ALBUM_IMAGES) {
        let Some((raw, _)) = part.split_once('\'') else {
            continue;
        };
        let obj = match serde_json::from_str::<Value>(raw) {
            Ok(value) => value,
            Err(_) => continue,
        };
        let Some(image) = obj.get("image") else {
            continue;
        };
        let Some(url) = image.get("url").and_then(Value::as_str) else {
            continue;
        };
        let name = image
            .get("name")
            .and_then(Value::as_str)
            .or_else(|| image.get("filename").and_then(Value::as_str));
        if let Some(item) = public_image(url, name) {
            if seen.insert(item.url.clone()) {
                images.push(item);
            }
        }
    }
    images
}

async fn read_html(client: &Client, url: Url) -> Result<String, Box<dyn Error + Send + Sync>> {
    let mut response = client.get(url).send().await?.error_for_status()?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_HTML_BYTES as u64)
    {
        return Err("ImgBB HTML exceeds the configured response bound".into());
    }
    let mut content = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if content.len().saturating_add(chunk.len()) > MAX_HTML_BYTES {
            return Err("ImgBB HTML exceeds the configured response bound".into());
        }
        content.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8(content)?)
}

async fn resolve_at_base(
    client: &Client,
    base: &Url,
    target: Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    match target {
        Target::Image { id } => {
            let html = read_html(client, base.join(&id)?).await?;
            Ok(image_page(&html).into_iter().collect())
        }
        Target::Album { id } => {
            let html = read_html(client, base.join(&format!("album/{id}"))?).await?;
            Ok(album_page(&html))
        }
    }
}

pub async fn resolve(
    client: &Client,
    target: Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    resolve_at_base(client, &Url::parse("https://ibb.co/")?, target).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn img_bb_routes_are_exact_and_bounded() {
        assert_eq!(
            match_url("https://ibb.co/album/myAlbum99"),
            Some(Target::Album {
                id: "myAlbum99".into(),
            })
        );
        assert_eq!(
            match_url("https://ibb.co/someImage12"),
            Some(Target::Image {
                id: "someImage12".into(),
            })
        );
        for url in [
            "http://ibb.co/foobar",
            "https://notibb.co/foobar",
            "https://ibb.co/login",
            "https://ibb.co/album/",
            "https://ibb.co/album/a%2Fb",
            "https://ibb.co/foobar/other",
        ] {
            assert!(match_url(url).is_none(), "{url}");
        }
    }

    #[test]
    fn image_meta_extracts_only_approved_cdn_media() {
        let image = r#"<meta property="og:image" content="https://i.ibb.co/abc/image.webp">"#;
        let item = image_page(image).unwrap();
        assert_eq!(item.filename, "image.webp");
        assert_eq!(item.url, "https://i.ibb.co/abc/image.webp");
        let fake = r#"<meta property="og:image" content="https://evil.example/image.png">"#;
        assert!(image_page(fake).is_none());
        assert!(image_page("404: missing").is_none());
    }

    #[test]
    fn album_objects_preserve_order_and_deduplicate_cdn_images() {
        let page = concat!(
            "<div data-object='{\"image\":{\"url\":\"https://i.ibb.co/one/one.jpg\",\"name\":\"first.jpg\"}}'>",
            "<div data-object='{\"image\":{\"url\":\"https://i.ibb.co/two/two.png\",\"name\":\"second.png\"}}'>",
            "<div data-object='{\"image\":{\"url\":\"https://i.ibb.co/one/one.jpg\"}}'>",
            "<div data-object='{\"image\":{\"url\":\"https://attacker.test/x.png\"}}'>",
        );
        let media = album_page(page);
        assert_eq!(media.len(), 2);
        assert_eq!(media[0].filename, "first.jpg");
        assert_eq!(media[1].filename, "second.png");
    }

    #[tokio::test]
    async fn image_and_album_read_public_html_through_local_api() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (route, body) in [
                (
                    "GET /image123 HTTP/1.1",
                    r#"<meta property="og:image" content="https://i.ibb.co/abc/direct.jpg">"#,
                ),
                (
                    "GET /album/album123 HTTP/1.1",
                    r#"<div data-object='{"image":{"url":"https://i.ibb.co/abc/one.jpg","name":"one.jpg"}}'>"#,
                ),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buf = [0_u8; 4096];
                let len = socket.read(&mut buf).await.unwrap();
                assert!(String::from_utf8_lossy(&buf[..len]).starts_with(route));
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let image = resolve_at_base(
            &Client::new(),
            &base,
            Target::Image {
                id: "image123".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(image.len(), 1);
        assert_eq!(image[0].filename, "direct.jpg");
        let album = resolve_at_base(
            &Client::new(),
            &base,
            Target::Album {
                id: "album123".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(album.len(), 1);
        assert_eq!(album[0].filename, "one.jpg");
        server.await.unwrap();
    }
}
