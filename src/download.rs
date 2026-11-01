//! Conservative resumable HTTP downloader.
//! A partial file is never appended without a matching source fingerprint and
//! an HTTP validator. This is a safe subset of gallery-dl's HTTP semantics.
use crate::pacing::{DelayRange, RateRange};
use crate::{safe_filename, MediaItem};
use fs2::FileExt;
use reqwest::{
    header::{
        HeaderMap, ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_RANGE, CONTENT_TYPE, ETAG, IF_RANGE,
        LAST_MODIFIED, RANGE, RETRY_AFTER,
    },
    Client, StatusCode,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    error::Error,
    io,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{fs, io::AsyncWriteExt};

#[derive(Debug, Clone)]
pub struct DownloadOptions {
    pub destination: PathBuf,
    pub retries: u32,
    pub no_skip: bool,
    pub no_mtime: bool,
    /// Write directly to the destination; failed transfers may leave partial output.
    pub no_part: bool,
    /// Optional maximum number of response bytes per second.
    pub limit_rate: Option<RateRange>,
    /// Additional downloader-only headers from explicit configuration.
    pub headers: HeaderMap,
    /// Optional fixed or ranged pause between retries; Retry-After takes precedence.
    pub retry_delay: Option<DelayRange>,
    /// Optional fixed or ranged pause for 429 responses without a positive Retry-After.
    pub rate_limit_delay: Option<DelayRange>,
    /// Optional delay before each media HTTP attempt, including retries.
    pub request_delay: Option<DelayRange>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum DownloadOutcome {
    Saved(PathBuf),
    Skipped(PathBuf),
}

/// Advisory locks live in stable per-target files and are released by the OS
/// even if a process crashes. Never unlink a lockfile while another process
/// might have opened it: doing so can split the lock namespace.
struct OutputLock(std::fs::File);

impl Drop for OutputLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

async fn lock_output(
    dir: &Path,
    filename: &str,
) -> Result<OutputLock, Box<dyn Error + Send + Sync>> {
    let path = dir.join(format!(".{filename}.galerith.lock"));
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(OutputLock(file)),
            Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => {
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
            Err(err) => return Err(err.into()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PartMetadata {
    source_sha256: String,
    etag: Option<String>,
    last_modified: Option<String>,
    // Optional for backwards compatibility with pre-invariant .part.json files.
    #[serde(default)]
    total_length: Option<u64>,
}

impl PartMetadata {
    fn for_response(url: &str, response: &reqwest::Response) -> Self {
        Self {
            source_sha256: fingerprint(url),
            etag: header(response, ETAG),
            last_modified: header(response, LAST_MODIFIED),
            total_length: response.content_length(),
        }
    }

    fn validator(&self) -> Option<&str> {
        // Weak entity tags are unsuitable for If-Range.
        self.etag
            .as_deref()
            .filter(|tag| !tag.starts_with("W/"))
            .or(self.last_modified.as_deref())
    }
}

fn fingerprint(source: &str) -> String {
    let hash = Sha256::digest(source.as_bytes());
    hash.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn header(response: &reqwest::Response, name: reqwest::header::HeaderName) -> Option<String> {
    response
        .headers()
        .get(name)?
        .to_str()
        .ok()
        .map(str::to_owned)
}

/// A synthetic success response containing an HTML/JSON error document
/// cannot complete an unrelated media, PDF, archive or other direct file.
/// Legitimate HTML/JSON downloads with corresponding extensions are allowed.
fn media_error_document(item: &MediaItem, content_type: &str) -> bool {
    let extension = item
        .filename
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase());
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    match mime.as_str() {
        "text/html" => !matches!(extension.as_deref(), Some("html" | "htm")),
        "application/xhtml+xml" => extension.as_deref() != Some("xhtml"),
        "application/json" => !matches!(extension.as_deref(), Some("json" | "jsonl")),
        "application/problem+json" => true,
        _ => false,
    }
}

/// Reject a specific media type when the output suffix clearly identifies a
/// different format. Unknown and generic types (especially octet-stream) stay
/// permissive because many CDNs omit a useful type or serve vendor formats.
fn media_type_mismatches_extension(item: &MediaItem, content_type: &str) -> bool {
    let extension = item
        .filename
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase());
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let Some(extension) = extension.as_deref() else {
        // Custom filename templates may omit the source suffix entirely, so
        // there is no extension claim to compare with the response type.
        return false;
    };
    let expected = match mime.as_str() {
        "image/jpeg" => Some(&["jpg", "jpeg", "jpe"][..]),
        "image/apng" => Some(&["apng", "png"][..]),
        "image/png" => Some(&["png"][..]),
        "image/gif" => Some(&["gif"][..]),
        "image/webp" => Some(&["webp"][..]),
        "image/avif" => Some(&["avif"][..]),
        "image/heic" => Some(&["heic"][..]),
        "image/heif" => Some(&["heif"][..]),
        "image/vnd.adobe.photoshop" => Some(&["psd"][..]),
        "image/bmp" => Some(&["bmp"][..]),
        "image/tiff" => Some(&["tif", "tiff"][..]),
        "image/svg+xml" => Some(&["svg"][..]),
        "image/jxl" => Some(&["jxl"][..]),
        "image/vnd.microsoft.icon" | "image/x-icon" | "image/ico" => Some(&["ico"][..]),
        "audio/mpeg" | "audio/mp3" | "audio/x-mp3" => Some(&["mp3"][..]),
        "audio/mp4" | "audio/x-m4a" => Some(&["m4a"][..]),
        "audio/aac" => Some(&["aac"][..]),
        "audio/x-ms-wma" => Some(&["wma"][..]),
        "audio/ogg" => Some(&["ogg", "ogv", "opus"][..]),
        "audio/opus" => Some(&["opus"][..]),
        "audio/webm" => Some(&["webm"][..]),
        "audio/wav" | "audio/x-wav" => Some(&["wav"][..]),
        "audio/flac" | "audio/x-flac" => Some(&["flac"][..]),
        "video/mp4" | "video/x-m4v" => Some(&["mp4", "m4v"][..]),
        "video/mpeg" => Some(&["mpeg", "mpg", "vob", "m1v", "m2v"][..]),
        "video/3gpp" => Some(&["3gp"][..]),
        "video/3gpp2" => Some(&["3g2"][..]),
        "video/mp2t" => Some(&["m2ts", "mts"][..]),
        "video/x-msvideo" => Some(&["avi"][..]),
        "video/x-ms-wmv" => Some(&["wmv"][..]),
        "video/webm" => Some(&["webm"][..]),
        "video/quicktime" => Some(&["mov"][..]),
        "video/x-matroska" => Some(&["mkv"][..]),
        "video/ogg" => Some(&["ogm", "ogv"][..]),
        "application/vnd.ms-asf" | "video/x-ms-asf" => Some(&["asf"][..]),
        "application/x-shockwave-flash" => Some(&["swf"][..]),
        "application/pdf" => Some(&["pdf"][..]),
        "application/zip" | "application/x-zip-compressed" => Some(&["zip", "cbz"][..]),
        "application/vnd.rar" | "application/x-rar-compressed" => Some(&["rar"][..]),
        "application/x-7z-compressed" => Some(&["7z"][..]),
        "application/gzip" | "application/x-gzip" => Some(&["gz", "tgz"][..]),
        "application/x-xz" => Some(&["xz"][..]),
        "application/x-tar" => Some(&["tar"][..]),
        "application/x-bzip2" => Some(&["bz2"][..]),
        "application/zstd" | "application/x-zstd" => Some(&["zst"][..]),
        _ => None,
    };
    expected.is_some_and(|extensions| !extensions.contains(&extension))
}

/// Some servers label an HTML or JSON block page as application/octet-stream
/// or omit Content-Type entirely. Inspect only a bounded initial body prefix
/// for established binary/document outputs, never the full download.
fn looks_like_soft_error_body(item: &MediaItem, first: &[u8]) -> bool {
    let extension = item
        .filename
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase());
    let binary_extensions = [
        "jpg", "jpeg", "jpe", "png", "apng", "gif", "webp", "avif", "bmp", "tif", "tiff", "jxl",
        "ico", "heic", "heif", "psd", "svg", "mp4", "webm", "mov", "mkv", "avi", "mpeg", "mpg",
        "m4v", "vob", "m1v", "m2v", "3gp", "3g2", "m2ts", "mts", "asf", "mp3", "m4a", "aac", "wav",
        "flac", "ogg", "ogm", "ogv", "opus", "wma", "pdf", "zip", "cbz", "rar", "7z", "tar", "tgz",
        "gz", "bz2", "xz", "zst", "swf", "wmv",
    ];
    if !extension
        .as_deref()
        .is_some_and(|ext| binary_extensions.contains(&ext))
    {
        return false;
    }
    let prefix = first.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(first);
    let trimmed = prefix
        .iter()
        .copied()
        .skip_while(u8::is_ascii_whitespace)
        .take(160)
        .map(|byte| byte.to_ascii_lowercase())
        .collect::<Vec<_>>();
    trimmed.starts_with(b"<!doctype html")
        || trimmed.starts_with(b"<html")
        || trimmed.starts_with(b"<head")
        || trimmed.starts_with(b"<body")
        || (trimmed.starts_with(b"{")
            && (trimmed.windows(7).any(|part| part == b"\"error\"")
                || trimmed.windows(8).any(|part| part == b"\"errors\"")))
}

fn error(message: &str) -> Box<dyn Error + Send + Sync> {
    io::Error::new(io::ErrorKind::InvalidData, message).into()
}

/// Strictly parse "bytes start-end/total"; unknown totals are not accepted.
/// A server may return a shorter contiguous 206 chunk than requested, but
/// only a response completing the advertised total can finalize the file.
fn resumed_length(raw: &str, offset: u64) -> Option<(u64, u64)> {
    let value = raw.strip_prefix("bytes ")?;
    let (range, total) = value.split_once('/')?;
    let (start, end) = range.split_once('-')?;
    let start = start.parse::<u64>().ok()?;
    let end = end.parse::<u64>().ok()?;
    let total = total.parse::<u64>().ok()?;
    if start != offset || end < start || end.checked_add(1)? > total {
        return None;
    }
    Some((end.checked_sub(start)?.checked_add(1)?, total))
}

async fn partial_info(path: &Path, metadata_path: &Path, url: &str) -> (u64, Option<PartMetadata>) {
    let size = match fs::metadata(path).await {
        Ok(info) if info.is_file() => info.len(),
        _ => return (0, None),
    };
    if size == 0 {
        return (0, None);
    }
    let meta = match fs::read(metadata_path)
        .await
        .ok()
        .and_then(|bytes| serde_json::from_slice::<PartMetadata>(&bytes).ok())
    {
        Some(meta) => meta,
        None => return (0, None),
    };
    if meta.source_sha256 != fingerprint(url)
        || meta.validator().is_none()
        || meta.total_length.is_some_and(|total| size >= total)
    {
        return (0, None);
    }
    (size, Some(meta))
}

async fn delay(attempt: u32, retry_after: Option<Duration>, configured: Option<DelayRange>) {
    let fallback = configured
        .map(DelayRange::sample)
        .unwrap_or_else(|| Duration::from_millis(250 * (1_u64 << attempt.min(5))));
    tokio::time::sleep(retry_after.unwrap_or(fallback)).await;
}

fn parse_retry_after(value: &str, now: std::time::SystemTime) -> Option<Duration> {
    let delay = if let Ok(seconds) = value.trim().parse::<u64>() {
        Duration::from_secs(seconds.min(60))
    } else {
        let date = httpdate::parse_http_date(value.trim()).ok()?;
        date.duration_since(now).unwrap_or(Duration::ZERO)
    };
    Some(delay.min(Duration::from_secs(60)))
}

fn retry_after(response: &reqwest::Response) -> Option<Duration> {
    let value = header(response, RETRY_AFTER)?;
    parse_retry_after(&value, std::time::SystemTime::now())
}

async fn invalidate(part: &Path, metadata: &Path) -> io::Result<()> {
    if fs::try_exists(part).await? {
        fs::remove_file(part).await?;
    }
    if fs::try_exists(metadata).await? {
        fs::remove_file(metadata).await?;
    }
    Ok(())
}

async fn invalidate_for_mode(part: &Path, metadata: &Path, no_part: bool) -> io::Result<()> {
    if no_part {
        if fs::try_exists(metadata).await? {
            fs::remove_file(metadata).await?;
        }
        Ok(())
    } else {
        invalidate(part, metadata).await
    }
}

/// Never trust a preexisting destination or resume-sidecar alias. In
/// particular, File::create follows symlinks, and a symlinked "completed"
/// output could otherwise be falsely archived through the skip event.
/// This is a cooperative-process safety check, not protection against an
/// adversary swapping paths concurrently after validation.
async fn verify_output_entry(path: &Path, staging: bool) -> io::Result<()> {
    let info = match fs::symlink_metadata(path).await {
        Ok(info) => info,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };
    if !info.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "output or resume entry is not a regular file",
        ));
    }
    #[cfg(unix)]
    if staging {
        use std::os::unix::fs::MetadataExt;
        if info.nlink() > 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "resume entry has multiple hard links",
            ));
        }
    }
    #[cfg(not(unix))]
    let _ = staging;
    Ok(())
}

