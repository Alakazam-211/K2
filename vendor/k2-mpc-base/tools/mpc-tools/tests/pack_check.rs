//! `pack-check engine-free` and `carrier-row` against the Python that scripts/pack-checks/*.sh
//! ran before, from tests/data/pack-check (its generator is in the table's git history). The
//! exit code and first line are the old script's exactly; the findings after it are the same
//! lines (the old script listed them in its folder walk's order, which the file system chose;
//! the new one walks in name order).

mod common;
use common::*;

const TABLE: &str = include_str!("data/pack-check/pack-check.tsv");

fn lines_as_set(s: &str) -> (String, Vec<String>) {
    let mut it = s.lines();
    let first = it.next().unwrap_or("").to_string();
    let mut rest: Vec<String> = it.map(str::to_string).collect();
    rest.sort();
    (first, rest)
}

#[test]
fn every_stand_in_pack_judges_as_the_old_script() {
    let rows = rows(TABLE);
    let mut wrong = Vec::new();
    for (i, r) in rows.iter().enumerate() {
        let (check, meta, old_code, old_out) = (&r[0], &r[1], &r[2], unescape(&r[3]));
        let t = Temp::new("pack");
        let pack = t.path().join("pack");
        std::fs::create_dir(&pack).unwrap();
        for f in &r[4..] {
            let (p, c) = f.split_once('=').expect("PATH=CONTENT");
            let full = pack.join(unescape(p));
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(&full, unescape(c)).unwrap();
        }
        let out = if check == "engine-free" {
            std::fs::write(t.path().join("meta.json"), meta).unwrap();
            run(
                &[
                    "pack-check",
                    "engine-free",
                    "pack",
                    "--metadata",
                    "meta.json",
                ],
                b"",
                &[],
                t.path(),
            )
        } else {
            run(&["pack-check", "carrier-row", "pack"], b"", &[], t.path())
        };
        let want_code: i32 = old_code.parse().unwrap();
        if out.code != want_code || lines_as_set(&text(&out.stdout)) != lines_as_set(&old_out) {
            wrong.push(format!(
                "row {i} {check} {:?}: want exit {want_code} {old_out:?}, got exit {} {:?} ({})",
                &r[4..],
                out.code,
                text(&out.stdout),
                text(&out.stderr).trim()
            ));
        }
    }
    report("pack-check.tsv", rows.len(), 260, wrong);
}

#[test]
fn a_missing_pack_is_not_found() {
    let t = Temp::new("pack-missing");
    let out = run(&["pack-check", "carrier-row", "nope"], b"", &[], t.path());
    assert_eq!(out.code, 4, "{}", text(&out.stderr));
}
