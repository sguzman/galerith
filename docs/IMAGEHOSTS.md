# 🧩 Pinned imagehosts.py: Pixhost subset

Independent **partial** Rust port of pinned upstream `gallery_dl/extractor/imagehosts.py` at `7f226f74108501c8ac3120548370f68f19e6f9b3`. The upstream module contains many unrelated site extractors: Pixhost is only the first supported host family, not a complete module port.

Public `pixhost.to` and `pixhost.org` single-file `/show/DIRECTORY/TOKEN_NAME.EXT` URLs resolve first-party original images from one bounded HTML page. Public `/gallery/ID` URLs emit up to 96 deferred child queues from the first page only. Image and gallery source URLs, media origins, file extensions and child href attributes are strictly validated. Relative links are resolved only against a fixed Pixhost origin. Public `pixhostads=1` and `pixhosttest=1` cookies are sent to public pages. Scripts, `data-href`, external hosts and query-bearing children are not queued.

Each HTML response has a strict **2 MiB** body limit, and gallery children are deduplicated in source order. Typed file events preserve token and directory metadata; gallery queues also preserve the parent gallery ID. Flat file downloads retain safe file URLs and names.

[Linux CI 40055130109](https://github.com/sguzman/galerith/actions/runs/40055130109) passed formatting, Clippy, tests, roadmap/module consistency and release build. Tests are synthetic/offline, not live differential evidence.

Outstanding: all other pinned `imagehosts.py` extractors, full gallery pagination, POST sites, cookies/encoding variants, rich metadata, filename/archive rules and proven live site parity. Module status remains **partial**.


## Postimages public file and bounded gallery subset

Added `postimg.cc/ID`, `postimages.org/ID`, `pixxxels.cc/ID` and the corresponding public `/gallery/ID` aliases (including recognized `/image/ID` file routes). Individual public file pages read one 2 MiB-bounded HTML response and require a download anchor with a trusted `https://i.postimg.cc/` media URL, optionally with its known public `?dl=1` flag. Image naming uses bounded and sanitized visible alt text when present, falling back to the safe source filename. File typed messages retain the canonical token and post URL.

Public galleries use up to four read-only JSON `/json?action=list&page=N&album=ID` requests, limited to 2 MiB each and 128 unique queued image tokens overall, with one-second pacing and original order preserved. Queue metadata can include bounded width, height and filename hints. Foreign and malformed tokens are discarded, and URL normalization of dot segments cannot reclassify malformed galleries as files.

Verified with [CI 40055715703](https://github.com/sguzman/galerith/actions/runs/40055715703). The full upstream module and complete Postimages gallery title, metadata, filename/archive, aliases and live-site parity are still unaccepted.


## Imxto public images and guarded gallery queues

The pinned `imx.to/i/ID`, `imx.to/img-ID.html` and `img.yt/img-ID.html` public image shapes are normalized to safe first-party `imx.to` requests. The image form uses a read-only `POST` with the upstream public continue parameter `imgContinue`, reads a 2 MiB-bounded HTML page, and selects trusted `https://image.imx.to/` originals, preserving the token and safe original or fallback filename.

Public `imx.to/g/ID` galleries traverse up to four bounded pages and emit up to 128 distinct deferred file queues. Links must be actual first-party anchor attributes; page fragments, encoded path separators, foreign-host URLs and script-only strings are excluded. Pages are read with one-second pacing and deduplication.

[CI 40056127227](https://github.com/sguzman/galerith/actions/runs/40056127227) passed formatting, Clippy, tests, release build and acceptance consistency. This is synthetic offline evidence, not live parity. Imxto full sizes, dimensions, hashes, gallery title, filename/archive and complete upstream behavior remain unaccepted.


## Picstate public single-image subset

Pinned `picstate.com/view/full/ID` public pages now resolve a validated original media URL under the matching `picstate.com/file/ID/` or `/files/ID/` path. Only recognized public image extensions and source-matched IDs are accepted. Bounded, sanitized alternate image naming is used when present; typed events preserve the public post URL and alphanumeric token. One page request is limited to **2 MiB** of HTML, with no hidden private/profile traversal.

[CI 40056481549](https://github.com/sguzman/galerith/actions/runs/40056481549) passed offline matching, hostile-media rejection, normalization traversal regressions, loopback request, formatting, Clippy, tests and release build. Full metadata, archive/filename parity and live conformance remain unaccepted.


## Acidimg, Vipr and Imgspice public single-image subset

A shared Rust adapter in `src/extractor/basic_hosts.rs` now handles strict public source URL patterns for `acidimg.cc/img-TOKEN.html`, `vipr.im/TOKEN[.html]`, and `imgspice.com/TOKEN[/FILE]`. Acidimg uses the pinned public `imgContinue` POST parameter; Vipr and Imgspice request ordinary public pages. Host-specific HTML selectors extract original media with bounded optional filenames.

Each response is capped at **2 MiB** through declared content-length and streamed-body enforcement. Resulting media must use HTTPS and the corresponding domain (or its subdomains), with safe supported image extensions and no credentialed, query-bearing or fragment media links. Typed events carry the original alphanumeric token and public source URL; no numeric post identity is invented.

[Linux CI 40057318396](https://github.com/sguzman/galerith/actions/runs/40057318396) verifies source URL matching, host-specific HTML fixtures, foreign-origin rejection, Acidimg POST and oversized-response handling. This is entirely **offline/synthetic** evidence, not live differential conformance. Per-host metadata/filename/archive semantics, configurable request options and remaining `imagehosts.py` families are still unaccepted.


## Imgdrive, Imgtaxi and Imgwallet OpenGraph originals

An additional independently authored Rust adapter now recognizes pinned `imgdrive`, `imgtaxi`, and `imgwallet` public `/img-TOKEN.html` routes on the original `.com` and `.net` domains. It reads bounded OpenGraph `og:title` and `og:image` values, converts a same-site preview `/small/` path segment to the `/big/` original, and preserves a sanitized optional source title as a safe local filename. Typed messages carry the genuine alphanumeric token and canonical public page URL, without inventing a numeric post ID.

The HTML cap is **2 MiB** per response, checked against both declared and actual downloaded bytes. Foreign/credentialed origins, unsafe filename segments and query/fragment-bearing direct media are rejected.

[CI 40057685467](https://github.com/sguzman/galerith/actions/runs/40057685467) passed the pinned-source examples, unsafe input rejections, local one-request HTTP fixture, oversized content-length rejection, formatting, Clippy and release build. Full upstream metadata/date, all site-specific variants and live differential parity remain open.


## Turboimagehost pinned public image and bounded album subset

The read-only `/p/NUM/NAME.EXT.html` route resolves a public original media file, requiring a trusted first-party HTTPS image source, valid filename and real page token. The `/album/NUM/NAME` route emits ordered, deduplicated typed child queues from validated anchor `href` attributes. It does not follow foreign links, `data-href` or script-only strings.

HTML is capped at **2 MiB per response**; album traversal at **4 pages** and **160 unique child files**, with one-second interpage pacing and stop-on-empty accepted results. The flat album route explicitly requires the opt-in queue mode.

[Linux CI 40058313952](https://github.com/sguzman/galerith/actions/runs/40058313952) passed formatting, Clippy, tests, roadmap checks and release build. This is offline/synthetic evidence only. Complete upstream pagination, metadata, archive/filename conventions and live differential acceptance remain open.


## Imgadult guarded public image pages

The public `imgadult.com/img-HEX.html` route now resolves a safe original media URL within Imgadult's HTTPS domain and supported image extensions. Requests send the pinned public `img_i_d=1` cookie, and one bounded image element supplies original `src` and optional `alt` independently of attribute order. The boilerplate suffix in supported alternate names is dropped; unsafe names fall back to the original URL filename.

Each source HTML response is capped at **2 MiB**, with both content-length and streamed-body checks. Typed events retain the real hexadecimal page token; no numeric source-post scope is invented.

[CI 40058711190](https://github.com/sguzman/galerith/actions/runs/40058711190) passed strict URL matching, hostile origin tests, one-request public-cookie loopback fixture, formatter, Clippy, tests and release build. This is synthetic offline evidence, not full metadata/filename/archive or live differential parity.
