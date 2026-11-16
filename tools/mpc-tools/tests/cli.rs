//! The rules every subcommand keeps: exit codes, help and version, and `--json`.

mod common;
use common::*;

#[test]
fn usage_help_and_version() {
    let t = Temp::new("cli");
    let none = run(&[], b"", &[], t.path());
    assert_eq!(none.code, 2, "no subcommand is bad usage");
    assert!(none.stdout.is_empty(), "usage goes to stderr");
    assert_eq!(run(&["no-such-command"], b"", &[], t.path()).code, 2);
    assert_eq!(
        run(&["licences", "--no-such-flag"], b"", &[], t.path()).code,
        2
    );
    let help = run(&["--help"], b"", &[], t.path());
    assert_eq!(help.code, 0);
    let h = text(&help.stdout);
    for sub in ["licences", "undo", "pack-check", "unzip", "edit"] {
        assert!(h.contains(sub), "--help lists {sub}: {h}");
    }
    let v = run(&["--version"], b"", &[], t.path());
    assert_eq!(v.code, 0);
    assert_eq!(
        text(&v.stdout).trim(),
        format!("mpc-tools {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn json_is_one_document_for_results_and_failures() {
    let t = Temp::new("cli-json");
    let ok = run(
        &["--json", "licences", "--expr", "a=MIT"],
        b"",
        &[],
        t.path(),
    );
    assert_eq!(ok.code, 0);
    let doc: serde_json::Value = serde_json::from_slice(&ok.stdout).expect("one JSON document");
    assert_eq!(doc["ok"], true);
    let missing = run(&["--json", "unzip", "no.zip", "a", "b"], b"", &[], t.path());
    assert_eq!(missing.code, 4);
    let doc: serde_json::Value =
        serde_json::from_slice(&missing.stdout).expect("one JSON document");
    assert_eq!(doc["code"], 4);
    assert!(!missing.stderr.is_empty(), "the reason goes to stderr too");
}

#[test]
fn refused_changes_nothing() {
    let t = Temp::new("cli-refused");
    std::fs::write(t.path().join("f"), "x x").unwrap();
    let out = run(&["edit", "replace-once", "f", "x", "y"], b"", &[], t.path());
    assert_eq!(out.code, 3);
    assert_eq!(std::fs::read_to_string(t.path().join("f")).unwrap(), "x x");
}
