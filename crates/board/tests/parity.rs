//! Agent parity: every action on the list, run the person's way (the raw ops a canvas sends) and the
//! agent's way (a verb), with the same result. The `match` is exhaustive, so a new action can't land
//! without its test.

#[path = "support/verbs.rs"]
mod support;

use k2_mpc_board::json::{Fields, Value};
use k2_mpc_board::parity::{parity_markdown, PersonAction as A, Way, ROWS};
use k2_mpc_board::schema::{self, Kind};
use k2_mpc_board::verbs::{self, AlignTo, Axis, End, Layer, Planned, Side, Verb};
use k2_mpc_board::*;
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use support::*;

fn base() -> Board {
    let hash = Sha256::of(b"pic");
    board_of(
        &format!("{{\"h\":100,\"id\":\"s-1\",\"reactions\":{{\"y\":[\"p-2\"]}},\"text\":\"one\",\"type\":\"sticky\",\"votes\":{{\"p-2\":2}},\"w\":100,\"x\":0,\"y\":0}},\
{{\"group\":\"g-1\",\"h\":100,\"id\":\"s-2\",\"text\":\"two\",\"type\":\"sticky\",\"w\":100,\"x\":500,\"y\":0}},\
{{\"group\":\"g-1\",\"h\":100,\"id\":\"s-3\",\"text\":\"three\",\"type\":\"sticky\",\"w\":100,\"x\":700,\"y\":40}},\
{{\"color\":\"blue\",\"h\":100,\"id\":\"h-1\",\"stroke\":\"red\",\"text\":\"shape\",\"type\":\"shape\",\"w\":100,\"x\":50,\"y\":50}},\
{{\"from\":{{\"id\":\"s-1\"}},\"id\":\"c-1\",\"to\":{{\"id\":\"s-2\"}},\"type\":\"connector\"}},\
{{\"id\":\"q-1\",\"target\":\"s-2\",\"text\":\"pin\",\"type\":\"comment\"}},\
{{\"id\":\"p-9\",\"points\":[[0,0],[1,1]],\"type\":\"pen\",\"x\":9,\"y\":9}},\
{{\"h\":100,\"id\":\"l-1\",\"locked\":true,\"type\":\"sticky\",\"w\":100,\"x\":300,\"y\":0}},\
{{\"checks\":[{{\"done\":false,\"id\":\"c1\",\"text\":\"one\"}}],\"h\":100,\"id\":\"k-1\",\"title\":\"Find this\",\"type\":\"card\",\"w\":100,\"x\":900,\"y\":0}},\
{{\"columns\":[{{\"id\":\"a\"}}],\"h\":100,\"id\":\"b-1\",\"rows\":[{{\"id\":\"r\"}}],\"type\":\"table\",\"w\":100,\"x\":1100,\"y\":0}},\
{{\"color\":\"violet\",\"h\":60,\"id\":\"m-1\",\"text\":\"root\",\"type\":\"mind\",\"w\":200,\"x\":0,\"y\":500}},\
{{\"color\":\"blue\",\"h\":48,\"id\":\"m-2\",\"parent\":\"m-1\",\"type\":\"mind\",\"w\":160,\"x\":280,\"y\":500}},\
{{\"id\":\"f-1\",\"type\":\"frame\"}},\
{{\"id\":\"f-2\",\"type\":\"frame\"}},\
{{\"created\":{{\"at\":\"2026-11-01T00:00:00Z\",\"by\":\"p-2\"}},\"id\":\"i-1\",\"src\":{{\"blob\":\"{hash}\"}},\"type\":\"image\"}}"),
        "{\"title\":\"T\",\"voting\":{\"active\":true}}",
        "[\"f-1\",\"f-2\"]",
    )
}

fn canon(b: &Board) -> String {
    String::from_utf8(b.to_canonical_json().expect("canonical")).expect("utf-8")
}

fn stamped(mut f: Fields) -> Fields {
    f.insert(
        "created".into(),
        parse(&format!("{{\"at\":\"{NOW}\",\"by\":\"{ME}\"}}")),
    );
    f
}

fn add_op(i: &Id, kind: ObjectType, f: Fields) -> Op {
    Op::Add {
        object: Object {
            id: i.clone(),
            kind,
            fields: stamped(f),
        },
        at: Position::Front,
    }
}

fn set_op(b: &Board, i: &str, set: &[(&str, Value)]) -> Op {
    Op::Set {
        id: id(i),
        rev: rev(b, i),
        set: fields(set),
        unset: Vec::new(),
    }
}

fn del_op(b: &Board, i: &str) -> Op {
    Op::Del {
        id: id(i),
        rev: rev(b, i),
    }
}

