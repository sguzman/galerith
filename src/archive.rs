//! Native SQLite archive; its schema is not yet gallery-dl compatible.
use crate::MediaItem;
use fs2::FileExt;
use rusqlite::{params, Connection, OpenFlags};
use sha2::{Digest, Sha256};
use std::{
    error::Error,
    fs, io,
    path::{Path, PathBuf},
    time::Duration,
};

pub struct Archive {
    connection: Connection,
    lock_directory: Option<PathBuf>,
    upstream_directlink_archive: bool,
    upstream_reference: Option<Connection>,
    upstream_reference_table: String,
    // None = pinned default category prefix; Some = explicitly configured
    // upstream prefix, including empty for configured archive-table.
    upstream_reference_prefix: Option<String>,
}

/// Never unlink a held advisory lock file: doing so can split its namespace.
pub struct ArchiveItemLock(std::fs::File);

impl Drop for ArchiveItemLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

/// Only these pinned source categories guarantee one original per image
/// ID, with the same {id} archive contract for posts/searches/galleries.
/// Hash external archive keys before persisting them in the native DB.
fn archive_key_digest(key: &str) -> String {
    Sha256::digest(key.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Canonical pinned numeric formats use their parsed positive integer ID;
/// leading-zero variants must not inflate imported history counts.
fn canonical_numeric_archive_suffix(suffix: &str) -> bool {
    suffix
        .parse::<u64>()
        .ok()
        .is_some_and(|id| id > 0 && suffix == id.to_string())
}

/// Recognize only default pinned archive templates. A configured archive
/// table defaults to an *empty* prefix in upstream gallery-dl, so bare
/// numeric image IDs cannot safely be assigned to any Philomena site.
fn import_category(entry: &str, prefix: Option<&str>) -> Option<(&'static str, String)> {
    if prefix.is_none() {
        if let Some(path) = entry.strip_prefix("directlink") {
            if valid_directlink_format(path) {
                return Some(("directlink", entry.to_owned()));
            }
        }
        for category in [
            "derpibooru",
            "ponybooru",
            "furbooru",
            "danbooru",
            "e621",
            "e926",
            "e6ai",
            "yandere",
            "konachan",
            "sakugabooru",
            "lolibooru",
            "gelbooru",
            "rule34",
            "safebooru",
            "tbib",
            "hypnohub",
            "xbooru",
            "wallhaven",
            "4chan",
            "imgur",
            "500px",
            "photovogue",
            "fapachi",
            "fapello",
            "lightroom",
            "foriio",
            "piczel",
            "35photo",
            "unsplash",
            "lexica",
            "pixeldrain",
            "listal",
            "lensdump",
        ] {
            if let Some(id) = entry.strip_prefix(category) {
                if category == "wallhaven" && canonical_wallhaven_archive_suffix(id) {
                    return Some((category, entry.to_owned()));
                }
                if category == "4chan" && canonical_fourchan_archive_suffix(id) {
                    return Some((category, entry.to_owned()));
                }
                if category == "lensdump" && canonical_lensdump_image_id(id) {
                    return Some((category, entry.to_owned()));
                }
                if category == "listal" && canonical_listal_archive_suffix(id) {
                    return Some((category, entry.to_owned()));
                }
                if category == "pixeldrain" && canonical_pixeldrain_file_id(id) {
                    return Some((category, entry.to_owned()));
                }
                if category == "lexica" && canonical_lexica_image_id(id) {
                    return Some((category, entry.to_owned()));
                }
                if category == "unsplash" && canonical_unsplash_photo_id(id) {
                    return Some((category, entry.to_owned()));
                }
                if category == "35photo" && canonical_thirtyfivephoto_archive_suffix(id) {
                    return Some((category, entry.to_owned()));
                }
                if category == "piczel" && canonical_piczel_archive_suffix(id) {
                    return Some((category, entry.to_owned()));
                }
                if category == "foriio" && canonical_foriio_archive_suffix(id) {
                    return Some((category, entry.to_owned()));
                }
                if category == "lightroom" && canonical_lightroom_asset_id(id) {
                    return Some((category, entry.to_owned()));
                }
                if category == "fapello" && canonical_fapello_archive_suffix(id) {
                    return Some((category, entry.to_owned()));
                }
                if category == "fapachi" && canonical_fapachi_archive_suffix(id) {
                    return Some((category, entry.to_owned()));
                }
                if category == "photovogue" && canonical_photovogue_archive_suffix(id) {
                    return Some((category, entry.to_owned()));
                }
                if category == "500px" && canonical_500px_archive_suffix(id) {
                    return Some((category, entry.to_owned()));
                }
                if category == "imgur" && canonical_imgur_archive_suffix(id) {
                    return Some((category, entry.to_owned()));
                }
                if !matches!(
                    category,
                    "wallhaven"
                        | "4chan"
                        | "imgur"
                        | "500px"
                        | "photovogue"
                        | "fapachi"
                        | "fapello"
                        | "lightroom"
                        | "foriio"
                        | "piczel"
                        | "35photo"
                        | "unsplash"
                        | "lexica"
                        | "pixeldrain"
                        | "listal"
                        | "lensdump"
                ) && canonical_numeric_archive_suffix(id)
                {
                    return Some((category, entry.to_owned()));
                }
                if (is_booru_pool_category(category) || is_moebooru_category(category))
                    && canonical_pool_archive_suffix(id)
                {
                    return Some((category, entry.to_owned()));
                }
                if category == "danbooru" && canonical_favorite_group_archive_suffix(id) {
                    return Some((category, entry.to_owned()));
                }
                if is_e621_favorite_category(category) && canonical_favorite_archive_suffix(id) {
                    return Some((category, entry.to_owned()));
                }
                if (is_booru_pool_category(category)
                    || is_moebooru_category(category)
                    || is_gelbooru_category(category))
                    && canonical_tag_archive_suffix(id)
                {
                    return Some((category, entry.to_owned()));
                }
                if category == "danbooru" && canonical_media_asset_archive_suffix(id) {
                    return Some((category, entry.to_owned()));
                }
                if is_booru_pool_category(category) && canonical_popular_archive_suffix(id) {
                    return Some((category, entry.to_owned()));
                }
            }
        }
        return None;
    }

    let prefix = prefix?;
    let bare = entry.strip_prefix(prefix)?;
    // An empty named-table prefix can be a directlink entry, as can the
    // explicit directlink category prefix. Other source prefixes do not
    // prove directlink provenance even when their payload resembles a URL.
    if (prefix.is_empty() || prefix == "directlink") && valid_directlink_format(bare) {
        return Some(("directlink", format!("directlink{bare}")));
    }
    // A numeric ID is only unambiguous if the supplied source prefix itself
    // names the exact pinned category. Other custom prefixes might combine
    // several boorus with overlapping numeric IDs; skip rather than guess.
    for category in [
        "derpibooru",
        "ponybooru",
        "furbooru",
        "danbooru",
        "e621",
        "e926",
        "e6ai",
        "yandere",
        "konachan",
        "sakugabooru",
        "lolibooru",
        "gelbooru",
        "rule34",
        "safebooru",
        "tbib",
        "hypnohub",
        "xbooru",
        "wallhaven",
        "4chan",
        "imgur",
        "500px",
        "photovogue",
        "fapachi",
        "fapello",
        "lightroom",
        "foriio",
        "piczel",
        "35photo",
        "unsplash",
        "lexica",
        "pixeldrain",
        "listal",
        "lensdump",
    ] {
        if prefix == category
            && ((category == "lensdump" && canonical_lensdump_image_id(bare))
                || (category == "listal" && canonical_listal_archive_suffix(bare))
                || (category == "pixeldrain" && canonical_pixeldrain_file_id(bare))
                || (category == "lexica" && canonical_lexica_image_id(bare))
                || (category == "unsplash" && canonical_unsplash_photo_id(bare))
                || (category == "35photo" && canonical_thirtyfivephoto_archive_suffix(bare))
                || (category == "piczel" && canonical_piczel_archive_suffix(bare))
                || (category == "foriio" && canonical_foriio_archive_suffix(bare))
                || (category == "lightroom" && canonical_lightroom_asset_id(bare))
                || (category == "fapello" && canonical_fapello_archive_suffix(bare))
                || (category == "fapachi" && canonical_fapachi_archive_suffix(bare))
                || (category == "photovogue" && canonical_photovogue_archive_suffix(bare))
                || (category == "500px" && canonical_500px_archive_suffix(bare))
                || (category == "imgur" && canonical_imgur_archive_suffix(bare))
                || (category == "4chan" && canonical_fourchan_archive_suffix(bare))
                || (category == "wallhaven" && canonical_wallhaven_archive_suffix(bare))
                || (!matches!(
                    category,
                    "wallhaven"
                        | "4chan"
                        | "imgur"
                        | "500px"
                        | "photovogue"
                        | "fapachi"
                        | "fapello"
                        | "lightroom"
                        | "foriio"
                        | "piczel"
                        | "35photo"
                        | "unsplash"
                        | "lexica"
                        | "pixeldrain"
                        | "listal"
                        | "lensdump"
                ) && canonical_numeric_archive_suffix(bare))
                || ((is_booru_pool_category(category) || is_moebooru_category(category))
                    && canonical_pool_archive_suffix(bare))
                || (category == "danbooru" && canonical_favorite_group_archive_suffix(bare))
                || (is_e621_favorite_category(category) && canonical_favorite_archive_suffix(bare))
                || ((is_booru_pool_category(category)
                    || is_moebooru_category(category)
                    || is_gelbooru_category(category))
                    && canonical_tag_archive_suffix(bare))
                || (category == "danbooru" && canonical_media_asset_archive_suffix(bare))
                || (is_booru_pool_category(category) && canonical_popular_archive_suffix(bare)))
        {
            return Some((category, format!("{category}{bare}")));
        }
    }
    None
}

fn validated_calendar_date(date: &str) -> Option<(u32, u32, u32)> {
    let bytes = date.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit())
    {
        return None;
    }
    let year = date[..4].parse::<u32>().ok()?;
    let month = date[5..7].parse::<u32>().ok()?;
    let day = date[8..].parse::<u32>().ok()?;
    if year == 0 || !(1..=12).contains(&month) {
        return None;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    (day > 0 && day <= days).then_some((year, month, day))
}

/// The pinned upstream weekly popular extractor normalizes dates to
/// Monday. Sakamoto's proleptic Gregorian weekday (Sunday=0) avoids a
/// locale/timezone dependency and works across leap/month/year boundaries.
fn popular_week_monday(date: &str) -> Option<String> {
    let (mut year, mut month, day) = validated_calendar_date(date)?;
    let previous_year = i64::from(year) - i64::from(month < 3);
    let offsets = [0_i64, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let sunday_weekday = (previous_year + previous_year / 4 - previous_year / 100
        + previous_year / 400
        + offsets[(month - 1) as usize]
        + i64::from(day))
        % 7;
    let days_from_monday = (sunday_weekday + 6) % 7;
    let mut monday_day = i64::from(day) - days_from_monday;
    if monday_day <= 0 {
        if month == 1 {
            year = year.checked_sub(1)?;
            month = 12;
        } else {
            month -= 1;
        }
        let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
        let month_days = match month {
            2 if leap => 29,
            2 => 28,
            4 | 6 | 9 | 11 => 30,
            _ => 31,
        };
        monday_day += month_days;
    }
    Some(format!("{year:04}-{month:02}-{monday_day:02}"))
}

fn canonical_popular_archive_suffix(suffix: &str) -> bool {
    let Some((scale_date, id)) = suffix
        .strip_prefix("P_")
        .and_then(|value| value.rsplit_once('_'))
    else {
        return false;
    };
    let Some((scale, date)) = scale_date.split_once('_') else {
        return false;
    };
    let Ok(post) = id.parse::<u64>() else {
        return false;
    };
    if post == 0 {
        return false;
    }
    let valid_date = match scale {
        "d" => validated_calendar_date(date).is_some(),
        "w" => popular_week_monday(date).as_deref() == Some(date),
        "m" => {
            let mut candidate = date.to_owned();
            candidate.push_str("-01");
            validated_calendar_date(&candidate).is_some()
        }
        _ => false,
    };
    valid_date && suffix == format!("P_{scale}_{date}_{post}")
}

fn canonical_media_asset_archive_suffix(suffix: &str) -> bool {
    let Some(id) = suffix.strip_prefix('m') else {
        return false;
    };
    let Ok(asset) = id.parse::<u64>() else {
        return false;
    };
    asset > 0 && suffix == format!("m{asset}")
}

fn canonical_tag_archive_suffix(suffix: &str) -> bool {
    let Some((tags, id)) = suffix
        .strip_prefix("t_")
        .and_then(|rest| rest.rsplit_once('_'))
    else {
        return false;
    };
    if tags.len() > 1024 || tags.chars().any(char::is_control) {
        return false;
    }
    let Ok(post) = id.parse::<u64>() else {
        return false;
    };
    post > 0 && suffix == format!("t_{tags}_{post}")
}

fn is_e621_favorite_category(category: &str) -> bool {
    matches!(category, "e621" | "e926" | "e6ai")
}

fn canonical_favorite_archive_suffix(suffix: &str) -> bool {
    let Some((user, post)) = suffix
        .strip_prefix("f_")
        .and_then(|remaining| remaining.split_once('_'))
    else {
        return false;
    };
    let (Ok(user_id), Ok(post_id)) = (user.parse::<u64>(), post.parse::<u64>()) else {
        return false;
    };
    user_id > 0 && post_id > 0 && suffix == format!("f_{user_id}_{post_id}")
}

/// Pinned Moebooru categories with native post, pool and tag resolvers.
fn is_gelbooru_category(category: &str) -> bool {
    matches!(
        category,
        "gelbooru" | "rule34" | "safebooru" | "tbib" | "hypnohub" | "xbooru"
    )
}

fn is_moebooru_category(category: &str) -> bool {
    matches!(
        category,
        "yandere" | "konachan" | "sakugabooru" | "lolibooru"
    )
}

fn category_uses_source_scoped_ids(category: &str) -> bool {
    is_booru_pool_category(category)
        || is_moebooru_category(category)
        || is_gelbooru_category(category)
        || matches!(
            category,
            "wallhaven"
                | "4chan"
                | "imgur"
                | "500px"
                | "photovogue"
                | "fapachi"
                | "fapello"
                | "lightroom"
                | "foriio"
                | "piczel"
                | "35photo"
                | "unsplash"
                | "lexica"
                | "pixeldrain"
                | "listal"
                | "lensdump"
        )
}

fn is_booru_pool_category(category: &str) -> bool {
    matches!(category, "danbooru" | "e621" | "e926" | "e6ai")
}

/// The upstream pool template is p_{pool[id]}_{id}, with numeric IDs
/// sourced from the pool object and the post. Never parse tag/favorite rows.
fn canonical_pool_archive_suffix(suffix: &str) -> bool {
    let Some((pool, post)) = suffix
        .strip_prefix("p_")
        .and_then(|rest| rest.split_once('_'))
    else {
        return false;
    };
    let (Ok(pool_id), Ok(post_id)) = (pool.parse::<u64>(), post.parse::<u64>()) else {
        return false;
    };
    pool_id > 0 && post_id > 0 && suffix == format!("p_{pool_id}_{post_id}")
}

/// The pinned Danbooru favorite-group format is
/// fg_{favgroup[id]}_{id}, not a plain post or pool key.
fn canonical_favorite_group_archive_suffix(suffix: &str) -> bool {
    let Some((group, post)) = suffix
        .strip_prefix("fg_")
        .and_then(|value| value.split_once('_'))
    else {
        return false;
    };
    let (Ok(group_id), Ok(post_id)) = (group.parse::<u64>(), post.parse::<u64>()) else {
        return false;
    };
    group_id > 0 && post_id > 0 && suffix == format!("fg_{group_id}_{post_id}")
}

fn canonical_500px_archive_suffix(suffix: &str) -> bool {
    suffix.len() <= 18
        && suffix
            .parse::<u64>()
            .ok()
            .is_some_and(|id| id > 0 && suffix == id.to_string())
}

/// The pinned 500px archive_fmt is "{id}". Use its photo ID only when a
/// single-photo input and a trusted source-media host prove the same image.
fn fivehundredpx_archive_scope(item: &MediaItem, source_url: &str) -> Option<(String, String)> {
    if item.extractor != "500px" {
        return None;
    }
    let id = item.post_id.filter(|id| *id > 0)?;
    if !canonical_500px_archive_suffix(&id.to_string()) {
        return None;
    }
    let source = url::Url::parse(source_url).ok()?;
    if !matches!(source.scheme(), "http" | "https")
        || !matches!(source.host_str(), Some("500px.com" | "web.500px.com"))
        || source.port().is_some()
        || !source.username().is_empty()
        || source.password().is_some()
        || source.query().is_some()
        || source.fragment().is_some()
    {
        return None;
    }
    let parts: Vec<_> = source.path_segments()?.collect();
    let source_id = match parts.as_slice() {
        ["photo", source_id]
        | ["photo", source_id, ""]
        | ["photo", source_id, _]
        | ["photo", source_id, _, ""] => *source_id,
        _ => return None,
    };
    if !canonical_500px_archive_suffix(source_id) || source_id.parse::<u64>().ok()? != id {
        return None;
    }
    let media = url::Url::parse(&item.url).ok()?;
    let host = media.host_str()?;
    let approved = ["500px.com", "500px.org", "500pxcdn.com"]
        .iter()
        .any(|root| host == *root || host.ends_with(&format!(".{root}")));
    if media.scheme() != "https"
        || !approved
        || media.port().is_some()
        || !media.username().is_empty()
        || media.password().is_some()
    {
        return None;
    }
    Some((format!("image-id:{id}"), format!("500px{id}")))
}

fn canonical_photovogue_archive_suffix(suffix: &str) -> bool {
    suffix
        .parse::<u64>()
        .ok()
        .is_some_and(|id| id > 0 && suffix == id.to_string())
}

/// The pinned PhotoVogue photographer feed uses archive_fmt = "{id}" for
/// photos, without photographer in the key. Reject unverified or foreign
/// sources so a CDN URL can only inherit a proven photo's global ID.
fn photovogue_archive_scope(item: &MediaItem, source_url: &str) -> Option<(String, String)> {
    if item.extractor != "photovogue" {
        return None;
    }
    let id = item.post_id.filter(|id| *id > 0)?;
    let source = url::Url::parse(source_url).ok()?;
    if source.scheme() != "https"
        || !matches!(source.host_str(), Some("vogue.com" | "www.vogue.com"))
        || source.port().is_some()
        || !source.username().is_empty()
        || source.password().is_some()
        || source.query().is_some()
        || source.fragment().is_some()
    {
        return None;
    }
    let parts: Vec<_> = source.path_segments()?.collect();
    let ["photovogue", "photographers", photographer] = parts.as_slice() else {
        return None;
    };
    if photographer.is_empty()
        || photographer.len() > 18
        || !photographer.bytes().all(|byte| byte.is_ascii_digit())
        || photographer.parse::<u64>().ok()? == 0
    {
        return None;
    }
    let media = url::Url::parse(&item.url).ok()?;
    let host = media.host_str()?;
    if media.scheme() != "https"
        || !(host == "vogue.com" || host.ends_with(".vogue.com"))
        || media.port().is_some()
        || !media.username().is_empty()
        || media.password().is_some()
    {
        return None;
    }
    Some((format!("image-id:{id}"), format!("photovogue{id}")))
}

fn canonical_fapachi_archive_suffix(suffix: &str) -> bool {
    let Some((user, raw_id)) = suffix.rsplit_once('_') else {
        return false;
    };
    if !(1..=80).contains(&user.len())
        || user == "search"
        || !user
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        || raw_id.len() > 18
    {
        return false;
    }
    raw_id
        .parse::<u64>()
        .ok()
        .is_some_and(|id| id > 0 && suffix == format!("{user}_{id}"))
}

/// Pinned post archive format is {user}_{id}. The username is part of the
/// archive identity even for equal numeric post IDs or media URLs.
fn fapachi_archive_scope(item: &MediaItem, source_url: &str) -> Option<(String, String)> {
    if item.extractor != "fapachi" {
        return None;
    }
    let id = item.post_id.filter(|id| *id > 0)?;
    let source = url::Url::parse(source_url).ok()?;
    if !matches!(source.scheme(), "http" | "https")
        || !matches!(source.host_str(), Some("fapachi.com" | "www.fapachi.com"))
        || source.port().is_some()
        || !source.username().is_empty()
        || source.password().is_some()
        || source.query().is_some()
        || source.fragment().is_some()
    {
        return None;
    }
    let parts: Vec<_> = source.path_segments()?.collect();
    let (user, source_id) = match parts.as_slice() {
        [user, "media", source_id] | [user, "media", source_id, ""] => (*user, *source_id),
        _ => return None,
    };
    let suffix = format!("{user}_{source_id}");
    if !canonical_fapachi_archive_suffix(&suffix) || source_id.parse::<u64>().ok()? != id {
        return None;
    }
    let media = url::Url::parse(&item.url).ok()?;
    if media.scheme() != "https"
        || media.host_str() != Some("fapachi.com")
        || media.port().is_some()
        || !media.username().is_empty()
        || media.password().is_some()
        || media.query().is_some()
        || media.fragment().is_some()
    {
        return None;
    }
    let normalized = format!("fapachi{suffix}");
    Some((
        format!("post-sha256:{}", archive_key_digest(&normalized)),
        normalized,
    ))
}

/// The pinned Lightroom gallery extractor's archive_fmt is "{id}".
/// The native Lightroom extractor derives an indexed filename from the asset
/// ID returned by Adobe's image-list API; only this canonical shape proves
/// which asset owns a media rendition.
fn canonical_lightroom_asset_id(id: &str) -> bool {
    (1..=128).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn lightroom_archive_scope(item: &MediaItem, source_url: &str) -> Option<(String, String)> {
    if item.extractor != "lightroom" {
        return None;
    }
    let source = url::Url::parse(source_url).ok()?;
    if source.scheme() != "https"
        || source.host_str() != Some("lightroom.adobe.com")
        || source.port().is_some()
        || !source.username().is_empty()
        || source.password().is_some()
        || source.query().is_some()
        || source.fragment().is_some()
    {
        return None;
    }
    let parts: Vec<_> = source.path_segments()?.collect();
    let album = match parts.as_slice() {
        ["shares", id] => *id,
        _ => return None,
    };
    if !(1..=128).contains(&album.len())
        || !album
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return None;
    }
    let (ordinal, asset_extension) = item.filename.split_once('_')?;
    if ordinal.len() != 4
        || !ordinal.bytes().all(|byte| byte.is_ascii_digit())
        || ordinal.parse::<u16>().ok()? == 0
    {
        return None;
    }
    let (id, ext) = asset_extension.rsplit_once('.')?;
    if !canonical_lightroom_asset_id(id)
        || ext.is_empty()
        || ext.len() > 12
        || !ext.bytes().all(|byte| byte.is_ascii_alphanumeric())
    {
        return None;
    }
    let media = url::Url::parse(&item.url).ok()?;
    let host = media.host_str()?;
    if media.scheme() != "https"
        || !["adobe.com", "adobe.io", "adobe.net", "adobecdn.com"]
            .iter()
            .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
        || media.port().is_some()
        || !media.username().is_empty()
        || media.password().is_some()
    {
        return None;
    }
    let media_extension = media.path_segments()?.next_back()?.rsplit_once('.')?.1;
    if media_extension != ext {
        return None;
    }
    let upstream = format!("lightroom{id}");
    Some((
        format!("asset-sha256:{}", archive_key_digest(&upstream)),
        upstream,
    ))
}

fn canonical_fapello_archive_suffix(suffix: &str) -> bool {
    let Some((kind, rest)) = suffix.split_once('_') else {
        return false;
    };
    if !matches!(kind, "photo" | "video") {
        return false;
    }
    let Some((model, raw_id)) = rest.rsplit_once('_') else {
        return false;
    };
    if !(1..=80).contains(&model.len())
        || matches!(
            model,
            "search" | "popular_videos" | "top-likes" | "top-followers" | "videos" | "trending"
        )
        || !model
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        || raw_id.len() > 18
    {
        return false;
    }
    raw_id
        .parse::<u64>()
        .ok()
        .is_some_and(|id| id > 0 && suffix == format!("{kind}_{model}_{id}"))
}

/// Pinned archive_fmt = "{type}_{model}_{id}". The single-media native
/// preview only has enough provenance for known original media extensions.
fn fapello_archive_scope(item: &MediaItem, source_url: &str) -> Option<(String, String)> {
    if item.extractor != "fapello" {
        return None;
    }
    let id = item.post_id.filter(|value| *value > 0)?;
    let source = url::Url::parse(source_url).ok()?;
    if source.scheme() != "https"
        || !matches!(
            source.host_str(),
            Some("fapello.com" | "www.fapello.com" | "fapello.su" | "www.fapello.su")
        )
        || source.port().is_some()
        || !source.username().is_empty()
        || source.password().is_some()
        || source.query().is_some()
        || source.fragment().is_some()
    {
        return None;
    }
    let path: Vec<_> = source.path_segments()?.collect();
    let (model, source_id) = match path.as_slice() {
        [model, source_id] | [model, source_id, ""] => (*model, *source_id),
        _ => return None,
    };
    if source_id != id.to_string() {
        return None;
    }

    let media = url::Url::parse(&item.url).ok()?;
    let host = media.host_str()?;
    if media.scheme() != "https"
        || !(matches!(host, "fapello.com" | "fapello.su")
            || host.ends_with(".fapello.com")
            || host.ends_with(".fapello.su"))
        || media.port().is_some()
        || !media.username().is_empty()
        || media.password().is_some()
        || media.query().is_some()
        || media.fragment().is_some()
        || media.path().contains('%')
        || media.path().contains("..")
    {
        return None;
    }
    let extension = media
        .path_segments()?
        .next_back()?
        .rsplit_once('.')?
        .1
        .to_ascii_lowercase();
    let kind = match extension.as_str() {
        "mp4" | "webm" => "video",
        "jpg" | "jpeg" | "png" | "gif" | "webp" => "photo",
        _ => return None,
    };
    let suffix = format!("{kind}_{model}_{id}");
    if !canonical_fapello_archive_suffix(&suffix) {
        return None;
    }
    let key = format!("fapello{suffix}");
    Some((format!("post-sha256:{}", archive_key_digest(&key)), key))
}

/// The pinned Foriio work archive template is "{work_id}/{file_id}/{num}".
/// All three parts are required: a work can contain multiple originals, and
/// the same file ID can recur at another position.
/// Pinned 35photo image/user archive key: "{id}_{num}".
/// Tag and genre extractors use DIFFERENT archives, so only an exact public
/// individual-photo source URL proves this unprefixed image identity.
/// All pinned public Unsplash extractors archive by the API photo "{id}".
/// The native photo resolver already emits this ID as "id.jpg" for every
/// family, including favorites, collections and searches.
/// Pinned Lexica search archive_fmt = "{id}". Unlike arbitrary
/// search-result files, the native Lexica media URL itself embeds the API ID.
/// Pinned Pixeldrain file and album extractors archive by each file's
/// source ID, not its filename or containing list. Filesystem/folder mode
/// uses an unrelated path+position archive template and fails closed here.
/// Pinned Listal archive_fmt is "{id}/{filename}", where filename is
/// the original media basename without its extension. Same image IDs can
/// therefore have distinct originals; raw post ID alone is insufficient.
/// The pinned Lensdump image extractor uses "{id}", but gallery mode
/// inherits "{gallery_id}_{num}" from GalleryExtractor. Reject underscores
/// in imported image IDs so an album-position key cannot masquerade as a
/// completed individual image.
fn canonical_lensdump_image_id(id: &str) -> bool {
    (1..=80).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric())
}

fn lensdump_archive_scope(item: &MediaItem, source_url: &str) -> Option<(String, String)> {
    if item.extractor != "lensdump" {
        return None;
    }
    let source = url::Url::parse(source_url).ok()?;
    if source.scheme() != "https"
        || source.host_str() != Some("lensdump.com")
        || source.port().is_some()
        || !source.username().is_empty()
        || source.password().is_some()
        || source.query().is_some()
        || source.fragment().is_some()
    {
        return None;
    }
    let source_parts: Vec<_> = source.path_segments()?.collect();
    let id = match source_parts.as_slice() {
        ["i", id] | ["i", id, ""] => *id,
        _ => return None,
    };
    if !canonical_lensdump_image_id(id) {
        return None;
    }
    let original_name = item.filename.strip_prefix(&format!("lensdump_{id}_"))?;
    if !(1..=180).contains(&original_name.len())
        || original_name.contains("..")
        || !original_name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return None;
    }
    let media = url::Url::parse(&item.url).ok()?;
    let host = media.host_str()?;
    if media.scheme() != "https"
        || !(matches!(host, "lensdump.com" | "l3n.co")
            || host.ends_with(".lensdump.com")
            || host.ends_with(".l3n.co"))
        || media.port().is_some()
        || !media.username().is_empty()
        || media.password().is_some()
        || media.query().is_some()
        || media.fragment().is_some()
        || media.path().contains('%')
        || media.path_segments()?.next_back()? != original_name
    {
        return None;
    }
    let ext = original_name.rsplit_once('.')?.1;
    if !matches!(ext, "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif") {
        return None;
    }
    let upstream = format!("lensdump{id}");
    Some((
        format!("image-sha256:{}", archive_key_digest(&upstream)),
        upstream,
    ))
}

fn canonical_listal_archive_suffix(suffix: &str) -> bool {
    let Some((id, name)) = suffix.split_once('/') else {
        return false;
    };
    canonical_numeric_archive_suffix(id)
        && id.len() <= 16
        && (1..=180).contains(&name.len())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        && !name.contains("..")
}

fn listal_archive_scope(item: &MediaItem, source_url: &str) -> Option<(String, String)> {
    if item.extractor != "listal" {
        return None;
    }
    let source = url::Url::parse(source_url).ok()?;
    if source.scheme() != "https"
        || !matches!(source.host_str(), Some("listal.com" | "www.listal.com"))
        || source.port().is_some()
        || !source.username().is_empty()
        || source.password().is_some()
        || source.query().is_some()
        || source.fragment().is_some()
    {
        return None;
    }
    let source_parts: Vec<_> = source.path_segments()?.collect();
    let id = match source_parts.as_slice() {
        ["viewimage", id] | ["viewimage", id, ""] => *id,
        _ => return None,
    };
    if !canonical_numeric_archive_suffix(id) || id.len() > 16 {
        return None;
    }
    let prefix = format!("{id}_");
    let name = item.filename.strip_prefix(&prefix)?;
    let media = url::Url::parse(&item.url).ok()?;
    let host = media.host_str()?;
    if media.scheme() != "https"
        || !(host == "listal.com"
            || host == "lisimg.com"
            || host.ends_with(".listal.com")
            || host.ends_with(".lisimg.com"))
        || media.port().is_some()
        || !media.username().is_empty()
        || media.password().is_some()
        || media.query().is_some()
        || media.fragment().is_some()
        || media.path().contains('%')
    {
        return None;
    }
    if name != media.path_segments()?.next_back()? {
        return None;
    }
    let (stem, extension) = name.rsplit_once('.')?;
    if !matches!(extension, "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif") {
        return None;
    }
    let suffix = format!("{id}/{stem}");
    if !canonical_listal_archive_suffix(&suffix) {
        return None;
    }
    let upstream = format!("listal{suffix}");
    Some((
        format!("original-sha256:{}", archive_key_digest(&upstream)),
        upstream,
    ))
}

fn canonical_pixeldrain_file_id(id: &str) -> bool {
    (1..=128).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn pixeldrain_archive_scope(item: &MediaItem, source_url: &str) -> Option<(String, String)> {
    if item.extractor != "pixeldrain" {
        return None;
    }
    let source = url::Url::parse(source_url).ok()?;
    if source.scheme() != "https"
        || source.host_str() != Some("pixeldrain.com")
        || source.port().is_some()
        || !source.username().is_empty()
        || source.password().is_some()
        || source.query().is_some()
    {
        return None;
    }
    let parts: Vec<_> = source.path_segments()?.collect();
    let direct_id = match parts.as_slice() {
        ["u", id] | ["api", "file", id] if canonical_pixeldrain_file_id(id) => Some(*id),
        _ => None,
    };
    let album = match parts.as_slice() {
        ["l", id] | ["api", "list", id] if canonical_pixeldrain_file_id(id) => Some(*id),
        _ => None,
    };
    if direct_id.is_none() && album.is_none() {
        return None;
    }
    if let Some(fragment) = source.fragment() {
        if album.is_none()
            || !fragment
                .strip_prefix("item=")
                .and_then(|value| value.parse::<usize>().ok())
                .is_some_and(|value| value < 2000)
        {
            return None;
        }
    }
    let media = url::Url::parse(&item.url).ok()?;
    if media.scheme() != "https"
        || media.host_str() != Some("pixeldrain.com")
        || media.port().is_some()
        || !media.username().is_empty()
        || media.password().is_some()
        || media.query() != Some("download")
        || media.fragment().is_some()
    {
        return None;
    }
    let media_parts: Vec<_> = media.path_segments()?.collect();
    let ["api", "file", id] = media_parts.as_slice() else {
        return None;
    };
    if !canonical_pixeldrain_file_id(id) || direct_id.is_some_and(|direct| direct != *id) {
        return None;
    }
    let upstream = format!("pixeldrain{id}");
    Some((
        format!("file-sha256:{}", archive_key_digest(&upstream)),
        upstream,
    ))
}

fn canonical_lexica_image_id(id: &str) -> bool {
    (1..=128).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn lexica_archive_scope(item: &MediaItem, source_url: &str) -> Option<(String, String)> {
    if item.extractor != "lexica" {
        return None;
    }
    let id = item.filename.strip_suffix(".jpg")?;
    if !canonical_lexica_image_id(id) {
        return None;
    }
    let source = url::Url::parse(source_url).ok()?;
    if source.scheme() != "https"
        || source.host_str() != Some("lexica.art")
        || source.port().is_some()
        || !source.username().is_empty()
        || source.password().is_some()
        || source.fragment().is_some()
        || source.path() != "/"
    {
        return None;
    }
    let mut query = source.query_pairs();
    let (key, value) = query.next()?;
    if key != "q"
        || value.trim().is_empty()
        || value.len() > 240
        || value.chars().any(char::is_control)
        || query.next().is_some()
    {
        return None;
    }
    let media = url::Url::parse(&item.url).ok()?;
    if media.scheme() != "https"
        || media.host_str() != Some("lexica-serve-encoded-images2.sharif.workers.dev")
        || media.port().is_some()
        || !media.username().is_empty()
        || media.password().is_some()
        || media.query().is_some()
        || media.fragment().is_some()
    {
        return None;
    }
    let parts: Vec<_> = media.path_segments()?.collect();
    if !matches!(parts.as_slice(), ["full_jpg", media_id] if *media_id == id) {
        return None;
    }
    let upstream = format!("lexica{id}");
    Some((
        format!("image-sha256:{}", archive_key_digest(&upstream)),
        upstream,
    ))
}

fn canonical_unsplash_photo_id(id: &str) -> bool {
    (1..=128).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn unsplash_archive_scope(item: &MediaItem, source_url: &str) -> Option<(String, String)> {
    if item.extractor != "unsplash" {
        return None;
    }
    let id = item.filename.strip_suffix(".jpg")?;
    if !canonical_unsplash_photo_id(id) {
        return None;
    }
    let source = url::Url::parse(source_url).ok()?;
    if source.scheme() != "https"
        || source.host_str() != Some("unsplash.com")
        || source.port().is_some()
        || !source.username().is_empty()
        || source.password().is_some()
        || source.fragment().is_some()
        || source.path().contains('%')
    {
        return None;
    }
    let source_parts: Vec<_> = source.path_segments()?.collect();
    let is_photo = matches!(source_parts.as_slice(), ["photos", value] if *value == id);
    let is_user = matches!(source_parts.as_slice(), [user] if user
        .strip_prefix('@').is_some_and(canonical_unsplash_photo_id));
    let is_likes = matches!(source_parts.as_slice(), [user, "likes"] if user
        .strip_prefix('@').is_some_and(canonical_unsplash_photo_id));
    let is_collection = matches!(source_parts.as_slice(), ["collections", value] | ["collections", value, _] if canonical_unsplash_photo_id(value));
    let is_search = matches!(source_parts.as_slice(), ["s", "photos", value] if canonical_unsplash_photo_id(value));
    if !(is_photo || is_user || is_likes || is_collection || is_search) {
        return None;
    }
    if source.query().is_some() {
        if !is_search {
            return None;
        }
        let mut pairs = source.query_pairs();
        let (key, value) = pairs.next()?;
        if key != "orientation"
            || !matches!(value.as_ref(), "landscape" | "portrait" | "squarish")
            || pairs.next().is_some()
        {
            return None;
        }
    }

    let media = url::Url::parse(&item.url).ok()?;
    if media.scheme() != "https"
        || !matches!(
            media.host_str(),
            Some("images.unsplash.com" | "plus.unsplash.com")
        )
        || media.port().is_some()
        || !media.username().is_empty()
        || media.password().is_some()
        || media.fragment().is_some()
    {
        return None;
    }
    let upstream = format!("unsplash{id}");
    Some((
        format!("photo-sha256:{}", archive_key_digest(&upstream)),
        upstream,
    ))
}

fn canonical_thirtyfivephoto_archive_suffix(suffix: &str) -> bool {
    let Some((id, position)) = suffix.split_once('_') else {
        return false;
    };
    canonical_numeric_archive_suffix(id)
        && position
            .parse::<u8>()
            .ok()
            .is_some_and(|num| (1..=32).contains(&num) && position == num.to_string())
}

fn thirtyfivephoto_archive_scope(item: &MediaItem, source_url: &str) -> Option<(String, String)> {
    if item.extractor != "35photo" {
        return None;
    }
    let id = item.post_id.filter(|value| *value > 0)?;
    let source = url::Url::parse(source_url).ok()?;
    let host = source.host_str()?;
    let host_ok = host == "35photo.pro"
        || host.strip_suffix(".35photo.pro").is_some_and(|part| {
            (1..=24).contains(&part.len()) && part.bytes().all(|ch| ch.is_ascii_lowercase())
        });
    if source.scheme() != "https"
        || !host_ok
        || source.port().is_some()
        || !source.username().is_empty()
        || source.password().is_some()
        || source.query().is_some()
        || source.fragment().is_some()
    {
        return None;
    }
    let source_parts: Vec<_> = source.path_segments()?.collect();
    let route = match source_parts.as_slice() {
        [route] | [route, ""] => *route,
        _ => return None,
    };
    let photo_id = route.strip_prefix("photo_")?;
    if photo_id != id.to_string() {
        return None;
    }
    let (filename_id, rest) = item.filename.split_once('_')?;
    if filename_id != photo_id {
        return None;
    }
    let (padded, ext) = rest.rsplit_once('.')?;
    let num = padded.parse::<u8>().ok()?;
    if !(1..=32).contains(&num)
        || padded != format!("{num:02}")
        || !matches!(ext, "jpg" | "jpeg" | "png" | "webp" | "gif")
    {
        return None;
    }
    let media = url::Url::parse(&item.url).ok()?;
    let media_host = media.host_str()?;
    if media.scheme() != "https"
        || !(media_host == "35photo.pro" || media_host.ends_with(".35photo.pro"))
        || media.port().is_some()
        || !media.username().is_empty()
        || media.password().is_some()
        || media.query().is_some()
        || media.fragment().is_some()
        || media.path().contains('%')
        || media.path().contains("..")
        || media.path_segments()?.next_back()?.rsplit_once('.')?.1 != ext
    {
        return None;
    }
    let suffix = format!("{id}_{num}");
    if !canonical_thirtyfivephoto_archive_suffix(&suffix) {
        return None;
    }
    let upstream = format!("35photo{suffix}");
    Some((
        format!("photo-series-sha256:{}", archive_key_digest(&upstream)),
        upstream,
    ))
}

fn canonical_foriio_file_id(id: &str) -> bool {
    (1..=96).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn canonical_foriio_archive_suffix(suffix: &str) -> bool {
    let mut parts = suffix.split('/');
    let (Some(work), Some(file), Some(num), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    canonical_numeric_archive_suffix(work)
        && canonical_foriio_file_id(file)
        && num
            .parse::<u16>()
            .ok()
            .is_some_and(|n| (1..=128).contains(&n) && num == n.to_string())
}

fn foriio_archive_scope(item: &MediaItem, source_url: &str) -> Option<(String, String)> {
    if item.extractor != "foriio" {
        return None;
    }
    let work_id = item.post_id.filter(|id| *id > 0)?;
    let source = url::Url::parse(source_url).ok()?;
    if source.scheme() != "https"
        || !matches!(
            source.host_str(),
            Some("foriio.com" | "www.foriio.com" | "fori.io" | "www.fori.io")
        )
        || source.port().is_some()
        || !source.username().is_empty()
        || source.password().is_some()
        || source.query().is_some()
        || source.fragment().is_some()
    {
        return None;
    }
    let source_parts: Vec<_> = source.path_segments()?.collect();
    let source_work = match source_parts.as_slice() {
        ["works", id] | ["works", id, ""] => *id,
        _ => return None,
    };
    if source_work != work_id.to_string() {
        return None;
    }

    let (ordinal, filename) = item.filename.split_once(' ')?;
    let num = ordinal.parse::<u16>().ok()?;
    if !(1..=128).contains(&num) || ordinal != format!("{num:02}") {
        return None;
    }
    let (file_id, ext) = filename.rsplit_once('.')?;
    if !canonical_foriio_file_id(file_id)
        || !matches!(ext, "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif")
    {
        return None;
    }
    let media = url::Url::parse(&item.url).ok()?;
    if media.scheme() != "https"
        || media.host_str() != Some("foriio.imgix.net")
        || media.port().is_some()
        || !media.username().is_empty()
        || media.password().is_some()
        || media.query().is_some()
        || media.fragment().is_some()
    {
        return None;
    }
    let media_parts: Vec<_> = media.path_segments()?.collect();
    let ["store", media_file] = media_parts.as_slice() else {
        return None;
    };
    if media_file.rsplit_once('.')?.1 != ext {
        return None;
    }
    let suffix = format!("{work_id}/{file_id}/{num}");
    if !canonical_foriio_archive_suffix(&suffix) {
        return None;
    }
    let upstream = format!("foriio{suffix}");
    Some((
        format!("work-file-sha256:{}", archive_key_digest(&upstream)),
        upstream,
    ))
}

/// Pinned Piczel user/image archive_fmt = "{id}_{num}". Folder mode
/// instead uses "f{folder[id]}_{id}_{num}"; MediaItem lacks independently
/// verified folder metadata, so it must never be guessed here.
fn canonical_piczel_archive_suffix(suffix: &str) -> bool {
    let Some((post, num)) = suffix.rsplit_once('_') else {
        return false;
    };
    canonical_numeric_archive_suffix(post)
        && num
            .parse::<u8>()
            .ok()
            .is_some_and(|n| n <= 15 && num == n.to_string())
}

fn piczel_archive_scope(item: &MediaItem, source_url: &str) -> Option<(String, String)> {
    if item.extractor != "piczel" {
        return None;
    }
    let id = item.post_id.filter(|id| *id > 0)?;
    let source = url::Url::parse(source_url).ok()?;
    if source.scheme() != "https"
        || !matches!(source.host_str(), Some("piczel.tv" | "www.piczel.tv"))
        || source.port().is_some()
        || !source.username().is_empty()
        || source.password().is_some()
        || source.query().is_some()
        || source.fragment().is_some()
    {
        return None;
    }
    let parts: Vec<_> = source.path_segments()?.collect();
    match parts.as_slice() {
        ["gallery", "image", post] | ["gallery", "image", post, ""] if *post == id.to_string() => {}
        ["gallery", user] | ["gallery", user, ""]
            if (1..=96).contains(&user.len())
                && user
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_')) => {}
        // Folder keys need per-post folder[id], not merely the requested URL.
        _ => return None,
    }
    let prefix = format!("piczel_{id}_");
    let stem = item.filename.strip_prefix(&prefix)?;
    let (title, numbered_extension) = stem.rsplit_once('_')?;
    if title.is_empty() {
        return None;
    }
    let (number, ext) = numbered_extension.rsplit_once('.')?;
    let num = number.parse::<u8>().ok()?;
    if num > 15
        || number != format!("{num:02}")
        || !matches!(
            ext,
            "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif" | "mp4" | "webm"
        )
    {
        return None;
    }
    let media = url::Url::parse(&item.url).ok()?;
    let host = media.host_str()?;
    if media.scheme() != "https"
        || !(host == "piczel.tv" || host.ends_with(".piczel.tv"))
        || media.port().is_some()
        || !media.username().is_empty()
        || media.password().is_some()
        || media.query().is_some()
        || media.fragment().is_some()
        || media.path_segments()?.next_back()?.rsplit_once('.')?.1 != ext
    {
        return None;
    }
    let suffix = format!("{id}_{num}");
    if !canonical_piczel_archive_suffix(&suffix) {
        return None;
    }
    let upstream = format!("piczel{suffix}");
    Some((
        format!("post-sha256:{}", archive_key_digest(&upstream)),
        upstream,
    ))
}

fn valid_imgur_id(id: &str) -> bool {
    matches!(id.len(), 5 | 7) && id.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

fn canonical_imgur_archive_suffix(suffix: &str) -> bool {
    if valid_imgur_id(suffix) {
        return true;
    }
    suffix
        .split_once('_')
        .is_some_and(|(album, media)| valid_imgur_id(album) && valid_imgur_id(media))
}

/// Only a source-verified image or album has sufficient pinned archive
/// metadata; Imgur gallery dispatcher URLs are ambiguous until the API
/// identifies whether the result is a single image or an album.
fn imgur_archive_scope(item: &MediaItem, source_url: &str) -> Option<(String, String)> {
    if item.extractor != "imgur" {
        return None;
    }
    let media = url::Url::parse(&item.url).ok()?;
    if media.scheme() != "https"
        || media.host_str() != Some("i.imgur.com")
        || media.port().is_some()
        || !media.username().is_empty()
        || media.password().is_some()
        || media.query().is_some()
        || media.fragment().is_some()
    {
        return None;
    }
    let media_path: Vec<_> = media.path_segments()?.collect();
    let [filename] = media_path.as_slice() else {
        return None;
    };
    let (id, extension) = filename.rsplit_once('.')?;
    if !valid_imgur_id(id) || !matches!(extension, "jpg" | "jpeg" | "png" | "gif" | "webp" | "mp4")
    {
        return None;
    }
    let source = url::Url::parse(source_url).ok()?;
    if source.scheme() != "https"
        || !matches!(
            source.host_str(),
            Some("imgur.com" | "www.imgur.com" | "m.imgur.com" | "imgur.io" | "i.imgur.com")
        )
        || source.port().is_some()
        || !source.username().is_empty()
        || source.password().is_some()
        || source.query().is_some()
        || source.fragment().is_some()
    {
        return None;
    }
    let path: Vec<_> = source.path_segments()?.collect();
    match path.as_slice() {
        [source_id] if source_id == &id => Some((format!("image-id:{id}"), format!("imgur{id}"))),
        ["r", subreddit, source_id] if !subreddit.is_empty() && source_id == &id => {
            Some((format!("image-id:{id}"), format!("imgur{id}")))
        }
        [source_filename] if source.host_str() == Some("i.imgur.com") => {
            let (source_id, source_ext) = source_filename.rsplit_once('.')?;
            if source_id != id || source_ext != extension {
                return None;
            }
            Some((format!("image-id:{id}"), format!("imgur{id}")))
        }
        ["a", album] if valid_imgur_id(album) => Some((
            format!("album-id:{album}:media-id:{id}"),
            format!("imgur{album}_{id}"),
        )),
        _ => None,
    }
}

fn valid_fourchan_board(board: &str) -> bool {
    !board.is_empty()
        && board.len() <= 8
        && board
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

fn canonical_fourchan_archive_suffix(value: &str) -> bool {
    let mut fields = value.split('_');
    let (Some(board), Some(thread), Some(tim)) = (fields.next(), fields.next(), fields.next())
    else {
        return false;
    };
    if fields.next().is_some() || !valid_fourchan_board(board) {
        return false;
    }
    let (Ok(thread_id), Ok(media_tim)) = (thread.parse::<u64>(), tim.parse::<u64>()) else {
        return false;
    };
    thread_id > 0 && media_tim > 0 && value == format!("{board}_{thread_id}_{media_tim}")
}

/// Pinned gallery-dl 4chan archive template is {board}_{thread}_{tim}.
/// MediaItem.post_id is the CDN tim, not the 4chan post number.
fn fourchan_archive_scope(item: &MediaItem, source_url: &str) -> Option<(String, String)> {
    if item.extractor != "4chan" {
        return None;
    }
    let tim = item.post_id.filter(|value| *value > 0)?;
    let source = url::Url::parse(source_url).ok()?;
    if source.scheme() != "https"
        || !matches!(
            source.host_str(),
            Some("boards.4chan.org" | "boards.4channel.org")
        )
        || source.port().is_some()
        || !source.username().is_empty()
        || source.password().is_some()
        || source.query().is_some()
        || source.fragment().is_some()
    {
        return None;
    }
    let components: Vec<_> = source
        .path_segments()?
        .filter(|component| !component.is_empty())
        .collect();
    let (board, thread) = match components.as_slice() {
        [board, "thread", thread] => (*board, *thread),
        [board, "thread", thread, slug] if slug.len() <= 200 => (*board, *thread),
        _ => return None,
    };
    if !valid_fourchan_board(board) {
        return None;
    }
    let thread_id = thread.parse::<u64>().ok().filter(|id| *id > 0)?;
    if thread != thread_id.to_string() {
        return None;
    }
    let media = url::Url::parse(&item.url).ok()?;
    if media.scheme() != "https"
        || media.host_str() != Some("i.4cdn.org")
        || media.port().is_some()
        || !media.username().is_empty()
        || media.password().is_some()
        || media.query().is_some()
        || media.fragment().is_some()
    {
        return None;
    }
    let path: Vec<_> = media.path_segments()?.collect();
    let media_filename = match path.as_slice() {
        [media_board, filename] if *media_board == board => *filename,
        _ => return None,
    };
    let (stem, extension) = media_filename.rsplit_once('.')?;
    if extension.is_empty()
        || extension.len() > 10
        || !extension.bytes().all(|byte| byte.is_ascii_alphanumeric())
        || stem != tim.to_string()
    {
        return None;
    }
    Some((
        format!("thread-board:{board}:thread:{thread_id}:tim:{tim}"),
        format!("4chan{board}_{thread_id}_{tim}"),
    ))
}

fn valid_wallhaven_id(id: &str) -> bool {
    (6..=32).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

fn valid_wallhaven_user(user: &str) -> bool {
    (1..=64).contains(&user.len())
        && user
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn canonical_wallhaven_archive_suffix(suffix: &str) -> bool {
    if valid_wallhaven_id(suffix) {
        return true;
    }
    if let Some((query, id)) = suffix
        .strip_prefix("s_")
        .and_then(|rest| rest.rsplit_once('_'))
    {
        return !query.trim().is_empty()
            && query.len() <= 240
            && !query.chars().any(char::is_control)
            && valid_wallhaven_id(id);
    }
    if let Some((user, id)) = suffix
        .strip_prefix("u_")
        .and_then(|rest| rest.rsplit_once('_'))
    {
        return valid_wallhaven_user(user) && valid_wallhaven_id(id);
    }
    false
}

/// The existing Wallhaven parser validates CDN/ID coherence. Verify the
/// same origin and path again here before inferring IDs from MediaItem,
/// which intentionally has no string-valued post_id field.
fn wallhaven_wallpaper_id(item: &MediaItem) -> Option<String> {
    if item.extractor != "wallhaven" {
        return None;
    }
    let url = url::Url::parse(&item.url).ok()?;
    if url.scheme() != "https"
        || url.host_str() != Some("w.wallhaven.cc")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let segments: Vec<_> = url.path_segments()?.collect();
    let (group, filename) = match segments.as_slice() {
        ["full", group, filename] | [group, filename] => (*group, *filename),
        _ => return None,
    };
    if group.len() != 2 || !group.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return None;
    }
    let (id, ext) = filename.strip_prefix("wallhaven-")?.rsplit_once('.')?;
    if !valid_wallhaven_id(id) || !matches!(ext, "jpg" | "jpeg" | "png" | "webp") {
        return None;
    }
    Some(id.to_owned())
}

/// Pinned Wallhaven archive classes: image/collection={id},
/// search=s_{search[q]}_{id}, uploads=u_{username}_{id}.
fn wallhaven_archive_scope(item: &MediaItem, source_url: &str) -> Option<(String, String)> {
    let id = wallhaven_wallpaper_id(item)?;
    let source = url::Url::parse(source_url).ok()?;
    if source.scheme() != "https"
        || source.host_str() != Some("wallhaven.cc")
        || source.port().is_some()
        || !source.username().is_empty()
        || source.password().is_some()
        || source.fragment().is_some()
    {
        return None;
    }
    let path: Vec<_> = source.path_segments()?.collect();
    let key = match path.as_slice() {
        ["w", source_id] if source.query().is_none() && *source_id == id.as_str() => {
            format!("wallhaven{id}")
        }
        ["user", user, "favorites", collection]
            if source.query().is_none()
                && valid_wallhaven_user(user)
                && collection.parse::<u64>().ok().is_some_and(|x| x > 0) =>
        {
            format!("wallhaven{id}")
        }
        ["search"] | ["search", ""] => {
            let mut query = source.query_pairs();
            let (param, search) = query.next()?;
            if param != "q"
                || query.next().is_some()
                || search.trim().is_empty()
                || search.len() > 240
                || search.chars().any(char::is_control)
            {
                return None;
            }
            format!("wallhavens_{search}_{id}")
        }
        ["user", user] | ["user", user, ""] | ["user", user, "uploads"]
            if source.query().is_none() && valid_wallhaven_user(user) =>
        {
            format!("wallhavenu_{user}_{id}")
        }
        _ => return None,
    };
    let identity = if key == format!("wallhaven{id}") {
        format!("wallpaper-id:{id}")
    } else {
        format!("wallhaven-sha256:{}", archive_key_digest(&key))
    };
    Some((identity, key))
}

fn valid_directlink_format(path: &str) -> bool {
    let Some((host, file)) = path.split_once('/') else {
        return false;
    };
    !host.is_empty()
        && !host.contains(char::is_whitespace)
        && file
            .rsplit_once('.')
            .is_some_and(|(stem, ext)| !stem.is_empty() && !ext.is_empty())
}

/// gallery-dl uses prefix + archive_fmt; for an explicitly named table its
/// default prefix is empty instead of the extractor category.
fn upstream_archive_key_configured(item: &MediaItem, prefix: Option<&str>) -> Option<String> {
    let default = upstream_archive_key(item)?;
    match prefix {
        None => Some(default),
        Some(prefix) => {
            // A custom prefix can combine multiple extractor classes.
            // Neither numeric Philomena IDs nor URL-shaped directlink
            // payloads prove their source when a different class is named.
            if (philomena_id(item).is_some() && prefix != item.extractor)
                || (item.extractor == "directlink" && !prefix.is_empty() && prefix != "directlink")
            {
                return None;
            }
            Some(format!("{prefix}{}", default.strip_prefix(item.extractor)?))
        }
    }
}

/// Confirm a pinned Danbooru/E621 *post extractor* from the original
/// input URL. A tag search, pool, favorites or popular feed may contain
/// the same post ID but uses a different upstream archive template.
fn direct_booru_post(item: &MediaItem, source_url: &str) -> Option<u64> {
    let parsed = url::Url::parse(source_url).ok()?;
    if parsed.scheme() != "https"
        || parsed.port().is_some()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return None;
    }
    let host = parsed.host_str()?;
    match (item.extractor, host) {
        ("danbooru", "danbooru.donmai.us")
        | ("e621", "e621.net")
        | ("e926", "e926.net")
        | ("e6ai", "e6ai.net") => {}
        _ => return None,
    }
    let segments: Vec<_> = parsed.path_segments()?.collect();
    let url_id = match segments.as_slice() {
        ["posts", id] | ["post", "show", id] => id.parse::<u64>().ok()?,
        ["p", short] if matches!(item.extractor, "e621" | "e926" | "e6ai") => {
            u64::from_str_radix(short, 32).ok()?
        }
        _ => return None,
    };
    (url_id > 0 && item.post_id == Some(url_id)).then_some(url_id)
}

fn direct_post_key(id: u64) -> String {
    format!("direct-post-id:{id}")
}

/// A gallery-dl pool extractor uses a *different archive ID* from its
/// single-post extractor. The URL supplies the pool ID and the item holds
/// the post ID; both must match a pinned pool class and canonical site.
fn booru_pool(item: &MediaItem, source_url: &str) -> Option<(u64, u64)> {
    if !is_booru_pool_category(item.extractor) {
        return None;
    }
    let post = item.post_id.filter(|id| *id > 0)?;
    let parsed = url::Url::parse(source_url).ok()?;
    if parsed.scheme() != "https"
        || parsed.port().is_some()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return None;
    }
    match (item.extractor, parsed.host_str()?) {
        ("danbooru", "danbooru.donmai.us")
        | ("e621", "e621.net")
        | ("e926", "e926.net")
        | ("e6ai", "e6ai.net") => {}
        _ => return None,
    }
    let segments: Vec<_> = parsed.path_segments()?.collect();
    let pool = match segments.as_slice() {
        ["pools", id] | ["pool", "show", id] => id.parse::<u64>().ok()?,
        _ => return None,
    };
    (pool > 0).then_some((pool, post))
}

/// A favorite-group ID is part of the archive identity, and is available
/// from the already-supported Danbooru collection source URL.
fn danbooru_favorite_group(item: &MediaItem, source_url: &str) -> Option<(u64, u64)> {
    if item.extractor != "danbooru" {
        return None;
    }
    let post_id = item.post_id.filter(|id| *id > 0)?;
    let source = url::Url::parse(source_url).ok()?;
    if source.scheme() != "https"
        || source.port().is_some()
        || source.host_str() != Some("danbooru.donmai.us")
        || !source.username().is_empty()
        || source.password().is_some()
    {
        return None;
    }
    let segments: Vec<_> = source.path_segments()?.collect();
    let group_id = match segments.as_slice() {
        ["favorite_groups", id] | ["favorite_group", "show", id] => id.parse::<u64>().ok()?,
        _ => return None,
    };
    (group_id > 0).then_some((group_id, post_id))
}

/// The e621-family favorite extractor uses f_{user_id}_{id}. Require
/// explicit user_id=NUMBER on the original URL: /favorites without an
/// account selector depends on the active login and cannot be scoped safely.
fn e621_favorite(item: &MediaItem, source_url: &str) -> Option<(u64, u64)> {
    if !is_e621_favorite_category(item.extractor) {
        return None;
    }
    let post = item.post_id.filter(|id| *id > 0)?;
    let source = url::Url::parse(source_url).ok()?;
    if source.scheme() != "https"
        || source.port().is_some()
        || !source.username().is_empty()
        || source.password().is_some()
        || source.path() != "/favorites"
    {
        return None;
    }
    match (item.extractor, source.host_str()?) {
        ("e621", "e621.net") | ("e926", "e926.net") | ("e6ai", "e6ai.net") => {}
        _ => return None,
    }
    let mut user = None;
    for (key, value) in source.query_pairs() {
        if key == "user_id" {
            if user.is_some() {
                return None;
            }
            let id = value.parse::<u64>().ok().filter(|id| *id > 0)?;
            if value != id.to_string() {
                return None;
            }
            user = Some(id);
        }
    }
    Some((user?, post))
}

/// Pinned DanbooruTagExtractor and E621TagExtractor use
/// t_{search_tags}_{id}. Require an explicit, unique tags parameter and
/// only source paths that those exact extractor classes recognize.
fn booru_tag_search(item: &MediaItem, source_url: &str) -> Option<String> {
    if !is_booru_pool_category(item.extractor) || item.post_id.is_none_or(|id| id == 0) {
        return None;
    }
    let source = url::Url::parse(source_url).ok()?;
    if source.scheme() != "https"
        || source.port().is_some()
        || !source.username().is_empty()
        || source.password().is_some()
    {
        return None;
    }
    match (item.extractor, source.host_str()?, source.path()) {
        ("danbooru", "danbooru.donmai.us", "/posts")
        | ("e621", "e621.net", "/posts")
        | ("e621", "e621.net", "/post")
        | ("e926", "e926.net", "/posts")
        | ("e926", "e926.net", "/post")
        | ("e6ai", "e6ai.net", "/posts")
        | ("e6ai", "e6ai.net", "/post") => {}
        _ => return None,
    }
    let mut found = None;
    for (name, value) in source.query_pairs() {
        if name == "tags" {
            if found.is_some() {
                return None;
            }
            let tags = value.into_owned();
            if tags.len() > 1024 || tags.chars().any(char::is_control) {
                return None;
            }
            found = Some(tags);
        }
    }
    found
}

/// Pinned DanbooruMediaassetExtractor uses m{id}. Galerith's media asset
/// resolver preserves that asset ID as MediaItem.post_id, so confirm the
/// exact source asset route before using the stable upstream identity.
fn danbooru_media_asset(item: &MediaItem, source_url: &str) -> Option<u64> {
    if item.extractor != "danbooru" {
        return None;
    }
    let id = item.post_id.filter(|id| *id > 0)?;
    let source = url::Url::parse(source_url).ok()?;
    if source.scheme() != "https"
        || source.port().is_some()
        || source.host_str() != Some("danbooru.donmai.us")
        || !source.username().is_empty()
        || source.password().is_some()
    {
        return None;
    }
    let segments: Vec<_> = source.path_segments()?.collect();
    match segments.as_slice() {
        ["media_assets", source_id] if source_id.parse::<u64>().ok() == Some(id) => Some(id),
        _ => None,
    }
}

/// Pinned popular extractors use P_{scale[0]}_{date}_{id}. Daily,
/// weekly (normalized to Monday) and monthly scales are deterministic only
/// when the original popular URL explicitly specifies a valid date.
fn booru_popular(item: &MediaItem, source_url: &str) -> Option<String> {
    if !is_booru_pool_category(item.extractor) || item.post_id.is_none_or(|id| id == 0) {
        return None;
    }
    let source = url::Url::parse(source_url).ok()?;
    if source.scheme() != "https"
        || source.port().is_some()
        || !source.username().is_empty()
        || source.password().is_some()
    {
        return None;
    }
    let source_path = source.path();
    match (item.extractor, source.host_str()?, source_path) {
        ("danbooru", "danbooru.donmai.us", "/popular")
        | ("danbooru", "danbooru.donmai.us", "/explore/posts/popular")
        | ("e621", "e621.net", "/explore/posts/popular")
        | ("e926", "e926.net", "/explore/posts/popular")
        | ("e6ai", "e6ai.net", "/explore/posts/popular") => {}
        _ => return None,
    }
    let mut scale = None;
    let mut date = None;
    for (key, value) in source.query_pairs() {
        match key.as_ref() {
            "scale" if scale.is_none() => scale = Some(value.into_owned()),
            "date" if date.is_none() => date = Some(value.into_owned()),
            "scale" | "date" => return None,
            _ => {}
        }
    }
    let scale = scale?;
    let date = date?;
    validated_calendar_date(&date)?;
    let (initial, normalized_date) = match scale.as_str() {
        "day" => ("d", date),
        "week" => ("w", popular_week_monday(&date)?),
        "month" => ("m", date[..7].to_owned()),
        _ => return None,
    };
    Some(format!(
        "{}P_{initial}_{normalized_date}_{}",
        item.extractor, item.post_id?
    ))
}

/// Scoped identity unifies native archive records, advisory locks and
/// read-only upstream lookup for verified posts, pools, favorite groups,
/// tag searches, popular collections, media assets and explicit favorites.
/// Pinned Moebooru post={id}, pool=p_{pool}_{id}, tag=t_{search_tags}_{id}.
/// Never share an ID among unrelated collections or search queries.
fn moebooru_archive_scope(item: &MediaItem, source_url: &str) -> Option<(String, String)> {
    if !is_moebooru_category(item.extractor) {
        return None;
    }
    let post = item.post_id.filter(|id| *id > 0)?;
    let source = url::Url::parse(source_url).ok()?;
    if source.scheme() != "https"
        || source.port().is_some()
        || !source.username().is_empty()
        || source.password().is_some()
    {
        return None;
    }
    match (item.extractor, source.host_str()?) {
        ("yandere", "yande.re")
        | ("konachan", "konachan.com")
        | ("konachan", "konachan.net")
        | ("sakugabooru", "sakugabooru.com")
        | ("sakugabooru", "www.sakugabooru.com")
        | ("lolibooru", "lolibooru.moe") => {}
        _ => return None,
    }
    let segments: Vec<_> = source.path_segments()?.collect();
    match segments.as_slice() {
        ["post", "show", id] => {
            let source_id = id.parse::<u64>().ok()?;
            if source_id != post {
                return None;
            }
            Some((direct_post_key(post), format!("{}{post}", item.extractor)))
        }
        ["pool", "show", id] => {
            let pool = id.parse::<u64>().ok().filter(|id| *id > 0)?;
            Some((
                format!("pool-id:{pool}:post-id:{post}"),
                format!("{}p_{pool}_{post}", item.extractor),
            ))
        }
        ["post"] => {
            let mut tags = None;
            for (key, value) in source.query_pairs() {
                if key == "tags" {
                    if tags.is_some() {
                        return None;
                    }
                    let value = value.into_owned();
                    if value.len() > 1024 || value.chars().any(char::is_control) {
                        return None;
                    }
                    tags = Some(value);
                }
            }
            let normalized = format!("{}t_{}_{post}", item.extractor, tags?);
            Some((
                format!("tag-sha256:{}", archive_key_digest(&normalized)),
                normalized,
            ))
        }
        _ => None,
    }
}

/// Source-verified Gelbooru/Gelbooru Beta v0.2 post and tag extractors:
/// `{id}` for a direct post, `t_{search_tags}_{id}` for a tag search.
/// Only the canonical query order and native-supported paths are accepted.
fn gelbooru_archive_scope(item: &MediaItem, source_url: &str) -> Option<(String, String)> {
    if !is_gelbooru_category(item.extractor) {
        return None;
    }
    let post = item.post_id.filter(|id| *id > 0)?;
    let source = url::Url::parse(source_url).ok()?;
    if source.scheme() != "https"
        || source.port().is_some()
        || !source.username().is_empty()
        || source.password().is_some()
    {
        return None;
    }
    match (item.extractor, source.host_str()?) {
        ("gelbooru", "gelbooru.com")
        | ("gelbooru", "www.gelbooru.com")
        | ("rule34", "rule34.xxx")
        | ("safebooru", "safebooru.org")
        | ("tbib", "tbib.org")
        | ("hypnohub", "hypnohub.net")
        | ("xbooru", "xbooru.com") => {}
        _ => return None,
    }
    if source.path() != "/index.php" && !(item.extractor == "gelbooru" && source.path() == "/") {
        return None;
    }
    let query: Vec<_> = source.query_pairs().collect();
    match query.as_slice() {
        [(page, value), (action, kind), (field, id)]
            if page == "page"
                && value == "post"
                && action == "s"
                && kind == "view"
                && field == "id" =>
        {
            let parsed = id.parse::<u64>().ok()?;
            if parsed != post {
                return None;
            }
            Some((direct_post_key(post), format!("{}{post}", item.extractor)))
        }
        [(page, value), (action, kind), (field, tags)]
            if page == "page"
                && value == "post"
                && action == "s"
                && kind == "list"
                && field == "tags" =>
        {
            if tags.len() > 1024 || tags.chars().any(char::is_control) {
                return None;
            }
            let normalized = format!("{}t_{tags}_{post}", item.extractor);
            Some((
                format!("tag-sha256:{}", archive_key_digest(&normalized)),
                normalized,
            ))
        }
        _ => None,
    }
}

fn verified_archive_scope(item: &MediaItem, source_url: &str) -> Option<(String, String)> {
    booru_archive_scope(item, source_url)
        .or_else(|| moebooru_archive_scope(item, source_url))
        .or_else(|| gelbooru_archive_scope(item, source_url))
        .or_else(|| wallhaven_archive_scope(item, source_url))
        .or_else(|| fourchan_archive_scope(item, source_url))
        .or_else(|| imgur_archive_scope(item, source_url))
        .or_else(|| fivehundredpx_archive_scope(item, source_url))
        .or_else(|| photovogue_archive_scope(item, source_url))
        .or_else(|| fapachi_archive_scope(item, source_url))
        .or_else(|| fapello_archive_scope(item, source_url))
        .or_else(|| lightroom_archive_scope(item, source_url))
        .or_else(|| foriio_archive_scope(item, source_url))
        .or_else(|| piczel_archive_scope(item, source_url))
        .or_else(|| thirtyfivephoto_archive_scope(item, source_url))
        .or_else(|| unsplash_archive_scope(item, source_url))
        .or_else(|| lexica_archive_scope(item, source_url))
        .or_else(|| pixeldrain_archive_scope(item, source_url))
        .or_else(|| listal_archive_scope(item, source_url))
        .or_else(|| lensdump_archive_scope(item, source_url))
}

fn booru_archive_scope(item: &MediaItem, source_url: &str) -> Option<(String, String)> {
    if let Some(id) = direct_booru_post(item, source_url) {
        return Some((direct_post_key(id), format!("{}{id}", item.extractor)));
    }
    if let Some((pool, post)) = booru_pool(item, source_url) {
        return Some((
            format!("pool-id:{pool}:post-id:{post}"),
            format!("{}p_{pool}_{post}", item.extractor),
        ));
    }
    if let Some((group, post)) = danbooru_favorite_group(item, source_url) {
        return Some((
            format!("favorite-group-id:{group}:post-id:{post}"),
            format!("danboorufg_{group}_{post}"),
        ));
    }
    if let Some((user, post)) = e621_favorite(item, source_url) {
        return Some((
            format!("favorite-user-id:{user}:post-id:{post}"),
            format!("{}f_{user}_{post}", item.extractor),
        ));
    }
    if let Some(tags) = booru_tag_search(item, source_url) {
        let normalized = format!("{}t_{tags}_{}", item.extractor, item.post_id?);
        return Some((
            format!("tag-sha256:{}", archive_key_digest(&normalized)),
            normalized,
        ));
    }
    if let Some(asset) = danbooru_media_asset(item, source_url) {
        return Some((
            format!("media-asset-id:{asset}"),
            format!("danboorum{asset}"),
        ));
    }
    let normalized = booru_popular(item, source_url)?;
    Some((
        format!("popular-sha256:{}", archive_key_digest(&normalized)),
        normalized,
    ))
}

fn philomena_id(item: &MediaItem) -> Option<u64> {
    match item.extractor {
        "derpibooru" | "ponybooru" | "furbooru" => item.post_id.filter(|id| *id != 0),
        _ => None,
    }
}

fn legacy_url_key(item: &MediaItem) -> String {
    // Retain this older Galerith identity for backwards-compatible reads.
    // Arbitrary posts can contain multiple originals and must still use a
    // complete URL fingerprint rather than a post-only archive key.
    let digest = Sha256::digest(item.url.as_bytes());
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    match item.post_id {
        Some(id) => format!("post:{id}:url-sha256:{hex}"),
        None => format!("url-sha256:{hex}"),
    }
}

fn key(item: &MediaItem) -> String {
    // Stable native identity is safe for Philomena's single-image records,
    // avoiding duplicate downloads after source URL/token rotation.
    // Never generalize this to multi-original source posts.
    match philomena_id(item) {
        Some(id) => format!("image-id:{id}"),
        None => legacy_url_key(item),
    }
}

/// The pinned gallery-dl directlink archive format is the category prefix
/// followed by "{domain}/{path}/{filename}.{extension}". The upstream
/// path field starts after the leading URL slash, so nested paths retain
/// one domain separator while root-level files have an empty path field.
/// Signed URL query parameters do not contribute to this upstream key.
/// Do not write these potentially sensitive plaintext path keys from Galerith.
fn upstream_directlink_key(item: &MediaItem) -> Option<String> {
    if item.extractor != "directlink" {
        return None;
    }
    let parsed = url::Url::parse(&item.url).ok()?;
    if !matches!(parsed.scheme(), "http" | "https")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return None;
    }
    let domain = parsed.host_str()?;
    let authority = match parsed.port() {
        Some(port) => format!("{domain}:{port}"),
        None => domain.to_owned(),
    };
    // The upstream regex consumes the first '/' outside its path capture.
    let url_path = parsed.path().strip_prefix('/')?;
    let decoded = percent_encoding::percent_decode_str(url_path)
        .decode_utf8()
        .ok()?;
    let (directory, name) = decoded.rsplit_once('/').unwrap_or(("", decoded.as_ref()));
    let (stem, ext) = name.rsplit_once('.')?;
    if stem.is_empty() || ext.is_empty() {
        return None;
    }
    Some(format!(
        "directlink{authority}/{directory}/{stem}.{}",
        ext.to_ascii_lowercase()
    ))
}

/// Quote a SQLite table identifier; never interpolate an unquoted user value.
fn archive_table_identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn has_upstream_table(connection: &Connection, name: &str) -> Result<bool, rusqlite::Error> {
    let found: i64 = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1) WHERE name = 'entry' AND pk = 1 AND UPPER(type) = 'TEXT')",
        params![name],
        |row| row.get(0),
    )?;
    Ok(found != 0)
}

/// The pinned Philomena extractors (Derpibooru, Ponybooru, Furbooru)
/// inherit archive_fmt = "{id}" for post, search and gallery classes.
/// These categories have one original per image ID. Do not generalize this
/// to boorus whose tag, pool or favorite extractor keys have other formats.
fn upstream_archive_key(item: &MediaItem) -> Option<String> {
    match item.extractor {
        "directlink" => upstream_directlink_key(item),
        "derpibooru" | "ponybooru" | "furbooru" => {
            let id = item.post_id.filter(|id| *id != 0)?;
            Some(format!("{}{id}", item.extractor))
        }
        _ => None,
    }
}

impl Archive {
    pub fn open(path: &Path) -> Result<Self, Box<dyn Error + Send + Sync>> {
        Self::open_with_reference(path, None)
    }

    /// Optionally use an *existing* gallery-dl archive as a strictly
    /// read-only fallback for pinned directlink identities. Galerith writes
    /// all completed downloads to its own database, never the reference.
    pub fn open_with_reference(
        path: &Path,
        reference_path: Option<&Path>,
    ) -> Result<Self, Box<dyn Error + Send + Sync>> {
        Self::open_with_reference_options(path, reference_path, None, None)
    }

    /// Read from a configured upstream table. Unlike the implicit default
    /// table, an explicitly configured table uses an empty default prefix.
    /// Bare Philomena IDs in such a shared table are ambiguous, so cannot be
    /// migrated without a category-specific explicit prefix.
    pub fn open_with_reference_table(
        path: &Path,
        reference_path: Option<&Path>,
        reference_table: &str,
    ) -> Result<Self, Box<dyn Error + Send + Sync>> {
        Self::open_with_reference_options(path, reference_path, Some(reference_table), None)
    }

    pub fn open_with_reference_options(
        path: &Path,
        reference_path: Option<&Path>,
        table_option: Option<&str>,
        prefix_option: Option<&str>,
    ) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let reference_table = table_option.unwrap_or("archive");
        let resolved_prefix = prefix_option.or_else(|| table_option.map(|_| ""));
        if resolved_prefix.is_some_and(|p| p.len() > 128 || p.contains('\0')) {
            return Err(
                io::Error::new(io::ErrorKind::InvalidInput, "invalid archive prefix").into(),
            );
        }
        if reference_table.is_empty()
            || reference_table.len() > 128
            || reference_table.contains('\0')
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid gallery-dl archive table name",
            )
            .into());
        }
        // Validate the upstream source before creating Galerith's own DB.
        // The READ_ONLY flag never creates a missing upstream file.
        let upstream_reference = if let Some(reference_path) = reference_path {
            let source =
                Connection::open_with_flags(reference_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            source.busy_timeout(Duration::from_secs(5))?;
            if !has_upstream_table(&source, reference_table)? {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "gallery-dl archive table must have entry TEXT PRIMARY KEY",
                )
                .into());
            }
            // A read-only handle does not protect the original if the native
            // destination points to the *same* database via a relative path
            // or symlink. Reject before CREATE TABLE can modify that file.
            if path.exists() {
                let same_path = fs::canonicalize(path)? == fs::canonicalize(reference_path)?;
                // Canonicalization alone misses hard links. SQLite databases
                // opened through hard-link aliases must never be written via
                // the native handle while presented as a read-only reference.
                #[cfg(unix)]
                let same_inode = {
                    use std::os::unix::fs::MetadataExt;
                    let native_info = fs::metadata(path)?;
                    let source_info = fs::metadata(reference_path)?;
                    native_info.dev() == source_info.dev() && native_info.ino() == source_info.ino()
                };
                #[cfg(not(unix))]
                let same_inode = false;
                if same_path || same_inode {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "native archive and read-only gallery-dl archive must be different files",
                    )
                    .into());
                }
            }
            Some(source)
        } else {
            None
        };
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        let connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS downloads (
                category TEXT NOT NULL,
                identity TEXT NOT NULL,
                recorded_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                PRIMARY KEY(category, identity)
            );
            CREATE TABLE IF NOT EXISTS imported_gallery_dl (
                category TEXT NOT NULL,
                archive_sha256 TEXT NOT NULL,
                PRIMARY KEY(category, archive_sha256)
            ) WITHOUT ROWID;",
        )?;
        // Canonicalize the newly opened database path: relative and symlink
        // aliases must share the same interprocess lock namespace.
        let lock_directory = if path == Path::new(":memory:") {
            None
        } else {
            let mut name = fs::canonicalize(path)?.into_os_string();
            name.push(".galerith-locks");
            Some(PathBuf::from(name))
        };
        // Only consult a preexisting upstream archive table with the expected
        // primary-key column. Never create or mutate upstream's archive rows.
        let upstream_directlink_archive = has_upstream_table(&connection, "archive")?;
        Ok(Self {
            connection,
            lock_directory,
            upstream_directlink_archive,
            upstream_reference,
            upstream_reference_table: reference_table.to_owned(),
            upstream_reference_prefix: resolved_prefix.map(str::to_owned),
        })
    }

    /// Hold this lock through contains(), the HTTP transfer and record().
    /// Sixty-four stable stripes bound file-count growth. Hash collisions
    /// only cause harmless extra serialization, never false archive hits.
    /// An OS crash releases the lock without deleting its on-disk inode.
    pub async fn lock_item(
        &self,
        item: &MediaItem,
    ) -> Result<Option<ArchiveItemLock>, Box<dyn Error + Send + Sync>> {
        self.lock_item_for_source(item, "").await
    }

    /// A verified direct post must reserve its stable ID even when its
    /// CDN URL or signed token has changed between workers.
    pub async fn lock_item_for_source(
        &self,
        item: &MediaItem,
        source_url: &str,
    ) -> Result<Option<ArchiveItemLock>, Box<dyn Error + Send + Sync>> {
        let Some(directory) = &self.lock_directory else {
            return Ok(None);
        };
        fs::create_dir_all(directory)?;
        let mut hasher = Sha256::new();
        hasher.update(item.extractor.as_bytes());
        hasher.update([0]);
        let stable_scope = verified_archive_scope(item, source_url).map(|(native, _)| native);
        hasher.update(stable_scope.as_deref().unwrap_or(&key(item)).as_bytes());
        let stripe = hasher.finalize()[0] & 63;
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(directory.join(format!("{stripe:02x}.lock")))?;
        loop {
            match FileExt::try_lock_exclusive(&file) {
                Ok(()) => return Ok(Some(ArchiveItemLock(file))),
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                    tokio::time::sleep(Duration::from_millis(30)).await;
                }
                Err(err) => return Err(err.into()),
            }
        }
    }

    /// One-time, explicit offline migration of *supported* default upstream
    /// archive keys into SHA-256 native keys. Streams rows through a single
    /// SQLite transaction; failed imports roll back. Safe to re-run.
    ///
    /// This does not imply that an unsupported upstream archive entry is a
    /// successful Galerith download, and it does not alter the source file.
    pub fn import_reference(&self) -> Result<(usize, usize), Box<dyn Error + Send + Sync>> {
        let source = self.upstream_reference.as_ref().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "gallery-dl reference archive is required",
            )
        })?;
        let query = format!(
            "SELECT entry FROM {}",
            archive_table_identifier(&self.upstream_reference_table)
        );
        let mut select = source.prepare(&query)?;
        let mut rows = select.query([])?;
        let transaction = self.connection.unchecked_transaction()?;
        let mut matched = 0usize;
        let mut inserted = 0usize;
        {
            let mut insert = transaction.prepare(
                "INSERT OR IGNORE INTO imported_gallery_dl (category, archive_sha256) VALUES (?1, ?2)",
            )?;
            while let Some(row) = rows.next()? {
                let entry: String = row.get(0)?;
                if let Some((category, normalized)) =
                    import_category(&entry, self.upstream_reference_prefix.as_deref())
                {
                    matched += 1;
                    inserted +=
                        insert.execute(params![category, archive_key_digest(&normalized)])?;
                }
            }
        }
        transaction.commit()?;
        Ok((matched, inserted))
    }

    pub fn contains(&self, item: &MediaItem) -> Result<bool, rusqlite::Error> {
        let present: i64 = self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM downloads WHERE category = ?1 AND identity = ?2)",
            params![item.extractor, key(item)],
            |row| row.get(0),
        )?;
        if present != 0 {
            return Ok(true);
        }
        // Old Galerith Philomena downloads used the URL-based key. Preserve
        // existing archive history through this targeted migration read.
        if philomena_id(item).is_some() {
            let old: i64 = self.connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM downloads WHERE category = ?1 AND identity = ?2)",
                params![item.extractor, legacy_url_key(item)],
                |row| row.get(0),
            )?;
            if old != 0 {
                // The exact legacy URL fingerprint proves which Philomena
                // original was saved. Promote to its stable single-image ID
                // without deleting the historical row or re-downloading.
                // Never promote ambiguous post-only keys or foreign entries.
                self.connection.execute(
                    "INSERT OR IGNORE INTO downloads (category, identity) VALUES (?1, ?2)",
                    params![item.extractor, key(item)],
                )?;
                return Ok(true);
            }
        }
        if let Some(source_key) = upstream_archive_key(item) {
            let migrated: i64 = self.connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM imported_gallery_dl WHERE category = ?1 AND archive_sha256 = ?2)",
                params![item.extractor, archive_key_digest(&source_key)],
                |row| row.get(0),
            )?;
            if migrated != 0 {
                return Ok(true);
            }
            // Explicit external reference takes precedence, then the
            // existing same-file fallback for legacy mixed databases.
            let source = self
                .upstream_reference
                .as_ref()
                .or_else(|| self.upstream_directlink_archive.then_some(&self.connection));
            if let Some(source) = source {
                let table = if self.upstream_reference.is_some() {
                    &self.upstream_reference_table
                } else {
                    "archive"
                };
                let query = format!(
                    "SELECT EXISTS(SELECT 1 FROM {} WHERE entry = ?1)",
                    archive_table_identifier(table)
                );
                let lookup_key = if self.upstream_reference.is_some() {
                    upstream_archive_key_configured(item, self.upstream_reference_prefix.as_deref())
                } else {
                    Some(source_key)
                };
                if let Some(lookup_key) = lookup_key {
                    let upstream: i64 =
                        source.query_row(&query, params![lookup_key], |row| row.get(0))?;
                    return Ok(upstream != 0);
                }
            }
        }
        Ok(false)
    }

    /// Source-aware skip check for only the verified direct-post format.
    /// Never treat a feed/pool/favorite entry as a plain post archive key.
    /// Legacy URL-only rows cannot prove which extractor class/source owned
    /// a download once any stable, source-scoped identity exists in that
    /// category. Prefer a harmless retry over falsely skipping another feed.
    fn has_scoped_history(&self, category: &str) -> Result<bool, rusqlite::Error> {
        if !category_uses_source_scoped_ids(category) {
            return Ok(false);
        }
        let count: i64 = self.connection.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM downloads
                WHERE category = ?1
                  AND identity NOT LIKE 'url-sha256:%'
                  AND identity NOT LIKE 'post:%:url-sha256:%'
            )",
            params![category],
            |row| row.get(0),
        )?;
        Ok(count != 0)
    }

    pub fn contains_for_source(
        &self,
        item: &MediaItem,
        source_url: &str,
    ) -> Result<bool, rusqlite::Error> {
        let Some((stable_key, normalized)) = verified_archive_scope(item, source_url) else {
            if !self.has_scoped_history(item.extractor)? && self.contains(item)? {
                return Ok(true);
            }
            // Old builds mislabeled e926 source downloads as e621 even for
            // tag/pool/favorite feeds. Preserve *exact* downloaded URL hashes
            // when the original input can be verified as the e926 site.
            if item.extractor == "e926" {
                let verified = url::Url::parse(source_url).ok().is_some_and(|source| {
                    source.scheme() == "https"
                        && source.host_str() == Some("e926.net")
                        && source.username().is_empty()
                        && source.password().is_none()
                });
                if verified && !self.has_scoped_history("e621")? {
                    let old: i64 = self.connection.query_row(
                        "SELECT EXISTS(SELECT 1 FROM downloads WHERE category = 'e621' AND identity = ?1)",
                        params![legacy_url_key(item)],
                        |row| row.get(0),
                    )?;
                    if old != 0 {
                        self.record(item)?;
                        return Ok(true);
                    }
                }
            }
            return Ok(false);
        };
        let recorded: i64 = self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM downloads WHERE category = ?1 AND identity = ?2)",
            params![item.extractor, stable_key],
            |row| row.get(0),
        )?;
        if recorded != 0 {
            return Ok(true);
        }
        if !self.has_scoped_history(item.extractor)? && self.contains(item)? {
            // Only a purely URL-hash-only historical category proves that
            // the exact original was saved with unknown source provenance.
            // Mixed native source-scoped categories are ambiguous. Do not
            // infer that a match in one pool/search belongs to another.
            self.connection.execute(
                "INSERT OR IGNORE INTO downloads (category, identity) VALUES (?1, ?2)",
                params![item.extractor, stable_key],
            )?;
            return Ok(true);
        }
        if item.extractor == "e926" && !self.has_scoped_history("e621")? {
            // Before correcting the pinned site category, Galerith recorded
            // e926 under e621. An EXACT original URL digest is proof that
            // the media was downloaded, unlike an ambiguous post-only key.
            let previous: i64 = self.connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM downloads WHERE category = 'e621' AND identity = ?1)",
                params![legacy_url_key(item)],
                |row| row.get(0),
            )?;
            if previous != 0 {
                self.record_for_source(item, source_url)?;
                return Ok(true);
            }
        }

        let migrated: i64 = self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM imported_gallery_dl WHERE category = ?1 AND archive_sha256 = ?2)",
            params![item.extractor, archive_key_digest(&normalized)],
            |row| row.get(0),
        )?;
        if migrated != 0 {
            return Ok(true);
        }
        let source = self
            .upstream_reference
            .as_ref()
            .or_else(|| self.upstream_directlink_archive.then_some(&self.connection));
        if let Some(source) = source {
            // Numeric keys without a category prefix are ambiguous when
            // a table is shared. Permit only a matching category-specific
            // explicit prefix, or the implicit category-prefixed default.
            let configured = self.upstream_reference.as_ref().and_then(|_| {
                self.upstream_reference_prefix
                    .as_deref()
                    .map(|prefix| (prefix == item.extractor).then(|| normalized.clone()))
            });
            let upstream_key = if self.upstream_reference.is_some() {
                match configured {
                    Some(Some(key)) => Some(key),
                    Some(None) => None,
                    None => Some(normalized),
                }
            } else {
                Some(normalized)
            };
            if let Some(upstream_key) = upstream_key {
                let table = if self.upstream_reference.is_some() {
                    &self.upstream_reference_table
                } else {
                    "archive"
                };
                let query = format!(
                    "SELECT EXISTS(SELECT 1 FROM {} WHERE entry = ?1)",
                    archive_table_identifier(table)
                );
                let upstream: i64 =
                    source.query_row(&query, params![upstream_key], |row| row.get(0))?;
                return Ok(upstream != 0);
            }
        }
        Ok(false)
    }

    /// A successful source-scoped download writes only its verified archive
    /// identity. Writing a bare media-URL fingerprint too would incorrectly
    /// suppress the same URL in a different search, pool or upload owner.
    /// Historical URL-only rows remain readable when provenance is unambiguous.
    pub fn record_for_source(
        &self,
        item: &MediaItem,
        source_url: &str,
    ) -> Result<(), rusqlite::Error> {
        let Some((stable_key, _)) = verified_archive_scope(item, source_url) else {
            return self.record(item);
        };
        self.connection.execute(
            "INSERT OR IGNORE INTO downloads (category, identity) VALUES (?1, ?2)",
            params![item.extractor, stable_key],
        )?;
        Ok(())
    }

    /// Call only after a successful download and completion rename. Merely
    /// finding a preexisting filename is insufficient evidence for archiving.
    pub fn record(&self, item: &MediaItem) -> Result<(), rusqlite::Error> {
        self.connection.execute(
            "INSERT OR IGNORE INTO downloads (category, identity) VALUES (?1, ?2)",
            params![item.extractor, key(item)],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract_direct_media;

    #[test]
    fn successful_records_survive_reopen_without_persisting_token_url() {
        let path =
            std::env::temp_dir().join(format!("galerith-archive-{}.sqlite", std::process::id()));
        let _ = fs::remove_file(&path);
        let item =
            extract_direct_media("https://example.org/image.jpg?token=private-token-test").unwrap();
        {
            let archive = Archive::open(&path).unwrap();
            assert!(!archive.contains(&item).unwrap());
            archive.record(&item).unwrap();
            archive.record(&item).unwrap();
            assert!(archive.contains(&item).unwrap());
        }
        {
            let archive = Archive::open(&path).unwrap();
            assert!(archive.contains(&item).unwrap());
            let different = extract_direct_media("https://example.org/other.jpg").unwrap();
            assert!(!archive.contains(&different).unwrap());
        }
        let bytes = fs::read(&path).unwrap();
        assert!(!bytes
            .windows(b"private-token-test".len())
            .any(|chunk| chunk == b"private-token-test"));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn multiple_originals_sharing_a_post_id_are_not_suppressed() {
        let path = std::env::temp_dir().join(format!(
            "galerith-archive-multimedia-{}.sqlite",
            std::process::id()
        ));
        let _ = fs::remove_file(&path);
        let one = MediaItem {
            url: "https://cdn.example.org/p/17/first.jpg?token=secret-first".into(),
            filename: "first.jpg".into(),
            extractor: "multi-image",
            post_id: Some(17),
        };
        let two = MediaItem {
            url: "https://cdn.example.org/p/17/second.jpg?token=secret-second".into(),
            filename: "second.jpg".into(),
            ..one.clone()
        };
        let same_url_new_name = MediaItem {
            filename: "renamed-first.jpg".into(),
            ..one.clone()
        };
        let query_distinguished = MediaItem {
            url: "https://cdn.example.org/media?id=2".into(),
            filename: "query-2.jpg".into(),
            ..one.clone()
        };
        let other_query = MediaItem {
            url: "https://cdn.example.org/media?id=1".into(),
            filename: "query-1.jpg".into(),
            ..one.clone()
        };
        {
            let archive = Archive::open(&path).unwrap();
            archive.record(&one).unwrap();
            assert!(archive.contains(&one).unwrap());
            assert!(archive.contains(&same_url_new_name).unwrap());
            assert!(!archive.contains(&two).unwrap());
            archive.record(&two).unwrap();
            archive.record(&query_distinguished).unwrap();
            assert!(!archive.contains(&other_query).unwrap());
            assert!(archive.contains(&two).unwrap());
        }
        let archive = Archive::open(&path).unwrap();
        assert!(archive.contains(&one).unwrap());
        assert!(archive.contains(&two).unwrap());
        assert!(!archive.contains(&other_query).unwrap());
        drop(archive);
        let bytes = fs::read(&path).unwrap();
        for sensitive in ["secret-first", "secret-second", "https://cdn.example.org"] {
            assert!(!bytes
                .windows(sensitive.len())
                .any(|part| part == sensitive.as_bytes()));
        }
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn legacy_post_only_keys_do_not_falsely_suppress_other_media() {
        let path = std::env::temp_dir().join(format!(
            "galerith-archive-legacy-{}.sqlite",
            std::process::id()
        ));
        let _ = fs::remove_file(&path);
        let item = MediaItem {
            url: "https://cdn.example.org/p/77/a.jpg".into(),
            filename: "a.jpg".into(),
            extractor: "multiple",
            post_id: Some(77),
        };
        let archive = Archive::open(&path).unwrap();
        archive
            .connection
            .execute(
                "INSERT INTO downloads (category, identity) VALUES (?1, ?2)",
                params!["multiple", "post:77"],
            )
            .unwrap();
        // The old format does not say which media original was downloaded.
        // Favor a safe re-download over suppressing potentially missing files.
        assert!(!archive.contains(&item).unwrap());
        archive.record(&item).unwrap();
        assert!(archive.contains(&item).unwrap());
        drop(archive);
        fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn archive_lock_serializes_rechecks_after_recording() {
        let path = std::env::temp_dir().join(format!(
            "galerith-archive-lock-{}.sqlite",
            std::process::id()
        ));
        let _ = fs::remove_file(&path);
        let item = MediaItem {
            url: "https://cdn.example.org/lock/5.jpg?token=old".into(),
            filename: "5.jpg".into(),
            extractor: "derpibooru",
            post_id: Some(5),
        };
        let rotated = MediaItem {
            url: "https://another-cdn.example.org/5.png?token=new".into(),
            filename: "5.png".into(),
            ..item.clone()
        };
        assert_eq!(key(&item), key(&rotated));
        let first = Archive::open(&path).unwrap();
        let guard = first.lock_item(&item).await.unwrap().unwrap();
        assert!(!first.contains(&item).unwrap());
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let next_path = path.clone();
        let next_item = rotated;
        let worker = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let second = Archive::open(&next_path).unwrap();
                ready_tx.send(()).unwrap();
                let _second_guard = second.lock_item(&next_item).await.unwrap().unwrap();
                done_tx.send(second.contains(&next_item).unwrap()).unwrap();
            });
        });
        ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(done_rx.recv_timeout(Duration::from_millis(150)).is_err());
        first.record(&item).unwrap();
        drop(guard);
        assert!(done_rx.recv_timeout(Duration::from_secs(5)).unwrap());
        worker.join().unwrap();
        let locks = first.lock_directory.clone().unwrap();
        drop(first);
        fs::remove_dir_all(locks).unwrap();
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn reads_pinned_gallery_dl_directlink_entries_without_writing_to_them() {
        let path = std::env::temp_dir().join(format!(
            "galerith-upstream-archive-{}.sqlite",
            std::process::id()
        ));
        let _ = fs::remove_file(&path);
        let original = Connection::open(&path).unwrap();
        original
            .execute_batch("CREATE TABLE archive(entry TEXT PRIMARY KEY) WITHOUT ROWID;")
            .unwrap();
        original
            .execute(
                "INSERT INTO archive(entry) VALUES (?1)",
                params!["directlinkexample.org/images/photo.jpg"],
            )
            .unwrap();
        drop(original);

        let archive = Archive::open(&path).unwrap();
        let old = extract_direct_media("https://example.org/images/photo.JPG?X-Amz-Signature=old")
            .unwrap();
        assert!(archive.contains(&old).unwrap());
        let rotated =
            extract_direct_media("https://example.org/images/photo.JPG?X-Amz-Signature=rotated")
                .unwrap();
        assert!(archive.contains(&rotated).unwrap());
        let another = extract_direct_media("https://example.org/images/other.jpg").unwrap();
        assert!(!archive.contains(&another).unwrap());
        assert_eq!(
            upstream_directlink_key(
                &extract_direct_media("https://example.org/photo.jpg?auth=rotated").unwrap()
            ),
            Some("directlinkexample.org//photo.jpg".to_owned())
        );
        archive.record(&another).unwrap();
        assert!(archive.contains(&another).unwrap());
        let entries: i64 = archive
            .connection
            .query_row("SELECT COUNT(*) FROM archive", [], |row| row.get(0))
            .unwrap();
        assert_eq!(entries, 1);
        let lock_dir = archive.lock_directory.clone().unwrap();
        drop(archive);
        if lock_dir.exists() {
            fs::remove_dir_all(lock_dir).unwrap();
        }
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn quoted_custom_archive_table_is_read_only_and_importable() {
        let root =
            std::env::temp_dir().join(format!("galerith-import-table-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let native = root.join("native.sqlite");
        let reference = root.join("source.sqlite");
        let custom = "my \"quoted\" archive";
        let source = Connection::open(&reference).unwrap();
        let sql = format!(
            "CREATE TABLE {}(entry TEXT PRIMARY KEY) WITHOUT ROWID;",
            archive_table_identifier(custom)
        );
        source.execute_batch(&sql).unwrap();
        source
            .execute(
                &format!(
                    "INSERT INTO {} (entry) VALUES (?1)",
                    archive_table_identifier(custom)
                ),
                params!["example.org/photos/pinned.jpg"],
            )
            .unwrap();
        drop(source);
        let bytes = fs::read(&reference).unwrap();

        let archive =
            Archive::open_with_reference_table(&native, Some(&reference), custom).unwrap();
        let item =
            extract_direct_media("https://example.org/photos/pinned.jpg?token=rotated").unwrap();
        assert!(archive.contains(&item).unwrap());
        assert_eq!(archive.import_reference().unwrap(), (1, 1));
        drop(archive);
        assert_eq!(fs::read(&reference).unwrap(), bytes);
        fs::remove_file(reference).unwrap();
        let native_only = Archive::open(&native).unwrap();
        assert!(native_only.contains(&item).unwrap());
        drop(native_only);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn named_table_skips_ambiguous_numeric_ids_without_explicit_site_prefix() {
        assert_eq!(import_category("123", Some("")), None);
        assert_eq!(import_category("123", Some("shared_")), None);
        assert_eq!(
            import_category("derpibooru123", None),
            Some(("derpibooru", "derpibooru123".to_owned()))
        );
        assert_eq!(
            import_category("derpibooru123", Some("derpibooru")),
            Some(("derpibooru", "derpibooru123".to_owned()))
        );
        assert_eq!(
            import_category("example.org//photo.jpg", Some("")),
            Some(("directlink", "directlinkexample.org//photo.jpg".to_owned()))
        );
        assert!(upstream_archive_key_configured(
            &MediaItem {
                url: "https://cdn.invalid/a.jpg".into(),
                filename: "a.jpg".into(),
                extractor: "ponybooru",
                post_id: Some(123),
            },
            Some(""),
        )
        .is_none());
    }

    #[test]
    fn numeric_foreign_ids_must_match_pinned_canonical_integer_format() {
        for (category, item) in [
            ("danbooru", "danbooru123"),
            ("derpibooru", "derpibooru123"),
            ("e621", "e621123"),
            ("konachan", "konachan123"),
            ("gelbooru", "gelbooru123"),
        ] {
            assert_eq!(
                import_category(item, None),
                Some((category, item.to_owned()))
            );
            let padded = format!("{category}00123");
            assert_eq!(import_category(&padded, None), None);
            assert_eq!(import_category("00123", Some(category)), None);
            assert_eq!(import_category("0", Some(category)), None);
            assert_eq!(
                import_category(item, Some(category)),
                Some((category, item.to_owned()))
            );
        }
        assert!(!canonical_numeric_archive_suffix("18446744073709551616"));
        assert!(!canonical_numeric_archive_suffix("-123"));
    }

    #[test]
    fn offline_import_rejects_cross_category_directlinks_and_numeric_4chan_keys() {
        // A 4chan archive key must identify its board, thread and media tim,
        // never just an unscoped integer from a foreign archive.
        assert_eq!(import_category("4chan123", None), None);
        assert_eq!(import_category("123", Some("4chan")), None);
        assert_eq!(
            import_category("4chana_123_456", None),
            Some(("4chan", "4chana_123_456".to_owned()))
        );
        assert_eq!(
            import_category("4chana_123_456", Some("4chan")),
            Some(("4chan", "4chana_123_456".to_owned()))
        );
        // A named archive for an unrelated extractor must not silently
        // import a URL-shaped row under the directlink category.
        assert_eq!(
            import_category("booru.example/photo.jpg", Some("booru")),
            None
        );
        assert_eq!(
            import_category("fapachiexample.org/photo.jpg", Some("fapachi")),
            None
        );
        assert_eq!(
            import_category("example.org//photo.jpg", Some("")),
            Some(("directlink", "directlinkexample.org//photo.jpg".to_owned()))
        );
        assert_eq!(
            import_category("directlinkexample.org//photo.jpg", Some("directlink")),
            Some(("directlink", "directlinkexample.org//photo.jpg".to_owned()))
        );
    }

    #[test]
    fn foreign_prefix_cannot_migrate_an_unrelated_directlink_archive_hit() {
        let root =
            std::env::temp_dir().join(format!("galerith-import-isolation-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let source = root.join("foreign.sqlite");
        let native = root.join("native.sqlite");
        let db = Connection::open(&source).unwrap();
        db.execute_batch(
            "CREATE TABLE named(entry TEXT PRIMARY KEY) WITHOUT ROWID;
             INSERT INTO named VALUES ('fapachimodel_12345');
             INSERT INTO named VALUES ('fapachiexample.org/photos/item.jpg');
             INSERT INTO named VALUES ('12345');",
        )
        .unwrap();
        drop(db);
        let original = fs::read(&source).unwrap();
        let archive = Archive::open_with_reference_options(
            &native,
            Some(&source),
            Some("named"),
            Some("fapachi"),
        )
        .unwrap();
        let unrelated =
            extract_direct_media("https://example.org/photos/item.jpg?signature=rotated").unwrap();
        assert!(
            !archive.contains(&unrelated).unwrap(),
            "live read-only lookup must not confuse a foreign prefix with directlink"
        );
        assert_eq!(archive.import_reference().unwrap(), (1, 1));
        assert_eq!(archive.import_reference().unwrap(), (1, 0));
        drop(archive);
        assert_eq!(fs::read(&source).unwrap(), original);
        fs::remove_file(source).unwrap();

        let reopened = Archive::open(&native).unwrap();
        let fapachi = MediaItem {
            url: "https://fapachi.com/uploads/rotated.jpg".into(),
            filename: "rotated.jpg".into(),
            extractor: "fapachi",
            post_id: Some(12345),
        };
        assert!(reopened
            .contains_for_source(&fapachi, "https://fapachi.com/model/media/12345")
            .unwrap());
        let unrelated =
            extract_direct_media("https://example.org/photos/item.jpg?signature=rotated").unwrap();
        assert!(!reopened.contains(&unrelated).unwrap());
        drop(reopened);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn mixed_supported_archive_matrix_migrates_only_source_verified_keys() {
        let root =
            std::env::temp_dir().join(format!("galerith-import-matrix-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let foreign = root.join("gallery-dl.sqlite");
        let native = root.join("native.sqlite");
        let supported = [
            "directlinkexample.org//photo.jpg",
            "derpibooru123",
            "ponybooru123",
            "furbooru123",
            "danbooru123",
            "e621123",
            "e926123",
            "e6ai123",
            "yandere123",
            "konachan123",
            "sakugabooru123",
            "lolibooru123",
            "gelbooru123",
            "rule34123",
            "safebooru123",
            "tbib123",
            "hypnohub123",
            "xbooru123",
            "wallhaven94x38z",
            "4chana_123_456",
            "imgurabcdefg",
            "500px123",
            "photovogue123",
            "fapachimodel_123",
            "fapellophoto_model_123",
            "fapellovideo_model_123",
            "lightroomasset-A",
            "foriio12345/10/1",
            "piczel1001_0",
            "35photo12345_1",
            "unsplashabc-A",
            "lexicaabc-A",
            "pixeldrainFile12",
            "listal12345/full",
        ];
        let rejected = [
            "danbooru00123",
            "e6210",
            "4chan123",
            "4chana_0123_456",
            "500px0123",
            "photovogue0123",
            "fapachimodel_0123",
            "fapellophoto_model_0123",
            "lightroombad%2fid",
            "foriio12345/10/01",
            "piczel1001_00",
            "35photo12345_01",
            "unsplashbad%2fid",
            "lexicabad%2fid",
            "pixeldrainbad%2fid",
            "listal012345/full",
            "totally-unsupported-entry",
        ];
        let db = Connection::open(&foreign).unwrap();
        db.execute_batch("CREATE TABLE archive(entry TEXT PRIMARY KEY) WITHOUT ROWID;")
            .unwrap();
        for &entry in supported.iter().chain(rejected.iter()) {
            db.execute("INSERT INTO archive(entry) VALUES (?1)", [entry])
                .unwrap();
        }
        drop(db);
        let original = fs::read(&foreign).unwrap();

        let archive = Archive::open_with_reference(&native, Some(&foreign)).unwrap();
        assert_eq!(
            archive.import_reference().unwrap(),
            (supported.len(), supported.len())
        );
        assert_eq!(archive.import_reference().unwrap(), (supported.len(), 0));
        drop(archive);
        assert_eq!(fs::read(&foreign).unwrap(), original);
        fs::remove_file(foreign).unwrap();

        let archive = Archive::open(&native).unwrap();
        let direct =
            extract_direct_media("https://example.org/photo.jpg?signature=rotated-and-private")
                .unwrap();
        assert!(archive.contains(&direct).unwrap());
        for (item, source) in [
            (
                MediaItem {
                    url: "https://i.4cdn.org/a/456.jpg".into(),
                    filename: "456.jpg".into(),
                    extractor: "4chan",
                    post_id: Some(456),
                },
                "https://boards.4chan.org/a/thread/123",
            ),
            (
                MediaItem {
                    url: "https://img.500px.com/rotated.jpg".into(),
                    filename: "123.jpg".into(),
                    extractor: "500px",
                    post_id: Some(123),
                },
                "https://500px.com/photo/123",
            ),
            (
                MediaItem {
                    url: "https://assets.vogue.com/rotated.webp".into(),
                    filename: "123.webp".into(),
                    extractor: "photovogue",
                    post_id: Some(123),
                },
                "https://www.vogue.com/photovogue/photographers/777",
            ),
            (
                MediaItem {
                    url: "https://fapachi.com/uploads/rotated.jpg".into(),
                    filename: "model_123.jpg".into(),
                    extractor: "fapachi",
                    post_id: Some(123),
                },
                "https://fapachi.com/model/media/123",
            ),
            (
                MediaItem {
                    url: "https://assets.adobe.net/rotated.webp".into(),
                    filename: "0020_asset-A.webp".into(),
                    extractor: "lightroom",
                    post_id: None,
                },
                "https://lightroom.adobe.com/shares/abc",
            ),
            (
                MediaItem {
                    url: "https://foriio.imgix.net/store/first.png".into(),
                    filename: "01 10.png".into(),
                    extractor: "foriio",
                    post_id: Some(12345),
                },
                "https://foriio.com/works/12345",
            ),
            (
                MediaItem {
                    url: "https://cdn.piczel.tv/changed.jpg".into(),
                    filename: "piczel_1001_Aurora_00.jpg".into(),
                    extractor: "piczel",
                    post_id: Some(1001),
                },
                "https://piczel.tv/gallery/artist",
            ),
            (
                MediaItem {
                    url: "https://img.35photo.pro/one.jpg".into(),
                    filename: "12345_01.jpg".into(),
                    extractor: "35photo",
                    post_id: Some(12345),
                },
                "https://35photo.pro/photo_12345",
            ),
            (
                MediaItem {
                    url: "https://images.unsplash.com/new?ixid=changed".into(),
                    filename: "abc-A.jpg".into(),
                    extractor: "unsplash",
                    post_id: None,
                },
                "https://unsplash.com/@alice",
            ),
            (
                MediaItem {
                    url: "https://lexica-serve-encoded-images2.sharif.workers.dev/full_jpg/abc-A"
                        .into(),
                    filename: "abc-A.jpg".into(),
                    extractor: "lexica",
                    post_id: None,
                },
                "https://lexica.art/?q=night",
            ),
            (
                MediaItem {
                    url: "https://pixeldrain.com/api/file/File12?download".into(),
                    filename: "renamed.jpg".into(),
                    extractor: "pixeldrain",
                    post_id: None,
                },
                "https://pixeldrain.com/l/album_99",
            ),
            (
                MediaItem {
                    url: "https://cdn.lisimg.com/rotated/full.png".into(),
                    filename: "12345_full.png".into(),
                    extractor: "listal",
                    post_id: None,
                },
                "https://www.listal.com/viewimage/12345",
            ),
            (
                MediaItem {
                    url: "https://i.imgur.com/abcdefg.webp".into(),
                    filename: "abcdefg.webp".into(),
                    extractor: "imgur",
                    post_id: None,
                },
                "https://imgur.com/abcdefg",
            ),
        ] {
            assert!(
                archive.contains_for_source(&item, source).unwrap(),
                "imported source not recognized: {source}"
            );
        }
        let absent = extract_direct_media("https://example.org/missing.jpg").unwrap();
        assert!(!archive.contains(&absent).unwrap());
        drop(archive);
        let bytes = fs::read(&native).unwrap();
        for plaintext in [
            "example.org",
            "fapachimodel_123",
            "signature=rotated-and-private",
        ] {
            assert!(!bytes
                .windows(plaintext.len())
                .any(|chunk| chunk == plaintext.as_bytes()));
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_archive_import_rolls_back_all_native_rows() {
        let root =
            std::env::temp_dir().join(format!("galerith-import-rollback-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let native_path = root.join("native.sqlite");
        let source_path = root.join("malformed.sqlite");
        let foreign = Connection::open(&source_path).unwrap();
        // SQLite's ordinary TEXT PRIMARY KEY column can still accept BLOB
        // values. A malformed source must not leave a half-imported history.
        foreign
            .execute_batch(
                "CREATE TABLE archive(entry TEXT PRIMARY KEY);
                 INSERT INTO archive(entry) VALUES ('directlinkexample.org/valid.jpg');
                 INSERT INTO archive(entry) VALUES (X'FF');",
            )
            .unwrap();
        drop(foreign);
        let initial = fs::read(&source_path).unwrap();
        let native = Archive::open_with_reference(&native_path, Some(&source_path)).unwrap();
        assert!(native.import_reference().is_err());
        let count: i64 = native
            .connection
            .query_row("SELECT COUNT(*) FROM imported_gallery_dl", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0, "failed imports must be atomic");
        drop(native);
        assert_eq!(fs::read(&source_path).unwrap(), initial);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn imported_supported_keys_are_private_idempotent_and_survive_reference_removal() {
        let root = std::env::temp_dir().join(format!("galerith-import-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let native_path = root.join("native.sqlite");
        let upstream_path = root.join("gallery-dl.sqlite");
        let upstream = Connection::open(&upstream_path).unwrap();
        upstream
            .execute_batch("CREATE TABLE archive(entry TEXT PRIMARY KEY) WITHOUT ROWID;")
            .unwrap();
        for key in [
            "directlinkexample.org/photos/hello.jpg",
            "derpibooru123",
            "ponybooru321",
            "furbooru456",
            "danboorut_example_5",
            "not-supported",
        ] {
            upstream
                .execute("INSERT INTO archive(entry) VALUES (?1)", params![key])
                .unwrap();
        }
        drop(upstream);
        let bytes = fs::read(&upstream_path).unwrap();
        let archive = Archive::open_with_reference(&native_path, Some(&upstream_path)).unwrap();
        assert_eq!(archive.import_reference().unwrap(), (5, 5));
        assert_eq!(archive.import_reference().unwrap(), (5, 0));
        drop(archive);
        assert_eq!(fs::read(&upstream_path).unwrap(), bytes);
        fs::remove_file(&upstream_path).unwrap();
        let local = Archive::open(&native_path).unwrap();
        let url =
            extract_direct_media("https://example.org/photos/hello.jpg?signature=changed").unwrap();
        assert!(local.contains(&url).unwrap());
        for (category, id) in [("derpibooru", 123), ("ponybooru", 321), ("furbooru", 456)] {
            let item = MediaItem {
                url: format!("https://cdn.example.net/{id}.png?token=rotated"),
                filename: format!("{id}.png"),
                extractor: category,
                post_id: Some(id),
            };
            assert!(local.contains(&item).unwrap());
            let unrelated = MediaItem {
                post_id: Some(id + 1),
                ..item
            };
            assert!(!local.contains(&unrelated).unwrap());
        }
        let unsafe_media = MediaItem {
            url: "https://cdn.example.net/5.jpg".into(),
            filename: "5.jpg".into(),
            extractor: "danbooru",
            post_id: Some(5),
        };
        assert!(!local.contains(&unsafe_media).unwrap());
        drop(local);
        let native_bytes = fs::read(&native_path).unwrap();
        for forbidden in ["hello.jpg", "photos/", "example.org", "signature=changed"] {
            assert!(!native_bytes
                .windows(forbidden.len())
                .any(|part| part == forbidden.as_bytes()));
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn external_gallery_dl_archive_is_never_modified() {
        let root =
            std::env::temp_dir().join(format!("galerith-external-archive-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let native = root.join("native.sqlite");
        let reference = root.join("gallery-dl.sqlite");
        let original = Connection::open(&reference).unwrap();
        original
            .execute_batch("CREATE TABLE archive(entry TEXT PRIMARY KEY) WITHOUT ROWID;")
            .unwrap();
        original
            .execute(
                "INSERT INTO archive(entry) VALUES (?1)",
                params!["directlinkexample.org/gallery/known.jpg"],
            )
            .unwrap();
        drop(original);
        let original_bytes = fs::read(&reference).unwrap();

        let archive = Archive::open_with_reference(&native, Some(&reference)).unwrap();
        let known = extract_direct_media("https://example.org/gallery/known.jpg?sig=new").unwrap();
        assert!(archive.contains(&known).unwrap());
        let unknown = extract_direct_media("https://example.org/gallery/new.png").unwrap();
        assert!(!archive.contains(&unknown).unwrap());
        archive.record(&unknown).unwrap();
        assert!(archive.contains(&unknown).unwrap());
        drop(archive);
        assert_eq!(fs::read(&reference).unwrap(), original_bytes);
        let foreign =
            Connection::open_with_flags(&reference, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        assert!(!has_upstream_table(&Connection::open(&native).unwrap(), "archive").unwrap());
        assert!(has_upstream_table(&foreign, "archive").unwrap());
        let rows: i64 = foreign
            .query_row("SELECT COUNT(*) FROM archive", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, 1);
        drop(foreign);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn same_file_reference_is_rejected_without_modifying_database() {
        let root =
            std::env::temp_dir().join(format!("galerith-reference-alias-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let reference = root.join("gallery.sqlite");
        let alias = root.join("alias.sqlite");
        let hardlink = root.join("hardlink.sqlite");
        let original = Connection::open(&reference).unwrap();
        original
            .execute_batch("CREATE TABLE archive(entry TEXT PRIMARY KEY) WITHOUT ROWID;")
            .unwrap();
        drop(original);
        let original_bytes = fs::read(&reference).unwrap();

        assert!(Archive::open_with_reference(&reference, Some(&reference)).is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&reference, &alias).unwrap();
            assert!(Archive::open_with_reference(&alias, Some(&reference)).is_err());
            fs::hard_link(&reference, &hardlink).unwrap();
            assert!(Archive::open_with_reference(&hardlink, Some(&reference)).is_err());
        }
        assert_eq!(fs::read(&reference).unwrap(), original_bytes);
        let existing = Connection::open(&reference).unwrap();
        let native_table: i64 = existing
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='downloads')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(native_table, 0);
        drop(existing);
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn direct_booru_post_source_locks_rotate_with_the_post_id() {
        let root =
            std::env::temp_dir().join(format!("galerith-direct-post-lock-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let path = root.join("native.sqlite");
        let post_url = "https://e621.net/posts/88";
        let original = MediaItem {
            url: "https://static.e621.net/88.jpg?auth=first".into(),
            filename: "88.jpg".into(),
            extractor: "e621",
            post_id: Some(88),
        };
        let rotated = MediaItem {
            url: "https://static2.e621.net/88.jpg?auth=second".into(),
            ..original.clone()
        };
        assert_ne!(key(&original), key(&rotated));
        let first = Archive::open(&path).unwrap();
        let first_guard = first
            .lock_item_for_source(&original, post_url)
            .await
            .unwrap()
            .unwrap();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let other_path = path.clone();
        let worker = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async {
                let second = Archive::open(&other_path).unwrap();
                ready_tx.send(()).unwrap();
                let _guard = second
                    .lock_item_for_source(&rotated, post_url)
                    .await
                    .unwrap()
                    .unwrap();
                done_tx
                    .send(second.contains_for_source(&rotated, post_url).unwrap())
                    .unwrap();
            });
        });
        ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(done_rx.recv_timeout(Duration::from_millis(150)).is_err());
        first.record_for_source(&original, post_url).unwrap();
        drop(first_guard);
        assert!(done_rx.recv_timeout(Duration::from_secs(5)).unwrap());
        worker.join().unwrap();
        drop(first);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn booru_post_source_scope_prevents_feed_and_cross_site_false_matches() {
        let root =
            std::env::temp_dir().join(format!("galerith-direct-post-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let archive = Archive::open(&root.join("native.sqlite")).unwrap();
        let original = MediaItem {
            url: "https://static.e621.net/data/88.jpg?token=old".into(),
            filename: "88.jpg".into(),
            extractor: "e621",
            post_id: Some(88),
        };
        let rotated = MediaItem {
            url: "https://static.e621.net/data/88.jpg?token=new".into(),
            ..original.clone()
        };
        let direct = "https://e621.net/posts/88";
        assert!(!archive.contains_for_source(&rotated, direct).unwrap());
        archive.record_for_source(&original, direct).unwrap();
        assert!(archive.contains_for_source(&rotated, direct).unwrap());
        assert!(!archive
            .contains_for_source(&rotated, "https://e621.net/posts?tags=wolf")
            .unwrap());
        assert!(!archive
            .contains_for_source(&rotated, "https://e621.net/pools/88")
            .unwrap());
        assert!(!archive
            .contains_for_source(&rotated, "https://e621.net/posts/89")
            .unwrap());
        assert!(!archive
            .contains_for_source(&rotated, "https://e926.net/posts/88")
            .unwrap());
        assert!(!archive
            .contains_for_source(&rotated, "https://attacker.invalid/posts/88")
            .unwrap());
        assert!(archive
            .contains_for_source(&rotated, "https://e621.net/p/2o")
            .unwrap());
        let wrong_category = MediaItem {
            extractor: "danbooru",
            ..rotated.clone()
        };
        assert!(!archive
            .contains_for_source(&wrong_category, direct)
            .unwrap());
        assert!(!archive
            .contains_for_source(&wrong_category, "https://danbooru.donmai.us/posts/88")
            .unwrap());
        let wrong_post = MediaItem {
            post_id: Some(89),
            ..rotated
        };
        assert!(!archive.contains_for_source(&wrong_post, direct).unwrap());
        drop(archive);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn booru_post_exact_legacy_url_is_promoted_without_guessing_rotations() {
        let root = std::env::temp_dir().join(format!(
            "galerith-direct-post-legacy-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let archive = Archive::open(&root.join("native.sqlite")).unwrap();
        let item = MediaItem {
            url: "https://assets.example.org/danbooru/99.png?sig=first".into(),
            filename: "99.png".into(),
            extractor: "danbooru",
            post_id: Some(99),
        };
        let rotated = MediaItem {
            url: "https://assets.example.org/danbooru/99.png?sig=second".into(),
            ..item.clone()
        };
        let original_post = "https://danbooru.donmai.us/posts/99";
        archive.record(&item).unwrap();
        assert!(!archive
            .contains_for_source(&rotated, original_post)
            .unwrap());
        assert!(archive.contains_for_source(&item, original_post).unwrap());
        assert!(archive
            .contains_for_source(&rotated, original_post)
            .unwrap());
        assert!(!archive
            .contains_for_source(&rotated, "https://danbooru.donmai.us/posts?tags=rating:s")
            .unwrap());
        drop(archive);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn corrected_e926_category_keeps_exact_legacy_downloads() {
        let root =
            std::env::temp_dir().join(format!("galerith-e926-category-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let archive = Archive::open(&root.join("native.sqlite")).unwrap();
        let original = MediaItem {
            url: "https://static1.e926.net/data/901.jpg?sig=old".into(),
            filename: "901.jpg".into(),
            extractor: "e926",
            post_id: Some(901),
        };
        let rotated = MediaItem {
            url: "https://static1.e926.net/data/901.jpg?sig=new".into(),
            ..original.clone()
        };
        let post = "https://e926.net/posts/901";
        archive
            .connection
            .execute(
                "INSERT INTO downloads (category, identity) VALUES ('e621', ?1)",
                params![legacy_url_key(&original)],
            )
            .unwrap();
        assert!(!archive.contains_for_source(&rotated, post).unwrap());
        assert!(archive.contains_for_source(&original, post).unwrap());
        assert!(archive.contains_for_source(&rotated, post).unwrap());
        assert!(!archive
            .contains_for_source(&rotated, "https://e621.net/posts/901")
            .unwrap());
        drop(archive);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn e926_feed_legacy_recovery_requires_exact_url_and_verified_site() {
        let root =
            std::env::temp_dir().join(format!("galerith-e926-feed-legacy-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let archive = Archive::open(&root.join("native.sqlite")).unwrap();
        let media = MediaItem {
            url: "https://static1.e926.net/data/12.jpg?sig=old".into(),
            filename: "12.jpg".into(),
            extractor: "e926",
            post_id: Some(12),
        };
        let rotated = MediaItem {
            url: "https://static1.e926.net/data/12.jpg?sig=new".into(),
            ..media.clone()
        };
        archive
            .connection
            .execute(
                "INSERT INTO downloads (category, identity) VALUES ('e621', ?1)",
                params![legacy_url_key(&media)],
            )
            .unwrap();
        let feed = "https://e926.net/posts?tags=artist";
        assert!(!archive
            .contains_for_source(&media, "https://e621.net/posts?tags=artist")
            .unwrap());
        assert!(!archive
            .contains_for_source(&media, "https://user:pass@e926.net/posts?tags=artist")
            .unwrap());
        assert!(archive.contains_for_source(&media, feed).unwrap());
        assert!(archive.contains_for_source(&rotated, feed).unwrap());
        assert!(archive.contains_for_source(&media, feed).unwrap());
        let native: i64 = archive
            .connection
            .query_row(
                "SELECT COUNT(*) FROM downloads WHERE category = 'e926'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(native, 1);
        drop(archive);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pool_native_keys_do_not_collide_with_post_or_feed_keys() {
        let root = std::env::temp_dir().join(format!("galerith-pool-scope-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let archive = Archive::open(&root.join("native.sqlite")).unwrap();
        let item = MediaItem {
            url: "https://cdn.example.net/88.jpg?token=first".into(),
            filename: "88.jpg".into(),
            extractor: "e621",
            post_id: Some(88),
        };
        let rotated = MediaItem {
            url: "https://other.example.net/88.png?token=second".into(),
            ..item.clone()
        };
        let pool = "https://e621.net/pools/42";
        assert_eq!(booru_pool(&item, pool), Some((42, 88)));
        assert!(!archive.contains_for_source(&rotated, pool).unwrap());
        archive.record_for_source(&item, pool).unwrap();
        assert!(archive.contains_for_source(&rotated, pool).unwrap());
        assert!(archive
            .contains_for_source(&rotated, "https://e621.net/pool/show/42")
            .unwrap());
        for unrelated in [
            "https://e621.net/pools/43",
            "https://e621.net/posts/88",
            "https://e621.net/posts?tags=pool:42",
            "https://e621.net/favorites?user_id=42",
            "https://e926.net/pools/42",
        ] {
            assert!(!archive.contains_for_source(&rotated, unrelated).unwrap());
        }
        let other_post = MediaItem {
            post_id: Some(89),
            ..rotated
        };
        assert!(!archive.contains_for_source(&other_post, pool).unwrap());
        assert!(canonical_pool_archive_suffix("p_42_88"));
        for invalid in [
            "p_0_88",
            "p_42_0",
            "p_042_88",
            "p_42_088",
            "p_42_88_9",
            "t_dog_88",
        ] {
            assert!(!canonical_pool_archive_suffix(invalid));
        }
        drop(archive);
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn pool_archive_lock_coordinates_rotated_urls_across_processes() {
        let root = std::env::temp_dir().join(format!("galerith-pool-lock-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let db = root.join("native.sqlite");
        let pool = "https://e621.net/pools/42";
        let original = MediaItem {
            url: "https://cdn.example.net/88.jpg?sig=one".into(),
            filename: "88.jpg".into(),
            extractor: "e621",
            post_id: Some(88),
        };
        let rotated = MediaItem {
            url: "https://second-cdn.example.net/88.png?sig=two".into(),
            filename: "88.png".into(),
            ..original.clone()
        };
        assert_ne!(key(&original), key(&rotated));
        let first = Archive::open(&db).unwrap();
        let guard = first
            .lock_item_for_source(&original, pool)
            .await
            .unwrap()
            .unwrap();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let db_again = db.clone();
        let worker = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async {
                let second = Archive::open(&db_again).unwrap();
                ready_tx.send(()).unwrap();
                let _guard = second
                    .lock_item_for_source(&rotated, pool)
                    .await
                    .unwrap()
                    .unwrap();
                done_tx
                    .send(second.contains_for_source(&rotated, pool).unwrap())
                    .unwrap();
            });
        });
        ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(done_rx.recv_timeout(Duration::from_millis(150)).is_err());
        first.record_for_source(&original, pool).unwrap();
        drop(guard);
        assert!(done_rx.recv_timeout(Duration::from_secs(5)).unwrap());
        worker.join().unwrap();
        drop(first);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn imported_pool_ids_are_category_scoped_and_usable_offline() {
        let root =
            std::env::temp_dir().join(format!("galerith-pool-import-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let native = root.join("native.sqlite");
        let reference = root.join("gallery-dl.sqlite");
        let original = Connection::open(&reference).unwrap();
        original
            .execute_batch(
                "CREATE TABLE archive(entry TEXT PRIMARY KEY) WITHOUT ROWID;
                 INSERT INTO archive VALUES ('danboorup_42_88');
                 INSERT INTO archive VALUES ('e621p_42_88');
                 INSERT INTO archive VALUES ('e926p_42_88');
                 INSERT INTO archive VALUES ('e6aip_42_88');
                 INSERT INTO archive VALUES ('e621t_dog_88');",
            )
            .unwrap();
        drop(original);
        let unchanged = fs::read(&reference).unwrap();
        let archive = Archive::open_with_reference(&native, Some(&reference)).unwrap();
        assert_eq!(archive.import_reference().unwrap(), (5, 5));
        assert_eq!(archive.import_reference().unwrap(), (5, 0));
        for (category, host) in [
            ("danbooru", "danbooru.donmai.us"),
            ("e621", "e621.net"),
            ("e926", "e926.net"),
            ("e6ai", "e6ai.net"),
        ] {
            let item = MediaItem {
                url: "https://cdn.example.net/88.jpg?auth=rotated".into(),
                filename: "88.jpg".into(),
                extractor: category,
                post_id: Some(88),
            };
            assert!(archive
                .contains_for_source(&item, &format!("https://{host}/pools/42"))
                .unwrap());
            assert!(!archive
                .contains_for_source(&item, &format!("https://{host}/pools/43"))
                .unwrap());
            assert!(!archive
                .contains_for_source(&item, &format!("https://{host}/posts/88"))
                .unwrap());
        }
        drop(archive);
        assert_eq!(fs::read(&reference).unwrap(), unchanged);
        fs::remove_file(reference).unwrap();
        let offline = Archive::open(&native).unwrap();
        let item = MediaItem {
            url: "https://new.example.net/88.png?auth=new".into(),
            filename: "88.png".into(),
            extractor: "e926",
            post_id: Some(88),
        };
        assert!(offline
            .contains_for_source(&item, "https://e926.net/pools/42")
            .unwrap());
        assert!(!offline
            .contains_for_source(&item, "https://e621.net/pools/42")
            .unwrap());
        drop(offline);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn custom_pool_table_needs_matching_site_prefix() {
        let root =
            std::env::temp_dir().join(format!("galerith-pool-prefix-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let original = root.join("reference.sqlite");
        let source = Connection::open(&original).unwrap();
        source
            .execute_batch(
                "CREATE TABLE named(entry TEXT PRIMARY KEY) WITHOUT ROWID;
                 INSERT INTO named VALUES ('p_42_88');
                 INSERT INTO named VALUES ('e621p_42_88');",
            )
            .unwrap();
        drop(source);
        let item = MediaItem {
            url: "https://cdn.example.net/88.jpg".into(),
            filename: "88.jpg".into(),
            extractor: "e621",
            post_id: Some(88),
        };
        let ambiguous = Archive::open_with_reference_table(
            &root.join("ambiguous.sqlite"),
            Some(&original),
            "named",
        )
        .unwrap();
        assert_eq!(ambiguous.import_reference().unwrap(), (0, 0));
        assert!(!ambiguous
            .contains_for_source(&item, "https://e621.net/pools/42")
            .unwrap());
        drop(ambiguous);
        let target = root.join("target.sqlite");
        let correct = Archive::open_with_reference_options(
            &target,
            Some(&original),
            Some("named"),
            Some("e621"),
        )
        .unwrap();
        assert_eq!(correct.import_reference().unwrap(), (1, 1));
        assert!(correct
            .contains_for_source(&item, "https://e621.net/pools/42")
            .unwrap());
        drop(correct);
        fs::remove_file(original).unwrap();
        let native = Archive::open(&target).unwrap();
        assert!(native
            .contains_for_source(&item, "https://e621.net/pools/42")
            .unwrap());
        drop(native);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn favorite_group_archive_is_distinct_from_posts_pools_and_other_groups() {
        let root = std::env::temp_dir().join(format!("galerith-favgroup-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let db = root.join("native.sqlite");
        let archive = Archive::open(&db).unwrap();
        let original = MediaItem {
            url: "https://cdn.example.org/88.jpg?token=one".into(),
            filename: "88.jpg".into(),
            extractor: "danbooru",
            post_id: Some(88),
        };
        let rotated = MediaItem {
            url: "https://other.example.org/88.png?token=two".into(),
            ..original.clone()
        };
        let group = "https://danbooru.donmai.us/favorite_groups/77";
        assert_eq!(danbooru_favorite_group(&original, group), Some((77, 88)));
        archive.record_for_source(&original, group).unwrap();
        assert!(archive.contains_for_source(&rotated, group).unwrap());
        assert!(archive
            .contains_for_source(
                &rotated,
                "https://danbooru.donmai.us/favorite_group/show/77"
            )
            .unwrap());
        for unrelated in [
            "https://danbooru.donmai.us/favorite_groups/78",
            "https://danbooru.donmai.us/pools/77",
            "https://danbooru.donmai.us/posts/88",
            "https://danbooru.donmai.us/posts?tags=fox",
            "https://safebooru.donmai.us/favorite_groups/77",
        ] {
            assert!(!archive.contains_for_source(&rotated, unrelated).unwrap());
        }
        assert!(canonical_favorite_group_archive_suffix("fg_77_88"));
        for invalid in ["fg_0_88", "fg_77_0", "fg_077_88", "fg_77_088", "p_77_88"] {
            assert!(!canonical_favorite_group_archive_suffix(invalid));
        }
        drop(archive);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn imported_favorite_groups_are_available_offline_without_false_post_hits() {
        let root =
            std::env::temp_dir().join(format!("galerith-favgroup-import-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let native = root.join("native.sqlite");
        let foreign = root.join("reference.sqlite");
        let source = Connection::open(&foreign).unwrap();
        source
            .execute_batch(
                "CREATE TABLE archive(entry TEXT PRIMARY KEY) WITHOUT ROWID;
                 INSERT INTO archive VALUES ('danboorufg_77_88');
                 INSERT INTO archive VALUES ('danboorup_77_88');
                 INSERT INTO archive VALUES ('danboorut_dog_88');",
            )
            .unwrap();
        drop(source);
        let original_bytes = fs::read(&foreign).unwrap();
        let archive = Archive::open_with_reference(&native, Some(&foreign)).unwrap();
        assert_eq!(archive.import_reference().unwrap(), (3, 3));
        assert_eq!(archive.import_reference().unwrap(), (3, 0));
        drop(archive);
        assert_eq!(fs::read(&foreign).unwrap(), original_bytes);
        fs::remove_file(foreign).unwrap();
        let archive = Archive::open(&native).unwrap();
        let media = MediaItem {
            url: "https://new-cdn.example.org/88.png".into(),
            filename: "88.png".into(),
            extractor: "danbooru",
            post_id: Some(88),
        };
        assert!(archive
            .contains_for_source(&media, "https://danbooru.donmai.us/favorite_groups/77")
            .unwrap());
        assert!(archive
            .contains_for_source(&media, "https://danbooru.donmai.us/pools/77")
            .unwrap());
        assert!(!archive
            .contains_for_source(&media, "https://danbooru.donmai.us/posts/88")
            .unwrap());
        assert!(!archive
            .contains_for_source(&media, "https://danbooru.donmai.us/favorite_groups/78")
            .unwrap());
        drop(archive);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn account_specific_e621_favorites_do_not_confuse_users_posts_or_feeds() {
        let root = std::env::temp_dir().join(format!("galerith-e621-favs-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let archive = Archive::open(&root.join("native.sqlite")).unwrap();
        let original = MediaItem {
            url: "https://cdn.example.org/88.jpg?sig=first".into(),
            filename: "88.jpg".into(),
            extractor: "e621",
            post_id: Some(88),
        };
        let rotated = MediaItem {
            url: "https://other-cdn.example.org/88.png?sig=second".into(),
            ..original.clone()
        };
        let favorites = "https://e621.net/favorites?user_id=42";
        assert_eq!(e621_favorite(&original, favorites), Some((42, 88)));
        archive.record_for_source(&original, favorites).unwrap();
        assert!(archive.contains_for_source(&rotated, favorites).unwrap());
        assert!(archive
            .contains_for_source(&rotated, "https://e621.net/favorites?page=2&user_id=42")
            .unwrap());
        for other in [
            "https://e621.net/favorites?user_id=43",
            "https://e621.net/favorites",
            "https://e621.net/posts/88",
            "https://e621.net/pools/42",
            "https://e926.net/favorites?user_id=42",
            "https://attacker.example/favorites?user_id=42",
            "https://e621.net/favorites?user_id=042",
            "https://e621.net/favorites?user_id=42&user_id=43",
            "https://user:secret@e621.net/favorites?user_id=42",
        ] {
            assert!(
                !archive.contains_for_source(&rotated, other).unwrap(),
                "{other}"
            );
        }
        let different_post = MediaItem {
            post_id: Some(89),
            ..rotated
        };
        assert!(!archive
            .contains_for_source(&different_post, favorites)
            .unwrap());
        assert!(canonical_favorite_archive_suffix("f_42_88"));
        for key in [
            "f__88",
            "f_0_88",
            "f_42_0",
            "f_042_88",
            "f_42_088",
            "f_42_88_1",
        ] {
            assert!(!canonical_favorite_archive_suffix(key), "{key}");
        }
        drop(archive);
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn account_specific_favorite_lock_serializes_rotating_media_urls() {
        let root =
            std::env::temp_dir().join(format!("galerith-favorite-lock-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let db = root.join("native.sqlite");
        let favorites = "https://e621.net/favorites?user_id=42";
        let original = MediaItem {
            url: "https://first.example.org/88.jpg?sig=first".into(),
            filename: "88.jpg".into(),
            extractor: "e621",
            post_id: Some(88),
        };
        let rotated = MediaItem {
            url: "https://second.example.org/88.png?sig=second".into(),
            filename: "88.png".into(),
            ..original.clone()
        };
        assert_ne!(key(&original), key(&rotated));
        let first = Archive::open(&db).unwrap();
        let guard = first
            .lock_item_for_source(&original, favorites)
            .await
            .unwrap()
            .unwrap();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let other_db = db.clone();
        let worker = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async {
                let second = Archive::open(&other_db).unwrap();
                ready_tx.send(()).unwrap();
                let _guard = second
                    .lock_item_for_source(&rotated, favorites)
                    .await
                    .unwrap()
                    .unwrap();
                done_tx
                    .send(second.contains_for_source(&rotated, favorites).unwrap())
                    .unwrap();
            });
        });
        ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(done_rx.recv_timeout(Duration::from_millis(150)).is_err());
        first.record_for_source(&original, favorites).unwrap();
        drop(guard);
        assert!(done_rx.recv_timeout(Duration::from_secs(5)).unwrap());
        worker.join().unwrap();
        drop(first);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn imported_e621_favorites_remain_private_and_available_offline() {
        let root =
            std::env::temp_dir().join(format!("galerith-favs-import-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let native = root.join("native.sqlite");
        let foreign = root.join("reference.sqlite");
        let source = Connection::open(&foreign).unwrap();
        source
            .execute_batch(
                "CREATE TABLE archive(entry TEXT PRIMARY KEY) WITHOUT ROWID;
                 INSERT INTO archive VALUES ('e621f_42_88');
                 INSERT INTO archive VALUES ('e926f_42_88');
                 INSERT INTO archive VALUES ('e6aif_42_88');
                 INSERT INTO archive VALUES ('e621f__88');
                 INSERT INTO archive VALUES ('e621t_fox_88');",
            )
            .unwrap();
        drop(source);
        let source_bytes = fs::read(&foreign).unwrap();
        let archive = Archive::open_with_reference(&native, Some(&foreign)).unwrap();
        assert_eq!(archive.import_reference().unwrap(), (4, 4));
        assert_eq!(archive.import_reference().unwrap(), (4, 0));
        drop(archive);
        assert_eq!(fs::read(&foreign).unwrap(), source_bytes);
        fs::remove_file(foreign).unwrap();
        let saved = fs::read(&native).unwrap();
        assert!(!saved
            .windows(b"e621f_42_88".len())
            .any(|chunk| chunk == b"e621f_42_88"));
        let archive = Archive::open(&native).unwrap();
        for (category, host) in [
            ("e621", "e621.net"),
            ("e926", "e926.net"),
            ("e6ai", "e6ai.net"),
        ] {
            let media = MediaItem {
                url: "https://cdn.example.org/88.png?changed=1".into(),
                filename: "88.png".into(),
                extractor: category,
                post_id: Some(88),
            };
            assert!(archive
                .contains_for_source(&media, &format!("https://{host}/favorites?user_id=42"))
                .unwrap());
            assert!(!archive
                .contains_for_source(&media, &format!("https://{host}/favorites?user_id=43"))
                .unwrap());
            assert!(!archive
                .contains_for_source(&media, &format!("https://{host}/posts/88"))
                .unwrap());
        }
        drop(archive);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn named_favorite_table_requires_actual_matching_site_prefix() {
        let root =
            std::env::temp_dir().join(format!("galerith-favs-prefix-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let foreign = root.join("gallery-dl.sqlite");
        let source = Connection::open(&foreign).unwrap();
        source
            .execute_batch(
                "CREATE TABLE selected(entry TEXT PRIMARY KEY) WITHOUT ROWID;
                 INSERT INTO selected VALUES ('f_42_88');
                 INSERT INTO selected VALUES ('e621f_42_88');",
            )
            .unwrap();
        drop(source);
        let media = MediaItem {
            url: "https://cdn.example.net/new.jpg".into(),
            filename: "new.jpg".into(),
            extractor: "e621",
            post_id: Some(88),
        };
        let bare = Archive::open_with_reference_table(
            &root.join("bare.sqlite"),
            Some(&foreign),
            "selected",
        )
        .unwrap();
        assert_eq!(bare.import_reference().unwrap(), (0, 0));
        assert!(!bare
            .contains_for_source(&media, "https://e621.net/favorites?user_id=42")
            .unwrap());
        drop(bare);
        let native_path = root.join("native.sqlite");
        let native = Archive::open_with_reference_options(
            &native_path,
            Some(&foreign),
            Some("selected"),
            Some("e621"),
        )
        .unwrap();
        assert_eq!(native.import_reference().unwrap(), (1, 1));
        assert!(native
            .contains_for_source(&media, "https://e621.net/favorites?user_id=42")
            .unwrap());
        drop(native);
        fs::remove_file(foreign).unwrap();
        let offline = Archive::open(&native_path).unwrap();
        assert!(offline
            .contains_for_source(&media, "https://e621.net/favorites?user_id=42")
            .unwrap());
        drop(offline);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn tag_search_archive_identity_is_source_scoped_and_private() {
        let root = std::env::temp_dir().join(format!("galerith-tag-id-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let native = root.join("native.sqlite");
        let archive = Archive::open(&native).unwrap();
        let original = MediaItem {
            url: "https://old-cdn.example.org/88.jpg?sig=old".into(),
            filename: "88.jpg".into(),
            extractor: "e621",
            post_id: Some(88),
        };
        let rotated = MediaItem {
            url: "https://new-cdn.example.org/88.png?sig=new".into(),
            ..original.clone()
        };
        let query = "https://e621.net/posts?tags=blue+fox";
        assert_eq!(booru_tag_search(&original, query), Some("blue fox".into()));
        archive.record_for_source(&original, query).unwrap();
        assert!(archive.contains_for_source(&rotated, query).unwrap());
        for different in [
            "https://e621.net/posts?tags=blue+cat",
            "https://e621.net/posts?tags=blue%2Bfox",
            "https://e621.net/posts?tags=blue+fox&tags=blue+fox",
            "https://e621.net/posts/88",
            "https://e621.net/pools/88",
            "https://e621.net/favorites?user_id=42",
            "https://e926.net/posts?tags=blue+fox",
            "https://attacker.example/posts?tags=blue+fox",
        ] {
            assert!(!archive.contains_for_source(&rotated, different).unwrap());
        }
        let bytes = fs::read(&native).unwrap();
        for secret in [b"blue fox".as_slice(), b"sig=old".as_slice()] {
            assert!(!bytes.windows(secret.len()).any(|part| part == secret));
        }
        assert!(canonical_tag_archive_suffix("t_blue fox_88"));
        assert!(canonical_tag_archive_suffix("t__88"));
        for bad in ["t_cat_0", "t_cat_088", "t_cat_8_suffix", "f_42_88"] {
            assert!(!canonical_tag_archive_suffix(bad));
        }
        drop(archive);
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn tag_archive_locks_same_query_across_rotating_media_urls() {
        let root = std::env::temp_dir().join(format!("galerith-tag-lock-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let db = root.join("native.sqlite");
        let search = "https://e621.net/posts?tags=blue+fox";
        let original = MediaItem {
            url: "https://first.example.net/88.jpg?auth=first".into(),
            filename: "88.jpg".into(),
            extractor: "e621",
            post_id: Some(88),
        };
        let rotated = MediaItem {
            url: "https://second.example.net/88.png?auth=second".into(),
            filename: "88.png".into(),
            ..original.clone()
        };
        assert_ne!(key(&original), key(&rotated));
        let first = Archive::open(&db).unwrap();
        let guard = first
            .lock_item_for_source(&original, search)
            .await
            .unwrap()
            .unwrap();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let other_db = db.clone();
        let worker = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async {
                let second = Archive::open(&other_db).unwrap();
                ready_tx.send(()).unwrap();
                let _guard = second
                    .lock_item_for_source(&rotated, search)
                    .await
                    .unwrap()
                    .unwrap();
                done_tx
                    .send(second.contains_for_source(&rotated, search).unwrap())
                    .unwrap();
            });
        });
        ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(done_rx.recv_timeout(Duration::from_millis(150)).is_err());
        first.record_for_source(&original, search).unwrap();
        drop(guard);
        assert!(done_rx.recv_timeout(Duration::from_secs(5)).unwrap());
        worker.join().unwrap();
        drop(first);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn named_tag_archive_requires_actual_site_prefix() {
        let root = std::env::temp_dir().join(format!("galerith-tag-prefix-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let foreign = root.join("source.sqlite");
        let upstream = Connection::open(&foreign).unwrap();
        upstream
            .execute_batch(
                "CREATE TABLE custom(entry TEXT PRIMARY KEY) WITHOUT ROWID;
                 INSERT INTO custom VALUES ('t_blue fox_88');
                 INSERT INTO custom VALUES ('e621t_blue fox_88');",
            )
            .unwrap();
        drop(upstream);
        let before = fs::read(&foreign).unwrap();
        let media = MediaItem {
            url: "https://cdn.example.net/88.jpg".into(),
            filename: "88.jpg".into(),
            extractor: "e621",
            post_id: Some(88),
        };
        let search = "https://e621.net/posts?tags=blue+fox";
        let ambiguous = Archive::open_with_reference_table(
            &root.join("ambiguous.sqlite"),
            Some(&foreign),
            "custom",
        )
        .unwrap();
        assert_eq!(ambiguous.import_reference().unwrap(), (0, 0));
        assert!(!ambiguous.contains_for_source(&media, search).unwrap());
        drop(ambiguous);
        let native_path = root.join("native.sqlite");
        let supported = Archive::open_with_reference_options(
            &native_path,
            Some(&foreign),
            Some("custom"),
            Some("e621"),
        )
        .unwrap();
        assert!(supported.contains_for_source(&media, search).unwrap());
        assert_eq!(supported.import_reference().unwrap(), (1, 1));
        drop(supported);
        assert_eq!(fs::read(&foreign).unwrap(), before);
        fs::remove_file(foreign).unwrap();
        let offline = Archive::open(&native_path).unwrap();
        assert!(offline.contains_for_source(&media, search).unwrap());
        assert!(!offline
            .contains_for_source(&media, "https://e621.net/posts?tags=blue+cat")
            .unwrap());
        drop(offline);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn upstream_tag_archive_import_is_hashed_idempotent_and_offline_usable() {
        let root = std::env::temp_dir().join(format!("galerith-tag-import-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let foreign = root.join("reference.sqlite");
        let native = root.join("native.sqlite");
        let upstream = Connection::open(&foreign).unwrap();
        upstream
            .execute_batch(
                "CREATE TABLE archive(entry TEXT PRIMARY KEY) WITHOUT ROWID;
                 INSERT INTO archive VALUES ('danboorut_blue fox_88');
                 INSERT INTO archive VALUES ('e621t_blue fox_88');
                 INSERT INTO archive VALUES ('e926t_blue fox_88');
                 INSERT INTO archive VALUES ('e6ait_blue fox_88');
                 INSERT INTO archive VALUES ('e621f_42_88');",
            )
            .unwrap();
        drop(upstream);
        let source_bytes = fs::read(&foreign).unwrap();
        let archive = Archive::open_with_reference(&native, Some(&foreign)).unwrap();
        assert_eq!(archive.import_reference().unwrap(), (5, 5));
        assert_eq!(archive.import_reference().unwrap(), (5, 0));
        drop(archive);
        assert_eq!(fs::read(&foreign).unwrap(), source_bytes);
        fs::remove_file(foreign).unwrap();
        let saved = fs::read(&native).unwrap();
        assert!(!saved
            .windows(b"blue fox".len())
            .any(|part| part == b"blue fox"));
        let archive = Archive::open(&native).unwrap();
        for (category, host) in [
            ("danbooru", "danbooru.donmai.us"),
            ("e621", "e621.net"),
            ("e926", "e926.net"),
            ("e6ai", "e6ai.net"),
        ] {
            let media = MediaItem {
                url: "https://rotated.example.net/88.jpg".into(),
                filename: "88.jpg".into(),
                extractor: category,
                post_id: Some(88),
            };
            assert!(archive
                .contains_for_source(&media, &format!("https://{host}/posts?tags=blue+fox"))
                .unwrap());
            assert!(!archive
                .contains_for_source(&media, &format!("https://{host}/posts?tags=blue+cat"))
                .unwrap());
            assert!(!archive
                .contains_for_source(&media, &format!("https://{host}/posts/88"))
                .unwrap());
        }
        drop(archive);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn danbooru_media_asset_archive_does_not_match_post_or_other_asset() {
        let root =
            std::env::temp_dir().join(format!("galerith-asset-scope-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let archive = Archive::open(&root.join("native.sqlite")).unwrap();
        let original = MediaItem {
            url: "https://cdn.example.net/88.jpg?token=old".into(),
            filename: "88.jpg".into(),
            extractor: "danbooru",
            post_id: Some(88),
        };
        let rotated = MediaItem {
            url: "https://new.example.net/88.webp?token=new".into(),
            ..original.clone()
        };
        let source = "https://danbooru.donmai.us/media_assets/88";
        assert_eq!(danbooru_media_asset(&original, source), Some(88));
        archive.record_for_source(&original, source).unwrap();
        assert!(archive.contains_for_source(&rotated, source).unwrap());
        for unrelated in [
            "https://danbooru.donmai.us/media_assets/89",
            "https://danbooru.donmai.us/posts/88",
            "https://danbooru.donmai.us/pools/88",
            "https://danbooru.donmai.us/favorite_groups/88",
            "https://safebooru.donmai.us/media_assets/88",
            "https://attacker.example/media_assets/88",
        ] {
            assert!(!archive.contains_for_source(&rotated, unrelated).unwrap());
        }
        assert!(canonical_media_asset_archive_suffix("m88"));
        for invalid in ["m0", "m088", "m88_alt", "p_88_88", "m"] {
            assert!(!canonical_media_asset_archive_suffix(invalid));
        }
        drop(archive);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn danbooru_media_asset_reference_import_is_private_and_offline_safe() {
        let root =
            std::env::temp_dir().join(format!("galerith-asset-import-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let native = root.join("native.sqlite");
        let foreign = root.join("foreign.sqlite");
        let upstream = Connection::open(&foreign).unwrap();
        upstream
            .execute_batch(
                "CREATE TABLE archive(entry TEXT PRIMARY KEY) WITHOUT ROWID;
                 INSERT INTO archive VALUES ('danboorum88');
                 INSERT INTO archive VALUES ('danbooru88');
                 INSERT INTO archive VALUES ('danboorup_42_88');",
            )
            .unwrap();
        drop(upstream);
        let before = fs::read(&foreign).unwrap();
        let archive = Archive::open_with_reference(&native, Some(&foreign)).unwrap();
        assert_eq!(archive.import_reference().unwrap(), (3, 3));
        assert_eq!(archive.import_reference().unwrap(), (3, 0));
        drop(archive);
        assert_eq!(fs::read(&foreign).unwrap(), before);
        fs::remove_file(foreign).unwrap();
        let bytes = fs::read(&native).unwrap();
        assert!(!bytes
            .windows(b"danboorum88".len())
            .any(|part| part == b"danboorum88"));
        let archive = Archive::open(&native).unwrap();
        let item = MediaItem {
            url: "https://rotated.example.net/88.webp".into(),
            filename: "88.webp".into(),
            extractor: "danbooru",
            post_id: Some(88),
        };
        assert!(archive
            .contains_for_source(&item, "https://danbooru.donmai.us/media_assets/88")
            .unwrap());
        assert!(!archive
            .contains_for_source(&item, "https://danbooru.donmai.us/media_assets/89")
            .unwrap());
        drop(archive);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn day_month_popular_archive_scope_requires_explicit_valid_dates() {
        let root = std::env::temp_dir().join(format!("galerith-popular-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let archive = Archive::open(&root.join("native.sqlite")).unwrap();
        let original = MediaItem {
            url: "https://cdn.example.org/88.jpg?sig=old".into(),
            filename: "88.jpg".into(),
            extractor: "e621",
            post_id: Some(88),
        };
        let rotated = MediaItem {
            url: "https://other.example.org/88.png?sig=new".into(),
            ..original.clone()
        };
        let daily = "https://e621.net/explore/posts/popular?scale=day&date=2026-10-25";
        let monthly = "https://e621.net/explore/posts/popular?scale=month&date=2026-10-25";
        let weekly = "https://e621.net/explore/posts/popular?scale=week&date=2026-10-25";
        assert_eq!(
            booru_popular(&original, daily),
            Some("e621P_d_2026-10-25_88".into())
        );
        assert_eq!(
            booru_popular(&original, monthly),
            Some("e621P_m_2026-10_88".into())
        );
        assert_eq!(
            booru_popular(&original, weekly),
            Some("e621P_w_2026-10-19_88".into())
        );
        archive.record_for_source(&original, daily).unwrap();
        assert!(archive.contains_for_source(&rotated, daily).unwrap());
        for other in [
            monthly,
            "https://e621.net/explore/posts/popular?scale=day&date=2026-10-24",
            "https://e621.net/explore/posts/popular?scale=day",
            "https://e621.net/explore/posts/popular?scale=week&date=2026-10-25",
            "https://e621.net/explore/posts/popular?scale=day&date=2026-02-30",
            "https://e621.net/explore/posts/popular?scale=day&date=2026-10-25&date=2026-10-26",
            "https://e926.net/explore/posts/popular?scale=day&date=2026-10-25",
            "https://e621.net/posts/88",
        ] {
            assert!(!archive.contains_for_source(&rotated, other).unwrap());
        }
        assert!(validated_calendar_date("2024-02-29").is_some());
        assert!(validated_calendar_date("2025-02-29").is_none());
        assert!(validated_calendar_date("0000-01-01").is_none());
        assert!(canonical_popular_archive_suffix("P_d_2026-10-25_88"));
        assert!(canonical_popular_archive_suffix("P_m_2026-10_88"));
        assert!(canonical_popular_archive_suffix("P_w_2026-10-19_88"));
        assert_eq!(
            popular_week_monday("2026-10-25").as_deref(),
            Some("2026-10-19")
        );
        assert_eq!(
            popular_week_monday("2027-01-01").as_deref(),
            Some("2026-12-28")
        );
        assert_eq!(
            popular_week_monday("2024-03-01").as_deref(),
            Some("2024-02-26")
        );
        for invalid in [
            "P_d_2026-02-30_88",
            "P_m_2026-13_88",
            "P_w_2026-10-25_88",
            "P_d_2026-10-25_0",
            "P_d_2026-10-25_088",
        ] {
            assert!(!canonical_popular_archive_suffix(invalid));
        }
        drop(archive);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dated_popular_archive_import_stays_private_and_offline() {
        let root =
            std::env::temp_dir().join(format!("galerith-popular-import-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let native = root.join("native.sqlite");
        let foreign = root.join("foreign.sqlite");
        let upstream = Connection::open(&foreign).unwrap();
        upstream
            .execute_batch(
                "CREATE TABLE archive(entry TEXT PRIMARY KEY) WITHOUT ROWID;
                 INSERT INTO archive VALUES ('danbooruP_d_2026-10-25_88');
                 INSERT INTO archive VALUES ('e621P_m_2026-10_88');
                 INSERT INTO archive VALUES ('e926P_d_2026-10-25_88');
                 INSERT INTO archive VALUES ('e6aiP_m_2026-10_88');
                 INSERT INTO archive VALUES ('e621P_w_2026-10-19_88');",
            )
            .unwrap();
        drop(upstream);
        let bytes = fs::read(&foreign).unwrap();
        let archive = Archive::open_with_reference(&native, Some(&foreign)).unwrap();
        assert_eq!(archive.import_reference().unwrap(), (5, 5));
        assert_eq!(archive.import_reference().unwrap(), (5, 0));
        drop(archive);
        assert_eq!(fs::read(&foreign).unwrap(), bytes);
        fs::remove_file(foreign).unwrap();
        let saved = fs::read(&native).unwrap();
        assert!(!saved
            .windows(b"e621P_m_2026-10_88".len())
            .any(|part| part == b"e621P_m_2026-10_88"));
        let offline = Archive::open(&native).unwrap();
        let media = MediaItem {
            url: "https://cdn.example.org/88.jpg?sig=rotated".into(),
            filename: "88.jpg".into(),
            extractor: "e621",
            post_id: Some(88),
        };
        assert!(offline
            .contains_for_source(
                &media,
                "https://e621.net/explore/posts/popular?scale=month&date=2026-10-01"
            )
            .unwrap());
        assert!(!offline
            .contains_for_source(
                &media,
                "https://e621.net/explore/posts/popular?scale=day&date=2026-10-25"
            )
            .unwrap());
        assert!(offline
            .contains_for_source(
                &media,
                "https://e621.net/explore/posts/popular?scale=week&date=2026-10-25"
            )
            .unwrap());
        drop(offline);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn gelbooru_families_use_private_site_scoped_post_and_tag_archive_keys() {
        let root =
            std::env::temp_dir().join(format!("galerith-gel-archive-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let foreign = root.join("foreign.sqlite");
        let native = root.join("native.sqlite");
        let upstream = Connection::open(&foreign).unwrap();
        upstream
            .execute_batch("CREATE TABLE archive(entry TEXT PRIMARY KEY) WITHOUT ROWID;")
            .unwrap();
        for category in [
            "gelbooru",
            "rule34",
            "safebooru",
            "tbib",
            "hypnohub",
            "xbooru",
        ] {
            for suffix in ["88", "t_blue fox_88"] {
                upstream
                    .execute(
                        "INSERT INTO archive(entry) VALUES (?1)",
                        [format!("{category}{suffix}")],
                    )
                    .unwrap();
            }
        }
        drop(upstream);
        let foreign_bytes = fs::read(&foreign).unwrap();
        let archive = Archive::open_with_reference(&native, Some(&foreign)).unwrap();
        assert_eq!(archive.import_reference().unwrap(), (12, 12));
        assert_eq!(archive.import_reference().unwrap(), (12, 0));
        drop(archive);
        assert_eq!(fs::read(&foreign).unwrap(), foreign_bytes);
        fs::remove_file(foreign).unwrap();
        let stored = fs::read(&native).unwrap();
        assert!(!stored.windows(b"blue fox".len()).any(|x| x == b"blue fox"));
        let archive = Archive::open(&native).unwrap();
        for (category, host) in [
            ("gelbooru", "gelbooru.com"),
            ("rule34", "rule34.xxx"),
            ("safebooru", "safebooru.org"),
            ("tbib", "tbib.org"),
            ("hypnohub", "hypnohub.net"),
            ("xbooru", "xbooru.com"),
        ] {
            let media = MediaItem {
                url: format!("https://cdn.example.org/{category}/88.jpg?rotated=yes"),
                filename: "88.jpg".into(),
                extractor: category,
                post_id: Some(88),
            };
            for kind in ["view&id=88", "list&tags=blue+fox"] {
                let input = format!("https://{host}/index.php?page=post&s={kind}");
                assert!(
                    archive.contains_for_source(&media, &input).unwrap(),
                    "missing pinned category key for {input}"
                );
            }
            for route in [
                "/index.php?page=post&s=view&id=89",
                "/index.php?page=post&s=list&tags=blue+cat",
                "/index.php?page=post&s=list&tags=blue+fox&tags=blue+fox",
                "/index.php?page=pool&s=show&id=77",
            ] {
                assert!(!archive
                    .contains_for_source(&media, &format!("https://{host}{route}"))
                    .unwrap());
            }
            assert!(!archive
                .contains_for_source(
                    &media,
                    &format!("https://{host}:9443/index.php?page=post&s=view&id=88"),
                )
                .unwrap());
        }
        drop(archive);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn gelbooru_named_import_requires_matching_category_prefix() {
        let root = std::env::temp_dir().join(format!("galerith-gel-prefix-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let foreign = root.join("reference.sqlite");
        let upstream = Connection::open(&foreign).unwrap();
        upstream
            .execute_batch(
                "CREATE TABLE entries(entry TEXT PRIMARY KEY) WITHOUT ROWID;
                 INSERT INTO entries VALUES ('88');
                 INSERT INTO entries VALUES ('safebooru88');
                 INSERT INTO entries VALUES ('safeboorut_blue fox_88');",
            )
            .unwrap();
        drop(upstream);
        let media = MediaItem {
            url: "https://cdn.example.org/88.jpg?new=1".into(),
            filename: "88.jpg".into(),
            extractor: "safebooru",
            post_id: Some(88),
        };
        let no_prefix = Archive::open_with_reference_table(
            &root.join("ambiguous.sqlite"),
            Some(&foreign),
            "entries",
        )
        .unwrap();
        assert_eq!(no_prefix.import_reference().unwrap(), (0, 0));
        drop(no_prefix);
        let native = root.join("native.sqlite");
        let exact = Archive::open_with_reference_options(
            &native,
            Some(&foreign),
            Some("entries"),
            Some("safebooru"),
        )
        .unwrap();
        assert_eq!(exact.import_reference().unwrap(), (2, 2));
        assert!(exact
            .contains_for_source(
                &media,
                "https://safebooru.org/index.php?page=post&s=view&id=88",
            )
            .unwrap());
        drop(exact);
        fs::remove_file(foreign).unwrap();
        let offline = Archive::open(&native).unwrap();
        assert!(offline
            .contains_for_source(
                &media,
                "https://safebooru.org/index.php?page=post&s=list&tags=blue+fox",
            )
            .unwrap());
        drop(offline);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn moebooru_post_pool_and_tags_have_independent_private_archive_ids() {
        let root =
            std::env::temp_dir().join(format!("galerith-moe-archive-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let foreign = root.join("upstream.sqlite");
        let native = root.join("native.sqlite");
        let source = Connection::open(&foreign).unwrap();
        source
            .execute_batch("CREATE TABLE archive(entry TEXT PRIMARY KEY) WITHOUT ROWID;")
            .unwrap();
        for category in ["yandere", "konachan", "sakugabooru", "lolibooru"] {
            for suffix in ["88", "p_77_88", "t_blue fox_88"] {
                source
                    .execute(
                        "INSERT INTO archive(entry) VALUES (?1)",
                        [format!("{category}{suffix}")],
                    )
                    .unwrap();
            }
        }
        drop(source);
        let foreign_bytes = fs::read(&foreign).unwrap();
        let archive = Archive::open_with_reference(&native, Some(&foreign)).unwrap();
        assert_eq!(archive.import_reference().unwrap(), (12, 12));
        assert_eq!(archive.import_reference().unwrap(), (12, 0));
        drop(archive);
        assert_eq!(fs::read(&foreign).unwrap(), foreign_bytes);
        fs::remove_file(foreign).unwrap();
        let stored = fs::read(&native).unwrap();
        assert!(!stored.windows(b"blue fox".len()).any(|b| b == b"blue fox"));
        let archive = Archive::open(&native).unwrap();
        for (category, host) in [
            ("yandere", "yande.re"),
            ("konachan", "konachan.net"),
            ("sakugabooru", "www.sakugabooru.com"),
            ("lolibooru", "lolibooru.moe"),
        ] {
            let item = MediaItem {
                url: format!("https://new-cdn.example.org/{category}/88.jpg?rotated=1"),
                filename: "88.jpg".into(),
                extractor: category,
                post_id: Some(88),
            };
            for route in ["/post/show/88", "/pool/show/77", "/post?tags=blue+fox"] {
                let source_url = format!("https://{host}{route}");
                assert!(
                    archive.contains_for_source(&item, &source_url).unwrap(),
                    "missing imported archive identity for {source_url}"
                );
            }
            for unrelated in [
                "/post/show/89",
                "/pool/show/78",
                "/post?tags=blue+cat",
                "/post?tags=blue+fox&tags=blue+fox",
            ] {
                assert!(!archive
                    .contains_for_source(&item, &format!("https://{host}{unrelated}"))
                    .unwrap());
            }
            assert!(!archive
                .contains_for_source(&item, &format!("https://{host}:9443/post/show/88"))
                .unwrap());
        }
        drop(archive);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn moebooru_named_archive_requires_a_proven_category_prefix() {
        let root = std::env::temp_dir().join(format!("galerith-moe-prefix-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let foreign = root.join("source.sqlite");
        let db = Connection::open(&foreign).unwrap();
        db.execute_batch(
            "CREATE TABLE entries(entry TEXT PRIMARY KEY) WITHOUT ROWID;
             INSERT INTO entries VALUES ('88');
             INSERT INTO entries VALUES ('yandere88');
             INSERT INTO entries VALUES ('yanderep_77_88');",
        )
        .unwrap();
        drop(db);
        let item = MediaItem {
            url: "https://cdn.example.net/88.jpg?fresh=1".into(),
            filename: "88.jpg".into(),
            extractor: "yandere",
            post_id: Some(88),
        };
        let unprefixed = Archive::open_with_reference_table(
            &root.join("default.sqlite"),
            Some(&foreign),
            "entries",
        )
        .unwrap();
        assert_eq!(unprefixed.import_reference().unwrap(), (0, 0));
        assert!(!unprefixed
            .contains_for_source(&item, "https://yande.re/post/show/88")
            .unwrap());
        drop(unprefixed);
        let native = root.join("native.sqlite");
        let scoped = Archive::open_with_reference_options(
            &native,
            Some(&foreign),
            Some("entries"),
            Some("yandere"),
        )
        .unwrap();
        assert_eq!(scoped.import_reference().unwrap(), (2, 2));
        assert!(scoped
            .contains_for_source(&item, "https://yande.re/post/show/88")
            .unwrap());
        assert!(scoped
            .contains_for_source(&item, "https://yande.re/pool/show/77")
            .unwrap());
        drop(scoped);
        fs::remove_file(foreign).unwrap();
        let offline = Archive::open(&native).unwrap();
        assert!(offline
            .contains_for_source(&item, "https://yande.re/post/show/88")
            .unwrap());
        drop(offline);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn alternate_https_ports_cannot_reuse_scoped_booru_archive_ids() {
        let root =
            std::env::temp_dir().join(format!("galerith-bool-alt-port-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let archive = Archive::open(&root.join("native.sqlite")).unwrap();
        for (category, host, route) in [
            ("danbooru", "danbooru.donmai.us", "/posts/88"),
            ("danbooru", "danbooru.donmai.us", "/pools/77"),
            ("danbooru", "danbooru.donmai.us", "/favorite_groups/77"),
            ("danbooru", "danbooru.donmai.us", "/media_assets/88"),
            ("danbooru", "danbooru.donmai.us", "/posts?tags=blue+fox"),
            (
                "danbooru",
                "danbooru.donmai.us",
                "/explore/posts/popular?scale=week&date=2026-10-25",
            ),
            ("e621", "e621.net", "/posts/88"),
            ("e621", "e621.net", "/pools/77"),
            ("e621", "e621.net", "/favorites?user_id=42"),
            ("e621", "e621.net", "/posts?tags=blue+fox"),
            (
                "e621",
                "e621.net",
                "/explore/posts/popular?scale=month&date=2026-10-25",
            ),
            ("e926", "e926.net", "/favorites?user_id=42"),
            ("e6ai", "e6ai.net", "/pools/77"),
        ] {
            let regular = format!("https://{host}{route}");
            let alternate_port = format!("https://{host}:9443{route}");
            let item = MediaItem {
                url: format!("https://cdn.example.test/{category}/88.jpg?token=first"),
                filename: "88.jpg".into(),
                extractor: category,
                post_id: Some(88),
            };
            let rotated = MediaItem {
                url: format!("https://cdn.example.test/{category}/88.webp?token=second"),
                filename: "88.webp".into(),
                ..item.clone()
            };
            assert!(
                booru_archive_scope(&item, &regular).is_some(),
                "expected source-specific archive identity for {regular}"
            );
            assert!(
                booru_archive_scope(&rotated, &alternate_port).is_none(),
                "unexpected archive identity for {alternate_port}"
            );
            archive.record_for_source(&item, &regular).unwrap();
            assert!(
                !archive
                    .contains_for_source(&rotated, &alternate_port)
                    .unwrap(),
                "alternate port incorrectly matched {regular}"
            );
        }
        drop(archive);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn wallhaven_source_verified_identity_separates_extractors_and_users() {
        let root = std::env::temp_dir().join(format!("galerith-wallhaven-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let archive = Archive::open(&root.join("native.sqlite")).unwrap();
        let first = MediaItem {
            url: "https://w.wallhaven.cc/full/94/wallhaven-94x38z.jpg".into(),
            filename: "wallhaven-94x38z.jpg".into(),
            extractor: "wallhaven",
            post_id: None,
        };
        let converted = MediaItem {
            url: "https://w.wallhaven.cc/full/94/wallhaven-94x38z.png".into(),
            filename: "wallhaven-94x38z.png".into(),
            ..first.clone()
        };
        let post = "https://wallhaven.cc/w/94x38z";
        let search = "https://wallhaven.cc/search?q=blue+sky";
        let uploads = "https://wallhaven.cc/user/example-1/uploads";
        let collection = "https://wallhaven.cc/user/example-1/favorites/77";
        assert_eq!(
            wallhaven_archive_scope(&first, post).unwrap().1,
            "wallhaven94x38z"
        );
        assert_eq!(
            wallhaven_archive_scope(&first, search).unwrap().1,
            "wallhavens_blue sky_94x38z"
        );
        assert_eq!(
            wallhaven_archive_scope(&first, uploads).unwrap().1,
            "wallhavenu_example-1_94x38z"
        );
        archive.record_for_source(&first, post).unwrap();
        assert!(archive.contains_for_source(&converted, post).unwrap());
        assert!(archive.contains_for_source(&converted, collection).unwrap());
        assert!(!archive.contains_for_source(&converted, search).unwrap());
        assert!(!archive.contains_for_source(&converted, uploads).unwrap());
        archive.record_for_source(&first, search).unwrap();
        assert!(archive.contains_for_source(&converted, search).unwrap());
        assert!(!archive
            .contains_for_source(&converted, "https://wallhaven.cc/search?q=blue+moon")
            .unwrap());
        archive.record_for_source(&first, uploads).unwrap();
        assert!(archive.contains_for_source(&converted, uploads).unwrap());
        for unrelated in [
            "https://wallhaven.cc/user/another/uploads",
            "https://wallhaven.cc/user/example-1/favorites/77?other=1",
            "https://wallhaven.cc/w/94x38y",
            "https://attacker.invalid/w/94x38z",
            "http://wallhaven.cc/w/94x38z",
            "https://wallhaven.cc:9443/w/94x38z",
        ] {
            assert!(
                !archive.contains_for_source(&converted, unrelated).unwrap(),
                "{unrelated}"
            );
        }
        let renamed = MediaItem {
            filename: "my-custom-wallpaper.jpg".into(),
            ..first.clone()
        };
        assert!(wallhaven_archive_scope(&renamed, post).is_some());
        let mismatched = MediaItem {
            url: "https://w.wallhaven.cc/full/94/wallhaven-94x38y.jpg".into(),
            ..first.clone()
        };
        assert!(wallhaven_archive_scope(&mismatched, post).is_none());
        let outside = MediaItem {
            url: "https://attacker.invalid/full/94/wallhaven-94x38z.jpg".into(),
            ..first
        };
        assert!(wallhaven_archive_scope(&outside, post).is_none());
        let bytes = fs::read(root.join("native.sqlite")).unwrap();
        for sensitive in [b"blue sky".as_slice(), b"example-1".as_slice()] {
            assert!(!bytes
                .windows(sensitive.len())
                .any(|chunk| chunk == sensitive));
        }
        drop(archive);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn wallhaven_reference_import_is_idempotent_private_and_offline_reopenable() {
        let root =
            std::env::temp_dir().join(format!("galerith-wallhaven-import-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let native = root.join("native.sqlite");
        let source = root.join("source.sqlite");
        let connection = Connection::open(&source).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE archive(entry TEXT PRIMARY KEY) WITHOUT ROWID;
                 INSERT INTO archive VALUES ('wallhaven94x38z');
                 INSERT INTO archive VALUES ('wallhavens_blue sky_94x38z');
                 INSERT INTO archive VALUES ('wallhavenu_example-1_94x38z');
                 INSERT INTO archive VALUES ('wallhavens__94x38z');
                 INSERT INTO archive VALUES ('wallhaven123');",
            )
            .unwrap();
        drop(connection);
        let original = fs::read(&source).unwrap();
        let archive = Archive::open_with_reference(&native, Some(&source)).unwrap();
        assert_eq!(archive.import_reference().unwrap(), (3, 3));
        assert_eq!(archive.import_reference().unwrap(), (3, 0));
        let media = MediaItem {
            url: "https://w.wallhaven.cc/full/94/wallhaven-94x38z.png".into(),
            filename: "wallhaven-94x38z.png".into(),
            extractor: "wallhaven",
            post_id: None,
        };
        assert!(archive
            .contains_for_source(&media, "https://wallhaven.cc/search?q=blue+sky")
            .unwrap());
        assert!(!archive
            .contains_for_source(&media, "https://wallhaven.cc/search?q=blue+moon")
            .unwrap());
        drop(archive);
        assert_eq!(fs::read(&source).unwrap(), original);
        fs::remove_file(source).unwrap();
        let reopened = Archive::open(&native).unwrap();
        for input in [
            "https://wallhaven.cc/w/94x38z",
            "https://wallhaven.cc/search?q=blue+sky",
            "https://wallhaven.cc/user/example-1/uploads",
        ] {
            assert!(reopened.contains_for_source(&media, input).unwrap());
        }
        assert!(!reopened
            .contains_for_source(&media, "https://wallhaven.cc/user/example-2/uploads")
            .unwrap());
        drop(reopened);
        let stored = fs::read(&native).unwrap();
        for sensitive in [b"blue sky".as_slice(), b"example-1".as_slice()] {
            assert!(!stored
                .windows(sensitive.len())
                .any(|chunk| chunk == sensitive));
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn wallhaven_named_reference_requires_real_matching_category_prefix() {
        let root =
            std::env::temp_dir().join(format!("galerith-wallhaven-prefix-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let foreign = root.join("source.sqlite");
        let original = Connection::open(&foreign).unwrap();
        original
            .execute_batch(
                "CREATE TABLE selected(entry TEXT PRIMARY KEY) WITHOUT ROWID;
                 INSERT INTO selected VALUES ('94x38z');
                 INSERT INTO selected VALUES ('wallhaven94x38z');
                 INSERT INTO selected VALUES ('s_blue sky_94x38z');
                 INSERT INTO selected VALUES ('wallhavens_blue sky_94x38z');",
            )
            .unwrap();
        drop(original);
        let before = fs::read(&foreign).unwrap();
        let item = MediaItem {
            url: "https://w.wallhaven.cc/full/94/wallhaven-94x38z.jpg".into(),
            filename: "wallhaven-94x38z.jpg".into(),
            extractor: "wallhaven",
            post_id: None,
        };
        let ambiguous = Archive::open_with_reference_table(
            &root.join("ambiguous.sqlite"),
            Some(&foreign),
            "selected",
        )
        .unwrap();
        assert_eq!(ambiguous.import_reference().unwrap(), (0, 0));
        assert!(!ambiguous
            .contains_for_source(&item, "https://wallhaven.cc/w/94x38z")
            .unwrap());
        drop(ambiguous);
        let path = root.join("native.sqlite");
        let supported = Archive::open_with_reference_options(
            &path,
            Some(&foreign),
            Some("selected"),
            Some("wallhaven"),
        )
        .unwrap();
        assert_eq!(supported.import_reference().unwrap(), (2, 2));
        assert!(supported
            .contains_for_source(&item, "https://wallhaven.cc/w/94x38z")
            .unwrap());
        assert!(supported
            .contains_for_source(&item, "https://wallhaven.cc/search?q=blue+sky")
            .unwrap());
        drop(supported);
        assert_eq!(fs::read(&foreign).unwrap(), before);
        fs::remove_file(foreign).unwrap();
        let reopened = Archive::open(&path).unwrap();
        assert!(reopened
            .contains_for_source(&item, "https://wallhaven.cc/search?q=blue+sky")
            .unwrap());
        drop(reopened);
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn wallhaven_wallpaper_archive_lock_handles_cdn_extension_rotation() {
        let root =
            std::env::temp_dir().join(format!("galerith-wallhaven-lock-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let db = root.join("native.sqlite");
        let image = "https://wallhaven.cc/w/94x38z";
        let original = MediaItem {
            url: "https://w.wallhaven.cc/full/94/wallhaven-94x38z.jpg".into(),
            filename: "wallhaven-94x38z.jpg".into(),
            extractor: "wallhaven",
            post_id: None,
        };
        let rotated = MediaItem {
            url: "https://w.wallhaven.cc/full/94/wallhaven-94x38z.png".into(),
            filename: "wallhaven-94x38z.png".into(),
            ..original.clone()
        };
        assert_ne!(key(&original), key(&rotated));
        let first = Archive::open(&db).unwrap();
        let guard = first
            .lock_item_for_source(&original, image)
            .await
            .unwrap()
            .unwrap();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let other_db = db.clone();
        let worker = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async {
                let other = Archive::open(&other_db).unwrap();
                ready_tx.send(()).unwrap();
                let _guard = other
                    .lock_item_for_source(&rotated, image)
                    .await
                    .unwrap()
                    .unwrap();
                done_tx
                    .send(other.contains_for_source(&rotated, image).unwrap())
                    .unwrap();
            });
        });
        ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(done_rx.recv_timeout(Duration::from_millis(150)).is_err());
        first.record_for_source(&original, image).unwrap();
        drop(guard);
        assert!(done_rx.recv_timeout(Duration::from_secs(5)).unwrap());
        worker.join().unwrap();
        drop(first);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn exact_same_media_url_in_different_scopes_cannot_false_skip() {
        let root =
            std::env::temp_dir().join(format!("galerith-scope-collision-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let db = root.join("archive.sqlite");
        let first = MediaItem {
            url: "https://cdn.example.net/88.jpg".into(),
            filename: "88.jpg".into(),
            extractor: "e621",
            post_id: Some(88),
        };
        let pool = "https://e621.net/pools/77";
        let other_pool = "https://e621.net/pools/78";
        let tag = "https://e621.net/posts?tags=wolf";
        let archive = Archive::open(&db).unwrap();
        archive.record_for_source(&first, pool).unwrap();
        assert!(archive.contains_for_source(&first, pool).unwrap());
        assert!(!archive.contains_for_source(&first, other_pool).unwrap());
        assert!(!archive.contains_for_source(&first, tag).unwrap());
        assert!(!archive
            .contains_for_source(&first, "https://e621.net/posts/88")
            .unwrap());
        assert!(!archive.contains(&first).unwrap());
        archive.record_for_source(&first, tag).unwrap();
        assert!(archive.contains_for_source(&first, tag).unwrap());
        assert!(!archive.contains_for_source(&first, other_pool).unwrap());
        let count: i64 = archive
            .connection
            .query_row("SELECT COUNT(*) FROM downloads", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            count, 2,
            "source-scoped records cannot create URL-only keys"
        );
        drop(archive);

        let wallpaper = MediaItem {
            url: "https://w.wallhaven.cc/full/94/wallhaven-94x38z.jpg".into(),
            filename: "wallhaven-94x38z.jpg".into(),
            extractor: "wallhaven",
            post_id: None,
        };
        let reopened = Archive::open(&db).unwrap();
        let search = "https://wallhaven.cc/search?q=blue+sky";
        reopened.record_for_source(&wallpaper, search).unwrap();
        assert!(reopened.contains_for_source(&wallpaper, search).unwrap());
        assert!(!reopened
            .contains_for_source(&wallpaper, "https://wallhaven.cc/search?q=blue+moon")
            .unwrap());
        assert!(!reopened
            .contains_for_source(&wallpaper, "https://wallhaven.cc/user/someone/uploads")
            .unwrap());
        drop(reopened);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn old_mixed_scope_archive_fails_safe_without_promoting_url_hash() {
        let root =
            std::env::temp_dir().join(format!("galerith-ambiguous-scope-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let archive = Archive::open(&root.join("native.sqlite")).unwrap();
        let media = MediaItem {
            url: "https://cdn.example.org/99.jpg".into(),
            filename: "99.jpg".into(),
            extractor: "danbooru",
            post_id: Some(99),
        };
        archive
            .connection
            .execute(
                "INSERT INTO downloads(category, identity) VALUES (?1, ?2)",
                params![media.extractor, key(&media)],
            )
            .unwrap();
        assert!(archive
            .contains_for_source(&media, "https://danbooru.donmai.us/posts/99")
            .unwrap());
        // Older releases wrote BOTH stable and URL-hash rows for a completed
        // scoped file. The URL-only key is now ambiguous across source classes.
        let another = MediaItem {
            url: "https://cdn.example.org/101.jpg".into(),
            filename: "101.jpg".into(),
            post_id: Some(101),
            ..media.clone()
        };
        archive
            .record_for_source(&another, "https://danbooru.donmai.us/posts/101")
            .unwrap();
        assert!(!archive
            .contains_for_source(&media, "https://danbooru.donmai.us/posts?tags=cat")
            .unwrap());
        drop(archive);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fourchan_scope_separates_threads_and_boards_with_identical_cdn_media() {
        let root = std::env::temp_dir().join(format!("galerith-fourchan-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let archive = Archive::open(&root.join("native.sqlite")).unwrap();
        let original = MediaItem {
            url: "https://i.4cdn.org/a/456.jpg".into(),
            filename: "456 synthetic.jpg".into(),
            extractor: "4chan",
            post_id: Some(456),
        };
        let first = "https://boards.4channel.org/a/thread/123";
        let second = "https://boards.4chan.org/a/thread/124";
        assert_eq!(
            fourchan_archive_scope(&original, first).unwrap().1,
            "4chana_123_456"
        );
        archive.record_for_source(&original, first).unwrap();
        assert!(archive.contains_for_source(&original, first).unwrap());
        assert!(archive
            .contains_for_source(&original, "https://boards.4chan.org/a/thread/123/synthetic")
            .unwrap());
        assert!(!archive.contains_for_source(&original, second).unwrap());
        for unrelated in [
            "https://boards.4chan.org/b/thread/123",
            "https://boards.4chan.org/a/thread/125",
            "https://boards.4chan.org/a/123",
            "https://boards.4chan.org/a/thread/000123",
            "https://boards.4chan.org/a/thread/123?foo=bar",
            "https://attacker.invalid/a/thread/123",
            "http://boards.4chan.org/a/thread/123",
            "https://boards.4chan.org:9443/a/thread/123",
        ] {
            assert!(!archive.contains_for_source(&original, unrelated).unwrap());
        }
        let another = MediaItem {
            url: "https://i.4cdn.org/a/457.jpg".into(),
            post_id: Some(457),
            ..original.clone()
        };
        assert!(!archive.contains_for_source(&another, first).unwrap());
        let forged = MediaItem {
            url: "https://i.4cdn.org/b/456.jpg".into(),
            ..original.clone()
        };
        assert!(fourchan_archive_scope(&forged, first).is_none());
        let wrong_tim = MediaItem {
            post_id: Some(457),
            ..original.clone()
        };
        assert!(fourchan_archive_scope(&wrong_tim, first).is_none());
        assert!(canonical_fourchan_archive_suffix("a_123_456"));
        for malformed in [
            "a_0_456",
            "a_123_0",
            "a_0123_456",
            "a_123_0456",
            "A_123_456",
            "a_123_456_7",
        ] {
            assert!(!canonical_fourchan_archive_suffix(malformed));
        }
        drop(archive);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fourchan_offline_import_respects_thread_and_board_context() {
        let root =
            std::env::temp_dir().join(format!("galerith-fourchan-import-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let foreign = root.join("reference.sqlite");
        let native = root.join("native.sqlite");
        let source = Connection::open(&foreign).unwrap();
        source
            .execute_batch(
                "CREATE TABLE archive(entry TEXT PRIMARY KEY) WITHOUT ROWID;
                 INSERT INTO archive VALUES ('4chana_123_456');
                 INSERT INTO archive VALUES ('4chanb_123_456');
                 INSERT INTO archive VALUES ('4chana_124_456');
                 INSERT INTO archive VALUES ('4chana_0123_456');
                 INSERT INTO archive VALUES ('4chana_123_0');",
            )
            .unwrap();
        drop(source);
        let original = fs::read(&foreign).unwrap();
        let archive = Archive::open_with_reference(&native, Some(&foreign)).unwrap();
        assert_eq!(archive.import_reference().unwrap(), (3, 3));
        assert_eq!(archive.import_reference().unwrap(), (3, 0));
        drop(archive);
        assert_eq!(fs::read(&foreign).unwrap(), original);
        fs::remove_file(foreign).unwrap();
        let archive = Archive::open(&native).unwrap();
        let item = MediaItem {
            url: "https://i.4cdn.org/a/456.jpg".into(),
            filename: "456 example.jpg".into(),
            extractor: "4chan",
            post_id: Some(456),
        };
        for thread in [123, 124] {
            assert!(archive
                .contains_for_source(
                    &item,
                    &format!("https://boards.4chan.org/a/thread/{thread}")
                )
                .unwrap());
        }
        assert!(!archive
            .contains_for_source(&item, "https://boards.4chan.org/a/thread/125")
            .unwrap());
        assert!(!archive
            .contains_for_source(&item, "https://boards.4chan.org/b/thread/123")
            .unwrap());
        drop(archive);
        let bytes = fs::read(&native).unwrap();
        assert!(!bytes
            .windows(b"4chana_123_456".len())
            .any(|part| part == b"4chana_123_456"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fourchan_numeric_foreign_entries_are_not_counted_as_migratable() {
        let root =
            std::env::temp_dir().join(format!("galerith-fourchan-numeric-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let source = root.join("gallery-dl.sqlite");
        let db = Connection::open(&source).unwrap();
        db.execute_batch(
            "CREATE TABLE archive(entry TEXT PRIMARY KEY) WITHOUT ROWID;
             INSERT INTO archive VALUES ('4chan123');
             INSERT INTO archive VALUES ('4chana_123_456');
             CREATE TABLE named(entry TEXT PRIMARY KEY) WITHOUT ROWID;
             INSERT INTO named VALUES ('123');
             INSERT INTO named VALUES ('a_123_456');
             INSERT INTO named VALUES ('4chana_123_456');",
        )
        .unwrap();
        drop(db);
        let original = fs::read(&source).unwrap();

        let default =
            Archive::open_with_reference(&root.join("default.sqlite"), Some(&source)).unwrap();
        assert_eq!(default.import_reference().unwrap(), (1, 1));
        assert_eq!(default.import_reference().unwrap(), (1, 0));
        drop(default);
        let custom = Archive::open_with_reference_options(
            &root.join("custom.sqlite"),
            Some(&source),
            Some("named"),
            Some("4chan"),
        )
        .unwrap();
        assert_eq!(custom.import_reference().unwrap(), (1, 1));
        assert_eq!(custom.import_reference().unwrap(), (1, 0));
        drop(custom);
        assert_eq!(fs::read(&source).unwrap(), original);
        fs::remove_file(source).unwrap();

        let item = MediaItem {
            url: "https://i.4cdn.org/a/456.jpg".into(),
            filename: "456.jpg".into(),
            extractor: "4chan",
            post_id: Some(456),
        };
        for path in ["default.sqlite", "custom.sqlite"] {
            let archive = Archive::open(&root.join(path)).unwrap();
            assert!(archive
                .contains_for_source(&item, "https://boards.4chan.org/a/thread/123")
                .unwrap());
            assert!(!archive
                .contains_for_source(&item, "https://boards.4chan.org/a/thread/124")
                .unwrap());
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fourchan_named_archive_table_requires_genuine_4chan_prefix() {
        let root =
            std::env::temp_dir().join(format!("galerith-fourchan-named-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let foreign = root.join("reference.sqlite");
        let original = Connection::open(&foreign).unwrap();
        original
            .execute_batch(
                "CREATE TABLE named(entry TEXT PRIMARY KEY) WITHOUT ROWID;
                 INSERT INTO named VALUES ('a_123_456');
                 INSERT INTO named VALUES ('4chana_123_456');",
            )
            .unwrap();
        drop(original);
        let foreign_bytes = fs::read(&foreign).unwrap();
        let item = MediaItem {
            url: "https://i.4cdn.org/a/456.jpg".into(),
            filename: "456 fake.jpg".into(),
            extractor: "4chan",
            post_id: Some(456),
        };
        let input = "https://boards.4chan.org/a/thread/123";
        let unknown = Archive::open_with_reference_table(
            &root.join("unknown.sqlite"),
            Some(&foreign),
            "named",
        )
        .unwrap();
        assert_eq!(unknown.import_reference().unwrap(), (0, 0));
        assert!(!unknown.contains_for_source(&item, input).unwrap());
        drop(unknown);
        let native = root.join("native.sqlite");
        let valid = Archive::open_with_reference_options(
            &native,
            Some(&foreign),
            Some("named"),
            Some("4chan"),
        )
        .unwrap();
        assert_eq!(valid.import_reference().unwrap(), (1, 1));
        assert!(valid.contains_for_source(&item, input).unwrap());
        drop(valid);
        assert_eq!(fs::read(&foreign).unwrap(), foreign_bytes);
        fs::remove_file(foreign).unwrap();
        let offline = Archive::open(&native).unwrap();
        assert!(offline.contains_for_source(&item, input).unwrap());
        drop(offline);
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn fourchan_thread_archive_lock_serializes_same_tim_in_same_thread() {
        let root =
            std::env::temp_dir().join(format!("galerith-fourchan-lock-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let db = root.join("native.sqlite");
        let thread = "https://boards.4chan.org/a/thread/123";
        let first_item = MediaItem {
            url: "https://i.4cdn.org/a/456.jpg".into(),
            filename: "456 red.jpg".into(),
            extractor: "4chan",
            post_id: Some(456),
        };
        let next_item = MediaItem {
            url: "https://i.4cdn.org/a/456.png".into(),
            filename: "456 red.png".into(),
            ..first_item.clone()
        };
        assert_ne!(key(&first_item), key(&next_item));
        let first = Archive::open(&db).unwrap();
        let guard = first
            .lock_item_for_source(&first_item, thread)
            .await
            .unwrap()
            .unwrap();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let db_again = db.clone();
        let worker = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async {
                let second = Archive::open(&db_again).unwrap();
                ready_tx.send(()).unwrap();
                let _guard = second
                    .lock_item_for_source(&next_item, thread)
                    .await
                    .unwrap()
                    .unwrap();
                done_tx
                    .send(second.contains_for_source(&next_item, thread).unwrap())
                    .unwrap();
            });
        });
        ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(done_rx.recv_timeout(Duration::from_millis(150)).is_err());
        first.record_for_source(&first_item, thread).unwrap();
        drop(guard);
        assert!(done_rx.recv_timeout(Duration::from_secs(5)).unwrap());
        worker.join().unwrap();
        drop(first);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fivehundredpx_single_photo_id_survives_changed_cdn_url_without_cross_matches() {
        let root = std::env::temp_dir().join(format!("galerith-500px-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let archive = Archive::open(&root.join("native.sqlite")).unwrap();
        let original = MediaItem {
            url: "https://img.500px.com/original.jpg?sig=one".into(),
            filename: "12345.jpg".into(),
            extractor: "500px",
            post_id: Some(12345),
        };
        let rotated = MediaItem {
            url: "https://drscdn.500px.org/changed.webp?sig=two".into(),
            filename: "12345.webp".into(),
            ..original.clone()
        };
        let source = "https://500px.com/photo/12345/Golden-Landscape";
        assert_eq!(
            fivehundredpx_archive_scope(&original, source).unwrap().1,
            "500px12345"
        );
        assert_ne!(key(&original), key(&rotated));
        archive.record_for_source(&original, source).unwrap();
        assert!(archive.contains_for_source(&rotated, source).unwrap());
        assert!(archive
            .contains_for_source(&rotated, "https://web.500px.com/photo/12345")
            .unwrap());
        for invalid in [
            "https://500px.com/photo/12346",
            "https://500px.com/photo/12345?secret=1",
            "https://500px.com:8443/photo/12345",
            "https://other.invalid/photo/12345",
            "https://500px.com/alice/galleries/12345",
        ] {
            assert!(fivehundredpx_archive_scope(&rotated, invalid).is_none());
        }
        let other = MediaItem {
            post_id: Some(12346),
            ..rotated.clone()
        };
        assert!(!archive
            .contains_for_source(&other, "https://500px.com/photo/12346")
            .unwrap());
        let attacker = MediaItem {
            url: "https://fake500px.com/photo/12345.jpg".into(),
            ..rotated
        };
        assert!(fivehundredpx_archive_scope(&attacker, source).is_none());
        for invalid in ["0", "012345", "123abc", "18446744073709551616"] {
            assert!(!canonical_500px_archive_suffix(invalid));
        }
        drop(archive);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fivehundredpx_import_is_idempotent_private_and_works_without_reference() {
        let root =
            std::env::temp_dir().join(format!("galerith-500px-import-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let source = root.join("foreign.sqlite");
        let reference = Connection::open(&source).unwrap();
        reference
            .execute_batch(
                "CREATE TABLE archive(entry TEXT PRIMARY KEY) WITHOUT ROWID;
             INSERT INTO archive VALUES ('500px12345');
             INSERT INTO archive VALUES ('500px012345');
             INSERT INTO archive VALUES ('500px0');
             INSERT INTO archive VALUES ('500px12346');",
            )
            .unwrap();
        drop(reference);
        let before = fs::read(&source).unwrap();
        let native = root.join("native.sqlite");
        let archive = Archive::open_with_reference(&native, Some(&source)).unwrap();
        assert_eq!(archive.import_reference().unwrap(), (2, 2));
        assert_eq!(archive.import_reference().unwrap(), (2, 0));
        drop(archive);
        assert_eq!(fs::read(&source).unwrap(), before);
        fs::remove_file(source).unwrap();
        let offline = Archive::open(&native).unwrap();
        let photo = MediaItem {
            url: "https://drscdn.500px.org/rotated.jpg?token=secret".into(),
            filename: "12345.jpg".into(),
            extractor: "500px",
            post_id: Some(12345),
        };
        assert!(offline
            .contains_for_source(&photo, "https://500px.com/photo/12345")
            .unwrap());
        assert!(!offline
            .contains_for_source(&photo, "https://500px.com/photo/12347")
            .unwrap());
        drop(offline);
        let bytes = fs::read(native).unwrap();
        assert!(!bytes
            .windows(b"500px12345".len())
            .any(|chunk| chunk == b"500px12345"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fivehundredpx_named_archive_table_needs_exact_category_prefix() {
        let root =
            std::env::temp_dir().join(format!("galerith-500px-custom-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let source = root.join("foreign.sqlite");
        let connection = Connection::open(&source).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE custom(entry TEXT PRIMARY KEY) WITHOUT ROWID;
                 INSERT INTO custom VALUES ('12345');
                 INSERT INTO custom VALUES ('500px12345');
                 INSERT INTO custom VALUES ('500px012345');",
            )
            .unwrap();
        drop(connection);
        let before = fs::read(&source).unwrap();
        let media = MediaItem {
            url: "https://drscdn.500px.org/photo.jpg?sig=rotated".into(),
            filename: "12345.jpg".into(),
            extractor: "500px",
            post_id: Some(12345),
        };
        let page = "https://500px.com/photo/12345";
        let ambiguous = Archive::open_with_reference_table(
            &root.join("unknown.sqlite"),
            Some(&source),
            "custom",
        )
        .unwrap();
        assert_eq!(ambiguous.import_reference().unwrap(), (0, 0));
        assert!(!ambiguous.contains_for_source(&media, page).unwrap());
        drop(ambiguous);
        let native = root.join("native.sqlite");
        let scoped = Archive::open_with_reference_options(
            &native,
            Some(&source),
            Some("custom"),
            Some("500px"),
        )
        .unwrap();
        assert_eq!(scoped.import_reference().unwrap(), (1, 1));
        assert!(scoped.contains_for_source(&media, page).unwrap());
        drop(scoped);
        assert_eq!(fs::read(&source).unwrap(), before);
        fs::remove_file(source).unwrap();
        let offline = Archive::open(&native).unwrap();
        assert!(offline.contains_for_source(&media, page).unwrap());
        drop(offline);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn photovogue_photo_identity_is_global_across_verified_photographers() {
        let root = std::env::temp_dir().join(format!("galerith-photovogue-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let archive = Archive::open(&root.join("native.sqlite")).unwrap();
        let photo = MediaItem {
            url: "https://assets.vogue.com/photo/one.jpg?sig=old".into(),
            filename: "145 Autumn.jpg".into(),
            extractor: "photovogue",
            post_id: Some(145),
        };
        let rotated = MediaItem {
            url: "https://assets.vogue.com/photo/next.webp?sig=new".into(),
            filename: "145 Autumn.webp".into(),
            ..photo.clone()
        };
        let first = "https://www.vogue.com/photovogue/photographers/12345";
        let second = "https://vogue.com/photovogue/photographers/67890";
        assert_eq!(
            photovogue_archive_scope(&photo, first).unwrap().1,
            "photovogue145"
        );
        archive.record_for_source(&photo, first).unwrap();
        assert!(archive.contains_for_source(&rotated, second).unwrap());
        let unrelated = MediaItem {
            post_id: Some(146),
            ..rotated.clone()
        };
        assert!(!archive.contains_for_source(&unrelated, second).unwrap());
        for bad in [
            "https://fakevogue.com/photovogue/photographers/12345",
            "https://www.vogue.com/photovogue/photographers/12345?account=1",
            "https://www.vogue.com:8443/photovogue/photographers/12345",
            "https://www.vogue.com/photovogue/photographers/abcde",
            "https://www.vogue.com/photovogue/photo/145",
        ] {
            assert!(photovogue_archive_scope(&rotated, bad).is_none(), "{bad}");
        }
        let attacker = MediaItem {
            url: "https://assets.vogue.com.evil.test/one.jpg".into(),
            ..rotated
        };
        assert!(photovogue_archive_scope(&attacker, first).is_none());
        drop(archive);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn photovogue_archive_import_is_private_and_requires_named_table_prefix() {
        let root =
            std::env::temp_dir().join(format!("galerith-photovogue-import-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let foreign = root.join("foreign.sqlite");
        let connection = Connection::open(&foreign).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE archive(entry TEXT PRIMARY KEY) WITHOUT ROWID;
                 INSERT INTO archive VALUES ('photovogue145');
                 INSERT INTO archive VALUES ('photovogue0145');
                 INSERT INTO archive VALUES ('photovogue0');
                 CREATE TABLE custom(entry TEXT PRIMARY KEY) WITHOUT ROWID;
                 INSERT INTO custom VALUES ('145');
                 INSERT INTO custom VALUES ('photovogue145');",
            )
            .unwrap();
        drop(connection);
        let before = fs::read(&foreign).unwrap();
        let media = MediaItem {
            url: "https://assets.vogue.com/rotated.webp?sig=new".into(),
            filename: "145 Autumn.webp".into(),
            extractor: "photovogue",
            post_id: Some(145),
        };
        let origin = "https://www.vogue.com/photovogue/photographers/12345";
        let native = root.join("native.sqlite");
        let default = Archive::open_with_reference(&native, Some(&foreign)).unwrap();
        assert_eq!(default.import_reference().unwrap(), (1, 1));
        assert_eq!(default.import_reference().unwrap(), (1, 0));
        drop(default);
        let unscoped = Archive::open_with_reference_table(
            &root.join("unscoped.sqlite"),
            Some(&foreign),
            "custom",
        )
        .unwrap();
        assert_eq!(unscoped.import_reference().unwrap(), (0, 0));
        assert!(!unscoped.contains_for_source(&media, origin).unwrap());
        drop(unscoped);
        let scoped = Archive::open_with_reference_options(
            &root.join("custom.sqlite"),
            Some(&foreign),
            Some("custom"),
            Some("photovogue"),
        )
        .unwrap();
        assert_eq!(scoped.import_reference().unwrap(), (1, 1));
        assert!(scoped.contains_for_source(&media, origin).unwrap());
        drop(scoped);
        assert_eq!(fs::read(&foreign).unwrap(), before);
        fs::remove_file(foreign).unwrap();
        let detached = Archive::open(&native).unwrap();
        assert!(detached.contains_for_source(&media, origin).unwrap());
        drop(detached);
        let bytes = fs::read(&native).unwrap();
        assert!(!bytes
            .windows(b"photovogue145".len())
            .any(|part| part == b"photovogue145"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fapachi_source_archive_is_user_scoped_even_for_identical_media_url() {
        let root = std::env::temp_dir().join(format!("galerith-fapachi-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let archive = Archive::open(&root.join("native.sqlite")).unwrap();
        let item = MediaItem {
            url: "https://fapachi.com/uploads/12345.jpg".into(),
            filename: "model_12345.jpg".into(),
            extractor: "fapachi",
            post_id: Some(12345),
        };
        let first = "https://fapachi.com/model/media/12345";
        let other = "https://fapachi.com/another/media/12345";
        let rotated = MediaItem {
            url: "https://fapachi.com/uploads/12345.webp".into(),
            filename: "model_12345.webp".into(),
            ..item.clone()
        };
        assert_eq!(
            fapachi_archive_scope(&item, first).unwrap().1,
            "fapachimodel_12345"
        );
        archive.record_for_source(&item, first).unwrap();
        assert!(archive.contains_for_source(&rotated, first).unwrap());
        assert!(!archive.contains_for_source(&item, other).unwrap());
        assert!(!archive
            .contains_for_source(&item, "https://fapachi.com/model/media/12346")
            .unwrap());
        for invalid in [
            "https://fapachi.com/model/media/012345",
            "https://fapachi.com/model/page/1",
            "https://fapachi.com/search/media/12345",
            "https://fakefapachi.com/model/media/12345",
            "https://fapachi.com:8443/model/media/12345",
            "https://fapachi.com/model/media/12345?token=secret",
        ] {
            assert!(fapachi_archive_scope(&item, invalid).is_none(), "{invalid}");
        }
        let attacker = MediaItem {
            url: "https://fapachi.com.evil.test/uploads/12345.jpg".into(),
            ..item
        };
        assert!(fapachi_archive_scope(&attacker, first).is_none());
        for invalid in [
            "model_0",
            "model_012345",
            "search_12345",
            "model_18446744073709551616",
            "model_12345_other",
        ] {
            assert!(!canonical_fapachi_archive_suffix(invalid), "{invalid}");
        }
        assert!(canonical_fapachi_archive_suffix("model_name_12345"));
        drop(archive);
        let bytes = fs::read(root.join("native.sqlite")).unwrap();
        assert!(!bytes
            .windows(b"model_12345".len())
            .any(|part| part == b"model_12345"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fapachi_import_requires_canonical_user_and_read_only_source() {
        let root =
            std::env::temp_dir().join(format!("galerith-fapachi-import-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let foreign = root.join("foreign.sqlite");
        let connection = Connection::open(&foreign).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE archive(entry TEXT PRIMARY KEY) WITHOUT ROWID;
                 INSERT INTO archive VALUES ('fapachimodel_12345');
                 INSERT INTO archive VALUES ('fapachianother_12345');
                 INSERT INTO archive VALUES ('fapachimodel_012345');
                 INSERT INTO archive VALUES ('fapachisearch_12345');
                 CREATE TABLE custom(entry TEXT PRIMARY KEY) WITHOUT ROWID;
                 INSERT INTO custom VALUES ('model_12345');
                 INSERT INTO custom VALUES ('fapachimodel_12345');",
            )
            .unwrap();
        drop(connection);
        let before = fs::read(&foreign).unwrap();
        let native = root.join("native.sqlite");
        let archive = Archive::open_with_reference(&native, Some(&foreign)).unwrap();
        assert_eq!(archive.import_reference().unwrap(), (2, 2));
        assert_eq!(archive.import_reference().unwrap(), (2, 0));
        drop(archive);
        let item = MediaItem {
            url: "https://fapachi.com/uploads/rotated.webp".into(),
            filename: "model_12345.webp".into(),
            extractor: "fapachi",
            post_id: Some(12345),
        };
        let page = "https://fapachi.com/model/media/12345";
        let unnamed = Archive::open_with_reference_table(
            &root.join("unnamed.sqlite"),
            Some(&foreign),
            "custom",
        )
        .unwrap();
        assert_eq!(unnamed.import_reference().unwrap(), (0, 0));
        assert!(!unnamed.contains_for_source(&item, page).unwrap());
        drop(unnamed);
        let named = Archive::open_with_reference_options(
            &root.join("named.sqlite"),
            Some(&foreign),
            Some("custom"),
            Some("fapachi"),
        )
        .unwrap();
        assert_eq!(named.import_reference().unwrap(), (1, 1));
        assert!(named.contains_for_source(&item, page).unwrap());
        drop(named);
        assert_eq!(fs::read(&foreign).unwrap(), before);
        fs::remove_file(foreign).unwrap();
        let offline = Archive::open(&native).unwrap();
        assert!(offline.contains_for_source(&item, page).unwrap());
        drop(offline);
        let bytes = fs::read(&native).unwrap();
        assert!(!bytes
            .windows(b"fapachimodel_12345".len())
            .any(|part| part == b"fapachimodel_12345"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fapello_post_type_model_and_media_identity_are_isolated() {
        let root = std::env::temp_dir().join(format!("galerith-fapello-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let native = root.join("native.sqlite");
        let archive = Archive::open(&native).unwrap();
        let image = MediaItem {
            url: "https://cdn.fapello.com/media/image.jpg".into(),
            filename: "model_12345.jpg".into(),
            extractor: "fapello",
            post_id: Some(12345),
        };
        let rotated = MediaItem {
            url: "https://cdn.fapello.su/media/new.webp".into(),
            filename: "model_12345.webp".into(),
            ..image.clone()
        };
        let video = MediaItem {
            url: "https://cdn.fapello.com/media/video.mp4".into(),
            filename: "model_12345.mp4".into(),
            ..image.clone()
        };
        let source = "https://fapello.com/model/12345/";
        assert_eq!(
            fapello_archive_scope(&image, source).unwrap().1,
            "fapellophoto_model_12345"
        );
        assert_eq!(
            fapello_archive_scope(&video, source).unwrap().1,
            "fapellovideo_model_12345"
        );
        archive.record_for_source(&image, source).unwrap();
        assert!(archive.contains_for_source(&rotated, source).unwrap());
        assert!(archive
            .contains_for_source(&rotated, "https://fapello.su/model/12345")
            .unwrap());
        assert!(!archive.contains_for_source(&video, source).unwrap());
        assert!(!archive
            .contains_for_source(&image, "https://fapello.com/other/12345")
            .unwrap());
        for bad in [
            "https://fapello.com/model/012345",
            "https://fapello.com/top-likes/12345",
            "https://fapello.com/model/12346",
            "https://evil-fapello.com/model/12345",
            "https://fapello.com:8443/model/12345",
            "https://fapello.com/model/12345?secret=1",
        ] {
            assert!(fapello_archive_scope(&image, bad).is_none(), "{bad}");
        }
        let forged = MediaItem {
            url: "https://cdn.fapello.com.evil.test/media.jpg".into(),
            ..image
        };
        assert!(fapello_archive_scope(&forged, source).is_none());
        for bad in [
            "movie_model_12345",
            "photo_model_012345",
            "photo_model_0",
            "photo_search_12345",
        ] {
            assert!(!canonical_fapello_archive_suffix(bad), "{bad}");
        }
        assert!(canonical_fapello_archive_suffix("photo_model_name_12345"));
        drop(archive);
        let bytes = fs::read(native).unwrap();
        assert!(!bytes
            .windows(b"model_12345".len())
            .any(|part| part == b"model_12345"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fapello_reference_import_is_private_and_source_free_after_reopen() {
        let root =
            std::env::temp_dir().join(format!("galerith-fapello-import-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let foreign = root.join("foreign.sqlite");
        let db = Connection::open(&foreign).unwrap();
        db.execute_batch(
            "CREATE TABLE archive(entry TEXT PRIMARY KEY) WITHOUT ROWID;
             INSERT INTO archive VALUES ('fapellophoto_model_12345');
             INSERT INTO archive VALUES ('fapellovideo_model_12345');
             INSERT INTO archive VALUES ('fapellophoto_model_012345');
             INSERT INTO archive VALUES ('fapellophoto_search_12345');
             CREATE TABLE named(entry TEXT PRIMARY KEY) WITHOUT ROWID;
             INSERT INTO named VALUES ('photo_model_12345');
             INSERT INTO named VALUES ('fapellophoto_model_12345');",
        )
        .unwrap();
        drop(db);
        let original = fs::read(&foreign).unwrap();
        let native = root.join("native.sqlite");
        let archive = Archive::open_with_reference(&native, Some(&foreign)).unwrap();
        assert_eq!(archive.import_reference().unwrap(), (2, 2));
        assert_eq!(archive.import_reference().unwrap(), (2, 0));
        drop(archive);
        let ambiguous = Archive::open_with_reference_table(
            &root.join("ambiguous.sqlite"),
            Some(&foreign),
            "named",
        )
        .unwrap();
        assert_eq!(ambiguous.import_reference().unwrap(), (0, 0));
        drop(ambiguous);
        let named = Archive::open_with_reference_options(
            &root.join("named.sqlite"),
            Some(&foreign),
            Some("named"),
            Some("fapello"),
        )
        .unwrap();
        assert_eq!(named.import_reference().unwrap(), (1, 1));
        drop(named);
        assert_eq!(fs::read(&foreign).unwrap(), original);
        fs::remove_file(foreign).unwrap();
        let offline = Archive::open(&native).unwrap();
        let photo = MediaItem {
            url: "https://cdn.fapello.su/rotated.webp".into(),
            filename: "rotated.webp".into(),
            extractor: "fapello",
            post_id: Some(12345),
        };
        assert!(offline
            .contains_for_source(&photo, "https://fapello.com/model/12345")
            .unwrap());
        assert!(!offline
            .contains_for_source(&photo, "https://fapello.com/other/12345")
            .unwrap());
        let video = MediaItem {
            url: "https://cdn.fapello.com/rotated.mp4".into(),
            ..photo
        };
        assert!(offline
            .contains_for_source(&video, "https://fapello.com/model/12345")
            .unwrap());
        drop(offline);
        let bytes = fs::read(&native).unwrap();
        assert!(!bytes
            .windows(b"fapellophoto_model_12345".len())
            .any(|part| part == b"fapellophoto_model_12345"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn lightroom_asset_ids_survive_rendition_rotation_without_confusing_assets() {
        let root = std::env::temp_dir().join(format!("galerith-lightroom-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let native = root.join("native.sqlite");
        let archive = Archive::open(&native).unwrap();
        let first = MediaItem {
            url: "https://assets.adobe.io/original.jpg?token=private".into(),
            filename: "0001_asset-A.jpg".into(),
            extractor: "lightroom",
            post_id: None,
        };
        let rendition = MediaItem {
            url: "https://images.adobecdn.com/renditions/rotated.webp?token=changed".into(),
            filename: "0019_asset-A.webp".into(),
            ..first.clone()
        };
        let second = MediaItem {
            filename: "0002_asset-B.jpg".into(),
            ..first.clone()
        };
        let share = "https://lightroom.adobe.com/shares/012345abcdef";
        assert_eq!(
            lightroom_archive_scope(&first, share).unwrap().1,
            "lightroomasset-A"
        );
        archive.record_for_source(&first, share).unwrap();
        assert!(archive.contains_for_source(&rendition, share).unwrap());
        assert!(archive
            .contains_for_source(
                &rendition,
                "https://lightroom.adobe.com/shares/fedcba012345",
            )
            .unwrap());
        assert!(!archive.contains_for_source(&second, share).unwrap());
        for invalid in [
            "https://lightroom.adobe.com/shares/GGG",
            "https://lightroom.adobe.com/shares/abc/extra",
            "https://lightroom.adobe.com:8443/shares/abc",
            "https://evil.lightroom.adobe.com/shares/abc",
            "https://lightroom.adobe.com/shares/abc?secret=1",
        ] {
            assert!(
                lightroom_archive_scope(&first, invalid).is_none(),
                "{invalid}"
            );
        }
        for invalid in [
            "0000_asset-A.jpg",
            "10000_asset-A.jpg",
            "0001_asset-A.png",
            "0001_abc%2fdef.jpg",
        ] {
            let item = MediaItem {
                filename: invalid.into(),
                ..first.clone()
            };
            assert!(lightroom_archive_scope(&item, share).is_none(), "{invalid}");
        }
        let malicious = MediaItem {
            url: "https://adobe.io.evil.test/asset-A.jpg".into(),
            ..first
        };
        assert!(lightroom_archive_scope(&malicious, share).is_none());
        drop(archive);
        let bytes = fs::read(&native).unwrap();
        for secret in ["asset-A", "private", "rotated.webp"] {
            assert!(
                !bytes
                    .windows(secret.len())
                    .any(|chunk| chunk == secret.as_bytes()),
                "{secret}"
            );
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn lightroom_import_requires_proven_category_and_survives_offline_reopen() {
        let root =
            std::env::temp_dir().join(format!("galerith-lightroom-import-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let foreign = root.join("gallery-dl.sqlite");
        let db = Connection::open(&foreign).unwrap();
        db.execute_batch(
            "CREATE TABLE archive(entry TEXT PRIMARY KEY) WITHOUT ROWID;
             INSERT INTO archive VALUES ('lightroomasset-A');
             INSERT INTO archive VALUES ('lightroomasset-B');
             INSERT INTO archive VALUES ('lightroombad%2fid');
             CREATE TABLE named(entry TEXT PRIMARY KEY) WITHOUT ROWID;
             INSERT INTO named VALUES ('asset-A');
             INSERT INTO named VALUES ('lightroomasset-A');",
        )
        .unwrap();
        drop(db);
        let before = fs::read(&foreign).unwrap();
        let native = root.join("native.sqlite");
        let archive = Archive::open_with_reference(&native, Some(&foreign)).unwrap();
        assert_eq!(archive.import_reference().unwrap(), (2, 2));
        assert_eq!(archive.import_reference().unwrap(), (2, 0));
        drop(archive);
        let unscoped = Archive::open_with_reference_table(
            &root.join("unscoped.sqlite"),
            Some(&foreign),
            "named",
        )
        .unwrap();
        assert_eq!(unscoped.import_reference().unwrap(), (0, 0));
        drop(unscoped);
        let named = Archive::open_with_reference_options(
            &root.join("named.sqlite"),
            Some(&foreign),
            Some("named"),
            Some("lightroom"),
        )
        .unwrap();
        assert_eq!(named.import_reference().unwrap(), (1, 1));
        drop(named);
        assert_eq!(fs::read(&foreign).unwrap(), before);
        fs::remove_file(foreign).unwrap();
        let local = Archive::open(&native).unwrap();
        let found = MediaItem {
            url: "https://assets.adobe.net/changed.webp?token=rotated".into(),
            filename: "0021_asset-A.webp".into(),
            extractor: "lightroom",
            post_id: None,
        };
        assert!(local
            .contains_for_source(&found, "https://lightroom.adobe.com/shares/abc")
            .unwrap());
        let missing = MediaItem {
            filename: "0001_asset-C.webp".into(),
            ..found
        };
        assert!(!local
            .contains_for_source(&missing, "https://lightroom.adobe.com/shares/abc")
            .unwrap());
        drop(local);
        let bytes = fs::read(native).unwrap();
        assert!(!bytes
            .windows(b"lightroomasset-A".len())
            .any(|chunk| chunk == b"lightroomasset-A"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn foriio_multi_originals_remain_independent_across_cdn_changes() {
        let root = std::env::temp_dir().join(format!("galerith-foriio-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let native = root.join("native.sqlite");
        let archive = Archive::open(&native).unwrap();
        let original = MediaItem {
            url: "https://foriio.imgix.net/store/first.png".into(),
            filename: "01 file-A.png".into(),
            extractor: "foriio",
            post_id: Some(12345),
        };
        let rotated = MediaItem {
            url: "https://foriio.imgix.net/store/new.png".into(),
            ..original.clone()
        };
        let second = MediaItem {
            url: "https://foriio.imgix.net/store/second.png".into(),
            filename: "02 file-B.png".into(),
            ..original.clone()
        };
        let repeated = MediaItem {
            filename: "02 file-A.png".into(),
            ..original.clone()
        };
        let share = "https://foriio.com/works/12345";
        assert_eq!(
            foriio_archive_scope(&original, share).unwrap().1,
            "foriio12345/file-A/1"
        );
        archive.record_for_source(&original, share).unwrap();
        assert!(archive.contains_for_source(&rotated, share).unwrap());
        assert!(archive
            .contains_for_source(&rotated, "https://www.fori.io/works/12345")
            .unwrap());
        assert!(!archive.contains_for_source(&second, share).unwrap());
        assert!(!archive.contains_for_source(&repeated, share).unwrap());
        assert!(!archive
            .contains_for_source(&original, "https://foriio.com/works/12346")
            .unwrap());
        for wrong_source in [
            "https://foriio.com/works/012345",
            "https://foriio.com/works/12345?secret=1",
            "https://evilforiio.com/works/12345",
            "https://foriio.com:8443/works/12345",
            "https://foriio.com/artist",
        ] {
            assert!(
                foriio_archive_scope(&original, wrong_source).is_none(),
                "{wrong_source}"
            );
        }
        let evil = MediaItem {
            url: "https://foriio.imgix.net.evil.test/store/first.png".into(),
            ..original.clone()
        };
        assert!(foriio_archive_scope(&evil, share).is_none());
        for wrong_filename in [
            "00 file-A.png",
            "1 file-A.png",
            "129 file-A.png",
            "01 file-A.jpg",
            "01 bad%2fid.png",
        ] {
            let item = MediaItem {
                filename: wrong_filename.into(),
                ..original.clone()
            };
            assert!(
                foriio_archive_scope(&item, share).is_none(),
                "{wrong_filename}"
            );
        }
        for key in [
            "012345/file-A/1",
            "12345/file-A/01",
            "12345/file-A/0",
            "12345/file-A/129",
        ] {
            assert!(!canonical_foriio_archive_suffix(key), "{key}");
        }
        drop(archive);
        let bytes = fs::read(&native).unwrap();
        for private in ["file-A", "first.png", "12345/file-A"] {
            assert!(
                !bytes
                    .windows(private.len())
                    .any(|part| part == private.as_bytes()),
                "{private}"
            );
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn foriio_reference_import_is_private_idempotent_and_offline_usable() {
        let root =
            std::env::temp_dir().join(format!("galerith-foriio-import-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let foreign = root.join("gallery-dl.sqlite");
        let db = Connection::open(&foreign).unwrap();
        db.execute_batch(
            "CREATE TABLE archive(entry TEXT PRIMARY KEY) WITHOUT ROWID;
             INSERT INTO archive VALUES ('foriio12345/file-A/1');
             INSERT INTO archive VALUES ('foriio12345/file-B/2');
             INSERT INTO archive VALUES ('foriio12345/file-A/01');
             CREATE TABLE named(entry TEXT PRIMARY KEY) WITHOUT ROWID;
             INSERT INTO named VALUES ('12345/file-A/1');
             INSERT INTO named VALUES ('foriio12345/file-A/1');",
        )
        .unwrap();
        drop(db);
        let original_bytes = fs::read(&foreign).unwrap();
        let native = root.join("native.sqlite");
        let archive = Archive::open_with_reference(&native, Some(&foreign)).unwrap();
        assert_eq!(archive.import_reference().unwrap(), (2, 2));
        assert_eq!(archive.import_reference().unwrap(), (2, 0));
        drop(archive);
        let unscoped = Archive::open_with_reference_table(
            &root.join("unscoped.sqlite"),
            Some(&foreign),
            "named",
        )
        .unwrap();
        assert_eq!(unscoped.import_reference().unwrap(), (0, 0));
        drop(unscoped);
        let named = Archive::open_with_reference_options(
            &root.join("named.sqlite"),
            Some(&foreign),
            Some("named"),
            Some("foriio"),
        )
        .unwrap();
        assert_eq!(named.import_reference().unwrap(), (1, 1));
        drop(named);
        assert_eq!(fs::read(&foreign).unwrap(), original_bytes);
        fs::remove_file(foreign).unwrap();
        let reopened = Archive::open(&native).unwrap();
        let first = MediaItem {
            url: "https://foriio.imgix.net/store/changed.png".into(),
            filename: "01 file-A.png".into(),
            extractor: "foriio",
            post_id: Some(12345),
        };
        assert!(reopened
            .contains_for_source(&first, "https://foriio.com/works/12345")
            .unwrap());
        let different_position = MediaItem {
            filename: "02 file-A.png".into(),
            ..first.clone()
        };
        assert!(!reopened
            .contains_for_source(&different_position, "https://foriio.com/works/12345")
            .unwrap());
        let second = MediaItem {
            filename: "02 file-B.png".into(),
            ..first
        };
        assert!(reopened
            .contains_for_source(&second, "https://foriio.com/works/12345")
            .unwrap());
        drop(reopened);
        let bytes = fs::read(native).unwrap();
        assert!(!bytes
            .windows(b"foriio12345/file-A/1".len())
            .any(|part| part == b"foriio12345/file-A/1"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn piczel_user_and_image_keys_keep_separate_original_positions() {
        let root = std::env::temp_dir().join(format!("galerith-piczel-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let native = root.join("native.sqlite");
        let archive = Archive::open(&native).unwrap();
        let first = MediaItem {
            url: "https://cdn.piczel.tv/first.jpg".into(),
            filename: "piczel_1001_Aurora_00.jpg".into(),
            extractor: "piczel",
            post_id: Some(1001),
        };
        let moved = MediaItem {
            url: "https://media.piczel.tv/rotated.jpg".into(),
            ..first.clone()
        };
        let second = MediaItem {
            filename: "piczel_1001_Aurora_01.jpg".into(),
            ..first.clone()
        };
        let other_post = MediaItem {
            filename: "piczel_1002_Aurora_00.jpg".into(),
            post_id: Some(1002),
            ..first.clone()
        };
        let user = "https://piczel.tv/gallery/artist";
        assert_eq!(
            piczel_archive_scope(&first, user).unwrap().1,
            "piczel1001_0"
        );
        archive.record_for_source(&first, user).unwrap();
        assert!(archive.contains_for_source(&moved, user).unwrap());
        assert!(archive
            .contains_for_source(&moved, "https://piczel.tv/gallery/image/1001")
            .unwrap());
        assert!(!archive.contains_for_source(&second, user).unwrap());
        assert!(!archive.contains_for_source(&other_post, user).unwrap());
        assert!(piczel_archive_scope(&first, "https://piczel.tv/gallery/artist/5").is_none());
        for bad in [
            "https://piczel.tv/gallery/image/1002",
            "https://piczel.tv/gallery/artist/5",
            "https://piczel.tv:8443/gallery/artist",
            "https://piczel.tv/gallery/artist?secret=1",
            "https://evil.piczel.tv.evil.test/gallery/artist",
        ] {
            assert!(piczel_archive_scope(&first, bad).is_none(), "{bad}");
        }
        let forged = MediaItem {
            url: "https://cdn.piczel.tv.evil.test/first.jpg".into(),
            ..first.clone()
        };
        assert!(piczel_archive_scope(&forged, user).is_none());
        for bad_name in [
            "piczel_1001_Aurora_0.jpg",
            "piczel_1001_Aurora_16.jpg",
            "piczel_1002_Aurora_00.jpg",
            "piczel_1001_Aurora_00.png",
        ] {
            let item = MediaItem {
                filename: bad_name.into(),
                ..first.clone()
            };
            assert!(piczel_archive_scope(&item, user).is_none(), "{bad_name}");
        }
        for malformed in ["1001_00", "1001_16", "01001_0", "f5_1001_0"] {
            assert!(!canonical_piczel_archive_suffix(malformed), "{malformed}");
        }
        drop(archive);
        let bytes = fs::read(native).unwrap();
        for private in ["1001_0", "Aurora", "rotated.jpg"] {
            assert!(
                !bytes
                    .windows(private.len())
                    .any(|part| part == private.as_bytes()),
                "{private}"
            );
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn piczel_default_and_named_import_never_claim_unverified_folder_keys() {
        let root =
            std::env::temp_dir().join(format!("galerith-piczel-import-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let foreign = root.join("gallery-dl.sqlite");
        let db = Connection::open(&foreign).unwrap();
        db.execute_batch(
            "CREATE TABLE archive(entry TEXT PRIMARY KEY) WITHOUT ROWID;
             INSERT INTO archive VALUES ('piczel1001_0');
             INSERT INTO archive VALUES ('piczel1001_1');
             INSERT INTO archive VALUES ('piczel1001_00');
             INSERT INTO archive VALUES ('piczelf5_1001_0');
             CREATE TABLE named(entry TEXT PRIMARY KEY) WITHOUT ROWID;
             INSERT INTO named VALUES ('1001_0');
             INSERT INTO named VALUES ('piczel1001_0');",
        )
        .unwrap();
        drop(db);
        let original = fs::read(&foreign).unwrap();
        let native = root.join("native.sqlite");
        let archive = Archive::open_with_reference(&native, Some(&foreign)).unwrap();
        assert_eq!(archive.import_reference().unwrap(), (2, 2));
        assert_eq!(archive.import_reference().unwrap(), (2, 0));
        drop(archive);
        let unscoped = Archive::open_with_reference_table(
            &root.join("unscoped.sqlite"),
            Some(&foreign),
            "named",
        )
        .unwrap();
        assert_eq!(unscoped.import_reference().unwrap(), (0, 0));
        drop(unscoped);
        let prefixed = Archive::open_with_reference_options(
            &root.join("named.sqlite"),
            Some(&foreign),
            Some("named"),
            Some("piczel"),
        )
        .unwrap();
        assert_eq!(prefixed.import_reference().unwrap(), (1, 1));
        drop(prefixed);
        assert_eq!(fs::read(&foreign).unwrap(), original);
        fs::remove_file(foreign).unwrap();
        let archive = Archive::open(&native).unwrap();
        let item = MediaItem {
            url: "https://cdn.piczel.tv/changed.jpg".into(),
            filename: "piczel_1001_Aurora_00.jpg".into(),
            extractor: "piczel",
            post_id: Some(1001),
        };
        assert!(archive
            .contains_for_source(&item, "https://piczel.tv/gallery/artist")
            .unwrap());
        assert!(archive
            .contains_for_source(&item, "https://piczel.tv/gallery/image/1001")
            .unwrap());
        assert!(!archive
            .contains_for_source(&item, "https://piczel.tv/gallery/artist/5")
            .unwrap());
        let missing = MediaItem {
            filename: "piczel_1001_Aurora_02.jpg".into(),
            ..item
        };
        assert!(!archive
            .contains_for_source(&missing, "https://piczel.tv/gallery/artist")
            .unwrap());
        drop(archive);
        let bytes = fs::read(native).unwrap();
        assert!(!bytes
            .windows(b"piczel1001_0".len())
            .any(|part| part == b"piczel1001_0"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn thirtyfivephoto_archive_keeps_series_originals_distinct() {
        let root = std::env::temp_dir().join(format!("galerith-35photo-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let db = root.join("native.sqlite");
        let archive = Archive::open(&db).unwrap();
        let first = MediaItem {
            url: "https://img.35photo.pro/a.jpg".into(),
            filename: "12345_01.jpg".into(),
            extractor: "35photo",
            post_id: Some(12345),
        };
        let rotated = MediaItem {
            url: "https://cdn.35photo.pro/new.jpg".into(),
            ..first.clone()
        };
        let other = MediaItem {
            url: "https://cdn.35photo.pro/b.jpg".into(),
            filename: "12345_02.jpg".into(),
            ..first.clone()
        };
        let source = "https://35photo.pro/photo_12345/";
        assert_eq!(
            thirtyfivephoto_archive_scope(&first, source).unwrap().1,
            "35photo12345_1"
        );
        archive.record_for_source(&first, source).unwrap();
        assert!(archive.contains_for_source(&rotated, source).unwrap());
        assert!(!archive.contains_for_source(&other, source).unwrap());
        for bad in [
            "https://35photo.pro/tags/people",
            "https://35photo.pro/genre_10/",
            "https://35photo.pro/photo_012345/",
            "https://35photo.pro/photo_12345?token=private",
            "https://evil35photo.pro/photo_12345",
        ] {
            assert!(
                thirtyfivephoto_archive_scope(&first, bad).is_none(),
                "{bad}"
            );
        }
        for bad in [
            "12345_00.jpg",
            "12345_33.jpg",
            "12345_1.jpg",
            "12345_01.png",
        ] {
            let item = MediaItem {
                filename: bad.into(),
                ..first.clone()
            };
            assert!(
                thirtyfivephoto_archive_scope(&item, source).is_none(),
                "{bad}"
            );
        }
        let evil = MediaItem {
            url: "https://img.35photo.pro.evil.test/a.jpg".into(),
            ..first
        };
        assert!(thirtyfivephoto_archive_scope(&evil, source).is_none());
        for invalid in ["012345_1", "12345_01", "12345_0", "12345_33"] {
            assert!(!canonical_thirtyfivephoto_archive_suffix(invalid));
        }
        drop(archive);
        let bytes = fs::read(db).unwrap();
        for plaintext in ["12345_1", "new.jpg", "a.jpg"] {
            assert!(!bytes
                .windows(plaintext.len())
                .any(|part| part == plaintext.as_bytes()));
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn thirtyfivephoto_import_rejects_ambiguous_tag_and_genre_keys() {
        let root =
            std::env::temp_dir().join(format!("galerith-35photo-import-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let foreign = root.join("foreign.sqlite");
        let db = Connection::open(&foreign).unwrap();
        db.execute_batch(
            "CREATE TABLE archive(entry TEXT PRIMARY KEY) WITHOUT ROWID;
             INSERT INTO archive VALUES ('35photo12345_1');
             INSERT INTO archive VALUES ('35photo12345_2');
             INSERT INTO archive VALUES ('35photo12345_01');
             INSERT INTO archive VALUES ('35phototcats_12345_1');
             INSERT INTO archive VALUES ('35photog10_12345_1');
             CREATE TABLE named(entry TEXT PRIMARY KEY) WITHOUT ROWID;
             INSERT INTO named VALUES ('12345_1');
             INSERT INTO named VALUES ('35photo12345_1');",
        )
        .unwrap();
        drop(db);
        let before = fs::read(&foreign).unwrap();
        let native = root.join("native.sqlite");
        let archive = Archive::open_with_reference(&native, Some(&foreign)).unwrap();
        assert_eq!(archive.import_reference().unwrap(), (2, 2));
        assert_eq!(archive.import_reference().unwrap(), (2, 0));
        drop(archive);
        let ambiguous = Archive::open_with_reference_table(
            &root.join("ambiguous.sqlite"),
            Some(&foreign),
            "named",
        )
        .unwrap();
        assert_eq!(ambiguous.import_reference().unwrap(), (0, 0));
        drop(ambiguous);
        let named = Archive::open_with_reference_options(
            &root.join("named.sqlite"),
            Some(&foreign),
            Some("named"),
            Some("35photo"),
        )
        .unwrap();
        assert_eq!(named.import_reference().unwrap(), (1, 1));
        drop(named);
        assert_eq!(fs::read(&foreign).unwrap(), before);
        fs::remove_file(foreign).unwrap();
        let reopened = Archive::open(&native).unwrap();
        let original = MediaItem {
            url: "https://cdn.35photo.pro/rotated.jpg".into(),
            filename: "12345_01.jpg".into(),
            extractor: "35photo",
            post_id: Some(12345),
        };
        assert!(reopened
            .contains_for_source(&original, "https://35photo.pro/photo_12345/")
            .unwrap());
        let second = MediaItem {
            filename: "12345_02.jpg".into(),
            ..original.clone()
        };
        assert!(reopened
            .contains_for_source(&second, "https://35photo.pro/photo_12345/")
            .unwrap());
        let third = MediaItem {
            filename: "12345_03.jpg".into(),
            ..original
        };
        assert!(!reopened
            .contains_for_source(&third, "https://35photo.pro/photo_12345/")
            .unwrap());
        drop(reopened);
        let bytes = fs::read(native).unwrap();
        assert!(!bytes
            .windows(b"35photo12345_1".len())
            .any(|part| part == b"35photo12345_1"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unsplash_photo_identity_spans_public_feeds_and_rotated_cdn() {
        let root = std::env::temp_dir().join(format!("galerith-unsplash-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let native = root.join("native.sqlite");
        let archive = Archive::open(&native).unwrap();
        let photo = MediaItem {
            url: "https://images.unsplash.com/photo-a?ixid=secret".into(),
            filename: "abc-A.jpg".into(),
            extractor: "unsplash",
            post_id: None,
        };
        let rotated = MediaItem {
            url: "https://plus.unsplash.com/photo-new?ixid=changed".into(),
            ..photo.clone()
        };
        archive
            .record_for_source(&photo, "https://unsplash.com/photos/abc-A")
            .unwrap();
        for source in [
            "https://unsplash.com/photos/abc-A",
            "https://unsplash.com/@alice",
            "https://unsplash.com/@alice/likes",
            "https://unsplash.com/collections/12345/nature",
            "https://unsplash.com/s/photos/sky?orientation=landscape",
        ] {
            assert!(
                archive.contains_for_source(&rotated, source).unwrap(),
                "{source}"
            );
        }
        let other = MediaItem {
            filename: "abc-B.jpg".into(),
            ..rotated.clone()
        };
        assert!(!archive
            .contains_for_source(&other, "https://unsplash.com/@alice")
            .unwrap());
        for invalid in [
            "https://unsplash.com/photos/abc-B",
            "http://unsplash.com/photos/abc-A",
            "https://evil.unsplash.com/@alice",
            "https://unsplash.com:8443/@alice",
            "https://unsplash.com/s/photos/sky?orientation=invalid",
            "https://unsplash.com/@alice?token=private",
            "https://unsplash.com/photos/abc-A?secret=1",
        ] {
            assert!(
                unsplash_archive_scope(&photo, invalid).is_none(),
                "{invalid}"
            );
        }
        let forged = MediaItem {
            url: "https://images.unsplash.com.evil.test/pic".into(),
            ..photo
        };
        assert!(unsplash_archive_scope(&forged, "https://unsplash.com/@alice").is_none());
        drop(archive);
        let bytes = fs::read(&native).unwrap();
        for secret in ["abc-A", "photo-a", "secret"] {
            assert!(!bytes
                .windows(secret.len())
                .any(|part| part == secret.as_bytes()));
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unsplash_import_is_idempotent_scoped_and_available_offline() {
        let root =
            std::env::temp_dir().join(format!("galerith-unsplash-import-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let foreign = root.join("foreign.sqlite");
        let db = Connection::open(&foreign).unwrap();
        db.execute_batch(
            "CREATE TABLE archive(entry TEXT PRIMARY KEY) WITHOUT ROWID;
             INSERT INTO archive VALUES ('unsplashabc-A');
             INSERT INTO archive VALUES ('unsplashabc-B');
             INSERT INTO archive VALUES ('unsplashbad%2fid');
             CREATE TABLE named(entry TEXT PRIMARY KEY) WITHOUT ROWID;
             INSERT INTO named VALUES ('abc-A');
             INSERT INTO named VALUES ('unsplashabc-A');",
        )
        .unwrap();
        drop(db);
        let source_bytes = fs::read(&foreign).unwrap();
        let native = root.join("native.sqlite");
        let archive = Archive::open_with_reference(&native, Some(&foreign)).unwrap();
        assert_eq!(archive.import_reference().unwrap(), (2, 2));
        assert_eq!(archive.import_reference().unwrap(), (2, 0));
        drop(archive);
        let ambiguous = Archive::open_with_reference_table(
            &root.join("ambiguous.sqlite"),
            Some(&foreign),
            "named",
        )
        .unwrap();
        assert_eq!(ambiguous.import_reference().unwrap(), (0, 0));
        drop(ambiguous);
        let named = Archive::open_with_reference_options(
            &root.join("named.sqlite"),
            Some(&foreign),
            Some("named"),
            Some("unsplash"),
        )
        .unwrap();
        assert_eq!(named.import_reference().unwrap(), (1, 1));
        drop(named);
        assert_eq!(fs::read(&foreign).unwrap(), source_bytes);
        fs::remove_file(foreign).unwrap();
        let offline = Archive::open(&native).unwrap();
        let photo = MediaItem {
            url: "https://plus.unsplash.com/changed?ixid=rotated".into(),
            filename: "abc-A.jpg".into(),
            extractor: "unsplash",
            post_id: None,
        };
        assert!(offline
            .contains_for_source(&photo, "https://unsplash.com/collections/123/flowers")
            .unwrap());
        let missing = MediaItem {
            filename: "abc-C.jpg".into(),
            ..photo
        };
        assert!(!offline
            .contains_for_source(&missing, "https://unsplash.com/collections/123/flowers")
            .unwrap());
        drop(offline);
        let bytes = fs::read(&native).unwrap();
        assert!(!bytes
            .windows(b"unsplashabc-A".len())
            .any(|part| part == b"unsplashabc-A"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn lexica_images_are_scoped_by_verified_asset_id_not_search_query() {
        let root = std::env::temp_dir().join(format!("galerith-lexica-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let db = root.join("native.sqlite");
        let archive = Archive::open(&db).unwrap();
        let first = MediaItem {
            url: "https://lexica-serve-encoded-images2.sharif.workers.dev/full_jpg/abc-A".into(),
            filename: "abc-A.jpg".into(),
            extractor: "lexica",
            post_id: None,
        };
        archive
            .record_for_source(&first, "https://lexica.art/?q=red+fox")
            .unwrap();
        assert!(archive
            .contains_for_source(&first, "https://lexica.art/?q=blue+fox")
            .unwrap());
        let second = MediaItem {
            url: "https://lexica-serve-encoded-images2.sharif.workers.dev/full_jpg/abc-B".into(),
            filename: "abc-B.jpg".into(),
            ..first.clone()
        };
        assert!(!archive
            .contains_for_source(&second, "https://lexica.art/?q=red+fox")
            .unwrap());
        for bad_source in [
            "http://lexica.art/?q=red",
            "https://lexica.art.evil.test/?q=red",
            "https://lexica.art/?q=",
            "https://lexica.art/?q=one&q=two",
            "https://lexica.art/?q=red&secret=private",
            "https://lexica.art:8443/?q=red",
        ] {
            assert!(lexica_archive_scope(&first, bad_source).is_none());
        }
        for bad_url in [
            "https://lexica-serve-encoded-images2.sharif.workers.dev/full_jpg/abc-B",
            "https://lexica-serve-encoded-images2.sharif.workers.dev/full_jpg/abc-A?token=1",
            "https://evil.sharif.workers.dev/full_jpg/abc-A",
        ] {
            let item = MediaItem {
                url: bad_url.into(),
                ..first.clone()
            };
            assert!(lexica_archive_scope(&item, "https://lexica.art/?q=red").is_none());
        }
        drop(archive);
        let raw = fs::read(db).unwrap();
        for plaintext in ["abc-A", "red+fox"] {
            assert!(!raw
                .windows(plaintext.len())
                .any(|part| part == plaintext.as_bytes()));
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn lexica_import_requires_category_and_survives_source_removal() {
        let root =
            std::env::temp_dir().join(format!("galerith-lexica-import-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let foreign = root.join("foreign.sqlite");
        let db = Connection::open(&foreign).unwrap();
        db.execute_batch(
            "CREATE TABLE archive(entry TEXT PRIMARY KEY) WITHOUT ROWID;
             INSERT INTO archive VALUES ('lexicaabc-A');
             INSERT INTO archive VALUES ('lexicaabc-B');
             INSERT INTO archive VALUES ('lexicabad%2fid');
             CREATE TABLE named(entry TEXT PRIMARY KEY) WITHOUT ROWID;
             INSERT INTO named VALUES ('abc-A');
             INSERT INTO named VALUES ('lexicaabc-A');",
        )
        .unwrap();
        drop(db);
        let before = fs::read(&foreign).unwrap();
        let native = root.join("native.sqlite");
        let archive = Archive::open_with_reference(&native, Some(&foreign)).unwrap();
        assert_eq!(archive.import_reference().unwrap(), (2, 2));
        assert_eq!(archive.import_reference().unwrap(), (2, 0));
        drop(archive);
        let unscoped = Archive::open_with_reference_table(
            &root.join("unscoped.sqlite"),
            Some(&foreign),
            "named",
        )
        .unwrap();
        assert_eq!(unscoped.import_reference().unwrap(), (0, 0));
        drop(unscoped);
        let named = Archive::open_with_reference_options(
            &root.join("named.sqlite"),
            Some(&foreign),
            Some("named"),
            Some("lexica"),
        )
        .unwrap();
        assert_eq!(named.import_reference().unwrap(), (1, 1));
        drop(named);
        assert_eq!(fs::read(&foreign).unwrap(), before);
        fs::remove_file(foreign).unwrap();
        let offline = Archive::open(&native).unwrap();
        let first = MediaItem {
            url: "https://lexica-serve-encoded-images2.sharif.workers.dev/full_jpg/abc-A".into(),
            filename: "abc-A.jpg".into(),
            extractor: "lexica",
            post_id: None,
        };
        assert!(offline
            .contains_for_source(&first, "https://lexica.art/?q=other")
            .unwrap());
        let absent = MediaItem {
            url: "https://lexica-serve-encoded-images2.sharif.workers.dev/full_jpg/abc-C".into(),
            filename: "abc-C.jpg".into(),
            ..first
        };
        assert!(!offline
            .contains_for_source(&absent, "https://lexica.art/?q=other")
            .unwrap());
        drop(offline);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pixeldrain_file_id_survives_album_filename_and_media_changes() {
        let root = std::env::temp_dir().join(format!("galerith-pixeldrain-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let native = root.join("native.sqlite");
        let archive = Archive::open(&native).unwrap();
        let file = MediaItem {
            url: "https://pixeldrain.com/api/file/File12?download".into(),
            filename: "summer one.jpg".into(),
            extractor: "pixeldrain",
            post_id: None,
        };
        archive
            .record_for_source(&file, "https://pixeldrain.com/u/File12")
            .unwrap();
        let renamed = MediaItem {
            filename: "album 001.jpg".into(),
            ..file.clone()
        };
        for source in [
            "https://pixeldrain.com/api/file/File12",
            "https://pixeldrain.com/l/album_99",
            "https://pixeldrain.com/api/list/album_99#item=1",
        ] {
            assert!(archive.contains_for_source(&renamed, source).unwrap());
        }
        let other = MediaItem {
            url: "https://pixeldrain.com/api/file/File13?download".into(),
            ..file.clone()
        };
        assert!(!archive
            .contains_for_source(&other, "https://pixeldrain.com/l/album_99")
            .unwrap());
        for invalid in [
            "https://pixeldrain.com/u/File13",
            "https://pixeldrain.com/api/list/album_99/zip",
            "https://pixeldrain.com/d/folder",
            "https://pixeldrain.com/l/album_99#item=2000",
            "https://pixeldrain.com/l/album_99?secret=1",
            "https://pixeldrain.com:8443/u/File12",
            "https://evil.pixeldrain.com/u/File12",
        ] {
            assert!(pixeldrain_archive_scope(&file, invalid).is_none());
        }
        for invalid in [
            "https://pixeldrain.com/api/file/File13?download",
            "https://pixeldrain.com/api/file/File12?attach",
            "https://pixeldrain.com/api/file/File12?download&secret=1",
            "https://pixeldrain.com.evil.test/api/file/File12?download",
        ] {
            let item = MediaItem {
                url: invalid.into(),
                ..file.clone()
            };
            assert!(pixeldrain_archive_scope(&item, "https://pixeldrain.com/u/File12").is_none());
        }
        drop(archive);
        let bytes = fs::read(native).unwrap();
        for plaintext in ["File12", "summer one"] {
            assert!(!bytes
                .windows(plaintext.len())
                .any(|part| part == plaintext.as_bytes()));
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pixeldrain_import_requires_proven_file_category_and_survives_reopen() {
        let root =
            std::env::temp_dir().join(format!("galerith-pixeldrain-import-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let foreign = root.join("foreign.sqlite");
        let db = Connection::open(&foreign).unwrap();
        db.execute_batch(
            "CREATE TABLE archive(entry TEXT PRIMARY KEY) WITHOUT ROWID;
             INSERT INTO archive VALUES ('pixeldrainFile12');
             INSERT INTO archive VALUES ('pixeldrainFile13');
             INSERT INTO archive VALUES ('pixeldrainbad%2fid');
             CREATE TABLE named(entry TEXT PRIMARY KEY) WITHOUT ROWID;
             INSERT INTO named VALUES ('File12');
             INSERT INTO named VALUES ('pixeldrainFile12');",
        )
        .unwrap();
        drop(db);
        let source = fs::read(&foreign).unwrap();
        let native = root.join("native.sqlite");
        let archive = Archive::open_with_reference(&native, Some(&foreign)).unwrap();
        assert_eq!(archive.import_reference().unwrap(), (2, 2));
        assert_eq!(archive.import_reference().unwrap(), (2, 0));
        drop(archive);
        let ambiguous = Archive::open_with_reference_table(
            &root.join("ambiguous.sqlite"),
            Some(&foreign),
            "named",
        )
        .unwrap();
        assert_eq!(ambiguous.import_reference().unwrap(), (0, 0));
        drop(ambiguous);
        let named = Archive::open_with_reference_options(
            &root.join("named.sqlite"),
            Some(&foreign),
            Some("named"),
            Some("pixeldrain"),
        )
        .unwrap();
        assert_eq!(named.import_reference().unwrap(), (1, 1));
        drop(named);
        assert_eq!(fs::read(&foreign).unwrap(), source);
        fs::remove_file(foreign).unwrap();
        let archive = Archive::open(&native).unwrap();
        let file = MediaItem {
            url: "https://pixeldrain.com/api/file/File12?download".into(),
            filename: "new title.jpg".into(),
            extractor: "pixeldrain",
            post_id: None,
        };
        assert!(archive
            .contains_for_source(&file, "https://pixeldrain.com/l/album_99")
            .unwrap());
        let other = MediaItem {
            url: "https://pixeldrain.com/api/file/File14?download".into(),
            ..file
        };
        assert!(!archive
            .contains_for_source(&other, "https://pixeldrain.com/l/album_99")
            .unwrap());
        drop(archive);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn listal_originals_keep_separate_ids_with_same_post() {
        let root = std::env::temp_dir().join(format!("galerith-listal-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let native = root.join("native.sqlite");
        let archive = Archive::open(&native).unwrap();
        let first = MediaItem {
            url: "https://image.listal.com/pics/full.png".into(),
            filename: "12345_full.png".into(),
            extractor: "listal",
            post_id: None,
        };
        let rotated = MediaItem {
            url: "https://cdn.lisimg.com/new/full.png".into(),
            ..first.clone()
        };
        let second = MediaItem {
            url: "https://image.listal.com/pics/other.png".into(),
            filename: "12345_other.png".into(),
            ..first.clone()
        };
        let source = "https://www.listal.com/viewimage/12345";
        archive.record_for_source(&first, source).unwrap();
        assert_eq!(
            listal_archive_scope(&first, source).unwrap().1,
            "listal12345/full"
        );
        assert!(archive.contains_for_source(&rotated, source).unwrap());
        assert!(archive
            .contains_for_source(&rotated, "https://www.listal.com/viewimage/12345/")
            .unwrap());
        assert!(!archive.contains_for_source(&second, source).unwrap());
        for bad in [
            "https://www.listal.com/viewimage/012345",
            "https://www.listal.com/alice/pictures",
            "https://listal.com/viewimage/54321",
            "https://listal.com/viewimage/12345?token=private",
            "https://evil.listal.com/viewimage/12345",
        ] {
            assert!(listal_archive_scope(&first, bad).is_none());
        }
        for wrong in [
            "https://image.listal.com/pics/other.png",
            "https://image.listal.com.evil.test/pics/full.png",
            "https://image.listal.com/pics/full.png?token=private",
        ] {
            let item = MediaItem {
                url: wrong.into(),
                ..first.clone()
            };
            assert!(listal_archive_scope(&item, source).is_none());
        }
        for bad in ["012345/full", "12345/../full", "12345/", "12345/full%2f"] {
            assert!(!canonical_listal_archive_suffix(bad));
        }
        drop(archive);
        let bytes = fs::read(native).unwrap();
        for secret in ["12345/full", "full.png"] {
            assert!(!bytes
                .windows(secret.len())
                .any(|part| part == secret.as_bytes()));
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn listal_reference_migration_preserves_individual_originals_offline() {
        let root =
            std::env::temp_dir().join(format!("galerith-listal-import-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let foreign = root.join("foreign.sqlite");
        let db = Connection::open(&foreign).unwrap();
        db.execute_batch(
            "CREATE TABLE archive(entry TEXT PRIMARY KEY) WITHOUT ROWID;
             INSERT INTO archive VALUES ('listal12345/full');
             INSERT INTO archive VALUES ('listal12345/other');
             INSERT INTO archive VALUES ('listal012345/full');
             CREATE TABLE named(entry TEXT PRIMARY KEY) WITHOUT ROWID;
             INSERT INTO named VALUES ('12345/full');
             INSERT INTO named VALUES ('listal12345/full');",
        )
        .unwrap();
        drop(db);
        let before = fs::read(&foreign).unwrap();
        let native = root.join("native.sqlite");
        let archive = Archive::open_with_reference(&native, Some(&foreign)).unwrap();
        assert_eq!(archive.import_reference().unwrap(), (2, 2));
        assert_eq!(archive.import_reference().unwrap(), (2, 0));
        drop(archive);
        let ambiguous = Archive::open_with_reference_table(
            &root.join("ambiguous.sqlite"),
            Some(&foreign),
            "named",
        )
        .unwrap();
        assert_eq!(ambiguous.import_reference().unwrap(), (0, 0));
        drop(ambiguous);
        let named = Archive::open_with_reference_options(
            &root.join("named.sqlite"),
            Some(&foreign),
            Some("named"),
            Some("listal"),
        )
        .unwrap();
        assert_eq!(named.import_reference().unwrap(), (1, 1));
        drop(named);
        assert_eq!(fs::read(&foreign).unwrap(), before);
        fs::remove_file(foreign).unwrap();
        let offline = Archive::open(&native).unwrap();
        let item = MediaItem {
            url: "https://cdn.lisimg.com/rotated/full.png".into(),
            filename: "12345_full.png".into(),
            extractor: "listal",
            post_id: None,
        };
        assert!(offline
            .contains_for_source(&item, "https://www.listal.com/viewimage/12345")
            .unwrap());
        let different = MediaItem {
            url: "https://cdn.lisimg.com/rotated/third.png".into(),
            filename: "12345_third.png".into(),
            ..item
        };
        assert!(!offline
            .contains_for_source(&different, "https://www.listal.com/viewimage/12345")
            .unwrap());
        drop(offline);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn imgur_album_and_single_image_identities_keep_originals_separate() {
        let root = std::env::temp_dir().join(format!("galerith-imgur-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let archive = Archive::open(&root.join("native.sqlite")).unwrap();
        let media = MediaItem {
            url: "https://i.imgur.com/abcdefg.jpg".into(),
            filename: "imgur_abcde_001_abcdefg.jpg".into(),
            extractor: "imgur",
            post_id: None,
        };
        let other_original = MediaItem {
            url: "https://i.imgur.com/qwertyu.jpg".into(),
            filename: "imgur_abcde_002_qwertyu.jpg".into(),
            ..media.clone()
        };
        let same_album = "https://imgur.com/a/abcde";
        let other_album = "https://imgur.com/a/fghij";
        let direct = "https://imgur.com/abcdefg";
        assert_eq!(
            imgur_archive_scope(&media, same_album).unwrap().1,
            "imgurabcde_abcdefg"
        );
        assert_eq!(
            imgur_archive_scope(&media, direct).unwrap().1,
            "imgurabcdefg"
        );
        archive.record_for_source(&media, same_album).unwrap();
        assert!(archive.contains_for_source(&media, same_album).unwrap());
        assert!(!archive
            .contains_for_source(&other_original, same_album)
            .unwrap());
        assert!(!archive.contains_for_source(&media, other_album).unwrap());
        assert!(!archive.contains_for_source(&media, direct).unwrap());
        assert!(!archive
            .contains_for_source(&media, "https://imgur.com/gallery/abcde")
            .unwrap());
        archive.record_for_source(&media, direct).unwrap();
        assert!(archive.contains_for_source(&media, direct).unwrap());
        for invalid in [
            "https://imgur.com/abcde",
            "https://imgur.com/abcdefg?auth=secret",
            "https://imgur.com:9443/abcdefg",
            "http://imgur.com/abcdefg",
            "https://attacker.invalid/abcdefg",
            "https://imgur.com/a/abcde?index=1",
        ] {
            assert!(!archive.contains_for_source(&media, invalid).unwrap());
        }
        let wrong_host = MediaItem {
            url: "https://attacker.invalid/abcdefg.jpg".into(),
            ..media.clone()
        };
        assert!(imgur_archive_scope(&wrong_host, direct).is_none());
        assert!(canonical_imgur_archive_suffix("abcdefg"));
        assert!(canonical_imgur_archive_suffix("abcde_abcdefg"));
        for malformed in [
            "abcd",
            "abcdefgh",
            "abcd_abcdefg",
            "abcde_abcdefgh",
            "abcde_abcdefg_123",
        ] {
            assert!(!canonical_imgur_archive_suffix(malformed));
        }
        drop(archive);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn imgur_reference_archive_migrates_only_canonical_image_and_album_ids() {
        let root =
            std::env::temp_dir().join(format!("galerith-imgur-import-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let source = root.join("gallery-dl.sqlite");
        let native = root.join("native.sqlite");
        let upstream = Connection::open(&source).unwrap();
        upstream
            .execute_batch(
                "CREATE TABLE archive(entry TEXT PRIMARY KEY) WITHOUT ROWID;
                 INSERT INTO archive VALUES ('imgurabcdefg');
                 INSERT INTO archive VALUES ('imgurabcde_abcdefg');
                 INSERT INTO archive VALUES ('imgurfghij_abcdefg');
                 INSERT INTO archive VALUES ('imgurabcd_abcdefg');
                 INSERT INTO archive VALUES ('imgurabcde_abcdefgh');",
            )
            .unwrap();
        drop(upstream);
        let before = fs::read(&source).unwrap();
        let archive = Archive::open_with_reference(&native, Some(&source)).unwrap();
        assert_eq!(archive.import_reference().unwrap(), (3, 3));
        assert_eq!(archive.import_reference().unwrap(), (3, 0));
        drop(archive);
        assert_eq!(fs::read(&source).unwrap(), before);
        fs::remove_file(source).unwrap();
        let archive = Archive::open(&native).unwrap();
        let media = MediaItem {
            url: "https://i.imgur.com/abcdefg.png".into(),
            filename: "imgur_abcdefg.png".into(),
            extractor: "imgur",
            post_id: None,
        };
        for source in [
            "https://imgur.com/abcdefg",
            "https://imgur.com/a/abcde",
            "https://imgur.com/a/fghij",
        ] {
            assert!(archive.contains_for_source(&media, source).unwrap());
        }
        assert!(!archive
            .contains_for_source(&media, "https://imgur.com/a/abcdf")
            .unwrap());
        drop(archive);
        let bytes = fs::read(&native).unwrap();
        assert!(!bytes
            .windows(b"imgurabcde_abcdefg".len())
            .any(|chunk| chunk == b"imgurabcde_abcdefg"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn imgur_named_table_only_imports_explicit_category_prefixed_keys() {
        let root =
            std::env::temp_dir().join(format!("galerith-imgur-prefix-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let foreign = root.join("reference.sqlite");
        let reference = Connection::open(&foreign).unwrap();
        reference
            .execute_batch(
                "CREATE TABLE selected(entry TEXT PRIMARY KEY) WITHOUT ROWID;
                 INSERT INTO selected VALUES ('abcdefg');
                 INSERT INTO selected VALUES ('imgurabcdefg');
                 INSERT INTO selected VALUES ('abcde_abcdefg');
                 INSERT INTO selected VALUES ('imgurabcde_abcdefg');",
            )
            .unwrap();
        drop(reference);
        let source_bytes = fs::read(&foreign).unwrap();
        let media = MediaItem {
            url: "https://i.imgur.com/abcdefg.jpg".into(),
            filename: "any-destination.jpg".into(),
            extractor: "imgur",
            post_id: None,
        };
        let source = "https://imgur.com/a/abcde";
        let unknown = Archive::open_with_reference_table(
            &root.join("unknown.sqlite"),
            Some(&foreign),
            "selected",
        )
        .unwrap();
        assert_eq!(unknown.import_reference().unwrap(), (0, 0));
        assert!(!unknown.contains_for_source(&media, source).unwrap());
        drop(unknown);
        let path = root.join("native.sqlite");
        let supported = Archive::open_with_reference_options(
            &path,
            Some(&foreign),
            Some("selected"),
            Some("imgur"),
        )
        .unwrap();
        assert_eq!(supported.import_reference().unwrap(), (2, 2));
        assert!(supported.contains_for_source(&media, source).unwrap());
        drop(supported);
        assert_eq!(fs::read(&foreign).unwrap(), source_bytes);
        fs::remove_file(foreign).unwrap();
        let offline = Archive::open(&path).unwrap();
        assert!(offline.contains_for_source(&media, source).unwrap());
        drop(offline);
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn imgur_album_archive_lock_survives_file_extension_change() {
        let root = std::env::temp_dir().join(format!("galerith-imgur-lock-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let db = root.join("native.sqlite");
        let album = "https://imgur.com/a/abcde";
        let original = MediaItem {
            url: "https://i.imgur.com/abcdefg.jpg".into(),
            filename: "imgur_abcde_001_abcdefg.jpg".into(),
            extractor: "imgur",
            post_id: None,
        };
        let rotated = MediaItem {
            url: "https://i.imgur.com/abcdefg.png".into(),
            filename: "imgur_abcde_001_abcdefg.png".into(),
            ..original.clone()
        };
        assert_ne!(key(&original), key(&rotated));
        let first = Archive::open(&db).unwrap();
        let guard = first
            .lock_item_for_source(&original, album)
            .await
            .unwrap()
            .unwrap();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let other_db = db.clone();
        let worker = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async {
                let other = Archive::open(&other_db).unwrap();
                ready_tx.send(()).unwrap();
                let _guard = other
                    .lock_item_for_source(&rotated, album)
                    .await
                    .unwrap()
                    .unwrap();
                done_tx
                    .send(other.contains_for_source(&rotated, album).unwrap())
                    .unwrap();
            });
        });
        ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(done_rx.recv_timeout(Duration::from_millis(150)).is_err());
        first.record_for_source(&original, album).unwrap();
        drop(guard);
        assert!(done_rx.recv_timeout(Duration::from_secs(5)).unwrap());
        worker.join().unwrap();
        drop(first);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn imported_booru_post_ids_need_verified_post_scope() {
        let root = std::env::temp_dir().join(format!(
            "galerith-direct-post-import-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let native = root.join("native.sqlite");
        let foreign = root.join("gallery-dl.sqlite");
        let upstream = Connection::open(&foreign).unwrap();
        upstream
            .execute_batch(
                "CREATE TABLE archive(entry TEXT PRIMARY KEY) WITHOUT ROWID;
                 INSERT INTO archive VALUES ('danbooru123');
                 INSERT INTO archive VALUES ('e621234');
                 INSERT INTO archive VALUES ('e6ai345');
                 INSERT INTO archive VALUES ('e926456');
                 INSERT INTO archive VALUES ('danboorut_fox_123');",
            )
            .unwrap();
        drop(upstream);
        let original_bytes = fs::read(&foreign).unwrap();
        let archive = Archive::open_with_reference(&native, Some(&foreign)).unwrap();
        assert_eq!(archive.import_reference().unwrap(), (5, 5));
        let samples = [
            ("danbooru", "https://danbooru.donmai.us/posts/123", 123),
            ("e621", "https://e621.net/posts/234", 234),
            ("e6ai", "https://e6ai.net/posts/345", 345),
            ("e926", "https://e926.net/posts/456", 456),
        ];
        for (category, source, id) in samples {
            let media = MediaItem {
                url: format!("https://cdn.example.org/{id}.jpg?token=fresh"),
                filename: format!("{id}.jpg"),
                extractor: category,
                post_id: Some(id),
            };
            assert!(archive.contains_for_source(&media, source).unwrap());
            let feed = source.replace("/posts/", "/posts?tags=");
            assert!(!archive.contains_for_source(&media, &feed).unwrap());
        }
        drop(archive);
        assert_eq!(fs::read(&foreign).unwrap(), original_bytes);
        fs::remove_file(foreign).unwrap();
        let reopened = Archive::open(&native).unwrap();
        let media = MediaItem {
            url: "https://cdn.example.org/234.jpg?token=changed".into(),
            filename: "234.jpg".into(),
            extractor: "e621",
            post_id: Some(234),
        };
        assert!(reopened
            .contains_for_source(&media, "https://e621.net/posts/234")
            .unwrap());
        assert!(!reopened
            .contains_for_source(&media, "https://e621.net/posts?tags=fox")
            .unwrap());
        drop(reopened);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn philomena_native_archive_survives_rotated_urls_and_old_digest_rows() {
        let path =
            std::env::temp_dir().join(format!("galerith-philomena-native-{}", std::process::id()));
        let _ = fs::remove_file(&path);
        let archive = Archive::open(&path).unwrap();
        let original = MediaItem {
            url: "https://cdn.example.net/123.jpg?token=old".into(),
            filename: "123.jpg".into(),
            extractor: "derpibooru",
            post_id: Some(123),
        };
        let rotated = MediaItem {
            url: "https://new-cdn.example.net/another/123.png?token=new".into(),
            filename: "123.png".into(),
            ..original.clone()
        };
        archive.record(&original).unwrap();
        assert!(archive.contains(&rotated).unwrap());
        assert_eq!(key(&original), "image-id:123");
        let another_site = MediaItem {
            extractor: "ponybooru",
            ..original.clone()
        };
        assert!(!archive.contains(&another_site).unwrap());
        let another_image = MediaItem {
            post_id: Some(124),
            ..original.clone()
        };
        assert!(!archive.contains(&another_image).unwrap());

        let previously_saved = MediaItem {
            extractor: "furbooru",
            post_id: Some(991),
            ..original.clone()
        };
        archive
            .connection
            .execute(
                "INSERT INTO downloads (category, identity) VALUES (?1, ?2)",
                params![
                    previously_saved.extractor,
                    legacy_url_key(&previously_saved)
                ],
            )
            .unwrap();
        assert!(archive.contains(&previously_saved).unwrap());
        // An exact legacy URL digest is proof of the saved original. The
        // successful archive check promotes it without a second download.
        let migrated: i64 = archive
            .connection
            .query_row(
                "SELECT COUNT(*) FROM downloads WHERE category = ?1 AND identity = ?2",
                params![previously_saved.extractor, key(&previously_saved)],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(migrated, 1);
        let unknown_rotation = MediaItem {
            url: "https://cdn.example.net/991.jpg?token=changed".into(),
            ..previously_saved.clone()
        };
        assert!(archive.contains(&unknown_rotation).unwrap());
        drop(archive);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn philomena_default_archive_ids_are_scoped_by_site_and_post() {
        let root =
            std::env::temp_dir().join(format!("galerith-philomena-archive-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let native = root.join("native.sqlite");
        let reference = root.join("upstream.sqlite");
        let original = Connection::open(&reference).unwrap();
        original
            .execute_batch("CREATE TABLE archive(entry TEXT PRIMARY KEY) WITHOUT ROWID;")
            .unwrap();
        for entry in ["derpibooru123", "ponybooru456", "furbooru789"] {
            original
                .execute("INSERT INTO archive(entry) VALUES (?1)", params![entry])
                .unwrap();
        }
        drop(original);
        let original_bytes = fs::read(&reference).unwrap();
        let archive = Archive::open_with_reference(&native, Some(&reference)).unwrap();
        for (site, id) in [("derpibooru", 123), ("ponybooru", 456), ("furbooru", 789)] {
            let item = MediaItem {
                url: format!("https://cdn.example.net/rotated/{id}.png?auth=changed"),
                filename: format!("{id}.png"),
                extractor: site,
                post_id: Some(id),
            };
            assert!(archive.contains(&item).unwrap(), "{site}:{id}");
            let mut other = item.clone();
            other.post_id = Some(id + 1);
            assert!(!archive.contains(&other).unwrap());
            other.post_id = None;
            assert!(!archive.contains(&other).unwrap());
            other.post_id = Some(id);
            other.extractor = "danbooru";
            assert!(!archive.contains(&other).unwrap());
        }
        let wrong_site = MediaItem {
            url: "https://cdn.example.net/123.jpg".into(),
            filename: "123.jpg".into(),
            extractor: "ponybooru",
            post_id: Some(123),
        };
        assert!(!archive.contains(&wrong_site).unwrap());
        drop(archive);
        assert_eq!(fs::read(&reference).unwrap(), original_bytes);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_or_malformed_external_archive_fails_without_creating_native() {
        let root =
            std::env::temp_dir().join(format!("galerith-external-invalid-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let native = root.join("native.sqlite");
        let missing = root.join("missing.sqlite");
        assert!(Archive::open_with_reference(&native, Some(&missing)).is_err());
        assert!(!native.exists());
        assert!(!missing.exists());
        let invalid = root.join("invalid.sqlite");
        let connection = Connection::open(&invalid).unwrap();
        connection
            .execute_batch("CREATE TABLE archive(entry INTEGER PRIMARY KEY);")
            .unwrap();
        drop(connection);
        assert!(Archive::open_with_reference(&native, Some(&invalid)).is_err());
        assert!(!native.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unrelated_archive_table_does_not_trigger_false_matches() {
        let path = std::env::temp_dir().join(format!(
            "galerith-unrelated-archive-{}.sqlite",
            std::process::id()
        ));
        let _ = fs::remove_file(&path);
        let con = Connection::open(&path).unwrap();
        con.execute_batch("CREATE TABLE archive(other TEXT PRIMARY KEY);")
            .unwrap();
        drop(con);
        let archive = Archive::open(&path).unwrap();
        assert!(!archive.upstream_directlink_archive);
        assert!(!archive
            .contains(&extract_direct_media("https://example.org/image.jpg").unwrap())
            .unwrap());
        drop(archive);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn memory_archive_does_not_create_filesystem_locks() {
        let archive = Archive::open(Path::new(":memory:")).unwrap();
        assert!(archive.lock_directory.is_none());
    }

    #[test]
    fn post_identity_is_scoped_by_extractor() {
        let path =
            std::env::temp_dir().join(format!("galerith-categories-{}.sqlite", std::process::id()));
        let _ = fs::remove_file(&path);
        let archive = Archive::open(&path).unwrap();
        let item = MediaItem {
            url: "https://example.org/post/25".into(),
            filename: "25.jpg".into(),
            extractor: "danbooru",
            post_id: Some(25),
        };
        archive.record(&item).unwrap();
        let mut different = item.clone();
        different.extractor = "directlink";
        assert!(!archive.contains(&different).unwrap());
        different.extractor = "danbooru";
        different.filename = "renamed.jpg".into();
        assert!(archive.contains(&different).unwrap());
        drop(archive);
        fs::remove_file(path).unwrap();
    }
}
