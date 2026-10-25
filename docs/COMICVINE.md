# 🖼️ ComicVine guarded public tag galleries

Pinned upstream source: [gallery-dl comicvine.py](https://github.com/mikf/gallery-dl/blob/7f226f74108501c8ac3120548370f68f19e6f9b3/gallery_dl/extractor/comicvine.py). **Independent partial Rust port, not complete parity.**

The native dispatcher matches strict public `comicvine.gamespot.com/TAG/NUMBER-NUMBER/images/` gallery routes, rejecting forged hosts, credentials, ports, encoded traversal and unexpected query fields. The public gallery HTML supplies a validated `data-gallery-id`, followed by pinned `/js/image-data.json` requests with `images`, `start`, `count=1000` and `object` parameters. Requests reject unexpected redirects and enforce a **2 MiB actual-response cap**. Pagination is capped at **3 API pages / 3000 image originals**, with one-second pacing and first-seen URL deduplication.

Validated originals may use first-party `comicvine.gamespot.com`, `static.comicvine.com` or `comicvine1.cbsistatic.com` HTTPS image hosts, with safe UTF-8 percent-decoded path segments and supported image extensions. Typed directory/URL events preserve known source image ID, gallery object ID, tag slug, file ordinal, nonempty tag names, date source and validated Gregorian `date` (calendar only, no invented time or timezone). Other metadata is not guessed.

Offline fixture coverage includes strict public routing, trusted media/original dedup and tag parsing, a local gallery-HTML plus one-page JSON API HTTP test, a realistic 1000-image first page followed by a second-page source offset of 1000 with overlap removal, and invalid/leap calendar-date rejection. Full Linux [CI 40154046581](https://github.com/sguzman/galerith/actions/runs/40154046581) passed format, Clippy, tests, roadmap consistency and release build.

**Not accepted:** all upstream dynamic image CDN and extension variants, complete post metadata and filename/directory/archive templates, pagination beyond three pages, cookie/authorization variants and live pinned upstream differential equivalence. Successful synthetic tests do not establish live site compatibility.
