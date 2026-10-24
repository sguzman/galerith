//! Conservative public Fappic image subset of pinned imagehosts.py.
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

fn valid_token(token: &str) -> bool {
    (10..=80).contains(&token.len())
        && token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn valid_page_name(name: &str) -> bool {
    (5..=180).contains(&name.len())
        && !name.contains("..")
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

pub fn match_url(input: &str) -> Option<Target> {
    let lower = input.to_ascii_lowercase();
    if ["/../", "/./", "%2e", "%2f", "%5c"]
        .iter()
        .any(|pattern| lower.contains(pattern))
    {
        return None;
    }
    let url = Url::parse(input).ok()?;
    let host = url.host_str()?;
    let trusted = matches!(host, "fappic.com" | "www.fappic.com")
        || host
            .strip_prefix("img")
            .and_then(|suffix| suffix.strip_suffix(".fappic.com"))
            .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()));
    if url.scheme() != "https"
        || !trusted
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let parts: Vec<_> = url.path_segments()?.collect();
    let token = match parts.as_slice() {
        [token, filename] if valid_token(token) && valid_page_name(filename) => *token,
        ["i", folder, thumb_token, filename]
            if (1..=10).contains(&folder.len())
                && folder.bytes().all(|byte| byte.is_ascii_digit())
                && valid_page_name(filename) =>
        {
            let source = thumb_token.strip_suffix("_t")?;
            if !valid_token(source) {
                return None;
            }
            source
        }
        _ => return None,
    };
    Some(Target {
        token: token.to_owned(),
    })
}

