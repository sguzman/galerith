//! Opt-in, privacy-preserving response page dumps for shared extractor API calls.
use std::{
    future::Future,
    io,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
use tokio::{fs::OpenOptions, io::AsyncWriteExt};
use url::Url;

tokio::task_local! {
    static RESPONSE_PAGE_DIRECTORY: PathBuf;
}

static PAGE_SEQUENCE: AtomicU64 = AtomicU64::new(1);

pub async fn scope_response_pages<F: Future>(directory: PathBuf, future: F) -> F::Output {
    RESPONSE_PAGE_DIRECTORY.scope(directory, future).await
}

/// Save a bounded response body when the caller enabled `--write-pages`.
/// Generated filenames contain no URL, query, or credential data.
pub async fn dump_response_page(_url: &Url, body: &[u8]) -> io::Result<Option<PathBuf>> {
    let Ok(directory) = RESPONSE_PAGE_DIRECTORY.try_with(Clone::clone) else {
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
                if let Err(error) = file.write_all(body).await {
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

        assert_eq!(dump_response_page(&url, b"ignored").await.unwrap(), None);
        let path = scope_response_pages(directory.clone(), async {
            dump_response_page(&url, b"fixture response body")
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
}
