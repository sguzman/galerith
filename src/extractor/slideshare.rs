//! Guarded public Slideshare Next.js presentations, pinned slideshare.py subset.
use crate::{
    message::{media_metadata, ExtractorMessage, Metadata},
    MediaItem,
};
use reqwest::Client;
use serde_json::Value;
use std::error::Error;
use url::Url;

const MAX_HTML: usize = 2 * 1024 * 1024;
const MAX_SLIDES: u64 = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Presentation {
    user: String,
    slug: String,
}

fn safe_component(s: &str) -> bool {
    (1..=120).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        && !s.contains("..")
}

pub fn match_url(input: &str) -> Option<Presentation> {
    let url = Url::parse(input).ok()?;
    if !matches!(url.scheme(), "https" | "http")
        || !matches!(
            url.host_str(),
            Some("slideshare.net" | "www.slideshare.net")
        )
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || input.contains('%')
    {
        return None;
    }
    let paths: Vec<_> = url.path_segments()?.collect();
    let (user, slug) = match paths.as_slice() {
        [user, slug] | [user, slug, ""] => (*user, *slug),
        ["mobile", user, slug] | ["mobile", user, slug, ""] => (*user, *slug),
        _ => return None,
    };
    (safe_component(user) && safe_component(slug)).then(|| Presentation {
        user: user.into(),
        slug: slug.into(),
    })
}

fn read_slideshow(html: &str) -> Option<Value> {
    for script in html.split("<script").skip(1).take(64) {
        let Some((attrs, tail)) = script.split_once('>') else {
            continue;
        };
        if !attrs.contains("id=\"__NEXT_DATA__\"") && !attrs.contains("id='__NEXT_DATA__'") {
            continue;
        }
        let (body, _) = tail.split_once("</script>")?;
        return serde_json::from_str::<Value>(body)
            .ok()?
            .pointer("/props/pageProps/slideshow")
            .cloned();
    }
    None
}

fn trusted_host(input: &str) -> Option<Url> {
    let url = Url::parse(input).ok()?;
    let host = url.host_str()?;
    (url.scheme() == "https"
        && (host == "slidesharecdn.com" || host.ends_with(".slidesharecdn.com"))
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && url.path() == "/")
        .then_some(url)
}

fn records(
    html: &str,
    target: &Presentation,
) -> Result<Vec<(MediaItem, Metadata)>, Box<dyn Error + Send + Sync>> {
    let slide =
        read_slideshow(html).ok_or("Slideshare page lacks supported Next.js slide state")?;
    let count = slide
        .get("totalSlides")
        .and_then(Value::as_u64)
        .filter(|c| (1..=MAX_SLIDES).contains(c))
        .ok_or("Slideshare slide count is missing or outside public cap")?;
    let props = slide
        .get("slides")
        .ok_or("Slideshare slide media properties missing")?;
    let mut base = props
        .get("host")
        .and_then(Value::as_str)
        .and_then(trusted_host)
        .ok_or("Slideshare media origin is not a trusted CDN")?;
    let location = props
        .get("imageLocation")
        .and_then(Value::as_str)
        .ok_or("Slideshare slide location missing")?;
    if location.len() > 240 || location.split('/').any(|s| !safe_component(s)) {
        return Err("Slideshare media path contains unsafe segments".into());
    }
    let name = props
        .get("title")
        .and_then(Value::as_str)
        .filter(|s| safe_component(s))
        .ok_or("Slideshare source media filename is unsafe")?;
    let mut common = Metadata::new();
    common.insert("category".into(), "slideshare".into());
    common.insert("subcategory".into(), "presentation".into());
    common.insert("user".into(), target.user.clone().into());
    common.insert("presentation".into(), target.slug.clone().into());
    common.insert("count".into(), count.into());
    common.insert(
        "post_url".into(),
        format!("https://www.slideshare.net/{}/{}", target.user, target.slug).into(),
    );
    for (key, max) in [
        ("username", 120),
        ("title", 500),
        ("description", 4096),
        ("createdAt", 60),
    ] {
        if let Some(text) = slide
            .get(key)
            .and_then(Value::as_str)
            .filter(|s| s.len() <= max && !s.chars().any(char::is_control))
        {
            let field = if key == "createdAt" {
                "date_source"
            } else if key == "username" {
                "source_username"
            } else {
                key
            };
            common.insert(field.into(), text.trim().into());
        }
    }
    for key in ["views", "likes"] {
        if let Some(n) = slide.get(key).and_then(Value::as_u64) {
            common.insert(key.into(), n.into());
        }
    }
    let mut records = Vec::with_capacity(count as usize);
    for num in 1..=count {
        {
            let mut segments = base
                .path_segments_mut()
                .map_err(|()| "Slideshare CDN path rejected")?;
            for part in location.split('/') {
                segments.push(part);
            }
            segments.push("95").push(&format!("{name}-{num}-1024.jpg"));
        }
        let item = MediaItem {
            url: base.to_string(),
            filename: format!("{}-{num:02}.jpg", target.slug),
            extractor: "slideshare",
            post_id: None,
        };
        let mut meta = media_metadata(&item);
        for (key, value) in &common {
            meta.insert(key.clone(), value.clone());
        }
        meta.insert("num".into(), num.into());
        meta.insert("extension".into(), "jpg".into());
        records.push((item, meta));
        base = trusted_host(props.get("host").and_then(Value::as_str).unwrap())
            .ok_or("Slideshare CDN state unexpectedly changed")?;
    }
    Ok(records)
}

