# ⏩ Safe selected-media pagination pushdown

Galerith's native **typed message job preview** can push a finite, positive file-index ceiling into selected API extractors. This avoids fetching later public API pages once enough **validated downloadable media** have been collected. It does **not** implement gallery-dl's complete streaming and early-stop behavior.

## Supported previews

| Extractor | Source endpoint | Stop rule | Local verification |
|---|---|---|---|
| Wallhaven | public image/search/upload/collection media API | Valid, unique images reach finite file ceiling | `src/extractor/wallhaven.rs` local HTTP fixtures |
| Gelbooru JSON | public `post` JSON/DAPI | Valid media reach finite ceiling; source-post count controls short-page detection | `src/extractor/gelbooru.rs` local HTTP fixtures |
| Gelbooru v0.2 XML family | public XML/DAPI of five allowlisted hosts | Valid media reach finite ceiling; raw `<post>` count controls short-page detection | `src/extractor/gelbooru_v02.rs` local HTTP fixtures |
| Moebooru | public JSON post/tag/pool/popular feeds | Valid media reach finite ceiling; raw JSON record count controls short-page detection | `src/extractor/moebooru.rs` local HTTP fixtures |
| Danbooru tag searches | public `posts.json` tag/page API | Valid media reach finite ceiling; source array count retains existing short-page threshold | `src/extractor/danbooru.rs` local HTTP fixtures |
| e621/e926/e6ai tag searches | public `posts.json` tags/page API | Valid media reach finite ceiling; the raw post count preserves the existing 300-of-320 short-page threshold | `src/extractor/e621.rs` local HTTP fixtures |

Danbooru and e621-family early stopping is limited to **public tag searches**. Pools, favorite groups, popular feeds, media assets, artist queries and random posts retain their previous resolver behavior. The e621 family includes `e621.net`, `e926.net` and `e6ai.net` (plus the registered public frontend aliases).

The XML family is Rule34, Safebooru, TBIB, Hypnohub and Xbooru as registered by the current preview. Moebooru includes the project's registered Yande.re, Konachan, Sakugabooru and Lolibooru host routes. These are **preview capabilities**, not fully ported gallery-dl extractor modules.

## Rules and exclusions

1. Only a finite selector, such as `--job-file-range 1:12:2`, can produce a conservative upper bound. Mixed or open-ended ranges require the unbounded preview route.
2. The typed dispatcher uses `IndexRange::finite_upper_bound` to derive a safe maximum ordinal; the underlying extractor still validates the requested media.
3. Directory-scope selection can exclude earlier media or change ordinal order. The dispatcher must **disable** finite file pushdown where that could omit later eligible files.
4. A response containing 100 source posts but only one usable media file is **not** a short page solely because it yielded one file. Gelbooru JSON, the XML family, Moebooru and e621-family tag searches count actual API posts separately from accepted media.
5. Finite limits clamp to at least one media item. Selected file messages are still filtered by the typed job after extraction; the producer's ceiling is a safe optimization, not a replacement for selector semantics.
6. Queue depth, child-count, message-count and per-site request policies still apply. This is not a guaranteed global HTTP request budget for every supported website.
7. The default `resolve` path and legacy `--range` retain their preexisting behavior. The pushdown is opt-in through typed job selectors.

## Acceptance boundary

Offline loopback fixtures verify early request termination and continuation past unavailable media. The documented implementation reached green Linux CI (format, Clippy, tests and release build) in [run 40039969843](https://github.com/sguzman/galerith/actions/runs/40039969843) and Danbooru search fixtures in [run 40040566586](https://github.com/sguzman/galerith/actions/runs/40040566586). The e621-family tag-search ceiling has additional loopback tests for full first pages and unavailable files; it does not imply complete upstream conformance.

**Still open:** pushing down file/post/child predicates into all pinned upstream paginators, preserving real grouped post semantics in every extractor, request cancellation during streaming, per-site differential tests, complete filename/metadata handling, and authenticated live-site conformance. No extra extractor module is counted as fully ported on the strength of this optimization.
