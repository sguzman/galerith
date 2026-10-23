//! Independent, partial Imgbox public image-page extractor.
//! Galleries, original metadata/numbering, album templates, authentication,
//! and live upstream parity are not yet supported.
use crate::{safe_filename, MediaItem};
use reqwest::Client;
use std::error::Error;
use url::Url;

const MAX_HTML_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Image { key: String },
}

fn valid_key(key: &str) -> bool {
    key.len() == 8 && key.bytes().all(|ch| ch.is_ascii_alphanumeric())
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
    if url.scheme() != "https" || !valid_imgbox_host(url.host_str()?) {
        return None;
    }
    let parts: Vec<_> = url.path_segments()?.collect();
    let key = match parts.as_slice() {
        [key] if valid_key(key) => *key,
        [first, second, key]
            if first.len() == 2
                && second.len() == 2
                && first.bytes().all(|ch| ch.is_ascii_hexdigit())
                && second.bytes().all(|ch| ch.is_ascii_hexdigit())
                && valid_key(key)
                && url
                    .host_str()?
                    .strip_prefix("images")
                    .is_some() =>
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

async fn page_html(
    client: &Client,
    endpoint: Url,
) -> Result<String, Box<dyn Error + Send + Sync>> {
    let mut response = client.get(endpoint).send().await?.error_for_status()?;
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

pub async fn resolve(
    client: &Client,
    target: Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse("https://imgbox.com/")?;
    match target {
        Target::Image { key } => resolve_at_base(client, &base, &key).await,
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
        assert!(match_url("https://imgbox.com/g/abcdefghij").is_none());
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
}
