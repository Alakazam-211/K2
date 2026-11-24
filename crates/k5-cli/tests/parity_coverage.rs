//! The parity coverage gate (`scripts/parity-coverage.py`, which says how
//! a case is linked to its form): every census form with no parity case
//! is listed in `tests/data/parity_uncovered.tsv`, and that list may only
//! fall. It fails when a form loses its last case, when a new census form
//! comes with none, when a tag names no form, and when the list is stale
//! because a form gained a case. The ship bar is an empty list.
//!
//! K5-only forms (`tests/data/k5_only_forms.tsv`) are out of
//! the count; each names the unit tests that cover it, and those must be
//! `#[test]` fns in their files. A form leaves the count only by that list.
//!
//! A check of the repo's sources by its Python script, the same on every
//! OS: it runs where the scripts' `python3` is (macOS and Linux).
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn gate(root: &Path) -> Output {
    run(root, &["--check"])
}

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new("python3")
        .arg(self::root().join("scripts/parity-coverage.py"))
        .args(args)
        .arg("--root")
        .arg(root)
        .output()
        .expect("run python3 scripts/parity-coverage.py")
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
    // Every test file: the parity files carry the tags, and the K5-only
    // list names unit tests in the others.
    copy_dir("crates/k5-cli/tests", &|n| n.ends_with(".rs"));
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

    // A `native:` tag names a router form: one that does passes, one
    // that names none fails.
    std::fs::write(&file, format!("{src}\n// native: publish status\n")).unwrap();
    assert!(gate(&dir).status.success(), "{}", text(&gate(&dir)));
    std::fs::write(&file, format!("{src}\n// native: publish statuz\n")).unwrap();
    let o = gate(&dir);
    assert!(!o.status.success(), "a bad native tag passed: {}", text(&o));
    assert!(
        text(&o).contains("'publish statuz' is no router form"),
        "{}",
        text(&o)
    );
    std::fs::write(&file, &src).unwrap();

    // The list names a form that has a case: the list is stale. (The
    // list may be empty, the ship bar, so a covered form is put on it.)
    let list = dir.join("crates/k5-cli/tests/data/parity_uncovered.tsv");
    let committed = std::fs::read_to_string(&list).unwrap();
    std::fs::write(&list, format!("{committed}whoami\twhoami --json\n")).unwrap();
    let o = gate(&dir);
    assert!(!o.status.success(), "a stale list passed: {}", text(&o));
    assert!(text(&o).contains("the ceiling is stale"), "{}", text(&o));
    assert!(text(&o).contains("whoami --json"), "{}", text(&o));

    std::fs::remove_dir_all(&dir).unwrap();
}

/// The K5-only list's bites: the scratch tree as it is passes; a form
/// taken off the list comes back into the count, uncovered; a named test
/// that is no `#[test]` fn fails; a listed form that is no census form
/// fails; a parity tag on a K5-only form fails; and a ceiling form that
/// left the census without the list fails `--check` and `--write`, unless
/// `--write --accept-gone`.
#[test]
fn the_k5_only_list_bites() {
    let dir = scratch("k5-only");
    assert!(gate(&dir).status.success(), "{}", text(&gate(&dir)));
    let list = dir.join("crates/k5-cli/tests/data/k5_only_forms.tsv");
    let src = std::fs::read_to_string(&list).unwrap();
    assert!(src.lines().any(|l| l.starts_with("cli status\t")), "{src}");
    assert!(
        src.lines().any(|l| l.starts_with("cli status --json\t")),
        "{src}"
    );

    // Off the list: `cli status` is counted again, with no case.
    let off: String = src
        .lines()
        .filter(|l| !l.starts_with("cli status\t"))
        .map(|l| format!("{l}\n"))
        .collect();
    std::fs::write(&list, off).unwrap();
    let o = gate(&dir);
    assert!(
        !o.status.success(),
        "a form off the list passed: {}",
        text(&o)
    );
    assert!(text(&o).contains("(front)\tcli status\n"), "{}", text(&o));
    std::fs::write(&list, &src).unwrap();

    // A named test that is no #[test] fn.
    std::fs::write(
        &list,
        src.replacen(
            "::cli_status_json_parses_and_matches_the_table",
            "::no_such_test",
            1,
        ),
    )
    .unwrap();
    let o = gate(&dir);
    assert!(!o.status.success(), "a missing test passed: {}", text(&o));
    assert!(
        text(&o).contains("no #[test] fn no_such_test"),
        "{}",
        text(&o)
    );
    std::fs::write(&list, &src).unwrap();

    // A listed form that is no census form.
    let stray = "cli statuz\tcrates/k5-cli/tests/front_native.rs::only_the_exact_pair_checks\n";
    std::fs::write(&list, format!("{src}{stray}")).unwrap();
    let o = gate(&dir);
    assert!(!o.status.success(), "a stray form passed: {}", text(&o));
    assert!(
        text(&o).contains("'cli statuz' is no census form"),
        "{}",
        text(&o)
    );
    std::fs::write(&list, &src).unwrap();

    // A parity tag on a K5-only form.
    let tagged = dir.join("crates/k5-cli/tests/whoami_parity.rs");
    let whoami = std::fs::read_to_string(&tagged).unwrap();
    std::fs::write(&tagged, format!("{whoami}\n// covers: cli status\n")).unwrap();
    let o = gate(&dir);
    assert!(
        !o.status.success(),
        "a tag on a K5-only form passed: {}",
        text(&o)
    );
    assert!(text(&o).contains("'cli status' is K5-only"), "{}", text(&o));
    std::fs::write(&tagged, &whoami).unwrap();

    // A ceiling form that left the census, not on the list.
    let ceiling = dir.join("crates/k5-cli/tests/data/parity_uncovered.tsv");
    let ceil = std::fs::read_to_string(&ceiling).unwrap();
    std::fs::write(&ceiling, format!("{ceil}(front)\tgone form\n")).unwrap();
    for args in [&["--check"][..], &["--write"][..]] {
        let o = run(&dir, args);
        assert!(
            !o.status.success(),
            "{args:?}: a gone form passed: {}",
            text(&o)
        );
        assert!(
            text(&o).contains("left the census and are not on the K5-only list"),
            "{}",
            text(&o)
        );
    }
    let o = run(&dir, &["--write", "--accept-gone"]);
    assert!(o.status.success(), "{}", text(&o));
    assert!(gate(&dir).status.success(), "{}", text(&gate(&dir)));

    std::fs::remove_dir_all(&dir).unwrap();
}
