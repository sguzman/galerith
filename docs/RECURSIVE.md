# 🔁 Recursive public and local URL lists — partial port

**Pinned upstream:** [gallery-dl recursive.py](https://github.com/mikf/gallery-dl/blob/7f226f74108501c8ac3120548370f68f19e6f9b3/gallery_dl/extractor/recursive.py). This is an independent, security-bounded partial Rust implementation; it is **not full upstream recursive behavior**.

Accepted inputs include explicit `r:file:///ABSOLUTE/PATH` and `recursive:file:///ABSOLUTE/PATH` UTF-8 local files, remote public HTTP(S) addresses with `r:` or `recursive:`, and explicitly selected public FQDN source paths without a leading scheme (defaulting to HTTPS). There is no implicit filesystem discovery, arbitrary command execution or eager media download.

Local files and remote HTTP responses are each capped at **2 MiB**. The pinned URL scanning order is preserved and duplicates are not silently removed; at most **128** safe child HTTP(S) queue entries are emitted, with `category=recursive`, `subcategory=file` and a 1-based ordinal. The separately bounded native queue/job engine controls traversal or downloading of any child and follows its own registry and user opt-in policies.

Remote sources require a public DNS domain, disallow credentials, explicit ports and fragments, and resolve through DNS with address pinning only after rejecting special-use/private addresses. Network requests disable automatic redirects, proxies and long-running responses. HTTP redirects, oversized responses and malformed UTF-8 fail closed. Tests cover local limits, ordered duplicate queues, host and IP validation, plus actual local HTTP success, redirect and oversized-response fixtures.

**Evidence:** [CI 40208717387](https://github.com/sguzman/galerith/actions/runs/40208717387) passed formatting, strict Clippy, roadmap consistency, tests, release build and the separate pinned upstream directlink comparison oracle. This is offline/synthetic acceptance, **not live-site compatibility**.

**Remaining parity:** upstream unbounded file and HTTP sources, all `ensure_http_scheme` variations, unrestricted URL regex queue behavior, arbitrary host handling, complete gallery-dl child queue propagation and live differential parity. Those gaps are intentional hard restrictions of the current preview, not completed checks.
