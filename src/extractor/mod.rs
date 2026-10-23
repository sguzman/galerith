//! An independently written extractor dispatcher. Only documented, tested subsets
//! are enabled. Unsupported gallery URLs fail rather than pretending parity.
pub mod bluesky;
pub mod danbooru;
pub mod e621;
pub mod foolfuuka;
pub mod fourchan;
pub mod gelbooru;
pub mod gelbooru_v02;
pub mod moebooru;
pub mod philomena;

use crate::{extract_direct_media, MediaItem};
use reqwest::Client;
use std::error::Error;

pub fn extractor_name(input: &str) -> Option<&'static str> {
    if foolfuuka::match_url(input).is_some()
        || foolfuuka::match_board_url(input).is_some()
        || foolfuuka::match_gallery_url(input).is_some()
        || foolfuuka::match_search_url(input).is_some()
    {
        Some("foolfuuka")
    } else if philomena::match_url(input).is_some() {
        Some("philomena")
    } else if moebooru::match_url(input).is_some() {
        Some("moebooru")
    } else if gelbooru_v02::match_url(input).is_some() {
        Some("gelbooru_v02")
    } else if fourchan::match_url(input).is_some() || fourchan::match_board_url(input).is_some() {
        Some("4chan")
    } else if gelbooru::match_url(input).is_some() {
        Some("gelbooru")
    } else if bluesky::match_url(input).is_some() {
        Some("bluesky")
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
    if let Some(target) = foolfuuka::match_url(input) {
        return foolfuuka::resolve(client, target).await;
    }
    if let Some(target) = foolfuuka::match_board_url(input) {
        return foolfuuka::resolve_board(client, target, max_pages).await;
    }
    if let Some(target) = foolfuuka::match_gallery_url(input) {
        return foolfuuka::resolve_gallery(client, target, max_pages).await;
    }
    if let Some(target) = foolfuuka::match_search_url(input) {
        return foolfuuka::resolve_search(client, target, max_pages).await;
    }
    if let Some(target) = philomena::match_url(input) {
        return philomena::resolve(client, target, max_pages).await;
    }
    if let Some(target) = moebooru::match_url(input) {
        return moebooru::resolve(client, target, max_pages).await;
    }
    if let Some(target) = gelbooru_v02::match_url(input) {
        return gelbooru_v02::resolve(client, target, max_pages).await;
    }
    if let Some(target) = fourchan::match_url(input) {
        return fourchan::resolve(client, target).await;
    }
    if let Some(board) = fourchan::match_board_url(input) {
        return fourchan::resolve_board(client, &board, max_pages).await;
    }
    if let Some(target) = gelbooru::match_url(input) {
        return gelbooru::resolve(client, target, max_pages).await;
    }
    if let Some(target) = bluesky::match_url(input) {
        return bluesky::resolve(client, target, max_pages).await;
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
            extractor_name("https://bsky.app/profile/alice.bsky.social/post/3n9"),
            Some("bluesky")
        );
        assert_eq!(
            extractor_name("https://bsky.app/profile/alice.bsky.social/avatar"),
            Some("bluesky")
        );
        assert_eq!(
            extractor_name("https://bsky.app/profile/alice.bsky.social/banner"),
            Some("bluesky")
        );
        assert_eq!(
            extractor_name("https://bsky.app/search?q=blue+fox"),
            Some("bluesky")
        );
        assert_eq!(
            extractor_name("https://bsky.app/hashtag/art/latest"),
            Some("bluesky")
        );
        assert_eq!(
            extractor_name("https://bsky.app/profile/alice.bsky.social/feed/gen1"),
            Some("bluesky")
        );
        assert_eq!(
            extractor_name("https://bsky.app/profile/alice.bsky.social/lists/list1"),
            Some("bluesky")
        );
        assert_eq!(
            extractor_name("https://danbooru.donmai.us/posts/random?tags=fox"),
            Some("danbooru")
        );
        assert_eq!(
            extractor_name("https://danbooru.donmai.us/artists?search%5Bname%5D=blue"),
            Some("danbooru")
        );
        assert_eq!(
            extractor_name("https://example.org/a.jpg"),
            Some("directlink")
        );
        assert_eq!(
            extractor_name("https://e621.cc/?tags=blue+sky"),
            Some("e621")
        );
        assert_eq!(extractor_name("https://e621.net/p/d"), Some("e621"));
        assert_eq!(
            extractor_name("https://e621.anthro.fr/?q=fox"),
            Some("e621")
        );
        assert_eq!(
            extractor_name("https://e621.net/post/index/3/fox"),
            Some("e621")
        );
        assert_eq!(extractor_name("https://pixiv.net/en/artworks/123"), None);
    }
}
