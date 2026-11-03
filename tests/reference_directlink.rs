//! Offline differential against the pinned upstream gallery-dl directlink
//! extractor. Upstream Python is executed ONLY by the isolated CI oracle job;
//! Galerith and its runtime remain Rust-native.
#[path = "support/tls.rs"]
mod tls;

use serde_json::{json, Value};
use std::{
    env,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

const PINNED: &str = "7f226f74108501c8ac3120548370f68f19e6f9b3";
const URLS: &[&str] = &[
    "https://example.org/photo.png",
    "https://assets.example.org/arts/Poster%20Art.JPEG?download=1#first",
    "https://cdn.example.org/one/two/cover%20art.webp?size=large",
    "http://images.example.org/a/track%20one.mp3?source=public",
    "https://static.example.org/media/image.avif",
    "https://downloads.example.org/books/book.pdf",
    "https://cdn.example.org/archive/media.7z?mirror=one",
    "https://videos.example.org/clips/preview.webm?quality=original#top",
    "https://audio.example.org/tracks/recording.opus",
    "https://scan.example.org/photos/art.heic",
    "https://assets.example.org/work/old.PSD",
    "https://media.example.org/print/photo.JPE",
    "https://assets.example.org/galleries/%E9%9F%B3%E6%A5%BD.jpg?lang=ja",
];

fn spawn(mut command: Command, label: &str) -> String {
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("{label}: process launch failed: {error}"));
    assert!(
        output.status.success(),
        "{label}: exit {:?}: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("reference and native output must be UTF-8")
}

fn native_events(url: &str) -> Vec<Value> {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_galerith"));
    cmd.args(["--dump-messages", url]);
    spawn(cmd, "native Galerith")
        .lines()
        .map(|line| serde_json::from_str(line).expect("native typed event JSON"))
        .collect()
}

fn native_upstream_tuples(url: &str) -> Vec<Value> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_galerith"));
    command.args(["--dump-json", url]);
    serde_json::from_str(&spawn(command, "native upstream-compatible JSON"))
        .expect("native upstream-compatible message tuple array")
}

fn pinned_json_output(root: &Path, url: &str, options: &[&str]) -> Value {
    let python = env::var_os("GALERITH_REFERENCE_PYTHON").unwrap_or_else(|| "python3".into());
    let mut command = Command::new(python);
    command.args(["-m", "gallery_dl", "--config-ignore", "--no-input"]);
    command.args(options);
    command.arg(url);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    serde_json::from_str(&spawn(command, "pinned gallery-dl JSON output"))
        .expect("pinned JSON message tuple array")
}

fn pinned_resolve_urls(root: &Path, url: &str) -> String {
    let python = env::var_os("GALERITH_REFERENCE_PYTHON").unwrap_or_else(|| "python3".into());
    let mut command = Command::new(python);
    command.args([
        "-m",
        "gallery_dl",
        "--config-ignore",
        "--no-input",
        "--resolve-urls",
        url,
    ]);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    spawn(command, "pinned gallery-dl --resolve-urls")
}

fn pinned_events(root: &Path, url: &str) -> Vec<Value> {
    let mut cmd = Command::new("python3");
    cmd.args([
        "-m",
        "gallery_dl",
        "--config-ignore",
        "--no-input",
        "--dump-json",
        url,
    ]);
    cmd.env("PYTHONPATH", root);
    cmd.env("PYTHONNOUSERSITE", "1");
    cmd.env("PYTHONDONTWRITEBYTECODE", "1");
    serde_json::from_str(&spawn(cmd, "pinned gallery-dl")).expect("upstream typed-message JSON")
}

fn serve_retry_sequence<const N: usize>(
    responses: [&'static [u8]; N],
) -> (String, std::thread::JoinHandle<()>) {
    serve_retry_sequence_at("/asset.jpg", responses)
}

fn serve_retry_sequence_at<const N: usize>(
    path: &'static str,
    responses: [&'static [u8]; N],
) -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}{}", listener.local_addr().unwrap(), path);
    let server = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        for response in responses {
            let (mut socket, _) = loop {
                match listener.accept() {
                    Ok(connection) => break connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "retry fixture timed out"
                        );
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    Err(error) => panic!("accept retry fixture connection: {error}"),
                }
            };
            socket
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut request = [0_u8; 2048];
            assert!(socket.read(&mut request).unwrap() > 0);
            socket.write_all(response).unwrap();
        }
    });
    (url, server)
}

fn serve_two_successes(response: &'static [u8]) -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}/asset.jpg", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        for _ in 0..2 {
            let (mut socket, _) = loop {
                match listener.accept() {
                    Ok(connection) => break connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "success fixture timed out"
                        );
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    Err(error) => panic!("accept success fixture connection: {error}"),
                }
            };
            socket
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut request = [0_u8; 1024];
            assert!(socket.read(&mut request).unwrap() > 0);
            socket.write_all(response).unwrap();
        }
    });
    (url, server)
}

fn contains_file_with_bytes(directory: &Path, expected: &[u8]) -> bool {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let path = entry.path();
        if path.is_dir() {
            contains_file_with_bytes(&path, expected)
        } else {
            std::fs::read(path).is_ok_and(|bytes| bytes == expected)
        }
    })
}

fn saved_paths_with_bytes(directory: &Path, expected: &[u8]) -> Vec<std::path::PathBuf> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    entries
        .flatten()
        .flat_map(|entry| {
            let path = entry.path();
            if path.is_dir() {
                saved_paths_with_bytes(&path, expected)
            } else if std::fs::read(&path).is_ok_and(|bytes| bytes == expected) {
                vec![path]
            } else {
                Vec::new()
            }
        })
        .collect()
}

fn saved_paths(directory: &Path) -> Vec<std::path::PathBuf> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    entries
        .flatten()
        .flat_map(|entry| {
            let path = entry.path();
            if path.is_dir() {
                saved_paths(&path)
            } else {
                vec![path]
            }
        })
        .collect()
}

fn run_reference_filename_filesystem_fixture(
    working_root: &Path,
    pinned_root: &Path,
    label: &str,
    reference: bool,
    template: &str,
) -> std::path::PathBuf {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut request = Vec::new();
        let mut buffer = [0_u8; 1024];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let count = socket.read(&mut buffer).unwrap();
            assert_ne!(count, 0, "client closed before sending HTTP headers");
            request.extend_from_slice(&buffer[..count]);
            assert!(request.len() < 16 * 1024, "HTTP headers too large");
        }
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: 4\r\nConnection: close\r\n\r\nJPEG",
            )
            .unwrap();
    });

    let output = working_root.join(format!("{label}-filename-output"));
    std::fs::create_dir_all(&output).unwrap();
    let url = format!("http://{address}/asset.jpg");
    let mut command = if reference {
        let mut command = Command::new("python3");
        command.args(["-m", "gallery_dl", "--config-ignore", "--no-input"]);
        command
    } else {
        Command::new(env!("CARGO_BIN_EXE_galerith"))
    };
    if reference {
        command.args(["--filename", template, "-D", output.to_str().unwrap(), &url]);
        command.env("PYTHONPATH", pinned_root);
        command.env("PYTHONNOUSERSITE", "1");
        command.env("PYTHONDONTWRITEBYTECODE", "1");
    } else {
        command.args(["--filename", template, "-d", output.to_str().unwrap(), &url]);
    }
    let result = command
        .output()
        .expect("launch filename filesystem fixture");
    assert!(
        result.status.success(),
        "{label}: client failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    server.join().unwrap();
    let paths = saved_paths_with_bytes(&output, b"JPEG");
    assert_eq!(
        paths.len(),
        1,
        "{label}: expected exactly one saved fixture"
    );
    let relative = paths[0].strip_prefix(&output).unwrap().to_owned();
    std::fs::remove_dir_all(output).unwrap();
    relative
}

fn run_user_agent_fixture(
    output_root: &Path,
    pinned_root: Option<&Path>,
    label: &str,
    reference: bool,
    proxy_scope: &str,
    headers_scope: &str,
) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut request = Vec::new();
        let mut buffer = [0_u8; 1024];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let count = socket.read(&mut buffer).unwrap();
            assert_ne!(count, 0, "client closed before sending HTTP headers");
            request.extend_from_slice(&buffer[..count]);
            assert!(request.len() < 16 * 1024, "HTTP request headers too large");
        }
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: 4\r\nConnection: close\r\n\r\nJPEG",
            )
            .unwrap();
        String::from_utf8_lossy(&request).into_owned()
    });

    let output = output_root.join(format!("{label}-output"));
    std::fs::create_dir_all(&output).unwrap();
    let url = "http://example.invalid/asset.jpg";
    let proxy = format!("http://user:secret@{address}");
    let proxy_option = format!("{proxy_scope}={}", serde_json::to_string(&proxy).unwrap());
    let headers_option = format!(
        "{headers_scope}={}",
        serde_json::to_string(&serde_json::json!({"X-Preview": "header"})).unwrap()
    );
    let user_agent = "galerith-reference-probe/7";
    let extractor_user_agent_option = "extractor.directlink.user-agent=extractor-reference-agent/9";
    let mut command = if reference {
        let mut command = Command::new("python3");
        command.args([
            "-m",
            "gallery_dl",
            "--config-ignore",
            "--no-input",
            "--quiet",
            "-a",
            user_agent,
            "-o",
            extractor_user_agent_option,
            "-o",
            &headers_option,
            "-o",
            &proxy_option,
            "-R",
            "0",
            "-d",
            output.to_str().unwrap(),
            url,
        ]);
        command.env(
            "PYTHONPATH",
            pinned_root.expect("pinned source root required"),
        );
        command.env("PYTHONNOUSERSITE", "1");
        command.env("PYTHONDONTWRITEBYTECODE", "1");
        command
    } else {
        let mut command = Command::new(env!("CARGO_BIN_EXE_galerith"));
        command.args([
            "--quiet",
            "-a",
            user_agent,
            "-o",
            extractor_user_agent_option,
            "-o",
            &headers_option,
            "-o",
            &proxy_option,
            "--retries",
            "0",
            "-d",
            output.to_str().unwrap(),
            url,
        ]);
        command
    };
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
    let result = command.output().expect("launch network fixture client");
    assert!(
        result.status.success(),
        "{label}: client failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let request = server.join().unwrap();
    std::fs::remove_dir_all(output).unwrap();
    request
}

fn run_extractor_source_address_fixture(
    output_root: &Path,
    pinned_root: Option<&Path>,
    label: &str,
    reference: bool,
) -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut socket, peer) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut request = [0; 2048];
        assert!(socket.read(&mut request).unwrap() > 0);
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: 4\r\nConnection: close\r\n\r\nJPEG",
            )
            .unwrap();
        peer
    });
    let output = output_root.join(format!("{label}-output"));
    std::fs::create_dir_all(&output).unwrap();
    let url = format!("http://{address}/asset.jpg");
    let source_address = "extractor.directlink.source-address=127.0.0.2";
    let mut command = if reference {
        let mut command = Command::new("python3");
        command.args([
            "-m",
            "gallery_dl",
            "--config-ignore",
            "--no-input",
            "--quiet",
            "-o",
            source_address,
            "-R",
            "0",
            "-d",
            output.to_str().unwrap(),
            &url,
        ]);
        command.env(
            "PYTHONPATH",
            pinned_root.expect("pinned source root required"),
        );
        command.env("PYTHONNOUSERSITE", "1");
        command.env("PYTHONDONTWRITEBYTECODE", "1");
        command
    } else {
        let mut command = Command::new(env!("CARGO_BIN_EXE_galerith"));
        command.args([
            "--quiet",
            "-o",
            source_address,
            "--retries",
            "0",
            "-d",
            output.to_str().unwrap(),
            &url,
        ]);
        command
    };
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
    let result = command.output().unwrap();
    assert!(
        result.status.success(),
        "{label}: client failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        contains_file_with_bytes(&output, b"JPEG"),
        "{label}: client did not save the fixture body"
    );
    let peer = server.join().unwrap();
    std::fs::remove_dir_all(output).unwrap();
    peer
}

fn run_extractor_user_agent_fixture(
    output_root: &Path,
    pinned_root: Option<&Path>,
    label: &str,
    reference: bool,
) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut request = Vec::new();
        let mut buffer = [0; 1024];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let count = socket.read(&mut buffer).unwrap();
            assert_ne!(count, 0, "client closed before sending HTTP headers");
            request.extend_from_slice(&buffer[..count]);
        }
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: 4\r\nConnection: close\r\n\r\nJPEG",
            )
            .unwrap();
        String::from_utf8_lossy(&request).into_owned()
    });
    let output = output_root.join(format!("{label}-output"));
    std::fs::create_dir_all(&output).unwrap();
    let url = format!("http://{address}/asset.jpg");
    let user_agent_option = "extractor.directlink.user-agent=extractor-reference-agent/9";
    let mut command = if reference {
        let mut command = Command::new("python3");
        command.args([
            "-m",
            "gallery_dl",
            "--config-ignore",
            "--no-input",
            "--quiet",
            "-o",
            user_agent_option,
            "-R",
            "0",
            "-d",
            output.to_str().unwrap(),
            &url,
        ]);
        command.env(
            "PYTHONPATH",
            pinned_root.expect("pinned source root required"),
        );
        command.env("PYTHONNOUSERSITE", "1");
        command.env("PYTHONDONTWRITEBYTECODE", "1");
        command
    } else {
        let mut command = Command::new(env!("CARGO_BIN_EXE_galerith"));
        command.args([
            "--quiet",
            "-o",
            user_agent_option,
            "--retries",
            "0",
            "-d",
            output.to_str().unwrap(),
            &url,
        ]);
        command
    };
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
    let result = command.output().unwrap();
    assert!(
        result.status.success(),
        "{label}: client failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let request = server.join().unwrap();
    assert!(
        contains_file_with_bytes(&output, b"JPEG"),
        "{label}: client did not save the fixture body"
    );
    std::fs::remove_dir_all(output).unwrap();
    request
}

fn run_429_retry_delay_fixture(
    working_root: &Path,
    pinned_root: &Path,
    label: &str,
    reference: bool,
) -> std::time::Duration {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let accept = || loop {
            match listener.accept() {
                Ok((socket, _)) => break socket,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "429 fixture timed out"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                Err(error) => panic!("accept 429 fixture request: {error}"),
            }
        };
        let mut first = accept();
        first
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut request = [0_u8; 2048];
        assert!(first.read(&mut request).unwrap() > 0);
        first
            .write_all(
                b"HTTP/1.1 429 Too Many Requests\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
        drop(first);

        let started = std::time::Instant::now();
        let mut second = accept();
        let elapsed = started.elapsed();
        second
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        assert!(second.read(&mut request).unwrap() > 0);
        second
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: 4\r\nConnection: close\r\n\r\nJPEG",
            )
            .unwrap();
        elapsed
    });

    let output = working_root.join(format!("{label}-429-output"));
    std::fs::create_dir_all(&output).unwrap();
    let url = format!("http://{address}/retry.jpg");
    let mut command = if reference {
        let mut command = Command::new("python3");
        command.args([
            "-m",
            "gallery_dl",
            "--config-ignore",
            "--no-input",
            "--quiet",
            "--sleep-429",
            "0.05",
            "-o",
            "downloader.http.retry-codes=[429]",
            "-R",
            "1",
            "-d",
            output.to_str().unwrap(),
            &url,
        ]);
        command.env("PYTHONPATH", pinned_root);
        command.env("PYTHONNOUSERSITE", "1");
        command.env("PYTHONDONTWRITEBYTECODE", "1");
        command
    } else {
        let mut command = Command::new(env!("CARGO_BIN_EXE_galerith"));
        command.args([
            "--quiet",
            "--sleep-429",
            "0.05",
            "-o",
            "downloader.http.retry-codes=[429]",
            "--retries",
            "1",
            "-d",
            output.to_str().unwrap(),
            &url,
        ]);
        command
    };
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
    let result = command.output().expect("launch 429 retry fixture client");
    assert!(
        result.status.success(),
        "{label}: client failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let elapsed = server.join().unwrap();
    if reference {
        assert!(
            contains_file_with_bytes(&output, b"JPEG"),
            "{label}: pinned CLI did not save the fixture body"
        );
    } else {
        assert_eq!(std::fs::read(output.join("retry.jpg")).unwrap(), b"JPEG");
    }
    std::fs::remove_dir_all(output).unwrap();
    elapsed
}

