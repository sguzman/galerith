# 🏗️ Galerith architecture

Galerith targets complete [gallery-dl](https://github.com/mikf/gallery-dl) behavior. The current executable is a small preview, not the MVP.

```text
CLI ── explicit config overlay (JSON/YAML/TOML + -o overrides)
  ├── URL dispatch and site-specific extractor subset
  │    ├── direct media links
  │    ├── Danbooru posts and tags
  │    ├── e621/e926/e6ai posts and tags
  │    ├── Gelbooru JSON posts and tags
  │    ├── Gelbooru v0.2 XML API family (five hosts)
  │    └── 4chan / 4channel thread and bounded board media
  ├── read-only typed message inspection (directory, URL, deferred queue)
  │    ├── opt-in, four-level bounded queue traversal with scoped metadata
  │    ├── typed directory/file/child ordinal selection before actions
  │    └── true 4chan API post scopes; finite Wallhaven/Gelbooru/Moebooru API page stopping
  ├── opt-in --follow-queues: typed URL events feed native downloads
  ├── 1-based range selector / bounded filename formatter
  └── HTTP(S) client with Netscape cookie jar and bounded pacing
       └── download (resumes / retries / output lock)
            ├── filesystem
            ├── native SQLite archive
            └── redacted JSON metadata sidecar
```

- `src/lib.rs`: shared `MediaItem` and direct media matcher
- `src/extractor/`: independently written, limited native extractor previews; read-only typed event adapter, Wallhaven deferred collection queues and bounded multi-level inspection and optional queue-driven downloads, but no full gallery-dl recursive queue, metadata, redirect or job compatibility contract
- `src/message.rs`: typed directory/URL/queue events, bounded iterative job traversal, metadata scope isolation and file/child/directory selection; [job filters](JOB_FILTERS.md) and [post scopes/paging](POST_SCOPES_AND_PAGING.md) and [4chan queue](FOURCHAN_QUEUE.md). [Detailed preview contract](MESSAGE_PROTOCOL.md)
- `src/config.rs`: overlay of explicit JSON/YAML/TOML files and dotted `-o` options, not default-file discovery or full upstream option semantics
- `src/filename.rs`: safe leaf-name formatter with common fields, padding, escaped braces; no conditional formatting or full upstream dialect
- `src/range.rs`: gallery-dl-compatible index syntax parsed in Rust; applies after the current page-bounded extractors return
- `src/privacy.rs`: URL and network-error redaction for ordinary diagnostics
- `src/cookies.rs`: Netscape cookie file importer; not browser account/token import
- `src/download.rs`: validator-bound HTTP resume, transient retries including numeric and HTTP-date Retry-After, output path locks and complete-file renames
- `src/archive.rs`: SQLite duplicate ledger using Galerith keys; incompatible with full upstream archive semantics
- `src/postprocess.rs`: atomic, limited media JSON sidecars (`--write-metadata`); the full gallery-dl postprocessor lifecycle remains outstanding
- `src/bin/galerith-roadmap.rs`: task/acceptance and extractor inventory verification; CI checks both generated Markdown documents

Tests are synthetic and offline: `tests/cli.rs`, `tests/message_cli.rs`, `tests/message_download_cli.rs`, `tests/config_cli.rs`, `tests/cookie_cli.rs`, `tests/download_http.rs`, `tests/download_resume.rs`, `tests/download_collision.rs`, `tests/download_quality.rs`, `tests/archive_cli.rs`, `tests/metadata_cli.rs`, `tests/http_options_cli.rs`, and `tests/sleep_cli.rs`.

Never include private conversations, credentials, real user media, logged-in sessions or protected site data in the public repository.

## Missing core infrastructure

Complete queue execution and full metadata dictionaries, authentication/login protocols, supported-site class inventory, full config and formatter dialects, archive interchange, file/post filtering, complete postprocessors (ZIP/CBZ/mtime/exec/rename), downloader integrations, and the rest of the pinned 272-file module inventory.
