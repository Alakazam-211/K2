//! Test 5: a call's JSON. A refusal is the kit's report whole, plus `code` (the row's name) and the
//! echo under `on` (kind, path, version, mode `file`); one refusal for each exit class the board's
//! rows give. A host bug has no row, so no id and no exit. An answer is the echo, whether it was a
//! dry run, and the value. Board values in either print as `read` gives them: 12.5, and -0.0 as 0.

mod support;

use k2_mpc_board_bridge::board::json::Value as BValue;
use k2_mpc_board_bridge::board::*;
use k2_mpc_board_bridge::json::{answer_json, refused_json};
use k2_mpc_board_bridge::kit::errors::{ErrorReport, Value as KValue};
use k2_mpc_board_bridge::rows::Phase;
use k2_mpc_board_bridge::{read_value, report, Answer, HostBug, Refused, Why};
use support::*;

const PATH: &str = "plans/q4.k5board";

fn on(version: Option<&str>) -> On {
    On {
        kind: FORMAT,
        path: PATH.to_string(),
        version: version.map(str::to_string),
        mode: Mode::File,
    }
}

fn guard() -> String {
    Sha256::of(b"the file as read").to_string()
}

fn sample(id: &str) -> ErrorReport {
    let (_, r) = samples()
        .into_iter()
        .find(|(i, _)| *i == id)
        .unwrap_or_else(|| panic!("no sample {id}"));
    r.unwrap_or_else(|b| panic!("{id}'s sample is a host bug: {b}"))
}

/// The echo, as the JSON carries it.
fn assert_echo(v: &KValue, version: Option<&str>) {
    let echo = v
        .get("on")
        .expect("check: the JSON carries the echo under `on`");
    assert_eq!(echo.get("kind"), Some(&KValue::from("k5board")), "on.kind");
    assert_eq!(echo.get("path"), Some(&KValue::from(PATH)), "on.path");
    assert_eq!(
        echo.get("version"),
        Some(&version.map_or(KValue::Null, KValue::from)),
        "on.version"
    );
    assert_eq!(echo.get("mode"), Some(&KValue::from("file")), "on.mode");
}

/// One refusal for each exit class the board's rows give: 1, 2, 4 and 5.
#[test]
fn a_refusal_is_the_kits_report_with_code_and_the_echo() {
    let g = guard();
    for (id, exit) in [
        ("MPC-E830", 1u64),
        ("MPC-E853", 2),
        ("MPC-E838", 4),
        ("MPC-E835", 5),
    ] {
        let r = sample(id);
        let kit = r.to_value();
        let v = refused_json(&Refused {
            on: on(Some(&g)),
            why: Why::Report(r.clone()),
        });
        assert_eq!(v.get("id"), Some(&KValue::from(id)), "{id}: id");
        assert_eq!(
            v.get("exit").and_then(KValue::as_u64),
            Some(exit),
            "{id}: exit"
        );
        assert_eq!(
            v.get("code"),
            Some(&KValue::from(r.entry().name)),
            "{id}: code is the row's name"
        );
        assert!(
            matches!(v.get("details"), Some(KValue::Obj(_))),
            "{id}: details is an object"
        );
        // the kit's report, whole: every key it gives, with its value
        let KValue::Obj(pairs) = &kit else {
            panic!("{id}: the kit's report is an object");
        };
        for (k, want) in pairs {
            assert_eq!(v.get(k), Some(want), "{id}: the kit's {k}");
        }
        assert_echo(&v, Some(&g));
    }
}

/// A refusal from `open` read no guard: its version is null.
#[test]
fn a_refusal_before_the_file_was_read_has_no_version() {
    let r = sample("MPC-E842");
    let v = refused_json(&Refused {
        on: on(None),
        why: Why::Report(r),
    });
    assert_eq!(v.get("id"), Some(&KValue::from("MPC-E842")), "id");
    assert_eq!(v.get("code"), Some(&KValue::from("board_zip")), "code");
    assert_echo(&v, None);
}

