//! Synthetic HTTP proxy and user-agent CLI regression tests.
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
    process::Command,
};

fn destination(suffix: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "galerith-http-options-{suffix}-{}",
        std::process::id()
    ))
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
        assert!(
            line.starts_with("GET http://example.invalid/asset.jpg HTTP/1.1"),
            "{line}"
        );
        assert!(
            lower.contains("user-agent: synthetic-galerith-agent/42"),
            "{line}"
        );
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nPROX")
            .unwrap();
    });
    let dir = destination("proxy");
    let _ = fs::remove_dir_all(&dir);
    let proxy_address = format!("http://127.0.0.1:{port}");
    let args = [
        "-a",
        "synthetic-galerith-agent/42",
        "--proxy",
        proxy_address.as_str(),
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
fn proxy_url_credentials_are_sent_only_to_the_proxy() {
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
        assert!(
            lower
                .lines()
                .any(|header| header.starts_with("proxy-authorization: basic dxnlcjpzzwnyzxq=")),
            "missing proxy authorization header: {line}"
        );
        assert!(!lower
            .lines()
            .any(|header| header.starts_with("authorization:")));
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nAUTH")
            .unwrap();
    });
    let dir = destination("proxy-auth");
    let _ = fs::remove_dir_all(&dir);
    let proxy_address = format!("http://user:secret@127.0.0.1:{port}");
    let output = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args([
            "--proxy",
            proxy_address.as_str(),
            "-d",
            dir.to_str().unwrap(),
            "http://example.invalid/authenticated.jpg",
        ])
        .output()
        .unwrap();
    server.join().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!String::from_utf8_lossy(&output.stderr).contains("secret"));
    assert_eq!(fs::read(dir.join("authenticated.jpg")).unwrap(), b"AUTH");
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

#[test]
fn configured_downloader_headers_reach_media_requests() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut request = [0_u8; 4096];
        let count = socket.read(&mut request).unwrap();
        let request = String::from_utf8_lossy(&request[..count]);
        assert!(
            request
                .to_ascii_lowercase()
                .contains("x-fixture-token: private-fixture-value"),
            "custom header was not sent: {request}"
        );
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nHEAD")
            .unwrap();
    });
    let dir = destination("configured-header");
    let _ = fs::remove_dir_all(&dir);
    let output = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args([
            "-4",
            "-o",
            r#"downloader.http.headers={"X-Fixture-Token":"private-fixture-value"}"#,
            "-d",
            dir.to_str().unwrap(),
        ])
        .arg(format!("http://{address}/asset.jpg"))
        .output()
        .unwrap();
    server.join().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read(dir.join("asset.jpg")).unwrap(), b"HEAD");
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn force_ip_families_are_mutually_exclusive() {
    let output = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args([
            "--force-ipv4",
            "--force-ipv6",
            "--simulate",
            "https://example.org/item.jpg",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
}

#[test]
fn invalid_configured_header_is_rejected_without_printing_its_value() {
    let output = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args([
            "-o",
            r#"downloader.http.headers={"X-Fixture":"private\r\nInjected: secret"}"#,
            "--simulate",
            "https://example.org/item.jpg",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid downloader HTTP header value"));
    assert!(!stderr.contains("private"));
    assert!(!stderr.contains("secret"));
}
