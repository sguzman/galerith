# 🌐 HTTP client option preview

Galerith currently exposes **`-a` / `--user-agent`**, **`--proxy`**, **`--http-timeout`**, and constant **`--sleep`**. The first two use the same native Rust HTTP client for API and media requests.

- `-a 'MyAgent/1.0'`: send this explicit User-Agent on requests. Without it, the client identifies itself as Galerith with the package version.
- `--proxy http://127.0.0.1:8080`: route HTTP(S) traffic through the supplied proxy URL. The CLI handles malformed proxy URLs with a generic error rather than printing the supplied credentials.
- `--http-timeout 30`: client timeout, expressed in seconds.
- `--sleep 0.15`: wait this many seconds before each media download. Values must be finite and between 0 and 3600; upstream delay ranges and sleep-request modes are not supported.

**Privacy:** Proxy URLs may contain credentials. Avoid putting secrets in arguments or shell history: another local process may be able to inspect command-line arguments. Do not store credentials in public scripts, issues or fixtures.

**Not yet upstream parity:** all address-family flags, proxy behavior across every transport, advanced TLS options, response diagnostics, per-host pacing and complete reference networking options remain open.

**Verification:** [CI 39957860428](https://github.com/sguzman/galerith/actions/runs/39957860428) includes synthetic proxy/User-Agent, bounded sleep and date-based retry tests; no public origin is contacted.
