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
    if max_bytes == 0 {
        return Err("JSON response byte limit must be positive".into());
    }
    let mut response = client.get(endpoint).send().await?.error_for_status()?;
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
    Ok(serde_json::from_slice(&body)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

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
