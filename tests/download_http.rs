//! Network tests use a loopback fixture server, never live gallery websites.
use galerith::{
    download::{download, DownloadOptions, DownloadOutcome},
    extract_direct_media,
};
use std::{
    fs,
    path::PathBuf,
    process::Command,
    time::{Duration, Instant, UNIX_EPOCH},
};
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
        no_mtime: false,
        no_part: false,
        limit_rate: None,
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
async fn common_mime_alias_still_rejects_a_conflicting_filename_extension() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0_u8; 512];
        assert!(stream.read(&mut request).await.unwrap() > 0);
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: image/x-icon\r\nContent-Length: 4\r\nConnection: close\r\n\r\nICON",
            )
            .await
            .unwrap();
    });
    let dir = test_dir("mime-alias-mismatch");
    let _ = fs::remove_dir_all(&dir);
    let item = extract_direct_media(&format!("http://{addr}/misnamed.jpg")).unwrap();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let opts = DownloadOptions {
        retries: 0,
        ..options(dir.clone())
    };

    let error = download(&client, &item, &opts).await.unwrap_err();
    assert!(error.to_string().contains("incompatible Content-Type"));
    assert!(!dir.join("misnamed.jpg").exists());
    assert!(!dir.join("misnamed.jpg.part").exists());
    server.await.unwrap();
    fs::remove_dir_all(dir).unwrap();
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
async fn direct_asf_media_matches_its_response_mime() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0_u8; 512];
        assert!(stream.read(&mut request).await.unwrap() > 0);
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/vnd.ms-asf\r\nContent-Length: 4\r\nConnection: close\r\n\r\nASF0")
            .await
            .unwrap();
    });
    let dir = test_dir("asf");
    let _ = fs::remove_dir_all(&dir);
    let item = extract_direct_media(&format!("http://{addr}/recording.asf")).unwrap();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    assert_eq!(
        download(&client, &item, &options(dir.clone()))
            .await
            .unwrap(),
        DownloadOutcome::Saved(dir.join("recording.asf"))
    );
    assert_eq!(fs::read(dir.join("recording.asf")).unwrap(), b"ASF0");
    server.await.unwrap();
    fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn no_mtime_ignores_http_last_modified_header() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0_u8; 1024];
        assert!(stream.read(&mut request).await.unwrap() > 0);
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nLast-Modified: Wed, 21 Oct 2015 07:28:00 GMT\r\nContent-Length: 4\r\nConnection: close\r\n\r\nJPEG")
            .await
            .unwrap();
    });
    let dir = test_dir("no-mtime");
    let _ = fs::remove_dir_all(&dir);
    let url = format!("http://{addr}/photo.jpg");
    let output_dir = dir.clone();
    let output = tokio::task::spawn_blocking(move || {
        Command::new(env!("CARGO_BIN_EXE_galerith"))
            .args(["--no-mtime", "-d"])
            .arg(output_dir)
            .arg(url)
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    server.await.unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let modified = fs::metadata(dir.join("photo.jpg"))
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    assert!(
        modified > 1_450_000_000,
        "old Last-Modified was applied: {modified}"
    );
    fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn no_part_writes_directly_to_the_output_path() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0_u8; 1024];
        assert!(stream.read(&mut request).await.unwrap() > 0);
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: 4\r\nConnection: close\r\n\r\nJPEG",
            )
            .await
            .unwrap();
    });
    let dir = test_dir("no-part");
    let _ = fs::remove_dir_all(&dir);
    let url = format!("http://{addr}/photo.jpg");
    let output_dir = dir.clone();
    let output = tokio::task::spawn_blocking(move || {
        Command::new(env!("CARGO_BIN_EXE_galerith"))
            .args(["--no-part", "-d"])
            .arg(output_dir)
            .arg(url)
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    server.await.unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read(dir.join("photo.jpg")).unwrap(), b"JPEG");
    assert!(!dir.join("photo.jpg.part").exists());
    assert!(!dir.join("photo.jpg.part.json").exists());
    fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn failed_no_part_transfer_leaves_a_partial_at_the_requested_output_path() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0_u8; 1024];
        assert!(stream.read(&mut request).await.unwrap() > 0);
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: 8\r\nConnection: close\r\n\r\nBAD",
            )
            .await
            .unwrap();
    });
    let dir = test_dir("no-part-incomplete");
    let _ = fs::remove_dir_all(&dir);
    let url = format!("http://{addr}/photo.jpg");
    let output_dir = dir.clone();
    let output = tokio::task::spawn_blocking(move || {
        Command::new(env!("CARGO_BIN_EXE_galerith"))
            .args(["--no-part", "--retries", "0", "-d"])
            .arg(output_dir)
            .arg(url)
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    server.await.unwrap();
    assert!(!output.status.success());
    assert_eq!(fs::read(dir.join("photo.jpg")).unwrap(), b"BAD");
    assert!(!dir.join("photo.jpg.part").exists());
    fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn limit_rate_paces_body_bytes_at_the_configured_average() {
    assert_limit_rate_paces_body_bytes("8k", "limited-rate", 900).await;
}

#[tokio::test]
async fn ranged_limit_rate_paces_body_bytes_at_the_fastest_configured_rate() {
    assert_limit_rate_paces_body_bytes("4k-8k", "limited-rate-range", 900).await;
}

async fn assert_limit_rate_paces_body_bytes(
    rate: &'static str,
    label: &'static str,
    minimum_ms: u64,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0_u8; 1024];
        assert!(stream.read(&mut request).await.unwrap() > 0);
        let body = vec![b'X'; 8192];
        stream
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        stream.write_all(&body).await.unwrap();
    });
    let dir = test_dir(label);
    let _ = fs::remove_dir_all(&dir);
    let url = format!("http://{addr}/photo.jpg");
    let output_dir = dir.clone();
    let started = Instant::now();
    let output = tokio::task::spawn_blocking(move || {
        Command::new(env!("CARGO_BIN_EXE_galerith"))
            .args(["--limit-rate", rate, "-d"])
            .arg(output_dir)
            .arg(url)
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    let elapsed = started.elapsed();
    server.await.unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        elapsed >= Duration::from_millis(minimum_ms),
        "rate limit elapsed {elapsed:?}"
    );
    assert_eq!(fs::metadata(dir.join("photo.jpg")).unwrap().len(), 8192);
    fs::remove_dir_all(dir).unwrap();
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
