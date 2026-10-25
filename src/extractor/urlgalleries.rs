//! Guarded public Urlgalleries album -> image-page queue subset of pinned urlgalleries.py.
//! The source exposes external originals; queue traversal is opt-in.
use crate::message::{queued_url, ExtractorMessage, Metadata};
use reqwest::{redirect::Policy, Client};
use std::{error::Error, time::Duration};
use url::{Host, Url};

const MAX_HTML: usize = 2 * 1024 * 1024;
const MAX_CHILDREN: usize = 48;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gallery {
    blog: String,
    id: String,
    slug: Option<String>,
}

fn safe_slug(text: &str) -> bool {
    (1..=96).contains(&text.len())
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

fn valid_id(id: &str) -> bool {
    (1..=16).contains(&id.len())
        && id.bytes().all(|b| b.is_ascii_digit())
        && id.parse::<u64>().ok().is_some_and(|n| n > 0)
}

pub fn match_url(raw: &str) -> Option<Gallery> {
    let lower = raw.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "/../", "/./"]
        .iter()
        .any(|v| lower.contains(v))
    {
        return None;
    }
    let url = Url::parse(raw).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let host = url.host_str()?;
    if host != "urlgalleries.com" && host != "www.urlgalleries.com" {
        let prefix = host.strip_suffix(".urlgalleries.com")?;
        if !safe_slug(prefix) || prefix.contains('.') {
            return None;
        }
    }
    let segs: Vec<_> = url.path_segments()?.collect();
    let (blog, id, slug) = match segs.as_slice() {
        [blog, id] | [blog, id, ""] => (*blog, *id, None),
        [blog, id, slug] | [blog, id, slug, ""] => (*blog, *id, Some(*slug)),
        _ => return None,
    };
    if !safe_slug(blog) || !valid_id(id) || slug.is_some_and(|s| !safe_slug(s)) {
        return None;
    }
    Some(Gallery {
        blog: blog.into(),
        id: id.into(),
        slug: slug.map(ToOwned::to_owned),
    })
}

fn field<'a>(html: &'a str, start: &str, end: &str) -> Option<&'a str> {
    html.split_once(start)?.1.split_once(end).map(|(v, _)| v)
}

fn text_field(raw: &str) -> Option<String> {
    if raw.len() > 240 || raw.chars().any(char::is_control) {
        return None;
    }
    Some(
        raw.trim()
            .replace("&amp;", "&")
            .replace("&quot;", "\"")
            .replace("&#39;", "'"),
    )
}

fn source_metadata(html: &str, gallery: &Gallery) -> Metadata {
    let mut meta = Metadata::new();
    meta.insert("category".into(), "urlgalleries".into());
    meta.insert("subcategory".into(), "gallery".into());
    meta.insert("gallery_id".into(), gallery.id.clone().into());
    meta.insert("blog".into(), gallery.blog.clone().into());
    for (key, before, after) in [
        ("date_source", "\"datePublished\":\"", "\""),
        (
            "title",
            "\"@type\":\"ListItem\",\"position\":3,\"name\":\"",
            "\"",
        ),
        (
            "source_blog",
            "\"@type\":\"ListItem\",\"position\":2,\"name\":\"",
            "\"",
        ),
        (
            "description",
            "property=\"og:description\" content=\"",
            "\"",
        ),
    ] {
        if let Some(v) = field(html, before, after).and_then(text_field) {
            meta.insert(key.into(), v.into());
        }
    }
    meta
}

fn scoped_page(root: &Url, raw: &str) -> Option<Url> {
    let url = root.join(raw).ok()?;
    if url.origin() != root.origin()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path().contains('%')
        || url.path().contains("..")
    {
        return None;
    }
    Some(url)
}

fn image_pages(html: &str, root: &Url) -> Result<Vec<Url>, Box<dyn Error + Send + Sync>> {
    let section = field(html, "<section ", "</section>")
        .ok_or("Urlgalleries public album lacks a source gallery section")?;
    let mut pages = Vec::new();
    for chunk in section.split(" href=\"").skip(1) {
        let Some((raw, _)) = chunk.split_once('"') else {
            continue;
        };
        let endpoint = scoped_page(root, raw)
            .ok_or("Urlgalleries source image page has a foreign or unsafe URL")?;
        if pages.len() >= MAX_CHILDREN {
            return Err("Urlgalleries public album exceeds bounded image count".into());
        }
        pages.push(endpoint);
    }
    if pages.is_empty() {
        return Err("Urlgalleries source album lacks public image links".into());
    }
    Ok(pages)
}

fn trusted_external(raw: &str) -> Option<String> {
    let url = Url::parse(raw).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let Some(Host::Domain(host)) = url.host() else {
        return None;
    };
    if !host.contains('.')
        || host.len() > 253
        || [
            "localhost",
            ".localhost",
            ".local",
            ".internal",
            ".onion",
            ".lan",
            ".test",
        ]
        .iter()
        .any(|suffix| host == *suffix || host.ends_with(suffix))
    {
        return None;
    }
    Some(url.to_string())
}

