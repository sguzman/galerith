//! Resumable transfers are tested exclusively against synthetic localhost HTTP.
use galerith::{
    download::{download, DownloadOptions, DownloadOutcome},
    extract_direct_media, MediaItem,
};
use std::{fs, path::PathBuf, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    time::timeout,
};

fn dir(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!("galerith-resume-{label}-{}", std::process::id()))
}
fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap()
}
fn settings(destination: PathBuf, retries: u32) -> DownloadOptions {
    DownloadOptions {
        destination,
        retries,
        no_skip: false,
    }
}
async fn request(stream: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 1024];
    loop {
        let n = stream.read(&mut buffer).await.unwrap();
        assert!(n > 0, "peer closed before request headers");
        bytes.extend_from_slice(&buffer[..n]);
        assert!(bytes.len() <= 16_384, "unexpected oversized request");
        if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            return String::from_utf8(bytes).unwrap().to_ascii_lowercase();
        }
    }
}
async fn next(listener: &TcpListener) -> TcpStream {
    let (stream, _) = timeout(Duration::from_secs(5), listener.accept())
        .await
        .expect("HTTP request timeout")
        .unwrap();
    stream
}
fn clean(path: &PathBuf) {
    let _ = fs::remove_dir_all(path);
}

#[tokio::test]
async fn redirect_disabled_and_accepted_status_are_not_completed_downloads() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        for (status, extra, body) in [
            ("302 Found", "Location: /other\r\n", b"REDIRECT".as_slice()),
            ("202 Accepted", "", b"PENDING".as_slice()),
            ("200 OK", "Content-Type: image/jpeg\r\n", b"GOOD".as_slice()),
        ] {
            let mut stream = next(&listener).await;
            let request_text = request(&mut stream).await;
            assert!(
                request_text.starts_with("get /media.jpg "),
                "{request_text}"
            );
            let response = format!(
                "HTTP/1.1 {status}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            stream.write_all(body).await.unwrap();
        }
    });
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let dest = dir("unaccepted-http-status");
    clean(&dest);
    let item = extract_direct_media(&format!("http://{addr}/media.jpg")).unwrap();
    for _ in 0..2 {
        let result = download(&client, &item, &settings(dest.clone(), 0)).await;
        assert!(result.is_err(), "unaccepted response status was saved");
        assert!(!dest.join("media.jpg").exists());
        assert!(!dest.join("media.jpg.part").exists());
        assert!(!dest.join("media.jpg.part.json").exists());
    }
    assert_eq!(
        download(&client, &item, &settings(dest.clone(), 0))
            .await
            .unwrap(),
        DownloadOutcome::Saved(dest.join("media.jpg"))
    );
    assert_eq!(fs::read(dest.join("media.jpg")).unwrap(), b"GOOD");
    server.await.unwrap();
    clean(&dest);
}

#[cfg(unix)]
#[tokio::test]
async fn preexisting_output_and_resume_aliases_cannot_overwrite_other_files() {
    use std::os::unix::fs::symlink;
    let dest = dir("output-alias-rejection");
    clean(&dest);
    fs::create_dir_all(&dest).unwrap();
    let victim = dest.join("unrelated.txt");
    fs::write(&victim, b"UNRELATED-CONTENT").unwrap();
    let media = dest.join("photo.jpg");
    let part = dest.join("photo.jpg.part");
    let sidecar = dest.join("photo.jpg.part.json");
    let item = extract_direct_media("http://127.0.0.1:1/photo.jpg").unwrap();

    for alias in [&media, &part, &sidecar] {
        symlink(&victim, alias).unwrap();
        let failed = download(&client(), &item, &settings(dest.clone(), 0)).await;
        assert!(failed.is_err(), "unexpectedly followed alias: {alias:?}");
        assert_eq!(fs::read(&victim).unwrap(), b"UNRELATED-CONTENT");
        fs::remove_file(alias).unwrap();
    }
    fs::hard_link(&victim, &part).unwrap();
    let failed = download(&client(), &item, &settings(dest.clone(), 0)).await;
    assert!(
        failed.is_err(),
        "hard-linked partial was unexpectedly trusted"
    );
    assert_eq!(fs::read(&victim).unwrap(), b"UNRELATED-CONTENT");
    fs::remove_file(part).unwrap();
    clean(&dest);
}

