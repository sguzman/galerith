# 📷 Imgur guarded public preview

**Pinned reference:** [gallery-dl imgur.py](https://github.com/mikf/gallery-dl/blob/7f226f74108501c8ac3120548370f68f19e6f9b3/gallery_dl/extractor/imgur.py).

Galerith independently implements a **partial** native Rust preview of public Imgur single images, albums, and gallery links. The matcher recognizes validated five- or seven-character IDs on approved HTTP(S) Imgur hosts and rejects unsafe origins, credentials, query-bearing input and traversal. Files are selected through the pinned public `post/v1/media/ID` API and public albums through `post/v1/albums/ID`, with an explicit public client ID. The fetched JSON is limited to **2 MiB** even when the response is streamed.

Original media is reconstructed only as `https://i.imgur.com/ID.EXT` after validating the source media ID and supported extension; public JPEG extension is normalized to JPG, and animated GIF is projected to MP4 in this initial preview. Albums are limited to 160 media entries per response and preserve source order and source-backed album/ordinal/count metadata. A public `/gallery/ID` resolves through the pinned `post/v1/posts/ID` classification and emits one deferred image-or-album queue; following queues is opt-in.

The public image request, source routes, media normalization, gallery classification, global dispatcher integration, formatting, Clippy, Rust unit and CLI tests, consistency verification and release build passed [Linux CI 40122108041](https://github.com/sguzman/galerith/actions/runs/40122108041). This evidence uses **synthetic/offline fixtures**, not live-site differential testing.

**Not accepted:** every user, favorites, search, tag, subreddit and authenticated account mode; hidden albums, full pagination; accurate date/title/archive/filename options and media-response validation; configuration parity and opt-in live upstream comparison. This is **not** a complete `imgur.py` port.
