//! Independent Pixeldrain public file and list JSON extractor preview.
//! Authenticated endpoints, recursive folders, ZIP mode, full metadata and
//! live conformance are intentionally excluded.
use crate::{safe_filename, MediaItem};
use reqwest::Client;
use serde_json::Value;
use std::{collections::HashSet, error::Error};
use url::Url;

const MAX_JSON_BYTES: usize = 2 * 1024 * 1024;
const MAX_ALBUM_FILES: usize = 2000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    File { id: String },
    Album { id: String, selected: Option<usize> },
    AlbumZip { id: String },
    Folder { path: String },
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == b'_')
}

fn safe_folder_path(raw: &str) -> Option<String> {
    let parts: Vec<_> = raw.split('/').collect();
    if parts.is_empty() || parts.len() > 12 || raw.len() > 1024 {
        return None;
    }
    for segment in &parts {
        let decoded = percent_encoding::percent_decode_str(segment)
            .decode_utf8()
            .ok()?;
        if decoded.is_empty()
            || decoded.len() > 200
            || matches!(decoded.as_ref(), "." | "..")
            || decoded
                .chars()
                .any(|ch| ch.is_control() || ch == '/' || ch == '\\' || ch == '?' || ch == '#')
        {
            return None;
        }
    }
    Some(raw.to_owned())
}

fn folder_item(child: &Value, parent: &str) -> Option<MediaItem> {
    if child.get("type")?.as_str()? != "file" {
        return None;
    }
    let name = child.get("name")?.as_str()?;
    if name == ".search_index.gz" || name.is_empty() || name.len() > 255 {
        return None;
    }
    let filename = safe_filename(name);
    if matches!(filename.as_str(), "" | "." | "..") {
        return None;
    }
    let path = child.get("path")?.as_str()?.strip_prefix('/')?;
    let relative = safe_folder_path(path)?;
    // Only direct children of the requested folder are included, without
    // silently traversing recursive directories or other filesystem roots.
    let remainder = relative.strip_prefix(parent)?;
    if !remainder.is_empty() {
        let suffix = remainder.strip_prefix('/')?;
        if suffix.is_empty() || suffix.contains('/') {
            return None;
        }
    }
    if child.get("availability").is_some_and(unavailable) {
        return None;
    }
    let mut url = Url::parse("https://pixeldrain.com/").ok()?;
    url.set_path(&format!("/api/filesystem/{relative}"));
    url.set_query(Some("attach"));
    Some(MediaItem {
        url: url.to_string(),
        filename,
        extractor: "pixeldrain",
        post_id: None,
    })
}

pub fn match_url(input: &str) -> Option<Target> {
    // Reject dot segments before URL normalization can silently erase them.
    let lower = input.to_ascii_lowercase();
    if lower.contains("%2e") || lower.contains("/../") || lower.contains("/./") {
        return None;
    }
    let url = Url::parse(input).ok()?;
    if url.scheme() != "https" || url.host_str() != Some("pixeldrain.com") {
        return None;
    }
    let path: Vec<_> = url.path_segments()?.collect();
    match path.as_slice() {
        ["u", id] | ["api", "file", id] if valid_id(id) => Some(Target::File { id: (*id).into() }),
        ["api", "list", id, "zip"] if valid_id(id) => {
            Some(Target::AlbumZip { id: (*id).into() })
        }
        ["d", rest @ ..] | ["api", "filesystem", rest @ ..] if !rest.is_empty() => {
            let path = safe_folder_path(&rest.join("/"))?;
            Some(Target::Folder { path })
        }
        ["l", id] | ["api", "list", id] if valid_id(id) => {
            let selected = match url.fragment() {
                Some(raw) => {
                    let number = raw.strip_prefix("item=")?.parse::<usize>().ok()?;
                    (number < MAX_ALBUM_FILES).then_some(number)?
                }
                None => {
                    return Some(Target::Album {
                        id: (*id).into(),
                        selected: None,
                    })
                }
            };
            Some(Target::Album {
                id: (*id).into(),
                selected: Some(selected),
            })
        }
        _ => None,
    }
}

