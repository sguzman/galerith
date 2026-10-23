//! Conservative resumable HTTP downloader.
//! A partial file is never appended without a matching source fingerprint and
//! an HTTP validator. This is a safe subset of gallery-dl's HTTP semantics.
use crate::{safe_filename, MediaItem};
use reqwest::{
    header::{CONTENT_RANGE, ETAG, IF_RANGE, LAST_MODIFIED, RANGE, RETRY_AFTER},
    Client, StatusCode,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{error::Error, io, path::{Path, PathBuf}, time::Duration};
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
        self.etag.as_deref().filter(|tag| !tag.starts_with("W/"))
            .or(self.last_modified.as_deref())
    }
}

fn fingerprint(source: &str) -> String {
    let hash = Sha256::digest(source.as_bytes());
    hash.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn header(response: &reqwest::Response, name: reqwest::header::HeaderName) -> Option<String> {
    response.headers().get(name)?.to_str().ok().map(str::to_owned)
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

async fn partial_info(path: &Path, metadata_path: &Path, url: &str)
    -> (u64, Option<PartMetadata>)
{
    let size = match fs::metadata(path).await {
        Ok(info) if info.is_file() => info.len(),
        _ => return (0, None),
    };
    if size == 0 {
        return (0, None);
    }
    let meta = match fs::read(metadata_path).await
        .ok().and_then(|bytes| serde_json::from_slice::<PartMetadata>(&bytes).ok()) {
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

fn retry_after(response: &reqwest::Response) -> Option<Duration> {
    // HTTP dates are a future compatibility task; bounded delta-seconds are supported.
    let seconds = header(response, RETRY_AFTER)?.parse::<u64>().ok()?;
    Some(Duration::from_secs(seconds.min(60)))
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
    let target = options.destination.join(&item.filename);
    if !options.no_skip && fs::try_exists(&target).await? {
        return Ok(DownloadOutcome::Skipped(target));
    }
    let part = options.destination.join(format!("{}.part", item.filename));
    let metadata = options.destination.join(format!("{}.part.json", item.filename));

    for attempt in 0..=options.retries {
        let (offset, prior) = partial_info(&part, &metadata, &item.url).await;
        let mut request = client.get(&item.url);
        if offset > 0 {
            request = request
                .header(RANGE, format!("bytes={offset}-"))
                .header(IF_RANGE, prior.as_ref().and_then(PartMetadata::validator)
                    .ok_or_else(|| error("resume validator missing"))?);
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
            return Err(error("range not satisfiable; partial reset, retries exhausted"));
        }
        let mut response = response.error_for_status()?;
        let append = offset > 0 && status == StatusCode::PARTIAL_CONTENT;
        if status == StatusCode::PARTIAL_CONTENT && !append {
            return Err(error("unexpected partial response without a range request"));
        }

        let resume_range = if append {
            let header = header(&response, CONTENT_RANGE)
                .ok_or_else(|| error("missing Content-Range for resumed download"))?;
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
                if old.etag.as_ref().zip(new_etag.as_ref()).is_some_and(|(a, b)| a != b)
                    || old.last_modified.as_ref().zip(new_modified.as_ref())
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

        let mut file = if append {
            fs::OpenOptions::new().append(true).open(&part).await?
        } else {
            fs::File::create(&part).await?
        };
        if !append {
            let info = PartMetadata::for_response(&item.url, &response);
            fs::write(&metadata, serde_json::to_vec(&info)?).await?;
        }

        let expected_body = resume_range.map(|(body, _)| body)
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
            if attempt < options.retries {
                delay(attempt, None).await;
                continue;
            }
            // Keep a validated .part pair for the next invocation.
            return Err(err);
        }
        fs::rename(&part, &target).await?;
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
    fn stable_source_digest() {
        assert_eq!(fingerprint("abc").len(), 64);
        assert_ne!(fingerprint("abc"), fingerprint("abcd"));
    }
}
