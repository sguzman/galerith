//! Pinned imagehosts.py Picstate public image-only subset.
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
}

fn valid_id(value: &str) -> bool {
    (1..=80).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

pub fn match_url(input: &str) -> Option<Target> {
    let lower = input.to_ascii_lowercase();
    if lower.contains("/../")
        || lower.contains("/./")
        || lower.contains("/%2e%2e/")
        || lower.contains("/%2e/")
    {
        return None;
    }
    let url = Url::parse(input).ok()?;
    if url.scheme() != "https"
        || !matches!(url.host_str(), Some("picstate.com" | "www.picstate.com"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let path: Vec<_> = url.path_segments()?.collect();
    match path.as_slice() {
        ["view", "full", id] if valid_id(id) => Some(Target { id: (*id).into() }),
        _ => None,
    }
}

fn media_from_page(
    html: &str,
    id: &str,
) -> Result<(MediaItem, Metadata), Box<dyn Error + Send + Sync>> {
    let section = html
        .split_once(" id=\"image_container\"")
        .map(|(_, rest)| rest)
        .ok_or("Picstate page lacks public image container")?;
    let tag = section
        .split_once("<img ")
        .map(|(_, rest)| rest)
        .and_then(|tail| tail.split_once('>'))
        .map(|(attrs, _)| attrs)
        .ok_or("Picstate page lacks image element")?;
    let image = tag
        .split_once("src=\"")
        .and_then(|(_, rest)| rest.split_once('"'))
        .map(|(value, _)| value)
        .ok_or("Picstate public image has no src")?;
    let url = Url::parse(image)?;
    if url.scheme() != "https"
        || !matches!(url.host_str(), Some("picstate.com" | "www.picstate.com"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("Picstate media link has unsupported origin".into());
    }
    let segments: Vec<_> = url
        .path_segments()
        .ok_or("Picstate media URL has no path")?
        .collect();
    let [kind, source_id, filename] = segments.as_slice() else {
        return Err("Picstate public media path differs from expected shape".into());
    };
    if !matches!(*kind, "file" | "files") || *source_id != id {
        return Err("Picstate original media source ID does not match page".into());
    }
    let ext = filename
        .rsplit_once('.')
        .ok_or("Picstate media lacks extension")?
        .1;
    if filename.is_empty()
        || filename.len() > 240
        || filename.contains('%')
        || !matches!(
            ext.to_ascii_lowercase().as_str(),
            "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif"
        )
    {
        return Err("Picstate media extension or filename is unsafe".into());
    }
    let safe_media_name = safe_filename(filename);
    let custom_name = tag
        .split_once("alt=\"")
        .and_then(|(_, rest)| rest.split_once('"'))
        .map(|(value, _)| value)
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 230
                && !value.contains('/')
                && !value.contains('\\')
                && !value.chars().any(char::is_control)
        })
        .map(|value| {
            let has_ext = value.rsplit_once('.').is_some_and(|(_, suffix)| {
                matches!(
                    suffix.to_ascii_lowercase().as_str(),
                    "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif"
                )
            });
            let name = if has_ext {
                value.to_owned()
            } else {
                format!("{value}.{ext}")
            };
            safe_filename(&name)
        })
        .unwrap_or(safe_media_name);
    let file = MediaItem {
        url: url.to_string(),
        filename: custom_name,
        extractor: "picstate",
        post_id: None,
    };
    let mut meta = media_metadata(&file);
    meta.insert("subcategory".into(), "image".into());
    meta.insert("token".into(), id.into());
    meta.insert(
        "post_url".into(),
        format!("https://picstate.com/view/full/{id}").into(),
    );
    Ok((file, meta))
}

async fn at_base(
    client: &Client,
    id: &str,
    base: &Url,
) -> Result<(MediaItem, Metadata), Box<dyn Error + Send + Sync>> {
    let mut response = client
        .get(base.join(&format!("view/full/{id}"))?)
        .send()
        .await?
        .error_for_status()?;
    if response
        .content_length()
        .is_some_and(|v| v > MAX_HTML_BYTES as u64)
    {
        return Err("Picstate public HTML exceeds byte limit".into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_HTML_BYTES.saturating_sub(body.len()) {
            return Err("Picstate public HTML exceeds byte limit".into());
        }
        body.extend_from_slice(&chunk);
    }
    media_from_page(std::str::from_utf8(&body)?, id)
}

pub async fn resolve(
    client: &Client,
    target: Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let (item, _) = at_base(client, &target.id, &Url::parse("https://picstate.com/")?).await?;
    Ok(vec![item])
}

pub async fn resolve_messages(
    client: &Client,
    target: Target,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let (item, meta) = at_base(client, &target.id, &Url::parse("https://picstate.com/")?).await?;
    Ok(vec![
        ExtractorMessage::Directory {
            metadata: meta.clone(),
        },
        ExtractorMessage::Url {
            url: item.url,
            metadata: meta,
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
    fn pinned_public_image_urls_are_narrow() {
        for id in ["23416694_sfyue", "21812181_qxtpb"] {
            assert_eq!(
                match_url(&format!("https://picstate.com/view/full/{id}")),
                Some(Target { id: id.into() })
            );
        }
        for bad in [
            "http://picstate.com/view/full/23416694_sfyue",
            "https://evilpicstate.com/view/full/23416694_sfyue",
            "https://picstate.com/view/full/23416694_sfyue?private=1",
            "https://picstate.com/view/full/a%2Fb",
            "https://picstate.com/view/full/a/other",
            "https://picstate.com/view/full/x/../23416694_sfyue",
            "https://picstate.com/view/full/x/%2e%2e/23416694_sfyue",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn trusted_original_image_and_source_identity_are_required() {
        let body = r#"<div id="image_container"><img src="https://picstate.com/file/23416694_sfyue/test-___-_22__.png" alt="test-___-_22__"></div>"#;
        let (item, meta) = media_from_page(body, "23416694_sfyue").unwrap();
        assert_eq!(
            item.url,
            "https://picstate.com/file/23416694_sfyue/test-___-_22__.png"
        );
        assert_eq!(item.filename, "test-___-_22__.png");
        assert_eq!(meta["token"], "23416694_sfyue");
        assert!(media_from_page(body, "different_id").is_err());
        assert!(media_from_page("<html/>", "23416694_sfyue").is_err());
        for bad in [
            r#"<div id="image_container"><img src="https://foreign.example/file/23416694_sfyue/a.png"></div>"#,
            r#"<div id="image_container"><img src="https://picstate.com/file/23416694_sfyue/a.exe"></div>"#,
            r#"<div id="image_container"><img src="http://picstate.com/file/23416694_sfyue/a.png"></div>"#,
        ] {
            assert!(media_from_page(bad, "23416694_sfyue").is_err());
        }
    }

    #[tokio::test]
    async fn bounded_public_image_page_makes_one_http_request() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = [0_u8; 4096];
            let n = stream.read(&mut bytes).await.unwrap();
            assert!(String::from_utf8_lossy(&bytes[..n])
                .starts_with("GET /view/full/21812181_qxtpb HTTP/1.1"));
            let body = r#"<div id="image_container"><img src="https://picstate.com/files/21812181_qxtpb/0.jpg" alt="0"></div>"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let (file, _) = at_base(&Client::new(), "21812181_qxtpb", &base)
            .await
            .unwrap();
        assert_eq!(file.filename, "0.jpg");
        server.await.unwrap();
    }
}
