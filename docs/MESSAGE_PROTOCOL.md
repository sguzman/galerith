# 📨 Typed extractor messages — incremental compatibility foundation

Galerith's independent Rust extractor-event protocol is an **inspection
preview**, not a complete recreation of gallery-dl's extraction jobs.

## Event types

`src/message.rs` defines JSON-serializable messages, each with a `type`
discriminator and a metadata dictionary:

| Type | Fields | Meaning |
|---|---|---|
| `directory` | `metadata` | Start the next media item's directory/metadata scope |
| `url` | `url`, `metadata` | Downloadable media link with metadata |
| `queue` | `url`, `metadata` | Deferred child extractor URL; **not downloaded media** |

All three are distinct types. Queue events do not silently resolve children.
Future extraction jobs need bounded recursion, cycle, authentication,
per-extractor and depth policies.

## Read-only CLI preview

Use `galerith --dump-messages 'https://example.org/image.jpg'`
for newline-delimited JSON. The native direct-media adapter emits a
`directory` event followed by a `url` event, with `category`, `extractor`,
`filename`, `domain`, `subcategory`, URL `path`, filename `stem` and
`extension`, plus optional decoded `query` and `fragment`. These source fields
are derived from the input URL, not inferred from an external API. The regular
`--dump-json` output remains the established flattened `MediaItem` schema.
The generic adapters for other sites do not infer metadata already lost in
a flattened list.

For `https://wallhaven.cc/user/USER/favorites` the same flag instead reads
the public collection index and emits `queue` events for validated unique
collection IDs. They retain `username`, `collection_id`, `category` and
`subcategory`, plus bounded public `label`, `count` and `public` fields if
the collection API supplies them with matching types. Oversized/control-character
labels and arbitrary API properties are not forwarded. Repeated collection IDs
retain the first record. No child galleries are downloaded or traversed in this
mode. The normal downloader still uses the bounded flattened-media pipeline by
default.

Optional `--expand-queues --queue-depth N` follows **one to four levels**
of deferred children in this read-only inspection mode (default depth 1).
It preserves parent `queue` events and inserts child events in depth-first
order. Child URLs are deduplicated across the job using normalized URL identities
(hostname case and fragment aliases do not bypass cycle detection; query
parameters remain significant), the default child cap is
8, the hard cap is 16 (`--max-queue-children N`), the global event cap is
8192, and every child starts after a conservative 1400ms delay. Child
extractor pages are bounded separately to at most four. Unexpanded queues
remain visible as deferred events. Cycles do not recurse forever.

The iterative job carries parent directory/queue context into child messages,
but child-supplied values take precedence. It deliberately strips inherited
`filename`, `extractor`, `post_id` and `source_post_id` at child boundaries so an unrelated
parent media identity cannot contaminate child files.

**Opt-in file jobs:** `galerith --follow-queues 'URL'` resolves queued children
to depth two, with at most eight distinct children, then passes **only** typed
`url` events to the normal downloader/archive/sidecar pipeline. Directory
and queue messages cannot be misinterpreted as downloadable files. Queued child URLs are also checked for HTTP(S), a recognized native
**website extractor** (never the arbitrary direct-file fallback), a public
domain-style hostname rather than an IP address or local hostname, and no
URL credentials **before child extractor dispatch**. Only approved site URL
matchers can be followed. This reduces the attack surface; it is not a
promise of complete DNS-rebinding protection or upstream security parity. Candidate
file URLs must be HTTP(S), must not contain URL credentials, and require a
single safe filename and recognized extractor provenance. An offline CLI
fixture verifies actual HTTP transfer and atomic file creation. This is
not a complete replacement for upstream archive/metadata/job semantics.

For Danbooru post/search/collection results, the e621 family, the
Gelbooru JSON and XML families, Moebooru and Philomena, the typed
projection retains a real numeric `source_post_id` when the native media
record includes its actual post ID. This is **not** full post grouping:
posts without downloadable media are absent from these flattened previews.
An isolated Danbooru media asset is explicitly excluded from this
projection because its asset ID need not be a source post ID. Parent
`post_id` metadata is not silently replaced.

Job-time positive ordinal selectors can filter file URLs, queued child requests
and directory scopes before the corresponding actions. Inspection uses
`--message-file-range`, `--message-child-range` and `--message-directory-range`;
typed downloads use `--job-file-range`, `--job-child-range` and
`--job-directory-range`. See [job filtering](JOB_FILTERS.md) for exact
sequence semantics, supported syntax and upstream limitations.

The flag is intentionally distinct from upstream command aliases. It
conflicts with other preview output modes, media ranges, filename overrides,
archive writing and metadata writing. It creates no local files.

## Acceptance boundary

- Existing `MediaItem` CLI/download/archive behavior remains intact.
- The native 4chan typed **board** adapter now emits bounded deferred thread
  queues, while the native 4chan typed **thread** adapter does preserve source API post scopes
  (including text-only posts). The generic `src/extractor/mod.rs` adapter cannot reconstruct original
  post groupings or metadata omitted by upstream site previews.
- Wallhaven collection indexes preserve deferred queue boundaries in
  inspection mode, and can opt into the new bounded child-download path.
  Regular downloads still use the older flattened path by default.
- The new iterative stack is a **bounded preview**; full gallery-dl
  message-job control, extractor class instantiation, filter ordering,
  redirects, arbitrary recursion, archive/metadata propagation and
  differential comparison remain unaccepted.
- Queue consumers must never treat deferred child URLs as media downloads.
- JSON-lines output is Galerith's inspection format, **not** byte-for-byte
  compatibility with gallery-dl's `--dump-json`.

Offline regression tests: `src/message.rs`, Wallhaven extractor fixtures,
`tests/message_cli.rs`, and `tests/message_download_cli.rs`. Synthetic and
loopback-only tests cannot establish live parity.
