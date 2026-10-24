//! Conservative Silverpic public original-image subset of pinned imagehosts.py.
//! No live site, filename template or complete archive compatibility is implied.
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

fn safe_page(name: &str) -> bool {
    (6..=200).contains(&name.len())
        && name.ends_with(".html")
        && !name.contains("..")
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
}

pub fn match_url(input: &str) -> Option<Target> {
    let lower = input.to_ascii_lowercase();
    if ["/../", "/./", "%2e", "%2f", "%5c"]
        .iter()
        .any(|t| lower.contains(t))
    {
        return None;
    }
    let url = Url::parse(input).ok()?;
    if url.scheme() != "https"
        || !matches!(
            url.host_str(),
            Some("silverpic.net" | "www.silverpic.net" | "silverpic.com" | "www.silverpic.com")
        )
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let parts: Vec<_> = url.path_segments()?.collect();
    let [token, name] = parts.as_slice() else {
        return None;
    };
    if !(10..=80).contains(&token.len())
        || !token
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        || !safe_page(name)
    {
        return None;
    }
    Some(Target {
        token: (*token).into(),
        page: (*name).into(),
    })
}

fn attribute<'a>(tag: &'a str, key: &str) -> Option<&'a str> {
    let marker = format!(" {key}=\"");
    tag.split_once(&marker)?
        .1
        .split_once('"')
        .map(|(value, _)| value)
}

fn decode_safe_name(alt: &str, ext: &str) -> Option<String> {
    if alt.len() > 220 {
        return None;
    }
    let title = alt
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    if title.is_empty()
        || title.contains('/')
        || title.contains('\\')
        || title.chars().any(char::is_control)
    {
        return None;
    }
    let named = if title.rsplit_once('.').is_some_and(|(_, suffix)| {
        matches!(
            suffix.to_ascii_lowercase().as_str(),
            "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif"
        )
    }) {
        title
    } else {
        format!("{title}.{ext}")
    };
    Some(safe_filename(&named))
}