#[cfg(unix)]
#[tokio::test]
async fn stale_temporary_metadata_symlink_is_not_followed() {
    use std::os::unix::fs::symlink;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let mut stream = next(&listener).await;
        let request_text = request(&mut stream).await;
        assert!(request_text.contains("/media.jpg"), "{request_text}");
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nETag: \"known\"\r\nContent-Length: 4\r\nConnection: close\r\n\r\nGOOD",
            )
            .await
            .unwrap();
    });
    let dest = dir("atomic-sidecar-symlink");
    clean(&dest);
    fs::create_dir_all(&dest).unwrap();
    let victim = dest.join("unrelated.json");
    fs::write(&victim, b"IMPORTANT DATA").unwrap();
    let pending = dest.join("media.jpg.part.json.tmp");
    symlink(&victim, &pending).unwrap();
    let item = extract_direct_media(&format!("http://{addr}/media.jpg")).unwrap();
    assert_eq!(
        download(&client(), &item, &settings(dest.clone(), 0))
            .await
            .unwrap(),
        DownloadOutcome::Saved(dest.join("media.jpg"))
    );
    server.await.unwrap();
    assert_eq!(fs::read(&victim).unwrap(), b"IMPORTANT DATA");
    assert_eq!(fs::read(dest.join("media.jpg")).unwrap(), b"GOOD");
    assert!(!pending.exists());
    assert!(!dest.join("media.jpg.part.json").exists());
    clean(&dest);
}

#[tokio::test]
async fn legitimate_html_body_can_be_saved_by_downloader_library() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let mut stream = next(&listener).await;
        let received = request(&mut stream).await;
        assert!(received.contains("/index.html"), "{received}");
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 16\r\nConnection: close\r\n\r\n<html>OK</html>!",
            )
            .await
            .unwrap();
    });
    let path = dir("legitimate-html-library");
    clean(&path);
    let item = MediaItem {
        url: format!("http://{addr}/index.html"),
        filename: "index.html".into(),
        extractor: "test",
        post_id: None,
    };
    assert_eq!(
        download(&client(), &item, &settings(path.clone(), 0))
            .await
            .unwrap(),
        DownloadOutcome::Saved(path.join("index.html"))
    );
    assert_eq!(
        fs::read(path.join("index.html")).unwrap(),
        b"<html>OK</html>!"
    );
    server.await.unwrap();
    clean(&path);
}

#[tokio::test]
async fn ranged_get_receiving_a_full_200_html_error_restarts_without_stale_partial() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let mut initial = next(&listener).await;
        let req = request(&mut initial).await;
        assert!(!req.contains("\r\nrange:"), "{req}");
        initial
            .write_all(
                b"HTTP/1.1 200 OK\r\nETag: \"stable\"\r\nContent-Length: 8\r\nConnection: close\r\n\r\nABCD",
            )
            .await
            .unwrap();
        drop(initial);

        let mut blocked = next(&listener).await;
        let req = request(&mut blocked).await;
        assert!(req.contains("\r\nrange: bytes=4-\r\n"), "{req}");
        assert!(req.contains("\r\nif-range: \"stable\"\r\n"), "{req}");
        blocked
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 8\r\nConnection: close\r\n\r\n<html>NO",
            )
            .await
            .unwrap();
        drop(blocked);

        let mut recovered = next(&listener).await;
        let req = request(&mut recovered).await;
        assert!(!req.contains("\r\nrange:"), "{req}");
        assert!(!req.contains("\r\nif-range:"), "{req}");
        recovered
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: 8\r\nConnection: close\r\n\r\nGOODDATA",
            )
            .await
            .unwrap();
    });
    let path = dir("full-html-block-after-range");
    clean(&path);
    let item = extract_direct_media(&format!("http://{addr}/photo.jpg")).unwrap();
    assert_eq!(
        download(&client(), &item, &settings(path.clone(), 2))
            .await
            .unwrap(),
        DownloadOutcome::Saved(path.join("photo.jpg"))
    );
    assert_eq!(fs::read(path.join("photo.jpg")).unwrap(), b"GOODDATA");
    assert!(!path.join("photo.jpg.part").exists());
    assert!(!path.join("photo.jpg.part.json").exists());
    server.await.unwrap();
    clean(&path);
}

