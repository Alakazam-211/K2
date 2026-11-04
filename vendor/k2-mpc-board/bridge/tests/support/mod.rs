//! What the bridge's tests share: a board, a deterministic host, and one sample of each refusal,
//! made the way the board crate makes it wherever it can be.

#![allow(dead_code)]

use k2_mpc_board_bridge::board::json::{Fields, Value};
use k2_mpc_board_bridge::board::verbs::{self, Ctx, Me, Planned, Verb, VerbError};
use k2_mpc_board_bridge::board::*;
use k2_mpc_board_bridge::kit::errors::ErrorReport;
use k2_mpc_board_bridge::report;
use k2_mpc_board_bridge::{HostBug, Phase};

pub const ME: &str = "p-1";

pub fn id(s: &str) -> Id {
    Id::new(s).expect("an id")
}

pub fn text(s: &str) -> Value {
    Value::Str(s.to_string())
}

pub fn fields(pairs: &[(&str, Value)]) -> Fields {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

pub fn board() -> Board {
    let text = "{\"frames\":[],\"objects\":[\
{\"id\":\"s-1\",\"text\":\"one\",\"type\":\"sticky\",\"x\":12.5,\"y\":0},\
{\"id\":\"s-2\",\"text\":\"two\",\"type\":\"sticky\",\"x\":200,\"y\":0},\
{\"id\":\"l-1\",\"locked\":true,\"text\":\"fixed\",\"type\":\"sticky\"},\
{\"checks\":[{\"done\":false,\"id\":\"c1\",\"text\":\"one\"}],\"id\":\"k-1\",\"type\":\"card\"},\
{\"id\":\"s-0000000000\",\"type\":\"sticky\"}],\
\"people\":{\"p-1\":{\"kind\":\"agent\",\"name\":\"Ann\"}},\
\"settings\":{\"title\":\"T\",\"voting\":{\"active\":false}}}";
    Board::from_json(text.as_bytes(), &Limits::V1).expect("the board")
}

pub fn obj<'b>(b: &'b Board, i: &str) -> &'b Object {
    b.objects
        .iter()
        .find(|o| o.id.as_str() == i)
        .unwrap_or_else(|| panic!("no object {i}"))
}

pub fn rev(b: &Board, i: &str) -> Sha256 {
    obj(b, i).rev().expect("a rev")
}

pub fn named(b: &Board, i: &str) -> verbs::ObjRev {
    verbs::ObjRev {
        id: id(i),
        rev: rev(b, i),
    }
}

/// A host: a counter for its random bytes (or one value, again and again), one actor, a clock.
pub fn plan_with(
    b: &Board,
    by: bool,
    same_random: bool,
    verb: &Verb,
) -> Result<Planned, VerbError> {
    let mut n = 1u64;
    let mut random = move || {
        if !same_random {
            n += 1;
        }
        if same_random {
            [0u8; 8]
        } else {
            n.to_le_bytes()
        }
    };
    let mut cx = Ctx {
        group_id: "g".to_string(),
        by: by.then(|| Me {
            token: id(ME),
            new: None,
        }),
        at: Some("2026-11-03T12:00:00Z".to_string()),
        random: &mut random,
    };
    verbs::plan(b, &mut cx, verb)
}

pub fn plan_err(b: &Board, verb: &Verb) -> VerbError {
    plan_with(b, true, false, verb).expect_err("the verb refuses")
}

/// A group that doesn't apply, and why.
pub fn conflict_of(b: &Board, ops: Vec<Op>) -> Conflict {
    let mut b = b.clone();
    let g = Group {
        id: "g".into(),
        by: Some(id(ME)),
        ops,
    };
    apply_group(&mut b, &g).expect_err("the group conflicts")
}

pub fn stale() -> Sha256 {
    Sha256::of(b"an old revision")
}

pub fn refusal(code: RefusalCode, at: Option<&str>) -> Refusal {
    match at {
        Some(at) => Refusal::at(code, at, "a sample"),
        None => Refusal::new(code, "a sample"),
    }
}

