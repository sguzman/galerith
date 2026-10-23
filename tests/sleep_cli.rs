//! Offline pacing tests; no third-party network traffic.
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
    process::Command,
    time::{Duration, Instant},
};

fn destination() -> PathBuf {
    std::env::temp_dir().join(format!("galerith-pacing-cli-{}", std::process::id()))
}

#[test]
fn constant_sleep_delays_media_request() {
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
    let dir = destination();
    let _ = fs::remove_dir_all(&dir);
    let url = format!("http://{addr}/image.jpg");
    let started = Instant::now();
    let output = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args(["--sleep", "0.15", "-d", dir.to_str().unwrap(), &url])
        .output()
        .unwrap();
    server.join().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        started.elapsed() >= Duration::from_millis(140),
        "media download occurred before the requested delay"
    );
    assert_eq!(fs::read(dir.join("image.jpg")).unwrap(), b"PAUS");
    fs::remove_dir_all(dir).unwrap();
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
