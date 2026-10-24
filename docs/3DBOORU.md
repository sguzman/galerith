# 🧩 3dbooru guarded Behoimi public adapter

Pinned source: [gallery-dl 3dbooru.py](https://github.com/mikf/gallery-dl/blob/7f226f74108501c8ac3120548370f68f19e6f9b3/gallery_dl/extractor/3dbooru.py). **Status: independent partial Rust compatibility**, sharing the native [Moebooru adapter](../src/extractor/moebooru.rs) rather than duplicating its post/tag/pool iteration.

The matcher accepts strictly first-party `behoimi.org` and `www.behoimi.org` public post IDs, `/post?tags=...` or `/post/index?tags=...` searches, pool IDs and day/week/month/recent popular pages, with valid http or https entry URLs. Behoimi's pinned API is **HTTP** `http://behoimi.org/post/index.json` for post/tag/pool queries, unlike the other Moebooru hosts' HTTPS `/post.json`. API requests set pinned `Referer: http://behoimi.org/post/show/` and `Accept-Encoding: identity`, validate same-origin redirects and cap received JSON to **8 MiB**.

Native Moebooru record parsing chooses available original file URLs, validates source post IDs and extensions, and preserves the catalog category `3dbooru`. Generic bounded page counts and finite media-selection limits apply. Unknown/forged hosts, credential-bearing URLs, nonstandard ports, zero post IDs and plaintext HTTP to unrelated Moebooru domains are rejected.

[Linux CI 40143946115](https://github.com/sguzman/galerith/actions/runs/40143946115) passed format, strict Clippy, tests, source-ledger consistency and release build. Synthetic HTTP tests validate the Behoimi API path, query tags, pinned headers and returned JPEG item. This does **not** establish live Behoimi availability.

**Outstanding:** full upstream source metadata, filename/directory/archive outputs, permissive/age-sensitive media behavior, authenticated access, all API response variants, and live pinned upstream differential parity. This module is not marked complete.
