//! Independent public WallpaperCave wallpaper-page extractor preview.
//! Full upstream URL coverage, metadata, archive templates and opt-in live
//! conformance are not yet supported.
use crate::{safe_filename, MediaItem};
use reqwest::Client;
use std::{collections::HashSet, error::Error};
use url::Url;

const MAX_HTML_BYTES: usize = 2 * 1024 * 1024;
const MAX_WALLPAPERS: usize = 2000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Wallpaper { path: String },
}

pub fn match_url(input: &str) -> Option<Target> {
    if input.to_ascii_lowercase().contains("%2e") {
        return None;
    }
    let url = Url::parse(input).ok()?;
    if url.scheme() != "https"
        || !matches!(url.host_str(), Some("wallpapercave.com" | "www.wallpapercave.com"))
    {
        return None;
    }
    let parts: Vec<_> = url.path_segments()?.collect();
    match parts.as_slice() {
        ["w", id]
            if id.starts_with("wp")
                && id.len() <= 32
                && id[2..].bytes().all(|b| b.is_ascii_digit())
                && id.len() > 2 =>
        {
            Some(Target::Wallpaper {
                path: format!("w/{id}"),
            })
        }
        [slug]
            if slug.len() >= 12
                && slug.len() <= 150
                && slug.ends_with("-wallpapers")
                && slug
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-') =>
        {
            Some(Target::Wallpaper {
                path: (*slug).to_owned(),
            })
        }
        _ => None,
    }
}

fn media_from_path(raw: &str) -> Option<MediaItem> {
    let url = Url::parse(raw)
        .or_else(|_| Url::parse("https://wallpapercave.com/").and_then(|base| base.join(raw)))
        .ok()?;
    if url.scheme() != "https"
        || !matches!(url.host_str(), Some("wallpapercave.com" | "www.wallpapercave.com"))
    {
        return None;
    }
    let last = url.path_segments()?.next_back()?;
    let ext = last.rsplit_once('.')?.1;
    if ext.is_empty() || ext.len() > 12 || !ext.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return None;
    }
    let filename = safe_filename(last);
    if matches!(filename.as_str(), "" | "." | "..") {
        return None;
    }
    Some(MediaItem {
        url: url.to_string(),
        filename,
        extractor: "wallpapercave",
        post_id: None,
    })
}

fn collect_paths<'a>(html: &'a str, marker: &str) -> Vec<&'a str> {
    html.split(marker)
        .skip(1)
        .take(MAX_WALLPAPERS)
        .filter_map(|fragment| fragment.split_once('"').map(|(path, _)| path))
        .collect()
}

fn wallpapers(html: &str) -> Vec<MediaItem> {
    let downloads = collect_paths(html, "class=\"download\" href=\"");
    let candidates: Vec<_> = if !downloads.is_empty() {
        downloads
    } else if let Some((_, rest)) = html.split_once("id=\"tdownload\"") {
        let link = rest
            .split_once("href=\"")
            .and_then(|(_, part)| part.split_once('"'))
            .map(|(url, _)| url);
        link.into_iter().collect()
    } else {
        html.split("class=\"wallpaper\" id=\"wp")
            .skip(1)
            .take(MAX_WALLPAPERS)
            .filter_map(|part| {
                let block = part.split_once("</picture>")?.0;
                block
                    .rsplit_once(" src=\"")
                    .and_then(|(_, right)| right.split_once('"'))
                    .map(|(path, _)| path)
            })
            .collect()
    };
    let mut found = Vec::new();
    let mut seen = HashSet::new();
    for raw in candidates {
        if let Some(media) = media_from_path(raw) {
            if seen.insert(media.url.clone()) {
                found.push(media);
            }
        }
    }
    found
}

