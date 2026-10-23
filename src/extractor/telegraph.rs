//! Independent public Telegraph article-image extractor preview.
//! Complete gallery-dl metadata, captions, exact archive/naming semantics and
//! live upstream behavior are not accepted yet.
use crate::{safe_filename, MediaItem};
use reqwest::Client;
use std::{collections::HashSet, error::Error};
use url::Url;

const MAX_HTML_BYTES: usize = 2 * 1024 * 1024;
const MAX_ARTICLE_IMAGES: usize = 2000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Article { slug: String },
}

pub fn match_url(input: &str) -> Option<Target> {
    // Reject encoded dot segments prior to URL normalization.
    if input.to_ascii_lowercase().contains("%2e") {
        return None;
    }
    let url = Url::parse(input).ok()?;
    if url.scheme() != "https" || !matches!(url.host_str(), Some("telegra.ph" | "www.telegra.ph")) {
        return None;
    }
    let parts: Vec<_> = url.path_segments()?.collect();
    let [slug] = parts.as_slice() else {
        return None;
    };
    if slug.is_empty()
        || slug.len() > 200
        || !slug
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return None;
    }
    Some(Target::Article {
        slug: (*slug).to_owned(),
    })
}

fn image_media(raw: &str, base: &Url, index: usize, padding: usize) -> Option<MediaItem> {
    if raw.starts_with("/embed/") {
        return None;
    }
    let asset = base.join(raw).ok()?;
    if asset.scheme() != "https"
        || !matches!(asset.host_str(), Some("telegra.ph" | "www.telegra.ph"))
    {
        return None;
    }
    let original = asset.path_segments()?.next_back()?;
    let ext = original.rsplit_once('.')?.1;
    if ext.is_empty() || ext.len() > 12 || !ext.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return None;
    }
    let original = safe_filename(original);
    if matches!(original.as_str(), "" | "." | "..") {
        return None;
    }
    Some(MediaItem {
        url: asset.to_string(),
        filename: format!("{index:0padding$}_{original}"),
        extractor: "telegraph",
        post_id: None,
    })
}

fn first_src(fragment: &str) -> Option<&str> {
    // Figure tag may contain a video embed or thumbnail. Use only an actual
    // <img> element when processing figures.
    let image = fragment.split_once("<img")?.1;
    let tag = image.split_once('>')?.0;
    for marker in ["src=\"", "src='"] {
        if let Some((_, right)) = tag.split_once(marker) {
            let quote = if marker.ends_with('"') { '"' } else { '\'' };
            return right.split_once(quote).map(|(src, _)| src);
        }
    }
    None
}

fn article_images(html: &str, base: &Url) -> Vec<MediaItem> {
    let figures: Vec<_> = html
        .split("<figure>")
        .skip(1)
        .take(MAX_ARTICLE_IMAGES)
        .filter_map(|part| part.split_once("</figure>").map(|(figure, _)| figure))
        .collect();
    let sources: Vec<_> = if !figures.is_empty() {
        figures
            .iter()
            .filter_map(|figure| first_src(figure))
            .collect()
    } else {
        html.split("<img")
            .skip(1)
            .take(MAX_ARTICLE_IMAGES)
            .filter_map(|part| {
                let tag = part.split_once('>')?.0;
                for marker in ["src=\"", "src='"] {
                    if let Some((_, right)) = tag.split_once(marker) {
                        let quote = if marker.ends_with('"') { '"' } else { '\'' };
                        return right.split_once(quote).map(|(src, _)| src);
                    }
                }
                None
            })
            .collect()
    };
    let padding = sources.len().max(1).to_string().len();
    let mut seen = HashSet::new();
    let mut images = Vec::new();
    for raw in sources {
        if let Some(media) = image_media(raw, base, images.len() + 1, padding) {
            if seen.insert(media.url.clone()) {
                images.push(media);
            }
        }
    }
    images
}

async fn read_html(client: &Client, url: Url) -> Result<String, Box<dyn Error + Send + Sync>> {
    let mut response = client.get(url).send().await?.error_for_status()?;
    if response
        .content_length()
        .is_some_and(|n| n > MAX_HTML_BYTES as u64)
    {
        return Err("Telegraph HTML response exceeds the configured safety bound".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len().saturating_add(chunk.len()) > MAX_HTML_BYTES {
            return Err("Telegraph HTML response exceeds the configured safety bound".into());
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
        Target::Article { slug } => {
            let page = read_html(client, base.join(&slug)?).await?;
            // A localhost page fixture changes only the fetch base. Assets
            // still use the canonical approved telegra.ph origin.
            Ok(article_images(&page, &Url::parse("https://telegra.ph/")?))
        }
    }
}

pub async fn resolve(
    client: &Client,
    target: Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    resolve_at_base(client, &Url::parse("https://telegra.ph/")?, target).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_public_telegraph_single_article_routes_match() {
        assert_eq!(
            match_url("https://telegra.ph/My-Article-03-04"),
            Some(Target::Article {
                slug: "My-Article-03-04".into(),
            })
        );
        for raw in [
            "http://telegra.ph/My-Article",
            "https://evil.telegra.ph/My-Article",
            "https://telegra.ph/",
            "https://telegra.ph/article/extra",
            "https://telegra.ph/bad%2Fpath",
            "https://telegra.ph/%2e%2e/evil",
        ] {
            assert!(match_url(raw).is_none(), "{raw}");
        }
    }

    #[test]
    fn figures_extract_canonical_files_but_not_embeds_or_foreign_hosts() {
        let html = concat!(
            "<figure><img src=\"/file/a.jpg\"><figcaption>One</figcaption></figure>",
            "<figure><img src=\"https://telegra.ph/file/b.webp\"></figure>",
            "<figure><iframe src=\"/embed/abc\"></iframe></figure>",
            "<figure><img src=\"https://evil.example.org/malware.png\"></figure>",
            "<figure><img src=\"/file/a.jpg\"></figure>"
        );
        let base = Url::parse("https://telegra.ph/").unwrap();
        let images = article_images(html, &base);
        assert_eq!(images.len(), 2);
        assert_eq!(images[0].filename, "1_a.jpg");
        assert_eq!(images[0].url, "https://telegra.ph/file/a.jpg");
        assert_eq!(images[1].filename, "2_b.webp");
    }

    #[test]
    fn unwrapped_images_work_but_embeds_do_not() {
        let base = Url::parse("https://telegra.ph/").unwrap();
        let html = "<img src='/file/pic1.png'><img src='/embed/unsafe'><img src='/file/pic2.jpg'>";
        let images = article_images(html, &base);
        assert_eq!(images.len(), 2);
        assert_eq!(images[0].filename, "1_pic1.png");
        assert_eq!(images[1].filename, "2_pic2.jpg");
    }

    #[tokio::test]
    async fn article_fetches_html_from_localhost_fixture() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = [0_u8; 4096];
            let n = socket.read(&mut buffer).await.unwrap();
            let request = String::from_utf8_lossy(&buffer[..n]);
            assert!(
                request.starts_with("GET /Article-03-04 HTTP/1.1"),
                "{request}"
            );
            let body = "<figure><img src=\"/file/sample.jpg\"></figure>";
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(reply.as_bytes()).await.unwrap();
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let files = resolve_at_base(
            &Client::new(),
            &base,
            Target::Article {
                slug: "Article-03-04".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].filename, "1_sample.jpg");
        server.await.unwrap();
    }
}
