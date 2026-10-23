//! Independent, partial public Imgpile post-media extractor.
//! Public profile feed pagination is bounded; complete metadata, authentication
//! and live-site equivalence are not yet supported.
use crate::{safe_filename, MediaItem};
use reqwest::Client;
use serde_json::Value;
use std::{collections::HashSet, error::Error};
use url::Url;

const MAX_HTML_BYTES: usize = 2 * 1024 * 1024;
const MAX_POST_MEDIA: usize = 2000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Post { slug: String },
    User { name: String },
}

fn valid_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= 128
        && slug.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn valid_user(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

pub fn match_url(raw: &str) -> Option<Target> {
    let url = Url::parse(raw).ok()?;
    if url.scheme() != "https" || !matches!(url.host_str(), Some("imgpile.com" | "www.imgpile.com"))
    {
        return None;
    }
    match url.path_segments()?.collect::<Vec<_>>().as_slice() {
        ["p", slug] if valid_slug(slug) => Some(Target::Post {
            slug: (*slug).to_owned(),
        }),
        ["u", name] if valid_user(name) => Some(Target::User {
            name: (*name).to_owned(),
        }),
        _ => None,
    }
}

fn supported_asset(raw: &str) -> Option<(Url, String)> {
    let url = Url::parse(raw).ok()?;
    let host = url.host_str()?;
    if url.scheme() != "https" || (host != "imgpile.com" && !host.ends_with(".imgpile.com")) {
        return None;
    }
    let filename = url.path_segments()?.next_back()?;
    if filename.is_empty() || filename.len() > 240 {
        return None;
    }
    let extension = filename.rsplit_once('.')?.1;
    if extension.is_empty()
        || extension.len() > 12
        || !extension.bytes().all(|b| b.is_ascii_alphanumeric())
    {
        return None;
    }
    Some((url.clone(), safe_filename(filename)))
}

fn post_media(html: &str) -> Vec<MediaItem> {
    let mut media = Vec::new();
    let mut seen = HashSet::new();
    for section in html
        .split("class=\"post-media")
        .skip(1)
        .take(MAX_POST_MEDIA)
    {
        let fragment = section
            .split_once("</div>")
            .map(|(first, _)| first)
            .unwrap_or(section);
        let Some(raw) = fragment
            .split_once("<a href=\"")
            .and_then(|(_, rest)| rest.split_once('"'))
            .map(|(link, _)| link)
        else {
            continue;
        };
        let Some((url, filename)) = supported_asset(raw) else {
            continue;
        };
        let post_id = fragment
            .split_once("data-media-id=\"")
            .and_then(|(_, part)| part.split_once('"'))
            .and_then(|(value, _)| value.parse::<u64>().ok());
        if seen.insert(url.to_string()) {
            media.push(MediaItem {
                url: url.to_string(),
                filename,
                extractor: "imgpile",
                post_id,
            });
        }
    }
    media
}

async fn read_html(client: &Client, url: Url) -> Result<String, Box<dyn Error + Send + Sync>> {
    let mut response = client.get(url).send().await?.error_for_status()?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_HTML_BYTES as u64)
    {
        return Err("Imgpile HTML exceeds the configured safety bound".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len().saturating_add(chunk.len()) > MAX_HTML_BYTES {
            return Err("Imgpile HTML exceeds the configured safety bound".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8(bytes)?)
}

async fn read_json(client: &Client, url: Url) -> Result<Value, Box<dyn Error + Send + Sync>> {
    let mut response = client.get(url).send().await?.error_for_status()?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_HTML_BYTES as u64)
    {
        return Err("Imgpile API JSON exceeds the configured safety bound".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len().saturating_add(chunk.len()) > MAX_HTML_BYTES {
            return Err("Imgpile API JSON exceeds the configured safety bound".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(serde_json::from_slice(&bytes)?)
}

fn scoped_next_page(current: &Url, first: &Url, raw: &str, name: &str) -> Option<Url> {
    let mut next = current.join(raw).ok()?;
    if next.origin() != first.origin() || next.path() != first.path() {
        return None;
    }
    let mut username_seen = false;
    let mut public_seen = false;
    for (key, value) in next.query_pairs() {
        match key.as_ref() {
            "username" => {
                if username_seen || value != name {
                    return None;
                }
                username_seen = true;
            }
            "visibility" => {
                if public_seen || value != "public" {
                    return None;
                }
                public_seen = true;
            }
            _ => {}
        }
    }
    {
        let mut query = next.query_pairs_mut();
        if !username_seen {
            query.append_pair("username", name);
        }
        if !public_seen {
            query.append_pair("visibility", "public");
        }
    }
    (next != *current).then_some(next)
}

async fn user_media(
    client: &Client,
    base: &Url,
    name: &str,
    max_pages: u16,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let mut first = base.join("api/v1/posts")?;
    first
        .query_pairs_mut()
        .append_pair("limit", "100")
        .append_pair("sort", "latest")
        .append_pair("period", "all")
        .append_pair("visibility", "public")
        .append_pair("username", name);
    let mut page = first.clone();
    let mut visited_pages = HashSet::new();
    let mut slugs = Vec::new();
    let mut seen_slugs = HashSet::new();
    for index in 0..max_pages.clamp(1, 10) {
        if !visited_pages.insert(page.to_string()) {
            break;
        }
        if index > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
        let response = read_json(client, page.clone()).await?;
        let posts = response
            .get("data")
            .and_then(Value::as_array)
            .ok_or("Imgpile public profile API missing posts")?;
        for post in posts.iter().take(100) {
            if let Some(slug) = post
                .get("slug")
                .and_then(Value::as_str)
                .filter(|slug| valid_slug(slug))
            {
                if seen_slugs.insert(slug.to_owned()) {
                    slugs.push(slug.to_owned());
                }
            }
        }
        if slugs.len() >= 1000 {
            break;
        }
        let next = response
            .get("links")
            .and_then(|links| links.get("next"))
            .and_then(Value::as_str)
            .filter(|link| !link.is_empty() && link.len() <= 1024);
        let Some(next) = next.and_then(|link| scoped_next_page(&page, &first, link, name))
        else {
            break;
        };
        page = next;
    }
    let mut media = Vec::new();
    let mut seen_urls = HashSet::new();
    for (index, slug) in slugs.iter().enumerate() {
        if index > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
        let html = read_html(client, base.join(&format!("p/{slug}"))?).await?;
        for item in post_media(&html) {
            if seen_urls.insert(item.url.clone()) {
                media.push(item);
            }
        }
    }
    Ok(media)
}

async fn resolve_at_base(
    client: &Client,
    base: &Url,
    target: Target,
    max_pages: u16,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    match target {
        Target::Post { slug } => {
            let html = read_html(client, base.join(&format!("p/{slug}"))?).await?;
            Ok(post_media(&html))
        }
        Target::User { name } => user_media(client, base, &name, max_pages).await,
    }
}

pub async fn resolve(
    client: &Client,
    target: Target,
    max_pages: u16,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    resolve_at_base(
        client,
        &Url::parse("https://imgpile.com/")?,
        target,
        max_pages,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conservative_imgpile_post_routes() {
        assert_eq!(
            match_url("https://imgpile.com/p/AbCdEfG"),
            Some(Target::Post {
                slug: "AbCdEfG".into(),
            })
        );
        assert_eq!(
            match_url("https://imgpile.com/u/creator-1"),
            Some(Target::User {
                name: "creator-1".into(),
            })
        );
        for url in [
            "http://imgpile.com/p/AbCdEfG",
            "https://evil.imgpile.com/p/AbCdEfG",
            "https://imgpile.com/p/bad%2Fpath",
            "https://imgpile.com/p/",
            "https://imgpile.com/u/invalid%2Fuser",
        ] {
            assert!(match_url(url).is_none(), "{url}");
        }
    }

    #[test]
    fn extracts_ordered_post_assets_with_checked_cdn_and_ids() {
        let html = concat!(
            "<div class=\"post-media\" data-media-id=\"17\"><a href=\"https://cdn.imgpile.com/photo/one.jpg\">one</a></div>",
            "<div class=\"post-media\" data-media-id=\"18\"><a href=\"https://imgpile.com/photo/two.webp\">two</a></div>",
            "<div class=\"post-media\"><a href=\"https://cdn.imgpile.com/photo/one.jpg\">duplicate</a></div>",
            "<div class=\"post-media\"><a href=\"https://imgpile.com.evil.test/unsafe.jpg\">unsafe</a></div>",
        );
        let images = post_media(html);
        assert_eq!(images.len(), 2);
        assert_eq!(images[0].filename, "one.jpg");
        assert_eq!(images[0].post_id, Some(17));
        assert_eq!(images[1].filename, "two.webp");
        assert_eq!(images[1].post_id, Some(18));
    }

    #[tokio::test]
    async fn public_user_feed_preserves_order_across_two_json_pages() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (route, body) in [
                (
                    "GET /api/v1/posts?limit=100&sort=latest&period=all&visibility=public&username=creator HTTP/1.1",
                    r#"{"data":[{"slug":"first12"},{"slug":"first12"}],"links":{"next":"?page=2"}}"#,
                ),
                (
                    "GET /api/v1/posts?page=2&username=creator&visibility=public HTTP/1.1",
                    r#"{"data":[{"slug":"second34"}],"links":{"next":null}}"#,
                ),
                (
                    "GET /p/first12 HTTP/1.1",
                    r#"<div class="post-media" data-media-id="71"><a href="https://cdn.imgpile.com/image/one.jpg">one</a></div>"#,
                ),
                (
                    "GET /p/second34 HTTP/1.1",
                    r#"<div class="post-media" data-media-id="72"><a href="https://cdn.imgpile.com/image/two.png">two</a></div>"#,
                ),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buffer = [0_u8; 4096];
                let count = socket.read(&mut buffer).await.unwrap();
                let request = String::from_utf8_lossy(&buffer[..count]);
                assert!(request.starts_with(route), "{request}");
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{address}/")).unwrap();
        let items = resolve_at_base(
            &Client::new(),
            &base,
            Target::User {
                name: "creator".into(),
            },
            2,
        )
        .await
        .unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].filename, "one.jpg");
        assert_eq!(items[0].post_id, Some(71));
        assert_eq!(items[1].filename, "two.png");
        server.await.unwrap();
    }

    #[test]
    fn profile_pagination_keeps_actor_scope_and_public_visibility() {
        let first = Url::parse("https://imgpile.com/api/v1/posts?username=owner").unwrap();
        let next = scoped_next_page(&first, &first, "?page=2", "owner").unwrap();
        assert_eq!(
            next.as_str(),
            "https://imgpile.com/api/v1/posts?page=2&username=owner&visibility=public"
        );
        assert!(scoped_next_page(&next, &first, "?page=2", "owner").is_none());
        assert!(scoped_next_page(&first, &first, "https://evil.example.org/api/v1/posts?page=2", "owner").is_none());
        assert!(scoped_next_page(&first, &first, "/admin?page=2", "owner").is_none());
        assert!(scoped_next_page(&first, &first, "?page=2&username=other", "owner").is_none());
        assert!(scoped_next_page(&first, &first, "?page=2&visibility=private", "owner").is_none());
    }


    #[tokio::test]
    async fn public_post_resolves_from_local_html() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0_u8; 4096];
            let n = socket.read(&mut buf).await.unwrap();
            let request = String::from_utf8_lossy(&buf[..n]);
            assert!(request.starts_with("GET /p/PostAB12 HTTP/1.1"), "{request}");
            let body = r#"<div class="post-media" data-media-id="91"><a href="https://cdn.imgpile.com/media/image.png">download</a></div>"#;
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(reply.as_bytes()).await.unwrap();
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let media = resolve_at_base(
            &Client::new(),
            &base,
            Target::Post {
                slug: "PostAB12".into(),
            },
            1,
        )
        .await
        .unwrap();
        assert_eq!(media.len(), 1);
        assert_eq!(media[0].filename, "image.png");
        assert_eq!(media[0].post_id, Some(91));
        server.await.unwrap();
    }
}
