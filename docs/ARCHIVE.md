# 🗃️ Native download archive (preview)

The `--download-archive FILE` option creates a SQLite file and records **only successfully completed downloads**. Previously recorded items are skipped before fetching their media bytes. Existing files skipped by filename alone are **not** automatically recorded, since their origin has not been verified.

## Format

- Database: SQLite table `downloads(category, identity, recorded_at)`; unique composite key on category and identity
- For direct-media URLs, `identity` is the SHA-256 digest of the URL and `category` identifies the extractor. Signed URLs are not stored in plaintext.
- For items with a post ID, `identity` is `post:POST_ID:url-sha256:DIGEST`, scoped to the extractor. The digest fingerprints the complete media URL so several distinct originals belonging to one post cannot suppress one another. URL query strings are included in the fingerprint but not persisted in plaintext.
- Legacy `post:POST_ID` rows cannot distinguish which original was previously downloaded. They remain in the database but do not suppress media under the new key. This may cause safe repeat downloads after upgrading; it avoids silently dropping images.
- Signed URL rotations can change the digest and require redownloading. Full gallery-dl archive ID compatibility and migrations are still pending.
- Reopening the database retains results across invocations.
- Extract-only/dry-run modes (`--get-urls`, `--dump-json`, `--simulate`, `--no-download`) do not open or modify the archive.
- Successful overwrite downloads are recorded, but skipping an already existing filename does not imply download success.

**Limitations:** The database has a Galerith-specific schema, not the gallery-dl archive schema. There is no interchange with gallery-dl, migration, cache-expiry model or distributed reservation/claim protocol yet. File-output locks protect concurrent writes, but the archive currently does not prevent two separate concurrent processes from both querying the same unrecorded entry before a first successful download.

Verification: [CI 40218601459](https://github.com/sguzman/galerith/actions/runs/40218601459) includes multi-original post IDs, query-distinguished media, legacy post-only record safety and plaintext secret rejection. Earlier [CI 39948367906](https://github.com/sguzman/galerith/actions/runs/39948367906) verifies CLI persistence and archive isolation.
