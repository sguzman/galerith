//! Guarded public Vanilla Rock post media and bounded tag/category queue subset.
use crate::{
    message::{media_metadata, queued_url, ExtractorMessage, Metadata},
    MediaItem,
};
use reqwest::Client;
use std::{collections::HashSet, error::Error, time::Duration};
use url::Url;

const MAX_HTML: usize = 2 * 1024 * 1024;
const MAX_POST_MEDIA: usize = 160;
const MAX_LIST_PAGES: u16 = 4;
const MAX_CHILDREN: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Post(String),
    Listing { kind: String, slug: String },
}

fn safe_slug(name: &str) -> bool {
    (1..=120).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

fn has_unsafe_path(raw: &str) -> bool {
    let lower = raw.to_ascii_lowercase();
    ["%2e", "%2f", "%5c", "/../", "/./", "\\"]
        .iter()
        .any(|needle| lower.contains(needle))
}

pub fn match_url(raw: &str) -> Option<Target> {
    if has_unsafe_path(raw) {
        return None;
    }
    let url = Url::parse(raw).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !matches!(
            url.host_str(),
            Some("vanilla-rock.com" | "www.vanilla-rock.com")
        )
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || raw.contains('%')
    {
        return None;
    }
    let parts: Vec<_> = url.path_segments()?.collect();
    match parts.as_slice() {
        [kind, slug] | [kind, slug, ""]
            if matches!(*kind, "tag" | "category") && safe_slug(slug) =>
        {
            Some(Target::Listing {
                kind: (*kind).into(),
                slug: (*slug).into(),
            })
        }
        [slug] | [slug, ""] if safe_slug(slug) && !matches!(*slug, "tag" | "category") => {
            Some(Target::Post((*slug).into()))
        }
        _ => None,
    }
}

fn capture<'a>(page: &'a str, before: &str, after: &str) -> Option<&'a str> {
    page.split_once(before)?
        .1
        .split_once(after)
        .map(|(text, _)| text)
}

fn plain_text(s: &str) -> Option<String> {
    if s.len() > 300 || s.chars().any(char::is_control) || s.contains('<') {
        return None;
    }
    Some(
        s.replace("&amp;", "&")
            .replace("&quot;", "\"")
            .trim()
            .into(),
    )
}

