# 🗺️ Galerith MVP roadmap

**Source of truth:** [progress.json](progress.json). Percentages reflect accepted atomic checks, not production readiness. No unverified CI or live site testing is counted.

**Overall:** `████░░░░░░ 42%` across 56 atomic tasks. **Every milestone is part of the complete-port MVP.**

## 🧱 M0 — Identity, provenance, project controls — `███████░░░ 66%`
**Acceptance gate:** Pinned upstream reference, honest task ledger, CI-validated Rust core.

### 🧭 P0 — Source reference and identity — `████████░░ 75%`

- 📌 **T001** Pin upstream commit and module inventory — `███████░░░ 67%`
  - [x] Record upstream commit — docs/SOURCE_BASELINE.md
  - [x] Catalog Python extractor files — docs/port-matrix.json
  - [ ] Automate catalog drift detection
- 🎨 **T002** Assign OKLCH accent and README swatch — `███████░░░ 67%`
  - [x] Specify perceptual color — docs/COLOR.md
  - [x] Commit SVG and embed it — assets/galerith-color.svg; README.md
  - [ ] Verify accessible application contrast
- ⚖️ **T003** Establish upstream licensing and provenance boundary — `███████░░░ 67%`
  - [x] Preserve repository's initial MIT file — LICENSE
  - [x] Document GPL-2.0 upstream and independent implementation policy — docs/PROVENANCE.md
  - [ ] Review each imported/translated source before distribution
- 📊 **T004** Establish verifiable milestone/task progress — `██████████ 100%`
  - [x] Commit machine ledger — docs/progress.json
  - [x] Ship Rust renderer and generated roadmap — src/bin/galerith-roadmap.rs; docs/ROADMAP.md
  - [x] CI validates rendered files — GitHub Actions 39897837049: Rust format, clippy, unit/CLI tests, roadmap/matrix verification and release build passed

### 🦀 P1 — CLI foundation and quality gates — `██████░░░░ 56%`

- ⌨️ **T101** Native Rust binary and command parsing — `████████░░ 75%`
  - [x] Commit native CLI — src/main.rs; Cargo.toml
  - [x] Write offline CLI tests — tests/cli.rs
  - [x] Verify clean Linux CI — GitHub Actions 39897837049: Rust format, clippy, unit/CLI tests, roadmap/matrix verification and release build passed
  - [ ] Reach complete upstream argument parity
- 📥 **T102** URL inputs and input-file semantics — `█████░░░░░ 50%`
  - [x] Accept arguments and URL files — src/main.rs
  - [x] Support stdin — src/main.rs
  - [ ] Match input-file mutation/retry behavior
  - [ ] Differential CLI acceptance fixtures
- 🖨️ **T103** Output, simulation and extraction flags — `█████░░░░░ 50%`
  - [x] Implement basic URL/JSON/simulate flags — src/main.rs
  - [x] Test flags without networking — tests/cli.rs
  - [ ] Reproduce upstream JSON message protocol
  - [ ] Implement all print/list/diagnostics modes
- 📦 **T104** Release-safe packaging and documentation — `█████░░░░░ 50%`
  - [x] Document preview limitations — README.md
  - [x] Compile Linux release in CI — GitHub Actions 39897837049: Rust format, clippy, unit/CLI tests, roadmap/matrix verification and release build passed
  - [ ] Package distribution and checksums
  - [ ] Validate install on EndeavourOS

## 🌐 M1 — HTTP, storage and download parity — `█████░░░░░ 54%`
**Acceptance gate:** Resume, throttling, archive history, filename template and network behavior match reference.

### 📡 P2 — HTTP transfer engine — `██████░░░░ 58%`

- ⬇️ **T201** Stream media downloads — `████████░░ 75%`
  - [x] Implement chunked async HTTP writing — src/download.rs
  - [x] Add local HTTP fixture tests — tests/download_http.rs; GitHub Actions 39898967165 passed
  - [x] Verify interrupted/large downloads — GitHub Actions 39947298737: local HTTP resume, collision, multi-megabyte, retry and Danbooru API tests passed
  - [ ] Match upstream downloader behavior
