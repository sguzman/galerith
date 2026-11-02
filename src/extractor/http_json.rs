//! A bounded public API JSON reader for extractor previews.
//! Stop reading response bodies before a remote endpoint can exhaust memory.
//! Authentication, redirect policy and exact upstream limits are separate work.

use reqwest::Client;
use serde_json::Value;
use std::error::Error;
use url::Url;

pub async fn fetch_json_limited(
    client: &Client,
    endpoint: Url,
    max_bytes: usize,
) -> Result<Value, Box<dyn Error + Send + Sync>> {
    if crate::cache::Cache::allows(&endpoint) {
        if let Some(cache) = crate::cache::Cache::current() {
            return cache.fetch_json(client, endpoint, max_bytes).await;
        }
    }
    fetch_json_uncached(client, endpoint, max_bytes).await
}

pub(crate) async fn fetch_json_uncached(
    client: &Client,
    endpoint: Url,
    max_bytes: usize,
) -> Result<Value, Box<dyn Error + Send + Sync>> {
    Ok(
        fetch_json_uncached_with_final_url(client, endpoint, max_bytes)
            .await?
            .0,
    )
}

pub(crate) async fn fetch_json_uncached_with_final_url(
    client: &Client,
    endpoint: Url,
    max_bytes: usize,
) -> Result<(Value, Url), Box<dyn Error + Send + Sync>> {
    fetch_json_uncached_with_final_url_inner(client, endpoint, max_bytes, None).await
}

pub(crate) async fn fetch_json_uncached_from_origin(
    client: &Client,
    endpoint: Url,
    max_bytes: usize,
) -> Result<Value, Box<dyn Error + Send + Sync>> {
    let (value, _) = fetch_json_uncached_with_final_url_inner(
        client,
        endpoint.clone(),
        max_bytes,
        Some(endpoint),
    )
    .await?;
    Ok(value)
}

async fn fetch_json_uncached_with_final_url_inner(
    client: &Client,
    endpoint: Url,
    max_bytes: usize,
    required_origin: Option<Url>,
) -> Result<(Value, Url), Box<dyn Error + Send + Sync>> {
    if max_bytes == 0 {
        return Err("JSON response byte limit must be positive".into());
    }
    crate::pacing::wait_for_request_to(endpoint.as_str()).await;
    let request = client.get(endpoint).build()?;
    let request_method = request.method().clone();
    let request_url = request.url().clone();
    let mut request_headers = crate::http_diagnostics::default_request_headers();
    request_headers.extend(request.headers().clone());
    let mut response = client.execute(request).await?;
    let final_url = response.url().clone();
    if required_origin
        .as_ref()
        .is_some_and(|expected| final_url.origin() != expected.origin())
    {
        return Err("extractor API response left its source origin".into());
    }
    let response_info = crate::http_diagnostics::ResponsePageInfo {
        method: request_method,
        request_url,
        request_headers,
        status: response.status(),
        response_headers: response.headers().clone(),
    };
    if let Err(status_error) = response.error_for_status_ref() {
        if crate::http_diagnostics::response_page_dump_enabled() {
            if let Ok(body) = read_response_body_limited(&mut response, max_bytes).await {
                if let Err(error) = crate::http_diagnostics::dump_response_page(
                    &final_url,
                    &body,
                    Some(&response_info),
                )
                .await
                {
                    eprintln!("galerith: response page dump failed ({})", error.kind());
                }
            }
        }
        return Err(status_error.into());
    }
    let body = read_response_body_limited(&mut response, max_bytes).await?;
    if let Err(error) =
        crate::http_diagnostics::dump_response_page(&final_url, &body, Some(&response_info)).await
    {
        eprintln!("galerith: response page dump failed ({})", error.kind());
    }
    Ok((serde_json::from_slice(&body)?, final_url))
}