#[tokio::test]
async fn disguised_206_error_suffix_is_discarded_before_clean_retry() {
    // A truncated file followed by an HTML/JSON-style block-page suffix
    // must not become an apparently successful media download.
    for (label, mime) in [
        ("untyped-206-block", ""),
        (
            "octet-206-block",
            "Content-Type: application/octet-stream\r\n",
        ),
        ("typed-206-block", "Content-Type: text/html\r\n"),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mime = mime.to_owned();
        let server = tokio::spawn(async move {
            let mut initial = next(&listener).await;
            let req = request(&mut initial).await;
            assert!(!req.contains("\r\nrange:"), "{req}");
            initial
                .write_all(
                    b"HTTP/1.1 200 OK\r\nETag: \"stable\"\r\nContent-Length: 16\r\nConnection: close\r\n\r\nMEDIA123",
                )
                .await
                .unwrap();
            drop(initial);

            let mut rejected = next(&listener).await;
            let req = request(&mut rejected).await;
            assert!(req.contains("\r\nrange: bytes=8-\r\n"), "{req}");
            assert!(req.contains("\r\nif-range: \"stable\"\r\n"), "{req}");
            let header = format!(
                "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 8-15/16\r\nContent-Length: 8\r\nETag: \"stable\"\r\n{mime}Connection: close\r\n\r\n"
            );
            rejected.write_all(header.as_bytes()).await.unwrap();
            rejected.write_all(b"<html>NO").await.unwrap();
            drop(rejected);

            let mut fresh = next(&listener).await;
            let req = request(&mut fresh).await;
            assert!(!req.contains("\r\nrange:"), "{req}");
            assert!(!req.contains("\r\nif-range:"), "{req}");
            fresh
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nETag: \"new\"\r\nContent-Length: 16\r\nConnection: close\r\n\r\nMEDIA123ABCDEFGH",
                )
                .await
                .unwrap();
        });

        let path = dir(label);
        clean(&path);
        let item = extract_direct_media(&format!("http://{addr}/photo.jpg")).unwrap();
        assert_eq!(
            download(&client(), &item, &settings(path.clone(), 2))
                .await
                .unwrap(),
            DownloadOutcome::Saved(path.join("photo.jpg"))
        );
        assert_eq!(
            fs::read(path.join("photo.jpg")).unwrap(),
            b"MEDIA123ABCDEFGH"
        );
        assert!(!path.join("photo.jpg.part").exists());
        assert!(!path.join("photo.jpg.part.json").exists());
        server.await.unwrap();
        clean(&path);
    }
}

