# 🪷 ImagePond public single-file preview

ImagePond is a **partial**, independently authored Rust adaptation of pinned upstream `gallery_dl/extractor/imagepond.py` at commit `7f226f74108501c8ac3120548370f68f19e6f9b3`.

The current safe subset recognizes public ImagePond single-file shapes `/i/ID`, `/img/ID`, `/image/ID` and `/video/ID` under `imagepond.net` or `www.imagepond.net`. For each supported URL it makes one bounded HTML request and reads the matching OpenGraph image/video link. It rejects foreign hosts, userinfo/credentials, query-bearing source URLs, non-HTTPS media, unsupported extensions and unsafe filenames. The media URL must be hosted by ImagePond or one of its subdomains.

The source HTML byte ceiling is **2 MiB**, enforced against both declared content lengths and streamed chunks. The extractor emits the safe flattened `MediaItem` normally, or directory/URL typed events including validated source ID and bounded optional title when message inspection is requested. Numeric post IDs are not invented from nonnumeric source slugs.

Verified offline tests cover image and video URL selection, unsafe media rejection, strict page matching, one-request HTTP resolution and oversized declared responses. Evidence: [Linux CI 40051989239](https://github.com/sguzman/galerith/actions/runs/40051989239).

**Not implemented:** the pinned upstream ImagePond album/user generators, their paginated child queues, the complete metadata and date/video duration model, broad Chevereto source and media variants, all media MIME/format aliases, filename/archive policies and live upstream differential conformance. No upstream module is fully accepted on the strength of this preview.

For exact scope and acceptance status see the [full module matrix](port-matrix.json) and [roadmap](ROADMAP.md).