/// Runs one verb the agent's way and the person's way. The boards must match, the verb must do
/// something, and its inverse must take it back.
fn both(b: &Board, verb: &Verb, person: impl FnOnce(&Planned) -> Vec<Op>) -> Vec<&'static str> {
    let planned = plan_as(b, Who::Me, 1000, verb).expect("check: the verb plans");
    let mut agent = b.clone();
    apply_group(&mut agent, &planned.group).expect("check: the agent's way applies");
    let mut persons = b.clone();
    let group = Group {
        id: "person".to_string(),
        by: Some(id(ME)),
        ops: person(&planned),
    };
    apply_group(&mut persons, &group).expect("check: the person's way applies");
    assert_eq!(
        canon(&agent),
        canon(&persons),
        "{} differs from the person's way",
        verb.name()
    );
    assert_ne!(canon(&agent), canon(b), "{} did nothing", verb.name());
    agent.validate(&Limits::V1).expect("the result is valid");
    assert_inverse(b, &planned.group);
    vec![verb.name()]
}

fn add_case(kind: ObjectType, f: Fields) -> Vec<&'static str> {
    let verb = Verb::Add {
        kind: kind.clone(),
        fields: f.clone(),
        at: Position::Front,
    };
    both(&base(), &verb, |p| vec![add_op(&p.ids[0], kind, f)])
}

/// The id of the item a verb added to a list.
fn new_item(b: &Board, p: &Planned, on: &str, field: &str) -> String {
    let mut next = b.clone();
    apply_group(&mut next, &p.group).expect("it applies");
    let ids = |board: &Board| -> Vec<String> {
        match obj(board, on).fields.get(field) {
            Some(Value::Arr(a)) => a
                .iter()
                .map(|v| match v {
                    Value::Obj(f) => match &f["id"] {
                        Value::Str(s) => s.clone(),
                        other => panic!("an id: {other:?}"),
                    },
                    other => panic!("an object: {other:?}"),
                })
                .collect(),
            _ => Vec::new(),
        }
    };
    let old = ids(b);
    ids(&next)
        .into_iter()
        .find(|i| !old.contains(i))
        .expect("a new item")
}

fn order_case(ids: &[&str], to: Layer, z: Vec<Op>) -> Vec<&'static str> {
    let b = base();
    let verb = Verb::Order {
        ids: ids.iter().map(|i| id(i)).collect(),
        to,
    };
    both(&b, &verb, |_| z)
}