fn run_http_timeout_fixture(
    working_root: &Path,
    pinned_root: &Path,
    label: &str,
    reference: bool,
) -> std::time::Duration {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().expect("accept timed media request");
        socket
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        let mut request = [0_u8; 2048];
        assert!(socket.read(&mut request).unwrap() > 0);
        let started = std::time::Instant::now();
        let mut probe = [0_u8; 1];
        let disconnected = match socket.read(&mut probe) {
            Ok(0) => true,
            Ok(_) => false,
            Err(error) => matches!(
                error.kind(),
                std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::UnexpectedEof
                    | std::io::ErrorKind::NotConnected
            ),
        };
        let elapsed = started.elapsed();
        if !disconnected {
            let _ = socket.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: 4\r\nConnection: close\r\n\r\nJPEG",
            );
        }
        (elapsed, disconnected)
    });

    let output = working_root.join(format!("{label}-timeout-output"));
    std::fs::create_dir_all(&output).unwrap();
    let url = format!("http://{address}/timeout.jpg");
    let mut command = if reference {
        let mut command = Command::new("python3");
        command.args([
            "-m",
            "gallery_dl",
            "--config-ignore",
            "--no-input",
            "--quiet",
            "-o",
            "downloader.http.timeout=0.1",
            "-R",
            "0",
            "-d",
            output.to_str().unwrap(),
            &url,
        ]);
        command.env("PYTHONPATH", pinned_root);
        command.env("PYTHONNOUSERSITE", "1");
        command.env("PYTHONDONTWRITEBYTECODE", "1");
        command
    } else {
        let mut command = Command::new(env!("CARGO_BIN_EXE_galerith"));
        command.args([
            "--quiet",
            "-o",
            "downloader.http.timeout=0.1",
            "--retries",
            "0",
            "-d",
            output.to_str().unwrap(),
            &url,
        ]);
        command
    };
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
    let result = command.output().expect("launch timeout fixture client");
    let (elapsed, disconnected) = server.join().unwrap();
    assert!(
        !result.status.success(),
        "{label}: media timeout was not enforced: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        disconnected,
        "{label}: client kept the timed-out media connection open"
    );
    assert!(
        !output.join("timeout.jpg").exists(),
        "{label}: timed out media was published"
    );
    std::fs::remove_dir_all(output).unwrap();
    elapsed
}

fn run_reference_sleep_interval_fixture(
    working_root: &Path,
    pinned_root: &Path,
    label: &str,
    reference: bool,
    option: &str,
    skip_existing: bool,
) -> std::time::Duration {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let mut previous = None;
        let mut interval = None;
        for expected in ["/first.jpg", "/last.jpg"] {
            let (mut socket, _) = listener.accept().unwrap();
            let arrived = std::time::Instant::now();
            socket
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let count = socket.read(&mut buffer).unwrap();
                assert_ne!(count, 0, "client closed before sending HTTP headers");
                request.extend_from_slice(&buffer[..count]);
                assert!(request.len() < 16 * 1024, "HTTP headers too large");
            }
            assert!(
                String::from_utf8_lossy(&request).starts_with(&format!("GET {expected} HTTP/1.1")),
                "expected {expected}, got {}",
                String::from_utf8_lossy(&request)
            );
            if let Some(previous) = previous {
                interval = Some(arrived.duration_since(previous));
            }
            previous = Some(arrived);
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: 4\r\nConnection: close\r\n\r\nJPEG",
                )
                .unwrap();
        }
        interval.unwrap()
    });

    let output = working_root.join(format!("{label}-sleep-output"));
    std::fs::create_dir_all(&output).unwrap();
    let first = format!("http://{address}/first.jpg");
    let skipped = format!("http://{address}/skip.jpg");
    let last = format!("http://{address}/last.jpg");
    let existing_path = if reference {
        output.join(format!("{address}__skip.jpg"))
    } else {
        output.join("skip.jpg")
    };
    if skip_existing {
        std::fs::write(&existing_path, b"existing").unwrap();
    }
    let mut inputs = vec![first.clone()];
    if skip_existing {
        inputs.push(skipped.clone());
        inputs.push(skipped);
    }
    inputs.push(last);
    let mut command = if reference {
        let mut command = Command::new("python3");
        command.args([
            "-m",
            "gallery_dl",
            "--config-ignore",
            "--no-input",
            "--quiet",
        ]);
        command
    } else {
        let mut command = Command::new(env!("CARGO_BIN_EXE_galerith"));
        command.args(["--quiet"]);
        command
    };
    command.args([option, "0.12-0.18"]);
    if reference {
        command.args(["-D", output.to_str().unwrap()]);
    } else {
        command.args(["-d", output.to_str().unwrap()]);
    }
    command.args(&inputs);
    if reference {
        command.env("PYTHONPATH", pinned_root);
        command.env("PYTHONNOUSERSITE", "1");
        command.env("PYTHONDONTWRITEBYTECODE", "1");
    }
    let result = command.output().expect("launch reference sleep fixture");
    assert!(
        result.status.success(),
        "{label}: client failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    if skip_existing {
        assert_eq!(std::fs::read(existing_path).unwrap(), b"existing");
    }
    assert!(contains_file_with_bytes(&output, b"JPEG"));
    let interval = server.join().unwrap();
    std::fs::remove_dir_all(output).unwrap();
    interval
}

fn run_reference_media_sleep_request_case(
    working_root: &Path,
    pinned_root: &Path,
    label: &str,
    reference: bool,
) -> std::time::Duration {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let mut previous = None;
        let mut interval = None;
        for _ in 0..2 {
            let (mut socket, _) = listener.accept().unwrap();
            let arrived = std::time::Instant::now();
            socket
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut request = [0_u8; 2048];
            assert!(socket.read(&mut request).unwrap() > 0);
            if let Some(previous) = previous {
                interval = Some(arrived.duration_since(previous));
            }
            previous = Some(arrived);
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: 4\r\nConnection: close\r\n\r\nJPEG",
                )
                .unwrap();
        }
        interval.unwrap()
    });

    let output = working_root.join(format!("{label}-media-sleep-output"));
    std::fs::create_dir_all(&output).unwrap();
    let first = format!("http://{address}/first.jpg");
    let second = format!("http://{address}/second.jpg");
    let mut command = if reference {
        let mut command = Command::new("python3");
        command.args([
            "-m",
            "gallery_dl",
            "--config-ignore",
            "--no-input",
            "--quiet",
            "--sleep-request",
            "0.12-0.18",
            "-d",
            output.to_str().unwrap(),
            &first,
            &second,
        ]);
        command.env("PYTHONPATH", pinned_root);
        command.env("PYTHONNOUSERSITE", "1");
        command.env("PYTHONDONTWRITEBYTECODE", "1");
        command
    } else {
        let mut command = Command::new(env!("CARGO_BIN_EXE_galerith"));
        command.args([
            "--quiet",
            "--sleep-request",
            "0.12-0.18",
            "--retries",
            "0",
            "-d",
            output.to_str().unwrap(),
            &first,
            &second,
        ]);
        command
    };
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
    let result = command
        .output()
        .expect("launch media sleep-request fixture");
    assert!(
        result.status.success(),
        "{label}: client failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(contains_file_with_bytes(&output, b"JPEG"));
    let interval = server.join().unwrap();
    std::fs::remove_dir_all(output).unwrap();
    interval
}

fn run_429_default_fixture(working_root: &Path, pinned_root: &Path, label: &str, reference: bool) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let accept = |timeout: std::time::Duration| {
            let deadline = std::time::Instant::now() + timeout;
            loop {
                match listener.accept() {
                    Ok((socket, _)) => return Some(socket),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        if std::time::Instant::now() >= deadline {
                            return None;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    Err(error) => panic!("accept default 429 fixture request: {error}"),
                }
            }
        };
        let mut first =
            accept(std::time::Duration::from_secs(5)).expect("first default 429 request");
        first
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut request = [0_u8; 2048];
        assert!(first.read(&mut request).unwrap() > 0);
        first
            .write_all(
                b"HTTP/1.1 429 Too Many Requests\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
        drop(first);

        if let Some(mut second) = accept(std::time::Duration::from_millis(1800)) {
            second
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            assert!(second.read(&mut request).unwrap() > 0);
            second
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: 4\r\nConnection: close\r\n\r\nJPEG",
                )
                .unwrap();
            true
        } else {
            false
        }
    });

    let output = working_root.join(format!("{label}-429-default-output"));
    std::fs::create_dir_all(&output).unwrap();
    let url = format!("http://{address}/retry.jpg");
    let mut command = if reference {
        let mut command = Command::new("python3");
        command.args([
            "-m",
            "gallery_dl",
            "--config-ignore",
            "--no-input",
            "--quiet",
            "--sleep-429",
            "0",
            "-R",
            "1",
            "-d",
            output.to_str().unwrap(),
            &url,
        ]);
        command.env("PYTHONPATH", pinned_root);
        command.env("PYTHONNOUSERSITE", "1");
        command.env("PYTHONDONTWRITEBYTECODE", "1");
        command
    } else {
        let mut command = Command::new(env!("CARGO_BIN_EXE_galerith"));
        command.args([
            "--quiet",
            "--sleep-429",
            "0",
            "--retries",
            "1",
            "-d",
            output.to_str().unwrap(),
            &url,
        ]);
        command
    };
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
    let result = command.output().expect("launch default 429 fixture client");
    assert!(
        !result.status.success(),
        "{label}: client unexpectedly retried an unconfigured 429: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        !server.join().unwrap(),
        "{label}: client sent another request after an unconfigured 429"
    );
    std::fs::remove_dir_all(output).unwrap();
}

fn run_input_retry_case(
    root: &Path,
    pinned_root: &Path,
    label: &str,
    responses: [&'static [u8]; 4],
    expected_success: bool,
) {
    let (url, server) = serve_retry_sequence(responses);
    let reference_input = root.join(format!("{label}-reference.txt"));
    let native_input = root.join(format!("{label}-native.txt"));
    let reference_output = root.join(format!("{label}-reference-output"));
    let native_output = root.join(format!("{label}-native-output"));
    std::fs::create_dir_all(&reference_output).unwrap();
    std::fs::create_dir_all(&native_output).unwrap();
    std::fs::write(&reference_input, format!("{url}\n")).unwrap();
    std::fs::write(&native_input, format!("{url}\n")).unwrap();

    let mut reference = Command::new("python3");
    reference.args([
        "-m",
        "gallery_dl",
        "--config-ignore",
        "--no-input",
        "--quiet",
        "--sleep-retries",
        "0",
        "-R",
        "1",
        "-I",
        reference_input.to_str().unwrap(),
        "-d",
        reference_output.to_str().unwrap(),
    ]);
    reference.env("PYTHONPATH", pinned_root);
    reference.env("PYTHONNOUSERSITE", "1");
    reference.env("PYTHONDONTWRITEBYTECODE", "1");
    for key in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ] {
        reference.env_remove(key);
    }
    let reference_result = reference.output().expect("launch pinned CLI");
    let mut native = Command::new(env!("CARGO_BIN_EXE_galerith"));
    native.args([
        "--quiet",
        "--sleep-retries",
        "0",
        "--retries",
        "1",
        "--input-file-comment",
        native_input.to_str().unwrap(),
        "-d",
        native_output.to_str().unwrap(),
    ]);
    for key in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ] {
        native.env_remove(key);
    }
    let native_result = native.output().unwrap();
    server.join().unwrap();

    assert_eq!(
        reference_result.status.success(),
        expected_success,
        "{label}: pinned exit status"
    );
    assert_eq!(
        native_result.status.success(),
        expected_success,
        "{label}: native exit status: {}",
        String::from_utf8_lossy(&native_result.stderr)
    );
    let expected_input = if expected_success {
        format!("# {url}\n")
    } else {
        format!("{url}\n")
    };
    assert_eq!(
        std::fs::read_to_string(&reference_input).unwrap(),
        expected_input,
        "{label}: pinned input-file mutation"
    );
    assert_eq!(
        std::fs::read_to_string(&native_input).unwrap(),
        expected_input,
        "{label}: native input-file mutation"
    );
    if expected_success {
        assert!(
            contains_file_with_bytes(&reference_output, b"JPEG"),
            "{label}: pinned CLI did not save the fixture body"
        );
        assert_eq!(
            std::fs::read(native_output.join("asset.jpg")).unwrap(),
            b"JPEG"
        );
    }
}

