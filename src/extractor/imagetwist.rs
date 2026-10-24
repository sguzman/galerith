//! Conservative public Imagetwist/Imagehaha original-image subset of imagehosts.py.
use crate::{
    message::{media_metadata, ExtractorMessage, Metadata},
    safe_filename, MediaItem,
};
use reqwest::Client;
use std::error::Error;
use url::Url;

const MAX_HTML_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    token: String,
    host: String,
}

fn valid_host(host: &str) -> bool {
    matches!(
        host,
        "imagetwist.com" | "www.imagetwist.com" | "phun.imagetwist.com"
            | "imagehaha.com" | "www.imagehaha.com" | "phun.imagehaha.com"
    )
}

fn valid_token(value: &str) -> bool {
    value.len() == 12
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

pub fn match_url(input: &str) -> Option<Image> {
    let lower = input.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "/../", "/./"]
        .iter()
        .any(|pattern| lower.contains(pattern))
    {
        return None;
    }
    let parsed = Url::parse(input).ok()?;
    let host = parsed.host_str()?;
    if parsed.scheme() != "https"
        || !valid_host(host)
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.port().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return None;
    }
    let parts: Vec<_> = parsed.path_segments()?.collect();
    let token = match parts.as_slice() {
        [token] if valid_token(token) => *token,
        [token, filename] if valid_token(token) && safe_hint(filename) => *token,
        _ => return None,
    };
    Some(Image {
        token: token.to_owned(),
        host: host.to_owned(),
    })
}

fn safe_hint(value: &str) -> bool {
    (5..=220).contains(&value.len())
        && !value.contains("..")
        && value.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.')
        })
        && value.rsplit_once('.').is_some_and(|(_, extension)| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif"
            )
        })
}

