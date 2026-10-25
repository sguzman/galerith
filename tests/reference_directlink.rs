//! Offline differential against the pinned upstream gallery-dl directlink
//! extractor. Upstream Python is executed ONLY by the isolated CI oracle job;
//! Galerith and its runtime remain Rust-native.
use serde_json::{json, Value};
use std::{env, path::Path, process::Command};

const PINNED: &str = "7f226f74108501c8ac3120548370f68f19e6f9b3";
const URLS: &[&str] = &[
    "https://example.org/photo.png",
    "https://assets.example.org/arts/Poster%20Art.JPEG?download=1#first",
    "https://cdn.example.org/one/two/cover%20art.webp?size=large",
    "http://images.example.org/a/track%20one.mp3?source=public",
    "https://static.example.org/media/image.avif",
    "https://downloads.example.org/books/book.pdf",
    "https://cdn.example.org/archive/media.7z?mirror=one",
    "https://videos.example.org/clips/preview.webm?quality=original#top",
    "https://audio.example.org/tracks/recording.opus",
    "https://scan.example.org/photos/art.heic",
    "https://assets.example.org/work/old.PSD",
    "https://media.example.org/print/photo.JPE",
    "https://assets.example.org/galleries/%E9%9F%B3%E6%A5%BD.jpg?lang=ja",
];

fn spawn(mut command: Command, label: &str) -> String {
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("{label}: process launch failed: {error}"));
    assert!(
        output.status.success(),
        "{label}: exit {:?}: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("reference and native output must be UTF-8")
}

fn native_events(url: &str) -> Vec<Value> {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_galerith"));
    cmd.args(["--dump-messages", url]);
    spawn(cmd, "native Galerith")
        .lines()
        .map(|line| serde_json::from_str(line).expect("native typed event JSON"))
        .collect()
}

fn pinned_events(root: &Path, url: &str) -> Vec<Value> {
    let mut cmd = Command::new("python3");
    cmd.args([
        "-m",
        "gallery_dl",
        "--config-ignore",
        "--no-input",
        "--dump-json",
        url,
    ]);
    cmd.env("PYTHONPATH", root);
    cmd.env("PYTHONNOUSERSITE", "1");
    cmd.env("PYTHONDONTWRITEBYTECODE", "1");
    serde_json::from_str(&spawn(cmd, "pinned gallery-dl")).expect("upstream typed-message JSON")
}

fn same_field(native: &Value, reference: &Value, field: &str, label: &str) {
    assert_eq!(
        native.get(field).and_then(Value::as_str),
        reference.get(field).and_then(Value::as_str),
        "{label}: upstream field {field}"
    );
}

