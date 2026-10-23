# 🗃️ Native download archive (preview)

The `--download-archive FILE` option creates a SQLite file and records **only successfully completed downloads**. Previously recorded items are skipped before fetching their media bytes. Existing files skipped by filename alone are **not** automatically recorded, since their origin has not been verified.

## Format

- Database: SQLite table `downloads(category, identity, recorded_at)`; unique composite key on category and identity
- For direct-media URLs, `identity` is the SHA-256 digest of the URL and `category` identifies the extractor. Signed URLs are not stored in plaintext.
- For the preliminary Danbooru extractor, `identity` is its post ID, namespaced to its extractor. This allows a previously downloaded post to be recognized after URL changes.
- Reopening the database retains results across invocations.
- Extract-only/dry-run modes (`--get-urls`, `--dump-json`, `--simulate`, `--no-download`) do not open or modify the archive.
- Successful overwrite downloads are recorded, but skipping an already existing filename does not imply download success.

**Limitations:** The database has a Galerith-specific schema, not the gallery-dl archive schema. There is no interchange with gallery-dl, migration, cache-expiry model or distributed reservation/claim protocol yet. File-output locks protect concurrent writes, but the archive currently does not prevent two separate concurrent processes from both querying the same unrecorded entry before a first successful download.

Verification: [GitHub Actions 39948367906](https://github.com/sguzman/galerith/actions/runs/39948367906), including synthetic HTTP download + repeat CLI calls, persistence after reopen and extraction-only isolation.
