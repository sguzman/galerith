//! Partial pinned imhentai.py public gallery extractor for six mirror families.
pub mod listing;
use crate::{
    message::{media_metadata, ExtractorMessage, Metadata},
    MediaItem,
};
use reqwest::Client;
use serde_json::Value;
use std::error::Error;
use url::Url;

const MAX_HTML: usize = 2 * 1024 * 1024;
const MAX_IMAGES: usize = 160;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gallery {
    host: String,
    id: String,
}

fn supported_host(host: &str) -> Option<&str> {
    let host = host.strip_prefix("www.").unwrap_or(host);
    matches!(
        host,
        "imhentai.xxx"
            | "hentaiera.com"
            | "hentairox.com"
            | "hentaifox.com"
            | "hentaienvy.com"
            | "hentaizap.com"
    )
    .then_some(host)
}

pub fn match_url(raw: &str) -> Option<Gallery> {
    let lower = raw.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "/../", "/./"]
        .iter()
        .any(|term| lower.contains(term))
    {
        return None;
    }
    let url = Url::parse(raw).ok()?;
    let host = supported_host(url.host_str()?)?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let parts: Vec<_> = url.path_segments()?.collect();
    let id = match parts.as_slice() {
        ["gallery" | "view", id] | ["gallery" | "view", id, ""] => *id,
        _ => return None,
    };
    if !(1..=16).contains(&id.len())
        || !id.bytes().all(|b| b.is_ascii_digit())
        || id.parse::<u64>().ok().filter(|n| *n > 0).is_none()
    {
        return None;
    }
    Some(Gallery {
        host: host.to_owned(),
        id: id.to_owned(),
    })
}

fn original_seed(html: &str, host: &str) -> Option<Url> {
    let raw = html.split_once("data-src=\"")?.1.split_once('"')?.0;
    let mut url = if raw.starts_with("//") {
        Url::parse(&format!("https:{raw}")).ok()?
    } else {
        Url::parse(raw).ok()?
    };
    if url.scheme() == "http" {
        url.set_scheme("https").ok()?;
    }
    let asset_host = url.host_str()?;
    if url.scheme() != "https"
        || !(asset_host == host || asset_host.ends_with(&format!(".{host}")))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path().contains('%')
        || !url.path().rsplit('/').next()?.contains('.')
    {
        return None;
    }
    Some(url)
}

fn extension(code: &str) -> Option<&'static str> {
    match code {
        "j" => Some("jpg"),
        "p" => Some("png"),
        "g" => Some("gif"),
        "w" => Some("webp"),
        "a" => Some("avif"),
        _ => None,
    }
}

fn gallery_title(html: &str) -> Option<String> {
    let title = html
        .split_once("<h1>")?
        .1
        .split_once('<')?
        .0
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");
    if title.is_empty()
        || title.len() > 200
        || title.contains('<')
        || title.chars().any(char::is_control)
    {
        return None;
    }
    Some(title)
}

fn parse_gallery(
    html: &str,
    gallery: &Gallery,
) -> Result<Vec<(MediaItem, Metadata)>, Box<dyn Error + Send + Sync>> {
    let seed = original_seed(html, &gallery.host)
        .ok_or("IMHentai public page has no trusted first-party image root")?;
    let json = html
        .split_once("$.parseJSON('")
        .and_then(|(_, remainder)| remainder.split_once("')"))
        .map(|(value, _)| value)
        .ok_or("IMHentai public gallery lacks its image table")?;
    let table: Value = serde_json::from_str(json)?;
    let map = table
        .as_object()
        .ok_or("IMHentai gallery image table must be an object")?;
    if map.is_empty() || map.len() > MAX_IMAGES {
        return Err("IMHentai gallery image count outside supported bounds".into());
    }

    let count = map.len();
    let title = gallery_title(html);
    let mut items = Vec::with_capacity(count);
    for page in 1..=count {
        let value = map
            .get(&page.to_string())
            .and_then(Value::as_str)
            .ok_or("IMHentai gallery pages must be contiguous")?;
        let mut parts = value.split(',');
        let ext = extension(parts.next().unwrap_or(""))
            .ok_or("IMHentai media extension is unsupported")?;
        let width: u32 = parts
            .next()
            .ok_or("IMHentai media width missing")?
            .parse()?;
        let height: u32 = parts
            .next()
            .ok_or("IMHentai media height missing")?
            .parse()?;
        if parts.next().is_some() || !(1..=50000).contains(&width) || !(1..=50000).contains(&height)
        {
            return Err("IMHentai image dimensions or field count invalid".into());
        }
        let media = seed.join(&format!("{page}.{ext}"))?;
        let item = MediaItem {
            url: media.to_string(),
            filename: format!("{}_{}.{}", gallery.id, page, ext),
            extractor: "imhentai",
            post_id: None,
        };
        let mut meta = media_metadata(&item);
        meta.insert("subcategory".into(), "gallery".into());
        meta.insert("gallery_id".into(), gallery.id.parse::<u64>()?.into());
        meta.insert("page".into(), page.into());
        meta.insert("count".into(), count.into());
        meta.insert("width".into(), width.into());
        meta.insert("height".into(), height.into());
        meta.insert(
            "post_url".into(),
            format!("https://{}/gallery/{}/", gallery.host, gallery.id).into(),
        );
        if let Some(title) = &title {
            meta.insert("title".into(), title.clone().into());
        }
        items.push((item, meta));
    }
    Ok(items)
}

