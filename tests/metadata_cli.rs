//! Synthetic metadata sidecar end-to-end coverage, without third-party traffic.
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
    process::Command,
};

fn dir() -> PathBuf {
    std::env::temp_dir().join(format!("galerith-sidecar-cli-{}", std::process::id()))
}

#[test]
fn writes_json_after_download_and_for_existing_file_without_exposing_query_tokens() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut request = [0_u8; 2048];
        assert!(socket.read(&mut request).unwrap() > 0);
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nIMG")
            .unwrap();
    });
    let dir = dir();
    let _ = fs::remove_dir_all(&dir);
    let url = format!("http://{addr}/image.jpg?sig=top-secret");
    let args = ["--write-metadata", "-d", dir.to_str().unwrap(), &url];
    let output = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args(args)
        .output()
        .unwrap();
    server.join().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read(dir.join("image.jpg")).unwrap(), b"IMG");
    let sidecar = dir.join("image.jpg.json");
    let value: serde_json::Value = serde_json::from_slice(&fs::read(&sidecar).unwrap()).unwrap();
    assert_eq!(value["filename"], "image.jpg");
    assert_eq!(value["category"], "directlink");
    assert!(!value["source_url"].as_str().unwrap().contains("top-secret"));

    fs::remove_file(&sidecar).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(sidecar.exists());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn dry_run_with_metadata_flag_never_creates_sidecar() {
    let dir = dir().with_extension("dry");
    let _ = fs::remove_dir_all(&dir);
    let output = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args([
            "--write-metadata",
            "--simulate",
            "-d",
            dir.to_str().unwrap(),
            "https://example.org/preview.jpg",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(!dir.exists());
}
