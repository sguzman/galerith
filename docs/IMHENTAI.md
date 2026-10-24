# 📚 IMHentai-family partial Rust port

Pinned source: [gallery-dl imhentai.py](https://github.com/mikf/gallery-dl/blob/7f226f74108501c8ac3120548370f68f19e6f9b3/gallery_dl/extractor/imhentai.py). **Status: partial.** Implementation is independent, and all acceptance cited below is offline/synthetic, not live-source conformance.

## Public galleries

Supported mirror roots: `imhentai.xxx`, `hentaiera.com`, `hentairox.com`, `hentaifox.com`, `hentaienvy.com`, `hentaizap.com`, with `www` aliases. Strict public numeric `/gallery/ID` and `/view/ID` routes canonically read one HTTPS `/gallery/ID/` page, capped at **2 MiB of actual HTML**. Canonical requests reject invalid IDs, credentials, hostile queries, traversal, lookalike domains and nondefault ports.

The upstream embedded `$.parseJSON('...')` media table yields a maximum of **160 ordered original files**, requiring contiguous image indices, validated extension and bounded numeric dimensions. Only matching first-party HTTPS media hosts or subdomains are accepted. Typed directory and URL events carry actual gallery ID, page, count, dimensions, canonical post URL, and optional visible title. The page parser additionally recognizes bounded visible subtitle, category/type, and sorted/unique artist, tag and language entries. Missing fields are not fabricated.

## Public listing queues

The preview matches public artist, character, category, group, language, parody and tag routes, plus the exact `/search/?key=QUERY` and `/advanced-search/?key=QUERY` forms. It extracts literal HTML gallery anchors before ranking/sidebar markup, emits first-seen unique **typed child queue** URLs, and preserves source listing kind/value without including untrusted outside links. Queues are **opt-in**; ordinary flat listing mode is explicitly not a download substitute.

Traversal is limited to **four pages**, **128 discovered children** and **2 MiB per HTML request** with one-second pacing. Same-origin, monotonic page transitions are required. The generic queue-job consumer separately caps how many discovered children it follows. Foreign origins, private and unsupported queries, unexpected redirects and unsafe pagination are rejected.

## Evidence and remaining parity

[CI 40126359460](https://github.com/sguzman/galerith/actions/runs/40126359460) accepted public gallery routing, JSON-media parsing, hostile origin checks and one-request HTTP fixture. [CI 40127448827](https://github.com/sguzman/galerith/actions/runs/40127448827) accepted bounded public tag/search listing and visible gallery metadata fixtures. [CI 40127743477](https://github.com/sguzman/galerith/actions/runs/40127743477) accepted cross-origin redirect rejection with real loopback HTTP redirects, formatting, Clippy, tests and release build.

**Unaccepted:** missing-JSON image fallback requests, complete tag/search query dialects, rich attribution/metadata and dates, complete upstream original variants, cookies/session behavior, filename/archive/directory templates, and live pinned upstream differential equivalence. No part of this source module is claimed complete.
