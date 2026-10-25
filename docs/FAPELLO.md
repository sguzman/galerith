# 📸 Fapello public originals and discovery: partial port

Pinned source: [gallery-dl fapello.py](https://github.com/mikf/gallery-dl/blob/7f226f74108501c8ac3120548370f68f19e6f9b3/gallery_dl/extractor/fapello.py). This independent Rust implementation covers a **bounded public subset**, not full pinned upstream compatibility.

Recognized roots are `fapello.com` and `fapello.su`, including `www` aliases. Public `/MODEL/ID` posts, `/MODEL` listings and the pinned `top-likes`, `top-followers`, `videos`, `trending` and `popular_videos/FILTER` paths are routed. The matcher rejects forged domains, credentials, query or fragment fields, encoded traversal and invalid IDs. Trailing-slash ranked path matching is tested to avoid misrouting such paths as model names.

Public original media is read from the pinned source `uk-align-center` region, using actual post model/numeric ID, source photo/video classification, HTTPS trusted media URL, extension and optional video poster. Before parsing an original media URL, raw traversal and encoded escape sequences are rejected so URL normalization cannot silently erase them. The pinned rendition markers `.md` and `.th` are stripped from source URL variants.

Model/path discovery emits canonical typed deferred URLs with exact model/path queue constraints and first-seen deduplication. Listing source HTML is capped at **2 MiB**; discovery is restricted to **4 pages** and **128 child queues**, with one-second pacing. No recursive child media is downloaded unless the separate queue-following mode is explicitly selected.

[Full Linux CI 40216126190](https://github.com/sguzman/galerith/actions/runs/40216126190) passed formatting, Clippy, extractor and integration tests, roadmap consistency, release compilation and the separate pinned directlink oracle. Test fixtures include public originals, ranked path routing, unsafe raw-media traversal, first-party model/list queues and a bounded three-request post/two-AJAX-page loopback HTTP sequence.

**Remaining:** exact alternative source HTML layouts, metadata/date/source variation, filename/directory/archive template semantics, unlimited source histories, authenticated modes and live pinned differential source compatibility. No live Fapello access is claimed.