#[test]
fn pinned_cli_and_native_match_stdin_url_list_parsing() {
    let input = b"\n# comment\nhttps://example.org/first.jpg?size=large\nhttps://cdn.example.org/second.webp\n";

    let mut native = Command::new(env!("CARGO_BIN_EXE_galerith"));
    native
        .args(["--get-urls", "--input-file", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    let mut native = native.spawn().expect("launch native stdin CLI");
    native.stdin.take().unwrap().write_all(input).unwrap();
    let native = native.wait_with_output().unwrap();
    assert!(
        native.status.success(),
        "native stdin CLI failed: {}",
        String::from_utf8_lossy(&native.stderr)
    );
    assert_eq!(
        String::from_utf8(native.stdout.clone()).unwrap(),
        "https://example.org/first.jpg?size=large\nhttps://cdn.example.org/second.webp\n"
    );

    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for stdin input comparison");

    let mut reference = Command::new("python3");
    reference.args([
        "-m",
        "gallery_dl",
        "--config-ignore",
        "--no-input",
        "--get-urls",
        "--input-file",
        "-",
    ]);
    reference.env("PYTHONPATH", &root);
    reference.env("PYTHONNOUSERSITE", "1");
    reference.env("PYTHONDONTWRITEBYTECODE", "1");
    reference.stdin(Stdio::piped()).stdout(Stdio::piped());
    let mut reference = reference.spawn().expect("launch pinned stdin CLI");
    reference.stdin.take().unwrap().write_all(input).unwrap();
    let reference = reference.wait_with_output().unwrap();

    assert!(reference.status.success(), "pinned stdin CLI failed");
    assert_eq!(native.stdout, reference.stdout);
    assert_eq!(
        String::from_utf8(native.stdout).unwrap(),
        "https://example.org/first.jpg?size=large\nhttps://cdn.example.org/second.webp\n"
    );
}

#[test]
fn pinned_and_native_extractor_info_match_directlink() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for extractor-info comparison");
    let url = "https://cdn.example.org/gallery/photo.jpg";
    let mut reference = Command::new("python3");
    reference.args([
        "-m",
        "gallery_dl",
        "--config-ignore",
        "--no-input",
        "-E",
        url,
    ]);
    reference.env("PYTHONPATH", root);
    reference.env("PYTHONNOUSERSITE", "1");
    reference.env("PYTHONDONTWRITEBYTECODE", "1");
    let reference = spawn(reference, "pinned gallery-dl --extractor-info");

    let native = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args(["-E", url])
        .output()
        .unwrap();
    assert!(
        native.status.success(),
        "native --extractor-info failed: {}",
        String::from_utf8_lossy(&native.stderr)
    );
    assert_eq!(String::from_utf8(native.stdout).unwrap(), reference);
}

fn same_field(native: &Value, reference: &Value, field: &str, label: &str) {
    assert_eq!(
        native.get(field).and_then(Value::as_str),
        reference.get(field).and_then(Value::as_str),
        "{label}: upstream field {field}"
    );
}

fn native_sidecar(
    item: &galerith::MediaItem,
    target: &Path,
    runtime: &tokio::runtime::Runtime,
) -> Value {
    std::fs::write(target, b"metadata fixture").unwrap();
    let metadata = galerith::extractor::directlink::resolve_messages(&item.url)
        .unwrap()
        .into_iter()
        .find_map(|message| match message {
            galerith::message::ExtractorMessage::Url { metadata, .. } => Some(metadata),
            _ => None,
        })
        .unwrap();
    let sidecar = runtime
        .block_on(galerith::postprocess::write_metadata(
            target, item, &metadata,
        ))
        .unwrap();
    serde_json::from_slice(&std::fs::read(sidecar).unwrap()).unwrap()
}

#[test]
fn live_pinned_runtime_directlink_events_match_on_source_metadata() {
    // This test is intentionally optional for ordinary cargo test runs.
    // Dedicated CI sets both variables and must fail if the pin is missing.
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("CI reference oracle must supply the pinned upstream directory");
    let root = Path::new(&root);
    let version = std::fs::read_to_string(root.join("gallery_dl/version.py"))
        .expect("pinned upstream source missing");
    assert!(!version.is_empty());
    let commit = env::var("GALERITH_PINNED_REFERENCE_SHA")
        .expect("CI must declare expected upstream git revision");
    assert_eq!(commit, PINNED, "upstream pin changed without acceptance");
    let mut verified_cases = Vec::with_capacity(URLS.len());
    let db_dir = env::temp_dir().join(format!(
        "galerith-pinned-archive-oracle-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&db_dir);
    std::fs::create_dir_all(&db_dir).unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    for (case_index, url) in URLS.iter().enumerate() {
        let rust = native_events(url);
        let native_tuples = native_upstream_tuples(url);
        let source = pinned_events(root, url);
        assert_eq!(rust.len(), 2, "{url}: native event count");
        assert_eq!(native_tuples.len(), rust.len(), "{url}: native tuple count");
        assert_eq!(source.len(), 2, "{url}: upstream event count");
        for (ordinal, (native, upstream)) in rust.iter().zip(&source).enumerate() {
            let tuple = upstream.as_array().expect("upstream message tuple");
            let kind = if ordinal == 0 { 2 } else { 3 };
            assert_eq!(tuple.first().and_then(Value::as_i64), Some(kind));
            assert_eq!(
                native["type"],
                if ordinal == 0 { "directory" } else { "url" }
            );
            if ordinal == 1 {
                assert_eq!(native["url"], tuple[1], "{url}: original URL identity");
            }
            let reference = tuple.last().expect("pinned source metadata");
            let metadata = &native["metadata"];
            assert_eq!(
                &native_tuples[ordinal], upstream,
                "{url}: complete upstream tuple output"
            );
            let sidecar = if ordinal == 1 {
                let item = galerith::extract_direct_media(url).unwrap();
                Some(native_sidecar(
                    &item,
                    &db_dir.join(format!("metadata-{case_index}-{}", item.filename)),
                    &runtime,
                ))
            } else {
                None
            };
            for key in [
                "category",
                "subcategory",
                "domain",
                "path",
                "extension",
                "query",
                "fragment",
            ] {
                same_field(metadata, reference, key, url);
            }
            if let Some(sidecar) = sidecar {
                for key in ["category", "domain", "subcategory", "path", "extension"] {
                    same_field(&sidecar, reference, key, url);
                }
                assert_eq!(
                    sidecar["stem"], reference["filename"],
                    "{url}: sidecar stem"
                );
                assert!(
                    sidecar["source_url"]
                        .as_str()
                        .is_some_and(|source| !source.contains('?') && !source.contains('#')),
                    "{url}: sidecar must omit query and fragment"
                );
            }
            assert_eq!(
                metadata["stem"], reference["filename"],
                "{url}: pinned filename stem"
            );
        }
        // Upstream formats the extension in lowercase, whereas the native
        // flat filename preview preserves the source extension's spelling.
        // Compare the upstream extension and stem independently above;
        // do not conceal this outstanding filename-policy divergence.
        let parsed = url::Url::parse(url).expect("verified test URL");
        let basename = parsed
            .path_segments()
            .and_then(|mut segments| segments.next_back())
            .expect("directlink basename");
        let basename = percent_encoding::percent_decode_str(basename)
            .decode_utf8()
            .expect("UTF-8 fixture filename");
        assert_eq!(
            rust[0]["metadata"]["filename"],
            basename.as_ref(),
            "{url}: native flat filename policy"
        );
        // Derive the archive entry from the *actual pinned upstream*
        // extractor metadata. Then verify that Galerith recognizes it via
        // a gallery-dl-shaped SQLite archive without modifying that table.
        let source_fields = source[0]
            .as_array()
            .and_then(|tuple| tuple.last())
            .expect("pinned directory metadata");
        let upstream_key = format!(
            "directlink{}/{}/{}.{}",
            source_fields["domain"].as_str().unwrap(),
            source_fields["path"].as_str().unwrap(),
            source_fields["filename"].as_str().unwrap(),
            source_fields["extension"].as_str().unwrap()
        );
        let archive_path = db_dir.join(format!("case-{case_index}.sqlite"));
        let connection = rusqlite::Connection::open(&archive_path).unwrap();
        connection
            .execute_batch("CREATE TABLE archive(entry TEXT PRIMARY KEY) WITHOUT ROWID;")
            .unwrap();
        connection
            .execute(
                "INSERT INTO archive(entry) VALUES (?1)",
                rusqlite::params![upstream_key],
            )
            .unwrap();
        drop(connection);
        let upstream_bytes = std::fs::read(&archive_path).unwrap();
        let native_path = db_dir.join(format!("native-{case_index}.sqlite"));
        let native_archive =
            galerith::archive::Archive::open_with_reference(&native_path, Some(&archive_path))
                .unwrap();
        let item = galerith::extract_direct_media(url).unwrap();
        assert!(
            native_archive.contains(&item).unwrap(),
            "{url}: pinned directlink archive key must be recognized"
        );
        let unchanged: i64 = rusqlite::Connection::open(&archive_path)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM archive", [], |row| row.get(0))
            .unwrap();
        assert_eq!(unchanged, 1, "foreign archive must remain untouched");
        assert_eq!(
            native_archive.import_reference().unwrap(),
            (1, 1),
            "{url}: import must recognize exactly one upstream archive key"
        );
        drop(native_archive);
        assert_eq!(
            std::fs::read(&archive_path).unwrap(),
            upstream_bytes,
            "{url}: external upstream archive bytes must not change"
        );
        let reopened = galerith::archive::Archive::open(&native_path).unwrap();
        assert!(
            reopened.contains(&item).unwrap(),
            "{url}: imported key must work without the upstream connection"
        );
        drop(reopened);
        let native_bytes = std::fs::read(&native_path).unwrap();
        assert!(
            !native_bytes
                .windows(upstream_key.len())
                .any(|part| part == upstream_key.as_bytes()),
            "{url}: imported archive must not persist plaintext upstream IDs"
        );

        // Upstream sets archive-prefix="" when archive-table is explicitly
        // configured. Differential-check that site-independent directlink
        // keys from the actual pinned source retain their meaning after
        // safe, category-scoped native import.
        let bare_key = upstream_key
            .strip_prefix("directlink")
            .expect("pinned category prefix");
        let custom_path = db_dir.join(format!("custom-{case_index}.sqlite"));
        let custom_source = rusqlite::Connection::open(&custom_path).unwrap();
        custom_source
            .execute_batch("CREATE TABLE named (entry TEXT PRIMARY KEY) WITHOUT ROWID;")
            .unwrap();
        custom_source
            .execute(
                "INSERT INTO named (entry) VALUES (?1)",
                rusqlite::params![bare_key],
            )
            .unwrap();
        drop(custom_source);
        let source_bytes = std::fs::read(&custom_path).unwrap();
        let custom_native = db_dir.join(format!("custom-native-{case_index}.sqlite"));
        let named = galerith::archive::Archive::open_with_reference_options(
            &custom_native,
            Some(&custom_path),
            Some("named"),
            None,
        )
        .unwrap();
        assert!(
            named.contains(&item).unwrap(),
            "{url}: named table default must omit the category prefix"
        );
        assert_eq!(named.import_reference().unwrap(), (1, 1));
        drop(named);
        assert_eq!(std::fs::read(&custom_path).unwrap(), source_bytes);
        let detached = galerith::archive::Archive::open(&custom_native).unwrap();
        assert!(
            detached.contains(&item).unwrap(),
            "{url}: named table imported key must work offline"
        );
        drop(detached);

        verified_cases.push(json!({
            "url": url,
            "reference_events": source,
            "native_events": rust,
            "status": "source_fields_and_readonly_archive_match",
            "archive_key_compatible": true,
            "external_archive_read_only": true,
            "offline_migration_recognized": true,
            "named_table_prefix_semantics": "default_empty_verified",
            "known_divergence": "upstream formatted extension is lowercase; native flat filename retains original extension case"
        }));
    }
    std::fs::remove_dir_all(&db_dir).unwrap();
    if let Ok(report_path) = env::var("GALERITH_ORACLE_REPORT_PATH") {
        let path = Path::new(&report_path);
        std::fs::create_dir_all(path.parent().expect("oracle report parent"))
            .expect("create oracle report directory");
        let report = json!({
            "schema_version": 1,
            "upstream_commit": PINNED,
            "scope": "pinned-directlink-offline-source-metadata",
            "cases": verified_cases,
            "limitations": [
                "Source metadata and default directlink archive read-compatibility only, not general filename/archive semantics",
                "No non-directlink extractor or live source website was contacted",
                "Only repository CI executes the original upstream Python interpreter"
            ]
        });
        std::fs::write(
            path,
            format!("{}\n", serde_json::to_string_pretty(&report).unwrap()),
        )
        .expect("write pinned reference comparison evidence");
    }
}

#[test]
fn pinned_cli_and_native_match_input_mutation_after_retry_success_and_exhaustion() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("CI reference oracle must supply the pinned upstream directory");
    let root = Path::new(&root);
    let commit = env::var("GALERITH_PINNED_REFERENCE_SHA")
        .expect("CI must declare expected upstream git revision");
    assert_eq!(commit, PINNED, "upstream pin changed without acceptance");
    let case_root = env::temp_dir().join(format!(
        "galerith-pinned-input-oracle-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&case_root);
    std::fs::create_dir_all(&case_root).unwrap();
    const RETRYABLE: &[u8] =
        b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    const SUCCESS: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: 4\r\nConnection: close\r\n\r\nJPEG";
    run_input_retry_case(
        &case_root,
        root,
        "retry-success",
        [RETRYABLE, SUCCESS, RETRYABLE, SUCCESS],
        true,
    );
    run_input_retry_case(
        &case_root,
        root,
        "retry-exhausted",
        [RETRYABLE, RETRYABLE, RETRYABLE, RETRYABLE],
        false,
    );
    std::fs::remove_dir_all(case_root).unwrap();
}

#[test]
fn pinned_cli_and_native_match_delete_input_line_after_success() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let pinned_root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("CI reference oracle must supply the pinned upstream directory");
    let pinned_root = Path::new(&pinned_root);
    let commit = env::var("GALERITH_PINNED_REFERENCE_SHA")
        .expect("CI must declare expected upstream git revision");
    assert_eq!(commit, PINNED, "upstream pin changed without acceptance");

    let case_root = env::temp_dir().join(format!(
        "galerith-pinned-delete-input-oracle-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&case_root);
    std::fs::create_dir_all(&case_root).unwrap();
    let (url, server) = serve_two_successes(
        b"HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: 4\r\nConnection: close\r\n\r\nJPEG",
    );

    for (label, pinned) in [("reference", true), ("native", false)] {
        let input = case_root.join(format!("{label}.txt"));
        let output = case_root.join(format!("{label}-output"));
        std::fs::create_dir_all(&output).unwrap();
        std::fs::write(&input, format!("{url}\n# keep this comment\n")).unwrap();

        let mut command = if pinned {
            let mut command = Command::new("python3");
            command.args(["-m", "gallery_dl", "--config-ignore", "--no-input"]);
            command.env("PYTHONPATH", pinned_root);
            command.env("PYTHONNOUSERSITE", "1");
            command.env("PYTHONDONTWRITEBYTECODE", "1");
            command
        } else {
            Command::new(env!("CARGO_BIN_EXE_galerith"))
        };
        command
            .args(["--quiet", "-x"])
            .arg(&input)
            .arg("-d")
            .arg(&output);
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
        let result = command.output().expect("launch input-delete CLI");
        assert!(
            result.status.success(),
            "{label}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            std::fs::read_to_string(&input).unwrap(),
            "# keep this comment\n",
            "{label}: successful URL should be deleted and other lines retained"
        );
        assert!(
            contains_file_with_bytes(&output, b"JPEG"),
            "{label}: CLI did not save the fixture body"
        );
    }

    server.join().unwrap();
    std::fs::remove_dir_all(case_root).unwrap();
}

#[test]
fn pinned_and_native_clients_match_proxy_auth_and_user_agent() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let reference_root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("CI reference oracle must supply the pinned upstream directory");
    let reference_root = Path::new(&reference_root);
    let commit = env::var("GALERITH_PINNED_REFERENCE_SHA")
        .expect("CI must declare expected upstream git revision");
    assert_eq!(commit, PINNED, "upstream pin changed without acceptance");
    let root = env::temp_dir().join(format!("galerith-user-agent-oracle-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    for (scope, header_scope, label) in [
        (
            "extractor.directlink.proxy",
            "extractor.directlink.headers",
            "extractor-proxy",
        ),
        (
            "downloader.http.proxy",
            "downloader.http.headers",
            "downloader-proxy",
        ),
    ] {
        let pinned = run_user_agent_fixture(
            &root,
            Some(reference_root),
            &format!("pinned-{label}"),
            true,
            scope,
            header_scope,
        );
        let native = run_user_agent_fixture(
            &root,
            None,
            &format!("native-{label}"),
            false,
            scope,
            header_scope,
        );
        for (label, request) in [("pinned", pinned), ("native", native)] {
            assert!(
                request.starts_with("GET http://example.invalid/asset.jpg HTTP/1.1\r\n"),
                "{label}: proxy did not receive absolute-form request: {request}"
            );
            assert!(
                request
                    .to_ascii_lowercase()
                    .lines()
                    .any(|header| header == "user-agent: galerith-reference-probe/7"),
                "{label}: configured User-Agent missing from request: {request}"
            );
            assert!(
                request
                    .to_ascii_lowercase()
                    .lines()
                    .any(|header| header == "accept: */*"),
                "{label}: default HTTP Accept header missing from request: {request}"
            );
            assert!(
                request
                    .to_ascii_lowercase()
                    .lines()
                    .any(|header| header == "x-preview: header"),
                "{label}: configured media header missing from request: {request}"
            );
            assert!(
                request
                    .to_ascii_lowercase()
                    .lines()
                    .any(|header| header == "proxy-authorization: basic dxnlcjpzzwnyzxq="),
                "{label}: proxy credentials missing from proxy request: {request}"
            );
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn pinned_and_native_clients_match_authenticated_https_proxy_connect() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let reference_root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("CI reference oracle must supply the pinned upstream directory");
    let reference_root = Path::new(&reference_root);
    let commit = env::var("GALERITH_PINNED_REFERENCE_SHA")
        .expect("CI must declare expected upstream git revision");
    assert_eq!(commit, PINNED, "upstream pin changed without acceptance");

    let root = env::temp_dir().join(format!(
        "galerith-https-proxy-oracle-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let run = |label: &str, reference: bool| {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let proxy = format!("http://user:secret@{address}");
        let proxy_option = format!(
            "extractor.directlink.proxy={}",
            serde_json::to_string(&proxy).unwrap()
        );
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let count = socket.read(&mut buffer).unwrap();
                assert_ne!(count, 0, "client closed before sending CONNECT headers");
                request.extend_from_slice(&buffer[..count]);
                assert!(request.len() < 16 * 1024, "CONNECT headers too large");
            }
            socket
                .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                .unwrap();
            String::from_utf8_lossy(&request).into_owned()
        });

        let output = root.join(format!("{label}-output"));
        std::fs::create_dir_all(&output).unwrap();
        let mut command = if reference {
            let mut command = Command::new("python3");
            command.args([
                "-m",
                "gallery_dl",
                "--config-ignore",
                "--no-input",
                "--quiet",
                "-o",
                &proxy_option,
                "-R",
                "0",
                "-d",
                output.to_str().unwrap(),
                "https://example.invalid/asset.jpg",
            ]);
            command.env("PYTHONPATH", reference_root);
            command.env("PYTHONNOUSERSITE", "1");
            command.env("PYTHONDONTWRITEBYTECODE", "1");
            command
        } else {
            let mut command = Command::new(env!("CARGO_BIN_EXE_galerith"));
            command.args([
                "--quiet",
                "-o",
                &proxy_option,
                "--retries",
                "0",
                "-d",
                output.to_str().unwrap(),
                "https://example.invalid/asset.jpg",
            ]);
            command
        };
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
        let result = command.output().expect("launch HTTPS proxy fixture client");
        assert!(
            !result.status.success(),
            "fixture should close after CONNECT"
        );
        assert!(
            !String::from_utf8_lossy(&result.stderr).contains("secret"),
            "{label}: proxy credential leaked into diagnostics"
        );
        let request = server.join().unwrap();
        std::fs::remove_dir_all(output).unwrap();
        request
    };

    let pinned = run("pinned", true);
    let native = run("native", false);
    for (label, request) in [("pinned", pinned.as_str()), ("native", native.as_str())] {
        assert!(
            request.starts_with("CONNECT example.invalid:443 HTTP/1.1\r\n"),
            "{label}: expected an HTTPS CONNECT tunnel, got {request}"
        );
        assert!(
            request
                .to_ascii_lowercase()
                .lines()
                .any(|header| header == "proxy-authorization: basic dxnlcjpzzwnyzxq="),
            "{label}: proxy credentials missing from CONNECT: {request}"
        );
        assert!(
            !request.to_ascii_lowercase().contains("\r\nauthorization:"),
            "{label}: proxy credentials leaked into origin Authorization: {request}"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn pinned_and_native_clients_match_extractor_source_address_binding() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let pinned_root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("CI reference oracle must supply the pinned upstream directory");
    let pinned_root = Path::new(&pinned_root);
    let commit = env::var("GALERITH_PINNED_REFERENCE_SHA")
        .expect("CI must declare expected upstream git revision");
    assert_eq!(commit, PINNED, "upstream pin changed without acceptance");
    let root = env::temp_dir().join(format!(
        "galerith-source-address-oracle-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let pinned = run_extractor_source_address_fixture(&root, Some(pinned_root), "pinned", true);
    let native = run_extractor_source_address_fixture(&root, None, "native", false);
    let expected = "127.0.0.2".parse::<std::net::IpAddr>().unwrap();
    assert_eq!(pinned.ip(), expected, "pinned client bound {pinned}");
    assert_eq!(native.ip(), expected, "native client bound {native}");
    assert_eq!(pinned.ip(), native.ip());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn pinned_and_native_clients_match_extractor_user_agent_config() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let pinned_root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("CI reference oracle must supply the pinned upstream directory");
    let pinned_root = Path::new(&pinned_root);
    let commit = env::var("GALERITH_PINNED_REFERENCE_SHA")
        .expect("CI must declare expected upstream git revision");
    assert_eq!(commit, PINNED, "upstream pin changed without acceptance");
    let root = env::temp_dir().join(format!(
        "galerith-user-agent-config-oracle-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let pinned = run_extractor_user_agent_fixture(&root, Some(pinned_root), "pinned", true);
    let native = run_extractor_user_agent_fixture(&root, None, "native", false);
    for (label, request) in [("pinned", pinned), ("native", native)] {
        assert!(
            request
                .to_ascii_lowercase()
                .lines()
                .any(|header| { header == "user-agent: extractor-reference-agent/9" }),
            "{label}: category User-Agent was not applied: {request}"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn pinned_and_native_clients_match_429_retry_interval() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let pinned_root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("CI reference oracle must supply the pinned upstream directory");
    let pinned_root = Path::new(&pinned_root);
    let commit = env::var("GALERITH_PINNED_REFERENCE_SHA")
        .expect("CI must declare expected upstream git revision");
    assert_eq!(commit, PINNED, "upstream pin changed without acceptance");
    let root = env::temp_dir().join(format!("galerith-429-oracle-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let pinned = run_429_retry_delay_fixture(&root, pinned_root, "pinned", true);
    let native = run_429_retry_delay_fixture(&root, pinned_root, "native", false);
    for (label, elapsed) in [("pinned", pinned), ("native", native)] {
        assert!(
            (std::time::Duration::from_millis(900)..std::time::Duration::from_secs(3))
                .contains(&elapsed),
            "{label}: configured first 429 retry interval was {elapsed:?}"
        );
    }
    assert!(
        pinned.abs_diff(native) < std::time::Duration::from_millis(500),
        "pinned/native 429 intervals differ: pinned={pinned:?}, native={native:?}"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn pinned_and_native_clients_match_downloader_http_timeout() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let pinned_root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("CI reference oracle must supply the pinned upstream directory");
    let pinned_root = Path::new(&pinned_root);
    let commit = env::var("GALERITH_PINNED_REFERENCE_SHA")
        .expect("CI must declare expected upstream git revision");
    assert_eq!(commit, PINNED, "upstream pin changed without acceptance");
    let root = env::temp_dir().join(format!("galerith-timeout-oracle-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let pinned = run_http_timeout_fixture(&root, pinned_root, "pinned", true);
    let native = run_http_timeout_fixture(&root, pinned_root, "native", false);
    for (label, elapsed) in [("pinned", pinned), ("native", native)] {
        assert!(
            (std::time::Duration::from_millis(60)..std::time::Duration::from_millis(300))
                .contains(&elapsed),
            "{label}: configured 100ms media timeout elapsed after {elapsed:?}"
        );
    }
    assert!(
        pinned.abs_diff(native) < std::time::Duration::from_millis(150),
        "pinned/native timeout results differ: pinned={pinned:?}, native={native:?}"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn pinned_and_native_clients_match_extractor_and_skip_sleep_timing() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let pinned_root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("CI reference oracle must supply the pinned upstream directory");
    let pinned_root = Path::new(&pinned_root);
    let commit = env::var("GALERITH_PINNED_REFERENCE_SHA")
        .expect("CI must declare expected upstream git revision");
    assert_eq!(commit, PINNED, "upstream pin changed without acceptance");

    let root = env::temp_dir().join(format!("galerith-sleep-oracle-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    for (label, option, skip_existing, lower_bound_ms) in [
        ("extractor", "--sleep-extractor", false, 100),
        ("skip", "--sleep-skip", true, 200),
    ] {
        let pinned = run_reference_sleep_interval_fixture(
            &root,
            pinned_root,
            &format!("pinned-{label}"),
            true,
            option,
            skip_existing,
        );
        let native = run_reference_sleep_interval_fixture(
            &root,
            pinned_root,
            &format!("native-{label}"),
            false,
            option,
            skip_existing,
        );
        for (client, elapsed) in [("pinned", pinned), ("native", native)] {
            assert!(
                (std::time::Duration::from_millis(lower_bound_ms)
                    ..std::time::Duration::from_millis(900))
                    .contains(&elapsed),
                "{client} {option} interval was {elapsed:?}"
            );
        }
        assert!(
            pinned.abs_diff(native) < std::time::Duration::from_millis(350),
            "pinned/native {option} delays differ: pinned={pinned:?}, native={native:?}"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn pinned_and_native_sleep_request_does_not_delay_direct_media() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let pinned_root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("CI reference oracle must supply the pinned upstream directory");
    let pinned_root = Path::new(&pinned_root);
    let commit = env::var("GALERITH_PINNED_REFERENCE_SHA")
        .expect("CI must declare expected upstream git revision");
    assert_eq!(commit, PINNED, "upstream pin changed without acceptance");

    let root = env::temp_dir().join(format!(
        "galerith-sleep-request-media-oracle-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let pinned = run_reference_media_sleep_request_case(&root, pinned_root, "pinned", true);
    let native = run_reference_media_sleep_request_case(&root, pinned_root, "native", false);
    for (client, interval) in [("pinned", pinned), ("native", native)] {
        assert!(
            interval < std::time::Duration::from_millis(100),
            "{client}: sleep-request delayed direct media by {interval:?}"
        );
    }
    assert!(
        pinned.abs_diff(native) < std::time::Duration::from_millis(75),
        "pinned/native direct-media intervals differ: pinned={pinned:?}, native={native:?}"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn pinned_and_native_clients_save_the_same_formatted_filename() {
    let root = env::temp_dir().join(format!(
        "galerith-filename-filesystem-oracle-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let cases = [
        ("original-filename-shortcut", "/O", "asset.jpg"),
        (
            "conditional",
            "{category}_{extension:?[/]/}.{extension}",
            "directlink_[jpg].jpg",
        ),
        (
            "conditional-empty",
            "{category}{extension[99:]:?[/]/}",
            "directlink",
        ),
        (
            "conversion-and-escaped-braces",
            "{{proof}}_{category}_{extension!u}.{extension}",
            "{proof}_directlink_JPG.jpg",
        ),
        (
            "slice",
            "{category}_{extension[0:2]}.{extension}",
            "directlink_jp.jpg",
        ),
        ("byte-slice", "{filename[b0:3]}.{extension}", "ass.jpg"),
        (
            "filename-stem",
            "{category}_{filename}.{extension}",
            "directlink_asset.jpg",
        ),
        (
            "missing-alternative-fallback",
            "{post_id|category}",
            "directlink",
        ),
        (
            "first-nonempty-alternative",
            "{_lit[chosen]|category}",
            "chosen",
        ),
        (
            "string-alignment",
            "{category:_>12}-{filename:#^7}.{extension}",
            "__directlink-#asset#.jpg",
        ),
        (
            "literal-field",
            "{category}_{_lit[proof]}.{extension}",
            "directlink_proof.jpg",
        ),
        ("unicode-literal", "{_lit[猫]}", "猫"),
        ("unicode-alignment", "{_lit[猫]:_^5}", "__猫__"),
        ("string-precision", "{category:.5}", "direc"),
        ("url-encode-conversion", "{_lit[a b]!q}", "a%20b"),
        ("url-decode-conversion", "{_lit[a%20b]!Q}", "a b"),
        (
            "json-conversion",
            "{_lit[élan]!j:R\"/_/R\\/_/}",
            "__u00e9lan_",
        ),
        ("slug-conversion", "{_lit[Hello, World]!g}", "hello-world"),
        ("repr-conversion", r#"{_lit[foo]!r:R'/_/}"#, "_foo_"),
        (
            "ascii-conversion",
            r#"{_lit[é猫]!a:R'/_/R\/-/}"#,
            "_-xe9-u732b_",
        ),
        (
            "capitalize-conversion",
            "{_lit[hELLO WORLD]!c}",
            "Hello world",
        ),
        (
            "title-conversion",
            "{_lit[aUTUMN, ÉLAN vITAL]!C}",
            "Autumn, Élan Vital",
        ),
        ("trim-conversion", "{_lit[  hello  ]!t}", "hello"),
        (
            "whitespace-conversion",
            "{_lit[  spring\t\n  rain ]!W}",
            "spring rain",
        ),
        ("negative-slice", "{extension[-2:]}", "pg"),
        ("replacement-specifier", "{category:Ri/I/}", "dIrectlInk"),
        (
            "chained-replacement-specifiers",
            "{category:Ri/I/Rl/L/}",
            "dIrectLInk",
        ),
        (
            "replacement-conditional-chain",
            "{category:Ri/I/?[/]/}",
            "[dIrectlInk]",
        ),
        (
            "replacement-truncate-chain",
            "{category:Ri/I/X5/-/}",
            "dIre-",
        ),
        ("truncate-specifier", "{category:X5/-/}", "dire-"),
        ("overlength-replacement", "{category:L5/long/}", "long"),
    ];
    let oracle_enabled = env::var_os("GALERITH_REFERENCE_ORACLE").is_some();
    let pinned_root = if oracle_enabled {
        let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
            .expect("CI reference oracle must supply the pinned upstream directory");
        let commit = env::var("GALERITH_PINNED_REFERENCE_SHA")
            .expect("CI must declare expected upstream git revision");
        assert_eq!(commit, PINNED, "upstream pin changed without acceptance");
        Some(PathBuf::from(root))
    } else {
        None
    };
    for (label, template, expected) in cases {
        let native = run_reference_filename_filesystem_fixture(
            &root,
            Path::new("."),
            &format!("native-{label}"),
            false,
            template,
        );
        assert_eq!(native, Path::new(expected), "native {label} filename");
        if let Some(pinned_root) = pinned_root.as_deref() {
            let pinned = run_reference_filename_filesystem_fixture(
                &root,
                pinned_root,
                &format!("pinned-{label}"),
                true,
                template,
            );
            assert_eq!(pinned, native, "upstream/native {label} saved path differs");
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn pinned_formatter_matches_native_numeric_id_radix_formats() {
    let cases = ["d", "05d", "b", "#b", "o", "#o", "x", "8x", "08x", "#08x"];
    let item = galerith::MediaItem {
        url: "https://example.org/image.jpg".into(),
        filename: "image.jpg".into(),
        extractor: "directlink",
        post_id: Some(42),
    };
    let native = cases
        .iter()
        .map(|spec| galerith::filename::render(&format!("{{id:{spec}}}"), &item).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        native,
        [
            "42", "00042", "101010", "0b101010", "52", "0o52", "2a", "      2a", "0000002a",
            "0x00002a",
        ]
    );

    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for numeric formatter comparison");
    let format_specs = cases
        .iter()
        .map(|spec| format!("{{id:{spec}}}"))
        .collect::<Vec<_>>()
        .join("|");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        &format!(
            "from gallery_dl import formatter; formats={format_specs:?}.split('|'); print('|'.join(formatter.parse(value).format_map({{'id':42}}) for value in formats))"
        ),
    ]);
    command.env("PYTHONPATH", &root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    let upstream = spawn(command, "pinned numeric filename formatter");
    assert_eq!(upstream.trim(), native.join("|"));

    let mut grouped_item = item;
    grouped_item.post_id = Some(1_234_567);
    let grouped_native = [",", "_", "12,"]
        .iter()
        .map(|spec| galerith::filename::render(&format!("{{id:{spec}}}"), &grouped_item).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(grouped_native, ["1,234,567", "1_234_567", "   1,234,567"]);
    let mut grouped_command = Command::new("python3");
    grouped_command.args([
        "-c",
        "from gallery_dl import formatter; print('|'.join(formatter.parse('{id:'+spec+'}').format_map({'id':1234567}) for spec in (',', '_', '12,')))",
    ]);
    grouped_command.env("PYTHONPATH", &root);
    grouped_command.env("PYTHONNOUSERSITE", "1");
    grouped_command.env("PYTHONDONTWRITEBYTECODE", "1");
    let grouped_reference = spawn(grouped_command, "Python grouped numeric formatter");
    assert_eq!(grouped_reference.trim(), grouped_native.join("|"));
}

#[test]
fn pinned_and_native_clients_match_last_modified_mtime() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let pinned_root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("CI reference oracle must supply the pinned upstream directory");
    let pinned_root = Path::new(&pinned_root);
    let commit = env::var("GALERITH_PINNED_REFERENCE_SHA")
        .expect("CI must declare expected upstream git revision");
    assert_eq!(commit, PINNED, "upstream pin changed without acceptance");

    let root = env::temp_dir().join(format!(
        "galerith-pinned-mtime-oracle-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let (url, server) = serve_two_successes(
        b"HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: 4\r\nLast-Modified: Wed, 21 Oct 2015 07:28:00 GMT\r\nConnection: close\r\n\r\nJPEG",
    );

    let mut mtimes = Vec::new();
    for (label, pinned) in [("reference", true), ("native", false)] {
        let output = root.join(format!("{label}-output"));
        std::fs::create_dir_all(&output).unwrap();
        let mut command = if pinned {
            let mut command = Command::new("python3");
            command.args(["-m", "gallery_dl", "--config-ignore", "--no-input"]);
            command.env("PYTHONPATH", pinned_root);
            command.env("PYTHONNOUSERSITE", "1");
            command.env("PYTHONDONTWRITEBYTECODE", "1");
            command
        } else {
            Command::new(env!("CARGO_BIN_EXE_galerith"))
        };
        command.args(["--quiet", "-d"]).arg(&output).arg(&url);
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
        let result = command
            .output()
            .expect("launch Last-Modified fixture client");
        assert!(
            result.status.success(),
            "{label}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let files = saved_paths_with_bytes(&output, b"JPEG");
        assert_eq!(files.len(), 1, "{label}: expected one saved fixture file");
        mtimes.push((
            label,
            std::fs::metadata(&files[0]).unwrap().modified().unwrap(),
        ));
    }
    server.join().unwrap();

    let expected = httpdate::parse_http_date("Wed, 21 Oct 2015 07:28:00 GMT").unwrap();
    for (label, mtime) in mtimes {
        let delta = mtime
            .duration_since(expected)
            .or_else(|error| expected.duration_since(mtime).map_err(|_| error))
            .unwrap();
        assert!(
            delta < std::time::Duration::from_secs(2),
            "{label}: mtime differs from pinned Last-Modified by {delta:?}"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn pinned_and_native_filter_known_content_lengths_by_size_limits() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let pinned_root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("CI reference oracle must supply the pinned upstream directory");
    let pinned_root = Path::new(&pinned_root);
    let commit = env::var("GALERITH_PINNED_REFERENCE_SHA")
        .expect("CI must declare expected upstream git revision");
    assert_eq!(commit, PINNED, "upstream pin changed without acceptance");

    const SMALL: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nSMAL";
    const LARGE: &[u8] =
        b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\nLARGE123";
    let root = env::temp_dir().join(format!(
        "galerith-pinned-filesize-oracle-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let (url, server) =
        serve_retry_sequence([SMALL, SMALL, LARGE, LARGE, SMALL, SMALL, SMALL, SMALL]);

    for (label, pinned, flag, value, response_body) in [
        (
            "reference-min",
            true,
            "--filesize-min",
            "5",
            b"SMAL".as_slice(),
        ),
        (
            "native-min",
            false,
            "--filesize-min",
            "5",
            b"SMAL".as_slice(),
        ),
        (
            "reference-max",
            true,
            "--filesize-max",
            "7",
            b"LARGE123".as_slice(),
        ),
        (
            "native-max",
            false,
            "--filesize-max",
            "7",
            b"LARGE123".as_slice(),
        ),
    ] {
        let output = root.join(label);
        std::fs::create_dir_all(&output).unwrap();
        let mut command = if pinned {
            let mut command = Command::new("python3");
            command.args(["-m", "gallery_dl", "--config-ignore", "--no-input"]);
            command.env("PYTHONPATH", pinned_root);
            command.env("PYTHONNOUSERSITE", "1");
            command.env("PYTHONDONTWRITEBYTECODE", "1");
            command
        } else {
            Command::new(env!("CARGO_BIN_EXE_galerith"))
        };
        command
            .args(["--quiet", flag, value, "-d"])
            .arg(&output)
            .arg(&url);
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
        let result = command.output().expect("launch size-limit fixture client");
        assert!(
            result.status.success(),
            "{label}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(
            !contains_file_with_bytes(&output, response_body),
            "{label}: saved a response outside the configured size bounds"
        );
    }

    for (label, pinned, clear_global_limit) in [
        ("reference-http-config-min", true, false),
        ("native-http-config-min", false, false),
        ("reference-http-config-cleared", true, true),
        ("native-http-config-cleared", false, true),
    ] {
        let output = root.join(label);
        std::fs::create_dir_all(&output).unwrap();
        let mut command = if pinned {
            let mut command = Command::new("python3");
            command.args(["-m", "gallery_dl", "--config-ignore", "--no-input"]);
            command.env("PYTHONPATH", pinned_root);
            command.env("PYTHONNOUSERSITE", "1");
            command.env("PYTHONDONTWRITEBYTECODE", "1");
            command
        } else {
            Command::new(env!("CARGO_BIN_EXE_galerith"))
        };
        command.args(["--quiet", "-o", "downloader.http.filesize-min=5"]);
        if clear_global_limit {
            command.args(["--filesize-min", "0"]);
        }
        command.args(["-d"]).arg(&output).arg(&url);
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
        let result = command
            .output()
            .expect("launch size-limit config fixture client");
        assert!(
            result.status.success(),
            "{label}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            contains_file_with_bytes(&output, b"SMAL"),
            clear_global_limit,
            "{label}: unexpected behavior for the HTTP size limit configuration"
        );
    }
    server.join().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn pinned_and_native_html_validation_config_allow_html_payloads() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let pinned_root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("CI reference oracle must supply the pinned upstream directory");
    let pinned_root = Path::new(&pinned_root);
    let commit = env::var("GALERITH_PINNED_REFERENCE_SHA")
        .expect("CI must declare expected upstream git revision");
    assert_eq!(commit, PINNED, "upstream pin changed without acceptance");

    const HTML: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 15\r\nConnection: close\r\n\r\n<html>ok</html>";
    let root = env::temp_dir().join(format!(
        "galerith-pinned-html-validation-oracle-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let (url, server) = serve_retry_sequence([HTML, HTML, HTML, HTML]);

    for (label, pinned, global_override) in [
        ("reference-nested", true, false),
        ("native-nested", false, false),
        ("reference-global", true, true),
        ("native-global", false, true),
    ] {
        let output = root.join(label);
        std::fs::create_dir_all(&output).unwrap();
        let mut command = if pinned {
            let mut command = Command::new("python3");
            command.args(["-m", "gallery_dl", "--config-ignore", "--no-input"]);
            command.env("PYTHONPATH", pinned_root);
            command.env("PYTHONNOUSERSITE", "1");
            command.env("PYTHONDONTWRITEBYTECODE", "1");
            command
        } else {
            Command::new(env!("CARGO_BIN_EXE_galerith"))
        };
        command.args(["--quiet", "-o", "downloader.http.validate-html=true"]);
        if global_override {
            command.args(["-o", "validate-html=false"]);
        } else {
            command.args(["-o", "downloader.http.validate-html=false"]);
        }
        command.args(["-d"]).arg(&output).arg(&url);
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
        let result = command
            .output()
            .expect("launch HTML-validation fixture client");
        assert!(
            result.status.success(),
            "{label}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(
            contains_file_with_bytes(&output, b"<html>ok</html>"),
            "{label}: disabled HTML validation should permit the fixture payload"
        );
    }
    server.join().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn pinned_and_native_http_rate_config_limit_download_speed() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let pinned_root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("CI reference oracle must supply the pinned upstream directory");
    let pinned_root = Path::new(&pinned_root);
    let commit = env::var("GALERITH_PINNED_REFERENCE_SHA")
        .expect("CI must declare expected upstream git revision");
    assert_eq!(commit, PINNED, "upstream pin changed without acceptance");

    let body = vec![b'R'; 8192];
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(&body);
    let response: &'static [u8] = Box::leak(response.into_boxed_slice());
    let root = env::temp_dir().join(format!(
        "galerith-pinned-rate-config-oracle-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let (url, server) = serve_retry_sequence([response, response]);

    for (label, pinned) in [("reference", true), ("native", false)] {
        let output = root.join(label);
        std::fs::create_dir_all(&output).unwrap();
        let mut command = if pinned {
            let mut command = Command::new("python3");
            command.args(["-m", "gallery_dl", "--config-ignore", "--no-input"]);
            command.env("PYTHONPATH", pinned_root);
            command.env("PYTHONNOUSERSITE", "1");
            command.env("PYTHONDONTWRITEBYTECODE", "1");
            command
        } else {
            Command::new(env!("CARGO_BIN_EXE_galerith"))
        };
        command
            .args(["--quiet", "-o", "downloader.http.rate=8k", "-d"])
            .arg(&output)
            .arg(&url);
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
        let started = std::time::Instant::now();
        let result = command.output().expect("launch rate fixture client");
        let elapsed = started.elapsed();
        assert!(
            result.status.success(),
            "{label}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(
            contains_file_with_bytes(&output, &body),
            "{label}: downloaded bytes differed"
        );
        assert!(
            elapsed >= std::time::Duration::from_millis(900),
            "{label}: configured 8 KiB/s rate was not applied ({elapsed:?})"
        );
    }
    server.join().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn pinned_and_native_http_signature_correction_adjust_output_extension() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let pinned_root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("CI reference oracle must supply the pinned upstream directory");
    let commit = env::var("GALERITH_PINNED_REFERENCE_SHA")
        .expect("CI must declare expected upstream git revision");
    assert_eq!(commit, PINNED, "upstream pin changed without acceptance");

    const PNG: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: 29\r\nConnection: close\r\n\r\n\x89PNG\r\n\x1a\nsynthetic-png-payload";
    let root = env::temp_dir().join(format!(
        "galerith-pinned-extension-oracle-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let (url, server) = serve_retry_sequence([PNG, PNG]);

    for (label, pinned) in [("reference", true), ("native", false)] {
        let output = root.join(label);
        std::fs::create_dir_all(&output).unwrap();
        let mut command = if pinned {
            let mut command = Command::new("python3");
            command.args(["-m", "gallery_dl", "--config-ignore", "--no-input"]);
            command.env("PYTHONPATH", &pinned_root);
            command.env("PYTHONNOUSERSITE", "1");
            command.env("PYTHONDONTWRITEBYTECODE", "1");
            command
        } else {
            Command::new(env!("CARGO_BIN_EXE_galerith"))
        };
        command
            .args([
                "--quiet",
                "-o",
                "downloader.http.adjust-extensions=true",
                "-d",
            ])
            .arg(&output)
            .arg(&url);
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
        let result = command
            .output()
            .expect("launch signature-correction fixture client");
        assert!(
            result.status.success(),
            "{label}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let filenames = saved_paths(&output)
            .into_iter()
            .filter_map(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .collect::<Vec<_>>();
        assert!(
            filenames.iter().any(|filename| filename.ends_with(".png")),
            "{label}: expected corrected PNG output, found {filenames:?}"
        );
        assert!(
            filenames.iter().all(|filename| !filename.ends_with(".jpg")),
            "{label}: original suffix must not remain: {filenames:?}"
        );
    }
    server.join().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn pinned_and_native_clients_skip_tag_sidecars_when_tags_are_missing() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let pinned_root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for tag-sidecar differential");
    let output_root =
        env::temp_dir().join(format!("galerith-tags-reference-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&output_root);
    std::fs::create_dir_all(&output_root).unwrap();

    let (pinned_url, pinned_server) = serve_retry_sequence([
        b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nIMG",
    ]);
    let mut pinned =
        Command::new(env::var_os("GALERITH_REFERENCE_PYTHON").unwrap_or_else(|| "python3".into()));
    pinned.args([
        "-m",
        "gallery_dl",
        "--config-ignore",
        "--no-input",
        "--write-tags",
        "-d",
    ]);
    pinned.arg(output_root.join("pinned"));
    pinned.arg(pinned_url);
    pinned.env("PYTHONPATH", pinned_root);
    pinned.env("PYTHONNOUSERSITE", "1");
    pinned.env("PYTHONDONTWRITEBYTECODE", "1");
    spawn(pinned, "pinned gallery-dl --write-tags");
    pinned_server.join().unwrap();

    let (native_url, native_server) = serve_retry_sequence([
        b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nIMG",
    ]);
    let mut native = Command::new(env!("CARGO_BIN_EXE_galerith"));
    native.args(["--write-tags", "-d"]);
    native.arg(output_root.join("native"));
    native.arg(native_url);
    spawn(native, "native Galerith --write-tags");
    native_server.join().unwrap();

    let pinned_tags = output_root.join("pinned/asset.jpg.txt");
    let native_tags = output_root.join("native/asset.jpg.txt");
    assert_eq!(native_tags.exists(), pinned_tags.exists());
    assert!(!native_tags.exists(), "directlink has no tag metadata");
    std::fs::remove_dir_all(output_root).unwrap();
}

#[test]
fn pinned_and_native_write_matching_directlink_metadata_core() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let pinned_root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for per-media metadata differential");
    let output_root = env::temp_dir().join(format!(
        "galerith-metadata-reference-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&output_root);
    std::fs::create_dir_all(&output_root).unwrap();

    let (pinned_url, pinned_server) = serve_retry_sequence([
        b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nIMG",
    ]);
    let mut pinned =
        Command::new(env::var_os("GALERITH_REFERENCE_PYTHON").unwrap_or_else(|| "python3".into()));
    pinned.args([
        "-m",
        "gallery_dl",
        "--config-ignore",
        "--no-input",
        "--write-metadata",
        "-D",
    ]);
    pinned.arg(output_root.join("pinned"));
    pinned.arg(pinned_url);
    pinned.env("PYTHONPATH", pinned_root);
    pinned.env("PYTHONNOUSERSITE", "1");
    pinned.env("PYTHONDONTWRITEBYTECODE", "1");
    spawn(pinned, "pinned gallery-dl --write-metadata");
    pinned_server.join().unwrap();

    let (native_url, native_server) = serve_retry_sequence([
        b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nIMG",
    ]);
    let mut native = Command::new(env!("CARGO_BIN_EXE_galerith"));
    native.args(["--write-metadata", "-d"]);
    native.arg(output_root.join("native"));
    native.arg(native_url);
    spawn(native, "native Galerith --write-metadata");
    native_server.join().unwrap();

    let pinned_sidecar = saved_paths(&output_root.join("pinned"))
        .into_iter()
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .expect("pinned per-media JSON sidecar created");
    let native_sidecar = saved_paths(&output_root.join("native"))
        .into_iter()
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .expect("native per-media JSON sidecar created");
    let pinned_metadata: Value = serde_json::from_slice(&std::fs::read(pinned_sidecar).unwrap())
        .expect("pinned per-media JSON is valid");
    let native_metadata: Value = serde_json::from_slice(&std::fs::read(native_sidecar).unwrap())
        .expect("native per-media JSON is valid");
    for key in [
        "category",
        "filename",
        "extension",
        "domain",
        "subcategory",
        "path",
    ] {
        assert_eq!(
            native_metadata.get(key),
            pinned_metadata.get(key),
            "directlink per-media metadata field {key} differs"
        );
    }
    std::fs::remove_dir_all(output_root).unwrap();
}

#[test]
fn pinned_python_mimetypes_matches_extended_mime_suffix_table() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let commit = env::var("GALERITH_PINNED_REFERENCE_SHA")
        .expect("CI must declare expected upstream git revision");
    assert_eq!(commit, PINNED, "upstream pin changed without acceptance");

    let cases = [
        ("image/x-bmp", "bin"),
        ("image/x-ms-bmp", "bin"),
        ("image/ico", "bin"),
        ("image/icon", "bin"),
        ("image/x-icon", "bin"),
        ("image/vnd.microsoft.icon", "ico"),
        ("image/x-photoshop", "bin"),
        ("application/x-photoshop", "bin"),
        ("image/vnd.adobe.photoshop", "psd"),
        ("audio/webm", "bin"),
        ("video/ogg", "ogv"),
        ("audio/ogg", "oga"),
        ("application/ogg", "ogx"),
        ("video/m4v", "bin"),
        ("video/x-m4v", "bin"),
        ("audio/wav", "bin"),
        ("audio/x-aac", "bin"),
        ("audio/flac", "flac"),
        ("application/problem+json", "bin"),
        ("application/xml", "xsl"),
        ("application/x-zip", "bin"),
        ("application/x-zip-compressed", "bin"),
        ("application/rar", "bin"),
        ("application/x-rar", "bin"),
        ("application/x-rar-compressed", "bin"),
        ("application/vnd.apple.mpegurl", "m3u"),
        ("application/x-mpegurl", "bin"),
        ("application/x-pdf", "bin"),
        ("application/x-gzip", "bin"),
        ("application/x-zstd", "bin"),
        ("application/x-7z-compressed", "7z"),
        ("image/tiff", "tiff"),
        ("image/jp2", "jp2"),
        ("image/ief", "ief"),
        ("image/x-cmu-raster", "ras"),
        ("image/x-rgb", "rgb"),
        ("image/x-xbitmap", "xbm"),
        ("image/x-xpixmap", "xpm"),
        ("image/x-xwindowdump", "xwd"),
        ("image/heic-sequence", "heics"),
        ("image/heif-sequence", "heifs"),
        ("image/jxl", "jxl"),
        ("image/x-portable-pixmap", "ppm"),
        ("image/x-portable-graymap", "pgm"),
        ("image/x-portable-bitmap", "pbm"),
        ("image/x-portable-anymap", "pnm"),
        ("video/x-flv", "flv"),
        ("video/x-sgi-movie", "movie"),
        ("video/x-ms-asf", "bin"),
        ("application/vnd.ms-asf", "asf"),
        ("audio/mp4", "m4a"),
        ("audio/3gpp", "3gp"),
        ("audio/3gpp2", "3g2"),
        ("audio/x-aiff", "aif"),
        ("audio/basic", "au"),
        ("audio/x-pn-realaudio", "ra"),
    ];
    let python = env::var_os("GALERITH_REFERENCE_PYTHON").unwrap_or_else(|| "python3".into());
    let mut command = Command::new(python);
    command.args([
        "-c",
        "import mimetypes, sys; print('\\n'.join(mimetypes.guess_extension(x, strict=False) or '.bin' for x in sys.argv[1:]))",
    ]);
    command.args(cases.iter().map(|(mime, _)| mime));
    command.env("PYTHONNOUSERSITE", "1");
    let output = command
        .output()
        .expect("run Python's MIME database reference");
    assert!(
        output.status.success(),
        "Python MIME lookup failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let actual = String::from_utf8(output.stdout).unwrap();
    let actual: Vec<_> = actual.lines().collect();
    assert_eq!(actual.len(), cases.len());
    let mut mismatches = Vec::new();
    for ((mime, expected), actual) in cases.iter().zip(actual) {
        if actual != format!(".{expected}") {
            mismatches.push((mime, expected, actual));
        }
    }
    assert!(
        mismatches.is_empty(),
        "Python MIME fallback mismatches: {mismatches:?}"
    );
}

#[test]
fn pinned_and_native_clients_correct_aac_hls_and_dash_signatures() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let pinned_root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("CI reference oracle must supply the pinned upstream directory");
    let commit = env::var("GALERITH_PINNED_REFERENCE_SHA")
        .expect("CI must declare expected upstream git revision");
    assert_eq!(commit, PINNED, "upstream pin changed without acceptance");

    const AAC: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Type: audio/aac\r\nContent-Length: 5\r\nConnection: close\r\n\r\n\xff\xf1AAC";
    const HLS: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Type: application/vnd.apple.mpegurl\r\nContent-Length: 25\r\nConnection: close\r\n\r\n#EXTM3U\n#EXT-X-VERSION:3\n";
    const DASH: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Type: application/dash+xml\r\nContent-Length: 11\r\nConnection: close\r\n\r\n<MPD></MPD>";
    const DASH_LATE: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Type: application/dash+xml\r\nContent-Length: 27\r\nConnection: close\r\n\r\n                <MPD></MPD>";
    const TIFF: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: 18\r\nConnection: close\r\n\r\nII*\0synthetic TIFF";
    const GZIP: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: 18\r\nConnection: close\r\n\r\n\x1f\x8b\x08\0synthetic gzip";
    const UNTYPED_HTML: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: 20\r\nConnection: close\r\n\r\n<html>Blocked</html>";
    const JSON: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 18\r\nConnection: close\r\n\r\n{\"error\":\"denied\"}";
    const PROBLEM_JSON: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Type: application/problem+json\r\nContent-Length: 18\r\nConnection: close\r\n\r\n{\"error\":\"denied\"}";
    const UNTYPED_JSON: &[u8] =
        b"HTTP/1.1 200 OK\r\nContent-Length: 18\r\nConnection: close\r\n\r\n{\"error\":\"denied\"}";
    let cases = [
        ("/asset.mp3", AAC, "mp3", "aac"),
        ("/asset.mp4", HLS, "mp4", "m3u8"),
        ("/asset.mp4", DASH, "mp4", "mpd"),
        ("/asset.mp4", DASH_LATE, "mp4", "mp4"),
        ("/asset.mp4", b"HTTP/1.1 200 OK\r\nContent-Type: video/quicktime\r\nContent-Length: 11\r\nConnection: close\r\n\r\n\0\0\0\x18ftypqt ", "mp4", "mp4"),
        ("/asset.jpg", TIFF, "jpg", "jpg"),
        ("/asset.jpg", GZIP, "jpg", "jpg"),
        ("/asset.jpg", b"HTTP/1.1 200 OK\r\nContent-Type: image/x-icon\r\nContent-Length: 4\r\nConnection: close\r\n\r\nICON", "jpg", "jpg"),
        ("/asset.jpg", UNTYPED_HTML, "jpg", "jpg"),
        ("/asset.jpg", JSON, "jpg", "jpg"),
        ("/asset.jpg", PROBLEM_JSON, "jpg", "jpg"),
        ("/asset.jpg", UNTYPED_JSON, "jpg", "jpg"),
    ];
    let root = env::temp_dir().join(format!(
        "galerith-pinned-media-signatures-oracle-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    for (index, (path, response, original_extension, expected_extension)) in
        cases.into_iter().enumerate()
    {
        let (url, server) = serve_retry_sequence_at(path, [response, response]);
        for (label, pinned) in [("reference", true), ("native", false)] {
            let output = root.join(format!("{label}-{index}"));
            std::fs::create_dir_all(&output).unwrap();
            let mut command = if pinned {
                let mut command = Command::new("python3");
                command.args(["-m", "gallery_dl", "--config-ignore", "--no-input"]);
                command.env("PYTHONPATH", &pinned_root);
                command.env("PYTHONNOUSERSITE", "1");
                command.env("PYTHONDONTWRITEBYTECODE", "1");
                command
            } else {
                Command::new(env!("CARGO_BIN_EXE_galerith"))
            };
            command
                .args([
                    "--quiet",
                    "-o",
                    "downloader.http.adjust-extensions=true",
                    "-d",
                ])
                .arg(&output)
                .arg(&url);
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
            let result = command
                .output()
                .expect("launch media-signature fixture client");
            assert!(
                result.status.success(),
                "{label}: {}",
                String::from_utf8_lossy(&result.stderr)
            );
            let filenames = saved_paths(&output)
                .into_iter()
                .filter_map(|path| {
                    path.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                })
                .collect::<Vec<_>>();
            assert!(
                filenames
                    .iter()
                    .any(|filename| filename.ends_with(&format!(".{expected_extension}"))),
                "{label}: expected .{expected_extension} output, found {filenames:?}"
            );
            if expected_extension != original_extension {
                assert!(
                    filenames
                        .iter()
                        .all(|filename| !filename.ends_with(&format!(".{original_extension}"))),
                    "{label}: original suffix remained after correction: {filenames:?}"
                );
            }
        }
        server.join().unwrap();
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn pinned_and_native_clients_adjust_generic_binary_html_signatures() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let pinned_root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("CI reference oracle must supply the pinned upstream directory");
    let commit = env::var("GALERITH_PINNED_REFERENCE_SHA")
        .expect("CI must declare expected upstream git revision");
    assert_eq!(commit, PINNED, "upstream pin changed without acceptance");

    const HTML: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: 56\r\nConnection: close\r\n\r\n<!DOCTYPE html><html><body>public document</body></html>";
    let root = env::temp_dir().join(format!(
        "galerith-pinned-html-signature-oracle-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let (url, server) = serve_retry_sequence_at("/document.jpg", [HTML, HTML]);

    for (label, pinned) in [("reference", true), ("native", false)] {
        let output = root.join(label);
        std::fs::create_dir_all(&output).unwrap();
        let mut command = if pinned {
            let mut command = Command::new("python3");
            command.args(["-m", "gallery_dl", "--config-ignore", "--no-input"]);
            command.env("PYTHONPATH", &pinned_root);
            command.env("PYTHONNOUSERSITE", "1");
            command.env("PYTHONDONTWRITEBYTECODE", "1");
            command
        } else {
            Command::new(env!("CARGO_BIN_EXE_galerith"))
        };
        command
            .args([
                "--quiet",
                "-o",
                "downloader.http.adjust-extensions=true",
                "-d",
            ])
            .arg(&output)
            .arg(&url);
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
        let result = command
            .output()
            .expect("launch generic-HTML signature fixture client");
        assert!(
            result.status.success(),
            "{label}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let filenames = saved_paths(&output)
            .into_iter()
            .filter_map(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .collect::<Vec<_>>();
        assert!(
            filenames.iter().any(|filename| filename.ends_with(".html")),
            "{label}: expected corrected HTML output, found {filenames:?}"
        );
        assert!(
            filenames.iter().all(|filename| !filename.ends_with(".jpg")),
            "{label}: original JPEG suffix remained: {filenames:?}"
        );
    }
    server.join().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn pinned_and_native_clients_reject_integer_length_filename_conversion() {
    let template = "{_lit[é猫]!n}";
    let url = "https://example.org/image.jpg";
    let native = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args(["--filename", template, "--simulate", url])
        .output()
        .unwrap();
    assert!(
        !native.status.success(),
        "native formatter accepted a non-string filename conversion"
    );

    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for integer length conversion comparison");
    let pinned = Command::new("python3")
        .args([
            "-m",
            "gallery_dl",
            "--config-ignore",
            "--no-input",
            "--filename",
            template,
            "--simulate",
            url,
        ])
        .env("PYTHONPATH", root)
        .env("PYTHONNOUSERSITE", "1")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .output()
        .unwrap();
    assert!(
        !pinned.status.success(),
        "pinned gallery-dl accepted a non-string filename conversion"
    );
}

#[test]
fn pinned_and_native_dump_json_match_with_filename_override() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for filename override comparison");
    let url = "https://example.org/photo.png";
    let options = [
        "--filename",
        "{category}_{filename}.{extension}",
        "--dump-json",
    ];
    let mut native_command = Command::new(env!("CARGO_BIN_EXE_galerith"));
    native_command.args(options).arg(url);
    let native: Value = serde_json::from_str(&spawn(
        native_command,
        "native --dump-json with filename override",
    ))
    .unwrap();
    let pinned = pinned_json_output(Path::new(&root), url, &options);
    assert_eq!(
        native, pinned,
        "filename override changed tuple output differently"
    );
}

#[test]
fn pinned_and_native_resolve_json_match_directlink() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for resolve-json comparison");
    let url = "https://cdn.example.org/galleries/photo%20one.jpg?size=large";
    let native = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args(["--resolve-json", url])
        .output()
        .unwrap();
    assert!(
        native.status.success(),
        "native --resolve-json failed: {}",
        String::from_utf8_lossy(&native.stderr)
    );
    let native: Value = serde_json::from_slice(&native.stdout).unwrap();
    let pinned = pinned_json_output(Path::new(&root), url, &["--resolve-json"]);
    assert_eq!(
        native, pinned,
        "directlink resolve-json differs from pinned"
    );
}

#[test]
fn pinned_and_native_resolve_urls_match_directlink() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for resolve-urls comparison");
    let url = "https://cdn.example.org/galleries/photo%20one.jpg?size=large";
    let native = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args(["--resolve-urls", url])
        .output()
        .unwrap();
    assert!(
        native.status.success(),
        "native --resolve-urls failed: {}",
        String::from_utf8_lossy(&native.stderr)
    );
    assert_eq!(
        String::from_utf8(native.stdout).unwrap(),
        pinned_resolve_urls(Path::new(&root), url),
        "directlink resolve-urls output differs from pinned gallery-dl"
    );
}

#[test]
fn pinned_and_native_info_json_match_directlink_directory_metadata() {
    let root = env::temp_dir().join(format!(
        "galerith-info-json-reference-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let native_dir = root.join("native");
    std::fs::create_dir_all(&native_dir).unwrap();
    let url = "https://cdn.example.org/galleries/asset.jpg";
    let native = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args([
            "--write-info-json",
            "--no-download",
            "-d",
            native_dir.to_str().unwrap(),
            url,
        ])
        .output()
        .unwrap();
    assert!(
        native.status.success(),
        "native info-json failed: {}",
        String::from_utf8_lossy(&native.stderr)
    );
    let native_metadata: Value = serde_json::from_slice(
        &std::fs::read(native_dir.join("info.json")).expect("native info.json created"),
    )
    .unwrap();

    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        std::fs::remove_dir_all(root).unwrap();
        return;
    }
    let pinned_root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for info-json comparison");
    let pinned_dir = root.join("pinned");
    std::fs::create_dir_all(&pinned_dir).unwrap();
    let pinned = Command::new("python3")
        .args([
            "-m",
            "gallery_dl",
            "--config-ignore",
            "--no-input",
            "--write-info-json",
            "--no-download",
            "-D",
            pinned_dir.to_str().unwrap(),
            url,
        ])
        .env("PYTHONPATH", pinned_root)
        .env("PYTHONNOUSERSITE", "1")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .output()
        .unwrap();
    assert!(
        pinned.status.success(),
        "pinned info-json failed: {}",
        String::from_utf8_lossy(&pinned.stderr)
    );
    let pinned_metadata: Value = serde_json::from_slice(
        &std::fs::read(pinned_dir.join("info.json")).expect("pinned info.json created"),
    )
    .unwrap();
    for key in [
        "category",
        "filename",
        "domain",
        "subcategory",
        "path",
        "extension",
    ] {
        assert_eq!(
            native_metadata.get(key),
            pinned_metadata.get(key),
            "directlink info.json field {key} differs"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn pinned_and_native_clients_reject_empty_literal_filename_fields() {
    let template = "{_lit[]}";
    let url = "https://example.org/image.jpg";
    let native = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args(["--filename", template, "--simulate", url])
        .output()
        .unwrap();
    assert!(!native.status.success(), "native accepted an empty literal");

    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("CI reference oracle must supply the pinned upstream directory");
    let commit = env::var("GALERITH_PINNED_REFERENCE_SHA")
        .expect("CI must declare expected upstream git revision");
    assert_eq!(commit, PINNED, "upstream pin changed without acceptance");
    let pinned = Command::new("python3")
        .args([
            "-m",
            "gallery_dl",
            "--config-ignore",
            "--no-input",
            "--filename",
            template,
            "--simulate",
            url,
        ])
        .env("PYTHONPATH", Path::new(&root))
        .env("PYTHONNOUSERSITE", "1")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .output()
        .unwrap();
    assert!(
        !pinned.status.success(),
        "pinned gallery-dl accepted an empty literal"
    );
}

#[test]
fn pinned_and_native_clients_match_downloader_http_verify_false() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let pinned_root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("CI reference oracle must supply the pinned upstream directory");
    let pinned_root = Path::new(&pinned_root);
    let commit = env::var("GALERITH_PINNED_REFERENCE_SHA")
        .expect("CI must declare expected upstream git revision");
    assert_eq!(commit, PINNED, "upstream pin changed without acceptance");

    let root = env::temp_dir().join(format!("galerith-verify-oracle-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let server_root = root.join("server");
    std::fs::create_dir_all(&server_root).unwrap();
    std::fs::write(server_root.join("asset.jpg"), b"TLS!").unwrap();
    let address_probe = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = address_probe.local_addr().unwrap();
    drop(address_probe);

    let certificates = tls::create_ca_signed_server_certificate(&root.join("certificates"));
    let mut server = Command::new("openssl")
        .args([
            "s_server",
            "-quiet",
            "-WWW",
            "-naccept",
            "11",
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
        .expect("start local TLS fixture");

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
    let ca_certificates =
        reqwest::Certificate::from_pem_bundle(&std::fs::read(&certificates.ca).unwrap()).unwrap();
    let mut direct_client = reqwest::Client::builder();
    for certificate in ca_certificates {
        direct_client = direct_client.add_root_certificate(certificate);
    }
    let direct_client = direct_client.build().unwrap();
    let direct_runtime = tokio::runtime::Runtime::new().unwrap();
    let direct_response = direct_runtime
        .block_on(direct_client.get(&url).send())
        .unwrap_or_else(|error| panic!("direct CA trust probe failed: {error:?}"));
    assert_eq!(
        direct_runtime
            .block_on(direct_response.bytes())
            .unwrap()
            .as_ref(),
        b"TLS!"
    );
    let ca_option = format!(
        "downloader.http.verify={}",
        serde_json::to_string(certificates.ca.to_str().unwrap()).unwrap()
    );
    let extractor_ca_option = format!(
        "extractor.directlink.verify={}",
        serde_json::to_string(certificates.ca.to_str().unwrap()).unwrap()
    );
    let run = |label: &str, reference: bool, verification: &str| {
        let output = root.join(label);
        std::fs::create_dir_all(&output).unwrap();
        let mut command = if reference {
            let mut command = Command::new("python3");
            command.args([
                "-m",
                "gallery_dl",
                "--config-ignore",
                "--no-input",
                "--quiet",
                "-o",
                verification,
                "-R",
                "0",
                "-d",
                output.to_str().unwrap(),
                &url,
            ]);
            command.env("PYTHONPATH", pinned_root);
            command.env("PYTHONNOUSERSITE", "1");
            command.env("PYTHONDONTWRITEBYTECODE", "1");
            command
        } else {
            let mut command = Command::new(env!("CARGO_BIN_EXE_galerith"));
            command.args([
                "--quiet",
                "-o",
                verification,
                "--retries",
                "0",
                "-d",
                output.to_str().unwrap(),
                &url,
            ]);
            command
        };
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
        command.output().expect("launch TLS fixture client")
    };
    let pinned_disabled = run("pinned-disabled", true, "downloader.http.verify=false");
    let native_disabled = run("native-disabled", false, "downloader.http.verify=false");
    let pinned_ca_file = run("pinned-ca-file", true, &ca_option);
    let native_ca_file = run("native-ca-file", false, &ca_option);
    let pinned_extractor_disabled = run(
        "pinned-extractor-disabled",
        true,
        "extractor.directlink.verify=false",
    );
    let native_extractor_disabled = run(
        "native-extractor-disabled",
        false,
        "extractor.directlink.verify=false",
    );
    let pinned_extractor_ca_file = run("pinned-extractor-ca-file", true, &extractor_ca_option);
    let native_extractor_ca_file = run("native-extractor-ca-file", false, &extractor_ca_option);
    let _ = server.kill();
    let _ = server.wait();

    for (label, output) in [
        ("pinned-disabled", pinned_disabled),
        ("native-disabled", native_disabled),
        ("pinned-ca-file", pinned_ca_file),
        ("native-ca-file", native_ca_file),
        ("pinned-extractor-disabled", pinned_extractor_disabled),
        ("native-extractor-disabled", native_extractor_disabled),
        ("pinned-extractor-ca-file", pinned_extractor_ca_file),
        ("native-extractor-ca-file", native_extractor_ca_file),
    ] {
        assert!(
            output.status.success(),
            "{label}: client failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        if label.starts_with("pinned") {
            assert!(
                contains_file_with_bytes(&root.join(label), b"TLS!"),
                "{label}: client did not save the TLS fixture body"
            );
        } else {
            assert_eq!(
                std::fs::read(root.join(label).join("asset.jpg")).unwrap(),
                b"TLS!",
                "{label}: client did not save the TLS fixture body"
            );
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn pinned_and_native_clients_do_not_retry_429_by_default() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let pinned_root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("CI reference oracle must supply the pinned upstream directory");
    let pinned_root = Path::new(&pinned_root);
    let commit = env::var("GALERITH_PINNED_REFERENCE_SHA")
        .expect("CI must declare expected upstream git revision");
    assert_eq!(commit, PINNED, "upstream pin changed without acceptance");
    let root = env::temp_dir().join(format!(
        "galerith-429-default-oracle-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    run_429_default_fixture(&root, pinned_root, "pinned", true);
    run_429_default_fixture(&root, pinned_root, "native", false);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn pinned_philomena_archive_format_is_the_image_id() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for archive differential");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "from gallery_dl.extractor.philomena import PhilomenaExtractor; print(PhilomenaExtractor.archive_fmt)",
    ]);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(command, "pinned Philomena archive template").trim(),
        "{id}"
    );
}

#[test]
fn pinned_danbooru_e621_post_archive_templates_differ_from_feeds() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for archive differential");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "from gallery_dl.extractor.danbooru import DanbooruPostExtractor, DanbooruTagExtractor, DanbooruPoolExtractor; from gallery_dl.extractor.e621 import E621PostExtractor, E621TagExtractor, E621FavoriteExtractor; print('|'.join([DanbooruPostExtractor.archive_fmt, DanbooruTagExtractor.archive_fmt, DanbooruPoolExtractor.archive_fmt, E621PostExtractor.archive_fmt, E621TagExtractor.archive_fmt, E621FavoriteExtractor.archive_fmt]))",
    ]);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(command, "pinned Danbooru/E621 archive templates").trim(),
        "{id}|t_{search_tags}_{id}|p_{pool[id]}_{id}|{id}|t_{search_tags}_{id}|f_{user_id}_{id}"
    );
}

#[test]
fn pinned_direct_post_categories_match_source_sites() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for category differential");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "from gallery_dl import extractor; urls=('https://danbooru.donmai.us/posts/123', 'https://e621.net/posts/234', 'https://e926.net/posts/345', 'https://e6ai.net/posts/456'); print('|'.join(f'{ex.category}:{ex.archive_fmt}' for ex in (extractor.find(url) for url in urls)))",
    ]);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(command, "pinned source site categories").trim(),
        "danbooru:{id}|e621:{id}|e926:{id}|e6ai:{id}"
    );
}

#[test]
fn pinned_danbooru_and_e621_pool_archive_templates_are_pool_scoped() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root =
        env::var("GALERITH_PINNED_REFERENCE_ROOT").expect("pinned reference source is required");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "from gallery_dl.extractor.danbooru import DanbooruPoolExtractor,DanbooruPostExtractor; from gallery_dl.extractor.e621 import E621PoolExtractor,E621PostExtractor; print('|'.join((DanbooruPoolExtractor.archive_fmt,DanbooruPostExtractor.archive_fmt,E621PoolExtractor.archive_fmt,E621PostExtractor.archive_fmt)))",
    ]);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(command, "pinned pool archive templates").trim(),
        "p_{pool[id]}_{id}|{id}|p_{pool[id]}_{id}|{id}"
    );
}

#[test]
fn pinned_danbooru_favorite_group_template_is_not_a_post_or_pool() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT").expect("pinned source required");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "from gallery_dl.extractor.danbooru import DanbooruFavgroupExtractor,DanbooruPoolExtractor,DanbooruPostExtractor; print('|'.join((DanbooruFavgroupExtractor.archive_fmt,DanbooruPoolExtractor.archive_fmt,DanbooruPostExtractor.archive_fmt)))",
    ]);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(command, "pinned favorite group template").trim(),
        "fg_{favgroup[id]}_{id}|p_{pool[id]}_{id}|{id}"
    );
}

#[test]
fn pinned_e621_family_favorite_archive_template_includes_user_id() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for e621 favorite template");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "from gallery_dl.extractor.e621 import E621FavoriteExtractor,E621PostExtractor,E621PoolExtractor; print('|'.join((E621FavoriteExtractor.archive_fmt,E621PostExtractor.archive_fmt,E621PoolExtractor.archive_fmt)))",
    ]);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(command, "pinned E621 favorite archive template").trim(),
        "f_{user_id}_{id}|{id}|p_{pool[id]}_{id}"
    );
}

#[test]
fn pinned_tag_extractors_use_full_query_in_archive_id() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for tag archive oracle");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "from gallery_dl.extractor.danbooru import DanbooruTagExtractor,DanbooruPostExtractor; from gallery_dl.extractor.e621 import E621TagExtractor; print('|'.join((DanbooruTagExtractor.archive_fmt,DanbooruPostExtractor.archive_fmt,E621TagExtractor.archive_fmt)))",
    ]);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(command, "pinned tag archive templates").trim(),
        "t_{search_tags}_{id}|{id}|t_{search_tags}_{id}"
    );
}

#[test]
fn pinned_danbooru_media_asset_id_is_distinct_from_post_id() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for media-asset ID oracle");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "from gallery_dl.extractor.danbooru import DanbooruMediaassetExtractor,DanbooruPostExtractor; print('|'.join((DanbooruMediaassetExtractor.archive_fmt,DanbooruPostExtractor.archive_fmt)))",
    ]);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(command, "pinned media asset archive template").trim(),
        "m{id}|{id}"
    );
}

#[test]
fn pinned_dated_popular_archive_matches_runtime_metadata_formatting() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for popular metadata oracle");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "from gallery_dl import extractor; urls=('https://danbooru.donmai.us/explore/posts/popular?scale=day&date=2026-10-25','https://e621.net/explore/posts/popular?scale=month&date=2026-10-25','https://e926.net/explore/posts/popular?scale=day&date=2026-10-25','https://e6ai.net/explore/posts/popular?scale=month&date=2026-10-25','https://danbooru.donmai.us/explore/posts/popular?scale=week&date=2026-10-25','https://e621.net/explore/posts/popular?scale=week&date=2027-01-01','https://e926.net/explore/posts/popular?scale=week&date=2024-03-01','https://e6ai.net/explore/posts/popular?scale=week&date=2026-10-25'); e=[extractor.find(url) for url in urls]; print('|'.join(x.category+x.archive_fmt.format(id=88,**x.metadata()) for x in e))",
    ]);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(command, "pinned dated popular archive metadata").trim(),
        "danbooruP_d_2026-10-25_88|e621P_m_2026-10_88|e926P_d_2026-10-25_88|e6aiP_m_2026-10_88|danbooruP_w_2026-10-19_88|e621P_w_2026-12-28_88|e926P_w_2024-02-26_88|e6aiP_w_2026-10-19_88"
    );
}

#[test]
fn pinned_moebooru_post_pool_and_tag_archive_templates() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for Moebooru archive oracle");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "from gallery_dl import extractor; hosts=('yande.re','konachan.net','www.sakugabooru.com','lolibooru.moe'); urls=[('https://'+host+route,kw) for host in hosts for route,kw in (('/post/show/88',{'id':88}),('/pool/show/77',{'pool':77,'id':88}),('/post?tags=blue+fox',{'search_tags':'blue fox','id':88}))]; print('|'.join(x.category+x.archive_fmt.format(**kw) for url,kw in urls for x in (extractor.find(url),)))",
    ]);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(command, "pinned Moebooru archive class formats").trim(),
        "yandere88|yanderep_77_88|yanderet_blue fox_88|konachan88|konachanp_77_88|konachant_blue fox_88|sakugabooru88|sakugaboorup_77_88|sakugaboorut_blue fox_88|lolibooru88|loliboorup_77_88|loliboorut_blue fox_88"
    );
}

#[test]
fn pinned_gelbooru_family_post_and_tag_archive_templates() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for Gelbooru archive oracle");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "from gallery_dl import extractor; hosts=('gelbooru.com','rule34.xxx','safebooru.org','tbib.org','hypnohub.net','xbooru.com'); urls=[('https://'+host+'/index.php?page=post&s='+route,kw) for host in hosts for route,kw in (('view&id=88',{'id':88}),('list&tags=blue+fox',{'search_tags':'blue fox','id':88}))]; print('|'.join(x.category+x.archive_fmt.format(**kw) for url,kw in urls for x in (extractor.find(url),)))",
    ]);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(command, "pinned Gelbooru site-specific archive formats").trim(),
        "gelbooru88|gelboorut_blue fox_88|rule3488|rule34t_blue fox_88|safebooru88|safeboorut_blue fox_88|tbib88|tbibt_blue fox_88|hypnohub88|hypnohubt_blue fox_88|xbooru88|xboorut_blue fox_88"
    );
}

#[test]
fn pinned_wallhaven_archive_classes_are_source_scoped() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for Wallhaven archive ID oracle");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "from gallery_dl import extractor; urls=('https://wallhaven.cc/w/94x38z','https://wallhaven.cc/search?q=blue+sky','https://wallhaven.cc/user/example-1/uploads','https://wallhaven.cc/user/example-1/favorites/77'); meta=({'id':'94x38z'},{'id':'94x38z','search':{'q':'blue sky'}},{'id':'94x38z','username':'example-1'},{'id':'94x38z'}); print('|'.join(x.category+x.archive_fmt.format(**m) for x,m in zip((extractor.find(url) for url in urls),meta)))",
    ]);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(command, "pinned Wallhaven archive classes").trim(),
        "wallhaven94x38z|wallhavens_blue sky_94x38z|wallhavenu_example-1_94x38z|wallhaven94x38z"
    );
}

#[test]
fn pinned_fourchan_archive_format_is_board_thread_media_tim() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for 4chan archive differential");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "from gallery_dl import extractor; urls=('https://boards.4chan.org/a/thread/123','https://boards.4channel.org/b/thread/124'); print('|'.join(ex.category+ex.archive_fmt.format(board=ex.board,thread=ex.thread,tim=456) for ex in (extractor.find(url) for url in urls)))",
    ]);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(command, "pinned 4chan board/thread/tim archive template").trim(),
        "4chana_123_456|4chanb_124_456"
    );
}

#[test]
fn pinned_archived_fourchan_formats_are_board_thread_post_number() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for archived 4chan differential");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "from gallery_dl import extractor; urls=('https://4archive.org/board/g/thread/1002','https://4chanarchives.com/board/a/thread/2002'); print('|'.join((lambda ex: ex.category+ex.archive_fmt.format(board=board,thread=thread,no=no))(extractor.find(url)) for url,board,thread,no in zip(urls,('g','a'),('1002','2002'),('1003','2003'))))",
    ]);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(
            command,
            "pinned archived 4chan board/thread/post archive templates"
        )
        .trim(),
        "4archiveg_1002_1003|4chanarchivesa_2002_2003"
    );
}

#[test]
fn pinned_twochan_archive_format_is_board_thread_file_tim() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for 2chan archive differential");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "from gallery_dl import extractor; urls=('https://dec.2chan.net/12/res/12345.htm','https://img.2chan.net/b/res/54321.htm'); print('|'.join((lambda ex: ex.category+ex.archive_fmt.format(board=ex.board,thread=ex.thread,tim=178000001))(extractor.find(url)) for url in urls))",
    ]);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(command, "pinned 2chan board/thread/tim archive template").trim(),
        "2chan12_12345_178000001|2chanb_54321_178000001"
    );
}

#[test]
fn pinned_twoch_archive_format_is_board_thread_file_tim() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for 2ch archive differential");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "from gallery_dl import extractor; urls=('https://2ch.hk/a/res/12345.html','https://2ch.su/b/res/54321.html'); print('|'.join((lambda ex: ex.category+ex.archive_fmt.format(board=board,thread=thread,tim=157000001))(extractor.find(url)) for url,board,thread in zip(urls,('a','b'),('12345','54321'))))",
    ]);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(command, "pinned 2ch board/thread/tim archive template").trim(),
        "2cha_12345_157000001|2chb_54321_157000001"
    );
}

#[test]
fn pinned_imgur_image_and_album_archive_ids_are_distinct() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for Imgur archive differential");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "from gallery_dl import extractor; sources=('https://imgur.com/abcdefg','https://imgur.com/a/abcde'); args=({'id':'abcdefg'},{'album':{'id':'abcde'},'id':'abcdefg'}); print('|'.join(ex.category+ex.archive_fmt.format(**kw) for ex,kw in zip((extractor.find(url) for url in sources),args)))",
    ]);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(command, "pinned Imgur image/album archive templates").trim(),
        "imgurabcdefg|imgurabcde_abcdefg"
    );
}

