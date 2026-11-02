//! Configuration and filename precedence, exercised through the public CLI.
use std::{fs, path::PathBuf, process::Command};

fn fixture_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("galerith-config-{name}-{}", std::process::id()))
}

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args(args)
        .output()
        .expect("start native CLI")
}

#[test]
fn explicit_json_yaml_toml_and_overrides_merge_in_order() {
    let dir = fixture_dir("formats");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let json = dir.join("first.json");
    let yaml = dir.join("second.yaml");
    let toml = dir.join("third.toml");
    fs::write(
        &json,
        r#"{"extractor":{"filename":"{category}_{filename}.{extension}"}}"#,
    )
    .unwrap();
    fs::write(&yaml, "filename: yaml_{filename}.{extension}\n").unwrap();
    fs::write(&toml, "filename = \"toml_{filename}.{extension}\"\n").unwrap();
    let args = [
        "-c",
        json.to_str().unwrap(),
        "--config-yaml",
        yaml.to_str().unwrap(),
        "--config-toml",
        toml.to_str().unwrap(),
        "-o",
        "filename=override_{filename}.{extension}",
        "--simulate",
        "https://example.org/file.png",
    ];
    let result = run(&args);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        String::from_utf8(result.stdout).unwrap(),
        "[simulate] override_file.png\n"
    );

    let cli = run(&[
        "--config-json",
        json.to_str().unwrap(),
        "--filename",
        "explicit_{filename}.{extension}",
        "--simulate",
        "https://example.org/file.png",
    ]);
    assert!(
        cli.status.success(),
        "{}",
        String::from_utf8_lossy(&cli.stderr)
    );
    assert_eq!(
        String::from_utf8(cli.stdout).unwrap(),
        "[simulate] explicit_file.png\n"
    );

    let fallback = run(&[
        "-c",
        json.to_str().unwrap(),
        "--simulate",
        "https://example.org/file.png",
    ]);
    assert!(fallback.status.success());
    assert_eq!(
        String::from_utf8(fallback.stdout).unwrap(),
        "[simulate] directlink_file.png\n"
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn bad_config_does_not_silently_become_empty_configuration() {
    let dir = fixture_dir("bad");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let file = dir.join("bad.json");
    fs::write(&file, "[1,2,3]").unwrap();
    let result = run(&[
        "-c",
        file.to_str().unwrap(),
        "--simulate",
        "https://example.org/a.jpg",
    ]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("must be an object"));
    fs::remove_dir_all(dir).unwrap();
}
