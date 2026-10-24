# 📖 Issuu guarded public preview

Pinned source: [gallery-dl issuu.py](https://github.com/mikf/gallery-dl/blob/7f226f74108501c8ac3120548370f68f19e6f9b3/gallery_dl/extractor/issuu.py). Independent Rust implementation, **partial** and restricted to public document pages and public publisher discovery.

The publication matcher recognizes first-party `issuu.com/USER/docs/SLUG` forms (including `www` and trailing slash), rejecting forged hosts, credentials, query/fragment parameters, traversal, unsafe publication slugs and nondefault ports. Bounded **2 MiB** HTML hydration is parsed as data, not executed. A validated public `initialDocumentData.document` record must provide `revisionId`, `publicationId` and positive `pageCount`, with URL-user consistency checked where the source names an owner. Output is capped at **240 pages** and constructs exact first-party CDN JPEG URLs using the pinned `image.isu.pub/REVISION-PUBLICATION/jpg/page_N.jpg` pattern.

Typed directory and URL events preserve available source title, original publish date, user, canonical document page URL, revision/publication IDs, ordinal and count; they do not invent an individual post ID or claim to implement upstream filename/archive formats.

Public `issuu.com/USER` and `/USER/PAGE` publisher URLs emit deferred child publication queues, with exact user/slug validation, first-seen deduplication, **4-page/144-child** discovery cap, 48 entries per source page and one-second inter-page pacing. Source page redirects outside their origin are rejected; user lists do not trigger unbounded eager publication downloads. Following children requires opt-in queue traversal.

Routes, hydration mapping, page count/owner rejection, public HTTP requests and bounded publisher queues passed full [Linux CI 40129349191](https://github.com/sguzman/galerith/actions/runs/40129349191). This verifies synthetic, offline fixtures only; live Issuu markup and CDN behavior are not yet validated.

**Remaining parity:** all additional hydration variants beyond the tested escaped shapes, unrestricted pagination, richer publication metadata, filename/directory/archive templates, media-response verification, optional authentication and live differential conformance. No claim of complete upstream module support.

**Escaped source hydration acceptance:** Source-script wrappers with escaped publication state and publisher `docs` arrays now have dedicated offline regression fixtures in [CI 40129891194](https://github.com/sguzman/galerith/actions/runs/40129891194). This does not verify the live site's current markup.
