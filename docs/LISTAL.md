# 🖼️ Listal public media preview

Pinned upstream: [gallery-dl listal.py](https://github.com/mikf/gallery-dl/blob/7f226f74108501c8ac3120548370f68f19e6f9b3/gallery_dl/extractor/listal.py). This is an **independent partial** Rust adaptation, not a claim of full Listal support.

Strict first-party `listal.com` and `www.listal.com` routes recognize public `/viewimage/NUMERIC_ID` pages and `/PERSON/pictures` collections. Source usernames and numeric image IDs are validated; URL credentials, lookalike domains, traversal, query-bearing routes and alternate ports are rejected.

The single-image reader fetches the upstream `/viewimage/IDh` public page once and requires an original image anchor with HTTPS media under `listal.com` or `lisimg.com` (including subdomains). The conservative extension whitelist covers JPG, PNG, GIF, WebP and AVIF. Typed directory and URL messages carry source image ID, canonical image page URL, validated visible title, dimensions, author and unparsed source date where available. Output filenames are sanitized; no image post ID is invented.

The public people-picture reader emits **deferred** first-party image queues from validated links. It deduplicates first-seen numeric IDs, limits discovery to **four pages / 128 images**, enforces actual **2 MiB** HTML caps per source request, and requires trusted next-page links. A foreign or lookalike hostname cannot force a child URL or pagination request. Following child media requires opt-in queue traversal.

Source-route, media metadata, hostile-host and one-request image fixtures plus a two-page deduplication loopback HTTP test passed [Linux CI 40130699655](https://github.com/sguzman/galerith/actions/runs/40130699655), including formatter, Clippy, tests, roadmap consistency and release build.

**Unaccepted:** full date parsing, upstream filename/directory/archive templates, all media/CDN formats and publisher layouts, unrestricted pagination, media response verification, cookies/authentication variations and live differential reference tests. This remains a partial upstream module.
