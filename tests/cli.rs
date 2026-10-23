use std::process::Command;

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
    assert!(first.status.success(), "{}", String::from_utf8_lossy(&first.stderr));
    assert_eq!(String::from_utf8_lossy(&first.stdout).trim(), url);

    let second = run(&["--range", "2", "--get-urls", url]);
    assert!(second.status.success());
    assert!(second.stdout.is_empty());

    let malformed = run(&["--range", "1:5:0", "--get-urls", url]);
    assert!(!malformed.status.success());
}
