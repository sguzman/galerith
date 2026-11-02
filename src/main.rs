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
    message::{
        to_upstream_tuple, ExtractorMessage, JobSelection, MAX_QUEUE_CHILDREN, MAX_QUEUE_DEPTH,
    },
    pacing::{with_request_delay_for_url, DelayRange, RateRange, RetryDelay},
    postprocess::{write_info_json, write_metadata},
    privacy::{display_error, display_url},
    range::IndexRange,
};
mod input;
use std::{
    error::Error,
    io::{self, Read},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

#[derive(Debug, Parser)]
#[command(
    name = "galerith",
    version,
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

    /// Print media URLs after resolving supported intermediary extractors
    #[arg(
        short = 'G',
        long = "resolve-urls",
        conflicts_with_all = [
            "get_urls",
            "follow_queues",
            "list_extractors",
            "list_modules",
            "dump_json",
            "resolve_json",
            "dump_messages",
            "simulate",
            "no_download",
            "file_range",
            "filename_template",
            "write_metadata",
            "write_info_json",
            "download_archive"
        ]
    )]
    resolve_urls: bool,

    /// Print upstream-compatible JSON messages after resolving intermediary extractors
    #[arg(
        short = 'J',
        long = "resolve-json",
        conflicts_with_all = [
            "get_urls",
            "follow_queues",
            "list_extractors",
            "list_modules",
            "dump_json",
            "resolve_urls",
            "dump_messages",
            "simulate",
            "no_download",
            "file_range",
            "filename_template",
            "write_metadata",
            "write_info_json",
            "download_archive"
        ]
    )]
    resolve_json: bool,

    /// Print upstream-compatible directory/URL message tuples as JSON
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
            "write_info_json",
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

    /// Print the distinct native extractor modules currently implemented
    #[arg(long, conflicts_with = "list_extractors")]
    list_modules: bool,

    /// Select 1-based media indices, e.g. 5, 8-20, 1:24:3
    #[arg(long = "range", value_name = "RANGE")]
    file_range: Option<String>,

    /// Write a JSON sidecar for each saved or existing media file
    #[arg(long = "write-metadata")]
    write_metadata: bool,

    /// Write bounded directory metadata to info.json
    #[arg(long = "write-info-json")]
    write_info_json: bool,

    /// Write successful bounded API responses to .txt files; all includes redacted headers
    #[arg(
        long = "write-pages",
        value_enum,
        num_args = 0..=1,
        default_missing_value = "body",
        require_equals = true
    )]
    write_pages: Option<ResponsePageMode>,

    /// Seconds to wait before each media download (SECONDS or inclusive MIN-MAX)
    #[arg(long = "sleep", value_name = "SECONDS|MIN-MAX")]
    sleep: Option<String>,

    /// Seconds to wait after skipping a media download (SECONDS or inclusive MIN-MAX)
    #[arg(long = "sleep-skip", value_name = "SECONDS|MIN-MAX")]
    sleep_skip: Option<String>,

    /// Seconds to wait before extracting each input URL (SECONDS or inclusive MIN-MAX)
    #[arg(long = "sleep-extractor", value_name = "SECONDS|MIN-MAX")]
    sleep_extractor: Option<String>,

    /// Seconds to wait between extractor HTTP requests (SECONDS or inclusive MIN-MAX)
    #[arg(long = "sleep-request", value_name = "SECONDS|MIN-MAX")]
    sleep_request: Option<String>,

    /// Maximum HTTP retries (default 4)
    #[arg(short = 'R', long, value_name = "N")]
    retries: Option<u32>,

    /// Delay between retries (SECONDS, MIN-MAX, lin[:START[:MAX]]=SECONDS, exp[:BASE[:START[:MAX]]]=SECONDS)
    #[arg(long = "sleep-retries", value_name = "SCHEDULE")]
    sleep_retries: Option<String>,

    /// Delay before retrying a configured 429 response (default 60 seconds when Retry-After is absent)
    #[arg(long = "sleep-429", value_name = "SCHEDULE")]
    sleep_429: Option<String>,

    /// HTTP request timeout in seconds (default 30)
    #[arg(long, value_name = "SECONDS")]
    http_timeout: Option<f64>,

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

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum ResponsePageMode {
    Body,
    All,
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
    let result = if let Some(mode) = write_pages {
        match std::env::current_dir() {
            Ok(directory) => {
                let mode = match mode {
                    ResponsePageMode::Body => galerith::http_diagnostics::ResponsePageMode::Body,
                    ResponsePageMode::All => galerith::http_diagnostics::ResponsePageMode::All,
                };
                galerith::http_diagnostics::scope_response_pages(directory, mode, run(cli)).await
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

#[derive(Clone, Copy)]
struct HttpClientOptions<'a> {
    timeout: Duration,
    no_check_certificate: bool,
    root_certificates: Option<&'a [reqwest::Certificate]>,
    user_agent: Option<&'a str>,
    headers: Option<&'a reqwest::header::HeaderMap>,
    cookies: Option<&'a Arc<reqwest::cookie::Jar>>,
    proxy_settings: Option<&'a galerith::config::HttpProxySettings>,
    source_address: Option<std::net::IpAddr>,
    force_ipv4: bool,
    force_ipv6: bool,
}

fn build_http_client(options: HttpClientOptions<'_>) -> Result<reqwest::Client, &'static str> {
    let mut builder = reqwest::Client::builder()
        .timeout(options.timeout)
        .danger_accept_invalid_certs(options.no_check_certificate);
    if let Some(user_agent) = options.user_agent {
        builder = builder.user_agent(user_agent);
    }
    if let Some(headers) = options.headers {
        builder = builder.default_headers(headers.clone());
    }
    if let Some(certificates) = options.root_certificates {
        builder = builder.tls_built_in_root_certs(false);
        for certificate in certificates {
            builder = builder.add_root_certificate(certificate.clone());
        }
    }
    if let Some(jar) = options.cookies {
        builder = builder.cookie_provider(Arc::clone(jar));
    }
    if let Some(proxy_settings) = options.proxy_settings {
        // Never include proxy credentials in diagnostic output.
        if let Some(proxy_url) = proxy_settings.http.as_deref() {
            let proxy =
                reqwest::Proxy::http(proxy_url).map_err(|_| "invalid proxy configuration")?;
            builder = builder.proxy(proxy);
        }
        if let Some(proxy_url) = proxy_settings.https.as_deref() {
            let proxy =
                reqwest::Proxy::https(proxy_url).map_err(|_| "invalid proxy configuration")?;
            builder = builder.proxy(proxy);
        }
    }
    if let Some(address) = options.source_address {
        builder = builder.local_address(address);
    } else if options.force_ipv4 {
        builder = builder.local_address(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED));
    } else if options.force_ipv6 {
        builder = builder.local_address(std::net::IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED));
    }
    builder
        .build()
        .map_err(|_| "invalid HTTP client configuration")
}

fn load_http_ca_bundle(path: &std::path::Path) -> Result<Vec<reqwest::Certificate>, &'static str> {
    let pem = std::fs::read(path).map_err(|_| "could not read HTTP CA certificate")?;
    let certificates =
        reqwest::Certificate::from_pem_bundle(&pem).map_err(|_| "invalid HTTP CA certificate")?;
    if certificates.is_empty() {
        return Err("invalid HTTP CA certificate");
    }
    Ok(certificates)
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
    if cli.list_modules {
        let modules = PREVIEW_EXTRACTORS
            .iter()
            .map(|(name, _)| *name)
            .collect::<std::collections::BTreeSet<_>>();
        for module in modules {
            println!("{module}");
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
    let cli_sleep_skip = cli
        .sleep_skip
        .as_deref()
        .map(DelayRange::parse)
        .transpose()
        .map_err(|_| "invalid --sleep-skip: expected 0 to 3600 seconds or MIN-MAX")?;
    let cli_sleep_extractor = cli
        .sleep_extractor
        .as_deref()
        .map(DelayRange::parse)
        .transpose()
        .map_err(|_| "invalid --sleep-extractor: expected 0 to 3600 seconds or MIN-MAX")?;
    let cli_sleep_request = cli
        .sleep_request
        .as_deref()
        .map(DelayRange::parse)
        .transpose()
        .map_err(|_| "invalid --sleep-request: expected 0 to 3600 seconds or MIN-MAX")?;
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

    let cli_http_timeout = cli
        .http_timeout
        .map(|seconds| {
            if !seconds.is_finite() || seconds <= 0.0 {
                return Err("invalid --http-timeout: expected a positive number of seconds");
            }
            Duration::try_from_secs_f64(seconds)
                .map_err(|_| "invalid --http-timeout: value is out of range")
        })
        .transpose()?;
    let global_http_timeout = config
        .global_http_timeout()
        .map_err(std::io::Error::other)?;
    let downloader_http_timeout = config
        .downloader_http_timeout()
        .map_err(std::io::Error::other)?;
    let http_timeout = cli_http_timeout
        .or(global_http_timeout)
        .unwrap_or(Duration::from_secs(30));
    let media_http_timeout = cli_http_timeout
        .or(global_http_timeout)
        .or(downloader_http_timeout);

    let cookies = cli.cookies.as_deref().map(load_netscape_file).transpose()?;
    let cli_user_agent_set = cli.user_agent.is_some();
    let user_agent = cli.user_agent.unwrap_or_else(|| {
        format!(
            "Galerith/{} (+https://github.com/sguzman/galerith)",
            env!("CARGO_PKG_VERSION")
        )
    });
    let config_proxy = config
        .http_proxy_settings()
        .map_err(std::io::Error::other)?;
    let cli_proxy = cli
        .proxy
        .as_deref()
        .and_then(galerith::config::HttpProxySettings::all);
    let shared_proxy_settings = if cli.proxy.is_some() {
        cli_proxy
    } else {
        config_proxy
    };
    let downloader_proxy_settings = config
        .downloader_http_proxy_settings()
        .map_err(std::io::Error::other)?;
    let downloader_http_verify = config
        .downloader_http_verify()
        .map_err(std::io::Error::other)?;
    let client_options = HttpClientOptions {
        timeout: http_timeout,
        no_check_certificate: cli.no_check_certificate,
        root_certificates: None,
        user_agent: Some(&user_agent),
        headers: None,
        cookies: cookies.as_ref(),
        proxy_settings: shared_proxy_settings.as_ref(),
        source_address: cli.source_address,
        force_ipv4: cli.force_ipv4,
        force_ipv6: cli.force_ipv6,
    };
    let client = build_http_client(client_options)?;
    let options = DownloadOptions {
        destination: cli.destination.unwrap_or_else(|| {
            config
                .base_directory()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."))
        }),
        retries: cli.retries.unwrap_or(
            config
                .downloader_http_retries()
                .map_err(std::io::Error::other)?
                .unwrap_or(4),
        ),
        no_skip: cli.no_skip,
        no_mtime: cli.no_mtime
            || config
                .downloader_http_mtime()
                .map_err(std::io::Error::other)?
                == Some(false),
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
        retry_codes: config
            .downloader_http_retry_codes()
            .map_err(std::io::Error::other)?,
        http_timeout: media_http_timeout,
        retry_delay,
        rate_limit_delay,
        download_delay: None,
    };
    let archive = if cli.get_urls
        || cli.resolve_urls
        || cli.resolve_json
        || cli.dump_json
        || cli.dump_messages
        || cli.simulate
        || cli.no_download
    {
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
        let extractor_proxy_override = config
            .extractor_http_proxy_settings(category)
            .map_err(std::io::Error::other)?;
        let extractor_proxy_settings =
            extractor_proxy_override.unwrap_or_else(|| shared_proxy_settings.clone());
        let extractor_http_verify = config
            .extractor_http_verify(category)
            .map_err(std::io::Error::other)?;
        let extractor_source_address = config
            .extractor_http_source_address(category)
            .map_err(std::io::Error::other)?;
        let configured_user_agent = config
            .extractor_http_user_agent(category)
            .map_err(std::io::Error::other)?;
        let effective_user_agent = if cli_user_agent_set {
            user_agent.as_str()
        } else {
            configured_user_agent
                .as_deref()
                .filter(|user_agent| *user_agent != "auto")
                .unwrap_or(&user_agent)
        };
        let extractor_no_check_certificate = cli.no_check_certificate
            || extractor_http_verify.as_ref().is_some_and(|verify| {
                matches!(verify, galerith::config::HttpTlsVerification::Disabled)
            });
        let extractor_root_certificates = match extractor_http_verify.as_ref() {
            Some(galerith::config::HttpTlsVerification::CaFile(path))
                if !cli.no_check_certificate =>
            {
                Some(load_http_ca_bundle(path)?)
            }
            _ => None,
        };
        let extractor_headers = config
            .extractor_http_headers(category)
            .map_err(std::io::Error::other)?;
        let extractor_header_map = extractor_headers.as_ref().map(|headers| &headers.values);
        let mut extractor_client_options = client_options;
        extractor_client_options.no_check_certificate = extractor_no_check_certificate;
        extractor_client_options.root_certificates = extractor_root_certificates.as_deref();
        extractor_client_options.user_agent = Some(effective_user_agent);
        if cli.source_address.is_none() && !cli.force_ipv4 && !cli.force_ipv6 {
            extractor_client_options.source_address = extractor_source_address;
        }
        if extractor_headers
            .as_ref()
            .is_some_and(|headers| headers.disable_user_agent)
        {
            extractor_client_options.user_agent = None;
        }
        let mut diagnostic_request_headers = reqwest::header::HeaderMap::new();
        if let Some(user_agent) = extractor_client_options.user_agent {
            let value = reqwest::header::HeaderValue::from_str(user_agent)
                .map_err(|_| "invalid User-Agent header value")?;
            diagnostic_request_headers.insert(reqwest::header::USER_AGENT, value);
        }
        if let Some(headers) = extractor_header_map {
            diagnostic_request_headers.extend(headers.clone());
        }
        let extractor_client = if extractor_proxy_settings == shared_proxy_settings
            && extractor_header_map.is_none()
            && extractor_no_check_certificate == cli.no_check_certificate
            && extractor_root_certificates.is_none()
            && extractor_client_options.source_address == cli.source_address
            && extractor_client_options.user_agent == client_options.user_agent
        {
            client.clone()
        } else {
            build_http_client(HttpClientOptions {
                proxy_settings: extractor_proxy_settings.as_ref(),
                headers: extractor_header_map,
                ..extractor_client_options
            })?
        };
        let media_proxy_settings = downloader_proxy_settings
            .clone()
            .unwrap_or_else(|| extractor_proxy_settings.clone());
        let media_http_verify = downloader_http_verify
            .as_ref()
            .or(extractor_http_verify.as_ref());
        let media_no_check_certificate = cli.no_check_certificate
            || media_http_verify.is_some_and(|verify| {
                matches!(verify, galerith::config::HttpTlsVerification::Disabled)
            });
        let media_root_certificates = match media_http_verify {
            Some(galerith::config::HttpTlsVerification::CaFile(path))
                if !cli.no_check_certificate && downloader_http_verify.is_some() =>
            {
                Some(load_http_ca_bundle(path)?)
            }
            Some(galerith::config::HttpTlsVerification::CaFile(_)) if !cli.no_check_certificate => {
                extractor_root_certificates.clone()
            }
            _ => None,
        };
        let media_uses_extractor_tls =
            downloader_http_verify.is_none() || downloader_http_verify == extractor_http_verify;
        let media_client =
            if media_uses_extractor_tls && media_proxy_settings == extractor_proxy_settings {
                extractor_client.clone()
            } else if media_no_check_certificate == cli.no_check_certificate
                && media_root_certificates.is_none()
                && media_proxy_settings == shared_proxy_settings
                && extractor_header_map.is_none()
                && extractor_client_options.source_address == cli.source_address
                && extractor_client_options.user_agent == client_options.user_agent
            {
                client.clone()
            } else {
                build_http_client(HttpClientOptions {
                    no_check_certificate: media_no_check_certificate,
                    root_certificates: media_root_certificates.as_deref(),
                    proxy_settings: media_proxy_settings.as_ref(),
                    headers: extractor_header_map,
                    ..extractor_client_options
                })?
            };
        let api_request_delay = match cli_sleep_request {
            Some(delay) => Some(delay),
            None => config
                .duration(category, "sleep-request")
                .map_err(std::io::Error::other)?,
        };
        let extractor_delay = match cli_sleep_extractor {
            Some(delay) => Some(delay),
            None => config
                .duration(category, "sleep-extractor")
                .map_err(std::io::Error::other)?,
        };
        let configured_response_pages = if cli.write_pages.is_none()
            && config
                .extractor_write_pages(category)
                .map_err(std::io::Error::other)?
        {
            Some((
                std::env::current_dir()?,
                galerith::http_diagnostics::ResponsePageMode::Body,
            ))
        } else {
            None
        };
        if let Some(delay) = extractor_delay {
            tokio::time::sleep(delay.sample()).await;
        }
        if cli.dump_messages || cli.dump_json || cli.resolve_urls || cli.resolve_json {
            let message_future = async {
                if cli.resolve_urls || cli.resolve_json {
                    resolve_messages_traversing_selected(
                        &extractor_client,
                        &url,
                        cli.max_pages,
                        MAX_QUEUE_CHILDREN as u8,
                        MAX_QUEUE_DEPTH,
                        &JobSelection::default(),
                    )
                    .await
                } else if cli.expand_queues {
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
                        &extractor_client,
                        &url,
                        cli.max_pages,
                        cli.max_queue_children.unwrap_or(8),
                        cli.queue_depth.unwrap_or(1),
                        &selection,
                    )
                    .await
                } else {
                    resolve_messages(&extractor_client, &url, cli.max_pages).await
                }
            };
            let message_future =
                with_request_delay_for_url(&url, api_request_delay, message_future);
            let message_future = galerith::http_diagnostics::scope_default_request_headers(
                diagnostic_request_headers.clone(),
                message_future,
            );
            let events = galerith::http_diagnostics::scope_response_pages_if(
                configured_response_pages.clone(),
                async {
                    if let Some(cache) = &public_cache {
                        cache.clone().scope(message_future).await
                    } else {
                        message_future.await
                    }
                },
            )
            .await;
            match events {
                Ok(events) => {
                    if cli.dump_json || cli.resolve_json {
                        let tuples = events.iter().map(to_upstream_tuple).collect::<Vec<_>>();
                        println!("{}", serde_json::to_string(&tuples)?);
                    } else if cli.resolve_urls {
                        for event in events {
                            if let ExtractorMessage::Url { url, .. } = event {
                                println!("{url}");
                            }
                        }
                    } else {
                        for event in events {
                            println!("{}", serde_json::to_string(&event)?);
                        }
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
                resolve_media_job_selected(&extractor_client, &url, cli.max_pages, &job_selection)
                    .await
            } else {
                resolve(&extractor_client, &url, cli.max_pages).await
            }
        };
        let extraction_future =
            with_request_delay_for_url(&url, api_request_delay, extraction_future);
        let extraction_future = galerith::http_diagnostics::scope_default_request_headers(
            diagnostic_request_headers,
            extraction_future,
        );
        let extraction =
            galerith::http_diagnostics::scope_response_pages_if(configured_response_pages, async {
                if let Some(cache) = &public_cache {
                    cache.clone().scope(extraction_future).await
                } else {
                    extraction_future.await
                }
            })
            .await;
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
        if cli.write_info_json && !cli.simulate {
            if let Some(item) = items.first() {
                write_info_json(&options.destination, item).await?;
            }
        }
        for (index, mut item) in items.into_iter().enumerate() {
            if selected
                .as_ref()
                .is_some_and(|range| !range.contains(index + 1))
            {
                continue;
            }
            selected_items += 1;
            if !cli.get_urls {
                let original_filename = cli.filename_template.as_deref() == Some("/O");
                let template = if original_filename {
                    Some("{filename}.{extension}")
                } else {
                    cli.filename_template
                        .as_deref()
                        .or_else(|| config.filename(item.extractor))
                };
                if let Some(template) = template {
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
                    if !cli.get_urls
                        && !cli.resolve_urls
                        && !cli.resolve_json
                        && !cli.dump_json
                        && !cli.simulate
                        && !cli.no_download
                    {
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
            if cli.get_urls || cli.resolve_urls || cli.resolve_json {
                println!("{}", item.url);
            } else if cli.dump_json {
                println!("{}", serde_json::to_string(&item)?);
            } else if cli.simulate || cli.no_download {
                if !cli.quiet {
                    println!("[simulate] {}", item.filename);
                }
            } else {
                let mut media_options = options.clone();
                media_options.download_delay = download_sleep;
                match download(&media_client, &item, &media_options).await {
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
                        let skip_delay = match cli_sleep_skip {
                            Some(delay) => Some(delay),
                            None => config
                                .duration(item.extractor, "sleep-skip")
                                .map_err(std::io::Error::other)?,
                        };
                        if let Some(delay) = skip_delay {
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
            && !cli.resolve_urls
            && !cli.resolve_json
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

#[cfg(test)]
mod tests {
    use super::{Cli, ResponsePageMode};
    use clap::Parser;

    #[test]
    fn write_pages_accepts_body_default_and_header_inclusive_mode() {
        let body = Cli::try_parse_from(["galerith", "--write-pages"]).unwrap();
        assert!(matches!(body.write_pages, Some(ResponsePageMode::Body)));
        let all = Cli::try_parse_from(["galerith", "--write-pages=all"]).unwrap();
        assert!(matches!(all.write_pages, Some(ResponsePageMode::All)));
        assert!(Cli::try_parse_from(["galerith", "--write-pages=ALL"]).is_err());
    }
}
