# 🔬 Offline CLI golden corpus

This is a deterministic, read-only regression suite for Galerith's own current CLI output. **It is not yet a reference differential oracle against gallery-dl.** The upstream source pin is `7f226f74108501c8ac3120548370f68f19e6f9b3`, but no upstream Python code is executed by this suite.

The machine-readable corpus is [`tests/fixtures/cli_golden.json`](../tests/fixtures/cli_golden.json) and the Rust executable regression is [`tests/oracle_cli.rs`](../tests/oracle_cli.rs). Each case names its arguments and expected structured JSON subset, ordered JSON-line event stream, exact stdout or failure diagnostics with synthetic-token redaction. All cases avoid source-site HTTP requests and run offline in CI using the built Galerith binary.

The remaining work for the pinned comparison oracle includes **upstream-produced golden output**, site-by-site structured extractor comparisons, controlled pinned Python execution in an isolated comparison environment, and signed reproducible test reports. Do not credit those checks based on this internal corpus alone.

**Accepted internal corpus (CI 40205524617):** Eight deterministic binary-invocation regressions cover direct media JSON, source-file naming, typed directory/URL messages, exact URL output, positive and absent range filtering, unsupported extractor redaction and invalid range diagnostics. [Full Linux Rust CI](https://github.com/sguzman/galerith/actions/runs/40205524617) passed. This satisfies only the *automated Galerith CLI golden corpus* acceptance check. It does **not** satisfy pinned upstream differential comparisons or signed reproducible comparison reports.
