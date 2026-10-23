//! Native SQLite archive; its schema is not yet gallery-dl compatible.
use crate::MediaItem;
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};
use std::{error::Error, fs, path::Path, time::Duration};

pub struct Archive {
    connection: Connection,
}

fn key(item: &MediaItem) -> String {
    if let Some(id) = item.post_id {
        return format!("post:{id}");
    }
    let digest = Sha256::digest(item.url.as_bytes());
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("url-sha256:{hex}")
}

impl Archive {
    pub fn open(path: &Path) -> Result<Self, Box<dyn Error + Send + Sync>> {
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
            );",
        )?;
        Ok(Self { connection })
    }

    pub fn contains(&self, item: &MediaItem) -> Result<bool, rusqlite::Error> {
        let present: i64 = self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM downloads WHERE category = ?1 AND identity = ?2)",
            params![item.extractor, key(item)],
            |row| row.get(0),
        )?;
        Ok(present != 0)
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