/// One action, both ways, or a statement that it has no verb. Returns the verbs it ran.
fn exercise(action: A) -> Vec<&'static str> {
    let b = base();
    let revs = |ids: &[&str]| -> Vec<verbs::ObjRev> { ids.iter().map(|i| named(&b, i)).collect() };
    match action {
        A::AddSticky => add_case(ObjectType::Sticky, fields(&[("text", text("x"))])),
        A::AddText => add_case(ObjectType::Text, fields(&[("text", text("x"))])),
        A::AddShape => add_case(ObjectType::Shape, fields(&[("shape", text("star"))])),
        A::DrawPen => add_case(
            ObjectType::Pen,
            fields(&[("points", parse("[[0,0],[2,3]]"))]),
        ),
        A::AddFrame => add_case(ObjectType::Frame, fields(&[("title", text("x"))])),
        A::AddCard => add_case(ObjectType::Card, fields(&[("title", text("x"))])),
        A::AddTable => {
            let cols = parse("[{\"id\":\"a\"}]");
            let rows = parse("[{\"id\":\"r\"}]");
            add_case(
                ObjectType::Table,
                fields(&[("columns", cols), ("rows", rows)]),
            )
        }
        A::AddMindNode => add_case(ObjectType::Mind, fields(&[("text", text("x"))])),
        A::AddImage => {
            let src = parse(&format!("{{\"blob\":\"{}\"}}", Sha256::of(b"new")));
            add_case(ObjectType::Image, fields(&[("src", src)]))
        }
        A::PasteText => {
            let mut used = add_case(ObjectType::Text, fields(&[("text", text("x"))]));
            used.extend(labelled_case(&b));
            used
        }
        A::EditFields => {
            let set = [("text", text("new")), ("color", text("pink"))];
            let verb = Verb::Update {
                id: id("s-1"),
                rev: rev(&b, "s-1"),
                set: fields(&set),
                unset: Vec::new(),
            };
            both(&b, &verb, |_| vec![set_op(&b, "s-1", &set)])
        }
        A::Resize => {
            let set = [("w", Value::Num(10.0)), ("h", Value::Num(20.0))];
            let verb = Verb::Update {
                id: id("s-1"),
                rev: rev(&b, "s-1"),
                set: fields(&set),
                unset: Vec::new(),
            };
            both(&b, &verb, |_| vec![set_op(&b, "s-1", &set)])
        }
        A::Rotate => {
            let set = [("rot", Value::Num(15.0))];
            let verb = Verb::Update {
                id: id("s-1"),
                rev: rev(&b, "s-1"),
                set: fields(&set),
                unset: Vec::new(),
            };
            both(&b, &verb, |_| vec![set_op(&b, "s-1", &set)])
        }
        A::MoveObjects => {
            let verb = Verb::Move {
                ids: revs(&["s-2"]),
                dx: 5.0,
                dy: 6.0,
            };
            both(&b, &verb, |_| {
                vec![
                    set_op(
                        &b,
                        "s-2",
                        &[("x", Value::Num(505.0)), ("y", Value::Num(6.0))],
                    ),
                    set_op(
                        &b,
                        "s-3",
                        &[("x", Value::Num(705.0)), ("y", Value::Num(46.0))],
                    ),
                ]
            })
        }
        A::DrawConnector => {
            let verb = Verb::Connect {
                from: End::Object {
                    id: id("s-1"),
                    anchor: None,
                },
                to: End::Object {
                    id: id("h-1"),
                    anchor: Some("top".into()),
                },
                fields: Fields::new(),
            };
            both(&b, &verb, |p| {
                vec![add_op(
                    &p.ids[0],
                    ObjectType::Connector,
                    fields(&[
                        ("from", parse("{\"anchor\":\"auto\",\"id\":\"s-1\"}")),
                        ("to", parse("{\"anchor\":\"top\",\"id\":\"h-1\"}")),
                    ]),
                )]
            })
        }
        A::ReattachConnector => {
            let set = [("to", parse("{\"id\":\"h-1\"}"))];
            let verb = Verb::Update {
                id: id("c-1"),
                rev: rev(&b, "c-1"),
                set: fields(&set),
                unset: Vec::new(),
            };
            both(&b, &verb, |_| vec![set_op(&b, "c-1", &set)])
        }
        A::Delete => {
            let verb = Verb::Delete {
                id: id("s-2"),
                rev: rev(&b, "s-2"),
                cascade: true,
            };
            both(&b, &verb, |_| {
                vec![del_op(&b, "s-2"), del_op(&b, "c-1"), del_op(&b, "q-1")]
            })
        }
        A::EraseStroke => {
            let verb = Verb::Delete {
                id: id("p-9"),
                rev: rev(&b, "p-9"),
                cascade: false,
            };
            both(&b, &verb, |_| vec![del_op(&b, "p-9")])
        }
        A::Cut => {
            let read = verbs::list(&b, Some(&ObjectType::Pen)).expect("a list");
            assert_eq!(
                read[0].object,
                *obj(&b, "p-9"),
                "what's cut is what was read"
            );
            let verb = Verb::Delete {
                id: read[0].object.id.clone(),
                rev: read[0].rev,
                cascade: true,
            };
            let mut used = both(&b, &verb, |_| vec![del_op(&b, "p-9")]);
            used.push("list");
            used
        }
        A::Copy => {
            let read = verbs::list(&b, Some(&ObjectType::Pen)).expect("a list");
            let verb = Verb::Paste {
                objects: vec![read[0].object.clone()],
                x: 100.0,
                y: 100.0,
            };
            // The stored box starts at (9, 9), 1 by 1, so it lands at (100, 100).
            let mut used = both(&b, &verb, |p| {
                vec![add_op(
                    &p.ids[0],
                    ObjectType::Pen,
                    fields(&[
                        ("points", parse("[[0,0],[1,1]]")),
                        ("x", Value::Num(100.0)),
                        ("y", Value::Num(100.0)),
                    ]),
                )]
            });
            used.push("list");
            used
        }
        A::Duplicate => {
            let verb = Verb::Duplicate {
                ids: vec![id("h-1")],
                dx: 24.0,
                dy: 24.0,
            };
            both(&b, &verb, |p| {
                vec![add_op(
                    &p.ids[0],
                    ObjectType::Shape,
                    fields(&[
                        ("color", text("blue")),
                        ("h", Value::Num(100.0)),
                        ("stroke", text("red")),
                        ("text", text("shape")),
                        ("w", Value::Num(100.0)),
                        ("x", Value::Num(74.0)),
                        ("y", Value::Num(74.0)),
                    ]),
                )]
            })
        }
        A::ConnectedCopy => {
            let verb = Verb::ConnectedCopy {
                id: id("k-1"),
                side: Side::Bottom,
            };
            both(&b, &verb, |p| {
                vec![
                    add_op(
                        &p.ids[0],
                        ObjectType::Card,
                        fields(&[
                            ("h", Value::Num(100.0)),
                            ("title", text("")),
                            ("w", Value::Num(100.0)),
                            ("x", Value::Num(900.0)),
                            ("y", Value::Num(180.0)),
                        ]),
                    ),
                    add_op(
                        &p.ids[1],
                        ObjectType::Connector,
                        fields(&[
                            ("from", parse("{\"anchor\":\"bottom\",\"id\":\"k-1\"}")),
                            (
                                "to",
                                parse(&format!("{{\"anchor\":\"top\",\"id\":\"{}\"}}", p.ids[0])),
                            ),
                        ]),
                    ),
                ]
            })
        }
        A::CopyStyle => {
            let verb = Verb::CopyStyle {
                from: id("h-1"),
                to: revs(&["c-1"]),
            };
            both(&b, &verb, |_| {
                vec![set_op(&b, "c-1", &[("stroke", text("red"))])]
            })
        }
        A::Tidy => {
            let verb = Verb::Tidy {
                ids: revs(&["s-2", "s-3"]),
            };
            both(&b, &verb, |_| {
                vec![set_op(
                    &b,
                    "s-3",
                    &[("x", Value::Num(624.0)), ("y", Value::Num(0.0))],
                )]
            })
        }
        A::Align => {
            let verb = Verb::Align {
                ids: revs(&["s-2", "s-3"]),
                to: AlignTo::Top,
            };
            both(&b, &verb, |_| {
                vec![set_op(&b, "s-3", &[("y", Value::Num(0.0))])]
            })
        }
        A::Distribute => {
            let verb = Verb::Distribute {
                ids: revs(&["s-1", "l-1", "s-3"]),
                axis: Axis::Horizontal,
            };
            let unlocked = {
                let mut c = b.clone();
                let g = Group {
                    id: "g".into(),
                    by: None,
                    ops: vec![set_op(&b, "l-1", &[("locked", Value::Bool(false))])],
                };
                apply_group(&mut c, &g).expect("it unlocks");
                c
            };
            let verb = match verb {
                Verb::Distribute { axis, .. } => Verb::Distribute {
                    ids: ["s-1", "l-1", "s-3"]
                        .iter()
                        .map(|i| named(&unlocked, i))
                        .collect(),
                    axis,
                },
                other => other,
            };
            both(&unlocked, &verb, |_| {
                vec![set_op(&unlocked, "l-1", &[("x", Value::Num(350.0))])]
            })
        }
        A::Group => {
            let verb = Verb::Group {
                ids: revs(&["s-1", "h-1"]),
            };
            let planned = plan_as(&b, Who::Me, 1000, &verb).expect("plans");
            let Op::Set { set, .. } = &planned.group.ops[0] else {
                panic!("wanted a set");
            };
            let g = set["group"].clone();
            both(&b, &verb, |_| {
                vec![
                    set_op(&b, "s-1", &[("group", g.clone())]),
                    set_op(&b, "h-1", &[("group", g.clone())]),
                ]
            })
        }
        A::Ungroup => {
            let verb = Verb::Ungroup {
                ids: revs(&["s-2"]),
            };
            both(&b, &verb, |_| {
                vec![Op::Set {
                    id: id("s-2"),
                    rev: rev(&b, "s-2"),
                    set: Fields::new(),
                    unset: vec!["group".to_string()],
                }]
            })
        }
        A::Lock => {
            let verb = Verb::Lock {
                ids: revs(&["s-1"]),
                locked: true,
            };
            both(&b, &verb, |_| {
                vec![set_op(&b, "s-1", &[("locked", Value::Bool(true))])]
            })
        }
        A::Unlock => {
            let verb = Verb::Lock {
                ids: revs(&["l-1"]),
                locked: false,
            };
            both(&b, &verb, |_| {
                vec![set_op(&b, "l-1", &[("locked", Value::Bool(false))])]
            })
        }
        A::BringToFront => order_case(
            &["s-1"],
            Layer::Front,
            vec![Op::Z {
                id: id("s-1"),
                to: Position::Front,
            }],
        ),
        A::SendToBack => order_case(
            &["h-1"],
            Layer::Back,
            vec![Op::Z {
                id: id("h-1"),
                to: Position::Back,
            }],
        ),
        A::BringForward => order_case(
            &["s-1"],
            Layer::Forward,
            vec![Op::Z {
                id: id("s-1"),
                to: Position::After(vec![id("h-1")]),
            }],
        ),
        A::SendBackward => order_case(
            &["h-1"],
            Layer::Backward,
            vec![Op::Z {
                id: id("h-1"),
                to: Position::Back,
            }],
        ),
        A::ReorderFrames => {
            let to = Position::After(vec![id("f-2")]);
            let verb = Verb::FrameOrder {
                id: id("f-1"),
                to: to.clone(),
            };
            both(&b, &verb, |_| vec![Op::FrameOrder { id: id("f-1"), to }])
        }
        A::Comment => {
            let verb = Verb::Comment {
                target: Some(id("s-1")),
                text: "look".into(),
                point: None,
            };
            both(&b, &verb, |p| {
                vec![add_op(
                    &p.ids[0],
                    ObjectType::Comment,
                    fields(&[
                        ("target", text("s-1")),
                        ("text", text("look")),
                        ("x", Value::Num(88.0)),
                        ("y", Value::Num(-20.0)),
                    ]),
                )]
            })
        }
        A::Reply => {
            let verb = Verb::Reply {
                id: id("q-1"),
                text: "yes".into(),
            };
            both(&b, &verb, |p| {
                let item = new_item(&b, p, "q-1", "replies");
                let list = parse(&format!(
                    "[{{\"at\":\"{NOW}\",\"by\":\"{ME}\",\"id\":\"{item}\",\"text\":\"yes\"}}]"
                ));
                vec![set_op(&b, "q-1", &[("replies", list)])]
            })
        }
        A::Resolve => {
            let verb = Verb::Resolve {
                id: id("q-1"),
                rev: rev(&b, "q-1"),
                resolved: true,
            };
            both(&b, &verb, |_| {
                vec![set_op(&b, "q-1", &[("resolved", Value::Bool(true))])]
            })
        }
        A::DeleteComment => {
            let verb = Verb::Delete {
                id: id("q-1"),
                rev: rev(&b, "q-1"),
                cascade: true,
            };
            both(&b, &verb, |_| vec![del_op(&b, "q-1")])
        }
        A::React | A::Stamp => {
            let verb = Verb::React {
                id: id("s-1"),
                emoji: "\u{2705}".into(),
                on: Some(true),
            };
            let mine = parse(&format!("{{\"\u{2705}\":[\"{ME}\"]}}"));
            both(&b, &verb, |_| {
                vec![set_op(&b, "s-1", &[("reactions", mine)])]
            })
        }
        A::Vote => {
            let verb = Verb::Vote {
                id: id("s-1"),
                delta: 1,
            };
            let mine = parse(&format!("{{\"{ME}\":1}}"));
            both(&b, &verb, |_| vec![set_op(&b, "s-1", &[("votes", mine)])])
        }
        A::TakeBackVote => {
            let mut voted = b.clone();
            let mine = parse(&format!("{{\"{ME}\":1}}"));
            let g = Group {
                id: "g".into(),
                by: Some(id(ME)),
                ops: vec![set_op(&b, "s-1", &[("votes", mine)])],
            };
            apply_group(&mut voted, &g).expect("a vote");
            let verb = Verb::Vote {
                id: id("s-1"),
                delta: -1,
            };
            both(&voted, &verb, |_| {
                vec![set_op(
                    &voted,
                    "s-1",
                    &[("votes", Value::Obj(Fields::new()))],
                )]
            })
        }
        A::StartVoting => {
            let set = [("voting", parse("{\"active\":true,\"perPerson\":5}"))];
            meta_case(&b, &set)
        }
        A::RenameBoard => meta_case(&b, &[("title", text("New"))]),
        A::AddCheck => {
            let verb = Verb::CheckAdd {
                id: id("k-1"),
                text: "two".into(),
            };
            both(&b, &verb, |p| {
                let item = new_item(&b, p, "k-1", "checks");
                let list = parse(&format!(
                    "[{{\"id\":\"c1\"}},{{\"done\":false,\"id\":\"{item}\",\"text\":\"two\"}}]"
                ));
                vec![set_op(&b, "k-1", &[("checks", list)])]
            })
        }
        A::TickCheck => {
            let verb = Verb::CheckSet {
                id: id("k-1"),
                rev: rev(&b, "k-1"),
                item: "c1".into(),
                text: None,
                done: Some(true),
            };
            let list = parse("[{\"done\":true,\"id\":\"c1\"}]");
            both(&b, &verb, |_| vec![set_op(&b, "k-1", &[("checks", list)])])
        }
        A::RemoveCheck => {
            let verb = Verb::CheckRemove {
                id: id("k-1"),
                rev: rev(&b, "k-1"),
                item: "c1".into(),
            };
            both(&b, &verb, |_| {
                vec![set_op(&b, "k-1", &[("checks", parse("[]"))])]
            })
        }
        A::EditCell => {
            let verb = Verb::TableCell {
                id: id("b-1"),
                rev: rev(&b, "b-1"),
                row: "r".into(),
                column: "a".into(),
                text: "x".into(),
            };
            let rows = parse("[{\"cells\":{\"a\":\"x\"},\"id\":\"r\"}]");
            both(&b, &verb, |_| vec![set_op(&b, "b-1", &[("rows", rows)])])
        }
        A::AddRow => {
            let verb = Verb::TableAddRow {
                id: id("b-1"),
                after: Some("r".into()),
            };
            both(&b, &verb, |p| {
                let item = new_item(&b, p, "b-1", "rows");
                let rows = parse(&format!("[{{\"id\":\"r\"}},{{\"id\":\"{item}\"}}]"));
                vec![set_op(&b, "b-1", &[("rows", rows)])]
            })
        }
        A::RemoveRow => {
            let verb = Verb::TableRemoveRow {
                id: id("b-1"),
                rev: rev(&b, "b-1"),
                row: "r".into(),
            };
            both(&b, &verb, |_| {
                vec![set_op(&b, "b-1", &[("rows", parse("[]"))])]
            })
        }
        A::AddColumn => {
            let verb = Verb::TableAddColumn {
                id: id("b-1"),
                after: Some("a".into()),
                width: None,
            };
            both(&b, &verb, |p| {
                let item = new_item(&b, p, "b-1", "columns");
                let list = parse(&format!("[{{\"id\":\"a\"}},{{\"id\":\"{item}\"}}]"));
                vec![set_op(&b, "b-1", &[("columns", list)])]
            })
        }
        A::RemoveColumn => {
            let verb = Verb::TableRemoveColumn {
                id: id("b-1"),
                rev: rev(&b, "b-1"),
                column: "a".into(),
            };
            both(&b, &verb, |_| {
                vec![set_op(
                    &b,
                    "b-1",
                    &[
                        ("columns", parse("[]")),
                        ("rows", parse("[{\"id\":\"r\"}]")),
                    ],
                )]
            })
        }
        // No child sits left of the root, so the left is the lighter side.
        A::MindChild => mind_case(&b, "m-1", -240.0, 568.0, "green"),
        // A sibling of m-2 is a child of m-1.
        A::MindSibling => mind_case(&b, "m-1", -240.0, 568.0, "green"),
        A::UseTemplate => {
            let listed = verbs::templates();
            assert!(listed.iter().any(|(n, _)| *n == "mindmap"), "{listed:?}");
            let verb = Verb::Template {
                name: "mindmap".into(),
                x: 0.0,
                y: 0.0,
            };
            let mut used = vec!["templates"];
            used.extend(both(&b, &verb, |p| p.group.ops.clone()));
            used
        }
        A::Search => {
            let found = verbs::search(&b, "find THIS").expect("a search");
            assert_eq!(found.len(), 1);
            assert_eq!(found[0].object.id, id("k-1"));
            vec!["search"]
        }
        A::ReadBoard => {
            let all = verbs::list(&b, None).expect("a list");
            assert_eq!(all.len(), b.objects.len());
            let one = verbs::read(&b, &id("s-2")).expect("reads").expect("there");
            assert_eq!(one.object, *obj(&b, "s-2"));
            assert_eq!(one.rev, rev(&b, "s-2"));
            assert_eq!(one.references.len(), 2, "{:?}", one.references);
            verbs::fields(Some("sticky")).expect("fields");
            vec!["list", "read", "references", "fields"]
        }
        A::SeeAuthor => {
            let one = verbs::read(&b, &id("i-1")).expect("reads").expect("there");
            assert_eq!(
                get(&one.object, "created"),
                &parse("{\"at\":\"2026-11-01T00:00:00Z\",\"by\":\"p-2\"}")
            );
            vec!["read"]
        }
        A::Undo => {
            let (after, group) = edited(&b);
            let undo = verbs::inverse(&b, &after, "undo").expect("an inverse");
            let mut agent = after.clone();
            apply_group(&mut agent, &undo).expect("it applies");
            let mut person = after.clone();
            let back = Group {
                id: "person".into(),
                by: None,
                ops: vec![set_op(&after, "s-1", &[("text", text("one"))])],
            };
            apply_group(&mut person, &back).expect("it applies");
            assert_eq!(canon(&agent), canon(&person));
            assert_eq!(canon(&agent), canon(&b));
            drop(group);
            vec!["inverse"]
        }
        A::Redo => {
            let (after, _) = edited(&b);
            let undo = verbs::inverse(&b, &after, "undo").expect("an inverse");
            let mut undone = after.clone();
            apply_group(&mut undone, &undo).expect("it applies");
            let redo = verbs::inverse(&after, &undone, "redo").expect("an inverse");
            apply_group(&mut undone, &redo).expect("it applies");
            assert_eq!(canon(&undone), canon(&after));
            vec!["inverse"]
        }
        A::RestoreDeleted => {
            let del = Verb::Delete {
                id: id("h-1"),
                rev: rev(&b, "h-1"),
                cascade: true,
            };
            let planned = plan_as(&b, Who::Me, 1, &del).expect("plans");
            let mut gone = b.clone();
            apply_group(&mut gone, &planned.group).expect("it applies");
            let verb = Verb::PutBack {
                removed: planned.removed[0].clone(),
            };
            let used = both(&gone, &verb, |_| {
                vec![Op::Add {
                    object: obj(&b, "h-1").clone(),
                    at: Position::After(vec![id("s-3")]),
                }]
            });
            let p = plan_as(&gone, Who::Me, 1, &verb).expect("plans");
            apply_group(&mut gone, &p.group).expect("it applies");
            assert_eq!(canon(&gone), canon(&b), "all as it was");
            used
        }
        A::ClearMissingPicture => {
            let hash = Sha256::of(b"pic");
            let verb = Verb::ClearBlob { hash };
            both(&b, &verb, |_| vec![Op::ClearBlob { hash }])
        }
        A::PasteMany => labelled_case(&b),
        A::View | A::Select | A::SnapGuides | A::Appearance => Vec::new(),
        A::Timer | A::History => Vec::new(),
        A::Export | A::OpenLink | A::Access | A::Save => Vec::new(),
        A::Presence => Vec::new(),
    }
}

