//! The keys that depend on what a conflict is about: a refused group's row by the call it came
//! from, each held group's why, and a merge's conflicts of every kind, in the stated order.

mod support;

use k2_mpc_board_bridge::board::json::{Fields, Value as BValue};
use k2_mpc_board_bridge::board::*;
use k2_mpc_board_bridge::kit::errors::Value;
use k2_mpc_board_bridge::report;
use k2_mpc_board_bridge::rows::Phase;
use support::*;

#[test]
fn a_refused_group_takes_the_row_of_the_call_it_came_from() {
    let c = Conflict::Refused(refusal(RefusalCode::JsonSyntax, Some("/ops/0")));
    for (phase, want) in [
        (Phase::Ops, "MPC-E849"),
        (Phase::Input, "MPC-E847"),
        (Phase::Open, "MPC-E845"),
        (Phase::Write, "MPC-E847"),
    ] {
        let r = report::conflict_report(&c, phase).expect("check: a refused group's report");
        assert_eq!(r.id(), want, "{phase:?}");
        let verb = verbs::VerbError::Conflict(c.clone());
        let r = report::verb_error_report(&verb, "apply", phase).expect("check: a verb's");
        assert_eq!(r.id(), want, "{phase:?}: through a verb");
    }
}

/// A held group as the test reads it.
#[derive(Debug, PartialEq)]
struct HeldRow<'a> {
    group: Option<&'a str>,
    what: Option<&'a str>,
    id: Option<&'a str>,
    why: Option<&'a str>,
    at: Option<&'a str>,
}

/// What a test wants of a held group: its id, what, id, why and where.
fn want<'a>(
    group: &'a str,
    what: Option<&'a str>,
    id: Option<&'a str>,
    why: &'a str,
    at: Option<&'a str>,
) -> HeldRow<'a> {
    HeldRow {
        group: Some(group),
        what,
        id,
        why: Some(why),
        at,
    }
}

fn held(id: &str, conflict: Conflict) -> Held {
    Held {
        group: Group {
            id: id.into(),
            by: None,
            ops: vec![],
        },
        conflict,
    }
}

#[test]
fn each_held_group_says_why_in_the_order_given() {
    let groups = [
        held("g-1", Conflict::Deleted { id: id("s-9") }),
        held(
            "g-2",
            Conflict::Refused(refusal(RefusalCode::FieldValue, Some("/objects/s-1/w"))),
        ),
        held(
            "g-3",
            Conflict::Exists {
                target: Target::Person(id(ME)),
                theirs: BValue::Obj(Fields::new()),
            },
        ),
        held(
            "g-4",
            Conflict::Changed {
                target: Target::Settings,
                theirs: BValue::Obj(Fields::new()),
            },
        ),
    ];
    let r = report::held_report(&groups).expect("check: the held groups' report");
    assert_eq!(r.detail("count").and_then(Value::as_u64), Some(4), "count");
    let Some(Value::List(list)) = r.detail("held") else {
        panic!("held is a list: {}", r.to_json());
    };
    let got: Vec<HeldRow<'_>> = list
        .iter()
        .map(|h| {
            let c = h.get("conflict");
            let k = |key: &str| c.and_then(|c| c.get(key)).and_then(Value::as_str);
            HeldRow {
                group: h.get("group").and_then(Value::as_str),
                what: k("what"),
                id: k("id"),
                why: k("why"),
                at: k("at"),
            }
        })
        .collect();
    assert_eq!(
        got,
        [
            want("g-1", Some("object"), Some("s-9"), "deleted", None),
            want("g-2", None, None, "field_value", Some("/objects/s-1/w")),
            want("g-3", Some("person"), Some(ME), "exists", None),
            want("g-4", Some("settings"), None, "changed", None),
        ],
        "each group, its what, id, why and at"
    );
    for h in list {
        let c = h.get("conflict").expect("check: a conflict");
        assert!(
            c.get("target").is_none(),
            "a held group has no target key: {}",
            c.to_json()
        );
    }
}

fn with(text: &str) -> Board {
    Board::from_json(text.as_bytes(), &Limits::V1).expect("check: the board")
}