#[tokio::test]
async fn unexpected_304_or_412_during_resume_clears_partial_and_retries_full_body() {
    for (label, status) in [
        ("not-modified", "304 Not Modified"),
        ("precondition", "412 Precondition Failed"),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let response_status = status.to_owned();
        let server = tokio::spawn(async move {
            let mut first = next(&listener).await;
            let initial = request(&mut first).await;
            assert!(!initial.contains("\r\nrange:"), "{initial}");
            first
                .write_all(
                    b"HTTP/1.1 200 OK\r\nETag: \"stable\"\r\nContent-Length: 8\r\nConnection: close\r\n\r\nABCD",
                )
                .await
                .unwrap();
            drop(first);

            let mut rejected = next(&listener).await;
            let resumed = request(&mut rejected).await;
            assert!(resumed.contains("\r\nrange: bytes=4-\r\n"), "{resumed}");
            assert!(
                resumed.contains("\r\nif-range: \"stable\"\r\n"),
                "{resumed}"
            );
            let reply = format!(
                "HTTP/1.1 {response_status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            rejected.write_all(reply.as_bytes()).await.unwrap();
            drop(rejected);

            let mut fresh = next(&listener).await;
            let restarted = request(&mut fresh).await;
            assert!(!restarted.contains("\r\nrange:"), "{restarted}");
            assert!(!restarted.contains("\r\nif-range:"), "{restarted}");
            fresh
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nETag: \"new\"\r\nConnection: close\r\n\r\nWXYZQRST",
                )
                .await
                .unwrap();
        });
        let path = dir(label);
        clean(&path);
        let item = extract_direct_media(&format!("http://{addr}/media.jpg")).unwrap();
        assert_eq!(
            download(&client(), &item, &settings(path.clone(), 2))
                .await
                .unwrap(),
            DownloadOutcome::Saved(path.join("media.jpg"))
        );
        server.await.unwrap();
        assert_eq!(fs::read(path.join("media.jpg")).unwrap(), b"WXYZQRST");
        assert!(!path.join("media.jpg.part").exists());
        assert!(!path.join("media.jpg.part.json").exists());
        clean(&path);
    }
}

#[tokio::test]
async fn resumes_interrupted_http_body_with_matching_etag() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let mut first = next(&listener).await;
        let req = request(&mut first).await;
        assert!(!req.contains("\r\nrange:"));
        first.write_all(
            b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\nETag: \"same\"\r\nConnection: close\r\n\r\nABCD"
        ).await.unwrap();
        drop(first);
        let mut second = next(&listener).await;
        let req = request(&mut second).await;
        assert!(req.contains("\r\nrange: bytes=4-\r\n"), "{req}");
        assert!(req.contains("\r\nif-range: \"same\"\r\n"), "{req}");
        second.write_all(
            b"HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 4-9/10\r\nContent-Length: 6\r\nETag: \"same\"\r\nConnection: close\r\n\r\nEFGHIJ"
        ).await.unwrap();
    });
    let path = dir("same-etag");
    clean(&path);
    let item = extract_direct_media(&format!("http://{addr}/a.jpg")).unwrap();
    assert_eq!(
        download(&client(), &item, &settings(path.clone(), 2))
            .await
            .unwrap(),
        DownloadOutcome::Saved(path.join("a.jpg"))
    );
    assert_eq!(fs::read(path.join("a.jpg")).unwrap(), b"ABCDEFGHIJ");
    assert!(!path.join("a.jpg.part").exists());
    assert!(!path.join("a.jpg.part.json").exists());
    server.await.unwrap();
    clean(&path);
}

#[tokio::test]
async fn unsolicited_206_does_not_finalize_partial_and_retries_full_media() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let mut first = next(&listener).await;
        let request_text = request(&mut first).await;
        assert!(!request_text.contains("\r\nrange:"), "{request_text}");
        first
            .write_all(
                b"HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 1-3/4\r\nContent-Length: 3\r\nConnection: close\r\n\r\nOOD",
            )
            .await
            .unwrap();
        drop(first);
        let mut second = next(&listener).await;
        let request_text = request(&mut second).await;
        assert!(!request_text.contains("\r\nrange:"), "{request_text}");
        second
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nGOOD")
            .await
            .unwrap();
    });
    let path = dir("unsolicited-206");
    clean(&path);
    fs::create_dir_all(&path).unwrap();
    fs::write(path.join("unsafe.jpg.part"), b"stale data").unwrap();
    let item = extract_direct_media(&format!("http://{addr}/unsafe.jpg")).unwrap();
    assert_eq!(
        download(&client(), &item, &settings(path.clone(), 1))
            .await
            .unwrap(),
        DownloadOutcome::Saved(path.join("unsafe.jpg"))
    );
    assert_eq!(fs::read(path.join("unsafe.jpg")).unwrap(), b"GOOD");
    assert!(!path.join("unsafe.jpg.part").exists());
    assert!(!path.join("unsafe.jpg.part.json").exists());
    server.await.unwrap();
    clean(&path);
}

