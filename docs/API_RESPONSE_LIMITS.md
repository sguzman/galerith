# 🛡️ Bounded public JSON API responses

Galerith uses a shared Rust response reader in `src/extractor/http_json.rs`
for selected public API extractor previews. It reads response bodies in
bounded chunks and parses JSON only after enforcing the configured cap.

| Native extractor family | Maximum bytes per JSON response |
|---|---:|
| Danbooru | 8 MiB |
| e621/e926/e6ai | 8 MiB |
| Gelbooru JSON | 8 MiB |
| Moebooru JSON | 8 MiB |
| Philomena | 4 MiB |

The guard rejects a declared `Content-Length` above the limit without
reading the response body. It also checks actual streamed bytes when the
header is missing, incorrect or the server uses chunked transfer encoding.
HTTP failure statuses and malformed JSON are errors. Offline loopback tests
cover small valid payloads, oversized headers, oversized streamed bodies
and malformed JSON, along with existing extractor API fixtures.

These are Galerith's independent safety limits, **not** claims about
gallery-dl's upstream limits or a complete HTTP security boundary.
They do not impose a global byte budget across pages or nested queues, and
they do not replace host checks, redirect policy, rate limiting, response
type validation or authenticated API semantics.

**Verified:** [Linux CI 40045987953](https://github.com/sguzman/galerith/actions/runs/40045987953)
for the shared reader and e621/Philomena, and
[Linux CI 40046320687](https://github.com/sguzman/galerith/actions/runs/40046320687)
for Danbooru/Gelbooru/Moebooru integration.

The remaining upstream extractor compatibility work is tracked in
[the roadmap](ROADMAP.md), not counted complete by these response guards.
