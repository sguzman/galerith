# 🧬 Gelbooru Beta 0.2 XML API preview

The pinned gallery-dl reference has a shared XML/DAPI extractor family for `rule34.xxx`, `safebooru.org`, `tbib.org`, `hypnohub.net`, and `xbooru.com`. Galerith independently implements the **post and tag-search subset only**.

- Recognizes explicit HTTPS `/index.php?page=post&s=view&id=...` and `/index.php?page=post&s=list&tags=...` URLs, regardless of query parameter order.
- Queries the Gelbooru Beta 0.2 XML post API. Rule34 has a separate `api.rule34.xxx` API host.
- Parses `<posts><post ... /></posts>` XML, extracting safe HTTP(S) media URLs, numeric post IDs and a conservative media extension.
- Tag searches use bounded `--max-pages` traversal and stop when the returned page contains fewer than the current request limit.
- Extractor categories remain distinct across the five websites for download archive scoping.

**Not complete:** Upstream pools, favorites, authenticated endpoints, site-specific media fallback URLs, tags/notes/metadata, pagination edge cases, and source-compatible filename/archive policies remain to be ported. Real-site integration and parity have not been tested.

**Synthetic verification:** [CI 39959270238](https://github.com/sguzman/galerith/actions/runs/39959270238) runs exact-host URL, XML parsing, post API and tag-search request fixtures without public network traffic.
