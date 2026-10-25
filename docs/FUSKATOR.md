# 🖼️ Fuskator guarded public gallery and search preview

Pinned source: [gallery-dl fuskator.py](https://github.com/mikf/gallery-dl/blob/7f226f74108501c8ac3120548370f68f19e6f9b3/gallery_dl/extractor/fuskator.py). This is an **independent partial Rust implementation** and does not claim complete upstream compatibility.

The public matcher recognizes exact first-party `fuskator.com/thumbs/HASH` and `/expanded/HASH` galleries and bounded `/search/TAG` or `/page/TAG` discovery. Unsafe query parameters, lookalike hosts, credentials, traversal and alternate ports are rejected. Public gallery processing follows the pinned three-step source flow: GET `/thumbs/HASH/index.html`, anonymous POST `/ajax/auth.aspx` with XMLHttpRequest and Referer headers, then GET `/ajax/gal.aspx` with validated token, hash and epoch-second cache-busting parameter. Source HTML/token/JSON responses are capped at **2 MiB**, and foreign-origin redirects are rejected after HTTP resolution.

The original image mapping accepts trusted Fuskator HTTPS media URLs (including protocol-relative `//` source addresses), rejecting forged media origins, unsafe raw path traversal, query fields and unsupported extensions. It retains the gallery hash, visible title, optional numeric gallery ID, order, source image count, tag list, hits and rating in typed directory/URL events. Image discovery is bounded to **160 originals per gallery**.

Search mode emits deduplicated, deferred gallery queues with exact first-party canonical thumbnails. It is bounded to **four listing pages** and **128 child galleries**, with one-second inter-page pacing and source-scoped pagination. The Rust adapter does not silently download child originals without explicit queue-following mode.

The route/media/queue parser, last-link next-page origin checks and a real three-request local album HTML → anonymous auth token → gallery JSON fixture passed [Linux CI 40217072712](https://github.com/sguzman/galerith/actions/runs/40217072712), including format, Clippy, tests, roadmap consistency and release compilation. This does not establish live server/CDN success.

**Unaccepted:** unbounded page/result access, richer source page variants, complete field/date/filename/archive templates, special auth-cookie states, binary media-response verification and live upstream differential parity.
