//! Pinned public bit.ly and t.co URL-shortener queue subset.
use crate::message::{queued_url, ExtractorMessage, Metadata};
use reqwest::{header::LOCATION, redirect::Policy, Client, StatusCode};
use std::{error::Error, net::IpAddr};
use url::Url;

const MAX_PAGE_BYTES: usize = 32 * 1024;
const MAX_CODE_LEN: usize = 160;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShortLink {
    service: &'static str,
    code: String,
}

pub fn match_url(raw: &str) -> Option<ShortLink> {
    let lower = raw.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "/../", "/./"]
        .iter()
        .any(|s| lower.contains(s))
    {
        return None;
    }
    let link = Url::parse(raw).ok()?;
    if !matches!(link.scheme(), "http" | "https")
        || !link.username().is_empty()
        || link.password().is_some()
        || link.port().is_some()
        || link.query().is_some()
        || link.fragment().is_some()
    {
        return None;
    }
    let service = match link.host_str()? {
        "bit.ly" => "bitly",
        "t.co" => "tco",
        _ => return None,
    };
    let mut segments = link.path_segments()?;
    let code = segments.next()?;
    if segments.next().is_some()
        || !(1..=MAX_CODE_LEN).contains(&code.len())
        || !code
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return None;
    }
    Some(ShortLink {
        service,
        code: code.to_owned(),
    })
}

fn public_destination(input: &str, source: &Url) -> Option<Url> {
    if input.len() > 4096 || input.chars().any(char::is_control) || input.contains('\\') {
        return None;
    }
    let url = source.join(input).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    let domain = url.host_str()?.trim_end_matches('.').to_ascii_lowercase();
    if domain.parse::<IpAddr>().is_ok()
        || matches!(domain.as_str(), "localhost" | "localhost.localdomain")
        || [
            ".localhost",
            ".local",
            ".internal",
            ".onion",
            ".test",
            ".invalid",
            ".example",
        ]
        .iter()
        .any(|tail| domain.ends_with(tail))
        || url.port().is_some()
        || url == *source
    {
        return None;
    }
    Some(url)
}

fn header_target(response: &reqwest::Response, original: &Url) -> Option<Url> {
    let value = response.headers().get(LOCATION)?.to_str().ok()?;
    public_destination(value, original)
}

fn refresh_target(html: &str, original: &Url) -> Option<Url> {
    let normalized = html.to_ascii_lowercase();
    let start = normalized.find("http-equiv=\"refresh\"")?;
    let attrs = html.get(start..)?.split_once('>')?.0;
    let marker = attrs.find("content=\"")?;
    let rest = &attrs[marker + "content=\"".len()..];
    let payload = rest.split_once('"')?.0;
    let (_seconds, target) = payload.split_once(';')?;
    let (key, url) = target.trim().split_once('=')?;
    if !key.trim().eq_ignore_ascii_case("url") {
        return None;
    }
    public_destination(url.trim().trim_matches('\''), original)
}

