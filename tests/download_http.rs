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
        request_delay: None,
    }
}

#[tokio::test]
async fn downloads_atomically_and_skips_existing_files() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0_u8; 512];
        assert!(stream.read(&mut request).await.unwrap() > 0);
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
async fn known_content_type_does_not_reject_extensionless_custom_output() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0_u8; 512];
        assert!(stream.read(&mut request).await.unwrap() > 0);
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: 4\r\nConnection: close\r\n\r\nJPEG",
            )
            .await
            .unwrap();
    });
    let dir = test_dir("extensionless-output");
    let _ = fs::remove_dir_all(&dir);
    let mut item = extract_direct_media(&format!("http://{addr}/source.jpg")).unwrap();
    item.filename = "custom-output".into();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let destination = dir.join("custom-output");
    assert_eq!(
        download(&client, &item, &options(dir.clone()))
            .await
            .unwrap(),
        DownloadOutcome::Saved(destination.clone())
    );
    assert_eq!(fs::read(destination).unwrap(), b"JPEG");
    server.await.unwrap();
    fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn direct_heif_media_matches_its_response_mime() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0_u8; 512];
        assert!(stream.read(&mut request).await.unwrap() > 0);
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: image/heif\r\nContent-Length: 4\r\nConnection: close\r\n\r\nHEIF")
            .await
            .unwrap();
    });
    let dir = test_dir("heif");
    let _ = fs::remove_dir_all(&dir);
    let item = extract_direct_media(&format!("http://{addr}/photo.heif")).unwrap();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    assert_eq!(
        download(&client, &item, &options(dir.clone()))
            .await
            .unwrap(),
        DownloadOutcome::Saved(dir.join("photo.heif"))
    );
    assert_eq!(fs::read(dir.join("photo.heif")).unwrap(), b"HEIF");
    server.await.unwrap();
    fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn direct_apng_media_matches_its_response_mime() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0_u8; 512];
        assert!(stream.read(&mut request).await.unwrap() > 0);
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: image/apng\r\nContent-Length: 4\r\nConnection: close\r\n\r\nAPNG")
            .await
            .unwrap();
    });
    let dir = test_dir("apng");
    let _ = fs::remove_dir_all(&dir);
    let item = extract_direct_media(&format!("http://{addr}/animation.apng")).unwrap();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    assert_eq!(
        download(&client, &item, &options(dir.clone()))
            .await
            .unwrap(),
        DownloadOutcome::Saved(dir.join("animation.apng"))
    );
    assert_eq!(fs::read(dir.join("animation.apng")).unwrap(), b"APNG");
    server.await.unwrap();
    fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn direct_mpeg_transport_stream_matches_its_response_mime() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0_u8; 512];
        assert!(stream.read(&mut request).await.unwrap() > 0);
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: video/mp2t\r\nContent-Length: 4\r\nConnection: close\r\n\r\nTS00")
            .await
            .unwrap();
    });
    let dir = test_dir("mpeg-ts");
    let _ = fs::remove_dir_all(&dir);
    let item = extract_direct_media(&format!("http://{addr}/recording.m2ts")).unwrap();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    assert_eq!(
        download(&client, &item, &options(dir.clone()))
            .await
            .unwrap(),
        DownloadOutcome::Saved(dir.join("recording.m2ts"))
    );
    assert_eq!(fs::read(dir.join("recording.m2ts")).unwrap(), b"TS00");
    server.await.unwrap();
    fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn direct_icon_media_matches_its_response_mime() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0_u8; 512];
        assert!(stream.read(&mut request).await.unwrap() > 0);
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: image/vnd.microsoft.icon\r\nContent-Length: 4\r\nConnection: close\r\n\r\nICO0")
            .await
            .unwrap();
    });
    let dir = test_dir("icon");
    let _ = fs::remove_dir_all(&dir);
    let item = extract_direct_media(&format!("http://{addr}/favicon.ico")).unwrap();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    assert_eq!(
        download(&client, &item, &options(dir.clone()))
            .await
            .unwrap(),
        DownloadOutcome::Saved(dir.join("favicon.ico"))
    );
    assert_eq!(fs::read(dir.join("favicon.ico")).unwrap(), b"ICO0");
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
            assert!(stream.read(&mut request).await.unwrap() > 0);
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
        assert!(stream.read(&mut request).await.unwrap() > 0);
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

#[tokio::test]
async fn keeps_valid_http_last_modified_as_output_mtime() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0_u8; 512];
        assert!(stream.read(&mut request).await.unwrap() > 0);
        stream.write_all(
            b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nLast-Modified: Wed, 21 Oct 2015 07:28:00 GMT\r\nConnection: close\r\n\r\nTIME"
        ).await.unwrap();
    });
    let dir = test_dir("last-modified");
    let _ = fs::remove_dir_all(&dir);
    let item = extract_direct_media(&format!("http://{addr}/dated.png")).unwrap();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    assert_eq!(
        download(&client, &item, &options(dir.clone()))
            .await
            .unwrap(),
        DownloadOutcome::Saved(dir.join("dated.png")),
    );
    let actual = fs::metadata(dir.join("dated.png"))
        .unwrap()
        .modified()
        .unwrap();
    let expected = httpdate::parse_http_date("Wed, 21 Oct 2015 07:28:00 GMT").unwrap();
    assert!(actual.duration_since(expected).unwrap() < Duration::from_secs(2));
    assert_eq!(fs::read(dir.join("dated.png")).unwrap(), b"TIME");
    server.await.unwrap();
    fs::remove_dir_all(&dir).unwrap();
}
