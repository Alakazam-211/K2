//! `edit replace-once` against the Python edit tools/n1/n1.sh made before, from
//! tests/data/replace-once (its generator is in the table's git history): the old stop (exit 1)
//! is the new refusal (exit 3); the file is the old code's, byte for byte.

mod common;
use common::*;

const TABLE: &str = include_str!("data/replace-once/replace-once.tsv");

#[test]
fn every_case_edits_as_the_old_code() {
    let rows = rows(TABLE);
    let mut wrong = Vec::new();
    for (i, r) in rows.iter().enumerate() {
        let (s, a, b, old_code, after) = (
            unescape(&r[0]),
            unescape(&r[1]),
            unescape(&r[2]),
            &r[3],
            unescape(&r[4]),
        );
        let t = Temp::new("replace");
        std::fs::write(t.path().join("f"), &s).unwrap();
        let out = run(
            &["edit", "replace-once", "f", a.as_str(), b.as_str()],
            b"",
            &[],
            t.path(),
        );
        let want_code = if old_code == "0" { 0 } else { 3 };
        let got = std::fs::read_to_string(t.path().join("f")).unwrap();
        if out.code != want_code || got != after {
            wrong.push(format!(
                "row {i} {s:?} find {a:?} replace {b:?}: want exit {want_code} {after:?}, got exit {} {got:?} ({})",
                out.code,
                text(&out.stderr).trim()
            ));
        }
    }
    report("replace-once.tsv", rows.len(), 200, wrong);
}

#[test]
fn bytes_are_kept_as_they_are() {
    let t = Temp::new("replace-crlf");
    std::fs::write(t.path().join("f"), "a\r\nb\r\n").unwrap();
    let out = run(&["edit", "replace-once", "f", "b", "c"], b"", &[], t.path());
    assert_eq!(out.code, 0, "{}", text(&out.stderr));
    assert_eq!(
        std::fs::read_to_string(t.path().join("f")).unwrap(),
        "a\r\nc\r\n"
    );
}
