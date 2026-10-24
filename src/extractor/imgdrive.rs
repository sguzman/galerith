//! Independent public Imgdrive, Imgtaxi and Imgwallet previews for the
//! pinned imagehosts.py family. No authenticated or live-parity claims.
use crate::{
    message::{media_metadata, ExtractorMessage, Metadata},
    safe_filename, MediaItem,
};
use reqwest::Client;
use std::error::Error;
use url::Url;

const MAX_HTML_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Imgdrive,
    Imgtaxi,
    Imgwallet,
}

impl Family {
    pub(crate) fn category(self) -> &'static str {
        match self {
            Self::Imgdrive => "imgdrive",
            Self::Imgtaxi => "imgtaxi",
            Self::Imgwallet => "imgwallet",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub family: Family,
    domain: String,
    token: String,
}

fn family_for_host(host: &str) -> Option<Family> {
    let trimmed = host.strip_prefix("www.").unwrap_or(host);
    let name = trimmed.split_once('.')?.0;
    let tld = trimmed.split_once('.')?.1;
    if !matches!(tld, "com" | "net") {
        return None;
    }
    match name {
        "imgdrive" => Some(Family::Imgdrive),
        "imgtaxi" => Some(Family::Imgtaxi),
        "imgwallet" => Some(Family::Imgwallet),
        _ => None,
    }
}

fn safe_token(raw: &str) -> bool {
    (1..=80).contains(&raw.len()) && raw.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

pub fn match_url(input: &str) -> Option<Target> {
    let lower = input.to_ascii_lowercase();
    if lower.contains("/../")
        || lower.contains("/./")
        || lower.contains("/%2e")
        || lower.contains("%2f")
        || lower.contains("%5c")
    {
        return None;
    }
    let url = Url::parse(input).ok()?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let host = url.host_str()?;
    let family = family_for_host(host)?;
    let parts: Vec<_> = url.path_segments()?.collect();
    let [file] = parts.as_slice() else {
        return None;
    };
    let token = file.strip_prefix("img-")?.strip_suffix(".html")?;
    if !safe_token(token) {
        return None;
    }
    Some(Target {
        family,
        domain: host.strip_prefix("www.").unwrap_or(host).to_owned(),
        token: token.to_owned(),
    })
}

fn og_value<'a>(html: &'a str, key: &str) -> Option<&'a str> {
    let marker = format!("property=\"og:{key}\" content=\"");
    let value = html.split_once(&marker)?.1.split_once('"')?.0;
    if value.is_empty() || value.len() > 2048 || value.chars().any(char::is_control) {
        return None;
    }
    Some(value)
}

fn original_media(input: &str, domain: &str) -> Option<(Url, String)> {
    let mut url = Url::parse(input).ok()?;
    let host = url.host_str()?;
    if url.scheme() != "https"
        || !(host == domain || host.ends_with(&format!(".{domain}")))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    // Pinned Imagehost extractors replace the /small/ preview path with /big/.
    let original_path = url.path().replace("/small/", "/big/");
    url.set_path(&original_path);
    let filename = url.path_segments()?.next_back()?.to_owned();
    if filename.is_empty() || filename.len() > 240 || filename.contains('%') {
        return None;
    }
    let ext = filename.rsplit_once('.')?.1.to_ascii_lowercase();
    if !matches!(
        ext.as_str(),
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif"
    ) {
        return None;
    }
    let name = safe_filename(&filename);
    Some((url, name))
}

fn image_name(raw: &str, ext: &str) -> Option<String> {
    let title = raw.split(" | ").next()?.trim();
    if title.is_empty()
        || title.len() > 225
        || title.contains('/')
        || title.contains('\\')
        || title.chars().any(char::is_control)
    {
        return None;
    }
    let decoded = title
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");
    if decoded.chars().any(char::is_control) {
        return None;
    }
    let path = if decoded.rsplit_once('.').is_some_and(|(_, suffix)| {
        matches!(
            suffix.to_ascii_lowercase().as_str(),
            "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif"
        )
    }) {
        decoded
    } else {
        format!("{decoded}.{ext}")
    };
    Some(safe_filename(&path))
}

