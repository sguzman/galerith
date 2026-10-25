# 📝 Hatena Blog public entry and archive preview

Pinned source: [gallery-dl hatenablog.py](https://github.com/mikf/gallery-dl/blob/7f226f74108501c8ac3120548370f68f19e6f9b3/gallery_dl/extractor/hatenablog.py). **Status: independent partial Rust port**, not a complete source implementation.

The native dispatcher routes first-party public blog URLs on `*.hatenablog.com`, `*.hatenablog.jp`, `*.hatenadiary.com` and `*.hateblo.jp`, including an individual `/entry/PATH`, blog home, date/category archives and public search. Unsupported or private query keys, duplicate page/search fields, encoded/raw traversal, credentials, cross-origin redirects and forged subdomain lookalikes are rejected. Requests use HTTPS and cap each streamed HTML response at **2 MiB**.

Public article pages recognize pinned `hatena-fotolife` originals, validate HTTPS image origins, emit one typed directory per recognized article, and emit ordered source image URLs with available entry path, title, source date and per-article ordinal/count. Empty-image articles retain their directory event. Two separate posts referencing the same image retain independent article metadata. Media deduplication is per article, not across otherwise distinct posts.

Archive listings instead emit validated, first-seen **deferred child entry queues**. Traversal is capped at **four pages**, **128 queue children** and **160 media** (including home/full article listing results), with 500 ms inter-page pacing and cycle checks. In flat mode queue-bearing archive pages are explicitly not silently downloaded as originals.

[CI 40202535138](https://github.com/sguzman/galerith/actions/runs/40202535138) accepted strict Rust format, Clippy, offline tests and release build. The fixtures verify one-request original entries, two-page archive pagination, hostile source/path rejection, repeated originals across posts and image-less article directory cardinality.

**Unaccepted:** upstream `hatenablog:` custom-host alias syntax and custom domains, unrestricted blog pagination, all HTML layouts and Fotolife variants, exact parsed ISO datetime fields, source-specific filename/directory/archive defaults, live blog/CDN downloads and pinned upstream differential parity.

**Date normalization:** Source `<time datetime="...">` publication values are preserved verbatim as `date_source`. Valid RFC3339 timestamps also populate `date`, retaining the source's explicit `Z` or numeric timezone offset and optional fractional seconds. Invalid Gregorian dates or absent/invalid zones do not create a normalized `date`. The leap-year, calendar and timezone regressions passed [CI 40203001697](https://github.com/sguzman/galerith/actions/runs/40203001697).
