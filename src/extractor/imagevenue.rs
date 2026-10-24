//! Read-only ImageVenue single-image subset of pinned imagehosts.py.
//! All URL variants, image response checks, cookies and live parity remain open.
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
    host: String,
    request_path: String,
}

fn valid_id(id: &str) -> bool {
    (8..=10).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
}

fn digits(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn valid_legacy_file(value: &str) -> bool {
    (5..=220).contains(&value.len())
        && !value.contains("..")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        && value.rsplit_once('.').is_some_and(|(_, ext)| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "jpg" | "jpeg" | "png" | "gif" | "webp"
            )
        })
}

fn valid_host(host: &str) -> bool {
    matches!(host, "www.imagevenue.com" | "imagevenue.com")
        || host
            .strip_prefix("img")
            .and_then(|suffix| suffix.strip_suffix(".imagevenue.com"))
            .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
}

pub fn match_url(input: &str) -> Option<Target> {
    let lower = input.to_ascii_lowercase();
    if ["%2f", "%5c", "%2e", "/../", "/./"]
        .iter()
        .any(|term| lower.contains(term))
    {
        return None;
    }
    let url = Url::parse(input).ok()?;
    let host = url.host_str()?;
    if url.scheme() != "https"
        || !valid_host(host)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let parts: Vec<_> = url.path_segments()?.collect();
    let pairs: Vec<_> = url.query_pairs().collect();
    let (id, request_path) = match parts.as_slice() {
        [id] if valid_id(id) && pairs.is_empty() => ((*id).to_owned(), (*id).to_owned()),
        ["view", "o"] if pairs.len() == 3 => {
            let image = pairs.iter().find(|(key, _)| key == "i")?.1.as_ref();
            let host = pairs.iter().find(|(key, _)| key == "h")?.1.as_ref();
            let location = pairs.iter().find(|(key, _)| key == "l")?.1.as_ref();
            if !valid_legacy_file(image)
                || !host.strip_prefix("img").is_some_and(digits)
                || !location.strip_prefix("loc").is_some_and(digits)
                || pairs.iter().filter(|(key, _)| key == "i").count() != 1
                || pairs.iter().filter(|(key, _)| key == "h").count() != 1
                || pairs.iter().filter(|(key, _)| key == "l").count() != 1
            {
                return None;
            }
            (image.to_owned(), format!("view/o?{}", url.query()?))
        }
        ["img.php"] if pairs.len() == 1 && pairs[0].0 == "image" => {
            let image = pairs[0].1.as_ref();
            if !valid_legacy_file(image) {
                return None;
            }
            (image.to_owned(), format!("img.php?{}", url.query()?))
        }
        _ => return None,
    };
    Some(Target {
        id,
        host: host.into(),
        request_path,
    })
}

