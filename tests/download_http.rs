//! Network tests use a loopback fixture server, never live gallery websites.
use galerith::{
    download::{download, download_with_request, DownloadOptions, DownloadOutcome, MediaRequest},
    extract_direct_media, MediaItem,
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
        chunk_size: 32 * 1024,
        validate_html: true,
        adjust_extensions: true,
        filesize_min: None,
        filesize_max: None,
        headers: reqwest::header::HeaderMap::new(),
        retry_codes: vec![],
        http_timeout: None,
        retry_delay: None,
        rate_limit_delay: None,
        download_delay: None,
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
    let mut opts = options(dir.clone());
    opts.chunk_size = 3;
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
async fn per_media_request_method_body_and_headers_reach_http_server() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut chunk = [0_u8; 512];
        loop {
            let count = stream.read(&mut chunk).await.unwrap();
            assert!(count > 0);
            request.extend_from_slice(&chunk[..count]);
            let text = String::from_utf8_lossy(&request);
            if let Some(header_end) = text.find("\r\n\r\n") {
                let length = text[..header_end]
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or_default();
                if request.len() >= header_end + 4 + length {
                    break;
                }
            }
        }
        let request_text = String::from_utf8_lossy(&request);
        assert!(request_text.starts_with("POST /file.jpg HTTP/1.1"));
        assert!(request_text
            .to_ascii_lowercase()
            .contains("x-media-mode: global"));
        assert!(!request_text
            .to_ascii_lowercase()
            .contains("x-media-mode: file-specific"));
        assert!(!request_text.to_ascii_lowercase().contains("range: bytes="));
        assert!(request.ends_with(b"token=abc"));
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nDATA")
            .await
            .unwrap();
    });
    let dir = test_dir("per-media-http-request");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("file.jpg.part"), b"stale partial").unwrap();
    let item = extract_direct_media(&format!("http://{addr}/file.jpg")).unwrap();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let mut opts = options(dir.clone());
    opts.headers
        .insert("x-media-mode", "global".parse().unwrap());
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("x-media-mode", "file-specific".parse().unwrap());
    let request = MediaRequest {
        method: reqwest::Method::POST,
        body: Some(b"token=abc".to_vec()),
        headers,
    };
    assert_eq!(
        download_with_request(&client, &item, &opts, Some(&request))
            .await
            .unwrap(),
        DownloadOutcome::Saved(dir.join("file.jpg"))
    );
    server.await.unwrap();
    assert_eq!(fs::read(dir.join("file.jpg")).unwrap(), b"DATA");
    assert!(!dir.join("file.jpg.part").exists());
    fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn extensionless_outputs_use_upstream_streaming_and_audio_mime_aliases() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let cases = [
        ("application/vnd.apple.mpegurl", "m3u8"),
        ("application/x-mpegurl", "m3u8"),
        ("application/dash+xml", "mpd"),
        ("audio/x-aac", "aac"),
        ("image/tiff", "tiff"),
        ("image/jp2", "jp2"),
        ("image/heic-sequence", "heics"),
        ("image/heif-sequence", "heifs"),
        ("image/jxl", "jxl"),
        ("image/x-portable-pixmap", "ppm"),
        ("image/x-portable-graymap", "pgm"),
        ("image/x-portable-bitmap", "pbm"),
        ("image/x-portable-anymap", "pnm"),
        ("video/x-flv", "flv"),
        ("video/x-ms-asf", "asx"),
        ("application/vnd.ms-asf", "asf"),
        ("audio/mp4", "m4a"),
        ("audio/x-aiff", "aif"),
        ("audio/basic", "au"),
    ];
    let server_cases = cases;
    let server = tokio::spawn(async move {
        for (content_type, _) in server_cases {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 256];
            loop {
                let count = stream.read(&mut buffer).await.unwrap();
                assert_ne!(count, 0, "client closed before sending HTTP headers");
                request.extend_from_slice(&buffer[..count]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let body = b"synthetic media payload";
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            stream.write_all(body).await.unwrap();
        }
    });
    let dir = test_dir("mime-alias-extension");
    let _ = fs::remove_dir_all(&dir);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    for (index, (_, expected_extension)) in cases.into_iter().enumerate() {
        let item = MediaItem {
            url: format!("http://{addr}/asset-{index}"),
            filename: format!("extensionless-output-{index}"),
            extractor: "fixture",
            post_id: None,
        };
        assert_eq!(
            download(&client, &item, &options(dir.clone()))
                .await
                .unwrap(),
            DownloadOutcome::Saved(
                dir.join(format!("extensionless-output-{index}.{expected_extension}"))
            )
        );
    }
    server.await.unwrap();
    for (index, (_, expected_extension)) in cases.into_iter().enumerate() {
        assert_eq!(
            fs::read(dir.join(format!("extensionless-output-{index}.{expected_extension}")))
                .unwrap(),
            b"synthetic media payload"
        );
    }
    fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn recognized_html_signature_corrects_a_generic_binary_response() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0_u8; 512];
        assert!(stream.read(&mut request).await.unwrap() > 0);
        let body = b"<!DOCTYPE html><html><body>public document</body></html>";
        stream
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        stream.write_all(body).await.unwrap();
    });
    let dir = test_dir("html-signature-extension");
    let _ = fs::remove_dir_all(&dir);
    let item = extract_direct_media(&format!("http://{addr}/document.jpg")).unwrap();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();

    assert_eq!(
        download(&client, &item, &options(dir.clone()))
            .await
            .unwrap(),
        DownloadOutcome::Saved(dir.join("document.html"))
    );
    assert_eq!(
        fs::read(dir.join("document.html")).unwrap(),
        b"<!DOCTYPE html><html><body>public document</body></html>"
    );
    server.await.unwrap();
    fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn zero_download_chunk_size_is_rejected_before_creating_output() {
    let dir = test_dir("zero-chunk-size");
    let _ = fs::remove_dir_all(&dir);
    let item = extract_direct_media("http://127.0.0.1:1/file.png").unwrap();
    let mut opts = options(dir.clone());
    opts.chunk_size = 0;

    let error = download(&reqwest::Client::new(), &item, &opts)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("chunk size must be positive"));
    assert!(!dir.exists());
}

