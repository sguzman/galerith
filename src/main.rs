use clap::Parser;
use galerith::{
    archive::Archive,
    config::{Config, Format as ConfigFormat},
    cookies::load_netscape_file,
    download::{download, DownloadOptions, DownloadOutcome},
    extractor::{
        resolve, resolve_media_job_selected, resolve_messages,
        resolve_messages_traversing_selected, PREVIEW_EXTRACTORS,
    },
    filename::render as render_filename,
    message::JobSelection,
    postprocess::write_metadata,
    privacy::{display_error, display_url},
    range::IndexRange,
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

    /// Additional configuration file (JSON by default; YAML/TOML by extension)
    #[arg(short = 'c', long = "config", value_name = "FILE")]
    config_files: Vec<PathBuf>,

    /// Explicit JSON configuration files
    #[arg(long = "config-json", value_name = "FILE")]
    json_files: Vec<PathBuf>,

    /// Explicit YAML configuration files
    #[arg(long = "config-yaml", value_name = "FILE")]
    yaml_files: Vec<PathBuf>,

    /// Explicit TOML configuration files
    #[arg(long = "config-toml", value_name = "FILE")]
    toml_files: Vec<PathBuf>,

    /// Configuration override KEY=VALUE (JSON values or plain strings)
    #[arg(short = 'o', long = "option", value_name = "KEY=VALUE")]
    options: Vec<String>,

    /// Filename format (limited upstream-compatible named-field subset)
    #[arg(short = 'f', long = "filename", value_name = "FORMAT")]
    filename_template: Option<String>,

    /// Destination directory
    #[arg(short = 'd', long)]
    destination: Option<PathBuf>,

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

    /// Print typed extraction events as JSON lines (independent preview protocol)
    #[arg(
        long = "dump-messages",
        conflicts_with_all = [
            "get_urls",
            "dump_json",
            "simulate",
            "no_download",
            "file_range",
            "filename_template",
            "write_metadata",
            "download_archive"
        ]
    )]
    dump_messages: bool,

    /// Follow up to 16 distinct queued children by one level (read-only)
    #[arg(long = "expand-queues", requires = "dump_messages")]
    expand_queues: bool,

    /// Maximum distinct child queues expanded (defaults to 8)
    #[arg(
        long = "max-queue-children",
        requires = "expand_queues",
        value_parser = clap::value_parser!(u8).range(1..=16)
    )]
    max_queue_children: Option<u8>,

    /// Follow up to four queue levels (defaults to one in inspection mode)
    #[arg(
        long = "queue-depth",
        requires = "expand_queues",
        value_parser = clap::value_parser!(u8).range(1..=4)
    )]
    queue_depth: Option<u8>,

    /// Opt into the bounded two-level typed queue job for downloads/URL output
    #[arg(long = "follow-queues", conflicts_with = "dump_messages")]
    follow_queues: bool,

    /// Select 1-based URL-event ordinals before typed-job media downloads
    #[arg(
        long = "job-file-range",
        value_name = "RANGE",
        requires = "follow_queues",
        conflicts_with = "file_range"
    )]
    job_file_range: Option<String>,

    /// Select 1-based deferred-child ordinals before queued HTTP requests
    #[arg(
        long = "job-child-range",
        value_name = "RANGE",
        requires = "follow_queues"
    )]
    job_child_range: Option<String>,

    /// Select directory-scope ordinals before typed-job file/queue events
    #[arg(
        long = "job-directory-range",
        value_name = "RANGE",
        requires = "follow_queues"
    )]
    job_directory_range: Option<String>,

    /// Select actual source post IDs in typed download jobs, not media IDs
    #[arg(
        long = "job-source-post-range",
        value_name = "RANGE",
        requires = "follow_queues"
    )]
    job_source_post_range: Option<String>,

    /// Select URL-event ordinals in the read-only recursive message stream
    #[arg(
        long = "message-file-range",
        value_name = "RANGE",
        requires = "expand_queues"
    )]
    message_file_range: Option<String>,

    /// Select deferred queue ordinals before read-only child resolution
    #[arg(
        long = "message-child-range",
        value_name = "RANGE",
        requires = "expand_queues"
    )]
    message_child_range: Option<String>,

    /// Select directory-scope ordinals in read-only message inspection
    #[arg(
        long = "message-directory-range",
        value_name = "RANGE",
        requires = "expand_queues"
    )]
    message_directory_range: Option<String>,

    /// Select source post IDs in read-only typed message inspection
    #[arg(
        long = "message-source-post-range",
        value_name = "RANGE",
        requires = "expand_queues"
    )]
    message_source_post_range: Option<String>,

    /// Resolve URLs without downloading
    #[arg(long)]
    no_download: bool,

    /// Print currently implemented extractor names
    #[arg(long)]
    list_extractors: bool,

    /// Select 1-based media indices, e.g. 5, 8-20, 1:24:3
    #[arg(long = "range", value_name = "RANGE")]
    file_range: Option<String>,

    /// Write a JSON sidecar for each saved or existing media file
    #[arg(long = "write-metadata")]
    write_metadata: bool,

    /// Seconds to wait before each media download (constant-value preview)
    #[arg(long = "sleep", value_name = "SECONDS")]
    sleep: Option<f64>,

    /// Maximum HTTP retries
    #[arg(short = 'R', long, default_value_t = 4)]
    retries: u32,

    /// HTTP request timeout in seconds
    #[arg(long, default_value_t = 30)]
    http_timeout: u64,

    /// User-Agent HTTP header (upstream -a/--user-agent)
    #[arg(short = 'a', long = "user-agent", value_name = "UA")]
    user_agent: Option<String>,

    /// Proxy URL for HTTP(S) requests; credentials are not printed
    #[arg(long = "proxy", value_name = "URL")]
    proxy: Option<String>,

    /// Netscape cookie jar for authenticated media and API requests
    #[arg(short = 'C', long = "cookies", value_name = "FILE")]
    cookies: Option<PathBuf>,

    /// SQLite archive: record successful media and skip recorded items
    #[arg(long = "download-archive", value_name = "FILE")]
    download_archive: Option<PathBuf>,

    /// Existing gallery-dl SQLite archive to consult read-only for supported IDs
    #[arg(
        long = "gallery-dl-archive",
        value_name = "FILE",
        requires = "download_archive"
    )]
    gallery_dl_archive: Option<PathBuf>,

    /// Named source archive table (default: archive)
    #[arg(
        long = "gallery-dl-archive-table",
        value_name = "TABLE",
        requires = "gallery_dl_archive"
    )]
    gallery_dl_archive_table: Option<String>,

    /// Exact upstream archive-prefix override, including empty string
    #[arg(
        long = "gallery-dl-archive-prefix",
        value_name = "PREFIX",
        requires = "gallery_dl_archive"
    )]
    gallery_dl_archive_prefix: Option<String>,

    /// Import recognized default gallery-dl archive entries once, offline
    #[arg(
        long = "archive-import",
        requires_all = ["download_archive", "gallery_dl_archive"]
    )]
    archive_import: bool,

    /// Overwrite existing media instead of skipping
    #[arg(long)]
    no_skip: bool,

    /// Maximum extractor feed/search pages to fetch (preview default: 1)
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
    // Clap's presence constraints are not a substitute for validating these
    // typed-job selectors. Never silently ignore them in inspection mode.
    if (cli.job_file_range.is_some()
        || cli.job_child_range.is_some()
        || cli.job_directory_range.is_some()
        || cli.job_source_post_range.is_some())
        && !cli.follow_queues
    {
        return Err("--job-file-range and --job-child-range require --follow-queues".into());
    }
    if (cli.message_file_range.is_some()
        || cli.message_child_range.is_some()
        || cli.message_directory_range.is_some()
        || cli.message_source_post_range.is_some())
        && !(cli.dump_messages && cli.expand_queues)
    {
        return Err("--message-file-range and --message-child-range require --dump-messages --expand-queues".into());
    }
    if cli.list_extractors {
        for (name, description) in PREVIEW_EXTRACTORS {
            println!("{name} ({description}; partial parity)");
        }
        return Ok(());
    }
    if cli.archive_import {
        if !cli.urls.is_empty() || !cli.input_files.is_empty() {
            return Err("--archive-import is an offline command and does not accept URLs".into());
        }
        let native = cli
            .download_archive
            .as_deref()
            .ok_or("missing native archive")?;
        let source = cli
            .gallery_dl_archive
            .as_deref()
            .ok_or("missing reference archive")?;
        let archive = Archive::open_with_reference_options(
            native,
            Some(source),
            cli.gallery_dl_archive_table.as_deref(),
            cli.gallery_dl_archive_prefix.as_deref(),
        )?;
        let (recognized, newly_imported) = archive.import_reference()?;
        println!("archive import: {recognized} supported entries; {newly_imported} new");
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

    let mut config = Config::default();
    for path in &cli.config_files {
        config.load(path, ConfigFormat::from_path(path))?;
    }
    for path in &cli.json_files {
        config.load(path, ConfigFormat::Json)?;
    }
    for path in &cli.yaml_files {
        config.load(path, ConfigFormat::Yaml)?;
    }
    for path in &cli.toml_files {
        config.load(path, ConfigFormat::Toml)?;
    }
    for option in &cli.options {
        config.option(option)?;
    }

    let download_sleep = match cli.sleep {
        Some(seconds) if seconds.is_finite() && (0.0..=3600.0).contains(&seconds) => {
            Some(Duration::from_secs_f64(seconds))
        }
        Some(_) => return Err("invalid --sleep: expected 0 to 3600 seconds".into()),
        None => None,
    };

    let cookies = cli.cookies.as_deref().map(load_netscape_file).transpose()?;
    let mut client_builder = reqwest::Client::builder();
    if let Some(jar) = cookies {
        client_builder = client_builder.cookie_provider(jar);
    }
    if let Some(ref proxy_url) = cli.proxy {
        // Never include proxy credentials in diagnostic output.
        let proxy = reqwest::Proxy::all(proxy_url).map_err(|_| "invalid proxy configuration")?;
        client_builder = client_builder.proxy(proxy);
    }
    let user_agent = cli.user_agent.unwrap_or_else(|| {
        format!(
            "Galerith/{} (+https://github.com/sguzman/galerith)",
            env!("CARGO_PKG_VERSION")
        )
    });
    let client = client_builder
        .timeout(Duration::from_secs(cli.http_timeout))
        .user_agent(user_agent)
        .build()
        .map_err(|_| "invalid HTTP client configuration")?;
    let options = DownloadOptions {
        destination: cli.destination.unwrap_or_else(|| {
            config
                .base_directory()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."))
        }),
        retries: cli.retries,
        no_skip: cli.no_skip,
    };
    let archive =
        if cli.get_urls || cli.dump_json || cli.dump_messages || cli.simulate || cli.no_download {
            None
        } else {
            cli.download_archive
                .as_ref()
                .map(|path| {
                    Archive::open_with_reference_options(
                        path,
                        cli.gallery_dl_archive.as_deref(),
                        cli.gallery_dl_archive_table.as_deref(),
                        cli.gallery_dl_archive_prefix.as_deref(),
                    )
                })
                .transpose()?
        };
    let selected = cli
        .file_range
        .as_deref()
        .map(IndexRange::parse)
        .transpose()?;
    let job_selection = JobSelection {
        files: cli
            .job_file_range
            .as_deref()
            .map(IndexRange::parse)
            .transpose()?,
        children: cli
            .job_child_range
            .as_deref()
            .map(IndexRange::parse)
            .transpose()?,
        directories: cli
            .job_directory_range
            .as_deref()
            .map(IndexRange::parse)
            .transpose()?,
        source_posts: cli
            .job_source_post_range
            .as_deref()
            .map(IndexRange::parse)
            .transpose()?,
    };
    let mut failed = 0usize;
    for url in urls {
        if cli.dump_messages {
            let events = if cli.expand_queues {
                let selection = JobSelection {
                    files: cli
                        .message_file_range
                        .as_deref()
                        .map(IndexRange::parse)
                        .transpose()?,
                    children: cli
                        .message_child_range
                        .as_deref()
                        .map(IndexRange::parse)
                        .transpose()?,
                    directories: cli
                        .message_directory_range
                        .as_deref()
                        .map(IndexRange::parse)
                        .transpose()?,
                    source_posts: cli
                        .message_source_post_range
                        .as_deref()
                        .map(IndexRange::parse)
                        .transpose()?,
                };
                resolve_messages_traversing_selected(
                    &client,
                    &url,
                    cli.max_pages,
                    cli.max_queue_children.unwrap_or(8),
                    cli.queue_depth.unwrap_or(1),
                    &selection,
                )
                .await
            } else {
                resolve_messages(&client, &url, cli.max_pages).await
            };
            match events {
                Ok(events) => {
                    for event in events {
                        println!("{}", serde_json::to_string(&event)?);
                    }
                }
                Err(error) => {
                    eprintln!(
                        "galerith: {}: {}",
                        display_url(&url),
                        display_error(error.as_ref())
                    );
                    failed += 1;
                }
            }
            continue;
        }
        let extraction = if cli.follow_queues {
            resolve_media_job_selected(&client, &url, cli.max_pages, &job_selection).await
        } else {
            resolve(&client, &url, cli.max_pages).await
        };
        let items = match extraction {
            Ok(items) => items,
            Err(error) => {
                eprintln!(
                    "galerith: {}: {}",
                    display_url(&url),
                    display_error(error.as_ref())
                );
                failed += 1;
                continue;
            }
        };
        for (index, mut item) in items.into_iter().enumerate() {
            if selected
                .as_ref()
                .is_some_and(|range| !range.contains(index + 1))
            {
                continue;
            }
            if !cli.get_urls {
                if let Some(template) = cli
                    .filename_template
                    .as_deref()
                    .or_else(|| config.filename(item.extractor))
                {
                    match render_filename(template, &item) {
                        Ok(name) => item.filename = name,
                        Err(error) => {
                            eprintln!("galerith: invalid filename template: {error}");
                            failed += 1;
                            continue;
                        }
                    }
                }
            }
            // Re-check under the archive lock: another process may have
            // completed this same media item while we were waiting. Retain
            // the guard through transfer and durable archive insertion.
            let _archive_guard = if let Some(ref archive) = archive {
                archive.lock_item(&item).await?
            } else {
                None
            };
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
                if let Some(delay) = download_sleep {
                    tokio::time::sleep(delay).await;
                }
                match download(&client, &item, &options).await {
                    Ok(DownloadOutcome::Saved(path)) => {
                        if cli.write_metadata {
                            write_metadata(&path, &item).await?;
                        }
                        if let Some(ref archive) = archive {
                            archive.record(&item)?;
                        }
                        if !cli.quiet {
                            println!("saved {}", path.display());
                        }
                    }
                    Ok(DownloadOutcome::Skipped(path)) => {
                        if cli.write_metadata {
                            write_metadata(&path, &item).await?;
                        }
                        if !cli.quiet {
                            println!("skipped {}", path.display());
                        }
                    }
                    Err(error) => {
                        eprintln!(
                            "galerith: {}: {}",
                            display_url(&item.url),
                            display_error(error.as_ref())
                        );
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
