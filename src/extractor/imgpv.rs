//! Independent guarded public Imgpv image subset of pinned imagehosts.py.
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
    page: String,
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
    if url.scheme() != "https"
        || !matches!(url.host_str(), Some("imgpv.com" | "www.imgpv.com"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let parts: Vec<_> = url.path_segments()?.collect();
    let [token, page] = parts.as_slice() else {
        return None;
    };
    if !(10..=80).contains(&token.len())
        || !token
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        || !(6..=200).contains(&page.len())
        || !page.ends_with(".html")
        || page.contains("..")
        || !page
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
    {
        return None;
    }
    Some(Target {
        token: (*token).into(),
        page: (*page).into(),
    })
}

fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let marker = format!(" {name}=\"");
    tag.split_once(&marker)?
        .1
        .split_once('"')
        .map(|(value, _)| value)
}

fn trusted_media(raw: &str) -> Option<(Url, String)> {
    let url = Url::parse(raw).ok()?;
    let host = url.host_str()?;
    if url.scheme() != "https"
        || !(host == "imgpv.com" || host.ends_with(".imgpv.com"))
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
    if filename.is_empty()
        || filename.len() > 240
        || filename.contains('%')
        || !matches!(
            ext.as_str(),
            "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif"
        )
    {
        return None;
    }
    let safe = safe_filename(filename);
    Some((url, safe))
}

fn safe_alt(raw: &str, ext: &str) -> Option<String> {
    if raw.is_empty() || raw.len() > 230 {
        return None;
    }
    let text = raw
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");
    if text.is_empty()
        || text.contains('/')
        || text.contains('\\')
        || text.chars().any(char::is_control)
    {
        return None;
    }
    let filename = if text.rsplit_once('.').is_some_and(|(_, suffix)| {
        matches!(
            suffix.to_ascii_lowercase().as_str(),
            "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif"
        )
    }) {
        text
    } else {
        format!("{text}.{ext}")
    };
    Some(safe_filename(&filename))
}

fn parse_page(
    html: &str,
    target: &Target,
) -> Result<(MediaItem, Metadata), Box<dyn Error + Send + Sync>> {
    let after = html
        .split_once("id=\"img-preview\" src=\"")
        .map(|(_, remainder)| remainder)
        .ok_or("Imgpv public original image marker is absent")?;
    let (raw, rest) = after
        .split_once('"')
        .ok_or("Imgpv public original media attribute is incomplete")?;
    let (url, fallback) =
        trusted_media(raw).ok_or("Imgpv original media origin or filename is unsafe")?;
    let attrs = rest
        .split_once('>')
        .map(|(attrs, _)| attrs)
        .ok_or("Imgpv source image element is incomplete")?;
    let ext = fallback
        .rsplit_once('.')
        .map(|(_, ext)| ext)
        .unwrap_or("jpg");
    let filename = attribute(attrs, "alt")
        .and_then(|value| safe_alt(value, ext))
        .unwrap_or(fallback);
    let media = MediaItem {
        url: url.to_string(),
        filename,
        extractor: "imgpv",
        post_id: None,
    };
    let mut metadata = media_metadata(&media);
    metadata.insert("subcategory".into(), "image".into());
    metadata.insert("token".into(), target.token.clone().into());
    metadata.insert(
        "post_url".into(),
        format!("https://imgpv.com/{}/{}", target.token, target.page).into(),
    );
    if let Some((_, info)) = html.split_once("class=\"upinfo\">") {
        if let Some((_, content)) = info.split_once("<b>") {
            if let Some((raw_date, after_by)) = content.split_once("by") {
                let date = raw_date.trim();
                if (10..=32).contains(&date.len())
                    && date.bytes().all(|b| {
                        b.is_ascii_digit() || matches!(b, b'-' | b':' | b' ' | b'T' | b'Z')
                    })
                {
                    metadata.insert("upload_date_text".into(), date.into());
                }
                if let Some((_, after_open)) = after_by.split_once('>') {
                    if let Some((username, _)) = after_open.split_once('<') {
                        let username = username.trim();
                        if !username.is_empty()
                            && username.len() <= 80
                            && !username.chars().any(char::is_control)
                        {
                            metadata.insert("user".into(), username.into());
                        }
                    }
                }
            }
        }
    }
    Ok((media, metadata))
}