/// Publish the small resume validator file atomically. A killed worker may
/// leave a .tmp file, but must never leave a half-written canonical sidecar.
/// Use create_new so a stale or symlinked temp path cannot be followed.
async fn write_metadata_atomically(path: &Path, info: &PartMetadata) -> io::Result<()> {
    let pending = path.with_extension("json.tmp");
    match fs::symlink_metadata(&pending).await {
        Ok(info) if !info.is_dir() => fs::remove_file(&pending).await?,
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "resume metadata temporary path is a directory",
            ));
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }
    let encoded = serde_json::to_vec(info).map_err(io::Error::other)?;
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&pending)
        .await?;
    let written: io::Result<()> = async {
        output.write_all(&encoded).await?;
        output.sync_all().await
    }
    .await;
    drop(output);
    if let Err(err) = written {
        let _ = fs::remove_file(&pending).await;
        return Err(err);
    }
    if let Err(err) = fs::rename(&pending, path).await {
        let _ = fs::remove_file(&pending).await;
        return Err(err);
    }
    Ok(())
}

pub async fn download(
    client: &Client,
    item: &MediaItem,
    options: &DownloadOptions,
) -> Result<DownloadOutcome, Box<dyn Error + Send + Sync>> {
    // MediaItem is public, so validate even if a caller bypasses an extractor.
    if item.filename.is_empty()
        || item.filename == "."
        || item.filename == ".."
        || safe_filename(&item.filename) != item.filename
    {
        return Err(error("unsafe output filename"));
    }

    fs::create_dir_all(&options.destination).await?;
    // Hold this OS advisory lock through the existence check, transfer and
    // rename. Parallel Galerith workers cannot clobber the same .part file.
    let _lock = lock_output(&options.destination, &item.filename).await?;
    let target = options.destination.join(&item.filename);
    let part = if options.no_part {
        target.clone()
    } else {
        options.destination.join(format!("{}.part", item.filename))
    };
    let metadata = options
        .destination
        .join(format!("{}.part.json", item.filename));
    let limit_rate = options.limit_rate.map(RateRange::sample);
    verify_output_entry(&target, false).await?;
    verify_output_entry(&part, true).await?;
    verify_output_entry(&metadata, true).await?;
    if !options.no_skip && fs::try_exists(&target).await? {
        let existing = fs::metadata(&target).await?;
        // A zero-byte placeholder is not completed media and must not become
        // an archive hit via --archive-event skip.
        if existing.is_file() && existing.len() > 0 {
            return Ok(DownloadOutcome::Skipped(target));
        }
    }

    for attempt in 0..=options.retries {
        let (offset, prior) = if options.no_part {
            (0, None)
        } else {
            partial_info(&part, &metadata, &item.url).await
        };
        // Resume offsets refer to the raw representation: opt out of
        // HTTP content encoding to keep sidecar byte counts meaningful.
        let mut request = client.get(&item.url).header(ACCEPT_ENCODING, "identity");
        request = request.headers(options.headers.clone());
        crate::pacing::wait_for_request_url(
            &item.url,
            options.request_delay.map(DelayRange::sample),
        )
        .await;
        if offset > 0 {
            request = request.header(RANGE, format!("bytes={offset}-")).header(
                IF_RANGE,
                prior
                    .as_ref()
                    .and_then(PartMetadata::validator)
                    .ok_or_else(|| error("resume validator missing"))?,
            );
        }

        let response = match request.send().await {
            Ok(response) => response,
            Err(err) if attempt < options.retries => {
                let _ = err;
                delay(attempt, None, options.retry_delay).await;
                continue;
            }
            Err(err) => return Err(err.into()),
        };

        let status = response.status();
        if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
            let after = retry_after(&response);
            if let Some(after) = after {
                crate::pacing::defer_host_for_url(&item.url, after).await;
            }
            if attempt < options.retries {
                if after.is_none_or(|delay| delay.is_zero()) {
                    let configured_delay = if status == StatusCode::TOO_MANY_REQUESTS {
                        options.rate_limit_delay.or(options.retry_delay)
                    } else {
                        options.retry_delay
                    };
                    delay(attempt, None, configured_delay).await;
                }
                continue;
            }
        }
        if offset > 0 && status == StatusCode::RANGE_NOT_SATISFIABLE {
            invalidate_for_mode(&part, &metadata, options.no_part).await?;
            if attempt < options.retries {
                continue;
            }
            return Err(error(
                "range not satisfiable; partial reset, retries exhausted",
            ));
        }
        // Neither 304 nor 412 provides the requested media suffix. A server
        // may return them for an If-Range request despite the usual 200
        // fallback. Clear the stale validator/partial before a full retry.
        if offset > 0
            && matches!(
                status,
                StatusCode::NOT_MODIFIED | StatusCode::PRECONDITION_FAILED
            )
        {
            invalidate_for_mode(&part, &metadata, options.no_part).await?;
            if attempt < options.retries {
                continue;
            }
            return Err(error("resume precondition rejected; partial reset"));
        }
        let mut response = response.error_for_status()?;
        // A client configured without redirects may return a successful
        // 3xx response body, and a 202/203 is not a complete media download.
        // Only full 200 and explicitly validated 206 responses can be saved.
        if !matches!(status, StatusCode::OK | StatusCode::PARTIAL_CONTENT) {
            return Err(error("unexpected HTTP status for completed media"));
        }
        // Some image hosts return a branded HTML/JSON block page with 200.
        // Never archive it as completed image or video content.
        if header(&response, CONTENT_TYPE).is_some_and(|mime| {
            media_error_document(item, &mime) || media_type_mismatches_extension(item, &mime)
        }) {
            // Neither a 206 error suffix nor a 200 block page replacing a
            // Range request can satisfy the media transfer. A stale partial
            // must not make the next retry repeat the unusable range.
            if offset > 0 {
                invalidate_for_mode(&part, &metadata, options.no_part).await?;
                if attempt < options.retries {
                    continue;
                }
            }
            return Err(error(
                "incompatible Content-Type instead of requested media",
            ));
        }
        // Even when identity was requested, an HTTP server can ignore it.
        // Saving encoded response bytes under the original filename or
        // appending them to an identity partial would corrupt the media.
        if header(&response, CONTENT_ENCODING)
            .is_some_and(|encoding| !encoding.eq_ignore_ascii_case("identity"))
        {
            invalidate_for_mode(&part, &metadata, options.no_part).await?;
            if attempt < options.retries {
                continue;
            }
            return Err(error("non-identity Content-Encoding; partial reset"));
        }
        let append = offset > 0 && status == StatusCode::PARTIAL_CONTENT;
        if status == StatusCode::PARTIAL_CONTENT && !append {
            // A server must not choose a partial representation for a full
            // GET. Never write it as a complete media file or archive hit.
            invalidate_for_mode(&part, &metadata, options.no_part).await?;
            if attempt < options.retries {
                continue;
            }
            return Err(error("unexpected partial response without a range request"));
        }

        let resume_range = if append {
            let header = match header(&response, CONTENT_RANGE) {
                Some(value) => value,
                None => {
                    // An invalid 206 is not recoverable using the stale range:
                    // drop both persisted partial components before retrying.
                    invalidate_for_mode(&part, &metadata, options.no_part).await?;
                    if attempt < options.retries {
                        continue;
                    }
                    return Err(error("missing Content-Range; partial reset"));
                }
            };
            match resumed_length(&header, offset) {
                Some(range) => Some(range),
                None => {
                    invalidate_for_mode(&part, &metadata, options.no_part).await?;
                    if attempt < options.retries {
                        continue;
                    }
                    return Err(error("invalid Content-Range; partial reset"));
                }
            }
        } else {
            None
        };
        if let Some((expected, total)) = resume_range {
            if prior
                .as_ref()
                .and_then(|old| old.total_length)
                .is_some_and(|original_total| original_total != total)
            {
                // Even matching validators do not authorize a silently
                // changing representation size across resumed chunks.
                invalidate_for_mode(&part, &metadata, options.no_part).await?;
                if attempt < options.retries {
                    continue;
                }
                return Err(error("resumed representation total changed; partial reset"));
            }
            if response
                .content_length()
                .is_some_and(|declared| declared != expected)
            {
                // The claimed suffix and the declared transport body length
                // disagree before any bytes are appended. The stale partial
                // is not a trustworthy basis for the next Range request.
                invalidate_for_mode(&part, &metadata, options.no_part).await?;
                if attempt < options.retries {
                    continue;
                }
                return Err(error("resumed Content-Length disagrees with Content-Range"));
            }
        }
        if append {
            if let Some(old) = prior.as_ref() {
                // An If-Range server response without confirming validator is
                // accepted, but conflicting validators are never appended.
                let new_etag = header(&response, ETAG);
                let new_modified = header(&response, LAST_MODIFIED);
                if old
                    .etag
                    .as_ref()
                    .zip(new_etag.as_ref())
                    .is_some_and(|(a, b)| a != b)
                    || old
                        .last_modified
                        .as_ref()
                        .zip(new_modified.as_ref())
                        .is_some_and(|(a, b)| a != b)
                {
                    invalidate_for_mode(&part, &metadata, options.no_part).await?;
                    if attempt < options.retries {
                        continue;
                    }
                    return Err(error("resource validator changed during resume"));
                }
            }
        }

        if !append {
            // A fresh representation never inherits a previous partial or
            // sidecar; clear both before publishing its validated metadata.
            invalidate_for_mode(&part, &metadata, options.no_part).await?;
            if !options.no_part {
                write_metadata_atomically(
                    &metadata,
                    &PartMetadata::for_response(&item.url, &response),
                )
                .await?;
            }
        } else if let (Some((_, total)), Some(old)) = (resume_range, prior.as_ref()) {
            if old.total_length.is_none() {
                // Upgrade a legacy sidecar as soon as the first validated 206
                // reveals the full representation size. Persist before append
                // so another invocation cannot silently change it.
                let mut upgraded = old.clone();
                upgraded.total_length = Some(total);
                write_metadata_atomically(&metadata, &upgraded).await?;
            }
        }
        let mut file = if append {
            fs::OpenOptions::new().append(true).open(&part).await?
        } else {
            if options.no_part && fs::try_exists(&part).await? {
                fs::remove_file(&part).await?;
            }
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&part)
                .await?
        };

        // Prefer the current response. Only a validated 206 append can inherit
        // the saved Last-Modified timestamp from the initial response.
        let source_mtime = header(&response, LAST_MODIFIED)
            .or_else(|| {
                append
                    .then(|| prior.as_ref()?.last_modified.clone())
                    .flatten()
            })
            .and_then(|value| httpdate::parse_http_date(&value).ok());
        let expected_body = resume_range
            .map(|(body, _)| body)
            .or_else(|| response.content_length());
        let mut received = 0_u64;
        let transfer_started = tokio::time::Instant::now();
        let mut first_bytes = Vec::with_capacity(160);
        let mut transfer_error: Option<Box<dyn Error + Send + Sync>> = None;
        let mut oversized_response = false;
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) => {
                    if expected_body
                        .is_some_and(|max| received.saturating_add(chunk.len() as u64) > max)
                    {
                        // Never append an overlong chunk or retain any
                        // previously appended bytes from this response.
                        oversized_response = true;
                        transfer_error = Some(error("response exceeds declared body length"));
                        break;
                    }
                    if first_bytes.len() < 160 {
                        let count = (160 - first_bytes.len()).min(chunk.len());
                        first_bytes.extend_from_slice(&chunk[..count]);
                    }
                    if let Err(err) = file.write_all(&chunk).await {
                        transfer_error = Some(err.into());
                        break;
                    }
                    received = received.saturating_add(chunk.len() as u64);
                    if let Some(bytes_per_second) = limit_rate {
                        let target_nanos =
                            (u128::from(received) * 1_000_000_000) / u128::from(bytes_per_second);
                        let elapsed_nanos = transfer_started.elapsed().as_nanos();
                        if target_nanos > elapsed_nanos {
                            let wait_nanos =
                                (target_nanos - elapsed_nanos).min(u128::from(u64::MAX));
                            tokio::time::sleep(Duration::from_nanos(wait_nanos as u64)).await;
                        }
                    }
                }
                Ok(None) => break,
                Err(err) => {
                    transfer_error = Some(err.into());
                    break;
                }
            }
        }
        let misleading_error_body = looks_like_soft_error_body(item, &first_bytes);
        if misleading_error_body {
            transfer_error = Some(error("block page disguised as binary media"));
        }
        if transfer_error.is_none() {
            if received == 0 {
                // A successful HTTP status is not proof that any media bytes
                // arrived. Never finalize or archive an empty transfer.
                transfer_error = Some(error("empty media response"));
            }
            if let Some(expected) = expected_body {
                if received != expected {
                    transfer_error = Some(error("response body length mismatch"));
                }
            }
            if let Some((_, total)) = resume_range {
                if offset.saturating_add(received) > total {
                    transfer_error = Some(error("resumed total length mismatch"));
                }
            }
        }
        if transfer_error.is_none() {
            if let Err(err) = file.flush().await {
                transfer_error = Some(err.into());
            }
        }
        if transfer_error.is_none() {
            // A completed response is not ready for the atomic final rename
            // until the filesystem acknowledges its data and file metadata.
            // A sync error must never create a successful archive event.
            if let Err(err) = file.sync_all().await {
                transfer_error = Some(err.into());
            }
        }
        drop(file);
        if let Some(err) = transfer_error {
            if misleading_error_body || oversized_response || (received == 0 && !append) {
                // Discard deceptive or oversized responses and their sidecars.
                // Never retain an untrustworthy suffix for future resume.
                invalidate_for_mode(&part, &metadata, options.no_part).await?;
            }
            if attempt < options.retries {
                delay(attempt, None, options.retry_delay).await;
                continue;
            }
            // Keep a validated .part pair for the next invocation.
            return Err(err);
        }
        if let Some((_, total)) = resume_range {
            if offset + received < total {
                // A syntactically valid 206 may cover only one subrange.
                // Keep the synchronized partial and original If-Range
                // validator; request the next offset rather than publishing
                // an incomplete representation or an archive event.
                if attempt < options.retries {
                    continue;
                }
                return Err(error("resumed chunk saved; more bytes remain"));
            }
        }
        if !options.no_part {
            fs::rename(&part, &target).await?;
        }
        if !options.no_mtime {
            if let Some(modified) = source_mtime {
                // Last-Modified is optional, and unsupported filesystem timestamp
                // operations must never turn an already-committed media file into
                // an apparent download failure.
                match fs::OpenOptions::new().write(true).open(&target).await {
                    Ok(file) => {
                        if let Err(err) = file.into_std().await.set_modified(modified) {
                            eprintln!("galerith: cannot preserve source modification time: {err}");
                        }
                    }
                    Err(err) => eprintln!("galerith: cannot set source modification time: {err}"),
                }
            }
        }
        // Metadata is auxiliary; a successful media file must stay saved even
        // if sidecar cleanup is interrupted.
        let _ = fs::remove_file(&metadata).await;
        return Ok(DownloadOutcome::Saved(target));
    }
    unreachable!("bounded retry loop always returns")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_resumed_content_range() {
        assert_eq!(resumed_length("bytes 4-9/10", 4), Some((6, 10)));
        assert_eq!(resumed_length("bytes 5-9/10", 4), None);
        assert_eq!(resumed_length("bytes 4-8/10", 4), Some((5, 10)));
        assert_eq!(resumed_length("bytes 4-10/10", 4), None);
        assert_eq!(resumed_length("bytes 4-9/9", 4), None);
        assert_eq!(resumed_length("bytes 4-4/10", 4), Some((1, 10)));
        assert_eq!(resumed_length("bytes 4-9/*", 4), None);
        assert_eq!(resumed_length("bytes 4-3/10", 4), None);
    }

    #[test]
    fn blocked_html_and_json_bodies_cannot_masquerade_as_binary_media() {
        let item = MediaItem {
            url: "https://example.test/media.jpg".into(),
            filename: "photo.jpg".into(),
            extractor: "directlink",
            post_id: None,
        };
        assert!(looks_like_soft_error_body(
            &item,
            b"\xef\xbb\xbf  <!DOCTYPE HTML><html>blocked</html>"
        ));
        assert!(looks_like_soft_error_body(&item, b"<HTML>Forbidden</HTML>"));
        assert!(looks_like_soft_error_body(
            &item,
            b"{ \"error\": \"forbidden\" }"
        ));
        assert!(looks_like_soft_error_body(
            &item,
            b"{\"errors\":[\"denied\"]}"
        ));
        assert!(!looks_like_soft_error_body(&item, b"\xff\xd8\xff\xe0JPEG"));
        let pdf = MediaItem {
            filename: "manual.pdf".into(),
            ..item.clone()
        };
        assert!(looks_like_soft_error_body(&pdf, b"<html>PDF not found"));
        let html = MediaItem {
            filename: "index.html".into(),
            ..item.clone()
        };
        assert!(!looks_like_soft_error_body(&html, b"<html>Valid</html>"));
        let json = MediaItem {
            filename: "data.json".into(),
            ..item
        };
        assert!(!looks_like_soft_error_body(
            &json,
            b"{\"error\":\"actual JSON\"}"
        ));
    }

    #[test]
    fn retry_after_accepts_delta_seconds_and_http_dates() {
        let now = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_445_412_480 - 15);
        assert_eq!(parse_retry_after("5", now), Some(Duration::from_secs(5)));
        assert_eq!(
            parse_retry_after("9999", now),
            Some(Duration::from_secs(60))
        );
        assert_eq!(
            parse_retry_after("Wed, 21 Oct 2015 07:28:00 GMT", now),
            Some(Duration::from_secs(15))
        );
        let past = now + Duration::from_secs(30);
        assert_eq!(
            parse_retry_after("Wed, 21 Oct 2015 07:28:00 GMT", past),
            Some(Duration::ZERO)
        );
        assert_eq!(parse_retry_after("bogus", now), None);
        assert_eq!(parse_retry_after("-2", now), None);
    }

    #[test]
    fn reject_mismatched_soft_error_documents_for_direct_files() {
        let item = MediaItem {
            url: "https://example.test/media.jpg".into(),
            filename: "photo.jpg".into(),
            extractor: "directlink",
            post_id: None,
        };
        assert!(media_error_document(&item, "text/html; charset=UTF-8"));
        assert!(media_error_document(&item, "APPLICATION/JSON"));
        assert!(media_error_document(&item, "application/problem+json"));
        assert!(!media_error_document(&item, "image/jpeg"));
        assert!(!media_error_document(&item, "application/octet-stream"));
        let html = MediaItem {
            filename: "document.html".into(),
            ..item
        };
        assert!(!media_error_document(&html, "text/html"));
        assert!(media_error_document(&html, "application/problem+json"));
        let pdf = MediaItem {
            filename: "documents.pdf".into(),
            ..html.clone()
        };
        assert!(media_error_document(&pdf, "text/html"));
        assert!(media_error_document(&pdf, "application/json"));
        assert!(!media_error_document(&pdf, "application/pdf"));
        let archive = MediaItem {
            filename: "media.7z".into(),
            ..html.clone()
        };
        assert!(media_error_document(&archive, "text/html"));
        let json = MediaItem {
            filename: "data.json".into(),
            ..html
        };
        assert!(!media_error_document(&json, "application/json"));
        assert!(media_error_document(&json, "text/html"));
        assert!(media_error_document(&json, "application/problem+json"));
    }

    #[test]
    fn known_media_mime_types_match_expected_extensions() {
        let item = MediaItem {
            url: "https://example.test/media.jpg".into(),
            filename: "photo.jpg".into(),
            extractor: "directlink",
            post_id: None,
        };
        assert!(!media_type_mismatches_extension(
            &item,
            "image/jpeg; charset=binary"
        ));
        assert!(media_type_mismatches_extension(&item, "image/png"));
        assert!(media_type_mismatches_extension(&item, "application/pdf"));
        let templated_name = MediaItem {
            filename: "custom-output".into(),
            ..item.clone()
        };
        assert!(!media_type_mismatches_extension(
            &templated_name,
            "image/jpeg"
        ));
        assert!(!media_type_mismatches_extension(
            &item,
            "application/octet-stream"
        ));
        assert!(!media_type_mismatches_extension(
            &item,
            "application/vnd.site-media"
        ));

        for (filename, mime) in [
            ("cover.jpeg", "image/jpeg"),
            ("animation.apng", "image/apng"),
            ("favicon.ico", "image/vnd.microsoft.icon"),
            ("favicon.ico", "image/x-icon"),
            ("favicon.ico", "image/ico"),
            ("poster.tiff", "image/tiff"),
            ("raw.heic", "image/heic"),
            ("raw.heif", "image/heif"),
            ("design.psd", "image/vnd.adobe.photoshop"),
            ("sound.mp3", "audio/mpeg"),
            ("sound.mp3", "audio/mp3"),
            ("sound.mp3", "audio/x-mp3"),
            ("sound.m4a", "audio/mp4"),
            ("sound.m4a", "audio/x-m4a"),
            ("sound.aac", "audio/aac"),
            ("sound.wma", "audio/x-ms-wma"),
            ("clip.m4v", "video/mp4"),
            ("clip.m4v", "video/x-m4v"),
            ("clip.3gp", "video/3gpp"),
            ("clip.3g2", "video/3gpp2"),
            ("clip.asf", "application/vnd.ms-asf"),
            ("clip.asf", "video/x-ms-asf"),
            ("clip.wmv", "video/x-ms-wmv"),
            ("clip.m2ts", "video/mp2t"),
            ("clip.mts", "video/mp2t"),
            ("clip.avi", "video/x-msvideo"),
            ("clip.mpeg", "video/mpeg"),
            ("clip.mpg", "video/mpeg"),
            ("clip.vob", "video/mpeg"),
            ("clip.m1v", "video/mpeg"),
            ("clip.m2v", "video/mpeg"),
            ("clip.mkv", "video/x-matroska"),
            ("clip.ogm", "video/ogg"),
            ("track.opus", "audio/opus"),
            ("archive.cbz", "application/zip"),
            ("archive.7z", "application/x-7z-compressed"),
            ("backup.tar", "application/x-tar"),
            ("backup.tgz", "application/gzip"),
            ("backup.gz", "application/gzip"),
            ("backup.bz2", "application/x-bzip2"),
            ("backup.xz", "application/x-xz"),
            ("backup.zst", "application/zstd"),
        ] {
            let item = MediaItem {
                filename: filename.into(),
                ..item.clone()
            };
            assert!(
                !media_type_mismatches_extension(&item, mime),
                "{mime} should match {filename}"
            );
        }
    }

    #[test]
    fn bounded_soft_error_sniff_covers_all_directlink_media_suffix_families() {
        for extension in [
            "jpe", "heic", "heif", "psd", "jxl", "svg", "m4v", "wmv", "ogm", "ogv", "opus", "cbz",
            "tar", "tgz", "gz", "bz2", "xz", "zst", "swf",
        ] {
            let item = MediaItem {
                url: format!("https://example.test/file.{extension}"),
                filename: format!("file.{extension}"),
                extractor: "directlink",
                post_id: None,
            };
            assert!(
                looks_like_soft_error_body(&item, b"<html>Blocked</html>"),
                "HTML block page should not be accepted as {extension}"
            );
        }
    }

    #[test]
    fn stable_source_digest() {
        assert_eq!(fingerprint("abc").len(), 64);
        assert_ne!(fingerprint("abc"), fingerprint("abcd"));
    }
}
