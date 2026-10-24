//! Guarded Acidimg, Vipr and Imgspice single-image previews from the pinned
//! imagehosts.py reference. Only their public unauthenticated file shapes
//! are implemented; this is not complete upstream per-host parity.
use crate::{
    message::{media_metadata, ExtractorMessage, Metadata},
    safe_filename, MediaItem,
};
use reqwest::Client;
use std::error::Error;
use url::Url;

const MAX_HTML_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Host {
    Acidimg,
    Vipr,
    Imgspice,
}

impl Host {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Acidimg => "acidimg",
            Self::Vipr => "vipr",
            Self::Imgspice => "imgspice",
        }
    }

    fn origin(self) -> &'static str {
        match self {
            Self::Acidimg => "https://acidimg.cc/",
            Self::Vipr => "https://vipr.im/",
            Self::Imgspice => "https://imgspice.com/",
        }
    }

    fn trusted_media_host(self, host: &str) -> bool {
        let domain = match self {
            Self::Acidimg => "acidimg.cc",
            Self::Vipr => "vipr.im",
            Self::Imgspice => "imgspice.com",
        };
        host == domain || host.ends_with(&format!(".{domain}"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub host: Host,
    pub token: String,
    path: String,
}

fn safe_token(token: &str) -> bool {
    (1..=80).contains(&token.len())
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

pub fn match_url(input: &str) -> Option<Target> {
    // The URL parser normalizes dot segments, so reject them in the original
    // input before deciding whether a single-file path is allowed.
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
    let host = match url.host_str()? {
        "acidimg.cc" | "www.acidimg.cc" => Host::Acidimg,
        "vipr.im" | "www.vipr.im" => Host::Vipr,
        "imgspice.com" | "www.imgspice.com" => Host::Imgspice,
        _ => return None,
    };
    let parts: Vec<_> = url.path_segments()?.collect();
    let token = match (host, parts.as_slice()) {
        (Host::Acidimg, [file]) => file.strip_prefix("img-")?.strip_suffix(".html")?,
        (Host::Vipr, [file]) => file.strip_suffix(".html").unwrap_or(file),
        (Host::Imgspice, [token]) | (Host::Imgspice, [token, _]) => token,
        _ => return None,
    };
    if !safe_token(token)
        || (host == Host::Acidimg && !token.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()))
        || (host == Host::Imgspice
            && parts
                .iter()
                .any(|part| part.is_empty() || part.contains('%') || part.len() > 240))
    {
        return None;
    }
    Some(Target {
        host,
        token: token.to_owned(),
        path: parts.join("/"),
    })
}

fn attribute_after<'a>(body: &'a str, marker: &str, attribute: &str, quote: char) -> Option<&'a str> {
    let rest = body.split_once(marker)?.1;
    let prefix = format!("{attribute}={quote}");
    let before = rest.split_once('>')?.0;
    before
        .split_once(&prefix)?
        .1
        .split_once(quote)
        .map(|(value, _)| value)
}

fn media_link(host: Host, raw: &str) -> Option<(Url, String)> {
    let url = Url::parse(raw).ok()?;
    if url.scheme() != "https"
        || !host.trusted_media_host(url.host_str()?)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let filename = url.path_segments()?.next_back()?.to_owned();
    if filename.is_empty() || filename.len() > 240 || filename.contains('%') {
        return None;
    }
    let extension = filename.rsplit_once('.')?.1.to_ascii_lowercase();
    if !matches!(extension.as_str(), "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif") {
        return None;
    }
    let safe = safe_filename(&filename);
    Some((url, safe))
}

fn trusted_name(name: Option<&str>, extension: &str) -> Option<String> {
    let name = name?;
    if name.is_empty()
        || name.len() > 225
        || name.contains('/')
        || name.contains('\\')
        || name.chars().any(char::is_control)
    {
        return None;
    }
    let decoded = name.replace("&amp;", "&").replace("&quot;", "\"").replace("&#39;", "'");
    if decoded.chars().any(char::is_control) {
        return None;
    }
    let candidate = if decoded.rsplit_once('.').is_some_and(|(_, ext)| {
        matches!(ext.to_ascii_lowercase().as_str(), "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif")
    }) {
        decoded
    } else {
        format!("{decoded}.{extension}")
    };
    Some(safe_filename(&candidate))
}

