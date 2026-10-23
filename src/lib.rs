//! Initial extractor contracts. A complete port remains the MVP gate.
//! This module implements only direct media URL recognition, not site galleries.
pub mod archive;
pub mod config;
pub mod cookies;
pub mod download;
pub mod extractor;
pub mod filename;
pub mod message;
pub mod postprocess;
pub mod privacy;
pub mod range;

use serde::Serialize;
use url::Url;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MediaItem {
    pub url: String,
    pub filename: String,
    pub extractor: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub post_id: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtractError {
    InvalidUrl,
    UnsupportedUrl,
}

impl std::fmt::Display for ExtractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidUrl => write!(f, "invalid URL"),
            Self::UnsupportedUrl => write!(f, "no implemented extractor matches this URL"),
        }
    }
}

impl std::error::Error for ExtractError {}

/// Recognize ordinary HTTP(S) links to media files, without guessing a gallery's API.
pub fn extract_direct_media(input: &str) -> Result<MediaItem, ExtractError> {
    let parsed = Url::parse(input).map_err(|_| ExtractError::InvalidUrl)?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(ExtractError::UnsupportedUrl);
    }
    let segment = parsed
        .path_segments()
        .and_then(Iterator::last)
        .ok_or(ExtractError::UnsupportedUrl)?;
    let decoded = percent_encoding::percent_decode_str(segment)
        .decode_utf8()
        .map_err(|_| ExtractError::UnsupportedUrl)?;
    let (_, extension) = decoded
        .rsplit_once('.')
        .ok_or(ExtractError::UnsupportedUrl)?;
    if !matches!(
        extension.to_ascii_lowercase().as_str(),
        "jpg"
            | "jpeg"
            | "png"
            | "gif"
            | "webp"
            | "avif"
            | "bmp"
            | "tif"
            | "tiff"
            | "svg"
            | "jxl"
            | "mp4"
            | "webm"
            | "mov"
            | "m4v"
            | "mp3"
            | "ogg"
            | "opus"
            | "flac"
            | "wav"
            | "pdf"
            | "cbz"
            | "zip"
    ) {
        return Err(ExtractError::UnsupportedUrl);
    }
    let filename = safe_filename(&decoded);
    if filename.is_empty() || filename == "." || filename == ".." {
        return Err(ExtractError::UnsupportedUrl);
    }
    Ok(MediaItem {
        url: parsed.to_string(),
        filename,
        extractor: "directlink",
        post_id: None,
    })
}

/// Restrict the path to one filename component on Linux and other host filesystems.
pub fn safe_filename(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_control() || "<>:\"/\\|?*".contains(ch) {
                '_'
            } else {
                ch
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognized_image_with_query() {
        let item = extract_direct_media("https://example.org/picture%20one.JPG?size=full").unwrap();
        assert_eq!(item.filename, "picture one.JPG");
        assert_eq!(item.extractor, "directlink");
    }

    #[test]
    fn rejects_gallery_and_unsafe_scheme() {
        assert_eq!(
            extract_direct_media("https://example.org/album/42").unwrap_err(),
            ExtractError::UnsupportedUrl
        );
        assert_eq!(
            extract_direct_media("file:///tmp/local.png").unwrap_err(),
            ExtractError::UnsupportedUrl
        );
    }

    #[test]
    fn filename_sanitization_cannot_create_directory() {
        let item = extract_direct_media("https://example.org/%2E%2E%2Fevil.png").unwrap();
        assert_eq!(item.filename, ".._evil.png");
        assert_eq!(safe_filename("bad\\name?.jpg"), "bad_name_.jpg");
    }

    #[test]
    fn malformed_url_rejected() {
        assert_eq!(
            extract_direct_media("not a URL").unwrap_err(),
            ExtractError::InvalidUrl
        );
    }
}
