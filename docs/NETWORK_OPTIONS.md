# 🌐 HTTP client option preview

Galerith currently exposes **`-a` / `--user-agent`**, **`--proxy`**, **`--http-timeout`**, **`--force-ipv4` / `--force-ipv6`**, **`--source-address`**, and **`--sleep`**, **`--sleep-request`**, **`--sleep-extractor`**, and **`--sleep-skip`** as a fixed delay or inclusive range. The three scoped sleep flags override their matching configuration values. User-Agent, proxy, timeout, and address-family selection apply to the shared native Rust HTTP client. Media requests send `Accept: */*` and `Accept-Encoding: identity` to keep resumed byte offsets stable. `downloader.http.headers` applies extra headers to media transfers only. Object-valued `extractor.headers` and `extractor.<category>.headers` configure requests associated with that extractor, including media requests; category headers replace the shared header object.

- `-a 'MyAgent/1.0'`: send this explicit User-Agent on requests. Without it, the client identifies itself as Galerith with the package version.
- `extractor.user-agent` or `extractor.<category>.user-agent`: set an extractor-specific User-Agent string; category values take precedence and `"auto"` uses the global agent. An explicit `-a`/`--user-agent` overrides this setting. An explicit `User-Agent` in `extractor.headers` takes precedence over either setting, and a null header omits it.
- `--proxy http://127.0.0.1:8080`: route HTTP(S) traffic through the supplied proxy URL. A root `proxy` config value accepts one URL or an `{ "http": ..., "https": ... }` map; `--proxy` overrides that value, `extractor.proxy` or `extractor.<category>.proxy` overrides it for extractor requests and their media, and `downloader.http.proxy` takes precedence for media transfers. Schemes are added to scheme-less proxy addresses as `http://`. The CLI handles malformed proxy URLs with a generic error rather than printing the supplied credentials.
- `--http-timeout 30` or `--http-timeout 0.5`: client timeout, expressed as a positive finite number of seconds.
- `-o downloader.http.timeout=0.5`: override the timeout for media transfers only. An explicit `--http-timeout` or global `-o timeout=...` takes precedence; absent all three, the shared client keeps its 30-second default.
- `-o downloader.http.verify=false`: disable TLS certificate verification for media transfers only. A string path such as `-o 'downloader.http.verify="/path/to/ca-bundle.pem"'` uses that PEM certificate bundle as the trust store for media transfers. `extractor.verify` and `extractor.<category>.verify` accept the same boolean or CA bundle path for extractor requests and provide the media default; `downloader.http.verify` overrides them for media. `--no-check-certificate` takes precedence.
- `--force-ipv4` or `--force-ipv6`: bind outgoing connections to the selected address family. These options conflict with each other and with `--source-address`.
- `--source-address 192.0.2.10`: bind outgoing connections to this client-side IP address. The value must be a valid IP literal. `extractor.source-address` and `extractor.<category>.source-address` accept the same IP string; CLI source-address and family flags take precedence. Extractor config tuples with an explicit source port are not supported.
- `--option 'extractor.headers={"X-Client":"gallery"}'` or `--option 'extractor.e621.headers={"X-Client":"e621"}'`: set custom headers for extractor requests. Category-specific headers replace the shared `extractor.headers` object. Header names and string values are validated, and values are marked sensitive for diagnostics. Null values omit the named header, including `User-Agent`. Browser-profile header presets are not implemented.
- `--option 'downloader.http.headers={"Accept":"image/webp,*/*"}'`: add validated HTTP headers to media download requests. Invalid names and values fail before processing inputs, and error messages do not echo values.
- `-o downloader.http.retries=2`: set the media downloader's bounded retry count. An explicit `--retries` or global `-o retries=...` takes precedence; otherwise the default is 4. Negative and non-integer values are rejected.
- `-o downloader.http.mtime=false`: keep local modification time instead of applying the source `Last-Modified` header; global `-o mtime=...` and `--no-mtime` take precedence.
- `--write-pages` or `--write-pages=all`: save response bodies from the shared bounded JSON API reader, Hatena Blog public HTML requests, KHInsider album/track pages, Fuskator gallery/search requests, the Pixhost page reader, Catbox album pages, Imgbox image/gallery pages, ImgBB image/album pages, Imgth gallery pages, Imgpv image pages, Imgdrive/Imgtaxi/Imgwallet image pages, ImgPile profile API/post pages, Postimages public image pages, Soundgasm audio/user pages, Telegraph articles, Webmshare video pages, WallpaperCave pages, Comedy Wildlife Photography gallery pages, Desktopography site/exhibition/portfolio pages, Speaker Deck oEmbed/player pages, Fapachi post/listing pages, PhotoVogue photographer API pages, Keenspot comic pages, 8muses album pages, ImageChest gallery pages, and the grouped basic image-host HTML extractors as numbered `.txt` files in the current directory. `extractor.write-pages=true` or `extractor.<category>.write-pages=true` enables body-only dumps for that extractor when the CLI flag is absent; a category value takes precedence. Failed-status bodies are included only when they fit the configured response limit. `all` also records the request method and redacted URL, request-specific headers, configured User-Agent and extractor headers, response status and response headers. It redacts credentials, cookies, token-like headers, sensitive header values, URL user information, query strings, and redirect/referrer header values. Names contain no URL data; files use owner-only permissions on Unix. Other extractor HTTP transports do not yet use this diagnostic.
- `--limit-rate 500k` or `--limit-rate 500k-2M`: pace response-body writes at a fixed or per-download sampled rate. Values accept byte, k, m or g units; this does not yet establish full upstream rate-policy parity.
- `--sleep-retries 0.5`, `0.2-0.8`, `exp=0.5`, or `lin:1:30=0.5`: configure a fixed/ranged retry pause or a capped exponential/linear schedule. For a configured 429 without `Retry-After`, this delay is a lower bound alongside `--sleep-429` and the reference per-attempt minimum.
- `-o downloader.http.retry-codes=[429]` opts 429 responses into retry handling, matching gallery-dl's default (5xx responses retry automatically). `--sleep-429 2`, `1-3`, or `exp=1` sets a fixed/ranged or scheduled delay for configured 429 retries without `Retry-After`; the first retry waits at least one second, with the minimum increasing by one second per attempt. When unset, the reference default interval is 60 seconds. A supplied `Retry-After` value takes precedence, including `0` for an immediate retry.
- `--sleep 0.15`: wait this many seconds before each media download. `--sleep 0.1-0.3` samples an inclusive range before each media download. Values must be finite and between 0 and 3600 seconds.
- `--option extractor.sleep-skip=0.12`: wait after a media file is skipped. The preview supports fixed or ranged duration values and global/category configuration precedence for this option.
- `--option extractor.sleep-extractor=0.12`: wait before handling each input URL, also accepting the fixed or ranged duration forms.
- `--sleep-request` or `extractor.sleep-request`: reserve globally spaced request slots before paced extractor API calls. The shared JSON reader and Bluesky XRPC calls use their API endpoint origin; extractor hooks without an explicit endpoint use the input URL origin. This setting does not delay direct media downloads; use `--sleep` for those. A positive `Retry-After` on a media 429/5xx response also cools down that origin for later paced extractor requests. The duration supports fixed values and inclusive ranges, follows normal global/category option lookup, and the request-hook coverage remains incomplete.