#[test]
fn pinned_500px_archive_uses_photo_id_for_single_images() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for 500px archive differential");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "from gallery_dl import extractor; urls=('https://500px.com/photo/12345/Golden-Landscape','https://web.500px.com/photo/12346'); print('|'.join(ex.category+ex.archive_fmt.format(id=photo) for ex,photo in zip((extractor.find(url) for url in urls),(12345,12346))))",
    ]);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(command, "pinned 500px image archive format").trim(),
        "500px12345|500px12346"
    );
}

#[test]
fn pinned_photovogue_archive_is_photo_id_across_photographers() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for PhotoVogue archive differential");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "from gallery_dl import extractor; urls=('https://www.vogue.com/photovogue/photographers/12345','https://vogue.com/photovogue/photographers/67890'); print('|'.join(ex.category+ex.archive_fmt.format(id=145) for ex in (extractor.find(url) for url in urls)))",
    ]);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(command, "pinned PhotoVogue photo archive format").trim(),
        "photovogue145|photovogue145"
    );
}

#[test]
fn pinned_fapachi_post_archive_keys_include_username() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for Fapachi archive differential");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "from gallery_dl import extractor; urls=('https://fapachi.com/model/media/12345','https://fapachi.com/another/media/12345'); print('|'.join(ex.category+ex.archive_fmt.format(user=ex.user,id=ex.id) for ex in (extractor.find(url) for url in urls)))",
    ]);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(command, "pinned Fapachi source-scoped archive format").trim(),
        "fapachimodel_12345|fapachianother_12345"
    );
}

