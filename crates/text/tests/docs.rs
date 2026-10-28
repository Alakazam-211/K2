//! The README's file-type table is the data's: every extension, name, prefix and not-text
//! extension in `types` appears there, and every key there is in the data.

use std::collections::BTreeSet;

use k2_mpc_text::types::{NOT_TEXT, TABLE};

fn readme() -> String {
    std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../README.md"))
        .expect("README.md")
}

#[test]
fn the_readme_lists_every_key_in_the_table() {
    let md = readme();
    for g in TABLE {
        let row = md
            .lines()
            .find(|l| l.starts_with(&format!("| {} | {} |", g.label, g.ty.family.name())))
            .unwrap_or_else(|| panic!("README has no row for group {}", g.label));
        for e in g.extensions {
            assert!(row.contains(&format!("`.{e}`")), "{}: .{e}", g.label);
        }
        for n in g.names {
            assert!(row.contains(&format!("`{n}`")), "{}: {n}", g.label);
        }
        for p in g.prefixes {
            assert!(row.contains(&format!("`{p}*`")), "{}: {p}*", g.label);
        }
    }
    for e in NOT_TEXT {
        assert!(md.contains(&format!("`.{e}`")), "not text: .{e}");
    }
}

#[test]
fn the_readme_lists_nothing_the_table_lacks() {
    let md = readme();
    let start = md.find("## The table").expect("a table section");
    let section = &md[start..];
    let mut known: BTreeSet<String> = BTreeSet::new();
    for g in TABLE {
        known.extend(g.extensions.iter().map(|e| format!(".{e}")));
        known.extend(g.names.iter().map(|n| n.to_string()));
        known.extend(g.prefixes.iter().map(|p| format!("{p}*")));
    }
    known.extend(NOT_TEXT.iter().map(|e| format!(".{e}")));
    let mut extra = Vec::new();
    for l in section
        .lines()
        .filter(|l| l.starts_with("| ") && !l.starts_with("| Group"))
    {
        for span in l.split('`').skip(1).step_by(2) {
            if !known.contains(span) {
                extra.push(span.to_owned());
            }
        }
    }
    assert!(
        extra.is_empty(),
        "README table lists keys the data lacks: {extra:?}"
    );
}