#[test]
fn live_pinned_runtime_directlink_events_match_on_source_metadata() {
    // This test is intentionally optional for ordinary cargo test runs.
    // Dedicated CI sets both variables and must fail if the pin is missing.
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("CI reference oracle must supply the pinned upstream directory");
    let root = Path::new(&root);
    let version = std::fs::read_to_string(root.join("gallery_dl/version.py"))
        .expect("pinned upstream source missing");
    assert!(!version.is_empty());
    let commit = env::var("GALERITH_PINNED_REFERENCE_SHA")
        .expect("CI must declare expected upstream git revision");
    assert_eq!(commit, PINNED, "upstream pin changed without acceptance");
    let mut verified_cases = Vec::with_capacity(URLS.len());
    let db_dir = env::temp_dir().join(format!(
        "galerith-pinned-archive-oracle-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&db_dir);
    std::fs::create_dir_all(&db_dir).unwrap();
    for (case_index, url) in URLS.iter().enumerate() {
        let rust = native_events(url);
        let source = pinned_events(root, url);
        assert_eq!(rust.len(), 2, "{url}: native event count");
        assert_eq!(source.len(), 2, "{url}: upstream event count");
        for (ordinal, (native, upstream)) in rust.iter().zip(&source).enumerate() {
            let tuple = upstream.as_array().expect("upstream message tuple");
            let kind = if ordinal == 0 { 2 } else { 3 };
            assert_eq!(tuple.first().and_then(Value::as_i64), Some(kind));
            assert_eq!(
                native["type"],
                if ordinal == 0 { "directory" } else { "url" }
            );
            if ordinal == 1 {
                assert_eq!(native["url"], tuple[1], "{url}: original URL identity");
            }
            let reference = tuple.last().expect("pinned source metadata");
            let metadata = &native["metadata"];
            for key in [
                "category",
                "subcategory",
                "domain",
                "path",
                "extension",
                "query",
                "fragment",
            ] {
                same_field(metadata, reference, key, url);
            }
            assert_eq!(
                metadata["stem"], reference["filename"],
                "{url}: pinned filename stem"
            );
        }
        // Upstream formats the extension in lowercase, whereas the native
        // flat filename preview preserves the source extension's spelling.
        // Compare the upstream extension and stem independently above;
        // do not conceal this outstanding filename-policy divergence.
        let parsed = url::Url::parse(url).expect("verified test URL");
        let basename = parsed
            .path_segments()
            .and_then(|mut segments| segments.next_back())
            .expect("directlink basename");
        let basename = percent_encoding::percent_decode_str(basename)
            .decode_utf8()
            .expect("UTF-8 fixture filename");
        assert_eq!(
            rust[0]["metadata"]["filename"],
            basename.as_ref(),
            "{url}: native flat filename policy"
        );
        // Derive the archive entry from the *actual pinned upstream*
        // extractor metadata. Then verify that Galerith recognizes it via
        // a gallery-dl-shaped SQLite archive without modifying that table.
        let source_fields = source[0]
            .as_array()
            .and_then(|tuple| tuple.last())
            .expect("pinned directory metadata");
        let upstream_key = format!(
            "directlink{}/{}/{}.{}",
            source_fields["domain"].as_str().unwrap(),
            source_fields["path"].as_str().unwrap(),
            source_fields["filename"].as_str().unwrap(),
            source_fields["extension"].as_str().unwrap()
        );
        let archive_path = db_dir.join(format!("case-{case_index}.sqlite"));
        let connection = rusqlite::Connection::open(&archive_path).unwrap();
        connection
            .execute_batch("CREATE TABLE archive(entry TEXT PRIMARY KEY) WITHOUT ROWID;")
            .unwrap();
        connection
            .execute(
                "INSERT INTO archive(entry) VALUES (?1)",
                rusqlite::params![upstream_key],
            )
            .unwrap();
        drop(connection);
        let upstream_bytes = std::fs::read(&archive_path).unwrap();
        let native_path = db_dir.join(format!("native-{case_index}.sqlite"));
        let native_archive =
            galerith::archive::Archive::open_with_reference(&native_path, Some(&archive_path))
                .unwrap();
        let item = galerith::extract_direct_media(url).unwrap();
        assert!(
            native_archive.contains(&item).unwrap(),
            "{url}: pinned directlink archive key must be recognized"
        );
        let unchanged: i64 = rusqlite::Connection::open(&archive_path)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM archive", [], |row| row.get(0))
            .unwrap();
        assert_eq!(unchanged, 1, "foreign archive must remain untouched");
        assert_eq!(
            native_archive.import_reference().unwrap(),
            (1, 1),
            "{url}: import must recognize exactly one upstream archive key"
        );
        drop(native_archive);
        assert_eq!(
            std::fs::read(&archive_path).unwrap(),
            upstream_bytes,
            "{url}: external upstream archive bytes must not change"
        );
        let reopened = galerith::archive::Archive::open(&native_path).unwrap();
        assert!(
            reopened.contains(&item).unwrap(),
            "{url}: imported key must work without the upstream connection"
        );
        drop(reopened);
        let native_bytes = std::fs::read(&native_path).unwrap();
        assert!(
            !native_bytes
                .windows(upstream_key.len())
                .any(|part| part == upstream_key.as_bytes()),
            "{url}: imported archive must not persist plaintext upstream IDs"
        );

        // Upstream sets archive-prefix="" when archive-table is explicitly
        // configured. Differential-check that site-independent directlink
        // keys from the actual pinned source retain their meaning after
        // safe, category-scoped native import.
        let bare_key = upstream_key
            .strip_prefix("directlink")
            .expect("pinned category prefix");
        let custom_path = db_dir.join(format!("custom-{case_index}.sqlite"));
        let custom_source = rusqlite::Connection::open(&custom_path).unwrap();
        custom_source
            .execute_batch("CREATE TABLE named (entry TEXT PRIMARY KEY) WITHOUT ROWID;")
            .unwrap();
        custom_source
            .execute(
                "INSERT INTO named (entry) VALUES (?1)",
                rusqlite::params![bare_key],
            )
            .unwrap();
        drop(custom_source);
        let source_bytes = std::fs::read(&custom_path).unwrap();
        let custom_native = db_dir.join(format!("custom-native-{case_index}.sqlite"));
        let named = galerith::archive::Archive::open_with_reference_options(
            &custom_native,
            Some(&custom_path),
            Some("named"),
            None,
        )
        .unwrap();
        assert!(
            named.contains(&item).unwrap(),
            "{url}: named table default must omit the category prefix"
        );
        assert_eq!(named.import_reference().unwrap(), (1, 1));
        drop(named);
        assert_eq!(std::fs::read(&custom_path).unwrap(), source_bytes);
        let detached = galerith::archive::Archive::open(&custom_native).unwrap();
        assert!(
            detached.contains(&item).unwrap(),
            "{url}: named table imported key must work offline"
        );
        drop(detached);

        verified_cases.push(json!({
            "url": url,
            "reference_events": source,
            "native_events": rust,
            "status": "source_fields_and_readonly_archive_match",
            "archive_key_compatible": true,
            "external_archive_read_only": true,
            "offline_migration_recognized": true,
            "named_table_prefix_semantics": "default_empty_verified",
            "known_divergence": "upstream formatted extension is lowercase; native flat filename retains original extension case"
        }));
    }
    std::fs::remove_dir_all(&db_dir).unwrap();
    if let Ok(report_path) = env::var("GALERITH_ORACLE_REPORT_PATH") {
        let path = Path::new(&report_path);
        std::fs::create_dir_all(path.parent().expect("oracle report parent"))
            .expect("create oracle report directory");
        let report = json!({
            "schema_version": 1,
            "upstream_commit": PINNED,
            "scope": "pinned-directlink-offline-source-metadata",
            "cases": verified_cases,
            "limitations": [
                "Source metadata and default directlink archive read-compatibility only, not general filename/archive semantics",
                "No non-directlink extractor or live source website was contacted",
                "Only repository CI executes the original upstream Python interpreter"
            ]
        });
        std::fs::write(
            path,
            format!("{}\n", serde_json::to_string_pretty(&report).unwrap()),
        )
        .expect("write pinned reference comparison evidence");
    }
}

#[test]
fn pinned_philomena_archive_format_is_the_image_id() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for archive differential");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "from gallery_dl.extractor.philomena import PhilomenaExtractor; print(PhilomenaExtractor.archive_fmt)",
    ]);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(command, "pinned Philomena archive template").trim(),
        "{id}"
    );
}