async fn at_base(
    client: &Client,
    base: &Url,
    target: &Presentation,
) -> Result<Vec<(MediaItem, Metadata)>, Box<dyn Error + Send + Sync>> {
    let endpoint = base.join(&format!("{}/{}", target.user, target.slug))?;
    let mut response = client
        .get(endpoint.clone())
        .send()
        .await?
        .error_for_status()?;
    if response.url().origin() != endpoint.origin() {
        return Err("Slideshare source redirected off origin".into());
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_HTML as u64)
    {
        return Err("Slideshare source page exceeds HTML cap".into());
    }
    let mut html = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_HTML.saturating_sub(html.len()) {
            return Err("Slideshare source page exceeds HTML cap".into());
        }
        html.extend_from_slice(&chunk);
    }
    records(std::str::from_utf8(&html)?, target)
}

pub async fn resolve(
    client: &Client,
    target: Presentation,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    Ok(
        at_base(client, &Url::parse("https://www.slideshare.net/")?, &target)
            .await?
            .into_iter()
            .map(|(item, _)| item)
            .collect(),
    )
}

pub async fn resolve_messages(
    client: &Client,
    target: Presentation,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let mut out = Vec::new();
    for (item, metadata) in
        at_base(client, &Url::parse("https://www.slideshare.net/")?, &target).await?
    {
        out.push(ExtractorMessage::Directory {
            metadata: metadata.clone(),
        });
        out.push(ExtractorMessage::Url {
            url: item.url,
            metadata,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    fn fixture() -> &'static str {
        r#"<script id="__NEXT_DATA__" type="application/json">{"props":{"pageProps":{"slideshow":{"username":"speaker","title":"My Slides","description":"Deck","views":23,"likes":2,"createdAt":"2026-08-11T12:00:00Z","totalSlides":3,"slides":{"host":"https://image.slidesharecdn.com","imageLocation":"example/abc","title":"deck"}}}}}</script>"#
    }

    #[test]
    fn public_slideshare_canonical_and_mobile_paths_only() {
        assert_eq!(
            match_url("https://www.slideshare.net/speaker/my-deck")
                .unwrap()
                .user,
            "speaker"
        );
        assert_eq!(
            match_url("https://slideshare.net/mobile/speaker/my-deck")
                .unwrap()
                .slug,
            "my-deck"
        );
        for bad in [
            "https://evillslideshare.net/speaker/my-deck",
            "https://slideshare.net/speaker/my-deck?private=1",
            "https://slideshare.net/speaker/my-deck/../../secret",
            "https://slideshare.net/speaker/%2e%2e",
            "https://a:pass@slideshare.net/speaker/my-deck",
            "https://slideshare.net:8443/speaker/my-deck",
        ] {
            assert!(match_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn source_slides_preserve_image_numbering_metadata_and_trusted_cdn() {
        let target = match_url("https://www.slideshare.net/speaker/my-deck").unwrap();
        let slides = records(fixture(), &target).unwrap();
        assert_eq!(slides.len(), 3);
        assert_eq!(
            slides[0].0.url,
            "https://image.slidesharecdn.com/example/abc/95/deck-1-1024.jpg"
        );
        assert_eq!(slides[2].0.filename, "my-deck-03.jpg");
        assert_eq!(slides[1].1["num"], 2);
        assert_eq!(slides[0].1["title"], "My Slides");
        assert_eq!(slides[0].1["views"], 23);
        assert_eq!(slides[0].1["date_source"], "2026-08-11T12:00:00Z");
        for (from, to) in [
            (
                "image.slidesharecdn.com",
                "image.slidesharecdn.com.evil.test",
            ),
            ("example/abc", "example/../abc"),
            ("\"totalSlides\":3", "\"totalSlides\":10000"),
            ("\"totalSlides\":3", "\"totalSlides\":0"),
            ("\"title\":\"deck\"", "\"title\":\"%2f\""),
        ] {
            assert!(records(&fixture().replace(from, to), &target).is_err());
        }
    }

    #[tokio::test]
    async fn single_bounded_http_get_produces_three_slide_urls() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut data = [0u8; 4096];
            let n = socket.read(&mut data).await.unwrap();
            assert!(
                String::from_utf8_lossy(&data[..n]).starts_with("GET /speaker/my-deck HTTP/1.1")
            );
            let html = fixture();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
                html.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let target = match_url("https://www.slideshare.net/speaker/my-deck").unwrap();
        assert_eq!(
            at_base(&Client::new(), &base, &target).await.unwrap().len(),
            3
        );
        server.await.unwrap();
    }
}
