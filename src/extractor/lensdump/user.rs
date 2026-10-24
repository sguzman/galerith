//! Bounded public Lensdump user album discovery, independent pinned adapter.
use crate::message::{queued_url, ExtractorMessage, Metadata};
use reqwest::Client;
use std::{collections::HashSet, error::Error, time::Duration};
use url::Url;

const MAX_HTML_BYTES: usize = 2 * 1024 * 1024;
const MAX_PAGES: u16 = 4;
const MAX_CHILDREN: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub name: String,
    pub start: u16,
}

fn valid_user(s: &str) -> bool {
    (1..=80).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        && s != "a"
        && s != "i"
}

pub fn match_url(raw: &str) -> Option<User> {
    let lower = raw.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "/../", "/./"]
        .iter()
        .any(|bad| lower.contains(bad))
    {
        return None;
    }
    let url = Url::parse(raw).ok()?;
    if !matches!(url.scheme(), "https" | "http")
        || url.host_str() != Some("lensdump.com")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let paths: Vec<_> = url.path_segments()?.collect();
    let ([user] | [user, ""]) = paths.as_slice() else {
        return None;
    };
    if !valid_user(user) {
        return None;
    }
    let mut page = 1;
    if url.query().is_some() {
        let pairs: Vec<_> = url.query_pairs().collect();
        if pairs.len() != 2
            || pairs[0].0 != "sort"
            || pairs[0].1 != "date_asc"
            || pairs[1].0 != "page"
        {
            return None;
        }
        page = pairs[1].1.parse::<u16>().ok()?;
    }
    ((1..=MAX_PAGES).contains(&page)).then(|| User {
        name: (*user).to_owned(),
        start: page,
    })
}

fn album_link(raw: &str) -> Option<String> {
    let url = Url::parse("https://lensdump.com/").ok()?.join(raw).ok()?;
    if url.scheme() != "https"
        || url.host_str() != Some("lensdump.com")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let paths: Vec<_> = url.path_segments()?.collect();
    let (["a", id] | ["a", id, ""]) = paths.as_slice() else {
        return None;
    };
    if !super::safe_id(id) {
        return None;
    }
    Some(format!("https://lensdump.com/a/{id}"))
}

fn decode_page(
    html: &str,
    user: &User,
    seen: &mut HashSet<String>,
    result: &mut Vec<ExtractorMessage>,
) {
    for chunk in html.split("data-url-short=\"").skip(1).take(3200) {
        if result.len() >= MAX_CHILDREN {
            break;
        }
        let Some((href, _)) = chunk.split_once('"') else {
            continue;
        };
        let Some(url) = album_link(href) else {
            continue;
        };
        if !seen.insert(url.clone()) {
            continue;
        }
        let mut meta = Metadata::new();
        meta.insert("category".into(), "lensdump".into());
        meta.insert("subcategory".into(), "albums".into());
        meta.insert("username".into(), user.name.clone().into());
        meta.insert("album_id".into(), url.rsplit('/').next().unwrap().into());
        result.push(queued_url(url, meta));
    }
}

fn next_page(html: &str, base: &Url, user: &User, current: u16) -> Option<Url> {
    let next = html
        .split_once(" data-pagination=\"next\"")?
        .1
        .split_once('>')?
        .0;
    let raw = next.split_once("href=\"")?.1.split_once('"')?.0;
    let href = base.join(raw).ok()?;
    if href.origin() != base.origin()
        || href.path() != format!("/{}/", user.name)
        || !href.username().is_empty()
        || href.password().is_some()
        || href.fragment().is_some()
    {
        return None;
    }
    let pairs: Vec<_> = href.query_pairs().collect();
    if pairs.len() != 2 || pairs[0].0 != "sort" || pairs[0].1 != "date_asc" || pairs[1].0 != "page"
    {
        return None;
    }
    let ordinal = pairs[1].1.parse::<u16>().ok()?;
    (ordinal == current + 1 && ordinal <= MAX_PAGES).then_some(href)
}

