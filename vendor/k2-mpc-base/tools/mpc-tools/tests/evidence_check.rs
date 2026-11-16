//! `evidence check` against the Python tools/evidence/check.py it replaced, from
//! tests/data/evidence-check (its generator is in the table's git history): each note checked against its run records, the same lines
//! and exit code. The evidence tools' own fixtures are rows too.

mod common;
use common::*;

const TABLE: &str = include_str!("data/evidence-check/evidence-check.tsv");

#[test]
fn every_note_checks_as_the_old_tool() {
    let rows = rows(TABLE);
    let mut wrong = Vec::new();
    for r in &rows {
        let (case, md, old_code, old_out) = (&r[0], unescape(&r[1]), &r[2], unescape(&r[3]));
        let t = Temp::new("evcheck");
        std::fs::create_dir(t.path().join("runs")).unwrap();
        for f in &r[4..] {
            let (name, text) = f.split_once('=').expect("NAME=CONTENT");
            std::fs::write(t.path().join("runs").join(name), unescape(text)).unwrap();
        }
        std::fs::write(t.path().join("note.md"), &md).unwrap();
        let out = run(
            &["evidence", "check", "--runs", "runs", "note.md"],
            b"",
            &[],
            t.path(),
        );
        let want_code: i32 = old_code.parse().unwrap();
        if out.code != want_code || text(&out.stdout) != old_out {
            wrong.push(format!(
                "{case}: want exit {want_code} {old_out:?}, got exit {} {:?} ({})",
                out.code,
                text(&out.stdout),
                text(&out.stderr).trim()
            ));
        }
    }
    report("evidence-check.tsv", rows.len(), 312, wrong);
}
