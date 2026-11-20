//! The parity coverage gate (`scripts/parity-coverage.py`, which says how
//! a case is linked to its form): every census form with no parity case
//! is listed in `tests/data/parity_uncovered.tsv`, and that list may only
//! fall. It fails when a form loses its last case, when a new census form
//! comes with none, when a tag names no form, and when the list is stale
//! because a form gained a case. The ship bar is an empty list.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn gate(root: &Path) -> Output {
    Command::new("python3")
        .arg(self::root().join("scripts/parity-coverage.py"))
        .args(["--check", "--root"])
        .arg(root)
        .output()
        .expect("run python3 scripts/parity-coverage.py --check")
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

#[test]
fn the_ceiling_holds() {
    let o = gate(&root());
    // The count and the uncovered forms by family, for the log.
    println!("{}", text(&o));
    assert!(o.status.success(), "{}", text(&o));
}

/// The files the gate reads, copied to a scratch tree.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("k5-coverage-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let copy_dir = |from: &str, keep: &dyn Fn(&str) -> bool| {
        let to = dir.join(from);
        std::fs::create_dir_all(&to).unwrap();
        for e in std::fs::read_dir(root().join(from)).unwrap() {
            let e = e.unwrap();
            let name = e.file_name().into_string().unwrap();
            if e.file_type().unwrap().is_file() && keep(&name) {
                std::fs::copy(e.path(), to.join(&name)).unwrap();
            }
        }
    };
    copy_dir("crates/k5-cli/tests/data", &|n| n.ends_with(".tsv"));
    copy_dir("crates/k5-cli/tests", &|n| n.ends_with("_parity.rs"));
    copy_dir("crates/k5-cli/src/families", &|n| n.ends_with(".rs"));
    copy_dir("crates/k5-cli/src", &|n| n == "route.rs");
    copy_dir("tests/cli", &|n| n.ends_with(".sh"));
    dir
}

/// Bites: the scratch tree as it is passes; with one parity row deleted
/// (its tag goes with it) the gate fails and names the form; a tag that
/// names no form fails; a form that gains a case makes the list stale.
#[test]
fn the_gate_bites() {
    let dir = scratch("bite");
    assert!(gate(&dir).status.success(), "{}", text(&gate(&dir)));

    // Delete the row that covers `whoami --json`: from its tag to the
    // `),` that closes its case.
    let file = dir.join("crates/k5-cli/tests/whoami_parity.rs");
    let src = std::fs::read_to_string(&file).unwrap();
    let lines: Vec<&str> = src.split('\n').collect();
    let at = lines
        .iter()
        .position(|l| l.trim() == "// covers: whoami --json")
        .expect("the whoami --json tag");
    let indent = &lines[at][..lines[at].len() - lines[at].trim_start().len()];
    let end = (at..lines.len())
        .find(|&i| lines[i] == format!("{indent}),"))
        .expect("the end of its case");
    let cut = [&lines[..at], &lines[end + 1..]].concat().join("\n");
    std::fs::write(&file, &cut).unwrap();
    let o = gate(&dir);
    assert!(!o.status.success(), "a deleted row passed: {}", text(&o));
    assert!(text(&o).contains("whoami\twhoami --json"), "{}", text(&o));
    std::fs::write(&file, &src).unwrap();

    // A tag that names no census form.
    std::fs::write(
        &file,
        src.replacen("// covers: whoami --json", "// covers: whoami --jsn", 1),
    )
    .unwrap();
    let o = gate(&dir);
    assert!(!o.status.success(), "a bad tag passed: {}", text(&o));
    assert!(
        text(&o).contains("'whoami --jsn' is no census form"),
        "{}",
        text(&o)
    );
    std::fs::write(&file, &src).unwrap();

    // A form the list names gains a case: the list is stale.
    let first = std::fs::read_to_string(dir.join("crates/k5-cli/tests/data/parity_uncovered.tsv"))
        .unwrap()
        .lines()
        .find(|l| !l.starts_with('#'))
        .expect("an uncovered form")
        .split('\t')
        .nth(1)
        .unwrap()
        .to_string();
    std::fs::write(&file, format!("{src}\n// covers: {first}\n")).unwrap();
    let o = gate(&dir);
    assert!(!o.status.success(), "a stale list passed: {}", text(&o));
    assert!(text(&o).contains("the ceiling is stale"), "{}", text(&o));

    std::fs::remove_dir_all(&dir).unwrap();
}