fn safe_media(raw: &str) -> Option<(Url, String)> {
    let url = Url::parse(raw).ok()?;
    let host = url.host_str()?;
    if url.scheme() != "https"
        || !(host == "fappic.com" || host.ends_with(".fappic.com"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let filename = url.path_segments()?.next_back()?;
    let ext = filename.rsplit_once('.')?.1.to_ascii_lowercase();
    if filename.len() > 240
        || filename.is_empty()
        || filename.contains('%')
        || !matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif")
    {
        return None;
    }
    let safe = safe_filename(filename);
    Some((url, safe))
}

fn safe_alt(raw: &str, ext: &str) -> Option<String> {
    let name = raw
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");
    let title = ["Porn Pics ", "Porn-Pics ", "Porn Picture ", "Porn-Picture "]
        .iter()
        .find_map(|prefix| name.strip_prefix(prefix))
        .unwrap_or(&name);
    if title.is_empty()
        || title.len() > 220
        || title.contains('/')
        || title.contains('\\')
        || title.chars().any(char::is_control)
    {
        return None;
    }
    let filename = if title.rsplit_once('.').is_some_and(|(_, suffix)| {
        matches!(
            suffix.to_ascii_lowercase().as_str(),
            "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif"
        )
    }) {
        title.to_owned()
    } else {
        format!("{title}.{ext}")
    };
    Some(safe_filename(&filename))
}

fn parse_page(
    html: &str,
    target: &Target,
) -> Result<(MediaItem, Metadata), Box<dyn Error + Send + Sync>> {
    let content = html
        .split_once("<a href=\"#\"><img src=\"")
        .map(|(_, remainder)| remainder)
        .ok_or("Fappic public original image marker is missing")?;
    let (raw, rest) = content
        .split_once('"')
        .ok_or("Fappic public image URL is incomplete")?;
    let (url, fallback) =
        safe_media(raw).ok_or("Fappic original media origin or filename is unsafe")?;
    let attrs = rest
        .split_once('>')
        .map(|(attrs, _)| attrs)
        .ok_or("Fappic original image element is incomplete")?;
    let ext = fallback.rsplit_once('.').map(|(_, ext)| ext).unwrap_or("jpg");
    let filename = attrs
        .split_once(" alt=\"")
        .and_then(|(_, suffix)| suffix.split_once('"'))
        .and_then(|(alt, _)| safe_alt(alt, ext))
        .unwrap_or(fallback);
    let media = MediaItem {
        url: url.to_string(),
        filename,
        extractor: "fappic",
        post_id: None,
    };
    let mut metadata = media_metadata(&media);
    metadata.insert("subcategory".into(), "image".into());
    metadata.insert("token".into(), target.token.clone().into());
    metadata.insert(
        "post_url".into(),
        format!("https://fappic.com/{}/pic.jpg", target.token).into(),
    );
    Ok((media, metadata))
}

async fn at_base(
    client: &Client,
    target: &Target,
    base: &Url,
) -> Result<(MediaItem, Metadata), Box<dyn Error + Send + Sync>> {
    let endpoint = base.join(&format!("{}/pic.jpg", target.token))?;
    let mut response = client.get(endpoint).send().await?.error_for_status()?;
    if response
        .content_length()
        .is_some_and(|size| size > MAX_HTML_BYTES as u64)
    {
        return Err("Fappic source HTML exceeds response size cap".into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_HTML_BYTES.saturating_sub(body.len()) {
            return Err("Fappic source HTML exceeds response size cap".into());
        }
        body.extend_from_slice(&chunk);
    }
    parse_page(std::str::from_utf8(&body)?, target)
}

pub async fn resolve(
    client: &Client,
    target: Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let (item, _) = at_base(client, &target, &Url::parse("https://fappic.com/")?).await?;
    Ok(vec![item])
}

pub async fn resolve_messages(
    client: &Client,
    target: Target,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let (item, metadata) = at_base(client, &target, &Url::parse("https://fappic.com/")?).await?;
    Ok(vec![
        ExtractorMessage::Directory {
            metadata: metadata.clone(),
        },
        ExtractorMessage::Url {
            url: item.url,
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
    fn pinned_fappic_url_aliases_and_safe_thumbnail_token() {
        let target = Target {
            token: "abcde12345".into(),
        };
        assert_eq!(
            match_url("https://fappic.com/abcde12345/NAME.EXT"),
            Some(target.clone())
        );
        assert_eq!(
            match_url("https://img3.fappic.com/i/99/abcde12345_t/thumb.jpg"),
            Some(target)
        );
        for input in [
            "http://fappic.com/abcde12345/file.jpg",
            "https://evilfappic.com/abcde12345/file.jpg",
            "https://fappic.com/short/file.jpg",
            "https://fappic.com/abcde12345/file.jpg?secret=x",
            "https://fappic.com/abcde12345/file.jpg#fragment",
            "https://fappic.com/i/99/abcde12345/pic.jpg",
            "https://fappic.com/abcde12345/../file.jpg",
        ] {
            assert!(match_url(input).is_none(), "{input}");
        }
    }

    #[test]
    fn pinned_original_selector_and_sanitized_filename_prefix() {
        let target = match_url("https://fappic.com/abcde12345/photo.jpg").unwrap();
        let page = r##"<div><a href="#"><img src="https://img3.fappic.com/original/photo.png" alt="Porn Pics Nice &amp; Safe"></a></div>"##;
        let (media, metadata) = parse_page(page, &target).unwrap();
        assert_eq!(media.url, "https://img3.fappic.com/original/photo.png");
        assert_eq!(media.filename, "Nice & Safe.png");
        assert_eq!(metadata["token"], "abcde12345");
        assert!(metadata.get("source_post_id").is_none());
    }

    #[test]
    fn rejects_unsupported_origins_and_disallowed_names() {
        let target = match_url("https://fappic.com/abcde12345/photo.jpg").unwrap();
        for html in [
            r##"<a href="#"><img src="https://foreign.invalid/photo.png" alt="Nice">"##,
            r##"<a href="#"><img src="http://fappic.com/photo.png">"##,
            r##"<a href="#"><img src="https://fappic.com/photo.exe">"##,
            r##"<a href="#"><img src="https://fappic.com/photo.jpg?secret=x">"##,
        ] {
            assert!(parse_page(html, &target).is_err(), "{html}");
        }
        let (media, _) = parse_page(
            r##"<a href="#"><img src="https://fappic.com/file.webp" alt="../unsafe">"##,
            &target,
        )
        .unwrap();
        assert_eq!(media.filename, "file.webp");
    }

    #[tokio::test]
    async fn pinned_thumbnail_rewrite_uses_one_bounded_canonical_fetch() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 4096];
            let length = stream.read(&mut request).await.unwrap();
            assert!(
                String::from_utf8_lossy(&request[..length])
                    .starts_with("GET /abcde12345/pic.jpg HTTP/1.1")
            );
            let html = r##"<a href="#"><img src="https://img3.fappic.com/original/file.webp" alt="Picture">"##;
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
                html.len()
            );
            stream.write_all(reply.as_bytes()).await.unwrap();
        });
        let target = match_url("https://img3.fappic.com/i/99/abcde12345_t/thumb.jpg").unwrap();
        let base = Url::parse(&format!("http://{address}/")).unwrap();
        let (file, metadata) = at_base(&Client::new(), &target, &base).await.unwrap();
        assert_eq!(file.filename, "Picture.webp");
        assert_eq!(metadata["token"], "abcde12345");
        server.await.unwrap();
    }
}
