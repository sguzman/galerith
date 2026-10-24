# 🧩 Pinned imagehosts.py: Pixhost subset

Independent **partial** Rust port of pinned upstream `gallery_dl/extractor/imagehosts.py` at `7f226f74108501c8ac3120548370f68f19e6f9b3`. The upstream module contains many unrelated site extractors: Pixhost is only the first supported host family, not a complete module port.

Public `pixhost.to` and `pixhost.org` single-file `/show/DIRECTORY/TOKEN_NAME.EXT` URLs resolve first-party original images from one bounded HTML page. Public `/gallery/ID` URLs emit up to 96 deferred child queues from the first page only. Image and gallery source URLs, media origins, file extensions and child href attributes are strictly validated. Relative links are resolved only against a fixed Pixhost origin. Public `pixhostads=1` and `pixhosttest=1` cookies are sent to public pages. Scripts, `data-href`, external hosts and query-bearing children are not queued.

Each HTML response has a strict **2 MiB** body limit, and gallery children are deduplicated in source order. Typed file events preserve token and directory metadata; gallery queues also preserve the parent gallery ID. Flat file downloads retain safe file URLs and names.

[Linux CI 40055130109](https://github.com/sguzman/galerith/actions/runs/40055130109) passed formatting, Clippy, tests, roadmap/module consistency and release build. Tests are synthetic/offline, not live differential evidence.

Outstanding: all other pinned `imagehosts.py` extractors, full gallery pagination, POST sites, cookies/encoding variants, rich metadata, filename/archive rules and proven live site parity. Module status remains **partial**.
