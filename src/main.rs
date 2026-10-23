use clap::Parser;
use galerith::{
    archive::Archive,
    download::{download, DownloadOptions, DownloadOutcome},
    extractor::resolve,
};
use std::{
    error::Error,
    io::{self, Read},
    path::PathBuf,
    time::Duration,
};

#[derive(Debug, Parser)]
#[command(
    name = "galerith",
    about = "Rust gallery downloader (early compatibility preview)"
)]
struct Cli {
    /// Media URLs to process
    #[arg(value_name = "URL")]
    urls: Vec<String>,

    /// Destination directory
    #[arg(short = 'd', long, default_value = ".")]
    destination: PathBuf,

    /// URL input file(s); '-' reads standard input
    #[arg(short = 'i', long = "input-file", value_name = "FILE")]
    input_files: Vec<String>,

    /// Print extracted URLs instead of downloading
    #[arg(short = 'g', long)]
    get_urls: bool,

    /// Print extracted media objects as JSON instead of downloading
    #[arg(short = 'j', long)]
    dump_json: bool,

    /// Simulate extraction without downloading
    #[arg(short = 's', long)]
    simulate: bool,

    /// Resolve URLs without downloading
    #[arg(long)]
    no_download: bool,

    /// Print currently implemented extractor names
    #[arg(long)]
    list_extractors: bool,

    /// Maximum HTTP retries
    #[arg(short = 'R', long, default_value_t = 4)]
    retries: u32,

    /// HTTP request timeout in seconds
    #[arg(long, default_value_t = 30)]
    http_timeout: u64,

    /// SQLite archive: record successful media and skip recorded items
    #[arg(long = "download-archive", value_name = "FILE")]
    download_archive: Option<PathBuf>,

    /// Overwrite existing media instead of skipping
    #[arg(long)]
    no_skip: bool,

    /// Maximum Danbooru search pages to fetch (preview default: 1)
    #[arg(long, default_value_t = 1)]
    max_pages: u16,

    /// Suppress per-file status messages
    #[arg(short = 'q', long)]
    quiet: bool,
}

#[tokio::main]
async fn main() {
    if let Err(error) = run(Cli::parse()).await {
        eprintln!("galerith: {error}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<(), Box<dyn Error + Send + Sync>> {
    if cli.list_extractors {
        println!("directlink (direct HTTP(S) media URLs only; partial parity)");
        println!("danbooru (post URLs and tag searches only; partial parity)");
        return Ok(());
    }

    let mut urls = cli.urls;
    for input in &cli.input_files {
        let content = if input == "-" {
            let mut data = String::new();
            io::stdin().read_to_string(&mut data)?;
            data
        } else {
            std::fs::read_to_string(input)?
        };
        urls.extend(
            content
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !line.starts_with('#'))
                .map(str::to_owned),
        );
    }
    if urls.is_empty() {
        return Err("provide at least one URL or --input-file".into());
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(cli.http_timeout))
        .user_agent(format!(
            "Galerith/{} (+https://github.com/sguzman/galerith)",
            env!("CARGO_PKG_VERSION")
        ))
        .build()?;
    let options = DownloadOptions {
        destination: cli.destination,
        retries: cli.retries,
        no_skip: cli.no_skip,
    };
    let archive = if cli.get_urls || cli.dump_json || cli.simulate || cli.no_download {
        None
    } else {
        cli.download_archive.as_ref().map(|path| Archive::open(path)).transpose()?
    };
    let mut failed = 0usize;
    for url in urls {
        let items = match resolve(&client, &url, cli.max_pages).await {
            Ok(items) => items,
            Err(error) => {
                eprintln!("galerith: {url}: {error}");
                failed += 1;
                continue;
            }
        };
        for item in items {
            if let Some(ref archive) = archive {
                if archive.contains(&item)? {
                    if !cli.quiet {
                        println!("archived {}", item.filename);
                    }
                    continue;
                }
            }
            if cli.get_urls {
                println!("{}", item.url);
            } else if cli.dump_json {
                println!("{}", serde_json::to_string(&item)?);
            } else if cli.simulate || cli.no_download {
                if !cli.quiet {
                    println!("[simulate] {}", item.filename);
                }
            } else {
                match download(&client, &item, &options).await {
                    Ok(DownloadOutcome::Saved(path)) => {
                        if let Some(ref archive) = archive {
                            archive.record(&item)?;
                        }
                        if !cli.quiet {
                            println!("saved {}", path.display());
                        }
                    }
                    Ok(DownloadOutcome::Skipped(path)) => {
                        if !cli.quiet {
                            println!("skipped {}", path.display());
                        }
                    }
                    Err(error) => {
                        eprintln!("galerith: {}: {error}", item.url);
                        failed += 1;
                    }
                }
            }
        }
    }
    if failed > 0 {
        return Err(format!("{failed} URL(s) failed").into());
    }
    Ok(())
}
