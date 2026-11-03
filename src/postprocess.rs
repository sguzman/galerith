//! Bounded native per-file and gallery-level metadata writers; gallery-dl's
//! complete metadata vocabulary and postprocessor lifecycle remain future MVP work.
use crate::{
    message::{ExtractorMessage, Metadata},
    privacy::display_url,
    MediaItem,
};
use serde_json::json;
use std::{
    error::Error,
    ffi::OsString,
    io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::{
    fs::{self, OpenOptions},
    io::AsyncWriteExt,
};

fn sidecar_path(target: &Path) -> Result<PathBuf, io::Error> {
    sidecar_path_with_extension(target, "json")
}

fn sidecar_path_with_extension(target: &Path, extension: &str) -> Result<PathBuf, io::Error> {
    let mut filename = OsString::from(
        target
            .file_name()
            .ok_or_else(|| io::Error::other("missing downloaded filename"))?,
    );
    filename.push(format!(".{extension}"));
    Ok(target.with_file_name(filename))
}

fn temporary_path(output: &Path) -> Result<PathBuf, Box<dyn Error + Send + Sync>> {
    let mut tmp_name = OsString::from(
        output
            .file_name()
            .ok_or_else(|| io::Error::other("invalid metadata filename"))?,
    );
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    tmp_name.push(format!(".tmp.{}.{nanos}", std::process::id()));
    Ok(output.with_file_name(tmp_name))
}

async fn write_atomic(output: &Path, bytes: &[u8], tmp: &Path) -> io::Result<()> {
    // create_new prevents following a pre-existing symlink or truncating an
    // unrelated file if a stale or attacker-planted temporary path collides.
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(tmp)
        .await?;
    if let Err(error) = file.write_all(bytes).await {
        drop(file);
        let _ = fs::remove_file(tmp).await;
        return Err(error);
    }
    drop(file);
    if let Err(error) = fs::rename(tmp, output).await {
        let _ = fs::remove_file(tmp).await;
        return Err(error);
    }
    Ok(())
}

async fn write_json_atomic(
    output: &Path,
    value: &serde_json::Value,
) -> Result<PathBuf, Box<dyn Error + Send + Sync>> {
    let bytes = serde_json::to_vec_pretty(value)?;
    let tmp = temporary_path(output)?;
    write_atomic(output, &bytes, &tmp).await?;
    Ok(output.to_owned())
}

/// Write the URL event's tags as newline-separated text next to the media.
/// Missing tags do not produce a sidecar. Present tags are newline-terminated,
/// including an empty tag list, matching gallery-dl's tags postprocessor.
pub async fn write_tags(
    target: &Path,
    metadata: &Metadata,
) -> Result<PathBuf, Box<dyn Error + Send + Sync>> {
    let output = sidecar_path_with_extension(target, "txt")?;
    let values = match metadata.get("tags") {
        None => return Ok(output),
        Some(serde_json::Value::Array(values)) => values.as_slice(),
        Some(_) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "metadata tags must be an array",
            )
            .into());
        }
    };
    let tags = values
        .iter()
        .map(|value| {
            let tag = value.as_str().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "metadata tags must be strings")
            })?;
            if tag.chars().any(char::is_control) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "metadata tags cannot contain control characters",
                ));
            }
            Ok(tag)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let bytes = format!("{}\n", tags.join("\n"));
    let tmp = temporary_path(&output)?;
    write_atomic(&output, bytes.as_bytes(), &tmp).await?;
    Ok(output)
}

/// Write a JSON sidecar atomically after the media path exists.
/// Signed query parameters and URL credentials are intentionally excluded.
pub async fn write_metadata(
    target: &Path,
    item: &MediaItem,
) -> Result<PathBuf, Box<dyn Error + Send + Sync>> {
    let output = sidecar_path(target)?;
    let mut value = json!({
        "category": item.extractor,
        "filename": item.filename,
        "post_id": item.post_id,
        "source_url": display_url(&item.url),
    });
    if item.extractor == "directlink" {
        if let Some(crate::message::ExtractorMessage::Url { metadata, .. }) =
            crate::extractor::directlink::resolve_messages(&item.url)?
                .into_iter()
                .last()
        {
            for field in ["domain", "subcategory", "path", "stem", "extension"] {
                if let Some(source_value) = metadata.get(field) {
                    value[field] = source_value.clone();
                }
            }
        }
    }
    write_json_atomic(&output, &value).await
}

