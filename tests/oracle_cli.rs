//! Deterministic offline CLI golden corpus; deliberately separate from the
//! still-unimplemented pinned upstream differential extractor oracle.
use serde::Deserialize;
use serde_json::Value;
use std::process::Command;

#[derive(Debug, Deserialize)]
struct Corpus {
    schema_version: u64,
    baseline: String,
    scope: String,
    cases: Vec<Case>,
}
#[derive(Debug, Deserialize)]
struct Case {
    id: String,
    args: Vec<String>,
    expect: Expected,
}
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Expected {
    JsonSubset { value: Value },
    JsonLines { value: Vec<Value> },
    Stdout { text: String },
    ErrorContains { text: String, absent: Vec<String> },
}

fn assert_subset(actual: &Value, expected: &Value, label: &str) {
    match expected {
        Value::Object(map) => {
            let object = actual
                .as_object()
                .unwrap_or_else(|| panic!("{label}: expected JSON object; got {actual}"));
            for (key, value) in map {
                assert_subset(
                    object.get(key).unwrap_or_else(|| panic!("{label}: missing field {key}")),
                    value,
                    &format!("{label}.{key}"),
                );
            }
        }
        Value::Array(items) => {
            let array = actual
                .as_array()
                .unwrap_or_else(|| panic!("{label}: expected JSON array; got {actual}"));
            assert_eq!(array.len(), items.len(), "{label}: array length");
            for (index, value) in items.iter().enumerate() {
                assert_subset(&array[index], value, &format!("{label}[{index}]"));
            }
        }
        _ => assert_eq!(actual, expected, "{label}: golden value mismatch"),
    }
}

#[test]
fn offline_cli_golden_corpus_matches_explicit_expected_outputs() {
    let corpus: Corpus = serde_json::from_str(include_str!("fixtures/cli_golden.json"))
        .expect("parse pinned offline CLI golden corpus");
    assert_eq!(corpus.schema_version, 1);
    assert_eq!(corpus.scope, "galerith-cli-offline");
    assert_eq!(
        corpus.baseline,
        "7f226f74108501c8ac3120548370f68f19e6f9b3"
    );
    assert!(corpus.cases.len() >= 8, "golden corpus unexpectedly shrank");
    let mut ids = std::collections::HashSet::new();
    for case in corpus.cases {
        assert!(ids.insert(case.id.clone()), "duplicate corpus ID {}", case.id);
        let output = Command::new(env!("CARGO_BIN_EXE_galerith"))
            .args(&case.args)
            .output()
            .unwrap_or_else(|err| panic!("{}: failed to run Galerith: {err}", case.id));
        let stdout = String::from_utf8(output.stdout)
            .unwrap_or_else(|err| panic!("{}: non-UTF8 stdout: {err}", case.id));
        let stderr = String::from_utf8(output.stderr)
            .unwrap_or_else(|err| panic!("{}: non-UTF8 stderr: {err}", case.id));
        match case.expect {
            Expected::JsonSubset { value } => {
                assert!(output.status.success(), "{}: {stderr}", case.id);
                let observed: Value = serde_json::from_str(&stdout)
                    .unwrap_or_else(|err| panic!("{}: invalid JSON: {err}", case.id));
                assert_subset(&observed, &value, &case.id);
            }
            Expected::JsonLines { value } => {
                assert!(output.status.success(), "{}: {stderr}", case.id);
                let events = stdout
                    .lines()
                    .map(|line| serde_json::from_str::<Value>(line).expect("typed JSON line"))
                    .collect::<Vec<_>>();
                assert_eq!(events.len(), value.len(), "{}: event count", case.id);
                for (i, expected) in value.iter().enumerate() {
                    assert_subset(&events[i], expected, &format!("{}[{i}]", case.id));
                }
            }
            Expected::Stdout { text } => {
                assert!(output.status.success(), "{}: {stderr}", case.id);
                assert_eq!(stdout, text, "{}: exact stdout regression", case.id);
            }
            Expected::ErrorContains { text, absent } => {
                assert!(!output.status.success(), "{}: unexpectedly succeeded", case.id);
                let combined = format!("{stdout}{stderr}");
                assert!(combined.contains(&text), "{}: expected diagnostic: {text}; got {combined}", case.id);
                for secret in absent {
                    assert!(!combined.contains(&secret), "{}: leaked synthetic marker", case.id);
                }
            }
        }
    }
}