- 🔄 **T202** Retry, backoff and rate control — `█████░░░░░ 50%`
  - [x] Provide bounded transient retries — src/download.rs
  - [ ] Honor Retry-After and timing policies
  - [ ] Implement per-host and global pacing
  - [x] Verify HTTP 429/5xx fixture matrix — GitHub Actions 39947298737: local HTTP resume, collision, multi-megabyte, retry and Danbooru API tests passed
- ⏯️ **T203** Resume, partial files and atomic completion — `████████░░ 75%`
  - [x] Write to .part then rename — src/download.rs
  - [x] Implement Range-based resumes — GitHub Actions 39947298737: local HTTP resume, collision, multi-megabyte, retry and Danbooru API tests passed
  - [ ] Preserve metadata/mtime, validate lengths
  - [x] Pass interruption and collision tests — GitHub Actions 39947298737: local HTTP resume, collision, multi-megabyte, retry and Danbooru API tests passed
- 🔌 **T204** HTTP configuration and proxy parity — `███░░░░░░░ 25%`
  - [x] Set request timeout — src/main.rs
  - [ ] Proxy, TLS, IPv4/IPv6 and user-agent
  - [ ] Request headers/response diagnostics
  - [ ] Differential network tests
- 🌐 **T205** User-agent and proxy transport preview — `█████░░░░░ 50%`
  - [x] Accept upstream -a/--user-agent and --proxy flags — src/main.rs; GitHub Actions 39956648701 succeeded (format, Clippy, offline tests, release build); 4chan board fixtures also passed run 39956254401
  - [x] Verify proxy request routing and custom User-Agent offline — tests/http_options_cli.rs; GitHub Actions 39956648701 succeeded (format, Clippy, offline tests, release build); 4chan board fixtures also passed run 39956254401
  - [ ] Implement full proxy authentication, TLS and address-family selection
  - [ ] Compare upstream HTTP configuration behavior
- ⏱️ **T206** Retry-After dates and constant download pacing preview — `████████░░ 75%`
  - [x] Bound numeric Retry-After and support HTTP-date values — src/download.rs; GitHub Actions 39957617755 passed
  - [x] Test date handling, past values and invalid retry hints offline — src/download.rs unit tests; GitHub Actions 39957617755 passed
  - [x] Support constant --sleep and verify local HTTP request delay — src/main.rs; tests/sleep_cli.rs; GitHub Actions 39957860428 passed
  - [ ] Implement reference range-based sleeps, skip/extractor/request timing and pacing policy

### 🗃️ P3 — Paths, archive and persistence — `█████░░░░░ 50%`

- 🗂️ **T301** Safe names and format templates — `████░░░░░░ 40%`
  - [x] Sanitize a direct file basename — src/lib.rs
  - [ ] Format field interpolation and conditionals
  - [ ] Platform filename rules and normalization
  - [ ] Reference-compatible filesystem tests
  - [x] Verify safe named-field, escaped brace and ID-width format subset — src/filename.rs; tests/cli.rs; GitHub Actions 39954702418
- 🧾 **T302** SQLite archive, cache and duplicate rules — `███░░░░░░░ 25%`
  - [x] Archive schemas and durable writes — src/archive.rs; tests/archive_cli.rs; GitHub Actions 39948367906 (successful SQLite reopen, CLI archive round-trip)
  - [ ] Archive ID semantics across extractors
  - [ ] Cache expiry/locking/recovery
  - [ ] Migration and retry fixtures
- 📋 **T303** Metadata sidecars and extension handling — `████░░░░░░ 40%`
  - [x] Metadata model for extractors — src/lib.rs
  - [ ] Sidecar JSON/text/original metadata
  - [ ] Media extensions and MIME matching
  - [ ] Golden sidecar comparison tests
  - [x] Verify atomic per-media redacted JSON metadata preview — src/postprocess.rs; tests/metadata_cli.rs; GitHub Actions 39954702418
- 🗄️ **T304** Download history and collision semantics — `████████░░ 75%`
  - [x] Existing files skip by default — src/download.rs
  - [x] Safe overwrites and name conflict policy — GitHub Actions 39947298737: local HTTP resume, collision, multi-megabyte, retry and Danbooru API tests passed
  - [ ] Dedup across URLs/sites
  - [x] Regression tests for parallel tasks — GitHub Actions 39947298737: local HTTP resume, collision, multi-megabyte, retry and Danbooru API tests passed