/// One sample per row: the row the bridge must give, and the report it gives.
pub fn samples() -> Vec<(&'static str, Result<ErrorReport, HostBug>)> {
    let b = board();
    let mut out: Vec<(&'static str, Result<ErrorReport, HostBug>)> = Vec::new();
    let mut push = |id: &'static str, r: Result<ErrorReport, HostBug>| out.push((id, r));
    let changed = conflict_of(
        &b,
        vec![Op::Set {
            id: id("s-1"),
            rev: stale(),
            set: fields(&[("text", text("x"))]),
            unset: vec![],
        }],
    );
    push("MPC-E830", report::conflict_report(&changed, Phase::Input));
    let exists = conflict_of(
        &b,
        vec![Op::Add {
            object: obj(&b, "s-2").clone(),
            at: Position::Front,
        }],
    );
    push("MPC-E831", report::conflict_report(&exists, Phase::Input));
    push(
        "MPC-E832",
        report::save_report(&SaveError::Stale {
            read: Sha256::of(b"read"),
            on_disk: Sha256::of(b"on disk"),
        }),
    );
    let mut ours = b.clone();
    let mut theirs = b.clone();
    for (side, t) in [(&mut ours, "ours"), (&mut theirs, "theirs")] {
        let o = side
            .objects
            .iter_mut()
            .find(|o| o.id.as_str() == "s-1")
            .expect("s-1");
        o.fields.insert("text".into(), text(t));
        side.settings.insert("title".into(), text(t));
    }
    let merged = merge3(&b, &ours, &theirs).expect_err("both sides changed s-1 and the title");
    push("MPC-E833", report::merge_report(&merged));
    let held = Held {
        group: Group {
            id: "g-7".into(),
            by: None,
            ops: vec![],
        },
        conflict: changed.clone(),
    };
    push("MPC-E834", report::held_report(&[held]));
    let locked = conflict_of(
        &b,
        vec![Op::Set {
            id: id("l-1"),
            rev: rev(&b, "l-1"),
            set: fields(&[("text", text("x"))]),
            unset: vec![],
        }],
    );
    push("MPC-E835", report::conflict_report(&locked, Phase::Input));
    let vote = |d| Verb::Vote {
        id: id("s-1"),
        delta: d,
    };
    push(
        "MPC-E836",
        report::verb_error_report(&plan_err(&b, &vote(1)), "vote", Phase::Input),
    );
    let mut voting = b.clone();
    voting.settings.insert(
        "voting".into(),
        Value::Obj(fields(&[
            ("active", Value::Bool(true)),
            ("perPerson", Value::Num(1.0)),
        ])),
    );
    voting
        .objects
        .iter_mut()
        .find(|o| o.id.as_str() == "s-2")
        .expect("s-2")
        .fields
        .insert("votes".into(), Value::Obj(fields(&[(ME, Value::Num(1.0))])));
    push(
        "MPC-E837",
        report::verb_error_report(&plan_err(&voting, &vote(1)), "vote", Phase::Input),
    );
    // a verb that looks the object up when it plans
    let gone = Verb::React {
        id: id("nope"),
        emoji: "+1".into(),
        on: None,
    };
    push(
        "MPC-E838",
        report::verb_error_report(&plan_err(&b, &gone), "react", Phase::Input),
    );
    let template = Verb::Template {
        name: "nope".into(),
        x: 0.0,
        y: 0.0,
    };
    push(
        "MPC-E839",
        report::verb_error_report(&plan_err(&b, &template), "template", Phase::Input),
    );
    let check = Verb::CheckSet {
        id: id("k-1"),
        rev: rev(&b, "k-1"),
        item: "c9".into(),
        text: None,
        done: Some(true),
    };
    push(
        "MPC-E840",
        report::verb_error_report(&plan_err(&b, &check), "check-set", Phase::Input),
    );
    let blob = format!("blobs/{}", Sha256::of(b"a picture"));
    push(
        "MPC-E841",
        report::refusal_report(
            &refusal(RefusalCode::BlobMissing, Some(&blob)),
            Phase::Write,
        ),
    );
    push(
        "MPC-E842",
        report::refusal_report(
            &refusal(RefusalCode::ZipCrc, Some("board.json")),
            Phase::Open,
        ),
    );
    push(
        "MPC-E843",
        report::refusal_report(&refusal(RefusalCode::ManifestInvalid, None), Phase::Open),
    );
    let mut newer = refusal(RefusalCode::MinReaderTooHigh, None);
    newer.needs_reader = Some(2);
    push("MPC-E844", report::refusal_report(&newer, Phase::Open));
    push(
        "MPC-E845",
        report::refusal_report(&refusal(RefusalCode::JsonSyntax, Some("/")), Phase::Open),
    );
    push(
        "MPC-E846",
        report::refusal_report(&refusal(RefusalCode::LimitObjects, None), Phase::Open),
    );
    let bad = Verb::Add {
        kind: ObjectType::Sticky,
        fields: fields(&[("w", text("wide"))]),
        at: Position::Front,
    };
    let bad = plan_with(&b, true, false, &bad)
        .map(|p| {
            let mut next = b.clone();
            apply_group(&mut next, &p.group)
        })
        .expect("check: a bad width plans, and the group refuses");
    let bad = bad.expect_err("a width must be a number");
    push("MPC-E847", report::conflict_report(&bad, Phase::Input));
    push(
        "MPC-E848",
        report::refusal_report(&refusal(RefusalCode::LimitObjects, None), Phase::Write),
    );
    let ops = verbs::group_from_json(b"{\"ops\":", &Limits::V1).expect_err("not JSON");
    push("MPC-E849", report::refusal_report(&ops, Phase::Ops));
    let unknown_field = Verb::Add {
        kind: ObjectType::Sticky,
        fields: fields(&[("nope", text("x"))]),
        at: Position::Front,
    };
    push(
        "MPC-E850",
        report::verb_error_report(&plan_err(&b, &unknown_field), "add", Phase::Input),
    );
    let unknown_type = Verb::Add {
        kind: ObjectType::parse("zzz").expect("check: a type the format may hold"),
        fields: fields(&[]),
        at: Position::Front,
    };
    push(
        "MPC-E851",
        report::verb_error_report(&plan_err(&b, &unknown_type), "add", Phase::Input),
    );
    push(
        "MPC-E852",
        report::verb_error_report(&plan_err(&b, &vote(0)), "vote", Phase::Input),
    );
    let reply = Verb::Reply {
        id: id("s-1"),
        text: "hi".into(),
    };
    push(
        "MPC-E853",
        report::verb_error_report(&plan_err(&b, &reply), "reply", Phase::Input),
    );
    let group = Verb::Group {
        ids: vec![named(&b, "s-1")],
    };
    push(
        "MPC-E854",
        report::verb_error_report(&plan_err(&b, &group), "group", Phase::Input),
    );
    let react = Verb::React {
        id: id("s-1"),
        emoji: "+1".into(),
        on: None,
    };
    let nobody = plan_with(&b, false, false, &react).expect_err("no one is acting");
    push(
        "MPC-E855",
        report::verb_error_report(&nobody, "react", Phase::Input),
    );
    let add = Verb::Add {
        kind: ObjectType::Sticky,
        fields: fields(&[]),
        at: Position::Front,
    };
    let full = plan_with(&b, false, true, &add).expect_err("every random id is in use");
    push(
        "MPC-E856",
        report::verb_error_report(&full, "add", Phase::Input),
    );
    push(
        "MPC-E857",
        report::refusal_report(&refusal(RefusalCode::IoError, None), Phase::Write),
    );
    let settings = conflict_of(
        &b,
        vec![Op::Meta {
            rev: stale(),
            set: fields(&[("title", text("x"))]),
            unset: vec![],
        }],
    );
    push("MPC-E858", report::conflict_report(&settings, Phase::Input));
    let person = conflict_of(
        &b,
        vec![Op::AddPerson {
            token: id(ME),
            person: Person {
                name: Some("Someone else".into()),
                kind: None,
                extra: Fields::new(),
            },
        }],
    );
    push("MPC-E859", report::conflict_report(&person, Phase::Input));
    out
}

/// The `{key}`s of a message template, in order.
pub fn template_keys(template: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        let close = after.find('}').expect("check: a template's braces close");
        keys.push(after[..close].to_string());
        rest = &after[close + 1..];
    }
    keys
}

/// The rows the board's file verbs can give: MPC-E830 to MPC-E859.
pub fn board_rows() -> Vec<String> {
    (830..=859).map(|n| format!("MPC-E{n}")).collect()
}
