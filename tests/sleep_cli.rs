//! Offline pacing tests; no third-party network traffic.
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
    process::Command,
    time::{Duration, Instant},
};

fn destination(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "galerith-pacing-cli-{label}-{}",
        std::process::id()
    ))
}

fn run_download_with_setting(option: &str, setting: &str, label: &str) -> Duration {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let started = Instant::now();
        loop {
            match listener.accept() {
                Ok((mut socket, _)) => {
                    let mut buf = [0_u8; 2048];
                    assert!(socket.read(&mut buf).unwrap() > 0);
                    socket
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nPAUS",
                        )
                        .unwrap();
                    break;
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(started.elapsed() < Duration::from_secs(8));
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(err) => panic!("listener error: {err}"),
            }
        }
    });
    let dir = destination(label);
    let _ = fs::remove_dir_all(&dir);
    let url = format!("http://{addr}/image.jpg");
    let started = Instant::now();
    let output = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args([option, setting, "-d", dir.to_str().unwrap(), &url])
        .output()
        .unwrap();
    server.join().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read(dir.join("image.jpg")).unwrap(), b"PAUS");
    fs::remove_dir_all(dir).unwrap();
    started.elapsed()
}

#[test]
fn constant_sleep_delays_media_request() {
    assert!(
        run_download_with_setting("--sleep", "0.15", "fixed") >= Duration::from_millis(140),
        "media download occurred before the requested delay"
    );
}

#[test]
fn ranged_sleep_delays_media_request_by_at_least_its_minimum() {
    let elapsed = run_download_with_setting("--sleep", "0.1-0.3", "ranged");
    assert!(
        elapsed >= Duration::from_millis(90),
        "media download occurred before the range minimum"
    );
    assert!(
        elapsed < Duration::from_secs(2),
        "media download exceeded the configured range by too much"
    );
}

#[test]
fn configured_request_sleep_delays_each_media_transfer_attempt() {
    assert!(
        run_download_with_setting("--option", "extractor.sleep-request=0.12", "request",)
            >= Duration::from_millis(110),
        "media request occurred before configured sleep-request delay"
    );
}

#[test]
fn extractor_specific_request_sleep_delays_media_transfer() {
    assert!(
        run_download_with_setting(
            "--option",
            "extractor.directlink.sleep-request=0.12",
            "request-directlink",
        ) >= Duration::from_millis(110),
        "media request occurred before extractor-specific sleep-request delay"
    );
}

#[test]
fn configured_skip_sleep_waits_after_existing_file_is_skipped() {
    let dir = destination("existing-file-skip");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("image.jpg"), b"keep existing").unwrap();
    let started = Instant::now();
    let output = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args([
            "--option",
            "extractor.sleep-skip=0.12",
            "-d",
            dir.to_str().unwrap(),
            "https://example.org/image.jpg",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(started.elapsed() >= Duration::from_millis(110));
    assert_eq!(fs::read(dir.join("image.jpg")).unwrap(), b"keep existing");
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn configured_extractor_sleep_waits_before_handling_input_url() {
    let started = Instant::now();
    let output = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args([
            "--option",
            "extractor.sleep-extractor=0.12",
            "--simulate",
            "https://example.org/image.jpg",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(started.elapsed() >= Duration::from_millis(110));
}

#[test]
fn invalid_sleep_values_are_rejected_before_network() {
    for value in ["--sleep=-1", "--sleep=4000", "--sleep=NaN"] {
        let output = Command::new(env!("CARGO_BIN_EXE_galerith"))
            .args([value, "--simulate", "https://example.org/file.jpg"])
            .output()
            .unwrap();
        assert!(!output.status.success(), "unexpected success for {value}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("invalid --sleep"),
            "unexpected error for {value}"
        );
    }
}

#[test]
fn configured_retry_sleep_delays_a_transient_media_retry() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        for attempt in 0..2 {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0_u8; 1024];
            assert!(socket.read(&mut request).unwrap() > 0);
            let response: &[u8] = if attempt == 0 {
                b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            } else {
                b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nGOOD"
            };
            socket.write_all(response).unwrap();
        }
    });
    let dir = destination("retry-delay");
    let _ = fs::remove_dir_all(&dir);
    let started = Instant::now();
    let output = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args(["--sleep-retries", "0.15", "--retries", "1", "-d"])
        .arg(dir.to_str().unwrap())
        .arg(format!("http://{address}/image.jpg"))
        .output()
        .unwrap();
    let elapsed = started.elapsed();
    server.join().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(elapsed >= Duration::from_millis(140), "{elapsed:?}");
    assert_eq!(fs::read(dir.join("image.jpg")).unwrap(), b"GOOD");
    fs::remove_dir_all(dir).unwrap();
}