async fn read_small_html(
    mut response: reqwest::Response,
) -> Result<String, Box<dyn Error + Send + Sync>> {
    if response
        .content_length()
        .is_some_and(|n| n > MAX_PAGE_BYTES as u64)
    {
        return Err("Shortener refresh page exceeds byte cap".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_PAGE_BYTES.saturating_sub(bytes.len()) {
            return Err("Shortener refresh page exceeds byte cap".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8(bytes)?)
}

/// This client deliberately never follows HTTP redirects, so extracting a
/// short URL cannot itself contact a private service or arbitrary target.
async fn resolve_endpoint(
    endpoint: Url,
    service: &str,
) -> Result<Url, Box<dyn Error + Send + Sync>> {
    let client = Client::builder().redirect(Policy::none()).build()?;
    let head = client.head(endpoint.clone()).send().await?;
    if head.url().origin() != endpoint.origin() {
        return Err("Shortener initial request escaped its origin".into());
    }
    if let Some(dest) = header_target(&head, &endpoint) {
        return Ok(dest);
    }
    // Some public shorteners reject HEAD; t.co can also return an HTML
    // meta refresh instead of a redirect when a browser agent is present.
    if head.status() == StatusCode::METHOD_NOT_ALLOWED
        || head.status().is_success()
        || (head.status().is_redirection() && head.headers().get(LOCATION).is_none())
    {
        let response = client.get(endpoint.clone()).send().await?;
        if response.url().origin() != endpoint.origin() {
            return Err("Shortener GET escaped its origin".into());
        }
        if let Some(dest) = header_target(&response, &endpoint) {
            return Ok(dest);
        }
        if service == "tco" && response.status().is_success() {
            let page = read_small_html(response).await?;
            if let Some(dest) = refresh_target(&page, &endpoint) {
                return Ok(dest);
            }
        }
    }
    Err("Shortener source returned no safe public redirect target".into())
}

pub async fn resolve_messages(
    target: ShortLink,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let root = if target.service == "tco" {
        "https://t.co/"
    } else {
        "https://bit.ly/"
    };
    let endpoint = Url::parse(root)?.join(&target.code)?;
    let destination = resolve_endpoint(endpoint.clone(), target.service).await?;
    let mut metadata = Metadata::new();
    metadata.insert("category".into(), "urlshortener".into());
    metadata.insert("subcategory".into(), "link".into());
    metadata.insert("service".into(), target.service.into());
    metadata.insert("short_code".into(), target.code.into());
    metadata.insert("source_url".into(), endpoint.to_string().into());
    Ok(vec![queued_url(destination.to_string(), metadata)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    #[test]
    fn pinned_hosts_accept_only_public_short_codes() {
        assert_eq!(match_url("https://bit.ly/abc_12").unwrap().service, "bitly");
        assert_eq!(match_url("http://t.co/abc123").unwrap().service, "tco");
        for unsafe_url in [
            "https://evilbit.ly/abc123",
            "https://bit.ly.evil.test/abc123",
            "https://t.co/abc/def",
            "https://bit.ly/abc?password=1",
            "https://user:secret@bit.ly/code",
            "https://bit.ly:8443/code",
            "https://bit.ly/%2e%2e",
            "https://bit.ly/",
        ] {
            assert!(match_url(unsafe_url).is_none(), "{unsafe_url}");
        }
    }

    #[test]
    fn destination_rejects_private_origins_and_non_http_schemes() {
        let base = Url::parse("https://bit.ly/example").unwrap();
        for target in [
            "http://localhost/admin",
            "http://127.0.0.1/private",
            "https://192.168.1.1/",
            "http://[::1]/",
            "https://intranet.local/path",
            "https://service.internal/path",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "https://user:secret@example.org/file",
            "https://example.org:8443/path",
            "https://bit.ly/example",
        ] {
            assert!(public_destination(target, &base).is_none(), "{target}");
        }
        assert_eq!(
            public_destination("https://www.wikipedia.org/wiki/History", &base)
                .unwrap()
                .host_str(),
            Some("www.wikipedia.org")
        );
    }

    #[test]
    fn parses_bounded_tco_html_refresh_without_script_execution() {
        let base = Url::parse("https://t.co/abc").unwrap();
        let page = r#"<html><meta http-equiv="refresh" content="0; URL=https://www.wikipedia.org/wiki/History"></html>"#;
        assert_eq!(
            refresh_target(page, &base).unwrap().as_str(),
            "https://www.wikipedia.org/wiki/History"
        );
        assert!(refresh_target(
            r#"<meta http-equiv="refresh" content="0;url=http://localhost:1234/admin">"#,
            &base
        )
        .is_none());
    }

    #[tokio::test]
    async fn shortened_head_redirect_returns_target_without_following_it() {
        let server = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let worker = tokio::spawn(async move {
            let (mut socket, _) = server.accept().await.unwrap();
            let mut request = [0u8; 4096];
            let count = socket.read(&mut request).await.unwrap();
            assert!(String::from_utf8_lossy(&request[..count]).starts_with("HEAD /abc HTTP/1.1"));
            socket.write_all(b"HTTP/1.1 302 Found\r\nLocation: https://www.wikipedia.org/wiki/History\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
        });
        let location = Url::parse(&format!("http://{address}/abc")).unwrap();
        let destination = resolve_endpoint(location, "bitly").await.unwrap();
        assert_eq!(
            destination.as_str(),
            "https://www.wikipedia.org/wiki/History"
        );
        worker.await.unwrap();
    }

    #[tokio::test]
    async fn tco_head_then_get_meta_refresh_is_a_bounded_two_request_flow() {
        let server = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = server.local_addr().unwrap();
        let worker = tokio::spawn(async move {
            for (expected, response) in [
                (
                    "HEAD /abc HTTP/1.1".to_owned(),
                    "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_owned(),
                ),
                ("GET /abc HTTP/1.1".to_owned(), {
                    let page = r#"<meta http-equiv="refresh" content="0; url=https://www.wikipedia.org/">"#;
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}",
                        page.len()
                    )
                }),
            ] {
                let (mut socket, _) = server.accept().await.unwrap();
                let mut buffer = [0u8; 4096];
                let count = socket.read(&mut buffer).await.unwrap();
                assert!(String::from_utf8_lossy(&buffer[..count]).starts_with(&expected));
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let source = Url::parse(&format!("http://{addr}/abc")).unwrap();
        let destination = resolve_endpoint(source, "tco").await.unwrap();
        assert_eq!(destination.as_str(), "https://www.wikipedia.org/");
        worker.await.unwrap();
    }
}
