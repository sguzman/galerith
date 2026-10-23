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
- `src/lib.rs`: direct-media URL recognition and safe leaf filenames.
- `src/download.rs`: single-item asynchronous streaming, .part rename and bounded request retries.
- `tests/cli.rs`: basic offline CLI cases.
- `src/bin/galerith-roadmap.rs`: evidence-backed source-of-truth rendering.

Not yet implemented: extractor registry, website families, parent/child message contract, partial-range resumes, credential/cookie import, authenticated APIs, extraction page caching, template engine, archive DB, postprocessors, advanced flags and full site coverage.

Keep transport, parsing, per-site authentication and filesystem mutation separate. Modules may share API-family helpers, but every upstream extractor URL behavior is tracked in the port matrix. Do not load or execute Python at runtime. Keep public fixtures synthetic.