fn parse_media(body: &str, target: &Target) -> Result<(MediaItem, Metadata), Box<dyn Error + Send + Sync>> {
    let (raw, alt) = match target.host {
        Host::Acidimg => {
            let (start, quote) = if body.contains("<img class='centred'") {
                ("<img class='centred'", '\'')
            } else {
                ("<img class=\"centred\"", '"')
            };
            let url = attribute_after(body, start, "src", quote)
                .ok_or("Acidimg has no centered original image")?;
            let name = attribute_after(body, start, "alt", quote);
            (url, name)
        }
        Host::Vipr => {
            let url = attribute_after(body, "<img ", "src", '"')
                .ok_or("Vipr public image is absent")?;
            let name = attribute_after(body, "<img ", "alt", '"');
            (url, name)
        }
        Host::Imgspice => {
            let url = attribute_after(body, "id=\"imgpreview\"", "src", '"')
                .ok_or("Imgspice public preview image is absent")?;
            let name = attribute_after(body, "id=\"imgpreview\"", "alt", '"');
            (url, name)
        }
    };
    let (url, fallback) =
        media_link(target.host, raw).ok_or("Imagehost media origin or format is unsupported")?;
    let ext = fallback.rsplit_once('.').map(|(_, e)| e).unwrap_or("jpg");
    let filename = trusted_name(alt, ext).unwrap_or(fallback);
    let item = MediaItem {
        url: url.to_string(),
        filename,
        extractor: target.host.name(),
        post_id: None,
    };
    let mut meta = media_metadata(&item);
    meta.insert("subcategory".into(), "image".into());
    meta.insert("token".into(), target.token.clone().into());
    meta.insert(
        "post_url".into(),
        format!("{}{}", target.host.origin(), target.path).into(),
    );
    Ok((item, meta))
}

