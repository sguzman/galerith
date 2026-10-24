//! Guarded public Kabeuchi image-post user preview from pinned kabeuchi.py.
use crate::{
    message::{media_metadata, ExtractorMessage, Metadata},
    safe_filename, MediaItem,
};
use reqwest::Client;
use serde_json::Value;
use std::{collections::HashSet, error::Error, time::Duration};
use url::Url;

const MAX_HTML: usize = 2 * 1024 * 1024;
const MAX_JSON: usize = 2 * 1024 * 1024;
const MAX_PAGES: u16 = 4;
const MAX_POSTS_PER_PAGE: usize = 48;
const MAX_MEDIA: usize = 160;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    id: String,
}

fn numeric_id(s: &str) -> bool {
    (1..=16).contains(&s.len())
        && s.bytes().all(|b| b.is_ascii_digit())
        && s.parse::<u64>().ok().is_some_and(|v| v > 0)
}

pub fn match_url(raw: &str) -> Option<User> {
    let lower = raw.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "/../", "/./"]
        .iter()
        .any(|v| lower.contains(v))
    {
        return None;
    }
    let url = Url::parse(raw).ok()?;
    if !matches!(url.scheme(), "https" | "http")
        || url.host_str() != Some("kabe-uchiroom.com")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "/mypage" | "/mypage/")
    {
        return None;
    }
    let query: Vec<_> = url.query_pairs().collect();
    if query.len() != 1 || query[0].0 != "id" || !numeric_id(&query[0].1) {
        return None;
    }
    Some(User {
        id: query[0].1.to_string(),
    })
}

