//! What the verb tests share: a board, a deterministic host, and small readers.

#![allow(dead_code)]

use k2_mpc_board::json::{Fields, Value};
use k2_mpc_board::verbs::{self, Ctx, Me, Planned, Verb, VerbError};
use k2_mpc_board::*;

pub const NOW: &str = "2026-11-03T12:00:00Z";
pub const ME: &str = "p-1";
pub const OTHER: &str = "p-2";

pub fn id(s: &str) -> Id {
    Id::new(s).expect("an id")
}

pub fn text(s: &str) -> Value {
    Value::Str(s.to_string())
}

pub fn parse(s: &str) -> Value {
    json::parse(s.as_bytes(), &Limits::V1).expect("JSON")
}

pub fn fields(pairs: &[(&str, Value)]) -> Fields {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

pub fn board_of(objects: &str, settings: &str, frames: &str) -> Board {
    let text = format!(
        "{{\"frames\":{frames},\"objects\":[{objects}],\"people\":{{\"{ME}\":{{\"kind\":\"agent\",\"name\":\"Ann\"}},\"{OTHER}\":{{\"name\":\"Bo\"}}}},\"settings\":{settings}}}"
    );
    Board::from_json(text.as_bytes(), &Limits::V1).expect("the board")
}

pub fn obj<'b>(b: &'b Board, i: &str) -> &'b Object {
    b.objects
        .iter()
        .find(|o| o.id.as_str() == i)
        .unwrap_or_else(|| panic!("no object {i}"))
}

pub fn has(b: &Board, i: &str) -> bool {
    b.objects.iter().any(|o| o.id.as_str() == i)
}

pub fn rev(b: &Board, i: &str) -> Sha256 {
    obj(b, i).rev().expect("a rev")
}

pub fn order(b: &Board) -> Vec<&str> {
    b.objects.iter().map(|o| o.id.as_str()).collect()
}

pub fn named(b: &Board, i: &str) -> verbs::ObjRev {
    verbs::ObjRev {
        id: id(i),
        rev: rev(b, i),
    }
}

pub fn num(o: &Object, name: &str) -> f64 {
    match o.fields.get(name) {
        Some(Value::Num(n)) => *n,
        other => panic!("{} has no number {name}: {other:?}", o.id),
    }
}

pub fn get<'o>(o: &'o Object, name: &str) -> &'o Value {
    o.fields
        .get(name)
        .unwrap_or_else(|| panic!("{} has no {name}", o.id))
}

/// Who a test acts as.
pub enum Who {
    Nobody,
    Me,
    New,
}

/// A host: a counter for its random bytes, one actor, one clock.
pub fn plan_as(b: &Board, who: Who, seed: u64, verb: &Verb) -> Result<Planned, VerbError> {
    let mut n = seed;
    let mut random = move || {
        n += 1;
        n.to_le_bytes()
    };
    let by = match who {
        Who::Nobody => None,
        Who::Me => Some(Me {
            token: id(ME),
            new: None,
        }),
        Who::New => Some(verbs::me(b, Some("Cy"), Some("agent"), [9; 8])),
    };
    let mut cx = Ctx {
        group_id: "g".to_string(),
        by,
        at: Some(NOW.to_string()),
        random: &mut random,
    };
    verbs::plan(b, &mut cx, verb)
}

/// Plans and applies as [`run`] does, and gives back what stopped it as text, for a test to assert.
pub fn applies(b: &Board, verb: &Verb) -> Result<(), String> {
    let planned = plan_as(b, Who::Me, 1000, verb).map_err(|e| format!("it doesn't plan: {e:?}"))?;
    let mut next = b.clone();
    apply_group(&mut next, &planned.group)
        .map(|_| ())
        .map_err(|e| format!("it doesn't apply: {e:?}"))
}

/// Plans and applies as the test's usual actor, and gives back the board and the plan.
pub fn run(b: &Board, verb: &Verb) -> (Board, Planned, Report) {
    let planned = plan_as(b, Who::Me, 1000, verb).expect("it plans");
    let mut next = b.clone();
    let report = apply_group(&mut next, &planned.group).expect("it applies");
    (next, planned, report)
}

pub fn verb_code(e: VerbError) -> &'static str {
    match e {
        VerbError::Refused { code, .. } => code.as_str(),
        other => panic!("wanted a verb's refusal, got {other:?}"),
    }
}

pub fn refusal_of(e: Conflict) -> RefusalCode {
    match e {
        Conflict::Refused(r) => r.code,
        other => panic!("wanted a refusal, got {other:?}"),
    }
}

/// Everything `inverse` restores: objects with their order, frames and settings.
pub fn content(b: &Board) -> Value {
    let Value::Obj(mut v) = b.to_value() else {
        panic!("a board is an object");
    };
    v.remove("people");
    Value::Obj(v)
}

/// Applying a planned group and then its inverse gets back the same content, exactly.
pub fn assert_inverse(before: &Board, group: &Group) {
    let mut after = before.clone();
    apply_group(&mut after, group).expect("it applies");
    let back = verbs::inverse(before, &after, "undo").expect("an inverse");
    let mut undone = after.clone();
    apply_group(&mut undone, &back).unwrap_or_else(|e| panic!("check: the inverse applies: {e:?}"));
    let got = String::from_utf8(json::to_canonical(&content(&undone)).expect("canonical"))
        .expect("utf-8");
    let want =
        String::from_utf8(json::to_canonical(&content(before)).expect("canonical")).expect("utf-8");
    assert_eq!(got, want, "the inverse of {group:?}");
}
