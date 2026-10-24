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
fn directlink_message_dump_preserves_native_source_fields_without_changing_flat_json() {
    let input = "https://cdn.example.org/archive/cover%20art.jpe?size=original#image";
    let response = run(&["--dump-messages", input]);
    assert!(response.status.success());
    let events: Vec<serde_json::Value> = String::from_utf8(response.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["metadata"]["domain"], "cdn.example.org");
    assert_eq!(events[0]["metadata"]["subcategory"], "example.org");
    assert_eq!(events[0]["metadata"]["path"], "archive");
    assert_eq!(events[0]["metadata"]["stem"], "cover art");
    assert_eq!(events[0]["metadata"]["extension"], "jpe");
    assert_eq!(events[0]["metadata"]["filename"], "cover art.jpe");
    assert_eq!(events[1]["metadata"], events[0]["metadata"]);

    let legacy = run(&["--dump-json", input]);
    assert!(legacy.status.success());
    let item: serde_json::Value = serde_json::from_slice(&legacy.stdout).unwrap();
    assert_eq!(item["filename"], "cover art.jpe");
    assert!(item.get("domain").is_none());
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

#[test]
fn queue_depth_is_opt_in_bounded_and_has_no_effect_on_direct_media() {
    let base = run(&[
        "--dump-messages",
        "--expand-queues",
        "https://example.org/photo.png",
    ]);
    assert!(base.status.success());
    for depth in ["1", "2", "4"] {
        let result = run(&[
            "--dump-messages",
            "--expand-queues",
            "--queue-depth",
            depth,
            "https://example.org/photo.png",
        ]);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(result.stdout, base.stdout);
    }
    for options in [
        vec![
            "--dump-messages",
            "--queue-depth",
            "2",
            "https://example.org/photo.png",
        ],
        vec![
            "--dump-messages",
            "--expand-queues",
            "--queue-depth",
            "0",
            "https://example.org/photo.png",
        ],
        vec![
            "--dump-messages",
            "--expand-queues",
            "--queue-depth",
            "5",
            "https://example.org/photo.png",
        ],
    ] {
        let result = run(&options);
        assert!(!result.status.success(), "{options:?}");
    }
}

#[test]
fn follow_queues_download_job_preview_preserves_direct_media_json() {
    let plain = run(&["--dump-json", "https://example.org/photo.png"]);
    let queued = run(&[
        "--follow-queues",
        "--dump-json",
        "https://example.org/photo.png",
    ]);
    assert!(plain.status.success());
    assert!(
        queued.status.success(),
        "{}",
        String::from_utf8_lossy(&queued.stderr)
    );
    assert_eq!(plain.stdout, queued.stdout);
    let url = run(&[
        "--follow-queues",
        "--get-urls",
        "https://example.org/photo.png",
    ]);
    assert!(url.status.success());
    assert_eq!(
        String::from_utf8_lossy(&url.stdout).trim(),
        "https://example.org/photo.png"
    );
}

#[test]
fn message_inspection_and_queue_download_modes_are_distinct() {
    let output = run(&[
        "--follow-queues",
        "--dump-messages",
        "https://example.org/photo.png",
    ]);
    assert!(!output.status.success());
}

#[test]
fn typed_job_file_range_filters_before_the_downloader() {
    let url = "https://example.org/item.png";
    let first = run(&[
        "--follow-queues",
        "--job-file-range",
        "1",
        "--get-urls",
        url,
    ]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&first.stdout).trim(), url);

    let second = run(&[
        "--follow-queues",
        "--job-file-range",
        "2",
        "--get-urls",
        url,
    ]);
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert!(second.stdout.is_empty());
    let none = run(&[
        "--follow-queues",
        "--job-file-range",
        "1",
        "--job-child-range",
        "2:9:2",
        "--get-urls",
        url,
    ]);
    assert!(none.status.success());
    assert_eq!(none.stdout, first.stdout);
}

#[test]
fn job_filters_are_opt_in_and_use_checked_positive_range_syntax() {
    let url = "https://example.org/item.png";
    for args in [
        vec!["--job-file-range", "1", "--get-urls", url],
        vec!["--job-child-range", "1", "--get-urls", url],
        vec!["--follow-queues", "--job-file-range", "0", url],
        vec!["--follow-queues", "--job-child-range", "2:4:0", url],
        vec![
            "--follow-queues",
            "--job-file-range",
            "1",
            "--range",
            "1",
            url,
        ],
        vec![
            "--dump-messages",
            "--expand-queues",
            "--job-child-range",
            "1",
            url,
        ],
    ] {
        let response = run(&args);
        assert!(!response.status.success(), "{args:?}");
    }
}

