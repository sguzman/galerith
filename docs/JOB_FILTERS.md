# 🔎 Typed message job selectors

These are **native Galerith preview controls**, not full gallery-dl post/file/child filtering parity. They apply to the opt-in typed event job, not the legacy downloader by default.

## Typed download job

Run `galerith --follow-queues --job-file-range 1:6:2 --get-urls URL` to emit the first, third and fifth eligible file URLs. Use `--job-child-range 2-4` to follow only the second through fourth encountered deferred child queues. Use `--job-directory-range 2` to process only the second directory scope. All ranges use the existing one-based, positive, comma/range/slice grammar. The typed job still enforces two queue levels, at most eight expanded children, and the global event limit.

These controls operate **before** the selected file events become native downloader candidates. Child selection operates **before** the associated HTTP requests, so excluded child extractors are not contacted. Directory selection suppresses events and child requests belonging to excluded directory scopes.

## Read-only inspection

Use `galerith --dump-messages --expand-queues --message-file-range 2 --message-child-range 1-3 --message-directory-range 1 URL` to inspect the same filtering decisions as typed JSON lines without saving files. Add `--queue-depth 1` through `--queue-depth 4` for inspection-only depth. Both modes preserve deferred queue events when a queue is excluded by child index, depth, cycle detection or the resource cap, unless the containing directory scope is excluded.

## Exact scope and limitations

- **Directory** here means a typed `Directory` message. Some current preview adapters emit one directory per media item; this is not a reliable upstream post boundary.
- Directory indices count actual emitted directory messages globally for each root input, including visited child frames. Excluding a directory drops all subsequent URL and queue messages in that frame until the next directory; it does not fetch skipped queue children.
- File indices count eligible URL messages globally in depth-first order, after directory selection. Child indices count eligible queue messages globally, also after directory selection, **before** duplicate/cycle suppression. Excluded file messages cannot trigger a native download.
- These selectors do not enforce early HTTP pagination termination for already-resolving parent APIs. They do not implement gallery-dl expression predicates, complete post-range semantics, streaming job execution, redirect policies, or metadata-formatting parity.
- `--range` remains the legacy post-resolution media-index selector. It is not interchangeable with `--job-file-range`, and mixing the two is rejected.
- Selection is opt-in, with mode checks and validation for malformed/zero/descending ranges. The default extraction, archive, download and filename behavior is unchanged.

Implementation: `src/message.rs`, `src/extractor/mod.rs`, `src/range.rs` and `src/main.rs`. Synthetic unit and CLI fixtures exercise ordering, excluded child requests, directory scope, resource limits and error conditions. No live website differential acceptance is claimed.
