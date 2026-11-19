//! The rust_xlsxwriter test set (files saved by a spreadsheet app, fetched by reference:
//! `corpus/xlsxwriter.tsv`, `scripts/fetch-corpus.sh`). It runs when `OOXML_XLSXWRITER_CORPUS`
//! names the folder the files were fetched to, and says it skipped otherwise. Output names
//! files only, never their contents.
//!
//! On every file: the manifest's outcome; the package writes back to its own bytes; the
//! second reader agrees on every entry; it has no package problem; each write's plan, applied,
//! is the write; a spliced edit changes only that entry; and a three-way merge of two edits
//! apart holds both, every other entry theirs' raw record.

mod common;

use std::collections::BTreeMap;
use std::path::PathBuf;

use common::{limits, package_holds, plan_changes, plan_holds};
use k2_mpc_opc::merge::{merge3_bytes, Generic, Outcome};
use k2_mpc_opc::zip::Package;
use k2_mpc_opc::{name, Change, OpcPackage};

fn manifest() -> Vec<(String, String, String)> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/xlsxwriter.tsv");
    let text = std::fs::read_to_string(&p).expect("the manifest");
    text.lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            assert_eq!(f.len(), 3, "a manifest row has three fields");
            (f[0].to_owned(), f[1].to_owned(), f[2].to_owned())
        })
        .collect()
}

#[test]
fn the_manifest_names_each_file_once_with_a_hash() {
    let rows = manifest();
    assert_eq!(rows.len(), 1049);
    let mut seen = std::collections::BTreeSet::new();
    for (n, sum, expect) in &rows {
        assert!(seen.insert(n), "{n} twice");
        assert!(
            sum.len() == 64 && sum.bytes().all(|b| b.is_ascii_hexdigit()),
            "{n}"
        );
        assert!(expect == "ok" || expect.starts_with("decline:"), "{n}");
    }
}

/// An XML part to edit: the first worksheet, else the first `.xml` part in the zip.
fn edit_target(p: &OpcPackage<'_>, skip: &str) -> Option<String> {
    let keys: Vec<String> = p
        .zip
        .entries()
        .iter()
        .map(|e| name::key(&e.name))
        .filter(|k| k.ends_with(".xml") && k != name::CONTENT_TYPES && k != skip)
        .collect();
    keys.iter()
        .find(|k| k.starts_with("xl/worksheets/"))
        .or_else(|| keys.first())
        .cloned()
}

fn with_comment(c: &[u8], what: &str) -> Vec<u8> {
    let mut v = c.to_vec();
    v.extend_from_slice(format!("<!-- {what} -->").as_bytes());
    v
}

fn one_file(n: &str, bytes: &[u8], expect: &str) -> Result<(), String> {
    let l = limits();
    let p = match OpcPackage::open(bytes, &l) {
        Ok(p) if expect == "ok" => p,
        Ok(_) => return Err(format!("{n}: opened, not {expect}")),
        Err(d) if expect == format!("decline:{}", d.code) => return Ok(()),
        Err(d) => return Err(format!("{n}: {} ({}), not {expect}", d.code, d.message)),
    };
    package_holds(n, bytes)?;
    // each write's plan, applied, is the write
    for (what, ch) in plan_changes(&p) {
        plan_holds(&format!("{n} ({what})"), &p, &ch)?;
    }
    // a spliced edit changes only its entry
    let a = edit_target(&p, "").ok_or_else(|| format!("{n}: no XML part"))?;
    let ours = with_comment(&p.parts[&a].content, "ours");
    let mut ch = BTreeMap::new();
    ch.insert(a.clone(), Change::Replace(ours.clone()));
    let ob = p.write(&ch).map_err(|e| format!("{n}: write: {e}"))?;
    let (pz, oz) = (
        &p.zip,
        Package::read(&ob, &l).map_err(|e| format!("{n}: {e}"))?,
    );
    for (i, e) in pz.entries().iter().enumerate() {
        if name::key(&e.name) == a {
            if oz.inflate(i).map_err(|e| e.to_string())? != ours {
                return Err(format!("{n}: the edit isn't there"));
            }
        } else if oz.record_bytes(i) != pz.record_bytes(i) {
            return Err(format!("{n}: {} changed", e.name));
        }
    }
    // a merge of two edits apart
    let Some(b) = edit_target(&p, &a) else {
        return Ok(());
    };
    let theirs = with_comment(&p.parts[&b].content, "theirs");
    let mut ch = BTreeMap::new();
    ch.insert(b.clone(), Change::Replace(theirs.clone()));
    let tb = p.write(&ch).map_err(|e| format!("{n}: write: {e}"))?;
    let m = match merge3_bytes(bytes, &ob, &tb, &Generic, &l) {
        Outcome::Merged(m, _) => m,
        Outcome::Conflict(w) => return Err(format!("{n}: a conflict: {w}")),
        Outcome::Unsupported(d) => return Err(format!("{n}: unsupported: {d}")),
    };
    let q = OpcPackage::open(&m, &l).map_err(|e| format!("{n}: the merge: {e}"))?;
    if q.content(&a) != Some(&ours[..]) || q.content(&b) != Some(&theirs[..]) {
        return Err(format!("{n}: the merge lost an edit"));
    }
    let tz = Package::read(&tb, &l).map_err(|e| e.to_string())?;
    for (i, e) in tz.entries().iter().enumerate() {
        let k = name::key(&e.name);
        if k != a && q.zip.record_bytes(i) != tz.record_bytes(i) {
            return Err(format!("{n}: {} isn't theirs' record", e.name));
        }
    }
    Ok(())
}

#[test]
fn the_xlsxwriter_set_round_trips_and_merges() {
    let Some(dir) = std::env::var_os("OOXML_XLSXWRITER_CORPUS") else {
        println!("xlsxwriter corpus: skipped (OOXML_XLSXWRITER_CORPUS is not set)");
        return;
    };
    let dir = PathBuf::from(dir);
    let rows = manifest();
    let (mut ok, mut declined, mut bad) = (0, 0, Vec::new());
    for (n, _, expect) in &rows {
        let bytes = std::fs::read(dir.join(n)).unwrap_or_else(|_| panic!("{n}: not fetched"));
        match one_file(n, &bytes, expect) {
            Ok(()) if expect == "ok" => ok += 1,
            Ok(()) => declined += 1,
            Err(e) => bad.push(e),
        }
    }
    println!(
        "xlsxwriter corpus: {} files: {ok} byte-exact, spliced and merged; {declined} declined as listed; {} failed",
        rows.len(),
        bad.len()
    );
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}
