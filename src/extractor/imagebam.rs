//! Independent public ImageBam single-image and first-gallery-page preview.
//! Full gallery pagination, metadata, archive keys and live reference parity
//! remain unimplemented.
use crate::{safe_filename, MediaItem};
use reqwest::Client;
use std::{collections::HashSet, error::Error};
use url::Url;

const MAX_HTML_BYTES: usize = 2 * 1024 * 1024;
const MAX_GALLERY_IMAGES: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Image { path: String },
    Gallery { path: String },
}

fn safe_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && value.bytes().all(|c| c.is_ascii_alphanumeric())
}

pub fn match_url(input: &str) -> Option<Target> {
    let url = Url::parse(input).ok()?;
    if url.scheme() != "https"
        || !matches!(url.host_str(), Some("imagebam.com" | "www.imagebam.com"))
    {
        return None;
    }
    let path: Vec<_> = url.path_segments()?.collect();
    match path.as_slice() {
        ["gallery", id] if safe_id(id) => Some(Target::Gallery {
            path: format!("gallery/{id}"),
        }),
        ["view", id] if id.starts_with('G') && safe_id(id) => Some(Target::Gallery {
            path: format!("view/{id}"),
        }),
        ["image", id] if safe_id(id) => Some(Target::Image {
            path: format!("image/{id}"),
        }),
        ["view", id] if id.starts_with('M') && safe_id(id) => Some(Target::Image {
            path: format!("view/{id}"),
        }),
        _ => None,
    }
}

fn imagebam_cdn(raw: &str) -> Option<Url> {
    let url = Url::parse(raw).ok()?;
    let host = url.host_str()?;
    let numbered = host
        .strip_prefix("images")
        .and_then(|s| s.strip_suffix(".imagebam.com"))
        .is_some_and(|s| !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit()));
    if url.scheme() != "https" || !numbered {
        return None;
    }
    Some(url)
}

fn parse_image_page(html: &str) -> Option<MediaItem> {
    let after = html.split_once("<img src=\"https://images")?.1;
    let suffix = after.split_once('"')?.0;
    let raw = format!("https://images{suffix}");
    let url = imagebam_cdn(&raw)?;
    let ext = url.path().rsplit_once('.')?.1;
    if ext.is_empty() || ext.len() > 12 || !ext.bytes().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    let title = after
        .split_once("alt=\"")
        .and_then(|(_, tail)| tail.split_once('"'))
        .map(|(value, _)| value)
        .filter(|value| !value.is_empty() && value.len() <= 200);
    let filename = title
        .map(safe_filename)
        .filter(|name| !matches!(name.as_str(), "." | ".."))
        .unwrap_or_else(|| {
            url.path_segments()
                .and_then(Iterator::last)
                .map(safe_filename)
                .unwrap_or_else(|| format!("image.{ext}"))
        });
    Some(MediaItem {
        url: url.to_string(),
        filename,
        extractor: "imagebam",
        post_id: None,
    })
}

fn gallery_image_paths(html: &str) -> Vec<String> {
    let mut paths = Vec::new();
    let mut seen = HashSet::new();
    for fragment in html
        .split("href=\"https://www.imagebam.com/")
        .skip(1)
        .take(2000)
    {
        let Some((path, _)) = fragment.split_once('"') else {
            continue;
        };
        let mut parts = path.split('/');
        let Some(kind) = parts.next() else {
            continue;
        };
        let Some(id) = parts.next() else {
            continue;
        };
        if parts.next().is_some() || !matches!(kind, "view" | "image") || !safe_id(id) {
            continue;
        }
        if kind == "view" && !id.starts_with('M') {
            continue;
        }
        if seen.insert(path.to_owned()) {
            paths.push(path.to_owned());
        }
        if paths.len() == MAX_GALLERY_IMAGES {
            break;
        }
    }
    paths
}

