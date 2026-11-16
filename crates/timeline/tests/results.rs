//! What an agent gets back from each live verb as data: the call's result (`mpc.result/1`, the
//! kit's, asked for with `--json`) of every verb on the live rows' timeline, and of a refusal
//! of each code these calls can reach, pinned whole as golden files in `tests/results_golden/`
//! (the kit's pretty JSON; re-bless with `MPC_BLESS_GOLDEN=1`, `kit_view_golden` at its top).
//! Each call runs on a fresh fixture, so a golden doesn't depend on another call.
//!
//! Beside the goldens: every refusal code the verbs print is one of the pack's registered
//! entries, the kit's registry takes the pack (its verbs and its entries), each result's `data`
//! is the verb's own (`data_schema` `typed`, `data_spec` `timeline.<verb>/1`), and each
//! refusal's `error` is its entry, by id.

// only `check_in` is used here
#[allow(dead_code)]
mod kit_view_golden;
mod live_support;

use std::collections::BTreeSet;

use k2_mpc_base::errors::Value as Json;
use k2_mpc_base::parity::Registry;
use k2_mpc_timeline::live::errors::ERRORS;
use k2_mpc_timeline::live::TimelineLive;
use live_support::{cut, fx, run, run_based};

/// The golden folder.
const DIR: &str = "results_golden";

/// The reads, by golden name.
const READS: &[(&str, &[&str])] = &[
    ("timeline-info", &["timeline", "info"]),
    ("track-list-page", &["track", "list", "--limit", "2"]),
    (
        "clip-list-page",
        &["clip", "list", "@trk:1", "--limit", "2"],
    ),
    ("clip-at", &["clip", "at", "@trk:1", "01:00:02:00"]),
    ("clip-show", &["clip", "show", "@item:1"]),
    ("marker-list", &["marker", "list"]),
    ("media-list", &["media", "list"]),
];

/// The edits, by golden name, each on its own base.
const EDITS: &[(&str, &[&str])] = &[
    ("clip-move", &["clip", "move", "@item:5", "--to", "+12f"]),
    ("clip-trim", &["clip", "trim", "@item:4", "--out", "-6f"]),
    ("clip-rename", &["clip", "rename", "@item:2", "Shot B2"]),
    ("clip-enable-off", &["clip", "enable", "@item:1", "--off"]),
    ("clip-delete", &["clip", "delete", "@item:6"]),
    (
        "clip-add",
        &[
            "clip",
            "add",
            "--media",
            "@med:3",
            "--track",
            "@trk:3",
            "--at",
            "01:00:02:00",
            "--in",
            "20f",
            "--dur",
            "10f",
            "--name",
            "C on V2",
        ],
    ),
    (
        "marker-add",
        &[
            "marker",
            "add",
            "--on",
            "@item:4",
            "--at",
            "01:00:04:12",
            "--name",
            "cue",
        ],
    ),
    (
        "marker-set",
        &["marker", "set", "@mk:1", "--name", "beat two"],
    ),
    ("marker-delete", &["marker", "delete", "@mk:2"]),
    ("track-rename", &["track", "rename", "@trk:3", "B-roll"]),
];

/// Refusals, by golden name, with the code each must give.
const REFUSALS: &[(&str, &str, &[&str])] = &[
    (
        "refused-overlap",
        "tl_overlap",
        &["clip", "move", "@item:5", "--to", "01:00:02:00"],
    ),
    (
        "refused-off-grid",
        "tl_off_grid",
        &["clip", "move", "@item:5", "--to", "+0.01s"],
    ),
    (
        "refused-time",
        "tl_time",
        &["clip", "move", "@item:5", "--to", "soon"],
    ),
    (
        "refused-transition",
        "tl_transition",
        &["clip", "delete", "@item:2"],
    ),
    (
        "refused-not-editable",
        "tl_not_editable",
        &["clip", "rename", "@item:3", "x"],
    ),
    (
        "refused-track-kind",
        "tl_track_kind",
        &[
            "clip", "move", "@item:5", "--to", "+0f", "--track", "@trk:2",
        ],
    ),
    (
        "refused-range",
        "tl_range",
        &["clip", "trim", "@item:5", "--out", "-24f"],
    ),
    (
        "refused-not-found",
        "tl_not_found",
        &["clip", "show", "@item:99"],
    ),
];

/// A field of a JSON object.
fn get<'a>(v: &'a Json, k: &str) -> Option<&'a Json> {
    match v {
        Json::Obj(pairs) => pairs.iter().find(|(x, _)| x == k).map(|(_, v)| v),
        _ => None,
    }
}

