//! Guarded public Danke fürs Lesen manga chapter images and chapter queues.
use crate::{
    message::{media_metadata, queued_url, ExtractorMessage, Metadata},
    MediaItem,
};
use reqwest::Client;
use serde_json::Value;
use std::error::Error;
use url::Url;

const MAX_JSON: usize = 2 * 1024 * 1024;
const MAX_CHAPTER_IMAGES: usize = 240;
const MAX_CHAPTERS: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Manga(String),
    Chapter { slug: String, chapter: String },
}

fn safe_component(s: &str) -> bool {
    (1..=96).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub fn match_url(raw: &str) -> Option<Target> {
    let lower = raw.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "/../", "/./"]
        .iter()
        .any(|needle| lower.contains(needle))
    {
        return None;
    }
    let url = Url::parse(raw).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !matches!(url.host_str(), Some("danke.moe" | "www.danke.moe"))
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
        [route @ ("read" | "reader"), kind @ ("manga" | "series"), slug]
        | [route @ ("read" | "reader"), kind @ ("manga" | "series"), slug, ""]
            if !route.is_empty() && !kind.is_empty() && safe_component(slug) =>
        {
            Some(Target::Manga((*slug).to_owned()))
        }
        [("read" | "reader"), ("manga" | "series"), slug, chapter]
        | [("read" | "reader"), ("manga" | "series"), slug, chapter, ""]
        | [("read" | "reader"), ("manga" | "series"), slug, chapter, "1"]
        | [("read" | "reader"), ("manga" | "series"), slug, chapter, "1", ""]
            if safe_component(slug) && safe_component(chapter) =>
        {
            Some(Target::Chapter {
                slug: (*slug).to_owned(),
                chapter: (*chapter).to_owned(),
            })
        }
        _ => None,
    }
}

