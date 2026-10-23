//! Independent Pixeldrain public file and list JSON extractor preview.
//! Authenticated endpoints, filesystem folders, ZIP mode, full metadata and
//! live conformance are intentionally excluded.
use crate::{safe_filename, MediaItem};
use reqwest::Client;
use serde_json::Value;
use std::error::Error;
use url::Url;

const MAX_JSON_BYTES: usize = 2 * 1024 * 1024;
const MAX_ALBUM_FILES: usize = 2000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    File { id: String },
    Album { id: String, selected: Option<usize> },
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == b'_')
}

pub fn match_url(input: &str) -> Option<Target> {
    let url = Url::parse(input).ok()?;
    if url.scheme() != "https" || url.host_str() != Some("pixeldrain.com") {
        return None;
    }
    let path: Vec<_> = url.path_segments()?.collect();
    match path.as_slice() {
        ["u", id] | ["api", "file", id] if valid_id(id) => {
            Some(Target::File { id: (*id).into() })
        }
        ["l", id] | ["api", "list", id] if valid_id(id) => {
            let selected = match url.fragment() {
                Some(raw) => {
                    let number = raw.strip_prefix("item=")?.parse::<usize>().ok()?;
                    (number < MAX_ALBUM_FILES).then_some(number)?
                }
                None => return Some(Target::Album { id: (*id).into(), selected: None }),
            };
            Some(Target::Album {
                id: (*id).into(),
                selected: Some(selected),
            })
        }
        _ => None,
    }
}

fn item_from_file(value: &Value) -> Option<MediaItem> {
    let id = value.get("id")?.as_str()?.trim();
    if !valid_id(id) {
        return None;
    }
    let availability = value.get("availability");
    if availability.is_some_and(|v| {
        !v.is_null() && v != false && v != "" && v != 0
    }) {
        return None;
    }
    let name = value.get("name")?.as_str()?;
    if name.is_empty() || name.len() > 255 {
        return None;
    }
    let filename = safe_filename(name);
    if matches!(filename.as_str(), "" | "." | "..") {
        return None;
    }
    Some(MediaItem {
        url: format!("https://pixeldrain.com/api/file/{id}?download"),
        filename,
        extractor: "pixeldrain",
        post_id: None,
    })
}

fn album_media(album: &Value, selected: Option<usize>) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let files = album
        .get("files")
        .and_then(Value::as_array)
        .ok_or("Pixeldrain album response missing files")?;
    if files.len() > MAX_ALBUM_FILES {
        return Err("Pixeldrain album exceeds supported file bound".into());
    }
    let result = if let Some(index) = selected {
        files.get(index)
            .and_then(item_from_file)
            .into_iter()
            .collect()
    } else {
        files.iter().filter_map(item_from_file).collect()
    };
    Ok(result)
}

async fn read_json(client: &Client, url: Url) -> Result<Value, Box<dyn Error + Send + Sync>> {
    let mut response = client.get(url).send().await?.error_for_status()?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_JSON_BYTES as u64)
    {
        return Err("Pixeldrain API JSON exceeds the safety bound".into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len().saturating_add(chunk.len()) > MAX_JSON_BYTES {
            return Err("Pixeldrain API JSON exceeds the safety bound".into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(serde_json::from_slice(&body)?)
}

async fn resolve_at_base(
    client: &Client,
    base: &Url,
    target: Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    match target {
        Target::File { id } => {
            let info = read_json(client, base.join(&format!("api/file/{id}/info"))?).await?;
            Ok(item_from_file(&info).into_iter().collect())
        }
        Target::Album { id, selected } => {
            let album = read_json(client, base.join(&format!("api/list/{id}"))?).await?;
            album_media(&album, selected)
        }
    }
}

pub async fn resolve(
    client: &Client,
    target: Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse("https://pixeldrain.com/")?;
    resolve_at_base(client, &base, target).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn recognizes_public_urls_and_selected_album_items_only() {
        assert_eq!(
            match_url("https://pixeldrain.com/u/File12"),
            Some(Target::File { id: "File12".into() })
        );
        assert_eq!(
            match_url("https://pixeldrain.com/api/list/abc_99#item=1"),
            Some(Target::Album { id: "abc_99".into(), selected: Some(1) })
        );
        assert_eq!(
            match_url("https://pixeldrain.com/l/abc_99"),
            Some(Target::Album { id: "abc_99".into(), selected: None })
        );
        assert!(match_url("https://evil.pixeldrain.com/u/abc").is_none());
        assert!(match_url("http://pixeldrain.com/u/abc").is_none());
        assert!(match_url("https://pixeldrain.com/u/bad%2Fid").is_none());
        assert!(match_url("https://pixeldrain.com/l/abc#item=bad").is_none());
        assert!(match_url("https://pixeldrain.com/l/abc#item=2000").is_none());
    }

    #[test]
    fn files_require_safe_name_and_ignore_unavailable_entries() {
        let value = json!({"id":"File12","name":"photo one.jpg","availability":null});
        let item = item_from_file(&value).unwrap();
        assert_eq!(item.filename, "photo one.jpg");
        assert_eq!(item.url, "https://pixeldrain.com/api/file/File12?download");
        assert!(item_from_file(&json!({"id":"File12","name":"bad","availability":"deleted"})).is_none());
        assert!(item_from_file(&json!({"id":"bad/id","name":"x.jpg"})).is_none());
        let album = json!({"files":[
            {"id":"a","name":"one.jpg","availability":null},
            {"id":"b","name":"two.png","availability":"deleted"},
            {"id":"c","name":"three.webp"}
        ]});
        assert_eq!(album_media(&album, None).unwrap().len(), 2);
        assert_eq!(album_media(&album, Some(2)).unwrap()[0].filename, "three.webp");
        assert!(album_media(&album, Some(1)).unwrap().is_empty());
    }

    #[tokio::test]
    async fn file_and_album_public_apis_resolve_with_offline_json() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (route, body) in [
                (
                    "GET /api/file/abc123/info HTTP/1.1",
                    r#"{"id":"abc123","name":"standalone.jpg","availability":null}"#,
                ),
                (
                    "GET /api/list/list123 HTTP/1.1",
                    r#"{"id":"list123","files":[{"id":"abc123","name":"first.jpg"},{"id":"def456","name":"second.png"}]}"#,
                ),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buf = [0_u8; 4096];
                let n = socket.read(&mut buf).await.unwrap();
                let request = String::from_utf8_lossy(&buf[..n]);
                assert!(request.starts_with(route), "{request}");
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let file = resolve_at_base(&Client::new(), &base, Target::File { id: "abc123".into() })
            .await
            .unwrap();
        assert_eq!(file.len(), 1);
        assert_eq!(file[0].filename, "standalone.jpg");
        let album = resolve_at_base(
            &Client::new(),
            &base,
            Target::Album { id: "list123".into(), selected: None },
        )
        .await
        .unwrap();
        assert_eq!(album.len(), 2);
        assert_eq!(album[0].filename, "first.jpg");
        assert_eq!(album[1].filename, "second.png");
        server.await.unwrap();
    }
}
