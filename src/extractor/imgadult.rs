//! Independent pinned imagehosts.py Imgadult public single-image preview.
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
    token: String,
}

pub fn match_url(input: &str) -> Option<Target> {
    let lower = input.to_ascii_lowercase();
    if lower.contains("/../") || lower.contains("/./") || lower.contains("/%2e")
        || lower.contains("%2f") || lower.contains("%5c")
    {
        return None;
    }
    let url = Url::parse(input).ok()?;
    if url.scheme() != "https"
        || !matches!(url.host_str(), Some("imgadult.com" | "www.imgadult.com"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let parts: Vec<_> = url.path_segments()?.collect();
    let [name] = parts.as_slice() else {
        return None;
    };
    let token = name.strip_prefix("img-")?.strip_suffix(".html")?;
    if !(1..=80).contains(&token.len())
        || !token.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return None;
    }
    Some(Target { token: token.into() })
}

fn safe_media(raw: &str) -> Option<(Url, String)> {
    let url = Url::parse(raw).ok()?;
    if url.scheme() != "https"
        || !matches!(url.host_str(), Some("imgadult.com" | "www.imgadult.com"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let filename = url.path_segments()?.next_back()?.to_owned();
    let extension = filename.rsplit_once('.')?.1.to_ascii_lowercase();
    if filename.is_empty() || filename.len() > 240 || filename.contains('%')
        || !matches!(extension.as_str(), "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif")
    {
        return None;
    }
    let name = safe_filename(&filename);
    Some((url, name))
}

fn safe_alt(raw: &str, extension: &str) -> Option<String> {
    let name = raw
        .strip_suffix(" image hosted at ImgAdult.com")
        .unwrap_or(raw)
        .replace("&amp;", "&")
        .replace("&#39;", "'")
        .replace("&quot;", "\"");
    if name.is_empty()
        || name.len() > 230
        || name.contains('/')
        || name.contains('\\')
        || name.chars().any(char::is_control)
    {
        return None;
    }
    let final_name = if name.rsplit_once('.').is_some_and(|(_, ext)| {
        matches!(ext.to_ascii_lowercase().as_str(), "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif")
    }) {
        name
    } else {
        format!("{name}.{extension}")
    };
    Some(safe_filename(&final_name))
}

fn parse_page(
    html: &str,
    target: &Target,
) -> Result<(MediaItem, Metadata), Box<dyn Error + Send + Sync>> {
    let after = html
        .split_once("' src='")
        .map(|(_, text)| text)
        .ok_or("Imgadult public original img source is absent")?;
    let raw = after
        .split_once('\'')
        .map(|(url, _)| url)
        .ok_or("Imgadult original image URL is malformed")?;
    let (url, fallback) =
        safe_media(raw).ok_or("Imgadult public media origin is unsafe")?;
    let ext = fallback.rsplit_once('.').map(|(_, ext)| ext).unwrap_or("jpg");
    let alt = after
        .split_once("alt='")
        .and_then(|(_, text)| text.split_once('\''))
        .map(|(name, _)| name);
    let name = alt.and_then(|name| safe_alt(name, ext)).unwrap_or(fallback);
    let media = MediaItem {
        url: url.to_string(),
        filename: name,
        extractor: "imgadult",
        post_id: None,
    };
    let mut meta = media_metadata(&media);
    meta.insert("subcategory".into(), "image".into());
    meta.insert("token".into(), target.token.clone().into());
    meta.insert(
        "post_url".into(),
        format!("https://imgadult.com/img-{}.html", target.token).into(),
    );
    Ok((media, meta))
}

async fn at_base(
    client: &Client,
    target: &Target,
    base: &Url,
) -> Result<(MediaItem, Metadata), Box<dyn Error + Send + Sync>> {
    let mut response = client
        .get(base.join(&format!("img-{}.html", target.token))?)
        .header(reqwest::header::COOKIE, "img_i_d=1")
        .send()
        .await?
        .error_for_status()?;
    if response
        .content_length()
        .is_some_and(|size| size > MAX_HTML_BYTES as u64)
    {
        return Err("Imgadult public HTML exceeds response limit".into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_HTML_BYTES.saturating_sub(body.len()) {
            return Err("Imgadult public HTML exceeds response limit".into());
        }
        body.extend_from_slice(&chunk);
    }
    parse_page(std::str::from_utf8(&body)?, target)
}

pub async fn resolve(
    client: &Client,
    target: Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let (file, _) = at_base(client, &target, &Url::parse("https://imgadult.com/")?).await?;
    Ok(vec![file])
}

pub async fn resolve_messages(
    client: &Client,
    target: Target,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let (file, metadata) = at_base(client, &target, &Url::parse("https://imgadult.com/")?).await?;
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
    fn pinned_public_imgadult_url_and_safe_media_shapes() {
        assert_eq!(
            match_url("https://imgadult.com/img-686b4f451af05.html"),
            Some(Target { token: "686b4f451af05".into() })
        );
        for bad in [
            "http://imgadult.com/img-686b4f451af05.html",
            "https://evilimgadult.com/img-686b4f451af05.html",
            "https://imgadult.com/img-686b4f451af05.html?token=x",
            "https://imgadult.com/img-ABCDEF.html",
            "https://imgadult.com/img-a%2Fb.html",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
        for bad in [
            "http://imgadult.com/upload/big/a.jpg",
            "https://evilimgadult.com/upload/big/a.jpg",
            "https://imgadult.com/upload/big/a.jpg?secret=x",
            "https://imgadult.com/upload/big/a.exe",
        ] {
            assert!(safe_media(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn original_media_and_site_suffix_are_safely_decoded() {
        let html = "<a href='#'><img alt='1612 image hosted at ImgAdult.com' src='https://imgadult.com/upload/big/2025/07/07/686b4f451af04.jpg'></a>";
        let (file, meta) = parse_page(html, &Target { token: "686b4f451af05".into() }).unwrap();
        assert_eq!(file.url, "https://imgadult.com/upload/big/2025/07/07/686b4f451af04.jpg");
        assert_eq!(file.filename, "1612.jpg");
        assert_eq!(meta["token"], "686b4f451af05");
        assert!(safe_alt("../bad image hosted at ImgAdult.com", "jpg").is_none());
    }

    #[tokio::test]
    async fn cookie_and_one_html_request_are_exercised_offline() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 4096];
            let n = socket.read(&mut request).await.unwrap();
            let text = String::from_utf8_lossy(&request[..n]);
            assert!(text.starts_with("GET /img-686b4f451af05.html HTTP/1.1"), "{text}");
            assert!(text.to_ascii_lowercase().contains("cookie: img_i_d=1"), "{text}");
            let html = "<img alt='1612 image hosted at ImgAdult.com' src='https://imgadult.com/upload/big/2025/07/07/a.jpg'>";
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
                html.len()
            );
            socket.write_all(reply.as_bytes()).await.unwrap();
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let target = match_url("https://imgadult.com/img-686b4f451af05.html").unwrap();
        let (item, _) = at_base(&Client::new(), &target, &base).await.unwrap();
        assert_eq!(item.filename, "1612.jpg");
        server.await.unwrap();
    }
}
