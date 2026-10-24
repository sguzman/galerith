# 🖼️ ImageShack public REST previews

The ImageShack implementation is an **independently authored, partial Rust port** of pinned `gallery_dl/extractor/imageshack.py` (gallery-dl commit `7f226f74108501c8ac3120548370f68f19e6f9b3`). It is not live upstream parity.

## Supported public routes

- `https://imageshack.com/i/ID`: resolve one public image via a bounded `/rest_api/v2/images/ID` JSON read; accept only validated ImageShack/imagizer HTTPS media with safe image extensions. The source ID is alphanumeric, never fabricated into a numeric post ID.
- `https://imageshack.com/a/ID`, `/a/ID/1`: public album API listing, bounded and deduplicated; typed events carry an album ID and optional safe gallery title.
- `https://imageshack.com/user/USERNAME`: public user image index. Dotted usernames such as `Ezekiel.Gonzalez` are accepted while credentials, query tokens and foreign origins are not.

The REST client requests a bounded public JSON resource without an embedded API key. Each JSON body is capped at **2 MiB**, and listings at **40** rows/page, **4** pages and **160** media records. Between successive listing pages, requests are paced by one second. Individual records must have valid identifiers and acceptable HTTPS `imagizer.imageshack.com` or other explicitly image-hosted `*.imageshack.com` assets; private, malformed or duplicate media are omitted.

The accepted typed metadata subset is `file_id`, optional image `title`, `username`, `creation_date` Unix timestamp, `width` and `height`, and selected album/profile scalar context. Flat downloader records remain simple safe `MediaItem` values. Filenames prefer a safe original name when provided, otherwise the CDN filename.

## Verification and remaining work

[Linux CI 40054227077](https://github.com/sguzman/galerith/actions/runs/40054227077) validates public one-image API resolution, host filtering, ID matching and source metadata. [Linux CI 40054413824](https://github.com/sguzman/galerith/actions/runs/40054413824) includes safe public album/user listing cases and a local two-page 40+1 image API fixture. All upstream behavioral assertions remain synthetic or source-derived; this is **not** a live-site differential acceptance claim.

Missing: authenticated/private records, arbitrary account/album routing and offsets, full upstream metadata and naming/archive semantics, rich album and user settings, optional image formats, complete API error and redirect behavior, and verified live differential conformance. The full upstream gallery-dl port remains the MVP acceptance gate.