async fn read_response_body_limited(
    response: &mut reqwest::Response,
    max_bytes: usize,
) -> Result<Vec<u8>, Box<dyn Error + Send + Sync>> {
    if response
        .content_length()
        .is_some_and(|size| size > max_bytes as u64)
    {
        return Err("extractor JSON response exceeds byte limit".into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > max_bytes.saturating_sub(body.len()) {
            return Err("extractor JSON response exceeds byte limit".into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    #[tokio::test]
    async fn shared_reader_uses_opt_in_pool_cache_without_network() {
        let path = std::env::temp_dir().join(format!(
            "galerith-reader-cache-{}.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let cache = crate::cache::Cache::open(&path, 300).unwrap();
        let endpoint = Url::parse("https://e621.net/pools/88.json").unwrap();
        cache
            .fetch(&endpoint, 1024, || async {
                Ok(serde_json::json!({"post_ids":[88,89]}))
            })
            .await
            .unwrap();
        // Explicit localhost proxy prevents outgoing requests. A cached hit
        // through the actual shared extractor reader must still succeed.
        let client = Client::builder()
            .proxy(reqwest::Proxy::all("http://127.0.0.1:1").unwrap())
            .timeout(std::time::Duration::from_millis(200))
            .build()
            .unwrap();
        let hit = cache
            .clone()
            .scope(fetch_json_limited(&client, endpoint.clone(), 1024))
            .await
            .unwrap();
        assert_eq!(hit["post_ids"][1], 89);
        assert!(fetch_json_limited(&client, endpoint, 1024).await.is_err());
        drop(cache);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn uncached_http_reader_exposes_redirect_destination() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (expected, reply) in [
                (
                    "/initial",
                    "HTTP/1.1 302 Found\r\nLocation: /final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                ),
                (
                    "/final",
                    "HTTP/1.1 200 OK\r\nContent-Length: 17\r\nConnection: close\r\n\r\n{\"post_ids\":[88]}",
                ),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = [0_u8; 1024];
                let n = socket.read(&mut bytes).await.unwrap();
                assert!(
                    String::from_utf8_lossy(&bytes[..n]).contains(expected),
                    "unexpected request route"
                );
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let start = Url::parse(&format!("http://{address}/initial")).unwrap();
        let (value, final_url) =
            fetch_json_uncached_with_final_url(&Client::new(), start.clone(), 128)
                .await
                .unwrap();
        assert_eq!(value["post_ids"][0], 88);
        assert_ne!(start, final_url);
        assert_eq!(final_url.path(), "/final");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn origin_guard_rejects_redirect_before_saving_private_body() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for reply in [
                format!(
                    "HTTP/1.1 302 Found\r\nLocation: http://localhost:{}/private\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    address.port()
                ),
                "HTTP/1.1 200 OK\r\nContent-Length: 21\r\nConnection: close\r\n\r\nprivate redirect body"
                    .to_owned(),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0; 1024];
                assert!(socket.read(&mut request).await.unwrap() > 0);
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let directory =
            std::env::temp_dir().join(format!("galerith-http-json-origin-{}", std::process::id()));
        let _ = tokio::fs::remove_dir_all(&directory).await;
        tokio::fs::create_dir_all(&directory).await.unwrap();
        let endpoint = Url::parse(&format!("http://{address}/start")).unwrap();
        let result = crate::http_diagnostics::scope_response_pages(
            directory.clone(),
            crate::http_diagnostics::ResponsePageMode::All,
            async { fetch_json_uncached_from_origin(&Client::new(), endpoint, 1024).await },
        )
        .await;
        assert!(result.is_err());
        server.await.unwrap();
        let mut entries = tokio::fs::read_dir(&directory).await.unwrap();
        assert!(entries.next_entry().await.unwrap().is_none());
        tokio::fs::remove_dir_all(directory).await.unwrap();
    }

    #[tokio::test]
    async fn shared_json_reader_writes_body_only_when_page_dump_is_scoped() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 1024];
            assert!(stream.read(&mut request).await.unwrap() > 0);
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 17\r\nConnection: close\r\n\r\n{\"post_ids\":[88]}",
                )
                .await
                .unwrap();
        });
        let directory =
            std::env::temp_dir().join(format!("galerith-http-json-pages-{}", std::process::id()));
        let _ = tokio::fs::remove_dir_all(&directory).await;
        tokio::fs::create_dir_all(&directory).await.unwrap();
        let endpoint = Url::parse(&format!("http://{address}/page.json")).unwrap();
        let client = Client::new();
        let value = crate::http_diagnostics::scope_response_pages(
            directory.clone(),
            crate::http_diagnostics::ResponsePageMode::Body,
            async { fetch_json_uncached(&client, endpoint, 1024).await.unwrap() },
        )
        .await;
        assert_eq!(value["post_ids"][0], 88);
        server.await.unwrap();
        let mut entries = tokio::fs::read_dir(&directory).await.unwrap();
        let path = entries.next_entry().await.unwrap().unwrap().path();
        assert!(entries.next_entry().await.unwrap().is_none());
        assert!(path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .ends_with("_response.txt"));
        assert_eq!(
            tokio::fs::read(&path).await.unwrap(),
            b"{\"post_ids\":[88]}"
        );
        tokio::fs::remove_dir_all(directory).await.unwrap();
    }

    #[tokio::test]
    async fn shared_json_reader_all_mode_captures_redacted_response_headers() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let count = stream.read(&mut buffer).await.unwrap();
                assert_ne!(count, 0);
                request.extend_from_slice(&buffer[..count]);
            }
            let request = String::from_utf8_lossy(&request).to_ascii_lowercase();
            assert!(request.contains("x-diagnostic-token: client-secret"));
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nX-Diagnostic: fixture\r\nSet-Cookie: sid=private-secret; HttpOnly\r\nContent-Length: 17\r\nConnection: close\r\n\r\n{\"post_ids\":[88]}",
                )
                .await
                .unwrap();
        });
        let directory = std::env::temp_dir().join(format!(
            "galerith-http-json-pages-all-{}",
            std::process::id()
        ));
        let _ = tokio::fs::remove_dir_all(&directory).await;
        tokio::fs::create_dir_all(&directory).await.unwrap();
        let endpoint =
            Url::parse(&format!("http://{address}/page.json?token=request-secret")).unwrap();
        let mut default_headers = reqwest::header::HeaderMap::new();
        let mut diagnostic_token = reqwest::header::HeaderValue::from_static("client-secret");
        diagnostic_token.set_sensitive(true);
        default_headers.insert("x-diagnostic-token", diagnostic_token);
        let client = Client::builder()
            .default_headers(default_headers.clone())
            .build()
            .unwrap();
        let value = crate::http_diagnostics::scope_response_pages(
            directory.clone(),
            crate::http_diagnostics::ResponsePageMode::All,
            crate::http_diagnostics::scope_default_request_headers(default_headers, async {
                fetch_json_uncached(&client, endpoint, 1024).await.unwrap()
            }),
        )
        .await;
        assert_eq!(value["post_ids"][0], 88);
        server.await.unwrap();
        let mut entries = tokio::fs::read_dir(&directory).await.unwrap();
        let path = entries.next_entry().await.unwrap().unwrap().path();
        assert!(entries.next_entry().await.unwrap().is_none());
        let contents = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(contents.contains("GET http://127.0.0.1:"));
        assert!(contents.contains("Status: 200 OK"));
        assert!(contents.contains("x-diagnostic: fixture"));
        assert!(contents.contains("set-cookie: ***"));
        assert!(contents.contains("content-type: application/json"));
        assert!(contents.contains("x-diagnostic-token: ***"));
        assert!(!contents.contains("client-secret"));
        assert!(!contents.contains("request-secret"));
        assert!(!contents.contains("private-secret"));
        assert!(contents.ends_with("{\"post_ids\":[88]}"));
        tokio::fs::remove_dir_all(directory).await.unwrap();
    }

    #[tokio::test]
    async fn all_mode_dumps_bounded_error_responses_before_returning_status_error() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 1024];
            assert!(stream.read(&mut request).await.unwrap() > 0);
            stream
                .write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: 19\r\nConnection: close\r\n\r\n{\"error\":\"missing\"}",
                )
                .await
                .unwrap();
        });
        let directory = std::env::temp_dir().join(format!(
            "galerith-http-json-pages-error-{}",
            std::process::id()
        ));
        let _ = tokio::fs::remove_dir_all(&directory).await;
        tokio::fs::create_dir_all(&directory).await.unwrap();
        let endpoint = Url::parse(&format!("http://{address}/missing.json")).unwrap();
        let client = Client::new();
        let result = crate::http_diagnostics::scope_response_pages(
            directory.clone(),
            crate::http_diagnostics::ResponsePageMode::All,
            async { fetch_json_uncached(&client, endpoint, 128).await },
        )
        .await;
        assert!(result.unwrap_err().to_string().contains("404"));
        server.await.unwrap();
        let mut entries = tokio::fs::read_dir(&directory).await.unwrap();
        let path = entries.next_entry().await.unwrap().unwrap().path();
        assert!(entries.next_entry().await.unwrap().is_none());
        let contents = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(contents.contains("Status: 404 Not Found"));
        assert!(contents.ends_with("{\"error\":\"missing\"}"));
        tokio::fs::remove_dir_all(directory).await.unwrap();
    }

    #[tokio::test]
    async fn parses_small_json_within_limit() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 1024];
            let _ = socket.read(&mut request).await.unwrap();
            let body = r#"{"data":[1,2]}"#;
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(reply.as_bytes()).await.unwrap();
        });
        let url = Url::parse(&format!("http://{addr}/api")).unwrap();
        let value = fetch_json_limited(&Client::new(), url, 128).await.unwrap();
        assert_eq!(value["data"][1], 2);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn configured_request_delay_precedes_shared_json_api_request() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 1024];
            assert!(socket.read(&mut request).await.unwrap() > 0);
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
                .await
                .unwrap();
        });
        let endpoint = Url::parse(&format!("http://{addr}/api")).unwrap();
        let endpoint_string = endpoint.to_string();
        let client = Client::new();
        let future = fetch_json_uncached(&client, endpoint, 128);
        let started = tokio::time::Instant::now();
        let value = crate::pacing::with_request_delay_for_url(
            &endpoint_string,
            Some(crate::pacing::DelayRange::parse("0.12").unwrap()),
            future,
        )
        .await
        .unwrap();
        assert_eq!(value, serde_json::json!({}));
        assert!(started.elapsed() >= std::time::Duration::from_millis(110));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn shared_json_reader_uses_actual_endpoint_host_cooldown() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 1024];
            assert!(socket.read(&mut request).await.unwrap() > 0);
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
                .await
                .unwrap();
        });
        let endpoint = Url::parse(&format!("http://{addr}/api")).unwrap();
        let endpoint_string = endpoint.to_string();
        crate::pacing::defer_host_for_url(&endpoint_string, std::time::Duration::from_millis(120))
            .await;
        let client = Client::new();
        let future = fetch_json_uncached(&client, endpoint, 128);
        let started = tokio::time::Instant::now();
        crate::pacing::with_request_delay_for_url(&endpoint_string, None, future)
            .await
            .unwrap();
        assert!(started.elapsed() >= std::time::Duration::from_millis(110));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn request_delay_spaces_concurrent_shared_json_calls_globally() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let mut arrivals = Vec::new();
            for _ in 0..2 {
                let (mut socket, _) = listener.accept().await.unwrap();
                arrivals.push(tokio::time::Instant::now());
                let mut request = [0_u8; 1024];
                assert!(socket.read(&mut request).await.unwrap() > 0);
                socket
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
                    )
                    .await
                    .unwrap();
            }
            arrivals
        });
        let client = Client::new();
        let first = Url::parse(&format!("http://{addr}/first")).unwrap();
        let second = Url::parse(&format!("http://{addr}/second")).unwrap();
        let delay = Some(crate::pacing::DelayRange::parse("0.12").unwrap());
        let first_url = first.to_string();
        let second_url = second.to_string();
        let (first, second) = tokio::join!(
            crate::pacing::with_request_delay_for_url(
                &first_url,
                delay,
                fetch_json_uncached(&client, first, 128)
            ),
            crate::pacing::with_request_delay_for_url(
                &second_url,
                delay,
                fetch_json_uncached(&client, second, 128)
            )
        );
        assert!(first.is_ok());
        assert!(second.is_ok());
        let arrivals = server.await.unwrap();
        assert_eq!(arrivals.len(), 2);
        assert!(
            arrivals[1].duration_since(arrivals[0]) >= std::time::Duration::from_millis(100),
            "concurrent configured requests were not globally spaced"
        );
    }

    #[tokio::test]
    async fn rejects_oversized_content_length_without_body() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 1024];
            let _ = socket.read(&mut request).await.unwrap();
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 9999\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
        });
        let url = Url::parse(&format!("http://{addr}/api")).unwrap();
        let error = fetch_json_limited(&Client::new(), url, 32)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("byte limit"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn rejects_oversized_chunked_body_and_malformed_json() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for body in [r#"{"long":"abcdefghijklmnopqrst"}"#, "not json"] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0_u8; 1024];
                let _ = socket.read(&mut request).await.unwrap();
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:X}\r\n{body}\r\n0\r\n\r\n",
                    body.len()
                );
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let url = Url::parse(&format!("http://{addr}/api")).unwrap();
        let error = fetch_json_limited(&Client::new(), url.clone(), 16)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("byte limit"));
        assert!(fetch_json_limited(&Client::new(), url, 128).await.is_err());
        server.await.unwrap();
    }
}
