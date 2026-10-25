# 🧱 Are.na guarded public channels

Pinned source: [gallery-dl arena.py](https://github.com/mikf/gallery-dl/blob/7f226f74108501c8ac3120548370f68f19e6f9b3/gallery_dl/extractor/arena.py). **Independent partial Rust implementation**, not complete upstream parity.

The registered native CLI adapter matches public `are.na/USER/CHANNEL` and `www.are.na` URLs, rejecting credentials, port overrides, hostile queries, encoded traversal and unrelated hosts. It reads bounded `https://api.are.na/v2/channels/CHANNEL` metadata, followed by `/contents?page=N&per=100`, for at most **four API pages**, 100 blocks per page and 256 emitted original media. Every response is capped at **2 MiB** of actual JSON; unexpected cross-origin redirects are rejected. Pacing is one second between subsequent contents pages.

For each source block, attachments take precedence, then image original/display/large renditions in the pinned order. The current preview admits only explicitly validated `are.na` source host/subdomains over HTTPS with supported media extensions; it intentionally omits arbitrary external attachment/CDN URLs. Duplicate media URLs are suppressed. Directory/URL events retain source-backed channel ID/title metadata, owner/user records, block JSON and ordinal/ID without inventing inaccessible fields, archives or timezone conversions. The normal flat downloader receives the corresponding media URL and stable source-indexed filename.

[CI 40146310559](https://github.com/sguzman/galerith/actions/runs/40146310559) passed formatting, Clippy, tests and release build. It includes unsafe-route rejection, source attachment versus image fallback fixtures and a local two-request channel/contents HTTP regression. **Neither live Are.na API behavior nor download success from live CDN hosts is established.**

**Unaccepted:** external attachment and image storage sources, complete upstream original/filename/archive/directory/date semantics, uncapped public channel history, authenticated content and live differential conformance with the pinned upstream extractor.
