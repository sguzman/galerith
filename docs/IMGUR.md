# 📷 Imgur guarded public preview

**Pinned reference:** [gallery-dl imgur.py](https://github.com/mikf/gallery-dl/blob/7f226f74108501c8ac3120548370f68f19e6f9b3/gallery_dl/extractor/imgur.py).

Galerith independently implements a **partial** native Rust preview of public Imgur single images, albums, gallery links and selected public listing feeds. The matcher recognizes validated five- or seven-character IDs on approved HTTP(S) Imgur hosts and rejects unsafe origins, credentials, unauthorized query fields and traversal. Files are selected through the pinned public `post/v1/media/ID` API and public albums through `post/v1/albums/ID`, with an explicit public client ID. The fetched JSON is limited to **2 MiB** even when the response is streamed.

Original media is reconstructed only as `https://i.imgur.com/ID.EXT` after validating the source media ID and supported extension; public JPEG extension is normalized to JPG, and animated GIF is projected to MP4 in this initial preview. Albums are limited to 160 media entries per response and preserve source order and source-backed album/ordinal/count metadata. A public `/gallery/ID` resolves through the pinned `post/v1/posts/ID` classification and emits one deferred image-or-album queue; following queues is opt-in.

The public image request, source routes, media normalization, gallery classification, global dispatcher integration, formatting, Clippy, Rust unit and CLI tests, consistency verification and release build passed [Linux CI 40122108041](https://github.com/sguzman/galerith/actions/runs/40122108041). This evidence uses **synthetic/offline fixtures**, not live-site differential testing.

**Not accepted:** authenticated/private accounts and all their modes, hidden albums, unrestricted listing pagination, complete per-image/album metadata, full archive/filename templates, response validation, configuration parity and live differential parity. This is **not** a complete `imgur.py` port.

## Public listing queues

The independent listing adapter recognizes public `/user/NAME` posts/submissions, `/user/NAME/favorites`, `/r/SUBREDDIT`, `/t/TAG` and exact `/search?q=QUERY` forms. It uses the pinned public Imgur API endpoint families and **only validated five- or seven-character image/album IDs** to produce deferred first-party child URLs. It ignores duplicate IDs and records actual source page, listing type/value and media type in typed child queue metadata. Account `me` routes and extra/duplicated search parameters are explicitly excluded.

To bound public API traffic, list discovery is limited to **four pages**, **160 deferred child messages total**, **2 MiB JSON per request**, and one-second spacing between pages. Empty API pages stop pagination. The generic Galerith job traverser imposes an additional independent cap of **16 followed child queues per job**; the larger Imgur discovery set is not a promise that all children are downloaded in one run.

The strict source and search route test, deduplicated/ordered queue fixture, and three-page loopback API regression all passed [Linux CI 40122975656](https://github.com/sguzman/galerith/actions/runs/40122975656). No live Imgur account or search availability has been verified.

**Still outstanding:** account authentication, private/hidden albums, unrestricted upstream paging and source query variants, rich metadata, configuration-dependent output names/archive behavior, redirects and live reference differential acceptance. The pinned `imgur.py` module remains **partial**.
