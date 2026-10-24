# 🗃️ 4archive guarded public thread and board preview

Pinned upstream reference: [gallery-dl 4archive.py](https://github.com/mikf/gallery-dl/blob/7f226f74108501c8ac3120548370f68f19e6f9b3/gallery_dl/extractor/4archive.py). **This is a partial, independent Rust port.**

The native dispatcher recognizes public `https://4archive.org/board/BOARD/thread/NUMERIC_ID` and `/board/BOARD[/PAGE]` URLs. Board slugs, positive IDs, explicit pages, credentials, host lookalikes, default ports, paths and queries are validated before making source requests. A public thread fetch is restricted to **2 MiB** of response HTML, at most **256 source post blocks** and **160 trusted attachments**. The parser preserves the visible title, source post number, optional author, original unparsed date and comment, and direct image/video original URL if it belongs to `4archive.org` or `4cdn.org` and trusted subdomains via HTTPS. It does not create fabricated source timestamps or media metadata.

The public board mode emits first-seen **deferred typed thread queues** from numeric thread IDs, not an eager flat media file list. It is restricted to **four board pages** and **128 queued threads**, with one-second pacing. The generic queue traversal has its own separate cap. Redirects outside the source origin fail closed after the underlying HTTP request.

[Full CI 40140834057](https://github.com/sguzman/galerith/actions/runs/40140834057) passed formatting, strict Clippy, roadmap validation, tests and release build. The suite includes malformed URL/media/board validation and real source-thread and board GET fixtures. An earlier loopback regression exposed and led to repair of a file-text extraction boundary. **These are offline tests, not live site conformance.**

**Not yet accepted:** complete upstream metadata and datetime conversion, every post's independent directory event, file dimensions and sizes, exact filename/directory/archive templates, unlimited board pagination and all live source/media variants. No claim of a complete upstream module.
