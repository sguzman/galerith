# 📚 Keenspot guarded public webcomics

Pinned upstream source: [gallery-dl keenspot.py](https://github.com/mikf/gallery-dl/blob/7f226f74108501c8ac3120548370f68f19e6f9b3/gallery_dl/extractor/keenspot.py). **Status: partial independent Rust implementation.**

This subset recognizes public `COMIC.keenspot.com` subdomains, excluding `www` and `forums`, and optionally a single explicit public comic page. Source roots are fetched with bounded **2 MiB** HTML. Standard `<link rel="first">` and `<link rel="next">` comic navigation is followed within the exact source origin, up to **32 pages** and **128 images**, with **250 ms** pacing between page requests and visited-page/media deduplication. Invalid hosts, encoded traversal, query/fragment parameters, credentials, unsafe page redirects and foreign next-link origins are rejected.

Images are selected from supported pinned `class="ksc"` originals, excluding script references and requiring a trusted HTTPS `keenspot.com` or subdomain media endpoint with supported image extension and safely decoded filename. Directory and URL events preserve source comic, page, media ordinal and actual media URL. No nonexistent post, date or archive identifiers are fabricated.

Strict route/source image tests, hostile-link validation and a real local three-request root → first comic → second comic loopback fixture passed [Linux CI 40132748097](https://github.com/sguzman/galerith/actions/runs/40132748097), including format, Clippy, tests and release build.

**Unaccepted:** the source's special `brawlinthefamily`, `porcelain`, `lastblood`, `twokinds`, `flipside` and other archive/navigation variants; YouTube/media handoff; complete filename/archive conventions; production CDN/page compatibility and live pinned differential parity. The extracted standard layout is not a complete upstream module port.
