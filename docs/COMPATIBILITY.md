# 🧩 Preview features versus full gallery-dl compatibility

These features are **implemented subsets**; the full port is still the first MVP.

| Feature | Native preview support | Outstanding parity |
|---|---|---|
| Wallhaven | Public wallpapers, q-only search, user uploads and collection-index flattening with bounded/capped API requests, verified offline (CI 40031267694) | Complete queue-message semantics, advanced search, authentication, collection metadata, filenames/archive and live site equivalence |
| Wikimedia Commons | Public File: pages and article/category image lists through official Action API, approved upload CDN links, guarded pagination and identified requests (CI 40026028078) | Other Wikimedia and MediaWiki hosts, recursive subcategories, full file revisions, rich metadata/license attribution, upstream archive and filename behavior |
| Lexica | Public prompt-image search over bounded JSON POST/cursor API; validated JPEG IDs and deduplication (CI 40023659175) | Full prompt metadata, upstream archive/filename templates, authentication and live behavior |
| Basic direct links | HTTP(S) image/video/doc extensions | Complete directlink selection and MIME handling |
| Danbooru | Single posts and bounded tag searches | Pools, artists, metadata, login, full pagination |
| e621 family | e621/e926/e6ai posts and tags; MD5 URL fallback | Pools, favorites, popular feeds, artist URLs, authorization |
| Gelbooru | Posts and tag search via JSON API | Pools, favorites, redirects, rich metadata, pagination edge cases |
| Gelbooru v0.2 XML | Partial posts/tag-search API for rule34, safebooru.org, tbib, hypnohub and xbooru | Pools, favorites, auth, site fallback URLs, tags, metadata, full paging |
| 4chan / 4channel | Thread attachments and bounded board-to-thread traversal | Upstream directory/filename/metadata, archive edge cases, full queue model |
| `-f / --filename` | `{filename}`, `{stem}`, `{extension}`, `{category}`, `{extractor}`, `{id}`, `{post_id}`, escaped braces and integer ID padding | Full gallery-dl format grammar, transformations, conditions, arbitrary metadata |
| `--range` | 1-based single, inclusive, open-ended and stepped slice expressions | Early stopping and per-message file/post/child range semantics |
| Config | Explicit `-c`, `--config-json`, `--config-yaml`, `--config-toml`, `-o` with nested overlay; filename/base-directory subset | Default discovery, complete option precedence and namespaces, CLI config diagnostics |
| Cookies | Netscape cookies scoped by domain/path/TLS | Browser profile imports, login/token refresh |
| Archive | Native SQLite on successful downloads | gallery-dl archive schema, IDs, migration and cache controls |
| Resume | Range/If-Range validation, interrupted partials and bounded numeric/HTTP-date Retry-After | Full source-specific retry/mtime and rate policies |
| Network & pacing | `--user-agent`, `--proxy`, `--http-timeout`, constant `--sleep` | Advanced TLS/IPv4/IPv6, complete proxy and pacing dialects |
| `--write-metadata` | Atomic `<media-filename>.json` sidecar with redacted source URL, available fields only | Upstream metadata content, modes, event pipeline and filename rules |
| Error output | Removes userinfo/query/fragment and hides request error details | Full structured safe logging and diagnostic policy |

The source-module parity inventory is in [PORT_MATRIX.md](PORT_MATRIX.md). A source module may define multiple extractor classes and URL variants; source-module counts **must not** be confused with site counts. No module may be marked complete solely for having a Rust file or passing a small subset test.

Verification: [Linux CI run 39959270238](https://github.com/sguzman/galerith/actions/runs/39959270238).

No Python runtime dependency. This public repository uses only synthetic test credentials and assets.
