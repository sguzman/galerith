# 🖼️ 2chan guarded public Futaba thread preview

Pinned upstream: [gallery-dl 2chan.py](https://github.com/mikf/gallery-dl/blob/7f226f74108501c8ac3120548370f68f19e6f9b3/gallery_dl/extractor/2chan.py). This is an **independent partial Rust port**, not upstream full parity.

Public `SERVER.2chan.net/BOARD/res/THREAD.htm` source URLs (including validated extensionless aliases) are recognized with strict server/board and numeric thread identities. Credential-bearing URLs, port overrides, forged hosts, malformed paths, encoded traversal and query/fragment parameters are rejected.

The adapter reads the canonical first-party thread HTML, extracts source page title and board label, and walks visible post blocks to emit original `/BOARD/src/TIM.ext` attachments. Media preserve first-party source order, safe `TIM.ext` filenames, available numeric post IDs, author name, visible post time and plain-text comment; media origin, board, basename and extension must be validated. Non-original jump links and foreign media URLs are skipped rather than downloading unknown destinations.

Each source HTML response is capped at **2 MiB** including streamed bytes and at most **160 media** are emitted. A local single-request thread HTTP fixture verifies two original attachments; strict route, foreign-host and post metadata tests passed [CI 40139483057](https://github.com/sguzman/galerith/actions/runs/40139483057), with formatting, Clippy and release compilation. **No live Futaba source or CDN behavior was verified.**

**Unaccepted:** complete per-post source metadata, file size/dimensions, pinned board-name shortening, exact timestamps and dates, filename/directory/archive templates in all configurations, alternate thread layouts and live differential parity.