fn media_url(base: &Url, input: &str) -> Option<(String, String)> {
    if has_unsafe_path(input) {
        return None;
    }
    let mut url = base.join(input).ok()?;
    if url.scheme() == "http" {
        url.set_scheme("https").ok()?;
    }
    let host = url.host_str()?;
    if url.scheme() != "https"
        || !(host == "vanilla-rock.com" || host.ends_with(".vanilla-rock.com"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path().contains('%')
        || url.path().contains("..")
    {
        return None;
    }
    let name = url.path_segments()?.next_back()?;
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif"
    )
    .then(|| (url.to_string(), ext))
}

fn decode_post(
    html: &str,
    slug: &str,
    origin: &Url,
) -> Result<Vec<(MediaItem, Metadata)>, Box<dyn Error + Send + Sync>> {
    let title = capture(html, "<h1 class=\"entry-title\">", "<")
        .and_then(plain_text)
        .ok_or("Vanilla Rock public post lacks a supported title")?;
    let date = capture(html, "<div class=\"date\">", "</div>").and_then(plain_text);
    let mut images = Vec::new();
    let mut seen = HashSet::new();
    for block in html
        .split("<div class=\"main-img\">")
        .skip(1)
        .take(MAX_POST_MEDIA + 1)
    {
        let Some((within, _)) = block.split_once("</div>") else {
            continue;
        };
        let Some(link) = capture(within, "href=\"", "\"") else {
            continue;
        };
        let Some((url, ext)) = media_url(origin, link) else {
            return Err("Vanilla Rock post image is not a trusted first-party original".into());
        };
        if !seen.insert(url.clone()) {
            continue;
        };
        if images.len() >= MAX_POST_MEDIA {
            return Err("Vanilla Rock post exceeds bounded original image count".into());
        }
        let num = images.len() + 1;
        let item = MediaItem {
            url,
            filename: format!("{slug}_{num:02}.{ext}"),
            extractor: "vanillarock",
            post_id: None,
        };
        let mut meta = media_metadata(&item);
        meta.insert("subcategory".into(), "post".into());
        meta.insert("path".into(), slug.into());
        meta.insert("title".into(), title.clone().into());
        meta.insert("num".into(), num.into());
        meta.insert(
            "post_url".into(),
            format!("https://vanilla-rock.com/{slug}").into(),
        );
        if let Some(ref date) = date {
            meta.insert("date_source".into(), date.clone().into());
        }
        images.push((item, meta));
    }
    if images.is_empty() {
        return Err("Vanilla Rock public post includes no supported first-party originals".into());
    }
    let total = images.len();
    for (_, meta) in &mut images {
        meta.insert("count".into(), total.into());
    }
    Ok(images)
}

fn safe_post_href(raw: &str) -> Option<String> {
    if has_unsafe_path(raw) {
        return None;
    }
    let url = Url::parse("https://vanilla-rock.com/")
        .ok()?
        .join(raw)
        .ok()?;
    match match_url(url.as_str())? {
        Target::Post(slug) => Some(format!("https://vanilla-rock.com/{slug}")),
        _ => None,
    }
}

fn discover_posts(
    html: &str,
    seen: &mut HashSet<String>,
    out: &mut Vec<ExtractorMessage>,
    kind: &str,
    tag: &str,
) {
    for part in html.split("<h2 class=\"entry-title\">").skip(1).take(4000) {
        if out.len() >= MAX_CHILDREN {
            break;
        }
        let Some((heading, _)) = part.split_once("</h2>") else {
            continue;
        };
        let Some(href) = capture(heading, "href=\"", "\"") else {
            continue;
        };
        let Some(url) = safe_post_href(href) else {
            continue;
        };
        if !seen.insert(url.clone()) {
            continue;
        };
        let mut meta = Metadata::new();
        meta.insert("category".into(), "vanillarock".into());
        meta.insert("subcategory".into(), "tag".into());
        meta.insert("listing_kind".into(), kind.into());
        meta.insert("listing_tag".into(), tag.into());
        out.push(queued_url(url, meta));
    }
}

fn next_page(base: &Url, html: &str, kind: &str, tag: &str, current: u16) -> Option<Url> {
    let raw = capture(html, "class=\"next page-numbers\" href=\"", "\"")?;
    if has_unsafe_path(raw) {
        return None;
    }
    let url = base.join(raw).ok()?;
    if url.origin() != base.origin()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path().contains('%')
        || url.path() != format!("/{kind}/{tag}/page/{}/", current + 1)
    {
        return None;
    }
    Some(url)
}

async fn bounded_html(
    client: &Client,
    endpoint: Url,
) -> Result<String, Box<dyn Error + Send + Sync>> {
    let mut response = client
        .get(endpoint.clone())
        .send()
        .await?
        .error_for_status()?;
    if response.url().origin() != endpoint.origin() {
        return Err("Vanilla Rock page redirected outside origin".into());
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_HTML as u64)
    {
        return Err("Vanilla Rock page exceeds 2 MiB HTML cap".into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_HTML.saturating_sub(body.len()) {
            return Err("Vanilla Rock HTML response exceeds cap".into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8(body)?)
}

async fn post_at_base(
    client: &Client,
    base: &Url,
    slug: &str,
) -> Result<Vec<(MediaItem, Metadata)>, Box<dyn Error + Send + Sync>> {
    let page = bounded_html(client, base.join(slug)?).await?;
    decode_post(&page, slug, base)
}

async fn listing_at_base(
    client: &Client,
    base: &Url,
    kind: &str,
    tag: &str,
    max_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let mut endpoint = base.join(&format!("{kind}/{tag}/"))?;
    let mut seen = HashSet::new();
    let mut pages = HashSet::new();
    let mut out = Vec::new();
    for current in 1..=max_pages.clamp(1, MAX_LIST_PAGES) {
        if !pages.insert(endpoint.to_string()) {
            return Err("Vanilla Rock listing pagination repeated an earlier page".into());
        }
        if current > 1 {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        let html = bounded_html(client, endpoint.clone()).await?;
        discover_posts(&html, &mut seen, &mut out, kind, tag);
        if out.len() >= MAX_CHILDREN {
            break;
        }
        let Some(next) = next_page(base, &html, kind, tag, current) else {
            break;
        };
        endpoint = next;
    }
    Ok(out)
}

pub async fn resolve(
    client: &Client,
    target: Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    match target {
        Target::Post(slug) => {
            Ok(
                post_at_base(client, &Url::parse("https://vanilla-rock.com/")?, &slug)
                    .await?
                    .into_iter()
                    .map(|(item, _)| item)
                    .collect(),
            )
        }
        Target::Listing { .. } => Err(
            "Vanilla Rock tag/category sources require --dump-messages or --follow-queues".into(),
        ),
    }
}

pub async fn resolve_messages(
    client: &Client,
    target: Target,
    max_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse("https://vanilla-rock.com/")?;
    match target {
        Target::Post(slug) => {
            let mut events = Vec::new();
            for (item, metadata) in post_at_base(client, &base, &slug).await? {
                events.push(ExtractorMessage::Directory {
                    metadata: metadata.clone(),
                });
                events.push(ExtractorMessage::Url {
                    url: item.url,
                    metadata,
                });
            }
            Ok(events)
        }
        Target::Listing { kind, slug } => {
            listing_at_base(client, &base, &kind, &slug, max_pages).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    #[test]
    fn routes_public_post_and_listing_urls_without_traversal() {
        assert_eq!(
            match_url("https://vanilla-rock.com/my-post"),
            Some(Target::Post("my-post".into()))
        );
        assert_eq!(
            match_url("https://www.vanilla-rock.com/tag/illustration/"),
            Some(Target::Listing {
                kind: "tag".into(),
                slug: "illustration".into()
            })
        );
        for bad in [
            "https://evilvanilla-rock.com/my-post",
            "https://vanilla-rock.com/my-post?private=1",
            "https://vanilla-rock.com/tag/../secret",
            "https://vanilla-rock.com/my%2fpost",
            "https://name:pass@vanilla-rock.com/my-post",
            "https://vanilla-rock.com:8443/my-post",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn public_post_extracts_ordered_originals_and_source_metadata() {
        let html = r#"<h1 class="entry-title">Spring &amp; Summer</h1><div class="date">2026-04-20</div><div class="main-img"><a href="https://vanilla-rock.com/assets/one.jpg">Photo</a></div><div class="main-img"><a href="/assets/two.png">Photo</a></div>"#;
        let base = Url::parse("https://vanilla-rock.com/").unwrap();
        let media = decode_post(html, "my-post", &base).unwrap();
        assert_eq!(media.len(), 2);
        assert_eq!(media[0].0.filename, "my-post_01.jpg");
        assert_eq!(media[1].1["count"], 2);
        assert_eq!(media[0].1["title"], "Spring & Summer");
        assert_eq!(media[0].1["date_source"], "2026-04-20");
        assert!(decode_post(
            &html.replace(
                "vanilla-rock.com/assets",
                "vanilla-rock.com.evil.test/assets"
            ),
            "my-post",
            &base
        )
        .is_err());
    }

    #[test]
    fn listing_links_deduplicate_and_continuations_are_origin_scoped() {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        let html = r#"<h2 class="entry-title"><a href="/one">One</a></h2><h2 class="entry-title"><a href="https://vanilla-rock.com/one">Again</a></h2><h2 class="entry-title"><a href="/two">Two</a></h2><h2 class="entry-title"><a href="https://evilvanilla-rock.com/evil">Forged</a></h2>"#;
        discover_posts(html, &mut seen, &mut out, "tag", "animals");
        assert_eq!(out.len(), 2);
        assert_eq!(
            serde_json::to_value(&out[1]).unwrap()["url"],
            "https://vanilla-rock.com/two"
        );
        let base = Url::parse("https://vanilla-rock.com/").unwrap();
        assert!(next_page(
            &base,
            r#"<a class="next page-numbers" href="/tag/animals/page/2/">Next</a>"#,
            "tag",
            "animals",
            1
        )
        .is_some());
        assert!(next_page(&base,r#"<a class="next page-numbers" href="https://evilvanilla-rock.com/tag/animals/page/2/">Next</a>"#,"tag","animals",1).is_none());
    }

    #[tokio::test]
    async fn bounded_single_post_and_two_page_listing_http_fixtures() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (path, html) in [
                (
                    "/my-post",
                    r#"<h1 class="entry-title">Title</h1><div class="main-img"><a href="https://vanilla-rock.com/img/one.jpg">Photo</a></div>"#,
                ),
                (
                    "/tag/animals/",
                    r#"<h2 class="entry-title"><a href="/one">One</a></h2><a class="next page-numbers" href="/tag/animals/page/2/">Next</a>"#,
                ),
                (
                    "/tag/animals/page/2/",
                    r#"<h2 class="entry-title"><a href="/one">Repeat</a></h2><h2 class="entry-title"><a href="/two">Two</a></h2>"#,
                ),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = [0u8; 4096];
                let size = socket.read(&mut bytes).await.unwrap();
                assert!(String::from_utf8_lossy(&bytes[..size])
                    .starts_with(&format!("GET {path} HTTP/1.1")));
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
                    html.len()
                );
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{address}/")).unwrap();
        assert_eq!(
            post_at_base(&Client::new(), &base, "my-post")
                .await
                .unwrap()
                .len(),
            1
        );
        let children = listing_at_base(&Client::new(), &base, "tag", "animals", 4)
            .await
            .unwrap();
        assert_eq!(children.len(), 2);
        server.await.unwrap();
    }
}
