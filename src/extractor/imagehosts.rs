//! Conservative Pixhost subset of pinned upstream imagehosts.py.
//! Independent Rust public single-image and first-page gallery queue adapter.
use crate::{
    message::{media_metadata, queued_url, ExtractorMessage, Metadata},
    safe_filename, MediaItem,
};
use reqwest::Client;
use std::{collections::HashSet, error::Error};
use url::Url;

const MAX_HTML_BYTES: usize = 2 * 1024 * 1024;
const MAX_GALLERY_CHILDREN: usize = 96;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pixhost {
    File {
        path: String,
        token: String,
        directory: String,
    },
    Gallery {
        id: String,
    },
}

fn valid_gallery_id(raw: &str) -> bool {
    (1..=80).contains(&raw.len())
        && raw
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

fn valid_file_name(raw: &str) -> Option<&str> {
    if raw.len() > 240 || raw.contains('%') || raw.contains('/') {
        return None;
    }
    let (token, rest) = raw.split_once('_')?;
    if !(1..=32).contains(&token.len()) || !token.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let ext = rest.rsplit_once('.')?.1.to_ascii_lowercase();
    if !matches!(
        ext.as_str(),
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif"
    ) {
        return None;
    }
    Some(token)
}

pub fn match_url(input: &str) -> Option<Pixhost> {
    let url = Url::parse(input).ok()?;
    if url.scheme() != "https"
        || !matches!(
            url.host_str(),
            Some("pixhost.to" | "www.pixhost.to" | "pixhost.org" | "www.pixhost.org")
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
        ["show", directory, file]
            if !directory.is_empty()
                && directory.len() <= 10
                && directory.bytes().all(|b| b.is_ascii_digit()) =>
        {
            Some(Pixhost::File {
                path: format!("show/{directory}/{file}"),
                token: valid_file_name(file)?.to_owned(),
                directory: (*directory).into(),
            })
        }
        ["gallery", id] if valid_gallery_id(id) => Some(Pixhost::Gallery { id: (*id).into() }),
        _ => None,
    }
}

fn safe_original_media(raw: &str) -> Option<(Url, String)> {
    let url = Url::parse(raw).ok()?;
    let host = url.host_str()?;
    if url.scheme() != "https"
        || !(host == "pixhost.to"
            || host.ends_with(".pixhost.to")
            || host == "pixhost.org"
            || host.ends_with(".pixhost.org"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let filename = url.path_segments()?.next_back()?;
    if filename.len() > 240 || filename.is_empty() || filename.contains('%') {
        return None;
    }
    let ext = filename.rsplit_once('.')?.1.to_ascii_lowercase();
    if !matches!(
        ext.as_str(),
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif"
    ) {
        return None;
    }
    let name = safe_filename(filename);
    Some((url, name))
}

fn media_from_html(
    html: &str,
    token: &str,
    directory: &str,
) -> Result<(MediaItem, Metadata), Box<dyn Error + Send + Sync>> {
    let pos = html
        .find("class=\"image-img\" src=\"")
        .ok_or("Pixhost page has no recognized public original image")?;
    let rest = &html[pos + "class=\"image-img\" src=\"".len()..];
    let media = rest
        .split_once('"')
        .map(|(url, _)| url)
        .ok_or("Pixhost image attribute is incomplete")?;
    let (url, filename) =
        safe_original_media(media).ok_or("Pixhost original image host or file is unsupported")?;
    let item = MediaItem {
        url: url.to_string(),
        filename,
        extractor: "pixhost",
        post_id: None,
    };
    let mut metadata = media_metadata(&item);
    metadata.insert("subcategory".into(), "image".into());
    metadata.insert("token".into(), token.into());
    metadata.insert("directory".into(), directory.into());
    Ok((item, metadata))
}

async fn read_public_html(
    client: &Client,
    endpoint: Url,
) -> Result<String, Box<dyn Error + Send + Sync>> {
    let mut response = client.get(endpoint).send().await?.error_for_status()?;
    if response
        .content_length()
        .is_some_and(|size| size > MAX_HTML_BYTES as u64)
    {
        return Err("Pixhost public HTML response exceeds byte bound".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_HTML_BYTES.saturating_sub(bytes.len()) {
            return Err("Pixhost public HTML response exceeds byte bound".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8(bytes)?)
}

async fn file_at_base(
    client: &Client,
    path: &str,
    token: &str,
    directory: &str,
    base: &Url,
) -> Result<(MediaItem, Metadata), Box<dyn Error + Send + Sync>> {
    let html = read_public_html(client, base.join(path)?).await?;
    media_from_html(&html, token, directory)
}

fn gallery_queue(html: &str, id: &str) -> Vec<ExtractorMessage> {
    let mut queue = Vec::new();
    let mut seen = HashSet::new();
    let Some(fragment) = html.split_once("class=\"images\"").map(|(_, r)| r) else {
        return queue;
    };
    let fragment = fragment
        .split_once("</div>")
        .map(|(f, _)| f)
        .unwrap_or(fragment);
    for (position, _) in fragment.match_indices("href=\"") {
        let suffix = &fragment[position + "href=\"".len()..];
        let Some((raw, _)) = suffix.split_once('"') else {
            continue;
        };
        let Some(Pixhost::File {
            path,
            token,
            directory,
        }) = match_url(raw)
        else {
            continue;
        };
        let child = format!("https://pixhost.to/{path}");
        if !seen.insert(child.clone()) {
            continue;
        }
        let mut metadata = Metadata::new();
        metadata.insert("category".into(), "pixhost".into());
        metadata.insert("subcategory".into(), "image".into());
        metadata.insert("gallery_id".into(), id.into());
        metadata.insert("directory".into(), directory.into());
        metadata.insert("token".into(), token.into());
        queue.push(queued_url(child, metadata));
        if queue.len() >= MAX_GALLERY_CHILDREN {
            break;
        }
    }
    queue
}

async fn gallery_at_base(
    client: &Client,
    id: &str,
    base: &Url,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let html = read_public_html(client, base.join(&format!("gallery/{id}"))?).await?;
    Ok(gallery_queue(&html, id))
}

pub async fn resolve(
    client: &Client,
    target: Pixhost,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    match target {
        Pixhost::File {
            path,
            token,
            directory,
        } => {
            let (item, _) = file_at_base(
                client,
                &path,
                &token,
                &directory,
                &Url::parse("https://pixhost.to/")?,
            )
            .await?;
            Ok(vec![item])
        }
        Pixhost::Gallery { .. } => {
            Err("Pixhost gallery traversal requires --dump-messages or --follow-queues".into())
        }
    }
}

pub async fn resolve_messages(
    client: &Client,
    target: Pixhost,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    match target {
        Pixhost::File {
            path,
            token,
            directory,
        } => {
            let (item, metadata) = file_at_base(
                client,
                &path,
                &token,
                &directory,
                &Url::parse("https://pixhost.to/")?,
            )
            .await?;
            Ok(vec![
                ExtractorMessage::Directory {
                    metadata: metadata.clone(),
                },
                ExtractorMessage::Url {
                    url: item.url,
                    metadata,
                },
            ])
        }
        Pixhost::Gallery { id } => {
            gallery_at_base(client, &id, &Url::parse("https://pixhost.to/")?).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    #[test]
    fn matches_pinned_pixhost_file_and_gallery_urls_strictly() {
        assert_eq!(
            match_url("https://pixhost.to/show/190/130327671_test-.png"),
            Some(Pixhost::File {
                path: "show/190/130327671_test-.png".into(),
                token: "130327671".into(),
                directory: "190".into()
            })
        );
        assert_eq!(
            match_url("https://pixhost.to/gallery/jSMFq"),
            Some(Pixhost::Gallery { id: "jSMFq".into() })
        );
        for bad in [
            "http://pixhost.to/gallery/jSMFq",
            "https://evilpixhost.to/show/190/130327671_test-.png",
            "https://pixhost.to/show/190/name.jpg",
            "https://pixhost.to/show/190/130327671_test-.png?token=x",
            "https://pixhost.to/show/190/130327671_test-.png#fragment",
            "https://pixhost.to/gallery/../foo",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn pixhost_original_media_validation_rejects_foreign_sites() {
        let html = r#"<img class="image-img" src="https://img01.pixhost.to/images/190/original.png" alt="picture">"#;
        let (media, metadata) = media_from_html(html, "130327671", "190").unwrap();
        assert_eq!(media.filename, "original.png");
        assert_eq!(metadata["directory"], "190");
        assert_eq!(metadata["token"], "130327671");
        for bad in [
            "https://fakepixhost.to/file.jpg",
            "http://img01.pixhost.to/images/a.jpg",
            "https://img01.pixhost.to/images/a.jpg?token=secret",
            "https://img01.pixhost.to/images/a.exe",
        ] {
            assert!(safe_original_media(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn gallery_only_enqueues_first_party_public_file_urls_and_deduplicates() {
        let html = r#"<div class="images"><a href="https://pixhost.to/show/190/130327671_test-.png">One</a><a href="https://pixhost.to/show/190/130327671_test-.png">Duplicate</a><a href="https://evil.example/show/190/123_file.jpg">Foreign</a><a href="https://pixhost.to/show/191/987654_other.jpg">Two</a></div>"#;
        let queue = gallery_queue(html, "jSMFq");
        assert_eq!(queue.len(), 2);
        let first = serde_json::to_value(&queue[0]).unwrap();
        assert_eq!(
            first["url"],
            "https://pixhost.to/show/190/130327671_test-.png"
        );
        assert_eq!(first["metadata"]["gallery_id"], "jSMFq");
    }

    #[tokio::test]
    async fn file_page_uses_bounded_single_public_html_request() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = [0_u8; 4096];
            let n = stream.read(&mut bytes).await.unwrap();
            assert!(String::from_utf8_lossy(&bytes[..n])
                .starts_with("GET /show/190/130327671_test-.png HTTP/1.1"));
            let body =
                r#"<img class="image-img" src="https://img01.pixhost.to/images/190/original.png">"#;
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(reply.as_bytes()).await.unwrap();
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let (file, _) = file_at_base(
            &Client::new(),
            "show/190/130327671_test-.png",
            "130327671",
            "190",
            &base,
        )
        .await
        .unwrap();
        assert_eq!(file.filename, "original.png");
        server.await.unwrap();
    }
}
