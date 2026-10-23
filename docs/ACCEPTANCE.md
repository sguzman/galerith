# 🏁 Complete-port MVP acceptance policy

Galerith reaches its **first MVP** only after all of the following are demonstrably complete:

1. Every pinned upstream documented CLI option and configuration dialect has compatible behavior, with recorded deviations resolved.
2. Every applicable upstream extractor class and supported URL shape is implemented natively in Rust, including pagination, metadata, authentication variants and special extractors.
3. The source-module matrix has no unresolved entries; a class-level supported-site audit corroborates it.
4. Download behavior covers retries, resume, partial files, filtering, dedup, archive/cache, filename templates, postprocessing and external downloader integrations.
5. Local fixture and opt-in live tests exercise supported site variants and produce reference-comparable structured output without committing secrets.
6. Linux CI passes formatting, clippy, tests and release builds; independent EndeavourOS runtime checks are recorded.
7. The release is versioned and documents compatibility gaps accurately. No hidden Python runtime or wrapper dependency.

Percentages measure **verified acceptance checks**, not code-volume or guessed parity. A task cannot be 100% without all its checks and evidence. A matrix module cannot be `complete` based solely on a matching file name or CLI recognition. Current preview is **not** MVP.