fn unavailable(status: &Value) -> bool {
    match status {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_i64() != Some(0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}

fn item_from_file(value: &Value) -> Option<MediaItem> {
    let id = value.get("id")?.as_str()?.trim();
    if !valid_id(id) {
        return None;
    }
    if value.get("availability").is_some_and(unavailable) {
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

fn album_media(
    album: &Value,
    selected: Option<usize>,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let files = album
        .get("files")
        .and_then(Value::as_array)
        .ok_or("Pixeldrain album response missing files")?;
    if files.len() > MAX_ALBUM_FILES {
        return Err("Pixeldrain album exceeds supported file bound".into());
    }
    let result = if let Some(index) = selected {
        files
            .get(index)
            .and_then(item_from_file)
            .into_iter()
            .collect()
    } else {
        files.iter().filter_map(item_from_file).collect()
    };
    Ok(result)
}

fn album_zip_item(album: &Value, id: &str) -> Result<MediaItem, Box<dyn Error + Send + Sync>> {
    let title = album
        .get("title")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty() && name.len() <= 220)
        .unwrap_or(id);
    let stem = safe_filename(title);
    if matches!(stem.as_str(), "" | "." | "..") {
        return Err("Pixeldrain ZIP album title cannot be used as a filename".into());
    }
    Ok(MediaItem {
        url: format!("https://pixeldrain.com/api/list/{id}/zip"),
        filename: format!("{stem}.zip"),
        extractor: "pixeldrain",
        post_id: None,
    })
}

fn filesystem_media(
    value: &Value,
    parent: &str,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let paths = value
        .get("path")
        .and_then(Value::as_array)
        .ok_or("Pixeldrain filesystem response missing path")?;
    let index = value
        .get("base_index")
        .and_then(Value::as_u64)
        .and_then(|i| usize::try_from(i).ok())
        .ok_or("Pixeldrain filesystem response missing base_index")?;
    let root = paths
        .get(index)
        .ok_or("Pixeldrain filesystem base_index is out of range")?;
    if root.get("type").and_then(Value::as_str) != Some("dir") {
        return Ok(folder_item(root, parent).into_iter().collect());
    }
    let children = value
        .get("children")
        .and_then(Value::as_array)
        .ok_or("Pixeldrain filesystem response missing children")?;
    if children.len() > MAX_ALBUM_FILES {
        return Err("Pixeldrain filesystem exceeds supported direct-child bound".into());
    }
    let mut seen = HashSet::new();
    Ok(children
        .iter()
        .filter_map(|child| folder_item(child, parent))
        .filter(|item| seen.insert(item.url.clone()))
        .collect())
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
        Target::AlbumZip { id } => {
            let album = read_json(client, base.join(&format!("api/list/{id}"))?).await?;
            Ok(vec![album_zip_item(&album, &id)?])
        }
        Target::Folder { path } => {
            let response =
                read_json(client, base.join(&format!("api/filesystem/{path}?stat"))?).await?;
            filesystem_media(&response, &path)
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
            Some(Target::File {
                id: "File12".into()
            })
        );
        assert_eq!(
            match_url("https://pixeldrain.com/api/list/abc_99#item=1"),
            Some(Target::Album {
                id: "abc_99".into(),
                selected: Some(1)
            })
        );
        assert_eq!(
            match_url("https://pixeldrain.com/l/abc_99"),
            Some(Target::Album {
                id: "abc_99".into(),
                selected: None
            })
        );
        assert!(match_url("https://evil.pixeldrain.com/u/abc").is_none());
        assert!(match_url("http://pixeldrain.com/u/abc").is_none());
        assert!(match_url("https://pixeldrain.com/u/bad%2Fid").is_none());
        assert!(match_url("https://pixeldrain.com/l/abc#item=bad").is_none());
        assert_eq!(
            match_url("https://pixeldrain.com/api/list/abc_99/zip"),
            Some(Target::AlbumZip {
                id: "abc_99".into()
            })
        );
        assert!(match_url("https://pixeldrain.com/api/list/bad%2Fid/zip").is_none());
        assert!(match_url("https://pixeldrain.com/l/abc#item=2000").is_none());
    }

    #[test]
    fn filesystem_routes_and_child_paths_are_conservatively_scoped() {
        assert_eq!(
            match_url("https://pixeldrain.com/d/root123/photos"),
            Some(Target::Folder {
                path: "root123/photos".into()
            })
        );
        assert_eq!(
            match_url("https://pixeldrain.com/api/filesystem/root123"),
            Some(Target::Folder {
                path: "root123".into()
            })
        );
        assert!(match_url("https://pixeldrain.com/d/").is_none());
        assert!(match_url("https://pixeldrain.com/d/root123/%2e%2e/other").is_none());
        assert!(match_url("https://pixeldrain.com/d/root123/bad%2Fpath").is_none());
        let child = json!({
            "type":"file", "name":"photo.jpg", "path":"/root123/photos/photo.jpg"
        });
        let item = folder_item(&child, "root123/photos").unwrap();
        assert_eq!(item.filename, "photo.jpg");
        assert_eq!(
            item.url,
            "https://pixeldrain.com/api/filesystem/root123/photos/photo.jpg?attach"
        );
        assert!(folder_item(&child, "root123/different").is_none());
        let root_file = json!({
            "type": "file", "name": "direct.jpg", "path": "/root123/photos"
        });
        assert_eq!(
            folder_item(&root_file, "root123/photos").unwrap().filename,
            "direct.jpg"
        );
        assert!(folder_item(
            &json!({"type":"dir","name":"nested","path":"/root123/photos/nested"}),
            "root123/photos"
        )
        .is_none());
        assert!(folder_item(
            &json!({"type":"file","name":"nested.jpg","path":"/root123/photos/dir/nested.jpg"}),
            "root123/photos"
        )
        .is_none());
    }

    #[test]
    fn filesystem_stat_filters_children_without_traversal_or_duplicate_urls() {
        let response = json!({
            "base_index": 0,
            "path": [{"type":"dir","id":"root123","path":"/root123"}],
            "children": [
                {"type":"file","name":"first.jpg","path":"/root123/first.jpg"},
                {"type":"file","name":"first.jpg","path":"/root123/first.jpg"},
                {"type":"dir","name":"nested","path":"/root123/nested"},
                {"type":"file","name":"deep.jpg","path":"/root123/nested/deep.jpg"},
                {"type":"file","name":"hidden.png","path":"/root123/hidden.png","availability":"deleted"},
                {"type":"file","name":"last.webp","path":"/root123/last.webp"}
            ]
        });
        let media = filesystem_media(&response, "root123").unwrap();
        assert_eq!(media.len(), 2);
        assert_eq!(media[0].filename, "first.jpg");
        assert_eq!(media[1].filename, "last.webp");
    }

    #[test]
    fn files_require_safe_name_and_ignore_unavailable_entries() {
        let value = json!({"id":"File12","name":"photo one.jpg","availability":null});
        let item = item_from_file(&value).unwrap();
        assert_eq!(item.filename, "photo one.jpg");
        assert_eq!(item.url, "https://pixeldrain.com/api/file/File12?download");
        assert!(
            item_from_file(&json!({"id":"File12","name":"bad","availability":"deleted"})).is_none()
        );
        assert!(item_from_file(&json!({"id":"bad/id","name":"x.jpg"})).is_none());
        let album = json!({"files":[
            {"id":"a","name":"one.jpg","availability":null},
            {"id":"b","name":"two.png","availability":"deleted"},
            {"id":"c","name":"three.webp"}
        ]});
        assert_eq!(album_media(&album, None).unwrap().len(), 2);
        assert_eq!(
            album_media(&album, Some(2)).unwrap()[0].filename,
            "three.webp"
        );
        assert!(album_media(&album, Some(1)).unwrap().is_empty());
    }

    #[test]
    fn zip_preview_uses_album_title_and_no_implicit_authentication() {
        let result = album_zip_item(
            &json!({"id":"sample","title":"Summer photos"}),
            "sample",
        )
        .unwrap();
        assert_eq!(result.filename, "Summer photos.zip");
        assert_eq!(result.url, "https://pixeldrain.com/api/list/sample/zip");
        assert_eq!(
            album_zip_item(&json!({"title":"../unsafe"}), "abc")
                .unwrap()
                .filename,
            safe_filename("../unsafe").to_string() + ".zip"
        );
    }

    #[tokio::test]
    async fn zip_preview_fetches_public_list_metadata_before_download_url() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 4096];
            let size = socket.read(&mut request).await.unwrap();
            assert!(
                String::from_utf8_lossy(&request[..size])
                    .starts_with("GET /api/list/album1 HTTP/1.1")
            );
            let body = r#"{"id":"album1","title":"An album","file_count":3}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        let base = Url::parse(&format!("http://{address}/")).unwrap();
        let media = resolve_at_base(
            &Client::new(),
            &base,
            Target::AlbumZip {
                id: "album1".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(media.len(), 1);
        assert_eq!(media[0].filename, "An album.zip");
        assert_eq!(media[0].url, "https://pixeldrain.com/api/list/album1/zip");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn public_filesystem_stat_resolves_direct_files_through_local_json() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = [0_u8; 4096];
            let length = socket.read(&mut bytes).await.unwrap();
            let request = String::from_utf8_lossy(&bytes[..length]);
            assert!(
                request.starts_with("GET /api/filesystem/root123?stat HTTP/1.1"),
                "{request}"
            );
            let body = r#"{"base_index":0,"path":[{"type":"dir","id":"root123","path":"/root123"}],"children":[{"type":"file","name":"image.webp","path":"/root123/image.webp"}]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        let base = Url::parse(&format!("http://{address}/")).unwrap();
        let media = resolve_at_base(
            &Client::new(),
            &base,
            Target::Folder {
                path: "root123".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(media.len(), 1);
        assert_eq!(media[0].filename, "image.webp");
        server.await.unwrap();
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
        let file = resolve_at_base(
            &Client::new(),
            &base,
            Target::File {
                id: "abc123".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(file.len(), 1);
        assert_eq!(file[0].filename, "standalone.jpg");
        let album = resolve_at_base(
            &Client::new(),
            &base,
            Target::Album {
                id: "list123".into(),
                selected: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(album.len(), 2);
        assert_eq!(album[0].filename, "first.jpg");
        assert_eq!(album[1].filename, "second.png");
        server.await.unwrap();
    }
}
