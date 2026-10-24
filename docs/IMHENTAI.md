# 📚 IMHentai-family public gallery preview

Pinned source: [imhentai.py](https://github.com/mikf/gallery-dl/blob/7f226f74108501c8ac3120548370f68f19e6f9b3/gallery_dl/extractor/imhentai.py). This is an **independent partial Rust port**, not full compatibility.

The six supported gallery hostnames are `imhentai.xxx`, `hentaiera.com`, `hentairox.com`, `hentaifox.com`, `hentaienvy.com` and `hentaizap.com` (including `www`). Numeric public `/gallery/ID` and `/view/ID` aliases are canonically fetched as read-only HTTPS `/gallery/ID/` URLs. Host lookalikes, URL credentials, unknown query fields, alternate nondefault ports, encoded traversal and invalid IDs are rejected.

The first source HTML is limited to **2 MiB** of received bytes. Only the pinned embedded `$.parseJSON('...')` media table is accepted in this preview: pages must have contiguous numeric indices, widths/heights in a bounded range and one of five recognized image extensions. This subset emits at most **160 original image files**, using a secure same-site or subdomain media root and deterministic page order. Typed URL/directory messages preserve gallery ID, page number, count, media dimensions, canonical post URL and a bounded optional visible title, without fabricating an individual post ID.

The six-host matcher, ordered media parser, unsafe media/malformed records and one-request local HTTP fixture passed [Linux CI 40126359460](https://github.com/sguzman/galerith/actions/runs/40126359460), including formatter, Clippy, offline tests, roadmap consistency and release build.

**Outstanding:** pinned fallback original-image extension requests for galleries without JSON, tag and search result pagination, rich gallery tags/titles, filename/directory/archive templates, mirror cookies and other CDN variations, and opt-in live differential conformance. Unsupported cases are explicitly unaccepted.
