//! Test 3: the file registry `capabilities` lists. Every file verb the board crate has, in
//! `verbs::ARGS` order, and no other; each as a file verb, of its kind; its exits 0, those of
//! every `@file` row that names it, and the usage refusals' (MPC-E120, MPC-E122), which the
//! bridge's parser gives for any verb; the parity rows it serves, from `parity::ROWS`. Built once.

use k2_mpc_board_bridge::board::parity::{Way, ROWS};
use k2_mpc_board_bridge::board::verbs::{ARGS, OTHER_VERBS, WRITE_VERBS};
use k2_mpc_board_bridge::kit::errors::{
    capabilities, entries, lookup, run_meta, ErrorEntry, Value,
};
use k2_mpc_board_bridge::kit::parity::{Source, VerbKind, VerbSpec};
use k2_mpc_board_bridge::kit::session::Config;
use k2_mpc_board_bridge::parse::usage;
use k2_mpc_board_bridge::registry::{registry, serves_of};
use std::collections::BTreeSet;

fn verb(name: &str) -> &'static VerbSpec {
    let v = registry().get(name);
    assert!(v.is_some(), "the file verb {name} is listed");
    v.expect("check: listed")
}

/// The parser's usage refusals, which any verb can give.
const USAGE: &[&str] = &["MPC-E120", "MPC-E122"];

/// Every row a verb can give: the `@file` rows that name it, and the usage refusals.
fn rows_of(name: &str) -> Vec<&'static ErrorEntry> {
    let mut rows = file_rows(name);
    for id in USAGE {
        rows.push(lookup(id).expect("check: the kit has the usage rows"));
    }
    rows
}

/// The kit's rows marked `@file` that name `name`.
fn file_rows(name: &str) -> Vec<&'static ErrorEntry> {
    entries()
        .filter(|e| e.verbs.first() == Some(&"@file") && e.verbs[1..].contains(&name))
        .collect()
}

#[test]
fn every_file_verb_is_listed_and_no_other() {
    let listed: Vec<&str> = registry().verbs().iter().map(|v| v.name).collect();
    let args: Vec<&str> = ARGS.iter().map(|(n, _)| *n).collect();
    assert_eq!(listed, args, "the verbs, in ARGS order");
    // ARGS is the pack's: its write and other verbs, each once
    let pack: BTreeSet<&str> = WRITE_VERBS.iter().chain(OTHER_VERBS).copied().collect();
    assert_eq!(
        listed.iter().copied().collect::<BTreeSet<_>>(),
        pack,
        "no verb the pack lacks, and none of its left out"
    );
    assert_eq!(listed.len(), pack.len(), "each verb once");
    for v in registry().verbs() {
        assert_eq!(v.source, Source::File, "{}: a file verb", v.name);
    }
    assert!(
        registry().get("cat").is_none(),
        "a kit verb isn't one of the board's"
    );
}

#[test]
fn each_verb_is_of_its_kind() {
    const READS: &[&str] = &[
        "list",
        "read",
        "search",
        "fields",
        "templates",
        "references",
    ];
    for v in registry().verbs() {
        let want = if READS.contains(&v.name) {
            VerbKind::Read
        } else {
            VerbKind::Edit
        };
        assert_eq!(v.kind, want, "{}'s kind", v.name);
    }
    // the two that aren't in WRITE_VERBS but change the file
    assert_eq!(verb("apply").kind, VerbKind::Edit, "apply");
    assert_eq!(verb("inverse").kind, VerbKind::Edit, "inverse");
    assert_eq!(verb("read").kind, VerbKind::Read, "read");
}

/// Each verb's exits are 0, the exit of each `@file` row that names it, and the usage
/// refusals': every row's exit is there, and nothing else is.
#[test]
fn each_verbs_exits_are_its_rows() {
    for v in registry().verbs() {
        let rows = rows_of(v.name);
        assert!(v.exits.contains(&0), "{}: 0", v.name);
        assert!(v.exits.contains(&2), "{}: 2, a usage refusal", v.name);
        for e in &rows {
            assert!(
                v.exits.contains(&e.exit),
                "{}: {}'s exit {} is listed",
                v.name,
                e.id,
                e.exit
            );
        }
        for x in v.exits.iter().filter(|x| **x != 0) {
            assert!(
                rows.iter().any(|e| e.exit == *x),
                "{}: exit {x} comes from a row that names it",
                v.name
            );
        }
        let mut sorted = v.exits.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            v.exits,
            sorted.as_slice(),
            "{}: each exit once, in order",
            v.name
        );
    }
    // pinned, from the rows' own lists: read opens the file and looks an object up; vote can
    // also be refused on input and by voting; fields and templates read no file; any verb's
    // command line can be refused
    assert_eq!(verb("read").exits, &[0, 1, 2, 4], "read");
    assert_eq!(verb("vote").exits, &[0, 1, 2, 4, 5], "vote");
    assert_eq!(verb("fields").exits, &[0, 2], "fields");
    assert_eq!(verb("templates").exits, &[0, 2], "templates");
}