- 🧵 **T305** Safe field-based filename formatting preview — `███████░░░ 67%`
  - [x] Named fields, padding and escaped braces — src/filename.rs; tests/cli.rs; GitHub Actions 39954702418: format, Clippy, offline Rust tests and release build succeeded
  - [x] Reject dangerous or unsupported filename templates — src/filename.rs; GitHub Actions 39954702418: format, Clippy, offline Rust tests and release build succeeded
  - [ ] Full upstream conditional and transformation language
- 📄 **T306** Native JSON sidecar preview — `█████░░░░░ 50%`
  - [x] Sidecars for downloaded or existing media — src/postprocess.rs; tests/metadata_cli.rs; GitHub Actions 39954702418: format, Clippy, offline Rust tests and release build succeeded
  - [x] Token redaction and atomic sidecar rename — src/postprocess.rs; tests/metadata_cli.rs; GitHub Actions 39954702418: format, Clippy, offline Rust tests and release build succeeded
  - [ ] Full upstream metadata and output formats
  - [ ] Reference golden metadata comparisons

## 🧩 M2 — Extractor framework and site families — `█████░░░░░ 47%`
**Acceptance gate:** Extensible Rust extractor contracts, reusable site families, recursive pagination and dispatch.

### 🧬 P4 — Extractor architecture — `███░░░░░░░ 34%`

- 🧭 **T401** URL dispatch and category routing — `███░░░░░░░ 25%`
  - [x] Implement conservative direct-media matching — src/lib.rs
  - [ ] Introduce registries and priority rules
  - [ ] Support child extractors and queues
  - [ ] Differential URL-match fixtures
- 📨 **T402** Structured extractor message protocol — `███░░░░░░░ 25%`
  - [x] Represent a media item with source and filename — src/lib.rs
  - [ ] Directory metadata and file message variants
  - [ ] Deferred URL resolution and redirects
  - [ ] Reference message fixtures
- 🔎 **T403** Pagination, filtering and selectors — `██░░░░░░░░ 20%`
  - [ ] Iterative pagination and stop conditions
  - [ ] Ranges, filters and predicates
  - [ ] Order selection and deduplication
  - [ ] Page boundary tests
  - [x] Verify one-based single/range/slice selector subset — src/range.rs; tests/cli.rs; GitHub Actions 39954702418
- 🧪 **T404** Shared extractor harness — `█████░░░░░ 50%`
  - [x] Deterministic mocked HTTP responses — GitHub Actions 39947298737: local HTTP resume, collision, multi-megabyte, retry and Danbooru API tests passed
  - [ ] Fixture discovery and golden comparisons
  - [x] Per-site contract assertions — GitHub Actions 39947298737: local HTTP resume, collision, multi-megabyte, retry and Danbooru API tests passed
  - [ ] CI compatibility matrix
- 🔢 **T405** Media index selection preview — `█████░░░░░ 50%`
  - [x] Parse 1-based index, range and step syntax — src/range.rs; GitHub Actions 39954702418: format, Clippy, offline Rust tests and release build succeeded
  - [x] CLI index filtering and error regression tests — src/main.rs; tests/cli.rs; GitHub Actions 39954702418: format, Clippy, offline Rust tests and release build succeeded
  - [ ] Stop paging before unnecessary extractor requests
  - [ ] Full post and child range parity

### 🧱 P5 — Shared website families — `█████░░░░░ 52%`

- 🖼️ **T501** Imageboard and booru API families — `░░░░░░░░░░ 0%`
  - [ ] Implement generic booru API contracts
  - [ ] Port specialized API variants
  - [ ] Metadata and pagination fixtures
  - [ ] Differential sample gallery acceptance
- 📚 **T502** Manga and chapter readers — `░░░░░░░░░░ 0%`
  - [ ] Chapter/panel enumeration
  - [ ] Ordering and next-page logic
  - [ ] Image resolution and fallback
  - [ ] Fixture parity
- 🧑‍🎨 **T503** Social, creator and authenticated gallery families — `░░░░░░░░░░ 0%`
  - [ ] Account/profile/post abstractions
  - [ ] Nested albums/media handling
  - [ ] Pagination and rate rules
  - [ ] Compatibility fixtures
- 🌍 **T504** Generic and direct URL behavior — `███░░░░░░░ 25%`
  - [x] Recognize direct HTTP media URL — src/lib.rs
  - [ ] Match upstream directlink URL matching
  - [ ] Generic gallery discovery and crawling
  - [ ] Full reference fixtures