async fn at_base(
    client: &Client,
    target: &Target,
    base: &Url,
) -> Result<(MediaItem, Metadata), Box<dyn Error + Send + Sync>> {
    let endpoint = base.join(&format!("{}/{}", target.token, target.page))?;
    let mut response = client.get(endpoint).send().await?.error_for_status()?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_HTML_BYTES as u64)
    {
        return Err("Imgpv HTML source exceeds response limit".into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_HTML_BYTES.saturating_sub(body.len()) {
            return Err("Imgpv HTML source exceeds response limit".into());
        }
        body.extend_from_slice(&chunk);
    }
    parse_page(std::str::from_utf8(&body)?, target)
}

pub async fn resolve(
    client: &Client,
    target: Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let (item, _) = at_base(client, &target, &Url::parse("https://imgpv.com/")?).await?;
    Ok(vec![item])
}

pub async fn resolve_messages(
    client: &Client,
    target: Target,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let (item, metadata) = at_base(client, &target, &Url::parse("https://imgpv.com/")?).await?;
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
    fn pinned_urls_and_unsafe_source_shapes() {
        assert_eq!(
            match_url("https://www.imgpv.com/a1b2c3d4e5f6/NAME.EXT.html"),
            Some(Target {
                token: "a1b2c3d4e5f6".into(),
                page: "NAME.EXT.html".into(),
            })
        );
        for bad in [
            "http://imgpv.com/a1b2c3d4e5f6/NAME.EXT.html",
            "https://evilimgpv.com/a1b2c3d4e5f6/NAME.EXT.html",
            "https://imgpv.com/a1b2c3d4e5f6/name.jpg.html?token=secret",
            "https://imgpv.com/a1b2c3d4e5f6/name.jpg.html#fragment",
            "https://imgpv.com/a1b2c3d4e5f6/../name.jpg.html",
            "https://imgpv.com/a1b2c3d4e5f6/file%2Fname.jpg.html",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn original_and_optional_source_upload_metadata() {
        let target = match_url("https://imgpv.com/a1b2c3d4e5f6/NAME.EXT.html").unwrap();
        let html = r#"<img id="img-preview" src="https://cdn.imgpv.com/media/image.jpg" alt="A &amp; B"><div class="upinfo"><b>2026-10-24 09:15:00 by <a>alice</a></div>"#;
        let (media, data) = parse_page(html, &target).unwrap();
        assert_eq!(media.url, "https://cdn.imgpv.com/media/image.jpg");
        assert_eq!(media.filename, "A & B.jpg");
        assert_eq!(data["upload_date_text"], "2026-10-24 09:15:00");
        assert_eq!(data["user"], "alice");
        assert_eq!(data["token"], "a1b2c3d4e5f6");
        assert!(data.get("source_post_id").is_none());
    }

    #[test]
    fn reject_unsafe_originals_and_untrusted_scalar_fields() {
        let target = match_url("https://imgpv.com/a1b2c3d4e5f6/NAME.EXT.html").unwrap();
        for page in [
            r#"<img id="img-preview" src="https://outside.invalid/test.jpg">"#,
            r#"<img id="img-preview" src="http://imgpv.com/test.jpg">"#,
            r#"<img id="img-preview" src="https://imgpv.com/test.exe">"#,
            r#"<img id="img-preview" src="https://imgpv.com/test.jpg?secret=1">"#,
        ] {
            assert!(parse_page(page, &target).is_err(), "{page}");
        }
        let (media, data) = parse_page(
            r#"<img id="img-preview" src="https://imgpv.com/file.png" alt="../bad"><div class="upinfo"><b>unsafe<script> by <a>xx</a></div>"#,
            &target,
        )
        .unwrap();
        assert_eq!(media.filename, "file.png");
        assert!(data.get("upload_date_text").is_none());
    }

    #[tokio::test]
    async fn loopback_source_html_fetch_is_bounded_and_single_page() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 4096];
            let size = stream.read(&mut request).await.unwrap();
            assert!(String::from_utf8_lossy(&request[..size])
                .starts_with("GET /a1b2c3d4e5f6/NAME.EXT.html HTTP/1.1"));
            let html = r#"<img id="img-preview" src="https://imgpv.com/media/original.webp" alt="Public">"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
                html.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        let target = match_url("https://imgpv.com/a1b2c3d4e5f6/NAME.EXT.html").unwrap();
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let (file, meta) = at_base(&Client::new(), &target, &base).await.unwrap();
        assert_eq!(file.filename, "Public.webp");
        assert_eq!(meta["token"], "a1b2c3d4e5f6");
        server.await.unwrap();
    }
}
