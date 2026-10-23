# 🧩 Current native extractor previews

Every row here represents **partial functionality**, not a fully ported upstream extractor. See the [full pinned module matrix](PORT_MATRIX.md) for all 272 upstream modules.

| Rust extractor | Recognized inputs | Implemented subset | Major unresolved behavior |
|---|---|---|---|
| `directlink` | Direct HTTP(S) URLs ending in selected media extensions | Filename validation and direct streaming | Upstream matching, file policy and full downloader contract |
| `bluesky` | `bsky.app`/`www.bsky.app` public posts, profile posts/replies/media/likes, avatar/banner, search/hashtags, feed generators and lists | Public ATProto handle, profile, post-thread, searchPosts, getFeed/getListFeed and getActorLikes APIs; bounded cursor traversal, media dedup, CDN validation, author checks and nested images | Original PDS blobs, videos, quote traversal, authenticated APIs, complete metadata, default upstream likes/PDS semantics and live differential acceptance |
| `catbox` | `catbox.moe`/`www.catbox.moe` albums and `files`/`litter`/`de.catbox.moe` file hosts | Direct file URLs, safe filename validation, bounded public album HTML link extraction and deduplication | Album metadata, full template/archive behavior, HTML variations and live upstream conformance |
| `danbooru` | `danbooru.donmai.us` and `safebooru.donmai.us` posts, tags, random posts, pools, favorite groups and legacy routes | JSON post/search with starting-page selection (200-per-page and short-page exit), ordered collections, random, popular, original media assets and single-artist/artist-search extraction | Accounts, full metadata, alternate formats and complete site parity |
| `e621` | `e621.net`, `e621.cc` and `e621.anthro.fr` frontend aliases, `e926.net`, `e6ai.net` | JSON posts and paginated tag/legacy index search (320-per-page, short-page exit), base-32 posts, popular/favorites, ordered pools, artist searches, MD5 recovery | Auth, full metadata, alternate formats and complete artist workflows |
| `gelbooru` | `gelbooru.com` individual post and tag search pages | Public JSON post and bounded tag-search request | Pools, favorites, redirects, auth, filename and metadata semantics |
| `gelbooru_v02` | Five pinned XML/DAPI hostnames and post/tag-search pages | XML post and bounded tag-search requests, full-page pagination despite missing media | Full pool/favorites/auth/metadata and host-specific media handling |
| `moebooru` | `yande.re`, `konachan.com`/`.net`, `sakugabooru.com`, `lolibooru.moe` | JSON single post, bounded tag search, pool posts and four popular-feed modes | Popular metadata, auth and complete upstream naming |
| `philomena` | `derpibooru.org`, `ponybooru.org`, `furbooru.org` | JSON images, bounded search and galleries, tag slugs, SVG originals | Full auth, filters, metadata and SVG edge cases |
| `foolfuuka` | Nine pinned 4chan archive host families | Archived thread, bounded board, gallery and search JSON extraction, deduplication, ordered replies | Reference search semantics, metadata, redirects, full filename rules |
| `4chan` | `boards.4chan.org` and `boards.4channel.org` threads and board listings | Thread attachments, board page-to-thread recursion and duplicate thread ID suppression | Upstream directory templates, metadata, edge-case formats and full archival semantics |

`--max-pages` controls current bounded search/board traversal. Every extractor uses synthetic offline fixtures in CI; passing those fixtures is **not** live-site conformance.

**Verified integration:** [CI 39995628800](https://github.com/sguzman/galerith/actions/runs/39995628800) (Catbox file/album local fixtures and full Rust format, Clippy, tests, roadmap verification and release build).
