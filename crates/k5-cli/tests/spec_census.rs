//! The spec verbs' census, `tests/data/spec_census.tsv`: one row per
//! `k2 --schema` entry with its verb forms, where bash answers it, what it
//! sends and prints, and whether it can be a spec verb
//! (`scripts/spec-census.py`, which says how each column is read). It is
//! read from cli/src, so it must be current, and its spec rows must be the
//! spec table's.

use std::path::PathBuf;
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn census() -> String {
    let p = root().join("crates/k5-cli/tests/data/spec_census.tsv");
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

#[test]
fn the_census_is_current() {
    let o = Command::new("python3")
        .arg(root().join("scripts/spec-census.py"))
        .arg("--check")
        .output()
        .expect("run python3 scripts/spec-census.py --check");
    assert!(
        o.status.success(),
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
}

#[test]
fn its_spec_rows_are_the_table_s() {
    let text = census();
    let mut rows = text.lines().filter(|l| !l.starts_with('#'));
    assert_eq!(
        rows.next(),
        Some("entry\tforms\twhere\tvia\toutput\tkind\ttier")
    );
    let mut spec: Vec<&str> = Vec::new();
    let mut forms = 0;
    let mut entries = 0;
    for r in rows {
        let cols: Vec<&str> = r.split('\t').collect();
        assert_eq!(cols.len(), 7, "{r}");
        entries += 1;
        forms += cols[1].parse::<usize>().expect("forms is a number");
        if cols[5] == "spec" {
            spec.push(cols[0]);
        }
    }
    let table: Vec<&str> = k5_cli::spec::table::SPEC.iter().map(|v| v.entry).collect();
    assert_eq!(spec, table);
    let head = format!("# {entries} entries, {forms} verb forms");
    assert!(text.lines().nth(1) == Some(head.as_str()), "{head}");
}

/// K2's ruling: a row is anchored on the function and the arm word, never
/// a line number, so an edit elsewhere in a file moves no row. `where` is
/// `-` or `cli/src/<file>:<function>:<words>`, with a `~` when no arm names
/// the last word.
#[test]
fn where_is_a_function_and_its_arm_words() {
    let text = census();
    let ident = |s: &str| s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    let mut anchored = 0;
    for r in text.lines().filter(|l| !l.starts_with('#')).skip(1) {
        let cols: Vec<&str> = r.split('\t').collect();
        let place = cols[2];
        if place == "-" {
            continue;
        }
        let parts: Vec<&str> = place.split(':').collect();
        assert_eq!(parts.len(), 3, "{r}");
        assert!(
            parts[0].starts_with("cli/src/") && parts[0].ends_with(".sh"),
            "{r}"
        );
        assert!(ident(parts[1]), "{r}: the function");
        let words = parts[2].strip_suffix('~').unwrap_or(parts[2]);
        assert!(
            !words.is_empty() || parts[2].ends_with('~'),
            "{r}: an exact row names its arm word"
        );
        assert!(
            !words
                .split(' ')
                .any(|w| !w.is_empty() && w.bytes().all(|b| b.is_ascii_digit())),
            "{r}: a line number"
        );
        // The words are the entry's own, in its order.
        if !words.is_empty() {
            assert!(
                format!(" {} ", cols[0]).contains(&format!(" {words} ")),
                "{r}: the arm words are not the entry's"
            );
        }
        anchored += 1;
    }
    assert!(anchored > 800, "only {anchored} anchored rows");
}