fn safe_media(raw: &str) -> Option<(Url, String)> {
    let url = Url::parse(raw).ok()?;
    let host = url.host_str()?;
    if url.scheme() != "https"
        || !(host == "imagevenue.com" || host.ends_with(".imagevenue.com"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let name = url.path_segments()?.next_back()?;
    if name.is_empty() || name.len() > 240 || name.contains('%') {
        return None;
    }
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    if !matches!(
        ext.as_str(),
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif"
    ) {
        return None;
    }
    let filename = safe_filename(name);
    Some((url, filename))
}

fn safe_alt(raw: &str, extension: &str) -> Option<String> {
    let decoded = raw
        .replace("&quot;", "\"")
        .replace("&gt;", ">")
        .replace("&lt;", "<")
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
    let name = if decoded.rsplit_once('.').is_some_and(|(_, ext)| {
        matches!(
            ext.to_ascii_lowercase().as_str(),
            "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif"
        )
    }) {
        decoded
    } else {
        format!("{decoded}.{extension}")
    };
    Some(safe_filename(&name))
}

fn parse_page(
    html: &str,
    target: &Target,
) -> Result<(MediaItem, Metadata), Box<dyn Error + Send + Sync>> {
    let card = html
        .split_once("class=\"card-body")
        .map(|(_, after)| after)
        .ok_or("ImageVenue public image card is missing")?;
    let mut found = None;
    for part in card.split("<img").skip(1).take(8) {
        let Some((attrs, _)) = part.split_once('>') else {
            continue;
        };
        let Some(raw) = attrs
            .split_once("src=\"")
            .and_then(|(_, s)| s.split_once('"'))
            .map(|(raw, _)| raw)
        else {
            continue;
        };
        if raw.ends_with("/loader.svg") {
            continue;
        }
        if let Some((url, fallback)) = safe_media(raw) {
            let ext = fallback.rsplit_once('.').map(|(_, e)| e).unwrap_or("jpg");
            let filename = attrs
                .split_once("alt=\"")
                .and_then(|(_, s)| s.split_once('"'))
                .and_then(|(alt, _)| safe_alt(alt, ext))
                .unwrap_or(fallback);
            found = Some((url, filename));
            break;
        }
    }
    let (url, filename) = found.ok_or("ImageVenue page lacks a validated original image")?;
    let item = MediaItem {
        url: url.to_string(),
        filename,
        extractor: "imagevenue",
        post_id: None,
    };
    let mut metadata = media_metadata(&item);
    metadata.insert("subcategory".into(), "image".into());
    metadata.insert("token".into(), target.id.clone().into());
    metadata.insert("source_host".into(), target.host.clone().into());
    Ok((item, metadata))
}

async fn resolve_at_base(
    client: &Client,
    target: &Target,
    base: &Url,
) -> Result<(MediaItem, Metadata), Box<dyn Error + Send + Sync>> {
    let mut response = client
        .get(base.join(&target.request_path)?)
        .send()
        .await?
        .error_for_status()?;
    if response
        .content_length()
        .is_some_and(|n| n > MAX_HTML_BYTES as u64)
    {
        return Err("ImageVenue HTML exceeds preview byte limit".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_HTML_BYTES.saturating_sub(bytes.len()) {
            return Err("ImageVenue HTML exceeds preview byte limit".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    parse_page(std::str::from_utf8(&bytes)?, target)
}

pub async fn resolve(
    client: &Client,
    target: Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse(&format!("https://{}/", target.host))?;
    let (file, _) = resolve_at_base(client, &target, &base).await?;
    Ok(vec![file])
}

pub async fn resolve_messages(
    client: &Client,
    target: Target,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse(&format!("https://{}/", target.host))?;
    let (file, metadata) = resolve_at_base(client, &target, &base).await?;
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
    fn pinned_simple_source_and_img_subdomain_are_conservative() {
        assert_eq!(
            match_url("https://www.imagevenue.com/ME13LS07"),
            Some(Target {
                id: "ME13LS07".into(),
                host: "www.imagevenue.com".into(),
                request_path: "ME13LS07".into(),
            })
        );
        assert!(match_url("https://img28116.imagevenue.com/ME13LS07").is_some());
        for bad in [
            "http://www.imagevenue.com/ME13LS07",
            "https://evilimagevenue.com/ME13LS07",
            "https://imgxxx.imagevenue.com/ME13LS07",
            "https://www.imagevenue.com/ME13LS07?token=private",
            "https://www.imagevenue.com/ME13LS07#anchor",
            "https://www.imagevenue.com/ME13LS07/extra",
            "https://www.imagevenue.com/a%2fb",
            "https://www.imagevenue.com/short",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn pinned_legacy_view_and_img_php_routes_allow_only_safe_query_fields() {
        let view =
            match_url("https://www.imagevenue.com/view/o?i=92518_13732377lo.jpg&h=img150&l=loc486")
                .unwrap();
        assert_eq!(view.id, "92518_13732377lo.jpg");
        assert!(view.request_path.starts_with("view/o?i="));
        let image =
            match_url("https://img28116.imagevenue.com/img.php?image=th_52709_test_122_64lo.jpg")
                .unwrap();
        assert_eq!(image.id, "th_52709_test_122_64lo.jpg");
        for bad in [
            "https://www.imagevenue.com/view/o?i=foo.jpg&h=img150",
            "https://www.imagevenue.com/view/o?i=foo.jpg&h=img150&l=loc486&token=secret",
            "https://www.imagevenue.com/view/o?i=foo.jpg&i=evil.jpg&l=loc486",
            "https://www.imagevenue.com/view/o?i=../escape.jpg&h=img150&l=loc486",
            "https://img28116.imagevenue.com/img.php?image=a.jpg&token=secret",
            "https://img28116.imagevenue.com/img.php?image=javascript:alert(1)",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn pinned_card_skips_loader_and_preserves_safe_unicode_alt() {
        let target = match_url("https://www.imagevenue.com/ME13LS07").unwrap();
        let html = r#"<div class="card-body image"><img src="/loader.svg"><img src="https://cdn-images.imagevenue.com/10/ac/05/ME13LS07_o.png" alt="test-テスト-&quot;&amp;&gt;"></div>"#;
        let (media, meta) = parse_page(html, &target).unwrap();
        assert_eq!(
            media.url,
            "https://cdn-images.imagevenue.com/10/ac/05/ME13LS07_o.png"
        );
        assert!(media.filename.ends_with(".png"));
        assert!(media.filename.contains("テスト"));
        assert_eq!(meta["token"], "ME13LS07");
        assert_eq!(meta["source_host"], "www.imagevenue.com");
        assert!(media.post_id.is_none());
    }

    #[test]
    fn rejects_untrusted_originals_and_missing_card() {
        let target = match_url("https://www.imagevenue.com/ME13LS07").unwrap();
        assert!(parse_page("<html>No card</html>", &target).is_err());
        assert!(parse_page(
            r#"<div class="card-body"><img src="https://evil.example/image.jpg"></div>"#,
            &target,
        )
        .is_err());
        for bad in [
            "http://cdn-images.imagevenue.com/a.jpg",
            "https://evilimagevenue.com/a.jpg",
            "https://user:secret@cdn-images.imagevenue.com/a.jpg",
            "https://cdn-images.imagevenue.com/a.jpg?token=1",
            "https://cdn-images.imagevenue.com/a.svg",
        ] {
            assert!(safe_media(bad).is_none(), "{bad}");
        }
    }

    #[tokio::test]
    async fn bounded_public_html_uses_one_page_request() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 4096];
            let n = socket.read(&mut request).await.unwrap();
            assert!(String::from_utf8_lossy(&request[..n]).starts_with("GET /ME13LS07 HTTP/1.1"));
            let html = r#"<div class="card-body"><img src="https://cdn-images.imagevenue.com/10/ac/05/ME13LS07_o.png" alt="Public"></div>"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
                html.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let target = match_url("https://www.imagevenue.com/ME13LS07").unwrap();
        let (file, metadata) = resolve_at_base(&Client::new(), &target, &base)
            .await
            .unwrap();
        assert_eq!(file.filename, "Public.png");
        assert_eq!(metadata["token"], "ME13LS07");
        server.await.unwrap();
    }
}
