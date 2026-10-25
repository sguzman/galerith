# 📸 S3ndpics public post, user and search partial port

Pinned upstream: [gallery-dl s3ndpics.py](https://github.com/mikf/gallery-dl/blob/7f226f74108501c8ac3120548370f68f19e6f9b3/gallery_dl/extractor/s3ndpics.py). **This is an independent partial Rust port, not full source parity.**

Supported public routes are `s3nd.pics/post/HEX`, `s3nd.pics/user/NAME`, and a narrowly validated `s3nd.pics/search?q=VALUE` (or `tag`/`tags`) query. The native dispatcher requests the source JSON post API or resolves a public username to its source ID, then reads bounded `/api/posts` listing pages. Anonymous production API calls use a short-timeout no-redirect client. The shared download client is not used for the private API.

Typed directory and URL records carry source post ID, supported created/updated time strings, source user information, media ordinal, type, original path and a source-backed post permalink. Original URLs are accepted only from validated relative file paths and reconstructed on the pinned `https://s3.s3nd.pics/s3nd-pics/` media root. Encoded traversal, query/fragment injection, foreign absolute URLs and unapproved extensions are rejected. No source IDs, timezones or archive IDs are invented.

Per-source JSON is limited to **2 MiB**. Public discovery is bounded to **4 pages**, **128 posts**, **48 posts per page**, **16 files per post**, and **160 emitted files** with 500 ms pacing between listing pages. These are deliberate guardrails, not upstream unlimited parity.

Offline route, source post/file and hostile-path fixtures and a three-request username → two public post pages loopback HTTP fixture passed [full Linux CI 40209522247](https://github.com/sguzman/galerith/actions/runs/40209522247), including formatting, Clippy, tests, release build and the pinned directlink oracle.

**Unaccepted:** the full upstream free-form search dialect, all original media formats, exact post/user source metadata and date types, filename/directory/archive templates, optional authentication and live differential conformance.
