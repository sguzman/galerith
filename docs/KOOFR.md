# 📁 Koofr public shared-file partial port

**Pinned upstream:** [gallery-dl koofr.py](https://github.com/mikf/gallery-dl/blob/7f226f74108501c8ac3120548370f68f19e6f9b3/gallery_dl/extractor/koofr.py). This is an **independent, incomplete Rust implementation**, not a claim of production compatibility.

The native CLI recognizes strict public `/links/ID` URLs on `koofr.net`, `koofr.eu` and their `app.` variants; all map to Koofr's canonical public app API. Credentials, arbitrary query parameters, encoded traversal, source host lookalikes and nondefault ports are refused. It reads public link metadata with a password-free `GET /api/v2/public/links/ID?password=` request, including the pinned frontend headers and canonical Referer. Source JSON response bodies are capped at **2 MiB** and must remain on their requested origin.

Public single-file shares return their validated, source-backed original URL and file metadata; directory shares traverse the public `/bundle` listing endpoint with caps of **160 files**, **48 entries per API page**, **32 directories**, and **4 nested levels**. Additional folder requests are paced at 250 ms. Filenames and relative paths are validated to reject traversal, control characters, unsafe percent escapes and foreign media hosts. Source hash, size, modified timestamp and ID are preserved when provided. Typed `directory` / `url` events expose share ID, title, original filename and relative folder path without inventing absent metadata.

[Full CI 40200014555](https://github.com/sguzman/galerith/actions/runs/40200014555) passed Rust formatting, strict Clippy, unit/loopback HTTP tests and release build. The synthetic tests cover one shared file, two nested bundle API pages, unsafe filenames and hostile origins. **These tests do not establish live Koofr availability or unrestricted folder parity.**

**Unaccepted:** `k00.fr` redirect-shortened links, shared password configuration, complete upstream template and archive formatting, all CDN-host variants, unlimited folder recursion, source timestamp conversion and live pinned-upstream differential verification.
