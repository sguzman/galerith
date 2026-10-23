//! Partial public Bluesky/ATProto image extraction.
//! Original PDS blobs, videos, quoted posts, authentication, metadata, and
//! other upstream gallery-dl routes remain outside this preview.
use crate::{safe_filename, MediaItem};
use reqwest::Client;
use serde_json::Value;
use std::{collections::HashSet, error::Error, time::Duration};
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Post { actor: String, rkey: String },
    Media { actor: String },
    Posts { actor: String },
    Replies { actor: String },
    Avatar { actor: String },
    Banner { actor: String },
}

fn valid_segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b':'))
}

pub fn match_url(input: &str) -> Option<Target> {
    let url = Url::parse(input).ok()?;
    if url.scheme() != "https" || !matches!(url.host_str()?, "bsky.app" | "www.bsky.app") {
        return None;
    }
    let parts: Vec<_> = url.path_segments()?.collect();
    match parts.as_slice() {
        ["profile", actor, "post", rkey] if valid_segment(actor) && valid_segment(rkey) => {
            Some(Target::Post {
                actor: (*actor).to_owned(),
                rkey: (*rkey).to_owned(),
            })
        }
        ["profile", actor] | ["profile", actor, ""] if valid_segment(actor) => {
            Some(Target::Media {
                actor: (*actor).to_owned(),
            })
        }
        ["profile", actor, "media"] if valid_segment(actor) => Some(Target::Media {
            actor: (*actor).to_owned(),
        }),
        ["profile", actor, "posts"] if valid_segment(actor) => Some(Target::Posts {
            actor: (*actor).to_owned(),
        }),
        ["profile", actor, "replies"] if valid_segment(actor) => Some(Target::Replies {
            actor: (*actor).to_owned(),
        }),
        ["profile", actor, "avatar"] if valid_segment(actor) => Some(Target::Avatar {
            actor: (*actor).to_owned(),
        }),
        ["profile", actor, "banner"] | ["profile", actor, "background"] if valid_segment(actor) => {
            Some(Target::Banner {
                actor: (*actor).to_owned(),
            })
        }
        _ => None,
    }
}

fn cdn_image(raw: &str) -> Option<(Url, String)> {
    let url = Url::parse(raw).ok()?;
    if url.scheme() != "https" || url.host_str() != Some("cdn.bsky.app") {
        return None;
    }
    let ext = url
        .path()
        .rsplit_once('@')
        .map(|(_, extension)| extension)
        .or_else(|| url.path().rsplit_once('.').map(|(_, extension)| extension))
        .unwrap_or("jpg")
        .to_owned();
    if ext.is_empty() || ext.len() > 12 || !ext.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return None;
    }
    Some((url, ext))
}

fn profile_image(profile: &Value, did: &str, kind: &str) -> Option<MediaItem> {
    if profile.get("did").and_then(Value::as_str)? != did {
        return None;
    }
    let raw = profile.get(kind).and_then(Value::as_str)?;
    let (url, ext) = cdn_image(raw)?;
    let path_segment = url.path_segments()?.next_back()?;
    let cid = path_segment.split('@').next()?;
    if cid.is_empty()
        || cid.len() > 200
        || !cid.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return None;
    }
    Some(MediaItem {
        url: url.to_string(),
        filename: safe_filename(&format!("{kind}_{cid}.{ext}")),
        extractor: "bluesky",
        post_id: None,
    })
}

fn parse_media(post: &Value, expected_did: &str) -> Vec<MediaItem> {
    let Some(author_did) = post
        .get("author")
        .and_then(|author| author.get("did"))
        .and_then(Value::as_str)
    else {
        return Vec::new();
    };
    if author_did != expected_did {
        return Vec::new();
    }
    let Some(rkey) = post
        .get("uri")
        .and_then(Value::as_str)
        .and_then(|uri| uri.rsplit('/').next())
        .filter(|rkey| valid_segment(rkey))
    else {
        return Vec::new();
    };
    let Some(embed) = post.get("embed") else {
        return Vec::new();
    };
    let media = embed.get("media").unwrap_or(embed);
    let Some(images) = media.get("images").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut items = Vec::new();
    for (i, image) in images.iter().enumerate() {
        let Some(raw) = image.get("fullsize").and_then(Value::as_str) else {
            continue;
        };
        let Some((url, ext)) = cdn_image(raw) else {
            continue;
        };
        items.push(MediaItem {
            url: url.to_string(),
            filename: safe_filename(&format!("{rkey}_{}.{ext}", i + 1)),
            extractor: "bluesky",
            post_id: None,
        });
    }
    items
}

