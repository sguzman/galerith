use clap::Parser;
use galerith::{
    archive::Archive,
    cache::Cache,
    config::{Config, Format as ConfigFormat},
    cookies::load_netscape_file,
    download::{download, DownloadOptions, DownloadOutcome},
    extractor::{
        extractor_name, resolve, resolve_media_job_selected, resolve_messages,
        resolve_messages_traversing_selected, PREVIEW_EXTRACTORS,
    },
    filename::render as render_filename,
    message::JobSelection,
    pacing::{with_request_delay_for_url, DelayRange, RateRange, RetryDelay},
    postprocess::write_metadata,
    privacy::{display_error, display_url},
    range::IndexRange,
};
mod input;
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

    /// Comment out successful URLs in FILE
    #[arg(short = 'I', long = "input-file-comment", value_name = "FILE")]
    input_file_comment: Vec<PathBuf>,

    /// Delete successful URLs from FILE
    #[arg(short = 'x', long = "input-file-delete", value_name = "FILE")]
    input_file_delete: Vec<PathBuf>,

    /// Append input URLs that failed to process
    #[arg(short = 'e', long = "error-file", value_name = "FILE")]
    error_file: Option<PathBuf>,

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

    /// Write bounded extractor API response bodies to numbered .txt files
    #[arg(long = "write-pages")]
    write_pages: bool,

    /// Seconds to wait before each media download (SECONDS or inclusive MIN-MAX)
    #[arg(long = "sleep", value_name = "SECONDS|MIN-MAX")]
    sleep: Option<String>,

    /// Maximum HTTP retries
    #[arg(short = 'R', long, default_value_t = 4)]
    retries: u32,

    /// Delay between retries (SECONDS, MIN-MAX, lin[:START[:MAX]]=SECONDS, exp[:BASE[:START[:MAX]]]=SECONDS)
    #[arg(long = "sleep-retries", value_name = "SCHEDULE")]
    sleep_retries: Option<String>,

    /// Delay before retrying a 429 response (default 60 seconds when Retry-After is absent)
    #[arg(long = "sleep-429", value_name = "SCHEDULE")]
    sleep_429: Option<String>,

    /// HTTP request timeout in seconds
    #[arg(long, default_value_t = 30.0)]
    http_timeout: f64,

    /// User-Agent HTTP header (upstream -a/--user-agent)
    #[arg(short = 'a', long = "user-agent", value_name = "UA")]
    user_agent: Option<String>,

    /// Proxy URL for HTTP(S) requests; credentials are not printed
    #[arg(long = "proxy", value_name = "URL")]
    proxy: Option<String>,

    /// Resolve and connect using IPv4 only
    #[arg(
        short = '4',
        long = "force-ipv4",
        conflicts_with_all = ["force_ipv6", "source_address"]
    )]
    force_ipv4: bool,

    /// Resolve and connect using IPv6 only
    #[arg(
        short = '6',
        long = "force-ipv6",
        conflicts_with_all = ["force_ipv4", "source_address"]
    )]
    force_ipv6: bool,

    /// Bind outgoing HTTP connections to this client-side IP address
    #[arg(
        long = "source-address",
        value_parser = clap::value_parser!(std::net::IpAddr),
        conflicts_with_all = ["force_ipv4", "force_ipv6"]
    )]
    source_address: Option<std::net::IpAddr>,

    /// Disable TLS certificate validation (unsafe; use only with trusted targets)
    #[arg(long = "no-check-certificate")]
    no_check_certificate: bool,

    /// Netscape cookie jar for authenticated media and API requests
    #[arg(short = 'C', long = "cookies", value_name = "FILE")]
    cookies: Option<PathBuf>,

    /// SQLite archive: record successful media and skip recorded items
    #[arg(long = "download-archive", value_name = "FILE")]
    download_archive: Option<PathBuf>,

    /// Archive recording events: file (default), skip, file,skip or skip,file
    #[arg(
        long = "archive-event",
        value_name = "EVENTS",
        requires = "download_archive",
        value_parser = ["file", "skip", "file,skip", "skip,file"]
    )]
    archive_event: Option<String>,

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

    /// Opt-in private SQLite cache for public pool/favorite-group API metadata
    #[arg(long = "cache-db", value_name = "FILE", conflicts_with_all = ["cookies", "archive_import"])]
    cache_db: Option<PathBuf>,

    /// Freshness lifetime in seconds (0 = never expire; maximum 86400)
    #[arg(long = "cache-ttl", value_name = "SECONDS", requires = "cache_db")]
    cache_ttl: Option<u64>,

    /// Overwrite existing media instead of skipping
    #[arg(long)]
    no_skip: bool,

    /// Do not set output modification times from HTTP Last-Modified
    #[arg(long)]
    no_mtime: bool,

    /// Write directly to the output file instead of using a .part file
    #[arg(long)]
    no_part: bool,

    /// Limit download speed (RATE, optionally with k/M/G suffix)
    #[arg(short = 'r', long = "limit-rate", value_name = "RATE")]
    limit_rate: Option<String>,

    /// Maximum extractor feed/search pages to fetch (preview default: 1)
    #[arg(long, default_value_t = 1)]
    max_pages: u16,

    /// Suppress per-file status messages
    #[arg(short = 'q', long)]
    quiet: bool,
}

