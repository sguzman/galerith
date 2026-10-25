# 🖼️ Fapachi public post and user listing preview

Pinned reference: [gallery-dl fapachi.py](https://github.com/mikf/gallery-dl/blob/7f226f74108501c8ac3120548370f68f19e6f9b3/gallery_dl/extractor/fapachi.py). **Independent partial Rust port.**

Public `fapachi.com/USER/media/NUMERIC_ID` posts and `/USER`, `/USER/page/N` listings are recognized on the exact first-party and `www` hosts. Usernames, post IDs, page numbers and source URLs are validated; credentials, unsafe query parameters, alternate ports, traversal and lookalike origins are rejected.

A public post is read under a **2 MiB HTML** bound, with the original image/video path extracted from the pinned `class="media-img"` and `data-src` selectors. Media is limited to safe first-party HTTPS paths with accepted extensions, preserving the source user, numeric post ID and canonical post URL. Flat post mode emits the original media item.

User feed discovery emits deferred, typed child post queues, limited to **four pages** and **128 children** with one-second pacing. Each link and next-page continuation is parsed and checked for the exact source user and first-party origin. Following children is opt-in; ordinary flat user download does not silently recurse.

[CI 40212411120](https://github.com/sguzman/galerith/actions/runs/40212411120) passed formatting, strict Clippy, local post and two-page HTTP regression, complete tests, pinned directlink comparison and release build. The two-page regression detected a Next-page parser issue that was corrected before acceptance. The synthetic fixtures do **not** confirm live site compatibility.

**Unaccepted:** complete upstream naming/archive/directory templates, remaining source metadata, unlimited user history, variants and live differential conformance.
