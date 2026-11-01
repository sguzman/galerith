//! Independent, partial Imgbox public image-page extractor.
//! Gallery traversal is bounded; complete metadata, archive keys, filename
//! templates, authentication and live upstream parity remain unsupported.
use crate::{safe_filename, MediaItem};
use reqwest::Client;
use std::{collections::HashSet, error::Error};
use url::Url;

const MAX_HTML_BYTES: usize = 2 * 1024 * 1024;
const MAX_GALLERY_IMAGES: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Image { key: String },
    Gallery { key: String },
}

fn valid_key(key: &str) -> bool {
    key.len() == 8 && key.bytes().all(|ch| ch.is_ascii_alphanumeric())
}

fn valid_gallery_key(key: &str) -> bool {
    key.len() == 10 && key.bytes().all(|ch| ch.is_ascii_alphanumeric())
}

fn valid_imgbox_host(host: &str) -> bool {
    matches!(host, "imgbox.com" | "www.imgbox.com" | "i.imgbox.com")
        || host
            .strip_prefix("images")
            .and_then(|part| part.strip_suffix(".imgbox.com"))
            .is_some_and(|part| !part.is_empty() && part.bytes().all(|ch| ch.is_ascii_digit()))
}

pub fn match_url(input: &str) -> Option<Target> {
    let url = Url::parse(input).ok()?;
    let host = url.host_str()?;
    if url.scheme() != "https" || !valid_imgbox_host(host) {
        return None;
    }
    let parts: Vec<_> = url.path_segments()?.collect();
    if matches!(host, "imgbox.com" | "www.imgbox.com") {
        if let ["g", gallery] = parts.as_slice() {
            if valid_gallery_key(gallery) {
                return Some(Target::Gallery {
                    key: (*gallery).to_owned(),
                });
            }
        }
    }
    let key = match parts.as_slice() {
        [key] if valid_key(key) => *key,
        [first, second, key]
            if first.len() == 2
                && second.len() == 2
                && first.bytes().all(|ch| ch.is_ascii_hexdigit())
                && second.bytes().all(|ch| ch.is_ascii_hexdigit())
                && valid_key(key)
                && url.host_str()?.strip_prefix("images").is_some() =>
        {
            *key
        }
        _ => return None,
    };
    Some(Target::Image {
        key: key.to_owned(),
    })
}

fn asset_url(raw: &str) -> Option<Url> {
    let url = Url::parse(raw).ok()?;
    let host = url.host_str()?;
    if url.scheme() != "https"
        || !host
            .strip_prefix("images")
            .and_then(|part| part.strip_suffix(".imgbox.com"))
            .is_some_and(|part| !part.is_empty() && part.bytes().all(|ch| ch.is_ascii_digit()))
    {
        return None;
    }
    Some(url)
}

fn parse_image_page(html: &str, key: &str) -> Option<MediaItem> {
    let image_tag = html
        .split_once("property=\"og:image\" content=\"")?
        .1
        .split_once('"')?
        .0;
    let image = asset_url(image_tag)?;
    let extension = image.path().rsplit_once('.')?.1;
    if extension.is_empty()
        || extension.len() > 12
        || !extension.bytes().all(|ch| ch.is_ascii_alphanumeric())
    {
        return None;
    }
    let titled = html
        .split_once("class=\"image-container\"")
        .and_then(|(_, suffix)| suffix.split_once(" title=\""))
        .and_then(|(_, suffix)| suffix.split_once('"'))
        .map(|(title, _)| title);
    let title = titled.unwrap_or(key);
    if title.len() > 200 {
        return None;
    }
    let mut filename = safe_filename(title);
    if filename.is_empty() || matches!(filename.as_str(), "." | "..") {
        filename = format!("{key}.{extension}");
    }
    if !filename.contains('.') {
        filename.push('.');
        filename.push_str(extension);
    }
    Some(MediaItem {
        url: image.to_string(),
        filename,
        extractor: "imgbox",
        post_id: None,
    })
}

fn gallery_keys(html: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let mut seen = HashSet::new();
    for fragment in html.split("<a href=\"/").skip(1).take(2000) {
        let Some((key, remainder)) = fragment.split_once('"') else {
            continue;
        };
        if !valid_key(key) || !remainder.starts_with("><img alt=\"") {
            continue;
        }
        if seen.insert(key.to_owned()) {
            keys.push(key.to_owned());
        }
        if keys.len() >= MAX_GALLERY_IMAGES {
            break;
        }
    }
    keys
}

