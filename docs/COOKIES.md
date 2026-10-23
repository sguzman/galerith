# 🍪 Netscape cookie file import (preview)

The CLI supports `-C FILE` and `--cookies FILE` to import additional cookies from a Netscape/Mozilla cookie file, using Rust's HTTP cookie jar.

## Supported import fields

- Domain, subdomain scope, path, secure transport, expiry timestamp, name and value.
- `#HttpOnly_` records are recognized rather than mistaken for ordinary comments.
- Invalid/malformed entries, expired cookies and obvious unsafe cookie names/values are not imported.
- Cookies are attached automatically to matching extractor API calls and media downloads through the same HTTP client. Domain, path and TLS scope checks are handled by the cookie jar.
- Missing cookie files fail; secrets and source cookie contents are not printed to ordinary output.

## Important limits

This is **not** full upstream cookie compatibility. Browser database import, cookies-export, profile/keyring selection, source-fidelity verification for every Netscape dialect, advanced expiry handling, OAuth flows and site login implementations are outstanding.

Cookie files contain sensitive session credentials. Keep them outside the repository and do not commit or attach them to issues. Only synthetic fixtures are included in tests.

Proof: [GitHub Actions 39950715590](https://github.com/sguzman/galerith/actions/runs/39950715590), including cookie scopes and a native CLI request carrying a synthetic session cookie.
