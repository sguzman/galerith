//! Conservative public ImageChest gallery preview. No API access token or
//! account/user profile support is implied by this read-only HTML adapter.
use crate::{safe_filename, MediaItem};
use reqwest::Client;
use serde_json::Value;
use std::{collections::HashSet, error::Error};
use url::Url;

const MAX_GALLERY_HTML_BYTES: usize = 2 * 1024 * 1024;
const MAX_GALLERY_FILES: usize = 256;

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

fn parse_html(body: &str) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
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
    let value: Value = serde_json::from_str(&decoded)?;
    gallery_files(&value)
}

/// Only the registered public gallery URL is accepted in normal dispatch.
/// The base URL parameter provides an offline loopback seam for tests.
async fn resolve_at_base(
    client: &Client,
    gallery: Gallery,
    base: &Url,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
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
    parse_html(std::str::from_utf8(&body)?)
}

pub async fn resolve(
    client: &Client,
    gallery: Gallery,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    resolve_at_base(client, gallery, &Url::parse("https://imgchest.com/")?).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn matches_exact_public_gallery_shape_only() {
        assert_eq!(
            match_url("https://www.imgchest.com/p/abc123xyz45"),
            Some(Gallery { id: "abc123xyz45".into() })
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
}
