# 🗂️ 4chanarchives guarded public preview

Pinned upstream source: [gallery-dl 4chanarchives.py](https://github.com/mikf/gallery-dl/blob/7f226f74108501c8ac3120548370f68f19e6f9b3/gallery_dl/extractor/4chanarchives.py). **Independent partial Rust port, not complete upstream parity.**

The native dispatcher accepts first-party public `4chanarchives.com/board/BOARD/thread/NUMERIC_ID` and `/board/BOARD[/PAGE]` URLs. Strict URL parsing rejects lookalike hosts, unexpected credentials, alternate ports, fragments, queries, traversal and invalid IDs. Source HTML is capped at **2 MiB**, with at most **256 public posts** and **160 extracted attachments** per thread. Trusted original image/video URLs are HTTPS `4chanarchives.com` or `4cdn.org` domains and their actual subdomains. Source OG title, post number, optional author, unparsed post time and visible comment are carried into typed media events; unverified dates and dimensions are not fabricated.

Board pages emit **typed deferred thread queues**, deduplicated by numeric thread ID and confined to the exact requested board. Discovery is limited to **four source board pages** and **128 queued threads**, with one-second pacing. Flat board URL extraction is not silently equated with recursive downloads. Redirects are checked against the requested source origin after HTTP response completion.

[CI 40141229791](https://github.com/sguzman/galerith/actions/runs/40141229791) passed format, strict Clippy, full tests, machine-ledger validation and release build. The suite covers hostile URL and attachment origins, trusted metadata, duplicate/wrong-board links, and real loopback thread and board HTTP GET fixtures. **These fixtures are synthetic; live-site compatibility is not established.**

**Unaccepted:** exact upstream directory events for text-only posts, complete timestamps, dimensions and file size, naming/archive templates, unrestricted board pagination and live pinned upstream differential conformance.
