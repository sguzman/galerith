//! Bounded public tag/search discovery for the pinned IMHentai mirror family.
//! Queues are opt-in; unsupported or private URLs are never followed.
use crate::message::{queued_url, ExtractorMessage, Metadata};
use reqwest::Client;
use std::{collections::HashSet, error::Error, time::Duration};
use url::Url;

const MAX_HTML: usize = 2 * 1024 * 1024;
const MAX_PAGES: u16 = 4;
const MAX_CHILDREN: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing {
    pub host: String,
    pub kind: String,
    pub key: String,
    path: String,
    search: bool,
}

fn safe_slug(s: &str) -> bool {
    (1..=96).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

pub fn match_url(raw: &str) -> Option<Listing> {
    let lower = raw.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "/../", "/./"]
        .iter()
        .any(|bad| lower.contains(bad))
    {
        return None;
    }
    let url = Url::parse(raw).ok()?;
    let host = super::supported_host(url.host_str()?)?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let segments: Vec<_> = url.path_segments()?.collect();
    let (kind, key, search) = match segments.as_slice() {
        ["artist" | "category" | "character" | "group" | "language" | "parody" | "tag", key]
        | ["artist" | "category" | "character" | "group" | "language" | "parody" | "tag", key, ""]
            if safe_slug(key) && url.query().is_none() =>
        {
            (segments[0].to_owned(), (*key).to_owned(), false)
        }
        ["search" | "advanced-search"] | ["search" | "advanced-search", ""] => {
            let params: Vec<_> = url.query_pairs().collect();
            if params.len() != 1 || params[0].0 != "key" {
                return None;
            }
            let key = params[0].1.as_ref();
            if key.trim().is_empty()
                || key.len() > 240
                || key.chars().count() > 120
                || key.chars().any(char::is_control)
            {
                return None;
            }
            (segments[0].to_owned(), key.to_owned(), true)
        }
        _ => return None,
    };
    let path = if search {
        format!("/{kind}/")
    } else {
        format!("/{kind}/{key}/")
    };
    Some(Listing {
        host: host.to_owned(),
        kind,
        key,
        path,
        search,
    })
}

fn gallery_id(value: &str) -> Option<&str> {
    let id = value.strip_prefix("/gallery/")?.trim_end_matches('/');
    if id.is_empty()
        || id.len() > 16
        || !id.bytes().all(|b| b.is_ascii_digit())
        || id.parse::<u64>().ok().filter(|n| *n > 0).is_none()
    {
        return None;
    }
    Some(id)
}

fn gallery_events(
    html: &str,
    listing: &Listing,
    seen: &mut HashSet<String>,
    events: &mut Vec<ExtractorMessage>,
) {
    let before_ranking = html
        .split_once("class=\"ranking_list\"")
        .map(|(before, _)| before)
        .unwrap_or(html);
    for element in before_ranking.split("<a ").skip(1).take(5000) {
        if events.len() >= MAX_CHILDREN {
            break;
        }
        let attrs = element
            .split_once('>')
            .map(|(attributes, _)| attributes)
            .unwrap_or("");
        let raw = attrs
            .split_once("href=\"")
            .and_then(|(_, after)| after.split_once('"'))
            .or_else(|| {
                attrs
                    .split_once("href='")
                    .and_then(|(_, after)| after.split_once('\''))
            })
            .map(|(href, _)| href);
        let Some(id) = raw.and_then(gallery_id) else {
            continue;
        };
        if !seen.insert(id.to_owned()) {
            continue;
        }
        let mut meta = Metadata::new();
        meta.insert("category".into(), "imhentai".into());
        meta.insert(
            "subcategory".into(),
            (if listing.search { "search" } else { "tag" }).into(),
        );
        meta.insert("listing_type".into(), listing.kind.clone().into());
        meta.insert("listing_value".into(), listing.key.clone().into());
        meta.insert("gallery_id".into(), id.parse::<u64>().unwrap().into());
        events.push(queued_url(
            format!("https://{}/gallery/{id}/", listing.host),
            meta,
        ));
    }
}

