# 📨 Pinned upstream message.py compatibility

**Source:** [gallery-dl/extractor/message.py at the pinned commit](https://github.com/mikf/gallery-dl/blob/7f226f74108501c8ac3120548370f68f19e6f9b3/gallery_dl/extractor/message.py).

The upstream module defines exactly **three active message identifiers**: `Message.Directory = 2`, `Message.Url = 3`, and `Message.Queue = 6`. Its other historical identifiers, including Version, Headers, Cookies and Urllist, are commented out and are **not active** in this pin. It defines no source-network URLs, extractors or additional runtime functions.

**Rust equivalent:** `src/message.rs` defines `UpstreamMessageId` with precisely these numeric codes, rejects every unsupported/obsolete identifier and provides `to_upstream_tuple` and `from_upstream_tuple`. The pinned upstream tuples' second members are empty for Directory, download URL for Url and child-extractor URL for Queue. The Rust bridge preserves the complete `Metadata` payload and enforces the empty Directory member. The existing typed `ExtractorMessage` enum retains native JSON object serialization; the numeric tuple mapping is available separately so neither representation is conflated.

**Acceptance proof:** `pinned_upstream_message_identifiers_and_tuple_payloads_round_trip` verifies exact numeric IDs, rejected obsolete IDs, correct empty Directory URL, and bidirectional preservation for all three supported message classes. [Linux CI 40217657098](https://github.com/sguzman/galerith/actions/runs/40217657098) passed formatting, Clippy, tests and release build. This source-only, finite protocol-identifier helper has no live-site differential requirement.

**Scope boundary:** The **message.py source file** is marked complete, **not** the entire event, queue, redirect, downloader, filename or template compatibility work. The distinct broader roadmap task T402 remains at 75%; its deferred resolution and redirect acceptance condition is still open. No other unported module is credited from this mapping.
