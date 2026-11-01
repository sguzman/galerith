# ⚙️ Configuration support: incomplete preview

Galerith currently accepts explicitly supplied **JSON**, **YAML**, and **TOML** configurations:

- `-c FILE` / `--config FILE`: infer YAML/TOML from the filename extension; otherwise treat as JSON
- `--config-json FILE`, `--config-yaml FILE`, `--config-toml FILE`: choose the parser explicitly
- `-o KEY=VALUE` / `--option KEY=VALUE`: apply a dot-separated nested override, parsing JSON values when possible and otherwise treating them as strings

Current load order is all `-c` files, then all explicit JSON, YAML, TOML files in those groups, and finally `-o` overrides. This is a **limited implementation-specific precedence rule**, not a claim that flags are applied in the exact command-line order or that upstream precedence is fully reproduced.

Currently recognized options include `filename`, `extractor.filename`, `extractor.<category>.filename`, and `base-directory`. A global filename wins over category-specific or generic extractor filename settings. The CLI `-f` setting wins over configuration and `-d` overrides the configured base directory.

Filename formatting is a restricted subset: named fields, escaped braces, bounded numeric ID width, `!l`/`!u` lowercase and uppercase conversions, `!t` whitespace trimming, and `?prefix/suffix/` wrappers that add affixes only for non-empty values. Other conversions, field traversal, expressions, and conditional configuration maps are not implemented.

**Unimplemented:** automatic default config discovery, all site-level category/subcategory options, full config compatibility, conditional configuration maps and expressions, downloader-specific namespaces, config status/create/open commands, and a differential suite against pinned gallery-dl. Unknown options may be stored but are not necessarily acted on.

**Verification:** [CI 39952418847](https://github.com/sguzman/galerith/actions/runs/39952418847) includes local JSON/YAML/TOML merge and precedence fixtures.
