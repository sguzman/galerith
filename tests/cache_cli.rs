//! CLI acceptance for opt-in native public JSON metadata caching.
use std::{fs, path::PathBuf, process::Command};

fn temp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("galerith-cache-cli-{name}-{}", std::process::id()))
}

#[test]
fn cache_options_reject_unsafe_combinations_before_opening_database() {
    let dir = temp("invalid");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let db = dir.join("cache.sqlite");
    for args in [
        vec!["--cache-ttl", "5", "https://example.org/file.jpg"],
        vec![
            "--cache-db",
            db.to_str().unwrap(),
            "--cache-ttl",
            "86401",
            "https://example.org/file.jpg",
        ],
        vec![
            "--cache-db",
            db.to_str().unwrap(),
            "--cookies",
            "dummy.txt",
            "https://example.org/file.jpg",
        ],
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
            "0",
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
        .query_row("SELECT COUNT(*) FROM public_collection_cache", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(records, 0, "direct media does not belong in metadata cache");
    drop(conn);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn cache_rejects_archive_file_aliases_without_mutating_original() {
    let dir = temp("archive-alias");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let foreign = dir.join("foreign.sqlite");
    let reference = rusqlite::Connection::open(&foreign).unwrap();
    reference
        .execute_batch("CREATE TABLE archive(entry TEXT PRIMARY KEY) WITHOUT ROWID; INSERT INTO archive VALUES ('directlinkexample.org//test.jpg');")
        .unwrap();
    drop(reference);
    let bytes = fs::read(&foreign).unwrap();
    let native = dir.join("native.sqlite");
    for reference_arg in ["--gallery-dl-archive", "--download-archive"] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_galerith"));
        command.args(["--cache-db", foreign.to_str().unwrap()]);
        if reference_arg == "--gallery-dl-archive" {
            command.args(["--download-archive", native.to_str().unwrap()]);
        }
        let output = command
            .args([
                reference_arg,
                foreign.to_str().unwrap(),
                "--get-urls",
                "https://example.org/test.jpg",
            ])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("--cache-db must be separate"));
        assert_eq!(fs::read(&foreign).unwrap(), bytes);
    }
    let output = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args([
            "--cache-db",
            native.to_str().unwrap(),
            "--download-archive",
            native.to_str().unwrap(),
            "--get-urls",
            "https://example.org/test.jpg",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!native.exists());
    #[cfg(unix)]
    {
        let sym = dir.join("sym.sqlite");
        let hard = dir.join("hard.sqlite");
        std::os::unix::fs::symlink(&foreign, &sym).unwrap();
        fs::hard_link(&foreign, &hard).unwrap();
        for alias in [&sym, &hard] {
            let output = Command::new(env!("CARGO_BIN_EXE_galerith"))
                .args([
                    "--cache-db",
                    alias.to_str().unwrap(),
                    "--download-archive",
                    native.to_str().unwrap(),
                    "--gallery-dl-archive",
                    foreign.to_str().unwrap(),
                    "--get-urls",
                    "https://example.org/test.jpg",
                ])
                .output()
                .unwrap();
            assert!(!output.status.success());
            assert!(String::from_utf8_lossy(&output.stderr).contains("--cache-db must be separate"));
            assert_eq!(fs::read(&foreign).unwrap(), bytes);
        }
    }
    let subdir = dir.join("sub");
    fs::create_dir_all(&subdir).unwrap();
    let child = subdir.join("new.sqlite");
    let relative = subdir.join("./new.sqlite");
    let output = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args([
            "--cache-db",
            child.to_str().unwrap(),
            "--download-archive",
            relative.to_str().unwrap(),
            "--get-urls",
            "https://example.org/test.jpg",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!child.exists());
    fs::remove_dir_all(dir).unwrap();
}
