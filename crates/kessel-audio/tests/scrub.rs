//! Publish hygiene: the crate's files must not name internal hosts, paths,
//! environment variables or products of the team that wrote it. The
//! patterns are assembled at run time so this file doesn't match itself.

use std::path::{Path, PathBuf};

fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).expect("read dir") {
        let e = e.expect("entry");
        let p = e.path();
        let name = e.file_name();
        let name = name.to_string_lossy();
        if name == "target" || name.starts_with('.') {
            continue;
        }
        if p.is_dir() {
            files(&p, out);
        } else {
            out.push(p);
        }
    }
}

#[test]
fn no_internal_names() {
    let k = ["K", "2"].concat();
    let lower = k.to_lowercase();
    let banned: Vec<String> = vec![
        format!("{k}_"),
        format!("~/.{lower}"),
        format!(".{lower}/"),
        format!("{lower}.dev"),
        format!("{lower}::"),
        format!("{lower}-daemon"),
        format!("{lower}-core"),
        format!("{lower}n1"),
        format!("{lower}n9"),
        format!("{lower}-arch"),
        format!("z3{}", "mbp"),
        format!("sew{}", "lab"),
        format!("{}-211", "Alakazam"),
    ];
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut all = Vec::new();
    files(root, &mut all);
    assert!(all.len() > 10, "found only {} files", all.len());
    let mut hits = Vec::new();
    for f in &all {
        // The test speech corpus is binary Opus data (see tests/data/README.md).
        if f.extension().is_some_and(|e| e == "kaop") {
            continue;
        }
        let text = std::fs::read_to_string(f)
            .unwrap_or_else(|e| panic!("{}: not UTF-8 text: {e}", f.display()));
        for (i, line) in text.lines().enumerate() {
            for b in &banned {
                if line.contains(b.as_str()) {
                    hits.push(format!("{}:{}: {b:?}", f.display(), i + 1));
                }
            }
            // The product name as a word.
            let mut words = line.split(|c: char| !c.is_ascii_alphanumeric());
            if words.any(|w| w == k.as_str()) {
                hits.push(format!("{}:{}: the word {k:?}", f.display(), i + 1));
            }
        }
    }
    assert!(hits.is_empty(), "internal names found:\n{}", hits.join("\n"));
}
