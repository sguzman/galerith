//! Independent, bounded local-file subset of pinned gallery-dl recursive.py.
//! Only explicit r:file:// / recursive:file:// inputs are accepted; remote
//! list fetching, unbounded input and automatic downloads are NOT implemented.
use crate::message::{queued_url, ExtractorMessage, Metadata};
use std::{error::Error, path::PathBuf};
use tokio::io::AsyncReadExt;
use url::{Host, Url};

const MAX_BYTES: usize = 2 * 1024 * 1024;
const MAX_CHILDREN: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalList {
    path: PathBuf,
}

pub fn match_url(raw: &str) -> Option<LocalList> {
    let source = raw
        .strip_prefix("recursive:")
        .or_else(|| raw.strip_prefix("r:"))?;
    let lower = source.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "/../", "/./"]
        .iter()
        .any(|term| lower.contains(term))
    {
        return None;
    }
    let url = Url::parse(source).ok()?;
    if url.scheme() != "file"
        || url.query().is_some()
        || url.fragment().is_some()
        || url.host_str().is_some_and(|host| !host.is_empty())
    {
        return None;
    }
    Some(LocalList {
        path: url.to_file_path().ok()?,
    })
}

/// Keep the pinned extractor's first-seen occurrence order, including
/// repeated links. Invalid/unsafe URLs are not enqueued.
fn decode_links(source: &str) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let mut events = Vec::new();
    let mut cursor = 0;
    while cursor < source.len() {
        let tail = &source[cursor..];
        let next_http = tail.find("http://");
        let next_https = tail.find("https://");
        let Some(next) = (match (next_http, next_https) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) | (None, Some(a)) => Some(a),
            (None, None) => None,
        }) else {
            break;
        };
        let start = cursor + next;
        let bytes = &source.as_bytes()[start..];
        let width = bytes
            .iter()
            .position(|byte| byte.is_ascii_whitespace() || matches!(byte, b'"' | b'\'' | b'<' | b'>'))
            .unwrap_or(bytes.len());
        if width == 0 {
            return Err("Recursive URL cursor did not advance".into());
        }
        cursor = start + width;
        let Some(raw) = source.get(start..cursor) else { continue };
        let Ok(parsed) = Url::parse(raw) else { continue };
        if !matches!(parsed.scheme(), "http" | "https")
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.port().is_some()
            || !matches!(parsed.host(), Some(Host::Domain(host)) if
                !host.eq_ignore_ascii_case("localhost")
                && !host.ends_with(".localhost")
                && !host.ends_with(".local")
                && !host.ends_with(".internal"))
        {
            continue;
        }
        if events.len() >= MAX_CHILDREN {
            return Err("Recursive URL list exceeds bounded child count".into());
        }
        let mut metadata = Metadata::new();
        metadata.insert("category".into(), "recursive".into());
        metadata.insert("subcategory".into(), "file".into());
        metadata.insert("ordinal".into(), (events.len() + 1).into());
        events.push(queued_url(raw.to_owned(), metadata));
    }
    Ok(events)
}

pub async fn resolve_messages(
    source: LocalList,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let file = tokio::fs::File::open(&source.path).await?;
    let mut bytes = Vec::new();
    file.take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .await?;
    if bytes.len() > MAX_BYTES {
        return Err("Recursive local URL list exceeds 2 MiB".into());
    }
    let text = String::from_utf8(bytes)?;
    decode_links(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_local_recursive_urls_reject_remote_schemes_and_traversal() {
        let result = match_url("recursive:file:///tmp/galerith-source.txt").unwrap();
        assert_eq!(result.path, PathBuf::from("/tmp/galerith-source.txt"));
        assert!(match_url("r:file:///tmp/urls.txt").is_some());
        for bad in [
            "recursive:https://example.org/list.txt",
            "r:http://example.org/list.txt",
            "r:file:///tmp/../secret.txt",
            "r:file:///tmp/%2e%2e/private.txt",
            "r:file://elsewhere.example.org/tmp/list.txt",
            "r:file:///tmp/urls.txt?key=secret",
            "r:file:///tmp/urls.txt#fragment",
            "recursive:",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn ordered_url_file_preserves_duplicates_and_suppresses_unsafe_origins() {
        let content = "https://example.org/one.jpg\nhttps://example.org/one.jpg\n\
            http://127.0.0.1/private.jpg\nhttps://localhost/private\n\
            https://user:password@example.org/hidden.png\n\
            <a href=\"https://example.org/two.html\">Two</a>";
        let events = decode_links(content).unwrap();
        assert_eq!(events.len(), 3);
        let first = serde_json::to_value(&events[0]).unwrap();
        let last = serde_json::to_value(&events[2]).unwrap();
        assert_eq!(first["type"], "queue");
        assert_eq!(first["url"], "https://example.org/one.jpg");
        assert_eq!(last["url"], "https://example.org/two.html");
        assert_eq!(last["metadata"]["ordinal"], 3);
    }

    #[tokio::test]
    async fn local_file_is_bounded_and_queues_only_read_only_links() {
        let mut path = std::env::temp_dir();
        path.push(format!("galerith-recursive-fixture-{}.txt", std::process::id()));
        tokio::fs::write(&path, "https://example.org/first\nhttps://example.org/second\n")
            .await.unwrap();
        let url = format!("r:{}", Url::from_file_path(&path).unwrap());
        let events = resolve_messages(match_url(&url).unwrap()).await.unwrap();
        assert_eq!(events.len(), 2);
        tokio::fs::write(&path, vec![b'a'; MAX_BYTES + 1]).await.unwrap();
        assert!(resolve_messages(match_url(&url).unwrap()).await.is_err());
        tokio::fs::remove_file(path).await.unwrap();
    }
}
