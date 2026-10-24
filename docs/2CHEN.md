# 🧵 2chen public thread and catalog preview

Pinned upstream: [gallery-dl 2chen.py](https://github.com/mikf/gallery-dl/blob/7f226f74108501c8ac3120548370f68f19e6f9b3/gallery_dl/extractor/2chen.py). **Independent Rust partial port; not complete upstream equivalence.**

The adapter recognizes public Sturdychan and Schan threads `/BOARD/THREAD`, boards `/BOARD/`, and `/BOARD/catalog`. The pinned `2chen.moe` and `2chen.club` aliases canonically use `sturdychan.help`. It rejects forged hosts, credentials, private queries, alternate ports and encoded traversal.

Public thread HTML carries a validated source board name, visible thread title, and ordered `glass media` article originals. For supported file types, typed directory and URL events preserve the source filename, post number, visible author, unparsed date string, and available hash. Original media URLs must remain within the exact source origin, and traversal/unsafe filenames are rejected. The source date is intentionally **not assigned a timezone or converted to a fabricated UTC time**.

The public catalog emits deferred validated same-board thread queues, deduplicated by first-seen numeric thread IDs; queue expansion is opt-in. Each HTML response is capped at **2 MiB**, and supported threads and catalogs are limited to **128 media or child records** respectively.

[CI 40138833991](https://github.com/sguzman/galerith/actions/runs/40138833991) passed Rust formatting, strict Clippy, unit tests, roadmap consistency and release build. Synthetic fixtures include strict origin checks, two-image thread parsing and separate local HTTP thread/catalog GET tests. **Live source compatibility has not been established.**

**Unaccepted:** complete source-specific thread HTML layouts, UTC date conversion, full gallery-dl metadata/filename/directory/archive behavior, unrestricted catalog discovery, original CDN variants and live pinned differential parity.
