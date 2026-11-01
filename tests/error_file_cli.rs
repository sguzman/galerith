//! `--error-file` persists failed inputs for a later retry.
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
    process::Command,
};

fn dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("galerith-error-file-{name}-{}", std::process::id()))
}

#[test]
fn appends_failed_input_urls_without_changing_normal_output() {
    let root = dir("failed");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let errors = root.join("errors.txt");
    let urls = [
        "https://example.org/unsupported?token=synthetic-one",
        "https://example.org/unsupported?token=synthetic-two",
    ];
    for url in urls {
        let output = Command::new(env!("CARGO_BIN_EXE_galerith"))
            .args(["--error-file", errors.to_str().unwrap(), url])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("synthetic-"));
    }
    assert_eq!(
        fs::read_to_string(&errors).unwrap(),
        format!("{}\n{}\n", urls[0], urls[1])
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn successful_input_is_not_added_to_the_error_file() {
    let root = dir("success");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let errors = root.join("errors.txt");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/picture.jpg", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut request = [0_u8; 1024];
        assert!(socket.read(&mut request).unwrap() > 0);
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: 4\r\nConnection: close\r\n\r\nJPEG",
            )
            .unwrap();
    });
    let output = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args(["-e", errors.to_str().unwrap(), "-d"])
        .arg(&root)
        .arg(&url)
        .output()
        .unwrap();
    server.join().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!errors.exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn appends_download_failures_only_after_retries_are_exhausted() {
    let root = dir("retry-failed");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let errors = root.join("errors.txt");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/picture.jpg", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        for _ in 0..2 {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0_u8; 1024];
            assert!(socket.read(&mut request).unwrap() > 0);
            socket
                .write_all(
                    b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
        }
    });
    let output = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args([
            "--error-file",
            errors.to_str().unwrap(),
            "--retries",
            "1",
            "-d",
        ])
        .arg(&root)
        .arg(&url)
        .output()
        .unwrap();
    server.join().unwrap();
    assert!(!output.status.success());
    assert_eq!(fs::read_to_string(&errors).unwrap(), format!("{url}\n"));
    fs::remove_dir_all(root).unwrap();
}
