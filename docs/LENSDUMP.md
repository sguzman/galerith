# 📷 Lensdump guarded public images, albums and user listings

Pinned reference: [gallery-dl lensdump.py](https://github.com/mikf/gallery-dl/blob/7f226f74108501c8ac3120548370f68f19e6f9b3/gallery_dl/extractor/lensdump.py). **Status: independent partial Rust port**, not upstream full parity.

The normal CLI dispatcher now handles strict first-party `https://lensdump.com/i/ID` public images, `/a/ID` album paths and `/USER` public album feeds. Route identifiers are validated and credentials, lookalike hosts, encoded traversal and unsupported query fields rejected. Image HTML supplies a bounded original `og:image` and source-backed title, dimensions and unparsed date. Albums parse image `data-object` JSON, deduplicate first-seen image IDs and preserve available album/individual title, width, height and original media URL. Permitted media origins are first-party Lensdump and l3n subdomains on HTTPS with supported image extensions.

Source HTML is capped at **2 MiB**, even when streamed. An album is limited to **160 images / 4 pages**; user feeds to **128 child galleries / 4 pages**, with 1-second inter-page pacing. User pages accept only the source-pinned `sort=date_asc&page=N` continuation dialect. Child albums are emitted as typed, deferred queue records; a flat media download is not silently substituted for recursive following. Source redirects and pagination leading off origin are rejected.

[CI 40133703784](https://github.com/sguzman/galerith/actions/runs/40133703784) passed complete Rust formatting, Clippy, tests, ledger validation and release build. Included synthetic media mapping, hostile origin/query cases and real one-image/two-page user loopback HTTP fixtures. **No live Lensdump site or CDN parity has been established.**

**Outstanding:** full upstream album query options and sorting, additional direct CDN image matching, live gallery paging layouts, exact image date/extension, filename/directory/archive templates, all source metadata, cookie and private modes, and live pinned differential conformance.
