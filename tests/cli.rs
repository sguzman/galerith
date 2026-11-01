use std::{
    io::Write,
    process::{Command, Stdio},
};

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args(args)
        .output()
        .expect("spawn galerith")
}

#[test]
fn lists_only_supported_extractors() {
    let output = run(&["--list-extractors"]);
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("directlink"));
    assert!(!text.contains("pixiv"));
}

#[test]
fn url_extraction_is_offline() {
    let output = run(&["--get-urls", "https://example.org/photo.png?size=large"]);
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        "https://example.org/photo.png?size=large"
    );
}

#[test]
fn input_file_stdin_ignores_blank_and_comment_lines() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args(["--input-file", "-", "--get-urls"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"\n# ignore this\nhttps://example.org/a.jpg\nhttps://example.org/b.jpg\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "https://example.org/a.jpg\nhttps://example.org/b.jpg\n"
    );
}

#[test]
fn invalid_download_rate_is_rejected_before_processing_inputs() {
    let output = run(&[
        "--limit-rate",
        "0",
        "--simulate",
        "https://example.org/photo.jpg",
    ]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("download rate"));
}

#[test]
fn json_extraction_is_offline() {
    let output = run(&["--dump-json", "https://example.org/image.webp"]);
    assert!(output.status.success());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["filename"], "image.webp");
    assert_eq!(result["extractor"], "directlink");
}

#[test]
fn unsupported_urls_fail_explicitly() {
    let output = run(&["--simulate", "https://example.org/gallery/5"]);
    assert!(!output.status.success());
    assert!(String::from_utf8(output.stderr)
        .unwrap()
        .contains("no implemented extractor"));
}

#[test]
fn danbooru_registered_but_not_claimed_complete() {
    let output = run(&["--list-extractors"]);
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("danbooru"));
    assert!(text.contains("partial"));
}

#[test]
fn filename_template_updates_json_without_network() {
    let output = run(&[
        "-f",
        "{category}_{stem}.{extension}",
        "--dump-json",
        "https://example.org/image.webp",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let item: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(item["filename"], "directlink_image.webp");
}

#[test]
fn filename_template_applies_slices_and_conversions_through_cli() {
    let output = run(&[
        "-f",
        "{filename[-4:]!u}_{category!q}",
        "--dump-json",
        "https://example.org/image.webp",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let item: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(item["filename"], "WEBP_directlink");
}

#[test]
fn filename_template_applies_conditional_wrappers_through_cli() {
    let output = run(&[
        "-f",
        "{extension:?[/]/}_{stem}",
        "--dump-json",
        "https://example.org/image.webp",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let item: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(item["filename"], "[webp]_image");
}

#[test]
fn unsafe_filename_template_fails_without_file_access() {
    let output = run(&[
        "-f",
        "../{filename}",
        "--simulate",
        "https://example.org/image.webp",
    ]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unsafe output filename"));
}

#[test]
fn error_output_never_prints_signed_query_parameters() {
    let output = run(&[
        "--simulate",
        "https://example.org/gallery/42?api_key=private-token-in-query",
    ]);
    assert!(!output.status.success());
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(!combined.contains("private-token-in-query"));
    assert!(combined.contains("https://example.org/gallery/42"));
}

#[test]
fn range_selection_is_one_based_and_rejects_zero() {
    let url = "https://example.org/media.jpg";
    let first = run(&["--range", "1", "--get-urls", url]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&first.stdout).trim(), url);

    let second = run(&["--range", "2", "--get-urls", url]);
    assert!(second.status.success());
    assert!(second.stdout.is_empty());

    let malformed = run(&["--range", "1:5:0", "--get-urls", url]);
    assert!(!malformed.status.success());
}
