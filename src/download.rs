//! HTTP download engine. Authentication, resumed transfers and archive DB are future phases.
use crate::MediaItem;
use reqwest::{Client, StatusCode};
use std::{error::Error, path::PathBuf, time::Duration};
use tokio::{fs, io::AsyncWriteExt};

#[derive(Debug, Clone)]
pub struct DownloadOptions {
    pub destination: PathBuf,
    pub retries: u32,
    pub no_skip: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum DownloadOutcome {
    Saved(PathBuf),
    Skipped(PathBuf),
}

pub async fn download(
    client: &Client,
    item: &MediaItem,
    options: &DownloadOptions,
) -> Result<DownloadOutcome, Box<dyn Error + Send + Sync>> {
    fs::create_dir_all(&options.destination).await?;
    let target = options.destination.join(&item.filename);
    if !options.no_skip && fs::try_exists(&target).await? {
        return Ok(DownloadOutcome::Skipped(target));
    }

    let part = options.destination.join(format!("{}.part", item.filename));
    for attempt in 0..=options.retries {
        let response = client.get(&item.url).send().await;
        match response {
            Ok(response) => {
                let status = response.status();
                if (status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error())
                    && attempt < options.retries
                {
                    delay(attempt).await;
                    continue;
                }
                let mut response = response.error_for_status()?;
                let mut file = fs::File::create(&part).await?;
                let write_result: Result<(), Box<dyn Error + Send + Sync>> = async {
                    while let Some(chunk) = response.chunk().await? {
                        file.write_all(&chunk).await?;
                    }
                    file.flush().await?;
                    Ok(())
                }
                .await;
                drop(file);
                if let Err(error) = write_result {
                    let _ = fs::remove_file(&part).await;
                    return Err(error);
                }
                fs::rename(&part, &target).await?;
                return Ok(DownloadOutcome::Saved(target));
            }
            Err(error) if attempt < options.retries => {
                let _ = error;
                delay(attempt).await;
            }
            Err(error) => return Err(error.into()),
        }
    }
    unreachable!("retry loop always returns on final attempt")
}

async fn delay(attempt: u32) {
    let power = attempt.min(5);
    tokio::time::sleep(Duration::from_millis(250 * (1_u64 << power))).await;
}