async fn records(
    client: &Client,
    gallery: &Gallery,
    base: &Url,
) -> Result<Vec<(MediaItem, Metadata)>, Box<dyn Error + Send + Sync>> {
    let mut response = client
        .get(base.join(&format!("gallery/{}/", gallery.id))?)
        .send()
        .await?
        .error_for_status()?;
    if response
        .content_length()
        .is_some_and(|n| n > MAX_HTML as u64)
    {
        return Err("IMHentai source HTML exceeds safety limit".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_HTML.saturating_sub(bytes.len()) {
            return Err("IMHentai source HTML exceeds safety limit".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    parse_gallery(std::str::from_utf8(&bytes)?, gallery)
}

pub async fn resolve(
    client: &Client,
    gallery: Gallery,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse(&format!("https://{}/", gallery.host))?;
    Ok(records(client, &gallery, &base)
        .await?
        .into_iter()
        .map(|(item, _)| item)
        .collect())
}

pub async fn resolve_messages(
    client: &Client,
    gallery: Gallery,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse(&format!("https://{}/", gallery.host))?;
    let mut result = Vec::new();
    for (item, meta) in records(client, &gallery, &base).await? {
        result.push(ExtractorMessage::Directory {
            metadata: meta.clone(),
        });
        result.push(ExtractorMessage::Url {
            url: item.url,
            metadata: meta,
        });
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    #[test]
    fn pinned_public_gallery_routes_check_all_six_mirrors() {
        for host in [
            "imhentai.xxx",
            "hentaiera.com",
            "hentairox.com",
            "hentaifox.com",
            "hentaienvy.com",
            "hentaizap.com",
        ] {
            assert_eq!(
                match_url(&format!("https://www.{host}/view/12345/"))
                    .unwrap()
                    .id,
                "12345"
            );
        }
        for bad in [
            "https://evilimhentai.xxx/gallery/12345/",
            "https://imhentai.xxx/gallery/abcde/",
            "https://imhentai.xxx/gallery/0/",
            "https://imhentai.xxx/gallery/12345/?secret=x",
            "https://imhentai.xxx/gallery/12345/../../secret",
            "https://imhentai.xxx/gallery/12%2f34/",
            "https://name:password@imhentai.xxx/gallery/12345/",
            "https://imhentai.xxx:8443/gallery/12345/",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn json_page_order_and_supported_media_types_are_source_backed() {
        let gallery = match_url("https://imhentai.xxx/gallery/12345/").unwrap();
        let html = r#"<h1>A &amp; B</h1><img data-src="https://img.imhentai.xxx/galleries/12345/1t.jpg"><script>$.parseJSON('{"2":"p,900,1000","1":"j,500,600","3":"w,400,800"}')</script>"#;
        let media = parse_gallery(html, &gallery).unwrap();
        assert_eq!(media.len(), 3);
        assert_eq!(
            media[0].0.url,
            "https://img.imhentai.xxx/galleries/12345/1.jpg"
        );
        assert_eq!(
            media[1].0.url,
            "https://img.imhentai.xxx/galleries/12345/2.png"
        );
        assert_eq!(media[2].0.filename, "12345_3.webp");
        assert_eq!(media[1].1["width"], 900);
        assert_eq!(media[0].1["gallery_id"], 12345);
        assert_eq!(media[0].1["title"], "A & B");
        assert!(media.iter().all(|(item, _)| item.post_id.is_none()));
    }

    #[test]
    fn reject_foreign_media_roots_and_malformed_gallery_metadata() {
        let gallery = match_url("https://imhentai.xxx/gallery/12345").unwrap();
        for bad in [
            r#"<img data-src="https://evilimhentai.xxx/a.jpg"><script>$.parseJSON('{"1":"j,1,1"}')</script>"#,
            r#"<img data-src="https://other.example/a.jpg"><script>$.parseJSON('{"1":"j,1,1"}')</script>"#,
            r#"<img data-src="https://img.imhentai.xxx/a.jpg"><script>$.parseJSON('{"1":"x,1,1"}')</script>"#,
            r#"<img data-src="https://img.imhentai.xxx/a.jpg"><script>$.parseJSON('{"2":"j,1,1"}')</script>"#,
            r#"<img data-src="https://img.imhentai.xxx/a.jpg"><script>$.parseJSON('{"1":"j,0,1"}')</script>"#,
            r#"<img data-src="https://img.imhentai.xxx/a.jpg"><script>$.parseJSON('{"1":"j,1,1,extra"}')</script>"#,
        ] {
            assert!(parse_gallery(bad, &gallery).is_err(), "{bad}");
        }
    }

    #[tokio::test]
    async fn bounded_public_gallery_get_reads_one_source_page() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = [0u8; 4096];
            let n = socket.read(&mut bytes).await.unwrap();
            assert!(
                String::from_utf8_lossy(&bytes[..n]).starts_with("GET /gallery/12345/ HTTP/1.1")
            );
            let html = r#"<img data-src="https://img.imhentai.xxx/galleries/12345/1t.jpg"><script>$.parseJSON('{"1":"j,800,1200"}')</script>"#;
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
                html.len()
            );
            socket.write_all(reply.as_bytes()).await.unwrap();
        });
        let gallery = match_url("https://imhentai.xxx/gallery/12345/").unwrap();
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let result = records(&Client::new(), &gallery, &base).await.unwrap();
        assert_eq!(result[0].0.filename, "12345_1.jpg");
        server.await.unwrap();
    }
}