fn meta_case(b: &Board, set: &[(&str, Value)]) -> Vec<&'static str> {
    let rev = b.settings_rev().expect("a rev");
    let verb = Verb::Meta {
        rev,
        set: fields(set),
        unset: Vec::new(),
    };
    both(b, &verb, |_| {
        vec![Op::Meta {
            rev,
            set: fields(set),
            unset: Vec::new(),
        }]
    })
}

fn mind_case(b: &Board, parent: &str, x: f64, y: f64, colour: &str) -> Vec<&'static str> {
    let verb = Verb::MindChild {
        parent: id(parent),
        text: "new".into(),
    };
    both(b, &verb, |p| {
        vec![add_op(
            &p.ids[0],
            ObjectType::Mind,
            fields(&[
                ("color", text(colour)),
                ("h", Value::Num(48.0)),
                ("parent", text(parent)),
                ("text", text("new")),
                ("w", Value::Num(160.0)),
                ("x", Value::Num(x)),
                ("y", Value::Num(y)),
            ]),
        )]
    })
}

fn labelled_case(b: &Board) -> Vec<&'static str> {
    let raw = "{\"ops\":[{\"op\":\"add\",\"obj\":{\"id\":\"a\",\"text\":\"one\",\"type\":\"sticky\"}},\
{\"op\":\"add\",\"obj\":{\"from\":{\"id\":\"a\"},\"to\":{\"id\":\"s-1\"},\"type\":\"connector\"}}]}";
    let mut n = 77u64;
    let mut random = move || {
        n += 1;
        n.to_le_bytes()
    };
    let mut cx = verbs::Ctx {
        group_id: "g".into(),
        by: Some(verbs::me(b, Some("Ann"), Some("agent"), [1; 8])),
        at: Some(NOW.into()),
        random: &mut random,
    };
    let planned =
        verbs::labelled_from_json(b, &mut cx, raw.as_bytes(), &Limits::V1).expect("plans");
    let a = planned.labels["a"].clone();
    let person = format!(
        "{{\"id\":\"p\",\"by\":\"{ME}\",\"ops\":[\
{{\"op\":\"add\",\"obj\":{{\"created\":{{\"at\":\"{NOW}\",\"by\":\"{ME}\"}},\"id\":\"{a}\",\"text\":\"one\",\"type\":\"sticky\"}}}},\
{{\"op\":\"add\",\"obj\":{{\"created\":{{\"at\":\"{NOW}\",\"by\":\"{ME}\"}},\"from\":{{\"id\":\"{a}\"}},\"id\":\"{}\",\"to\":{{\"id\":\"s-1\"}},\"type\":\"connector\"}}}}]}}",
        planned.ids[1]
    );
    let group = verbs::group_from_json(person.as_bytes(), &Limits::V1).expect("raw ops");
    let mut agent = b.clone();
    apply_group(&mut agent, &planned.group).expect("the agent's way applies");
    let mut persons = b.clone();
    apply_group(&mut persons, &group).expect("the person's way applies");
    assert_eq!(
        canon(&agent),
        canon(&persons),
        "apply differs from the person's way"
    );
    assert_inverse(b, &planned.group);
    vec!["apply"]
}

