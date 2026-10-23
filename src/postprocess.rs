//! Small native per-file metadata writer; gallery-dl's complete metadata
//! vocabulary and postprocessor lifecycle are still future MVP work.
use crate::{privacy::display_url, MediaItem};
use serde_json::json;
use std::{
    error::Error,
    ffi::OsString,
    io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::fs;

fn sidecar_path(target: &Path) -> Result<PathBuf, io::Error> {
    let mut filename = OsString::from(
        target
            .file_name()
            .ok_or_else(|| io::Error::other("missing downloaded filename"))?,
    );
    filename.push(".json");
    Ok(target.with_file_name(filename))
}

/// Write a JSON sidecar atomically after the media path exists.
/// Signed query parameters and URL credentials are intentionally excluded.
pub async fn write_metadata(
    target: &Path,
    item: &MediaItem,
) -> Result<PathBuf, Box<dyn Error + Send + Sync>> {
    let output = sidecar_path(target)?;
    let value = json!({
        "category": item.extractor,
        "filename": item.filename,
        "post_id": item.post_id,
        "source_url": display_url(&item.url),
    });
    let bytes = serde_json::to_vec_pretty(&value)?;
    let mut tmp_name = OsString::from(
        output
            .file_name()
            .ok_or_else(|| io::Error::other("invalid sidecar filename"))?,
    );
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    tmp_name.push(format!(".tmp.{}.{nanos}", std::process::id()));
    let tmp = output.with_file_name(tmp_name);
    fs::write(&tmp, bytes).await?;
    if let Err(error) = fs::rename(&tmp, &output).await {
        let _ = fs::remove_file(&tmp).await;
        return Err(error.into());
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract_direct_media;

    #[tokio::test]
    async fn creates_redacted_per_file_metadata_with_no_temporary_residue() {
        let root =
            std::env::temp_dir().join(format!("galerith-metadata-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root).await;
        fs::create_dir_all(&root).await.unwrap();
        let item = extract_direct_media("https://user:private@example.org/picture.jpg?sig=secret")
            .unwrap();
        let target = root.join("picture.jpg");
        fs::write(&target, b"jpeg fixture").await.unwrap();
        let sidecar = write_metadata(&target, &item).await.unwrap();
        assert_eq!(sidecar, root.join("picture.jpg.json"));
        let bytes = fs::read(sidecar).await.unwrap();
        let content = String::from_utf8(bytes).unwrap();
        assert!(content.contains("https://example.org/picture.jpg"));
        assert!(!content.contains("private"));
        assert!(!content.contains("secret"));
        let entries = std::fs::read_dir(&root).unwrap().count();
        assert_eq!(entries, 2);
        fs::remove_dir_all(root).await.unwrap();
    }
}