fn parse_page(
    html: &str,
    target: &Target,
) -> Result<(MediaItem, Metadata), Box<dyn Error + Send + Sync>> {
    // Restrict every selected attribute to the single original image element.
    let (_, after_start) = html
        .split_once("<img src=\"/img/")
        .ok_or("Silverpic original image element is absent")?;
    let (attrs, _) = after_start
        .split_once('>')
        .ok_or("Silverpic original image element is incomplete")?;
    let (filename, suffix) = attrs
        .split_once('"')
        .ok_or("Silverpic original image path is incomplete")?;
    if filename.is_empty()
        || filename.len() > 240
        || filename.contains('%')
        || filename.contains('/')
        || filename.contains('\\')
        || filename.contains("..")
    {
        return Err("Silverpic original image filename is unsafe".into());
    }
    let extension = filename
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .ok_or("Silverpic original image extension is absent")?;
    if !matches!(
        extension.as_str(),
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif"
    ) {
        return Err("Silverpic original image extension is unsupported".into());
    }
    let root = Url::parse("https://silverpic.net/")?;
    let original = root.join(&format!("img/{filename}"))?;
    let name = attribute(suffix, "alt")
        .and_then(|alt| decode_safe_name(alt, &extension))
        .unwrap_or_else(|| safe_filename(filename));
    let media = MediaItem {
        url: original.to_string(),
        filename: name,
        extractor: "silverpic",
        post_id: None,
    };
    let mut metadata = media_metadata(&media);
    metadata.insert("subcategory".into(), "image".into());
    metadata.insert("token".into(), target.token.clone().into());
    metadata.insert(
        "post_url".into(),
        format!("https://silverpic.net/{}/{}", target.token, target.page).into(),
    );
    for axis in ["width", "height"] {
        if let Some(value) = attribute(suffix, axis)
            .and_then(|number| number.parse::<u32>().ok())
            .filter(|number| (1..=32768).contains(number))
        {
            metadata.insert(axis.into(), value.into());
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
        return Err("Silverpic public HTML response exceeds byte limit".into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_HTML_BYTES.saturating_sub(body.len()) {
            return Err("Silverpic public HTML response exceeds byte limit".into());
        }
        body.extend_from_slice(&chunk);
    }
    parse_page(std::str::from_utf8(&body)?, target)
}

pub async fn resolve(
    client: &Client,
    target: Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let (file, _) = at_base(client, &target, &Url::parse("https://silverpic.net/")?).await?;
    Ok(vec![file])
}

pub async fn resolve_messages(
    client: &Client,
    target: Target,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let (file, metadata) = at_base(client, &target, &Url::parse("https://silverpic.net/")?).await?;
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
    fn pinned_public_urls_and_rejected_auth_or_traversal() {
        assert_eq!(
            match_url("https://silverpic.net/a1b2c3d4e5f6/NAME.EXT.html"),
            Some(Target {
                token: "a1b2c3d4e5f6".into(),
                page: "NAME.EXT.html".into()
            })
        );
        assert!(match_url("https://www.silverpic.com/a1b2c3d4e5f6/name.jpg.html").is_some());
        for bad in [
            "http://silverpic.net/a1b2c3d4e5f6/name.jpg.html",
            "https://fake-silverpic.net/a1b2c3d4e5f6/name.jpg.html",
            "https://silverpic.net/a1b2c3d4e5f6/name.jpg.html?secret=1",
            "https://silverpic.net/a1b2c3d4e5f6/name.jpg.html#fragment",
            "https://silverpic.net/a1b2c3d4e5f6/../name.jpg.html",
            "https://silverpic.net/a1b2c3d4e5f6/file%2Fname.jpg.html",
            "https://silverpic.net/short/name.jpg.html",
            "https://silverpic.net/a1b2c3d4e5f6/other/third.html",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn original_media_and_numeric_dimensions_use_same_image_tag() {
        let target = match_url("https://silverpic.net/a1b2c3d4e5f6/picture.jpg.html").unwrap();
        let html = r#"<p>other</p><img src="/img/a1b2c3.jpg" alt="A &amp; B" width="720" height="480"><img width="999" height="999">"#;
        let (media, meta) = parse_page(html, &target).unwrap();
        assert_eq!(media.url, "https://silverpic.net/img/a1b2c3.jpg");
        assert_eq!(media.filename, "A & B.jpg");
        assert_eq!(meta["width"], 720);
        assert_eq!(meta["height"], 480);
        assert_eq!(meta["token"], "a1b2c3d4e5f6");
        assert!(meta.get("source_post_id").is_none());
    }

    #[test]
    fn original_media_path_and_alt_values_reject_untrusted_data() {
        let target = match_url("https://silverpic.net/a1b2c3d4e5f6/picture.jpg.html").unwrap();
        for unsafe_html in [
            r#"<img src="/img/../bad.jpg">"#,
            r#"<img src="/img/file%2Fbad.jpg">"#,
            r#"<img src="/img/file.exe">"#,
            r#"<img src="/img/folder/file.jpg">"#,
            r#"<img src="/img/bad.jpg""#,
        ] {
            assert!(parse_page(unsafe_html, &target).is_err(), "{unsafe_html}");
        }
        let (media, meta) = parse_page(
            r#"<img src="/img/file.png" alt="../unsafe" width="999999999999" height="-1">"#,
            &target,
        )
        .unwrap();
        assert_eq!(media.filename, "file.png");
        assert!(meta.get("width").is_none());
        assert!(meta.get("height").is_none());
    }

    #[tokio::test]
    async fn fetches_only_one_bounded_html_page() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = [0_u8; 4096];
            let n = stream.read(&mut bytes).await.unwrap();
            assert!(String::from_utf8_lossy(&bytes[..n])
                .starts_with("GET /a1b2c3d4e5f6/picture.jpg.html HTTP/1.1"));
            let html = r#"<img src="/img/image.webp" alt="Valid" width="1200" height="800">"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
                html.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let target = match_url("https://silverpic.net/a1b2c3d4e5f6/picture.jpg.html").unwrap();
        let (file, meta) = at_base(&Client::new(), &target, &base).await.unwrap();
        assert_eq!(file.filename, "Valid.webp");
        assert_eq!(meta["height"], 800);
        server.await.unwrap();
    }
}
