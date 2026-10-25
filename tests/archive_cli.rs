//! Verify archive behavior through the native executable without any live site.
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
    process::{Command, Stdio},
    time::Duration,
};

fn temp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "galerith-archive-cli-{name}-{}",
        std::process::id()
    ))
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
            b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nPASS"
                .as_slice(),
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
