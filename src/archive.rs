//! Native SQLite archive; its schema is not yet gallery-dl compatible.
use crate::MediaItem;
use fs2::FileExt;
use rusqlite::{params, Connection, OpenFlags};
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
    upstream_directlink_archive: bool,
    upstream_reference: Option<Connection>,
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

/// The pinned gallery-dl directlink archive format is the category prefix
/// followed by "{domain}/{path}/{filename}.{extension}". The upstream
/// path field starts after the leading URL slash, so nested paths retain
/// one domain separator while root-level files have an empty path field.
/// Signed URL query parameters do not contribute to this upstream key.
/// Do not write these potentially sensitive plaintext path keys from Galerith.
fn upstream_directlink_key(item: &MediaItem) -> Option<String> {
    if item.extractor != "directlink" {
        return None;
    }
    let parsed = url::Url::parse(&item.url).ok()?;
    if !matches!(parsed.scheme(), "http" | "https")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return None;
    }
    let domain = parsed.host_str()?;
    let authority = match parsed.port() {
        Some(port) => format!("{domain}:{port}"),
        None => domain.to_owned(),
    };
    // The upstream regex consumes the first '/' outside its path capture.
    let url_path = parsed.path().strip_prefix('/')?;
    let decoded = percent_encoding::percent_decode_str(url_path)
        .decode_utf8()
        .ok()?;
    let (directory, name) = decoded.rsplit_once('/').unwrap_or(("", decoded.as_ref()));
    let (stem, ext) = name.rsplit_once('.')?;
    if stem.is_empty() || ext.is_empty() {
        return None;
    }
    Some(format!(
        "directlink{authority}/{directory}/{stem}.{}",
        ext.to_ascii_lowercase()
    ))
}

fn has_upstream_table(connection: &Connection) -> Result<bool, rusqlite::Error> {
    let found: i64 = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('archive') WHERE name = 'entry' AND pk = 1 AND UPPER(type) = 'TEXT')",
        [],
        |row| row.get(0),
    )?;
    Ok(found != 0)
}

impl Archive {
    pub fn open(path: &Path) -> Result<Self, Box<dyn Error + Send + Sync>> {
        Self::open_with_reference(path, None)
    }