fn edited(b: &Board) -> (Board, Group) {
    let verb = Verb::Update {
        id: id("s-1"),
        rev: rev(b, "s-1"),
        set: fields(&[("text", text("changed"))]),
        unset: Vec::new(),
    };
    let planned = plan_as(b, Who::Me, 1, &verb).expect("plans");
    let mut after = b.clone();
    apply_group(&mut after, &planned.group).expect("it applies");
    (after, planned.group)
}

#[test]
fn every_action_has_a_way_and_runs_it() {
    let known: BTreeSet<&str> = verbs::WRITE_VERBS
        .iter()
        .chain(verbs::OTHER_VERBS)
        .copied()
        .collect();
    for row in ROWS {
        let used: BTreeSet<&str> = exercise(row.action).into_iter().collect();
        match row.way {
            Way::Verbs(list) => {
                assert!(!list.is_empty(), "{} names no verb", row.name);
                let listed: BTreeSet<&str> = list.iter().copied().collect();
                assert_eq!(listed, used, "{}: the row and its run differ", row.name);
                for verb in list {
                    assert!(known.contains(verb), "{}: no verb {verb}", row.name);
                }
            }
            Way::ViewOnly(why) | Way::NotInFile(why) | Way::Host(why) | Way::LiveLater(why) => {
                assert!(
                    !why.is_empty(),
                    "{} doesn't say why it has no verb",
                    row.name
                );
                assert!(used.is_empty(), "{} runs a verb it doesn't list", row.name);
            }
        }
    }
}

