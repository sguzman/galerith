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
    assert!(String::from_utf8(output.stderr).unwrap().contains("no implemented extractor"));
}
