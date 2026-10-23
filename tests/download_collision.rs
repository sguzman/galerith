//! Synthetic localhost test cases for output lock and safe replacement.
use galerith::{
    download::{download, DownloadOptions, DownloadOutcome},
    extract_direct_media,
};
use std::{fs, path::PathBuf, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

fn dest(suffix: &str) -> PathBuf {
    std::env::temp_dir().join(format!("galerith-lock-{suffix}-{}", std::process::id()))
}
fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap()
}
fn options(destination: PathBuf, no_skip: bool) -> DownloadOptions {
    DownloadOptions {
        destination,
        retries: 1,
        no_skip,
    }
}
#[tokio::test]
async fn concurrent_same_filename_downloads_once_then_skips() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let path = dest("same-file");
    let _ = fs::remove_dir_all(&path);
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut headers = [0u8; 2048];
        assert!(stream.read(&mut headers).await.unwrap() > 0);
        tokio::time::sleep(Duration::from_millis(150)).await;
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nDATA")
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(300), listener.accept())
                .await
                .is_err()
        );
    });
    let item = extract_direct_media(&format!("http://{addr}/parallel.jpg")).unwrap();
    let c = client();
    let shared_options = options(path.clone(), false);
    let a = download(&c, &item, &shared_options);
    let b = download(&c, &item, &shared_options);
    let (a, b) = tokio::join!(a, b);
    let results = [a.unwrap(), b.unwrap()];
    assert_eq!(
        results
            .iter()
            .filter(|x| matches!(x, DownloadOutcome::Saved(_)))
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|x| matches!(x, DownloadOutcome::Skipped(_)))
            .count(),
        1
    );
    assert_eq!(fs::read(path.join("parallel.jpg")).unwrap(), b"DATA");
    assert!(!path.join("parallel.jpg.part").exists());
    server.await.unwrap();
    fs::remove_dir_all(&path).unwrap();
}

#[tokio::test]
async fn overwrite_replaces_target_only_after_successful_download() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let path = dest("replace");
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    fs::write(path.join("replace.jpg"), b"OLD").unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut headers = [0u8; 2048];
        assert!(stream.read(&mut headers).await.unwrap() > 0);
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nNEW")
            .await
            .unwrap();
    });
    let item = extract_direct_media(&format!("http://{addr}/replace.jpg")).unwrap();
    assert_eq!(
        download(&client(), &item, &options(path.clone(), true))
            .await
            .unwrap(),
        DownloadOutcome::Saved(path.join("replace.jpg"))
    );
    assert_eq!(fs::read(path.join("replace.jpg")).unwrap(), b"NEW");
    server.await.unwrap();
    fs::remove_dir_all(&path).unwrap();
}

#[tokio::test]
async fn download_failure_does_not_destroy_previous_target() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let path = dest("failed-replace");
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    fs::write(path.join("failure.jpg"), b"EXISTING").unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut headers = [0u8; 2048];
        assert!(stream.read(&mut headers).await.unwrap() > 0);
        stream
            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
    });
    let item = extract_direct_media(&format!("http://{addr}/failure.jpg")).unwrap();
    assert!(download(&client(), &item, &options(path.clone(), true))
        .await
        .is_err());
    assert_eq!(fs::read(path.join("failure.jpg")).unwrap(), b"EXISTING");
    server.await.unwrap();
    fs::remove_dir_all(&path).unwrap();
}
