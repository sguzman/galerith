# 🖼️ Pictoa public images and album discovery

Pinned source: [gallery-dl pictoa.py](https://github.com/mikf/gallery-dl/blob/7f226f74108501c8ac3120548370f68f19e6f9b3/gallery_dl/extractor/pictoa.py). **Independent guarded partial port.**

The matcher accepts first-party Pictoa single-image and album routes, including source album title prefixes such as `albums/NAME-12345/678.html` and `albums/NAME-12345.html`. It resolves canonical public `https://pictoa.com/albums/12345/678.html` images from source `og:title` and `og:image` fields with validated album/image IDs, title and trusted first-party HTTPS originals. A single-image request emits one typed directory and original URL event.

Album discovery fetches source `h1` and supported category tags, extracts literal nofollow image links within the main album content, and emits first-seen unique **typed child queues**. It does not eagerly download child media without queue-following. Discovery is limited to **4 pages**, **128 children**, **2 MiB HTML per page** and **one-second pacing**; foreign-origin continuations, duplicate images and nonmonotonic `?page=N` links are rejected.

[CI 40198045711](https://github.com/sguzman/galerith/actions/runs/40198045711) passed format, Clippy, offline source fixtures, roadmap checks and release build. Tests include OG original mapping, off-origin media rejection, album tag/child queues and real local one-image and two-album-page HTTP requests. These are not evidence of live Pictoa site or image CDN compatibility.

**Outstanding parity:** all pinned host/HTML variants, unrestricted album pagination and image count, exact filename/directory/archive conventions, full tag/metadata decoding, media response behavior, and live upstream differential testing.