async fn resolve_actor(
    client: &Client,
    base: &Url,
    actor: &str,
) -> Result<String, Box<dyn Error + Send + Sync>> {
    if actor.starts_with("did:") {
        return Ok(actor.to_owned());
    }
    let mut endpoint = base.join("xrpc/com.atproto.identity.resolveHandle")?;
    endpoint.query_pairs_mut().append_pair("handle", actor);
    let response: Value = client
        .get(endpoint)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let did = response
        .get("did")
        .and_then(Value::as_str)
        .filter(|did| {
            did.len() <= 256
                && (did.starts_with("did:plc:") || did.starts_with("did:web:"))
                && valid_segment(did)
        })
        .ok_or("invalid Bluesky DID resolution response")?;
    Ok(did.to_owned())
}

pub async fn resolve(
    client: &Client,
    target: Target,
    max_pages: u16,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse("https://api.bsky.app/")?;
    resolve_at_base(client, target, max_pages, &base).await
}

async fn resolve_at_base(
    client: &Client,
    target: Target,
    max_pages: u16,
    base: &Url,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let actor = match &target {
        Target::Post { actor, .. }
        | Target::Media { actor }
        | Target::Posts { actor }
        | Target::Replies { actor }
        | Target::Avatar { actor }
        | Target::Banner { actor } => actor,
    };
    let did = resolve_actor(client, base, actor).await?;
    match target {
        Target::Avatar { .. } | Target::Banner { .. } => {
            let kind = if matches!(target, Target::Avatar { .. }) {
                "avatar"
            } else {
                "banner"
            };
            let mut endpoint = base.join("xrpc/app.bsky.actor.getProfile")?;
            endpoint.query_pairs_mut().append_pair("actor", &did);
            let profile: Value = client
                .get(endpoint)
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            Ok(profile_image(&profile, &did, kind).into_iter().collect())
        }
        Target::Post { rkey, .. } => {
            let mut endpoint = base.join("xrpc/app.bsky.feed.getPostThread")?;
            endpoint
                .query_pairs_mut()
                .append_pair("uri", &format!("at://{did}/app.bsky.feed.post/{rkey}"))
                .append_pair("depth", "0")
                .append_pair("parentHeight", "0");
            let body: Value = client
                .get(endpoint)
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            let post = body
                .get("thread")
                .and_then(|thread| thread.get("post"))
                .ok_or("Bluesky thread response missing post")?;
            Ok(parse_media(post, &did))
        }
        Target::Media { .. } | Target::Posts { .. } | Target::Replies { .. } => {
            let filter = match &target {
                Target::Media { .. } => "posts_with_media",
                Target::Posts { .. } => "posts_and_author_threads",
                Target::Replies { .. } => "posts_with_replies",
                Target::Post { .. } | Target::Avatar { .. } | Target::Banner { .. } => {
                    unreachable!("single posts and profile images are resolved above")
                }
            };
            let mut result = Vec::new();
            let mut seen = HashSet::new();
            let mut cursor: Option<String> = None;
            for offset in 0..max_pages.max(1) {
                if offset > 0 {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                let mut endpoint = base.join("xrpc/app.bsky.feed.getAuthorFeed")?;
                {
                    let mut query = endpoint.query_pairs_mut();
                    query
                        .append_pair("actor", &did)
                        .append_pair("filter", filter)
                        .append_pair("limit", "100");
                    if let Some(cursor) = &cursor {
                        query.append_pair("cursor", cursor);
                    }
                }
                let body: Value = client
                    .get(endpoint)
                    .send()
                    .await?
                    .error_for_status()?
                    .json()
                    .await?;
                let feed = body
                    .get("feed")
                    .and_then(Value::as_array)
                    .ok_or("Bluesky author feed response missing entries")?;
                if feed.is_empty() {
                    break;
                }
                for entry in feed {
                    if let Some(post) = entry.get("post") {
                        for item in parse_media(post, &did) {
                            if seen.insert(item.url.clone()) {
                                result.push(item);
                            }
                        }
                    }
                }
                cursor = body
                    .get("cursor")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty() && value.len() <= 512)
                    .map(str::to_owned);
                if cursor.is_none() {
                    break;
                }
            }
            Ok(result)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn routes_public_bluesky_post_and_media_only() {
        assert_eq!(
            match_url("https://bsky.app/profile/alice.bsky.social/post/3n9"),
            Some(Target::Post {
                actor: "alice.bsky.social".into(),
                rkey: "3n9".into(),
            })
        );
        assert_eq!(
            match_url("https://www.bsky.app/profile/did:plc:alice/media"),
            Some(Target::Media {
                actor: "did:plc:alice".into(),
            })
        );
        assert_eq!(
            match_url("https://bsky.app/profile/alice.bsky.social/posts"),
            Some(Target::Posts {
                actor: "alice.bsky.social".into(),
            })
        );
        assert_eq!(
            match_url("https://bsky.app/profile/alice.bsky.social/replies"),
            Some(Target::Replies {
                actor: "alice.bsky.social".into(),
            })
        );
        assert_eq!(
            match_url("https://bsky.app/profile/alice.bsky.social"),
            Some(Target::Media {
                actor: "alice.bsky.social".into(),
            })
        );
        assert_eq!(
            match_url("https://bsky.app/profile/alice.bsky.social/"),
            Some(Target::Media {
                actor: "alice.bsky.social".into(),
            })
        );
        assert_eq!(
            match_url("https://bsky.app/profile/alice.bsky.social/avatar"),
            Some(Target::Avatar {
                actor: "alice.bsky.social".into(),
            })
        );
        assert_eq!(
            match_url("https://bsky.app/profile/did:plc:alice/background"),
            Some(Target::Banner {
                actor: "did:plc:alice".into(),
            })
        );
        assert_eq!(
            match_url("https://bsky.app/profile/did:plc:alice/banner"),
            Some(Target::Banner {
                actor: "did:plc:alice".into(),
            })
        );
        assert!(match_url("https://bsky.app/profile/alice.bsky.social/likes").is_none());
        assert!(match_url("https://bsky.evil.app/profile/a/media").is_none());
        assert!(match_url("http://bsky.app/profile/a/media").is_none());
        assert!(match_url("https://bsky.app/profile/a%2Fb/media").is_none());
    }

    #[test]
    fn image_media_is_validated_and_other_authors_are_skipped() {
        let post = json!({
            "uri": "at://did:plc:alice/app.bsky.feed.post/3n9",
            "author": {"did": "did:plc:alice"},
            "embed": {"images": [
                {"fullsize": "https://cdn.bsky.app/img/feed_fullsize/plain/did:plc:alice/cid@jpeg"},
                {"fullsize": "file:///etc/shadow"},
                {"fullsize": "https://evil.example.org/forged.png"}
            ]}
        });
        let files = parse_media(&post, "did:plc:alice");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].filename, "3n9_1.jpeg");
        assert_eq!(files[0].extractor, "bluesky");
        assert_eq!(files[0].post_id, None);
        assert!(parse_media(&post, "did:plc:other").is_empty());
    }

    #[test]
    fn nested_record_with_media_images_are_retained() {
        let post = json!({
            "uri": "at://did:plc:alice/app.bsky.feed.post/3quote",
            "author": {"did": "did:plc:alice"},
            "embed": {"media": {"images": [
                {"fullsize": "https://cdn.bsky.app/img/feed_fullsize/plain/did:plc:alice/cid@webp"}
            ]}}
        });
        let media = parse_media(&post, "did:plc:alice");
        assert_eq!(media.len(), 1);
        assert_eq!(media[0].filename, "3quote_1.webp");
    }

    #[tokio::test]
    async fn public_avatar_and_banner_use_profile_json_and_checked_cdn_urls() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for _ in 0..2 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buf = [0_u8; 4096];
                let n = socket.read(&mut buf).await.unwrap();
                let request = String::from_utf8_lossy(&buf[..n]);
                assert!(
                    request.starts_with(
                        "GET /xrpc/app.bsky.actor.getProfile?actor=did%3Aplc%3Aalice HTTP/1.1"
                    ),
                    "{request}"
                );
                let body = r#"{"did":"did:plc:alice","avatar":"https://cdn.bsky.app/img/avatar/plain/did:plc:alice/abc123@jpeg","banner":"https://cdn.bsky.app/img/banner/plain/did:plc:alice/def456@png"}"#;
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{address}/")).unwrap();
        for (target, name) in [
            (
                Target::Avatar {
                    actor: "did:plc:alice".into(),
                },
                "avatar_abc123.jpeg",
            ),
            (
                Target::Banner {
                    actor: "did:plc:alice".into(),
                },
                "banner_def456.png",
            ),
        ] {
            let result = resolve_at_base(&Client::new(), target, 1, &base)
                .await
                .unwrap();
            assert_eq!(result.len(), 1);
            assert_eq!(result[0].filename, name);
        }
        server.await.unwrap();
        let invalid = json!({
            "did": "did:plc:other",
            "avatar": "https://cdn.bsky.app/img/avatar/plain/did:plc:alice/abc123@jpeg"
        });
        assert!(profile_image(&invalid, "did:plc:alice", "avatar").is_none());
        let invalid_host = json!({
            "did": "did:plc:alice",
            "avatar": "https://evil.example.org/img/cid@png"
        });
        assert!(profile_image(&invalid_host, "did:plc:alice", "avatar").is_none());
    }

    #[tokio::test]
    async fn post_resolves_handle_and_gets_public_image() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (path, body) in [
                (
                    "/xrpc/com.atproto.identity.resolveHandle?handle=alice.bsky.social",
                    r#"{"did":"did:plc:alice"}"#,
                ),
                (
                    "/xrpc/app.bsky.feed.getPostThread?",
                    r#"{"thread":{"post":{"uri":"at://did:plc:alice/app.bsky.feed.post/3n9","author":{"did":"did:plc:alice"},"embed":{"images":[{"fullsize":"https://cdn.bsky.app/img/feed_fullsize/plain/did:plc:alice/cid@jpeg"}]}}}}"#,
                ),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buf = [0_u8; 4096];
                let n = socket.read(&mut buf).await.unwrap();
                let request = String::from_utf8_lossy(&buf[..n]);
                assert!(request.starts_with(&format!("GET {path}")), "{request}");
                if path.contains("getPostThread") {
                    assert!(request.contains("app.bsky.feed.post"), "{request}");
                }
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{address}/")).unwrap();
        let target = Target::Post {
            actor: "alice.bsky.social".into(),
            rkey: "3n9".into(),
        };
        let images = resolve_at_base(&Client::new(), target, 1, &base)
            .await
            .unwrap();
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].filename, "3n9_1.jpeg");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn posts_and_replies_use_distinct_upstream_author_feed_filters() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (filter, rkey) in [
                ("posts_and_author_threads", "pp"),
                ("posts_with_replies", "rr"),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buf = [0_u8; 4096];
                let n = socket.read(&mut buf).await.unwrap();
                let request = String::from_utf8_lossy(&buf[..n]);
                assert!(request.contains(&format!("filter={filter}")), "{request}");
                let body = format!(
                    r#"{{"feed":[{{"post":{{"uri":"at://did:plc:alice/app.bsky.feed.post/{rkey}","author":{{"did":"did:plc:alice"}},"embed":{{"images":[{{"fullsize":"https://cdn.bsky.app/img/feed_fullsize/plain/{rkey}@jpeg"}}]}}}}}}]}}"#
                );
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{address}/")).unwrap();
        for (target, rkey) in [
            (
                Target::Posts {
                    actor: "did:plc:alice".into(),
                },
                "pp",
            ),
            (
                Target::Replies {
                    actor: "did:plc:alice".into(),
                },
                "rr",
            ),
        ] {
            let media = resolve_at_base(&Client::new(), target, 3, &base)
                .await
                .unwrap();
            assert_eq!(media.len(), 1);
            assert_eq!(media[0].filename, format!("{rkey}_1.jpeg"));
        }
        server.await.unwrap();
    }

    #[tokio::test]
    async fn media_feed_is_bounded_and_skips_reposts() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (path, body) in [
                (
                    "/xrpc/app.bsky.feed.getAuthorFeed?actor=did%3Aplc%3Aalice&filter=posts_with_media&limit=100",
                    r#"{"cursor":"NEXT","feed":[{"post":{"uri":"at://did:plc:alice/app.bsky.feed.post/aa","author":{"did":"did:plc:alice"},"embed":{"images":[{"fullsize":"https://cdn.bsky.app/img/feed_fullsize/plain/a@png"}]}}},{"post":{"uri":"at://did:plc:other/app.bsky.feed.post/bb","author":{"did":"did:plc:other"},"embed":{"images":[{"fullsize":"https://cdn.bsky.app/img/feed_fullsize/plain/b@png"}]}}}]}"#,
                ),
                (
                    "/xrpc/app.bsky.feed.getAuthorFeed?actor=did%3Aplc%3Aalice&filter=posts_with_media&limit=100&cursor=NEXT",
                    r#"{"feed":[]}"#,
                ),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buf = [0_u8; 4096];
                let n = socket.read(&mut buf).await.unwrap();
                let request = String::from_utf8_lossy(&buf[..n]);
                assert!(request.starts_with(&format!("GET {path}")), "{request}");
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{address}/")).unwrap();
        let target = Target::Media {
            actor: "did:plc:alice".into(),
        };
        let media = resolve_at_base(&Client::new(), target, 5, &base)
            .await
            .unwrap();
        assert_eq!(media.len(), 1);
        assert_eq!(media[0].filename, "aa_1.png");
        server.await.unwrap();
    }
}
