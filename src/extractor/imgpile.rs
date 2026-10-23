//! Independent, partial public Imgpile post-media extractor.
//! Profile feeds, complete metadata, authentication and live-site equivalence
//! are not yet supported.
use crate::{safe_filename, MediaItem};
use reqwest::Client;
use std::{collections::HashSet, error::Error};
use url::Url;

const MAX_HTML_BYTES: usize = 2 * 1024 * 1024;
const MAX_POST_MEDIA: usize = 2000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Post { slug: String },
}

fn valid_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= 128
        && slug.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

pub fn match_url(raw: &str) -> Option<Target> {
    let url = Url::parse(raw).ok()?;
    if url.scheme() != "https" || !matches!(url.host_str(), Some("imgpile.com" | "www.imgpile.com"))
    {
        return None;
    }
    match url.path_segments()?.collect::<Vec<_>>().as_slice() {
        ["p", slug] if valid_slug(slug) => Some(Target::Post {
            slug: (*slug).to_owned(),
        }),
        _ => None,
    }
}

fn supported_asset(raw: &str) -> Option<(Url, String)> {
    let url = Url::parse(raw).ok()?;
    let host = url.host_str()?;
    if url.scheme() != "https" || (host != "imgpile.com" && !host.ends_with(".imgpile.com")) {
        return None;
    }
    let filename = url.path_segments()?.next_back()?;
    if filename.is_empty() || filename.len() > 240 {
        return None;
    }
    let extension = filename.rsplit_once('.')?.1;
    if extension.is_empty()
        || extension.len() > 12
        || !extension.bytes().all(|b| b.is_ascii_alphanumeric())
    {
        return None;
    }
    Some((url.clone(), safe_filename(filename)))
}

fn post_media(html: &str) -> Vec<MediaItem> {
    let mut media = Vec::new();
    let mut seen = HashSet::new();
    for section in html
        .split("class=\"post-media")
        .skip(1)
        .take(MAX_POST_MEDIA)
    {
        let fragment = section
            .split_once("</div>")
            .map(|(first, _)| first)
            .unwrap_or(section);
        let Some(raw) = fragment
            .split_once("<a href=\"")
            .and_then(|(_, rest)| rest.split_once('"'))
            .map(|(link, _)| link)
        else {
            continue;
        };
        let Some((url, filename)) = supported_asset(raw) else {
            continue;
        };
        let post_id = fragment
            .split_once("data-media-id=\"")
            .and_then(|(_, part)| part.split_once('"'))
            .and_then(|(value, _)| value.parse::<u64>().ok());
        if seen.insert(url.to_string()) {
            media.push(MediaItem {
                url: url.to_string(),
                filename,
                extractor: "imgpile",
                post_id,
            });
        }
    }
    media
}

async fn read_html(client: &Client, url: Url) -> Result<String, Box<dyn Error + Send + Sync>> {
    let mut response = client.get(url).send().await?.error_for_status()?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_HTML_BYTES as u64)
    {
        return Err("Imgpile HTML exceeds the configured safety bound".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len().saturating_add(chunk.len()) > MAX_HTML_BYTES {
            return Err("Imgpile HTML exceeds the configured safety bound".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8(bytes)?)
}

async fn resolve_at_base(
    client: &Client,
    base: &Url,
    target: Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    match target {
        Target::Post { slug } => {
            let html = read_html(client, base.join(&format!("p/{slug}"))?).await?;
            Ok(post_media(&html))
        }
    }
}

pub async fn resolve(
    client: &Client,
    target: Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    resolve_at_base(client, &Url::parse("https://imgpile.com/")?, target).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conservative_imgpile_post_routes() {
        assert_eq!(
            match_url("https://imgpile.com/p/AbCdEfG"),
            Some(Target::Post {
                slug: "AbCdEfG".into(),
            })
        );
        for url in [
            "http://imgpile.com/p/AbCdEfG",
            "https://evil.imgpile.com/p/AbCdEfG",
            "https://imgpile.com/p/bad%2Fpath",
            "https://imgpile.com/p/",
            "https://imgpile.com/u/AbCdEfG",
        ] {
            assert!(match_url(url).is_none(), "{url}");
        }
    }

    #[test]
    fn extracts_ordered_post_assets_with_checked_cdn_and_ids() {
        let html = concat!(
            "<div class=\"post-media\" data-media-id=\"17\"><a href=\"https://cdn.imgpile.com/photo/one.jpg\">one</a></div>",
            "<div class=\"post-media\" data-media-id=\"18\"><a href=\"https://imgpile.com/photo/two.webp\">two</a></div>",
            "<div class=\"post-media\"><a href=\"https://cdn.imgpile.com/photo/one.jpg\">duplicate</a></div>",
            "<div class=\"post-media\"><a href=\"https://imgpile.com.evil.test/unsafe.jpg\">unsafe</a></div>",
        );
        let images = post_media(html);
        assert_eq!(images.len(), 2);
        assert_eq!(images[0].filename, "one.jpg");
        assert_eq!(images[0].post_id, Some(17));
        assert_eq!(images[1].filename, "two.webp");
        assert_eq!(images[1].post_id, Some(18));
    }

    #[tokio::test]
    async fn public_post_resolves_from_local_html() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0_u8; 4096];
            let n = socket.read(&mut buf).await.unwrap();
            let request = String::from_utf8_lossy(&buf[..n]);
            assert!(request.starts_with("GET /p/PostAB12 HTTP/1.1"), "{request}");
            let body = r#"<div class="post-media" data-media-id="91"><a href="https://cdn.imgpile.com/media/image.png">download</a></div>"#;
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(reply.as_bytes()).await.unwrap();
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let media = resolve_at_base(
            &Client::new(),
            &base,
            Target::Post {
                slug: "PostAB12".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(media.len(), 1);
        assert_eq!(media[0].filename, "image.png");
        assert_eq!(media[0].post_id, Some(91));
        server.await.unwrap();
    }
}