#[test]
fn every_verb_is_on_the_list() {
    let mut on_list = BTreeSet::new();
    for row in ROWS {
        if let Way::Verbs(list) = row.way {
            on_list.extend(list.iter().copied());
        }
    }
    for verb in verbs::WRITE_VERBS.iter().chain(verbs::OTHER_VERBS) {
        assert!(on_list.contains(verb), "{verb} is on no row");
    }
}

#[test]
fn action_names_are_unique() {
    let names: BTreeSet<&str> = ROWS.iter().map(|r| r.name).collect();
    assert_eq!(names.len(), ROWS.len());
}

fn sample(kind: &Kind) -> Option<Value> {
    Some(match kind {
        Kind::Num => Value::Num(12.5),
        Kind::Bool => Value::Bool(true),
        Kind::Str(_) => text("words"),
        Kind::Open(values) | Kind::Color(values) => text(values[0]),
        Kind::ObjectRef => text("s-0"),
        Kind::GroupId => text("g-0"),
        Kind::Tags => parse("[\"a\"]"),
        Kind::Points => parse("[[1,2],[3,4]]"),
        Kind::End => parse("{\"x\":1,\"y\":2}"),
        Kind::Checks => parse("[{\"done\":false,\"id\":\"i\",\"text\":\"t\"}]"),
        Kind::Columns => parse("[{\"id\":\"a\"}]"),
        Kind::Rows => parse("[{\"id\":\"r\"}]"),
        Kind::Reactions => parse(&format!("{{\"y\":[\"{ME}\"]}}")),
        Kind::Votes => parse(&format!("{{\"{ME}\":2}}")),
        Kind::Replies => parse(&format!(
            "[{{\"at\":\"{NOW}\",\"by\":\"{ME}\",\"id\":\"i\",\"text\":\"t\"}}]"
        )),
        Kind::Crop => parse("{\"h\":0.5,\"w\":0.5,\"x\":0,\"y\":0}"),
        Kind::Link => text("https://a.example"),
        Kind::Blob => parse(&format!("{{\"blob\":\"{}\"}}", Sha256::of(b"x"))),
        Kind::Created => parse(&format!("{{\"at\":\"{NOW}\",\"by\":\"{ME}\"}}")),
        Kind::Voting => return None,
    })
}

