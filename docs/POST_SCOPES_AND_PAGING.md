# 🧱 Source post scopes and safe early pagination

Galerith's first MVP remains the **complete gallery-dl port**. These features
are limited, independently implemented Rust previews with synthetic CI evidence.

## 4chan and 4channel: actual post scopes

A native 4chan **thread** URL now has a typed message path that reads the
public thread JSON API and emits one `directory` event per valid source post,
including text-only posts. The metadata includes `source_post_id` (the
original post's `no`), `thread_id` and `board`. Each post with a valid
attachment also emits a corresponding `url` message carrying its validated
filename and media URL.

The current `MediaItem.post_id` remains the media timestamp (`tim`)
rather than the 4chan source-post number. Typed URL messages deliberately
retain the existing `post_id` for downloader/archive compatibility and
add `source_post_id` as a **distinct** field. These are not interchangeable.

This means `--dump-messages --expand-queues --message-directory-range N`
and `--follow-queues --job-directory-range N` can select true post
scope ordinals for supported 4chan thread URLs, including text-only posts.
4chan **board listing** URLs instead expose bounded deferred thread queues.
They have no post IDs until a child thread is resolved; only the thread
adapter emits real post scopes, including text-only posts.

The parser and network seam have local regression tests for the real API
post boundary, text-only posts, invalid attachments, one request per thread,
and unchanged legacy file identity. **No live upstream differential parity
is claimed**, and the overall upstream message contract remains incomplete.

## Source-post ID selection across queued boards

The opt-in `--job-source-post-range RANGE` and read-only
`--message-source-post-range RANGE` select the native numeric
`source_post_id`, never the existing timestamp-based media `post_id`.
A concrete directory/URL with a missing or nonmatching source-post ID is
excluded. File ordinals are counted **after** this scope selection so
`--job-file-range 1` means the first eligible file, not the first file
in a rejected source post.

A 4chan board listing emits queues for threads, before post identities are
known. Queues without a source-post ID are permitted to resolve subject to
the existing depth, child-request and event caps; child post scopes are
then filtered when their IDs are known. A queue explicitly carrying an
invalid or excluded source-post ID does not trigger resolution. An excluded
directory scope also suppresses its child requests.

Source-post selection disables finite file-ceiling pushdown, since rejected
posts could otherwise cause necessary later API pages to be skipped. These
controls remain bounded preview semantics, not upstream-equivalent post
expression filtering or live website parity.


## Additional booru source-post projections

Danbooru regular post/search/collection, e621-family, Gelbooru JSON/XML,
Moebooru and Philomena media records carry numeric API post identifiers.
Their typed adapters expose `source_post_id` alongside the preserved
`post_id`, allowing a source-post selector to apply before a file-range
selector. The special Danbooru media-asset endpoint is excluded: its
independent asset ID cannot be presumed to be a post ID.

This projection cannot recreate absent/text-only post events, native
directory grouping for multiple media, or all upstream metadata. It is
not equivalent to the API-backed 4chan thread post grouping above.

## Wallhaven: bounded page requests for finite file ranges

When a typed file selector has only finitely many possible file ordinals,
Galerith conservatively computes its highest possible one-based ordinal.
For Wallhaven public wallpaper, search, uploads and individual collection
routes, that bound is propagated *into* the native API parser so further
HTTP pages are not requested once enough **valid unique** media has been
found. Untrusted media URLs are still checked against the known CDN host
and validated wallpaper IDs. Global API request budgets, pacing and media
limits remain in place.

For example, `--follow-queues --job-file-range 1 --get-urls`
on a supported Wallhaven search may stop after the first API page instead
of requesting all pages in the `--max-pages` window. It is not a promise
to avoid the initial API page.

**Safety conditions:**
- Open-ended file ranges have no finite bound and do **not** use this shortcut.
- A directory-range selector can discard an arbitrary number of preceding
  files; it disables the optimization to preserve correct selected ordinals.
- Child queue indices and cycle controls remain separate from file indices.
- Finite file-ceiling pushdown also covers selected Gelbooru, Moebooru
  and Danbooru API routes. The exact supported variants and limits are
  maintained in [pagination pushdown](PAGINATION_PUSHDOWN.md). Unsupported
  routes retain their existing eager behavior.
- Because the existing collector is not yet a streaming extractor interface,
  media already returned by the first API page may be parsed even if the
  caller asks for only its first entry. The network saving is **between**
  pages, not within a response.

Local HTTP tests verify that page 2 is not requested after the selected
unique media has been found, and that invalid/duplicate records do not
prematurely stop pagination. `IndexRange` has separate tests for finite,
stepped and open-ended selection bounds.

## Remaining acceptance

Full gallery-dl post directory semantics, source metadata fields, archive
identities, post expression filters, early termination for all site
paginators, shared streaming extraction and live differential parity remain
**open**. A passing synthetic fixture does not count as complete upstream
extractor parity.
