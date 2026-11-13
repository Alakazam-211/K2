//! The README's worked example, `tests/fixtures/example/`: a file, the
//! statement for its build and the `trust/` folder that came with it, signed
//! with the test keys (the golden vectors' node key, a fixed owner key).
//!
//! The files are what [`example_files`] writes, byte for byte (both
//! signatures are deterministic), and `kessel-attest verify` prints exactly
//! what the README shows. After a deliberate change, write them again with
//! `KESSEL_ATTEST_WRITE_EXAMPLE=1 cargo test --test example` and paste the
//! new output into the README.

mod common;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use common::*;
use serde_json::json;

fn example_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/example")
}

fn to_json_line(v: &serde_json::Value) -> Vec<u8> {
    (serde_json::to_string(v).unwrap() + "\n").into_bytes()
}

/// The owner of the example: a fixed test key, so its fingerprint is fixed.
fn owner() -> Owner {
    Owner::new(7)
}

/// Every file of the example, by its path under the example folder.
fn example_files() -> Vec<(String, Vec<u8>)> {
    let owner = owner();
    let node = Node::golden();
    let self_st = node.self_statement(|v| {
        v["key_born_at"] = json!(t("2026-09-01T00:00:00Z"));
        v["issued_at"] = json!(t("2026-09-02T00:00:00Z"));
    });
    let mut entry = owner.entry(&node, &body_digest(&self_st));
    entry["valid_from"] = json!(t("2026-09-01T00:00:00Z"));
    let roster = owner.roster(1, t("2026-10-06T00:00:00Z"), None, vec![entry]);
    let mut st = statement_for(&owner, &node, &self_st);
    let md = &mut st["predicate"]["runDetails"]["metadata"];
    md["startedOn"] = json!("2026-10-05T08:12:11Z");
    md["finishedOn"] = json!("2026-10-05T08:37:40Z");
    let statement_hex = body_digest(&self_st)["sha256:".len()..].to_string();
    vec![
        ("acme-linux-x86_64".into(), BIN_A_BYTES.to_vec()),
        ("provenance/L1.intoto.jsonl".into(), to_json_line(&node.envelope(&st))),
        (format!("trust/nodes/{}.pem", node.fp), node.pem().into_bytes()),
        (format!("trust/statements/{statement_hex}.json"), to_json_line(&self_st)),
        (format!("trust/rosters/{}.json", owner.fp), to_json_line(&roster)),
        ("trust/owners.json".into(), to_json_line(&json!([{"id": "acme-owner.example.test", "rosterKeyFp": owner.fp, "rosterSeq": 1}]))),
    ]
}

fn files_under(root: &Path, dir: &Path, out: &mut BTreeSet<String>) {
    for e in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
        let p = e.unwrap().path();
        if p.is_dir() {
            files_under(root, &p, out);
        } else {
            out.insert(p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/"));
        }
    }
}

#[test]
fn the_example_folder_is_what_the_generator_writes() {
    let dir = example_dir();
    let want = example_files();
    if std::env::var_os("KESSEL_ATTEST_WRITE_EXAMPLE").is_some() {
        // Overwrite in place (the other test reads the folder meanwhile),
        // then drop what the generator no longer writes.
        let mut old = BTreeSet::new();
        if dir.is_dir() {
            files_under(&dir, &dir, &mut old);
        }
        for rel in old.iter().filter(|r| !want.iter().any(|(n, _)| n == *r)) {
            std::fs::remove_file(dir.join(rel)).unwrap();
        }
        for (rel, bytes) in &want {
            let p = dir.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, bytes).unwrap();
        }
    }
    let mut have = BTreeSet::new();
    files_under(&dir, &dir, &mut have);
    let names: BTreeSet<String> = want.iter().map(|(n, _)| n.clone()).collect();
    assert_eq!(have, names, "the example folder's files (KESSEL_ATTEST_WRITE_EXAMPLE=1 writes them again)");
    for (rel, bytes) in &want {
        let on_disk = std::fs::read(dir.join(rel)).unwrap();
        assert!(on_disk == *bytes, "{rel} differs from what the generator writes (KESSEL_ATTEST_WRITE_EXAMPLE=1 writes it again)");
    }
}

#[test]
fn the_readme_shows_what_verify_prints_for_the_example() {
    let owner_fp = owner().fp;
    let o = Command::new(env!("CARGO_BIN_EXE_kessel-attest"))
        .current_dir(example_dir())
        .args(["verify", "acme-linux-x86_64", "--provenance", "provenance/L1.intoto.jsonl", "--trust", "trust", "--owner-key", &owner_fp])
        .output()
        .expect("run kessel-attest");
    let stdout = String::from_utf8(o.stdout).unwrap();
    assert_eq!(o.status.code(), Some(0), "{stdout}{}", String::from_utf8_lossy(&o.stderr));
    assert!(stdout.starts_with("verified: acme-linux-x86_64\n"), "{stdout}");
    let readme = std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("README.md")).unwrap();
    assert!(readme.contains(&format!("  --owner-key {owner_fp}\n")), "the README's command names the example owner key {owner_fp}");
    assert!(readme.contains(&format!("let owner_fp = \"{owner_fp}\"")), "the README's library example names the example owner key {owner_fp}");
    assert!(readme.contains(&format!("```text\n{stdout}```\n")), "the README shows this output exactly:\n{stdout}");
}
