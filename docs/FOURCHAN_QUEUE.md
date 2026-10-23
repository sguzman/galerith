# 🧵 Native 4chan board and thread message previews

Galerith implements a **limited, independently authored** Rust message route
for public 4chan and 4channel content. The complete gallery-dl port remains
the MVP; nothing in this document establishes complete upstream parity.

## Board URLs: defer threads instead of downloading them eagerly

For a recognized public board URL such as
`https://boards.4chan.org/g/`, `--dump-messages` now reads the public
`https://a.4cdn.org/g/threads.json` index once and emits a `queue`
event for each selected unique thread:

- Deferred URL: `https://boards.4chan.org/g/thread/123`
- Metadata: `category=4chan`, `board=g` and `thread_id=123`
- Source page selection follows the existing `--max-pages` preview bound
- At most **256 queued threads** are generated; board JSON is capped at
  **2 MiB** including streamed responses, with rejection on oversize bodies
- Duplicate, missing and zero thread IDs are skipped; no child is fetched
  by generating its queue message

`--dump-messages --expand-queues --message-child-range 2` follows the
second eligible child via the standard guarded queue dispatcher. Queue
traversal retains the existing global depth, child count and message caps,
and conservative inter-child delays. `--follow-queues` opts into the
typed child-file download path. The default downloader continues using
its existing eager board-to-thread media preview.

## Thread URLs: real source post boundaries

For `https://boards.4chan.org/g/thread/123`, the typed route reads the
thread JSON and emits a `directory` scope for each valid **source post**
number, including text-only posts; it emits `url` messages only for valid
attachments. Thread JSON bodies are also capped at **4 MiB** before
parsing, including when a response has no declared content length. The original post number is `source_post_id`; the existing
native `MediaItem.post_id` stays tied to the media file's `tim` value
for legacy downloader/archive compatibility.

This source grouping means directory-ordinal filtering can distinguish
real thread posts from downloadable file counts on this route. It is **not**
a generic gallery-dl post predicate.

## Verification and remaining gaps

CI runs [40039969843](https://github.com/sguzman/galerith/actions/runs/40039969843) and [40040148553](https://github.com/sguzman/galerith/actions/runs/40040148553) passed formatting, Clippy, tests and release build.

Offline regressions check deduplicated board queues, first N board pages,
the 256-thread ceiling, the 2 MiB response limit, absence of child requests
during root index discovery, and selective queue-to-thread-to-media
projection. Thread fixtures check text-only post scopes, native file identity
and a single thread API request.

Unsupported upstream features include full board-page semantics, metadata
and filenames, authorization edge cases, post predicates, dynamic redirect
handling, archive equivalence and live reference-differential parity. Other
extractor families retain their existing behavior.
