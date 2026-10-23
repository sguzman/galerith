//! An independently written extractor dispatcher. Only documented, tested subsets
//! are enabled. Unsupported gallery URLs fail rather than pretending parity.
pub mod bluesky;
pub mod catbox;
pub mod comedywildlifephoto;
pub mod danbooru;
pub mod desktopography;
pub mod e621;
pub mod foolfuuka;
pub mod fourchan;
pub mod gelbooru;
pub mod gelbooru_v02;
pub mod imagebam;
pub mod imgbb;
pub mod imgbox;
pub mod imgpile;
pub mod imgth;
pub mod lexica;
pub mod lightroom;
pub mod moebooru;
pub mod philomena;
pub mod photovogue;
pub mod pixeldrain;
pub mod speakerdeck;
pub mod telegraph;
pub mod unsplash;
pub mod wallhaven;
pub mod wallpapercave;
pub mod webmshare;
pub mod wikimedia;

use crate::{extract_direct_media, MediaItem};
use reqwest::Client;
use std::error::Error;

/// Only previews with a dispatcher implementation appear here. This is not
/// the complete gallery-dl site list and does not imply full extractor parity.
pub const PREVIEW_EXTRACTORS: &[(&str, &str)] = &[
    ("directlink", "direct HTTP(S) media URL subset"),
    ("4chan", "4chan/4channel threads and bounded boards"),
    ("bluesky", "public ATProto posts, profile media and feeds"),
    ("catbox", "public Catbox files and albums"),
    ("comedywildlifephoto", "public competition gallery images"),
    ("danbooru", "public Danbooru post, tag and collection APIs"),
    (
        "desktopography",
        "site, exhibition and portfolio wallpaper pages",
    ),
    ("e621", "e621/e926/e6ai public post and collection APIs"),
    (
        "foolfuuka",
        "archive threads, boards, galleries and searches",
    ),
    ("gelbooru", "public Gelbooru post and tag APIs"),
    ("gelbooru_v02", "XML/DAPI post and tag host family"),
    ("imagebam", "public image pages and bounded galleries"),
    ("imgbb", "public image pages and first-page albums"),
    ("imgbox", "public image pages and bounded galleries"),
    ("imgpile", "public post media and multiple-image pages"),
    (
        "imgth",
        "public gallery originals and bounded page traversal",
    ),
    (
        "lightroom",
        "public Lightroom shared albums and image renditions",
    ),
    (
        "lexica",
        "public prompt-image searches over POST cursor API",
    ),
    ("moebooru", "post, tag, pool and popular JSON routes"),
    ("philomena", "images, tags, searches and galleries"),
    ("photovogue", "public photographer photo API feeds"),
    (
        "pixeldrain",
        "public file, list and nonrecursive filesystem JSON APIs",
    ),
    ("speakerdeck", "public presentation slide images via oEmbed"),
    (
        "telegraph",
        "public article images and embedded figure filters",
    ),
    (
        "unsplash",
        "public photos, users, likes, collections and searches",
    ),
    (
        "wallhaven",
        "public wallpaper details, searches and uploads via API",
    ),
    (
        "wallpapercave",
        "public wallpaper download and gallery links",
    ),
    ("webmshare", "public video pages and OG media"),
    ("wikimedia", "public Commons File: images via Action API"),
];

