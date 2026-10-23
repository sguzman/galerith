//! Independent public Comedy Wildlife Photography gallery-image preview.
//! Complete figure metadata (captions and dimensions), archive formatting,
//! title/section metadata and live upstream equivalence remain unimplemented.
use crate::{safe_filename, MediaItem};
use reqwest::Client;
use std::{collections::HashSet, error::Error};
use url::Url;

const MAX_HTML_BYTES: usize = 2 * 1024 * 1024;
const MAX_GALLERY_IMAGES: usize = 2000;
const ROOT: &str = "https://www.comedywildlifephoto.com/";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Gallery { section: String, name: String },
}

fn valid_segment(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

pub fn match_url(input: &str) -> Option<Target> {
    // URLs may normalize dot segments before the path is inspected.
    if input.to_ascii_lowercase().contains("%2e") {
        return None;
    }
    let url = Url::parse(input).ok()?;
    if url.scheme() != "https"
        || !matches!(
            url.host_str(),
            Some("comedywildlifephoto.com" | "www.comedywildlifephoto.com")
        )
    {
        return None;
    }
    let parts: Vec<_> = url.path_segments()?.collect();
    let ["gallery", section, page] = parts.as_slice() else {
        return None;
    };
    let name = page.strip_suffix(".php")?;
    if !valid_segment(section) || !valid_segment(name) {
        return None;
    }
    Some(Target::Gallery {
        section: (*section).to_owned(),
        name: name.to_owned(),
    })
}

fn original_image(raw: &str, index: usize) -> Option<MediaItem> {
    // Upstream figures contain root-relative image links. Do not allow a
    // protocol-relative or absolute link to switch the download host.
    if !raw.starts_with('/') || raw.starts_with("//") || raw.contains("..") {
        return None;
    }
    let url = Url::parse(ROOT).ok()?.join(raw).ok()?;
    if url.scheme() != "https" || url.host_str() != Some("www.comedywildlifephoto.com") {
        return None;
    }
    let last = url.path_segments()?.next_back()?;
    let extension = last.rsplit_once('.')?.1;
    if extension.is_empty()
        || extension.len() > 12
        || !extension.bytes().all(|b| b.is_ascii_alphanumeric())
    {
        return None;
    }
    let name = safe_filename(last);
    if matches!(name.as_str(), "" | "." | "..") {
        return None;
    }
    Some(MediaItem {
        url: url.to_string(),
        filename: format!("{index:03} {name}"),
        extractor: "comedywildlifephoto",
        post_id: None,
    })
}

fn gallery_media(html: &str) -> Vec<MediaItem> {
    let mut found = Vec::new();
    let mut seen = HashSet::new();
    for figure in html.split("<figure").skip(1).take(MAX_GALLERY_IMAGES) {
        let Some((figure, _)) = figure.split_once("</figure>") else {
            continue;
        };
        let Some((url, _)) = figure
            .split_once("href=\"")
            .and_then(|(_, tail)| tail.split_once('"'))
        else {
            continue;
        };
        if let Some(item) = original_image(url, found.len() + 1) {
            if seen.insert(item.url.clone()) {
                found.push(item);
            }
        }
    }
    found
}

async fn read_html(client: &Client, url: Url) -> Result<String, Box<dyn Error + Send + Sync>> {
    let mut response = client.get(url).send().await?.error_for_status()?;
    if response
        .content_length()
        .is_some_and(|size| size > MAX_HTML_BYTES as u64)
    {
        return Err("Comedy Wildlife Photography page exceeds the HTML limit".into());
    }
    let mut buffer = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if buffer.len().saturating_add(chunk.len()) > MAX_HTML_BYTES {
            return Err("Comedy Wildlife Photography page exceeds the HTML limit".into());
        }
        buffer.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8(buffer)?)
}

async fn resolve_at_base(
    client: &Client,
    base: &Url,
    target: Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    match target {
        Target::Gallery { section, name } => {
            let html = read_html(client, base.join(&format!("gallery/{section}/{name}.php"))?)
                .await?;
            Ok(gallery_media(&html))
        }
    }
}

pub async fn resolve(
    client: &Client,
    target: Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    resolve_at_base(client, &Url::parse(ROOT)?, target).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_pinned_gallery_route_but_not_other_pages() {
        assert_eq!(
            match_url("https://www.comedywildlifephoto.com/gallery/2026/finalists.php"),
            Some(Target::Gallery {
                section: "2026".into(),
                name: "finalists".into(),
            })
        );
        for input in [
            "http://comedywildlifephoto.com/gallery/2026/finalists.php",
            "https://evil.comedywildlifephoto.com/gallery/2026/finalists.php",
            "https://comedywildlifephoto.com/gallery/2026/finalists.jpg",
            "https://comedywildlifephoto.com/gallery/2026/../finalists.php",
            "https://comedywildlifephoto.com/gallery/2026/a%2Fb.php",
            "https://comedywildlifephoto.com/gallery/2026/",
        ] {
            assert!(match_url(input).is_none(), "{input}");
        }
    }

    #[test]
    fn images_follow_figure_order_and_ignore_duplicate_or_off_site_media() {
        let html = concat!(
            "<figure data-size=\"100x200\"><a href=\"/media/first.jpg\">",
            "<figcaption>A caption</figcaption></a></figure>",
            "<figure data-size=\"200x300\"><a href=\"/media/second.png\"></a></figure>",
            "<figure><a href=\"/media/first.jpg\"></a></figure>",
            "<figure><a href=\"https://unsafe.example/picture.jpg\"></a></figure>",
            "<figure><a href=\"//unsafe.example/bad.jpg\"></a></figure>",
            "<figure><a href=\"/../private.jpg\"></a></figure>",
        );
        let media = gallery_media(html);
        assert_eq!(media.len(), 2);
        assert_eq!(media[0].filename, "001 first.jpg");
        assert_eq!(media[0].url, "https://www.comedywildlifephoto.com/media/first.jpg");
        assert_eq!(media[1].filename, "002 second.png");
    }

    #[tokio::test]
    async fn public_gallery_reads_first_page_from_local_http_fixture() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = [0_u8; 4096];
            let count = socket.read(&mut bytes).await.unwrap();
            let request = String::from_utf8_lossy(&bytes[..count]);
            assert!(
                request.starts_with("GET /gallery/2026/finalists.php HTTP/1.1"),
                "{request}"
            );
            let body = "<figure data-size=\"100x200\"><a href=\"/gallery/one.webp\"></a></figure>";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        let base = Url::parse(&format!("http://{address}/")).unwrap();
        let results = resolve_at_base(
            &Client::new(),
            &base,
            Target::Gallery {
                section: "2026".into(),
                name: "finalists".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].filename, "001 one.webp");
        server.await.unwrap();
    }
}
