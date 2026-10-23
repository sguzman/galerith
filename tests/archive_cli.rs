//! Verify archive behavior through the native executable without any live site.
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
    process::Command,
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
