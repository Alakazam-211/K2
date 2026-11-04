//! One group per code `GROUP_REFUSALS` lists, built as a host's Rust ops, on the board it's refused
//! on. Shared, by path, by the board crate's `tests/group_refusals.rs` (each is refused with its
//! code) and the bridge's tests (each refusal's row), so both hold exactly what `apply_group` gives.

#![allow(dead_code)]

use k2_mpc_board::json::{Fields, Value};
use k2_mpc_board::*;

/// A group, the board it's applied to, and the code it's refused with.
pub struct GroupCase {
    /// The code `apply_group` refuses it with.
    pub code: RefusalCode,
    /// The board it's applied to.
    pub board: Board,
    /// The group.
    pub group: Group,
}

fn id(s: &str) -> Id {
    Id::new(s).expect("an id")
}

fn text(s: &str) -> Value {
    Value::Str(s.to_string())
}

fn fields(pairs: &[(&str, Value)]) -> Fields {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

fn base() -> Board {
    let text = "{\"frames\":[],\"objects\":[\
{\"id\":\"s-1\",\"text\":\"one\",\"type\":\"sticky\"},\
{\"id\":\"l-1\",\"locked\":true,\"text\":\"fixed\",\"type\":\"sticky\"}],\
\"people\":{\"p-1\":{\"kind\":\"agent\",\"name\":\"Ann\"}},\"settings\":{}}";
    Board::from_json(text.as_bytes(), &Limits::V1).expect("the board")
}

fn rev(b: &Board, i: &str) -> Sha256 {
    b.objects
        .iter()
        .find(|o| o.id.as_str() == i)
        .expect("the object")
        .rev()
        .expect("a rev")
}

fn object(i: &str, kind: ObjectType, f: Fields) -> Object {
    Object {
        id: id(i),
        kind,
        fields: f,
    }
}

fn add(o: Object) -> Op {
    Op::Add {
        object: o,
        at: Position::Front,
    }
}

fn sticky(f: &[(&str, Value)]) -> Op {
    add(object("s-9", ObjectType::Sticky, fields(f)))
}

fn nested(depth: usize) -> Value {
    let mut v = Value::Null;
    for _ in 0..depth {
        v = Value::Arr(vec![v]);
    }
    v
}

fn case(code: RefusalCode, board: &Board, ops: Vec<Op>) -> GroupCase {
    GroupCase {
        code,
        board: board.clone(),
        group: Group {
            id: "g".into(),
            by: Some(id("p-1")),
            ops,
        },
    }
}

/// Every case, in `GROUP_REFUSALS`'s order.
pub fn group_cases() -> Vec<GroupCase> {
    let b = base();
    let mut many = b.clone();
    for i in 0..Limits::V1.objects - many.objects.len() as u64 {
        many.objects
            .push(object(&format!("o-{i}"), ObjectType::Sticky, Fields::new()));
    }
    let mut crowd = b.clone();
    for i in 0..Limits::V1.people - crowd.people.len() as u64 {
        crowd
            .people
            .insert(id(&format!("q-{i}")), Person::default());
    }
    let check = |t: &str| {
        Value::Obj(fields(&[
            ("done", Value::Bool(false)),
            ("id", text("c1")),
            ("text", text(t)),
        ]))
    };
    vec![
        case(
            RefusalCode::ObjectLocked,
            &b,
            vec![Op::Set {
                id: id("l-1"),
                rev: rev(&b, "l-1"),
                set: fields(&[("text", text("x"))]),
                unset: vec![],
            }],
        ),
        case(
            RefusalCode::FramesMismatch,
            &b,
            vec![Op::FrameOrder {
                id: id("s-1"),
                to: Position::Front,
            }],
        ),
        case(
            RefusalCode::FieldValue,
            &b,
            vec![Op::Set {
                id: id("s-1"),
                rev: rev(&b, "s-1"),
                set: fields(&[("id", text("x"))]),
                unset: vec![],
            }],
        ),
        case(RefusalCode::LimitObjects, &many, vec![sticky(&[])]),
        case(
            RefusalCode::LimitPeople,
            &crowd,
            vec![Op::AddPerson {
                token: id("p-9"),
                person: Person::default(),
            }],
        ),
        case(
            RefusalCode::JsonNumberRange,
            &b,
            vec![sticky(&[("later", Value::Num(1e300))])],
        ),
        case(
            RefusalCode::LimitDepth,
            &b,
            vec![sticky(&[("later", nested(80))])],
        ),
        case(
            RefusalCode::FieldKind,
            &b,
            vec![sticky(&[("x", text("wide"))])],
        ),
        case(
            RefusalCode::LimitField,
            &b,
            vec![sticky(&[("text", text(&"a".repeat(100_000)))])],
        ),
        case(
            RefusalCode::FieldNameInvalid,
            &b,
            vec![sticky(&[("a\u{1}", text("x"))])],
        ),
        case(
            RefusalCode::PersonUnknown,
            &b,
            vec![sticky(&[(
                "created",
                Value::Obj(fields(&[
                    ("by", text("p-9")),
                    ("at", text("2026-11-03T12:00:00Z")),
                ])),
            )])],
        ),
        case(
            RefusalCode::IdDuplicate,
            &b,
            vec![add(object(
                "k-9",
                ObjectType::Card,
                fields(&[("checks", Value::Arr(vec![check("a"), check("b")]))]),
            ))],
        ),
        case(
            RefusalCode::CellColumnUnknown,
            &b,
            vec![add(object(
                "b-9",
                ObjectType::Table,
                fields(&[
                    (
                        "columns",
                        Value::Arr(vec![Value::Obj(fields(&[("id", text("a"))]))]),
                    ),
                    (
                        "rows",
                        Value::Arr(vec![Value::Obj(fields(&[
                            ("id", text("r1")),
                            ("cells", Value::Obj(fields(&[("z", text("x"))]))),
                        ]))]),
                    ),
                ]),
            ))],
        ),
        case(
            RefusalCode::TypeInvalid,
            &b,
            vec![add(object(
                "o-9",
                ObjectType::Unknown("Not A Name".into()),
                Fields::new(),
            ))],
        ),
        case(
            RefusalCode::IdInvalid,
            &b,
            vec![sticky(&[("frame", text("Not An Id"))])],
        ),
    ]
}
