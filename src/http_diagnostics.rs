//! Opt-in, privacy-preserving response page dumps for shared extractor API calls.
use reqwest::{header::HeaderMap, Method, StatusCode};
use std::{
    future::Future,
    io,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
use tokio::{fs::OpenOptions, io::AsyncWriteExt};
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponsePageMode {
    Body,
    All,
}

pub struct ResponsePageInfo {
    pub method: Method,
    pub request_url: Url,
    pub request_headers: HeaderMap,
    pub status: StatusCode,
    pub response_headers: HeaderMap,
}

tokio::task_local! {
    static RESPONSE_PAGE_OPTIONS: (PathBuf, ResponsePageMode);
    static DEFAULT_REQUEST_HEADERS: HeaderMap;
}

static PAGE_SEQUENCE: AtomicU64 = AtomicU64::new(1);

pub async fn scope_response_pages<F: Future>(
    directory: PathBuf,
    mode: ResponsePageMode,
    future: F,
) -> F::Output {
    RESPONSE_PAGE_OPTIONS.scope((directory, mode), future).await
}

pub async fn scope_response_pages_if<F: Future>(
    options: Option<(PathBuf, ResponsePageMode)>,
    future: F,
) -> F::Output {
    match options {
        Some((directory, mode)) => scope_response_pages(directory, mode, future).await,
        None => future.await,
    }
}

pub async fn scope_default_request_headers<F: Future>(headers: HeaderMap, future: F) -> F::Output {
    DEFAULT_REQUEST_HEADERS.scope(headers, future).await
}

pub fn default_request_headers() -> HeaderMap {
    DEFAULT_REQUEST_HEADERS
        .try_with(Clone::clone)
        .unwrap_or_default()
}

pub fn default_request_headers_scope() -> Option<HeaderMap> {
    DEFAULT_REQUEST_HEADERS.try_with(Clone::clone).ok()
}

pub fn response_page_scope() -> Option<(PathBuf, ResponsePageMode)> {
    RESPONSE_PAGE_OPTIONS.try_with(Clone::clone).ok()
}

/// Save a bounded response body when the caller enabled `--write-pages`.
/// Generated filenames contain no URL, query, or credential data.
pub async fn dump_response_page(
    _url: &Url,
    body: &[u8],
    response: Option<&ResponsePageInfo>,
) -> io::Result<Option<PathBuf>> {
    let Ok((directory, mode)) = RESPONSE_PAGE_OPTIONS.try_with(Clone::clone) else {
        return Ok(None);
    };

    for _ in 0..1024 {
        let sequence = PAGE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = directory.join(format!("{sequence:02}_response.txt"));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);

        match options.open(&path).await {
            Ok(mut file) => {
                let written = async {
                    if mode == ResponsePageMode::All {
                        if let Some(response) = response {
                            let headers = format!(
                                "{} {}\nStatus: {}\n\nRequest Headers\n---------------\n{}\n\nResponse Headers\n----------------\n{}\n\n",
                                response.method,
                                redacted_url(&response.request_url),
                                response.status,
                                format_headers(&response.request_headers),
                                format_headers(&response.response_headers),
                            );
                            file.write_all(headers.as_bytes()).await?;
                        }
                    }
                    file.write_all(body).await
                }
                .await;
                if let Err(error) = written {
                    drop(file);
                    let _ = tokio::fs::remove_file(&path).await;
                    return Err(error);
                }
                if let Err(error) = file.flush().await {
                    drop(file);
                    let _ = tokio::fs::remove_file(&path).await;
                    return Err(error);
                }
                return Ok(Some(path));
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }

    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "unable to allocate a unique response page filename",
    ))
}

pub fn response_page_dump_enabled() -> bool {
    RESPONSE_PAGE_OPTIONS.try_with(|_| ()).is_ok()
}

