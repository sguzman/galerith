# 🧬 Agn.ph guarded public XML posts and tags

Pinned upstream: [gallery-dl agnph.py](https://github.com/mikf/gallery-dl/blob/7f226f74108501c8ac3120548370f68f19e6f9b3/gallery_dl/extractor/agnph.py). This is an **independent partial Rust port**, not a claim of full booru or upstream source parity.

Registered CLI routes accept first-party `agn.ph/gallery/post/show/NUMERIC_ID` individual posts and public `agn.ph/gallery/post/?search=TAG` feeds with bounded page indices. Host lookalikes, credentials, nondefault ports, malformed/duplicate search or page fields, encoded path traversal and unsupported parameters are rejected.

The post adapter requests the pinned `/gallery/post/show/ID/?api=xml` source; tags use `/gallery/post/?search=TAG&api=xml&page=N`. Responses are limited to **2 MiB** of actual XML bytes and must remain on the request origin. XML is parsed as data with an explicit post ID and first-party HTTPS original media URL; output retains available source status, original created-at timestamp and tag text. Searches use the pinned **45-post page** and XML `offset`/`count` continuation, with a **four-page/160-media** cap, stable first-seen post IDs and one-second page pacing. Typed directory and media URL events are available without pretending to implement all booru metadata.

The direct-post source fixture, malicious route/media checks and 45-then-one two-page local HTTP pagination regression passed [full Linux CI 40147398949](https://github.com/sguzman/galerith/actions/runs/40147398949), including Rust format, Clippy, tests, consistency and release build. No live Agn.ph API or media download parity is established.

**Unaccepted:** upstream age-confirmation cookies, per-post HTML artist/character/species/copyright/general tag grouping, full post field and timestamps, complete booru file variants, precise filename/directory/archive formats, arbitrary user history and live pinned differential behavior.
