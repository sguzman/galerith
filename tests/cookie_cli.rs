//! Synthetic cookie import end-to-end; no real credentials are stored.
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
    process::Command,
    thread,
    time::{Duration, Instant},
};

fn destination() -> PathBuf {
    std::env::temp_dir().join(format!("galerith-cookie-cli-{}", std::process::id()))
}

#[test]
fn netscape_cookie_flows_to_media_request_without_logging_secret() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let start = Instant::now();
        loop {
            match listener.accept() {
                Ok((mut socket, _)) => {
                    socket.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                    let mut bytes = [0_u8; 4096];
                    let count = socket.read(&mut bytes).unwrap();
                    let request = String::from_utf8_lossy(&bytes[..count]);
                    assert!(request.to_ascii_lowercase().contains("cookie: session=synthetic-cookie"));
                    socket.write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nPASS"
                    ).unwrap();
                    break;
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(start.elapsed() < Duration::from_secs(5), "no HTTP request received");
                    thread::sleep(Duration::from_millis(10));
                }
                Err(err) => panic!("unexpected listener failure: {err}"),
            }
        }
    });
    let dir = destination();
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let cookie_path = dir.join("synthetic-cookies.txt");
    fs::write(
        &cookie_path,
        "127.0.0.1\tFALSE\t/\tFALSE\t0\tsession\tsynthetic-cookie\n",
    ).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args(["--cookies", cookie_path.to_str().unwrap(), "-d", dir.to_str().unwrap()])
        .arg(format!("http://{addr}/media.jpg"))
        .output()
        .unwrap();
    server.join().unwrap();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert_eq!(fs::read(dir.join("media.jpg")).unwrap(), b"PASS");
    assert!(!String::from_utf8_lossy(&result.stdout).contains("synthetic-cookie"));
    assert!(!String::from_utf8_lossy(&result.stderr).contains("synthetic-cookie"));
    fs::remove_dir_all(&dir).unwrap();
}
