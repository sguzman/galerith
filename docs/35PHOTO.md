# 📸 35photo guarded public extractor subset

Pinned source: [gallery-dl 35photo.py](https://github.com/mikf/gallery-dl/blob/7f226f74108501c8ac3120548370f68f19e6f9b3/gallery_dl/extractor/35photo.py). This is an **independent partial** Rust adaptation and not complete upstream compatibility.

The native CLI recognizes public `35photo.pro/photo_ID/`, `/tags/TAG/`, `/genre_ID/` and `/USER/` routes, including approved source subdomains. URL matching rejects lookalike hosts, credentials, queries, nonstandard ports, invalid numeric photo and genre identifiers and traversal. Original image and source metadata are requested from `api.35photo.pro/?method=photo.getData&photoId=ID`. The parser validates matching source IDs and first-party HTTPS media origins, preserves single-image or bounded series order, and carries available title, description, user, views, favorites, score, date source, tags and per-series metadata.

Public listing pages emit **typed deferred child-photo URLs**, with source-scoped category values, deduplication and up to four page/128-child traversal bounds. Each response is limited to **2 MiB**, and additional tag-page requests are paced one second apart; one photo's series is limited to 32 images. Following queues is opt-in; a listing-only route is not a flat image download. Genre and user routes currently expose an initial page rather than upstream's AJAX pagination; this is a known parity gap.

Source URL validation, original/series JSON decoding, trusted-host listing queues, and the original-photo API plus tag-listing multi-page local HTTP fixture passed full [Linux CI 40142431871](https://github.com/sguzman/galerith/actions/runs/40142431871), including formatting, Clippy, tests and release build. **No live source/API/CDN conformance is claimed.**

**Unaccepted:** full user/genre AJAX page handling, source-aware date/filename/directory/archive equivalence, unrestricted listing pagination, optional metadata variations, and live pinned upstream differential parity.