async fn read_html(client: &Client, endpoint: Url) -> Result<String, Box<dyn Error + Send + Sync>> {
    let mut response = client.get(endpoint.clone()).send().await?;
    if response.url().origin() != endpoint.origin() || response.status().is_redirection() {
        return Err("Urlgalleries public source redirected outside origin".into());
    }
    response = response.error_for_status()?;
    if response
        .content_length()
        .is_some_and(|n| n > MAX_HTML as u64)
    {
        return Err("Urlgalleries source HTML exceeds byte bound".into());
    }
    let mut data = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_HTML.saturating_sub(data.len()) {
            return Err("Urlgalleries source HTML exceeds byte bound".into());
        }
        data.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8(data)?)
}

async fn at_base(
    client: &Client,
    gallery: &Gallery,
    base: &Url,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let path = format!(
        "{}/{}{}/",
        gallery.blog,
        gallery.id,
        gallery
            .slug
            .as_ref()
            .map(|s| format!("/{s}"))
            .unwrap_or_default()
    );
    let mut first = base.join(&path)?;
    first.query_pairs_mut().append_pair("a", "10000");
    let html = read_html(client, first).await?;
    let source_pages = image_pages(&html, base)?;
    let parent = source_metadata(&html, gallery);
    let mut result = Vec::with_capacity(source_pages.len() + 1);
    let mut dir = parent.clone();
    dir.insert("count".into(), source_pages.len().into());
    result.push(ExtractorMessage::Directory {
        metadata: dir.clone(),
    });
    for (i, page) in source_pages.iter().enumerate() {
        if i > 0 {
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        let html = read_html(client, page.clone()).await?;
        let original = field(&html, " IMAGE_URL = \"", "\"")
            .ok_or("Urlgalleries image page lacks an original URL")?;
        let url = trusted_external(original)
            .ok_or("Urlgalleries image original has an untrusted destination")?;
        let mut metadata = dir.clone();
        metadata.insert("num".into(), (i + 1).into());
        metadata.insert("page_url".into(), page.to_string().into());
        result.push(queued_url(url, metadata));
    }
    Ok(result)
}

pub async fn resolve_messages(
    _client: &Client,
    gallery: Gallery,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let client = Client::builder()
        .redirect(Policy::none())
        .timeout(Duration::from_secs(15))
        .build()?;
    at_base(&client, &gallery, &Url::parse("https://urlgalleries.com/")?).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    fn input() -> Gallery {
        match_url("https://urlgalleries.com/blog/12345/title").unwrap()
    }

    #[test]
    fn pinned_public_gallery_routes_reject_forged_sources_and_queries() {
        assert_eq!(input().id, "12345");
        assert!(match_url("https://demo.urlgalleries.com/blog/12345/title").is_some());
        for bad in [
            "https://evil-urlgalleries.com/blog/12345/title",
            "https://urlgalleries.com/blog/0/title",
            "https://urlgalleries.com/blog/no-id/title",
            "https://urlgalleries.com/blog/12345?secret=1",
            "https://urlgalleries.com/blog/12345/../../private",
            "https://name:pass@urlgalleries.com/blog/12345",
            "https://urlgalleries.com:8443/blog/12345/title",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn gallery_section_extracts_ordered_first_party_image_pages() {
        let html = r#"<section class="images"><a href="/blog/12345/image-1">one</a><a href="/blog/12345/image-2">two</a></section>"#;
        let base = Url::parse("https://urlgalleries.com/").unwrap();
        let pages = image_pages(html, &base).unwrap();
        assert_eq!(pages.len(), 2);
        assert_eq!(pages[0].path(), "/blog/12345/image-1");
        assert!(image_pages(r#"<section class="images"><a href="https://evil-urlgalleries.com/foreign">x</a></section>"#,&base).is_err());
        assert!(trusted_external("https://image.example.org/photo.jpg").is_some());
        assert!(trusted_external("http://127.0.0.1/private").is_none());
        assert!(trusted_external("https://example.local/file.png").is_none());
    }

    #[tokio::test]
    async fn bounded_public_gallery_and_image_pages_emit_typed_original_queues() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (path, body) in [
                (
                    "/blog/12345/title/?a=10000",
                    r#"<section class="images"><a href="/blog/12345/image-1">one</a><a href="/blog/12345/image-2">two</a></section>"#,
                ),
                (
                    "/blog/12345/image-1",
                    r#" IMAGE_URL = "https://img.example.org/photo-one.jpg""#,
                ),
                (
                    "/blog/12345/image-2",
                    r#" IMAGE_URL = "https://img.example.org/photo-two.png""#,
                ),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0u8; 4096];
                let n = socket.read(&mut request).await.unwrap();
                assert!(String::from_utf8_lossy(&request[..n])
                    .starts_with(&format!("GET {path} HTTP/1.1")));
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{address}/")).unwrap();
        let events = at_base(&Client::new(), &input(), &base).await.unwrap();
        assert_eq!(events.len(), 3);
        let queued = serde_json::to_value(&events[2]).unwrap();
        assert_eq!(queued["type"], "queue");
        assert_eq!(queued["url"], "https://img.example.org/photo-two.png");
        assert_eq!(queued["metadata"]["num"], 2);
        server.await.unwrap();
    }
}
