//! Pinned Turboimagehost public single-image preview (imagehosts.py).
use crate::{
    message::{media_metadata, ExtractorMessage, Metadata},
    safe_filename, MediaItem,
};
use reqwest::Client;
use std::error::Error;
use url::Url;

const MAX_HTML_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    id: String,
    path: String,
}

pub fn match_url(input: &str) -> Option<Target> {
    let raw = input.to_ascii_lowercase();
    if raw.contains("/../") || raw.contains("/./") || raw.contains("/%2e")
        || raw.contains("%2f") || raw.contains("%5c")
    {
        return None;
    }
    let url = Url::parse(input).ok()?;
    if url.scheme() != "https"
        || !matches!(url.host_str(), Some("turboimagehost.com" | "www.turboimagehost.com"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let parts: Vec<_> = url.path_segments()?.collect();
    let ["p", id, file] = parts.as_slice() else {
        return None;
    };
    if !(1..=24).contains(&id.len()) || !id.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let name = file.strip_suffix(".html")?;
    if name.is_empty() || name.len() > 180
        || !name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return None;
    }
    Some(Target {
        id: (*id).to_owned(),
        path: format!("p/{id}/{file}"),
    })
}

fn trusted_media(raw: &str) -> Option<(Url, String)> {
    let url = Url::parse(raw).ok()?;
    let host = url.host_str()?;
    if url.scheme() != "https"
        || !(host == "turboimagehost.com" || host.ends_with(".turboimagehost.com"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let name = url.path_segments()?.next_back()?.to_owned();
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    if name.is_empty() || name.len() > 240 || name.contains('%')
        || !matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif")
    {
        return None;
    }
    let filename = safe_filename(&name);
    Some((url, filename))
}

fn parse_page(html: &str, target: &Target) -> Result<(MediaItem, Metadata), Box<dyn Error + Send + Sync>> {
    let tag = html.split_once("<img ").map(|(_, v)| v)
        .and_then(|v| v.split_once('>').map(|(tag, _)| tag))
        .ok_or("Turboimagehost public image is absent")?;
    let link = tag.split_once("src=\"")
        .and_then(|(_, v)| v.split_once('"').map(|(url, _)| url))
        .ok_or("Turboimagehost image has no src")?;
    let (url, filename) = trusted_media(link)
        .ok_or("Turboimagehost media origin is unsupported")?;
    let item = MediaItem {
        url: url.to_string(), filename, extractor: "turboimagehost", post_id: None,
    };
    let mut metadata = media_metadata(&item);
    metadata.insert("subcategory".into(), "image".into());
    metadata.insert("token".into(), target.id.clone().into());
    metadata.insert(
        "post_url".into(),
        format!("https://www.turboimagehost.com/{}", target.path).into(),
    );
    Ok((item, metadata))
}

async fn at_base(
    client: &Client, target: &Target, base: &Url,
) -> Result<(MediaItem, Metadata), Box<dyn Error + Send + Sync>> {
    let mut response = client.get(base.join(&target.path)?).send().await?.error_for_status()?;
    if response.content_length().is_some_and(|n| n > MAX_HTML_BYTES as u64) {
        return Err("Turboimagehost public HTML exceeds response cap".into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_HTML_BYTES.saturating_sub(body.len()) {
            return Err("Turboimagehost public HTML exceeds response cap".into());
        }
        body.extend_from_slice(&chunk);
    }
    parse_page(std::str::from_utf8(&body)?, target)
}

pub async fn resolve(
    client: &Client, target: Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let (item, _) = at_base(
        client, &target, &Url::parse("https://www.turboimagehost.com/")?,
    ).await?;
    Ok(vec![item])
}

pub async fn resolve_messages(
    client: &Client, target: Target,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let (item, metadata) = at_base(
        client, &target, &Url::parse("https://www.turboimagehost.com/")?,
    ).await?;
    Ok(vec![
        ExtractorMessage::Directory { metadata: metadata.clone() },
        ExtractorMessage::Url { url: item.url, metadata },
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::TcpListener};

    #[test]
    fn matches_only_public_image_url_shapes() {
        assert_eq!(
            match_url("https://www.turboimagehost.com/p/39078423/test--.png.html"),
            Some(Target {
                id: "39078423".into(),
                path: "p/39078423/test--.png.html".into()
            })
        );
        for bad in [
            "http://turboimagehost.com/p/39078423/file.jpg.html",
            "https://evilturboimagehost.com/p/39078423/file.jpg.html",
            "https://turboimagehost.com/p/not-digits/file.jpg.html",
            "https://turboimagehost.com/p/39078423/file.jpg.html?token=x",
            "https://turboimagehost.com/album/344597/example",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn first_public_image_uses_trusted_media_and_real_id() {
        let target = match_url("https://turboimagehost.com/p/39078423/test--.png.html").unwrap();
        let html = r#"<img src="https://i1.turboimagehost.com/files/photo.png">"#;
        let (item, metadata) = parse_page(html, &target).unwrap();
        assert_eq!(item.filename, "photo.png");
        assert_eq!(metadata["token"], "39078423");
        assert_eq!(metadata["post_url"], "https://www.turboimagehost.com/p/39078423/test--.png.html");
        for bad in [
            "http://i1.turboimagehost.com/files/photo.jpg",
            "https://evil.example/photo.jpg",
            "https://evilturboimagehost.com/photo.jpg",
            "https://i1.turboimagehost.com/photo.jpg?token=x",
        ] {
            assert!(trusted_media(bad).is_none(), "{bad}");
        }
    }

    #[tokio::test]
    async fn public_image_fetches_one_bounded_html_response() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 4096];
            let n = socket.read(&mut request).await.unwrap();
            assert!(String::from_utf8_lossy(&request[..n])
                .starts_with("GET /p/39078423/test--.png.html HTTP/1.1"));
            let html = r#"<img src="https://img.turboimagehost.com/files/photo.png">"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
                html.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        let base = Url::parse(&format!("http://{address}/")).unwrap();
        let target = match_url("https://www.turboimagehost.com/p/39078423/test--.png.html").unwrap();
        let (item, _) = at_base(&Client::new(), &target, &base).await.unwrap();
        assert_eq!(item.filename, "photo.png");
        server.await.unwrap();
    }
}