#[tokio::test]
async fn bounded_206_subranges_continue_to_final_total_without_publishing_intermediate_file() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let mut first = next(&listener).await;
        let text = request(&mut first).await;
        assert!(!text.contains("\r\nrange:"), "{text}");
        first
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\nETag: \"chunked\"\r\nConnection: close\r\n\r\nABCD",
            )
            .await
            .unwrap();
        drop(first);

        let mut middle = next(&listener).await;
        let text = request(&mut middle).await;
        assert!(text.contains("\r\nrange: bytes=4-\r\n"), "{text}");
        assert!(text.contains("\r\nif-range: \"chunked\"\r\n"), "{text}");
        middle
            .write_all(
                b"HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 4-6/10\r\nContent-Length: 3\r\nETag: \"chunked\"\r\nConnection: close\r\n\r\nEFG",
            )
            .await
            .unwrap();
        drop(middle);

        let mut last = next(&listener).await;
        let text = request(&mut last).await;
        assert!(text.contains("\r\nrange: bytes=7-\r\n"), "{text}");
        assert!(text.contains("\r\nif-range: \"chunked\"\r\n"), "{text}");
        last
            .write_all(
                b"HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 7-9/10\r\nContent-Length: 3\r\nETag: \"chunked\"\r\nConnection: close\r\n\r\nHIJ",
            )
            .await
            .unwrap();
    });
    let path = dir("bounded-206-chunks");
    clean(&path);
    let item = extract_direct_media(&format!("http://{addr}/image.jpg")).unwrap();
    assert_eq!(
        download(&client(), &item, &settings(path.clone(), 2))
            .await
            .unwrap(),
        DownloadOutcome::Saved(path.join("image.jpg"))
    );
    assert_eq!(fs::read(path.join("image.jpg")).unwrap(), b"ABCDEFGHIJ");
    assert!(!path.join("image.jpg.part").exists());
    assert!(!path.join("image.jpg.part.json").exists());
    server.await.unwrap();
    clean(&path);
}

#[tokio::test]
async fn bounded_206_subrange_exhaustion_persists_validator_and_resumes_next_invocation() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let mut first = next(&listener).await;
        let text = request(&mut first).await;
        assert!(!text.contains("\r\nrange:"), "{text}");
        first
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\nETag: \"same\"\r\nConnection: close\r\n\r\nABCD",
            )
            .await
            .unwrap();
        drop(first);

        let mut middle = next(&listener).await;
        let text = request(&mut middle).await;
        assert!(text.contains("\r\nrange: bytes=4-\r\n"), "{text}");
        middle
            .write_all(
                b"HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 4-6/10\r\nContent-Length: 3\r\nETag: \"same\"\r\nConnection: close\r\n\r\nEFG",
            )
            .await
            .unwrap();
        drop(middle);

        let mut last = next(&listener).await;
        let text = request(&mut last).await;
        assert!(text.contains("\r\nrange: bytes=7-\r\n"), "{text}");
        assert!(text.contains("\r\nif-range: \"same\"\r\n"), "{text}");
        last
            .write_all(
                b"HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 7-9/10\r\nContent-Length: 3\r\nETag: \"same\"\r\nConnection: close\r\n\r\nHIJ",
            )
            .await
            .unwrap();
    });
    let path = dir("bounded-206-process-resume");
    clean(&path);
    let item = extract_direct_media(&format!("http://{addr}/image.jpg")).unwrap();
    let limited = download(&client(), &item, &settings(path.clone(), 1)).await;
    assert!(
        limited.is_err(),
        "subrange must not be finalized or archived"
    );
    assert!(!path.join("image.jpg").exists());
    assert_eq!(fs::read(path.join("image.jpg.part")).unwrap(), b"ABCDEFG");
    let sidecar = fs::read_to_string(path.join("image.jpg.part.json")).unwrap();
    assert!(sidecar.contains("same"));
    assert_eq!(
        download(&client(), &item, &settings(path.clone(), 0))
            .await
            .unwrap(),
        DownloadOutcome::Saved(path.join("image.jpg"))
    );
    assert_eq!(fs::read(path.join("image.jpg")).unwrap(), b"ABCDEFGHIJ");
    server.await.unwrap();
    clean(&path);
}

