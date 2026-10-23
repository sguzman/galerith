//! Synthetic HTTP proxy and user-agent CLI regression tests.
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
    process::Command,
};

fn destination() -> PathBuf {
    std::env::temp_dir().join(format!("galerith-http-options-{}", std::process::id()))
}

#[test]
fn custom_user_agent_and_proxy_are_sent_to_synthetic_proxy_only() {
    let proxy = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = proxy.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = proxy.accept().unwrap();
        socket
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut request = [0_u8; 4096];
        let count = socket.read(&mut request).unwrap();
        let line = String::from_utf8_lossy(&request[..count]);
        let lower = line.to_ascii_lowercase();
        assert!(line.starts_with("GET http://example.invalid/asset.jpg HTTP/1.1"), "{line}");
        assert!(lower.contains("user-agent: synthetic-galerith-agent/42"), "{line}");
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nPROX")
            .unwrap();
    });
    let dir = destination();
    let _ = fs::remove_dir_all(&dir);
    let args = [
        "-a",
        "synthetic-galerith-agent/42",
        "--proxy",
        &format!("http://127.0.0.1:{port}"),
        "-d",
        dir.to_str().unwrap(),
        "http://example.invalid/asset.jpg",
    ];
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
    assert_eq!(fs::read(dir.join("asset.jpg")).unwrap(), b"PROX");
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn proxy_parse_error_does_not_print_password() {
    let output = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args([
            "--proxy",
            "http://[bad-private-passwd",
            "--simulate",
            "https://example.org/item.jpg",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid proxy configuration"));
    assert!(!stderr.contains("bad-private-passwd"));
}
