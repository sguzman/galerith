# 📺 BBC public programme gallery preview

Pinned upstream: [gallery-dl bbc.py](https://github.com/mikf/gallery-dl/blob/7f226f74108501c8ac3120548370f68f19e6f9b3/gallery_dl/extractor/bbc.py). Independent Rust implementation; **partial parity only**.

Recognized public inputs are first-party `bbc.co.uk/programmes/ID`, `/programmes/ID/GALLERY`, and `/programmes/ID/galleries?page=N`, with optional `www`. Hosts, credential-bearing URLs, unexpected queries, encoded/raw traversal, unknown page IDs and nondefault ports are rejected. The normal native dispatcher recognizes both gallery originals and the deferred programme gallery collection.

A gallery requires source `class="gallery__thumbnail"` image rows; the pinned default thumbnail path `/320x180_b/` is transformed into the upstream default `/1920xn/` rendition. HTTPS originals must come from the exact `ichef.bbci.co.uk` host and contain a supported image extension. Source-backed metadata includes the programme ID, available JSON-LD `itemListElement` hierarchy, visible title/description, image title/synopsis and ordinal/count. Original-order images are deduplicated by resolved URL; no unverified image hosts or metadata are invented.

Programme `/galleries` pages emit validated, deferred **typed gallery queues**, deduplicated across at most **four pages and 128 children**; queue-following is opt-in. Each public HTML response is capped at **2 MiB**, each gallery to **160 originals**, and subsequent programme pages are paced by 500 ms. Redirects ending on another origin are rejected. The gallery and collection modes are distinct; the flat extractor does not silently download a gallery index as original media.

[Linux CI 40204015367](https://github.com/sguzman/galerith/actions/runs/40204015367) passed Rust formatting, strict Clippy, tests, registry checks, and release build. Offline regressions cover trusted images and metadata, hostile route/media/origin rejection, one real gallery GET and two programme-listing HTTP requests. **Live BBC pages and media are not yet validated.**

**Unaccepted:** configurable width and original fallback image URLs, all BBC gallery HTML/JSON-LD layouts and media renditions, original filename/directory/archive templates, unrestricted pagination, and live differential parity against the pinned Python source.
