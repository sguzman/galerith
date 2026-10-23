//! Network tests use a loopback fixture server, never live gallery websites.
use galerith::{
    download::{download, DownloadOptions, DownloadOutcome},
    extract_direct_media,
};
use std::{fs, path::PathBuf, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

fn test_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("galerith-{name}-{}", std::process::id()))
}

fn options(destination: PathBuf) -> DownloadOptions {
    DownloadOptions {
        destination,
        retries: 2,
        no_skip: false,
    }
}

#[tokio::test]
async fn downloads_atomically_and_skips_existing_files() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0_u8; 512];
        stream.read(&mut request).await.unwrap();
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\n12345678")
            .await
            .unwrap();
    });
    let dir = test_dir("stream");
    let _ = fs::remove_dir_all(&dir);
    let item = extract_direct_media(&format!("http://{addr}/file.png")).unwrap();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let opts = options(dir.clone());
    let result = download(&client, &item, &opts).await.unwrap();
    assert_eq!(result, DownloadOutcome::Saved(dir.join("file.png")));
    assert_eq!(fs::read(dir.join("file.png")).unwrap(), b"12345678");
    assert!(!dir.join("file.png.part").exists());
    assert_eq!(
        download(&client, &item, &opts).await.unwrap(),
        DownloadOutcome::Skipped(dir.join("file.png"))
    );
    server.await.unwrap();
    fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn retries_transient_server_errors() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        for attempt in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 512];
            stream.read(&mut request).await.unwrap();
            let reply: &[u8] = if attempt == 0 {
                b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            } else {
                b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK"
            };
            stream.write_all(reply).await.unwrap();
        }
    });
    let dir = test_dir("retry");
    let _ = fs::remove_dir_all(&dir);
    let item = extract_direct_media(&format!("http://{addr}/retry.jpg")).unwrap();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    download(&client, &item, &options(dir.clone()))
        .await
        .unwrap();
    assert_eq!(fs::read(dir.join("retry.jpg")).unwrap(), b"OK");
    server.await.unwrap();
    fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn leaves_no_target_on_404() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0_u8; 512];
        stream.read(&mut request).await.unwrap();
        stream
            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
    });
    let dir = test_dir("missing");
    let _ = fs::remove_dir_all(&dir);
    let item = extract_direct_media(&format!("http://{addr}/missing.jpg")).unwrap();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    assert!(download(&client, &item, &options(dir.clone()))
        .await
        .is_err());
    assert!(!dir.join("missing.jpg").exists());
    assert!(!dir.join("missing.jpg.part").exists());
    server.await.unwrap();
    fs::remove_dir_all(&dir).unwrap();
}
