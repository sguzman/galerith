//! Guarded public Issuu publication preview against the pinned issuu.py.
pub mod publisher;
use crate::{
    message::{media_metadata, ExtractorMessage, Metadata},
    MediaItem,
};
use reqwest::Client;
use serde_json::Value;
use std::error::Error;
use url::Url;

const MAX_HTML: usize = 2 * 1024 * 1024;
const MAX_PAGES: u64 = 240;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Publication {
    user: String,
    slug: String,
}

fn valid_slug(s: &str) -> bool {
    (1..=96).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub fn match_url(raw: &str) -> Option<Publication> {
    let lower = raw.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "/../", "/./"]
        .iter()
        .any(|s| lower.contains(s))
    {
        return None;
    }
    let url = Url::parse(raw).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !matches!(url.host_str(), Some("issuu.com" | "www.issuu.com"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let segments: Vec<_> = url.path_segments()?.collect();
    match segments.as_slice() {
        [user, "docs", slug] | [user, "docs", slug, ""] if valid_slug(user) && valid_slug(slug) => {
            Some(Publication {
                user: (*user).to_owned(),
                slug: (*slug).to_owned(),
            })
        }
        _ => None,
    }
}

fn find_document(value: &Value, depth: usize) -> Option<&Value> {
    if depth > 12 {
        return None;
    }
    if let Some(doc) = value.pointer("/initialDocumentData/document") {
        return Some(doc);
    }
    match value {
        Value::Array(items) => items.iter().find_map(|v| find_document(v, depth + 1)),
        Value::Object(fields) => fields.values().find_map(|v| find_document(v, depth + 1)),
        _ => None,
    }
}

fn parse_document(html: &str) -> Option<Value> {
    // The pinned site embeds a JSON hydration object and sometimes escapes its
    // quotation marks inside a script string. Do not evaluate page scripts.
    for script in html.split("<script").skip(1).take(48) {
        let Some((_, rest)) = script.split_once('>') else {
            continue;
        };
        let Some((content, _)) = rest.split_once("</script>") else {
            continue;
        };
        for decoded in [content.to_owned(), content.replace("\\\"", "\"")] {
            if let Ok(root) = serde_json::from_str::<Value>(&decoded) {
                if let Some(doc) = find_document(&root, 0) {
                    return Some(doc.clone());
                }
            }
            // The documentTextVersion hydration segment can be embedded in a
            // larger JS expression. Scan only balanced JSON roots, not JS.
            for (offset, _) in decoded.match_indices("documentTextVersion").take(8) {
                for (start, _) in decoded[..offset].match_indices('{').rev().take(16) {
                    let mut depth = 0_u32;
                    let mut quoted = false;
                    let mut escape = false;
                    let mut end = None;
                    for (index, ch) in decoded[start..].char_indices() {
                        if escape {
                            escape = false;
                            continue;
                        }
                        if quoted && ch == '\\' {
                            escape = true;
                            continue;
                        }
                        if ch == '"' {
                            quoted = !quoted;
                            continue;
                        }
                        if !quoted {
                            if ch == '{' {
                                depth += 1;
                            }
                            if ch == '}' {
                                depth -= 1;
                                if depth == 0 {
                                    end = Some(start + index + 1);
                                    break;
                                }
                            }
                        }
                    }
                    let Some(stop) = end else { continue };
                    let Some(raw) = decoded.get(start..stop) else {
                        continue;
                    };
                    if let Ok(root) = serde_json::from_str::<Value>(raw) {
                        if let Some(doc) = find_document(&root, 0) {
                            return Some(doc.clone());
                        }
                    }
                }
            }
        }
    }
    None
}

fn bounded_id(document: &Value, name: &str) -> Option<&str> {
    let id = document.get(name)?.as_str()?;
    ((1..=96).contains(&id.len())
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'))
    .then_some(id)
}

fn records(
    html: &str,
    publication: &Publication,
) -> Result<Vec<(MediaItem, Metadata)>, Box<dyn Error + Send + Sync>> {
    let doc = parse_document(html)
        .ok_or("Issuu public page lacks supported source hydration metadata")?;
    let revision = bounded_id(&doc, "revisionId").ok_or("Issuu public revision ID invalid")?;
    let id = bounded_id(&doc, "publicationId").ok_or("Issuu public publication ID invalid")?;
    let count = doc
        .get("pageCount")
        .and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse::<u64>().ok()))
        .ok_or("Issuu public document page count unavailable")?;
    if !(1..=MAX_PAGES).contains(&count) {
        return Err("Issuu document exceeds supported page bounds".into());
    }
    if doc
        .get("username")
        .and_then(Value::as_str)
        .is_some_and(|name| name != publication.user)
    {
        return Err("Issuu document owner differs from URL owner".into());
    }
    let title = doc
        .get("title")
        .and_then(Value::as_str)
        .filter(|s| s.len() <= 240 && !s.chars().any(char::is_control))
        .unwrap_or(&publication.slug);
    let post_url = format!(
        "https://issuu.com/{}/docs/{}",
        publication.user, publication.slug
    );
    let mut result = Vec::with_capacity(count as usize);
    for num in 1..=count {
        let item = MediaItem {
            url: format!("https://image.isu.pub/{revision}-{id}/jpg/page_{num}.jpg"),
            filename: format!("{id}_{num:03}.jpg"),
            extractor: "issuu",
            post_id: None,
        };
        let mut metadata = media_metadata(&item);
        metadata.insert("subcategory".into(), "publication".into());
        metadata.insert("publication_id".into(), id.into());
        metadata.insert("revision_id".into(), revision.into());
        metadata.insert("username".into(), publication.user.clone().into());
        metadata.insert("title".into(), title.into());
        metadata.insert("num".into(), num.into());
        metadata.insert("count".into(), count.into());
        metadata.insert("post_url".into(), post_url.clone().into());
        if let Some(date) = doc
            .get("originalPublishDateInISOString")
            .and_then(Value::as_str)
            .filter(|s| s.len() <= 48 && !s.chars().any(char::is_control))
        {
            metadata.insert("original_publish_date".into(), date.into());
        }
        result.push((item, metadata));
    }
    Ok(result)
}

async fn at_base(
    client: &Client,
    base: &Url,
    publication: &Publication,
) -> Result<Vec<(MediaItem, Metadata)>, Box<dyn Error + Send + Sync>> {
    let endpoint = base.join(&format!("{}/docs/{}", publication.user, publication.slug))?;
    let mut response = client
        .get(endpoint.clone())
        .send()
        .await?
        .error_for_status()?;
    if response.url().origin() != endpoint.origin() {
        return Err("Issuu public page redirected outside its origin".into());
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_HTML as u64)
    {
        return Err("Issuu source HTML exceeds supported size limit".into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_HTML.saturating_sub(body.len()) {
            return Err("Issuu source HTML exceeds supported size limit".into());
        }
        body.extend_from_slice(&chunk);
    }
    records(std::str::from_utf8(&body)?, publication)
}

pub async fn resolve(
    client: &Client,
    publication: Publication,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse("https://issuu.com/")?;
    Ok(at_base(client, &base, &publication)
        .await?
        .into_iter()
        .map(|(item, _)| item)
        .collect())
}

pub async fn resolve_messages(
    client: &Client,
    publication: Publication,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse("https://issuu.com/")?;
    let mut events = Vec::new();
    for (item, metadata) in at_base(client, &base, &publication).await? {
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

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    fn fixture() -> &'static str {
        r#"<script type="application/json">{"props":{"initialDocumentData":{"document":{"revisionId":"rev12","publicationId":"pub34","pageCount":3,"username":"press","title":"Autumn Review","originalPublishDateInISOString":"2026-09-10T00:00:00Z"}}}}</script>"#
    }

    #[test]
    fn pinned_publication_routes_reject_unsafe_inputs() {
        assert_eq!(
            match_url("https://issuu.com/press/docs/fall-2026")
                .unwrap()
                .slug,
            "fall-2026"
        );
        assert!(match_url("https://www.issuu.com/press/docs/fall-2026/").is_some());
        for bad in [
            "https://fakeissuu.com/press/docs/book",
            "https://issuu.com/press/docs/book?private=1",
            "https://issuu.com/press/docs/../../private",
            "https://name:pass@issuu.com/press/docs/book",
            "https://issuu.com:8443/press/docs/book",
            "https://issuu.com/press/docs/unsafe%2fpath",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn validated_hydration_creates_canonical_media_pages_with_source_metadata() {
        let publication = match_url("https://issuu.com/press/docs/fall-2026").unwrap();
        let pages = records(fixture(), &publication).unwrap();
        assert_eq!(pages.len(), 3);
        assert_eq!(
            pages[0].0.url,
            "https://image.isu.pub/rev12-pub34/jpg/page_1.jpg"
        );
        assert_eq!(pages[2].0.filename, "pub34_003.jpg");
        assert_eq!(pages[1].1["num"], 2);
        assert_eq!(pages[0].1["title"], "Autumn Review");
        assert_eq!(pages[0].1["original_publish_date"], "2026-09-10T00:00:00Z");
        assert!(pages.iter().all(|(item, _)| item.post_id.is_none()));
        assert!(records(&fixture().replace("press", "different"), &publication).is_err());
        assert!(records(
            &fixture().replace("\"pageCount\":3", "\"pageCount\":3000"),
            &publication
        )
        .is_err());
    }

    #[tokio::test]
    async fn bounded_public_html_uses_one_get_for_document() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = [0u8; 4096];
            let bytes_read = socket.read(&mut buffer).await.unwrap();
            assert!(String::from_utf8_lossy(&buffer[..bytes_read])
                .starts_with("GET /press/docs/fall-2026 HTTP/1.1"));
            let html = fixture();
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
                html.len()
            );
            socket.write_all(reply.as_bytes()).await.unwrap();
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let publication = match_url("https://issuu.com/press/docs/fall-2026").unwrap();
        let pages = at_base(&Client::new(), &base, &publication).await.unwrap();
        assert_eq!(pages.len(), 3);
        server.await.unwrap();
    }
}