/// Each verb serves the parity rows whose way names it, in the list's order.
#[test]
fn each_verb_serves_its_parity_rows() {
    for v in registry().verbs() {
        let want: Vec<&str> = ROWS
            .iter()
            .filter(|r| matches!(r.way, Way::Verbs(vs) if vs.contains(&v.name)))
            .map(|r| r.name)
            .collect();
        assert!(!want.is_empty(), "{} serves a row", v.name);
        assert_eq!(v.serves_named, want.as_slice(), "{}'s rows", v.name);
        assert!(
            v.serves.is_empty(),
            "{}: none of the kit's own rows",
            v.name
        );
    }
    assert_eq!(
        verb("read").serves_named,
        &["read-board", "see-author"],
        "read"
    );
    assert_eq!(verb("inverse").serves_named, &["undo", "redo"], "inverse");
    // every verb a row names is one of the registry's
    for r in ROWS {
        if let Way::Verbs(vs) = r.way {
            for n in vs {
                assert!(registry().get(n).is_some(), "row {} names {n}", r.name);
            }
        }
    }
}

/// Each verb's parity rows, asked of the bridge's own list for it, before any registry is built.
#[test]
fn each_verbs_rows_are_the_lists_rows_that_name_it() {
    for (name, _) in ARGS {
        let want: Vec<&str> = ROWS
            .iter()
            .filter(|r| matches!(r.way, Way::Verbs(vs) if vs.contains(name)))
            .map(|r| r.name)
            .collect();
        assert_eq!(serves_of(name), want, "{name}'s rows");
    }
    assert_eq!(serves_of("read"), ["read-board", "see-author"], "read");
    assert_eq!(serves_of("list"), ["cut", "copy", "read-board"], "list");
}

/// An argument the parser takes as `--<field> <text>` pairs, not by its own name.
struct FieldsArg {
    verb: &'static str,
    arg: &'static str,
}

const FIELDS_ARGS: &[FieldsArg] = &[
    FieldsArg {
        verb: "add",
        arg: "fields",
    },
    FieldsArg {
        verb: "update",
        arg: "set",
    },
    FieldsArg {
        verb: "update",
        arg: "unset",
    },
    FieldsArg {
        verb: "connect",
        arg: "fields",
    },
    FieldsArg {
        verb: "meta",
        arg: "set",
    },
    FieldsArg {
        verb: "meta",
        arg: "unset",
    },
];

/// Each verb's usage line is the parser's, and names every argument the verb takes: a required
/// one as `<arg…>`, an optional one as `[--arg <arg…>]`, and fields as `--<field> <text>` pairs.
#[test]
fn each_verbs_usage_is_the_parsers_and_names_its_args() {
    for (name, args) in ARGS {
        let want = usage(name);
        assert!(want.is_some(), "the parser has a usage line for {name}");
        let v = verb(name);
        assert_eq!(Some(v.usage), want.as_deref(), "{name}: the parser's usage");
        assert!(
            v.usage.split(' ').next() == Some(*name),
            "{name}: the line starts with the verb: {}",
            v.usage
        );
        for a in *args {
            let bare = a.trim_end_matches('?');
            let named = if FIELDS_ARGS.iter().any(|f| f.verb == *name && f.arg == bare) {
                v.usage.contains(" <text>|null]…")
            } else if a.ends_with('?') {
                v.usage.contains(&format!("[--{bare} <{bare}"))
            } else {
                v.usage.contains(&format!(" <{bare}"))
            };
            assert!(named, "{name}: the usage names {a}: {}", v.usage);
        }
    }
}

#[test]
fn it_is_built_once() {
    assert!(std::ptr::eq(registry(), registry()), "the same registry");
}

/// What the kit's `capabilities` prints from it: every file verb, with its exits, and the ids of
/// the rows that name it.
#[test]
fn the_kits_capabilities_list_the_file_verbs() {
    let c = capabilities(&[registry()], &Config::default());
    let Some(Value::List(listed)) = c.get("verbs") else {
        panic!("capabilities lists verbs");
    };
    let names: Vec<&str> = listed
        .iter()
        .map(|v| v.get("name").and_then(Value::as_str).unwrap_or(""))
        .collect();
    let args: Vec<&str> = ARGS.iter().map(|(n, _)| *n).collect();
    assert_eq!(names, args, "capabilities' verbs");
    for v in listed {
        let name = v.get("name").and_then(Value::as_str).unwrap_or("");
        assert_eq!(
            v.get("source"),
            Some(&Value::from("file")),
            "{name}: a file verb"
        );
        assert_eq!(
            v.get("exits"),
            Some(&Value::list(verb(name).exits.iter().copied())),
            "{name}'s exits"
        );
        let ids: BTreeSet<String> = match v.get("errors") {
            Some(Value::List(l)) => l
                .iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect(),
            _ => BTreeSet::new(),
        };
        let want: BTreeSet<String> = file_rows(name).iter().map(|e| e.id.to_string()).collect();
        assert_eq!(ids, want, "{name}: the ids of the rows that name it");
    }
    let out = run_meta(
        &["capabilities".to_string(), "--json".to_string()],
        true,
        &[registry()],
        &Config::default(),
    )
    .expect("check: capabilities is a meta verb");
    assert_eq!(out.exit, 0, "capabilities exits 0");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("\"name\": \"table-remove-column\""),
        "the JSON names the board's verbs"
    );
}