async fn at_base(
    client: &Client,
    target: &Target,
    base: &Url,
) -> Result<(MediaItem, Metadata), Box<dyn Error + Send + Sync>> {
    let endpoint = base.join(&target.path)?;
    let mut request = client.request(
        if target.host == Host::Acidimg {
            reqwest::Method::POST
        } else {
            reqwest::Method::GET
        },
        endpoint,
    );
    if target.host == Host::Acidimg {
        request = request.form(&[("imgContinue", "Continue+to+image+...+")]);
    }
    let mut response = request.send().await?.error_for_status()?;
    if response
        .content_length()
        .is_some_and(|bytes| bytes > MAX_HTML_BYTES as u64)
    {
        return Err("Imagehost public HTML exceeds response limit".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_HTML_BYTES.saturating_sub(bytes.len()) {
            return Err("Imagehost public HTML exceeds response limit".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    parse_media(std::str::from_utf8(&bytes)?, target)
}

pub async fn resolve(
    client: &Client,
    target: Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse(target.host.origin())?;
    let (item, _) = at_base(client, &target, &base).await?;
    Ok(vec![item])
}

pub async fn resolve_messages(
    client: &Client,
    target: Target,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse(target.host.origin())?;
    let (item, metadata) = at_base(client, &target, &base).await?;
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
    fn pinned_acidimg_vipr_and_imgspice_routes_are_narrow() {
        for (url, host, token) in [
            ("https://acidimg.cc/img-5acb6b9de4640.html", Host::Acidimg, "5acb6b9de4640"),
            ("https://vipr.im/kcd5jcuhgs3v.html", Host::Vipr, "kcd5jcuhgs3v"),
            ("https://vipr.im/yyqomiutt768", Host::Vipr, "yyqomiutt768"),
            ("https://imgspice.com/q1taxkhxprrn/58410038_cal022jsp_308191001.jpg.html", Host::Imgspice, "q1taxkhxprrn"),
        ] {
            let target = match_url(url).expect(url);
            assert_eq!(target.host, host);
            assert_eq!(target.token, token);
        }
        for bad in [
            "http://acidimg.cc/img-abc123.html",
            "https://evilacidimg.cc/img-abc123.html",
            "https://acidimg.cc/img-abc123.html?token=x",
            "https://acidimg.cc/img-abc123.html#part",
            "https://acidimg.cc/img-ABC123.html",
            "https://vipr.im/a/b",
            "https://vipr.im/a%2Fb",
            "https://imgspice.com/a/../b",
            "https://imgspice.com/a/%2e%2e/b",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn host_specific_public_html_parses_safe_names_and_media() {
        let examples = [
            (
                "https://acidimg.cc/img-abc123.html",
                "<img class='centred' src='https://img.acidimg.cc/files/abc123.jpg' alt='photo'>",
                "photo.jpg",
            ),
            (
                "https://vipr.im/kcd5jcuhgs3v.html",
                "<img src=\"https://i7.vipr.im/i/00021/kcd5jcuhgs3v.jpg/sommer01035.jpg\" alt=\"sommer01035\">",
                "sommer01035.jpg",
            ),
            (
                "https://imgspice.com/q1taxkhxprrn/picture.jpg.html",
                "<div id=\"imgpreview\" src=\"https://img30.imgspice.com/i/03792/q1taxkhxprrn.jpg\" alt=\"picture.jpg\"></div>",
                "picture.jpg",
            ),
        ];
        for (url, html, name) in examples {
            let target = match_url(url).unwrap();
            let (media, meta) = parse_media(html, &target).unwrap();
            assert_eq!(media.filename, name);
            assert_eq!(media.extractor, target.host.name());
            assert_eq!(meta["token"], target.token);
            assert_eq!(meta["subcategory"], "image");
            assert!(meta.get("source_post_id").is_none());
        }
    }

    #[test]
    fn hostile_media_hosts_and_filename_values_do_not_escape_origins() {
        assert!(media_link(Host::Vipr, "https://evilvipr.im/a.jpg").is_none());
        assert!(media_link(Host::Vipr, "http://i7.vipr.im/a.jpg").is_none());
        assert!(media_link(Host::Imgspice, "https://imgspice.com/a.jpg?token=x").is_none());
        assert!(media_link(Host::Acidimg, "https://attacker.example/a.jpg").is_none());
        assert!(trusted_name(Some("../escape"), "jpg").is_none());
        assert!(trusted_name(Some("nested/escape"), "jpg").is_none());
        assert!(trusted_name(Some("bad\u{0001}name"), "jpg").is_none());
    }

    #[tokio::test]
    async fn acidimg_uses_pinned_post_continuation_and_one_bounded_page() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 4096];
            let n = socket.read(&mut request).await.unwrap();
            let line = String::from_utf8_lossy(&request[..n]);
            assert!(line.starts_with("POST /img-abc123.html HTTP/1.1"), "{line}");
            assert!(line.to_ascii_lowercase().contains("content-type: application/x-www-form-urlencoded"), "{line}");
            let body = "<img class='centred' src='https://img.acidimg.cc/files/abc123.png' alt='Original'>";
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(reply.as_bytes()).await.unwrap();
        });
        let target = match_url("https://acidimg.cc/img-abc123.html").unwrap();
        let base = Url::parse(&format!("http://{address}/")).unwrap();
        let (item, meta) = at_base(&Client::new(), &target, &base).await.unwrap();
        assert_eq!(item.filename, "Original.png");
        assert_eq!(meta["token"], "abc123");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn declared_oversized_public_body_is_rejected_before_parsing() {
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
        let target = match_url("https://vipr.im/kcd5jcuhgs3v.html").unwrap();
        let base = Url::parse(&format!("http://{address}/")).unwrap();
        let err = at_base(&Client::new(), &target, &base).await.unwrap_err();
        assert!(err.to_string().contains("response limit"));
        server.await.unwrap();
    }
}
