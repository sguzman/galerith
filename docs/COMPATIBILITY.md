# 🧩 Preview features versus full gallery-dl compatibility

These features are **implemented subsets**; the full port is still the first MVP.

| Feature | Native preview support | Outstanding parity |
|---|---|---|
| Wallhaven | Public wallpapers, q-only search, user uploads and collection-index flattening or deferred typed queues with bounded label/count/public metadata, capped API requests and finite typed-file early-stop when safe (CI 40039081132) | Complete queue-message semantics, advanced search, authentication, complete collection metadata, filenames/archive and live site equivalence |
| Wikimedia Commons | Public File: pages and article/category image lists through official Action API, approved upload CDN links, guarded pagination and identified requests (CI 40026028078) | Other Wikimedia and MediaWiki hosts, recursive subcategories, full file revisions, rich metadata/license attribution, upstream archive and filename behavior |
| Lexica | Public prompt-image search over bounded JSON POST/cursor API; validated JPEG IDs and deduplication (CI 40023659175) | Full prompt metadata, upstream archive/filename templates, authentication and live behavior |
| Typed messages | JSON-lines directory/URL/queue inspection; native 4chan API post scopes and actual numeric booru post-ID projections; bounded 1–4-level traversal; typed file/child/directory filters; finite file-range pagination stop in Wallhaven, Gelbooru JSON/XML, Moebooru, Danbooru tags, e621-family tag/popular/favorites feeds and Philomena search/gallery results (CI 40042896536, 40042877773) | Complete post/queue jobs, predicates, early termination for all sources, source metadata, archive, redirects and live differential conformance |
| Basic direct links | HTTP(S) image/video/doc/archive extensions and typed URL-origin metadata (CI 40043179773) | Complete directlink selection and MIME handling |
| ImageChest | Public gallery files and typed profile-to-gallery queues, validated CDN links, bounded HTTP response/pagination and typed gallery metadata (CI 40047578834) | Authenticated API, full gallery metadata/filenames, user-page parity and live upstream differential conformance |
| ImagePond | Public OG single image/video pages and bounded read-only album/user child queues, pinned JS-fallback links, bare usernames and sanitized album headings with strict first-party anchor/host rules, safe media, typed OG dimensions/MIME/thumbnail events and two-page offline fixtures (CI 40051989239, 40052501580) | Complete site aliases/layouts, CDN variants, rich video/album metadata, archive/filename formats and live differential parity |
| ImageShack | Public single images plus bounded user and album REST listings with safe media IDs/CDN URLs, typed metadata, 40-item pages and local HTTP tests (CI 40054227077, 40054413824) | Authentication, private albums, rich metadata, full archive/filename policy, advanced pagination and live differential acceptance |
| Pixhost (`imagehosts.py`) | Public images and bounded first-page galleries, strict origin/cookies and typed queues (CI 40055130109) | Other host families, gallery pagination, filename/archive parity and live differential conformance |
| Turboimagehost (`imagehosts.py`) | Public single image pages and guarded typed album child queues, bounded/paced with local regression (CI 40058313952) | Full album pages, metadata, archive/file naming and live parity |
| Acidimg/Vipr/Imgspice (`imagehosts.py`) | Pinned public image routes, Acidimg form POST, safe host-limited media and 2 MiB HTML guard with synthetic tests (CI 40057318396) | Rich metadata, all hosts/options, archive naming and live differential conformance |
| Imgadult (`imagehosts.py`) | Public single originals, pinned site cookie, safe alternate filename and strict bounded HTML (CI 40058711190) | Full upstream metadata/archive and live differential acceptance |
| Silverpic (`imagehosts.py`) | Public source-matched `/img/` originals, typed dimensions/source identity, bounded HTML and offline fixture (CI 40113694094) | Complete metadata, naming/archive templates and live pinned upstream parity |
| Imgpv (`imagehosts.py`) | Public image-preview originals and bounded optional user/date text (CI 40114488724) | Upstream date object, metadata/filename/archive fidelity and live site acceptance |
| Fappic (`imagehosts.py`) | Public source and thumbnail rewrite into a guarded original image with safe alt and one-request HTML fixture (CI 40115128753) | Complete site source/filename/archive variants and live differential conformance |
| Imgdrive/Imgtaxi/Imgwallet (`imagehosts.py`) | Public OG images, safe source title and `/small/` to `/big/` original projection with bounded HTML (CI 40057685467) | Full metadata/archive behavior and live differential acceptance |
| Postimages (`imagehosts.py`) | Public files and bounded gallery JSON child queues with safe CDN links, deduplication and loopback fixtures (CI 40055715703) | Complete imagehosts family and Postimages metadata, naming, pagination and live reference parity |
| Imxto (`imagehosts.py`) | Public POST images and bounded album child queue pages with safe first-party source/media URLs and offline tests (CI 40056127227) | Complete gallery pagination/metadata, filename/archive policy and live reference parity |
| Picstate (`imagehosts.py`) | Public source-matched original images, bounded HTML and offline HTTP fixture (CI 40056481549) | Full original naming/archive semantics and live upstream differential acceptance |
| Danbooru | Public posts, tags, random, popular, media assets, pools, favorite groups, artist queries and bounded JSON responses (CI 40046320687) | Complete metadata, alternative files, auth, upstream archive/directory formatting and live differential parity |
| e621 family | e621/e926/e6ai post/tag/pool/artist/popular/favorites previews; MD5 fallback, selected-media early stopping and 8 MiB JSON responses (CI 40042896536, 40045987953) | Complete pool/artist/media ordering, auth, metadata/filename/archive semantics and live differential parity |
| Gelbooru | Public post and tag API previews with finite selected-media stopping and bounded JSON responses (CI 40046320687) | Pools, favorites, redirects, authentication, metadata/filenames and live site parity |
| Moebooru | Public post/tag/pool/popular JSON routes with bounded response bodies and finite file-range optimization (CI 40046320687) | Complete metadata, filename, auth, popular and pool upstream parity |
| Philomena | Public post/search/gallery and SVG previews, finite media page stops and 4 MiB JSON response cap (CI 40042877773, 40045987953) | Full filter, metadata, auth, filename, archive and live differential parity |
| Gelbooru v0.2 XML | Partial posts/tag-search API for rule34, safebooru.org, tbib, hypnohub and xbooru | Pools, favorites, auth, site fallback URLs, tags, metadata, full paging |
| 4chan / 4channel | Public thread attachments, typed source-backed post scopes, 256-entry deferred board-to-thread queues, 2 MiB board and 4 MiB thread JSON guards, bounded child selection (CI 40040148553) | Full upstream metadata/directory naming, archive behavior, live board/queue variants, post predicates, auth and reference parity |
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
