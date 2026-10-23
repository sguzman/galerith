# ⚖️ Provenance and license boundary

Galerith's initial, user-created repository carries the **MIT license**. Its initial Rust implementation is independently authored using public feature descriptions, URLs and CLI documentation; no upstream gallery-dl Python code has been copied.

Upstream gallery-dl, maintained at [mikf/gallery-dl](https://github.com/mikf/gallery-dl), is **GPL-2.0**. It is used as a behavior and compatibility reference at the pinned commit in [SOURCE_BASELINE.md](SOURCE_BASELINE.md). A feature-compatible independently written implementation does not automatically become a derivative merely because it supports the same inputs/outputs. However, **translating or adapting upstream GPL code would create obligations under GPL-2.0**, and the current MIT license does not override those obligations.

**Policy:** Before any translated GPL implementation is committed, document provenance and reconcile the repository license to GPL-2.0, retaining all legally required notices and attribution. Do not copy implementations while claiming they are clean-room. Any copied test material/fixtures must have its own license/redistribution assessment.

Never store credentials, real user cookies, account sessions, private media archives or private conversations in the public repository. Use synthetic fixtures and independently authored tests.