async fn bytes(
    response: reqwest::Response,
    size: usize,
) -> Result<Vec<u8>, Box<dyn Error + Send + Sync>> {
    let mut response = response.error_for_status()?;
    if response.content_length().is_some_and(|n| n > size as u64) {
        return Err("Kabeuchi source response exceeds byte cap".into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > size.saturating_sub(body.len()) {
            return Err("Kabeuchi source response exceeds byte cap".into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn parse_target(html: &str) -> Option<String> {
    let id = html.split_once("user_friend_id = \"")?.1.split_once('"')?.0;
    numeric_id(id).then(|| id.to_owned())
}

fn safe_image_name(name: &str) -> bool {
    if name.len() > 200
        || name.is_empty()
        || name.chars().any(char::is_control)
        || name.contains('/')
        || name.contains('\\')
        || name.contains('%')
        || name.contains("..")
    {
        return false;
    }
    let Some((_, ext)) = name.rsplit_once('.') else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "jpg" | "jpeg" | "png" | "gif" | "webp"
    )
}

fn serial_id(value: &Value) -> Option<String> {
    let scalar = value
        .as_str()
        .map(ToOwned::to_owned)
        .or_else(|| value.as_u64().map(|n| n.to_string()))?;
    numeric_id(&scalar).then_some(scalar)
}

fn read_post(
    value: &Value,
    user: &User,
    media: &mut Vec<(MediaItem, Metadata)>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    if value.get("is_ad").and_then(Value::as_bool).unwrap_or(false) {
        return Ok(());
    }
    let Some(post_id) = value.get("id").and_then(serial_id) else {
        return Ok(());
    };
    let Some(first) = value.get("image1").and_then(Value::as_str) else {
        return Ok(());
    };
    if first.is_empty() {
        return Ok(());
    }
    let uid_final = user.id.as_bytes().last().ok_or("Empty Kabeuchi ID")?;
    let mut common = Metadata::new();
    common.insert("category".into(), "kabeuchi".into());
    common.insert("subcategory".into(), "user".into());
    common.insert("user_id".into(), user.id.clone().into());
    common.insert("id".into(), post_id.clone().into());
    common.insert(
        "post_url".into(),
        format!("https://kabe-uchiroom.com/mypage/?id={}", user.id).into(),
    );
    for key in ["title", "twitter_user_id", "twitter_id"] {
        if let Some(field) = value
            .get(key)
            .and_then(Value::as_str)
            .filter(|s| s.len() <= 240 && !s.chars().any(char::is_control))
        {
            common.insert(key.into(), field.into());
        }
    }
    if let Some(date) = value
        .get("created_at")
        .and_then(Value::as_str)
        .filter(|s| s.len() <= 48 && !s.chars().any(char::is_control))
    {
        common.insert("date_source".into(), date.into());
    }
    for num in 1..=6 {
        let key = format!("image{num}");
        let Some(name) = value.get(&key).and_then(Value::as_str) else {
            break;
        };
        if name.is_empty() {
            break;
        }
        if !safe_image_name(name) {
            return Err(
                "Kabeuchi source image name includes an unsafe path or unsupported extension"
                    .into(),
            );
        }
        if media.len() >= MAX_MEDIA {
            return Err("Kabeuchi public source exceeds media cap".into());
        }
        let mut url = Url::parse("https://kabe-uchiroom.com/")?;
        url.path_segments_mut()
            .map_err(|()| "Kabeuchi media root has no path segments")?
            .extend([
                "accounts",
                "upfile",
                std::str::from_utf8(&[*uid_final])?,
                &user.id,
                name,
            ]);
        let filename = safe_filename(&format!("{post_id}_{num:02}_{name}"));
        let item = MediaItem {
            url: url.into(),
            filename,
            extractor: "kabeuchi",
            post_id: post_id.parse::<u64>().ok(),
        };
        let mut metadata = media_metadata(&item);
        for (key, field) in &common {
            metadata.insert(key.clone(), field.clone());
        }
        metadata.insert("num".into(), num.into());
        media.push((item, metadata));
    }
    Ok(())
}

fn decode_posts(
    root: &Value,
    user: &User,
    media: &mut Vec<(MediaItem, Metadata)>,
) -> Result<Option<String>, Box<dyn Error + Send + Sync>> {
    let posts = root
        .get("datas")
        .and_then(Value::as_array)
        .ok_or("Kabeuchi posts response lacks a public data array")?;
    if posts.len() > MAX_POSTS_PER_PAGE {
        return Err("Kabeuchi public posts page exceeds bounded source size".into());
    }
    for post in posts {
        read_post(post, user, media)?;
    }
    if posts.is_empty() {
        return Ok(None);
    }
    let last = posts
        .last()
        .and_then(|entry| entry.get("id"))
        .and_then(serial_id)
        .ok_or("Kabeuchi public page lacks a usable pagination ID")?;
    let terminal = root
        .get("last_data")
        .and_then(serial_id)
        .ok_or("Kabeuchi posts response lacks a terminal page identifier")?;
    Ok((last != terminal).then_some(last))
}

async fn at_base(
    client: &Client,
    base: &Url,
    user: &User,
    max_pages: u16,
) -> Result<Vec<(MediaItem, Metadata)>, Box<dyn Error + Send + Sync>> {
    let mut url = base.join("mypage/")?;
    url.query_pairs_mut().append_pair("id", &user.id);
    let response = client.get(url.clone()).send().await?;
    if response.url().origin() != url.origin() {
        return Err("Kabeuchi profile redirected outside its source origin".into());
    }
    let html = String::from_utf8(bytes(response, MAX_HTML).await?)?;
    let target_id = parse_target(&html)
        .ok_or("Kabeuchi public page does not expose a validated upload target ID")?;
    let mut media = Vec::new();
    let mut previous = String::new();
    let mut seen = HashSet::new();
    for page in 0..max_pages.clamp(1, MAX_PAGES) {
        if page > 0 {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        let endpoint = base.join("get_posts.php")?;
        let fields = [
            ("user_id", "0".to_owned()),
            ("target_id", target_id.clone()),
            ("type", "uploads".to_owned()),
            ("sort_type", "0".to_owned()),
            ("category_id", "all".to_owned()),
            ("latest_post", previous.clone()),
            ("page_num", page.to_string()),
        ];
        let response = client.post(endpoint.clone()).form(&fields).send().await?;
        if response.url().origin() != endpoint.origin() {
            return Err("Kabeuchi pagination redirected outside its source origin".into());
        }
        let decoded: Value = serde_json::from_slice(&bytes(response, MAX_JSON).await?)?;
        let next = decode_posts(&decoded, user, &mut media)?;
        let Some(next) = next else { break };
        if !seen.insert(next.clone()) {
            return Err("Kabeuchi pagination repeated its previous post cursor".into());
        }
        previous = next;
    }
    Ok(media)
}

pub async fn resolve(
    client: &Client,
    user: User,
    max_pages: u16,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse("https://kabe-uchiroom.com/")?;
    Ok(at_base(client, &base, &user, max_pages)
        .await?
        .into_iter()
        .map(|(item, _)| item)
        .collect())
}

pub async fn resolve_messages(
    client: &Client,
    user: User,
    max_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse("https://kabe-uchiroom.com/")?;
    let mut events = Vec::new();
    for (item, metadata) in at_base(client, &base, &user, max_pages).await? {
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

    #[test]
    fn numeric_public_profile_routes_reject_unsafe_queries_and_hosts() {
        assert_eq!(
            match_url("https://kabe-uchiroom.com/mypage/?id=12345")
                .unwrap()
                .id,
            "12345"
        );
        for bad in [
            "https://fakekabe-uchiroom.com/mypage/?id=12345",
            "https://kabe-uchiroom.com/mypage/?id=0",
            "https://kabe-uchiroom.com/mypage/?id=notanid",
            "https://kabe-uchiroom.com/mypage/?id=12345&secret=1",
            "https://name:pass@kabe-uchiroom.com/mypage/?id=12345",
            "https://kabe-uchiroom.com:8443/mypage/?id=12345",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn validated_public_posts_emit_ordered_media_and_skip_ads() {
        let user = match_url("https://kabe-uchiroom.com/mypage/?id=12345").unwrap();
        let info = serde_json::json!({"datas":[
            {"id":1001,"image1":"first.jpg","image2":"second.png","image3":"","title":"Artwork","created_at":"2026-04-06T11:00:00Z"},
            {"id":1002,"image1":"advert.jpg","is_ad":true},
            {"id":1003,"image1":""}
        ],"last_data":1003});
        let mut media = Vec::new();
        assert_eq!(decode_posts(&info, &user, &mut media).unwrap(), None);
        assert_eq!(media.len(), 2);
        assert_eq!(
            media[0].0.url,
            "https://kabe-uchiroom.com/accounts/upfile/5/12345/first.jpg"
        );
        assert_eq!(media[1].0.filename, "1001_02_second.png");
        assert_eq!(media[0].1["date_source"], "2026-04-06T11:00:00Z");
        let evil =
            serde_json::json!({"datas":[{"id":1001,"image1":"../secret.jpg"}],"last_data":1001});
        assert!(decode_posts(&evil, &user, &mut Vec::new()).is_err());
    }

    #[test]
    fn validates_target_ids_and_detects_repeated_page_cursors() {
        assert_eq!(
            parse_target(r#"user_friend_id = "12345""#),
            Some("12345".into())
        );
        assert_eq!(parse_target(r#"user_friend_id = "private/token""#), None);
        assert!(!safe_image_name("bad/secret.jpg"));
        assert!(!safe_image_name("evil%2fsecret.png"));
    }

    #[tokio::test]
    async fn bounded_upload_posts_follow_two_cursor_pages_in_order() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (method_path, body) in [
                (
                    "GET /mypage/?id=12345",
                    r#"<script>user_friend_id = "999"</script>"#,
                ),
                (
                    "POST /get_posts.php",
                    r#"{"datas":[{"id":1001,"image1":"first.jpg"}],"last_data":1002}"#,
                ),
                (
                    "POST /get_posts.php",
                    r#"{"datas":[{"id":1002,"image1":"second.png"}],"last_data":1002}"#,
                ),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0u8; 8192];
                let count = socket.read(&mut request).await.unwrap();
                assert!(
                    String::from_utf8_lossy(&request[..count]).starts_with(method_path)
                );
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{address}/")).unwrap();
        let user = match_url("https://kabe-uchiroom.com/mypage/?id=12345").unwrap();
        let media = at_base(&Client::new(), &base, &user, 3).await.unwrap();
        assert_eq!(media.len(), 2);
        assert_eq!(media[0].0.filename, "1001_01_first.jpg");
        assert_eq!(media[1].0.filename, "1002_01_second.png");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn public_profile_plus_form_post_are_bounded_local_requests() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (path, body) in [
                (
                    "GET /mypage/?id=12345",
                    r#"<script>user_friend_id = "999"</script>"#,
                ),
                (
                    "POST /get_posts.php",
                    r#"{"datas":[{"id":1001,"image1":"first.jpg"}],"last_data":1001}"#,
                ),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0u8; 8192];
                let size = socket.read(&mut request).await.unwrap();
                assert!(String::from_utf8_lossy(&request[..size]).starts_with(path));
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{address}/")).unwrap();
        let user = match_url("https://kabe-uchiroom.com/mypage/?id=12345").unwrap();
        let images = at_base(&Client::new(), &base, &user, 2).await.unwrap();
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].0.filename, "1001_01_first.jpg");
        server.await.unwrap();
    }
}
