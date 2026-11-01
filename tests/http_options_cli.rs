//! Synthetic HTTP proxy and user-agent CLI regression tests.
#[path = "support/tls.rs"]
mod tls;

use std::{
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    process::Command,
};

fn destination(suffix: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "galerith-http-options-{suffix}-{}",
        std::process::id()
    ))
}

fn source_address_for_request(
    listener: TcpListener,
    options: &[&str],
    host: &str,
    suffix: &str,
) -> std::net::SocketAddr {
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).unwrap();
    let server = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let (mut socket, peer) = loop {
            match listener.accept() {
                Ok(connection) => break connection,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "HTTP fixture timed out"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                Err(error) => panic!("accept HTTP fixture connection: {error}"),
            }
        };
        socket
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut request = [0_u8; 1024];
        assert!(socket.read(&mut request).unwrap() > 0);
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: 4\r\nConnection: close\r\n\r\nJPEG",
            )
            .unwrap();
        peer
    });
    let dir = destination(suffix);
    let _ = fs::remove_dir_all(&dir);
    let url = format!("http://{host}:{port}/asset.jpg");
    let output = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args(options)
        .args(["--retries", "0", "-d", dir.to_str().unwrap(), &url])
        .output()
        .unwrap();
    let peer = server.join().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read(dir.join("asset.jpg")).unwrap(), b"JPEG");
    fs::remove_dir_all(dir).unwrap();
    peer
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
        "-4",
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
fn proxy_from_config_routes_http_downloads() {
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
        assert!(
            line.starts_with("GET http://example.invalid/asset.jpg HTTP/1.1"),
            "proxy config did not route an absolute-form request: {line}"
        );
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: 4\r\nConnection: close\r\n\r\nJPEG",
            )
            .unwrap();
    });
    let dir = destination("proxy-config");
    let _ = fs::remove_dir_all(&dir);
    let unused_proxy = TcpListener::bind("127.0.0.1:0").unwrap();
    let unused_proxy_port = unused_proxy.local_addr().unwrap().port();
    drop(unused_proxy);
    let global_proxy_option = format!(
        "proxy={}",
        serde_json::to_string(&format!("127.0.0.1:{unused_proxy_port}")).unwrap()
    );
    let media_proxy_option = format!(
        "downloader.http.proxy={}",
        serde_json::to_string(&serde_json::json!({
            "http": format!("127.0.0.1:{port}")
        }))
        .unwrap()
    );
    let mut command = Command::new(env!("CARGO_BIN_EXE_galerith"));
    command.args([
        "-o",
        &global_proxy_option,
        "-o",
        &media_proxy_option,
        "-d",
        dir.to_str().unwrap(),
        "http://example.invalid/asset.jpg",
    ]);
    for key in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ] {
        command.env_remove(key);
    }
    let output = command.output().unwrap();
    server.join().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read(dir.join("asset.jpg")).unwrap(), b"JPEG");
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
    let proxy_option = format!("proxy={}", serde_json::to_string(&proxy_address).unwrap());
    let output = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args([
            "-o",
            &proxy_option,
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
fn https_targets_use_a_proxy_connect_tunnel_before_tls() {
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
        assert!(
            line.starts_with("CONNECT example.invalid:443 HTTP/1.1\r\n"),
            "expected CONNECT tunnel request, got {line}"
        );
        assert!(!line.to_ascii_lowercase().contains("authorization:"));
        socket
            .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
            .unwrap();
        let count = socket.read(&mut request).unwrap();
        assert!(count >= 3, "client did not begin a TLS handshake");
        assert_eq!(
            request[0], 0x16,
            "first tunneled record was not TLS handshake"
        );
        assert_eq!(request[1], 0x03, "missing TLS record version");
    });
    let dir = destination("https-connect");
    let _ = fs::remove_dir_all(&dir);
    let proxy_address = format!("http://127.0.0.1:{port}");
    let proxy_option = format!(
        "proxy={}",
        serde_json::to_string(&serde_json::json!({ "https": proxy_address })).unwrap()
    );
    let output = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args([
            "-o",
            &proxy_option,
            "--retries",
            "0",
            "-d",
            dir.to_str().unwrap(),
            "https://example.invalid/asset.jpg",
        ])
        .output()
        .unwrap();
    server.join().unwrap();
    assert!(
        !output.status.success(),
        "the synthetic tunnel has no TLS origin"
    );
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains(&proxy_address),
        "proxy endpoint leaked into diagnostics"
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn tls_certificate_validation_is_enabled_by_default_and_can_be_disabled_explicitly() {
    let root = destination("tls-certificate");
    let _ = fs::remove_dir_all(&root);
    let server_root = root.join("server");
    let download_root = root.join("downloads");
    fs::create_dir_all(&server_root).unwrap();
    fs::create_dir_all(&download_root).unwrap();
    fs::write(server_root.join("asset.jpg"), b"TLS!").unwrap();
    let certificates = tls::create_ca_signed_server_certificate(&root.join("certificates"));

    let reservation = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = reservation.local_addr().unwrap();
    drop(reservation);
    let mut server = Command::new("openssl")
        .args([
            "s_server",
            "-quiet",
            "-WWW",
            "-naccept",
            "5",
            "-accept",
            &address.to_string(),
            "-cert",
            certificates.server.to_str().unwrap(),
            "-key",
            certificates.server_key.to_str().unwrap(),
        ])
        .current_dir(&server_root)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("openssl is required for the local TLS fixture");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if TcpStream::connect(address).is_ok() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "TLS fixture did not start"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    let url = format!("https://{address}/asset.jpg");
    let verified = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args(["--retries", "0", "-d", download_root.to_str().unwrap()])
        .arg(&url)
        .output()
        .unwrap();
    assert!(
        !verified.status.success(),
        "self-signed TLS unexpectedly passed verification"
    );

    let configured = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args([
            "-o",
            "downloader.http.verify=false",
            "--retries",
            "0",
            "-d",
            download_root.to_str().unwrap(),
        ])
        .arg(&url)
        .output()
        .unwrap();
    assert!(
        configured.status.success(),
        "{}",
        String::from_utf8_lossy(&configured.stderr)
    );
    assert_eq!(fs::read(download_root.join("asset.jpg")).unwrap(), b"TLS!");
    fs::remove_file(download_root.join("asset.jpg")).unwrap();

    let ca_option = format!(
        "downloader.http.verify={}",
        serde_json::to_string(certificates.ca.to_str().unwrap()).unwrap()
    );
    let trusted = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args([
            "-o",
            &ca_option,
            "--retries",
            "0",
            "-d",
            download_root.to_str().unwrap(),
        ])
        .arg(&url)
        .output()
        .unwrap();
    assert!(
        trusted.status.success(),
        "{}",
        String::from_utf8_lossy(&trusted.stderr)
    );
    assert_eq!(fs::read(download_root.join("asset.jpg")).unwrap(), b"TLS!");
    fs::remove_file(download_root.join("asset.jpg")).unwrap();

    let unverified = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args([
            "-o",
            "downloader.http.verify=true",
            "--no-check-certificate",
            "--retries",
            "0",
            "-d",
            download_root.to_str().unwrap(),
        ])
        .arg(&url)
        .output()
        .unwrap();
    let _ = server.kill();
    let _ = server.wait();
    assert!(
        unverified.status.success(),
        "{}",
        String::from_utf8_lossy(&unverified.stderr)
    );
    assert_eq!(fs::read(download_root.join("asset.jpg")).unwrap(), b"TLS!");
    fs::remove_dir_all(root).unwrap();
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
            "--source-address",
            "127.0.0.1",
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

    for args in [
        vec!["--force-ipv4", "--simulate", "https://example.org/item.jpg"],
        vec!["--force-ipv6", "--simulate", "https://example.org/item.jpg"],
        vec![
            "--source-address",
            "::1",
            "--simulate",
            "https://example.org/item.jpg",
        ],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_galerith"))
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn source_address_must_be_a_valid_ip_and_cannot_mix_with_family_flags() {
    let invalid = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args([
            "--source-address",
            "not-an-ip",
            "--simulate",
            "https://example.org/item.jpg",
        ])
        .output()
        .unwrap();
    assert!(!invalid.status.success());

    let conflict = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args([
            "--source-address",
            "127.0.0.1",
            "--force-ipv4",
            "--simulate",
            "https://example.org/item.jpg",
        ])
        .output()
        .unwrap();
    assert!(!conflict.status.success());
}

#[test]
fn force_ip_family_flags_bind_real_client_sockets() {
    let ipv4_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let ipv4_peer = source_address_for_request(
        ipv4_listener,
        &["--force-ipv4"],
        "localhost",
        "force-ipv4-family",
    );
    assert!(ipv4_peer.is_ipv4(), "IPv4 request used peer {ipv4_peer}");

    let ipv6_listener = match TcpListener::bind("[::1]:0") {
        Ok(listener) => listener,
        Err(error) if error.kind() == std::io::ErrorKind::AddrNotAvailable => return,
        Err(error) => panic!("bind IPv6 localhost fixture: {error}"),
    };
    let ipv6_peer = source_address_for_request(
        ipv6_listener,
        &["--force-ipv6"],
        "[::1]",
        "force-ipv6-family",
    );
    assert!(ipv6_peer.is_ipv6(), "IPv6 request used peer {ipv6_peer}");
}

#[test]
fn source_address_binds_the_actual_client_socket() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let peer = source_address_for_request(
        listener,
        &["--source-address", "127.0.0.2"],
        "127.0.0.1",
        "source-address-binding",
    );
    assert_eq!(peer.ip(), "127.0.0.2".parse::<std::net::IpAddr>().unwrap());
}

#[test]
fn fractional_http_timeout_expires_a_stalled_media_request() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut request = [0_u8; 1024];
        let _ = socket.read(&mut request);
        std::thread::sleep(std::time::Duration::from_millis(350));
        let _ = socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nSLOW");
    });
    let dir = destination("fractional-timeout");
    let _ = fs::remove_dir_all(&dir);
    let started = std::time::Instant::now();
    let output = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args(["--http-timeout", "0.1", "--retries", "0", "-d"])
        .arg(dir.to_str().unwrap())
        .arg(format!("http://{address}/asset.jpg"))
        .output()
        .unwrap();
    let elapsed = started.elapsed();
    server.join().unwrap();
    assert!(!output.status.success());
    assert!(
        elapsed < std::time::Duration::from_millis(300),
        "{elapsed:?}"
    );
    assert!(!dir.join("asset.jpg").exists());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn downloader_timeout_config_expires_media_request() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut request = [0_u8; 1024];
        let _ = socket.read(&mut request);
        std::thread::sleep(std::time::Duration::from_millis(350));
        let _ = socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nSLOW");
    });
    let dir = destination("downloader-config-timeout");
    let _ = fs::remove_dir_all(&dir);
    let started = std::time::Instant::now();
    let output = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args(["-o", "downloader.http.timeout=0.1", "--retries", "0", "-d"])
        .arg(dir.to_str().unwrap())
        .arg(format!("http://{address}/asset.jpg"))
        .output()
        .unwrap();
    let elapsed = started.elapsed();
    server.join().unwrap();
    assert!(!output.status.success());
    assert!(
        elapsed < std::time::Duration::from_millis(300),
        "downloader-scoped timeout was not applied: {elapsed:?}"
    );
    assert!(!dir.join("asset.jpg").exists());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn global_http_timeout_config_takes_precedence_over_downloader_timeout() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut request = [0_u8; 1024];
        let _ = socket.read(&mut request);
        std::thread::sleep(std::time::Duration::from_millis(350));
        let _ = socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nSLOW");
    });
    let dir = destination("global-timeout-precedence");
    let _ = fs::remove_dir_all(&dir);
    let output = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args([
            "-o",
            "downloader.http.timeout=0.1",
            "-o",
            "timeout=1",
            "--retries",
            "0",
            "-d",
        ])
        .arg(dir.to_str().unwrap())
        .arg(format!("http://{address}/asset.jpg"))
        .output()
        .unwrap();
    server.join().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read(dir.join("asset.jpg")).unwrap(), b"SLOW");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn invalid_http_timeouts_are_rejected() {
    for value in ["0", "-1", "NaN", "inf"] {
        let output = Command::new(env!("CARGO_BIN_EXE_galerith"))
            .arg(format!("--http-timeout={value}"))
            .args(["--simulate", "https://example.org/item.jpg"])
            .output()
            .unwrap();
        assert!(!output.status.success(), "unexpected success for {value}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("invalid --http-timeout"),
            "unexpected error for {value}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
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
