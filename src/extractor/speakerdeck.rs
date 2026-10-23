//! Independent public Speaker Deck slide-image extractor preview.
//! Metadata/archiving parity, live comparisons and nonpublic presentations
//! remain outside the currently accepted subset.
use crate::{safe_filename, MediaItem};
use reqwest::Client;
use serde_json::Value;
use std::{collections::HashSet, error::Error};
use url::Url;

const MAX_PAGE_BYTES: usize = 2 * 1024 * 1024;
const MAX_SLIDES: usize = 2000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Presentation { user: String, slug: String },
}

fn valid_part(input: &str) -> bool {
    !input.is_empty()
        && input.len() <= 128
        && input
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

pub fn match_url(input: &str) -> Option<Target> {
    if input.to_ascii_lowercase().contains("%2e") {
        return None;
    }
    let url = Url::parse(input).ok()?;
    if url.scheme() != "https"
        || !matches!(url.host_str(), Some("speakerdeck.com" | "www.speakerdeck.com"))
    {
        return None;
    }
    let parts: Vec<_> = url.path_segments()?.collect();
    match parts.as_slice() {
        [user, slug] if valid_part(user) && valid_part(slug) => {
            Some(Target::Presentation {
                user: (*user).to_owned(),
                slug: (*slug).to_owned(),
            })
        }
        _ => None,
    }
}

fn player_id(html: &str) -> Option<String> {
    let id = html
        .split_once("speakerdeck.com/player/")?
        .1
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_')
        .next()?;
    valid_part(id).then(|| id.to_owned())
}

fn slide_media(html: &str) -> Vec<MediaItem> {
    let mut seen = HashSet::new();
    let mut urls = Vec::new();
    for item in html.split("js-sd-slide").skip(1).take(MAX_SLIDES) {
        let Some(url) = item
            .split_once("data-url=\"")
            .and_then(|(_, section)| section.split_once('"'))
            .map(|(raw, _)| raw)
        else {
            continue;
        };
        let Some(url) = Url::parse(url).ok() else {
            continue;
        };
        let Some(host) = url.host_str() else {
            continue;
        };
        let approved = matches!(host, "speakerd.s3.amazonaws.com" | "speakerdeck.com")
            || host.ends_with(".speakerdeck.com");
        if url.scheme() != "https" || !approved {
            continue;
        }
        let Some(name) = url.path_segments().and_then(|mut s| s.next_back()) else {
            continue;
        };
        if name.is_empty() || name.len() > 220 {
            continue;
        }
        let Some(ext) = name.rsplit_once('.').map(|(_, ext)| ext) else {
            continue;
        };
        if ext.is_empty()
            || ext.len() > 12
            || !ext.bytes().all(|b| b.is_ascii_alphanumeric())
        {
            continue;
        }
        if seen.insert(url.to_string()) {
            urls.push((url, safe_filename(name)));
        }
    }
    let padding = urls.len().max(1).to_string().len().max(2);
    urls.into_iter()
        .enumerate()
        .map(|(i, (url, filename))| MediaItem {
            url: url.to_string(),
            filename: format!("{:0padding$}_{filename}", i + 1),
            extractor: "speakerdeck",
            post_id: None,
        })
        .collect()
}

async fn read_bytes(client: &Client, url: Url) -> Result<Vec<u8>, Box<dyn Error + Send + Sync>> {
    let mut response = client.get(url).send().await?.error_for_status()?;
    if response
        .content_length()
        .is_some_and(|n| n > MAX_PAGE_BYTES as u64)
    {
        return Err("Speaker Deck response exceeds the configured safety bound".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len().saturating_add(chunk.len()) > MAX_PAGE_BYTES {
            return Err("Speaker Deck response exceeds the configured safety bound".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

async fn resolve_at_base(
    client: &Client,
    base: &Url,
    target: Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    match target {
        Target::Presentation { user, slug } => {
            let mut endpoint = base.join("oembed.json")?;
            let mut canonical = Url::parse("https://speakerdeck.com/")?;
            canonical.set_path(&format!("/{user}/{slug}"));
            endpoint
                .query_pairs_mut()
                .append_pair("url", canonical.as_str());
            let info: Value = serde_json::from_slice(&read_bytes(client, endpoint).await?)?;
            let html = info
                .get("html")
                .and_then(Value::as_str)
                .ok_or("Speaker Deck oEmbed missing player HTML")?;
            let id = player_id(html).ok_or("Speaker Deck oEmbed missing verified player ID")?;
            let page = String::from_utf8(
                read_bytes(client, base.join(&format!("player/{id}"))?).await?,
            )?;
            Ok(slide_media(&page))
        }
    }
}

pub async fn resolve(
    client: &Client,
    target: Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    resolve_at_base(client, &Url::parse("https://speakerdeck.com/")?, target).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presentation_url_and_player_id_are_validated() {
        assert_eq!(
            match_url("https://speakerdeck.com/person/my-deck"),
            Some(Target::Presentation {
                user: "person".into(),
                slug: "my-deck".into(),
            })
        );
        assert_eq!(
            player_id(r#"<iframe src="//speakerdeck.com/player/aBcd-1234"></iframe>"#).as_deref(),
            Some("aBcd-1234")
        );
        assert!(player_id("No player found").is_none());
        for input in [
            "http://speakerdeck.com/person/my-deck",
            "https://evil.speakerdeck.com/person/my-deck",
            "https://speakerdeck.com/person/my-deck/more",
            "https://speakerdeck.com/person/%2e%2e/my-deck",
            "https://speakerdeck.com/person/",
        ] {
            assert!(match_url(input).is_none(), "{input}");
        }
    }

    #[test]
    fn slide_media_validates_hosts_and_deduplicates_in_presentation_order() {
        let html = concat!(
            r#"<div class="js-sd-slide" data-url="https://files.speakerdeck.com/x/one.jpg">"#,
            r#"<div class="js-sd-slide" data-url="https://speakerd.s3.amazonaws.com/x/two.png">"#,
            r#"<div class="js-sd-slide" data-url="https://files.speakerdeck.com/x/one.jpg">"#,
            r#"<div class="js-sd-slide" data-url="https://speakerdeck.com.evil.org/x/three.jpg">"#,
        );
        let media = slide_media(html);
        assert_eq!(media.len(), 2);
        assert_eq!(media[0].filename, "01_one.jpg");
        assert_eq!(media[1].filename, "02_two.png");
    }

    #[tokio::test]
    async fn oembed_to_player_slide_resolution_uses_local_two_request_fixture() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (route, body) in [
                (
                    "GET /oembed.json?url=https%3A%2F%2Fspeakerdeck.com%2Fperson%2Fslides HTTP/1.1",
                    r#"{"title":"Test","html":"<iframe src=\"//speakerdeck.com/player/player_123\"></iframe>"}"#,
                ),
                (
                    "GET /player/player_123 HTTP/1.1",
                    r#"<div class="js-sd-slide" data-url="https://files.speakerdeck.com/path/slide01.jpg"></div>"#,
                ),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = [0_u8; 4096];
                let n = socket.read(&mut bytes).await.unwrap();
                let request = String::from_utf8_lossy(&bytes[..n]);
                assert!(request.starts_with(route), "{request}");
                let answer = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(answer.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let slides = resolve_at_base(
            &Client::new(),
            &base,
            Target::Presentation {
                user: "person".into(),
                slug: "slides".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(slides.len(), 1);
        assert_eq!(slides[0].filename, "01_slide01.jpg");
        server.await.unwrap();
    }
}
