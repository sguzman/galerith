# 🖼️ ImageVenue public original-image preview

This is an independent **partial** Rust port of ImageVenue in pinned upstream `imagehosts.py` (gallery-dl commit `7f226f74108501c8ac3120548370f68f19e6f9b3`).

Supports `https://www.imagevenue.com/ME13LS07`-shaped uppercase image IDs and numeric `imgNN.imagevenue.com` hosts. Pinned legacy `/view/o?i=SAFE_IMAGE&h=imgNUMBER&l=locNUMBER` and `/img.php?image=SAFE_IMAGE` sources are accepted only with strictly validated query fields. Matching historical routes does not guarantee all historical pages are live or parsable.

A read-only bounded (2 MiB declared and streamed) source HTML request locates the public `card-body`, skips `loader.svg` and selects a trusted HTTPS ImageVenue original image. Optional escaped `alt` filename preserves Unicode, excludes control/path characters and safely inherits image extension. Typed events preserve source token and source host; flat media values remain unchanged. The alphanumeric source identity is never invented as a numeric post ID.

Evidence: [Linux CI 40118064201](https://github.com/sguzman/galerith/actions/runs/40118064201) and [40118283168](https://github.com/sguzman/galerith/actions/runs/40118283168), including offline source, media-host and local HTTP regressions.

**Still open:** pinned ImageVenue cookies, all old URL layouts, the custom media-response 404 placeholder signature, file metadata, filename/archive formatting and live differential acceptance. The entire multi-host upstream source module remains partial.