pub fn extractor_name(input: &str) -> Option<&'static str> {
    if foolfuuka::match_url(input).is_some()
        || foolfuuka::match_board_url(input).is_some()
        || foolfuuka::match_gallery_url(input).is_some()
        || foolfuuka::match_search_url(input).is_some()
    {
        Some("foolfuuka")
    } else if philomena::match_url(input).is_some() {
        Some("philomena")
    } else if photovogue::match_url(input).is_some() {
        Some("photovogue")
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
    } else if catbox::match_url(input).is_some() {
        Some("catbox")
    } else if comedywildlifephoto::match_url(input).is_some() {
        Some("comedywildlifephoto")
    } else if imagebam::match_url(input).is_some() {
        Some("imagebam")
    } else if imgbb::match_url(input).is_some() {
        Some("imgbb")
    } else if imgbox::match_url(input).is_some() {
        Some("imgbox")
    } else if imgpile::match_url(input).is_some() {
        Some("imgpile")
    } else if imgth::match_url(input).is_some() {
        Some("imgth")
    } else if lightroom::match_url(input).is_some() {
        Some("lightroom")
    } else if lexica::match_url(input).is_some() {
        Some("lexica")
    } else if e621::match_url(input).is_some() {
        Some("e621")
    } else if danbooru::match_url(input).is_some() {
        Some("danbooru")
    } else if desktopography::match_url(input).is_some() {
        Some("desktopography")
    } else if pixeldrain::match_url(input).is_some() {
        Some("pixeldrain")
    } else if telegraph::match_url(input).is_some() {
        Some("telegraph")
    } else if speakerdeck::match_url(input).is_some() {
        Some("speakerdeck")
    } else if wallhaven::match_url(input).is_some() {
        Some("wallhaven")
    } else if wallpapercave::match_url(input).is_some() {
        Some("wallpapercave")
    } else if webmshare::match_url(input).is_some() {
        Some("webmshare")
    } else if wikimedia::match_url(input).is_some() {
        Some("wikimedia")
    } else if unsplash::match_url(input).is_some() {
        Some("unsplash")
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
    if let Some(target) = photovogue::match_url(input) {
        return photovogue::resolve(client, target, max_pages).await;
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
    if let Some(target) = catbox::match_url(input) {
        return catbox::resolve(client, target).await;
    }
    if let Some(target) = comedywildlifephoto::match_url(input) {
        return comedywildlifephoto::resolve(client, target).await;
    }
    if let Some(target) = imagebam::match_url(input) {
        return imagebam::resolve(client, target, max_pages).await;
    }
    if let Some(target) = imgbb::match_url(input) {
        return imgbb::resolve(client, target).await;
    }
    if let Some(target) = imgbox::match_url(input) {
        return imgbox::resolve(client, target).await;
    }
    if let Some(target) = imgpile::match_url(input) {
        return imgpile::resolve(client, target, max_pages).await;
    }
    if let Some(target) = imgth::match_url(input) {
        return imgth::resolve(client, target, max_pages).await;
    }
    if let Some(target) = lightroom::match_url(input) {
        return lightroom::resolve(client, target, max_pages).await;
    }
    if let Some(target) = lexica::match_url(input) {
        return lexica::resolve(client, target, max_pages).await;
    }
    if let Some(target) = e621::match_url(input) {
        return e621::resolve(client, target, max_pages).await;
    }
    if let Some(target) = danbooru::match_url(input) {
        return danbooru::resolve(client, target, max_pages).await;
    }
    if let Some(target) = desktopography::match_url(input) {
        return desktopography::resolve(client, target, max_pages).await;
    }
    if let Some(target) = pixeldrain::match_url(input) {
        return pixeldrain::resolve(client, target).await;
    }
    if let Some(target) = telegraph::match_url(input) {
        return telegraph::resolve(client, target).await;
    }
    if let Some(target) = speakerdeck::match_url(input) {
        return speakerdeck::resolve(client, target).await;
    }
    if let Some(target) = wallhaven::match_url(input) {
        return wallhaven::resolve(client, target, max_pages).await;
    }
    if let Some(target) = wallpapercave::match_url(input) {
        return wallpapercave::resolve(client, target).await;
    }
    if let Some(target) = webmshare::match_url(input) {
        return webmshare::resolve(client, target).await;
    }
    if let Some(target) = wikimedia::match_url(input) {
        return wikimedia::resolve(client, target, max_pages).await;
    }
    if let Some(target) = unsplash::match_url(input) {
        return unsplash::resolve(client, target, max_pages).await;
    }
    Ok(vec![extract_direct_media(input)?])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preview_registry_matches_routed_extractors() {
        let fixtures = [
            ("https://example.org/file.jpg", "directlink"),
            ("https://boards.4chan.org/g/thread/123", "4chan"),
            (
                "https://bsky.app/profile/alice.bsky.social/post/3n9",
                "bluesky",
            ),
            ("https://catbox.moe/c/album123", "catbox"),
            (
                "https://www.comedywildlifephoto.com/gallery/2026/finalists.php",
                "comedywildlifephoto",
            ),
            ("https://danbooru.donmai.us/posts/123", "danbooru"),
            (
                "https://desktopography.net/portfolios/forest-glow",
                "desktopography",
            ),
            ("https://e621.net/posts/123", "e621"),
            ("https://archive.4plebs.org/tg/thread/123", "foolfuuka"),
            (
                "https://gelbooru.com/index.php?page=post&s=view&id=123",
                "gelbooru",
            ),
            (
                "https://safebooru.org/index.php?tags=blue+sky&s=list&page=post",
                "gelbooru_v02",
            ),
            ("https://www.imagebam.com/view/M123abc", "imagebam"),
            ("https://ibb.co/album/album123", "imgbb"),
            ("https://imgbox.com/a1B2c3D4", "imgbox"),
            ("https://imgpile.com/p/AbCdEfG", "imgpile"),
            ("https://imgth.com/gallery/123/great", "imgth"),
            ("https://lightroom.adobe.com/shares/abcdef0123", "lightroom"),
            ("https://lexica.art/?q=blue+sky", "lexica"),
            ("https://imgpile.com/u/creator-1", "imgpile"),
            ("https://pixeldrain.com/d/root123", "pixeldrain"),
            ("https://telegra.ph/Article-03-04", "telegraph"),
            ("https://speakerdeck.com/person/my-deck", "speakerdeck"),
            ("https://wallpapercave.com/w/wp12345", "wallpapercave"),
            ("https://wallhaven.cc/w/94x38z", "wallhaven"),
            ("https://wallhaven.cc/search?q=blue+sky", "wallhaven"),
            (
                "https://wallhaven.cc/user/example-1/favorites/12345",
                "wallhaven",
            ),
            ("https://webmshare.com/play/ABc_123", "webmshare"),
            (
                "https://commons.wikimedia.org/wiki/File:Blue_sky.png",
                "wikimedia",
            ),
            (
                "https://commons.wikimedia.org/wiki/Category:Night_sky",
                "wikimedia",
            ),
            (
                "https://commons.wikimedia.org/wiki/Featured_pictures",
                "wikimedia",
            ),
            ("https://unsplash.com/photos/abc-123", "unsplash"),
            (
                "https://www.vogue.com/photovogue/photographers/12345",
                "photovogue",
            ),
            ("https://pixeldrain.com/api/list/album1/zip", "pixeldrain"),
            ("https://yande.re/post/show/123", "moebooru"),
            ("https://derpibooru.org/images/123", "philomena"),
            ("https://pixeldrain.com/u/File12", "pixeldrain"),
        ];
        let mut names = std::collections::HashSet::new();
        for &(name, description) in PREVIEW_EXTRACTORS {
            assert!(!name.is_empty() && !description.is_empty());
            assert!(names.insert(name), "duplicate preview name: {name}");
        }
        for (url, expected) in fixtures {
            assert_eq!(extractor_name(url), Some(expected));
            assert!(names.contains(expected), "undocumented preview: {expected}");
        }
        let covered: std::collections::HashSet<_> =
            fixtures.iter().map(|(_, expected)| *expected).collect();
        assert_eq!(names, covered);
    }

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
            extractor_name("https://bsky.app/profile/alice.bsky.social/likes"),
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
            extractor_name("https://files.catbox.moe/abc123.jpg"),
            Some("catbox")
        );
        assert_eq!(
            extractor_name("https://catbox.moe/c/album123"),
            Some("catbox")
        );
        assert_eq!(
            extractor_name("https://pixeldrain.com/l/album123#item=1"),
            Some("pixeldrain")
        );
        assert_eq!(
            extractor_name("https://imgbox.com/g/abcdefghij"),
            Some("imgbox")
        );
        assert_eq!(
            extractor_name("https://www.imagebam.com/gallery/album7"),
            Some("imagebam")
        );
        assert_eq!(extractor_name("https://ibb.co/someImage12"), Some("imgbb"));
        assert_eq!(
            extractor_name("https://imgpile.com/p/PostAB12"),
            Some("imgpile")
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
