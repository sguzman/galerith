//! Independent public Imgth gallery extractor preview.
//! Full metadata, archive templates, authentication and live upstream parity
//! remain outstanding.
use crate::{safe_filename, MediaItem};
use reqwest::Client;
use std::{collections::HashSet, error::Error};
use url::Url;

const MAX_HTML_BYTES: usize = 2 * 1024 * 1024;
const MAX_GALLERY_IMAGES: usize = 2000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Gallery { id: String },
}

pub fn match_url(input: &str) -> Option<Target> {
    let url = Url::parse(input).ok()?;
    if url.scheme() != "https"
        || !matches!(url.host_str(), Some("imgth.com" | "www.imgth.com"))
    {
        return None;
    }
    let parts: Vec<_> = url.path_segments()?.collect();
    let id = match parts.as_slice() {
        ["gallery", id] | ["gallery", id, _] => *id,
        _ => return None,
    };
    if id.is_empty() || id.len() > 18 || !id.bytes().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    Some(Target::Gallery { id: id.to_owned() })
}

fn original_from_thumb(raw: &str) -> Option<MediaItem> {
    let pos = raw.find("/thumbs/")?;
    let prefix = &raw[..pos];
    if !prefix.is_empty() && prefix != "https://imgth.com" && prefix != "https://www.imgth.com" {
        return None;
    }
    let path = &raw[pos + "/thumbs/".len()..];
    if path.is_empty()
        || path.len() > 512
        || path.split('/').any(|s| s.is_empty() || matches!(s, "." | ".."))
        || !path
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'/' | b'.' | b'-' | b'_'))
    {
        return None;
    }
    let last = path.rsplit('/').next()?;
    let ext = last.rsplit_once('.')?.1;
    if ext.is_empty() || ext.len() > 12 {
        return None;
    }
    let mut url = Url::parse("https://imgth.com/images/").ok()?;
    url.set_path(&format!("/images/{path}"));
    Some(MediaItem {
        url: url.to_string(),
        filename: safe_filename(last),
        extractor: "imgth",
        post_id: None,
    })
}

fn thumbnails(html: &str) -> Vec<MediaItem> {
    let Some((_, rest)) = html.split_once("<ul class=\"thumbnails\">") else {
        return Vec::new();
    };
    let Some((section, _)) = rest.split_once("</ul>") else {
        return Vec::new();
    };
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    for chunk in section.split("<img src=\"").skip(1).take(MAX_GALLERY_IMAGES) {
        if let Some((raw, _)) = chunk.split_once('"') {
            if let Some(item) = original_from_thumb(raw) {
                if seen.insert(item.url.clone()) {
                    result.push(item);
                }
            }
        }
    }
    result
}

async fn read_html(client: &Client, url: Url) -> Result<String, Box<dyn Error + Send + Sync>> {
    let mut response = client.get(url).send().await?.error_for_status()?;
    if response
        .content_length()
        .is_some_and(|n| n > MAX_HTML_BYTES as u64)
    {
        return Err("Imgth page HTML exceeds the configured safety bound".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len().saturating_add(chunk.len()) > MAX_HTML_BYTES {
            return Err("Imgth page HTML exceeds the configured safety bound".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8(bytes)?)
}

async fn resolve_at_base(
    client: &Client,
    base: &Url,
    target: Target,
    max_pages: u16,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    match target {
        Target::Gallery { id } => {
            let mut media = Vec::new();
            let mut seen = HashSet::new();
            for page in 0..max_pages.clamp(1, 100) {
                if page > 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                }
                let path = if page == 0 {
                    format!("gallery/{id}/g/")
                } else {
                    format!("gallery/{id}/g/page/{page}")
                };
                let html = read_html(client, base.join(&path)?).await?;
                for item in thumbnails(&html) {
                    if seen.insert(item.url.clone()) {
                        media.push(item);
                    }
                    if media.len() >= MAX_GALLERY_IMAGES {
                        break;
                    }
                }
                if media.len() >= MAX_GALLERY_IMAGES || !html.contains("<li class=\"next\">") {
                    break;
                }
            }
            Ok(media)
        }
    }
}

pub async fn resolve(
    client: &Client,
    target: Target,
    max_pages: u16,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    resolve_at_base(client, &Url::parse("https://imgth.com/")?, target, max_pages).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_imgth_gallery_matching_is_conservative() {
        assert_eq!(
            match_url("https://imgth.com/gallery/123/some-title"),
            Some(Target::Gallery { id: "123".into() })
        );
        for input in [
            "http://imgth.com/gallery/123",
            "https://imgth.com.evil.com/gallery/123",
            "https://imgth.com/gallery/none",
            "https://imgth.com/gallery/123/extra/segments",
            "https://imgth.com/gallery/123%2F456",
        ] {
            assert!(match_url(input).is_none(), "{input}");
        }
    }

    #[test]
    fn thumbnail_paths_map_to_same_origin_originals() {
        let sample = concat!(
            "<ul class=\"thumbnails\">",
            "<li><img src=\"https://imgth.com/thumbs/abc/one.jpg\"></li>",
            "<li><img src=\"/thumbs/abc/two.png\"></li>",
            "<li><img src=\"/thumbs/abc/one.jpg\"></li>",
            "<li><img src=\"https://evil.com/thumbs/abc/bad.jpg\"></li>",
            "<li><img src=\"/thumbs/../private.jpg\"></li>",
            "</ul>",
        );
        let media = thumbnails(sample);
        assert_eq!(media.len(), 2);
        assert_eq!(media[0].url, "https://imgth.com/images/abc/one.jpg");
        assert_eq!(media[0].filename, "one.jpg");
        assert_eq!(media[1].url, "https://imgth.com/images/abc/two.png");
    }

    #[tokio::test]
    async fn gallery_crawls_only_bounded_next_pages_in_order() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (route, body) in [
                (
                    "GET /gallery/123/g/ HTTP/1.1",
                    "<ul class=\"thumbnails\"><img src=\"/thumbs/abc/one.jpg\"></ul><li class=\"next\">",
                ),
                (
                    "GET /gallery/123/g/page/1 HTTP/1.1",
                    "<ul class=\"thumbnails\"><img src=\"/thumbs/abc/two.jpg\"><img src=\"/thumbs/abc/one.jpg\"></ul>",
                ),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buffer = [0_u8; 4096];
                let n = socket.read(&mut buffer).await.unwrap();
                let request = String::from_utf8_lossy(&buffer[..n]);
                assert!(request.starts_with(route), "{request}");
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let result = resolve_at_base(
            &Client::new(),
            &base,
            Target::Gallery { id: "123".into() },
            2,
        )
        .await
        .unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].filename, "one.jpg");
        assert_eq!(result[1].filename, "two.jpg");
        server.await.unwrap();
    }
}