async fn read_html(client: &Client, url: Url) -> Result<String, Box<dyn Error + Send + Sync>> {
    let mut response = client.get(url).send().await?.error_for_status()?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_HTML_BYTES as u64)
    {
        return Err("WallpaperCave HTML exceeds the configured safety bound".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len().saturating_add(chunk.len()) > MAX_HTML_BYTES {
            return Err("WallpaperCave HTML exceeds the configured safety bound".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8(bytes)?)
}

async fn resolve_at_base(
    client: &Client,
    base: &Url,
    target: Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    match target {
        Target::Wallpaper { path } => {
            let html = read_html(client, base.join(&path)?).await?;
            Ok(wallpapers(&html))
        }
    }
}

pub async fn resolve(
    client: &Client,
    target: Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    resolve_at_base(client, &Url::parse("https://wallpapercave.com/")?, target).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wallpaper_routes_have_exact_hosts_and_safe_paths() {
        assert_eq!(
            match_url("https://wallpapercave.com/w/wp12345"),
            Some(Target::Wallpaper {
                path: "w/wp12345".into()
            })
        );
        assert_eq!(
            match_url("https://www.wallpapercave.com/nature-wallpapers"),
            Some(Target::Wallpaper {
                path: "nature-wallpapers".into()
            })
        );
        for input in [
            "http://wallpapercave.com/w/wp12345",
            "https://evil.wallpapercave.com/w/wp12345",
            "https://wallpapercave.com/w/wp",
            "https://wallpapercave.com/w/notwp123",
            "https://wallpapercave.com/other/page",
            "https://wallpapercave.com/w/wp123%2Fprivate",
        ] {
            assert!(match_url(input).is_none(), "{input}");
        }
    }

    #[test]
    fn download_links_take_priority_over_fallbacks() {
        let html = concat!(
            "<a class=\"download\" href=\"/dwp1x/wp1.jpg\"></a>",
            "<a class=\"download\" href=\"/dwp1x/wp2.png\"></a>",
            "<a class=\"download\" href=\"/dwp1x/wp1.jpg\"></a>",
            "<a class=\"download\" href=\"https://evil.example/wp3.jpg\"></a>",
            "<div id=\"tdownload\" href=\"/dwp1x/fallback.jpg\"></div>",
        );
        let media = wallpapers(html);
        assert_eq!(media.len(), 2);
        assert_eq!(media[0].url, "https://wallpapercave.com/dwp1x/wp1.jpg");
        assert_eq!(media[1].filename, "wp2.png");
    }

    #[test]
    fn fallback_download_and_picture_gallery_paths() {
        let direct = "<a id=\"tdownload\" href=\"/wp/wallpaper.jpg\"></a>";
        assert_eq!(wallpapers(direct)[0].filename, "wallpaper.jpg");
        let gallery = concat!(
            "<picture class=\"wallpaper\" id=\"wp123\"><source src=\"/wp/one.webp\"></picture>",
            "<picture class=\"wallpaper\" id=\"wp456\"><img src=\"/wp/two.jpg\"></picture>"
        );
        let media = wallpapers(gallery);
        assert_eq!(media.len(), 2);
        assert_eq!(media[0].filename, "one.webp");
        assert_eq!(media[1].filename, "two.jpg");
        assert!(wallpapers("<a class=\"download\" href=\"https://evil.example/p.jpg\"></a>").is_empty());
    }

    #[tokio::test]
    async fn local_wallpaper_http_fixture_returns_valid_download_links() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = [0_u8; 4096];
            let n = socket.read(&mut buffer).await.unwrap();
            let request = String::from_utf8_lossy(&buffer[..n]);
            assert!(request.starts_with("GET /w/wp12345 HTTP/1.1"), "{request}");
            let body = "<a class=\"download\" href=\"/dwp1x/sample.webp\"></a>";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let media = resolve_at_base(
            &Client::new(),
            &base,
            Target::Wallpaper {
                path: "w/wp12345".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(media.len(), 1);
        assert_eq!(media[0].filename, "sample.webp");
        server.await.unwrap();
    }
}
