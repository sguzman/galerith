//! Independent, bounded local-file subset of pinned gallery-dl recursive.py.
//! Explicit local file lists and public HTTP(S) sources are supported.
//! Every response and child queue is bounded; no implicit media download.
use crate::message::{queued_url, ExtractorMessage, Metadata};
use reqwest::{redirect::Policy, Client};
use std::{error::Error, net::IpAddr, path::PathBuf, time::Duration};
use tokio::io::AsyncReadExt;
use url::{Host, Url};

const MAX_BYTES: usize = 2 * 1024 * 1024;
const MAX_CHILDREN: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Local(PathBuf),
    Remote(Url),
}

fn public_host(host: &str) -> bool {
    let host = host.trim_end_matches('.');
    host.contains('.')
        && host.len() <= 253
        && !host.is_empty()
        && ![
            "localhost",
            ".localhost",
            ".local",
            ".internal",
            ".test",
            ".invalid",
            ".onion",
            ".home",
            ".lan",
        ]
        .iter()
        .any(|suffix| host == *suffix || host.ends_with(suffix))
        && host.split('.').all(|part| {
            !part.is_empty()
                && part.len() <= 63
                && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

pub fn match_url(raw: &str) -> Option<Source> {
    let source = raw
        .strip_prefix("recursive:")
        .or_else(|| raw.strip_prefix("r:"))?;
    if source.is_empty() || source.len() > 4096 {
        return None;
    }
    let lower = source.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "/../", "/./"]
        .iter()
        .any(|term| lower.contains(term))
    {
        return None;
    }
    if source.starts_with("file://") {
        let url = Url::parse(source).ok()?;
        if url.query().is_some()
            || url.fragment().is_some()
            || url.host_str().is_some_and(|host| !host.is_empty())
        {
            return None;
        }
        return Some(Source::Local(url.to_file_path().ok()?));
    }
    let candidate = if source.starts_with("https://") || source.starts_with("http://") {
        source.to_owned()
    } else if source.contains("://") || source.starts_with('/') || source.starts_with("//") {
        return None;
    } else {
        format!("https://{source}")
    };
    let url = Url::parse(&candidate).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.fragment().is_some()
        || !public_host(url.host_str()?)
        || !matches!(url.host(), Some(Host::Domain(_)))
    {
        return None;
    }
    Some(Source::Remote(url))
}

/// Reject non-public and special-use addresses before connecting to a
/// remotely supplied URL list. DNS is pinned to an approved address.
fn public_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(addr) => {
            let [a, b, c, _] = addr.octets();
            !(a == 0
                || a == 10
                || a == 127
                || a >= 224
                || (a == 100 && (64..=127).contains(&b))
                || (a == 169 && b == 254)
                || (a == 172 && (16..=31).contains(&b))
                || (a == 192 && (b == 168 || b == 0 || (b == 88 && c == 99)))
                || (a == 198 && (b == 18 || b == 19 || (b == 51 && c == 100)))
                || (a == 203 && b == 0 && c == 113))
        }
        IpAddr::V6(addr) => {
            if let Some(v4) = addr.to_ipv4_mapped() {
                return public_address(IpAddr::V4(v4));
            }
            let parts = addr.segments();
            !(addr.is_loopback()
                || addr.is_unspecified()
                || addr.is_multicast()
                || (parts[0] & 0xfe00) == 0xfc00
                || (parts[0] & 0xffc0) == 0xfe80
                || (parts[0] == 0x2001 && parts[1] == 0x0db8))
        }
    }
}

async fn fetch_remote(url: Url) -> Result<Vec<u8>, Box<dyn Error + Send + Sync>> {
    let host = url.host_str().ok_or("Recursive remote URL has no host")?.to_owned();
    let port = url
        .port_or_known_default()
        .ok_or("Recursive URL has no port")?;
    let mut addresses = tokio::net::lookup_host((host.as_str(), port)).await?;
    let first = addresses
        .next()
        .ok_or("Recursive remote DNS has no addresses")?;
    if !public_address(first.ip()) || addresses.any(|addr| !public_address(addr.ip())) {
        return Err("Recursive URL resolves to private or special-use network".into());
    }
    let client = Client::builder()
        .redirect(Policy::none())
        .no_proxy()
        .timeout(Duration::from_secs(15))
        .resolve(&host, first)
        .build()?;
    let response = client.get(url).send().await?;
    read_remote_response(response).await
}

