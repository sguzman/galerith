//! Initial extractor contracts. A complete port remains the MVP gate.
//! This module implements only direct media URL recognition, not site galleries.
pub mod archive;
pub mod cache;
pub mod config;
pub mod cookies;
pub mod download;
pub mod extractor;
pub mod filename;
pub mod message;
pub mod pacing;
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
            | "jpe"
            | "heic"
            | "heif"
            | "psd"
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
            | "mkv"
            | "webm"
            | "mov"
            | "m4v"
            | "mp3"
            | "ogg"
            | "ogm"
            | "ogv"
            | "opus"
            | "flac"
            | "wav"
            | "pdf"
            | "cbz"
            | "zip"
            | "rar"
            | "7z"
            | "swf"
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

/// Normalize a source name to one filename component portable across common filesystems.
pub fn safe_filename(value: &str) -> String {
    let mut safe = value
        .chars()
        .map(|ch| {
            if ch.is_control() || "<>:\"/\\|?*".contains(ch) {
                '_'
            } else {
                ch
            }
        })
        .collect::<String>();
    // Windows strips trailing spaces and dots, which can make two distinct
    // source names collide or turn a name into a reserved device alias.
    let trimmed_len = safe.trim_end_matches([' ', '.']).len();
    safe.truncate(trimmed_len);

    // Keep names usable when downloaded on Windows, even when Galerith runs
    // on a filesystem that permits these DOS device names.
    let stem = safe.split('.').next().unwrap_or_default();
    let uppercase = stem.to_ascii_uppercase();
    let reserved = matches!(
        uppercase.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || ["COM", "LPT"].iter().any(|prefix| {
        uppercase
            .strip_prefix(prefix)
            .is_some_and(|suffix| suffix.len() == 1 && matches!(suffix.as_bytes()[0], b'1'..=b'9'))
    });
    if reserved {
        safe.insert(0, '_');
    }
    safe
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
    fn recognizes_additional_pinned_directlink_media_extensions() {
        for extension in [
            "jpe", "heic", "heif", "psd", "mkv", "ogm", "ogv", "rar", "7z", "swf",
        ] {
            let url = format!("https://assets.example.org/downloads/file.{extension}");
            let item = extract_direct_media(&url).unwrap();
            assert_eq!(item.filename, format!("file.{extension}"));
        }
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
    fn filenames_are_portable_across_windows_and_posix_rules() {
        for (source, expected) in [
            ("CON", "_CON"),
            ("con.txt", "_con.txt"),
            ("PRN.jpg", "_PRN.jpg"),
            ("AUX", "_AUX"),
            ("NUL.png", "_NUL.png"),
            ("COM9.log", "_COM9.log"),
            ("LPT1", "_LPT1"),
            ("CONIN$", "_CONIN$"),
            ("CONOUT$.jpg", "_CONOUT$.jpg"),
            ("trailing. ", "trailing"),
            ("dot.", "dot"),
        ] {
            assert_eq!(safe_filename(source), expected, "source {source:?}");
        }
        assert_eq!(safe_filename("normal name.png"), "normal name.png");
    }

    #[test]
    fn malformed_url_rejected() {
        assert_eq!(
            extract_direct_media("not a URL").unwrap_err(),
            ExtractError::InvalidUrl
        );
    }
}
