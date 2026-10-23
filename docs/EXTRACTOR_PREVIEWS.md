# 🧩 Current native extractor previews

Every row here represents **partial functionality**, not a fully ported upstream extractor. See the [full pinned module matrix](PORT_MATRIX.md) for all 272 upstream modules.

| Rust extractor | Recognized inputs | Implemented subset | Major unresolved behavior |
|---|---|---|---|
| `directlink` | Direct HTTP(S) URLs ending in selected media extensions | Filename validation and direct streaming | Upstream matching, file policy and full downloader contract |
| `danbooru` | `danbooru.donmai.us` and `safebooru.donmai.us` posts and tags | JSON API post and bounded tag search | Accounts, metadata, original image fallbacks and full site variants |
| `e621` | `e621.net`, `e926.net`, `e6ai.net` posts and tags | JSON post, bounded tag search, selected MD5 media URL recovery | Pools, favorites, popular, auth, metadata and complete formats |
| `gelbooru` | `gelbooru.com` individual post and tag search pages | Public JSON post and bounded tag-search request | Pools, favorites, redirects, auth, filename and metadata semantics |
| `gelbooru_v02` | Five pinned XML/DAPI hostnames and post/tag-search pages | XML post and bounded tag-search requests | Full pool/favorites/auth/metadata and host-specific media handling |
| `moebooru` | `yande.re`, `konachan.com`/`.net`, `sakugabooru.com`, `lolibooru.moe` | JSON single post, bounded tag search and pool posts | Popular feeds, metadata, auth and complete upstream naming |
| `philomena` | `derpibooru.org`, `ponybooru.org`, `furbooru.org` | JSON images, bounded search and galleries | Full auth, filters, tags, metadata and SVG handling |
| `foolfuuka` | Nine pinned 4chan archive host families | Archived thread, bounded board and gallery JSON extraction, deduplication, ordered replies | Search, metadata, redirects, full filename semantics |
| `4chan` | `boards.4chan.org` and `boards.4channel.org` threads and board listings | Thread attachments, board page-to-thread recursion and duplicate thread ID suppression | Upstream directory templates, metadata, edge-case formats and full archival semantics |

`--max-pages` controls current bounded search/board traversal. Every extractor uses synthetic offline fixtures in CI; passing those fixtures is **not** live-site conformance.

**Verified integration:** [CI 39963720920](https://github.com/sguzman/galerith/actions/runs/39963720920) (FoolFuuka gallery and all existing offline tests).