- 🖌️ **T505** First Danbooru API-backed post and tag extractor — `████████░░ 75%`
  - [x] Match Danbooru post and tag search URLs — src/extractor/danbooru.rs; src/extractor/mod.rs
  - [x] Parse JSON media records with safe filenames — src/extractor/danbooru.rs
  - [x] Verify synthetic API parser and CLI tests in Linux CI — GitHub Actions 39898967165: test and release build passed
  - [ ] Reproduce complete upstream Danbooru semantics and authentication
- 🦊 **T506** Initial e621, e926 and e6ai JSON post and tag extraction — `████████░░ 75%`
  - [x] Match only documented post and tag-search URL subsets — src/extractor/e621.rs; src/extractor/mod.rs
  - [x] Parse media and skip unavailable URLs safely — src/extractor/e621.rs local JSON parser fixtures
  - [x] Verify post and paginated tag-search API fixtures in Linux CI — GitHub Actions 39949514147 (format, Clippy, synthetic API tests, release build)
  - [ ] Implement upstream pools, artist, favorite, popular, account and metadata behavior
- 🌸 **T507** Initial Gelbooru JSON API post and tag-search extraction — `████████░░ 75%`
  - [x] Recognize post and tag searches with query-order independence — src/extractor/gelbooru.rs URL matching and parser tests
  - [x] Resolve synthetic Gelbooru JSON post API media records — src/extractor/gelbooru.rs local API fixture
  - [x] Verify registered extractor and unit suite in Linux CI — GitHub Actions 39954702418
  - [ ] Reproduce full pools, favorites, redirection, pagination and metadata semantics
- 🧵 **T508** 4chan thread and bounded board extractor preview — `████████░░ 75%`
  - [x] Recognize public 4chan and 4channel thread/board URLs — src/extractor/fourchan.rs; GitHub Actions 39956648701 succeeded (format, Clippy, offline tests, release build); 4chan board fixtures also passed run 39956254401
  - [x] Parse thread JSON attachments with safe file names — src/extractor/fourchan.rs synthetic fixtures; GitHub Actions 39956648701 succeeded (format, Clippy, offline tests, release build); 4chan board fixtures also passed run 39956254401
  - [x] Expand board pages into threads and deduplicate IDs — src/extractor/fourchan.rs local API fixtures; GitHub Actions 39956648701 succeeded (format, Clippy, offline tests, release build); 4chan board fixtures also passed run 39956254401
  - [ ] Reproduce full upstream directory, archive, metadata and edge-case semantics
- 🧬 **T509** Gelbooru v0.2 XML API family preview — `████████░░ 75%`
  - [x] Recognize supported post and tag URLs for five pinned API hosts — src/extractor/gelbooru_v02.rs; GitHub Actions 39959270238 passed
  - [x] Parse XML post metadata and reject unsafe media schemes — src/extractor/gelbooru_v02.rs unit fixtures; GitHub Actions 39959270238 passed
  - [x] Verify synthetic HTTP post and bounded tag-search DAPI requests — src/extractor/gelbooru_v02.rs local TCP fixtures; GitHub Actions 39959270238 passed
  - [ ] Port complete pools, favorites, auth, fallback URLs, tags and reference metadata
- 🎏 **T510** Moebooru JSON family post/tag/pool extractor preview — `████████░░ 75%`
  - [x] Route four upstream site families and three supported URL shapes — src/extractor/moebooru.rs; CI 39960525151
  - [x] Parse synthetic JSON media and validate metadata-driven filenames — src/extractor/moebooru.rs synthetic JSON unit fixtures; CI 39960525151
  - [x] Verify post and pool API queries with localhost HTTP fixtures — src/extractor/moebooru.rs synthetic server tests; CI 39960525151
  - [ ] Reproduce popular, complete pool metadata and upstream auth/format semantics
- 🦄 **T511** Philomena JSON API images, searches and galleries preview — `████████░░ 75%`
  - [x] Match only pinned Philomena host and supported routes — src/extractor/philomena.rs; GitHub Actions 39961510804
  - [x] Parse representations and fallback media with safe output names — src/extractor/philomena.rs JSON parser fixtures; GitHub Actions 39961510804
  - [x] Verify synthetic image and gallery search API requests — src/extractor/philomena.rs local HTTP fixtures; GitHub Actions 39961510804
  - [ ] Implement full tags, search filters, auth, SVG rules and upstream metadata
