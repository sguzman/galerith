//! Independent public ImageShack single-image REST preview.
//! Auth, album/user pagination and full upstream metadata remain unaccepted.
use crate::{
    message::{media_metadata, ExtractorMessage, Metadata},
    safe_filename, MediaItem,
};
use reqwest::Client;
use serde_json::Value;
use std::{
    error::Error,
    time::{SystemTime, UNIX_EPOCH},
};
use url::Url;

const MAX_API_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub id: String,
}

fn valid_id(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

pub fn match_url(input: &str) -> Option<Image> {
    let url = Url::parse(input).ok()?;
    if url.scheme() != "https"
        || !matches!(
            url.host_str(),
            Some("imageshack.com" | "www.imageshack.com")
        )
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let parts: Vec<_> = url.path_segments()?.collect();
    match parts.as_slice() {
        ["i", id] | ["i", id, ""] if valid_id(id) => Some(Image {
            id: (*id).to_owned(),
        }),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Listing {
    Album(String),
    User(String),
}

fn valid_username(value: &str) -> bool {
    (1..=80).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

pub fn match_listing_url(input: &str) -> Option<Listing> {
    let url = Url::parse(input).ok()?;
    if url.scheme() != "https"
        || !matches!(
            url.host_str(),
            Some("imageshack.com" | "www.imageshack.com")
        )
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let path: Vec<_> = url.path_segments()?.collect();
    match path.as_slice() {
        ["a", id] | ["a", id, "1"] if valid_id(id) => Some(Listing::Album((*id).to_owned())),
        ["user", username] if valid_username(username) => {
            Some(Listing::User((*username).to_owned()))
        }
        _ => None,
    }
}

const MAX_LIST_PAGES: u16 = 4;
const PAGE_LIMIT: usize = 40;
const MAX_LIST_IMAGES: usize = 160;

fn listed_images(
    response: &Value,
    listing: &Listing,
    seen: &mut std::collections::HashSet<String>,
    files: &mut Vec<(MediaItem, Metadata)>,
) -> Result<(usize, Option<u64>), Box<dyn Error + Send + Sync>> {
    let result = response
        .get("result")
        .and_then(Value::as_object)
        .ok_or("ImageShack listing API has no result object")?;
    let records = result
        .get("images")
        .and_then(Value::as_array)
        .ok_or("ImageShack listing API has no images array")?;
    if records.len() > PAGE_LIMIT {
        return Err("ImageShack listing exceeds the accepted page size".into());
    }
    let album_title = result.get("title").and_then(Value::as_str).filter(|title| {
        !title.is_empty() && title.len() <= 256 && !title.chars().any(char::is_control)
    });
    for record in records {
        let Some(id) = record.get("id").and_then(Value::as_str) else {
            continue;
        };
        if !valid_id(id) {
            continue;
        }
        let wrapped = serde_json::json!({"result":record});
        let Ok((media, mut metadata)) = parse_image(&wrapped, id) else {
            continue;
        };
        match listing {
            Listing::Album(album_id) => {
                metadata.insert("gallery_id".into(), album_id.clone().into());
                if let Some(title) = album_title {
                    metadata.insert("gallery_title".into(), title.into());
                }
            }
            Listing::User(username) => {
                metadata.insert("profile".into(), username.clone().into());
            }
        }
        if seen.insert(media.url.clone()) {
            files.push((media, metadata));
            if files.len() >= MAX_LIST_IMAGES {
                break;
            }
        }
    }
    let total = result.get("total").and_then(Value::as_u64);
    Ok((records.len(), total))
}

async fn listing_at_base(
    client: &Client,
    listing: &Listing,
    max_pages: u16,
    base: &Url,
) -> Result<Vec<(MediaItem, Metadata)>, Box<dyn Error + Send + Sync>> {
    let route = match listing {
        Listing::Album(id) => format!("rest_api/v2/albums/{id}"),
        Listing::User(_) => "rest_api/v2/images".to_owned(),
    };
    let mut files = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for page in 0..max_pages.clamp(1, MAX_LIST_PAGES) {
        let offset = page as usize * PAGE_LIMIT;
        let mut endpoint = base.join(&route)?;
        {
            let mut pairs = endpoint.query_pairs_mut();
            pairs
                .append_pair("limit", &PAGE_LIMIT.to_string())
                .append_pair("offset", &offset.to_string())
                .append_pair("hide_empty", "true");
            match listing {
                Listing::Album(_) => {
                    pairs.append_pair("show_private", "false");
                }
                Listing::User(username) => {
                    pairs.append_pair("username", username);
                }
            }
            let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
            pairs.append_pair("ts", &timestamp.to_string());
        }
        let value = super::http_json::fetch_json_limited(client, endpoint, MAX_API_BYTES).await?;
        let (count, total) = listed_images(&value, listing, &mut seen, &mut files)?;
        if count == 0
            || count < PAGE_LIMIT
            || files.len() >= MAX_LIST_IMAGES
            || total.is_some_and(|n| (offset + count) as u64 >= n)
        {
            break;
        }
        if page + 1 < max_pages.min(MAX_LIST_PAGES) {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    }
    Ok(files)
}

pub async fn resolve_listing(
    client: &Client,
    listing: Listing,
    max_pages: u16,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let files = listing_at_base(
        client,
        &listing,
        max_pages,
        &Url::parse("https://imageshack.com/")?,
    )
    .await?;
    Ok(files.into_iter().map(|(item, _)| item).collect())
}

pub async fn resolve_listing_messages(
    client: &Client,
    listing: Listing,
    max_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let files = listing_at_base(
        client,
        &listing,
        max_pages,
        &Url::parse("https://imageshack.com/")?,
    )
    .await?;
    let mut events = Vec::with_capacity(files.len() * 2);
    for (item, metadata) in files {
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

fn safe_media(raw: &str) -> Option<(Url, String)> {
    let expanded = if raw.starts_with("https://") {
        raw.to_owned()
    } else if raw.starts_with("//") {
        format!("https:{raw}")
    } else if raw.starts_with("imagizer.imageshack.com/") || raw.starts_with("imageshack.com/") {
        format!("https://{raw}")
    } else {
        return None;
    };
    let url = Url::parse(&expanded).ok()?;
    let host = url.host_str()?;
    if url.scheme() != "https"
        || !(host == "imageshack.com" || host.ends_with(".imageshack.com"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let name = url.path_segments()?.next_back()?;
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    if !matches!(
        ext.as_str(),
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif"
    ) || name.is_empty()
        || name.len() > 240
        || name.contains('%')
    {
        return None;
    }
    let filename = safe_filename(name);
    Some((url, filename))
}

fn safe_original_name(value: &str) -> Option<String> {
    if value.is_empty()
        || value.len() > 240
        || value.contains('/')
        || value.contains('\\')
        || value.chars().any(char::is_control)
        || value.starts_with('.')
    {
        return None;
    }
    let ext = value.rsplit_once('.')?.1.to_ascii_lowercase();
    if !matches!(
        ext.as_str(),
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif"
    ) {
        return None;
    }
    Some(safe_filename(value))
}

fn parse_image(
    data: &Value,
    expected_id: &str,
) -> Result<(MediaItem, Metadata), Box<dyn Error + Send + Sync>> {
    let image = data
        .get("result")
        .and_then(Value::as_object)
        .ok_or("ImageShack API response lacks image result")?;
    let id = image
        .get("id")
        .and_then(Value::as_str)
        .ok_or("ImageShack API result lacks an image ID")?;
    if !valid_id(id) || id != expected_id {
        return Err("ImageShack API result ID differs from requested image".into());
    }
    if image.get("public") == Some(&Value::Bool(false)) {
        return Err("ImageShack private image is outside the public preview".into());
    }
    let link = image
        .get("direct_link")
        .and_then(Value::as_str)
        .ok_or("ImageShack API result has no direct media link")?;
    let (url, fallback_name) =
        safe_media(link).ok_or("ImageShack API returned an unsupported media origin")?;
    let filename = image
        .get("original_filename")
        .and_then(Value::as_str)
        .and_then(safe_original_name)
        .unwrap_or(fallback_name);
    let item = MediaItem {
        url: url.to_string(),
        filename,
        extractor: "imageshack",
        post_id: None,
    };
    let mut metadata = media_metadata(&item);
    metadata.insert("subcategory".into(), "image".into());
    metadata.insert("file_id".into(), id.into());
    for axis in ["width", "height"] {
        if let Some(value) = image
            .get(axis)
            .and_then(Value::as_u64)
            .filter(|v| (1..=32768).contains(v))
        {
            metadata.insert(axis.into(), value.into());
        }
    }
    if let Some(date) = image
        .get("creation_date")
        .and_then(Value::as_u64)
        .filter(|v| (946684800..=4102444800).contains(v))
    {
        metadata.insert("creation_date".into(), date.into());
    }
    if let Some(title) = image
        .get("title")
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty() && v.len() <= 256 && !v.chars().any(char::is_control))
    {
        metadata.insert("title".into(), title.into());
    }
    if let Some(owner) = image
        .get("owner")
        .and_then(|owner| owner.get("username"))
        .and_then(Value::as_str)
        .filter(|v| {
            !v.is_empty()
                && v.len() <= 80
                && v.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
        })
    {
        metadata.insert("username".into(), owner.into());
    }
    Ok((item, metadata))
}

async fn resolve_at_base(
    client: &Client,
    image: &Image,
    base: &Url,
) -> Result<(MediaItem, Metadata), Box<dyn Error + Send + Sync>> {
    let mut endpoint = base.join(&format!("rest_api/v2/images/{}", image.id))?;
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    endpoint
        .query_pairs_mut()
        .append_pair("ts", &timestamp.to_string());
    let data = super::http_json::fetch_json_limited(client, endpoint, MAX_API_BYTES).await?;
    parse_image(&data, &image.id)
}

pub async fn resolve(
    client: &Client,
    image: Image,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let (file, _) =
        resolve_at_base(client, &image, &Url::parse("https://imageshack.com/")?).await?;
    Ok(vec![file])
}

pub async fn resolve_messages(
    client: &Client,
    image: Image,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let (file, metadata) =
        resolve_at_base(client, &image, &Url::parse("https://imageshack.com/")?).await?;
    Ok(vec![
        ExtractorMessage::Directory {
            metadata: metadata.clone(),
        },
        ExtractorMessage::Url {
            url: file.url,
            metadata,
        },
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    #[test]
    fn public_image_urls_require_supported_host_and_safe_id() {
        assert_eq!(
            match_url("https://imageshack.com/i/iq8ac04fj"),
            Some(Image {
                id: "iq8ac04fj".into()
            })
        );
        for bad in [
            "http://imageshack.com/i/iq8ac04fj",
            "https://imageshack.com/a/OGtz",
            "https://evilimageshack.com/i/iq8ac04fj",
            "https://imageshack.com/i/iq8ac04fj?auth=1",
            "https://imageshack.com/i/iq8ac04fj#section",
            "https://imageshack.com/i/a%2Fb",
            "https://imageshack.com/i/a/other",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn image_api_parses_pinned_public_image_shape_and_optional_metadata() {
        let source = json!({"result":{
            "id":"iq8ac04fj",
            "public":true,
            "direct_link":"imagizer.imageshack.com/img674/3530/8ac04f.jpg",
            "original_filename":"247fa952439aa690b7d28494f88b8c2b.jpg",
            "title":"Up in the treetops",
            "width":364,
            "height":512,
            "creation_date":1405450816,
            "owner":{"username":"TomKovilick"}
        }});
        let (file, metadata) = parse_image(&source, "iq8ac04fj").unwrap();
        assert_eq!(
            file.url,
            "https://imagizer.imageshack.com/img674/3530/8ac04f.jpg"
        );
        assert_eq!(file.filename, "247fa952439aa690b7d28494f88b8c2b.jpg");
        assert_eq!(file.post_id, None);
        assert_eq!(metadata["file_id"], "iq8ac04fj");
        assert_eq!(metadata["username"], "TomKovilick");
        assert_eq!(metadata["width"], 364);
        assert_eq!(metadata["height"], 512);
    }

    #[test]
    fn public_album_and_user_routes_are_strict_and_follow_pinned_shapes() {
        assert_eq!(
            match_listing_url("https://imageshack.com/a/OGtz/1"),
            Some(Listing::Album("OGtz".into()))
        );
        assert_eq!(
            match_listing_url("https://imageshack.com/user/Ezekiel.Gonzalez"),
            Some(Listing::User("Ezekiel.Gonzalez".into()))
        );
        for bad in [
            "http://imageshack.com/a/OGtz",
            "https://imageshack.com/a/OGtz/9",
            "https://imageshack.com/a/OGtz?private=1",
            "https://imageshack.com/user/a%2Fb",
            "https://imageshack.com/user/a?token=secret",
            "https://evilimageshack.com/a/OGtz",
        ] {
            assert!(match_listing_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn list_projects_safe_unique_public_records_and_gallery_title() {
        let records = json!({"result":{
            "title":"Public Gallery", "total":4,
            "images":[
                {"id":"alpha123", "direct_link":"imagizer.imageshack.com/a.jpg", "public":true},
                {"id":"alpha123", "direct_link":"imagizer.imageshack.com/a.jpg", "public":true},
                {"id":"private123", "direct_link":"imagizer.imageshack.com/private.jpg", "public":false},
                {"id":"bad123", "direct_link":"https://attacker.example/evil.jpg"}
            ]
        }});
        let mut seen = std::collections::HashSet::new();
        let mut files = Vec::new();
        let (count, total) = listed_images(
            &records,
            &Listing::Album("OGtz".into()),
            &mut seen,
            &mut files,
        )
        .unwrap();
        assert_eq!(count, 4);
        assert_eq!(total, Some(4));
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].1["gallery_id"], "OGtz");
        assert_eq!(files[0].1["gallery_title"], "Public Gallery");
    }

    #[tokio::test]
    async fn user_index_paginates_40_plus_1_records_without_duplicate_downloads() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server =
            tokio::spawn(async move {
                for page in 0..2 {
                    let (mut socket, _) = listener.accept().await.unwrap();
                    let mut request = [0_u8; 4096];
                    let n = socket.read(&mut request).await.unwrap();
                    let req = String::from_utf8_lossy(&request[..n]);
                    assert!(req.starts_with("GET /rest_api/v2/images?limit=40"), "{req}");
                    assert!(req.contains(&format!("offset={}", page * 40)), "{req}");
                    assert!(req.contains("username=alice"), "{req}");
                    let images: Vec<Value> =
                        if page == 0 {
                            (0..40).map(|id| json!({
                        "id":format!("file{id:03}"),
                        "public":true,
                        "direct_link":format!("imagizer.imageshack.com/img/{id:03}.jpg")
                    })).collect()
                        } else {
                            vec![json!({
                                "id":"file000",
                                "public":true,
                                "direct_link":"imagizer.imageshack.com/img/000.jpg"
                            })]
                        };
                    let body = json!({"result":{"total":41,"images":images}}).to_string();
                    let reply = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    socket.write_all(reply.as_bytes()).await.unwrap();
                }
            });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let files = listing_at_base(&Client::new(), &Listing::User("alice".into()), 3, &base)
            .await
            .unwrap();
        assert_eq!(files.len(), 40);
        assert_eq!(files[0].1["profile"], "alice");
        server.await.unwrap();
    }

    #[test]
    fn rejects_private_mismatched_and_untrusted_media_records() {
        let base = json!({"result":{
            "id":"iq8ac04fj",
            "direct_link":"https://evil.example/image.jpg"
        }});
        assert!(parse_image(&base, "iq8ac04fj").is_err());
        assert!(parse_image(
            &json!({"result":{
                "id":"not-matching", "direct_link":"imagizer.imageshack.com/foo.jpg"
            }}),
            "iq8ac04fj"
        )
        .is_err());
        assert!(parse_image(
            &json!({"result":{
                "id":"iq8ac04fj", "public":false,
                "direct_link":"imagizer.imageshack.com/foo.jpg"
            }}),
            "iq8ac04fj"
        )
        .is_err());
        for bad in [
            "http://imagizer.imageshack.com/a.jpg",
            "https://imagizer.imageshack.com/a.jpg?token=x",
            "https://user:password@imagizer.imageshack.com/a.jpg",
            "https://imagizer.imageshack.com/a.exe",
            "https://evilimageshack.com/a.jpg",
        ] {
            assert!(safe_media(bad).is_none(), "{bad}");
        }
        assert!(safe_original_name("../unsafe.jpg").is_none());
        assert!(safe_original_name("dir/name.jpg").is_none());
    }

    #[tokio::test]
    async fn loopback_public_image_api_uses_bounded_read_and_validates_identity() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut data = [0_u8; 4096];
            let n = socket.read(&mut data).await.unwrap();
            let request = String::from_utf8_lossy(&data[..n]);
            assert!(
                request.starts_with("GET /rest_api/v2/images/iq8ac04fj?ts="),
                "{request}"
            );
            let body = json!({"result":{
                "id":"iq8ac04fj",
                "public":true,
                "direct_link":"//imagizer.imageshack.com/img674/3530/8ac04f.jpg"
            }})
            .to_string();
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(reply.as_bytes()).await.unwrap();
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let (file, metadata) = resolve_at_base(
            &Client::new(),
            &Image {
                id: "iq8ac04fj".into(),
            },
            &base,
        )
        .await
        .unwrap();
        assert_eq!(file.filename, "8ac04f.jpg");
        assert_eq!(metadata["file_id"], "iq8ac04fj");
        server.await.unwrap();
    }
}
