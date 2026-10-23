//! End-to-end loopback-only test of the opt-in typed message download path.
use std::{fs, path::PathBuf, process::Command};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

fn fixture_dir() -> PathBuf {
    std::env::temp_dir().join(format!("galerith-message-job-{}", std::process::id()))
}

#[tokio::test]
async fn opt_in_message_job_reuses_native_atomic_download_pipeline() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        loop {
            let mut buf = [0_u8; 2048];
            let n = stream.read(&mut buf).await.unwrap();
            assert!(n > 0);
            request.extend_from_slice(&buf[..n]);
            assert!(request.len() <= 8192);
            if request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                break;
            }
        }
        assert!(String::from_utf8_lossy(&request).starts_with("GET /asset.png HTTP/1.1"));
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\n12345678")
            .await
            .unwrap();
    });
    let dir = fixture_dir();
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let input = format!("http://{addr}/asset.png");
    let output_dir = dir.clone();
    let output = tokio::task::spawn_blocking(move || {
        Command::new(env!("CARGO_BIN_EXE_galerith"))
            .args([
                "--follow-queues",
                "--retries",
                "0",
                "--http-timeout",
                "5",
                "--destination",
                output_dir.to_str().unwrap(),
                &input,
            ])
            .output()
            .expect("run Galerith")
    })
    .await
    .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(20), server)
        .await
        .expect("local HTTP fixture was not requested")
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read(dir.join("asset.png")).unwrap(), b"12345678");
    assert!(!dir.join("asset.png.part").exists());
    fs::remove_dir_all(dir).unwrap();
}
