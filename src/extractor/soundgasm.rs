//! Pinned public Soundgasm audio and user-listing subset; no private endpoints.
use crate::{
    message::{media_metadata, queued_url, ExtractorMessage, Metadata},
    safe_filename, MediaItem,
};
use reqwest::Client;
use std::{collections::HashSet, error::Error};
use url::Url;

const MAX_HTML: usize = 2 * 1024 * 1024;
const MAX_CHILDREN: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Audio { user: String, slug: String },
    User { user: String },
}

fn safe_name(name: &str) -> bool {
    (1..=120).contains(&name.len())
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
        && !name.contains("..")
}

pub fn match_url(input: &str) -> Option<Target> {
    let url = Url::parse(input).ok()?;
    if !matches!(url.scheme(), "https" | "http")
        || !matches!(url.host_str(), Some("soundgasm.net" | "www.soundgasm.net"))
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
    match paths.as_slice() {
        ["u" | "user", user, slug] | ["u" | "user", user, slug, ""]
            if safe_name(user) && safe_name(slug) =>
        {
            Some(Target::Audio { user: (*user).into(), slug: (*slug).into() })
        }
        ["u" | "user", user] | ["u" | "user", user, ""]
            if safe_name(user) =>
        {
            Some(Target::User { user: (*user).into() })
        }
        _ => None,
    }
}

fn capture<'a>(page: &'a str, before: &str, after: &str) -> Option<&'a str> {
    page.split_once(before)?.1.split_once(after).map(|(body, _)| body)
}

fn strip_markup(raw: &str) -> String {
    let mut output = String::new();
    let mut in_tag = false;
    for ch in raw.chars() {
        if ch == '<' { in_tag = true; }
        else if ch == '>' { in_tag = false; }
        else if !in_tag { output.push(ch); }
    }
    output.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .trim()
        .to_owned()
}

