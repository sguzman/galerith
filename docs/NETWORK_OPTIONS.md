# 🌐 HTTP client option preview

Galerith currently exposes **`-a` / `--user-agent`**, **`--proxy`**, and **`--http-timeout`**. The first two use the same native Rust HTTP client for API and media requests.

- `-a 'MyAgent/1.0'`: send this explicit User-Agent on requests. Without it, the client identifies itself as Galerith with the package version.
- `--proxy http://127.0.0.1:8080`: route HTTP(S) traffic through the supplied proxy URL. The CLI handles malformed proxy URLs with a generic error rather than printing the supplied credentials.
- `--http-timeout 30`: client timeout, expressed in seconds.

**Privacy:** Proxy URLs may contain credentials. Avoid putting secrets in arguments or shell history: another local process may be able to inspect command-line arguments. Do not store credentials in public scripts, issues or fixtures.

**Not yet upstream parity:** all address-family flags, proxy behavior across every transport, advanced TLS options, response diagnostics, per-host pacing and complete reference networking options remain open.

**Verification:** [CI 39956648701](https://github.com/sguzman/galerith/actions/runs/39956648701) uses a synthetic localhost proxy to assert HTTP request routing and the custom User-Agent without contacting a public origin.
