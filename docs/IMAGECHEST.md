# 🖼️ ImageChest public extractor preview

Galerith's ImageChest support is an independently implemented Rust **partial** port of pinned upstream `gallery_dl/extractor/imagechest.py` at `7f226f74108501c8ac3120548370f68f19e6f9b3`. It is not the complete gallery-dl extractor or a claim of live-site parity.

## Public routes and request model

- `https://imgchest.com/p/ID`: 11-character alphanumeric public gallery ID. One bounded HTML request reads the `data-page` JSON payload and extracts `props.post.files`. Only safe HTTPS links of the form `https://cdn.imgchest.com/files/ID.ext` with recognized media extensions are emitted. File links and IDs are cross-checked and repeated URLs are deduplicated. Flat extraction and typed messages share the same parser.
- `https://imgchest.com/u/USERNAME`: public user gallery index, **typed queue path only**. The pinned upstream-style `/api/posts` public listing uses safe constructed page numbers, no credentials and no server-provided continuation URL. It emits validated `queue` messages, not downloadable files. A matching ID or a canonical public `link` establishes the gallery identity; if both are present they must agree.
- Read-only queue expansion through `--dump-messages --expand-queues` or typed download mode `--follow-queues` can visit supported child galleries subject to the global queue safety policies. The default flat user-profile route is not yet implemented and returns an explicit unsupported-mode error.

Gallery responses are limited to **2 MiB** of HTML and **256** file records. User API JSON responses are limited to **2 MiB** each, with at most **4** pages and **32** emitted galleries, plus a **100-record** maximum per page and one-second pacing between requests. This is not a global byte budget. Non-public, malformed or noncanonical URL shapes are not followed.

## Typed metadata subset

Gallery `directory` and `url` events preserve `gallery_id`, `category`, `filename` and `extractor`. When present and valid, public page JSON additionally provides `title` (at most 256 bytes), `username` (restricted to known-safe public profile characters), `image_count` (bounded), per-file `file_id`, `position` and `description` (at most 1024 bytes). Control characters and invalid/out-of-range values are excluded. A gallery's alphanumeric ID is **not** invented as a numeric `post_id` or `source_post_id`. Flat JSON and downloader media items retain their original compatible fields.

## Evidence and remaining acceptance

The public gallery and user-queue implementation passed [Linux CI 40047578834](https://github.com/sguzman/galerith/actions/runs/40047578834). Canonical listing-link validation, metadata sanitization and single-fetch typed metadata fixtures passed [Linux CI 40051572947](https://github.com/sguzman/galerith/actions/runs/40051572947). All evidence is synthetic/offline; it is not an assertion that the current ImageChest website was exercised.

ImageChest's [published post API documentation](https://imgchest.com/docs/api/1.0/endpoints/post) and [user API documentation](https://imgchest.com/docs/api/1.0/endpoints/user) describe additional authenticated and newer API behavior. The present preview does not implement bearer-token access, private galleries, authenticated user feeds, deletion/editing, all image variants, complete metadata fields, filename/archive format conventions, all HTML layouts, redirect equivalence, or live differential conformance.

The acceptance contract remains the complete upstream gallery-dl port; this single pinned extractor module is correctly marked **partial** in [port-matrix.json](port-matrix.json).
