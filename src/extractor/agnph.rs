//! Guarded public Agn.ph XML posts and tagged search subset (pinned agnph.py).
use crate::{
    message::{media_metadata, ExtractorMessage, Metadata},
    safe_filename, MediaItem,
};
use reqwest::Client;
use roxmltree::{Document, Node};
use std::{collections::HashSet, error::Error, time::Duration};
use url::Url;

const MAX_XML: usize = 2 * 1024 * 1024;
const MAX_PAGES: u16 = 4;
const MAX_POSTS: usize = 45;
const MAX_MEDIA: usize = 160;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Post(u64),
    Search { tags: String, start: u16 },
}

fn positive(s: &str) -> Option<u64> {
    let n = s.parse::<u64>().ok()?;
    (n > 0 && s.len() <= 18 && s.bytes().all(|b| b.is_ascii_digit())).then_some(n)
}

fn safe_search(s: &str) -> bool {
    s.len() <= 150
        && !s.chars().any(char::is_control)
        && !s.contains('/')
        && !s.contains('\\')
        && !s.contains('%')
}

pub fn match_url(raw: &str) -> Option<Target> {
    let candidate = if raw.starts_with("agn.ph/") {
        format!("https://{raw}")
    } else {
        raw.to_owned()
    };
    let lower = candidate.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "/../", "/./"]
        .iter()
        .any(|s| lower.contains(s))
    {
        return None;
    }
    let u = Url::parse(&candidate).ok()?;
    if !matches!(u.scheme(), "http" | "https")
        || u.host_str() != Some("agn.ph")
        || u.port().is_some()
        || !u.username().is_empty()
        || u.password().is_some()
        || u.fragment().is_some()
    {
        return None;
    }
    let paths: Vec<_> = u.path_segments()?.collect();
    match paths.as_slice() {
        ["gallery", "post", "show", id] | ["gallery", "post", "show", id, ""] => {
            if u.query().is_none() {
                positive(id).map(Target::Post)
            } else {
                None
            }
        }
        ["gallery", "post"] | ["gallery", "post", ""] => {
            let mut tags = String::new();
            let mut page = 1;
            if u.query().is_some() {
                for (k, v) in u.query_pairs() {
                    match k.as_ref() {
                        "search" if tags.is_empty() => {
                            if !safe_search(&v) {
                                return None;
                            }
                            tags = v.into_owned();
                        }
                        "page" if page == 1 => {
                            page = v.parse().ok()?;
                        }
                        _ => return None,
                    }
                }
            }
            (1..=MAX_PAGES)
                .contains(&page)
                .then_some(Target::Search { tags, start: page })
        }
        _ => None,
    }
}

