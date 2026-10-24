# 🖼️ 8muses public album partial port

Pinned source: [gallery-dl 8muses.py](https://github.com/mikf/gallery-dl/blob/7f226f74108501c8ac3120548370f68f19e6f9b3/gallery_dl/extractor/8muses.py). **Independent guarded Rust subset**, not a complete upstream port.

The native dispatcher recognizes public `/comics/album/PATH` paths under canonical `comics.8muses.com` or supported upstream host aliases. Unsafe path segments, encoded traversal, lookalike domains, credentials, nondefault ports, query fields and fragments are rejected before source HTTP. The source page reader caps real streamed HTML at **2 MiB** and rejects foreign-origin redirects after HTTP completion.

The pinned `id="ractive-public" type="text/plain"` payload is decoded with the upstream printable-ASCII rotation (`chr(33 + (ord(c) + 14) % 94)`), then parsed as JSON **without executing scripts**. Source `pictures[].publicUri` hashes produce canonical HTTPS `/image/fl/HASH` originals and ordered numbered JPG filenames. Available album title, ID, source date and views are carried in typed metadata. Public nested album `permalink` entries are emitted as deferred typed queues; private albums, unsafe slugs and duplicate permalinks are skipped. Discovery is limited to **four source pages**, **160 image originals** and **128 child album queues**, with one-second page pacing.

[Linux CI 40141793059](https://github.com/sguzman/galerith/actions/runs/40141793059) passed strict formatting, Clippy, tests, machine-ledger checks and release build. Synthetic fixtures verify reversible encoded JSON decoding, original ordering, private album suppression, deduplication and two actual loopback HTTP page GETs. **Live source markup and CDN media behavior have not been tested.**

**Unaccepted:** complete upstream optional query parameters, parent/album metadata and dates, all source page types and pagination, directory/filename/archive equivalence, authenticated/private collection behavior, live media availability and pinned upstream differential parity.
