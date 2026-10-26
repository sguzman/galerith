//! Verify archive behavior through the native executable without any live site.
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

fn temp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "galerith-archive-cli-{name}-{}",
        std::process::id()
    ))
}

// Deterministically model a validated partial from a previous process.
// HTTP stacks differ in whether a truncated Content-Length body exposes its
// initial chunk before surfacing the connection-closed error.
fn seed_resumable_partial(dir: &Path, file: &str, url: &str, etag: &str) {
    let sha = Sha256::digest(url.as_bytes());
    let fingerprint: String = sha.iter().map(|byte| format!("{byte:02x}")).collect();
    let metadata = serde_json::json!({
        "source_sha256": fingerprint,
        "etag": etag,
        "last_modified": null
    });
    fs::write(dir.join(format!("{file}.part")), b"1234").unwrap();
    fs::write(
        dir.join(format!("{file}.part.json")),
        serde_json::to_vec(&metadata).unwrap(),
    )
    .unwrap();
}

#[test]
fn completed_download_is_not_fetched_again_even_if_file_is_removed() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut bytes = [0u8; 1024];
        assert!(socket.read(&mut bytes).unwrap() > 0);
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nPASS")
            .unwrap();
    });
    let dir = temp("stored");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let db = dir.join("archive.sqlite");
    let url = format!("http://{addr}/saved.jpg");
    let args = [
        "-d".to_owned(),
        dir.to_string_lossy().into_owned(),
        "--download-archive".to_owned(),
        db.to_string_lossy().into_owned(),
        url,
    ];

    let first = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args(&args)
        .output()
        .unwrap();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    server.join().unwrap();
    assert_eq!(fs::read(dir.join("saved.jpg")).unwrap(), b"PASS");
    assert!(db.exists());
    fs::remove_file(dir.join("saved.jpg")).unwrap();

    // The HTTP server is no longer running. A missing archive check would
    // attempt a connection and fail rather than succeeding with "archived".
    let second = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args(&args)
        .output()
        .unwrap();
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert!(String::from_utf8_lossy(&second.stdout).contains("archived saved.jpg"));
    assert!(!dir.join("saved.jpg").exists());
    fs::remove_dir_all(dir).unwrap();
}

