//! Bounded Issuu public publisher-to-publication queues from pinned issuu.py.
use crate::message::{queued_url, ExtractorMessage, Metadata};
use reqwest::Client;
use serde_json::Value;
use std::{collections::HashSet, error::Error, time::Duration};
use url::Url;

const MAX_HTML: usize = 2 * 1024 * 1024;
const MAX_USER_PAGES: u16 = 4;
const MAX_CHILDREN: usize = 144;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Publisher {
    user: String,
    start: u16,
}

pub fn match_url(raw: &str) -> Option<Publisher> {
    let lower = raw.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "/../", "/./"]
        .iter()
        .any(|bad| lower.contains(bad))
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
    let (user, start) = match segments.as_slice() {
        [user] | [user, ""] => (*user, 1_u16),
        [user, page] => (*user, page.parse().ok()?),
        _ => return None,
    };
    if !super::valid_slug(user) || !(1..=MAX_USER_PAGES).contains(&start) {
        return None;
    }
    Some(Publisher {
        user: user.to_owned(),
        start,
    })
}

fn docs_node(value: &Value, depth: u8) -> Option<&Vec<Value>> {
    if depth > 12 {
        return None;
    }
    if let Some(docs) = value.get("docs").and_then(Value::as_array) {
        return Some(docs);
    }
    match value {
        Value::Array(items) => items.iter().find_map(|value| docs_node(value, depth + 1)),
        Value::Object(obj) => obj.values().find_map(|value| docs_node(value, depth + 1)),
        _ => None,
    }
}

fn parse_docs(html: &str) -> Option<Vec<Value>> {
    for script in html.split("<script").skip(1).take(48) {
        let Some((_, rest)) = script.split_once('>') else {
            continue;
        };
        let Some((body, _)) = rest.split_once("</script>") else {
            continue;
        };
        for decoded in [body.to_owned(), body.replace("\\\"", "\"")] {
            if let Ok(value) = serde_json::from_str::<Value>(&decoded) {
                if let Some(docs) = docs_node(&value, 0) {
                    return Some(docs.clone());
                }
            }
        }
    }
    None
}

fn decode_user_page(
    html: &str,
    publisher: &Publisher,
    seen: &mut HashSet<String>,
    events: &mut Vec<ExtractorMessage>,
) -> Result<usize, Box<dyn Error + Send + Sync>> {
    let docs =
        parse_docs(html).ok_or("Issuu public publisher page lacks supported document metadata")?;
    if docs.len() > 48 {
        return Err("Issuu publisher source page exceeds the pinned per-page limit".into());
    }
    for doc in &docs {
        if events.len() >= MAX_CHILDREN {
            break;
        }
        let Some(uri) = doc.get("uri").and_then(Value::as_str) else {
            continue;
        };
        let parts: Vec<_> = uri.trim_start_matches('/').split('/').collect();
        let [user, "docs", slug] = parts.as_slice() else {
            continue;
        };
        if *user != publisher.user || !super::valid_slug(slug) {
            continue;
        }
        let url = format!("https://issuu.com/{user}/docs/{slug}");
        if !seen.insert(url.clone()) {
            continue;
        }
        let mut metadata = Metadata::new();
        metadata.insert("category".into(), "issuu".into());
        metadata.insert("subcategory".into(), "user".into());
        metadata.insert("username".into(), publisher.user.clone().into());
        metadata.insert("publication_slug".into(), (*slug).into());
        events.push(queued_url(url, metadata));
    }
    Ok(docs.len())
}

async fn page_html(client: &Client, endpoint: Url) -> Result<String, Box<dyn Error + Send + Sync>> {
    let mut response = client
        .get(endpoint.clone())
        .send()
        .await?
        .error_for_status()?;
    if response.url().origin() != endpoint.origin() {
        return Err("Issuu publisher page redirected outside origin".into());
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_HTML as u64)
    {
        return Err("Issuu publisher HTML exceeds configured size limit".into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_HTML.saturating_sub(body.len()) {
            return Err("Issuu publisher HTML exceeds configured size limit".into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8(body)?)
}

async fn at_base(
    client: &Client,
    base: &Url,
    publisher: &Publisher,
    max_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    let end = publisher
        .start
        .saturating_add(max_pages.clamp(1, MAX_USER_PAGES))
        .min(MAX_USER_PAGES + 1);
    for page in publisher.start..end {
        if page > publisher.start {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        let path = if page == 1 {
            publisher.user.clone()
        } else {
            format!("{}/{page}", publisher.user)
        };
        let body = page_html(client, base.join(&path)?).await?;
        let count = decode_user_page(&body, publisher, &mut seen, &mut result)?;
        if count < 48 || result.len() >= MAX_CHILDREN {
            break;
        }
    }
    Ok(result)
}

pub async fn resolve_messages(
    client: &Client,
    publisher: Publisher,
    max_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse("https://issuu.com/")?;
    at_base(client, &base, &publisher, max_pages).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    #[test]
    fn publisher_routes_reject_credentials_unsafe_slugs_and_unbounded_pages() {
        assert_eq!(match_url("https://issuu.com/press").unwrap().start, 1);
        assert_eq!(match_url("https://issuu.com/press/2").unwrap().start, 2);
        for bad in [
            "https://evilissuu.com/press",
            "https://issuu.com/press/5",
            "https://issuu.com/press/0",
            "https://issuu.com/press?private=1",
            "https://issuu.com/press/%2e%2e",
            "https://user:password@issuu.com/press",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn publisher_deduplicates_scoped_public_documents_and_preserves_queue_metadata() {
        let publisher = match_url("https://issuu.com/press").unwrap();
        let html = r#"<script type="application/json">{"props":{"docs":[{"uri":"press/docs/a"},{"uri":"press/docs/a"},{"uri":"press/docs/b"},{"uri":"other/docs/c"},{"uri":"press/docs/../invalid"}]}}</script>"#;
        let mut seen = HashSet::new();
        let mut result = Vec::new();
        assert_eq!(
            decode_user_page(html, &publisher, &mut seen, &mut result).unwrap(),
            5
        );
        assert_eq!(result.len(), 2);
        let first = serde_json::to_value(&result[0]).unwrap();
        assert_eq!(first["url"], "https://issuu.com/press/docs/a");
        assert_eq!(first["metadata"]["username"], "press");
        assert_eq!(first["metadata"]["publication_slug"], "a");
    }

    #[tokio::test]
    async fn publisher_stops_after_a_short_public_document_page() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 4096];
            let n = socket.read(&mut request).await.unwrap();
            assert!(String::from_utf8_lossy(&request[..n]).starts_with("GET /press HTTP/1.1"));
            let html = r#"<script>{"props":{"docs":[{"uri":"press/docs/a"},{"uri":"press/docs/b"}]}}</script>"#;
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
                html.len()
            );
            socket.write_all(reply.as_bytes()).await.unwrap();
        });
        let pubr = match_url("https://issuu.com/press").unwrap();
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let events = at_base(&Client::new(), &base, &pubr, 4).await.unwrap();
        assert_eq!(events.len(), 2);
        server.await.unwrap();
    }
}
