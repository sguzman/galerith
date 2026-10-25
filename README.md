# Galerith 🖼️🦀

**A Rust-native port in progress targeting complete [gallery-dl](https://github.com/mikf/gallery-dl) compatibility.**

![Galerith archival-jade project color](assets/galerith-color.svg)

**Color:** `#90D8AA` · `oklch(82.22% 0.0973 155.6)` ([rationale](docs/COLOR.md)).

> **Early bootstrap — not the MVP.** The MVP requires complete Rust-native gallery-dl feature and extractor compatibility. Public and authenticated extractor support is tracked source-by-source in the [pinned parity matrix](docs/PORT_MATRIX.md); partial adapters and synthetic tests are not evidence of live-site compatibility. The single currently complete source-file entry is an independently tested helper, not completion of a gallery website.

## Native CLI preview

```sh
cargo run -- --list-extractors
cargo run -- --get-urls 'https://example.org/sample.jpg'
cargo run -- --dump-json 'https://example.org/sample.jpg'
cargo run -- --simulate 'https://example.org/sample.jpg'
cargo run -- --range '1:10:2' --get-urls 'https://example.org/sample.jpg'
cargo run -- --filename '{category}_{stem}.{extension}' --dump-json 'https://example.org/sample.jpg'
cargo run -- --write-metadata -d ./media 'https://example.org/sample.jpg'
cargo run -- --config-json ./settings.json --get-urls 'https://example.org/sample.jpg'
cargo run -- --download-archive ./gallery.sqlite -d ./media 'https://example.org/sample.jpg'
cargo run -- --download-archive ./native.sqlite --gallery-dl-archive ./gallery-dl.sqlite -d ./media 'https://example.org/sample.jpg'
cargo run -- --download-archive ./native.sqlite --gallery-dl-archive ./gallery-dl.sqlite --archive-import
cargo run -- --get-urls 'https://safebooru.donmai.us/posts/123'
cargo run -- -C ./cookies.txt 'https://example.org/private.jpg'
cargo run -- -d ./media 'https://example.org/sample.jpg'
cargo run -- -f '{category}_{filename}' --range '1:5:2' --simulate 'https://example.org/sample.jpg'
cargo run -- -c ./config.toml --write-metadata 'https://example.org/sample.jpg'
cargo run -- -a 'MyAgent/1.0' --proxy 'http://127.0.0.1:8080' 'https://example.org/sample.jpg'
cargo run -- --sleep 0.15 -d ./media 'https://example.org/sample.jpg'
cargo run -- --get-urls 'https://boards.4chan.org/g/thread/123456'
```

The native preview includes resumable transfers and retries, cross-process download/archive locks, hash-only import of supported gallery-dl archive identities, source-scoped Danbooru/e621-family and Moebooru and Gelbooru-family archive IDs, and an optional native SQLite public collection metadata cache (`--cache-db`, `--cache-ttl`). Gallery-dl's Python-pickled cache is not reused. Configuration supports explicit JSON, YAML and TOML files; complete archive and cache parity remains an acceptance gate.

## Port tracking

**Overall MVP:** `███████░░░ 69%` (160 acceptance-tracked tasks). This is task acceptance progress, **not** site coverage or feature parity.

**Pinned extractor-source inventory:** 1 complete · 76 partial · 195 not ported (272 upstream Python files).

[🗺️ Roadmap](docs/ROADMAP.md) · [🔢 Evidence ledger](docs/progress.json) · [🧩 Every upstream extractor module](docs/PORT_MATRIX.md) · [🏁 MVP acceptance gate](docs/ACCEPTANCE.md) · [🏗️ Architecture](docs/ARCHITECTURE.md) · [📨 Message preview](docs/MESSAGE_PROTOCOL.md) · [🔎 Typed job filters](docs/JOB_FILTERS.md) · [🧱 Post scopes and pagination](docs/POST_SCOPES_AND_PAGING.md) · [🧵 4chan queue](docs/FOURCHAN_QUEUE.md) · [📌 Source baseline](docs/SOURCE_BASELINE.md) · [⚖️ Provenance](docs/PROVENANCE.md) · [🎨 Identity](docs/COLOR.md) · [📋 Issue queue](docs/ISSUES.md).

The extractor matrix tracks 272 pinned upstream Python files; this inventory is **not** a site count. No file is marked fully ported without acceptance evidence. Lexica's public POST/cursor preview has four CI-backed accepted checks; complete upstream parity remains open.

## Development

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
cargo run --bin galerith-roadmap -- --check
```

**Verified Linux CI:** [run 40040566586](https://github.com/sguzman/galerith/actions/runs/40040566586) (format, clippy, offline tests, roadmap/matrix check and release build). This verifies the bootstrap only, not upstream parity. The repository uses one native Rust crate, not a monorepo. Push CI publishes a `galerith/rust-ci` commit status for machine-readable verification.

## License

Galerith's original independent Rust code is MIT licensed; upstream gallery-dl is GPL-2.0. No upstream Python source is copied into this initial implementation. **Direct translations/derivative implementations must follow the GPL-2.0 requirements**, and the repository license would need to change before distributing those translations. See [provenance](docs/PROVENANCE.md).