#[test]
fn pinned_fapello_post_archive_separates_type_model_and_id() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for Fapello archive differential");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "from gallery_dl import extractor; urls=('https://fapello.com/model/12345/','https://fapello.su/other/12345/'); a,b=(extractor.find(url) for url in urls); print('|'.join((a.category+a.archive_fmt.format(type=kind,model=a.model,id=12345)) for kind in ('photo','video'))+'|'+b.category+b.archive_fmt.format(type='photo',model=b.model,id=12345))",
    ]);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(command, "pinned Fapello post archive format").trim(),
        "fapellophoto_model_12345|fapellovideo_model_12345|fapellophoto_other_12345"
    );
}

#[test]
fn pinned_lightroom_gallery_archive_uses_global_asset_ids() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned reference required for Lightroom archive differential");
    let mut cmd = Command::new("python3");
    cmd.args([
        "-c",
        "from gallery_dl import extractor; x=extractor.find('https://lightroom.adobe.com/shares/012345abcdef'); print(x.category+'|'+x.subcategory+'|'+x.archive_fmt.format(id='asset-A')+'|'+x.archive_fmt.format(id='asset-B'))",
    ]);
    cmd.env("PYTHONPATH", root);
    cmd.env("PYTHONNOUSERSITE", "1");
    cmd.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(cmd, "pinned Lightroom asset archive format").trim(),
        "lightroom|gallery|asset-A|asset-B"
    );
}

