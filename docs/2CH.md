# 🧵 2ch guarded thread and board preview

Pinned upstream: [gallery-dl 2ch.py](https://github.com/mikf/gallery-dl/blob/7f226f74108501c8ac3120548370f68f19e6f9b3/gallery_dl/extractor/2ch.py). **Independent partial Rust port, not full compatibility.**

Public `2ch.org`, `2ch.su`, `2ch.life`, and `2ch.hk` thread/board source URLs are matched with strict first-party origin, numeric thread ID and safe board slug validation. The legacy `2ch.hk` alias canonically uses `2ch.org`. A thread `/BOARD/res/ID.html` fetches the corresponding `/BOARD/res/ID.json` source. Original attachments preserve source-ordered posts, original name, file extension, timestamps as numeric source fields, optional numeric post IDs, source-derived HTML-stripped title (up to 50 characters) and optional attachment fields.

Attachment URLs must stay on the exact source origin and reject traversal, credentials, unknown formats and query/fragment redirection. Untrusted JSON metadata cannot replace safe downloader filename/extractor fields. Public boards are deferred thread queues based on `/BOARD/index.json` and bounded numbered JSON pages; flattening a board without queue mode is rejected.

Each JSON response is capped at **2 MiB**, with at most **160 thread media**, **128 board queue children** and **four board pages** with one-second inter-page pacing. Two local HTTP fixtures cover a real thread JSON GET and an index-plus-next-board-page sequence. URL rejection, first-seen deduplication and metadata provenance are tested in [CI 40138163930](https://github.com/sguzman/galerith/actions/runs/40138163930), which passed strict formatting, Clippy, tests and release compilation.

**Outstanding:** exact gallery-dl timestamp/date semantics, full filename/directory/archive templates, complete post/file metadata edge cases, unrestricted public board history and live differential conformance. These results do not establish live 2ch availability.
