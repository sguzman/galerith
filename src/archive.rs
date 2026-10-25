//! Native SQLite archive; its schema is not yet gallery-dl compatible.
use crate::MediaItem;
use fs2::FileExt;
use rusqlite::{params, Connection};
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
}

/// Never unlink a held advisory lock file: doing so can split its namespace.
pub struct ArchiveItemLock(std::fs::File);

impl Drop for ArchiveItemLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

fn key(item: &MediaItem) -> String {
    // Archive entries are recorded per downloaded media item, not per post.
    // Posts may contain several distinct originals with the same post_id.
    // Use the full source URL fingerprint to avoid incorrectly treating
    // query-distinguished resources as the same media. Persist only the hash,
    // never source URLs or signed query credentials.
    let digest = Sha256::digest(item.url.as_bytes());
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    match item.post_id {
        Some(id) => format!("post:{id}:url-sha256:{hex}"),
        None => format!("url-sha256:{hex}"),
    }
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
        // Canonicalize the newly opened database path: relative and symlink
        // aliases must share the same interprocess lock namespace.
        let lock_directory = if path == Path::new(":memory:") {
            None
        } else {
            let mut name = fs::canonicalize(path)?.into_os_string();
            name.push(".galerith-locks");
            Some(PathBuf::from(name))
        };
        Ok(Self {
            connection,
            lock_directory,
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
        let Some(directory) = &self.lock_directory else {
            return Ok(None);
        };
        fs::create_dir_all(directory)?;
        let mut hasher = Sha256::new();
        hasher.update(item.extractor.as_bytes());
        hasher.update([0]);
        hasher.update(key(item).as_bytes());
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
            url: "https://cdn.example.org/lock/5.jpg?token=hidden".into(),
            filename: "5.jpg".into(),
            extractor: "archive-lock-test",
            post_id: Some(5),
        };
        let first = Archive::open(&path).unwrap();
        let guard = first.lock_item(&item).await.unwrap().unwrap();
        assert!(!first.contains(&item).unwrap());
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let next_path = path.clone();
        let next_item = item.clone();
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