fn format_headers(headers: &HeaderMap) -> String {
    let mut entries = headers
        .iter()
        .map(|(name, value)| {
            let name = name.as_str();
            let sensitive_value = value.is_sensitive();
            let value = value.to_str().unwrap_or("<non-text header value>");
            let lower = name.to_ascii_lowercase();
            let sensitive = sensitive_value
                || [
                    "authorization",
                    "auth",
                    "cookie",
                    "token",
                    "secret",
                    "api-key",
                    "credential",
                    "password",
                    "session",
                ]
                .iter()
                .any(|marker| lower.contains(marker))
                || matches!(lower.as_str(), "set-cookie" | "location" | "referer");
            format!("{name}: {}", if sensitive { "***" } else { value })
        })
        .collect::<Vec<_>>();
    entries.sort_unstable();
    entries.join("\n")
}

fn redacted_url(url: &Url) -> String {
    let mut safe = url.clone();
    let _ = safe.set_username("");
    let _ = safe.set_password(None);
    safe.set_query(None);
    safe.set_fragment(None);
    safe.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn response_page_dump_is_opt_in_and_does_not_put_url_data_in_filename() {
        let directory =
            std::env::temp_dir().join(format!("galerith-response-pages-{}", std::process::id()));
        let _ = tokio::fs::remove_dir_all(&directory).await;
        tokio::fs::create_dir_all(&directory).await.unwrap();
        let url = Url::parse("https://user:secret@example.test/private?token=hidden").unwrap();

        assert_eq!(
            dump_response_page(&url, b"ignored", None).await.unwrap(),
            None
        );
        let path = scope_response_pages(directory.clone(), ResponsePageMode::Body, async {
            dump_response_page(&url, b"fixture response body", None)
                .await
                .unwrap()
                .unwrap()
        })
        .await;
        assert!(path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .ends_with("_response.txt"));
        assert_eq!(
            tokio::fs::read(&path).await.unwrap(),
            b"fixture response body"
        );
        assert!(!path.to_string_lossy().contains("secret"));
        assert!(!path.to_string_lossy().contains("hidden"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                tokio::fs::metadata(&path)
                    .await
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        tokio::fs::remove_dir_all(directory).await.unwrap();
    }

    #[tokio::test]
    async fn all_mode_records_status_and_redacts_credentials_from_both_header_sets() {
        let directory = std::env::temp_dir().join(format!(
            "galerith-response-pages-all-{}",
            std::process::id()
        ));
        let _ = tokio::fs::remove_dir_all(&directory).await;
        tokio::fs::create_dir_all(&directory).await.unwrap();
        let url = Url::parse("https://example.test/private?token=url-secret").unwrap();
        let mut request_headers = HeaderMap::new();
        request_headers.insert("authorization", "Bearer request-secret".parse().unwrap());
        request_headers.insert("x-api-key", "request-key".parse().unwrap());
        let mut response_headers = HeaderMap::new();
        response_headers.insert("content-type", "application/json".parse().unwrap());
        response_headers.insert(
            "set-cookie",
            "sid=response-secret; HttpOnly".parse().unwrap(),
        );
        response_headers.insert(
            "location",
            "https://example.test/?token=redirect-secret"
                .parse()
                .unwrap(),
        );
        let info = ResponsePageInfo {
            method: Method::GET,
            request_url: url.clone(),
            request_headers,
            status: StatusCode::OK,
            response_headers,
        };
        let path = scope_response_pages(directory.clone(), ResponsePageMode::All, async {
            dump_response_page(&url, b"{}", Some(&info))
                .await
                .unwrap()
                .unwrap()
        })
        .await;
        let contents = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(contents.contains("GET https://example.test/private"));
        assert!(!contents.contains("url-secret"));
        assert!(contents.contains("Status: 200 OK"));
        assert!(contents.contains("authorization: ***"));
        assert!(contents.contains("x-api-key: ***"));
        assert!(contents.contains("set-cookie: ***"));
        assert!(contents.contains("location: ***"));
        assert!(contents.contains("content-type: application/json"));
        assert!(contents.contains("{}"));
        assert!(!contents.contains("request-secret"));
        assert!(!contents.contains("request-key"));
        assert!(!contents.contains("response-secret"));
        assert!(!contents.contains("redirect-secret"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                tokio::fs::metadata(&path)
                    .await
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        tokio::fs::remove_dir_all(directory).await.unwrap();
    }
}
