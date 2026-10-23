//! Independent Desktopography site, exhibition and portfolio extractor preview.
//! Full upstream metadata, archive keys, output-directory semantics and live
//! acceptance remain outside the verified subset.
use crate::{safe_filename, MediaItem};
use reqwest::Client;
use std::{collections::HashSet, error::Error};
use url::Url;

const ROOT: &str = "https://desktopography.net/";
const MAX_HTML_BYTES: usize = 2 * 1024 * 1024;
const MAX_EXHIBITIONS: usize = 30;
const MAX_ENTRIES: usize = 200;
const MAX_MEDIA: usize = 2000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Site,
    Exhibition { year: String },
    Entry { slug: String },
}

fn valid_slug(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'-' | b'_'))
}

pub fn match_url(input: &str) -> Option<Target> {
    if input.to_ascii_lowercase().contains("%2e") {
        return None;
    }
    let url = Url::parse(input).ok()?;
    if url.scheme() != "https" || url.host_str() != Some("desktopography.net") {
        return None;
    }
    let path = url.path().trim_end_matches('/');
    if path.is_empty() {
        return Some(Target::Site);
    }
    let parts: Vec<_> = path.split('/').filter(|part| !part.is_empty()).collect();
    match parts.as_slice() {
        [exhibition] => {
            let year = exhibition.strip_prefix("exhibition-")?;
            if year.len() == 4 && year.bytes().all(|ch| ch.is_ascii_digit()) {
                Some(Target::Exhibition {
                    year: year.to_owned(),
                })
            } else {
                None
            }
        }
        ["portfolios", slug] if valid_slug(slug) => Some(Target::Entry {
            slug: (*slug).to_owned(),
        }),
        _ => None,
    }
}

fn exhibition_years(html: &str) -> Vec<String> {
    let mut years = Vec::new();
    let mut seen = HashSet::new();
    for part in html
        .split("<a href=\"https://desktopography.net/exhibition-")
        .skip(1)
        .take(MAX_EXHIBITIONS * 4)
    {
        let Some((year, _)) = part.split_once("/\">") else {
            continue;
        };
        if year.len() == 4
            && year.bytes().all(|ch| ch.is_ascii_digit())
            && seen.insert(year.to_owned())
        {
            years.push(year.to_owned());
        }
        if years.len() >= MAX_EXHIBITIONS {
            break;
        }
    }
    years
}

fn exhibition_entries(html: &str) -> Vec<String> {
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    for part in html
        .split("<a class=\"overlay-background\" href=\"https://desktopography.net/portfolios/")
        .skip(1)
        .take(MAX_ENTRIES * 4)
    {
        let Some((slug, _)) = part.split_once("\">") else {
            continue;
        };
        let slug = slug.trim_end_matches('/');
        if valid_slug(slug) && seen.insert(slug.to_owned()) {
            entries.push(slug.to_owned());
        }
        if entries.len() >= MAX_ENTRIES {
            break;
        }
    }
    entries
}

fn portfolio_media(html: &str) -> Vec<MediaItem> {
    let mut media = Vec::new();
    let mut seen = HashSet::new();
    for part in html
        .split("<a target=\"_blank\" href=\"https://desktopography.net")
        .skip(1)
        .take(MAX_MEDIA)
    {
        let Some((fragment, _)) = part.split_once("\">") else {
            continue;
        };
        let Some((path, suggested)) =
            fragment.split_once("\" class=\"wallpaper-button\" download=\"")
        else {
            continue;
        };
        if !path.starts_with('/')
            || path.starts_with("//")
            || path.contains("..")
            || path.len() > 500
        {
            continue;
        }
        let Some(url) = Url::parse(ROOT).ok().and_then(|url| url.join(path).ok()) else {
            continue;
        };
        if url.scheme() != "https" || url.host_str() != Some("desktopography.net") {
            continue;
        }
        let Some(original) = url.path_segments().and_then(|mut parts| parts.next_back()) else {
            continue;
        };
        let Some(extension) = original.rsplit_once('.').map(|(_, ext)| ext) else {
            continue;
        };
        if extension.is_empty()
            || extension.len() > 12
            || !extension.bytes().all(|byte| byte.is_ascii_alphanumeric())
        {
            continue;
        }
        let proposed = if suggested.is_empty() || suggested.len() > 200 {
            original
        } else {
            suggested
        };
        let filename = safe_filename(proposed);
        if matches!(filename.as_str(), "" | "." | "..") {
            continue;
        }
        if seen.insert(url.to_string()) {
            media.push(MediaItem {
                url: url.to_string(),
                filename,
                extractor: "desktopography",
                post_id: None,
            });
        }
    }
    media
}