fn text(v: Option<&Json>) -> Option<&str> {
    match v {
        Some(Json::Str(s)) => Some(s),
        _ => None,
    }
}

/// `argv` with `--json`, as `c` on a fresh fixture of the cut (an edit on its own base): the
/// call's result.
fn result_of(tag: &str, argv: &[&str], edit: bool) -> Json {
    let f = fx(tag, |_| {});
    f.write(&cut());
    let s = f.open();
    let a = f.agent(&s);
    let mut all: Vec<&str> = argv.to_vec();
    all.push("--json");
    let out = if edit {
        run_based(&s, a, &all)
    } else {
        run(&s, a, &all)
    }
    .unwrap_or_else(|e| panic!("{argv:?}: the call runs: {e:?}"));
    out.result()
        .unwrap_or_else(|| panic!("{argv:?}: --json gives a result"))
        .clone()
}

#[test]
fn every_refusal_code_the_verbs_print_is_registered() {
    let src = include_str!("../src/live/verbs.rs");
    let mut used = BTreeSet::new();
    for (i, _) in src.match_indices("\"tl_") {
        let rest = &src[i + 1..];
        let end = rest.find('"').expect("a closing quote");
        used.insert(&rest[..end]);
    }
    assert!(used.len() >= 10, "the verbs' codes are found: {used:?}");
    let registered: BTreeSet<&str> = ERRORS.iter().map(|e| e.name).collect();
    let missing: Vec<&&str> = used.iter().filter(|c| !registered.contains(**c)).collect();
    assert!(missing.is_empty(), "codes with no entry: {missing:?}");
    let ids: BTreeSet<&str> = ERRORS.iter().map(|e| e.id).collect();
    assert_eq!(ids.len(), ERRORS.len(), "each entry its own id");
}

#[test]
fn the_kits_registry_takes_the_pack_its_verbs_and_its_entries() {
    let r = Registry::for_pack(&TimelineLive::default()).expect("the registry takes the pack");
    assert_eq!(r.errors().len(), ERRORS.len());
    for v in k2_mpc_timeline::live::verbs::VERBS {
        assert!(r.get(v.name()).is_some(), "{} is in the registry", v.name());
    }
}

#[test]
fn each_reads_result_is_its_data_pinned() {
    let mut failures = Vec::new();
    for (name, argv) in READS {
        let r = result_of(name, argv, false);
        assert_eq!(
            text(get(&r, "data_schema")),
            Some("typed"),
            "{name}: typed data"
        );
        assert_eq!(get(&r, "ok"), Some(&Json::Bool(true)), "{name}: ok");
        assert_eq!(
            text(get(&r, "data_spec")),
            Some(format!("timeline.{}/1", argv[..2].join("-")).as_str()),
            "{name}: the data's shape, named"
        );
        if let Err(e) = kit_view_golden::check_in(DIR, name, &r) {
            failures.push(e);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn each_edits_result_is_its_data_and_change_pinned() {
    let mut failures = Vec::new();
    for (name, argv) in EDITS {
        let r = result_of(name, argv, true);
        assert_eq!(
            text(get(&r, "data_schema")),
            Some("typed"),
            "{name}: typed data"
        );
        assert_eq!(get(&r, "ok"), Some(&Json::Bool(true)), "{name}: ok");
        assert_eq!(
            text(get(&r, "data_spec")),
            Some(format!("timeline.{}/1", argv[..2].join("-")).as_str()),
            "{name}: the data's shape, named"
        );
        assert_eq!(text(get(&r, "kind")), Some("edit"), "{name}: an edit");
        if let Err(e) = kit_view_golden::check_in(DIR, name, &r) {
            failures.push(e);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn each_refusal_is_its_registered_entry_pinned() {
    let mut failures = Vec::new();
    for (name, code, argv) in REFUSALS {
        let edit = argv[0] != "clip" || argv[1] != "show";
        let r = result_of(name, argv, edit);
        assert_eq!(get(&r, "ok"), Some(&Json::Bool(false)), "{name}: refused");
        let error = get(&r, "error").unwrap_or_else(|| panic!("{name}: an error"));
        let entry = ERRORS
            .iter()
            .find(|e| e.name == *code)
            .unwrap_or_else(|| panic!("{code} is registered"));
        assert_eq!(
            text(get(error, "id")),
            Some(entry.id),
            "{name}: its entry's id"
        );
        if let Err(e) = kit_view_golden::check_in(DIR, name, &r) {
            failures.push(e);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
