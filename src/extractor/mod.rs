//! An independently written extractor dispatcher. Only documented, tested subsets
//! are enabled. Unsupported gallery URLs fail rather than pretending parity.
pub mod arena;
pub mod basic_hosts;
pub mod bluesky;
pub mod catbox;
pub mod comedywildlifephoto;
pub mod danbooru;
pub mod desktopography;
pub mod directlink;
pub mod e621;
pub mod eightmuses;
pub mod fappic;
pub mod fivehundredpx;
pub mod foolfuuka;
pub mod fourarchive;
pub mod fourchan;
pub mod fourchanarchives;
pub mod gelbooru;
pub mod gelbooru_v02;
pub mod http_json;
pub mod imagebam;
pub mod imagechest;
pub mod imagehosts;
pub mod imagepond;
pub mod imageshack;
pub mod imagetwist;
pub mod imagevenue;
pub mod imgadult;
pub mod imgbb;
pub mod imgbox;
pub mod imgclick;
pub mod imgdrive;
pub mod imgpile;
pub mod imgpv;
pub mod imgth;
pub mod imgur;
pub mod imhentai;
pub mod imxto;
pub mod issuu;
pub mod kabeuchi;
pub mod keenspot;
pub mod khinsider;
pub mod lensdump;
pub mod lexica;
pub mod lightroom;
pub mod listal;
pub mod moebooru;
pub mod philomena;
pub mod photovogue;
pub mod picstate;
pub mod pixeldrain;
pub mod postimg;
pub mod silverpic;
pub mod speakerdeck;
pub mod telegraph;
pub mod thirtyfivephoto;
pub mod turboimagehost;
pub mod twoch;
pub mod twochan;
pub mod twochen;
pub mod unsplash;
pub mod urlshortener;
pub mod wallhaven;
pub mod wallpapercave;
pub mod webmshare;
pub mod wikimedia;

use crate::{
    extract_direct_media,
    message::{
        from_media, from_source_post_media, traverse_queue_job_filtered_with, ExtractorMessage,
        JobSelection,
    },
    MediaItem,
};
use reqwest::Client;
use std::error::Error;