#[test]
fn pinned_danbooru_e621_post_archive_templates_differ_from_feeds() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for archive differential");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "from gallery_dl.extractor.danbooru import DanbooruPostExtractor, DanbooruTagExtractor, DanbooruPoolExtractor; from gallery_dl.extractor.e621 import E621PostExtractor, E621TagExtractor, E621FavoriteExtractor; print('|'.join([DanbooruPostExtractor.archive_fmt, DanbooruTagExtractor.archive_fmt, DanbooruPoolExtractor.archive_fmt, E621PostExtractor.archive_fmt, E621TagExtractor.archive_fmt, E621FavoriteExtractor.archive_fmt]))",
    ]);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(command, "pinned Danbooru/E621 archive templates").trim(),
        "{id}|t_{search_tags}_{id}|p_{pool[id]}_{id}|{id}|t_{search_tags}_{id}|f_{user_id}_{id}"
    );
}

#[test]
fn pinned_direct_post_categories_match_source_sites() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for category differential");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "from gallery_dl import extractor; urls=('https://danbooru.donmai.us/posts/123', 'https://e621.net/posts/234', 'https://e926.net/posts/345', 'https://e6ai.net/posts/456'); print('|'.join(f'{ex.category}:{ex.archive_fmt}' for ex in (extractor.find(url) for url in urls)))",
    ]);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(command, "pinned source site categories").trim(),
        "danbooru:{id}|e621:{id}|e926:{id}|e6ai:{id}"
    );
}

#[test]
fn pinned_danbooru_and_e621_pool_archive_templates_are_pool_scoped() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned reference source is required");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "from gallery_dl.extractor.danbooru import DanbooruPoolExtractor,DanbooruPostExtractor; from gallery_dl.extractor.e621 import E621PoolExtractor,E621PostExtractor; print('|'.join((DanbooruPoolExtractor.archive_fmt,DanbooruPostExtractor.archive_fmt,E621PoolExtractor.archive_fmt,E621PostExtractor.archive_fmt)))",
    ]);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(command, "pinned pool archive templates").trim(),
        "p_{pool[id]}_{id}|{id}|p_{pool[id]}_{id}|{id}"
    );
}