fn next_page(
    html: &str,
    current: &Url,
    listing: &Listing,
    current_page: u16,
) -> Result<Option<Url>, Box<dyn Error + Send + Sync>> {
    let raw = html
        .rsplit_once("class='page-link' href='")
        .and_then(|(_, after)| after.split_once('\''))
        .or_else(|| {
            html.rsplit_once("class=\"page-link\" href=\"")
                .and_then(|(_, after)| after.split_once('"'))
        })
        .map(|(href, _)| href);
    let Some(raw) = raw else { return Ok(None) };
    if raw == "#" {
        return Ok(None);
    }
    let next = current.join(raw)?;
    // The public entry point always starts with HTTPS. Compare to the
    // already-validated origin so the local HTTP fixture follows the same rule.
    if next.scheme() != current.scheme()
        || next.host_str() != current.host_str()
        || !next.username().is_empty()
        || next.password().is_some()
        || next.port_or_known_default() != current.port_or_known_default()
        || next.fragment().is_some()
        || next.path() != listing.path
    {
        return Err("IMHentai listing next page escaped source scope".into());
    }
    let params: Vec<_> = next.query_pairs().collect();
    if params.len() > (if listing.search { 2 } else { 1 })
        || params
            .iter()
            .any(|(key, _)| !matches!(key.as_ref(), "page" | "key"))
    {
        return Err("IMHentai next page has unknown query parameters".into());
    }
    let page_values: Vec<_> = params
        .iter()
        .filter(|(k, _)| k == "page")
        .map(|(_, v)| v.as_ref())
        .collect();
    if page_values.len() != 1 {
        return Err("IMHentai listing next page lacks a unique ordinal".into());
    }
    if listing.search {
        let keys: Vec<_> = params
            .iter()
            .filter(|(k, _)| k == "key")
            .map(|(_, v)| v.as_ref())
            .collect();
        if keys != [listing.key.as_str()] {
            return Err("IMHentai search continuation changed its query".into());
        }
    } else if params.iter().any(|(k, _)| k == "key") {
        return Err("IMHentai tag continuation changed its query kind".into());
    }
    let page: u16 = page_values[0].parse()?;
    if page <= current_page {
        return Err("IMHentai listing continuation did not advance".into());
    }
    if page > MAX_PAGES {
        return Ok(None);
    }
    Ok(Some(next))
}

