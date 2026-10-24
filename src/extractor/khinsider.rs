//! Guarded public KHInsider soundtrack subset from pinned khinsider.py.
//! This supports public album->track->media HTML only, with bounded traffic.
use crate::{
    message::{media_metadata, ExtractorMessage, Metadata},
    safe_filename, MediaItem,
};
use reqwest::Client;
use std::{error::Error, time::Duration};
use url::Url;

const MAX_HTML: usize = 2 * 1024 * 1024;
const MAX_TRACKS: usize = 128;
const MAX_FORMATS_PER_TRACK: usize = 12;
const TRACK_SPACING_MS: u64 = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Album {
    slug: String,
}

fn valid_slug(s: &str) -> bool {
    (1..=130).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

pub fn match_url(raw: &str) -> Option<Album> {
    let lower = raw.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "/../", "/./"]
        .iter()
        .any(|part| lower.contains(part))
    {
        return None;
    }
    let url = Url::parse(raw).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str() != Some("downloads.khinsider.com")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let paths: Vec<_> = url.path_segments()?.collect();
    match paths.as_slice() {
        ["game-soundtracks", "album", slug] | ["game-soundtracks", "album", slug, ""]
            if valid_slug(slug) =>
        {
            Some(Album {
                slug: (*slug).to_owned(),
            })
        }
        _ => None,
    }
}

fn extract<'a>(page: &'a str, start: &str, end: &str) -> Option<&'a str> {
    page.split_once(start)?.1.split_once(end).map(|(s, _)| s)
}

fn visible_text(value: &str) -> Option<String> {
    if value.len() > 240 || value.chars().any(char::is_control) || value.contains('<') {
        return None;
    }
    Some(
        value
            .replace("&amp;", "&")
            .replace("&quot;", "\"")
            .replace("&#39;", "'"),
    )
}

fn metadata(page: &str, slug: &str) -> Metadata {
    let mut data = Metadata::new();
    data.insert("category".into(), "khinsider".into());
    data.insert("subcategory".into(), "soundtrack".into());
    data.insert("album_slug".into(), slug.into());
    if let Some(name) = extract(page, "<h2>", "<").and_then(visible_text) {
        data.insert("album_name".into(), name.into());
    }
    for (key, start, end) in [
        ("year", "Year: <b>", "<"),
        ("catalog", "Catalog Number: <b>", "<"),
        ("album_type", "Album type: <b>", "<"),
        ("added", "Date Added: <b>", "<"),
        ("file_count_source", "Number of Files: <b>", "<"),
    ] {
        if let Some(text) = extract(page, start, end).and_then(visible_text) {
            data.insert(key.into(), text.into());
        }
    }
    data
}

fn trusted_album_path(url: &Url, slug: &str) -> bool {
    if url.scheme() != "https"
        || url.host_str() != Some("downloads.khinsider.com")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.fragment().is_some()
        || url.query().is_some()
    {
        return false;
    }
    let Some(paths) = url.path_segments() else {
        return false;
    };
    let segments: Vec<_> = paths.collect();
    matches!(segments.as_slice(), ["game-soundtracks", "album", album, file]
        if *album == slug && !file.is_empty() && !file.contains('%') && !file.contains(".."))
}

fn track_pages(page: &str, slug: &str) -> Result<Vec<Url>, Box<dyn Error + Send + Sync>> {
    if !page.contains("Download all songs at once:") {
        return Err("KHInsider source is not a public soundtrack album".into());
    }
    let songs = extract(page, "<table id=\"songlist\">", "</table>")
        .ok_or("KHInsider source lacks pinned public song table")?;
    let base = Url::parse("https://downloads.khinsider.com/")?;
    let mut tracks = Vec::new();
    for block in songs
        .split("<td class=\"clickable-row\"><a href=\"")
        .skip(1)
    {
        let Some(path) = block.split_once('"').map(|(href, _)| href) else {
            continue;
        };
        let url = base.join(path)?;
        if !trusted_album_path(&url, slug) {
            return Err("KHInsider song row escapes its public album path".into());
        }
        if tracks.len() >= MAX_TRACKS {
            return Err("KHInsider album exceeds the bounded track limit".into());
        }
        tracks.push(url);
    }
    if tracks.is_empty() {
        return Err("KHInsider album has no supported public song rows".into());
    }
    Ok(tracks)
}

