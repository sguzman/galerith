//! Read-only, bounded public Imgur collection discovery.
//! This is a pinned upstream subset, not user authentication or private content.
use crate::message::{queued_url, ExtractorMessage, Metadata};
use reqwest::Client;
use serde_json::Value;
use std::{collections::HashSet, error::Error, time::Duration};
use url::Url;

const MAX_LISTING_PAGES: u16 = 4;
const MAX_LISTING_CHILDREN: usize = 160;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Listing {
    UserPosts(String),
    UserFavorites(String),
    Subreddit(String),
    Tag(String),
    Search(String),
}

fn safe_key(s: &str) -> bool {
    (1..=80).contains(&s.len())
        && s != "."
        && s != ".."
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

pub fn match_url(url: &Url) -> Option<Listing> {
    if !matches!(
        url.host_str(),
        Some("imgur.com" | "www.imgur.com" | "m.imgur.com" | "imgur.io")
    ) {
        return None;
    }
    let parts: Vec<_> = url.path_segments()?.collect();
    if matches!(parts.as_slice(), ["search"] | ["search", ""]) {
        let pairs: Vec<_> = url.query_pairs().collect();
        if pairs.len() != 1 || pairs[0].0 != "q" {
            return None;
        }
        let q = pairs[0].1.as_ref();
        if q.trim().is_empty()
            || q.len() > 240
            || q.chars().count() > 120
            || q.chars().any(char::is_control)
        {
            return None;
        }
        return Some(Listing::Search(q.to_owned()));
    }
    if url.query().is_some() {
        return None;
    }
    match parts.as_slice() {
        ["user", user] | ["user", user, "posts" | "submitted"]
            if safe_key(user) && *user != "me" =>
        {
            Some(Listing::UserPosts((*user).to_owned()))
        }
        ["user", user, "favorites"] if safe_key(user) && *user != "me" => {
            Some(Listing::UserFavorites((*user).to_owned()))
        }
        ["r", community] if safe_key(community) => {
            Some(Listing::Subreddit((*community).to_owned()))
        }
        ["t", tag] if safe_key(tag) => Some(Listing::Tag((*tag).to_owned())),
        _ => None,
    }
}

impl Listing {
    fn category(&self) -> &'static str {
        match self {
            Self::UserPosts(_) => "user",
            Self::UserFavorites(_) => "favorite",
            Self::Subreddit(_) => "subreddit",
            Self::Tag(_) => "tag",
            Self::Search(_) => "search",
        }
    }

    fn value(&self) -> &str {
        match self {
            Self::UserPosts(value)
            | Self::UserFavorites(value)
            | Self::Subreddit(value)
            | Self::Tag(value)
            | Self::Search(value) => value,
        }
    }

    fn api_path(&self, page: u16) -> String {
        match self {
            Self::UserPosts(user) => format!("3/account/{user}/submissions/{page}"),
            Self::UserFavorites(user) => format!("3/account/{user}/gallery_favorites/{page}"),
            Self::Subreddit(sub) => format!("3/gallery/r/{sub}/{page}"),
            Self::Tag(tag) => format!("3/gallery/t/{tag}/{page}"),
            Self::Search(query) => {
                let mut serializer = url::form_urlencoded::Serializer::new(String::new());
                serializer.append_pair("q", query);
                let pairs = serializer.finish();
                format!("3/gallery/search/{page}?{pairs}")
            }
        }
    }
}

fn queue_page(
    value: &Value,
    listing: &Listing,
    page: u16,
    seen: &mut HashSet<String>,
    children: &mut Vec<ExtractorMessage>,
) -> Result<bool, Box<dyn Error + Send + Sync>> {
    let root = super::unwrap_data(value);
    let images = match listing {
        Listing::Tag(_) => root.get("items").and_then(Value::as_array),
        _ => root.as_array(),
    }
    .ok_or("Imgur public listing returned an invalid collection")?;

    for image in images {
        if children.len() >= MAX_LISTING_CHILDREN {
            break;
        }
        let Some(id) = image.get("id").and_then(Value::as_str) else {
            continue;
        };
        let Some(is_album) = image.get("is_album").and_then(Value::as_bool) else {
            continue;
        };
        if !super::valid_id(id) || !seen.insert(id.to_owned()) {
            continue;
        }
        let mut metadata = Metadata::new();
        metadata.insert("category".into(), "imgur".into());
        metadata.insert("subcategory".into(), listing.category().into());
        metadata.insert("listing_value".into(), listing.value().to_owned().into());
        metadata.insert("listing_page".into(), page.into());
        metadata.insert("media_id".into(), id.to_owned().into());
        metadata.insert("is_album".into(), is_album.into());
        let url = if is_album {
            format!("https://imgur.com/a/{id}")
        } else {
            format!("https://imgur.com/{id}")
        };
        children.push(queued_url(url, metadata));
    }
    Ok(!images.is_empty())
}

