# Galerith 🖼️🦀

**A Rust-native port in progress targeting complete [gallery-dl](https://github.com/mikf/gallery-dl) compatibility.**

![Galerith archival-jade project color](assets/galerith-color.svg)

**Color:** `#90D8AA` · `oklch(82.22% 0.0973 155.6)` ([rationale](docs/COLOR.md)).

> **Early bootstrap — not the MVP.** The MVP is **complete gallery-dl feature and extractor compatibility**. Current preview extractors cover direct HTTP(S) media URLs, Danbooru posts/tag searches, e621/e926/e6ai posts/tag searches, and Gelbooru posts/tag searches. **None** currently achieves full upstream parity. No production parity claim is made.

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
cargo run -- --get-urls 'https://safebooru.donmai.us/posts/123'
cargo run -- -C ./cookies.txt 'https://example.org/private.jpg'
cargo run -- -d ./media 'https://example.org/sample.jpg'
cargo run -- -f '{category}_{filename}' --range '1:5:2' --simulate 'https://example.org/sample.jpg'
cargo run -- -c ./config.toml --write-metadata 'https://example.org/sample.jpg'
```

`galerith` is the executable name. The current downloader uses chunked HTTP(S) transfer, validated resumable `.part` files, bounded retries, output locks, existing-file skips, and a native SQLite download archive. See [HTTP recovery guarantees](docs/DOWNLOADER.md), [SQLite archive behavior](docs/ARCHIVE.md), and [format/config/metadata boundaries](docs/COMPATIBILITY.md). Missing upstream features include many options, nearly all site-specific extraction, advanced filename templates and metadata vocabularies, browser cookie importing and site-specific auth, complete resume/archive parity, most postprocessors, and reference JSON message parity. Basic JSON/YAML/TOML configuration, Netscape cookie files, index ranges, filename fields and redacted JSON metadata sidecars are partial previews. Supported preliminary booru tag searches default to one API page (adjust with `--max-pages`). Do not rely on this preview as a replacement for gallery-dl.

## Port tracking

**Overall MVP:** `████░░░░░░ 36%` (44 acceptance-tracked tasks). This is task acceptance progress, **not** site coverage or feature parity.

[🗺️ Roadmap](docs/ROADMAP.md) · [🔢 Evidence ledger](docs/progress.json) · [🧩 Every upstream extractor module](docs/PORT_MATRIX.md) · [🏁 MVP acceptance gate](docs/ACCEPTANCE.md) · [🏗️ Architecture](docs/ARCHITECTURE.md) · [📌 Source baseline](docs/SOURCE_BASELINE.md) · [⚖️ Provenance](docs/PROVENANCE.md) · [🎨 Identity](docs/COLOR.md) · [📋 Issue queue](docs/ISSUES.md).

The extractor matrix tracks 272 pinned upstream Python files; this inventory is **not** a site count. No file is marked fully ported without acceptance evidence.

## Development

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
cargo run --bin galerith-roadmap -- --check
```

**Verified Linux CI:** [run 39954702418](https://github.com/sguzman/galerith/actions/runs/39954702418) (format, clippy, offline tests, roadmap/matrix check and release build). This verifies the bootstrap only, not upstream parity. The repository uses one native Rust crate, not a monorepo.

## License

Galerith's original independent Rust code is MIT licensed; upstream gallery-dl is GPL-2.0. No upstream Python source is copied into this initial implementation. **Direct translations/derivative implementations must follow the GPL-2.0 requirements**, and the repository license would need to change before distributing those translations. See [provenance](docs/PROVENANCE.md).
