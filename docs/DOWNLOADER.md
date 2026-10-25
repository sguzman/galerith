# ⬇️ HTTP downloader: preview behavior

The downloader is **not gallery-dl compatible yet**. This document describes behavior verified by local synthetic HTTP tests.

## Files and crash recovery

- A completed output is saved as its requested safe leaf filename.
- A live download writes to `<filename>.part`, not directly to an existing completed output. Successful transfer and checks rename the partial to the target.
- Its `<filename>.part.json` sidecar holds a SHA-256 fingerprint of the source URL plus available HTTP validators (ETag and Last-Modified), **not the source URL itself**. The metadata file is removed after a successful transfer.
- Existing partials are resumable **only** when a matching URL fingerprint and a usable strong ETag or Last-Modified validator exist. Otherwise, downloading restarts from the beginning. This is deliberately more conservative than some gallery-dl behaviors.
- A resume sends `Range` and `If-Range`. A full HTTP 200 response truncates the old partial and downloads fresh content. A valid 206 response must begin at the requested offset, have a compatible validator when supplied, and describe the entire remaining suffix.
- HTTP 416 resets the partial for a later full request. A response body shorter than the declared Content-Length is never finalized.
- After a complete and validated transfer, a valid HTTP `Last-Modified` header is applied to the finalized file's filesystem modification time. A verified resumed response can inherit the original saved `Last-Modified` value if the continuation omits it. Invalid dates are ignored; timestamp-setting failures are nonfatal after the file has been saved.
- Recoverable transfer errors preserve the validated partial for a retry or another CLI invocation. If a partial is untagged/invalid, it is overwritten rather than blindly appended.
- If no complete-length metadata is provided by a server, end-of-stream cannot independently prove full content; checksum verification and stronger integrity guarantees remain pending.

## Locking and naming

Each target has a stable hidden `.<filename>.galerith.lock` advisory lock. The OS releases the file lock after process exit; the empty lock file intentionally **remains** to avoid races from unlinking a lock path while other processes hold file descriptors. The existence check, transfer and rename are all performed under the lock. Concurrent Galerith workers therefore serialize access to a single output.

The preview defaults to skipping existing output. The CLI `--no-skip` enables overwriting, but the old completed target remains until the replacement is fully downloaded and renamed.

Filename input is revalidated at the downloader boundary; callers cannot bypass extraction to write a path such as `../outside.jpg`.

## HTTP retry behavior

Connection errors, interrupted bodies, HTTP 429 and HTTP 5xx responses use bounded retries. Numeric seconds and RFC-style HTTP-date `Retry-After` values are accepted (each capped at 60 seconds). `--sleep` accepts constant nonnegative delays up to 3600 seconds before each media download. Sleep ranges, request/extractor/skip pacing and complete upstream retry policies remain unimplemented.

## Verified synthetic tests

- Resuming both during one invocation and across invocations with ETag/Last-Modified; preserving source mtime on fresh and resumed completed media.
- Restarting after a full HTTP 200 replaced a Range request, rejecting unsolicited 206 responses.
- Handling missing partial metadata and rejecting unsafe public `MediaItem` filenames.
- Multi-megabyte chunked transport, 429/500/503 retry sequences and 404 failures.
- Parallel same-output attempts (one download plus one skip); safe overwrite and error preservation.
- No real media, private account content, tokens, or site traffic are used in these regression fixtures.

Verification: [CI 40218907724](https://github.com/sguzman/galerith/actions/runs/40218907724) verifies Last-Modified file mtimes and the full Rust suite. Earlier [CI 39957860428](https://github.com/sguzman/galerith/actions/runs/39957860428) verifies bounded transfer recovery. 
