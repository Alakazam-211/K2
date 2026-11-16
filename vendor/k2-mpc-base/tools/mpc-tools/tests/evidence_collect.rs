//! `evidence collect` against the Python tools/evidence/collect.py it replaced, from
//! tests/data/evidence-collect (its generator is in the table's git history): each log's records as `--print` writes them, byte for
//! byte, and the same exit code.

mod common;
use common::*;

const TABLE: &str = include_str!("data/evidence-collect/evidence-collect.tsv");

#[test]
fn every_log_collects_as_the_old_tool() {
    let rows = rows(TABLE);
    let mut wrong = Vec::new();
    for r in &rows {
        let (name, job, log, old_code, old_out) =
            (&r[0], &r[1], unhex(&r[2]), &r[3], unescape(&r[4]));
        let t = Temp::new("evcollect");
        std::fs::write(t.path().join("j.log"), &log).unwrap();
        let out = run(
            &[
                "evidence",
                "collect",
                "--print",
                "--log",
                "j.log",
                "--job",
                job.as_str(),
            ],
            b"",
            &[],
            t.path(),
        );
        let want_code: i32 = old_code.parse().unwrap();
        if out.code != want_code || text(&out.stdout) != old_out {
            wrong.push(format!(
                "{name}: want exit {want_code}, got exit {} ({})\nwant {old_out:?}\ngot  {:?}",
                out.code,
                text(&out.stderr).trim(),
                text(&out.stdout)
            ));
        }
    }
    report("evidence-collect.tsv", rows.len(), 221, wrong);
}

#[test]
fn a_written_record_file_and_the_command_mode() {
    let t = Temp::new("evcollect-write");
    std::fs::write(
        t.path().join("j.log"),
        "SUMMARY mode=bfs states=12 levels=3 violations=none complete=true\n=== run a: exit 0, wall 1s\n",
    )
    .unwrap();
    let out = run(
        &[
            "evidence", "collect", "--log", "j.log", "--job", "ab12cd34", "--out", "runs",
        ],
        b"",
        &[],
        t.path(),
    );
    assert_eq!(out.code, 0, "{}", text(&out.stderr));
    assert_eq!(
        text(&out.stdout),
        "ab12cd34: 1 runs -> runs/ab12cd34.json\n"
    );
    let doc: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(t.path().join("runs/ab12cd34.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(doc["from"], "file j.log");
    assert_eq!(doc["runs"][0]["run"], "run_a");
    assert_eq!(doc["runs"][0]["status"], "COMPLETE");
    // the command mode: the log a command prints, {job} replaced (quoted)
    let out = run(
        &[
            "evidence",
            "collect",
            "--log-cmd",
            "cat j.log; echo {job} >&2",
            "--out",
            "runs2",
            "x1",
        ],
        b"",
        &[],
        t.path(),
    );
    assert_eq!(out.code, 0, "{}", text(&out.stderr));
    assert_eq!(text(&out.stdout), "x1: 1 runs -> runs2/x1.json\n");
    let out = run(
        &[
            "evidence",
            "collect",
            "--log-cmd",
            "true",
            "--out",
            "runs3",
            "x2",
        ],
        b"",
        &[],
        t.path(),
    );
    assert_eq!(out.code, 1, "an empty log is an error");
    assert!(
        text(&out.stderr).contains("x2: log not readable"),
        "{}",
        text(&out.stderr)
    );
    let out = run(
        &["evidence", "collect", "--log", "j.log"],
        b"",
        &[],
        t.path(),
    );
    assert_eq!(out.code, 2, "--log needs --job");
}
