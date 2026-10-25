//! CLI acceptance for opt-in native public JSON metadata caching.
use std::{fs, path::PathBuf, process::Command};

fn temp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "galerith-cache-cli-{name}-{}",
        std::process::id()
    ))
}

#[test]
fn cache_options_reject_unsafe_combinations_before_opening_database() {
    let dir = temp("invalid");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let db = dir.join("cache.sqlite");
    for args in [
        vec!["--cache-ttl", "5", "https://example.org/file.jpg"],
        vec!["--cache-db", db.to_str().unwrap(), "--cache-ttl", "0", "https://example.org/file.jpg"],
        vec!["--cache-db", db.to_str().unwrap(), "--cache-ttl", "86401", "https://example.org/file.jpg"],
        vec!["--cache-db", db.to_str().unwrap(), "--cookies", "dummy.txt", "https://example.org/file.jpg"],
    ] {
        let result = Command::new(env!("CARGO_BIN_EXE_galerith"))
            .args(args)
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert!(!db.exists(), "invalid cache option created native database");
    }
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn cache_is_explicit_and_has_independent_sqlite_schema() {
    let dir = temp("create");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let db = dir.join("public.sqlite");
    let result = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args([
            "--cache-db",
            db.to_str().unwrap(),
            "--cache-ttl",
            "60",
            "--get-urls",
            "https://example.org/photo.jpg",
        ])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let conn = rusqlite::Connection::open(&db).unwrap();
    let tables: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='public_collection_cache'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(tables, 1);
    let records: i64 = conn
        .query_row("SELECT COUNT(*) FROM public_collection_cache", [], |row| row.get(0))
        .unwrap();
    assert_eq!(records, 0, "direct media does not belong in metadata cache");
    drop(conn);
    fs::remove_dir_all(dir).unwrap();
}