fn trusted_audio(url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    let recognized = ["khinsider.com", "vgmsite.com", "vgmtreasurechest.com"]
        .iter()
        .any(|root| host == *root || host.ends_with(&format!(".{root}")));
    recognized
        && url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && !url.path().contains('%')
        && !url.path().contains("..")
}

fn decode_track(
    html: &str,
    slug: &str,
    ordinal: usize,
    album_meta: &Metadata,
) -> Result<Vec<(MediaItem, Metadata)>, Box<dyn Error + Send + Sync>> {
    let mut candidates = Vec::new();
    for block in html
        .split("<p><a href=\"")
        .skip(1)
        .take(MAX_FORMATS_PER_TRACK + 1)
    {
        if candidates.len() >= MAX_FORMATS_PER_TRACK {
            return Err("KHInsider track exposes too many media format links".into());
        }
        let Some(raw) = block.split_once('"').map(|(url, _)| url) else {
            continue;
        };
        let Ok(url) = Url::parse(raw) else { continue };
        if !trusted_audio(&url) {
            continue;
        }
        let Some(last) = url.path_segments().and_then(|mut s| s.next_back()) else {
            continue;
        };
        let Some((_, ext)) = last.rsplit_once('.') else {
            continue;
        };
        let ext = ext.to_ascii_lowercase();
        if !matches!(ext.as_str(), "mp3" | "flac" | "ogg" | "wav" | "m4a") {
            continue;
        }
        let name = safe_filename(last);
        if name.is_empty() || matches!(name.as_str(), "." | "..") {
            continue;
        }
        candidates.push((url, name, ext));
    }
    if candidates.is_empty() {
        return Err("KHInsider track has no trusted public audio original".into());
    }
    let matching = candidates.iter().filter(|(_, _, format)| format == "mp3");
    let selected = if matching.clone().next().is_some() {
        matching.collect::<Vec<_>>()
    } else {
        vec![&candidates[0]]
    };
    let mut result = Vec::with_capacity(selected.len());
    for (url, basename, ext) in selected {
        let item = MediaItem {
            url: url.to_string(),
            filename: format!("{slug}_{ordinal:03}_{basename}"),
            extractor: "khinsider",
            post_id: None,
        };
        let mut data = media_metadata(&item);
        for (key, value) in album_meta {
            data.insert(key.clone(), value.clone());
        }
        data.insert("num".into(), ordinal.into());
        data.insert("type".into(), "track".into());
        data.insert("extension".into(), ext.clone().into());
        result.push((item, data));
    }
    Ok(result)
}

