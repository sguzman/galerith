//! Synthetic HTTP quality checks: large downloads and retry policies.
use galerith::{
    download::{download, DownloadOptions, DownloadOutcome},
    extract_direct_media,
};
use std::{fs, path::PathBuf, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

fn destination(suffix: &str) -> PathBuf {
    std::env::temp_dir().join(format!("galerith-quality-{suffix}-{}", std::process::id()))
}
fn opts(path: PathBuf, retries: u32) -> DownloadOptions {
    DownloadOptions {
        destination: path,
        retries,
        no_skip: false,
        no_mtime: false,
        no_part: false,
        limit_rate: None,
        headers: reqwest::header::HeaderMap::new(),
        retry_codes: vec![],
        http_timeout: None,
        retry_delay: None,
        rate_limit_delay: None,
        request_delay: None,
        download_delay: None,
    }
}
fn opts_with_429(path: PathBuf, retries: u32) -> DownloadOptions {
    DownloadOptions {
        retry_codes: vec![reqwest::StatusCode::TOO_MANY_REQUESTS],
        http_timeout: None,
        ..opts(path, retries)
    }
}
fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap()
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
        let headers =
            format!("HTTP/1.1 200 OK\r\nContent-Length: {size}\r\nConnection: close\r\n\r\n");
        stream.write_all(headers.as_bytes()).await.unwrap();
        for _ in 0..(size / 4096) {
            stream.write_all(&[0xA5; 4096]).await.unwrap();
        }
    });
    let path = destination("large");
    let _ = fs::remove_dir_all(&path);
    let item = extract_direct_media(&format!("http://{addr}/large.webp")).unwrap();
    assert_eq!(
        download(&client(), &item, &opts(path.clone(), 0))
            .await
            .unwrap(),
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
async fn does_not_retry_429_unless_configured_as_retryable() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0u8; 2048];
        assert!(stream.read(&mut request).await.unwrap() > 0);
        stream
            .write_all(
                b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 0\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();
    });
    let path = destination("429-default");
    let _ = fs::remove_dir_all(&path);
    let item = extract_direct_media(&format!("http://{addr}/default.jpg")).unwrap();
    assert!(download(&client(), &item, &opts(path.clone(), 3))
        .await
        .is_err());
    server.await.unwrap();
    assert!(!path.join("default.jpg").exists());
    let _ = fs::remove_dir_all(path);
}

#[tokio::test]
async fn recovers_from_429_500_and_503_then_saves() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        for status in [
            "429 Too Many Requests",
            "500 Internal Server Error",
            "503 Service Unavailable",
            "200 OK",
        ] {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 2048];
            assert!(stream.read(&mut request).await.unwrap() > 0);
            let (body, delay) = if status == "200 OK" {
                ("GOOD", "")
            } else {
                ("", "Retry-After: 0\r\n")
            };
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
    download(&client(), &item, &opts_with_429(path.clone(), 3))
        .await
        .unwrap();
    assert_eq!(fs::read(path.join("attempt.jpg")).unwrap(), b"GOOD");
    assert!(!path.join("attempt.jpg.part").exists());
    server.await.unwrap();
    fs::remove_dir_all(&path).unwrap();
}

#[tokio::test]
async fn honors_retry_after_before_retrying_rate_limited_media() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        for (status, body, retry_after) in [
            ("429 Too Many Requests", "", "Retry-After: 1\r\n"),
            ("200 OK", "GOOD", ""),
        ] {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 2048];
            assert!(stream.read(&mut request).await.unwrap() > 0);
            let reply = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\n{retry_after}Connection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(reply.as_bytes()).await.unwrap();
        }
    });
    let path = destination("retry-after-delay");
    let _ = fs::remove_dir_all(&path);
    let item = extract_direct_media(&format!("http://{addr}/retry-after.jpg")).unwrap();
    let started = tokio::time::Instant::now();
    download(&client(), &item, &opts_with_429(path.clone(), 1))
        .await
        .unwrap();
    assert!(
        started.elapsed() >= Duration::from_millis(950),
        "retry began before the server's Retry-After delay"
    );
    assert_eq!(fs::read(path.join("retry-after.jpg")).unwrap(), b"GOOD");
    server.await.unwrap();
    fs::remove_dir_all(&path).unwrap();
}

#[tokio::test]
async fn retry_after_cooldown_delays_later_media_on_the_same_origin() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        for response in [
            b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".as_slice(),
            b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nGOOD".as_slice(),
        ] {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 2048];
            assert!(stream.read(&mut request).await.unwrap() > 0);
            stream.write_all(response).await.unwrap();
        }
    });
    let path = destination("same-origin-cooldown");
    let _ = fs::remove_dir_all(&path);
    let first = extract_direct_media(&format!("http://{addr}/limited.jpg")).unwrap();
    assert!(download(&client(), &first, &opts(path.clone(), 0))
        .await
        .is_err());

    let second = extract_direct_media(&format!("http://{addr}/unrelated.jpg")).unwrap();
    let started = tokio::time::Instant::now();
    download(&client(), &second, &opts(path.clone(), 0))
        .await
        .unwrap();
    assert!(
        started.elapsed() >= Duration::from_millis(950),
        "another media request to the throttled origin was not delayed"
    );
    assert_eq!(fs::read(path.join("unrelated.jpg")).unwrap(), b"GOOD");
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
    let err = download(&client(), &item, &opts(path.clone(), 0))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("unexpected partial response"));
    assert!(!path.join("bad.jpg").exists());
    assert!(!path.join("bad.jpg.part").exists());
    server.await.unwrap();
    fs::remove_dir_all(&path).unwrap();
}