- 🗄️ **T512** FoolFuuka archive family thread media preview — `████████░░ 75%`
  - [x] Match nine pinned archive hosts and strict thread URL forms — src/extractor/foolfuuka.rs; CI 39962347749
  - [x] Parse ordered replies and validate attachment URLs/filenames — src/extractor/foolfuuka.rs synthetic archive parser fixtures; CI 39962347749
  - [x] Exercise thread JSON API using localhost HTTP fixture — src/extractor/foolfuuka.rs local API test; CI 39962347749
  - [ ] Complete archive-specific redirects, searches, board/gallery routes and metadata

## 🕸️ M3 — Full upstream extractor catalog — `█░░░░░░░░░ 12%`
**Acceptance gate:** All pinned upstream extractor modules have independently accepted functionality; 0 missing supported sites.

### 📚 P6 — Module-by-module port — `█░░░░░░░░░ 8%`

- 📐 **T601** Track every upstream extractor source module — `███░░░░░░░ 25%`
  - [x] Commit pinned inventory — docs/port-matrix.json
  - [ ] Map each class/category within modules
  - [ ] Report implemented/tested parity counts
  - [ ] Accept all modules with source-referenced evidence
- 🔬 **T602** Port every site-specific module — `░░░░░░░░░░ 0%`
  - [ ] Implement all catalogued extractor sites
  - [ ] Exercise all documented URL variants
  - [ ] Validate pagination and metadata
  - [ ] No unresolved module parity gaps
- 🛠️ **T603** Port every helper and special extractor — `░░░░░░░░░░ 0%`
  - [ ] Classify helper utilities separately
  - [ ] Port intermediary URL resolvers
  - [ ] Recursive and no-op behaviors
  - [ ] Test all helper chains

### 🔄 P7 — Upstream drift and support claims — `██░░░░░░░░ 17%`

- 📆 **T701** Detect upstream extractor changes — `███░░░░░░░ 33%`
  - [x] Fetch pinned catalog for baseline — docs/SOURCE_BASELINE.md
  - [ ] Automate subsequent baseline comparisons
  - [ ] Assess removed/renamed site definitions
- 🧪 **T702** Catalog conformance evidence — `░░░░░░░░░░ 0%`
  - [ ] Stable per-site test manifest
  - [ ] Mock offline tests for core variants
  - [ ] Opt-in live site probes with rate policies
  - [ ] No unsupported claimed-complete category

## 🔐 M4 — Authentication, configuration, processing — `██░░░░░░░░ 21%`
**Acceptance gate:** Cookies, login, config dialects, pipeline transformations, and external integrations reach upstream parity.

### 🪪 P8 — Sessions, cookies and auth — `██░░░░░░░░ 24%`

- 🍪 **T801** Cookie jars and browser imports — `███░░░░░░░ 25%`
  - [x] Netscape cookie files — src/cookies.rs; tests/cookie_cli.rs; GitHub Actions 39950715590 (domain/path/TLS scopes and authenticated HTTP integration)
  - [ ] Browser store imports
  - [ ] Domain/path/security semantics
  - [ ] Fixture comparisons
- 🔑 **T802** Tokens, OAuth and account sessions — `░░░░░░░░░░ 0%`
  - [ ] Authentication/session API
  - [ ] Token refresh and secret storage
  - [ ] Site-specific login challenges
  - [ ] Authenticated extractor tests
- 🛡️ **T803** Safe boundaries and telemetry — `██░░░░░░░░ 20%`
  - [ ] No secrets in logs
  - [ ] Redirect/cookie leak prevention
  - [ ] Credential file permissions
  - [ ] Adversarial fixture verification
  - [x] Mask query credentials and passwords in CLI error output — src/privacy.rs; tests/cli.rs; GitHub Actions 39954702418
- 🔒 **T804** Signed URL diagnostic redaction preview — `█████░░░░░ 50%`
  - [x] Strip query tokens and URL credentials in errors — src/privacy.rs; GitHub Actions 39954702418: format, Clippy, offline Rust tests and release build succeeded
  - [x] Test signed URL redaction in CLI failures — tests/cli.rs; GitHub Actions 39954702418: format, Clippy, offline Rust tests and release build succeeded
  - [ ] All remaining diagnostic paths audited
  - [ ] Redirect/cookie leakage adversarial fixtures

