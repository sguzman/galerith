# 🎨 Picarto guarded public channel galleries

Pinned upstream: [gallery-dl picarto.py](https://github.com/mikf/gallery-dl/blob/7f226f74108501c8ac3120548370f68f19e6f9b3/gallery_dl/extractor/picarto.py). **Independent partial port; full upstream parity is not yet accepted.**

Public `picarto.tv/USER/gallery[/TITLE]` URLs route to the first-party JSON endpoint `ptvintern.picarto.tv/api/channel-gallery` with pinned filtering and reverse-published sort parameters. Responses are constrained to **2 MiB** and **30 posts per page**, with **4 pages** and **160 media outputs** maximum. Foreign redirects, malformed source paths and unsafe original image names are rejected.

The adapter extracts first-party `images.picarto.tv/gallery/NAME` originals, followed by bounded source `variations`, carrying source post ID, visible title, original date string and variant ID where supplied. It emits typed directory and media URL events and does not substitute guessed IDs or media URLs.

The extractor and registry passed [CI 40157062846](https://github.com/sguzman/galerith/actions/runs/40157062846): Rust formatting, strict Clippy, unit/HTTP tests and release build. Offline tests include ordered first/variation images, hostile names, and real two-page JSON HTTP pagination. This **does not** establish current live Picarto API, CDN or authentication compatibility.

**Outstanding parity:** complete post and variation field overlays, source date type semantics, directory events for posts lacking images, exact filename/archive templates, unrestricted history, authentication and live pinned differential equivalence.