async fn fetch_html(
    client: &Client,
    endpoint: Url,
    origin: &Url,
) -> Result<String, Box<dyn Error + Send + Sync>> {
    let mut response = client.get(endpoint).send().await?.error_for_status()?;
    if response.url().origin() != origin.origin() {
        return Err("KHInsider source redirected outside trusted album origin".into());
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_HTML as u64)
    {
        return Err("KHInsider source HTML exceeds bounded response size".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_HTML.saturating_sub(bytes.len()) {
            return Err("KHInsider source HTML exceeds bounded response size".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8(bytes)?)
}

async fn records_at_base(
    client: &Client,
    album: &Album,
    base: &Url,
) -> Result<Vec<(MediaItem, Metadata)>, Box<dyn Error + Send + Sync>> {
    let page = fetch_html(
        client,
        base.join(&format!("game-soundtracks/album/{}", album.slug))?,
        base,
    )
    .await?;
    let tracks = track_pages(&page, &album.slug)?;
    let meta = metadata(&page, &album.slug);
    let mut result = Vec::new();
    for (i, track) in tracks.iter().enumerate() {
        if i > 0 {
            tokio::time::sleep(Duration::from_millis(TRACK_SPACING_MS)).await;
        }
        let endpoint = if base.scheme() == "http" {
            // Used only by a loopback fixture; production always uses HTTPS.
            base.join(track.path())?
        } else {
            track.clone()
        };
        let html = fetch_html(client, endpoint, base).await?;
        result.extend(decode_track(&html, &album.slug, i + 1, &meta)?);
    }
    Ok(result)
}

pub async fn resolve(
    client: &Client,
    album: Album,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse("https://downloads.khinsider.com/")?;
    Ok(records_at_base(client, &album, &base)
        .await?
        .into_iter()
        .map(|(item, _)| item)
        .collect())
}

pub async fn resolve_messages(
    client: &Client,
    album: Album,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let base = Url::parse("https://downloads.khinsider.com/")?;
    let mut events = Vec::new();
    for (item, metadata) in records_at_base(client, &album, &base).await? {
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

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    fn album_html() -> &'static str {
        r#"<h2>Game &amp; Sounds</h2>Year: <b>2025</b>Number of Files: <b>2</b>Download all songs at once:<table id="songlist"><td class="clickable-row"><a href="/game-soundtracks/album/game-ost/track-1">One</a></td><td class="clickable-row"><a href="/game-soundtracks/album/game-ost/track-2">Two</a></td></table>"#
    }

    #[test]
    fn matches_only_pinned_album_origins_and_validated_slugs() {
        assert_eq!(
            match_url("https://downloads.khinsider.com/game-soundtracks/album/game-ost")
                .unwrap()
                .slug,
            "game-ost"
        );
        for invalid in [
            "https://evilkhinsider.com/game-soundtracks/album/game-ost",
            "https://www.khinsider.com/game-soundtracks/album/game-ost",
            "https://downloads.khinsider.com/game-soundtracks/album/../../private",
            "https://downloads.khinsider.com/game-soundtracks/album/game-ost?key=secret",
            "https://name:pw@downloads.khinsider.com/game-soundtracks/album/game-ost",
            "https://downloads.khinsider.com:8443/game-soundtracks/album/game-ost",
        ] {
            assert!(match_url(invalid).is_none(), "{invalid}");
        }
    }

    #[test]
    fn preserves_album_song_order_and_rejects_out_of_album_tracks() {
        let tracks = track_pages(album_html(), "game-ost").unwrap();
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].path(), "/game-soundtracks/album/game-ost/track-1");
        assert_eq!(tracks[1].path(), "/game-soundtracks/album/game-ost/track-2");
        let evil = album_html().replace("game-ost/track-2", "other/track-2");
        assert!(track_pages(&evil, "game-ost").is_err());
        assert_eq!(
            metadata(album_html(), "game-ost")["album_name"],
            "Game & Sounds"
        );
    }

    #[test]
    fn default_mp3_selects_mp3_then_first_fallback_with_trusted_media() {
        let meta = metadata(album_html(), "game-ost");
        let html = r#"<p><a href="https://dl.vgmsite.com/one.flac">FLAC</a></p><p><a href="https://dl.vgmsite.com/one.mp3">MP3</a></p>"#;
        let tracks = decode_track(html, "game-ost", 1, &meta).unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].0.url, "https://dl.vgmsite.com/one.mp3");
        assert_eq!(tracks[0].1["num"], 1);
        assert_eq!(tracks[0].1["year"], "2025");
        let fallback = decode_track(
            r#"<p><a href="https://dl.vgmsite.com/only.flac">FLAC</a></p>"#,
            "game-ost",
            2,
            &meta,
        )
        .unwrap();
        assert_eq!(fallback[0].1["extension"], "flac");
        let forged = decode_track(
            r#"<p><a href="https://evil-vgmsite.com/fake.mp3">Bad</a></p>"#,
            "game-ost",
            1,
            &meta,
        );
        assert!(forged.is_err());
    }

    #[tokio::test]
    async fn bounded_album_and_two_track_pages_resolve_three_local_http_requests() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (path, body) in [
                ("/game-soundtracks/album/game-ost", album_html()),
                (
                    "/game-soundtracks/album/game-ost/track-1",
                    r#"<p><a href="https://dl.vgmsite.com/first.mp3">MP3</a></p>"#,
                ),
                (
                    "/game-soundtracks/album/game-ost/track-2",
                    r#"<p><a href="https://dl.vgmtreasurechest.com/second.flac">FLAC</a></p>"#,
                ),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0u8; 4096];
                let n = socket.read(&mut request).await.unwrap();
                assert!(String::from_utf8_lossy(&request[..n])
                    .starts_with(&format!("GET {path} HTTP/1.1")));
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let album =
            match_url("https://downloads.khinsider.com/game-soundtracks/album/game-ost").unwrap();
        let files = records_at_base(&Client::new(), &album, &base)
            .await
            .unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].0.filename, "game-ost_001_first.mp3");
        assert_eq!(files[1].1["extension"], "flac");
        server.await.unwrap();
    }
}
