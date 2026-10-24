# 🌀 Imagetwist and Imagehaha public previews

This independent Rust implementation is a **partial** adaptation of the pinned Imagetwist extractors within `gallery_dl/extractor/imagehosts.py` (upstream gallery-dl `7f226f74108501c8ac3120548370f68f19e6f9b3`).

The single-file path supports Imagetwist (root, www and phun) and Imagehaha trusted source hosts, 12-character lowercase-alphanumeric image IDs and safe optional filename suffixes. The image source is fetched with a 2 MiB limit on declared and streamed HTML bytes. The original media must have an HTTPS first-party Imagetwist/Imagehaha host, a supported image extension, and no credentials, query tokens or fragments. A `/imgs/` placeholder explicitly reports unavailable content. Safe HTML `alt` filenames are supported; flat media records keep a single URL, and typed events retain source token, category, post URL and filename.

The public gallery subset recognizes `/p/USER/NUMERIC_ID` and `/p/USER/NUMERIC_ID/TITLE`, emits ordered deduplicated first-party image child queues with gallery ID, username and optional title, and checks same-origin, forward-moving `page` links. It enforces four pages, 160 queued children and 2 MiB per HTML response; consecutive pages are paced one second apart. The eager flat-gallery mode remains explicitly unsupported unless queue following is requested.

All results are **synthetic offline acceptance tests**, not verified live upstream parity. Evidence: [Linux CI 40119078981](https://github.com/sguzman/galerith/actions/runs/40119078981), including public source/missing-image fixtures, page-loopback and hostile continuation URLs.

**Outstanding:** upstream root-query `?op=user_public&fld_id=...&page=...` gallery entry forms, every original gallery/layout/host combination, cookies, authentication, full file metadata, filename/archive templates, media response validation and live reference differential tests. The pinned multi-host module remains partial.
