//! Partial, independently implemented public Catbox file and album extractor.
//! Full upstream metadata, URL aliases and live-site parity remain outstanding.
use crate::{safe_filename, MediaItem};
use reqwest::Client;
use std::{collections::HashSet, error::Error};
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    File { url: String, filename: String },
    Album { id: String },
}

fn clean_name(raw: &str) -> Option<String> {
    let decoded = percent_encoding::percent_decode_str(raw)
        .decode_utf8()
        .ok()?;
    if decoded.is_empty()
        || decoded.len() > 255
        || matches!(decoded.as_ref(), "." | "..")
        || decoded.chars().any(|ch| ch.is_control() || ch == '/' || ch == '\\')
    {
        return None;
    }
    Some(safe_filename(&decoded))
}

pub fn match_url(input: &str) -> Option<Target> {
    let url = Url::parse(input).ok()?;
    if url.scheme() != "https" {
        return None;
    }
    let host = url.host_str()?;
    let segments: Vec<_> = url.path_segments()?.collect();
    match (host, segments.as_slice()) {
        ("catbox.moe" | "www.catbox.moe", ["c", id])
            if !id.is_empty()
                && id.len() <= 64
                && id
                    .bytes()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'-' | b'_')) =>
        {
            Some(Target::Album { id: (*id).into() })
        }
        ("files.catbox.moe" | "litter.catbox.moe" | "de.catbox.moe", [name]) => {
            let filename = clean_name(name)?;
            Some(Target::File {
                url: url.to_string(),
                filename,
            })
        }
        _ => None,
    }
}

fn media_from_url(input: &str) -> Option<MediaItem> {
    match match_url(input)? {
        Target::File { url, filename } => Some(MediaItem {
            url,
            filename,
            extractor: "catbox",
            post_id: None,
        }),
        Target::Album { .. } => None,
    }
}

fn parse_album_html(html: &str) -> Vec<MediaItem> {
    let mut media = Vec::new();
    let mut seen = HashSet::new();
    for fragment in html.split(">https://files.catbox.moe/").skip(1).take(2000) {
        let name = fragment.split('<').next().unwrap_or("");
        if name.is_empty() || name.contains('&') || name.contains('"') {
            continue;
        }
        let raw = format!("https://files.catbox.moe/{name}");
        if let Some(item) = media_from_url(&raw) {
            if seen.insert(item.url.clone()) {
                media.push(item);
            }
        }
    }
    media
}

async fn resolve_album_at_base(
    client: &Client,
    base: &Url,
    id: &str,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let endpoint = base.join(&format!("c/{id}"))?;
    let page = client.get(endpoint).send().await?.error_for_status()?.text().await?;
    Ok(parse_album_html(&page))
}

pub async fn resolve(
    client: &Client,
    target: Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    match target {
        Target::File { url, filename } => Ok(vec![MediaItem {
            url,
            filename,
            extractor: "catbox",
            post_id: None,
        }]),
        Target::Album { id } => {
            let base = Url::parse("https://catbox.moe/")?;
            resolve_album_at_base(client, &base, &id).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_only_supported_catbox_hosts_and_paths() {
        assert_eq!(
            match_url("https://catbox.moe/c/album123"),
            Some(Target::Album {
                id: "album123".into(),
            })
        );
        assert_eq!(
            match_url("https://files.catbox.moe/abc123.jpg"),
            Some(Target::File {
                url: "https://files.catbox.moe/abc123.jpg".into(),
                filename: "abc123.jpg".into(),
            })
        );
        assert!(matches!(
            match_url("https://litter.catbox.moe/archive.zip"),
            Some(Target::File { .. })
        ));
        assert!(match_url("https://catbox.moe/c/a%2Fb").is_none());
        assert!(match_url("https://files.catbox.moe/%2E%2E").is_none());
        assert!(match_url("https://files.catbox.moe/bad%2Fname.jpg").is_none());
        assert!(match_url("https://evil.catbox.moe/c/album123").is_none());
        assert!(match_url("http://catbox.moe/c/album123").is_none());
    }

    #[test]
    fn album_html_preserves_order_and_deduplicates_verified_file_links() {
        let html = concat!(
            "<a>https://files.catbox.moe/one.jpg</a>",
            "<a>https://files.catbox.moe/two.png</a>",
            "<a>https://files.catbox.moe/one.jpg</a>",
            "<a>https://evil.example.org/unsafe.jpg</a>",
            "<a>https://files.catbox.moe/bad%2Fname.jpg</a>",
        );
        let media = parse_album_html(html);
        assert_eq!(media.len(), 2);
        assert_eq!(media[0].filename, "one.jpg");
        assert_eq!(media[1].filename, "two.png");
    }

    #[tokio::test]
    async fn album_resolves_through_bounded_local_html_fixture() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0_u8; 4096];
            let n = socket.read(&mut buf).await.unwrap();
            let request = String::from_utf8_lossy(&buf[..n]);
            assert!(request.starts_with("GET /c/album123 HTTP/1.1"), "{request}");
            let body = "<p>https://files.catbox.moe/image.webp</p>";
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(reply.as_bytes()).await.unwrap();
        });
        let base = Url::parse(&format!("http://{address}/")).unwrap();
        let media = resolve_album_at_base(&Client::new(), &base, "album123")
            .await
            .unwrap();
        assert_eq!(media.len(), 1);
        assert_eq!(media[0].filename, "image.webp");
        server.await.unwrap();
    }
}