    /// Optionally use an *existing* gallery-dl archive as a strictly
    /// read-only fallback for pinned directlink identities. Galerith writes
    /// all completed downloads to its own database, never the reference.
    pub fn open_with_reference(
        path: &Path,
        reference_path: Option<&Path>,
    ) -> Result<Self, Box<dyn Error + Send + Sync>> {
        // Validate the upstream source before creating Galerith's own DB.
        // The READ_ONLY flag never creates a missing upstream file.
        let upstream_reference = if let Some(reference_path) = reference_path {
            let source =
                Connection::open_with_flags(reference_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            source.busy_timeout(Duration::from_secs(5))?;
            if !has_upstream_table(&source)? {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "gallery-dl archive requires archive(entry TEXT PRIMARY KEY)",
                )
                .into());
            }
            // A read-only handle does not protect the original if the native
            // destination points to the *same* database via a relative path
            // or symlink. Reject before CREATE TABLE can modify that file.
            if path.exists() && fs::canonicalize(path)? == fs::canonicalize(reference_path)? {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "native archive and read-only gallery-dl archive must be different files",
                )
                .into());
            }
            Some(source)
        } else {
            None
        };
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
        // Only consult a preexisting upstream archive table with the expected
        // primary-key column. Never create or mutate upstream's archive rows.
        let upstream_directlink_archive = has_upstream_table(&connection)?;
        Ok(Self {
            connection,
            lock_directory,
            upstream_directlink_archive,
            upstream_reference,
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
        if present != 0 {
            return Ok(true);
        }
        if let Some(source_key) = upstream_directlink_key(item) {
            // Explicit external reference takes precedence, then the
            // existing same-file fallback for legacy mixed databases.
            let source = self
                .upstream_reference
                .as_ref()
                .or_else(|| self.upstream_directlink_archive.then_some(&self.connection));
            if let Some(source) = source {
                let upstream: i64 = source.query_row(
                    "SELECT EXISTS(SELECT 1 FROM archive WHERE entry = ?1)",
                    params![source_key],
                    |row| row.get(0),
                )?;
                return Ok(upstream != 0);
            }
        }
        Ok(false)
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
    fn reads_pinned_gallery_dl_directlink_entries_without_writing_to_them() {
        let path = std::env::temp_dir().join(format!(
            "galerith-upstream-archive-{}.sqlite",
            std::process::id()
        ));
        let _ = fs::remove_file(&path);
        let original = Connection::open(&path).unwrap();
        original
            .execute_batch("CREATE TABLE archive(entry TEXT PRIMARY KEY) WITHOUT ROWID;")
            .unwrap();
        original
            .execute(
                "INSERT INTO archive(entry) VALUES (?1)",
                params!["directlinkexample.org/images/photo.jpg"],
            )
            .unwrap();
        drop(original);

        let archive = Archive::open(&path).unwrap();
        let old = extract_direct_media("https://example.org/images/photo.JPG?X-Amz-Signature=old")
            .unwrap();
        assert!(archive.contains(&old).unwrap());
        let rotated =
            extract_direct_media("https://example.org/images/photo.JPG?X-Amz-Signature=rotated")
                .unwrap();
        assert!(archive.contains(&rotated).unwrap());
        let another = extract_direct_media("https://example.org/images/other.jpg").unwrap();
        assert!(!archive.contains(&another).unwrap());
        assert_eq!(
            upstream_directlink_key(
                &extract_direct_media("https://example.org/photo.jpg?auth=rotated").unwrap()
            ),
            Some("directlinkexample.org//photo.jpg".to_owned())
        );
        archive.record(&another).unwrap();
        assert!(archive.contains(&another).unwrap());
        let entries: i64 = archive
            .connection
            .query_row("SELECT COUNT(*) FROM archive", [], |row| row.get(0))
            .unwrap();
        assert_eq!(entries, 1);
        let lock_dir = archive.lock_directory.clone().unwrap();
        drop(archive);
        if lock_dir.exists() {
            fs::remove_dir_all(lock_dir).unwrap();
        }
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn external_gallery_dl_archive_is_never_modified() {
        let root =
            std::env::temp_dir().join(format!("galerith-external-archive-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let native = root.join("native.sqlite");
        let reference = root.join("gallery-dl.sqlite");
        let original = Connection::open(&reference).unwrap();
        original
            .execute_batch("CREATE TABLE archive(entry TEXT PRIMARY KEY) WITHOUT ROWID;")
            .unwrap();
        original
            .execute(
                "INSERT INTO archive(entry) VALUES (?1)",
                params!["directlinkexample.org/gallery/known.jpg"],
            )
            .unwrap();
        drop(original);
        let original_bytes = fs::read(&reference).unwrap();

        let archive = Archive::open_with_reference(&native, Some(&reference)).unwrap();
        let known = extract_direct_media("https://example.org/gallery/known.jpg?sig=new").unwrap();
        assert!(archive.contains(&known).unwrap());
        let unknown = extract_direct_media("https://example.org/gallery/new.png").unwrap();
        assert!(!archive.contains(&unknown).unwrap());
        archive.record(&unknown).unwrap();
        assert!(archive.contains(&unknown).unwrap());
        drop(archive);
        assert_eq!(fs::read(&reference).unwrap(), original_bytes);
        let foreign =
            Connection::open_with_flags(&reference, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        assert!(!has_upstream_table(&Connection::open(&native).unwrap()).unwrap());
        assert!(has_upstream_table(&foreign).unwrap());
        let rows: i64 = foreign
            .query_row("SELECT COUNT(*) FROM archive", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, 1);
        drop(foreign);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn same_file_reference_is_rejected_without_modifying_database() {
        let root =
            std::env::temp_dir().join(format!("galerith-reference-alias-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let reference = root.join("gallery.sqlite");
        let alias = root.join("alias.sqlite");
        let original = Connection::open(&reference).unwrap();
        original
            .execute_batch("CREATE TABLE archive(entry TEXT PRIMARY KEY) WITHOUT ROWID;")
            .unwrap();
        drop(original);
        let original_bytes = fs::read(&reference).unwrap();

        assert!(Archive::open_with_reference(&reference, Some(&reference)).is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&reference, &alias).unwrap();
            assert!(Archive::open_with_reference(&alias, Some(&reference)).is_err());
        }
        assert_eq!(fs::read(&reference).unwrap(), original_bytes);
        let existing = Connection::open(&reference).unwrap();
        let native_table: i64 = existing
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='downloads')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(native_table, 0);
        drop(existing);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_or_malformed_external_archive_fails_without_creating_native() {
        let root =
            std::env::temp_dir().join(format!("galerith-external-invalid-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let native = root.join("native.sqlite");
        let missing = root.join("missing.sqlite");
        assert!(Archive::open_with_reference(&native, Some(&missing)).is_err());
        assert!(!native.exists());
        assert!(!missing.exists());
        let invalid = root.join("invalid.sqlite");
        let connection = Connection::open(&invalid).unwrap();
        connection
            .execute_batch("CREATE TABLE archive(entry INTEGER PRIMARY KEY);")
            .unwrap();
        drop(connection);
        assert!(Archive::open_with_reference(&native, Some(&invalid)).is_err());
        assert!(!native.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unrelated_archive_table_does_not_trigger_false_matches() {
        let path = std::env::temp_dir().join(format!(
            "galerith-unrelated-archive-{}.sqlite",
            std::process::id()
        ));
        let _ = fs::remove_file(&path);
        let con = Connection::open(&path).unwrap();
        con.execute_batch("CREATE TABLE archive(other TEXT PRIMARY KEY);")
            .unwrap();
        drop(con);
        let archive = Archive::open(&path).unwrap();
        assert!(!archive.upstream_directlink_archive);
        assert!(!archive
            .contains(&extract_direct_media("https://example.org/image.jpg").unwrap())
            .unwrap());
        drop(archive);
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