// Cache and download/reference archives must never alias: otherwise opening
// the cache may create tables or prune entries in an unrelated database.
fn same_cache_archive_file(
    left: &std::path::Path,
    right: &std::path::Path,
) -> std::io::Result<bool> {
    if left.exists() && right.exists() {
        if std::fs::canonicalize(left)? == std::fs::canonicalize(right)? {
            return Ok(true);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let a = std::fs::metadata(left)?;
            let b = std::fs::metadata(right)?;
            return Ok(a.dev() == b.dev() && a.ino() == b.ino());
        }
        #[cfg(not(unix))]
        {
            return Ok(false);
        }
    }
    // If either file is not created yet, compare the *parent directory*
    // after resolving symlinks, to catch ./ and symlinked directory aliases.
    let left_parent = left
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(std::path::Path::new("."));
    let right_parent = right
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(std::path::Path::new("."));
    if left.file_name() == right.file_name() {
        if let (Ok(a), Ok(b)) = (
            std::fs::canonicalize(left_parent),
            std::fs::canonicalize(right_parent),
        ) {
            return Ok(a == b);
        }
    }
    Ok(left == right)
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let write_pages = cli.write_pages;
    let result = if write_pages {
        match std::env::current_dir() {
            Ok(directory) => {
                galerith::http_diagnostics::scope_response_pages(directory, run(cli)).await
            }
            Err(error) => Err(Box::new(error) as Box<dyn Error + Send + Sync>),
        }
    } else {
        run(cli).await
    };
    if let Err(error) = result {
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
    if let Some(cache) = cli.cache_db.as_deref() {
        for archive in [
            cli.download_archive.as_deref(),
            cli.gallery_dl_archive.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            if same_cache_archive_file(cache, archive)? {
                return Err(
                    "--cache-db must be separate from native and gallery-dl archive files".into(),
                );
            }
        }
    }
    if cli.archive_import {
        if !cli.urls.is_empty()
            || !cli.input_files.is_empty()
            || !cli.input_file_comment.is_empty()
            || !cli.input_file_delete.is_empty()
            || cli.error_file.is_some()
        {
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

    let mut urls = cli
        .urls
        .into_iter()
        .map(|url| input::InputUrl {
            url,
            mutation: None,
        })
        .collect::<Vec<_>>();
    for input in &cli.input_files {
        let content = if input == "-" {
            let mut data = String::new();
            io::stdin().read_to_string(&mut data)?;
            data
        } else {
            std::fs::read_to_string(input)?
        };
        let path = (input != "-").then(|| PathBuf::from(input));
        urls.extend(input::parse_urls(
            &content,
            path.as_deref(),
            input::InputFileAction::ReadOnly,
        ));
    }
    for (files, action) in [
        (&cli.input_file_comment, input::InputFileAction::Comment),
        (&cli.input_file_delete, input::InputFileAction::Delete),
    ] {
        for path in files {
            let content = std::fs::read_to_string(path)?;
            urls.extend(input::parse_urls(&content, Some(path), action));
        }
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

    let download_sleep = cli
        .sleep
        .as_deref()
        .map(DelayRange::parse)
        .transpose()
        .map_err(|_| "invalid --sleep: expected 0 to 3600 seconds or MIN-MAX")?;
    let retry_delay = cli
        .sleep_retries
        .as_deref()
        .map(RetryDelay::parse)
        .transpose()
        .map_err(|_| "invalid --sleep-retries: expected a duration or capped lin/exp schedule")?;
    let rate_limit_delay = cli
        .sleep_429
        .as_deref()
        .map(RetryDelay::parse)
        .transpose()
        .map_err(|_| "invalid --sleep-429: expected a duration or capped lin/exp schedule")?;

    if !cli.http_timeout.is_finite() || cli.http_timeout <= 0.0 {
        return Err("invalid --http-timeout: expected a positive number of seconds".into());
    }
    let http_timeout = Duration::try_from_secs_f64(cli.http_timeout)
        .map_err(|_| "invalid --http-timeout: value is out of range")?;

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
    if let Some(address) = cli.source_address {
        client_builder = client_builder.local_address(address);
    } else if cli.force_ipv4 {
        client_builder =
            client_builder.local_address(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED));
    } else if cli.force_ipv6 {
        client_builder =
            client_builder.local_address(std::net::IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED));
    }
    let user_agent = cli.user_agent.unwrap_or_else(|| {
        format!(
            "Galerith/{} (+https://github.com/sguzman/galerith)",
            env!("CARGO_PKG_VERSION")
        )
    });
    let client = client_builder
        .timeout(http_timeout)
        .danger_accept_invalid_certs(cli.no_check_certificate)
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
        no_mtime: cli.no_mtime,
        no_part: cli.no_part,
        limit_rate: cli
            .limit_rate
            .as_deref()
            .map(RateRange::parse)
            .transpose()
            .map_err(std::io::Error::other)?,
        headers: config
            .downloader_http_headers()
            .map_err(std::io::Error::other)?,
        retry_delay,
        rate_limit_delay,
        request_delay: None,
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
    let record_download = cli
        .archive_event
        .as_deref()
        .is_none_or(|events| events.split(',').any(|event| event == "file"));
    let record_skip = cli
        .archive_event
        .as_deref()
        .is_some_and(|events| events.split(',').any(|event| event == "skip"));
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
    let public_cache = cli
        .cache_db
        .as_deref()
        .map(|path| Cache::open(path, cli.cache_ttl.unwrap_or(300)))
        .transpose()?;
    let mut failed = 0usize;
    for source in urls {
        let url = source.url;
        let mut input_succeeded = true;
        let mut selected_items = 0usize;
        let category = extractor_name(&url).unwrap_or("");
        let api_request_delay = config
            .duration(category, "sleep-request")
            .map_err(std::io::Error::other)?;
        if let Some(delay) = config
            .duration(category, "sleep-extractor")
            .map_err(std::io::Error::other)?
        {
            tokio::time::sleep(delay.sample()).await;
        }
        if cli.dump_messages {
            let message_future = async {
                if cli.expand_queues {
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
                }
            };
            let message_future =
                with_request_delay_for_url(&url, api_request_delay, message_future);
            let events = if let Some(cache) = &public_cache {
                cache.clone().scope(message_future).await
            } else {
                message_future.await
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
                    append_error_url(cli.error_file.as_deref(), &url, &mut failed);
                }
            }
            continue;
        }
        let extraction_future = async {
            if cli.follow_queues {
                resolve_media_job_selected(&client, &url, cli.max_pages, &job_selection).await
            } else {
                resolve(&client, &url, cli.max_pages).await
            }
        };
        let extraction_future =
            with_request_delay_for_url(&url, api_request_delay, extraction_future);
        let extraction = if let Some(cache) = &public_cache {
            cache.clone().scope(extraction_future).await
        } else {
            extraction_future.await
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
                append_error_url(cli.error_file.as_deref(), &url, &mut failed);
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
            selected_items += 1;
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
                            input_succeeded = false;
                            continue;
                        }
                    }
                }
            }
            // Re-check under the archive lock: another process may have
            // completed this same media item while we were waiting. Retain
            // the guard through transfer and durable archive insertion.
            let _archive_guard = if let Some(ref archive) = archive {
                archive.lock_item_for_source(&item, &url).await?
            } else {
                None
            };
            if let Some(ref archive) = archive {
                if archive.contains_for_source(&item, &url)? {
                    if !cli.quiet {
                        println!("archived {}", item.filename);
                    }
                    if !cli.get_urls && !cli.dump_json && !cli.simulate && !cli.no_download {
                        if let Some(delay) = config
                            .duration(item.extractor, "sleep-skip")
                            .map_err(std::io::Error::other)?
                        {
                            tokio::time::sleep(delay.sample()).await;
                        }
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
                    tokio::time::sleep(delay.sample()).await;
                }
                let mut media_options = options.clone();
                media_options.request_delay = config
                    .duration(item.extractor, "sleep-request")
                    .map_err(std::io::Error::other)?;
                match download(&client, &item, &media_options).await {
                    Ok(DownloadOutcome::Saved(path)) => {
                        if cli.write_metadata {
                            write_metadata(&path, &item).await?;
                        }
                        if record_download {
                            if let Some(ref archive) = archive {
                                archive.record_for_source(&item, &url)?;
                            }
                        }
                        if !cli.quiet {
                            println!("saved {}", path.display());
                        }
                    }
                    Ok(DownloadOutcome::Skipped(path)) => {
                        if cli.write_metadata {
                            write_metadata(&path, &item).await?;
                        }
                        if record_skip {
                            if let Some(ref archive) = archive {
                                archive.record_for_source(&item, &url)?;
                            }
                        }
                        if !cli.quiet {
                            println!("skipped {}", path.display());
                        }
                        if let Some(delay) = config
                            .duration(item.extractor, "sleep-skip")
                            .map_err(std::io::Error::other)?
                        {
                            tokio::time::sleep(delay.sample()).await;
                        }
                    }
                    Err(error) => {
                        eprintln!(
                            "galerith: {}: {}",
                            display_url(&item.url),
                            display_error(error.as_ref())
                        );
                        failed += 1;
                        input_succeeded = false;
                    }
                }
            }
        }
        if input_succeeded
            && selected_items > 0
            && !cli.get_urls
            && !cli.dump_json
            && !cli.simulate
            && !cli.no_download
        {
            if let Some((path, action)) = source.mutation {
                if let Err(error) = input::mark_success(&path, &url, action) {
                    eprintln!(
                        "galerith: cannot update input file {}: {error}",
                        path.display()
                    );
                    failed += 1;
                }
            }
        }
        if !input_succeeded {
            append_error_url(cli.error_file.as_deref(), &url, &mut failed);
        }
    }
    if failed > 0 {
        return Err(format!("{failed} URL(s) failed").into());
    }
    Ok(())
}

fn append_error_url(path: Option<&std::path::Path>, url: &str, failed: &mut usize) {
    let Some(path) = path else {
        return;
    };
    if let Err(error) = input::append_failed_url(path, url) {
        eprintln!(
            "galerith: cannot append failed URL to {}: {error}",
            path.display()
        );
        *failed += 1;
    }
}
