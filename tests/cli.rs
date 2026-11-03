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
fn version_flag_prints_the_native_package_version() {
    let output = run(&["--version"]);
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("galerith {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn filesize_limits_accept_size_suffixes_and_independent_bounds() {
    let valid = run(&[
        "--filesize-min",
        "500k",
        "--filesize-max",
        "2.5M",
        "--simulate",
        "https://example.org/photo.jpg",
    ]);
    assert!(
        valid.status.success(),
        "{}",
        String::from_utf8_lossy(&valid.stderr)
    );

    let inverted = run(&[
        "--filesize-min",
        "4M",
        "--filesize-max",
        "2M",
        "--simulate",
        "https://example.org/photo.jpg",
    ]);
    assert!(
        inverted.status.success(),
        "{}",
        String::from_utf8_lossy(&inverted.stderr)
    );

    let bad_suffix = run(&[
        "--filesize-min",
        "12XB",
        "--simulate",
        "https://example.org/photo.jpg",
    ]);
    assert!(!bad_suffix.status.success());
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
fn filters_native_extractor_listing_by_category() {
    let output = run(&["--list-extractors=danbooru,directlink"]);
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("danbooru"));
    assert!(text.contains("directlink"));
    assert!(!text.contains("wallhaven"));
    assert!(!text.contains("pixiv"));
}

#[test]
fn extractor_info_reports_directlink_defaults_without_network() {
    let output = run(&["-E", "https://cdn.example.org/gallery/photo.jpg"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("Category / Subcategory\n  \"directlink\" / \"example.org\""));
    assert!(text.contains("Filename format (default):"));
    assert!(text.contains("Directory format (default):"));
    assert!(text.contains("Archive format (default):"));
    assert!(!text.contains("Request interval"));
}

#[test]
fn extractor_info_reports_configured_values_and_directlink_defaults() {
    let output = run(&[
        "--extractor-info",
        "--option",
        "extractor.filename=custom/{filename}.{extension}",
        "https://cdn.example.org/gallery/photo.jpg",
    ]);
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("Filename format (custom):"));
    assert!(text.contains("custom/{filename}.{extension}"));
    assert!(text.contains("Filename format (default):"));
    assert!(text.contains("{domain}/{path}/{filename}.{extension}"));
}

#[test]
fn extractor_info_rejects_unsupported_sources_and_conflicting_output_modes() {
    let unsupported = run(&["-E", "https://e621.net/posts/123"]);
    assert!(!unsupported.status.success());
    assert!(String::from_utf8_lossy(&unsupported.stderr)
        .contains("currently supports direct media URLs only"));

    let conflicting = run(&[
        "-E",
        "--dump-json",
        "https://cdn.example.org/gallery/photo.jpg",
    ]);
    assert!(!conflicting.status.success());
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
fn tag_sidecar_option_conflicts_with_url_only_output_modes() {
    for args in [
        vec![
            "--get-urls",
            "--write-tags",
            "https://example.org/image.jpg",
        ],
        vec![
            "--dump-json",
            "--write-tags",
            "https://example.org/image.jpg",
        ],
    ] {
        let output = run(&args);
        assert!(!output.status.success());
    }
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
    assert_eq!(result.as_array().unwrap().len(), 2);
    assert_eq!(result[0][0], 2);
    assert_eq!(result[0].as_array().unwrap().len(), 2);
    assert_eq!(result[0][1]["category"], "directlink");
    assert_eq!(result[0][1]["filename"], "image");
    assert_eq!(result[1][0], 3);
    assert_eq!(result[1][1], "https://example.org/image.webp");
    assert_eq!(result[1].as_array().unwrap().len(), 3);
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
fn lists_only_native_extractor_modules() {
    let output = run(&["--list-modules"]);
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    let modules = text.lines().collect::<Vec<_>>();
    assert!(modules.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(modules.contains(&"directlink"));
    assert!(modules.contains(&"danbooru"));
    assert!(!modules.contains(&"pixiv"));
    assert!(!run(&["--list-modules", "--list-extractors"])
        .status
        .success());
}

#[test]
fn filename_template_updates_simulated_path_without_network() {
    let output = run(&[
        "-f",
        "{category}_{stem}.{extension}",
        "--simulate",
        "https://example.org/image.webp",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "[simulate] directlink_image.webp\n"
    );
}

#[test]
fn original_filename_shortcut_preserves_source_name() {
    let output = run(&["-f", "/O", "--simulate", "https://example.org/image.webp"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "[simulate] image.webp\n"
    );
}

#[test]
fn filename_template_applies_slices_and_conversions_through_cli() {
    let output = run(&[
        "-f",
        "{filename[-3:]!u}_{category!q}",
        "--simulate",
        "https://example.org/image.webp",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "[simulate] AGE_directlink\n"
    );
}

#[test]
fn filename_template_applies_conditional_wrappers_through_cli() {
    let output = run(&[
        "-f",
        "{extension:?[/]/}_{stem}",
        "--simulate",
        "https://example.org/image.webp",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "[simulate] [webp]_image\n"
    );
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
