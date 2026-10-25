# 🗂️ Picazor public user-files preview

Pinned source: [gallery-dl picazor.py](https://github.com/mikf/gallery-dl/blob/7f226f74108501c8ac3120548370f68f19e6f9b3/gallery_dl/extractor/picazor.py). **Independent partial port, not full upstream equivalence.**

Recognizes exact public `picazor.com/LANG/USER` and `www.picazor.com` profile paths with two-letter lowercase language and validated user slug. It reads the pinned `GET /api/files/USER/sfiles?page=N` source API with the original `Referer: https://picazor.com/en/USER`, observing **2 MiB JSON**, **48 files per page**, **4 pages** and **160 media output** caps. Files are returned in source order with available ID, numeric order, subject and exact path. One typed directory event is emitted on the first usable original, followed by typed URLs.

Media URL paths are required to be local and safe even before URL normalization: hostile domains, raw or encoded dot-traversal, encoded separators, double-encoded segments, credentials and sensitive query parameters are rejected. Supported originals include common image extensions and MP4/WebM media. Filename approximation uses the upstream ID plus zero-padded original order and extension.

[CI 40197408931](https://github.com/sguzman/galerith/actions/runs/40197408931) passed Linux Rust formatting, strict Clippy, unit/adversarial tests, roadmap checks and release build. A real two-request HTTP fixture checks API pagination and the pinned referer. No live Picazor request, browser session or remote asset response is claimed validated.

**Outstanding:** full upstream file type and metadata mappings, exact archive/directory/filename behaviors, unbounded pagination, authenticated/browser variants and live pinned differential comparison.
