# 🏗️ Architecture and port plan

The target is full compatibility with pinned gallery-dl, not a new downloader with a similar name.

```text
CLI / config / input files
            |
URL matcher + extractor registry
            |
Extractor messages (directory, queue, media, deferred URLs)
            |
Selection / metadata templates / postprocessing
            |
HTTP client + authentication + rate policies
            |
Filesystem / SQLite archive / resume + cache
```

Currently shipped:
- `src/main.rs`: first Rust CLI and a strict subset of upstream flags.
- `src/lib.rs`: direct-media URL recognition, shared metadata and safe leaf filenames.
- `src/extractor/`: preliminary site URL dispatch and Danbooru posts API reader (partial parity).
- `src/download.rs`: streamed downloads, HTTP validator-bound resumes, content-range validation, bounded retries and per-target advisory locking (see [download policy](DOWNLOADER.md)).
- `src/archive.rs`: conservative SQLite completed-download ledger, independent of upstream gallery-dl archive representation (see [archive policy](ARCHIVE.md)).
- `tests/cli.rs`: basic offline CLI cases.
- `tests/download_http.rs`: loopback tests for stream writes, retries and 404 handling.
- `tests/download_resume.rs`, `tests/download_collision.rs`, `tests/download_quality.rs`: deterministic interruption, concurrency, overwrite, large-transfer and status-policy tests.
- `tests/archive_cli.rs`: on-disk archive skip after first successful download, without a second network request.
- `src/bin/galerith-roadmap.rs`: evidence-backed source-of-truth rendering.

Not yet implemented: generalized extractor registry, complete website families, parent/child message contract, full upstream resume policies, credential/cookie import, authenticated APIs, extraction page caching, template engine, full archive parity, postprocessors, advanced flags and full site coverage.

Keep transport, parsing, per-site authentication and filesystem mutation separate. Modules may share API-family helpers, but every upstream extractor URL behavior is tracked in the port matrix. Do not load or execute Python at runtime. Keep public fixtures synthetic.
