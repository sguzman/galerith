//! Conservative public ImageChest gallery and user-index previews.
//! Public HTML and deferred profile queues are read-only; authenticated API
//! access, account operations and full upstream compatibility are not ported.
use crate::{
    message::{media_metadata, queued_url, ExtractorMessage, Metadata},
    safe_filename, MediaItem,
};
use reqwest::Client;
use serde_json::Value;
use std::{collections::HashSet, error::Error, time::Duration};
use url::Url;

const MAX_GALLERY_HTML_BYTES: usize = 2 * 1024 * 1024;
const MAX_GALLERY_FILES: usize = 256;
const MAX_USER_INDEX_PAGES: u16 = 4;
const MAX_USER_INDEX_QUEUES: usize = 32;
const MAX_USER_INDEX_RECORDS: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gallery {
    pub id: String,
}

fn valid_id(value: &str) -> bool {
    value.len() == 11 && value.bytes().all(|b| b.is_ascii_alphanumeric())
}

pub fn match_url(input: &str) -> Option<Gallery> {
    let url = Url::parse(input).ok()?;
    if url.scheme() != "https"
        || !matches!(url.host_str(), Some("imgchest.com" | "www.imgchest.com"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
    {
        return None;
    }
    let parts: Vec<_> = url.path_segments()?.collect();
    let id = match parts.as_slice() {
        ["p", id] | ["p", id, ""] if valid_id(id) => *id,
        _ => return None,
    };
    Some(Gallery { id: id.into() })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserProfile {
    pub username: String,
}

fn valid_user(value: &str) -> bool {
    (1..=40).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

pub fn match_user_url(input: &str) -> Option<UserProfile> {
    let url = Url::parse(input).ok()?;
    if url.scheme() != "https"
        || !matches!(url.host_str(), Some("imgchest.com" | "www.imgchest.com"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
    {
        return None;
    }
    let parts: Vec<_> = url.path_segments()?.collect();
    let username = match parts.as_slice() {
        ["u", name] | ["u", name, ""] if valid_user(name) => *name,
        _ => return None,
    };
    Some(UserProfile {
        username: username.to_owned(),
    })
}

fn user_gallery_queues(
    body: &Value,
    username: &str,
    seen: &mut HashSet<String>,
    output: &mut Vec<ExtractorMessage>,
) -> Result<usize, Box<dyn Error + Send + Sync>> {
    let posts = body
        .get("data")
        .and_then(Value::as_array)
        .ok_or("ImageChest profile index must contain a post array")?;
    if posts.len() > MAX_USER_INDEX_RECORDS {
        return Err("ImageChest profile index exceeds the record limit".into());
    }
    for post in posts {
        // The pinned upstream user extractor yields the API's 'link' field.
        // Validate it independently rather than trusting an arbitrary host.
        // A valid ID alone is allowed for older public listing shapes.
        let linked = post.get("link").and_then(Value::as_str).and_then(match_url);
        let id = match (post.get("id"), linked) {
            (Some(Value::String(id)), Some(gallery)) if valid_id(id) && gallery.id == *id => {
                id.clone()
            }
            (None, Some(gallery)) => gallery.id,
            (Some(Value::String(id)), None) if post.get("link").is_none() && valid_id(id) => {
                id.clone()
            }
            _ => continue,
        };
        if post
            .get("privacy")
            .and_then(Value::as_str)
            .is_some_and(|privacy| privacy != "public")
            || post
                .get("username")
                .and_then(Value::as_str)
                .is_some_and(|owner| owner != username)
        {
            continue;
        }
        if !seen.insert(id.clone()) {
            continue;
        }
        let mut metadata = Metadata::new();
        metadata.insert("category".into(), "imagechest".into());
        metadata.insert("subcategory".into(), "gallery".into());
        metadata.insert("username".into(), username.into());
        metadata.insert("gallery_id".into(), id.into());
        if let Some(title) = post.get("title").and_then(Value::as_str) {
            if !title.is_empty() && title.len() <= 256 && !title.chars().any(char::is_control) {
                metadata.insert("title".into(), title.into());
            }
        }
        output.push(queued_url(format!("https://imgchest.com/p/{id}"), metadata));
        if output.len() >= MAX_USER_INDEX_QUEUES {
            break;
        }
    }
    Ok(posts.len())
}

async fn queue_user_at_base(
    client: &Client,
    user: &UserProfile,
    max_pages: u16,
    base: &Url,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let mut events = Vec::new();
    let mut seen = HashSet::new();
    for page in 1..=max_pages.clamp(1, MAX_USER_INDEX_PAGES) {
        let mut endpoint = base.join("api/posts")?;
        endpoint
            .query_pairs_mut()
            .append_pair("page", &page.to_string())
            .append_pair("sort", "new")
            .append_pair("tag", "")
            .append_pair("q", "")
            .append_pair("username", &user.username)
            .append_pair("nsfw", "true");
        let body =
            super::http_json::fetch_json_limited(client, endpoint, MAX_GALLERY_HTML_BYTES).await?;
        let raw_count = user_gallery_queues(&body, &user.username, &mut seen, &mut events)?;
        if events.len() >= MAX_USER_INDEX_QUEUES || raw_count == 0 {
            break;
        }
        // Upstream paginates until the source returns an empty data array.
        // A short nonempty page, or untrusted next-page URL, is not a stop.
        if page < max_pages.min(MAX_USER_INDEX_PAGES) {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
    Ok(events)
}

/// Read-only user profile listing: deferred public gallery URLs, never files.
pub async fn queued_user_galleries(
    client: &Client,
    user: &UserProfile,
    max_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    queue_user_at_base(
        client,
        user,
        max_pages,
        &Url::parse("https://imgchest.com/")?,
    )
    .await
}

fn gallery_files(data: &Value) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let files = data
        .pointer("/props/post/files")
        .and_then(Value::as_array)
        .ok_or("ImageChest gallery is missing public file records")?;
    if files.len() > MAX_GALLERY_FILES {
        return Err("ImageChest gallery exceeds the preview file limit".into());
    }
    let mut output = Vec::new();
    let mut seen = HashSet::new();
    for file in files {
        let Some(link) = file.get("link").and_then(Value::as_str) else {
            continue;
        };
        let Ok(url) = Url::parse(link) else { continue };
        if url.scheme() != "https"
            || url.host_str() != Some("cdn.imgchest.com")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            continue;
        }
        let Some(segments) = url.path_segments() else {
            continue;
        };
        let parts: Vec<_> = segments.collect();
        let ["files", filename] = parts.as_slice() else {
            continue;
        };
        let Some((id, extension)) = filename.rsplit_once('.') else {
            continue;
        };
        if !valid_id(id)
            || !matches!(
                extension.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "gif" | "webp" | "avif" | "mp4" | "webm"
            )
            || file
                .get("id")
                .and_then(Value::as_str)
                .is_some_and(|declared| declared != id)
        {
            continue;
        }
        if seen.insert(url.to_string()) {
            output.push(MediaItem {
                url: url.to_string(),
                filename: safe_filename(filename),
                extractor: "imagechest",
                post_id: None,
            });
        }
    }
    Ok(output)
}

#[cfg(test)]
fn parse_html(body: &str) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    gallery_files(&parse_page_payload(body)?)
}

fn parse_page_payload(body: &str) -> Result<Value, Box<dyn Error + Send + Sync>> {
    let start = body
        .find("data-page=\"")
        .ok_or("ImageChest gallery has no public page payload")?;
    let after = &body[start + "data-page=\"".len()..];
    let attribute = after
        .split_once('"')
        .map(|(value, _)| value)
        .ok_or("ImageChest gallery has an invalid page attribute")?;
    let decoded = attribute
        .replace("&quot;", "\"")
        .replace("&#34;", "\"")
        .replace("&#x22;", "\"")
        .replace("&amp;", "&");
    Ok(serde_json::from_str(&decoded)?)
}

/// Only the registered public gallery URL is accepted in normal dispatch.
/// The base URL parameter provides an offline loopback seam for tests.
async fn resolve_at_base(
    client: &Client,
    gallery: Gallery,
    base: &Url,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    gallery_files(&gallery_page_at_base(client, &gallery, base).await?)
}

async fn gallery_page_at_base(
    client: &Client,
    gallery: &Gallery,
    base: &Url,
) -> Result<Value, Box<dyn Error + Send + Sync>> {
    let endpoint = base.join(&format!("p/{}", gallery.id))?;
    let mut response = client.get(endpoint).send().await?.error_for_status()?;
    if response
        .content_length()
        .is_some_and(|size| size > MAX_GALLERY_HTML_BYTES as u64)
    {
        return Err("ImageChest gallery HTML exceeds the response limit".into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_GALLERY_HTML_BYTES.saturating_sub(body.len()) {
            return Err("ImageChest gallery HTML exceeds the response limit".into());
        }
        body.extend_from_slice(&chunk);
    }
    parse_page_payload(std::str::from_utf8(&body)?)
}

pub async fn resolve(
    client: &Client,
    gallery: Gallery,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    resolve_at_base(client, gallery, &Url::parse("https://imgchest.com/")?).await
}

fn safe_scalar(value: Option<&Value>, max_bytes: usize) -> Option<&str> {
    value.and_then(Value::as_str).filter(|text| {
        !text.is_empty() && text.len() <= max_bytes && !text.chars().any(char::is_control)
    })
}

#[cfg(test)]
fn gallery_messages(gallery: &Gallery, items: Vec<MediaItem>) -> Vec<ExtractorMessage> {
    gallery_messages_with_fields(gallery, items, &Value::Null)
}

fn gallery_messages_with_fields(
    gallery: &Gallery,
    items: Vec<MediaItem>,
    page: &Value,
) -> Vec<ExtractorMessage> {
    let post = page.pointer("/props/post");
    let files = page.pointer("/props/post/files").and_then(Value::as_array);
    let mut events = Vec::with_capacity(items.len().saturating_mul(2));
    for item in items {
        let mut metadata = media_metadata(&item);
        metadata.insert("gallery_id".into(), gallery.id.clone().into());
        if let Some(post) = post {
            if let Some(title) = safe_scalar(post.get("title"), 256) {
                metadata.insert("title".into(), title.into());
            }
            if let Some(username) = safe_scalar(post.get("username"), 40).filter(|u| valid_user(u)) {
                metadata.insert("username".into(), username.into());
            }
            if let Some(count) = post
                .get("image_count")
                .and_then(Value::as_u64)
                .filter(|count| *count <= MAX_GALLERY_FILES as u64)
            {
                metadata.insert("image_count".into(), count.into());
            }
        }
        if let Some(file) = files.and_then(|all| {
            all.iter()
                .find(|row| row.get("link").and_then(Value::as_str) == Some(item.url.as_str()))
        }) {
            if let Some(id) = file.get("id").and_then(Value::as_str).filter(|id| valid_id(id)) {
                metadata.insert("file_id".into(), id.into());
            }
            if let Some(position) = file
                .get("position")
                .and_then(Value::as_u64)
                .filter(|position| (1..=MAX_GALLERY_FILES as u64).contains(position))
            {
                metadata.insert("position".into(), position.into());
            }
            if let Some(description) = safe_scalar(file.get("description"), 1024) {
                metadata.insert("description".into(), description.into());
            }
        }
        events.push(ExtractorMessage::Directory {
            metadata: metadata.clone(),
        });
        events.push(ExtractorMessage::Url {
            url: item.url,
            metadata,
        });
    }
    events
}

/// Native message inspection preserves bounded gallery/file metadata from
/// the same public page request, not a second API fetch.
pub async fn resolve_messages(
    client: &Client,
    gallery: Gallery,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let page = gallery_page_at_base(client, &gallery, &Url::parse("https://imgchest.com/")?).await?;
    let media = gallery_files(&page)?;
    Ok(gallery_messages_with_fields(&gallery, media, &page))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn matches_exact_public_gallery_shape_only() {
        assert_eq!(
            match_url("https://www.imgchest.com/p/abc123xyz45"),
            Some(Gallery {
                id: "abc123xyz45".into()
            })
        );
        for invalid in [
            "https://imgchest.com/u/user",
            "https://imgchest.com/p/short",
            "http://imgchest.com/p/abc123xyz45",
            "https://imgchest.com/p/abc123xyz45/extra",
            "https://imgchest.com/p/abc123xyz45?token=secret",
            "https://user:pass@imgchest.com/p/abc123xyz45",
            "https://fakeimgchest.com/p/abc123xyz45",
        ] {
            assert!(match_url(invalid).is_none(), "{invalid}");
        }
    }

    #[test]
    fn profile_routes_and_gallery_queue_ids_are_strictly_validated() {
        assert_eq!(
            match_user_url("https://imgchest.com/u/Public_User"),
            Some(UserProfile {
                username: "Public_User".into()
            })
        );
        for invalid in [
            "https://imgchest.com/u/",
            "https://imgchest.com/u/a%2Fb",
            "https://imgchest.com/u/name?token=x",
            "http://imgchest.com/u/name",
            "https://imgchest.com/u/name/other",
        ] {
            assert!(match_user_url(invalid).is_none(), "{invalid}");
        }
        let index = json!({"data":[
            {"id":"abc123xyz45","username":"alice","title":"Safe title","privacy":"public"},
            {"id":"abc123xyz45","username":"alice","title":"Duplicate"},
            {"id":"def456uvw78","username":"alice","privacy":"secret"},
            {"id":"ghi789jkl01","username":"someoneelse"},
            {"id":"bad/id","username":"alice"}
        ]});
        let mut seen = HashSet::new();
        let mut queue = Vec::new();
        assert_eq!(
            user_gallery_queues(&index, "alice", &mut seen, &mut queue).unwrap(),
            5
        );
        assert_eq!(queue.len(), 1);
        let value = serde_json::to_value(&queue[0]).unwrap();
        assert_eq!(value["type"], "queue");
        assert_eq!(value["url"], "https://imgchest.com/p/abc123xyz45");
        assert_eq!(value["metadata"]["gallery_id"], "abc123xyz45");
        assert_eq!(value["metadata"]["title"], "Safe title");
        assert!(user_gallery_queues(
            &json!({"data":vec![json!({"id":"abc123xyz45"}); MAX_USER_INDEX_RECORDS + 1]}),
            "alice",
            &mut seen,
            &mut queue,
        )
        .is_err());
    }

    #[test]
    fn profile_links_must_be_canonical_and_agree_with_record_identity() {
        let index = json!({"data":[
            {"link":"https://imgchest.com/p/abc123xyz45", "title":"Link only"},
            {"id":"def456uvw78", "link":"https://www.imgchest.com/p/def456uvw78"},
            {"id":"ghi789jkl01", "link":"https://attacker.example/p/ghi789jkl01"},
            {"id":"mno012pqr34", "link":"https://imgchest.com/p/not_the_id"},
            {"id":"stu345vwx67", "link":"https://imgchest.com/p/stu345vwx67?auth=secret"},
            {"id":"yzA678bcd90", "link":false}
        ]});
        let mut seen = HashSet::new();
        let mut output = Vec::new();
        assert_eq!(
            user_gallery_queues(&index, "alice", &mut seen, &mut output).unwrap(),
            6
        );
        assert_eq!(output.len(), 2);
        let urls: Vec<_> = output
            .iter()
            .map(|event| {
                serde_json::to_value(event).unwrap()["url"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        assert_eq!(
            urls,
            [
                "https://imgchest.com/p/abc123xyz45",
                "https://imgchest.com/p/def456uvw78"
            ]
        );
    }

    #[tokio::test]
    async fn profile_queue_paginates_locally_without_following_server_urls() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (page, id) in [
                (1, Some("abc123xyz45")),
                (2, Some("def456uvw78")),
                (3, None),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0_u8; 4096];
                let n = socket.read(&mut request).await.unwrap();
                let text = String::from_utf8_lossy(&request[..n]);
                assert!(text.starts_with("GET /api/posts?"), "{text}");
                assert!(text.contains(&format!("page={page}")), "{text}");
                assert!(text.contains("username=alice"), "{text}");
                let next = if page == 1 {
                    json!("https://untrusted.invalid/next")
                } else {
                    Value::Null
                };
                let records: Vec<Value> = id.map(|id| vec![json!({"id":id})]).unwrap_or_default();
                let body = json!({"data":records,"links":{"next":next}}).to_string();
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let api = Url::parse(&format!("http://{addr}/")).unwrap();
        let user = UserProfile {
            username: "alice".into(),
        };
        let events = queue_user_at_base(&Client::new(), &user, 3, &api)
            .await
            .unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(
            serde_json::to_value(&events[0]).unwrap()["url"],
            "https://imgchest.com/p/abc123xyz45"
        );
        assert_eq!(
            serde_json::to_value(&events[1]).unwrap()["url"],
            "https://imgchest.com/p/def456uvw78"
        );
        server.await.unwrap();
    }

    #[test]
    fn decodes_public_page_json_and_filters_untrusted_or_duplicate_media() {
        let data = json!({"props":{"post":{"files":[
            {"id":"abc123xyz45","link":"https://cdn.imgchest.com/files/abc123xyz45.jpg"},
            {"id":"abc123xyz45","link":"https://cdn.imgchest.com/files/abc123xyz45.jpg"},
            {"id":"abc123xyz45","link":"https://evil.example/files/abc123xyz45.jpg"},
            {"id":"wrong123456","link":"https://cdn.imgchest.com/files/abc123xyz45.png"},
            {"id":"foo123bar45","link":"https://cdn.imgchest.com/files/thumb/foo123bar45.png"}
        ]}}});
        let escaped = data.to_string().replace('"', "&quot;");
        let html = format!("<div data-page=\"{escaped}\"></div>");
        let files = parse_html(&html).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].filename, "abc123xyz45.jpg");
        assert_eq!(files[0].post_id, None);
    }
    #[test]
    fn typed_gallery_messages_preserve_alphanumeric_gallery_identity() {
        let gallery = Gallery {
            id: "abc123xyz45".into(),
        };
        let file = MediaItem {
            url: "https://cdn.imgchest.com/files/def456uvw78.webp".into(),
            filename: "def456uvw78.webp".into(),
            extractor: "imagechest",
            post_id: None,
        };
        let events = gallery_messages(&gallery, vec![file]);
        assert_eq!(events.len(), 2);
        let meta = serde_json::to_value(&events[0]).unwrap();
        assert_eq!(meta["metadata"]["gallery_id"], "abc123xyz45");
        assert_eq!(meta["metadata"]["category"], "imagechest");
        assert!(meta["metadata"].get("source_post_id").is_none());
        assert_eq!(
            serde_json::to_value(&events[1]).unwrap()["metadata"],
            meta["metadata"]
        );
    }

    #[test]
    fn typed_gallery_messages_preserve_validated_page_and_file_metadata() {
        let gallery = Gallery {
            id: "abc123xyz45".into(),
        };
        let url = "https://cdn.imgchest.com/files/def456uvw78.webp";
        let page = json!({"props":{"post":{
            "title":"Public gallery",
            "username":"alice",
            "image_count":1,
            "files":[{"id":"def456uvw78", "link":url, "position":1, "description":"Public image"}]
        }}});
        let media = gallery_files(&page).unwrap();
        let events = gallery_messages_with_fields(&gallery, media, &page);
        assert_eq!(events.len(), 2);
        let directory = serde_json::to_value(&events[0]).unwrap();
        let file = serde_json::to_value(&events[1]).unwrap();
        assert_eq!(file["url"], url);
        assert_eq!(directory["metadata"], file["metadata"]);
        let metadata = &file["metadata"];
        assert_eq!(metadata["title"], "Public gallery");
        assert_eq!(metadata["username"], "alice");
        assert_eq!(metadata["gallery_id"], "abc123xyz45");
        assert_eq!(metadata["file_id"], "def456uvw78");
        assert_eq!(metadata["position"], 1);
        assert_eq!(metadata["image_count"], 1);
        assert_eq!(metadata["description"], "Public image");
        assert!(metadata.get("source_post_id").is_none());
    }

    #[test]
    fn typed_gallery_drops_invalid_untrusted_scalar_metadata() {
        let gallery = Gallery {
            id: "abc123xyz45".into(),
        };
        let url = "https://cdn.imgchest.com/files/def456uvw78.webp";
        let page = json!({"props":{"post":{
            "title":"\u{0000}unsafe",
            "username":"name/invalid",
            "image_count":99999,
            "files":[{"id":"def456uvw78", "link":url, "position":99999, "description":"\u{0001}bad"}]
        }}});
        let events = gallery_messages_with_fields(&gallery, gallery_files(&page).unwrap(), &page);
        let file = serde_json::to_value(&events[1]).unwrap();
        for key in ["title","username","image_count","position","description"] {
            assert!(file["metadata"].get(key).is_none(), "{key}");
        }
        assert_eq!(file["metadata"]["file_id"], "def456uvw78");
    }

    #[test]
    fn rejects_invalid_page_shapes_and_excessive_gallery_records() {
        assert!(parse_html("<html>No public page JSON</html>").is_err());
        assert!(parse_html("<div data-page=\"{}\"></div>").is_err());
        let oversized = json!({"props":{"post":{
            "files": vec![json!({"id":"abc123xyz45"}); MAX_GALLERY_FILES + 1]
        }}});
        assert!(gallery_files(&oversized).is_err());
    }

    #[tokio::test]
    async fn local_public_gallery_fetches_one_page_without_media_downloads() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut headers = [0_u8; 4096];
            let n = socket.read(&mut headers).await.unwrap();
            let request = String::from_utf8_lossy(&headers[..n]);
            assert!(
                request.starts_with("GET /p/abc123xyz45 HTTP/1.1"),
                "{request}"
            );
            let page = json!({"props":{"post":{"files":[
                {"id":"abc123xyz45","link":"https://cdn.imgchest.com/files/abc123xyz45.png"},
                {"id":"def456uvw78","link":"https://cdn.imgchest.com/files/def456uvw78.webp"}
            ]}}});
            let escaped = page.to_string().replace('"', "&quot;");
            let body = format!("<section data-page=\"{escaped}\"></section>");
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(reply.as_bytes()).await.unwrap();
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let files = resolve_at_base(
            &Client::new(),
            Gallery {
                id: "abc123xyz45".into(),
            },
            &base,
        )
        .await
        .unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].filename, "abc123xyz45.png");
        assert_eq!(files[1].filename, "def456uvw78.webp");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn rejects_declared_oversized_gallery_before_reading_body() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 4096];
            let _ = socket.read(&mut request).await.unwrap();
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                MAX_GALLERY_HTML_BYTES + 1
            );
            socket.write_all(reply.as_bytes()).await.unwrap();
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let error = resolve_at_base(
            &Client::new(),
            Gallery {
                id: "abc123xyz45".into(),
            },
            &base,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("response limit"));
        server.await.unwrap();
    }
}
