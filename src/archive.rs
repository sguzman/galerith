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
        ] {
            if let Some(id) = entry.strip_prefix(category) {
                if id.parse::<u64>().ok().is_some_and(|n| n > 0) {
                    return Some((category, entry.to_owned()));
                }
                if is_booru_pool_category(category) && canonical_pool_archive_suffix(id) {
                    return Some((category, entry.to_owned()));
                }
                if category == "danbooru" && canonical_favorite_group_archive_suffix(id) {
                    return Some((category, entry.to_owned()));
                }
                if is_e621_favorite_category(category) && canonical_favorite_archive_suffix(id) {
                    return Some((category, entry.to_owned()));
                }
                if is_booru_pool_category(category) && canonical_tag_archive_suffix(id) {
                    return Some((category, entry.to_owned()));
                }
                if category == "danbooru" && canonical_media_asset_archive_suffix(id) {
                    return Some((category, entry.to_owned()));
                }
            }
        }
        return None;
    }

    let prefix = prefix?;
    let bare = entry.strip_prefix(prefix)?;
    if valid_directlink_format(bare) {
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
    ] {
        if prefix == category
            && (bare.parse::<u64>().ok().is_some_and(|n| n > 0)
                || (is_booru_pool_category(category) && canonical_pool_archive_suffix(bare))
                || (category == "danbooru" && canonical_favorite_group_archive_suffix(bare))
                || (is_e621_favorite_category(category) && canonical_favorite_archive_suffix(bare))
                || (is_booru_pool_category(category) && canonical_tag_archive_suffix(bare))
                || (category == "danbooru" && canonical_media_asset_archive_suffix(bare)))
        {
            return Some((category, format!("{category}{bare}")));
        }
    }
    None
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
            if philomena_id(item).is_some() && prefix != item.extractor {
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
    if parsed.scheme() != "https" || !parsed.username().is_empty() || parsed.password().is_some() {
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
    if parsed.scheme() != "https" || !parsed.username().is_empty() || parsed.password().is_some() {
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
    if source.scheme() != "https" || !source.username().is_empty() || source.password().is_some() {
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

/// Scoped identity unifies native archive records, advisory locks and
/// read-only upstream lookup for verified posts, pools, favorite groups,
/// tag searches, media assets and account-specific e621-family favorites.
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
    let asset = danbooru_media_asset(item, source_url)?;
    Some((format!("media-asset-id:{asset}"), format!("danboorum{asset}")))
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
        let stable_scope = booru_archive_scope(item, source_url).map(|(native, _)| native);
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
    pub fn contains_for_source(
        &self,
        item: &MediaItem,
        source_url: &str,
    ) -> Result<bool, rusqlite::Error> {
        let Some((stable_key, normalized)) = booru_archive_scope(item, source_url) else {
            if self.contains(item)? {
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
                if verified {
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
        if self.contains(item)? {
            // A prior exact source-URL hash proves the same media was
            // downloaded. Promote it for future direct-post CDN rotations.
            self.connection.execute(
                "INSERT OR IGNORE INTO downloads (category, identity) VALUES (?1, ?2)",
                params![item.extractor, stable_key],
            )?;
            return Ok(true);
        }
        if item.extractor == "e926" {
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

    /// Preserve direct-post stable identity only after an accepted file
    /// event, while keeping old URL-hash archive rows compatible.
    pub fn record_for_source(
        &self,
        item: &MediaItem,
        source_url: &str,
    ) -> Result<(), rusqlite::Error> {
        let Some((stable_key, _)) = booru_archive_scope(item, source_url) else {
            return self.record(item);
        };
        let transaction = self.connection.unchecked_transaction()?;
        transaction.execute(
            "INSERT OR IGNORE INTO downloads (category, identity) VALUES (?1, ?2)",
            params![item.extractor, key(item)],
        )?;
        transaction.execute(
            "INSERT OR IGNORE INTO downloads (category, identity) VALUES (?1, ?2)",
            params![item.extractor, stable_key],
        )?;
        transaction.commit()
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
        assert!(archive.contains(&media).unwrap());
        let native: i64 = archive
            .connection
            .query_row(
                "SELECT COUNT(*) FROM downloads WHERE category = 'e926'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(native, 2);
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
