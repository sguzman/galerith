# 🧵 Jschan 94chan public thread and board partial port

Pinned source: [gallery-dl jschan.py](https://github.com/mikf/gallery-dl/blob/7f226f74108501c8ac3120548370f68f19e6f9b3/gallery_dl/extractor/jschan.py). **Status: independent partial Rust implementation.**

Public route matching is restricted to the pinned `94chan.org` instance: `/BOARD/thread/ID.html` for individual threads and `/BOARD/`, `/BOARD/index.html`, `/BOARD/catalog.html` and numeric board page aliases. Board and thread identifiers are validated before any network request; credentials, lookalike hosts, queries, fragments, alternate ports and encoded traversal are rejected.

Thread mode fetches exactly `/BOARD/thread/ID.json` and requires the source thread ID to equal the requested ID. A bounded source parser emits one typed directory event plus ordered root/reply file URL events, retaining available thread subject, post ID, thread ID, original filename, first-party stored filename, source attachment count and zero-based attachment ordinal. Media files are constructed on pinned `https://94chan.org/file/FILENAME` only, with validated safe filename/extension. Flat thread extraction yields only media files.

Board mode fetches `/BOARD/catalog.json` and emits first-seen, deduplicated **deferred child thread queues** rather than eagerly fetching child media. The public API response is capped at **2 MiB**, with **256 source thread posts**, **160 media attachments**, **12 attachments per post**, and **128 catalog child entries** as conservative implementation bounds.

[CI 40211638711](https://github.com/sguzman/galerith/actions/runs/40211638711) passed native formatting, strict Clippy, synthetic fixture tests, a two-request loopback HTTP thread/catalog test, the pinned directlink comparison check, and release build. **Live 94chan responses and complete upstream behavior were not verified.**

**Remaining:** other instance support, full source metadata and post attributes, exact directory/filename/archive formats, large board and thread histories, original media response checks and live differential parity.
