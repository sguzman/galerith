# 🏗️ Galerith architecture

Galerith targets complete [gallery-dl](https://github.com/mikf/gallery-dl) behavior. The current executable is a small preview, not the MVP.

```text
CLI ── explicit config overlay (JSON/YAML/TOML + -o overrides)
  ├── URL dispatch and site-specific extractor subset
  │    ├── direct media links
  │    ├── Danbooru posts and tags
  │    ├── e621/e926/e6ai posts and tags
  │    └── Gelbooru posts and tags
  ├── 1-based range selector / bounded filename formatter
  └── HTTP(S) client with Netscape cookie jar
       └── download (resumes / retries / output lock)
            ├── filesystem
            ├── native SQLite archive
            └── redacted JSON metadata sidecar
```

- `src/lib.rs`: shared `MediaItem` and direct media matcher
- `src/extractor/`: independently written, limited booru API extractors; no global gallery-dl-equivalent registry or recursive queues yet
- `src/config.rs`: overlay of explicit JSON/YAML/TOML files and dotted `-o` options, not default-file discovery or full upstream option semantics
- `src/filename.rs`: safe leaf-name formatter with common fields, padding, escaped braces; no conditional formatting or full upstream dialect
- `src/range.rs`: gallery-dl-compatible index syntax parsed in Rust; applies after the current page-bounded extractors return
- `src/privacy.rs`: URL and network-error redaction for ordinary diagnostics
- `src/cookies.rs`: Netscape cookie file importer; not browser account/token import
- `src/download.rs`: validator-bound HTTP resume, transient retries, output path locks and complete-file renames
- `src/archive.rs`: SQLite duplicate ledger using Galerith keys; incompatible with full upstream archive semantics
- `src/postprocess.rs`: atomic, limited media JSON sidecars (`--write-metadata`); the full gallery-dl postprocessor lifecycle remains outstanding
- `src/bin/galerith-roadmap.rs`: task/acceptance and extractor inventory verification; CI checks both generated Markdown documents

Tests are synthetic and offline: `tests/cli.rs`, `tests/config_cli.rs`, `tests/cookie_cli.rs`, `tests/download_http.rs`, `tests/download_resume.rs`, `tests/download_collision.rs`, `tests/download_quality.rs`, `tests/archive_cli.rs`, and `tests/metadata_cli.rs`.

Never include private conversations, credentials, real user media, logged-in sessions or protected site data in the public repository.

## Missing core infrastructure

Extractor message queues and complete metadata dictionaries, authentication/login protocols, supported-site class inventory, full config and formatter dialects, archive interchange, file/post filtering, complete postprocessors (ZIP/CBZ/mtime/exec/rename), downloader integrations, and the rest of the pinned 272-file module inventory.
