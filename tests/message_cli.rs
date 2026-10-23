//! Independent, read-only extractor event inspection regression tests.
use std::process::Command;

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args(args)
        .output()
        .expect("launch galerith CLI")
}

#[test]
fn direct_media_message_dump_is_ordered_json_lines_without_network() {
    let response = run(&["--dump-messages", "https://example.org/photo.webp"]);
    assert!(
        response.status.success(),
        "{}",
        String::from_utf8_lossy(&response.stderr)
    );
    let output = String::from_utf8(response.stdout).unwrap();
    let events: Vec<serde_json::Value> = output
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["type"], "directory");
    assert_eq!(events[0]["metadata"]["category"], "directlink");
    assert_eq!(events[0]["metadata"]["filename"], "photo.webp");
    assert_eq!(events[1]["type"], "url");
    assert_eq!(events[1]["url"], "https://example.org/photo.webp");
    assert_eq!(events[1]["metadata"]["filename"], "photo.webp");
}

#[test]
fn message_mode_rejects_modes_that_would_have_ambiguous_output() {
    for forbidden in ["--dump-json", "--get-urls", "--simulate", "--no-download"] {
        let response = run(&[
            "--dump-messages",
            forbidden,
            "https://example.org/photo.png",
        ]);
        assert!(!response.status.success(), "{forbidden} should conflict");
    }
    let range = run(&[
        "--dump-messages",
        "--range",
        "1",
        "https://example.org/photo.png",
    ]);
    assert!(!range.status.success());
}

#[test]
fn legacy_media_dump_remains_unchanged() {
    let response = run(&["--dump-json", "https://example.org/photo.png"]);
    assert!(response.status.success());
    let object: serde_json::Value = serde_json::from_slice(&response.stdout).unwrap();
    assert_eq!(object["filename"], "photo.png");
    assert_eq!(object["extractor"], "directlink");
    assert!(object.get("type").is_none());
}

#[test]
fn message_mode_uses_redacted_error_diagnostics() {
    let response = run(&[
        "--dump-messages",
        "https://example.org/gallery/5?api_key=private-synthetic-token",
    ]);
    assert!(!response.status.success());
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&response.stdout),
        String::from_utf8_lossy(&response.stderr)
    );
    assert!(!combined.contains("private-synthetic-token"));
    assert!(combined.contains("no implemented extractor"));
}

#[test]
fn expanding_queues_is_opt_in_and_preserves_direct_media_output() {
    let original = run(&["--dump-messages", "https://example.org/photo.png"]);
    let expanded = run(&[
        "--dump-messages",
        "--expand-queues",
        "--max-queue-children",
        "2",
        "https://example.org/photo.png",
    ]);
    assert!(original.status.success());
    assert!(
        expanded.status.success(),
        "{}",
        String::from_utf8_lossy(&expanded.stderr)
    );
    assert_eq!(original.stdout, expanded.stdout);
}

#[test]
fn queue_controls_reject_missing_message_mode_and_unsafe_limits() {
    for args in [
        vec!["--expand-queues", "https://example.org/photo.png"],
        vec![
            "--dump-messages",
            "--max-queue-children",
            "2",
            "https://example.org/photo.png",
        ],
        vec![
            "--dump-messages",
            "--expand-queues",
            "--max-queue-children",
            "17",
            "https://example.org/photo.png",
        ],
        vec![
            "--dump-messages",
            "--expand-queues",
            "--max-queue-children",
            "0",
            "https://example.org/photo.png",
        ],
    ] {
        let response = run(&args);
        assert!(!response.status.success(), "{args:?} should be rejected");
    }
}