#[cfg(unix)]
#[test]
fn existing_symlink_does_not_count_as_archive_skip_or_overwrite_target() {
    use std::os::unix::fs::symlink;
    let dir = temp("symlinked-completed-media");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let unrelated = dir.join("unrelated.txt");
    fs::write(&unrelated, b"KEEP-ME").unwrap();
    let alias = dir.join("linked.jpg");
    symlink(&unrelated, &alias).unwrap();
    let db = dir.join("archive.sqlite");
    // With no server, a valid existing file would get skipped and archived
    // when --archive-event includes skip. A symlink must fail before HTTP.
    let failed = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .arg("-d")
        .arg(&dir)
        .arg("--download-archive")
        .arg(&db)
        .args(["--archive-event", "file,skip", "--retries", "0"])
        .arg("http://127.0.0.1:1/linked.jpg")
        .output()
        .unwrap();
    assert!(!failed.status.success());
    assert_eq!(fs::read(&unrelated).unwrap(), b"KEEP-ME");
    assert!(fs::symlink_metadata(&alias)
        .unwrap()
        .file_type()
        .is_symlink());
    let count: i64 = rusqlite::Connection::open(&db)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM downloads", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        count, 0,
        "symlinked output must never create archive history"
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn pdf_soft_error_does_not_create_an_archive_entry() {
    let dir = temp("pdf-soft-error");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let db = dir.join("archive.sqlite");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        for (path, mime, content) in [
            (
                "/manual.pdf",
                "text/html",
                b"<html>Blocked</html>".as_slice(),
            ),
            ("/manual.pdf", "application/pdf", b"%PDF-TEST".as_slice()),
        ] {
            let deadline = std::time::Instant::now() + Duration::from_secs(8);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "timed out waiting for document fixture"
                        );
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    Err(err) => panic!("document fixture listener error: {err}"),
                }
            };
            let mut buffer = [0_u8; 2048];
            let count = stream.read(&mut buffer).unwrap();
            let request = String::from_utf8_lossy(&buffer[..count]);
            assert!(request.contains(path), "{request}");
            let headers = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                content.len()
            );
            stream.write_all(headers.as_bytes()).unwrap();
            stream.write_all(content).unwrap();
        }
    });
    let invoke = |path: &str| {
        Command::new(env!("CARGO_BIN_EXE_galerith"))
            .arg("-d")
            .arg(&dir)
            .arg("--download-archive")
            .arg(&db)
            .args(["--retries", "0", "--http-timeout", "3"])
            .arg(format!("http://{addr}/{path}"))
            .output()
            .unwrap()
    };
    let failure = invoke("manual.pdf");
    assert!(!failure.status.success());
    assert!(!dir.join("manual.pdf").exists());
    let count = |db: &Path| -> i64 {
        rusqlite::Connection::open(db)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM downloads", [], |row| row.get(0))
            .unwrap()
    };
    assert_eq!(count(&db), 0);
    let completed = invoke("manual.pdf");
    assert!(
        completed.status.success(),
        "{}",
        String::from_utf8_lossy(&completed.stderr)
    );
    assert_eq!(fs::read(dir.join("manual.pdf")).unwrap(), b"%PDF-TEST");
    assert_eq!(count(&db), 1);
    server.join().unwrap();
    assert_eq!(count(&db), 1);
    fs::remove_file(dir.join("manual.pdf")).unwrap();
    let archived = invoke("manual.pdf");
    assert!(archived.status.success());
    assert!(String::from_utf8_lossy(&archived.stdout).contains("archived manual.pdf"));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn misleading_binary_mime_or_missing_content_type_cannot_poison_archive() {
    let dir = temp("misleading-binary-soft-errors");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let db = dir.join("archive.sqlite");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/image.jpg", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        for (mime, body) in [
            (
                Some("application/octet-stream"),
                b"<html>Blocked</html>".as_slice(),
            ),
            (None, b"{\"error\":\"denied\"}".as_slice()),
            (Some("image/jpeg"), b"GOOD".as_slice()),
        ] {
            let deadline = std::time::Instant::now() + Duration::from_secs(8);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "timed out waiting for disguised media fixture"
                        );
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    Err(err) => panic!("disguised media fixture listener error: {err}"),
                }
            };
            let mut buffer = [0_u8; 2048];
            let n = stream.read(&mut buffer).unwrap();
            assert!(String::from_utf8_lossy(&buffer[..n]).contains("/image.jpg"));
            let declared = mime
                .map(|mime| format!("Content-Type: {mime}\r\n"))
                .unwrap_or_default();
            let response = format!(
                "HTTP/1.1 200 OK\r\n{declared}Content-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
            stream.write_all(body).unwrap();
        }
    });
    let invoke = || {
        Command::new(env!("CARGO_BIN_EXE_galerith"))
            .arg("-d")
            .arg(&dir)
            .arg("--download-archive")
            .arg(&db)
            .args(["--retries", "0", "--http-timeout", "3"])
            .arg(&url)
            .output()
            .unwrap()
    };
    for _ in 0..2 {
        let failed = invoke();
        assert!(!failed.status.success(), "disguised error was accepted");
        assert!(!dir.join("image.jpg").exists());
        assert!(!dir.join("image.jpg.part").exists());
        assert!(!dir.join("image.jpg.part.json").exists());
        let count: i64 = rusqlite::Connection::open(&db)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM downloads", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }
    let saved = invoke();
    assert!(
        saved.status.success(),
        "{}",
        String::from_utf8_lossy(&saved.stderr)
    );
    server.join().unwrap();
    assert_eq!(fs::read(dir.join("image.jpg")).unwrap(), b"GOOD");
    let count: i64 = rusqlite::Connection::open(&db)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM downloads", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
    fs::remove_file(dir.join("image.jpg")).unwrap();
    let archived = invoke();
    assert!(archived.status.success());
    assert!(String::from_utf8_lossy(&archived.stdout).contains("archived image.jpg"));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn soft_200_error_documents_cannot_become_archived_image_media() {
    let dir = temp("soft-200-error-document");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let db = dir.join("archive.sqlite");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/media.jpg", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        for (content_type, body) in [
            (
                "text/html; charset=utf-8",
                b"<html>Blocked</html>".as_slice(),
            ),
            (
                "application/problem+json",
                b"{\"error\":\"blocked\"}".as_slice(),
            ),
            ("image/jpeg", b"GOOD".as_slice()),
        ] {
            let deadline = std::time::Instant::now() + Duration::from_secs(8);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "timed out waiting for soft error document test"
                        );
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    Err(err) => panic!("soft document server error: {err}"),
                }
            };
            let mut buffer = [0_u8; 2048];
            let count = stream.read(&mut buffer).unwrap();
            let request = String::from_utf8_lossy(&buffer[..count]);
            assert!(request.contains("/media.jpg"), "{request}");
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(header.as_bytes()).unwrap();
            stream.write_all(body).unwrap();
        }
    });
    let invoke = || {
        Command::new(env!("CARGO_BIN_EXE_galerith"))
            .arg("-d")
            .arg(&dir)
            .arg("--download-archive")
            .arg(&db)
            .args(["--retries", "0", "--http-timeout", "3"])
            .arg(&url)
            .output()
            .unwrap()
    };
    for _ in 0..2 {
        let failed = invoke();
        assert!(!failed.status.success(), "error document was accepted");
        assert!(!dir.join("media.jpg").exists());
        assert!(!dir.join("media.jpg.part").exists());
        assert!(!dir.join("media.jpg.part.json").exists());
        let count: i64 = rusqlite::Connection::open(&db)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM downloads", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0, "HTML/JSON error must never become an archive hit");
    }
    let successful = invoke();
    assert!(
        successful.status.success(),
        "{}",
        String::from_utf8_lossy(&successful.stderr)
    );
    server.join().unwrap();
    assert_eq!(fs::read(dir.join("media.jpg")).unwrap(), b"GOOD");
    let count: i64 = rusqlite::Connection::open(&db)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM downloads", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
    fs::remove_file(dir.join("media.jpg")).unwrap();
    let archived = invoke();
    assert!(archived.status.success());
    assert!(String::from_utf8_lossy(&archived.stdout).contains("archived media.jpg"));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn failed_download_never_creates_an_archive_hit_but_later_retry_does() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        for (status, body) in [
            ("503 Service Unavailable", b"ERROR".as_slice()),
            ("200 OK", b"PASS".as_slice()),
        ] {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0_u8; 1024];
            let count = socket.read(&mut request).unwrap();
            assert!(count > 0);
            assert!(String::from_utf8_lossy(&request[..count]).contains("/retry.jpg"));
            let headers = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            socket.write_all(headers.as_bytes()).unwrap();
            socket.write_all(body).unwrap();
        }
    });
    let dir = temp("failed-then-success");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let db = dir.join("archive.sqlite");
    let url = format!("http://{addr}/retry.jpg");
    let args = [
        "-d".to_owned(),
        dir.to_string_lossy().into_owned(),
        "--download-archive".to_owned(),
        db.to_string_lossy().into_owned(),
        "--retries".to_owned(),
        "0".into(),
        url,
    ];
    let failed = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args(&args)
        .output()
        .unwrap();
    assert!(!failed.status.success());
    assert!(!dir.join("retry.jpg").exists());
    let zero: i64 = rusqlite::Connection::open(&db)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM downloads", [], |row| row.get(0))
        .unwrap();
    assert_eq!(zero, 0, "failed HTTP status must not be archived");

    let succeeded = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args(&args)
        .output()
        .unwrap();
    assert!(
        succeeded.status.success(),
        "{}",
        String::from_utf8_lossy(&succeeded.stderr)
    );
    server.join().unwrap();
    assert_eq!(fs::read(dir.join("retry.jpg")).unwrap(), b"PASS");
    let one: i64 = rusqlite::Connection::open(&db)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM downloads", [], |row| row.get(0))
        .unwrap();
    assert_eq!(one, 1);

    fs::remove_file(dir.join("retry.jpg")).unwrap();
    let archived = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args(&args)
        .output()
        .unwrap();
    assert!(
        archived.status.success(),
        "{}",
        String::from_utf8_lossy(&archived.stderr)
    );
    assert!(String::from_utf8_lossy(&archived.stdout).contains("archived retry.jpg"));
    assert!(!dir.join("retry.jpg").exists());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn extraction_only_modes_do_not_create_archive_database() {
    let dir = temp("dry-run");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let db = dir.join("dont-create.sqlite");
    let result = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args(["--get-urls", "--download-archive"])
        .arg(&db)
        .arg("https://example.org/asset.png")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!db.exists());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn concurrent_cli_processes_do_not_fetch_the_same_archived_media_twice() {
    let dir = temp("concurrent");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let first_dir = dir.join("first");
    let second_dir = dir.join("second");
    let db = dir.join("archive.sqlite");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{addr}/race.jpg");

    let (seen_tx, seen_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut buffer = [0u8; 1024];
        assert!(socket.read(&mut buffer).unwrap() > 0);
        seen_tx.send(()).unwrap();
        release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nDATA")
            .unwrap();
    });
    let launch = |dest: &std::path::Path| {
        Command::new(env!("CARGO_BIN_EXE_galerith"))
            .arg("-d")
            .arg(dest)
            .arg("--download-archive")
            .arg(&db)
            .args(["--retries", "0", "--http-timeout", "2"])
            .arg(&url)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    };

    let first = launch(&first_dir);
    seen_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    // The first request is held open while the second independent process
    // starts and attempts to acquire the same archive identity lock.
    let second = launch(&second_dir);
    std::thread::sleep(Duration::from_millis(300));
    release_tx.send(()).unwrap();
    server.join().unwrap();
    let first = first.wait_with_output().unwrap();
    let second = second.wait_with_output().unwrap();
    assert!(
        first.status.success(),
        "first stderr: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(
        second.status.success(),
        "second stderr: {}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert_eq!(fs::read(first_dir.join("race.jpg")).unwrap(), b"DATA");
    assert!(!second_dir.join("race.jpg").exists());
    assert!(String::from_utf8_lossy(&second.stdout).contains("archived race.jpg"));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn rate_limited_retry_respects_archive_commit_only_after_success() {
    let dir = temp("rate-limit-retry");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let archive = dir.join("archive.sqlite");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/limited.jpg", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        for response in [
            b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 0\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                .as_slice(),
            b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 0\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                .as_slice(),
            b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nPASS"
                .as_slice(),
        ] {
            let deadline = std::time::Instant::now() + Duration::from_secs(8);
            let mut socket = loop {
                match listener.accept() {
                    Ok((socket, _)) => break socket,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "429 retry request never arrived"
                        );
                        std::thread::sleep(Duration::from_millis(25));
                    }
                    Err(error) => panic!("rate-limited listener error: {error}"),
                }
            };
            socket.set_nonblocking(false).unwrap();
            let mut buffer = [0_u8; 1024];
            let count = socket.read(&mut buffer).unwrap();
            assert!(String::from_utf8_lossy(&buffer[..count]).contains("/limited.jpg"));
            socket.write_all(response).unwrap();
        }
    });
    let invoke = |retries: &str| {
        Command::new(env!("CARGO_BIN_EXE_galerith"))
            .arg("-d")
            .arg(&dir)
            .arg("--download-archive")
            .arg(&archive)
            .args(["--http-timeout", "3", "--retries", retries])
            .arg(&url)
            .output()
            .unwrap()
    };
    let exhausted = invoke("0");
    assert!(!exhausted.status.success());
    assert!(!dir.join("limited.jpg").exists());
    let empty: i64 = rusqlite::Connection::open(&archive)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM downloads", [], |row| row.get(0))
        .unwrap();
    assert_eq!(empty, 0, "exhausted 429 must not be archived");

    let recovered = invoke("1");
    assert!(
        recovered.status.success(),
        "{}",
        String::from_utf8_lossy(&recovered.stderr)
    );
    server.join().unwrap();
    assert_eq!(fs::read(dir.join("limited.jpg")).unwrap(), b"PASS");
    let count: i64 = rusqlite::Connection::open(&archive)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM downloads", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
    fs::remove_file(dir.join("limited.jpg")).unwrap();
    let skipped = invoke("0");
    assert!(skipped.status.success());
    assert!(String::from_utf8_lossy(&skipped.stdout).contains("archived limited.jpg"));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn failed_download_is_not_archived_and_can_be_retried_on_next_invocation() {
    let dir = temp("failed-retry");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let db = dir.join("archive.sqlite");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/retry.jpg", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        for response in [
            b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                .as_slice(),
            b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nPASS".as_slice(),
        ] {
            let (mut socket, _) = listener.accept().unwrap();
            let mut buffer = [0u8; 1024];
            assert!(socket.read(&mut buffer).unwrap() > 0);
            socket.write_all(response).unwrap();
        }
    });
    let invoke = || {
        Command::new(env!("CARGO_BIN_EXE_galerith"))
            .arg("-d")
            .arg(&dir)
            .arg("--download-archive")
            .arg(&db)
            .args(["--retries", "0", "--http-timeout", "2"])
            .arg(&url)
            .output()
            .unwrap()
    };

    let failed = invoke();
    assert!(!failed.status.success());
    assert!(!dir.join("retry.jpg").exists());
    let saved = invoke();
    assert!(
        saved.status.success(),
        "{}",
        String::from_utf8_lossy(&saved.stderr)
    );
    server.join().unwrap();
    assert_eq!(fs::read(dir.join("retry.jpg")).unwrap(), b"PASS");
    fs::remove_file(dir.join("retry.jpg")).unwrap();
    let archived = invoke();
    assert!(
        archived.status.success(),
        "{}",
        String::from_utf8_lossy(&archived.stderr)
    );
    assert!(String::from_utf8_lossy(&archived.stdout).contains("archived retry.jpg"));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn truncated_transfer_resumes_with_if_range_without_poisoning_archive() {
    let dir = temp("interrupted-resume");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let db = dir.join("archive.sqlite");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/partial.jpg", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let (mut first, _) = listener.accept().unwrap();
        let mut buffer = [0_u8; 2048];
        let request_len = first.read(&mut buffer).unwrap();
        assert!(String::from_utf8_lossy(&buffer[..request_len]).contains("/partial.jpg"));
        first
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nETag: \"resumable\"\r\nConnection: close\r\n\r\n1234",
            )
            .unwrap();
        drop(first);
        listener.set_nonblocking(true).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(7);
        let mut second = loop {
            match listener.accept() {
                Ok((socket, _)) => break socket,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "resumed transfer never made a range request"
                    );
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(error) => panic!("resume listener error: {error}"),
            }
        };
        second.set_nonblocking(false).unwrap();
        let request_len = second.read(&mut buffer).unwrap();
        let request = String::from_utf8_lossy(&buffer[..request_len]).to_ascii_lowercase();
        assert!(request.contains("range: bytes=4-"), "{request}");
        assert!(request.contains("if-range: \"resumable\""), "{request}");
        second
            .write_all(
                b"HTTP/1.1 206 Partial Content\r\nContent-Length: 4\r\nContent-Range: bytes 4-7/8\r\nETag: \"resumable\"\r\nConnection: close\r\n\r\n5678",
            )
            .unwrap();
    });
    let invoke = || {
        Command::new(env!("CARGO_BIN_EXE_galerith"))
            .arg("-d")
            .arg(&dir)
            .arg("--download-archive")
            .arg(&db)
            .args(["--retries", "0", "--http-timeout", "3"])
            .arg(&url)
            .output()
            .unwrap()
    };
    let failed = invoke();
    assert!(!failed.status.success());
    assert!(!dir.join("partial.jpg").exists());
    seed_resumable_partial(&dir, "partial.jpg", &url, r#""resumable""#);
    assert_eq!(fs::read(dir.join("partial.jpg.part")).unwrap(), b"1234");
    let first_count: i64 = rusqlite::Connection::open(&db)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM downloads", [], |row| row.get(0))
        .unwrap();
    assert_eq!(first_count, 0);

    let resumed = invoke();
    assert!(
        resumed.status.success(),
        "{}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    server.join().unwrap();
    assert_eq!(fs::read(dir.join("partial.jpg")).unwrap(), b"12345678");
    assert!(!dir.join("partial.jpg.part").exists());
    let recorded: i64 = rusqlite::Connection::open(&db)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM downloads", [], |row| row.get(0))
        .unwrap();
    assert_eq!(recorded, 1);
    fs::remove_file(dir.join("partial.jpg")).unwrap();
    let skipped = invoke();
    assert!(
        skipped.status.success(),
        "{}",
        String::from_utf8_lossy(&skipped.stderr)
    );
    assert!(String::from_utf8_lossy(&skipped.stdout).contains("archived partial.jpg"));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn invalid_resumed_content_range_never_archives_partial_and_recovers_cleanly() {
    let dir = temp("invalid-range-resume");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let db = dir.join("archive.sqlite");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/bad-range.jpg", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        let accept = || {
            let deadline = std::time::Instant::now() + Duration::from_secs(8);
            loop {
                match listener.accept() {
                    Ok((socket, _)) => {
                        socket.set_nonblocking(false).unwrap();
                        return socket;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "timed out waiting for recovery HTTP request"
                        );
                        std::thread::sleep(Duration::from_millis(25));
                    }
                    Err(error) => panic!("range recovery listener error: {error}"),
                }
            }
        };
        let mut first = accept();
        let mut buffer = [0_u8; 2048];
        let count = first.read(&mut buffer).unwrap();
        assert!(String::from_utf8_lossy(&buffer[..count]).contains("/bad-range.jpg"));
        first.write_all(
            b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nETag: \"safe\"\r\nConnection: close\r\n\r\n1234",
        ).unwrap();
        drop(first);

        let mut second = accept();
        let count = second.read(&mut buffer).unwrap();
        let request = String::from_utf8_lossy(&buffer[..count]).to_ascii_lowercase();
        assert!(request.contains("range: bytes=4-"), "{request}");
        assert!(request.contains("if-range: \"safe\""), "{request}");
        second.write_all(
            b"HTTP/1.1 206 Partial Content\r\nContent-Length: 5\r\nContent-Range: bytes 3-7/8\r\nETag: \"safe\"\r\nConnection: close\r\n\r\n45678",
        ).unwrap();
        drop(second);

        let mut third = accept();
        let count = third.read(&mut buffer).unwrap();
        let request = String::from_utf8_lossy(&buffer[..count]).to_ascii_lowercase();
        assert!(
            !request.contains("range:"),
            "invalid partial was reused: {request}"
        );
        third.write_all(
            b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nETag: \"new\"\r\nConnection: close\r\n\r\nABCDEFGH",
        ).unwrap();
    });
    let invoke = || {
        Command::new(env!("CARGO_BIN_EXE_galerith"))
            .arg("-d")
            .arg(&dir)
            .arg("--download-archive")
            .arg(&db)
            .args(["--retries", "0", "--http-timeout", "3"])
            .arg(&url)
            .output()
            .unwrap()
    };
    let first = invoke();
    assert!(!first.status.success());
    seed_resumable_partial(&dir, "bad-range.jpg", &url, r#""safe""#);
    assert_eq!(fs::read(dir.join("bad-range.jpg.part")).unwrap(), b"1234");
    let second = invoke();
    assert!(!second.status.success());
    assert!(!dir.join("bad-range.jpg.part").exists());
    assert!(!dir.join("bad-range.jpg.part.json").exists());
    assert!(!dir.join("bad-range.jpg").exists());
    let count: i64 = rusqlite::Connection::open(&db)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM downloads", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0, "invalid resume must not record an archive hit");

    let third = invoke();
    assert!(
        third.status.success(),
        "{}",
        String::from_utf8_lossy(&third.stderr)
    );
    server.join().unwrap();
    assert_eq!(fs::read(dir.join("bad-range.jpg")).unwrap(), b"ABCDEFGH");
    let recorded: i64 = rusqlite::Connection::open(&db)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM downloads", [], |row| row.get(0))
        .unwrap();
    assert_eq!(recorded, 1);
    fs::remove_file(dir.join("bad-range.jpg")).unwrap();
    let archived = invoke();
    assert!(archived.status.success());
    assert!(String::from_utf8_lossy(&archived.stdout).contains("archived bad-range.jpg"));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn zero_byte_existing_output_is_not_skipped_or_archived_as_success() {
    let dir = temp("zero-output-recovery");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let db = dir.join("archive.sqlite");
    let target = dir.join("recovery.jpg");
    fs::write(&target, b"").unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/recovery.jpg", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 2048];
        let count = stream.read(&mut request).unwrap();
        let request = String::from_utf8_lossy(&request[..count]).to_ascii_lowercase();
        assert!(request.contains("/recovery.jpg"), "{request}");
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nGOOD")
            .unwrap();
    });
    let invoke = || {
        Command::new(env!("CARGO_BIN_EXE_galerith"))
            .arg("-d")
            .arg(&dir)
            .arg("--download-archive")
            .arg(&db)
            .args([
                "--archive-event",
                "file,skip",
                "--retries",
                "0",
                "--http-timeout",
                "3",
            ])
            .arg(&url)
            .output()
            .unwrap()
    };
    let saved = invoke();
    assert!(
        saved.status.success(),
        "{}",
        String::from_utf8_lossy(&saved.stderr)
    );
    server.join().unwrap();
    assert_eq!(fs::read(&target).unwrap(), b"GOOD");
    let count: i64 = rusqlite::Connection::open(&db)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM downloads", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
    fs::remove_file(&target).unwrap();
    let archived = invoke();
    assert!(archived.status.success());
    assert!(String::from_utf8_lossy(&archived.stdout).contains("archived recovery.jpg"));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn unsolicited_http_206_does_not_create_a_durable_archive_hit() {
    let dir = temp("unsolicited-206-archive");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let db = dir.join("archive.sqlite");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/unsolicited.jpg", listener.local_addr().unwrap());
    fs::write(dir.join("unsolicited.jpg.part"), b"stale").unwrap();
    let server = std::thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        for step in 0..2 {
            let deadline = std::time::Instant::now() + Duration::from_secs(8);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "timed out waiting for unsolicited 206 fixture"
                        );
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    Err(err) => panic!("unexpected listener failure: {err}"),
                }
            };
            let mut buffer = [0_u8; 2048];
            let count = stream.read(&mut buffer).unwrap();
            let request = String::from_utf8_lossy(&buffer[..count]).to_ascii_lowercase();
            assert!(!request.contains("range:"), "{request}");
            if step == 0 {
                stream.write_all(
                    b"HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 1-3/4\r\nContent-Length: 3\r\nConnection: close\r\n\r\nOOD",
                ).unwrap();
            } else {
                stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nGOOD",
                    )
                    .unwrap();
            }
        }
    });
    let invoke = || {
        Command::new(env!("CARGO_BIN_EXE_galerith"))
            .arg("-d")
            .arg(&dir)
            .arg("--download-archive")
            .arg(&db)
            .args(["--retries", "0", "--http-timeout", "3"])
            .arg(&url)
            .output()
            .unwrap()
    };
    let rejected = invoke();
    assert!(!rejected.status.success());
    assert!(!dir.join("unsolicited.jpg").exists());
    assert!(!dir.join("unsolicited.jpg.part").exists());
    assert!(!dir.join("unsolicited.jpg.part.json").exists());
    let count: i64 = rusqlite::Connection::open(&db)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM downloads", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
    let complete = invoke();
    assert!(
        complete.status.success(),
        "{}",
        String::from_utf8_lossy(&complete.stderr)
    );
    server.join().unwrap();
    assert_eq!(fs::read(dir.join("unsolicited.jpg")).unwrap(), b"GOOD");
    let count: i64 = rusqlite::Connection::open(&db)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM downloads", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
    fs::remove_file(dir.join("unsolicited.jpg")).unwrap();
    let archived = invoke();
    assert!(archived.status.success());
    assert!(String::from_utf8_lossy(&archived.stdout).contains("archived unsolicited.jpg"));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn encoded_http_media_and_encoded_resumes_never_poison_archives() {
    let dir = temp("encoded-media");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let db = dir.join("archive.sqlite");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/encoded.jpg", listener.local_addr().unwrap());
    seed_resumable_partial(&dir, "encoded.jpg", &url, r#""stable""#);
    let server = std::thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        for step in 0..3 {
            let deadline = std::time::Instant::now() + Duration::from_secs(8);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "timed out waiting for encoded response request"
                        );
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    Err(err) => panic!("encoded-media listener failed: {err}"),
                }
            };
            let mut buffer = [0_u8; 2048];
            let count = stream.read(&mut buffer).unwrap();
            let request = String::from_utf8_lossy(&buffer[..count]).to_ascii_lowercase();
            assert!(request.contains("accept-encoding: identity"), "{request}");
            if step == 0 {
                assert!(request.contains("range: bytes=4-"), "{request}");
                assert!(request.contains("if-range: \"stable\""), "{request}");
                stream.write_all(
                    b"HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 4-7/8\r\nContent-Encoding: gzip\r\nContent-Length: 4\r\nConnection: close\r\n\r\nGZIP",
                ).unwrap();
            } else if step == 1 {
                assert!(!request.contains("range:"), "{request}");
                stream.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: 4\r\nConnection: close\r\n\r\nGZIP",
                ).unwrap();
            } else {
                assert!(!request.contains("range:"), "{request}");
                stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nGOOD",
                    )
                    .unwrap();
            }
        }
    });
    let invoke = || {
        Command::new(env!("CARGO_BIN_EXE_galerith"))
            .arg("-d")
            .arg(&dir)
            .arg("--download-archive")
            .arg(&db)
            .args(["--retries", "0", "--http-timeout", "3"])
            .arg(&url)
            .output()
            .unwrap()
    };
    for _ in 0..2 {
        let rejected = invoke();
        assert!(!rejected.status.success());
        assert!(!dir.join("encoded.jpg").exists());
        assert!(!dir.join("encoded.jpg.part").exists());
        assert!(!dir.join("encoded.jpg.part.json").exists());
        let count: i64 = rusqlite::Connection::open(&db)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM downloads", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }
    let recovered = invoke();
    assert!(
        recovered.status.success(),
        "{}",
        String::from_utf8_lossy(&recovered.stderr)
    );
    server.join().unwrap();
    assert_eq!(fs::read(dir.join("encoded.jpg")).unwrap(), b"GOOD");
    let count: i64 = rusqlite::Connection::open(&db)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM downloads", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
    fs::remove_file(dir.join("encoded.jpg")).unwrap();
    let archived = invoke();
    assert!(archived.status.success());
    assert!(String::from_utf8_lossy(&archived.stdout).contains("archived encoded.jpg"));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn empty_http_bodies_never_create_archive_hits_and_later_valid_media_recovers() {
    let dir = temp("empty-http-media");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let db = dir.join("archive.sqlite");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/empty.jpg", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        for reply in [
            b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n".as_slice(),
            b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".as_slice(),
            b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nETag: \"valid\"\r\nConnection: close\r\n\r\nGOOD".as_slice(),
        ] {
            let (mut conn, _) = listener.accept().unwrap();
            let mut buffer = [0_u8; 2048];
            let count = conn.read(&mut buffer).unwrap();
            let request = String::from_utf8_lossy(&buffer[..count]).to_ascii_lowercase();
            assert!(!request.contains("range:"), "{request}");
            conn.write_all(reply).unwrap();
        }
    });
    let invoke = || {
        Command::new(env!("CARGO_BIN_EXE_galerith"))
            .arg("-d")
            .arg(&dir)
            .arg("--download-archive")
            .arg(&db)
            .args(["--retries", "0", "--http-timeout", "3"])
            .arg(&url)
            .output()
            .unwrap()
    };
    for _ in 0..2 {
        let rejected = invoke();
        assert!(!rejected.status.success());
        assert!(!dir.join("empty.jpg").exists());
        assert!(!dir.join("empty.jpg.part").exists());
        assert!(!dir.join("empty.jpg.part.json").exists());
        let count: i64 = rusqlite::Connection::open(&db)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM downloads", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }
    let success = invoke();
    assert!(
        success.status.success(),
        "{}",
        String::from_utf8_lossy(&success.stderr)
    );
    server.join().unwrap();
    assert_eq!(fs::read(dir.join("empty.jpg")).unwrap(), b"GOOD");
    let count: i64 = rusqlite::Connection::open(&db)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM downloads", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
    fs::remove_file(dir.join("empty.jpg")).unwrap();
    let archived = invoke();
    assert!(archived.status.success());
    assert!(String::from_utf8_lossy(&archived.stdout).contains("archived empty.jpg"));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn unexpected_304_and_412_clear_partial_without_false_archive_entries() {
    for (label, status) in [
        ("not-modified", "304 Not Modified"),
        ("precondition", "412 Precondition Failed"),
    ] {
        let dir = temp(label);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let db = dir.join("archive.sqlite");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/resume.jpg", listener.local_addr().unwrap());
        seed_resumable_partial(&dir, "resume.jpg", &url, r#""stable""#);
        let response_status = status.to_owned();
        let server = std::thread::spawn(move || {
            listener.set_nonblocking(true).unwrap();
            for stage in 0..2 {
                let deadline = std::time::Instant::now() + Duration::from_secs(8);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(
                                std::time::Instant::now() < deadline,
                                "timeout waiting for rejected resume and fresh transfer"
                            );
                            std::thread::sleep(Duration::from_millis(20));
                        }
                        Err(err) => panic!("resume HTTP fixture failed: {err}"),
                    }
                };
                let mut request = [0_u8; 2048];
                let count = stream.read(&mut request).unwrap();
                let text = String::from_utf8_lossy(&request[..count]).to_ascii_lowercase();
                if stage == 0 {
                    assert!(text.contains("range: bytes=4-"), "{text}");
                    assert!(text.contains("if-range: \"stable\""), "{text}");
                    let answer = format!(
                        "HTTP/1.1 {response_status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    );
                    stream.write_all(answer.as_bytes()).unwrap();
                } else {
                    assert!(!text.contains("range:"), "{text}");
                    assert!(!text.contains("if-range:"), "{text}");
                    stream
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\nWXYZQRST",
                        )
                        .unwrap();
                }
            }
        });
        let invoke = || {
            Command::new(env!("CARGO_BIN_EXE_galerith"))
                .arg("-d")
                .arg(&dir)
                .arg("--download-archive")
                .arg(&db)
                .args(["--retries", "0", "--http-timeout", "3"])
                .arg(&url)
                .output()
                .unwrap()
        };
        let rejected = invoke();
        assert!(!rejected.status.success());
        assert!(!dir.join("resume.jpg").exists());
        assert!(!dir.join("resume.jpg.part").exists());
        assert!(!dir.join("resume.jpg.part.json").exists());
        let count = |db: &Path| -> i64 {
            rusqlite::Connection::open(db)
                .unwrap()
                .query_row("SELECT COUNT(*) FROM downloads", [], |row| row.get(0))
                .unwrap()
        };
        assert_eq!(count(&db), 0);
        let saved = invoke();
        assert!(
            saved.status.success(),
            "{}",
            String::from_utf8_lossy(&saved.stderr)
        );
        server.join().unwrap();
        assert_eq!(fs::read(dir.join("resume.jpg")).unwrap(), b"WXYZQRST");
        assert_eq!(count(&db), 1);
        fs::remove_file(dir.join("resume.jpg")).unwrap();
        let archived = invoke();
        assert!(archived.status.success());
        assert!(String::from_utf8_lossy(&archived.stdout).contains("archived resume.jpg"));
        fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn missing_resumed_content_range_discards_partial_and_never_archives_failed_attempt() {
    let dir = temp("missing-content-range");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let db = dir.join("archive.sqlite");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!(
        "http://{}/missing-range.jpg",
        listener.local_addr().unwrap()
    );
    seed_resumable_partial(&dir, "missing-range.jpg", &url, r#""original""#);
    let server = std::thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        let accept = || {
            let deadline = std::time::Instant::now() + Duration::from_secs(8);
            loop {
                match listener.accept() {
                    Ok((socket, _)) => {
                        socket.set_nonblocking(false).unwrap();
                        return socket;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "timed out waiting for missing-range recovery"
                        );
                        std::thread::sleep(Duration::from_millis(25));
                    }
                    Err(error) => panic!("range recovery listener error: {error}"),
                }
            }
        };
        let mut first = accept();
        let mut buffer = [0_u8; 2048];
        let count = first.read(&mut buffer).unwrap();
        let request = String::from_utf8_lossy(&buffer[..count]).to_ascii_lowercase();
        assert!(request.contains("range: bytes=4-"), "{request}");
        assert!(request.contains("if-range: \"original\""), "{request}");
        first.write_all(
            b"HTTP/1.1 206 Partial Content\r\nContent-Length: 4\r\nETag: \"original\"\r\nConnection: close\r\n\r\n5678",
        ).unwrap();
        drop(first);

        let mut second = accept();
        let count = second.read(&mut buffer).unwrap();
        let request = String::from_utf8_lossy(&buffer[..count]).to_ascii_lowercase();
        assert!(
            !request.contains("range:"),
            "stale Range was reused: {request}"
        );
        assert!(
            !request.contains("if-range:"),
            "stale If-Range was reused: {request}"
        );
        second.write_all(
            b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nETag: \"fresh\"\r\nConnection: close\r\n\r\nABCDEFGH",
        ).unwrap();
    });
    let invoke = || {
        Command::new(env!("CARGO_BIN_EXE_galerith"))
            .arg("-d")
            .arg(&dir)
            .arg("--download-archive")
            .arg(&db)
            .args(["--retries", "0", "--http-timeout", "3"])
            .arg(&url)
            .output()
            .unwrap()
    };
    let rejected = invoke();
    assert!(!rejected.status.success());
    assert!(!dir.join("missing-range.jpg").exists());
    assert!(!dir.join("missing-range.jpg.part").exists());
    assert!(!dir.join("missing-range.jpg.part.json").exists());
    let recorded: i64 = rusqlite::Connection::open(&db)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM downloads", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        recorded, 0,
        "missing Content-Range cannot record an archive hit"
    );

    let recovered = invoke();
    assert!(
        recovered.status.success(),
        "{}",
        String::from_utf8_lossy(&recovered.stderr)
    );
    server.join().unwrap();
    assert_eq!(
        fs::read(dir.join("missing-range.jpg")).unwrap(),
        b"ABCDEFGH"
    );
    let recorded: i64 = rusqlite::Connection::open(&db)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM downloads", [], |row| row.get(0))
        .unwrap();
    assert_eq!(recorded, 1);
    fs::remove_file(dir.join("missing-range.jpg")).unwrap();
    let skipped = invoke();
    assert!(skipped.status.success());
    assert!(String::from_utf8_lossy(&skipped.stdout).contains("archived missing-range.jpg"));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn changed_etag_resume_discards_partial_before_archival() {
    let dir = temp("etag-change-resume");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let db = dir.join("archive.sqlite");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/etag-change.jpg", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        let accept = || {
            let deadline = std::time::Instant::now() + Duration::from_secs(8);
            loop {
                match listener.accept() {
                    Ok((socket, _)) => {
                        socket.set_nonblocking(false).unwrap();
                        return socket;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "timed out waiting for recovery HTTP request"
                        );
                        std::thread::sleep(Duration::from_millis(25));
                    }
                    Err(error) => panic!("validator recovery listener error: {error}"),
                }
            }
        };
        let mut first = accept();
        let mut buffer = [0_u8; 2048];
        let count = first.read(&mut buffer).unwrap();
        assert!(String::from_utf8_lossy(&buffer[..count]).contains("/etag-change.jpg"));
        first.write_all(
            b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nETag: \"original\"\r\nConnection: close\r\n\r\n1234",
        ).unwrap();
        drop(first);

        let mut second = accept();
        let count = second.read(&mut buffer).unwrap();
        let request = String::from_utf8_lossy(&buffer[..count]).to_ascii_lowercase();
        assert!(request.contains("range: bytes=4-"), "{request}");
        assert!(request.contains("if-range: \"original\""), "{request}");
        second.write_all(
            b"HTTP/1.1 206 Partial Content\r\nContent-Length: 4\r\nContent-Range: bytes 4-7/8\r\nETag: \"changed\"\r\nConnection: close\r\n\r\n5678",
        ).unwrap();
        drop(second);

        let mut third = accept();
        let count = third.read(&mut buffer).unwrap();
        let request = String::from_utf8_lossy(&buffer[..count]).to_ascii_lowercase();
        assert!(
            !request.contains("range:"),
            "mismatched validator partial was reused: {request}"
        );
        third.write_all(
            b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nETag: \"changed\"\r\nConnection: close\r\n\r\nABCDEFGH",
        ).unwrap();
    });
    let invoke = || {
        Command::new(env!("CARGO_BIN_EXE_galerith"))
            .arg("-d")
            .arg(&dir)
            .arg("--download-archive")
            .arg(&db)
            .args(["--retries", "0", "--http-timeout", "3"])
            .arg(&url)
            .output()
            .unwrap()
    };
    let first = invoke();
    assert!(!first.status.success());
    seed_resumable_partial(&dir, "etag-change.jpg", &url, r#""original""#);
    assert_eq!(fs::read(dir.join("etag-change.jpg.part")).unwrap(), b"1234");
    let second = invoke();
    assert!(!second.status.success());
    assert!(!dir.join("etag-change.jpg.part").exists());
    assert!(!dir.join("etag-change.jpg.part.json").exists());
    assert!(!dir.join("etag-change.jpg").exists());
    let count: i64 = rusqlite::Connection::open(&db)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM downloads", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0, "conflicting ETag must not create an archive hit");

    let third = invoke();
    assert!(
        third.status.success(),
        "{}",
        String::from_utf8_lossy(&third.stderr)
    );
    server.join().unwrap();
    assert_eq!(fs::read(dir.join("etag-change.jpg")).unwrap(), b"ABCDEFGH");
    let recorded: i64 = rusqlite::Connection::open(&db)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM downloads", [], |row| row.get(0))
        .unwrap();
    assert_eq!(recorded, 1);
    fs::remove_file(dir.join("etag-change.jpg")).unwrap();
    let archived = invoke();
    assert!(archived.status.success());
    assert!(String::from_utf8_lossy(&archived.stdout).contains("archived etag-change.jpg"));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn rejected_range_416_discards_stale_partial_without_creating_archive_hit() {
    let dir = temp("range-416-recovery");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let db = dir.join("archive.sqlite");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/outdated.jpg", listener.local_addr().unwrap());
    seed_resumable_partial(&dir, "outdated.jpg", &url, r#""stale""#);
    let server = std::thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        let accept = || {
            let deadline = std::time::Instant::now() + Duration::from_secs(8);
            loop {
                match listener.accept() {
                    Ok((socket, _)) => {
                        socket.set_nonblocking(false).unwrap();
                        return socket;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "timed out waiting for 416 recovery HTTP request"
                        );
                        std::thread::sleep(Duration::from_millis(25));
                    }
                    Err(error) => panic!("416 recovery listener error: {error}"),
                }
            }
        };
        let mut first = accept();
        let mut buffer = [0u8; 2048];
        let count = first.read(&mut buffer).unwrap();
        let request = String::from_utf8_lossy(&buffer[..count]).to_ascii_lowercase();
        assert!(request.contains("/outdated.jpg"), "{request}");
        assert!(request.contains("range: bytes=4-"), "{request}");
        assert!(request.contains("if-range: \"stale\""), "{request}");
        first.write_all(
            b"HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */3\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        ).unwrap();
        drop(first);

        let mut second = accept();
        let count = second.read(&mut buffer).unwrap();
        let request = String::from_utf8_lossy(&buffer[..count]).to_ascii_lowercase();
        assert!(request.contains("/outdated.jpg"), "{request}");
        assert!(
            !request.contains("range:"),
            "stale range was reused: {request}"
        );
        assert!(
            !request.contains("if-range:"),
            "stale validator was reused: {request}"
        );
        second.write_all(
            b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nETag: \"fresh\"\r\nConnection: close\r\n\r\nABCDEFGH",
        ).unwrap();
    });
    let invoke = || {
        Command::new(env!("CARGO_BIN_EXE_galerith"))
            .arg("-d")
            .arg(&dir)
            .arg("--download-archive")
            .arg(&db)
            .args(["--retries", "0", "--http-timeout", "3"])
            .arg(&url)
            .output()
            .unwrap()
    };
    let rejected = invoke();
    assert!(!rejected.status.success());
    assert!(!dir.join("outdated.jpg").exists());
    assert!(!dir.join("outdated.jpg.part").exists());
    assert!(!dir.join("outdated.jpg.part.json").exists());
    let count: i64 = rusqlite::Connection::open(&db)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM downloads", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0, "HTTP 416 must not create a durable archive hit");

    let recovered = invoke();
    assert!(
        recovered.status.success(),
        "{}",
        String::from_utf8_lossy(&recovered.stderr)
    );
    server.join().unwrap();
    assert_eq!(fs::read(dir.join("outdated.jpg")).unwrap(), b"ABCDEFGH");
    let count: i64 = rusqlite::Connection::open(&db)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM downloads", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1, "only the completed download should be archived");
    fs::remove_file(dir.join("outdated.jpg")).unwrap();
    let offline = invoke();
    assert!(offline.status.success());
    assert!(String::from_utf8_lossy(&offline.stdout).contains("archived outdated.jpg"));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn process_crash_releases_archive_lock_and_next_run_can_download() {
    let dir = temp("crash-recovery");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let db = dir.join("archive.sqlite");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/recovery.jpg", listener.local_addr().unwrap());
    let (seen_tx, seen_rx) = std::sync::mpsc::channel();
    let (killed_tx, killed_rx) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut first, _) = listener.accept().unwrap();
        let mut buffer = [0u8; 1024];
        assert!(first.read(&mut buffer).unwrap() > 0);
        seen_tx.send(()).unwrap();
        killed_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        drop(first);
        listener.set_nonblocking(true).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(6);
        loop {
            match listener.accept() {
                Ok((mut retry, _)) => {
                    assert!(retry.read(&mut buffer).unwrap() > 0);
                    retry
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nPASS",
                        )
                        .unwrap();
                    return;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "recovery request timed out"
                    );
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(error) => panic!("recovery listener error: {error}"),
            }
        }
    });
    let launch = || {
        Command::new(env!("CARGO_BIN_EXE_galerith"))
            .arg("-d")
            .arg(&dir)
            .arg("--download-archive")
            .arg(&db)
            .args(["--retries", "0", "--http-timeout", "2"])
            .arg(&url)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    };
    let mut interrupted = launch();
    seen_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    interrupted.kill().unwrap();
    interrupted.wait().unwrap();
    killed_tx.send(()).unwrap();

    // A crashed process leaves the lock inode in place, but the kernel drops
    // its flock. Do not hang CI indefinitely if recovery regresses.
    let mut retry = launch();
    let deadline = std::time::Instant::now() + Duration::from_secs(7);
    let result = loop {
        if let Some(status) = retry.try_wait().unwrap() {
            let output = retry.wait_with_output().unwrap();
            assert_eq!(status, output.status);
            break output;
        }
        if std::time::Instant::now() >= deadline {
            retry.kill().unwrap();
            retry.wait().unwrap();
            panic!("a stale archive lock blocked retry after process crash");
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    server.join().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(fs::read(dir.join("recovery.jpg")).unwrap(), b"PASS");
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn cli_uses_external_gallery_dl_archive_without_modifying_source() {
    let dir = temp("external-reference");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let native = dir.join("native.sqlite");
    let external = dir.join("upstream.sqlite");
    let source = rusqlite::Connection::open(&external).unwrap();
    source
        .execute_batch("CREATE TABLE archive(entry TEXT PRIMARY KEY) WITHOUT ROWID;")
        .unwrap();
    source
        .execute(
            "INSERT INTO archive(entry) VALUES (?1)",
            ["directlinkexample.org/images/old.jpg"],
        )
        .unwrap();
    drop(source);
    let original_bytes = fs::read(&external).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .arg("--download-archive")
        .arg(&native)
        .arg("--gallery-dl-archive")
        .arg(&external)
        .arg("-d")
        .arg(&dir)
        .arg("https://example.org/images/old.jpg?signature=rotated")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("archived old.jpg"));
    assert_eq!(fs::read(&external).unwrap(), original_bytes);
    assert!(native.exists());
    assert!(!dir.join("old.jpg").exists());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn cli_rejects_missing_reference_archive_without_creating_native_db() {
    let dir = temp("missing-reference");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let native = dir.join("native.sqlite");
    let external = dir.join("missing.sqlite");
    let output = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .arg("--download-archive")
        .arg(&native)
        .arg("--gallery-dl-archive")
        .arg(&external)
        .arg("https://example.org/image.jpg")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!native.exists());
    assert!(!external.exists());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn offline_import_migrates_verified_ids_without_original_database() {
    let dir = temp("offline-import");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let native = dir.join("native.sqlite");
    let source = dir.join("reference.sqlite");
    let reference = rusqlite::Connection::open(&source).unwrap();
    reference
        .execute_batch("CREATE TABLE archive(entry TEXT PRIMARY KEY) WITHOUT ROWID;")
        .unwrap();
    for key in [
        "directlinkexample.org/photos/picture.jpg",
        "derpibooru987",
        "unsupported-site-data",
    ] {
        reference
            .execute("INSERT INTO archive(entry) VALUES (?1)", [key])
            .unwrap();
    }
    drop(reference);
    let original_bytes = fs::read(&source).unwrap();

    let import = || {
        Command::new(env!("CARGO_BIN_EXE_galerith"))
            .arg("--download-archive")
            .arg(&native)
            .arg("--gallery-dl-archive")
            .arg(&source)
            .arg("--archive-import")
            .output()
            .unwrap()
    };
    let first = import();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(String::from_utf8_lossy(&first.stdout).contains("2 supported entries; 2 new"));
    let second = import();
    assert!(second.status.success());
    assert!(String::from_utf8_lossy(&second.stdout).contains("2 supported entries; 0 new"));
    assert_eq!(fs::read(&source).unwrap(), original_bytes);
    fs::remove_file(source).unwrap();

    // No HTTP server runs at example.org; the migrated match must skip it.
    let skipped = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .arg("--download-archive")
        .arg(&native)
        .arg("-d")
        .arg(&dir)
        .arg("https://example.org/photos/picture.jpg?signature=new")
        .output()
        .unwrap();
    assert!(
        skipped.status.success(),
        "{}",
        String::from_utf8_lossy(&skipped.stderr)
    );
    assert!(String::from_utf8_lossy(&skipped.stdout).contains("archived picture.jpg"));
    assert!(!dir.join("picture.jpg").exists());

    let persisted = fs::read(&native).unwrap();
    for marker in ["picture.jpg", "example.org", "photos/"] {
        assert!(!persisted
            .windows(marker.len())
            .any(|b| b == marker.as_bytes()));
    }
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn offline_import_rejects_urls_and_missing_reference_before_creating_native() {
    let dir = temp("import-errors");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let native = dir.join("native.sqlite");
    let source = dir.join("missing.sqlite");
    let bad = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .arg("--download-archive")
        .arg(&native)
        .arg("--gallery-dl-archive")
        .arg(&source)
        .arg("--archive-import")
        .arg("https://example.org/asset.png")
        .output()
        .unwrap();
    assert!(!bad.status.success());
    assert!(!native.exists());
    let missing = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .arg("--download-archive")
        .arg(&native)
        .arg("--gallery-dl-archive")
        .arg(&source)
        .arg("--archive-import")
        .output()
        .unwrap();
    assert!(!missing.status.success());
    assert!(!native.exists());
    assert!(!source.exists());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn offline_import_respects_selected_custom_foreign_table() {
    let dir = temp("named-import");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let native = dir.join("native.sqlite");
    let foreign = dir.join("foreign.sqlite");
    let source = rusqlite::Connection::open(&foreign).unwrap();
    source
        .execute_batch(
            "CREATE TABLE \"stored items\" (entry TEXT PRIMARY KEY) WITHOUT ROWID;
             INSERT INTO \"stored items\" (entry) VALUES ('example.org//item.jpg');",
        )
        .unwrap();
    drop(source);
    let before = fs::read(&foreign).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .arg("--download-archive")
        .arg(&native)
        .arg("--gallery-dl-archive")
        .arg(&foreign)
        .arg("--gallery-dl-archive-table")
        .arg("stored items")
        .arg("--archive-import")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8_lossy(&result.stdout).contains("1 supported entries; 1 new"));
    assert_eq!(fs::read(&foreign).unwrap(), before);
    fs::remove_file(foreign).unwrap();
    let verify = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .arg("--download-archive")
        .arg(&native)
        .arg("https://example.org/item.jpg?signature=changed")
        .output()
        .unwrap();
    assert!(
        verify.status.success(),
        "{}",
        String::from_utf8_lossy(&verify.stderr)
    );
    assert!(String::from_utf8_lossy(&verify.stdout).contains("archived item.jpg"));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn named_table_does_not_import_ambiguous_philomena_rows() {
    let dir = temp("ambiguous-prefix");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let native = dir.join("native.sqlite");
    let source = dir.join("foreign.sqlite");
    let sqlite = rusqlite::Connection::open(&source).unwrap();
    sqlite
        .execute_batch(
            "CREATE TABLE other (entry TEXT PRIMARY KEY) WITHOUT ROWID;
             INSERT INTO other VALUES ('123');
             INSERT INTO other VALUES ('example.org//photo.jpg');",
        )
        .unwrap();
    drop(sqlite);
    let run = |prefix: Option<&str>| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_galerith"));
        cmd.arg("--download-archive")
            .arg(&native)
            .arg("--gallery-dl-archive")
            .arg(&source)
            .arg("--gallery-dl-archive-table")
            .arg("other");
        if let Some(prefix) = prefix {
            cmd.arg("--gallery-dl-archive-prefix").arg(prefix);
        }
        cmd.arg("--archive-import").output().unwrap()
    };
    let first = run(None);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(String::from_utf8_lossy(&first.stdout).contains("1 supported entries; 1 new"));
    let second = run(None);
    assert!(second.status.success());
    assert!(String::from_utf8_lossy(&second.stdout).contains("1 supported entries; 0 new"));
    let counted: i64 = rusqlite::Connection::open(&native)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM imported_gallery_dl", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(counted, 1);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn explicit_site_archive_prefix_migrates_unambiguous_philomena_ids() {
    let dir = temp("site-prefix");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let native = dir.join("native.sqlite");
    let foreign = dir.join("foreign.sqlite");
    let source = rusqlite::Connection::open(&foreign).unwrap();
    source
        .execute_batch(
            "CREATE TABLE other (entry TEXT PRIMARY KEY) WITHOUT ROWID;
             INSERT INTO other VALUES ('123');
             CREATE TABLE dedicated (entry TEXT PRIMARY KEY) WITHOUT ROWID;
             INSERT INTO dedicated VALUES ('derpibooru123');",
        )
        .unwrap();
    drop(source);
    let source_bytes = fs::read(&foreign).unwrap();

    let import = |table: &str, prefix: Option<&str>| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_galerith"));
        command
            .arg("--download-archive")
            .arg(&native)
            .arg("--gallery-dl-archive")
            .arg(&foreign)
            .arg("--gallery-dl-archive-table")
            .arg(table);
        if let Some(prefix) = prefix {
            command.arg("--gallery-dl-archive-prefix").arg(prefix);
        }
        command.arg("--archive-import").output().unwrap()
    };
    let ambiguous = import("other", None);
    assert!(
        ambiguous.status.success(),
        "{}",
        String::from_utf8_lossy(&ambiguous.stderr)
    );
    assert!(String::from_utf8_lossy(&ambiguous.stdout).contains("0 supported entries; 0 new"));

    let unambiguous = import("dedicated", Some("derpibooru"));
    assert!(
        unambiguous.status.success(),
        "{}",
        String::from_utf8_lossy(&unambiguous.stderr)
    );
    assert!(String::from_utf8_lossy(&unambiguous.stdout).contains("1 supported entries; 1 new"));
    assert_eq!(fs::read(&foreign).unwrap(), source_bytes);
    fs::remove_file(foreign).unwrap();

    let saved = rusqlite::Connection::open(&native).unwrap();
    let count: i64 = saved
        .query_row("SELECT COUNT(*) FROM imported_gallery_dl", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 1);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn simultaneous_imports_share_one_durable_set_of_archive_keys() {
    let dir = temp("simultaneous-import");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let native = dir.join("native.sqlite");
    let reference = dir.join("source.sqlite");
    let source = rusqlite::Connection::open(&reference).unwrap();
    source
        .execute_batch("CREATE TABLE archive(entry TEXT PRIMARY KEY) WITHOUT ROWID;")
        .unwrap();
    for id in 1..=80 {
        source
            .execute(
                "INSERT INTO archive(entry) VALUES (?1)",
                [format!("directlinkexample.org/images/{id}.jpg")],
            )
            .unwrap();
    }
    drop(source);
    let original_bytes = fs::read(&reference).unwrap();
    let start = || {
        Command::new(env!("CARGO_BIN_EXE_galerith"))
            .arg("--download-archive")
            .arg(&native)
            .arg("--gallery-dl-archive")
            .arg(&reference)
            .arg("--archive-import")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    };
    let first = start();
    let second = start();
    for proc in [first, second] {
        let result = proc.wait_with_output().unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(String::from_utf8_lossy(&result.stdout).contains("80 supported entries"));
    }
    let saved = rusqlite::Connection::open(&native).unwrap();
    let count: i64 = saved
        .query_row("SELECT COUNT(*) FROM imported_gallery_dl", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 80);
    drop(saved);
    assert_eq!(fs::read(&reference).unwrap(), original_bytes);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn archive_event_skip_persists_only_when_explicitly_selected() {
    let dir = temp("archive-event-skip");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let db = dir.join("native.sqlite");
    let file = dir.join("preexisting.jpg");
    fs::write(&file, b"existing").unwrap();
    let url = "https://example.org/preexisting.jpg";
    let execute = |event: Option<&str>| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_galerith"));
        cmd.arg("-d").arg(&dir).arg("--download-archive").arg(&db);
        if let Some(event) = event {
            cmd.arg("--archive-event").arg(event);
        }
        cmd.arg(url).output().unwrap()
    };

    let skipped = execute(Some("skip"));
    assert!(
        skipped.status.success(),
        "{}",
        String::from_utf8_lossy(&skipped.stderr)
    );
    assert!(String::from_utf8_lossy(&skipped.stdout).contains("skipped"));
    fs::remove_file(&file).unwrap();

    // Nothing is listening on example.org: a network attempt would fail.
    let archived = execute(None);
    assert!(
        archived.status.success(),
        "{}",
        String::from_utf8_lossy(&archived.stderr)
    );
    assert!(String::from_utf8_lossy(&archived.stdout).contains("archived preexisting.jpg"));

    let invalid = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .arg("--download-archive")
        .arg(&db)
        .arg("--archive-event")
        .arg("after")
        .arg(url)
        .output()
        .unwrap();
    assert!(!invalid.status.success());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn archive_event_file_preserves_default_and_skip_only_omits_new_downloads() {
    let dir = temp("archive-event-file");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let file = dir.join("present.png");
    fs::write(&file, b"preexisting").unwrap();
    let db = dir.join("saved.sqlite");
    let preexisting = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .arg("-d")
        .arg(&dir)
        .arg("--download-archive")
        .arg(&db)
        .arg("--archive-event")
        .arg("file")
        .arg("https://example.org/present.png")
        .output()
        .unwrap();
    assert!(preexisting.status.success());
    let saved: i64 = rusqlite::Connection::open(&db)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM downloads", [], |row| row.get(0))
        .unwrap();
    assert_eq!(saved, 0, "default file event cannot claim existing files");

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}/new.jpg", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut request = [0u8; 1024];
        assert!(socket.read(&mut request).unwrap() > 0);
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nDATA")
            .unwrap();
    });
    let only_skip = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .arg("-d")
        .arg(&dir)
        .arg("--download-archive")
        .arg(&db)
        .arg("--archive-event")
        .arg("skip")
        .arg("--retries")
        .arg("0")
        .arg(&endpoint)
        .output()
        .unwrap();
    assert!(
        only_skip.status.success(),
        "{}",
        String::from_utf8_lossy(&only_skip.stderr)
    );
    server.join().unwrap();
    assert_eq!(fs::read(dir.join("new.jpg")).unwrap(), b"DATA");
    let count: i64 = rusqlite::Connection::open(&db)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM downloads", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0, "skip-only mode must not archive a new download");
    fs::remove_dir_all(dir).unwrap();
}