fn media_from_page(
    html: &str,
    target: &Target,
) -> Result<(MediaItem, Metadata), Box<dyn Error + Send + Sync>> {
    let title = og_value(html, "title").ok_or("Imagehost has no public OG title")?;
    let image = og_value(html, "image").ok_or("Imagehost has no public OG image")?;
    let (url, fallback) =
        original_media(image, &target.domain).ok_or("Imagehost image origin is not trusted")?;
    let ext = fallback
        .rsplit_once('.')
        .map(|(_, ext)| ext)
        .unwrap_or("jpg");
    let filename = image_name(title, ext).unwrap_or(fallback);
    let media = MediaItem {
        url: url.to_string(),
        filename,
        extractor: target.family.category(),
        post_id: None,
    };
    let mut metadata = media_metadata(&media);
    metadata.insert("subcategory".into(), "image".into());
    metadata.insert("token".into(), target.token.clone().into());
    metadata.insert(
        "post_url".into(),
        format!("https://{}/img-{}.html", target.domain, target.token).into(),
    );
    Ok((media, metadata))
}

async fn at_base(
    client: &Client,
    target: &Target,
    base: &Url,
) -> Result<(MediaItem, Metadata), Box<dyn Error + Send + Sync>> {
    let endpoint = base.join(&format!("img-{}.html", target.token))?;
    let mut response = client.get(endpoint).send().await?.error_for_status()?;
    if response
        .content_length()
        .is_some_and(|size| size > MAX_HTML_BYTES as u64)
    {
        return Err("Imagehost HTML response exceeds byte limit".into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_HTML_BYTES.saturating_sub(body.len()) {
            return Err("Imagehost HTML response exceeds byte limit".into());
        }
        body.extend_from_slice(&chunk);
    }
    media_from_page(std::str::from_utf8(&body)?, target)
}

pub async fn resolve(
    client: &Client,
    target: Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse(&format!("https://{}/", target.domain))?;
    let (media, _) = at_base(client, &target, &base).await?;
    Ok(vec![media])
}

pub async fn resolve_messages(
    client: &Client,
    target: Target,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse(&format!("https://{}/", target.domain))?;
    let (media, metadata) = at_base(client, &target, &base).await?;
    Ok(vec![
        ExtractorMessage::Directory {
            metadata: metadata.clone(),
        },
        ExtractorMessage::Url {
            url: media.url,
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
    fn pinned_imgdrive_imgtaxi_and_imgwallet_routes_match() {
        for (url, family, token) in [
            (
                "https://imgdrive.net/img-61ac0caeabf35.html",
                Family::Imgdrive,
                "61ac0caeabf35",
            ),
            (
                "https://imgtaxi.com/img-61c71cea940d6.html",
                Family::Imgtaxi,
                "61c71cea940d6",
            ),
            (
                "https://imgwallet.com/img-62fd3045ceec6.html",
                Family::Imgwallet,
                "62fd3045ceec6",
            ),
        ] {
            let target = match_url(url).unwrap();
            assert_eq!(target.family, family);
            assert_eq!(target.token, token);
        }
        for bad in [
            "http://imgdrive.net/img-61ac0caeabf35.html",
            "https://evilimgdrive.net/img-61ac0caeabf35.html",
            "https://imgdrive.net/img-61ac0caeabf35.html?auth=x",
            "https://imgdrive.net/img-61ac0caeabf35.html#fragment",
            "https://imgtaxi.net/a/../img-61c71cea940d6.html",
            "https://imgwallet.com/img-%2e%2e.html",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn og_images_upgrade_small_to_big_and_retain_safe_names() {
        let examples = [
            (
                "https://imgdrive.net/img-61ac0caeabf35.html",
                "https://imgdrive.net/images/small/2021/12/05/61ac0caeabf33.JPG",
                "5yl_0001 | imgdrive.net",
                "5yl_0001.JPG",
                "https://imgdrive.net/images/big/2021/12/05/61ac0caeabf33.JPG",
            ),
            (
                "https://imgtaxi.com/img-61c71cea940d6.html",
                "https://imgtaxi.com/images/small/2021/12/25/61c71cea940d5.jpg",
                "SLn_0001 | imgtaxi.com",
                "SLn_0001.jpg",
                "https://imgtaxi.com/images/big/2021/12/25/61c71cea940d5.jpg",
            ),
            (
                "https://imgwallet.com/img-62fd3045ceec6.html",
                "https://imgwallet.com/images/small/2022/08/17/62fd3045ceec5.JPG",
                "S0ph (1) | imgwallet.com",
                "S0ph (1).JPG",
                "https://imgwallet.com/images/big/2022/08/17/62fd3045ceec5.JPG",
            ),
        ];
        for (source, image, title, filename, expected_url) in examples {
            let target = match_url(source).unwrap();
            let html = format!(
                "<meta property=\"og:title\" content=\"{title}\"><meta property=\"og:image\" content=\"{image}\">"
            );
            let (media, meta) = media_from_page(&html, &target).unwrap();
            assert_eq!(media.url, expected_url);
            assert_eq!(media.filename, filename);
            assert_eq!(meta["token"], target.token);
            assert!(meta.get("source_post_id").is_none());
        }
    }

    #[test]
    fn unsafe_media_urls_and_original_names_are_excluded() {
        for unsafe_url in [
            "https://evilimgdrive.net/file.jpg",
            "http://imgdrive.net/file.jpg",
            "https://imgdrive.net/file.jpg?token=secret",
            "https://imgdrive.net/file.jpg#fragment",
            "https://user:password@imgdrive.net/file.jpg",
            "https://imgdrive.net/file.exe",
        ] {
            assert!(original_media(unsafe_url, "imgdrive.net").is_none());
        }
        assert!(image_name("../evil", "jpg").is_none());
        assert!(image_name("dir/name", "jpg").is_none());
        assert!(image_name("bad\u{0001}file", "jpg").is_none());
    }

    #[tokio::test]
    async fn public_page_reads_one_request_and_rejects_oversized_response_header() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 4096];
            let n = socket.read(&mut request).await.unwrap();
            let text = String::from_utf8_lossy(&request[..n]);
            assert!(
                text.starts_with("GET /img-61ac0caeabf35.html HTTP/1.1"),
                "{text}"
            );
            let html = "<meta property=\"og:title\" content=\"Public | imagehost\"><meta property=\"og:image\" content=\"https://imgdrive.net/images/small/file.png\">";
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
                html.len()
            );
            socket.write_all(reply.as_bytes()).await.unwrap();
        });
        let target = match_url("https://imgdrive.net/img-61ac0caeabf35.html").unwrap();
        let base = Url::parse(&format!("http://{address}/")).unwrap();
        let (file, _) = at_base(&Client::new(), &target, &base).await.unwrap();
        assert_eq!(file.url, "https://imgdrive.net/images/big/file.png");
        assert_eq!(file.filename, "Public.png");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn rejects_large_declared_response_without_parsing_body() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 4096];
            let _ = socket.read(&mut request).await.unwrap();
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                MAX_HTML_BYTES + 1
            );
            socket.write_all(reply.as_bytes()).await.unwrap();
        });
        let target = match_url("https://imgwallet.com/img-62fd3045ceec6.html").unwrap();
        let base = Url::parse(&format!("http://{address}/")).unwrap();
        let error = at_base(&Client::new(), &target, &base).await.unwrap_err();
        assert!(error.to_string().contains("byte limit"));
        server.await.unwrap();
    }
}