async fn read_html(client: &Client, endpoint: Url) -> Result<String, Box<dyn Error + Send + Sync>> {
    let mut response = client.get(endpoint).send().await?.error_for_status()?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_HTML_BYTES as u64)
    {
        return Err("Desktopography HTML exceeds the response limit".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len().saturating_add(chunk.len()) > MAX_HTML_BYTES {
            return Err("Desktopography HTML exceeds the response limit".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8(bytes)?)
}

async fn exhibition_media(
    client: &Client,
    base: &Url,
    year: &str,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let html = read_html(client, base.join(&format!("exhibition-{year}/"))?).await?;
    let entries = exhibition_entries(&html);
    let mut results = Vec::new();
    let mut seen = HashSet::new();
    for (index, slug) in entries.iter().enumerate() {
        if index > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
        let html = read_html(client, base.join(&format!("portfolios/{slug}"))?).await?;
        for item in portfolio_media(&html) {
            if seen.insert(item.url.clone()) {
                results.push(item);
            }
            if results.len() >= MAX_MEDIA {
                return Ok(results);
            }
        }
    }
    Ok(results)
}

async fn resolve_at_base(
    client: &Client,
    base: &Url,
    target: Target,
    max_pages: u16,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    match target {
        Target::Entry { slug } => {
            let html = read_html(client, base.join(&format!("portfolios/{slug}"))?).await?;
            Ok(portfolio_media(&html))
        }
        Target::Exhibition { year } => exhibition_media(client, base, &year).await,
        Target::Site => {
            let home = read_html(client, base.clone()).await?;
            let years = exhibition_years(&home);
            let mut items = Vec::new();
            let mut seen = HashSet::new();
            for (index, year) in years
                .iter()
                .take(usize::from(max_pages).clamp(1, MAX_EXHIBITIONS))
                .enumerate()
            {
                if index > 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                }
                for item in exhibition_media(client, base, year).await? {
                    if seen.insert(item.url.clone()) {
                        items.push(item);
                    }
                    if items.len() >= MAX_MEDIA {
                        return Ok(items);
                    }
                }
            }
            Ok(items)
        }
    }
}

pub async fn resolve(
    client: &Client,
    target: Target,
    max_pages: u16,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    resolve_at_base(client, &Url::parse(ROOT)?, target, max_pages).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_site_exhibitions_and_portfolio_entries_only() {
        assert_eq!(match_url("https://desktopography.net/"), Some(Target::Site));
        assert_eq!(
            match_url("https://desktopography.net/exhibition-2025/"),
            Some(Target::Exhibition {
                year: "2025".into()
            })
        );
        assert_eq!(
            match_url("https://desktopography.net/portfolios/forest-glow/"),
            Some(Target::Entry {
                slug: "forest-glow".into()
            })
        );
        for url in [
            "https://www.desktopography.net/",
            "http://desktopography.net/",
            "https://desktopography.net/exhibition-202a/",
            "https://desktopography.net/portfolios/hello%2Fworld",
            "https://desktopography.net/portfolios/a/extra",
        ] {
            assert!(match_url(url).is_none(), "{url}");
        }
    }

    #[test]
    fn site_and_exhibition_lists_preserve_order_and_unique_slugs() {
        let site = concat!(
            "<a href=\"https://desktopography.net/exhibition-2025/\">",
            "<a href=\"https://desktopography.net/exhibition-2024/\">",
            "<a href=\"https://desktopography.net/exhibition-2025/\">"
        );
        assert_eq!(exhibition_years(site), ["2025", "2024"]);
        let exhibition = concat!(
            "<a class=\"overlay-background\" href=\"https://desktopography.net/portfolios/first\">",
            "<a class=\"overlay-background\" href=\"https://desktopography.net/portfolios/second\">",
            "<a class=\"overlay-background\" href=\"https://desktopography.net/portfolios/first\">"
        );
        assert_eq!(exhibition_entries(exhibition), ["first", "second"]);
    }

    #[test]
    fn portfolio_only_accepts_canonical_wallpaper_buttons() {
        let html = concat!(
            "<a target=\"_blank\" href=\"https://desktopography.net/wallpapers/first.jpg\" class=\"wallpaper-button\" download=\"full.jpg\">",
            "<a target=\"_blank\" href=\"https://desktopography.net/wallpapers/second.webp\" class=\"wallpaper-button\" download=\"wide.webp\">",
            "<a target=\"_blank\" href=\"https://desktopography.net/wallpapers/first.jpg\" class=\"wallpaper-button\" download=\"dup.jpg\">",
            "<a target=\"_blank\" href=\"https://desktopography.net.evil.test/unsafe.jpg\" class=\"wallpaper-button\" download=\"evil.jpg\">"
        );
        let images = portfolio_media(html);
        assert_eq!(images.len(), 2);
        assert_eq!(images[0].filename, "full.jpg");
        assert_eq!(images[0].url, "https://desktopography.net/wallpapers/first.jpg");
        assert_eq!(images[1].filename, "wide.webp");
    }

    #[tokio::test]
    async fn site_to_exhibition_to_portfolio_follows_local_three_request_fixture() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (route, body) in [
                (
                    "GET / HTTP/1.1",
                    "<a href=\"https://desktopography.net/exhibition-2025/\">",
                ),
                (
                    "GET /exhibition-2025/ HTTP/1.1",
                    "<a class=\"overlay-background\" href=\"https://desktopography.net/portfolios/forest-glow\">",
                ),
                (
                    "GET /portfolios/forest-glow HTTP/1.1",
                    "<a target=\"_blank\" href=\"https://desktopography.net/files/forest.jpg\" class=\"wallpaper-button\" download=\"forest-wide.jpg\">",
                ),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buffer = [0_u8; 4096];
                let n = socket.read(&mut buffer).await.unwrap();
                let request = String::from_utf8_lossy(&buffer[..n]);
                assert!(request.starts_with(route), "{request}");
                let answer = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(answer.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{address}/")).unwrap();
        let result = resolve_at_base(&Client::new(), &base, Target::Site, 1)
            .await
            .unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].filename, "forest-wide.jpg");
        assert_eq!(result[0].url, "https://desktopography.net/files/forest.jpg");
        server.await.unwrap();
    }
}
