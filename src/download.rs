//! Conservative resumable HTTP downloader.
//! A partial file is never appended without a matching source fingerprint and
//! an HTTP validator. This is a safe subset of gallery-dl's HTTP semantics.
use crate::pacing::{DelayRange, RateRange, RetryDelay};
use crate::{safe_filename, MediaItem};
use fs2::FileExt;
use reqwest::{
    header::{
        HeaderMap, ACCEPT, ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_RANGE, CONTENT_TYPE, ETAG,
        IF_RANGE, LAST_MODIFIED, RANGE, RETRY_AFTER,
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
use tokio::{
    fs,
    io::{AsyncReadExt, AsyncWriteExt},
};

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
    /// Maximum number of response bytes written and paced in one iteration.
    pub chunk_size: usize,
    /// Whether HTML responses with non-HTML output names are rejected.
    pub validate_html: bool,
    /// Whether recognized file signatures correct the output extension.
    pub adjust_extensions: bool,
    /// Minimum response size; responses with a known smaller size are filtered.
    pub filesize_min: Option<u64>,
    /// Maximum response size; responses with a known larger size are filtered.
    pub filesize_max: Option<u64>,
    /// Additional downloader-only headers from explicit configuration.
    pub headers: HeaderMap,
    /// Additional response codes configured as retryable; 5xx are always retried.
    pub retry_codes: Vec<StatusCode>,
    /// Optional per-request media timeout, overriding the shared client's timeout.
    pub http_timeout: Option<Duration>,
    /// Optional fixed or ranged pause between retries; Retry-After takes precedence.
    pub retry_delay: Option<RetryDelay>,
    /// Optional configured pause for 429 responses; unset uses the reference 60-second interval.
    pub rate_limit_delay: Option<RetryDelay>,
    /// Optional delay before starting a non-skipped media download.
    pub download_delay: Option<DelayRange>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum DownloadOutcome {
    Saved(PathBuf),
    Skipped(PathBuf),
    Filtered(PathBuf),
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

fn source_extension(url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url).ok()?;
    Path::new(parsed.path())
        .extension()?
        .to_str()
        .map(str::to_ascii_lowercase)
}

fn media_extension(item: &MediaItem) -> Option<String> {
    Path::new(&item.filename)
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .or_else(|| source_extension(&item.url))
}

/// Match the upstream check for declared HTML responses when the output
/// suffix is not HTML.
fn media_error_document(item: &MediaItem, content_type: &str, validate_html: bool) -> bool {
    let extension = media_extension(item);
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    match mime.as_str() {
        "text/html" => validate_html && !matches!(extension.as_deref(), Some("html" | "htm")),
        _ => false,
    }
}

/// Pick the filename suffix for a response when the caller's template did not
/// provide one. This deliberately covers the stable MIME aliases used by the
/// direct HTTP downloader; unknown media types receive the upstream binary
/// fallback suffix.
fn media_extension_from_mime(content_type: Option<&str>) -> &'static str {
    let mime = content_type
        .unwrap_or("image/jpeg")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let mime = if mime.contains('/') {
        mime
    } else {
        format!("image/{mime}")
    };
    match mime.as_str() {
        "image/jpeg" | "image/jpg" => "jpg",
        "image/png" => "png",
        "image/gif" => "gif",
        "image/bmp" | "image/x-bmp" | "image/x-ms-bmp" => "bmp",
        "image/webp" => "webp",
        "image/avif" => "avif",
        "image/heic" => "heic",
        "image/heif" => "heif",
        "image/svg+xml" => "svg",
        "image/ico" | "image/icon" | "image/x-icon" | "image/vnd.microsoft.icon" => "ico",
        "image/x-photoshop" | "application/x-photoshop" | "image/vnd.adobe.photoshop" => "psd",
        "video/webm" | "audio/webm" => "webm",
        "video/ogg" | "audio/ogg" | "application/ogg" => "ogg",
        "video/mp4" => "mp4",
        "video/m4v" => "m4v",
        "video/x-m4v" => "m4v",
        "video/quicktime" => "mov",
        "video/mpeg" => "mpeg",
        "video/3gpp" => "3gp",
        "video/3gpp2" => "3g2",
        "video/x-msvideo" => "avi",
        "video/x-ms-wmv" => "wmv",
        "video/x-matroska" => "mkv",
        "audio/wav" | "audio/x-wav" => "wav",
        "audio/mpeg" => "mp3",
        "audio/aac" | "audio/x-aac" => "aac",
        "audio/flac" | "audio/x-flac" => "flac",
        "audio/opus" => "opus",
        "audio/x-ms-wma" => "wma",
        "text/html" => "html",
        "application/xhtml+xml" => "xhtml",
        "application/json" | "application/problem+json" => "json",
        "text/plain" => "txt",
        "text/css" => "css",
        "text/xml" | "application/xml" => "xml",
        "application/zip" | "application/x-zip" | "application/x-zip-compressed" => "zip",
        "application/rar" | "application/x-rar" | "application/x-rar-compressed" => "rar",
        "application/x-7z-compressed" => "7z",
        "application/vnd.apple.mpegurl" | "application/x-mpegurl" => "m3u8",
        "application/dash+xml" => "mpd",
        "application/pdf" | "application/x-pdf" => "pdf",
        "application/x-shockwave-flash" => "swf",
        "application/gzip" | "application/x-gzip" => "gz",
        "application/x-xz" => "xz",
        "application/x-tar" => "tar",
        "application/x-bzip2" => "bz2",
        "application/zstd" | "application/x-zstd" => "zst",
        "model/obj" => "obj",
        "application/octet-stream" => "bin",
        _ => "bin",
    }
}

/// Return a conservative extension correction for common, unambiguous file
/// signatures. The prefix is bounded by the downloader and is never trusted
/// for formats whose signatures overlap substantially.
fn signature_extension(first: &[u8], current_extension: Option<&str>) -> Option<&'static str> {
    let extension = current_extension?.to_ascii_lowercase();
    let checkable = [
        "jpg", "png", "gif", "bmp", "webp", "avif", "heic", "svg", "ico", "cur", "psd", "mp4",
        "m4v", "mov", "webm", "ogg", "wav", "mp3", "aac", "m3u8", "mpd", "zip", "rar", "7z", "pdf",
        "swf", "html", "htm", "blend", "obj", "clip", "bin",
    ];
    if !checkable.contains(&extension.as_str()) {
        return None;
    }
    let detected = if first.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if first.starts_with(b"\xff\xd8\xff") {
        Some("jpg")
    } else if first.starts_with(b"GIF87a") || first.starts_with(b"GIF89a") {
        Some("gif")
    } else if first.starts_with(b"%PDF-") {
        Some("pdf")
    } else if first.starts_with(b"BM") {
        Some("bmp")
    } else if first.get(0..4) == Some(b"RIFF") && first.get(8..12) == Some(b"WEBP") {
        Some("webp")
    } else if first.get(4..11) == Some(b"ftypavi") && matches!(first.get(11), Some(b'f' | b's')) {
        Some("avif")
    } else if first.get(4..10) == Some(b"ftyphe")
        && matches!(
            first.get(10..12),
            Some(b"ic" | b"im" | b"is" | b"ix" | b"vc" | b"vm" | b"vs")
        )
    {
        Some("heic")
    } else if first.starts_with(b"<?xml") {
        Some("svg")
    } else if first.starts_with(b"\0\0\x01\0") {
        Some("ico")
    } else if first.starts_with(b"\0\0\x02\0") {
        Some("cur")
    } else if first.starts_with(b"8BPS") {
        Some("psd")
    } else if first.get(4..8) == Some(b"ftyp")
        && first
            .get(8..11)
            .is_some_and(|brand| [b"mp4".as_slice(), b"avc", b"iso"].contains(&brand))
    {
        Some("mp4")
    } else if first.get(4..11) == Some(b"ftypM4V") {
        Some("m4v")
    } else if first.starts_with(b"\x1a\x45\xdf\xa3") {
        Some("webm")
    } else if first.starts_with(b"OggS") {
        Some("ogg")
    } else if first.get(0..4) == Some(b"RIFF") && first.get(8..12) == Some(b"WAVE") {
        Some("wav")
    } else if first.starts_with(b"ID3")
        || [b"\xff\xfb".as_slice(), b"\xff\xf3", b"\xff\xf2"]
            .contains(&first.get(0..2).unwrap_or_default())
    {
        Some("mp3")
    } else if [b"\xff\xf9".as_slice(), b"\xff\xf1"].contains(&first.get(0..2).unwrap_or_default()) {
        Some("aac")
    } else if first.starts_with(b"#EXTM3U") {
        Some("m3u8")
    } else if first
        .get(..16)
        .unwrap_or(first)
        .windows(4)
        .any(|window| window == b"<MPD")
    {
        Some("mpd")
    } else if first.starts_with(b"PK\x03\x04")
        || first.starts_with(b"PK\x05\x06")
        || first.starts_with(b"PK\x07\x08")
    {
        Some("zip")
    } else if first.starts_with(b"7z\xbc\xaf\x27\x1c") {
        Some("7z")
    } else if first.starts_with(b"Rar!\x1a\x07") {
        Some("rar")
    } else if first.starts_with(b"CWS") || first.starts_with(b"FWS") {
        Some("swf")
    } else if upstream_html_signature(first) {
        Some("html")
    } else if first.starts_with(b"BLENDER") {
        Some("blend")
    } else if first.starts_with(b"# Blender v") {
        Some("obj")
    } else if first.starts_with(b"CSFCHUNK") {
        Some("clip")
    } else {
        None
    };
    detected.filter(|detected| !extension.eq_ignore_ascii_case(detected))
}