async fn read_html(client: &Client, url: Url) -> Result<String, Box<dyn Error + Send + Sync>> {
    let mut resp = client.get(url.clone()).send().await?.error_for_status()?;
    if resp.url().origin() != url.origin() {
        return Err("Lensdump user page redirected outside first-party origin".into());
    }
    if resp
        .content_length()
        .is_some_and(|n| n > MAX_HTML_BYTES as u64)
    {
        return Err("Lensdump user HTML exceeds response byte cap".into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        if chunk.len() > MAX_HTML_BYTES.saturating_sub(body.len()) {
            return Err("Lensdump user HTML exceeds response byte cap".into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8(body)?)
}

async fn at_base(
    client: &Client,
    base: &Url,
    user: &User,
    max_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let end = (user.start + max_pages.clamp(1, MAX_PAGES)).min(MAX_PAGES + 1);
    for page in user.start..end {
        if page > user.start {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        let mut url = base.join(&format!("{}/", user.name))?;
        url.query_pairs_mut()
            .append_pair("sort", "date_asc")
            .append_pair("page", &page.to_string());
        let body = read_html(client, url).await?;
        decode_page(&body, user, &mut seen, &mut out);
        if out.len() >= MAX_CHILDREN {
            break;
        }
        if page == MAX_PAGES {
            break;
        }
        let current = base.join(&format!("{}/?sort=date_asc&page={page}", user.name))?;
        if next_page(&body, &current, user, page).is_none() {
            break;
        }
    }
    Ok(out)
}

pub async fn resolve_messages(
    client: &Client,
    user: User,
    max_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    at_base(
        client,
        &Url::parse("https://lensdump.com/")?,
        &user,
        max_pages,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    #[test]
    fn recognizes_public_profile_and_bounded_sort_page_only() {
        assert_eq!(match_url("https://lensdump.com/author").unwrap().start, 1);
        assert_eq!(
            match_url("https://lensdump.com/author/?sort=date_asc&page=2")
                .unwrap()
                .start,
            2
        );
        for bad in [
            "https://lensdump.com/i",
            "https://lensdump.com/a",
            "https://lensdump.com/author?sort=private&page=2",
            "https://lensdump.com/author?sort=date_asc&page=5",
            "https://lensdump.com/author?sort=date_asc&page=1&password=secret",
            "https://evillensdump.com/author",
            "https://lensdump.com/author/../../secret",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn queues_only_scoped_unduplicated_public_album_identifiers() {
        let user = match_url("https://lensdump.com/author").unwrap();
        let html = r#"<div data-url-short="/a/Album1"></div><div data-url-short="/a/Album1"></div><div data-url-short="https://lensdump.com/a/Album2"></div><div data-url-short="https://evil-lensdump.com/a/Forged"></div><div data-url-short="/a/../secret"></div>"#;
        let mut out = Vec::new();
        decode_page(html, &user, &mut HashSet::new(), &mut out);
        assert_eq!(out.len(), 2);
        let first = serde_json::to_value(&out[0]).unwrap();
        assert_eq!(first["url"], "https://lensdump.com/a/Album1");
        assert_eq!(first["metadata"]["subcategory"], "albums");
    }

    #[test]
    fn continuation_rejects_foreign_origins_and_nonsequential_pages() {
        let user = match_url("https://lensdump.com/author").unwrap();
        let base = Url::parse("https://lensdump.com/author/?sort=date_asc&page=1").unwrap();
        let good = r#"<a data-pagination="next" href="/author/?sort=date_asc&page=2">Next</a>"#;
        assert_eq!(
            next_page(good, &base, &user, 1).unwrap().query(),
            Some("sort=date_asc&page=2")
        );
        let bad = r#"<a data-pagination="next" href="https://evil-lensdump.com/author/?sort=date_asc&page=2">Next</a>"#;
        assert!(next_page(bad, &base, &user, 1).is_none());
        let skipped = r#"<a data-pagination="next" href="/author/?sort=date_asc&page=3">Next</a>"#;
        assert!(next_page(skipped, &base, &user, 1).is_none());
    }

    #[tokio::test]
    async fn public_user_album_queue_uses_bounded_two_requests() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (path, html) in [
                (
                    "/author/?sort=date_asc&page=1",
                    r#"<div data-url-short="/a/Album1"></div><a data-pagination="next" href="/author/?sort=date_asc&page=2">Next</a>"#,
                ),
                (
                    "/author/?sort=date_asc&page=2",
                    r#"<div data-url-short="/a/Album1"></div><div data-url-short="/a/Album2"></div>"#,
                ),
            ] {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut buf = [0u8; 4096];
                let size = stream.read(&mut buf).await.unwrap();
                assert!(String::from_utf8_lossy(&buf[..size])
                    .starts_with(&format!("GET {path} HTTP/1.1")));
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
                    html.len()
                );
                stream.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{address}/")).unwrap();
        let user = match_url("https://lensdump.com/author").unwrap();
        let children = at_base(&Client::new(), &base, &user, 4).await.unwrap();
        assert_eq!(children.len(), 2);
        assert_eq!(
            serde_json::to_value(&children[1]).unwrap()["url"],
            "https://lensdump.com/a/Album2"
        );
        server.await.unwrap();
    }
}
