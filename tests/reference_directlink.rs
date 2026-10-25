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
        drop(native_archive);
        assert_eq!(
            std::fs::read(&archive_path).unwrap(),
            upstream_bytes,
            "{url}: external upstream archive bytes must not change"
        );
        verified_cases.push(json!({
            "url": url,
            "reference_events": source,
            "native_events": rust,
            "status": "source_fields_and_readonly_archive_match",
            "archive_key_compatible": true,
            "external_archive_read_only": true,
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