#[tokio::test]
async fn overlong_206_suffix_is_discarded_and_restarted_without_range() {
    for (label, length_header) in [
        ("declared-length-overrun", "Content-Length: 8\r\n"),
        ("streamed-length-overrun", ""),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let mut initial = next(&listener).await;
            let req = request(&mut initial).await;
            assert!(!req.contains("\r\nrange:"), "{req}");
            initial
                .write_all(
                    b"HTTP/1.1 200 OK\r\nETag: \"same\"\r\nContent-Length: 10\r\nConnection: close\r\n\r\nABCD",
                )
                .await
                .unwrap();
            drop(initial);

            let mut invalid = next(&listener).await;
            let req = request(&mut invalid).await;
            assert!(req.contains("\r\nrange: bytes=4-\r\n"), "{req}");
            assert!(req.contains("\r\nif-range: \"same\"\r\n"), "{req}");
            let header = format!(
                "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 4-9/10\r\n{length_header}ETag: \"same\"\r\nConnection: close\r\n\r\n"
            );
            invalid.write_all(header.as_bytes()).await.unwrap();
            let _ = invalid.write_all(b"EFGHIJKL").await;
            drop(invalid);

            let mut fresh = next(&listener).await;
            let req = request(&mut fresh).await;
            assert!(!req.contains("\r\nrange:"), "{req}");
            assert!(!req.contains("\r\nif-range:"), "{req}");
            fresh
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\nETag: \"fresh\"\r\nConnection: close\r\n\r\nGOODABCDEF",
                )
                .await
                .unwrap();
        });
        let path = dir(label);
        clean(&path);
        let item = extract_direct_media(&format!("http://{addr}/image.jpg")).unwrap();
        assert_eq!(
            download(&client(), &item, &settings(path.clone(), 2))
                .await
                .unwrap(),
            DownloadOutcome::Saved(path.join("image.jpg"))
        );
        assert_eq!(fs::read(path.join("image.jpg")).unwrap(), b"GOODABCDEF");
        assert!(!path.join("image.jpg.part").exists());
        assert!(!path.join("image.jpg.part.json").exists());
        server.await.unwrap();
        clean(&path);
    }
}

#[tokio::test]
async fn missing_content_range_recovers_within_retry_budget() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let mut first = next(&listener).await;
        let req = request(&mut first).await;
        assert!(!req.contains("\r\nrange:"));
        first.write_all(
            b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nETag: \"old\"\r\nConnection: close\r\n\r\n1234",
        )
        .await
        .unwrap();
        drop(first);

        let mut second = next(&listener).await;
        let req = request(&mut second).await;
        assert!(req.contains("\r\nrange: bytes=4-\r\n"), "{req}");
        assert!(req.contains("\r\nif-range: \"old\"\r\n"), "{req}");
        second
            .write_all(
                b"HTTP/1.1 206 Partial Content\r\nContent-Length: 4\r\nETag: \"old\"\r\nConnection: close\r\n\r\n5678",
            )
            .await
            .unwrap();
        drop(second);

        let mut third = next(&listener).await;
        let req = request(&mut third).await;
        assert!(!req.contains("\r\nrange:"), "{req}");
        assert!(!req.contains("\r\nif-range:"), "{req}");
        third
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nETag: \"new\"\r\nConnection: close\r\n\r\nABCDEFGH",
            )
            .await
            .unwrap();
    });
    let path = dir("missing-content-range");
    clean(&path);
    let item = extract_direct_media(&format!("http://{addr}/range.jpg")).unwrap();
    assert_eq!(
        download(&client(), &item, &settings(path.clone(), 2))
            .await
            .unwrap(),
        DownloadOutcome::Saved(path.join("range.jpg"))
    );
    assert_eq!(fs::read(path.join("range.jpg")).unwrap(), b"ABCDEFGH");
    assert!(!path.join("range.jpg.part").exists());
    assert!(!path.join("range.jpg.part.json").exists());
    server.await.unwrap();
    clean(&path);
}

