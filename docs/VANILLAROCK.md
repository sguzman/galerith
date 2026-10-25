# 🗞️ Vanilla Rock public article-image preview

Pinned upstream: [gallery-dl vanillarock.py](https://github.com/mikf/gallery-dl/blob/7f226f74108501c8ac3120548370f68f19e6f9b3/gallery_dl/extractor/vanillarock.py). **Independent partial Rust port**, not full pinned gallery-dl parity.

The native CLI routes strict public `vanilla-rock.com/POST`, `/tag/TAG` and `/category/TAG` paths (with optional `www`). Credentials, host lookalikes, unsupported query/fragment parameters, encoded traversal and **raw** dot-segment traversal are rejected before URL normalization. Each public HTML response has a **2 MiB** streaming cap and cross-origin final redirect rejection.

Public posts read the visible `entry-title`, optional source date, and `main-img` anchor originals. The subset accepts HTTPS media on `vanilla-rock.com` or real subdomains only, checks safe extensions and filenames, deduplicates and preserves source order, and emits source-backed ordinal/count/title/date/path fields. Up to **160 originals** are included per supported post; unavailable post IDs, timezone and archive fields are not fabricated.

Tag/category pages read `entry-title` anchors into first-seen unique **typed child queues**, with strict source-origin and next-page sequence validation. Discovery is capped at **four pages and 128 queued posts**, with one-second pacing. Following post children requires explicit queue traversal. The flat listing view does not silently fetch their media.

[Full CI 40156584743](https://github.com/sguzman/galerith/actions/runs/40156584743) passed formatting, Clippy, all **497 Rust tests**, roadmap checks and release build. Tests include source-origin and traversal regressions plus one article GET and two-page tag discovery over local HTTP. A real raw traversal regression was caught and corrected during implementation, before acceptance.

**Unaccepted:** full source HTML/date/tag variations, additional trusted image CDNs, nested categories and all listing pagination modes, exact upstream filenames/directory/archive, unlimited histories and live pinned upstream differential conformance.
