# Galerith 🖼️🦀

**A complete Rust-native port of [gallery-dl](https://github.com/mikf/gallery-dl).**

![Galerith archival-jade project color](assets/galerith-color.svg)

**Color:** `#90D8AA` · `oklch(82.22% 0.0973 155.6)` ([rationale](docs/COLOR.md)).

> **Early bootstrap — not the MVP.** The MVP is **complete gallery-dl feature and extractor compatibility**. Current preview extractors cover direct HTTP(S) media URLs and basic Danbooru posts/tag searches. Neither implements complete upstream parity. No production parity claim is made.

## Native CLI preview

```sh
cargo run -- --list-extractors
cargo run -- --get-urls 'https://example.org/sample.jpg'
cargo run -- --dump-json 'https://example.org/sample.jpg'
cargo run -- --simulate 'https://example.org/sample.jpg'
cargo run -- -d ./media 'https://example.org/sample.jpg'
```

`galerith` is the executable name. The current downloader uses chunked HTTP(S) transfer, temporary `.part` files, bounded retries, and existing-file skips. Missing upstream features include most options, nearly all site-specific extraction, metadata-template parity, cookies/auth, resume support, archives, postprocessing, and reference JSON output. Danbooru tag searches default to one API page (adjust with `--max-pages`). Do not rely on this preview as a replacement for gallery-dl.

## Port tracking

**Overall MVP:** `███░░░░░░░ 25%` (42 acceptance-tracked tasks). This is task acceptance progress, **not** site coverage or feature parity.

[🗺️ Roadmap](docs/ROADMAP.md) · [🔢 Evidence ledger](docs/progress.json) · [🧩 Every upstream extractor module](docs/PORT_MATRIX.md) · [🏁 MVP acceptance gate](docs/ACCEPTANCE.md) · [🏗️ Architecture](docs/ARCHITECTURE.md) · [📌 Source baseline](docs/SOURCE_BASELINE.md) · [⚖️ Provenance](docs/PROVENANCE.md) · [🎨 Identity](docs/COLOR.md) · [📋 Issue queue](docs/ISSUES.md).

The extractor matrix tracks 272 pinned upstream Python files; this inventory is **not** a site count. No file is marked fully ported without acceptance evidence.

## Development

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
cargo run --bin galerith-roadmap -- --check
```

**Verified Linux CI:** [run 39898967165](https://github.com/sguzman/galerith/actions/runs/39898967165) (format, clippy, offline tests, roadmap/matrix check and release build). This verifies the bootstrap only, not upstream parity. The repository uses one native Rust crate, not a monorepo.

## License

Galerith's original independent Rust code is MIT licensed; upstream gallery-dl is GPL-2.0. No upstream Python source is copied into this initial implementation. **Direct translations/derivative implementations must follow the GPL-2.0 requirements**, and the repository license would need to change before distributing those translations. See [provenance](docs/PROVENANCE.md).
