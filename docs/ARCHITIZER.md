# 🏙️ Architizer guarded public projects and firms

Pinned source: [gallery-dl architizer.py](https://github.com/mikf/gallery-dl/blob/7f226f74108501c8ac3120548370f68f19e6f9b3/gallery_dl/extractor/architizer.py). **Independent partial Rust port**, not complete upstream conformance.

The native dispatcher accepts exact `https://architizer.com/projects/SLUG` and `/firms/SLUG` public routes. Host lookalikes, credentials, nondefault ports, traversal and query/fragment parameters are rejected. Public project pages are fetched once from canonical `/projects/SLUG/` with a **2 MiB** actual HTML byte cap and same-origin redirect guard. From source OG `og:image:secure_url` anchors, the preview selects the base image URL before any transformation query, preserving first-seen order and removing duplicates. Media hosts are restricted to Architizer first-party HTTPS domains and a selected `architizer-prod.imgix.net` CDN host, with validated image extensions; unverified external sources are not accepted. Up to **160 originals** are emitted with source-backed title, firm, slug, location and GID where visible, plus a stable ordinal.

Public `/firms/SLUG` feeds request the pinned `?requesting_merlin=pages` source page and emit up to **128 first-seen project child queues** using validated `/projects/SLUG` links. Firm queues are deferred typed `queue` messages, not implicit media downloads; following children is opt-in and remains subject to global queue limits.

[CI 40146768502](https://github.com/sguzman/galerith/actions/runs/40146768502) passed Rust formatting, Clippy, tests, ledger checks and release build. The offline tests cover malicious routes/media, source image metadata, deduped firm queues and a real two-request project plus firm local HTTP fixture. **Live Architizer markup and media CDN access are not verified.**

**Unaccepted:** all rich project metadata, complete image and media CDN variants, exact upstream filename/archive/directory formats, unrestricted firm/project pagination, optional cookies and live differential parity with pinned gallery-dl.
