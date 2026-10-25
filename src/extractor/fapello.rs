//! Guarded public Fapello post originals and model/trending discovery.
//! Independent bounded subset of pinned gallery-dl fapello.py.
use crate::{
    message::{media_metadata, queued_url, ExtractorMessage, Metadata},
    safe_filename, MediaItem,
};
use reqwest::Client;
use std::{collections::HashSet, error::Error, time::Duration};
use url::Url;

const MAX_HTML: usize = 2 * 1024 * 1024;
const MAX_PAGES: u16 = 4;
const MAX_CHILDREN: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Post {
        root: String,
        model: String,
        id: u64,
    },
    Model {
        root: String,
        model: String,
    },
    Path {
        root: String,
        path: String,
    },
}

fn valid_slug(raw: &str) -> bool {
    (1..=80).contains(&raw.len())
        && raw
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn numeric(raw: &str) -> Option<u64> {
    if !(1..=18).contains(&raw.len()) || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    raw.parse::<u64>().ok().filter(|n| *n > 0)
}

fn root_of(url: &Url) -> Option<String> {
    match url.host_str()? {
        "fapello.com" | "www.fapello.com" => Some("https://fapello.com".into()),
        "fapello.su" | "www.fapello.su" => Some("https://fapello.su".into()),
        _ => None,
    }
}

pub fn match_url(raw: &str) -> Option<Target> {
    let low = raw.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "/../", "/./"]
        .iter()
        .any(|part| low.contains(part))
    {
        return None;
    }
    let url = Url::parse(raw).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let root = root_of(&url)?;
    let parts: Vec<_> = url.path_segments()?.collect();
    match parts.as_slice() {
        [model, id] | [model, id, ""] if valid_slug(model) => {
            if *model == "popular_videos" && valid_slug(id) {
                return Some(Target::Path {
                    root,
                    path: format!("popular_videos/{id}"),
                });
            }
            if let Some(id) = numeric(id) {
                return Some(Target::Post {
                    root,
                    model: (*model).into(),
                    id,
                });
            }
            if !id.is_empty() {
                return None;
            }
            if matches!(
                *model,
                "top-likes" | "top-followers" | "videos" | "trending"
            ) {
                Some(Target::Path {
                    root,
                    path: (*model).into(),
                })
            } else if *model == "search" {
                None
            } else {
                Some(Target::Model {
                    root,
                    model: (*model).into(),
                })
            }
        }
        [model] | [model, ""] if valid_slug(model) => {
            if matches!(
                *model,
                "top-likes" | "top-followers" | "videos" | "trending"
            ) {
                Some(Target::Path {
                    root,
                    path: (*model).into(),
                })
            } else if *model == "search" {
                None
            } else {
                Some(Target::Model {
                    root,
                    model: (*model).into(),
                })
            }
        }
        ["popular_videos", filter] | ["popular_videos", filter, ""] if valid_slug(filter) => {
            Some(Target::Path {
                root,
                path: format!("popular_videos/{filter}"),
            })
        }
        _ => None,
    }
}

fn accepted_media(root: &str, raw: &str) -> Option<(Url, String, String)> {
    // Validate the raw reference before URL normalization can erase dot segments.
    let lower = raw.to_ascii_lowercase();
    if lower.contains("..")
        || ["%2e", "%2f", "%5c"].iter().any(|part| lower.contains(part))
    {
        return None;
    }
    let source = Url::parse(root).ok()?;
    let url = source.join(raw).ok()?;
    let host = url.host_str()?;
    let approved = matches!(host, "fapello.com" | "fapello.su")
        || host.ends_with(".fapello.com")
        || host.ends_with(".fapello.su");
    if !approved
        || url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path().contains('%')
        || url.path().contains("..")
    {
        return None;
    }
    let basename = url.path_segments()?.next_back()?;
    let extension = basename.rsplit_once('.')?.1.to_ascii_lowercase();
    if !matches!(
        extension.as_str(),
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "mp4" | "webm"
    ) {
        return None;
    }
    let filename = safe_filename(basename);
    (!filename.is_empty()).then_some((url, filename, extension))
}