/// Write a bounded gallery-level info.json from the directory metadata.
/// URL query and fragment values are excluded to avoid persisting tokens.
pub async fn write_info_json(
    destination: &Path,
    item: &MediaItem,
) -> Result<PathBuf, Box<dyn Error + Send + Sync>> {
    let metadata: Metadata = if item.extractor == "directlink" {
        crate::extractor::directlink::resolve_messages(&item.url)?
            .into_iter()
            .find_map(|message| match message {
                ExtractorMessage::Directory { metadata } => Some(metadata),
                _ => None,
            })
            .unwrap_or_else(|| crate::message::media_metadata(item))
    } else {
        crate::message::media_metadata(item)
    };
    let mut metadata = crate::message::upstream_metadata(&metadata);
    metadata.remove("query");
    metadata.remove("fragment");
    fs::create_dir_all(destination).await?;
    let output = destination.join("info.json");
    write_json_atomic(&output, &serde_json::Value::Object(metadata)).await
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

    #[tokio::test]
    async fn redacted_sidecar_matches_stable_synthetic_golden() {
        let root =
            std::env::temp_dir().join(format!("galerith-metadata-golden-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root).await;
        fs::create_dir_all(&root).await.unwrap();
        let item = extract_direct_media(
            "https://user:private@example.org/picture.jpg?sig=synthetic-secret#image",
        )
        .unwrap();
        let target = root.join("picture.jpg");
        fs::write(&target, b"synthetic image").await.unwrap();

        let sidecar = write_metadata(&target, &item).await.unwrap();
        let actual = fs::read_to_string(sidecar).await.unwrap();
        let expected = include_str!("../tests/fixtures/metadata/directlink.json").trim_end();
        assert_eq!(actual, expected);

        fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn tag_sidecar_is_newline_terminated_and_missing_tags_create_no_file() {
        let root = std::env::temp_dir().join(format!("galerith-tags-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root).await;
        fs::create_dir_all(&root).await.unwrap();
        let target = root.join("picture.jpg");
        let metadata = serde_json::from_value::<Metadata>(serde_json::json!({
            "tags": ["landscape", "night sky"]
        }))
        .unwrap();

        let sidecar = write_tags(&target, &metadata).await.unwrap();
        assert_eq!(sidecar, root.join("picture.jpg.txt"));
        assert_eq!(
            fs::read_to_string(&sidecar).await.unwrap(),
            "landscape\nnight sky\n"
        );

        let missing = write_tags(&root.join("untagged.png"), &Metadata::new())
            .await
            .unwrap();
        assert!(!missing.exists());

        let empty = write_tags(
            &root.join("empty-tags.png"),
            &serde_json::from_value::<Metadata>(serde_json::json!({ "tags": [] })).unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(fs::read(&empty).await.unwrap(), b"\n");
        fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn tag_sidecar_rejects_non_string_and_multiline_tags() {
        let root =
            std::env::temp_dir().join(format!("galerith-invalid-tags-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root).await;
        fs::create_dir_all(&root).await.unwrap();
        let target = root.join("picture.jpg");
        for tags in [
            serde_json::json!("not an array"),
            serde_json::json!([1]),
            serde_json::json!(["safe\nforged"]),
        ] {
            let metadata =
                serde_json::from_value::<Metadata>(serde_json::json!({ "tags": tags })).unwrap();
            assert!(write_tags(&target, &metadata).await.is_err());
            assert!(!root.join("picture.jpg.txt").exists());
        }
        fs::remove_dir_all(root).await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn atomic_sidecar_temp_creation_does_not_follow_existing_symlinks() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!(
            "galerith-metadata-symlink-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root).await;
        fs::create_dir_all(&root).await.unwrap();
        let output = root.join("image.jpg.json");
        let protected = root.join("protected");
        let tmp = root.join("image.jpg.json.tmp.planted");
        fs::write(&protected, b"preserve me").await.unwrap();
        symlink(&protected, &tmp).unwrap();

        assert!(write_atomic(&output, b"replacement", &tmp).await.is_err());
        assert_eq!(fs::read(&protected).await.unwrap(), b"preserve me");
        assert!(!output.exists());
        assert!(fs::symlink_metadata(&tmp)
            .await
            .unwrap()
            .file_type()
            .is_symlink());

        fs::remove_dir_all(root).await.unwrap();
    }
}
