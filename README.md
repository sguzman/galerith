# Galerith 🖼️🦀

**A Rust-native port in progress targeting complete [gallery-dl](https://github.com/mikf/gallery-dl) compatibility.**

![Galerith archival-jade project color](assets/galerith-color.svg)

**Color:** `#90D8AA` · `oklch(82.22% 0.0973 155.6)` ([rationale](docs/COLOR.md)).

> **Early bootstrap — not the MVP.** The MVP is complete native Rust gallery-dl feature and extractor compatibility. Current partial previews cover direct media links, Bluesky public posts/replies/media/search/feeds, Catbox, Imgbox, ImgBB, Imgpile, Imgth, ImageBam, Pixeldrain, Telegraph, Speaker Deck, WallpaperCave, Wallhaven public wallpapers/search/uploads, user profiles and flattened collection indexes, Comedy Wildlife Photography, PhotoVogue, Webmshare, Desktopography, Lightroom, Unsplash, Lexica, Wikimedia Commons files/articles/categories, Danbooru, e621/e926/e6ai, Gelbooru, the five-host Gelbooru v0.2 XML family, Moebooru, Philomena, FoolFuuka archives (threads, bounded boards, galleries and search), and 4chan/4channel threads and bounded boards. None has complete upstream extractor parity or live-site acceptance.

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
cargo run -- -a 'MyAgent/1.0' --proxy 'http://127.0.0.1:8080' 'https://example.org/sample.jpg'
cargo run -- --sleep 0.15 -d ./media 'https://example.org/sample.jpg'
cargo run -- --get-urls 'https://boards.4chan.org/g/thread/123456'
```

`galerith` is the executable name. The native preview includes stream/resume/retries, path locks, a Galerith-specific SQLite archive, explicit JSON/YAML/TOML configuration, Netscape cookies, filename fields, index ranges, redacted JSON metadata sidecars, proxy/user-agent options and limited `--sleep` pacing. The read-only `--dump-messages` flag exposes typed directory/URL/queue events. Opt-in `--expand-queues --queue-depth N` (1–4 levels, default 1) inspects bounded child queues without downloading. Separately, `--follow-queues` uses the two-level, maximum-eight-child message pipeline for actual media downloading, keeping the original resolver as the default. Four opt-in selector families filter typed file events, child queues, directory scopes and actual numeric source-post IDs before their actions; see [job filtering](docs/JOB_FILTERS.md). Source-post selection preserves media/archive IDs separately, follows source-ID-unknown board queues within bounded request limits, and disables unsafe finite-page shortcuts. Finite typed file ranges can also stop extra API pages in Wallhaven, Gelbooru JSON/XML, Moebooru and Danbooru tag searches; see [pagination pushdown](docs/PAGINATION_PUSHDOWN.md). 4chan typed threads preserve API-backed post scopes and typed board listings now emit bounded deferred thread queues. Selected finite file ranges can stop later child-queue requests; Wallhaven and several booru previews can stop later API pages when the bound is safe. See [source post scopes and pagination](docs/POST_SCOPES_AND_PAGING.md). Neither mode is full upstream message-job parity; see [message protocol](docs/MESSAGE_PROTOCOL.md). See [downloader](docs/DOWNLOADER.md), [network options](docs/NETWORK_OPTIONS.md), [configuration](docs/CONFIG.md), [extractor previews](docs/EXTRACTOR_PREVIEWS.md) and [Gelbooru XML subset](docs/GELBOORU_V02.md), and [full compatibility gaps](docs/COMPATIBILITY.md). Full source-site extraction, auth, metadata, format templates, archive interchange, postprocessors, full recursive message job handling and live-site conformance are still missing. Previews are not production replacements.

## Port tracking

**Overall MVP:** `██████░░░░ 58%` (89 acceptance-tracked tasks). This is task acceptance progress, **not** site coverage or feature parity.

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