fn post_record(
    html: &str,
    root: &str,
    model: &str,
    id: u64,
) -> Result<(MediaItem, Metadata), Box<dyn Error + Send + Sync>> {
    let (_, region) = html
        .split_once("class=\"uk-align-center\"")
        .ok_or("Fapello public post lacks original media region")?;
    let (region, _) = region
        .split_once("</div>")
        .ok_or("Fapello original media region truncated")?;
    let url_raw = region
        .split_once("src=\"")
        .and_then(|(_, rest)| rest.split_once('"'))
        .map(|(raw, _)| raw)
        .ok_or("Fapello media URL not available")?
        .replace(".md", "")
        .replace(".th", "");
    let (url, _, extension) =
        accepted_media(root, &url_raw).ok_or("Fapello source media URL is untrusted")?;
    let is_video = region.contains("type=\"video");
    let item = MediaItem {
        url: url.to_string(),
        filename: safe_filename(&format!("{model}_{id}.{extension}")),
        extractor: "fapello",
        post_id: Some(id),
    };
    let mut data = media_metadata(&item);
    data.insert("subcategory".into(), "post".into());
    data.insert("model".into(), model.into());
    data.insert("id".into(), id.into());
    data.insert(
        "type".into(),
        (if is_video { "video" } else { "photo" }).into(),
    );
    data.insert("extension".into(), extension.into());
    data.insert("post_url".into(), format!("{root}/{model}/{id}/").into());
    if let Some(poster) = region
        .split_once("poster=\"")
        .and_then(|(_, rest)| rest.split_once('"'))
        .map(|(value, _)| value)
        .filter(|value| value.len() <= 500 && !value.chars().any(char::is_control))
    {
        data.insert("thumbnail".into(), poster.into());
    }
    Ok((item, data))
}

fn scoped_link(root: &str, href: &str) -> Option<(Url, Target)> {
    if href.starts_with("//") || href.contains('%') || href.contains("..") {
        return None;
    }
    let base = Url::parse(root).ok()?;
    let url = base.join(href).ok()?;
    if url.origin() != base.origin()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let target = match_url(url.as_str())?;
    Some((url, target))
}

fn listing_queue(
    html: &str,
    root: &str,
    target: &Target,
    seen: &mut HashSet<String>,
    result: &mut Vec<ExtractorMessage>,
) {
    for link in html.split("<a href=\"").skip(1).take(4000) {
        if result.len() >= MAX_CHILDREN {
            break;
        }
        let Some((href, _)) = link.split_once('"') else {
            continue;
        };
        if href == "javascript:void(0);" {
            continue;
        }
        let Some((_, candidate)) = scoped_link(root, href) else {
            continue;
        };
        let acceptable = match (target, &candidate) {
            (Target::Model { model, .. }, Target::Post { model: child, .. }) => model == child,
            (Target::Path { path, .. }, Target::Model { .. })
                if matches!(path.as_str(), "top-likes" | "top-followers")
                    || root.ends_with(".su") && path == "trending" =>
            {
                true
            }
            (Target::Path { path, .. }, Target::Post { .. })
                if !matches!(path.as_str(), "top-likes" | "top-followers")
                    && !(root.ends_with(".su") && path == "trending") =>
            {
                true
            }
            _ => false,
        };
        if !acceptable {
            continue;
        }
        let canonical = match candidate {
            Target::Post { root, model, id } => format!("{root}/{model}/{id}/"),
            Target::Model { root, model } => format!("{root}/{model}/"),
            Target::Path { root, path } => format!("{root}/{path}/"),
        };
        if !seen.insert(canonical.clone()) {
            continue;
        }
        let mut metadata = Metadata::new();
        metadata.insert("category".into(), "fapello".into());
        metadata.insert("subcategory".into(), "queue".into());
        match target {
            Target::Model { model, .. } => {
                metadata.insert("model".into(), model.clone().into());
            }
            Target::Path { path, .. } => {
                metadata.insert("listing".into(), path.clone().into());
            }
            Target::Post { .. } => {}
        }
        result.push(queued_url(canonical, metadata));
    }
}