/// Match gallery-dl's bounded, case-insensitive HTML signature predicate.
fn upstream_html_signature(first: &[u8]) -> bool {
    let sample = first.get(..14).unwrap_or(first);
    let Some(start) = sample.iter().position(|byte| !byte.is_ascii_whitespace()) else {
        return false;
    };
    let candidate = &sample[start..];
    let signature = b"<!doctype html";
    !candidate.is_empty()
        && candidate.len() <= signature.len()
        && candidate
            .iter()
            .zip(signature)
            .all(|(actual, expected)| actual.to_ascii_lowercase() == *expected)
}

/// Some servers label an HTML or JSON block page as application/octet-stream
/// or omit Content-Type entirely. Inspect only a bounded initial body prefix
/// for established binary/document outputs, never the full download.
fn looks_like_soft_error_body(item: &MediaItem, first: &[u8], validate_html: bool) -> bool {
    let extension = media_extension(item);
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
    (validate_html
        && (trimmed.starts_with(b"<!doctype html")
            || trimmed.starts_with(b"<html")
            || trimmed.starts_with(b"<head")
            || trimmed.starts_with(b"<body")))
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

async fn delay(attempt: u32, retry_after: Option<Duration>, configured: Option<Duration>) {
    let fallback =
        configured.unwrap_or_else(|| Duration::from_millis(250 * (1_u64 << attempt.min(5))));
    tokio::time::sleep(retry_after.unwrap_or(fallback)).await;
}

fn configured_retry_delay(
    status: StatusCode,
    attempt: u32,
    retry_delay: Option<RetryDelay>,
    rate_limit_delay: Option<RetryDelay>,
) -> Option<Duration> {
    let general = retry_delay.map(|policy| policy.sample(attempt));
    if status == StatusCode::TOO_MANY_REQUESTS {
        let attempt_floor = Duration::from_secs(u64::from(attempt));
        let rate_limit = rate_limit_delay
            .map(|policy| policy.sample(attempt))
            .unwrap_or(Duration::from_secs(60))
            .max(attempt_floor);
        Some(general.map_or(rate_limit, |delay| delay.max(rate_limit)))
    } else {
        general
    }
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
    if options.chunk_size == 0 {
        return Err(error("download chunk size must be positive"));
    }

    fs::create_dir_all(&options.destination).await?;
    // Hold this OS advisory lock through the existence check, transfer and
    // rename. Parallel Galerith workers cannot clobber the same .part file.
    let output_lock = lock_output(&options.destination, &item.filename).await?;
    let mut target = options.destination.join(&item.filename);
    let url_extension = source_extension(&item.url);
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

    if let Some(delay) = options.download_delay {
        tokio::time::sleep(delay.sample()).await;
    }

    for attempt in 0..=options.retries {
        let (offset, prior) = if options.no_part {
            (0, None)
        } else {
            partial_info(&part, &metadata, &item.url).await
        };
        // Resume offsets refer to the raw representation: opt out of
        // HTTP content encoding to keep sidecar byte counts meaningful.
        let mut request = client
            .get(&item.url)
            .header(ACCEPT, "*/*")
            .header(ACCEPT_ENCODING, "identity");
        if let Some(timeout) = options.http_timeout {
            request = request.timeout(timeout);
        }
        request = request.headers(options.headers.clone());
        // Media retries do not use extractor sleep-request intervals, but
        // they still honor any Retry-After cooldown previously recorded for
        // this origin.
        crate::pacing::wait_for_request_url(&item.url, None).await;
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
                delay(
                    attempt,
                    None,
                    options.retry_delay.map(|policy| policy.sample(attempt + 1)),
                )
                .await;
                continue;
            }
            Err(err) => return Err(err.into()),
        };

        let status = response.status();
        let after = if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
            retry_after(&response)
        } else {
            None
        };
        if let Some(after) = after {
            crate::pacing::defer_host_for_url(&item.url, after).await;
        }
        if (status.is_server_error() || options.retry_codes.contains(&status))
            && attempt < options.retries
        {
            if after.is_none() {
                delay(
                    attempt,
                    None,
                    configured_retry_delay(
                        status,
                        attempt + 1,
                        options.retry_delay,
                        options.rate_limit_delay,
                    ),
                )
                .await;
            }
            continue;
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
        // Match the pinned validation rule for explicitly declared HTML.
        let response_content_type = header(&response, CONTENT_TYPE);
        let mime_extension = media_extension_from_mime(response_content_type.as_deref());
        if response_content_type
            .as_deref()
            .is_some_and(|mime| media_error_document(item, mime, options.validate_html))
        {
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
        let declared_size = resume_range
            .map(|(_, total)| total)
            .or_else(|| response.content_length());
        if declared_size.is_some_and(|size| {
            size > 0
                && (options.filesize_min.is_some_and(|minimum| size < minimum)
                    || options.filesize_max.is_some_and(|maximum| size > maximum))
        }) {
            return Ok(DownloadOutcome::Filtered(target));
        }
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
        let mut response_first_bytes = Vec::with_capacity(160);
        if append {
            let prior_file = fs::File::open(&part).await?;
            prior_file.take(160).read_to_end(&mut first_bytes).await?;
        }
        let mut transfer_error: Option<Box<dyn Error + Send + Sync>> = None;
        let mut oversized_response = false;
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) => {
                    for chunk in chunk.chunks(options.chunk_size) {
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
                        if response_first_bytes.len() < 160 {
                            let count = (160 - response_first_bytes.len()).min(chunk.len());
                            response_first_bytes.extend_from_slice(&chunk[..count]);
                        }
                        if let Err(err) = file.write_all(chunk).await {
                            transfer_error = Some(err.into());
                            break;
                        }
                        received = received.saturating_add(chunk.len() as u64);
                        if let Some(bytes_per_second) = limit_rate {
                            let target_nanos = (u128::from(received) * 1_000_000_000)
                                / u128::from(bytes_per_second);
                            let elapsed_nanos = transfer_started.elapsed().as_nanos();
                            if target_nanos > elapsed_nanos {
                                let wait_nanos =
                                    (target_nanos - elapsed_nanos).min(u128::from(u64::MAX));
                                tokio::time::sleep(Duration::from_nanos(wait_nanos as u64)).await;
                            }
                        }
                    }
                    if transfer_error.is_some() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(err) => {
                    transfer_error = Some(err.into());
                    break;
                }
            }
        }
        let current_extension = target
            .extension()
            .and_then(|value| value.to_str())
            .or(url_extension.as_deref())
            .or_else(|| url_extension.is_none().then_some(mime_extension));
        let detected_extension = options
            .adjust_extensions
            .then(|| signature_extension(&first_bytes, current_extension))
            .flatten();
        // A host can replace a resumed suffix with a block page while still
        // returning 206. Keep that conservative recovery guard scoped to a
        // request that already had a validated partial; fresh downloads
        // follow the pinned downloader's signature and MIME behavior.
        let misleading_error_body = offset > 0
            && detected_extension != Some("html")
            && looks_like_soft_error_body(item, &response_first_bytes, options.validate_html);
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
                delay(
                    attempt,
                    None,
                    options.retry_delay.map(|policy| policy.sample(attempt + 1)),
                )
                .await;
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
        let output_extension = detected_extension.or_else(|| {
            (target.extension().is_none() && url_extension.is_none()).then_some(mime_extension)
        });
        if let Some(extension) = output_extension {
            let old_extension = target.extension().and_then(|value| value.to_str());
            if !old_extension.is_some_and(|old| old.eq_ignore_ascii_case(extension)) {
                let corrected_target = target.with_extension(extension);
                drop(output_lock);
                let _corrected_output_lock = lock_output(
                    &options.destination,
                    corrected_target
                        .file_name()
                        .and_then(|s| s.to_str())
                        .ok_or_else(|| error("invalid corrected filename"))?,
                )
                .await?;
                verify_output_entry(&corrected_target, false).await?;
                if !options.no_skip && fs::try_exists(&corrected_target).await? {
                    let existing = fs::metadata(&corrected_target).await?;
                    if existing.is_file() && existing.len() > 0 {
                        if options.no_part {
                            fs::remove_file(&target).await?;
                        } else {
                            invalidate_for_mode(&part, &metadata, false).await?;
                        }
                        return Ok(DownloadOutcome::Skipped(corrected_target));
                    }
                }
                if options.no_part {
                    fs::rename(&target, &corrected_target).await?;
                }
                target = corrected_target;
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
    fn signature_correction_covers_common_checkable_formats_and_ignores_other_suffixes() {
        for (current, prefix, corrected) in [
            ("jpg", b"RIFFxxxxWEBP".as_slice(), "webp"),
            ("png", b"\0\0\x02\0", "cur"),
            ("mov", b"\x89PNG\r\n\x1a\n", "png"),
            ("jpg", b"xxxxftypavif", "avif"),
            ("jpg", b"xxxxftypheic", "heic"),
            ("jpg", b"xxxxftypmp4", "mp4"),
            ("jpg", b"RIFFxxxxWAVE", "wav"),
            ("jpg", b"\x1a\x45\xdf\xa3", "webm"),
            ("jpg", b"OggS", "ogg"),
            ("jpg", b"ID3", "mp3"),
            ("mp3", b"\xff\xf1\x50\x80", "aac"),
            ("mpd", b"#EXTM3U\n#EXT-X-VERSION:3", "m3u8"),
            ("jpg", b"<!DOCTYPE html><html>", "html"),
            ("htm", b"  <!doctype html>", "html"),
            (
                "m3u8",
                b"<MPD xmlns=\"urn:mpeg:dash:schema:mpd:2011\">",
                "mpd",
            ),
            ("jpg", b"8BPS", "psd"),
            ("jpg", b"BLENDER", "blend"),
            ("jpg", b"CSFCHUNK", "clip"),
        ] {
            assert_eq!(signature_extension(prefix, Some(current)), Some(corrected));
        }
        assert_eq!(
            signature_extension(b"                <MPD></MPD>", Some("mp4")),
            None,
            "DASH marker beyond upstream's 16-byte probe must not change the suffix"
        );
        assert_eq!(
            signature_extension(b"II*\0synthetic TIFF", Some("jpg")),
            None,
            "TIFF is not in the pinned signature table"
        );
        assert_eq!(
            signature_extension(b"\x1f\x8b\x08\0synthetic gzip", Some("jpg")),
            None,
            "gzip is not in the pinned signature table"
        );
        assert_eq!(
            signature_extension(b"\0\0\0\x18ftypqt ", Some("mp4")),
            None,
            "the pinned QuickTime predicate compares an 8-byte slice to a 7-byte marker"
        );
        assert_eq!(signature_extension(b"\x89PNG\r\n\x1a\n", Some("txt")), None);
    }

    #[test]
    fn mime_suffix_inference_uses_image_default_and_known_aliases() {
        assert_eq!(media_extension_from_mime(None), "jpg");
        assert_eq!(
            media_extension_from_mime(Some("image/jpeg; charset=binary")),
            "jpg"
        );
        assert_eq!(media_extension_from_mime(Some("image/x-icon")), "ico");
        assert_eq!(media_extension_from_mime(Some("video/quicktime")), "mov");
        assert_eq!(
            media_extension_from_mime(Some("application/x-7z-compressed")),
            "7z"
        );
        assert_eq!(
            media_extension_from_mime(Some("application/octet-stream")),
            "bin"
        );
        assert_eq!(media_extension_from_mime(Some("unknown/type")), "bin");

        for (mime, extension) in [
            ("audio/x-aac", "aac"),
            ("application/vnd.apple.mpegurl", "m3u8"),
            ("application/x-mpegurl", "m3u8"),
            ("application/dash+xml", "mpd"),
            ("application/x-zip", "zip"),
            ("application/x-zip-compressed", "zip"),
            ("application/x-rar", "rar"),
            ("application/x-rar-compressed", "rar"),
        ] {
            assert_eq!(
                media_extension_from_mime(Some(mime)),
                extension,
                "unexpected extension for {mime}"
            );
        }
    }

    #[test]
    fn absent_429_retry_after_uses_reference_interval_unless_configured() {
        let rate_limit = StatusCode::TOO_MANY_REQUESTS;
        assert_eq!(
            configured_retry_delay(rate_limit, 1, None, None),
            Some(Duration::from_secs(60))
        );

        let zero = RetryDelay::parse("0").unwrap();
        assert_eq!(
            configured_retry_delay(rate_limit, 1, None, Some(zero)),
            Some(Duration::from_secs(1))
        );

        let general = RetryDelay::parse("0.5").unwrap();
        let short_rate = RetryDelay::parse("0.15").unwrap();
        assert_eq!(
            configured_retry_delay(rate_limit, 1, Some(general), Some(short_rate)),
            Some(Duration::from_secs(1))
        );
        let long_rate = RetryDelay::parse("2").unwrap();
        assert_eq!(
            configured_retry_delay(rate_limit, 1, Some(general), Some(long_rate)),
            Some(Duration::from_secs(2))
        );
        let long_general = RetryDelay::parse("1.5").unwrap();
        assert_eq!(
            configured_retry_delay(rate_limit, 1, Some(long_general), Some(zero)),
            Some(Duration::from_millis(1500))
        );
        let two_seconds = RetryDelay::parse("2").unwrap();
        assert_eq!(
            configured_retry_delay(rate_limit, 3, None, Some(two_seconds)),
            Some(Duration::from_secs(3))
        );
        assert_eq!(
            configured_retry_delay(StatusCode::SERVICE_UNAVAILABLE, 1, Some(general), None),
            Some(Duration::from_millis(500))
        );
    }

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
            b"\xef\xbb\xbf  <!DOCTYPE HTML><html>blocked</html>",
            true
        ));
        assert!(looks_like_soft_error_body(
            &item,
            b"<HTML>Forbidden</HTML>",
            true
        ));
        assert!(!looks_like_soft_error_body(
            &item,
            b"<HTML>Allowed</HTML>",
            false
        ));
        assert!(looks_like_soft_error_body(
            &item,
            b"{ \"error\": \"forbidden\" }",
            true
        ));
        assert!(looks_like_soft_error_body(
            &item,
            b"{\"errors\":[\"denied\"]}",
            false
        ));
        assert!(!looks_like_soft_error_body(
            &item,
            b"\xff\xd8\xff\xe0JPEG",
            true
        ));
        let pdf = MediaItem {
            filename: "manual.pdf".into(),
            ..item.clone()
        };
        assert!(looks_like_soft_error_body(
            &pdf,
            b"<html>PDF not found",
            true
        ));
        let html = MediaItem {
            filename: "index.html".into(),
            ..item.clone()
        };
        assert!(!looks_like_soft_error_body(
            &html,
            b"<html>Valid</html>",
            true
        ));
        let json = MediaItem {
            filename: "data.json".into(),
            ..item
        };
        assert!(!looks_like_soft_error_body(
            &json,
            b"{\"error\":\"actual JSON\"}",
            true
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
    fn reject_only_declared_html_for_non_html_outputs() {
        let item = MediaItem {
            url: "https://example.test/media.jpg".into(),
            filename: "photo.jpg".into(),
            extractor: "directlink",
            post_id: None,
        };
        assert!(media_error_document(
            &item,
            "text/html; charset=UTF-8",
            true
        ));
        assert!(!media_error_document(
            &item,
            "text/html; charset=UTF-8",
            false
        ));
        assert!(!media_error_document(&item, "APPLICATION/JSON", true));
        assert!(!media_error_document(&item, "APPLICATION/JSON", false));
        assert!(!media_error_document(
            &item,
            "application/problem+json",
            true
        ));
        assert!(!media_error_document(&item, "application/xhtml+xml", true));
        assert!(!media_error_document(&item, "image/jpeg", true));
        assert!(!media_error_document(
            &item,
            "application/octet-stream",
            true
        ));
        let html = MediaItem {
            filename: "document.html".into(),
            ..item
        };
        assert!(!media_error_document(&html, "text/html", true));
        assert!(!media_error_document(&html, "text/html", false));
        assert!(!media_error_document(
            &html,
            "application/problem+json",
            true
        ));
        let pdf = MediaItem {
            filename: "documents.pdf".into(),
            ..html.clone()
        };
        assert!(media_error_document(&pdf, "text/html", true));
        assert!(!media_error_document(&pdf, "text/html", false));
        assert!(!media_error_document(&pdf, "application/json", true));
        assert!(!media_error_document(&pdf, "application/pdf", true));
        let archive = MediaItem {
            filename: "media.7z".into(),
            ..html.clone()
        };
        assert!(media_error_document(&archive, "text/html", true));
        let json = MediaItem {
            filename: "data.json".into(),
            ..html
        };
        assert!(!media_error_document(&json, "application/json", true));
        assert!(media_error_document(&json, "text/html", true));
        assert!(!media_error_document(
            &json,
            "application/problem+json",
            true
        ));
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
                looks_like_soft_error_body(&item, b"<html>Blocked</html>", true),
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