/// A host bug has no row: no id, no code, and a null exit, so the host gives its own.
#[test]
fn a_host_bug_has_no_row_and_no_exit() {
    let bug = HostBug::Unmapped {
        code: "zip_a_new_one",
    };
    let refused = Refused {
        on: on(Some(&guard())),
        why: Why::HostBug(bug.clone()),
    };
    assert_eq!(refused.why.exit(), None, "Why::exit");
    let v = refused_json(&refused);
    assert_eq!(v.get("exit"), Some(&KValue::Null), "exit");
    assert_eq!(
        v.get("host_bug"),
        Some(&KValue::from(bug.to_string())),
        "the bug, in words"
    );
    assert_eq!(v.get("id"), None, "no row, so no id");
    assert_eq!(v.get("code"), None, "no row, so no code");
    assert_echo(&v, Some(&guard()));
}

/// A success: the echo, the dry-run flag and the value, as given.
#[test]
fn an_answer_is_the_echo_the_dry_run_flag_and_the_value() {
    let g = guard();
    let value = KValue::obj([("ids", KValue::list(["s-9"]))]);
    for dry_run in [false, true] {
        let v = answer_json(&Answer {
            on: on(Some(&g)),
            value: value.clone(),
            dry_run,
        });
        assert_echo(&v, Some(&g));
        assert_eq!(
            v.get("dry_run"),
            Some(&KValue::Bool(dry_run)),
            "dry_run {dry_run}"
        );
        assert_eq!(v.get("value"), Some(&value), "the value, as given");
        assert_eq!(v.get("id"), None, "an answer carries no row");
    }
}

/// Through the JSON path: a refusal's `theirs` and `read`'s answer print the same numbers, 12.5 as
/// 12.5 and -0.0 as 0, never `-0`. (B2's `values.rs` holds the report against `read` itself.)
#[test]
fn the_json_prints_board_numbers_as_read_does() {
    let mut b = board();
    b.objects
        .iter_mut()
        .find(|o| o.id.as_str() == "s-1")
        .expect("check: s-1 is on the board")
        .fields
        .insert("y".into(), BValue::Num(-0.0));
    let c = conflict_of(
        &b,
        vec![Op::Set {
            id: id("s-1"),
            rev: stale(),
            set: fields(&[("text", text("x"))]),
            unset: vec![],
        }],
    );
    let r = report::conflict_report(&c, Phase::Input).expect("check: a conflict's report");
    let refused = refused_json(&Refused {
        on: on(Some(&guard())),
        why: Why::Report(r),
    });
    let view = verbs::read(&b, &id("s-1"))
        .expect("check: read reads")
        .expect("check: s-1 is there");
    let answered = answer_json(&Answer {
        on: on(Some(&guard())),
        value: read_value(&view).expect("check: read's value"),
        dry_run: false,
    });
    let theirs = refused
        .get("details")
        .and_then(|d| d.get("theirs"))
        .expect("check: details.theirs");
    let read = answered
        .get("value")
        .and_then(|v| v.get("object"))
        .expect("check: read's object");
    for field in ["x", "y"] {
        assert_eq!(
            theirs.get(field).map(KValue::to_json),
            read.get(field).map(KValue::to_json),
            "{field}: the refusal prints what read prints"
        );
    }
    assert_eq!(
        theirs.get("x").map(KValue::to_json).as_deref(),
        Some("12.5"),
        "x"
    );
    assert_eq!(
        theirs.get("y").map(KValue::to_json).as_deref(),
        Some("0"),
        "-0.0 prints as 0"
    );
    let text = refused.to_json();
    assert!(
        text.contains("\"y\":0"),
        "the whole JSON says y is 0: {text}"
    );
    assert!(!text.contains(":-0"), "no number prints as -0: {text}");
}
