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
    if max_bytes == 0 {
        return Err("JSON response byte limit must be positive".into());
    }
    let mut response = client.get(endpoint).send().await?.error_for_status()?;
    let final_url = response.url().clone();
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
    Ok((serde_json::from_slice(&body)?, final_url))
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