/// Everything a person's panel can set is reachable: every field of every type, by `add` and by
/// `update`, and the value comes back as sent.
#[test]
fn every_field_of_every_type_is_reachable() {
    let b = board_of("", "{}", "[]");
    let mut checked = 0;
    for (type_name, own) in schema::TYPES {
        let kind = ObjectType::parse(type_name).expect("a type");
        for field in own.iter().chain(schema::COMMON) {
            let value = sample(&field.kind).expect("a sample");
            let f = fields(&[(field.name, value.clone())]);
            let add = Verb::Add {
                kind: kind.clone(),
                fields: f.clone(),
                at: Position::Front,
            };
            let (next, planned, _) = run(&b, &add);
            let o = obj(&next, planned.ids[0].as_str());
            assert_eq!(
                o.fields.get(field.name),
                Some(&value),
                "add {type_name}.{}",
                field.name
            );

            let empty = Verb::Add {
                kind: kind.clone(),
                fields: if *type_name == "table" {
                    fields(&[("rows", parse("[]"))])
                } else {
                    Fields::new()
                },
                at: Position::Front,
            };
            let (start, planned, _) = run(&b, &empty);
            let target = planned.ids[0].as_str();
            let update = Verb::Update {
                id: id(target),
                rev: rev(&start, target),
                set: f,
                unset: Vec::new(),
            };
            let (next, _, _) = run(&start, &update);
            assert_eq!(
                obj(&next, target).fields.get(field.name),
                Some(&value),
                "update {type_name}.{}",
                field.name
            );
            checked += 1;
        }
    }
    let total: usize = schema::TYPES
        .iter()
        .map(|(_, f)| f.len() + schema::COMMON.len())
        .sum();
    assert_eq!(checked, total);
    assert!(checked > 100, "only {checked} fields");
}

