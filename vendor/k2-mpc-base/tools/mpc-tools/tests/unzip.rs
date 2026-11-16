//! `unzip` against Python's zipfile as the packs ran it, from tests/data/unzip
//! (gen_unzip_table.py): `read` rows take one member out (the same bytes; a missing member is
//! now exit 4, other failures exit 1), `extract` rows take all of them out (the same files and
//! folders; the same exit code).

mod common;
use common::*;

use std::path::Path;

const TABLE: &str = include_str!("data/unzip/unzip.tsv");

fn listing(d: &Path) -> String {
    fn go(base: &Path, dir: &Path, out: &mut Vec<String>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let e = e.unwrap();
            let p = e.path();
            let rel = p.strip_prefix(base).unwrap().to_string_lossy().into_owned();
            if e.file_type().unwrap().is_dir() {
                out.push(format!("{rel}/"));
                go(base, &p, out);
            } else {
                out.push(format!("{rel}:{}", sha256(&std::fs::read(&p).unwrap())));
            }
        }
    }
    let mut out = Vec::new();
    go(d, d, &mut out);
    out.sort();
    if out.is_empty() {
        "-".into()
    } else {
        out.join(" | ")
    }
}

#[test]
fn every_case_unzips_as_python_did() {
    let rows = rows(TABLE);
    let zips = data("unzip").join("zips");
    let mut wrong = Vec::new();
    for (i, r) in rows.iter().enumerate() {
        let (mode, zip, member, old_code, old_err, result) =
            (&r[0], &r[1], unescape(&r[2]), &r[3], &r[4], unescape(&r[5]));
        let zp = zips.join(format!("{zip}.zip"));
        let zp = zp.to_str().unwrap();
        let t = Temp::new("unzip");
        if mode == "read" {
            let out = run(
                &["unzip", zp, member.as_str(), "out.bin"],
                b"",
                &[],
                t.path(),
            );
            let want_code = match (old_code.as_str(), old_err.as_str()) {
                ("0", _) => 0,
                (_, "KeyError") => 4,
                _ => 1,
            };
            let got = if out.code == 0 {
                let b = std::fs::read(t.path().join("out.bin")).unwrap_or_default();
                format!("{} {}", sha256(&b), b.len())
            } else {
                "-".to_string()
            };
            if out.code != want_code || got != result {
                wrong.push(format!(
                    "row {i} read {zip} {member:?}: want exit {want_code} ({old_err}) {result}, got exit {} {got} ({})",
                    out.code,
                    text(&out.stderr).trim()
                ));
            }
        } else {
            let dest = t.path().join("x").join("dest");
            std::fs::create_dir_all(&dest).unwrap();
            let out = run(&["unzip", zp, "--all", "x/dest"], b"", &[], t.path());
            let want_code: i32 = old_code.parse().unwrap();
            let got = listing(&dest);
            if out.code != want_code || got != result {
                wrong.push(format!(
                    "row {i} extract {zip}: want exit {want_code} ({old_err}) {result}, got exit {} {got} ({})",
                    out.code,
                    text(&out.stderr).trim()
                ));
            }
        }
    }
    report("unzip.tsv", rows.len(), 132, wrong);
}

#[test]
fn a_missing_zip_is_not_found() {
    let t = Temp::new("unzip-missing");
    let out = run(&["unzip", "no.zip", "a", "b"], b"", &[], t.path());
    assert_eq!(out.code, 4, "{}", text(&out.stderr));
}

#[test]
fn crc32_of_known_strings() {
    assert_eq!(mpc_tools::unzip::crc32(b""), 0);
    assert_eq!(mpc_tools::unzip::crc32(b"123456789"), 0xCBF4_3926);
}