#[test]
fn pinned_foriio_multi_original_archive_ids() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned reference required for Foriio archive differential");
    let mut cmd = Command::new("python3");
    cmd.args([
        "-c",
        "from gallery_dl import extractor; x=extractor.find('https://foriio.com/works/12345'); print(x.category+'|'+x.subcategory+'|'+x.archive_fmt.format(work_id=12345,file_id='file-A',num=1)+'|'+x.archive_fmt.format(work_id=12345,file_id='file-B',num=2)+'|'+x.archive_fmt.format(work_id=12345,file_id='file-A',num=2))",
    ]);
    cmd.env("PYTHONPATH", root);
    cmd.env("PYTHONNOUSERSITE", "1");
    cmd.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(cmd, "pinned Foriio multi-original archive format").trim(),
        "foriio|work|12345/file-A/1|12345/file-B/2|12345/file-A/2"
    );
}

#[test]
fn pinned_piczel_image_and_folder_archive_templates() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned reference required for Piczel archive differential");
    let mut cmd = Command::new("python3");
    cmd.args([
        "-c",
        "from gallery_dl import extractor; u=extractor.find('https://piczel.tv/gallery/artist'); i=extractor.find('https://piczel.tv/gallery/image/1001'); f=extractor.find('https://piczel.tv/gallery/artist/5'); print('|'.join((u.subcategory,i.subcategory,f.subcategory,u.archive_fmt.format(id=1001,num=0),i.archive_fmt.format(id=1001,num=1),f.archive_fmt.format(folder={'id':5},id=1001,num=0))))",
    ]);
    cmd.env("PYTHONPATH", root);
    cmd.env("PYTHONNOUSERSITE", "1");
    cmd.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(cmd, "pinned Piczel image versus folder archive templates").trim(),
        "user|image|folder|1001_0|1001_1|f5_1001_0"
    );
}