pub async fn resolve_messages(
    client: &Client,
    base: &Url,
    listing: Listing,
    max_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    let pages = max_pages.clamp(1, MAX_LISTING_PAGES);
    for page in 0..pages {
        if page > 0 {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        let value = super::public_json(client, base, &listing.api_path(page), false).await?;
        let more = queue_page(&value, &listing, page, &mut seen, &mut result)?;
        if !more || result.len() >= MAX_LISTING_CHILDREN {
            break;
        }
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
    fn public_listing_routes_are_strict_and_scoped() {
        let match_it = |s| match_url(&Url::parse(s).unwrap());
        assert_eq!(
            match_it("https://imgur.com/user/artist/posts"),
            Some(Listing::UserPosts("artist".into()))
        );
        assert_eq!(
            match_it("https://imgur.com/user/artist/favorites"),
            Some(Listing::UserFavorites("artist".into()))
        );
        assert_eq!(
            match_it("https://imgur.com/r/cats"),
            Some(Listing::Subreddit("cats".into()))
        );
        assert_eq!(
            match_it("https://imgur.com/t/landscape"),
            Some(Listing::Tag("landscape".into()))
        );
        assert_eq!(
            match_it("https://imgur.com/search?q=blue+sky"),
            Some(Listing::Search("blue sky".into()))
        );
        for bad in [
            "https://imgur.com/user/me",
            "https://imgur.com/user/a%2Fb",
            "https://imgur.com/r/cats?secret=x",
            "https://imgur.com/search?q=blue&q=red",
            "https://imgur.com/search?private=x",
            "https://imgur.com/search?q=%00bad",
        ] {
            assert!(match_it(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn queue_items_validate_ids_preserve_order_and_classification() {
        let mut seen = HashSet::new();
        let mut events = Vec::new();
        let data = serde_json::json!({"data":[
          {"id":"abcde","is_album":false},
          {"id":"abcdefg","is_album":true},
          {"id":"abcde","is_album":false},
          {"id":"../../etc","is_album":true},
          {"id":"abc1234","is_album":"true"}
        ]});
        assert!(queue_page(
            &data,
            &Listing::UserPosts("artist".into()),
            0,
            &mut seen,
            &mut events
        )
        .unwrap());
        assert_eq!(events.len(), 2);
        let first = serde_json::to_value(&events[0]).unwrap();
        assert_eq!(first["url"], "https://imgur.com/abcde");
        assert_eq!(first["metadata"]["listing_page"], 0);
        assert_eq!(first["metadata"]["listing_value"], "artist");
        let second = serde_json::to_value(&events[1]).unwrap();
        assert_eq!(second["url"], "https://imgur.com/a/abcdefg");
        assert!(!queue_page(
            &serde_json::json!({"data":[]}),
            &Listing::UserPosts("artist".into()),
            1,
            &mut seen,
            &mut events
        )
        .unwrap());
        let tags = serde_json::json!({"data":{"items":[{"id":"xyz12","is_album":false}]}});
        assert!(queue_page(
            &tags,
            &Listing::Tag("art".into()),
            0,
            &mut seen,
            &mut events
        )
        .unwrap());
        assert_eq!(events.len(), 3);
    }

    #[tokio::test]
    async fn bounded_public_listing_paginates_then_stops_at_empty_page() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (page, json) in [
                (0, r#"{"data":[{"id":"abcde","is_album":false}]}"#),
                (
                    1,
                    r#"{"data":[{"id":"abcde","is_album":false},{"id":"xyz12","is_album":true}]}"#,
                ),
                (2, r#"{"data":[]}"#),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buf = [0u8; 4096];
                let count = socket.read(&mut buf).await.unwrap();
                let request = String::from_utf8_lossy(&buf[..count]);
                assert!(
                    request.starts_with(&format!(
                        "GET /3/account/artist/submissions/{page} HTTP/1.1"
                    )),
                    "{request}"
                );
                assert!(
                    request
                        .to_ascii_lowercase()
                        .contains("authorization: client-id 546c25a59c58ad7"),
                    "{request}"
                );
                let reply=format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json}",json.len());
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{address}/")).unwrap();
        let output = resolve_messages(
            &Client::new(),
            &base,
            Listing::UserPosts("artist".into()),
            4,
        )
        .await
        .unwrap();
        let links = output
            .iter()
            .map(|x| {
                serde_json::to_value(x).unwrap()["url"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            links,
            vec!["https://imgur.com/abcde", "https://imgur.com/a/xyz12"]
        );
        server.await.unwrap();
    }
}
