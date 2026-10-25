# 📊 Slideshare public slideshow preview

Pinned upstream source: [gallery-dl slideshare.py](https://github.com/mikf/gallery-dl/blob/7f226f74108501c8ac3120548370f68f19e6f9b3/gallery_dl/extractor/slideshare.py). **Independent partial Rust port; not complete upstream parity.**

The native CLI recognizes strict public `slideshare.net/USER/PRESENTATION` and `/mobile/USER/PRESENTATION` (with optional `www`) URLs, normalizes to the first-party HTTPS presentation page, and rejects encoded traversal, invalid path segments, credentials, lookalike origins, unsupported ports, queries and fragments. The page is fetched once with **2 MiB** response and redirected-origin bounds. Scripts are never executed.

Public `<script id="__NEXT_DATA__">` JSON is read as data from `props.pageProps.slideshow`. The parser preserves visible source `username`, title, description, views, likes and timestamp, and validates the source `slides.host`, `imageLocation`, `title` and `totalSlides` fields. Each numbered JPEG uses the pinned `HOST/IMAGELOCATION/95/TITLE-N-1024.jpg` image mapping; output names follow `PRESENTATION-NN.jpg`. Only trusted HTTPS slidesharecdn.com origins/subdomains with safe path components are accepted. The preview caps source decks at **256 slides**, with correct ordered indices and typed Directory/URL media records.

The offline tests validate desktop/mobile matching, CDN spoofing, malformed media names, unsupported slide counts and a real loopback one-page GET producing three original image URLs. The complete Rust workflow, including strict Clippy and release build, passed [CI 40155515706](https://github.com/sguzman/galerith/actions/runs/40155515706).

**Remaining pinned parity:** unrestricted valid large decks, every media/CDN format and hydration variant, full datetime and filename/directory/archive semantics, redirect/response behavior, authenticated or restricted presentations, and live pinned upstream differential tests. No production source conformance is claimed.
