//! The pack's corpus row on a folder of real files kept outside every repo (the private-corpus
//! job, `scripts/private-corpus.sh`). Every PSD and PSB file under the folder
//! `PSD_PRIVATE_CORPUS` names is read by the vendored reader (`PsdFile::from_bytes`) and written
//! back (`PsdFile::to_bytes`), which must give the same bytes. It prints counts only: per class
//! (psd, psb) the files, their bytes, and how many wrote back the same bytes, wrote back other
//! bytes, failed to write or were refused, and whether the class passed. Never a path, a file
//! name or any content: each line is checked before it is printed, and a line holding a path
//! separator or what could be a file's extension fails the row and is withheld. The row fails
//! without `PSD_PRIVATE_CORPUS`, and when any file doesn't write back its own bytes. The vendored
//! reader's own corpus test, which names files in its output, is not run.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use photocraft_psd::PsdFile;

/// The test-only plant: `MPC_PSD_PLANT=corpus-prints-a-name` makes the row try to print a
/// file's name, which its output check must stop.
const PLANT: &str = "MPC_PSD_PLANT";

#[derive(Default)]
struct Class {
    files: u64,
    bytes: u64,
    same: u64,
    other: u64,
    write_failed: u64,
    refused: u64,
}

/// Every PSD and PSB file under `dir`, with its class.
fn walk(dir: &Path, out: &mut Vec<(PathBuf, &'static str)>) {
    let entries = std::fs::read_dir(dir).expect("row: the corpus folder reads");
    for e in entries {
        let p = e.expect("row: a folder entry reads").path();
        if p.is_dir() {
            walk(&p, out);
        } else if let Some(c) = class(&p) {
            out.push((p, c));
        }
    }
}

/// The file's class by its extension, as the vendored reader's corpus test picks its files.
fn class(p: &Path) -> Option<&'static str> {
    let e = p.extension()?.to_str()?;
    if e.eq_ignore_ascii_case("psd") {
        Some("psd")
    } else if e.eq_ignore_ascii_case("psb") {
        Some("psb")
    } else {
        None
    }
}

/// Whether a line could show a path or a file name: a path separator, or a dot before a letter.
fn could_name_a_file(line: &str) -> bool {
    line.contains(['/', '\\'])
        || line
            .as_bytes()
            .windows(2)
            .any(|w| w[0] == b'.' && w[1].is_ascii_alphabetic())
}

/// Prints `line` only when it can't show a path or a file name.
fn say(line: &str) {
    assert!(
        !could_name_a_file(line),
        "row: an output line could show a path or a file name (the line is withheld)"
    );
    println!("{line}");
}

#[test]
#[ignore = "the private corpus: scripts/private-corpus.sh runs it"]
fn the_corpus_writes_back_its_own_bytes_counts_only() {
    let dir = std::env::var_os("PSD_PRIVATE_CORPUS")
        .map(PathBuf::from)
        .expect("PSD_PRIVATE_CORPUS: the corpus folder (scripts/private-corpus.sh)");
    let plant = std::env::var(PLANT).ok();
    match plant.as_deref() {
        None => say(&format!("{PLANT}=(unset)")),
        Some("corpus-prints-a-name") => say(&format!("{PLANT}=corpus-prints-a-name")),
        Some(_) => panic!("row: {PLANT} names no plant this row knows"),
    }
    let mut all = Vec::new();
    walk(&dir, &mut all);
    all.sort();
    let mut by: BTreeMap<&str, Class> = BTreeMap::new();
    for (p, c) in &all {
        if plant.is_some() {
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned());
            say(&format!("planted: {}", name.unwrap_or_default()));
        }
        let bytes = std::fs::read(p).expect("row: a corpus file reads");
        let k = by.entry(*c).or_default();
        k.files += 1;
        k.bytes += bytes.len() as u64;
        match PsdFile::from_bytes(&bytes) {
            Err(_) => k.refused += 1,
            Ok(f) => match f.to_bytes() {
                Ok(out) if out == bytes => k.same += 1,
                Ok(_) => k.other += 1,
                Err(_) => k.write_failed += 1,
            },
        }
    }
    let mut failed = 0;
    for (c, k) in &by {
        let bad = k.other + k.write_failed + k.refused;
        failed += bad;
        say(&format!(
            "class {c}: {} files, {} bytes; same {}, other {}, write failed {}, refused {}: {}",
            k.files,
            k.bytes,
            k.same,
            k.other,
            k.write_failed,
            k.refused,
            if bad == 0 { "pass" } else { "FAIL" }
        ));
    }
    let files: u64 = by.values().map(|k| k.files).sum();
    say(&format!(
        "corpus: {files} files in {} classes; {failed} didn't write back their own bytes",
        by.len()
    ));
    assert!(files > 0, "row: the corpus folder holds no PSD or PSB file");
    assert!(
        failed == 0,
        "row: {failed} files didn't write back their own bytes"
    );
}