fn media_url(raw: &str) -> Option<(Url, String)> {
    let url = Url::parse(raw).ok()?;
    let host = url.host_str()?;
    if url.scheme() != "https"
        || !(host == "imagetwist.com"
            || host.ends_with(".imagetwist.com")
            || host == "imagehaha.com"
            || host.ends_with(".imagehaha.com"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path().starts_with("/imgs/")
    {
        return None;
    }
    let filename = url.path_segments()?.next_back()?;
    let ext = filename.rsplit_once('.')?.1.to_ascii_lowercase();
    if filename.is_empty()
        || filename.len() > 240
        || filename.contains('%')
        || !matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif")
    {
        return None;
    }
    let name = safe_filename(filename);
    Some((url, name))
}

fn alt_filename(alt: &str, ext: &str) -> Option<String> {
    let decoded = alt
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    if decoded.is_empty()
        || decoded.len() > 225
        || decoded.contains('/')
        || decoded.contains('\\')
        || decoded.chars().any(char::is_control)
    {
        return None;
    }
    let name = if decoded.rsplit_once('.').is_some_and(|(_, extension)| {
        matches!(
            extension.to_ascii_lowercase().as_str(),
            "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif"
        )
    }) {
        decoded
    } else {
        format!("{decoded}.{ext}")
    };
    Some(safe_filename(&name))
}

fn parse_page(
    html: &str,
    image: &Image,
) -> Result<(MediaItem, Metadata), Box<dyn Error + Send + Sync>> {
    let mut original = None;
    for node in html.split("<img").skip(1).take(12) {
        let Some((attrs, _)) = node.split_once('>') else {
            continue;
        };
        let Some(link) = attrs
            .split_once("src=\"")
            .and_then(|(_, s)| s.split_once('"'))
            .map(|(link, _)| link)
        else {
            continue;
        };
        if link.starts_with("/imgs/") {
            return Err("Imagetwist reports an unavailable original image".into());
        }
        if let Some((url, fallback)) = media_url(link) {
            let ext = fallback.rsplit_once('.').map(|(_, ext)| ext).unwrap_or("jpg");
            let filename = attrs
                .split_once("alt=\"")
                .and_then(|(_, s)| s.split_once('"'))
                .and_then(|(alt, _)| alt_filename(alt, ext))
                .unwrap_or(fallback);
            original = Some((url, filename));
            break;
        }
    }
    let (url, filename) = original.ok_or("Imagetwist page has no validated original")?;
    let file = MediaItem {
        url: url.to_string(),
        filename,
        extractor: "imagetwist",
        post_id: None,
    };
    let mut metadata = media_metadata(&file);
    metadata.insert("subcategory".into(), "image".into());
    metadata.insert("token".into(), image.token.clone().into());
    metadata.insert("post_url".into(), format!("https://{}/{}", image.host, image.token).into());
    Ok((file, metadata))
}

async fn resolve_at_base(
    client: &Client,
    image: &Image,
    base: &Url,
) -> Result<(MediaItem, Metadata), Box<dyn Error + Send + Sync>> {
    let mut response = client
        .get(base.join(&image.token)?)
        .send()
        .await?
        .error_for_status()?;
    if response
        .content_length()
        .is_some_and(|n| n > MAX_HTML_BYTES as u64)
    {
        return Err("Imagetwist HTML exceeds preview byte bound".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_HTML_BYTES.saturating_sub(bytes.len()) {
            return Err("Imagetwist HTML exceeds preview byte bound".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    parse_page(std::str::from_utf8(&bytes)?, image)
}

pub async fn resolve(
    client: &Client,
    image: Image,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse(&format!("https://{}/", image.host))?;
    let (file, _) = resolve_at_base(client, &image, &base).await?;
    Ok(vec![file])
}

pub async fn resolve_messages(
    client: &Client,
    image: Image,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse(&format!("https://{}/", image.host))?;
    let (file, metadata) = resolve_at_base(client, &image, &base).await?;
    Ok(vec![
        ExtractorMessage::Directory {
            metadata: metadata.clone(),
        },
        ExtractorMessage::Url {
            url: file.url,
            metadata,
        },
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    #[test]
    fn pinned_image_hosts_and_token_routes() {
        for host in [
            "imagetwist.com",
            "www.imagetwist.com",
            "phun.imagetwist.com",
            "imagehaha.com",
        ] {
            assert!(match_url(&format!("https://{host}/f1i2s4vhvbrq/test.png")).is_some());
        }
        for bad in [
            "http://imagetwist.com/f1i2s4vhvbrq",
            "https://evilimagetwist.com/f1i2s4vhvbrq",
            "https://imagetwist.com/f1i2s4vhvbrq?private=1",
            "https://imagetwist.com/abc/filename.jpg",
            "https://imagetwist.com/f1i2s4vhvbrq/../../tmp",
            "https://imagetwist.com/f1i2s4vhvbrq/file.exe",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn original_media_keeps_safe_filename_and_rejects_placeholder() {
        let image = match_url("https://imagetwist.com/f1i2s4vhvbrq").unwrap();
        let html = r#"<img src="https://img.imagetwist.com/files/original.png" alt="test &amp; image">"#;
        let (file, metadata) = parse_page(html, &image).unwrap();
        assert_eq!(file.filename, "test & image.png");
        assert_eq!(metadata["token"], "f1i2s4vhvbrq");
        assert!(parse_page(r#"<img src="/imgs/noimage.jpg">"#, &image).is_err());
        for bad in [
            "http://img.imagetwist.com/files/a.jpg",
            "https://evilimagetwist.com/files/a.jpg",
            "https://img.imagetwist.com/imgs/blank.png",
            "https://img.imagetwist.com/files/a.jpg?token=secret",
        ] {
            assert!(media_url(bad).is_none(), "{bad}");
        }
    }

    #[tokio::test]
    async fn fetches_one_bounded_public_page_without_downloading_media() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = [0u8; 4096];
            let n = socket.read(&mut bytes).await.unwrap();
            assert!(String::from_utf8_lossy(&bytes[..n])
                .starts_with("GET /f1i2s4vhvbrq HTTP/1.1"));
            let html = r#"<img src="https://img.imagetwist.com/files/original.png" alt="Public">"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
                html.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let image = match_url("https://imagetwist.com/f1i2s4vhvbrq").unwrap();
        let (file, _) = resolve_at_base(&Client::new(), &image, &base)
            .await
            .unwrap();
        assert_eq!(file.filename, "Public.png");
        server.await.unwrap();
    }
}
