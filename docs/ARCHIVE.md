# 🗃️ Native download archive (preview)

The `--download-archive FILE` option creates a SQLite file and records **only successfully completed downloads**. Previously recorded items are skipped before fetching their media bytes. Existing files skipped by filename alone are **not** automatically recorded, since their origin has not been verified.

## Format

- Database: SQLite table `downloads(category, identity, recorded_at)`; unique composite key on category and identity
- For direct-media URLs, `identity` is the SHA-256 digest of the URL and `category` identifies the extractor. Signed URLs are not stored in plaintext.
- For items with a post ID, `identity` is `post:POST_ID:url-sha256:DIGEST`, scoped to the extractor. The digest fingerprints the complete media URL so several distinct originals belonging to one post cannot suppress one another. URL query strings are included in the fingerprint but not persisted in plaintext.
- Legacy `post:POST_ID` rows cannot distinguish which original was previously downloaded. They remain in the database but do not suppress media under the new key. This may cause safe repeat downloads after upgrading; it avoids silently dropping images.
- Signed URL rotations can change Galerith's native digest and require redownloading. If an existing database also contains an upstream `archive(entry TEXT PRIMARY KEY)` table, Galerith recognizes the pinned gallery-dl **directlink** default archive keys as a read-only fallback (including their query-independent matching). It never adds or modifies entries in that foreign table. Extractor-specific formats, custom tables, and general gallery-dl archive interchange/migrations remain unsupported.
- Reopening the database retains results across invocations.
- Cooperating Galerith processes serialize the archive check, download, and record with OS advisory locks. The 64 stable lock stripes live next to the canonical database path in a `.galerith-locks` directory; relative and symlink database aliases share locks. Guard release on process exit/crash permits recovery without a stale claim. Lock files are never unlinked and contain no media URLs.
- Extract-only/dry-run modes (`--get-urls`, `--dump-json`, `--simulate`, `--no-download`) do not open or modify the archive.
- Successful overwrite downloads are recorded, but skipping an already existing filename does not imply download success.

**Limitations:** The native `downloads` table is Galerith-specific and is not the gallery-dl archive schema. There is no interchange with gallery-dl, migration, cache-expiry model or distributed reservation/claim protocol yet. File-output locks protect concurrent writes; archive locks additionally prevent cooperating Galerith processes from simultaneously downloading the same archive identity. gallery-dl and other writers do not honor these private locks.

Verification: [CI 40218601459](https://github.com/sguzman/galerith/actions/runs/40218601459) includes multi-original post IDs, query-distinguished media, legacy post-only record safety and plaintext secret rejection. Earlier [CI 39948367906](https://github.com/sguzman/galerith/actions/runs/39948367906) verifies CLI persistence and archive isolation.


Further accepted verification: [CI 40221714131](https://github.com/sguzman/galerith/actions/runs/40221714131) passed cross-process deduplication, failed-download retry, killed-process archive lock recovery, full Rust checks and the pinned directlink oracle. [CI 40221440025](https://github.com/sguzman/galerith/actions/runs/40221440025) independently passed the 13-case upstream directlink archive-key differential. These are partial compatibility improvements only: general gallery-dl archive-ID, custom archive-table, migration and cache-expiry parity remain open.