fn media_link(input: &str) -> Option<(String, String)> {
    let url = Url::parse(input).ok()?;
    let host = url.host_str()?;
    if url.scheme() != "https"
        || !(host == "soundgasm.net" || host.ends_with(".soundgasm.net"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let mut parts = url.path_segments()?;
    let filename = parts.next_back()?;
    if filename.contains('%') || filename.contains("..") || filename.contains('\\')
        || filename.is_empty()
    {
        return None;
    }
    let extension = filename.rsplit_once('.')?.1.to_ascii_lowercase();
    matches!(extension.as_str(), "mp3" | "m4a" | "ogg" | "wav" | "flac" | "aac")
        .then(|| (url.to_string(), extension))
}

fn parse_audio(
    page: &str,
    user: &str,
    slug: &str,
) -> Result<(MediaItem, Metadata), Box<dyn Error + Send + Sync>> {
    let label = capture(page, "aria-label=\"title\">", "<")
        .ok_or("Soundgasm public audio has no supported title")?;
    let title = strip_markup(label);
    if title.is_empty() || title.len() > 240 || title.chars().any(char::is_control) {
        return Err("Soundgasm public title is not a safe source value".into());
    }
    let description = capture(page, "class=\"jp-description\">", "</div>")
        .map(strip_markup).unwrap_or_default();
    if description.len() > 4096 || description.chars().any(char::is_control) {
        return Err("Soundgasm source description exceeds bounded field limits".into());
    }
    let formats = capture(page, "\"setMedia\", {", "}")
        .ok_or("Soundgasm page lacks a supported media source call")?;
    let raw_url = formats.split_once(": \"")
        .or_else(|| formats.split_once(":\""))
        .and_then(|(_, trailing)| trailing.split_once('"').map(|(url, _)| url))
        .ok_or("Soundgasm audio source has no bounded media URL")?;
    let (url, extension) = media_link(raw_url)
        .ok_or("Soundgasm original audio does not use an approved public CDN URL")?;
    let filename = safe_filename(&format!("{title}.{extension}"));
    if filename.is_empty() || matches!(filename.as_str(), "." | "..") {
        return Err("Soundgasm filename is invalid".into());
    }
    let item = MediaItem {
        url,
        filename,
        extractor: "soundgasm",
        post_id: None,
    };
    let mut meta = media_metadata(&item);
    meta.insert("subcategory".into(), "audio".into());
    meta.insert("user".into(), user.into());
    meta.insert("slug".into(), slug.into());
    meta.insert("title".into(), title.into());
    meta.insert("description".into(), description.into());
    meta.insert("extension".into(), extension.into());
    meta.insert("post_url".into(), format!("https://soundgasm.net/u/{user}/{slug}").into());
    Ok((item, meta))
}

fn discover_user(
    page: &str,
    user: &str,
) -> Vec<ExtractorMessage> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let base = Url::parse("https://soundgasm.net/").unwrap();
    for chunk in page.split("class=\"sound-details\">").skip(1).take(2400) {
        if out.len() >= MAX_CHILDREN { break; }
        let Some(anchor) = chunk.split_once("</a>").map(|(body, _)| body) else { continue };
        let Some(href) = capture(anchor, "<a href=\"", "\"") else { continue };
        let Ok(url) = base.join(href) else { continue };
        let Some(Target::Audio { user: found_user, slug }) = match_url(url.as_str()) else { continue };
        if found_user != user || !seen.insert(slug.clone()) { continue };
        let canonical = format!("https://soundgasm.net/u/{user}/{slug}");
        let mut meta = Metadata::new();
        meta.insert("category".into(), "soundgasm".into());
        meta.insert("subcategory".into(), "user".into());
        meta.insert("user".into(), user.into());
        meta.insert("slug".into(), slug.into());
        out.push(queued_url(canonical, meta));
    }
    out
}

async fn bounded_html(
    client: &Client,
    page: Url,
) -> Result<String, Box<dyn Error + Send + Sync>> {
    let mut response = client.get(page.clone()).send().await?.error_for_status()?;
    if response.url().origin() != page.origin() {
        return Err("Soundgasm public page redirected outside source origin".into());
    }
    if response.content_length().is_some_and(|n| n > MAX_HTML as u64) {
        return Err("Soundgasm public page exceeds HTML cap".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_HTML.saturating_sub(bytes.len()) {
            return Err("Soundgasm public page exceeds HTML cap".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8(bytes)?)
}

async fn at_base(
    client: &Client,
    base: &Url,
    target: Target,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    match target {
        Target::Audio { user, slug } => {
            let page = bounded_html(client, base.join(&format!("u/{user}/{slug}"))?).await?;
            let (item, metadata) = parse_audio(&page, &user, &slug)?;
            Ok(vec![
                ExtractorMessage::Directory { metadata: metadata.clone() },
                ExtractorMessage::Url { url: item.url, metadata },
            ])
        }
        Target::User { user } => {
            let page = bounded_html(client, base.join(&format!("user/{user}"))?).await?;
            Ok(discover_user(&page, &user))
        }
    }
}

pub async fn resolve(
    client: &Client,
    target: Target,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    match target {
        Target::Audio { user, slug } => {
            let base = Url::parse("https://soundgasm.net/")?;
            let page = bounded_html(client, base.join(&format!("u/{user}/{slug}"))?).await?;
            Ok(vec![parse_audio(&page, &user, &slug)?.0])
        }
        Target::User { .. } => Err("Soundgasm user pages require --dump-messages or --follow-queues".into()),
    }
}

pub async fn resolve_messages(
    client: &Client,
    target: Target,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    at_base(client, &Url::parse("https://soundgasm.net/")?, target).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::TcpListener};

    fn audio_fixture() -> &'static str {
        r#"<div aria-label="title">An &amp; Example</div><div class="jp-description">A <b>sample</b> sound</div><script>player("setMedia", {m4a: "https://media.soundgasm.net/sounds/example.m4a"});</script>"#
    }

    #[test]
    fn accepts_pinned_user_and_sound_urls_only() {
        assert_eq!(
            match_url("https://soundgasm.net/u/alice/first"),
            Some(Target::Audio { user: "alice".into(), slug: "first".into() })
        );
        assert_eq!(
            match_url("https://www.soundgasm.net/user/alice"),
            Some(Target::User { user: "alice".into() })
        );
        for bad in [
            "https://fake-soundgasm.net/u/alice/first",
            "https://soundgasm.net/u/alice/first?private=1",
            "https://soundgasm.net/u/alice/%2e%2e",
            "https://name:pass@soundgasm.net/u/alice/first",
            "https://soundgasm.net:8443/u/alice/first",
        ] { assert!(match_url(bad).is_none(), "{bad}"); }
    }

    #[test]
    fn public_audio_uses_first_party_original_and_source_metadata() {
        let (item, meta) = parse_audio(audio_fixture(), "alice", "first").unwrap();
        assert_eq!(item.url, "https://media.soundgasm.net/sounds/example.m4a");
        assert_eq!(meta["title"], "An & Example");
        assert_eq!(meta["description"], "A sample sound");
        assert_eq!(meta["extension"], "m4a");
        let bad = audio_fixture().replace("media.soundgasm.net", "media.soundgasm.net.evil.test");
        assert!(parse_audio(&bad, "alice", "first").is_err());
    }

    #[test]
    fn user_discovery_is_deferred_first_seen_and_foreign_links_excluded() {
        let page = r#"<div class="sound-details"><a href="/u/alice/first">First</a></div><div class="sound-details"><a href="https://soundgasm.net/u/alice/first">Duplicate</a></div><div class="sound-details"><a href="/u/alice/second">Second</a></div><div class="sound-details"><a href="/u/bob/foreign">Other</a></div><div class="sound-details"><a href="https://evil-soundgasm.net/u/alice/evil">Evil</a></div>"#;
        let events = discover_user(page, "alice");
        assert_eq!(events.len(), 2);
        let last = serde_json::to_value(&events[1]).unwrap();
        assert_eq!(last["url"], "https://soundgasm.net/u/alice/second");
        assert_eq!(last["metadata"]["subcategory"], "user");
    }

    #[tokio::test]
    async fn public_audio_and_user_listing_each_read_one_bounded_http_page() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (path, html) in [
                ("/u/alice/first", audio_fixture()),
                ("/user/alice", r#"<div class="sound-details"><a href="/u/alice/first">First</a></div>"#),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0u8; 4096];
                let n = socket.read(&mut request).await.unwrap();
                assert!(String::from_utf8_lossy(&request[..n])
                    .starts_with(&format!("GET {path} HTTP/1.1")));
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
                    html.len()
                );
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{address}/")).unwrap();
        assert_eq!(
            at_base(&Client::new(), &base, Target::Audio { user: "alice".into(), slug: "first".into() }).await.unwrap().len(),
            2
        );
        assert_eq!(
            at_base(&Client::new(), &base, Target::User { user: "alice".into() }).await.unwrap().len(),
            1
        );
        server.await.unwrap();
    }
}
