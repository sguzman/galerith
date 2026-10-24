# 🖼️ Kabeuchi public user-image partial port

Pinned upstream: [gallery-dl kabeuchi.py](https://github.com/mikf/gallery-dl/blob/7f226f74108501c8ac3120548370f68f19e6f9b3/gallery_dl/extractor/kabeuchi.py). **Independent partial Rust port, not complete source parity.**

Strict first-party `kabe-uchiroom.com/mypage/?id=NUMERIC` URLs are resolved by fetching the public profile and reading a source-backed numeric `user_friend_id`. Public source upload JSON is fetched with the upstream-style `POST /get_posts.php` form fields: fixed anonymous `user_id=0`, validated target ID, `type=uploads`, sort/category selectors, source `latest_post` cursor and bounded page number. HTML and JSON responses are individually capped at **2 MiB**, rejecting cross-origin redirects, malformed page arrays, repeated cursors or unexpected response sizes.

The Rust adapter emits at most **160 public originals** across **4 API pages** and at most **48 posts per source page**, using up to six ordered image slots per source post. It skips ads (including truthy numeric/text flags) and posts with no first image, stops at the first empty image slot, validates media basename/extension and constructs first-party `/accounts/upfile/{last_uid_digit}/{uid}/NAME` HTTPS originals. Available source ID, title, optional Twitter identifiers, and original date strings are carried in typed directory/URL messages, without fabricated date zones.

[Full Linux CI 40135167658](https://github.com/sguzman/galerith/actions/runs/40135167658) passed format, strict Clippy, tests, roadmap checks and release build. Tests include strict URL and unsafe image cases plus a public-profile GET → upload-POST one-page and two-page cursor HTTP fixture. **These are synthetic tests, not evidence of live access or authenticated user coverage.**

**Unaccepted:** unlimited public posts, complete upstream metadata/date parsing, original filename/archive/directory templates, source API variation, private/authenticated content and live pinned upstream differential conformance.