fn next_gallery_page(html: &str, current: &Url, first: &Url) -> Option<Url> {
    let following = html.split_once("rel=\"next\" aria-label=\"Next")?.1;
    let raw = following.split_once("href=\"")?.1.split_once('"')?.0;
    let next = current.join(raw).ok()?;
    (next.origin() == first.origin()
        && next.path() == first.path()
        && next != *current)
        .then_some(next)
}

async fn read_html(client: &Client, endpoint: Url) -> Result<String, Box<dyn Error + Send + Sync>> {
    let mut response = client.get(endpoint).send().await?.error_for_status()?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_HTML_BYTES as u64)
    {
        return Err("ImageBam HTML exceeds the configured safety bound".into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len().saturating_add(chunk.len()) > MAX_HTML_BYTES {
            return Err("ImageBam HTML exceeds the configured safety bound".into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8(body)?)
}

async fn resolve_at_base(
    client: &Client,
    base: &Url,
    target: Target,
    max_pages: u16,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    match target {
        Target::Image { path } => {
            let html = read_html(client, base.join(&path)?).await?;
            Ok(vec![
                parse_image_page(&html).ok_or("ImageBam image missing original media")?
            ])
        }
        Target::Gallery { path } => {
            let first = base.join(&path)?;
            let mut page = first.clone();
            let mut visited_pages = HashSet::new();
            let mut visited_images = HashSet::new();
            let mut image_paths = Vec::new();
            for index in 0..max_pages.clamp(1, 100) {
                if !visited_pages.insert(page.to_string()) {
                    break;
                }
                if index > 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                }
                let html = read_html(client, page.clone()).await?;
                for image in gallery_image_paths(&html) {
                    if visited_images.insert(image.clone()) {
                        image_paths.push(image);
                    }
                    if image_paths.len() == MAX_GALLERY_IMAGES {
                        break;
                    }
                }
                if image_paths.len() == MAX_GALLERY_IMAGES {
                    break;
                }
                let Some(next) = next_gallery_page(&html, &page, &first) else {
                    break;
                };
                page = next;
            }
            // Pinned gallery-dl preserves the reverse of gallery listing order.
            image_paths.reverse();
            let mut media = Vec::new();
            for image_path in image_paths {
                let html = read_html(client, base.join(&image_path)?).await?;
                if let Some(item) = parse_image_page(&html) {
                    media.push(item);
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
    let base = Url::parse("https://www.imagebam.com/")?;
    resolve_at_base(client, &base, target, max_pages).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_imagebam_image_and_gallery_url_matching() {
        assert_eq!(
            match_url("https://www.imagebam.com/view/M123abc"),
            Some(Target::Image {
                path: "view/M123abc".into(),
            })
        );
        assert_eq!(
            match_url("https://imagebam.com/view/G123abc"),
            Some(Target::Gallery {
                path: "view/G123abc".into(),
            })
        );
        assert_eq!(
            match_url("https://www.imagebam.com/gallery/album7"),
            Some(Target::Gallery {
                path: "gallery/album7".into(),
            })
        );
        assert!(match_url("https://evil.imagebam.com/view/M123").is_none());
        assert!(match_url("http://imagebam.com/image/123").is_none());
        assert!(match_url("https://www.imagebam.com/view/X123").is_none());
        assert!(match_url("https://www.imagebam.com/gallery/bad%2Fid").is_none());
    }

    #[test]
    fn image_page_requires_original_media_from_checked_cdn() {
        let source =
            r#"<img src="https://images3.imagebam.com/a/b/picture.png" alt="original.png">"#;
        let item = parse_image_page(source).unwrap();
        assert_eq!(item.filename, "original.png");
        assert_eq!(item.url, "https://images3.imagebam.com/a/b/picture.png");
        assert_eq!(item.extractor, "imagebam");
        let forged = r#"<img src="https://images3.evil.test/a/b/picture.png" alt="fake.png">"#;
        assert!(parse_image_page(forged).is_none());
        assert!(parse_image_page("<img src=\"file:///etc/passwd\">").is_none());
    }

    #[test]
    fn gallery_image_paths_deduplicate_and_restore_reference_order() {
        let html = concat!(
            "<a href=\"https://www.imagebam.com/view/Mfirst1\">",
            "<a href=\"https://www.imagebam.com/view/Msecond2\">",
            "<a href=\"https://www.imagebam.com/view/Mfirst1\">",
            "<a href=\"https://www.imagebam.com/view/Galbum\">",
            "<a href=\"https://evil.example.com/view/Mwrong\">",
        );
        assert_eq!(gallery_image_paths(html), ["view/Mfirst1", "view/Msecond2"]);
    }

    #[test]
    fn gallery_next_pages_are_same_origin_and_same_gallery_only() {
        let root = Url::parse("https://www.imagebam.com/gallery/abc123").unwrap();
        let html = r#"<a rel="next" aria-label="Next page" href="?page=2">More</a>"#;
        let next = next_gallery_page(html, &root, &root).unwrap();
        assert_eq!(next.as_str(), "https://www.imagebam.com/gallery/abc123?page=2");
        assert_eq!(next_gallery_page(html, &next, &root), None);
        let bad = r#"<a rel="next" aria-label="Next" href="https://evil.example.org/trap">"#;
        assert!(next_gallery_page(bad, &root, &root).is_none());
        let escaped = r#"<a rel="next" aria-label="Next" href="/image/evil">"#;
        assert!(next_gallery_page(escaped, &root, &root).is_none());
    }

    #[tokio::test]
    async fn gallery_next_page_is_bounded_and_orders_media_across_pages() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (route, body) in [
                (
                    "GET /gallery/album7 HTTP/1.1",
                    "<a href=\"https://www.imagebam.com/view/Mfirst1\"><a rel=\"next\" aria-label=\"Next page\" href=\"?page=2\">",
                ),
                (
                    "GET /gallery/album7?page=2 HTTP/1.1",
                    "<a href=\"https://www.imagebam.com/view/Msecond2\">",
                ),
                (
                    "GET /view/Msecond2 HTTP/1.1",
                    r#"<img src="https://images2.imagebam.com/img/two.png" alt="two.png">"#,
                ),
                (
                    "GET /view/Mfirst1 HTTP/1.1",
                    r#"<img src="https://images2.imagebam.com/img/one.jpg" alt="one.jpg">"#,
                ),
            ] {
                let (mut connection, _) = listener.accept().await.unwrap();
                let mut buf = [0_u8; 4096];
                let n = connection.read(&mut buf).await.unwrap();
                let request = String::from_utf8_lossy(&buf[..n]);
                assert!(request.starts_with(route), "{request}");
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                connection.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{address}/")).unwrap();
        let result = resolve_at_base(
            &Client::new(),
            &base,
            Target::Gallery {
                path: "gallery/album7".into(),
            },
            2,
        )
        .await
        .unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].filename, "two.png");
        assert_eq!(result[1].filename, "one.jpg");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn gallery_fetches_first_page_and_ordered_original_image_pages() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (route, body) in [
                (
                    "GET /gallery/album7 HTTP/1.1",
                    "<a href=\"https://www.imagebam.com/view/Mfirst1\"><a href=\"https://www.imagebam.com/view/Msecond2\">",
                ),
                (
                    "GET /view/Msecond2 HTTP/1.1",
                    r#"<img src="https://images2.imagebam.com/img/two.png" alt="two.png">"#,
                ),
                (
                    "GET /view/Mfirst1 HTTP/1.1",
                    r#"<img src="https://images2.imagebam.com/img/one.jpg" alt="one.jpg">"#,
                ),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buf = [0_u8; 4096];
                let n = socket.read(&mut buf).await.unwrap();
                let request = String::from_utf8_lossy(&buf[..n]);
                assert!(request.starts_with(route), "{request}");
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{address}/")).unwrap();
        let result = resolve_at_base(
            &Client::new(),
            &base,
            Target::Gallery {
                path: "gallery/album7".into(),
            },
            1,
        )
        .await
        .unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].filename, "two.png");
        assert_eq!(result[1].filename, "one.jpg");
        server.await.unwrap();
    }
}