#[test]
fn pinned_thirtyfivephoto_series_archive_uses_one_based_position() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned gallery-dl reference required for 35photo archive differential");
    let mut cmd = Command::new("python3");
    cmd.args([
        "-c",
        "from gallery_dl import extractor; a=extractor.find('https://35photo.pro/photo_12345/'); t=extractor.find('https://35photo.pro/tags/flower/'); g=extractor.find('https://35photo.pro/genre_10/'); print('|'.join((a.category+a.archive_fmt.format(id=12345,num=n) for n in (1,2)))+'|'+t.category+t.archive_fmt.format(search_tag='flower',id=12345,num=1)+'|'+g.category+g.archive_fmt.format(genre_id=10,id=12345,num=1))",
    ]);
    cmd.env("PYTHONPATH", root);
    cmd.env("PYTHONNOUSERSITE", "1");
    cmd.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(cmd, "pinned 35photo series/tag/genre archive formats").trim(),
        "35photo12345_1|35photo12345_2|35phototflower_12345_1|35photog10_12345_1"
    );
}

#[test]
fn pinned_unsplash_archive_uses_one_global_photo_id_across_extractor_families() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned gallery-dl source required for Unsplash archive differential");
    let mut cmd = Command::new("python3");
    cmd.args([
        "-c",
        "from gallery_dl import extractor; urls=('https://unsplash.com/photos/abc-A','https://unsplash.com/@alice','https://unsplash.com/@alice/likes','https://unsplash.com/collections/12345/flowers','https://unsplash.com/s/photos/sky'); items=[extractor.find(u) for u in urls]; print('|'.join(x.category+x.archive_fmt.format(id='abc-A') for x in items))",
    ]);
    cmd.env("PYTHONPATH", root);
    cmd.env("PYTHONNOUSERSITE", "1");
    cmd.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(cmd, "pinned Unsplash cross-source archive identity").trim(),
        "unsplashabc-A|unsplashabc-A|unsplashabc-A|unsplashabc-A|unsplashabc-A"
    );
}

#[test]
fn pinned_lexica_search_archive_uses_api_image_id() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned gallery-dl reference required for Lexica archive differential");
    let mut cmd = Command::new("python3");
    cmd.args([
        "-c",
        "from gallery_dl import extractor; a=extractor.find('https://lexica.art/?q=red+fox'); b=extractor.find('https://lexica.art/?q=blue+fox'); print(a.category+'|'+a.subcategory+'|'+a.archive_fmt.format(id='abc-A')+'|'+b.archive_fmt.format(id='abc-A'))",
    ]);
    cmd.env("PYTHONPATH", root);
    cmd.env("PYTHONNOUSERSITE", "1");
    cmd.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(cmd, "pinned Lexica image archive format").trim(),
        "lexica|search|abc-A|abc-A"
    );
}