**Privacy:** Proxy URLs and custom header values may contain credentials. Avoid putting secrets in arguments or shell history: another local process may be able to inspect command-line arguments. Do not store credentials in public configuration files, scripts, issues or fixtures.

`--write-pages` is opt-in and may save private response content. Review and remove the generated files when they are no longer needed.

Are.na channel details and paginated contents use the shared bounded JSON reader, so both API responses can be captured with the same body-only or redacted diagnostic modes.

ImagePond single-image pages and paginated album/user listings capture bounded HTML responses with those diagnostic modes as well.

ImageBam gallery pagination and its original-image pages are captured through the same bounded HTML diagnostics path.

Foriio work details and paginated user-work API responses preserve their request headers in the redacted diagnostic mode.

Architizer project and firm listing pages also capture bounded HTML bodies after the existing same-origin redirect check.

Ahottie album and tag listing requests capture bounded, same-origin HTML pages with query strings omitted from diagnostic URLs.

BBC programme and gallery pages use the same bounded diagnostic capture after enforcing the public-origin redirect guard.

Silverpic’s bounded single-image page is also saved when response diagnostics are enabled.

Fappic’s canonical image page uses the same opt-in bounded HTML response capture.

PixelDrain public file, album, and folder API JSON responses use the shared bounded diagnostic reader.

Gelbooru v0.2 XML/DAPI responses are bounded to 4 MiB and can be saved with redacted request/response diagnostics.

35photo image API, tag, user, and genre page requests use bounded diagnostic capture; AJAX requests retain their request-specific headers.

