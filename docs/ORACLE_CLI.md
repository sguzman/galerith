# 🔬 Offline CLI golden corpus

This is a deterministic, read-only regression suite for Galerith's own current CLI output. **It is not yet a reference differential oracle against gallery-dl.** The upstream source pin is `7f226f74108501c8ac3120548370f68f19e6f9b3`, but no upstream Python code is executed by this suite.

The machine-readable corpus is [`tests/fixtures/cli_golden.json`](../tests/fixtures/cli_golden.json) and the Rust executable regression is [`tests/oracle_cli.rs`](../tests/oracle_cli.rs). Each case names its arguments and expected structured JSON subset, ordered JSON-line event stream, exact stdout or failure diagnostics with synthetic-token redaction. All cases avoid source-site HTTP requests and run offline in CI using the built Galerith binary.

The remaining work for the pinned comparison oracle includes **upstream-produced golden output**, site-by-site structured extractor comparisons, controlled pinned Python execution in an isolated comparison environment, and signed reproducible test reports. Do not credit those checks based on this internal corpus alone.

**Accepted internal corpus (CI 40205524617):** Eight deterministic binary-invocation regressions cover direct media JSON, source-file naming, typed directory/URL messages, exact URL output, positive and absent range filtering, unsupported extractor redaction and invalid range diagnostics. [Full Linux Rust CI](https://github.com/sguzman/galerith/actions/runs/40205524617) passed. This satisfies only the *automated Galerith CLI golden corpus* acceptance check. It does **not** satisfy pinned upstream differential comparisons or signed reproducible comparison reports.

## 🔬 Isolated pinned-upstream differential oracle

A separate CI job now checks out original gallery-dl at exact immutable commit `7f226f74108501c8ac3120548370f68f19e6f9b3` and executes its directlink extractor using a short-lived, CI-only Python 3.12 reference environment. **Galerith's production binary remains Rust-only.** The reference checkout SHA is explicitly checked; test inputs use four synthetic public direct-media URLs and do not contact any of those source hosts.

The Rust integration test [`tests/reference_directlink.rs`](../tests/reference_directlink.rs) invokes both CLIs, compares source-produced directory/URL message order, URLs, and source-backed category, subcategory, domain, path, filename stem, extension, query and fragment metadata. The [verified CI run 40206360597](https://github.com/sguzman/galerith/actions/runs/40206360597) passed both ordinary Rust tests and the pinned upstream job. Their combined status is published as one GitHub commit status; a failure in either path fails acceptance.

**Known divergence:** Upstream lowercases the extension for filename formatting, while Galerith's *flattened preview* preserves the source filename extension's capitalization. This check tests upstream filename stem and extension metadata independently; it deliberately does not assert complete archive, directory or filename-template parity. No broader site module, HTTP availability, postprocessor or signed reproducible report has been accepted from this narrow differential test. The remaining T1001 checks continue to require actual upstream comparisons beyond directlink and verifiable signed reports.
