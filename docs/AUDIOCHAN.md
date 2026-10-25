# 🎧 Audiochan guarded public originals and user audio feeds

Pinned source: [gallery-dl audiochan.py](https://github.com/mikf/gallery-dl/blob/7f226f74108501c8ac3120548370f68f19e6f9b3/gallery_dl/extractor/audiochan.py). **Status: independent partial Rust adapter.**

Strict public `audiochan.com/a/SLUG` and `audiochan.com/u/USER` first-party routes reject lookalike hosts, credentials, encoded traversal, unsafe slugs and private query parameters. Public individual audio uses pinned `https://api.audiochan.com/audios/slug/SLUG` JSON, requiring matching source slug and a validated direct `audioFile.url` original on a trusted HTTPS `audiochan.com` or subdomain media host. Original IDs, title/slug, filename/extension, created and updated date strings, category:name tags, nested description text, and available credit-user details are preserved in typed directory/URL records.

The Rust request layer sends the pinned `Origin`, JSON content type and `Sec-Fetch` API headers, enforcing **2 MiB streamed JSON response limits** and first-party redirect checks. Public `/u/USER` records resolve the source user identity and browse the pinned `/users/USER/audios?sfw_only=false&sort=new&page=N&limit=12` endpoint. At most **4 pages / 48 audio child queues** are returned with one-second pacing, source `has_more` pagination and first-seen slug deduplication. User listings require typed `--dump-messages` or explicit `--follow-queues` traversal; no unbounded downloads are silently triggered.

When the source only exposes a segmented `audioFile.stream_url` but no direct `audioFile.url`, this adapter rejects the work explicitly: Galerith does **not** yet implement gallery-dl's segmented-audio transfer semantics. Media hosts not yet recognized are similarly unaccepted rather than silently trusted.

[CI 40154632481](https://github.com/sguzman/galerith/actions/runs/40154632481) passed native routing/URL, trusted original metadata, nested tags/descriptions, media identity rejection and pinned request-header fixtures, a three-request single audio+user test, and a real two-page user pagination/duplicate suppression HTTP fixture, strict Clippy, format and release build. This is offline/synthetic verification only.

**Unaccepted:** segmented HTTP audio transfer fallback and its download headers, collection and search routes, site-specific alternative direct media origins and complex users, complete date, filename/directory/archive template support, arbitrary-depth feeds and live upstream differential conformance.
