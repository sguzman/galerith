# 🎨 Piczel public gallery, folder and image preview

Pinned upstream: [gallery-dl piczel.py](https://github.com/mikf/gallery-dl/blob/7f226f74108501c8ac3120548370f68f19e6f9b3/gallery_dl/extractor/piczel.py). **Independent partial Rust port.**

The adapter recognizes strict first-party `piczel.tv/gallery/USER`, `/gallery/USER/FOLDER_ID` and `/gallery/image/ID` routes. It reads the corresponding pinned JSON APIs `/api/users/USER/gallery?page=N`, `/api/gallery/folder/ID?page=N` and `/api/gallery/ID`, with **2 MiB** JSON response caps, **4-page / 48-post-per-page** pagination bounds and **16 images per post / 160 originals** limits. Pagination requires monotonically increasing `next_page` ordinals within the bounded range.

Each post emits a typed directory event with available source-backed post ID, title, username, folder, tags and date text, followed by ordered original URL events (single image index 0 or multi-image indices starting at 0). Media paths require HTTPS on validated `piczel.tv` or actual subdomains, with supported image/video extensions; hostile image hosts and encoded path separators are rejected. No date zone is fabricated, and upstream special folder/archive templates remain unaccepted.

[Full CI 40198813205](https://github.com/sguzman/galerith/actions/runs/40198813205) passed Rust formatting, strict Clippy, unit/HTTP tests, roadmap checks and release build. Tests cover hostile URL/media rejection, ordered multi-image metadata, cursor bounds, a two-page user-gallery loopback HTTP fixture, and separate folder/image API endpoints. Live Piczel/CDN or authenticated behavior has not been checked.

**Remaining parity:** full original CDN variety, richer post/variation overlay fields and source date parsing, all user/folder filename-directory-archive behaviors, unbounded history and live pinned upstream differential equivalence.
