# 🪷 ImagePond public single-file preview

ImagePond is a **partial**, independently authored Rust adaptation of pinned upstream `gallery_dl/extractor/imagepond.py` at commit `7f226f74108501c8ac3120548370f68f19e6f9b3`.

The current safe subset recognizes public ImagePond single-file shapes `/i/ID`, `/img/ID`, `/image/ID` and `/video/ID` under `imagepond.net` or `www.imagepond.net`. For each supported URL it makes one bounded HTML request and reads the matching OpenGraph image/video link. It rejects foreign hosts, userinfo/credentials, query-bearing source URLs, non-HTTPS media, unsupported extensions and unsafe filenames. The media URL must be hosted by ImagePond or one of its subdomains.

The source HTML byte ceiling is **2 MiB**, enforced against both declared content lengths and streamed chunks. The extractor emits the safe flattened `MediaItem` normally, or directory/URL typed events including validated source ID and bounded optional title when message inspection is requested. Numeric post IDs are not invented from nonnumeric source slugs.

Verified offline tests cover image and video URL selection, unsafe media rejection, strict page matching, one-request HTTP resolution and oversized declared responses. Evidence: [Linux CI 40051989239](https://github.com/sguzman/galerith/actions/runs/40051989239).

## Album and user typed queues

The bounded read-only `/a/ID`, `/album/ID` and explicit `/user/USERNAME` routes emit deferred `queue` messages with validated first-party image/video child URLs and parent album or username context. They can be inspected with `--dump-messages --expand-queues` or used with opt-in `--follow-queues`. The default eager/flat album or user URL remains explicitly unsupported.

Listing HTML responses are limited to **2 MiB** each, traversal to **4 pages** and output to **48 distinct child URLs**. A supported `rel="next"` link must remain at the same origin and path, have only a numeric advancing page query, and fit inside the page cap. Invalid/external links are rejected without following them. The resolver paces consecutive pages by one second and preserves first-seen child ordering across pages. [Linux CI 40052501580](https://github.com/sguzman/galerith/actions/runs/40052501580) validates origin controls and two-page local HTTP/dedup fixtures.

**Still missing:** all pinned upstream album/user URL aliases, exhaustive original Chevereto HTML layouts and arbitrary continuation queries, complete metadata and date/video duration models, broad media/thumbnail/MIME variants, filename/archive policies, account behavior and live upstream differential conformance. No upstream module is fully accepted on the strength of these previews.

For exact scope and acceptance status see the [full module matrix](port-matrix.json) and [roadmap](ROADMAP.md).


## Additional typed file metadata and queue-origin safeguards

A verified metadata subset preserves public OpenGraph image/video class and up to 32,768-pixel dimensions, and videos can carry allowlisted MIME type plus a same-site thumbnail URL. Missing, malformed, overflow and untrusted fields are excluded without changing the flattened media record. The deferred queue scanner only constructs links from recognized first-party anchor attributes, not naked `/i/` substrings inside scripts or foreign links. See [CI 40053074485](https://github.com/sguzman/galerith/actions/runs/40053074485). This is deliberately narrower than the upstream full file metadata and DOM/HTML compatibility contract.


## Pinned upstream URL and album-title extensions

The literal ImagePond/Chevereto album fallback representation `'javascript:void(0)' : 'URL'` is recognized as **data only**; scripts are not executed. The candidate is validated using the same strict first-party public-file URL matcher as normal child anchors, and original document order is preserved. Pinned reference example IDs include `chv_317512` and `1000423939.zb8Fxy`. The public username form `/dariusbbb24` (also `/ap000`) routes to the guarded user index just like `/user/USERNAME`, without treating reserved routes as usernames.

Public album headings are carried as a sanitized optional `album` field in deferred typed events. The first valid heading can be inherited by children on later pages. The value does not silently rename files or change flat download behavior.

Offline verification: [CI 40053615659](https://github.com/sguzman/galerith/actions/runs/40053615659) for pinned fallback, [CI 40053728460](https://github.com/sguzman/galerith/actions/runs/40053728460) for bare username routes, and [CI 40053800312](https://github.com/sguzman/galerith/actions/runs/40053800312) for headings. The multi-page inherited-title check is in [CI 40053864707](https://github.com/sguzman/galerith/actions/runs/40053864707). These remain offline previews, not complete upstream acceptance.