#[tokio::test]
async fn content_type_adds_extension_when_source_and_custom_output_are_extensionless() {
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
    item.url = format!("http://{addr}/source");
    item.filename = "custom-output".into();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let destination = dir.join("custom-output.jpg");
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
async fn unknown_signature_with_conflicting_mime_is_saved_like_upstream() {
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

    assert_eq!(
        download(&client, &item, &opts).await.unwrap(),
        DownloadOutcome::Saved(dir.join("misnamed.jpg"))
    );
    assert_eq!(fs::read(dir.join("misnamed.jpg")).unwrap(), b"ICON");
    assert!(!dir.join("misnamed.jpg.part").exists());
    server.await.unwrap();
    fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn recognized_signature_corrects_a_misleading_extension() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0_u8; 512];
        assert!(stream.read(&mut request).await.unwrap() > 0);
        let body = b"\x89PNG\r\n\x1a\nsynthetic image";
        stream
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        stream.write_all(body).await.unwrap();
    });
    let dir = test_dir("signature-extension");
    let _ = fs::remove_dir_all(&dir);
    let item = extract_direct_media(&format!("http://{addr}/misnamed.jpg")).unwrap();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();

    assert_eq!(
        download(&client, &item, &options(dir.clone()))
            .await
            .unwrap(),
        DownloadOutcome::Saved(dir.join("misnamed.png"))
    );
    assert!(fs::read(dir.join("misnamed.png"))
        .unwrap()
        .starts_with(b"\x89PNG\r\n\x1a\n"));
    assert!(!dir.join("misnamed.jpg").exists());
    server.await.unwrap();
    fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn recognized_signature_correction_skips_an_existing_corrected_target() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0_u8; 512];
        assert!(stream.read(&mut request).await.unwrap() > 0);
        let body = b"\x89PNG\r\n\x1a\nsynthetic image";
        stream
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        stream.write_all(body).await.unwrap();
    });
    let dir = test_dir("signature-extension-collision");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("misnamed.png"), b"existing good file").unwrap();
    let item = extract_direct_media(&format!("http://{addr}/misnamed.jpg")).unwrap();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();

    assert_eq!(
        download(&client, &item, &options(dir.clone()))
            .await
            .unwrap(),
        DownloadOutcome::Skipped(dir.join("misnamed.png"))
    );
    assert_eq!(
        fs::read(dir.join("misnamed.png")).unwrap(),
        b"existing good file"
    );
    assert!(!dir.join("misnamed.jpg").exists());
    server.await.unwrap();
    fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn no_part_mode_corrects_a_completed_signature_after_direct_write() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0_u8; 512];
        assert!(stream.read(&mut request).await.unwrap() > 0);
        let body = b"\x89PNG\r\n\x1a\nsynthetic image";
        stream
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        stream.write_all(body).await.unwrap();
    });
    let dir = test_dir("signature-extension-no-part");
    let _ = fs::remove_dir_all(&dir);
    let item = extract_direct_media(&format!("http://{addr}/misnamed.jpg")).unwrap();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let mut opts = options(dir.clone());
    opts.no_part = true;

    assert_eq!(
        download(&client, &item, &opts).await.unwrap(),
        DownloadOutcome::Saved(dir.join("misnamed.png"))
    );
    assert!(dir.join("misnamed.png").exists());
    assert!(!dir.join("misnamed.jpg").exists());
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
async fn direct_wmv_media_matches_its_response_mime() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0_u8; 512];
        assert!(stream.read(&mut request).await.unwrap() > 0);
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: video/x-ms-wmv\r\nContent-Length: 4\r\nConnection: close\r\n\r\nWMV0")
            .await
            .unwrap();
    });
    let dir = test_dir("wmv");
    let _ = fs::remove_dir_all(&dir);
    let item = extract_direct_media(&format!("http://{addr}/recording.wmv")).unwrap();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    assert_eq!(
        download(&client, &item, &options(dir.clone()))
            .await
            .unwrap(),
        DownloadOutcome::Saved(dir.join("recording.wmv"))
    );
    assert_eq!(fs::read(dir.join("recording.wmv")).unwrap(), b"WMV0");
    server.await.unwrap();
    fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn direct_wma_media_matches_its_response_mime() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0_u8; 512];
        assert!(stream.read(&mut request).await.unwrap() > 0);
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: audio/x-ms-wma\r\nContent-Length: 4\r\nConnection: close\r\n\r\nWMA0")
            .await
            .unwrap();
    });
    let dir = test_dir("wma");
    let _ = fs::remove_dir_all(&dir);
    let item = extract_direct_media(&format!("http://{addr}/recording.wma")).unwrap();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    assert_eq!(
        download(&client, &item, &options(dir.clone()))
            .await
            .unwrap(),
        DownloadOutcome::Saved(dir.join("recording.wma"))
    );
    assert_eq!(fs::read(dir.join("recording.wma")).unwrap(), b"WMA0");
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
            .args(["--no-mtime", "-o", "downloader.http.mtime=true", "-d"])
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
async fn downloader_mtime_config_disables_source_last_modified() {
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
    let dir = test_dir("mtime-config");
    let _ = fs::remove_dir_all(&dir);
    let url = format!("http://{addr}/photo.jpg");
    let output_dir = dir.clone();
    let output = tokio::task::spawn_blocking(move || {
        Command::new(env!("CARGO_BIN_EXE_galerith"))
            .args(["-o", "downloader.http.mtime=false", "-d"])
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
        modified > 1_700_000_000,
        "source Last-Modified was applied despite downloader config: {modified}"
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
                b"HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: 8\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();
        stream.flush().await.unwrap();
        stream.write_all(b"BAD").await.unwrap();
        stream.flush().await.unwrap();
        // Keep the connection open long enough for the client to consume the
        // deliberately short body before the server closes it.
        tokio::time::sleep(Duration::from_millis(100)).await;
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
    assert_limit_rate_paces_body_bytes(None, Some("8k"), "limited-rate", 900).await;
}

#[tokio::test]
async fn ranged_limit_rate_paces_body_bytes_at_the_fastest_configured_rate() {
    assert_limit_rate_paces_body_bytes(None, Some("4k-8k"), "limited-rate-range", 900).await;
}

#[tokio::test]
async fn configured_downloader_rate_paces_body_bytes() {
    assert_limit_rate_paces_body_bytes(Some("8k"), None, "configured-rate", 900).await;
}

#[tokio::test]
async fn cli_limit_rate_overrides_downloader_rate_configuration() {
    assert_limit_rate_paces_body_bytes(Some("1M"), Some("8k"), "cli-rate-override", 900).await;
}

async fn assert_limit_rate_paces_body_bytes(
    configured_rate: Option<&'static str>,
    cli_rate: Option<&'static str>,
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
    let configured_rate = configured_rate.map(|rate| format!("downloader.http.rate={rate}"));
    let started = Instant::now();
    let output = tokio::task::spawn_blocking(move || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_galerith"));
        if let Some(configured_rate) = configured_rate {
            command.args(["-o", &configured_rate]);
        }
        if let Some(rate) = cli_rate {
            command.args(["--limit-rate", rate]);
        }
        command
            .args(["-d"])
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

#[tokio::test]
async fn known_content_lengths_outside_file_size_limits_are_filtered() {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    for (label, body, min, max) in [
        ("below-minimum", b"DATA".as_slice(), Some(5), None),
        ("above-maximum", b"TOO-LARGE".as_slice(), None, Some(8)),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let body = body.to_vec();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 512];
            assert!(stream.read(&mut request).await.unwrap() > 0);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            stream.write_all(&body).await.unwrap();
        });
        let dir = test_dir(label);
        let _ = fs::remove_dir_all(&dir);
        let item = extract_direct_media(&format!("http://{addr}/size.jpg")).unwrap();
        let mut opts = options(dir.clone());
        opts.filesize_min = min;
        opts.filesize_max = max;
        assert_eq!(
            download(&client, &item, &opts).await.unwrap(),
            DownloadOutcome::Filtered(dir.join("size.jpg")),
            "{label}"
        );
        assert!(
            !dir.join("size.jpg").exists(),
            "{label}: wrote filtered media"
        );
        assert!(
            !dir.join("size.jpg.part").exists(),
            "{label}: wrote partial media"
        );
        server.await.unwrap();
        fs::remove_dir_all(&dir).unwrap();
    }
}

#[tokio::test]
async fn unknown_content_length_is_not_filtered_by_declared_size_limits() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0_u8; 512];
        assert!(stream.read(&mut request).await.unwrap() > 0);
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n4\r\nDATA\r\n0\r\n\r\n",
            )
            .await
            .unwrap();
    });
    let dir = test_dir("unknown-size-limit");
    let _ = fs::remove_dir_all(&dir);
    let item = extract_direct_media(&format!("http://{addr}/unknown.jpg")).unwrap();
    let mut opts = options(dir.clone());
    opts.filesize_min = Some(5);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    assert_eq!(
        download(&client, &item, &opts).await.unwrap(),
        DownloadOutcome::Saved(dir.join("unknown.jpg"))
    );
    assert_eq!(fs::read(dir.join("unknown.jpg")).unwrap(), b"DATA");
    server.await.unwrap();
    fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn cli_file_size_limit_overrides_downloader_configuration() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        for _ in 0..3 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 512];
            assert!(stream.read(&mut request).await.unwrap() > 0);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nDATA")
                .await
                .unwrap();
        }
    });
    let dir = test_dir("filesize-config-precedence");
    let _ = fs::remove_dir_all(&dir);

    let config_only_dir = dir.join("config-only");
    let config_only_url = format!("http://{addr}/config-only.jpg");
    let output_dir = config_only_dir.clone();
    let output = tokio::task::spawn_blocking(move || {
        Command::new(env!("CARGO_BIN_EXE_galerith"))
            .args(["-o", "downloader.http.filesize-min=5", "-d"])
            .arg(output_dir)
            .arg(config_only_url)
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!config_only_dir.join("config-only.jpg").exists());

    let overridden_dir = dir.join("cli-override");
    let overridden_url = format!("http://{addr}/cli-override.jpg");
    let output_dir = overridden_dir.clone();
    let output = tokio::task::spawn_blocking(move || {
        Command::new(env!("CARGO_BIN_EXE_galerith"))
            .args([
                "-o",
                "downloader.http.filesize-min=5",
                "--filesize-min",
                "3",
                "-d",
            ])
            .arg(output_dir)
            .arg(overridden_url)
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read(overridden_dir.join("cli-override.jpg")).unwrap(),
        b"DATA"
    );

    let disabled_dir = dir.join("cli-disables-config");
    let disabled_url = format!("http://{addr}/cli-disables-config.jpg");
    let output_dir = disabled_dir.clone();
    let output = tokio::task::spawn_blocking(move || {
        Command::new(env!("CARGO_BIN_EXE_galerith"))
            .args([
                "-o",
                "downloader.http.filesize-min=5",
                "--filesize-min",
                "0",
                "-d",
            ])
            .arg(output_dir)
            .arg(disabled_url)
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read(disabled_dir.join("cli-disables-config.jpg")).unwrap(),
        b"DATA"
    );

    server.await.unwrap();
    fs::remove_dir_all(dir).unwrap();
}
