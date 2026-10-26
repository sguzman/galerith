//! Conservative resumable HTTP downloader.
//! A partial file is never appended without a matching source fingerprint and
//! an HTTP validator. This is a safe subset of gallery-dl's HTTP semantics.
use crate::{safe_filename, MediaItem};
use fs2::FileExt;
use reqwest::{
    header::{
        ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_RANGE, CONTENT_TYPE, ETAG, IF_RANGE,
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
}

impl PartMetadata {
    fn for_response(url: &str, response: &reqwest::Response) -> Self {
        Self {
            source_sha256: fingerprint(url),
            etag: header(response, ETAG),
            last_modified: header(response, LAST_MODIFIED),
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

/// An HTML/JSON application error with HTTP 200 is not image or video
/// content, even when its URL and output filename have a media extension.
/// Keep generic direct-file downloads (e.g. an actual .html file) intact.
fn media_error_document(item: &MediaItem, content_type: &str) -> bool {
    let extension = item
        .filename
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase());
    let media_exts = [
        "jpg", "jpeg", "png", "gif", "webp", "avif", "bmp", "tif", "tiff", "jxl", "heic", "heif",
        "mp4", "webm", "mov", "mkv", "mp3", "wav", "flac", "ogg",
    ];
    let expects_media = extension
        .as_deref()
        .is_some_and(|ext| media_exts.contains(&ext));
    if !expects_media {
        return false;
    }
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    matches!(
        mime.as_str(),
        "text/html" | "application/xhtml+xml" | "application/json" | "application/problem+json"
    )
}

fn error(message: &str) -> Box<dyn Error + Send + Sync> {
    io::Error::new(io::ErrorKind::InvalidData, message).into()
}

/// Strictly parse "bytes start-end/total"; unknown totals are not accepted for
/// finalizing a resumed file. A resumed response must describe the whole suffix.
fn resumed_length(raw: &str, offset: u64) -> Option<(u64, u64)> {
    let value = raw.strip_prefix("bytes ")?;
    let (range, total) = value.split_once('/')?;
    let (start, end) = range.split_once('-')?;
    let start = start.parse::<u64>().ok()?;
    let end = end.parse::<u64>().ok()?;
    let total = total.parse::<u64>().ok()?;
    if start != offset || end < start || end.checked_add(1)? != total {
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
    if meta.source_sha256 != fingerprint(url) || meta.validator().is_none() {
        return (0, None);
    }
    (size, Some(meta))
}

async fn delay(attempt: u32, retry_after: Option<Duration>) {
    let fallback = Duration::from_millis(250 * (1_u64 << attempt.min(5)));
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
    let part = options.destination.join(format!("{}.part", item.filename));
    let metadata = options
        .destination
        .join(format!("{}.part.json", item.filename));
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
        let (offset, prior) = partial_info(&part, &metadata, &item.url).await;
        // Resume offsets refer to the raw representation: opt out of
        // HTTP content encoding to keep sidecar byte counts meaningful.
        let mut request = client.get(&item.url).header(ACCEPT_ENCODING, "identity");
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
                delay(attempt, None).await;
                continue;
            }
            Err(err) => return Err(err.into()),
        };

        let status = response.status();
        if (status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error())
            && attempt < options.retries
        {
            let after = retry_after(&response);
            delay(attempt, after).await;
            continue;
        }
        if offset > 0 && status == StatusCode::RANGE_NOT_SATISFIABLE {
            invalidate(&part, &metadata).await?;
            if attempt < options.retries {
                continue;
            }
            return Err(error(
                "range not satisfiable; partial reset, retries exhausted",
            ));
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
        if header(&response, CONTENT_TYPE).is_some_and(|mime| media_error_document(item, &mime)) {
            return Err(error("application error document instead of media"));
        }
        // Even when identity was requested, an HTTP server can ignore it.
        // Saving encoded response bytes under the original filename or
        // appending them to an identity partial would corrupt the media.
        if header(&response, CONTENT_ENCODING)
            .is_some_and(|encoding| !encoding.eq_ignore_ascii_case("identity"))
        {
            invalidate(&part, &metadata).await?;
            if attempt < options.retries {
                continue;
            }
            return Err(error("non-identity Content-Encoding; partial reset"));
        }
        let append = offset > 0 && status == StatusCode::PARTIAL_CONTENT;
        if status == StatusCode::PARTIAL_CONTENT && !append {
            // A server must not choose a partial representation for a full
            // GET. Never write it as a complete media file or archive hit.
            invalidate(&part, &metadata).await?;
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
                    invalidate(&part, &metadata).await?;
                    if attempt < options.retries {
                        continue;
                    }
                    return Err(error("missing Content-Range; partial reset"));
                }
            };
            match resumed_length(&header, offset) {
                Some(range) => Some(range),
                None => {
                    invalidate(&part, &metadata).await?;
                    if attempt < options.retries {
                        continue;
                    }
                    return Err(error("invalid Content-Range; partial reset"));
                }
            }
        } else {
            None
        };
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
                    invalidate(&part, &metadata).await?;
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
            invalidate(&part, &metadata).await?;
            write_metadata_atomically(&metadata, &PartMetadata::for_response(&item.url, &response))
                .await?;
        }
        let mut file = if append {
            fs::OpenOptions::new().append(true).open(&part).await?
        } else {
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
        let mut transfer_error: Option<Box<dyn Error + Send + Sync>> = None;
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) => {
                    if let Err(err) = file.write_all(&chunk).await {
                        transfer_error = Some(err.into());
                        break;
                    }
                    received += chunk.len() as u64;
                }
                Ok(None) => break,
                Err(err) => {
                    transfer_error = Some(err.into());
                    break;
                }
            }
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
                if offset.saturating_add(received) != total {
                    transfer_error = Some(error("resumed total length mismatch"));
                }
            }
        }
        if transfer_error.is_none() {
            if let Err(err) = file.flush().await {
                transfer_error = Some(err.into());
            }
        }
        drop(file);
        if let Some(err) = transfer_error {
            if received == 0 && !append {
                // Drop an empty fresh response and its sidecar. A retained
                // validated partial from a failed resume is still recoverable.
                invalidate(&part, &metadata).await?;
            }
            if attempt < options.retries {
                delay(attempt, None).await;
                continue;
            }
            // Keep a validated .part pair for the next invocation.
            return Err(err);
        }
        fs::rename(&part, &target).await?;
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
        assert_eq!(resumed_length("bytes 4-8/10", 4), None);
        assert_eq!(resumed_length("bytes 4-9/*", 4), None);
        assert_eq!(resumed_length("bytes 4-3/10", 4), None);
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
    fn reject_error_documents_only_for_media_filenames() {
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
    }

    #[test]
    fn stable_source_digest() {
        assert_eq!(fingerprint("abc").len(), 64);
        assert_ne!(fingerprint("abc"), fingerprint("abcd"));
    }
}
