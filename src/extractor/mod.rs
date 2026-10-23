//! An independently written extractor dispatcher. Only documented, tested subsets
//! are enabled. Unsupported gallery URLs fail rather than pretending parity.
pub mod danbooru;
pub mod e621;
pub mod fourchan;
pub mod gelbooru;

use crate::{extract_direct_media, MediaItem};
use reqwest::Client;
use std::error::Error;

pub fn extractor_name(input: &str) -> Option<&'static str> {
    if fourchan::match_url(input).is_some() {
        Some("4chan")
    } else if gelbooru::match_url(input).is_some() {
        Some("gelbooru")
    } else if e621::match_url(input).is_some() {
        Some("e621")
    } else if danbooru::match_url(input).is_some() {
        Some("danbooru")
    } else if extract_direct_media(input).is_ok() {
        Some("directlink")
    } else {
        None
    }
}

pub async fn resolve(
    client: &Client,
    input: &str,
    max_pages: u16,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    if let Some(target) = fourchan::match_url(input) {
        return fourchan::resolve(client, target).await;
    }
    if let Some(target) = gelbooru::match_url(input) {
        return gelbooru::resolve(client, target, max_pages).await;
    }
    if let Some(target) = e621::match_url(input) {
        return e621::resolve(client, target, max_pages).await;
    }
    if let Some(target) = danbooru::match_url(input) {
        return danbooru::resolve(client, target, max_pages).await;
    }
    Ok(vec![extract_direct_media(input)?])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dispatch_does_not_claim_unsupported_sites() {
        assert_eq!(
            extractor_name("https://danbooru.donmai.us/posts/123"),
            Some("danbooru")
        );
        assert_eq!(
            extractor_name("https://example.org/a.jpg"),
            Some("directlink")
        );
        assert_eq!(extractor_name("https://pixiv.net/en/artworks/123"), None);
    }
}