#[test]
fn pinned_pixeldrain_file_and_album_archive_share_file_ids() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned gallery-dl source required for Pixeldrain archive differential");
    let mut cmd = Command::new("python3");
    cmd.args([
        "-c",
        "from gallery_dl import extractor; a=extractor.find('https://pixeldrain.com/u/File12'); b=extractor.find('https://pixeldrain.com/l/album_99'); c=extractor.find('https://pixeldrain.com/d/folder'); print(a.category+'|'+a.archive_fmt.format(id='File12')+'|'+b.archive_fmt.format(id='File12')+'|'+c.archive_fmt.format(path='/folder/item',num=1))",
    ]);
    cmd.env("PYTHONPATH", root);
    cmd.env("PYTHONNOUSERSITE", "1");
    cmd.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(cmd, "pinned Pixeldrain file/album/folder archive formats").trim(),
        "pixeldrain|File12|File12|/folder/item_1"
    );
}

#[test]
fn pinned_listal_image_archive_includes_original_filename() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned gallery-dl reference required for Listal archive differential");
    let mut cmd = Command::new("python3");
    cmd.args([
        "-c",
        "from gallery_dl import extractor; a=extractor.find('https://www.listal.com/viewimage/12345'); b=extractor.find('https://listal.com/alice/pictures'); print(a.category+'|'+a.archive_fmt.format(id=12345,filename='full')+'|'+b.archive_fmt.format(id=12345,filename='other'))",
    ]);
    cmd.env("PYTHONPATH", root);
    cmd.env("PYTHONNOUSERSITE", "1");
    cmd.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(cmd, "pinned Listal original filename archive format").trim(),
        "listal|12345/full|12345/other"
    );
}

#[test]
fn pinned_lensdump_image_and_album_archive_formats_are_distinct() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for Lensdump archive differential");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "from gallery_dl import extractor; a=extractor.find('https://lensdump.com/i/AbCd12'); b=extractor.find('https://lensdump.com/a/Album88'); print('|'.join((a.category,a.subcategory,a.archive_fmt.format(id='AbCd12'),b.category,b.subcategory,b.archive_fmt.format(gallery_id='Album88',num=1))))",
    ]);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(command, "pinned Lensdump image and album archive formats").trim(),
        "lensdump|image|AbCd12|lensdump|album|Album88_1"
    );
}

#[test]
fn pinned_webmshare_source_video_archive_is_id_not_title() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for Webmshare archive differential");
    let mut cmd = Command::new("python3");
    cmd.args([
        "-c",
        "from gallery_dl import extractor; a=extractor.find('https://webmshare.com/play/ABc_123'); b=extractor.find('https://s4.webmshare.com/download-webm/video99'); print('|'.join((a.category,a.subcategory,a.archive_fmt.format(id='ABc_123'),b.archive_fmt.format(id='video99'))))",
    ]);
    cmd.env("PYTHONPATH", root);
    cmd.env("PYTHONNOUSERSITE", "1");
    cmd.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(cmd, "pinned Webmshare video identity").trim(),
        "webmshare|video|ABc_123|video99"
    );
}

#[test]
fn pinned_catbox_album_and_file_archive_formats_are_disjoint() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for Catbox archive differential");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "from gallery_dl import extractor; a=extractor.find('https://files.catbox.moe/abc123.jpg'); b=extractor.find('https://catbox.moe/c/Album123'); print('|'.join((a.category,a.subcategory,a.archive_fmt.format(filename='abc123'),b.category,b.subcategory,b.archive_fmt.format(album_id='Album123',filename='abc123'))))",
    ]);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(command, "pinned Catbox source-scoped archive formats").trim(),
        "catbox|file|abc123|catbox|album|Album123_abc123"
    );
}

#[test]
fn pinned_slideshare_presentation_archive_uses_slug_and_page() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for Slideshare archive differential");
    let mut cmd = Command::new("python3");
    cmd.args([
        "-c",
        "from gallery_dl import extractor; x=extractor.find('https://www.slideshare.net/speaker/my-deck'); print('|'.join((x.category,x.subcategory,x.archive_fmt.format(presentation='my-deck',num=1),x.archive_fmt.format(presentation='my-deck',num=2))))",
    ]);
    cmd.env("PYTHONPATH", root);
    cmd.env("PYTHONNOUSERSITE", "1");
    cmd.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(cmd, "pinned Slideshare presentation-page archive format").trim(),
        "slideshare|presentation|my-deck_1|my-deck_2"
    );
}

#[test]
fn pinned_issuu_publication_page_archive_keys() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for Issuu archive differential");
    let mut cmd = Command::new("python3");
    cmd.args([
        "-c",
        "from gallery_dl import extractor; x=extractor.find('https://issuu.com/press/docs/fall-2026'); print('|'.join((x.category,x.subcategory,x.archive_fmt.format(document={'publicationId':'pub34'},num=1),x.archive_fmt.format(document={'publicationId':'pub34'},num=2))))",
    ]);
    cmd.env("PYTHONPATH", root);
    cmd.env("PYTHONNOUSERSITE", "1");
    cmd.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(cmd, "pinned Issuu publication-page archive format").trim(),
        "issuu|publication|pub34_1|pub34_2"
    );
}

#[test]
fn pinned_speakerdeck_presentation_archive_uses_page_position() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned reference required for Speaker Deck archive differential");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "from gallery_dl import extractor; x=extractor.find('https://speakerdeck.com/author/my-deck'); print('|'.join((x.category,x.subcategory,x.archive_fmt.format(presentation='my-deck',num=1),x.archive_fmt.format(presentation='my-deck',num=2))))",
    ]);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(command, "pinned Speaker Deck per-slide archive format").trim(),
        "speakerdeck|presentation|my-deck_1|my-deck_2"
    );
}

#[test]
fn pinned_imgbox_single_and_gallery_archive_keys() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned reference required for Imgbox archive differential");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "from gallery_dl import extractor; a=extractor.find('https://imgbox.com/a1B2c3D4'); b=extractor.find('https://imgbox.com/g/abcdefghij'); print('|'.join((a.category,a.subcategory,a.archive_fmt.format(image_key='a1B2c3D4'),b.category,b.subcategory,b.archive_fmt.format(gallery_key='abcdefghij',image_key='a1B2c3D4'))))",
    ]);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(command, "pinned Imgbox image and gallery archive keys").trim(),
        "imgbox|image|a1B2c3D4|imgbox|gallery|abcdefghij_a1B2c3D4"
    );
}

#[test]
fn pinned_imagebam_single_and_gallery_keys_are_distinct() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned reference required for ImageBam archive differential");
    let mut cmd = Command::new("python3");
    cmd.args([
        "-c",
        "from gallery_dl import extractor; a=extractor.find('https://www.imagebam.com/view/Mabc123'); b=extractor.find('https://www.imagebam.com/view/Gabc123'); print('|'.join((a.category,a.subcategory,a.archive_fmt.format(image_key='Mabc123'),b.category,b.subcategory,b.archive_fmt.format(gallery_key='Gabc123',image_key='Mabc123'))))",
    ]);
    cmd.env("PYTHONPATH", root);
    cmd.env("PYTHONNOUSERSITE", "1");
    cmd.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(cmd, "pinned ImageBam single/gallery source IDs").trim(),
        "imagebam|image|Mabc123|imagebam|gallery|Gabc123_Mabc123"
    );
}

#[test]
fn pinned_postimg_image_tokens_and_gallery_queue_are_distinct() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned reference required for Postimages archive differential");
    let mut cmd = Command::new("python3");
    cmd.args([
        "-c",
        "from gallery_dl import extractor; a=extractor.find('https://postimg.cc/PhJZt1Rw'); b=extractor.find('https://postimg.cc/image/PhJZt1Rw'); g=extractor.find('https://postimg.cc/gallery/wxpDLgX'); print('|'.join((a.category,a.subcategory,a.archive_fmt.format(token='PhJZt1Rw'),b.subcategory,b.archive_fmt.format(token='PhJZt1Rw'),g.subcategory)))",
    ]);
    cmd.env("PYTHONPATH", root);
    cmd.env("PYTHONNOUSERSITE", "1");
    cmd.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(
            cmd,
            "pinned Postimages image-token and gallery queue archive keys"
        )
        .trim(),
        "postimg|image|PhJZt1Rw|image|PhJZt1Rw|gallery"
    );
}

#[test]
fn pinned_imgpile_post_slug_and_media_ids_are_independent() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned reference required for Imgpile archive differential");
    let mut cmd = Command::new("python3");
    cmd.args([
        "-c",
        "from gallery_dl import extractor; p=extractor.find('https://imgpile.com/p/AbCdEfG'); u=extractor.find('https://imgpile.com/u/creator'); print('|'.join((p.category,p.subcategory,p.archive_fmt.format(post={'id_slug':'AbCdEfG'},id=17),p.archive_fmt.format(post={'id_slug':'AbCdEfG'},id=18),u.subcategory)))",
    ]);
    cmd.env("PYTHONPATH", root);
    cmd.env("PYTHONNOUSERSITE", "1");
    cmd.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(cmd, "pinned Imgpile post and individual media IDs").trim(),
        "imgpile|post|AbCdEfG_17|AbCdEfG_18|user"
    );
}

#[test]
fn pinned_imagepond_file_ids_and_album_queue_are_distinct() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned reference required for ImagePond archive differential");
    let mut cmd = Command::new("python3");
    cmd.args([
        "-c",
        "from gallery_dl import extractor; a=extractor.find('https://www.imagepond.net/i/file.A7'); b=extractor.find('https://www.imagepond.net/video/clip.B8'); q=extractor.find('https://www.imagepond.net/a/gallery123'); print('|'.join((a.category,a.subcategory,a.archive_fmt.format(id='file.A7'),b.subcategory,b.archive_fmt.format(id='clip.B8'),q.subcategory)))",
    ]);
    cmd.env("PYTHONPATH", root);
    cmd.env("PYTHONNOUSERSITE", "1");
    cmd.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(
            cmd,
            "pinned ImagePond file-image, video and album queue archive IDs"
        )
        .trim(),
        "imagepond|file|file.A7|file|clip.B8|album"
    );
}

#[test]
fn pinned_imxto_image_token_and_gallery_dispatch_are_distinct() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned reference required for Imxto archive differential");
    let mut cmd = Command::new("python3");
    cmd.args([
        "-c",
        "from gallery_dl import extractor; a=extractor.find('https://imx.to/i/1qdeva'); b=extractor.find('https://img.yt/img-57a2050547b97.html'); q=extractor.find('https://imx.to/g/ozdy'); print('|'.join((a.category,a.subcategory,a.archive_fmt.format(token='1qdeva'),b.category,b.subcategory,b.archive_fmt.format(token='57a2050547b97'),q.subcategory)))",
    ]);
    cmd.env("PYTHONPATH", root);
    cmd.env("PYTHONNOUSERSITE", "1");
    cmd.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(
            cmd,
            "pinned Imxto individual image token and gallery queue archive"
        )
        .trim(),
        "imxto|image|1qdeva|imxto|image|57a2050547b97|gallery"
    );
}

#[test]
fn pinned_imgadult_uses_strict_hex_image_page_token() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned reference required for ImgAdult archive differential");
    let mut cmd = Command::new("python3");
    cmd.args([
        "-c",
        "from gallery_dl import extractor; a=extractor.find('https://imgadult.com/img-686b4f451af05.html'); b=extractor.find('https://www.imgadult.com/img-abcdef012.html'); print('|'.join((a.category,a.subcategory,a.archive_fmt.format(token='686b4f451af05'),b.category,b.subcategory,b.archive_fmt.format(token='abcdef012'))))",
    ]);
    cmd.env("PYTHONPATH", root);
    cmd.env("PYTHONNOUSERSITE", "1");
    cmd.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(cmd, "pinned ImgAdult source-page archive tokens").trim(),
        "imgadult|image|686b4f451af05|imgadult|image|abcdef012"
    );
}

#[test]
fn pinned_imgclick_uses_individual_imagehost_token_archive_keys() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned reference required for Imgclick archive differential");
    let mut cmd = Command::new("python3");
    cmd.args([
        "-c",
        "from gallery_dl import extractor; a=extractor.find('https://imgclick.net/4tbrre1oxew9'); b=extractor.find('http://www.imgclick.net/ABc_123/sample.png.html'); print('|'.join((a.category,a.subcategory,a.archive_fmt.format(token='4tbrre1oxew9'),b.category,b.subcategory,b.archive_fmt.format(token='ABc_123'))))",
    ]);
    cmd.env("PYTHONPATH", root);
    cmd.env("PYTHONNOUSERSITE", "1");
    cmd.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(
            cmd,
            "pinned Imgclick image-host token and optional URL hint"
        )
        .trim(),
        "imgclick|image|4tbrre1oxew9|imgclick|image|ABc_123"
    );
}

#[test]
fn pinned_fappic_and_imgpv_imagehost_tokens_match_their_source_pages() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned reference required for image-host token differential");
    let mut cmd = Command::new("python3");
    cmd.args([
        "-c",
        "from gallery_dl import extractor; a=extractor.find('https://fappic.com/abcde12345/name.jpg'); b=extractor.find('https://fappic.com/i/123/abcde12345_t/thumb.jpg'); c=extractor.find('https://www.imgpv.com/a1b2c3d4f5g6/name.jpg.html'); print('|'.join((a.category,a.subcategory,a.archive_fmt.format(token=a.token),b.category,b.token,b.archive_fmt.format(token=b.token),c.category,c.subcategory,c.archive_fmt.format(token='a1b2c3d4f5g6'))))",
    ]);
    cmd.env("PYTHONPATH", root);
    cmd.env("PYTHONNOUSERSITE", "1");
    cmd.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(
            cmd,
            "pinned Fappic thumbnail and Imgpv individual archive tokens"
        )
        .trim(),
        "fappic|image|abcde12345|fappic|abcde12345|abcde12345|imgpv|image|a1b2c3d4f5g6"
    );
}

#[test]
fn pinned_imgdrive_imgtaxi_and_imgwallet_retain_distinct_token_categories() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned reference required for shared image-host family");
    let mut cmd = Command::new("python3");
    cmd.args([
        "-c",
        "from gallery_dl import extractor; urls=('https://imgdrive.net/img-AbC012.html','https://imgtaxi.com/img-AbC012.html','https://imgwallet.net/img-AbC012.html'); out=[]; [(out.extend((e.category,e.subcategory,e.archive_fmt.format(token=e.token)))) for e in (extractor.find(url) for url in urls)]; print('|'.join(out))",
    ]);
    cmd.env("PYTHONPATH", root);
    cmd.env("PYTHONNOUSERSITE", "1");
    cmd.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(
            cmd,
            "pinned image-host family distinct archive token categories"
        )
        .trim(),
        "imgdrive|image|AbC012|imgtaxi|image|AbC012|imgwallet|image|AbC012"
    );
}

#[test]
fn pinned_silverpic_and_picstate_imagehost_archive_ids() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned reference required for Silverpic/Picstate archive differential");
    let mut cmd = Command::new("python3");
    cmd.args([
        "-c",
        "from gallery_dl import extractor; a=extractor.find('https://silverpic.net/a1b2c3d4f5g6/NAME.EXT.html'); b=extractor.find('https://picstate.com/view/full/123'); print('|'.join((a.category,a.subcategory,a.archive_fmt.format(token='a1b2c3d4f5g6'),b.category,b.subcategory,b.archive_fmt.format(token='123'))))",
    ]);
    cmd.env("PYTHONPATH", root);
    cmd.env("PYTHONNOUSERSITE", "1");
    cmd.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(
            cmd,
            "pinned Silverpic and Picstate individual archive identities"
        )
        .trim(),
        "silverpic|image|a1b2c3d4f5g6|picstate|image|123"
    );
}

#[test]
fn pinned_imagetwist_and_turboimagehost_individual_tokens_are_separate_from_gallery() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned reference required for Imagetwist/Turboimagehost differential");
    let mut cmd = Command::new("python3");
    cmd.args([
        "-c",
        "from gallery_dl import extractor; a=extractor.find('https://imagetwist.com/123456abcdef/name.jpg'); b=extractor.find('https://www.turboimagehost.com/p/12345/name.jpg.html'); g=extractor.find('https://www.turboimagehost.com/album/12345/albumname'); print('|'.join((a.category,a.subcategory,a.archive_fmt.format(token='123456abcdef'),b.category,b.subcategory,b.archive_fmt.format(token='12345'),g.subcategory)))",
    ]);
    cmd.env("PYTHONPATH", root);
    cmd.env("PYTHONNOUSERSITE", "1");
    cmd.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(
            cmd,
            "pinned Imagetwist image and Turboimagehost image/album archive keys"
        )
        .trim(),
        "imagetwist|image|123456abcdef|turboimagehost|image|12345|gallery"
    );
}