### 🛠️ P9 — Config and post-processing — `██░░░░░░░░ 19%`

- ⚙️ **T901** Upstream configuration file semantics — `███░░░░░░░ 25%`
  - [x] JSON/YAML/TOML supported — src/config.rs; tests/config_cli.rs; GitHub Actions 39954702418: format, Clippy, offline Rust tests and release build succeeded
  - [ ] Precedence/overrides/compat options
  - [ ] Extractor/downloader namespace
  - [ ] Differential config fixtures
- 🔧 **T902** Postprocessors and actions — `░░░░░░░░░░ 0%`
  - [ ] Metadata/rename/zip processing
  - [ ] External command workflows
  - [ ] Filters and event hooks
  - [ ] Golden processed output tests
- 🎞️ **T903** Media and external downloader integrations — `░░░░░░░░░░ 0%`
  - [ ] Native media support
  - [ ] External downloader adapters
  - [ ] Format selection/variant metadata
  - [ ] End-to-end integration fixtures
- 🧾 **T904** Explicit config overlays and precedence preview — `█████░░░░░ 50%`
  - [x] JSON YAML TOML and typed overrides — src/config.rs; tests/config_cli.rs; GitHub Actions 39954702418: format, Clippy, offline Rust tests and release build succeeded
  - [x] Nested extractor config and CLI override precedence — src/config.rs; tests/config_cli.rs; GitHub Actions 39954702418: format, Clippy, offline Rust tests and release build succeeded
  - [ ] Automatic config lookup and all upstream namespaces
  - [ ] Upstream differential configuration tests

## 🚀 M5 — Conformance, release and MVP inauguration — `███░░░░░░░ 26%`
**Acceptance gate:** Complete feature and extractor parity verified on Linux, then stable MVP versioned release.

### 🧪 P10 — Differential quality assurance — `███░░░░░░░ 33%`

- 🔍 **T1001** Pin and run comparison oracle — `███░░░░░░░ 25%`
  - [x] Document upstream baseline — docs/SOURCE_BASELINE.md
  - [ ] Automated golden CLI corpus
  - [ ] Deterministic extractor comparisons
  - [ ] Signed reproducible reports
- 🧫 **T1002** Stress, errors and recovery — `█████░░░░░ 50%`
  - [x] Local fixture server — tests/download_http.rs; GitHub Actions 39898967165 passed
  - [x] Retry/resume/fault injection — GitHub Actions 39947298737: local HTTP resume, collision, multi-megabyte, retry and Danbooru API tests passed
  - [ ] Large galleries and concurrent ordering
  - [ ] Performance parity limits
- 📋 **T1003** CLI, config, extractor feature ledger — `███░░░░░░░ 25%`
  - [x] Document acceptance standard — docs/ACCEPTANCE.md
  - [ ] Feature-by-feature signed-off matrix
  - [ ] All catalog rows complete
  - [ ] No known failed acceptance gates

### 🏁 P11 — Release graduation — `██░░░░░░░░ 19%`

- 🐧 **T1101** EndeavourOS and portable Linux acceptance — `███░░░░░░░ 33%`
  - [x] Linux CI test jobs — GitHub Actions 39897837049: Rust format, clippy, unit/CLI tests, roadmap/matrix verification and release build passed
  - [ ] Native EndeavourOS install and runtime smoke
  - [ ] Long-run archive validation
- 📦 **T1102** Build and distribute native binaries — `███░░░░░░░ 25%`
  - [x] Release-mode build — GitHub Actions 39897837049: Rust format, clippy, unit/CLI tests, roadmap/matrix verification and release build passed
  - [ ] Versioned Linux artifact/checksums
  - [ ] Document repeatable installation
  - [ ] Supply-chain/license review
- 🎯 **T1103** Inaugurate complete-port MVP — `░░░░░░░░░░ 0%`
  - [ ] Every MVP milestone acceptance gate passed
  - [ ] Upstream parity report reviewed
  - [ ] All supported-site rows complete
  - [ ] Stable release tagged and documented

## 🏁 MVP condition

**Not reached.** Every milestone and every pinned catalog row must be complete with verification evidence. An intermediate working CLI is not an MVP.
