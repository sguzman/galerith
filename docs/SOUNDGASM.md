# 🎧 Soundgasm public audio and user queue preview

Pinned source: [gallery-dl soundgasm.py](https://github.com/mikf/gallery-dl/blob/7f226f74108501c8ac3120548370f68f19e6f9b3/gallery_dl/extractor/soundgasm.py). **Independent partial Rust implementation.**

The native dispatcher recognizes public `soundgasm.net/u/USER/SLUG` and `/user/USER` forms (with `www` and trailing slash), rejecting credentials, forged origins, unsafe slug components, alternate ports and private query fields. Each public source HTML request is capped at **2 MiB** and rejects redirects outside its source origin.

Supported clip pages read visible `aria-label="title"` and `jp-description` text and the pinned `"setMedia", {` audio declaration without executing scripts. The original audio URL is validated as HTTPS on `soundgasm.net` or its real subdomains with known audio extensions, and a safe source-derived filename, title, description, user and slug are emitted as typed directory and URL events.

Public user pages read `sound-details` anchors into first-seen unique **typed child queues**, scoped to that user and capped at **128 tracks**. Following children requires explicit queue traversal; ordinary flat mode does not eagerly download all user tracks.

[Full Linux CI 40155976406](https://github.com/sguzman/galerith/actions/runs/40155976406) passed strict formatting, Clippy, tests and release build. Synthetic tests verify first-party routing and media metadata, spoofed media rejection, duplicate/foreign user links and a real loopback audio GET plus user GET fixture. No live Soundgasm source or CDN conformance has been established.

**Remaining parity:** comprehensive markup and HTML entity decoding, all player source formats, eager user-source iteration semantics, exact filename/archive/directory rules, possible authenticated or restricted content, extensive pagination/transport behaviors and live pinned upstream differential parity.