async fn read_html(client: &Client, url: Url) -> Result<String, Box<dyn Error + Send + Sync>> {
    let mut response = client.get(url).send().await?.error_for_status()?;
    if response
        .content_length()
        .is_some_and(|n| n > MAX_HTML as u64)
    {
        return Err("IMHentai listing source exceeds HTML size limit".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_HTML.saturating_sub(bytes.len()) {
            return Err("IMHentai listing source exceeds HTML size limit".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8(bytes)?)
}

async fn resolve_at_base(
    client: &Client,
    listing: &Listing,
    base: &Url,
    requested_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let mut url = base.join(&listing.path)?;
    if listing.search {
        url.query_pairs_mut().append_pair("key", &listing.key);
    }
    let mut seen = HashSet::new();
    let mut events = Vec::new();
    let mut ordinal = 1;
    for request_number in 0..requested_pages.clamp(1, MAX_PAGES) {
        if request_number > 0 {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        let page = read_html(client, url.clone()).await?;
        gallery_events(&page, listing, &mut seen, &mut events);
        if events.len() >= MAX_CHILDREN {
            break;
        }
        let Some(next) = next_page(&page, &url, listing, ordinal)? else {
            break;
        };
        ordinal = next
            .query_pairs()
            .find(|(key, _)| key == "page")
            .and_then(|(_, value)| value.parse().ok())
            .ok_or("Missing IMHentai next page ordinal")?;
        url = next;
    }
    Ok(events)
}

pub async fn resolve_messages(
    client: &Client,
    listing: Listing,
    pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse(&format!("https://{}/", listing.host))?;
    resolve_at_base(client, &listing, &base, pages).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    #[test]
    fn pinned_tag_and_search_routes_reject_private_or_ambiguous_queries() {
        assert_eq!(
            match_url("https://imhentai.xxx/tag/art/").unwrap().key,
            "art"
        );
        assert_eq!(
            match_url("https://hentaifox.com/search/?key=blue+sky")
                .unwrap()
                .key,
            "blue sky"
        );
        for bad in [
            "https://imhentai.xxx/tag/art?secret=x",
            "https://imhentai.xxx/tag/a%2fb",
            "https://imhentai.xxx/search/?key=one&key=two",
            "https://imhentai.xxx/search/?private=1",
            "https://imhentai.xxx/tag/../../private",
            "https://evilimhentai.xxx/tag/art",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn listing_discards_rankings_duplicates_and_non_gallery_links() {
        let listing = match_url("https://imhentai.xxx/tag/art/").unwrap();
        let page = r#"<a href="/gallery/12345/">A</a><a href="/gallery/12345/">Again</a><a href="/gallery/12346/">B</a><a href="https://bad.example/gallery/1">Bad</a><div class="ranking_list"><a href="/gallery/99999/">Rank</a>"#;
        let mut seen = HashSet::new();
        let mut events = Vec::new();
        gallery_events(page, &listing, &mut seen, &mut events);
        assert_eq!(events.len(), 2);
        let first = serde_json::to_value(&events[0]).unwrap();
        assert_eq!(first["url"], "https://imhentai.xxx/gallery/12345/");
        assert_eq!(first["metadata"]["gallery_id"], 12345);
        assert_eq!(first["metadata"]["subcategory"], "tag");
        assert_eq!(first["metadata"]["listing_type"], "tag");
    }

    #[test]
    fn pagination_preserves_origin_path_and_monotonic_page_numbers() {
        let listing = match_url("https://imhentai.xxx/tag/art/").unwrap();
        let current = Url::parse("https://imhentai.xxx/tag/art/").unwrap();
        let good = "<a class='page-link' href='/tag/art/?page=2'>More</a>";
        assert_eq!(
            next_page(good, &current, &listing, 1)
                .unwrap()
                .unwrap()
                .as_str(),
            "https://imhentai.xxx/tag/art/?page=2"
        );
        for bad in [
            "<a class='page-link' href='https://evil.example/tag/art/?page=2'>More</a>",
            "<a class='page-link' href='/tag/other/?page=2'>More</a>",
            "<a class='page-link' href='/tag/art/?page=1'>More</a>",
            "<a class='page-link' href='/tag/art/?page=2&token=secret'>More</a>",
        ] {
            assert!(next_page(bad, &current, &listing, 1).is_err(), "{bad}");
        }
    }

    #[tokio::test]
    async fn two_public_listing_pages_keep_first_seen_children() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (path, html) in [
                (
                    "/tag/art/",
                    r#"<a href="/gallery/12345/">A</a><a class='page-link' href='/tag/art/?page=2'>Next</a>"#,
                ),
                (
                    "/tag/art/?page=2",
                    r#"<a href="/gallery/12345/">A</a><a href="/gallery/12346/">B</a>"#,
                ),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0u8; 4096];
                let n = socket.read(&mut request).await.unwrap();
                assert!(String::from_utf8_lossy(&request[..n])
                    .starts_with(&format!("GET {path} HTTP/1.1")));
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
                    html.len()
                );
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let listing = match_url("https://imhentai.xxx/tag/art/").unwrap();
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let results = resolve_at_base(&Client::new(), &listing, &base, 4)
            .await
            .unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(
            serde_json::to_value(&results[1]).unwrap()["url"],
            "https://imhentai.xxx/gallery/12346/"
        );
        server.await.unwrap();
    }
}
