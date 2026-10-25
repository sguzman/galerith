# 🖼️ Urlgalleries album and deferred image originals

Pinned source: [gallery-dl urlgalleries.py](https://github.com/mikf/gallery-dl/blob/7f226f74108501c8ac3120548370f68f19e6f9b3/gallery_dl/extractor/urlgalleries.py). **Independent partial Rust port.** Not complete upstream parity.

The matcher accepts first-party `urlgalleries.com/BLOG/ID[/TITLE]` URLs with a validated blog, numeric gallery ID and optional safe slug, including one-label site subdomains. It rejects lookalike origins, unexpected queries, credentials, untrusted ports and traversal. The source gallery is read from the pinned `?a=10000` page, with a **2 MiB** HTML cap. Visible gallery information is retained when present: title, source blog, original ISO date string, description, numeric source gallery ID, and selected gallery media count. Missing metadata is not fabricated.

The bounded album section supplies first-party image-page links. Each image page is read with the same byte and redirect restrictions. A supported `IMAGE_URL` original must be a parsed public HTTP(S) domain, without credentials, explicit ports or fragments, and is emitted as a **typed deferred queue**, not a downloaded media file. Maximum discovery is **48 image pages** with **300 ms** pacing; the native queue consumer still enforces independent follow limits.

[CI 40210181020](https://github.com/sguzman/galerith/actions/runs/40210181020) passed Rust formatting, strict Clippy, typed source tests, a real three-request album → two images loopback fixture, roadmap checks, release build and the independent pinned directlink oracle. **There is no live source or upstream differential proof.**

**Unaccepted:** the full pinned rich gallery tags/date/description parsing, source blog correction rules, every image-page layout, complete filename/directory/archive templates, unrestricted galleries and live original/CDN parity.
