//! Synthetic HTTP quality checks: large downloads and retry policies.
use galerith::{
    download::{download, DownloadOptions, DownloadOutcome},
    extract_direct_media,
};
use std::{fs, path::PathBuf, time::Duration};
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::TcpListener};

fn destination(suffix: &str) -> PathBuf {
    std::env::temp_dir().join(format!("galerith-quality-{suffix}-{}", std::process::id()))
}
fn opts(path: PathBuf, retries: u32) -> DownloadOptions {
    DownloadOptions { destination: path, retries, no_skip: false }
}
fn client() -> reqwest::Client {
    reqwest::Client::builder().timeout(Duration::from_secs(10)).build().unwrap()
}

#[tokio::test]
async fn streams_multi_megabyte_body_to_atomic_target() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let size = 3 * 1024 * 1024;
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buffer = [0u8; 2048];
        assert!(stream.read(&mut buffer).await.unwrap() > 0);
        let headers = format!("HTTP/1.1 200 OK\r\nContent-Length: {size}\r\nConnection: close\r\n\r\n");
        stream.write_all(headers.as_bytes()).await.unwrap();
        for _ in 0..(size / 4096) {
            stream.write_all(&[0xA5; 4096]).await.unwrap();
        }
    });
    let path = destination("large");
    let _ = fs::remove_dir_all(&path);
    let item = extract_direct_media(&format!("http://{addr}/large.webp")).unwrap();
    assert_eq!(
        download(&client(), &item, &opts(path.clone(), 0)).await.unwrap(),
        DownloadOutcome::Saved(path.join("large.webp"))
    );
    let output = fs::read(path.join("large.webp")).unwrap();
    assert_eq!(output.len(), size);
    assert!(output.iter().all(|x| *x == 0xA5));
    assert!(!path.join("large.webp.part").exists());
    server.await.unwrap();
    fs::remove_dir_all(&path).unwrap();
}

#[tokio::test]
async fn recovers_from_429_500_and_503_then_saves() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        for status in ["429 Too Many Requests", "500 Internal Server Error", "503 Service Unavailable", "200 OK"] {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 2048];
            assert!(stream.read(&mut request).await.unwrap() > 0);
            let (body, delay) = if status == "200 OK" { ("GOOD", "") } else { ("", "Retry-After: 0\r\n") };
            let reply = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\n{delay}Connection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(reply.as_bytes()).await.unwrap();
        }
    });
    let path = destination("retries");
    let _ = fs::remove_dir_all(&path);
    let item = extract_direct_media(&format!("http://{addr}/attempt.jpg")).unwrap();
    download(&client(), &item, &opts(path.clone(), 3)).await.unwrap();
    assert_eq!(fs::read(path.join("attempt.jpg")).unwrap(), b"GOOD");
    assert!(!path.join("attempt.jpg.part").exists());
    server.await.unwrap();
    fs::remove_dir_all(&path).unwrap();
}

#[tokio::test]
async fn rejects_unsolicited_partial_without_overwriting_anything() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0u8; 2048];
        assert!(stream.read(&mut request).await.unwrap() > 0);
        stream.write_all(
            b"HTTP/1.1 206 Partial Content\r\nContent-Length: 2\r\nContent-Range: bytes 5-6/7\r\nConnection: close\r\n\r\nNO"
        ).await.unwrap();
    });
    let path = destination("bad-range");
    let _ = fs::remove_dir_all(&path);
    let item = extract_direct_media(&format!("http://{addr}/bad.jpg")).unwrap();
    let err = download(&client(), &item, &opts(path.clone(), 0)).await.unwrap_err();
    assert!(err.to_string().contains("unexpected partial response"));
    assert!(!path.join("bad.jpg").exists());
    assert!(!path.join("bad.jpg.part").exists());
    server.await.unwrap();
    fs::remove_dir_all(&path).unwrap();
}
