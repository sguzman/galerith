# 🎨 Foriio guarded public image works and user queues

Pinned source: [gallery-dl foriio.py](https://github.com/mikf/gallery-dl/blob/7f226f74108501c8ac3120548370f68f19e6f9b3/gallery_dl/extractor/foriio.py). **Independent partial Rust port.**

Supported URLs are strict first-party `foriio.com/works/ID`, `foriio.com/USER`, equivalent `www` hosts and `fori.io` aliases. Inputs with unsafe host lookalikes, traversal, credentials, alternate ports, query parameters and fragments are refused. Native dispatch uses the pinned public `https://api.foriio.com/api/v1/works/ID` and `/v1/users/USER/works` endpoints.

An image work requires a matching source `work.id` and a nonempty bounded image list. Each source `urls.list` must be a validated HTTPS `foriio.imgix.net` image URL; the adapter reconstructs the upstream original `https://foriio.imgix.net/store/BASENAME` without thumbnail query settings. It preserves first-party source work/file IDs, title, date string, author object, dimensions, format, image ordinal/count and post URL in typed directory/URL events, with up to **128 original images** per work.

Public user work feeds emit deferred typed child work queues, not eager media downloads. The public API request retains Origin and Referer headers, `page=N&per_page=20`, strict source page/total validation, first-seen deduplication, at most **four pages / 80 children**, one-second pacing and a **2 MiB streamed JSON bound**. Cross-origin response redirects and malformed public page identities fail closed.

[Full CI 40149290826](https://github.com/sguzman/galerith/actions/runs/40149290826) passed formatting, strict Clippy, roadmap consistency, tests and release build. Synthetic fixtures cover exact work identity and rejected forged CDN paths, one-work HTTP JSON, and two-page user queues with duplicate suppression. These are **offline tests** rather than proof of compatibility with live Foriio API responses.

**Unaccepted:** upstream video, sound and external web-article outputs, user `posts` filters, previews/media options, complete date/filename/archive templates, full author metadata semantics and live pinned differential conformance.

**Copy-writing source images:** One image in a `copy_writing` record is now accepted with its source `id`, canonical Imgix original URL, numbered filename and count=1. The fallback does not claim to support textual article exports or other media delegation. Offline regression and the full [CI 40149604757](https://github.com/sguzman/galerith/actions/runs/40149604757) passed.
