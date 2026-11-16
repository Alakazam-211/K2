//! `undo ck-from-log` against the Python tools/undo/ck-from-log.py it replaced, from
//! tests/data/ck-from-log (its generator is in the table's git history): the same exit code,
//! stdout and files, byte for byte.

mod common;
use common::*;

const TABLE: &str = include_str!("data/ck-from-log/ck.tsv");

#[test]
fn every_log_gives_the_old_files() {
    let rows = rows(TABLE);
    let mut wrong = Vec::new();
    for (i, r) in rows.iter().enumerate() {
        let t = Temp::new("ck");
        let log = unhex(&r[0]);
        std::fs::write(t.path().join("j.log"), &log).unwrap();
        std::fs::create_dir(t.path().join("out")).unwrap();
        let out = run(
            &["undo", "ck-from-log", "j.log", "--out", "out"],
            b"",
            &[],
            t.path(),
        );
        let want_code: i32 = r[1].parse().unwrap();
        let want_stdout = unescape(&r[2]);
        let want_files: Vec<String> = r[3..].iter().map(|f| unescape(f.as_str())).collect();
        let mut names: Vec<_> = std::fs::read_dir(t.path().join("out"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        let got_files: Vec<String> = names
            .iter()
            .map(|n| {
                format!(
                    "{n}={}",
                    text(&std::fs::read(t.path().join("out").join(n)).unwrap())
                )
            })
            .collect();
        if out.code != want_code || text(&out.stdout) != want_stdout || got_files != want_files {
            wrong.push(format!(
                "row {i} log {:?}: want exit {want_code} {want_stdout:?} {want_files:?}, got exit {} {:?} {got_files:?} ({})",
                text(&log),
                out.code,
                text(&out.stdout),
                text(&out.stderr).trim()
            ));
        }
    }
    report("ck.tsv", rows.len(), 150, wrong);
}

#[test]
fn a_missing_log_is_not_found() {
    let t = Temp::new("ck-missing");
    let out = run(&["undo", "ck-from-log", "no.log"], b"", &[], t.path());
    assert_eq!(out.code, 4, "{}", text(&out.stderr));
}