#[tokio::test]
async fn retryable_rate_limit_preserves_resumable_partial_and_validator() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let mut first = next(&listener).await;
        let req = request(&mut first).await;
        assert!(!req.contains("\r\nrange:"));
        first
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nETag: \"stable\"\r\nConnection: close\r\n\r\nABCD",
            )
            .await
            .unwrap();
        drop(first);

        let mut limited = next(&listener).await;
        let req = request(&mut limited).await;
        assert!(req.contains("\r\nrange: bytes=4-\r\n"), "{req}");
        assert!(req.contains("\r\nif-range: \"stable\"\r\n"), "{req}");
        limited
            .write_all(
                b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 0\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();
        drop(limited);

        let mut resumed = next(&listener).await;
        let req = request(&mut resumed).await;
        assert!(req.contains("\r\nrange: bytes=4-\r\n"), "{req}");
        assert!(req.contains("\r\nif-range: \"stable\"\r\n"), "{req}");
        resumed
            .write_all(
                b"HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 4-7/8\r\nContent-Length: 4\r\nETag: \"stable\"\r\nConnection: close\r\n\r\nEFGH",
            )
            .await
            .unwrap();
    });
    let path = dir("429-validated-partial");
    clean(&path);
    let item = extract_direct_media(&format!("http://{addr}/429.jpg")).unwrap();
    assert_eq!(
        download(&client(), &item, &settings(path.clone(), 2))
            .await
            .unwrap(),
        DownloadOutcome::Saved(path.join("429.jpg"))
    );
    assert_eq!(fs::read(path.join("429.jpg")).unwrap(), b"ABCDEFGH");
    assert!(!path.join("429.jpg.part").exists());
    assert!(!path.join("429.jpg.part.json").exists());
    server.await.unwrap();
    clean(&path);
}

#[tokio::test]
async fn weak_etag_without_last_modified_cannot_authorize_resume() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let mut first = next(&listener).await;
        let req = request(&mut first).await;
        assert!(!req.contains("\r\nrange:"));
        first
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nETag: W/\"weak\"\r\nConnection: close\r\n\r\n1234",
            )
            .await
            .unwrap();
        drop(first);

        let mut second = next(&listener).await;
        let req = request(&mut second).await;
        assert!(!req.contains("\r\nrange:"), "{req}");
        assert!(!req.contains("\r\nif-range:"), "{req}");
        second
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nETag: \"strong\"\r\nConnection: close\r\n\r\nABCDEFGH",
            )
            .await
            .unwrap();
    });
    let path = dir("weak-validator");
    clean(&path);
    let item = extract_direct_media(&format!("http://{addr}/weak.jpg")).unwrap();
    assert_eq!(
        download(&client(), &item, &settings(path.clone(), 1))
            .await
            .unwrap(),
        DownloadOutcome::Saved(path.join("weak.jpg"))
    );
    assert_eq!(fs::read(path.join("weak.jpg")).unwrap(), b"ABCDEFGH");
    assert!(!path.join("weak.jpg.part").exists());
    assert!(!path.join("weak.jpg.part.json").exists());
    server.await.unwrap();
    clean(&path);
}

