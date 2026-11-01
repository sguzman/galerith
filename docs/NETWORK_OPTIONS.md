# 🌐 HTTP client option preview

Galerith currently exposes **`-a` / `--user-agent`**, **`--proxy`**, **`--http-timeout`**, **`--force-ipv4` / `--force-ipv6`**, **`--source-address`**, and **`--sleep`** as a fixed delay or inclusive range. User-Agent, proxy, timeout, and address-family selection apply to the shared native Rust HTTP client. `downloader.http.headers` applies extra headers to media transfers only.

- `-a 'MyAgent/1.0'`: send this explicit User-Agent on requests. Without it, the client identifies itself as Galerith with the package version.
- `--proxy http://127.0.0.1:8080`: route HTTP(S) traffic through the supplied proxy URL. The CLI handles malformed proxy URLs with a generic error rather than printing the supplied credentials.
- `--http-timeout 30`: client timeout, expressed in seconds.
- `--force-ipv4` or `--force-ipv6`: bind outgoing connections to the selected address family. These options conflict with each other and with `--source-address`.
- `--source-address 192.0.2.10`: bind outgoing connections to this client-side IP address. The value must be a valid IP literal.
- `--option 'downloader.http.headers={"Accept":"image/webp,*/*"}'`: add validated HTTP headers to media download requests. Invalid names and values fail before processing inputs, and error messages do not echo values.
- `--sleep 0.15`: wait this many seconds before each media download. `--sleep 0.1-0.3` samples an inclusive range before each media download. Values must be finite and between 0 and 3600 seconds.
- `--option extractor.sleep-skip=0.12`: wait after a media file is skipped. The preview supports fixed or ranged duration values and global/category configuration precedence for this option.
- `--option extractor.sleep-extractor=0.12`: wait before handling each input URL, also accepting the fixed or ranged duration forms.
- `--option extractor.sleep-request=0.12`: reserve globally spaced and per-origin slots before each configured media HTTP attempt (including retries) and before extractor API calls that use the shared bounded JSON reader or an explicit request-pacing hook. Media requests use their URL origin; the shared JSON reader and Bluesky XRPC calls use their API endpoint origin; other explicit extractor hooks use the input URL origin. A positive `Retry-After` on a media 429/5xx response also cools down that origin for later paced requests. The duration supports fixed values and inclusive ranges and follows the normal global/category option lookup. Extractor request-hook coverage is still incomplete.

**Privacy:** Proxy URLs and custom header values may contain credentials. Avoid putting secrets in arguments or shell history: another local process may be able to inspect command-line arguments. Do not store credentials in public configuration files, scripts, issues or fixtures.

**Not yet upstream parity:** pacing all extractor API requests, proxy behavior across every transport, advanced TLS options, response diagnostics and complete reference networking options remain open.

**Verification:** [CI 39957860428](https://github.com/sguzman/galerith/actions/runs/39957860428) includes synthetic proxy/User-Agent, bounded sleep and date-based retry tests; no public origin is contacted. The address-selection and configured-header additions were verified with local loopback tests and the full local format, Clippy, test, roadmap and release-build gates; no CI run for those additions has been confirmed.