#[test]
fn a_merges_conflicts_of_every_kind_in_the_stated_order() {
    let base = with(
        "{\"drop\":1,\"frames\":[],\"gone\":1,\"later\":1,\"objects\":[],\
\"people\":{\"p-1\":{\"name\":\"Ann\"}},\"settings\":{}}",
    );
    let ours = with(
        "{\"drop\":2,\"frames\":[],\"later\":2,\"objects\":[],\
\"people\":{\"p-1\":{\"name\":\"A\"},\"p-9\":{\"name\":\"X\"}},\"settings\":{}}",
    );
    let theirs = with(
        "{\"frames\":[],\"gone\":2,\"later\":3,\"objects\":[],\
\"people\":{\"p-1\":{\"name\":\"B\"},\"p-9\":{\"name\":\"Y\"}},\"settings\":{}}",
    );
    let e = merge3(&base, &ours, &theirs).expect_err("both sides changed the same things");
    let r = report::merge_report(&e).expect("check: the merge's report");
    assert_eq!(r.detail("count").and_then(Value::as_u64), Some(5), "count");
    let Some(Value::List(list)) = r.detail("conflicts") else {
        panic!("conflicts is a list: {}", r.to_json());
    };
    let got: Vec<(&str, &str, &str, [bool; 3])> = list
        .iter()
        .map(|c| {
            let k = |key: &str| c.get(key).and_then(Value::as_str).unwrap_or("");
            let has = |key: &str| c.get(key).is_some();
            (
                k("what"),
                k("id"),
                k("kind"),
                [has("base"), has("ours"), has("theirs")],
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            ("extra", "drop", "theirs_deleted", [true, true, false]),
            ("extra", "gone", "ours_deleted", [true, false, true]),
            ("extra", "later", "both_changed", [true, true, true]),
            ("person", "p-1", "both_changed", [true, true, true]),
            ("person", "p-9", "both_added", [false, true, true]),
        ],
        "top-level fields by name, then people by token, each with its sides"
    );
    let totals = r.detail("totals").expect("check: totals");
    for (k, n) in [("settings", 0), ("extra", 3), ("people", 2), ("objects", 0)] {
        assert_eq!(totals.get(k).and_then(Value::as_u64), Some(n), "totals.{k}");
    }
    let later = &list[2];
    for (side, n) in [("base", 1.0), ("ours", 2.0), ("theirs", 3.0)] {
        assert_eq!(
            later.get(side),
            Value::float(n).as_ref(),
            "later's {side} is the value itself"
        );
    }
}

#[test]
fn a_refusal_says_where_and_lists_the_bad_ids() {
    let r = report::refusal_report(
        &refusal(RefusalCode::FieldKind, Some("/objects/s-1/w")),
        Phase::Open,
    )
    .expect("check: a refusal's report");
    assert_eq!(
        r.detail("at").and_then(Value::as_str),
        Some("/objects/s-1/w"),
        "where"
    );
    let r = report::refusal_report(&refusal(RefusalCode::FieldKind, None), Phase::Open)
        .expect("check: a refusal's report");
    assert!(r.detail("at").is_none(), "no where, no at");
    assert!(r.detail("bad_ids").is_none(), "no bad ids, no list");
    let mut bad = refusal(RefusalCode::IdInvalid, None);
    bad.bad_ids = vec!["a b".into(), "c!".into()];
    bad.bad_ids_total = 3;
    bad.bad_ids_truncated = true;
    let r = report::refusal_report(&bad, Phase::Open).expect("check: a refusal's report");
    assert_eq!(
        r.detail("bad_ids"),
        Some(&Value::list(["a b", "c!"])),
        "the ids"
    );
    assert_eq!(
        r.detail("bad_ids_total").and_then(Value::as_u64),
        Some(3),
        "the total"
    );
    assert_eq!(
        r.detail("bad_ids_truncated"),
        Some(&Value::Bool(true)),
        "cut short"
    );
}

/// A held group refused for a number past 2^53 says why by its code, under the held groups' own
/// row, with no other row in it.
#[test]
fn a_held_refusal_says_its_code_and_names_no_other_row() {
    let c = Conflict::Refused(refusal(
        RefusalCode::JsonNumberRange,
        Some("/objects/s-9/later"),
    ));
    let r = report::held_report(&[held("g-1", c)]).expect("check: the held group's report");
    assert_eq!(r.id(), "MPC-E834");
    let Some(Value::List(list)) = r.detail("held") else {
        panic!("held is a list: {}", r.to_json());
    };
    let why = list[0]
        .get("conflict")
        .and_then(|c| c.get("why"))
        .and_then(Value::as_str);
    assert_eq!(why, Some("json_number_range"), "why is the code");
    let json = r.to_json();
    for other in ["MPC-E847", "MPC-E849", "MPC-E845"] {
        assert!(!json.contains(other), "no {other} in the report: {json}");
    }
}