async fn xml(client: &Client, endpoint: Url) -> Result<String, Box<dyn Error + Send + Sync>> {
    let mut response = client
        .get(endpoint.clone())
        .send()
        .await?
        .error_for_status()?;
    if response.url().origin() != endpoint.origin() {
        return Err("Agn.ph XML API redirected outside origin".into());
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_XML as u64)
    {
        return Err("Agn.ph XML exceeds byte cap".into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_XML.saturating_sub(body.len()) {
            return Err("Agn.ph XML exceeds byte cap".into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8(body)?)
}

fn field<'a>(post: Node<'a, 'a>, name: &str) -> Option<&'a str> {
    post.children().find(|n| n.has_tag_name(name))?.text()
}

fn media(raw: &str) -> Option<(String, String)> {
    let lower = raw.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "/../", "/./"]
        .iter()
        .any(|s| lower.contains(s))
    {
        return None;
    }
    let u = Url::parse(raw).ok()?;
    let host = u.host_str()?;
    if u.scheme() != "https"
        || !(host == "agn.ph" || host.ends_with(".agn.ph"))
        || u.port().is_some()
        || !u.username().is_empty()
        || u.password().is_some()
        || u.query().is_some()
        || u.fragment().is_some()
    {
        return None;
    }
    let basename = u.path_segments()?.next_back()?;
    let ext = basename.rsplit_once('.')?.1.to_ascii_lowercase();
    if !matches!(
        ext.as_str(),
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif" | "mp4"
    ) {
        return None;
    }
    Some((u.to_string(), ext))
}

fn parse_post(
    post: Node<'_, '_>,
    search: &str,
    seen: &mut HashSet<u64>,
    out: &mut Vec<(MediaItem, Metadata)>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let Some(id) = field(post, "id").and_then(positive) else {
        return Ok(());
    };
    let Some(file) = field(post, "file_url").or_else(|| field(post, "original_url")) else {
        return Ok(());
    };
    let Some((url, ext)) = media(file) else {
        return Ok(());
    };
    if !seen.insert(id) {
        return Ok(());
    }
    if out.len() >= MAX_MEDIA {
        return Err("Agn.ph search exceeds bounded media count".into());
    }
    let item = MediaItem {
        url,
        filename: safe_filename(&format!("{id}.{ext}")),
        extractor: "agnph",
        post_id: Some(id),
    };
    let mut md = media_metadata(&item);
    md.insert(
        "subcategory".into(),
        if search.is_empty() { "post" } else { "tag" }.into(),
    );
    md.insert("id".into(), id.into());
    md.insert("search_tags".into(), search.into());
    md.insert("extension".into(), ext.into());
    for (key, tag) in [
        ("date_source", "created_at"),
        ("status", "status"),
        ("tags_source", "tags"),
    ] {
        if let Some(raw) =
            field(post, tag).filter(|s| s.len() <= 1500 && !s.chars().any(char::is_control))
        {
            md.insert(key.into(), raw.trim().into());
        }
    }
    out.push((item, md));
    Ok(())
}

fn parse_page(
    xml: &str,
    search: &str,
    seen: &mut HashSet<u64>,
    out: &mut Vec<(MediaItem, Metadata)>,
) -> Result<bool, Box<dyn Error + Send + Sync>> {
    let doc = Document::parse(xml)?;
    let root = doc.root_element();
    if root.has_tag_name("post") {
        parse_post(root, search, seen, out)?;
        return Ok(true);
    }
    if !root.has_tag_name("posts") {
        return Err("Agn.ph XML response has unrecognized root".into());
    }
    let posts: Vec<_> = root.children().filter(|n| n.has_tag_name("post")).collect();
    if posts.len() > MAX_POSTS {
        return Err("Agn.ph XML response exceeds 45 posts per page".into());
    }
    for post in &posts {
        parse_post(*post, search, seen, out)?;
    }
    let offset = root
        .attribute("offset")
        .and_then(|s| s.parse::<usize>().ok())
        .ok_or("Agn.ph XML offset missing")?;
    let count = root
        .attribute("count")
        .and_then(|s| s.parse::<usize>().ok())
        .ok_or("Agn.ph XML count missing")?;
    Ok(offset.saturating_add(posts.len()) >= count || posts.is_empty())
}

async fn at_base(
    client: &Client,
    base: &Url,
    target: Target,
    max_pages: u16,
) -> Result<Vec<(MediaItem, Metadata)>, Box<dyn Error + Send + Sync>> {
    let mut results = Vec::new();
    let mut seen = HashSet::new();
    match target {
        Target::Post(id) => {
            let mut url = base.join(&format!("gallery/post/show/{id}/"))?;
            url.query_pairs_mut().append_pair("api", "xml");
            let body = xml(client, url).await?;
            parse_page(&body, "", &mut seen, &mut results)?;
        }
        Target::Search { tags, start } => {
            let end = start
                .saturating_add(max_pages.clamp(1, MAX_PAGES))
                .min(MAX_PAGES + 1);
            for page in start..end {
                if page > start {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                let mut url = base.join("gallery/post/")?;
                url.query_pairs_mut()
                    .append_pair("search", &tags)
                    .append_pair("api", "xml")
                    .append_pair("page", &page.to_string());
                let body = xml(client, url).await?;
                if parse_page(&body, &tags, &mut seen, &mut results)? {
                    break;
                }
            }
        }
    }
    Ok(results)
}

pub async fn resolve(
    client: &Client,
    target: Target,
    max_pages: u16,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    Ok(
        at_base(client, &Url::parse("https://agn.ph/")?, target, max_pages)
            .await?
            .into_iter()
            .map(|(m, _)| m)
            .collect(),
    )
}
pub async fn resolve_messages(
    client: &Client,
    target: Target,
    max_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let mut result = Vec::new();
    for (item, metadata) in
        at_base(client, &Url::parse("https://agn.ph/")?, target, max_pages).await?
    {
        result.push(ExtractorMessage::Directory {
            metadata: metadata.clone(),
        });
        result.push(ExtractorMessage::Url {
            url: item.url,
            metadata,
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
    fn public_post_and_tag_routes_reject_bad_origin_parameters() {
        assert_eq!(
            match_url("https://agn.ph/gallery/post/show/12345/"),
            Some(Target::Post(12345))
        );
        assert_eq!(
            match_url("agn.ph/gallery/post/?search=fox"),
            Some(Target::Search {
                tags: "fox".into(),
                start: 1
            })
        );
        for raw in [
            "https://fakeagn.ph/gallery/post/show/10",
            "https://agn.ph/gallery/post/show/0",
            "https://agn.ph:8443/gallery/post/show/10",
            "https://agn.ph/gallery/post/?search=fox&page=5",
            "https://agn.ph/gallery/post/?search=fox&password=secret",
            "https://user:pass@agn.ph/gallery/post/show/10",
        ] {
            assert!(match_url(raw).is_none(), "{raw}");
        }
    }

    #[test]
    fn xml_post_fields_and_search_pagination_are_source_backed() {
        let xml = r#"<posts offset="0" count="2"><post><id>14</id><file_url>https://img.agn.ph/pictures/fox.jpg</file_url><created_at>1720000000</created_at><status> active </status></post><post><id>15</id><file_url>https://fake-agn.ph/invalid.jpg</file_url></post></posts>"#;
        let mut seen = HashSet::new();
        let mut files = Vec::new();
        assert!(parse_page(xml, "fox", &mut seen, &mut files).unwrap());
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].0.filename, "14.jpg");
        assert_eq!(files[0].1["search_tags"], "fox");
        assert_eq!(files[0].1["status"], "active");
        assert_eq!(files[0].1["date_source"], "1720000000");
    }

    #[tokio::test]
    async fn single_post_and_tag_xml_use_public_bounded_api_requests() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (path, body) in [
                (
                    "/gallery/post/show/14/?api=xml",
                    r#"<post><id>14</id><file_url>https://img.agn.ph/pictures/fox.jpg</file_url></post>"#,
                ),
                (
                    "/gallery/post/?search=fox&api=xml&page=1",
                    r#"<posts offset="0" count="1"><post><id>15</id><file_url>https://cdn.agn.ph/media/owl.png</file_url></post></posts>"#,
                ),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buf = [0u8; 4096];
                let n = socket.read(&mut buf).await.unwrap();
                assert!(
                    String::from_utf8_lossy(&buf[..n]).starts_with(&format!("GET {path} HTTP/1.1"))
                );
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        assert_eq!(
            at_base(&Client::new(), &base, Target::Post(14), 4)
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            at_base(
                &Client::new(),
                &base,
                Target::Search {
                    tags: "fox".into(),
                    start: 1
                },
                4
            )
            .await
            .unwrap()
            .len(),
            1
        );
        server.await.unwrap();
    }
}
