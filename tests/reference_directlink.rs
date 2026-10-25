//! Offline differential against the pinned upstream gallery-dl directlink
//! extractor. Upstream Python is executed ONLY by the isolated CI oracle job;
//! Galerith and its runtime remain Rust-native.
use serde_json::Value;
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
    for url in URLS {
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
    }
}