fn api_url(root: &str, target: &Target, page: u16) -> Result<Url, Box<dyn Error + Send + Sync>> {
    let base = Url::parse(&format!("{root}/"))?;
    let path = match target {
        Target::Model { model, .. } => format!("ajax/model/{model}/page-{page}/"),
        Target::Path { path, .. } => {
            let translated = if root.ends_with(".su") {
                path.replace('-', "/")
            } else {
                path.clone()
            };
            format!("ajax/{translated}/page-{page}/")
        }
        Target::Post { .. } => return Err("Fapello post is not a listing".into()),
    };
    Ok(base.join(&path)?)
}

async fn html(client: &Client, endpoint: Url) -> Result<String, Box<dyn Error + Send + Sync>> {
    let mut response = client
        .get(endpoint.clone())
        .send()
        .await?
        .error_for_status()?;
    if response.url().origin() != endpoint.origin() {
        return Err("Fapello source redirect escaped origin".into());
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_HTML as u64)
    {
        return Err("Fapello source exceeds HTML limit".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_HTML.saturating_sub(bytes.len()) {
            return Err("Fapello source exceeds HTML limit".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8(bytes)?)
}

async fn post_at_base(
    client: &Client,
    base: &Url,
    root: &str,
    model: &str,
    id: u64,
) -> Result<(MediaItem, Metadata), Box<dyn Error + Send + Sync>> {
    let endpoint = base.join(&format!("{model}/{id}/"))?;
    post_record(&html(client, endpoint).await?, root, model, id)
}

async fn listing_at_base(
    client: &Client,
    base: &Url,
    target: &Target,
    max_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let root = match target {
        Target::Model { root, .. } | Target::Path { root, .. } => root,
        Target::Post { .. } => return Err("Fapello post is not a listing".into()),
    };
    let mut output = Vec::new();
    let mut seen = HashSet::new();
    for page in 1..=max_pages.clamp(1, MAX_PAGES) {
        if page > 1 {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        let url = api_url(root, target, page)?;
        let source = if base.scheme() == "http" {
            base.join(url.path())?
        } else {
            url
        };
        let body = html(client, source).await?;
        if body.trim().is_empty() {
            break;
        }
        let old = output.len();
        listing_queue(&body, root, target, &mut seen, &mut output);
        if old == output.len() || output.len() >= MAX_CHILDREN {
            break;
        }
    }
    Ok(output)
}

pub async fn resolve(
    client: &Client,
    target: Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    match target {
        Target::Post { root, model, id } => {
            let base = Url::parse(&format!("{root}/"))?;
            Ok(vec![
                post_at_base(client, &base, &root, &model, id).await?.0,
            ])
        }
        _ => {
            Err("Fapello model and listing feeds require --dump-messages or --follow-queues".into())
        }
    }
}

pub async fn resolve_messages(
    client: &Client,
    target: Target,
    max_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    match &target {
        Target::Post { root, model, id } => {
            let base = Url::parse(&format!("{root}/"))?;
            let (item, metadata) = post_at_base(client, &base, root, model, *id).await?;
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
        Target::Model { root, .. } | Target::Path { root, .. } => {
            listing_at_base(
                client,
                &Url::parse(&format!("{root}/"))?,
                &target,
                max_pages,
            )
            .await
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
    fn matches_pinned_com_su_posts_model_and_discovery_paths() {
        assert_eq!(
            match_url("https://fapello.com/model/12345"),
            Some(Target::Post {
                root: "https://fapello.com".into(),
                model: "model".into(),
                id: 12345
            })
        );
        assert_eq!(
            match_url("https://www.fapello.su/top-likes/"),
            Some(Target::Path {
                root: "https://fapello.su".into(),
                path: "top-likes".into()
            })
        );
        assert_eq!(
            match_url("https://fapello.su/popular_videos/video/"),
            Some(Target::Path {
                root: "https://fapello.su".into(),
                path: "popular_videos/video".into(),
            })
        );
        assert_eq!(
            match_url("https://fapello.com/trending/"),
            Some(Target::Path {
                root: "https://fapello.com".into(),
                path: "trending".into()
            })
        );
        for bad in [
            "https://fakefapello.com/model/123",
            "https://fapello.com/model/0",
            "https://fapello.com/search/",
            "https://fapello.com/model/?private=1",
            "https://name:pw@fapello.com/model",
            "https://fapello.su:8443/top-likes/",
            "https://fapello.com/model/../../private",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn post_original_video_photo_and_thumbnail_variants_are_scoped() {
        let html = r#"<div class="uk-align-center"><video type="video/mp4" poster="https://cdn.fapello.com/cover.jpg"><source src="https://cdn.fapello.com/media/original.md.mp4"></video></div>"#;
        let (item, metadata) = post_record(html, "https://fapello.com", "model", 12345).unwrap();
        assert_eq!(item.url, "https://cdn.fapello.com/media/original.mp4");
        assert_eq!(item.filename, "model_12345.mp4");
        assert_eq!(metadata["type"], "video");
        assert_eq!(metadata["id"], 12345);
        for forged in [
            r#"<div class="uk-align-center"><img src="https://evil-fapello.com/private.jpg"></div>"#,
            r#"<div class="uk-align-center"><img src="https://cdn.fapello.com/../private.jpg"></div>"#,
        ] {
            assert!(post_record(forged, "https://fapello.com", "model", 12345).is_err());
        }
    }

    #[test]
    fn bounded_ajax_discovery_deduplicates_first_party_model_posts_and_scopes_paths() {
        let target = match_url("https://fapello.com/model").unwrap();
        let html = r#"<a href="https://fapello.com/model/12345">First</a><a href="/model/12345/">dup</a><a href="https://fapello.com/model/12347/">Second</a><a href="https://evil-fapello.com/model/12348">Bad</a><a href="https://fapello.com/other/999">Other</a>"#;
        let mut output = Vec::new();
        listing_queue(
            html,
            "https://fapello.com",
            &target,
            &mut HashSet::new(),
            &mut output,
        );
        assert_eq!(output.len(), 2);
        assert_eq!(serde_json::to_value(&output[0]).unwrap()["type"], "queue");
        let source = match_url("https://fapello.su/top-likes").unwrap();
        assert_eq!(
            api_url("https://fapello.su", &source, 2).unwrap().path(),
            "/ajax/top/likes/page-2/"
        );
        let mut users = Vec::new();
        listing_queue(
            r#"<a href="https://fapello.su/person/">Person</a>"#,
            "https://fapello.su",
            &source,
            &mut HashSet::new(),
            &mut users,
        );
        assert_eq!(users.len(), 1);
    }

    #[tokio::test]
    async fn original_and_two_ajax_page_requests_are_bounded_in_local_fixture() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (route, body) in [
                (
                    "/model/12345/",
                    r#"<div class="uk-align-center"><img src="https://cdn.fapello.com/original.jpg"></div>"#,
                ),
                (
                    "/ajax/model/model/page-1/",
                    r#"<a href="https://fapello.com/model/12345">First</a>"#,
                ),
                (
                    "/ajax/model/model/page-2/",
                    r#"<a href="https://fapello.com/model/12347">Second</a>"#,
                ),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buffer = [0u8; 4096];
                let n = socket.read(&mut buffer).await.unwrap();
                assert!(String::from_utf8_lossy(&buffer[..n])
                    .starts_with(&format!("GET {route} HTTP/1.1")));
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{address}/")).unwrap();
        let post = post_at_base(&Client::new(), &base, "https://fapello.com", "model", 12345)
            .await
            .unwrap();
        assert_eq!(post.0.filename, "model_12345.jpg");
        let target = match_url("https://fapello.com/model").unwrap();
        let events = listing_at_base(&Client::new(), &base, &target, 2)
            .await
            .unwrap();
        assert_eq!(events.len(), 2);
        server.await.unwrap();
    }
}
