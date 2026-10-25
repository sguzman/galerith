//! Native opt-in cache for unauthenticated public collection metadata.
//! Deliberately not compatible with gallery-dl's Python-pickled cache format.
use fs2::FileExt;
use reqwest::Client;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    error::Error,
    fs,
    future::Future,
    io,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use url::Url;

tokio::task_local! { static ACTIVE: Arc<Cache>; }

pub struct Cache {
    path: PathBuf,
    lock_dir: PathBuf,
    ttl: u64,
}

fn digest(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Both supported metadata extractors require a post_ids array. Never
/// retain a syntactically valid API error document as a reusable cache hit.
fn valid_collection(value: &Value) -> bool {
    value
        .get("post_ids")
        .and_then(Value::as_array)
        .is_some_and(|posts| {
            posts
                .iter()
                .all(|post| post.as_u64().is_some_and(|id| id > 0))
        })
}

fn now() -> Result<i64, Box<dyn Error + Send + Sync>> {
    Ok(i64::try_from(
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
    )?)
}

fn connection(path: &Path) -> Result<Connection, Box<dyn Error + Send + Sync>> {
    let db = Connection::open(path)?;
    db.busy_timeout(Duration::from_secs(5))?;
    Ok(db)
}

impl Cache {
    pub fn open(path: &Path, ttl: u64) -> Result<Arc<Self>, Box<dyn Error + Send + Sync>> {
        if ttl > 86_400 || path == Path::new(":memory:") {
            return Err("cache requires a file path and TTL between 0 and 86400 seconds".into());
        }
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        let db = connection(path)?;
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS public_collection_cache (
                key_sha256 TEXT PRIMARY KEY,
                body_json BLOB NOT NULL,
                expires_at INTEGER NOT NULL
            ) WITHOUT ROWID;",
        )?;
        // An expired key may never be requested again. Prune stale metadata
        // when the cache is opened, retaining explicit expires_at=0 entries.
        db.execute(
            "DELETE FROM public_collection_cache WHERE expires_at != 0 AND expires_at <= ?1",
            params![now()?],
        )?;
        drop(db);
        let path = fs::canonicalize(path)?;
        let mut lock_dir = path.as_os_str().to_os_string();
        lock_dir.push(".galerith-cache-locks");
        Ok(Arc::new(Self {
            path,
            lock_dir: PathBuf::from(lock_dir),
            ttl,
        }))
    }

    pub async fn scope<F: Future>(self: Arc<Self>, work: F) -> F::Output {
        ACTIVE.scope(self, work).await
    }

    pub fn current() -> Option<Arc<Self>> {
        ACTIVE.try_with(Arc::clone).ok()
    }

    /// No query, credentials, alternate hosts or media bodies can enter.
    pub fn allows(url: &Url) -> bool {
        if url.scheme() != "https"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.port().is_some()
        {
            return false;
        }
        let Some(host) = url.host_str() else {
            return false;
        };
        if !matches!(
            host,
            "danbooru.donmai.us" | "e621.net" | "e926.net" | "e6ai.net"
        ) {
            return false;
        }
        let Some((collection, filename)) = url.path().trim_start_matches('/').split_once('/')
        else {
            return false;
        };
        if collection != "pools"
            && !(collection == "favorite_groups" && host == "danbooru.donmai.us")
        {
            return false;
        }
        filename
            .strip_suffix(".json")
            .and_then(|id| {
                id.parse::<u64>()
                    .ok()
                    .filter(|n| *n > 0 && n.to_string() == id)
            })
            .is_some()
    }

    fn read(&self, key: &str, limit: usize) -> Result<Option<Value>, Box<dyn Error + Send + Sync>> {
        let db = connection(&self.path)?;
        let row: Option<(Vec<u8>, i64)> = db
            .query_row(
                "SELECT body_json, expires_at FROM public_collection_cache WHERE key_sha256 = ?1",
                params![key],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((body, expires)) = row {
            if (expires == 0 || expires > now()?) && body.len() <= limit {
                if let Ok(value) = serde_json::from_slice(&body) {
                    if valid_collection(&value) {
                        return Ok(Some(value));
                    }
                }
            }
            // Expiration, size-limit changes and malformed stored JSON recover
            // by deleting the bad row and refreshing from the actual source.
            db.execute(
                "DELETE FROM public_collection_cache WHERE key_sha256 = ?1",
                params![key],
            )?;
        }
        Ok(None)
    }

    fn write(
        &self,
        key: &str,
        value: &Value,
        limit: usize,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        let body = serde_json::to_vec(value)?;
        if body.len() > limit {
            return Err("cached response exceeds byte limit".into());
        }
        let expires = if self.ttl == 0 {
            0
        } else {
            now()?
                .checked_add(i64::try_from(self.ttl)?)
                .ok_or("expiry overflow")?
        };
        connection(&self.path)?.execute(
            "INSERT INTO public_collection_cache(key_sha256, body_json, expires_at)
             VALUES (?1, ?2, ?3) ON CONFLICT(key_sha256) DO UPDATE SET
             body_json = excluded.body_json, expires_at = excluded.expires_at",
            params![key, body, expires],
        )?;
        Ok(())
    }

    async fn lock(&self, key: &str) -> Result<fs::File, Box<dyn Error + Send + Sync>> {
        fs::create_dir_all(&self.lock_dir)?;
        let stripe = u8::from_str_radix(&key[..2], 16)? & 63;
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.lock_dir.join(format!("{stripe:02x}.lock")))?;
        loop {
            match FileExt::try_lock_exclusive(&lock) {
                Ok(()) => return Ok(lock),
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                    tokio::time::sleep(Duration::from_millis(30)).await;
                }
                Err(err) => return Err(err.into()),
            }
        }
    }

    /// Single-flight via stable advisory files; OS crash releases the lock,
    /// and SQLite makes replacement atomic. Only successful JSON is cached.
    /// The verified final response URL is part of the persistence decision.
    /// Redirected responses remain usable to callers, but never poison a
    /// first-party cache key with content from a different endpoint.
    async fn fetch_with_final_url<F, Fut>(
        &self,
        endpoint: &Url,
        limit: usize,
        refresh: F,
    ) -> Result<Value, Box<dyn Error + Send + Sync>>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<(Value, Url), Box<dyn Error + Send + Sync>>>,
    {
        if limit == 0 {
            return Err("JSON response byte limit must be positive".into());
        }
        if !Self::allows(endpoint) {
            return Ok(refresh().await?.0);
        }
        let key = digest(endpoint.as_str());
        let _guard = self.lock(&key).await?;
        if let Some(value) = self.read(&key, limit)? {
            return Ok(value);
        }
        let (fresh, final_url) = refresh().await?;
        if final_url == *endpoint && valid_collection(&fresh) {
            self.write(&key, &fresh, limit)?;
        }
        Ok(fresh)
    }

    #[cfg(test)]
    pub(crate) async fn fetch<F, Fut>(
        &self,
        endpoint: &Url,
        limit: usize,
        refresh: F,
    ) -> Result<Value, Box<dyn Error + Send + Sync>>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Value, Box<dyn Error + Send + Sync>>>,
    {
        self.fetch_with_final_url(endpoint, limit, || async {
            Ok((refresh().await?, endpoint.clone()))
        })
        .await
    }

    pub async fn fetch_json(
        &self,
        client: &Client,
        endpoint: Url,
        limit: usize,
    ) -> Result<Value, Box<dyn Error + Send + Sync>> {
        self.fetch_with_final_url(&endpoint, limit, || {
            crate::extractor::http_json::fetch_json_uncached_with_final_url(
                client,
                endpoint.clone(),
                limit,
            )
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "galerith-cache-{name}-{}.sqlite",
            std::process::id()
        ))
    }

    #[test]
    fn public_collection_allowlist_rejects_user_specific_data() {
        for url in [
            "https://e621.net/pools/42.json",
            "https://e926.net/pools/42.json",
            "https://e6ai.net/pools/42.json",
            "https://danbooru.donmai.us/pools/42.json",
            "https://danbooru.donmai.us/favorite_groups/42.json",
        ] {
            assert!(Cache::allows(&Url::parse(url).unwrap()));
        }
        for url in [
            "https://e621.net/favorites/42.json",
            "http://e621.net/pools/42.json",
            "https://e621.net/pools/042.json",
            "https://e621.net/pools/0.json",
            "https://e621.net/pools/42.json?token=secret",
            "https://e621.net:9443/pools/42.json",
            "https://login:secret@e621.net/pools/42.json",
            "https://e621.net/posts/42.json",
            "https://other.example/pools/42.json",
            "https://e621.net/favorite_groups/42.json",
        ] {
            assert!(!Cache::allows(&Url::parse(url).unwrap()), "{url}");
        }
    }

    #[tokio::test]
    async fn redirected_response_is_returned_but_not_cached() {
        let path = path("redirect");
        let _ = fs::remove_file(&path);
        let cache = Cache::open(&path, 60).unwrap();
        let endpoint = Url::parse("https://e621.net/pools/321.json").unwrap();
        let final_url = Url::parse("https://other.example.org/api/error").unwrap();
        let redirected = cache
            .fetch_with_final_url(&endpoint, 1024, || async {
                Ok((serde_json::json!({"post_ids":[321]}), final_url))
            })
            .await
            .unwrap();
        assert_eq!(redirected["post_ids"][0], 321);
        let count: i64 = connection(&path)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM public_collection_cache", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0);
        let proper = cache
            .fetch_with_final_url(&endpoint, 1024, || async {
                Ok((serde_json::json!({"post_ids":[322]}), endpoint.clone()))
            })
            .await
            .unwrap();
        assert_eq!(proper["post_ids"][0], 322);
        assert_eq!(
            cache
                .fetch(&endpoint, 1024, || async {
                    Err::<Value, Box<dyn Error + Send + Sync>>("network unexpectedly used".into())
                })
                .await
                .unwrap()["post_ids"][0],
            322
        );
        drop(cache);
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn out_of_scope_refresh_never_creates_a_cache_record() {
        let path = path("disallowed");
        let _ = fs::remove_file(&path);
        let cache = Cache::open(&path, 60).unwrap();
        let outside = Url::parse("https://other.example/pools/42.json").unwrap();
        assert_eq!(
            cache
                .fetch(&outside, 1024, || async {
                    Ok(serde_json::json!({"post_ids":[1]}))
                })
                .await
                .unwrap()["post_ids"][0],
            1
        );
        assert_eq!(
            cache
                .fetch(&outside, 1024, || async {
                    Ok(serde_json::json!({"post_ids":[2]}))
                })
                .await
                .unwrap()["post_ids"][0],
            2
        );
        let count: i64 = connection(&path)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM public_collection_cache", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0);
        drop(cache);
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn zero_expiry_is_persisted_and_reused_without_expiration() {
        let path = path("forever");
        let _ = fs::remove_file(&path);
        let cache = Cache::open(&path, 0).unwrap();
        let endpoint = Url::parse("https://e621.net/pools/91.json").unwrap();
        let value = cache
            .fetch(&endpoint, 1024, || async {
                Ok(serde_json::json!({"post_ids":[91]}))
            })
            .await
            .unwrap();
        assert_eq!(value["post_ids"][0], 91);
        let stored_expiry: i64 = connection(&path)
            .unwrap()
            .query_row(
                "SELECT expires_at FROM public_collection_cache WHERE key_sha256=?1",
                params![digest(endpoint.as_str())],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored_expiry, 0);
        tokio::time::sleep(Duration::from_millis(1100)).await;
        let reused = cache
            .fetch(&endpoint, 1024, || async {
                Err::<Value, Box<dyn Error + Send + Sync>>("must use permanent cache".into())
            })
            .await
            .unwrap();
        assert_eq!(reused, value);
        drop(cache);
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn reopen_prunes_expired_rows_but_preserves_forever_entries() {
        let path = path("prune");
        let _ = fs::remove_file(&path);
        let cache = Cache::open(&path, 0).unwrap();
        let stale = Url::parse("https://e621.net/pools/92.json").unwrap();
        let forever = Url::parse("https://e621.net/pools/93.json").unwrap();
        for endpoint in [&stale, &forever] {
            cache
                .fetch(endpoint, 1024, || async {
                    Ok(serde_json::json!({"post_ids":[93]}))
                })
                .await
                .unwrap();
        }
        connection(&path)
            .unwrap()
            .execute(
                "UPDATE public_collection_cache SET expires_at = 1 WHERE key_sha256 = ?1",
                params![digest(stale.as_str())],
            )
            .unwrap();
        drop(cache);
        let reopened = Cache::open(&path, 30).unwrap();
        let row_count: i64 = connection(&path)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM public_collection_cache", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(row_count, 1);
        let preserved = reopened
            .fetch(&forever, 1024, || async {
                Err::<Value, Box<dyn Error + Send + Sync>>("must preserve forever".into())
            })
            .await
            .unwrap();
        assert_eq!(preserved["post_ids"][0], 93);
        let restored = reopened
            .fetch(&stale, 1024, || async {
                Ok(serde_json::json!({"post_ids":[92]}))
            })
            .await
            .unwrap();
        assert_eq!(restored["post_ids"][0], 92);
        drop(reopened);
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn expiry_corruption_and_failed_refresh_do_not_poison_cache() {
        let path = path("expiry");
        let _ = fs::remove_file(&path);
        let cache = Cache::open(&path, 1).unwrap();
        let endpoint = Url::parse("https://e621.net/pools/42.json").unwrap();
        let calls = AtomicUsize::new(0);
        let first = cache
            .fetch(&endpoint, 1024, || async {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(serde_json::json!({"post_ids":[1]}))
            })
            .await
            .unwrap();
        assert_eq!(first["post_ids"][0], 1);
        let again = cache
            .fetch(&endpoint, 1024, || async {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(serde_json::json!({"post_ids":[9]}))
            })
            .await
            .unwrap();
        assert_eq!(again, first);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        connection(&path)
            .unwrap()
            .execute(
                "UPDATE public_collection_cache SET body_json=?1 WHERE key_sha256=?2",
                params![b"corrupt".as_slice(), digest(endpoint.as_str())],
            )
            .unwrap();
        assert_eq!(
            cache
                .fetch(&endpoint, 1024, || async {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(serde_json::json!({"post_ids":[2]}))
                })
                .await
                .unwrap()["post_ids"][0],
            2
        );
        tokio::time::sleep(Duration::from_millis(1200)).await;
        assert!(cache
            .fetch(&endpoint, 1024, || async {
                Err::<Value, Box<dyn Error + Send + Sync>>("remote failed".into())
            })
            .await
            .is_err());
        assert_eq!(
            cache
                .fetch(&endpoint, 1024, || async {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(serde_json::json!({"post_ids":[3]}))
                })
                .await
                .unwrap()["post_ids"][0],
            3
        );
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        drop(cache);
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn valid_json_with_wrong_schema_is_never_cached_or_reused() {
        let path = path("schema");
        let _ = fs::remove_file(&path);
        let cache = Cache::open(&path, 30).unwrap();
        let endpoint = Url::parse("https://e621.net/pools/77.json").unwrap();
        let bad = cache
            .fetch(&endpoint, 1024, || async {
                Ok(serde_json::json!({"message":"permission denied"}))
            })
            .await
            .unwrap();
        assert_eq!(bad["message"], "permission denied");
        let recovered = cache
            .fetch(&endpoint, 1024, || async {
                Ok(serde_json::json!({"post_ids":[77]}))
            })
            .await
            .unwrap();
        assert_eq!(recovered["post_ids"][0], 77);
        connection(&path)
            .unwrap()
            .execute(
                "UPDATE public_collection_cache SET body_json = ?1 WHERE key_sha256 = ?2",
                params![
                    serde_json::to_vec(&serde_json::json!({"message":"stale error"})).unwrap(),
                    digest(endpoint.as_str())
                ],
            )
            .unwrap();
        let refreshed = cache
            .fetch(&endpoint, 1024, || async {
                Ok(serde_json::json!({"post_ids":[78]}))
            })
            .await
            .unwrap();
        assert_eq!(refreshed["post_ids"][0], 78);
        assert!(cache
            .fetch(&endpoint, 1024, || async {
                Err::<Value, Box<dyn Error + Send + Sync>>("must not hit network".into())
            })
            .await
            .is_ok());
        drop(cache);
        let _ = fs::remove_file(path);
    }

    /// Invoked only by the parent recovery regression. Run in a separate
    /// test-harness process so SIGKILL actually releases an OS file lock.
    #[test]
    fn locked_cache_process_helper() {
        let Some(db) = std::env::var_os("GALERITH_TEST_CACHE_DB") else {
            return;
        };
        let ready = std::env::var_os("GALERITH_TEST_CACHE_READY").unwrap();
        let cache = Cache::open(Path::new(&db), 60).unwrap();
        let url = Url::parse("https://e621.net/pools/901.json").unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let _ = cache
                .fetch(&url, 1024, || async {
                    fs::write(ready, b"locked").unwrap();
                    std::future::pending::<Result<Value, Box<dyn Error + Send + Sync>>>().await
                })
                .await;
        });
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn killed_process_releases_cache_lock_without_poisoning_database() {
        let db = path("killed-worker");
        let ready = path("killed-worker-ready");
        let _ = fs::remove_file(&db);
        let _ = fs::remove_file(&ready);
        let parent_cache = Cache::open(&db, 60).unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "cache::tests::locked_cache_process_helper"])
            .env("GALERITH_TEST_CACHE_DB", &db)
            .env("GALERITH_TEST_CACHE_READY", &ready)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let start = tokio::time::Instant::now();
        while !ready.exists() && start.elapsed() < Duration::from_secs(5) {
            if let Some(status) = child.try_wait().unwrap() {
                panic!("lock-holder process unexpectedly exited: {status}");
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        if !ready.exists() {
            let _ = child.kill();
            let _ = child.wait();
            panic!("cache lock-holder process did not become ready");
        }
        child.kill().unwrap();
        child.wait().unwrap();
        let endpoint = Url::parse("https://e621.net/pools/901.json").unwrap();
        let recovered = tokio::time::timeout(
            Duration::from_secs(5),
            parent_cache.fetch(&endpoint, 1024, || async {
                Ok(serde_json::json!({"post_ids":[901]}))
            }),
        )
        .await
        .expect("OS lock was not released after process termination")
        .unwrap();
        assert_eq!(recovered["post_ids"][0], 901);
        drop(parent_cache);
        let _ = fs::remove_file(db);
        let _ = fs::remove_file(ready);
    }

    #[tokio::test]
    async fn independent_handles_serialize_refresh_and_reopen() {
        let path = path("parallel");
        let _ = fs::remove_file(&path);
        let left = Cache::open(&path, 30).unwrap();
        let right = Cache::open(&path, 30).unwrap();
        let endpoint = Url::parse("https://e621.net/pools/42.json").unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let a_hits = Arc::clone(&hits);
        let b_hits = Arc::clone(&hits);
        let task_endpoint = endpoint.clone();
        let task = tokio::spawn(async move {
            left.fetch(&task_endpoint, 1024, || async move {
                a_hits.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(175)).await;
                Ok(serde_json::json!({"post_ids":[1]}))
            })
            .await
            .unwrap()
        });
        tokio::time::sleep(Duration::from_millis(40)).await;
        let same = right
            .fetch(&endpoint, 1024, || async move {
                b_hits.fetch_add(1, Ordering::SeqCst);
                Ok(serde_json::json!({"post_ids":[9]}))
            })
            .await
            .unwrap();
        assert_eq!(task.await.unwrap(), same);
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        let again = Cache::open(&path, 30).unwrap();
        assert_eq!(
            again
                .fetch(&endpoint, 1024, || async {
                    Err::<Value, Box<dyn Error + Send + Sync>>("would refresh".into())
                })
                .await
                .unwrap(),
            same
        );
        let _ = fs::remove_file(path);
    }
}