async fn manga_json(
    client: &Client,
    base: &Url,
    slug: &str,
) -> Result<Value, Box<dyn Error + Send + Sync>> {
    let endpoint = base.join(&format!("api/series/{slug}/"))?;
    let mut response = client
        .get(endpoint.clone())
        .send()
        .await?
        .error_for_status()?;
    if response.url().origin() != endpoint.origin() {
        return Err("Danke manga API redirected outside its origin".into());
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_JSON as u64)
    {
        return Err("Danke manga API exceeds response limit".into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_JSON.saturating_sub(body.len()) {
            return Err("Danke manga API exceeds response limit".into());
        }
        body.extend_from_slice(&chunk);
    }
    let value: Value = serde_json::from_slice(&body)?;
    if value.get("slug").and_then(Value::as_str) != Some(slug) {
        return Err("Danke API manga slug differs from requested source".into());
    }
    Ok(value)
}

fn chapter_key(chapter: &str) -> String {
    chapter.replace('-', ".")
}

fn source_metadata(value: &Value, slug: &str) -> Metadata {
    let mut result = Metadata::new();
    result.insert("category".into(), "dankefuerslesen".into());
    result.insert("subcategory".into(), "chapter".into());
    result.insert("manga_slug".into(), slug.into());
    for key in ["title", "author", "artist", "description"] {
        if let Some(field) = value
            .get(key)
            .filter(|field| field.is_string() || field.is_array())
        {
            result.insert(
                if key == "title" { "manga" } else { key }.into(),
                field.clone(),
            );
        }
    }
    result
}

fn chapter_media(
    source: &Value,
    slug: &str,
    chapter: &str,
) -> Result<Vec<(MediaItem, Metadata)>, Box<dyn Error + Send + Sync>> {
    let normalized = chapter_key(chapter);
    let data = source
        .pointer("/chapters")
        .and_then(|v| v.get(&normalized))
        .ok_or("Danke source manga has no requested chapter")?;
    let groups = data
        .get("groups")
        .and_then(Value::as_object)
        .ok_or("Danke chapter lacks image groups")?;
    if groups.len() != 1 {
        return Err(
            "Danke chapters with zero or multiple scanlation groups are not yet supported".into(),
        );
    }
    let (group_id, originals) = groups
        .iter()
        .next()
        .ok_or("Danke source chapter group missing")?;
    if !safe_component(group_id) {
        return Err("Danke chapter has an unsafe group ID".into());
    }
    let files = originals
        .as_array()
        .ok_or("Danke group media is not an image list")?;
    if files.is_empty() || files.len() > MAX_CHAPTER_IMAGES {
        return Err("Danke chapter image count outside bounded support".into());
    }
    let folder = data
        .get("folder")
        .and_then(Value::as_str)
        .filter(|folder| safe_component(folder))
        .ok_or("Danke chapter media folder is unsafe")?;
    let mut metadata = source_metadata(source, slug);
    metadata.insert("chapter_key".into(), normalized.into());
    metadata.insert("group_id".into(), group_id.clone().into());
    metadata.insert("count".into(), files.len().into());
    metadata.insert(
        "post_url".into(),
        format!("https://danke.moe/read/manga/{slug}/{chapter}/1/").into(),
    );
    if let Some(title) = data
        .get("title")
        .and_then(Value::as_str)
        .filter(|title| title.len() <= 240 && !title.chars().any(char::is_control))
    {
        metadata.insert("title".into(), title.into());
    }
    if let Some(volume) = data
        .get("volume")
        .filter(|v| v.is_string() || v.is_number())
    {
        metadata.insert("volume".into(), volume.clone());
    }
    if let Some(date) = data
        .pointer(&format!("/release_date/{group_id}"))
        .filter(|v| v.is_string() || v.is_number())
    {
        metadata.insert("release_date_source".into(), date.clone());
    }
    if let Some(group) = source
        .pointer(&format!("/groups/{group_id}"))
        .and_then(Value::as_str)
        .filter(|v| v.len() <= 200 && !v.chars().any(char::is_control))
    {
        metadata.insert("group".into(), group.into());
    }
    let mut out = Vec::new();
    for (index, entry) in files.iter().enumerate() {
        let name = entry.as_str().ok_or("Danke chapter filename is not text")?;
        if name.is_empty()
            || name.len() > 200
            || name.contains("..")
            || name.contains('/')
            || name.contains('\\')
            || name.contains('%')
            || name.chars().any(char::is_control)
        {
            return Err("Danke original has an unsafe filename".into());
        }
        let ext = name
            .rsplit_once('.')
            .map(|(_, ext)| ext.to_ascii_lowercase())
            .ok_or("Danke original filename lacks extension")?;
        if !matches!(
            ext.as_str(),
            "jpg" | "jpeg" | "png" | "webp" | "gif" | "avif"
        ) {
            return Err("Danke chapter image extension unsupported".into());
        }
        let mut url = Url::parse("https://danke.moe/")?;
        url.path_segments_mut()
            .map_err(|()| "Danke media source has no path")?
            .extend(["media", "manga", slug, "chapters", folder, group_id, name]);
        let item = MediaItem {
            url: url.to_string(),
            filename: format!("{:03}_{name}", index + 1),
            extractor: "dankefuerslesen",
            post_id: None,
        };
        let mut fields = media_metadata(&item);
        for (key, value) in &metadata {
            fields.insert(key.clone(), value.clone());
        }
        fields.insert("num".into(), (index + 1).into());
        fields.insert("extension".into(), ext.into());
        out.push((item, fields));
    }
    Ok(out)
}

fn chapter_queue(
    source: &Value,
    slug: &str,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let chapters = source
        .get("chapters")
        .and_then(Value::as_object)
        .ok_or("Danke manga source lacks chapters")?;
    if chapters.len() > MAX_CHAPTERS {
        return Err("Danke manga chapter listing exceeds bounded queue".into());
    }
    let mut out = Vec::new();
    for (key, data) in chapters {
        if key.is_empty() || key.len() > 96 || !key.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        {
            continue;
        }
        let ch = key.replace('.', "-");
        let mut metadata = source_metadata(source, slug);
        metadata.insert("subcategory".into(), "manga".into());
        metadata.insert("chapter_key".into(), key.clone().into());
        if let Some(title) = data
            .get("title")
            .and_then(Value::as_str)
            .filter(|v| v.len() <= 240 && !v.chars().any(char::is_control))
        {
            metadata.insert("title".into(), title.into());
        }
        out.push(queued_url(
            format!("https://danke.moe/read/manga/{slug}/{ch}/1/"),
            metadata,
        ));
    }
    Ok(out)
}

pub async fn resolve(
    client: &Client,
    target: Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    match target {
        Target::Chapter { slug, chapter } => {
            let source = manga_json(client, &Url::parse("https://danke.moe/")?, &slug).await?;
            Ok(chapter_media(&source, &slug, &chapter)?
                .into_iter()
                .map(|(m, _)| m)
                .collect())
        }
        Target::Manga(_) => {
            Err("Danke manga listing requires --dump-messages or --follow-queues".into())
        }
    }
}

pub async fn resolve_messages(
    client: &Client,
    target: Target,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let (slug, chapter) = match target {
        Target::Manga(slug) => (slug, None),
        Target::Chapter { slug, chapter } => (slug, Some(chapter)),
    };
    let source = manga_json(client, &Url::parse("https://danke.moe/")?, &slug).await?;
    if let Some(chapter) = chapter {
        let mut out = Vec::new();
        for (item, metadata) in chapter_media(&source, &slug, &chapter)? {
            out.push(ExtractorMessage::Directory {
                metadata: metadata.clone(),
            });
            out.push(ExtractorMessage::Url {
                url: item.url,
                metadata,
            });
        }
        Ok(out)
    } else {
        chapter_queue(&source, &slug)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    fn fixture() -> Value {
        serde_json::json!({
            "slug":"series","title":"Public Manga","author":["A"],"artist":["B"],
            "groups":{"12":"Scanners & Friends"},
            "chapters":{
                "1":{"title":"Opening","folder":"ch1","groups":{"12":["001.jpg","002.png"]},"release_date":{"12":1770000000}},
                "2.1":{"title":"Next","folder":"ch2","groups":{"12":["001.webp"]}}
            }
        })
    }

    #[test]
    fn supported_reader_manga_and_chapter_routes_reject_unsafe_origins() {
        assert_eq!(
            match_url("https://danke.moe/read/manga/series"),
            Some(Target::Manga("series".into()))
        );
        assert_eq!(
            match_url("https://www.danke.moe/reader/series/series/2-1/1/"),
            Some(Target::Chapter {
                slug: "series".into(),
                chapter: "2-1".into()
            })
        );
        for bad in [
            "https://fakedanke.moe/read/manga/series",
            "https://danke.moe/read/manga/series/../private",
            "https://danke.moe/read/manga/series?token=1",
            "https://danke.moe:8443/read/manga/series",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn chapter_originals_and_manga_queues_preserve_validated_source_metadata() {
        let media = chapter_media(&fixture(), "series", "1").unwrap();
        assert_eq!(media.len(), 2);
        assert_eq!(
            media[0].0.url,
            "https://danke.moe/media/manga/series/chapters/ch1/12/001.jpg"
        );
        assert_eq!(media[1].0.filename, "002_002.png");
        assert_eq!(media[0].1["title"], "Opening");
        assert_eq!(media[0].1["group"], "Scanners & Friends");
        assert_eq!(media[0].1["release_date_source"], 1770000000);
        let children = chapter_queue(&fixture(), "series").unwrap();
        assert_eq!(children.len(), 2);
        assert_eq!(
            serde_json::to_value(&children[1]).unwrap()["url"],
            "https://danke.moe/read/manga/series/2-1/1/"
        );
        let mut unsafe_source = fixture();
        unsafe_source["chapters"]["1"]["groups"]["12"][0] = "../private.jpg".into();
        assert!(chapter_media(&unsafe_source, "series", "1").is_err());
    }

    #[tokio::test]
    async fn bounded_public_series_json_reads_one_api_request_per_entry() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for _ in 0..2 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0u8; 4096];
                let n = socket.read(&mut request).await.unwrap();
                assert!(String::from_utf8_lossy(&request[..n])
                    .starts_with("GET /api/series/series/ HTTP/1.1"));
                let body = fixture().to_string();
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let source = manga_json(&Client::new(), &base, "series").await.unwrap();
        assert_eq!(chapter_media(&source, "series", "1").unwrap().len(), 2);
        assert_eq!(
            chapter_queue(
                &manga_json(&Client::new(), &base, "series").await.unwrap(),
                "series"
            )
            .unwrap()
            .len(),
            2
        );
        server.await.unwrap();
    }
}
