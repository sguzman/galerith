# 🌄 Ahottie guarded public gallery and discovery preview

Pinned reference: [gallery-dl ahottie.py](https://github.com/mikf/gallery-dl/blob/7f226f74108501c8ac3120548370f68f19e6f9b3/gallery_dl/extractor/ahottie.py). **Independent partial Rust port.** This is not a complete upstream compatibility claim.

Strict public `ahottie.top/albums/ID`, `/tags/TAG` and `/search?kw=QUERY` input forms are dispatched through the native CLI. The matcher rejects forged origins, credentials, encoded traversal, unsupported query variants, unexpected fragments and alternate ports. Individual galleries fetch bounded **2 MiB streamed HTML** pages. The image parser preserves source ordering, source-supported image extensions, visible gallery title and original date text. It emits typed directory/URL records and never fabricates unprovided metadata.

Public tag and keyword searches emit *typed, deferred* child-gallery queues with first-seen deduplication, a **four-page** discovery cap, at most **128 children** and one-second pacing; gallery media pages are capped at **four HTML pages** and **160 image originals**. The original gallery traversal rejects source redirects outside its origin and next-page links pointing to a different album, even when the foreign album identifier starts with the current identifier.

Local source tests, hostile origin/album links and bounded gallery/listing loopback HTTP fixtures passed [full CI 40148211788](https://github.com/sguzman/galerith/actions/runs/40148211788), including Rustfmt, strict Clippy, tests, roadmap consistency and release build.

**Unaccepted:** exact upstream gallery tags and normalized date, complete archive/filename/directory metadata, the upstream 2421-byte JPEG placeholder-response rejection, complete public query/page behavior, and live differential validation against pinned gallery-dl. These capabilities must not be counted as complete.