async fn page_html(client: &Client, endpoint: Url) -> Result<String, Box<dyn Error + Send + Sync>> {
    let initial_origin = endpoint.origin();
    let initial_host = endpoint.host_str().unwrap_or_default().to_owned();
    crate::pacing::wait_for_request().await;
    let mut response = client.get(endpoint).send().await?.error_for_status()?;
    let final_url = response.url();
    let local_fixture = initial_host
        .parse::<std::net::IpAddr>()
        .is_ok_and(|address| address.is_loopback());
    let trusted_destination = if local_fixture {
        final_url.origin() == initial_origin
    } else {
        matches!(initial_host.as_str(), "imgbox.com" | "www.imgbox.com")
            && final_url.scheme() == "https"
            && matches!(final_url.host_str(), Some("imgbox.com" | "www.imgbox.com"))
    };
    if !trusted_destination {
        return Err("Imgbox page redirected outside its trusted origin".into());
    }
    if response
        .content_length()
        .is_some_and(|len| len > MAX_HTML_BYTES as u64)
    {
        return Err("Imgbox HTML exceeds the configured safety bound".into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len().saturating_add(chunk.len()) > MAX_HTML_BYTES {
            return Err("Imgbox HTML exceeds the configured safety bound".into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8(body)?)
}

async fn resolve_at_base(
    client: &Client,
    base: &Url,
    key: &str,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let page = page_html(client, base.join(key)?).await?;
    let item = parse_image_page(&page, key).ok_or("Imgbox page missing a supported image")?;
    Ok(vec![item])
}

async fn resolve_gallery_at_base(
    client: &Client,
    base: &Url,
    key: &str,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let page = page_html(client, base.join(&format!("g/{key}"))?).await?;
    if page.contains("The specified gallery could not be found.") {
        return Err("Imgbox gallery does not exist".into());
    }
    let mut items = Vec::new();
    for image_key in gallery_keys(&page) {
        items.extend(resolve_at_base(client, base, &image_key).await?);
    }
    Ok(items)
}

pub async fn resolve(
    client: &Client,
    target: Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse("https://imgbox.com/")?;
    match target {
        Target::Image { key } => resolve_at_base(client, &base, &key).await,
        Target::Gallery { key } => resolve_gallery_at_base(client, &base, &key).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_public_imgbox_image_routes_are_recognized() {
        assert_eq!(
            match_url("https://imgbox.com/a1B2c3D4"),
            Some(Target::Image {
                key: "a1B2c3D4".into(),
            })
        );
        assert_eq!(
            match_url("https://images2.imgbox.com/ab/cd/a1B2c3D4"),
            Some(Target::Image {
                key: "a1B2c3D4".into(),
            })
        );
        assert_eq!(
            match_url("https://imgbox.com/g/abcdefghij"),
            Some(Target::Gallery {
                key: "abcdefghij".into(),
            })
        );
        assert!(match_url("https://imgbox.com/g/short").is_none());
        assert!(match_url("https://imgbox.com/short").is_none());
        assert!(match_url("https://evilimgbox.com/a1B2c3D4").is_none());
        assert!(match_url("https://imagesbad.imgbox.com/ab/cd/a1B2c3D4").is_none());
        assert!(match_url("http://imgbox.com/a1B2c3D4").is_none());
    }

    #[test]
    fn html_page_only_accepts_verified_imgbox_image_hosts() {
        let page = r#"<meta property="og:image" content="https://images2.imgbox.com/aa/bb/source.jpg"><div class="image-container" title="original.jpg"></div>"#;
        let media = parse_image_page(page, "a1B2c3D4").unwrap();
        assert_eq!(media.filename, "original.jpg");
        assert_eq!(media.url, "https://images2.imgbox.com/aa/bb/source.jpg");
        assert_eq!(media.extractor, "imgbox");
        let forged = r#"<meta property="og:image" content="https://evil.example.com/a.jpg">"#;
        assert!(parse_image_page(forged, "a1B2c3D4").is_none());
        assert!(parse_image_page("<h1>Unavailable</h1>", "a1B2c3D4").is_none());
    }

    #[test]
    fn gallery_keys_follow_page_order_and_ignore_invalid_or_duplicate_routes() {
        let page = concat!(
            "<a href=\"/a1B2c3D4\"><img alt=\"first\">",
            "<a href=\"/Z9y8X7w6\"><img alt=\"second\">",
            "<a href=\"/a1B2c3D4\"><img alt=\"duplicate\">",
            "<a href=\"/short\"><img alt=\"invalid\">",
            "<a href=\"/third123\">not an image",
        );
        assert_eq!(gallery_keys(page), ["a1B2c3D4", "Z9y8X7w6"]);
    }

    #[tokio::test]
    async fn gallery_follows_public_image_pages_in_listing_order() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let mut arrivals = Vec::new();
            for (route, body) in [
                (
                    "GET /g/abcdefghij HTTP/1.1",
                    "<a href=\"/a1B2c3D4\"><img alt=\"first\"><a href=\"/Z9y8X7w6\"><img alt=\"second\">",
                ),
                (
                    "GET /a1B2c3D4 HTTP/1.1",
                    r#"<meta property="og:image" content="https://images2.imgbox.com/a/b/one.jpg"><div class="image-container" title="one.jpg"></div>"#,
                ),
                (
                    "GET /Z9y8X7w6 HTTP/1.1",
                    r#"<meta property="og:image" content="https://images2.imgbox.com/a/b/two.png"><div class="image-container" title="two.png"></div>"#,
                ),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                arrivals.push(tokio::time::Instant::now());
                let mut buf = [0_u8; 4096];
                let n = socket.read(&mut buf).await.unwrap();
                let request = String::from_utf8_lossy(&buf[..n]);
                assert!(request.starts_with(route), "{request}");
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
            arrivals
        });
        let base = Url::parse(&format!("http://{address}/")).unwrap();
        let delay = crate::pacing::DelayRange::parse("0.12").unwrap();
        let images = crate::pacing::with_request_delay(
            Some(delay),
            resolve_gallery_at_base(&Client::new(), &base, "abcdefghij"),
        )
        .await
        .unwrap();
        assert_eq!(images.len(), 2);
        assert_eq!(images[0].filename, "one.jpg");
        assert_eq!(images[1].filename, "two.png");
        let arrivals = server.await.unwrap();
        assert_eq!(arrivals.len(), 3);
        for pair in arrivals.windows(2) {
            assert!(pair[1].duration_since(pair[0]) >= std::time::Duration::from_millis(100));
        }
    }

    #[tokio::test]
    async fn single_image_resolves_with_offline_html_server() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut connection, _) = listener.accept().await.unwrap();
            let mut buf = [0_u8; 4096];
            let n = connection.read(&mut buf).await.unwrap();
            let request = String::from_utf8_lossy(&buf[..n]);
            assert!(request.starts_with("GET /a1B2c3D4 HTTP/1.1"), "{request}");
            let body = r#"<meta property="og:image" content="https://images5.imgbox.com/x/y/pic.png"><div class="image-container" title="test.png"></div>"#;
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            connection.write_all(reply.as_bytes()).await.unwrap();
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let items = resolve_at_base(&Client::new(), &base, "a1B2c3D4")
            .await
            .unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].filename, "test.png");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn gallery_page_rejects_cross_origin_redirects() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let destination = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let destination_addr = destination.local_addr().unwrap();
        let destination_server = tokio::spawn(async move {
            let (mut socket, _) = destination.accept().await.unwrap();
            let mut request = [0_u8; 1024];
            assert!(socket.read(&mut request).await.unwrap() > 0);
            let body = r#"<a href="/a1B2c3D4"><img alt="untrusted"></a>"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        let source = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let source_addr = source.local_addr().unwrap();
        let source_server = tokio::spawn(async move {
            let (mut socket, _) = source.accept().await.unwrap();
            let mut request = [0_u8; 1024];
            assert!(socket.read(&mut request).await.unwrap() > 0);
            let response = format!(
                "HTTP/1.1 302 Found\r\nLocation: http://{destination_addr}/gallery\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        let base = Url::parse(&format!("http://{source_addr}/")).unwrap();
        assert!(resolve_gallery_at_base(&Client::new(), &base, "abcdefghij")
            .await
            .is_err());
        source_server.await.unwrap();
        destination_server.await.unwrap();
    }
}