#[tokio::test]
async fn persists_verified_partial_across_invocations() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let mut first = next(&listener).await;
        request(&mut first).await;
        first.write_all(
            b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nLast-Modified: Wed, 21 Oct 2015 07:28:00 GMT\r\nConnection: close\r\n\r\n12"
        ).await.unwrap();
        first.flush().await.unwrap();
        tokio::time::sleep(Duration::from_millis(80)).await;
        drop(first);
        let mut second = next(&listener).await;
        let req = request(&mut second).await;
        assert!(req.contains("\r\nrange: bytes=2-\r\n"), "{req}");
        assert!(
            req.contains("\r\nif-range: wed, 21 oct 2015 07:28:00 gmt\r\n"),
            "{req}"
        );
        second.write_all(
            b"HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 2-7/8\r\nContent-Length: 6\r\nConnection: close\r\n\r\n345678"
        ).await.unwrap();
    });
    let path = dir("cross-run");
    clean(&path);
    let item = extract_direct_media(&format!("http://{addr}/b.png")).unwrap();
    assert!(download(&client(), &item, &settings(path.clone(), 0))
        .await
        .is_err());
    assert_eq!(fs::read(path.join("b.png.part")).unwrap(), b"12");
    assert!(path.join("b.png.part.json").exists());
    assert!(!path.join("b.png.part.json.tmp").exists());
    assert_eq!(
        download(&client(), &item, &settings(path.clone(), 0))
            .await
            .unwrap(),
        DownloadOutcome::Saved(path.join("b.png"))
    );
    assert_eq!(fs::read(path.join("b.png")).unwrap(), b"12345678");
    let expected = httpdate::parse_http_date("Wed, 21 Oct 2015 07:28:00 GMT").unwrap();
    let actual = fs::metadata(path.join("b.png"))
        .unwrap()
        .modified()
        .unwrap();
    assert!(actual.duration_since(expected).unwrap() < Duration::from_secs(2));
    server.await.unwrap();
    clean(&path);
}

#[tokio::test]
async fn ignores_outdated_partial_when_server_sends_full_body() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let mut first = next(&listener).await;
        request(&mut first).await;
        first.write_all(
            b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nETag: \"old\"\r\nConnection: close\r\n\r\nAA"
        ).await.unwrap();
        drop(first);
        let mut second = next(&listener).await;
        let req = request(&mut second).await;
        assert!(req.contains("\r\nrange: bytes=2-\r\n"));
        second.write_all(
            b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nETag: \"new\"\r\nConnection: close\r\n\r\nZZZZZZ"
        ).await.unwrap();
    });
    let path = dir("ifrange-fallback");
    clean(&path);
    let item = extract_direct_media(&format!("http://{addr}/c.webp")).unwrap();
    download(&client(), &item, &settings(path.clone(), 2))
        .await
        .unwrap();
    assert_eq!(fs::read(path.join("c.webp")).unwrap(), b"ZZZZZZ");
    server.await.unwrap();
    clean(&path);
}

#[tokio::test]
async fn avoids_appending_unowned_partial_without_sidecar() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let mut stream = next(&listener).await;
        let req = request(&mut stream).await;
        assert!(!req.contains("\r\nrange:"));
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nGOOD")
            .await
            .unwrap();
    });
    let path = dir("orphan");
    clean(&path);
    fs::create_dir_all(&path).unwrap();
    fs::write(path.join("orphan.jpg.part"), b"OLD PARTIAL CONTENT").unwrap();
    let item = extract_direct_media(&format!("http://{addr}/orphan.jpg")).unwrap();
    download(&client(), &item, &settings(path.clone(), 0))
        .await
        .unwrap();
    assert_eq!(fs::read(path.join("orphan.jpg")).unwrap(), b"GOOD");
    server.await.unwrap();
    clean(&path);
}

#[tokio::test]
async fn rejects_path_traversal_in_public_media_item() {
    let path = dir("unsafe");
    clean(&path);
    let item = MediaItem {
        url: "https://example.test/hello.jpg".into(),
        filename: "../outside.jpg".into(),
        extractor: "directlink",
        post_id: None,
    };
    let err = download(&client(), &item, &settings(path.clone(), 0))
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("unsafe output filename"));
    assert!(!path.exists());
}