#[test]
fn every_setting_is_reachable() {
    let b = board_of("", "{}", "[]");
    for field in schema::SETTINGS {
        let value = sample(&field.kind).unwrap_or_else(|| parse("{\"active\":true}"));
        let rev = b.settings_rev().expect("a rev");
        let v = Verb::Meta {
            rev,
            set: fields(&[(field.name, value.clone())]),
            unset: Vec::new(),
        };
        let (next, _, _) = run(&b, &v);
        assert_eq!(
            next.settings.get(field.name),
            Some(&value),
            "{}",
            field.name
        );
    }
}

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn the_parity_doc_matches_the_list() {
    let got = fs::read_to_string(repo().join("docs/PARITY.md")).expect("the doc");
    assert_eq!(
        got,
        parity_markdown(),
        "docs/PARITY.md differs from the list: run the generator"
    );
}

#[test]
fn the_verbs_doc_names_every_verb_code_and_template() {
    let doc = fs::read_to_string(repo().join("docs/VERBS.md")).expect("the doc");
    for verb in verbs::WRITE_VERBS.iter().chain(verbs::OTHER_VERBS) {
        assert!(
            doc.contains(&format!("`{verb}`")),
            "docs/VERBS.md doesn't name {verb}"
        );
    }
    for code in verbs::VerbCode::ALL {
        assert!(
            doc.contains(&format!("`{}`", code.as_str())),
            "docs/VERBS.md doesn't name {}",
            code.as_str()
        );
    }
    for (name, _) in verbs::TEMPLATES {
        assert!(
            doc.contains(&format!("`{name}`")),
            "docs/VERBS.md doesn't name {name}"
        );
    }
}

#[test]
fn verb_names_and_their_list_agree() {
    // Every name a verb gives is on the list, for one of each.
    let b = base();
    let samples = vec![
        Verb::Add {
            kind: ObjectType::Sticky,
            fields: Fields::new(),
            at: Position::Front,
        },
        Verb::Group {
            ids: vec![named(&b, "s-1")],
        },
        Verb::ClearBlob {
            hash: Sha256::of(b"x"),
        },
    ];
    for v in samples {
        assert!(verbs::WRITE_VERBS.contains(&v.name()), "{}", v.name());
    }
}
