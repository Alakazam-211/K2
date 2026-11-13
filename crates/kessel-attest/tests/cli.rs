//! The binary: arguments, exit codes, the evidence-folder layout, `--json`.

mod common;

use std::process::{Command, Output};

use common::*;
use serde_json::{json, Value};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_kessel-attest")).args(args).output().expect("run kessel-attest")
}

fn code(o: &Output) -> i32 {
    o.status.code().expect("exited with a code")
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn p(path: &std::path::Path) -> String {
    path.display().to_string()
}

#[test]
fn help_and_version() {
    let o = run(&["--help"]);
    assert_eq!(code(&o), 0);
    for word in ["--owner-key", "--existed-before", "verified, time not proven", "EXIT"] {
        assert!(out(&o).contains(word), "{word}");
    }
    let o = run(&["--version"]);
    assert_eq!((code(&o), out(&o).trim()), (0, format!("kessel-attest {}", env!("CARGO_PKG_VERSION")).as_str()));
}

#[test]
fn bad_usage_exits_2_and_says_what_is_missing() {
    let w = World::new("cli-usage");
    w.write(t(AFTER_BUILD));
    let f = w.file("dl/acme-linux-x86_64", BIN_A_BYTES);
    let fp = p(&f);
    let prov = p(&w.provenance("L1"));
    let trust = p(&w.trust());
    let cases: Vec<(Vec<&str>, &str)> = vec![
        (vec!["check"], "unknown command"),
        (vec!["verify", &fp, "--provenance", &prov, "--trust", &trust], "--owner-key"),
        (vec!["verify", &fp, "--provenance", &prov, "--owner-key", &w.owner.fp], "--trust"),
        (vec!["verify", "--provenance", &prov, "--trust", &trust, "--owner-key", &w.owner.fp], "name the file"),
        (vec!["verify", &fp, "--trust", &trust, "--owner-key", &w.owner.fp], "--provenance"),
        (vec!["verify", &fp, "--provenance", &prov, "--trust", &trust, "--owner-key", &w.owner.fp, "--existed-before", "yesterday"], "YYYY-MM-DDTHH:MM:SSZ"),
        (vec!["verify", &fp, "--provenance", &prov, "--trust", &trust, "--owner-key", &w.owner.fp, "--isolation", "vm"], "--isolation"),
        (vec!["verify", &fp, "--provenance", &prov, "--trust", &trust, "--owner-key"], "needs a value"),
        (vec!["verify", "/nonexistent/file", "--provenance", &prov, "--trust", &trust, "--owner-key", &w.owner.fp], "not a file"),
    ];
    for (args, want) in cases {
        let o = run(&args);
        assert_eq!(code(&o), 2, "{args:?}: {}", err(&o));
        assert!(err(&o).contains(want), "{args:?}: {}", err(&o));
    }
}

#[test]
fn verified_exits_0_and_says_what_was_built() {
    let w = World::new("cli-ok");
    w.write(t(AFTER_BUILD));
    let f = w.file("dl/acme-linux-x86_64", BIN_A_BYTES);
    let o = run(&["verify", &p(&f), "--provenance", &p(&w.provenance("L1")), "--trust", &p(&w.trust()), "--owner-key", &w.owner.fp]);
    assert_eq!(code(&o), 0, "{}{}", out(&o), err(&o));
    let text = out(&o);
    assert!(text.starts_with(&format!("verified: {}\n", p(&f))), "{text}");
    assert!(text.contains("built by node test-node-01 (host host-a) of acme-owner.example.test from git+https://git.example.test/acme/app.git"), "{text}");
    assert!(text.contains("command: bash scripts/ci/build.sh x86_64"), "{text}");
}

#[test]
fn not_verified_exits_1_and_names_the_rule() {
    let w = World::new("cli-bad");
    w.write(t(AFTER_BUILD));
    let f = w.file("dl/acme-linux-x86_64", b"something else");
    let o = run(&["verify", &p(&f), "--provenance", &p(&w.provenance("L1")), "--trust", &p(&w.trust()), "--owner-key", &w.owner.fp]);
    assert_eq!(code(&o), 1, "{}", out(&o));
    assert!(out(&o).starts_with(&format!("NOT verified: {}: file: ", p(&f))), "{}", out(&o));
}

#[test]
fn time_not_proven_exits_3_and_existed_before_settles_it() {
    let mut w = World::new("cli-time");
    let fp = w.node.fp.clone();
    for e in w.entries.iter_mut().filter(|e| e["node_fp"] == json!(fp)) {
        e["status"] = json!("retired");
        e["valid_to"] = json!(t("2027-01-15T12:00:00Z"));
    }
    w.write(t(AFTER_BUILD));
    let f = w.file("dl/acme-linux-x86_64", BIN_A_BYTES);
    let base = ["verify".to_string(), p(&f), "--provenance".into(), p(&w.provenance("L1")), "--trust".into(), p(&w.trust()), "--owner-key".into(), w.owner.fp.clone()];
    let args: Vec<&str> = base.iter().map(String::as_str).collect();
    let o = run(&args);
    assert_eq!(code(&o), 3, "{}", out(&o));
    assert!(out(&o).starts_with("verified, time not proven: "), "{}", out(&o));
    let mut proven = args.clone();
    proven.extend(["--as-of", "2027-01-15T09:00:00Z"]);
    let o = run(&proven);
    assert_eq!(code(&o), 0, "{}", out(&o));
}

#[test]
fn an_evidence_folder_checks_its_fetched_files_and_json_says_so() {
    let w = World::new("cli-dir");
    w.write(t(AFTER_BUILD));
    // Fetched files sit at DIR/<label>/<subject name>.
    w.file("L1/dist/acme-linux-x86_64", BIN_A_BYTES);
    w.file("L1/dist/acme-linux-x86_64.sha256", SUM_A_BYTES);
    w.file("V1/dist/acme-linux-x86_64", BIN_A_BYTES);
    let dir = p(w.dir.path());
    let o = run(&["verify", "--dir", &dir, "--owner-key", &w.owner.fp, "--json"]);
    assert_eq!(code(&o), 0, "{}{}", out(&o), err(&o));
    let v: Value = serde_json::from_str(&out(&o)).expect("one JSON object");
    assert_eq!(v["verdict"], "verified");
    assert_eq!(v["exitCode"], 0);
    assert_eq!(v["statements"].as_array().unwrap().len(), 2);
    let files: Vec<&str> = v["checks"].as_array().unwrap().iter().filter(|c| c["rule"] == "file").map(|c| c["subject"].as_str().unwrap()).collect();
    assert_eq!(files.len(), 3, "{files:?}");
    assert_eq!(v["roster"]["seq"], 1);

    // A fetched file changed after the fact: refused, by name.
    w.file("V1/dist/acme-linux-x86_64", b"swapped");
    let o = run(&["verify", "--dir", &dir, "--owner-key", &w.owner.fp, "--json"]);
    assert_eq!(code(&o), 1);
    let v: Value = serde_json::from_str(&out(&o)).unwrap();
    assert_eq!(v["verdict"], "not_verified");
    assert_eq!(v["failed"]["rule"], "file");
    assert!(v["failed"]["subject"].as_str().unwrap().ends_with("V1/dist/acme-linux-x86_64"));
}