async fn read_remote_response(
    response: reqwest::Response,
) -> Result<Vec<u8>, Box<dyn Error + Send + Sync>> {
    if response.status().is_redirection() {
        return Err("Recursive remote URL list attempted an HTTP redirect".into());
    }
    let mut response = response.error_for_status()?;
    if response
        .content_length()
        .is_some_and(|n| n > MAX_BYTES as u64)
    {
        return Err("Recursive remote URL list exceeds 2 MiB".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_BYTES.saturating_sub(bytes.len()) {
            return Err("Recursive remote URL list exceeds 2 MiB".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

/// Keep the pinned extractor's first-seen occurrence order, including
/// repeated links. Invalid/unsafe URLs are not enqueued.
fn decode_links(source: &str) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let mut events = Vec::new();
    let mut cursor = 0;
    while cursor < source.len() {
        let tail = &source[cursor..];
        let next_http = tail.find("http://");
        let next_https = tail.find("https://");
        let Some(next) = (match (next_http, next_https) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) | (None, Some(a)) => Some(a),
            (None, None) => None,
        }) else {
            break;
        };
        let start = cursor + next;
        let bytes = &source.as_bytes()[start..];
        let width = bytes
            .iter()
            .position(|byte| {
                byte.is_ascii_whitespace() || matches!(byte, b'"' | b'\'' | b'<' | b'>')
            })
            .unwrap_or(bytes.len());
        if width == 0 {
            return Err("Recursive URL cursor did not advance".into());
        }
        cursor = start + width;
        let Some(raw) = source.get(start..cursor) else {
            continue;
        };
        let Ok(parsed) = Url::parse(raw) else {
            continue;
        };
        if !matches!(parsed.scheme(), "http" | "https")
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.port().is_some()
            || !matches!(parsed.host(), Some(Host::Domain(host)) if
                !host.eq_ignore_ascii_case("localhost")
                && !host.ends_with(".localhost")
                && !host.ends_with(".local")
                && !host.ends_with(".internal"))
        {
            continue;
        }
        if events.len() >= MAX_CHILDREN {
            return Err("Recursive URL list exceeds bounded child count".into());
        }
        let mut metadata = Metadata::new();
        metadata.insert("category".into(), "recursive".into());
        metadata.insert("subcategory".into(), "file".into());
        metadata.insert("ordinal".into(), (events.len() + 1).into());
        events.push(queued_url(raw.to_owned(), metadata));
    }
    Ok(events)
}

pub async fn resolve_messages(
    source: Source,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let bytes = match source {
        Source::Local(path) => {
            let file = tokio::fs::File::open(path).await?;
            let mut bytes = Vec::new();
            file.take((MAX_BYTES + 1) as u64)
                .read_to_end(&mut bytes)
                .await?;
            if bytes.len() > MAX_BYTES {
                return Err("Recursive local URL list exceeds 2 MiB".into());
            }
            bytes
        }
        Source::Remote(url) => fetch_remote(url).await?,
    };
    decode_links(&String::from_utf8(bytes)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_recursive_sources_support_explicit_local_and_public_remote() {
        assert_eq!(
            match_url("recursive:file:///tmp/galerith-source.txt"),
            Some(Source::Local(PathBuf::from("/tmp/galerith-source.txt")))
        );
        assert!(matches!(
            match_url("r:file:///tmp/urls.txt"),
            Some(Source::Local(_))
        ));
        assert!(matches!(
            match_url("recursive:https://pastebin.com/raw/FLwrCYsT"),
            Some(Source::Remote(_))
        ));
        assert!(matches!(
            match_url("r:example.org/public/urls.txt"),
            Some(Source::Remote(_))
        ));
        for bad in [
            "recursive:",
            "r:file:///tmp/../secret.txt",
            "r:file:///tmp/%2e%2e/private.txt",
            "r:file://elsewhere.example.org/tmp/list.txt",
            "r:file:///tmp/urls.txt?key=secret",
            "r:file:///tmp/urls.txt#fragment",
            "r:http://127.0.0.1/urls",
            "r:https://localhost/list",
            "r:http://myservice.internal/data",
            "r:http://example.org:8080/list",
            "r:http://name:password@example.org/data",
            "r:http://example.org/#fragment",
            "r:ftp://example.org/list",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn public_dns_guard_rejects_special_use_ip_ranges() {
        for raw in [
            "127.0.0.1",
            "10.3.4.5",
            "172.16.0.1",
            "192.168.1.2",
            "169.254.169.254",
            "100.100.100.100",
            "0.1.1.1",
            "198.18.1.1",
            "203.0.113.2",
            "::1",
            "fc00::1",
            "fe80::1",
            "2001:db8::1",
            "::ffff:127.0.0.1",
        ] {
            assert!(!public_address(raw.parse().unwrap()), "{raw}");
        }
        assert!(public_address("8.8.8.8".parse().unwrap()));
        assert!(public_address("2606:4700:4700::1111".parse().unwrap()));
    }

    #[test]
    fn ordered_url_file_preserves_duplicates_and_suppresses_unsafe_origins() {
        let content = "https://example.org/one.jpg\nhttps://example.org/one.jpg\n\
            http://127.0.0.1/private.jpg\nhttps://localhost/private\n\
            https://user:password@example.org/hidden.png\n\
            <a href=\"https://example.org/two.html\">Two</a>";
        let events = decode_links(content).unwrap();
        assert_eq!(events.len(), 3);
        let first = serde_json::to_value(&events[0]).unwrap();
        let last = serde_json::to_value(&events[2]).unwrap();
        assert_eq!(first["type"], "queue");
        assert_eq!(first["url"], "https://example.org/one.jpg");
        assert_eq!(last["url"], "https://example.org/two.html");
        assert_eq!(last["metadata"]["ordinal"], 3);
    }

    #[tokio::test]
    async fn remote_response_reader_caps_body_and_rejects_foreign_redirects() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (path, response) in [
                (
                    "/list",
                    "HTTP/1.1 200 OK\r\nContent-Length: 35\r\nConnection: close\r\n\r\nhttps://example.org/public/item.jpg".to_owned(),
                ),
                (
                    "/redirect",
                    "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1/private\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_owned(),
                ),
                (
                    "/oversized",
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        MAX_BYTES + 1
                    ),
                ),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0u8; 1024];
                let count = socket.read(&mut request).await.unwrap();
                assert!(
                    String::from_utf8_lossy(&request[..count])
                        .starts_with(&format!("GET {path} HTTP/1.1"))
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let client = Client::builder().redirect(Policy::none()).build().unwrap();
        let page = client.get(format!("http://{addr}/list")).send().await.unwrap();
        let body = String::from_utf8(read_remote_response(page).await.unwrap()).unwrap();
        assert_eq!(decode_links(&body).unwrap().len(), 1);
        let redirect = client.get(format!("http://{addr}/redirect")).send().await.unwrap();
        assert!(read_remote_response(redirect).await.is_err());
        let oversized = client.get(format!("http://{addr}/oversized")).send().await.unwrap();
        assert!(read_remote_response(oversized).await.is_err());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn local_file_is_bounded_and_queues_only_read_only_links() {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "galerith-recursive-fixture-{}.txt",
            std::process::id()
        ));
        tokio::fs::write(
            &path,
            "https://example.org/first\nhttps://example.org/second\n",
        )
        .await
        .unwrap();
        let url = format!("r:{}", Url::from_file_path(&path).unwrap());
        let events = resolve_messages(match_url(&url).unwrap()).await.unwrap();
        assert_eq!(events.len(), 2);
        tokio::fs::write(&path, vec![b'a'; MAX_BYTES + 1])
            .await
            .unwrap();
        assert!(resolve_messages(match_url(&url).unwrap()).await.is_err());
        tokio::fs::remove_file(path).await.unwrap();
    }
}
