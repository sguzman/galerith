# 🗃️ Native download archive (preview)

Galerith has a Rust/SQLite `--download-archive FILE` implementation. It is a **new Galerith archive format**, not yet an implementation of gallery-dl's archive schema or all extractor-specific archive IDs.

## CLI

```sh
galerith --download-archive ./state/gallery.sqlite -d ./media https://example.org/photo.jpg
```

- The SQLite database is created only for downloading; `--get-urls`, `--dump-json`, `--simulate`, and `--no-download` do not create or inspect it.
- Records are written **only after** a successful download and completion rename. A skip due solely to a pre-existing filename does not create an archive entry.
- Archive hits prevent network fetches even if the output file has subsequently been deleted; removing the output file does not remove its recorded archive identity.
- Category/identity are unique together. Danbooru uses `post:<post-id>` scoped to the Danbooru extractor. Direct-link URLs use a SHA-256 digest of the full source URL. This limits stored secrets, but expiring token parameters currently also affect the identity.
- The current schema is `downloads(category, identity, recorded_at)` with a compound primary key. Multiple processes use SQLite's lock handling and a finite busy timeout.
- This does not yet migrate or read an upstream gallery-dl SQLite archive. More complete extractor identities, schema migration tests, and upstream compatibility remain required for the MVP.

## Acceptance evidence

- `src/archive.rs`: database operations and independent unit tests for reopen, duplicate insert, extractor scoping and URL-token non-persistence.
- `tests/archive_cli.rs`: CLI downloads with `--download-archive`, persists state, deletes completed output and confirms a second invocation skips a now-unreachable server.
- CI: [39948367906](https://github.com/sguzman/galerith/actions/runs/39948367906).