Kabeuchi profile pages and paginated form posts are captured with bounded bodies, redacted cookies, and their request methods and headers. Wallhaven's JSON API responses retain the API Accept header and require redirects to remain on the API origin. Lightroom share HTML and paginated JSON responses are captured after same-origin redirect validation. 4chan board indexes and thread JSON responses use the same bounded diagnostics path with the API origin enforced. Pictoa album pages and image pages capture bodies only after the canonical origin redirect guard succeeds. Imxto image POSTs and paginated gallery pages retain their request methods and headers; cross-origin redirect bodies are not dumped. Unsplash photo, search, user, and collection API responses use the bounded same-origin JSON diagnostics reader. S3ndpics post and paginated user API responses preserve the source-origin redirect guard and diagnostics. Turboimagehost image and album pages capture bounded HTML when the redirect remains on the request origin. Urlgalleries source and image pages use bounded, same-origin HTML diagnostics. Lexica's cursor-based POST search retains its JSON request, Accept, and redacted Referer headers in bounded response diagnostics.

2chan thread pages capture bounded HTML after their first-party origin guard. Lensdump user listing pages capture bounded HTML after the same origin validation.

Shortener HEAD redirect metadata and t.co refresh pages can also be captured; redirect targets remain redacted in diagnostic headers.

Koofr k00.fr short-link responses capture redirect metadata without following the redirect; the Location header remains redacted.

IMHentai tag and search listing pages capture bounded HTML after same-origin redirect validation.

Recursive URL-list responses can be captured after the pinned public address and no-redirect checks succeed; their response bodies remain bounded to 2 MiB.

4archive thread and board pages use bounded same-origin response diagnostics.

Imgur's authenticated public API responses keep the Client-ID value redacted while capturing bounded diagnostics.

Imgclick's public continuation form POST keeps its method and form content type in diagnostics without writing the form body.

2ch thread and board index JSON pages use bounded response diagnostics with source-origin redirects rejected.

Koofr shared-link JSON captures its custom request headers while redacting the Referer and password query.

Picazor user-file pages preserve their source Referer request while redacting it in diagnostics.

3dbooru's Behoimi JSON requests retain the pinned Referer and identity encoding headers while using bounded same-origin response diagnostics.

Danke manga series API responses use the bounded diagnostics reader after same-origin validation.

ComicVine gallery HTML and image API responses capture bounded pages after enforcing the original request origin.

500px expanded photo API diagnostics retain the required Origin header and same-origin redirect check.

ImageVenue public image pages are captured with bounded diagnostics after redirects remain on the requested source origin.

Jschan board and thread JSON responses use bounded diagnostics with source-origin redirects rejected.

Picstate public image pages use bounded diagnostics and reject redirects outside the requested origin.

SlideShare presentation pages capture bounded HTML after same-origin redirect validation.

Audiochan audio and user API responses retain their CORS request headers in bounded diagnostics, with first-party redirect validation.

Vanilla Rock gallery and post HTML responses use bounded diagnostics after same-origin redirect validation.

Listal public image and paginated people pages capture bounded HTML responses after same-origin redirect validation.

4chanarchives thread and board HTML pages now use bounded diagnostics after source-origin redirect validation.

Imgadult image pages preserve their public cookie request while capturing bounded diagnostics; the cookie value is redacted.

Lensdump album and image HTML responses use the shared bounded diagnostics reader with first-party origin validation.

Issuu publication HTML responses capture bounded diagnostics after same-origin redirect validation.

Issuu publisher listing pages also capture bounded diagnostics and retain the publisher origin guard.

Picarto API pagination captures bounded JSON responses after same-origin redirect validation.

Piczel user, gallery, and folder API JSON responses use bounded diagnostics with first-party origin enforcement.

2chen thread and board HTML pages use bounded diagnostics with site-origin redirects rejected.

Fapello post and paginated listing HTML capture bounded response diagnostics after same-origin validation.

**Not yet upstream parity:** pacing all extractor API requests, proxy behavior across every transport, advanced TLS options, diagnostics for transports outside the shared bounded JSON reader, browser-profile header presets and complete reference networking options remain open.

**Verification:** [CI 41336923648](https://github.com/sguzman/galerith/actions/runs/41336923648) passed format, Clippy, all-target tests, roadmap consistency, release build and the pinned-reference job. Network fixtures use loopback endpoints; no public origin is contacted by those tests.
