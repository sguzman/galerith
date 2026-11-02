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
    run_download_with_args(&[option, setting], label)
}

fn run_download_with_args(args: &[&str], label: &str) -> Duration {
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
        .args(args)
        .args(["-d", dir.to_str().unwrap(), &url])
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
fn download_sleep_does_not_delay_an_existing_file_skip() {
    let dir = destination("download-sleep-existing-file");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("image.jpg"), b"keep existing").unwrap();
    let started = Instant::now();
    let output = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args([
            "--sleep",
            "0.15",
            "-d",
            dir.to_str().unwrap(),
            "https://example.org/image.jpg",
        ])
        .output()
        .unwrap();
    let elapsed = started.elapsed();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        elapsed < Duration::from_millis(120),
        "--sleep ran before the existing-file check: {elapsed:?}"
    );
    assert_eq!(fs::read(dir.join("image.jpg")).unwrap(), b"keep existing");
    fs::remove_dir_all(dir).unwrap();
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
fn sleep_request_does_not_delay_direct_media_transfers() {
    let elapsed = run_download_with_args(
        &[
            "--sleep-request",
            "0.8",
            "--option",
            "extractor.sleep-request=0.8",
        ],
        "request-not-media",
    );
    assert!(
        elapsed < Duration::from_millis(500),
        "extractor request pacing delayed a direct media transfer: {elapsed:?}"
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
fn ranged_skip_sleep_waits_after_existing_file_is_skipped() {
    let dir = destination("existing-file-skip-range");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("image.jpg"), b"keep existing").unwrap();
    let started = Instant::now();
    let output = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args([
            "--option",
            "extractor.sleep-skip=0.08-0.12",
            "-d",
            dir.to_str().unwrap(),
            "https://example.org/image.jpg",
        ])
        .output()
        .unwrap();
    let elapsed = started.elapsed();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(elapsed >= Duration::from_millis(70), "{elapsed:?}");
    assert!(elapsed < Duration::from_millis(500), "{elapsed:?}");
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
fn sleep_extractor_flag_delays_input_handling() {
    let started = Instant::now();
    let output = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args([
            "--sleep-extractor",
            "0.08",
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
    assert!(started.elapsed() >= Duration::from_millis(70));
}

#[test]
fn sleep_extractor_flag_overrides_longer_configured_delay() {
    let started = Instant::now();
    let output = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args([
            "--sleep-extractor",
            "0.08",
            "--option",
            "extractor.sleep-extractor=0.5",
            "--simulate",
            "https://example.org/image.jpg",
        ])
        .output()
        .unwrap();
    let elapsed = started.elapsed();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(elapsed >= Duration::from_millis(70), "{elapsed:?}");
    assert!(
        elapsed < Duration::from_millis(450),
        "the CLI delay did not override the longer configured delay: {elapsed:?}"
    );
}

#[test]
fn sleep_skip_flag_waits_after_existing_file_is_skipped() {
    let dir = destination("sleep-skip-flag");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("image.jpg"), b"keep existing").unwrap();
    let started = Instant::now();
    let output = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args([
            "--sleep-skip",
            "0.08",
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
    assert!(started.elapsed() >= Duration::from_millis(70));
    assert_eq!(fs::read(dir.join("image.jpg")).unwrap(), b"keep existing");
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn sleep_skip_flag_waits_after_a_size_filtered_download() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut request = [0_u8; 1024];
        assert!(socket.read(&mut request).unwrap() > 0);
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nDATA")
            .unwrap();
    });
    let dir = destination("sleep-skip-size-filter");
    let _ = fs::remove_dir_all(&dir);
    let started = Instant::now();
    let output = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args([
            "--sleep-skip",
            "0.08",
            "--filesize-min",
            "5",
            "-d",
            dir.to_str().unwrap(),
        ])
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
    assert!(elapsed >= Duration::from_millis(70), "{elapsed:?}");
    assert!(!dir.join("image.jpg").exists());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn sleep_skip_flag_overrides_longer_configured_delay() {
    let dir = destination("sleep-skip-cli-precedence");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("image.jpg"), b"keep existing").unwrap();
    let started = Instant::now();
    let output = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args([
            "--sleep-skip",
            "0.08",
            "--option",
            "extractor.sleep-skip=0.5",
            "-d",
            dir.to_str().unwrap(),
            "https://example.org/image.jpg",
        ])
        .output()
        .unwrap();
    let elapsed = started.elapsed();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(elapsed >= Duration::from_millis(70), "{elapsed:?}");
    assert!(
        elapsed < Duration::from_millis(450),
        "the CLI delay did not override the longer configured delay: {elapsed:?}"
    );
    assert_eq!(fs::read(dir.join("image.jpg")).unwrap(), b"keep existing");
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn ranged_extractor_sleep_is_sampled_before_handling_input_url() {
    let started = Instant::now();
    let output = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args([
            "--option",
            "extractor.sleep-extractor=0.08-0.12",
            "--simulate",
            "https://example.org/image.jpg",
        ])
        .output()
        .unwrap();
    let elapsed = started.elapsed();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(elapsed >= Duration::from_millis(70), "{elapsed:?}");
    assert!(elapsed < Duration::from_millis(500), "{elapsed:?}");
}

#[test]
fn invalid_sleep_values_are_rejected_before_network() {
    for (value, expected_error) in [
        ("--sleep=-1", "invalid --sleep"),
        ("--sleep=4000", "invalid --sleep"),
        ("--sleep=NaN", "invalid --sleep"),
        ("--sleep-retries=-1", "invalid --sleep-retries"),
        ("--sleep-retries=4000", "invalid --sleep-retries"),
        ("--sleep-retries=NaN", "invalid --sleep-retries"),
        ("--sleep-retries=exp:0=1", "invalid --sleep-retries"),
        ("--sleep-retries=lin:0:3601=1", "invalid --sleep-retries"),
        ("--sleep-429=-1", "invalid --sleep-429"),
        ("--sleep-429=4000", "invalid --sleep-429"),
        ("--sleep-429=NaN", "invalid --sleep-429"),
        ("--sleep-429=exp:NaN=1", "invalid --sleep-429"),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_galerith"))
            .args([value, "--simulate", "https://example.org/file.jpg"])
            .output()
            .unwrap();
        assert!(!output.status.success(), "unexpected success for {value}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(expected_error),
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

#[test]
fn exponential_retry_sleep_grows_for_each_attempt() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        for attempt in 0..3 {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0_u8; 1024];
            assert!(socket.read(&mut request).unwrap() > 0);
            let response: &[u8] = if attempt < 2 {
                b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            } else {
                b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nGOOD"
            };
            socket.write_all(response).unwrap();
        }
    });
    let dir = destination("retry-delay-exponential");
    let _ = fs::remove_dir_all(&dir);
    let started = Instant::now();
    let output = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args([
            "--sleep-retries",
            "exp=0.08",
            "--retries",
            "2",
            "-d",
            dir.to_str().unwrap(),
        ])
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
    assert!(elapsed >= Duration::from_millis(220), "{elapsed:?}");
    assert_eq!(fs::read(dir.join("image.jpg")).unwrap(), b"GOOD");
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn configured_sleep_429_respects_the_minimum_retry_interval() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        for attempt in 0..2 {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0_u8; 1024];
            assert!(socket.read(&mut request).unwrap() > 0);
            let response: &[u8] = if attempt == 0 {
                b"HTTP/1.1 429 Too Many Requests\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            } else {
                b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nGOOD"
            };
            socket.write_all(response).unwrap();
        }
    });
    let dir = destination("sleep-429");
    let _ = fs::remove_dir_all(&dir);
    let started = Instant::now();
    let output = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args([
            "-o",
            "downloader.http.retry-codes=[429]",
            "--sleep-429",
            "0.15",
            "--sleep-retries",
            "0",
            "--retries",
            "1",
            "-d",
            dir.to_str().unwrap(),
        ])
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
    assert!(elapsed >= Duration::from_millis(950), "{elapsed:?}");
    assert_eq!(fs::read(dir.join("image.jpg")).unwrap(), b"GOOD");
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn sleep_429_does_not_shorten_general_retry_delay() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        for attempt in 0..2 {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0_u8; 1024];
            assert!(socket.read(&mut request).unwrap() > 0);
            let response: &[u8] = if attempt == 0 {
                b"HTTP/1.1 429 Too Many Requests\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            } else {
                b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nGOOD"
            };
            socket.write_all(response).unwrap();
        }
    });
    let dir = destination("retry-delay-over-429");
    let _ = fs::remove_dir_all(&dir);
    let started = Instant::now();
    let output = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args([
            "-o",
            "downloader.http.retry-codes=[429]",
            "--sleep-429",
            "0",
            "--sleep-retries",
            "1.5",
            "--retries",
            "1",
            "-d",
            dir.to_str().unwrap(),
        ])
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
    assert!(elapsed >= Duration::from_millis(1490), "{elapsed:?}");
    assert_eq!(fs::read(dir.join("image.jpg")).unwrap(), b"GOOD");
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn downloader_retry_count_config_disables_retries_when_cli_value_is_absent() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut first = loop {
            match listener.accept() {
                Ok((socket, _)) => break socket,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "first request never arrived");
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("retry-config listener error: {error}"),
            }
        };
        first
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut request = [0_u8; 2048];
        assert!(first.read(&mut request).unwrap() > 0);
        first
            .write_all(
                b"HTTP/1.1 429 Too Many Requests\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
        drop(first);

        let deadline = Instant::now() + Duration::from_millis(1400);
        loop {
            match listener.accept() {
                Ok((mut socket, _)) => {
                    socket
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    assert!(socket.read(&mut request).unwrap() > 0);
                    socket
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nGOOD",
                        )
                        .unwrap();
                    return true;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return false;
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("retry-config listener error: {error}"),
            }
        }
    });
    let dir = destination("downloader-retries-config");
    let _ = fs::remove_dir_all(&dir);
    let output = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args([
            "-o",
            "downloader.http.retry-codes=[429]",
            "-o",
            "downloader.http.retries=0",
            "--sleep-429",
            "0",
            "-d",
        ])
        .arg(&dir)
        .arg(format!("http://{address}/image.jpg"))
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "zero configured retries unexpectedly succeeded: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !server.join().unwrap(),
        "configured zero retry made another request"
    );
    assert!(!dir.join("image.jpg").exists());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn explicit_cli_retry_count_overrides_downloader_http_config() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        for response in [
            b"HTTP/1.1 429 Too Many Requests\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                .as_slice(),
            b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nGOOD".as_slice(),
        ] {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = [0_u8; 2048];
            assert!(socket.read(&mut request).unwrap() > 0);
            socket.write_all(response).unwrap();
        }
    });
    let dir = destination("cli-retries-override-config");
    let _ = fs::remove_dir_all(&dir);
    let output = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args([
            "-o",
            "downloader.http.retry-codes=[429]",
            "-o",
            "downloader.http.retries=0",
            "--retries",
            "1",
            "--sleep-429",
            "0",
            "-d",
        ])
        .arg(&dir)
        .arg(format!("http://{address}/image.jpg"))
        .output()
        .unwrap();
    server.join().unwrap();
    assert!(
        output.status.success(),
        "explicit CLI retry count was ignored: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read(dir.join("image.jpg")).unwrap(), b"GOOD");
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn ranged_retry_sleep_is_sampled_between_transient_media_attempts() {
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
    let dir = destination("retry-delay-range");
    let _ = fs::remove_dir_all(&dir);
    let started = Instant::now();
    let output = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args([
            "--sleep-retries",
            "0.08-0.12",
            "--retries",
            "1",
            "-d",
            dir.to_str().unwrap(),
        ])
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
    assert!(elapsed >= Duration::from_millis(70), "{elapsed:?}");
    assert!(elapsed < Duration::from_millis(500), "{elapsed:?}");
    assert_eq!(fs::read(dir.join("image.jpg")).unwrap(), b"GOOD");
    fs::remove_dir_all(dir).unwrap();
}
