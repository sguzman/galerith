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
for newline-delimited JSON. The current direct-media adapter emits a
`directory` event followed by a `url` event. It exposes only native fields:
`category`, `extractor`, `filename` and, when known, `post_id`.
It does not infer source-site metadata already lost in a flattened list.

For `https://wallhaven.cc/user/USER/favorites` the same flag instead reads
the public collection index and emits `queue` events for validated unique
collection IDs. They retain `username`, `collection_id`, `category` and
`subcategory`. No child galleries are downloaded or traversed in this mode.
The normal downloader still uses the bounded flattened-media pipeline.

The flag is intentionally distinct from upstream command aliases. It
conflicts with other preview output modes, media ranges, filename overrides,
archive writing and metadata writing. It creates no local files.

## Acceptance boundary

- Existing `MediaItem` CLI/download/archive behavior remains intact.
- The generic `src/extractor/mod.rs` adapter cannot reconstruct original
  post groupings or metadata omitted by upstream site previews.
- Wallhaven collection indexes preserve deferred queue boundaries in
  inspection mode, but flatten child collections in download mode.
- Full gallery-dl message-job control, child extractor instantiation,
  filter ordering, redirects, metadata propagation, queue recursion
  and differential comparison remain unaccepted.
- Queue consumers must never treat deferred child URLs as media downloads.
- JSON-lines output is Galerith's inspection format, **not** byte-for-byte
  compatibility with gallery-dl's `--dump-json`.

Offline regression tests: `src/message.rs`, Wallhaven extractor fixtures,
and `tests/message_cli.rs`. Synthetic tests cannot establish live parity.