/// Only previews with a dispatcher implementation appear here. This is not
/// the complete gallery-dl site list and does not imply full extractor parity.
pub const PREVIEW_EXTRACTORS: &[(&str, &str)] = &[
    ("directlink", "direct HTTP(S) media URL subset"),
    (
        "arena",
        "bounded public Are.na channel images and attachments",
    ),
    (
        "urlshortener",
        "bounded bit.ly and t.co safe public link queues",
    ),
    ("4chan", "4chan/4channel threads and bounded boards"),
    (
        "8muses",
        "bounded public encoded album images and child queues",
    ),
    (
        "35photo",
        "guarded public image originals and listing queues",
    ),
    (
        "4archive",
        "public archived thread originals and bounded board queues",
    ),
    (
        "4chanarchives",
        "public archived thread originals and bounded board queues",
    ),
    ("2ch", "pinned 2ch public threads and bounded board queues"),
    (
        "2chen",
        "pinned 2chen public thread HTML and catalog queues",
    ),
    ("2chan", "pinned 2chan public Futaba thread originals"),
    ("bluesky", "public ATProto posts, profile media and feeds"),
    ("catbox", "public Catbox files and albums"),
    ("comedywildlifephoto", "public competition gallery images"),
    ("danbooru", "public Danbooru post, tag and collection APIs"),
    (
        "desktopography",
        "site, exhibition and portfolio wallpaper pages",
    ),
    ("e621", "e621/e926/e6ai public post and collection APIs"),
    ("fappic", "pinned public Fappic original-image preview"),
    (
        "imagevenue",
        "pinned public ImageVenue single-image preview",
    ),
    ("imgclick", "pinned public Imgclick original-image preview"),
    (
        "imgur",
        "pinned public Imgur images, albums and gallery queues",
    ),
    ("imhentai", "pinned public IMHentai-family galleries"),
    ("issuu", "pinned public Issuu publication pages"),
    (
        "khinsider",
        "public soundtrack albums with bounded track formats",
    ),
    (
        "kabeuchi",
        "public image-post user feeds with bounded pagination",
    ),
    ("keenspot", "public subdomain webcomic page originals"),
    (
        "listal",
        "pinned public Listal images and people-picture queues",
    ),
    (
        "lensdump",
        "pinned Lensdump public image and album originals",
    ),
    (
        "imagetwist",
        "pinned public Imagetwist and Imagehaha originals",
    ),
    (
        "foolfuuka",
        "archive threads, boards, galleries and searches",
    ),
    ("gelbooru", "public Gelbooru post and tag APIs"),
    ("gelbooru_v02", "XML/DAPI post and tag host family"),
    ("imagebam", "public image pages and bounded galleries"),
    ("imagechest", "public ImageChest gallery page media"),
    (
        "pixhost",
        "public Pixhost image pages and safe gallery queues",
    ),
    ("acidimg", "pinned public Acidimg POST image preview"),
    ("vipr", "pinned public Vipr image preview"),
    ("imgspice", "pinned public Imgspice image preview"),
    ("imagepond", "public ImagePond single-file OG media pages"),
    ("picstate", "public Picstate original image pages"),
    (
        "imgadult",
        "pinned public Imgadult cookie-based image preview",
    ),
    ("imgdrive", "pinned public Imgdrive OG image preview"),
    ("imgtaxi", "pinned public Imgtaxi OG image preview"),
    ("imgwallet", "pinned public Imgwallet OG image preview"),
    ("imxto", "pinned Imxto single images and guarded galleries"),
    ("imageshack", "public ImageShack individual image REST API"),
    ("imgbb", "public image pages and first-page albums"),
    ("imgbox", "public image pages and bounded galleries"),
    ("imgpile", "public post media and multiple-image pages"),
    ("imgpv", "pinned public Imgpv original-image subset"),
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
    (
        "500px",
        "public 500px single-photo expanded source originals",
    ),
    (
        "3dbooru",
        "pinned Behoimi post, tag, pool and popular Moebooru routes",
    ),
    ("philomena", "images, tags, searches and galleries"),
    ("photovogue", "public photographer photo API feeds"),
    (
        "pixeldrain",
        "public file, list and nonrecursive filesystem JSON APIs",
    ),
    (
        "postimg",
        "public Postimages files and bounded gallery queues",
    ),
    ("speakerdeck", "public presentation slide images via oEmbed"),
    ("silverpic", "pinned public Silverpic original-image subset"),
    (
        "telegraph",
        "public article images and embedded figure filters",
    ),
    ("turboimagehost", "public Turboimagehost single-image pages"),
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
    if arena::match_url(input).is_some() {
        Some("arena")
    } else if urlshortener::match_url(input).is_some() {
        Some("urlshortener")
    } else if foolfuuka::match_url(input).is_some()
        || foolfuuka::match_board_url(input).is_some()
        || foolfuuka::match_gallery_url(input).is_some()
        || foolfuuka::match_search_url(input).is_some()
    {
        Some("foolfuuka")
    } else if philomena::match_url(input).is_some() {
        Some("philomena")
    } else if photovogue::match_url(input).is_some() {
        Some("photovogue")
    } else if fivehundredpx::match_url(input).is_some() {
        Some("500px")
    } else if moebooru::is_3dbooru(input) {
        Some("3dbooru")
    } else if moebooru::match_url(input).is_some() {
        Some("moebooru")
    } else if gelbooru_v02::match_url(input).is_some() {
        Some("gelbooru_v02")
    } else if fourchan::match_url(input).is_some() || fourchan::match_board_url(input).is_some() {
        Some("4chan")
    } else if twoch::match_url(input).is_some() {
        Some("2ch")
    } else if twochen::match_url(input).is_some() {
        Some("2chen")
    } else if twochan::match_url(input).is_some() {
        Some("2chan")
    } else if fourarchive::match_url(input).is_some() {
        Some("4archive")
    } else if fourchanarchives::match_url(input).is_some() {
        Some("4chanarchives")
    } else if eightmuses::match_url(input).is_some() {
        Some("8muses")
    } else if thirtyfivephoto::match_url(input).is_some() {
        Some("35photo")
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
    } else if imagechest::match_url(input).is_some() || imagechest::match_user_url(input).is_some()
    {
        Some("imagechest")
    } else if let Some(target) = basic_hosts::match_url(input) {
        Some(target.host.name())
    } else if imagehosts::match_url(input).is_some() {
        Some("pixhost")
    } else if imgadult::match_url(input).is_some() {
        Some("imgadult")
    } else if silverpic::match_url(input).is_some() {
        Some("silverpic")
    } else if imgpv::match_url(input).is_some() {
        Some("imgpv")
    } else if fappic::match_url(input).is_some() {
        Some("fappic")
    } else if imagevenue::match_url(input).is_some() {
        Some("imagevenue")
    } else if imgclick::match_url(input).is_some() {
        Some("imgclick")
    } else if imgur::match_url(input).is_some() {
        Some("imgur")
    } else if imhentai::match_url(input).is_some() || imhentai::listing::match_url(input).is_some()
    {
        Some("imhentai")
    } else if issuu::match_url(input).is_some() || issuu::publisher::match_url(input).is_some() {
        Some("issuu")
    } else if listal::match_url(input).is_some() {
        Some("listal")
    } else if lensdump::match_url(input).is_some() || lensdump::user::match_url(input).is_some() {
        Some("lensdump")
    } else if khinsider::match_url(input).is_some() {
        Some("khinsider")
    } else if kabeuchi::match_url(input).is_some() {
        Some("kabeuchi")
    } else if keenspot::match_url(input).is_some() {
        Some("keenspot")
    } else if imagetwist::match_url(input).is_some()
        || imagetwist::match_gallery_url(input).is_some()
    {
        Some("imagetwist")
    } else if let Some(target) = imgdrive::match_url(input) {
        Some(target.family.category())
    } else if imxto::match_url(input).is_some() {
        Some("imxto")
    } else if picstate::match_url(input).is_some() {
        Some("picstate")
    } else if imagepond::match_url(input).is_some() || imagepond::match_queue_url(input).is_some() {
        Some("imagepond")
    } else if imageshack::match_url(input).is_some()
        || imageshack::match_listing_url(input).is_some()
    {
        Some("imageshack")
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
    } else if turboimagehost::match_url(input).is_some()
        || turboimagehost::match_album_url(input).is_some()
    {
        Some("turboimagehost")
    } else if postimg::match_url(input).is_some() {
        Some("postimg")
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
    if let Some(channel) = arena::match_url(input) {
        return arena::resolve(client, channel, max_pages).await;
    }
    if urlshortener::match_url(input).is_some() {
        return Err("Shortened public links require --dump-messages or --follow-queues".into());
    }
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
    if let Some(photo) = fivehundredpx::match_url(input) {
        return fivehundredpx::resolve(client, photo).await;
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
    if let Some(target) = twoch::match_url(input) {
        return twoch::resolve(client, target).await;
    }
    if let Some(target) = twochen::match_url(input) {
        return twochen::resolve(client, target).await;
    }
    if let Some(target) = twochan::match_url(input) {
        return twochan::resolve(client, target).await;
    }
    if let Some(target) = fourarchive::match_url(input) {
        return fourarchive::resolve(client, target).await;
    }
    if let Some(target) = fourchanarchives::match_url(input) {
        return fourchanarchives::resolve(client, target).await;
    }
    if let Some(album) = eightmuses::match_url(input) {
        return eightmuses::resolve(client, album).await;
    }
    if let Some(target) = thirtyfivephoto::match_url(input) {
        return thirtyfivephoto::resolve(client, target).await;
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
    if let Some(target) = imagechest::match_url(input) {
        return imagechest::resolve(client, target).await;
    }
    if let Some(target) = basic_hosts::match_url(input) {
        return basic_hosts::resolve(client, target).await;
    }
    if let Some(target) = imagehosts::match_url(input) {
        return imagehosts::resolve(client, target).await;
    }
    if let Some(target) = imgadult::match_url(input) {
        return imgadult::resolve(client, target).await;
    }
    if let Some(target) = silverpic::match_url(input) {
        return silverpic::resolve(client, target).await;
    }
    if let Some(target) = imgpv::match_url(input) {
        return imgpv::resolve(client, target).await;
    }
    if let Some(target) = fappic::match_url(input) {
        return fappic::resolve(client, target).await;
    }
    if let Some(target) = imagevenue::match_url(input) {
        return imagevenue::resolve(client, target).await;
    }
    if let Some(target) = imgclick::match_url(input) {
        return imgclick::resolve(client, target).await;
    }
    if let Some(target) = imgur::match_url(input) {
        return imgur::resolve(client, target).await;
    }
    if let Some(gallery) = imhentai::match_url(input) {
        return imhentai::resolve(client, gallery).await;
    }
    if let Some(publication) = issuu::match_url(input) {
        return issuu::resolve(client, publication).await;
    }
    if let Some(target) = listal::match_url(input) {
        return listal::resolve(client, target).await;
    }
    if let Some(target) = lensdump::match_url(input) {
        return lensdump::resolve(client, target, 4).await;
    }
    if lensdump::user::match_url(input).is_some() {
        return Err("Lensdump user albums require --dump-messages or --follow-queues".into());
    }
    if let Some(album) = khinsider::match_url(input) {
        return khinsider::resolve(client, album).await;
    }
    if let Some(user) = kabeuchi::match_url(input) {
        return kabeuchi::resolve(client, user, 4).await;
    }
    if let Some(comic) = keenspot::match_url(input) {
        return keenspot::resolve(client, comic).await;
    }
    if issuu::publisher::match_url(input).is_some() {
        return Err("Issuu publisher feeds require --dump-messages or --follow-queues".into());
    }
    if imhentai::listing::match_url(input).is_some() {
        return Err("IMHentai public listings require --dump-messages or --follow-queues".into());
    }
    if let Some(target) = imagetwist::match_url(input) {
        return imagetwist::resolve(client, target).await;
    }
    if imagetwist::match_gallery_url(input).is_some() {
        return Err("Imagetwist gallery requires --dump-messages or --follow-queues".into());
    }
    if let Some(target) = imgdrive::match_url(input) {
        return imgdrive::resolve(client, target).await;
    }
    if let Some(target) = imxto::match_url(input) {
        return imxto::resolve(client, target).await;
    }
    if let Some(target) = picstate::match_url(input) {
        return picstate::resolve(client, target).await;
    }
    if imagechest::match_user_url(input).is_some() {
        return Err(
            "ImageChest profile traversal requires --follow-queues or --dump-messages".into(),
        );
    }
    if let Some(target) = imagepond::match_url(input) {
        return imagepond::resolve(client, target).await;
    }
    if imagepond::match_queue_url(input).is_some() {
        return Err(
            "ImagePond album/profile traversal requires --follow-queues or --dump-messages".into(),
        );
    }
    if let Some(image) = imageshack::match_url(input) {
        return imageshack::resolve(client, image).await;
    }
    if let Some(listing) = imageshack::match_listing_url(input) {
        return imageshack::resolve_listing(client, listing, max_pages).await;
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
    if let Some(target) = turboimagehost::match_url(input) {
        return turboimagehost::resolve(client, target).await;
    }
    if turboimagehost::match_album_url(input).is_some() {
        return Err(
            "Turboimagehost album traversal requires --follow-queues or --dump-messages".into(),
        );
    }
    if let Some(target) = postimg::match_url(input) {
        return postimg::resolve(client, target).await;
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

/// Read-only preview of the extractor message stream. Flattened site
/// adapters produce directory/URL events from known MediaItem fields.
/// Wallhaven public collection indexes instead retain child queue events.
pub async fn resolve_messages(
    client: &Client,
    input: &str,
    max_pages: u16,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    if let Some(channel) = arena::match_url(input) {
        return arena::resolve_messages(client, channel, max_pages).await;
    }
    if let Some(wallhaven::Target::Collections { user }) = wallhaven::match_url(input) {
        return wallhaven::queued_collections(client, &user, max_pages).await;
    }
    if let Some(short) = urlshortener::match_url(input) {
        return urlshortener::resolve_messages(short).await;
    }
    if let Some(target) = fourchan::match_url(input) {
        return fourchan::resolve_messages(client, target).await;
    }
    if let Some(target) = twoch::match_url(input) {
        return twoch::resolve_messages(client, target, max_pages).await;
    }
    if let Some(target) = twochen::match_url(input) {
        return twochen::resolve_messages(client, target).await;
    }
    if let Some(target) = twochan::match_url(input) {
        return twochan::resolve_messages(client, target).await;
    }
    if let Some(target) = fourarchive::match_url(input) {
        return fourarchive::resolve_messages(client, target, max_pages).await;
    }
    if let Some(target) = fourchanarchives::match_url(input) {
        return fourchanarchives::resolve_messages(client, target, max_pages).await;
    }
    if let Some(album) = eightmuses::match_url(input) {
        return eightmuses::resolve_messages(client, album, max_pages).await;
    }
    if let Some(target) = thirtyfivephoto::match_url(input) {
        return thirtyfivephoto::resolve_messages(client, target, max_pages).await;
    }
    if let Some(board) = fourchan::match_board_url(input) {
        return fourchan::resolve_board_messages(client, &board, max_pages).await;
    }
    if let Some(user) = imagechest::match_user_url(input) {
        return imagechest::queued_user_galleries(client, &user, max_pages).await;
    }
    if let Some(gallery) = imagechest::match_url(input) {
        return imagechest::resolve_messages(client, gallery).await;
    }
    if let Some(target) = basic_hosts::match_url(input) {
        return basic_hosts::resolve_messages(client, target).await;
    }
    if let Some(target) = imagehosts::match_url(input) {
        return imagehosts::resolve_messages(client, target).await;
    }
    if let Some(target) = imgadult::match_url(input) {
        return imgadult::resolve_messages(client, target).await;
    }
    if let Some(target) = silverpic::match_url(input) {
        return silverpic::resolve_messages(client, target).await;
    }
    if let Some(target) = imgpv::match_url(input) {
        return imgpv::resolve_messages(client, target).await;
    }
    if let Some(target) = fappic::match_url(input) {
        return fappic::resolve_messages(client, target).await;
    }
    if let Some(target) = imagevenue::match_url(input) {
        return imagevenue::resolve_messages(client, target).await;
    }
    if let Some(target) = imgclick::match_url(input) {
        return imgclick::resolve_messages(client, target).await;
    }
    if let Some(target) = imgur::match_url(input) {
        return imgur::resolve_messages(client, target, max_pages).await;
    }
    if let Some(gallery) = imhentai::match_url(input) {
        return imhentai::resolve_messages(client, gallery).await;
    }
    if let Some(publication) = issuu::match_url(input) {
        return issuu::resolve_messages(client, publication).await;
    }
    if let Some(target) = listal::match_url(input) {
        return listal::resolve_messages(client, target, max_pages).await;
    }
    if let Some(target) = lensdump::match_url(input) {
        return lensdump::resolve_messages(client, target, max_pages).await;
    }
    if let Some(user) = lensdump::user::match_url(input) {
        return lensdump::user::resolve_messages(client, user, max_pages).await;
    }
    if let Some(album) = khinsider::match_url(input) {
        return khinsider::resolve_messages(client, album).await;
    }
    if let Some(user) = kabeuchi::match_url(input) {
        return kabeuchi::resolve_messages(client, user, max_pages).await;
    }
    if let Some(comic) = keenspot::match_url(input) {
        return keenspot::resolve_messages(client, comic).await;
    }
    if let Some(publisher) = issuu::publisher::match_url(input) {
        return issuu::publisher::resolve_messages(client, publisher, max_pages).await;
    }
    if let Some(listing) = imhentai::listing::match_url(input) {
        return imhentai::listing::resolve_messages(client, listing, max_pages).await;
    }
    if let Some(target) = imagetwist::match_url(input) {
        return imagetwist::resolve_messages(client, target).await;
    }
    if let Some(target) = imagetwist::match_gallery_url(input) {
        return imagetwist::resolve_gallery_messages(client, target, max_pages).await;
    }
    if let Some(target) = imgdrive::match_url(input) {
        return imgdrive::resolve_messages(client, target).await;
    }
    if let Some(target) = imxto::match_url(input) {
        return imxto::resolve_messages(client, target, max_pages).await;
    }
    if let Some(target) = picstate::match_url(input) {
        return picstate::resolve_messages(client, target).await;
    }
    if let Some(target) = imagepond::match_url(input) {
        return imagepond::resolve_messages(client, target).await;
    }
    if let Some(target) = imagepond::match_queue_url(input) {
        return imagepond::resolve_queue_messages(client, target, max_pages).await;
    }
    if let Some(image) = imageshack::match_url(input) {
        return imageshack::resolve_messages(client, image).await;
    }
    if let Some(target) = turboimagehost::match_url(input) {
        return turboimagehost::resolve_messages(client, target).await;
    }
    if let Some(album) = turboimagehost::match_album_url(input) {
        return turboimagehost::resolve_album_messages(client, album, max_pages).await;
    }
    if let Some(target) = postimg::match_url(input) {
        return postimg::resolve_messages(client, target, max_pages).await;
    }
    if let Some(listing) = imageshack::match_listing_url(input) {
        return imageshack::resolve_listing_messages(client, listing, max_pages).await;
    }
    if extractor_name(input) == Some("directlink") {
        return Ok(directlink::resolve_messages(input)?);
    }
    if let Some(photo) = fivehundredpx::match_url(input) {
        return fivehundredpx::resolve_messages(client, photo).await;
    }
    if let Some(target) = moebooru::match_url(input) {
        return Ok(from_source_post_media(
            moebooru::resolve(client, target, max_pages).await?,
        ));
    }
    if let Some(target) = gelbooru_v02::match_url(input) {
        return Ok(from_source_post_media(
            gelbooru_v02::resolve(client, target, max_pages).await?,
        ));
    }
    if let Some(target) = gelbooru::match_url(input) {
        return Ok(from_source_post_media(
            gelbooru::resolve(client, target, max_pages).await?,
        ));
    }
    if let Some(target) = philomena::match_url(input) {
        return Ok(from_source_post_media(
            philomena::resolve(client, target, max_pages).await?,
        ));
    }
    if let Some(target) = e621::match_url(input) {
        return Ok(from_source_post_media(
            e621::resolve(client, target, max_pages).await?,
        ));
    }
    if let Some(target) = danbooru::match_url(input) {
        // An isolated media-asset identifier is not necessarily its post ID.
        // Do not invent a source-post identity for this special endpoint.
        let is_asset = matches!(&target, danbooru::Target::MediaAsset { .. });
        let media = danbooru::resolve(client, target, max_pages).await?;
        return Ok(if is_asset {
            from_media(media)
        } else {
            from_source_post_media(media)
        });
    }
    Ok(from_media(resolve(client, input, max_pages).await?))
}

/// Push a finite selected-file ceiling into supported API pagination so
/// the extractor avoids HTTP pages that cannot contain requested files.
/// Other site previews retain their existing bounded eager resolution.
async fn resolve_messages_bounded(
    client: &Client,
    input: &str,
    max_pages: u16,
    file_bound: Option<usize>,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    if let (Some(limit), Some(target)) = (file_bound, wallhaven::match_url(input)) {
        if !matches!(target, wallhaven::Target::Collections { .. }) {
            return Ok(from_media(
                wallhaven::resolve_limited(client, target, max_pages, limit).await?,
            ));
        }
    }
    if let (Some(limit), Some(target)) = (file_bound, gelbooru::match_url(input)) {
        return Ok(from_source_post_media(
            gelbooru::resolve_limited(client, target, max_pages, limit).await?,
        ));
    }
    if let (Some(limit), Some(target)) = (file_bound, gelbooru_v02::match_url(input)) {
        return Ok(from_source_post_media(
            gelbooru_v02::resolve_limited(client, target, max_pages, limit).await?,
        ));
    }
    if let (Some(limit), Some(target)) = (file_bound, moebooru::match_url(input)) {
        return Ok(from_source_post_media(
            moebooru::resolve_limited(client, target, max_pages, limit).await?,
        ));
    }
    if let (Some(limit), Some(target @ danbooru::Target::Search { .. })) =
        (file_bound, danbooru::match_url(input))
    {
        return Ok(from_source_post_media(
            danbooru::resolve_limited(client, target, max_pages, limit).await?,
        ));
    }
    if let (Some(limit), Some(target)) = (file_bound, e621::match_url(input)) {
        if matches!(
            &target,
            e621::Target::Search { .. }
                | e621::Target::Popular { .. }
                | e621::Target::Favorites { .. }
        ) {
            return Ok(from_source_post_media(
                e621::resolve_limited(client, target, max_pages, limit).await?,
            ));
        }
    }
    if let (Some(limit), Some(target)) = (file_bound, philomena::match_url(input)) {
        return Ok(from_source_post_media(
            philomena::resolve_limited(client, target, max_pages, limit).await?,
        ));
    }
    resolve_messages(client, input, max_pages).await
}

/// Queue events delegate to known website extractors, never to arbitrary
/// direct-file hosts or a local-network endpoint. All current producers
/// construct first-party public child URLs rather than consuming remote URLs.
fn safe_queue_child_url(input: &str) -> bool {
    let Ok(parsed) = url::Url::parse(input) else {
        return false;
    };
    let Some(url::Host::Domain(domain)) = parsed.host() else {
        return false;
    };
    let domain = domain.trim_end_matches('.').to_ascii_lowercase();
    matches!(parsed.scheme(), "http" | "https")
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && !domain.is_empty()
        && domain != "localhost"
        && !domain.ends_with(".localhost")
        && !domain.ends_with(".local")
        && !domain.ends_with(".internal")
        && extractor_name(input).is_some_and(|category| category != "directlink")
}

/// Opt-in, bounded queue traversal for message inspection. Parent queue
/// events are retained; child resolvers inherit metadata scopes, with child
/// fields taking precedence. This is not a full gallery-dl download job.
pub async fn resolve_messages_traversing_selected(
    client: &Client,
    input: &str,
    max_pages: u16,
    max_children: u8,
    depth: u8,
    selection: &JobSelection,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    let file_bound = selection.early_file_ceiling();
    let events = resolve_messages_bounded(client, input, max_pages, file_bound).await?;
    traverse_queue_job_filtered_with(
        events,
        depth,
        max_children as usize,
        selection,
        |url| async move {
            if !safe_queue_child_url(&url) {
                return Err("queued URL has no supported safe native extractor".into());
            }
            tokio::time::sleep(std::time::Duration::from_millis(1400)).await;
            // Bound every queued child's page count independently; per-site
            // extractors may issue more than one API request per page.
            resolve_messages_bounded(client, &url, max_pages.min(4), file_bound).await
        },
    )
    .await
}

/// Unfiltered compatibility entry point used by message inspection.
pub async fn resolve_messages_traversing(
    client: &Client,
    input: &str,
    max_pages: u16,
    max_children: u8,
    depth: u8,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    resolve_messages_traversing_selected(
        client,
        input,
        max_pages,
        max_children,
        depth,
        &JobSelection::default(),
    )
    .await
}

/// Backward-compatible one-level queue preview (no recursive grandchildren).
pub async fn resolve_messages_expanding(
    client: &Client,
    input: &str,
    max_pages: u16,
    max_children: u8,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    resolve_messages_traversing(client, input, max_pages, max_children, 1).await
}

/// Convert typed URL events into the existing native file download
/// contract. Deferred Queue events and Directory scope markers must NEVER
/// become downloads. This adapter intentionally discards metadata that
/// MediaItem cannot yet represent and is not full gallery-dl job parity.
pub fn media_from_messages(
    events: Vec<ExtractorMessage>,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let mut media = Vec::new();
    for event in events {
        let ExtractorMessage::Url { url, metadata } = event else {
            continue;
        };
        let parsed = url::Url::parse(&url)?;
        if !matches!(parsed.scheme(), "http" | "https")
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
        {
            return Err("job URL contains an unsupported origin or credentials".into());
        }
        let filename = metadata
            .get("filename")
            .and_then(serde_json::Value::as_str)
            .ok_or("job URL missing a filename")?;
        if filename.is_empty()
            || filename == "."
            || filename == ".."
            || filename.len() > 255
            || crate::safe_filename(filename) != filename
        {
            return Err("job URL contains an unsafe filename".into());
        }
        let name = metadata
            .get("extractor")
            .and_then(serde_json::Value::as_str)
            .ok_or("job URL missing extractor provenance")?;
        let extractor = PREVIEW_EXTRACTORS
            .iter()
            .find(|(category, _)| *category == name)
            .map(|(category, _)| *category)
            .ok_or("job URL uses an unrecognized extractor category")?;
        let post_id = match metadata.get("post_id") {
            Some(value) => Some(
                value
                    .as_u64()
                    .ok_or("job post ID must be an unsigned integer")?,
            ),
            None => None,
        };
        media.push(MediaItem {
            url,
            filename: filename.to_owned(),
            extractor,
            post_id,
        });
    }
    Ok(media)
}

/// Opt-in bounded message-driven file resolution. The normal CLI still uses
/// the existing independent flattened-item resolver unless selected.
pub async fn resolve_media_job(
    client: &Client,
    input: &str,
    max_pages: u16,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    resolve_media_job_selected(client, input, max_pages, &JobSelection::default()).await
}

/// Apply file/child selection *during* the message job, before queue HTTP
/// requests and before any URL event is projected into the downloader.
pub async fn resolve_media_job_selected(
    client: &Client,
    input: &str,
    max_pages: u16,
    selection: &JobSelection,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let messages =
        resolve_messages_traversing_selected(client, input, max_pages, 8, 2, selection).await?;
    media_from_messages(messages)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preview_registry_matches_routed_extractors() {
        let fixtures = [
            ("https://example.org/file.jpg", "directlink"),
            ("https://are.na/alice/cassette-futurism", "arena"),
            ("https://boards.4chan.org/g/thread/123", "4chan"),
            ("https://2ch.org/a/res/12345.html", "2ch"),
            ("https://2ch.org/a/", "2ch"),
            ("https://sturdychan.help/a/12345", "2chen"),
            ("https://sturdychan.help/a/catalog", "2chen"),
            ("https://dec.2chan.net/12/res/12345.htm", "2chan"),
            ("https://4archive.org/board/a/thread/12345", "4archive"),
            ("https://4archive.org/board/a/", "4archive"),
            ("https://35photo.pro/photo_12345/", "35photo"),
            ("https://35photo.pro/tags/sunset/", "35photo"),
            (
                "https://4chanarchives.com/board/a/thread/12345",
                "4chanarchives",
            ),
            ("https://4chanarchives.com/board/a/", "4chanarchives"),
            (
                "https://comics.8muses.com/comics/album/Artist/Album",
                "8muses",
            ),
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
            ("https://imgchest.com/p/abc123xyz45", "imagechest"),
            (
                "https://silverpic.net/a1b2c3d4e5f6/NAME.EXT.html",
                "silverpic",
            ),
            ("https://www.imgpv.com/a1b2c3d4e5f6/NAME.EXT.html", "imgpv"),
            ("https://fappic.com/abcde12345/NAME.EXT", "fappic"),
            ("https://www.imagevenue.com/ME13LS07", "imagevenue"),
            (
                "https://imgclick.net/4tbrre1oxew9/test-_-_.png.html",
                "imgclick",
            ),
            ("https://imgur.com/abcdefg", "imgur"),
            ("https://imhentai.xxx/gallery/12345/", "imhentai"),
            ("https://issuu.com/press/docs/fall-2026", "issuu"),
            ("https://www.listal.com/viewimage/12345678", "listal"),
            ("https://kabe-uchiroom.com/mypage/?id=12345", "kabeuchi"),
            ("https://bit.ly/abc_12", "urlshortener"),
            ("https://t.co/abc123", "urlshortener"),
            ("https://lensdump.com/i/Abc12", "lensdump"),
            ("https://lensdump.com/a/Abc12", "lensdump"),
            ("https://lensdump.com/author", "lensdump"),
            ("https://story.keenspot.com/comic/one/", "keenspot"),
            (
                "https://downloads.khinsider.com/game-soundtracks/album/game-ost",
                "khinsider",
            ),
            ("https://issuu.com/press", "issuu"),
            ("https://imhentai.xxx/tag/art/", "imhentai"),
            ("https://imagetwist.com/f1i2s4vhvbrq/test.png", "imagetwist"),
            (
                "https://imagetwist.com/p/gdldev/747223/digits",
                "imagetwist",
            ),
            ("https://pixhost.to/show/190/130327671_test-.png", "pixhost"),
            ("https://pixhost.to/gallery/jSMFq", "pixhost"),
            ("https://acidimg.cc/img-5acb6b9de4640.html", "acidimg"),
            ("https://vipr.im/kcd5jcuhgs3v.html", "vipr"),
            (
                "https://imgspice.com/q1taxkhxprrn/example.jpg.html",
                "imgspice",
            ),
            ("https://imx.to/i/1qdeva", "imxto"),
            ("https://imgadult.com/img-686b4f451af05.html", "imgadult"),
            ("https://imgdrive.net/img-61ac0caeabf35.html", "imgdrive"),
            ("https://imgtaxi.com/img-61c71cea940d6.html", "imgtaxi"),
            ("https://imgwallet.com/img-62fd3045ceec6.html", "imgwallet"),
            ("https://imx.to/g/ozdy", "imxto"),
            ("https://picstate.com/view/full/23416694_sfyue", "picstate"),
            ("https://imgchest.com/u/Public_User", "imagechest"),
            ("https://www.imagepond.net/i/abc123", "imagepond"),
            ("https://www.imagepond.net/a/gallery123", "imagepond"),
            ("https://imageshack.com/i/iq8ac04fj", "imageshack"),
            ("https://imageshack.com/a/OGtz/1", "imageshack"),
            ("https://imageshack.com/user/Ezekiel.Gonzalez", "imageshack"),
            ("https://postimg.cc/Wtn2b3hC", "postimg"),
            ("https://postimg.cc/gallery/wxpDLgX", "postimg"),
            (
                "https://www.turboimagehost.com/p/39078423/test--.png.html",
                "turboimagehost",
            ),
            (
                "https://www.turboimagehost.com/album/344597/testimagegallery",
                "turboimagehost",
            ),
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
            ("https://wallhaven.cc/user/example-1/favorites", "wallhaven"),
            ("https://wallhaven.cc/user/example-1", "wallhaven"),
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
            ("http://behoimi.org/post/show/44", "3dbooru"),
            ("https://500px.com/photo/12345/Golden-Landscape", "500px"),
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

    #[test]
    fn typed_message_adapter_only_downloads_url_events() {
        let item = MediaItem {
            url: "https://example.org/file.jpg".into(),
            filename: "file.jpg".into(),
            extractor: "directlink",
            post_id: Some(42),
        };
        let mut events = from_media(vec![item.clone()]);
        events.insert(
            1,
            ExtractorMessage::Queue {
                url: "https://example.org/child.jpg".into(),
                metadata: serde_json::Map::new(),
            },
        );
        assert_eq!(media_from_messages(events).unwrap(), vec![item]);
    }

    #[test]
    fn message_download_adapter_rejects_untrusted_media_inputs() {
        use serde_json::json;
        let cases = [
            (
                "https://example.org/image.jpg",
                json!({"extractor":"directlink","filename":"../image.jpg"}),
            ),
            (
                "file:///tmp/image.jpg",
                json!({"extractor":"directlink","filename":"image.jpg"}),
            ),
            (
                "https://user:secret@example.org/image.jpg",
                json!({"extractor":"directlink","filename":"image.jpg"}),
            ),
            (
                "https://example.org/image.jpg",
                json!({"extractor":"unknown","filename":"image.jpg"}),
            ),
            (
                "https://example.org/image.jpg",
                json!({"extractor":"directlink"}),
            ),
            (
                "https://example.org/image.jpg",
                json!({"extractor":"directlink","filename":"image.jpg","post_id":"invalid"}),
            ),
        ];
        for (url, metadata) in cases {
            let event = ExtractorMessage::Url {
                url: url.into(),
                metadata: serde_json::from_value(metadata).unwrap(),
            };
            assert!(media_from_messages(vec![event]).is_err(), "{url}");
        }
    }

    #[test]
    fn typed_message_adapter_keeps_valid_child_provenance_and_filename() {
        let events = vec![ExtractorMessage::Url {
            url: "https://w.wallhaven.cc/full/94/wallhaven-94x38z.jpg".into(),
            metadata: serde_json::from_value(serde_json::json!({
                "category":"wallhaven",
                "extractor":"wallhaven",
                "filename":"wallhaven-94x38z.jpg",
                "collection_id":"12345"
            }))
            .unwrap(),
        }];
        let files = media_from_messages(events).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].extractor, "wallhaven");
        assert_eq!(files[0].filename, "wallhaven-94x38z.jpg");
    }

    #[test]
    fn queued_child_origins_must_have_safe_http_schemes_and_no_credentials() {
        for accepted in [
            "https://wallhaven.cc/w/94x38z",
            "https://commons.wikimedia.org/wiki/File:Blue_sky.png",
        ] {
            assert!(safe_queue_child_url(accepted), "{accepted}");
        }
        for rejected in [
            "javascript:alert(1)",
            "file:///tmp/file.jpg",
            "ftp://example.org/file.jpg",
            "https://user:secret@example.org/file.jpg",
            "https://user@example.org/file.jpg",
            "relative/gallery/2",
            "https://example.org/file.jpg",
            "http://127.0.0.1:1234/file.jpg",
            "http://[::1]/private.jpg",
            "https://localhost/private.jpg",
            "https://service.local/private.jpg",
            "https://internal.internal/private.jpg",
            "https://wallhaven.cc.evil.invalid/w/94x38z",
        ] {
            assert!(!safe_queue_child_url(rejected), "{rejected}");
        }
    }
}
