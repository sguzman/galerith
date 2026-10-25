//! Public k00.fr Koofr short-link aliases, with no untrusted redirects followed.
use super::Share;
use crate::{message::ExtractorMessage, MediaItem};
use reqwest::{header::LOCATION, redirect::Policy, Client};
use std::{error::Error, time::Duration};
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Code(String);

pub fn match_url(raw: &str) -> Option<Code> {
    if raw.contains('%') || ["/../", "/./", "/..", "/."].iter().any(|v| raw.ends_with(v) || raw.contains(v)) {
        return None;
    }
    let url = Url::parse(raw).ok()?;
    if !matches!(url.scheme(), "https" | "http")
        || url.host_str() != Some("k00.fr")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let parts: Vec<_> = url.path_segments()?.collect();
    let ([code] | [code, ""]) = parts.as_slice() else {
        return None;
    };
    if !(1..=96).contains(&code.len())
        || !code
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return None;
    }
    Some(Code((*code).to_owned()))
}

async fn short_at_base(
    no_redirect_client: &Client,
    base: &Url,
    code: &Code,
) -> Result<Share, Box<dyn Error + Send + Sync>> {
    let endpoint = base.join(&code.0)?;
    let response = no_redirect_client.get(endpoint).send().await?;
    if !response.status().is_redirection() {
        return Err("Koofr short URL is not a supported public redirect".into());
    }
    let raw_location = response
        .headers()
        .get(LOCATION)
        .ok_or("Koofr short URL has no redirect destination")?
        .to_str()?;
    if raw_location.len() > 512 {
        return Err("Koofr short redirect exceeds bounded destination size".into());
    }
    let location = Url::parse(raw_location)
        .map_err(|_| "Koofr short URL points to a nonabsolute or malformed destination")?;
    let share = super::match_url(location.as_str())
        .ok_or("Koofr short URL redirects outside a supported public Koofr link")?;
    Ok(share)
}

async fn resolve_code(code: &Code) -> Result<Share, Box<dyn Error + Send + Sync>> {
    // No cookies, credentials, or automatically followed redirects are used
    // for this untrusted short-link lookup.
    let client = Client::builder()
        .redirect(Policy::none())
        .timeout(Duration::from_secs(10))
        .build()?;
    let base = Url::parse("https://k00.fr/")?;
    short_at_base(&client, &base, code).await
}

pub async fn resolve(
    client: &Client,
    code: Code,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    super::resolve(client, resolve_code(&code).await?).await
}

pub async fn resolve_messages(
    client: &Client,
    code: Code,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>> {
    super::resolve_messages(client, resolve_code(&code).await?).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    #[test]
    fn short_codes_reject_forged_hosts_and_private_fields() {
        assert_eq!(
            match_url("https://k00.fr/Example7"),
            Some(Code("Example7".into()))
        );
        for invalid in [
            "https://evilk00.fr/Example7",
            "https://k00.fr/Example7?token=secret",
            "https://k00.fr/%2e%2e/secret",
            "https://k00.fr/../../private",
            "https://name:pass@k00.fr/Example7",
            "https://k00.fr:8443/Example7",
            "https://k00.fr/Example7/extra",
        ] {
            assert!(match_url(invalid).is_none(), "{invalid}");
        }
    }

    async fn fixture(location: &'static str) -> Result<Share, Box<dyn Error + Send + Sync>> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 4096];
            let n = stream.read(&mut request).await.unwrap();
            assert!(String::from_utf8_lossy(&request[..n]).starts_with("GET /Example7 HTTP/1.1"));
            let response = format!(
                "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        let client = Client::builder().redirect(Policy::none()).build()?;
        let base = Url::parse(&format!("http://{addr}/"))?;
        let result = short_at_base(&client, &base, &Code("Example7".into())).await;
        server.await?;
        result
    }

    #[tokio::test]
    async fn one_redirect_uses_canonical_public_link_without_following() {
        let target = fixture("https://app.koofr.net/links/1234-abc")
            .await
            .unwrap();
        assert_eq!(
            target,
            super::super::match_url("https://koofr.net/links/1234-abc").unwrap()
        );
    }

    #[tokio::test]
    async fn foreign_and_private_destinations_are_rejected_without_following() {
        assert!(fixture("https://evilkoofr.net/links/1234-abc")
            .await
            .is_err());
        assert!(
            fixture("https://app.koofr.net/links/1234-abc?password=private")
                .await
                .is_err()
        );
    }
}