#[test]
fn message_inspection_can_filter_file_events_without_fetching_media() {
    let url = "https://example.org/preview.jpg";
    let full = run(&["--dump-messages", "--expand-queues", url]);
    assert!(full.status.success());
    let only_first = run(&[
        "--dump-messages",
        "--expand-queues",
        "--message-file-range",
        "1",
        url,
    ]);
    assert!(only_first.status.success());
    assert_eq!(only_first.stdout, full.stdout);
    let exclude_first = run(&[
        "--dump-messages",
        "--expand-queues",
        "--message-file-range",
        "2",
        url,
    ]);
    assert!(exclude_first.status.success());
    let output = String::from_utf8(exclude_first.stdout).unwrap();
    let events: Vec<serde_json::Value> = output
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["type"], "directory");

    let child_only = run(&[
        "--dump-messages",
        "--expand-queues",
        "--message-child-range",
        "1",
        url,
    ]);
    assert!(child_only.status.success());
    assert_eq!(child_only.stdout, full.stdout);
}

#[test]
fn inspection_selectors_cannot_be_silently_ignored() {
    let url = "https://example.org/image.jpg";
    for args in [
        vec!["--message-file-range", "1", url],
        vec!["--dump-messages", "--message-child-range", "1", url],
        vec![
            "--dump-messages",
            "--expand-queues",
            "--message-file-range",
            "0",
            url,
        ],
        vec![
            "--dump-messages",
            "--expand-queues",
            "--message-child-range",
            "1:9:0",
            url,
        ],
        vec!["--follow-queues", "--message-file-range", "1", url],
    ] {
        let result = run(&args);
        assert!(!result.status.success(), "{args:?}");
    }
}

#[test]
fn directory_scope_ranges_filter_before_url_emission() {
    let url = "https://example.org/photo.png";
    let selected = run(&[
        "--follow-queues",
        "--job-directory-range",
        "1",
        "--get-urls",
        url,
    ]);
    assert!(selected.status.success());
    assert_eq!(String::from_utf8_lossy(&selected.stdout).trim(), url);
    let skipped = run(&[
        "--follow-queues",
        "--job-directory-range",
        "2",
        "--get-urls",
        url,
    ]);
    assert!(skipped.status.success());
    assert!(skipped.stdout.is_empty());

    let inspected = run(&[
        "--dump-messages",
        "--expand-queues",
        "--message-directory-range",
        "2",
        url,
    ]);
    assert!(inspected.status.success());
    assert!(inspected.stdout.is_empty());
}

#[test]
fn directory_scope_selector_requires_its_corresponding_job_mode() {
    let url = "https://example.org/photo.png";
    for args in [
        vec!["--job-directory-range", "1", url],
        vec!["--dump-messages", "--message-directory-range", "1", url],
        vec!["--follow-queues", "--job-directory-range", "0", url],
        vec![
            "--dump-messages",
            "--expand-queues",
            "--message-directory-range",
            "0",
            url,
        ],
    ] {
        let result = run(&args);
        assert!(!result.status.success(), "{args:?}");
    }
}

#[test]
fn source_post_range_requires_true_source_metadata() {
    let url = "https://example.org/asset.jpg";
    let output = run(&[
        "--follow-queues",
        "--job-source-post-range",
        "120-122",
        "--get-urls",
        url,
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stdout.is_empty(),
        "direct media has no source-post ID"
    );

    let message = run(&[
        "--dump-messages",
        "--expand-queues",
        "--message-source-post-range",
        "120:123",
        url,
    ]);
    assert!(
        message.status.success(),
        "{}",
        String::from_utf8_lossy(&message.stderr)
    );
    assert!(message.stdout.is_empty());
}

#[test]
fn source_post_range_is_bounded_and_never_ignored_in_other_modes() {
    let url = "https://example.org/asset.jpg";
    for args in [
        vec!["--job-source-post-range", "120", url],
        vec!["--message-source-post-range", "120", url],
        vec!["--dump-messages", "--message-source-post-range", "120", url],
        vec!["--follow-queues", "--job-source-post-range", "0", url],
        vec![
            "--dump-messages",
            "--expand-queues",
            "--message-source-post-range",
            "0",
            url,
        ],
        vec!["--follow-queues", "--job-source-post-range", "12:1", url],
    ] {
        let result = run(&args);
        assert!(!result.status.success(), "{args:?} should be rejected");
    }
}
