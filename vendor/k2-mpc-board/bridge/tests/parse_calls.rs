//! Calls built in code: a host that makes a board verb itself, with no command line, gets the same
//! call the parser gives. Each constructor names its verb, says whether it reads or changes, and
//! plans or reads as the board crate does, with refusals as the kit's rows.

use k2_mpc_board_bridge::board::json::{Fields, Value};
use k2_mpc_board_bridge::board::verbs::{
    self, AlignTo, Axis, Ctx, End, Layer, Me, ObjRev, Planned, Removed, Side, Verb, VerbError,
};
use k2_mpc_board_bridge::board::*;
use k2_mpc_board_bridge::parse::{Call, CallKind, ReadVerb};
use k2_mpc_board_bridge::{read_value, value_out, HostBug, Why};
use std::collections::BTreeSet;

fn id(s: &str) -> Id {
    Id::new(s).expect("an id")
}

fn board() -> Board {
    let text = "{\"frames\":[],\"objects\":[\
{\"id\":\"s-1\",\"text\":\"one\",\"type\":\"sticky\",\"x\":12.5,\"y\":0},\
{\"id\":\"s-2\",\"text\":\"two\",\"type\":\"sticky\",\"x\":200,\"y\":0},\
{\"from\":{\"id\":\"s-1\"},\"id\":\"c-1\",\"to\":{\"id\":\"s-2\"},\"type\":\"connector\"}],\
\"people\":{\"p-1\":{\"kind\":\"agent\",\"name\":\"Ann\"}},\
\"settings\":{\"title\":\"T\",\"voting\":{\"active\":false}}}";
    Board::from_json(text.as_bytes(), &Limits::V1).expect("the board")
}

/// The same host each time: a counter for random bytes, one actor, one clock.
fn with_cx<T>(f: impl FnOnce(&mut Ctx<'_>) -> T) -> T {
    let mut n = 0u64;
    let mut random = move || {
        n += 1;
        n.to_le_bytes()
    };
    let mut cx = Ctx {
        group_id: "g".to_string(),
        by: Some(Me {
            token: id("p-1"),
            new: None,
        }),
        at: Some("2026-11-04T00:00:00Z".to_string()),
        random: &mut random,
    };
    f(&mut cx)
}

fn call_plan(call: &Call, b: &Board) -> Result<Planned, Why> {
    with_cx(|cx| call.plan(b, cx))
}

fn pack_plan(verb: &Verb, b: &Board) -> Result<Planned, VerbError> {
    with_cx(|cx| verbs::plan(b, cx, verb))
}

fn one_of_each() -> Vec<Verb> {
    let i = id("s-1");
    let r = Sha256::of(b"x");
    let named = || {
        vec![ObjRev {
            id: i.clone(),
            rev: r,
        }]
    };
    let t = || "t".to_string();
    let object = Object {
        id: i.clone(),
        kind: ObjectType::Sticky,
        fields: Fields::new(),
    };
    vec![
        Verb::Add {
            kind: ObjectType::Sticky,
            fields: Fields::new(),
            at: Position::Front,
        },
        Verb::Update {
            id: i.clone(),
            rev: r,
            set: Fields::new(),
            unset: Vec::new(),
        },
        Verb::Delete {
            id: i.clone(),
            rev: r,
            cascade: true,
        },
        Verb::PutBack {
            removed: Removed {
                object: object.clone(),
                after: Vec::new(),
                frame_after: Vec::new(),
            },
        },
        Verb::Connect {
            from: End::Point { x: 0.0, y: 0.0 },
            to: End::Point { x: 1.0, y: 1.0 },
            fields: Fields::new(),
        },
        Verb::React {
            id: i.clone(),
            emoji: "y".into(),
            on: Some(true),
        },
        Verb::Vote {
            id: i.clone(),
            delta: 1,
        },
        Verb::Comment {
            target: None,
            text: t(),
            point: None,
        },
        Verb::Reply {
            id: i.clone(),
            text: t(),
        },
        Verb::Resolve {
            id: i.clone(),
            rev: r,
            resolved: true,
        },
        Verb::Meta {
            rev: r,
            set: Fields::new(),
            unset: Vec::new(),
        },
        Verb::Lock {
            ids: named(),
            locked: true,
        },
        Verb::CheckAdd {
            id: i.clone(),
            text: t(),
        },
        Verb::CheckSet {
            id: i.clone(),
            rev: r,
            item: t(),
            text: None,
            done: None,
        },
        Verb::CheckRemove {
            id: i.clone(),
            rev: r,
            item: t(),
        },
        Verb::TableCell {
            id: i.clone(),
            rev: r,
            row: t(),
            column: t(),
            text: t(),
        },
        Verb::TableAddRow {
            id: i.clone(),
            after: None,
        },
        Verb::TableRemoveRow {
            id: i.clone(),
            rev: r,
            row: t(),
        },
        Verb::TableAddColumn {
            id: i.clone(),
            after: None,
            width: None,
        },
        Verb::TableRemoveColumn {
            id: i.clone(),
            rev: r,
            column: t(),
        },
        Verb::Move {
            ids: named(),
            dx: 1.0,
            dy: 1.0,
        },
        Verb::Order {
            ids: vec![i.clone()],
            to: Layer::Front,
        },
        Verb::FrameOrder {
            id: i.clone(),
            to: Position::Front,
        },
        Verb::Group { ids: named() },
        Verb::Ungroup { ids: named() },
        Verb::Align {
            ids: named(),
            to: AlignTo::Left,
        },
        Verb::Distribute {
            ids: named(),
            axis: Axis::Horizontal,
        },
        Verb::Tidy { ids: named() },
        Verb::CopyStyle {
            from: i.clone(),
            to: named(),
        },
        Verb::Duplicate {
            ids: vec![i.clone()],
            dx: 1.0,
            dy: 1.0,
        },
        Verb::Paste {
            objects: vec![object],
            x: 0.0,
            y: 0.0,
        },
        Verb::ConnectedCopy {
            id: i.clone(),
            side: Side::Top,
        },
        Verb::MindChild {
            parent: i.clone(),
            text: t(),
        },
        Verb::Template {
            name: "kanban".into(),
            x: 0.0,
            y: 0.0,
        },
        Verb::ClearBlob { hash: r },
    ]
}

fn one_of_each_read() -> Vec<ReadVerb> {
    vec![
        ReadVerb::List { kind: None },
        ReadVerb::Read { id: id("s-1") },
        ReadVerb::Search {
            text: "one".to_string(),
        },
        ReadVerb::Fields { type_name: None },
        ReadVerb::Templates,
        ReadVerb::References { id: None },
        ReadVerb::Meta,
    ]
}

#[test]
fn a_call_from_a_verb_names_it_and_changes_the_file() {
    let samples = one_of_each();
    let sampled: BTreeSet<&str> = samples.iter().map(Verb::name).collect();
    let write: BTreeSet<&str> = verbs::WRITE_VERBS.iter().copied().collect();
    assert_eq!(sampled, write, "one sample of every verb plan takes");
    for v in samples {
        let call = Call::from_verb(v.clone());
        assert_eq!(call.name(), v.name(), "the call names its verb");
        assert_eq!(
            call.kind(),
            CallKind::Change,
            "{} changes the file",
            v.name()
        );
    }
}

#[test]
fn a_read_call_names_its_verb_and_reads() {
    let samples = one_of_each_read();
    let sampled: BTreeSet<&str> = samples.iter().map(ReadVerb::name).collect();
    let mut want: BTreeSet<&str> = verbs::OTHER_VERBS
        .iter()
        .copied()
        .filter(|v| !matches!(*v, "apply" | "inverse"))
        .collect();
    want.insert("meta");
    assert_eq!(
        sampled, want,
        "one sample of every read verb, and meta's read"
    );
    for r in samples {
        let call = Call::from_read(r.clone());
        assert_eq!(call.name(), r.name(), "the call names its verb");
        assert_eq!(call.kind(), CallKind::Read, "{} reads", r.name());
    }
}

#[test]
fn a_call_from_a_verb_plans_as_the_board_crate_does() {
    let b = board();
    let verb = Verb::Add {
        kind: ObjectType::Sticky,
        fields: [("text".to_string(), Value::Str("hi".to_string()))]
            .into_iter()
            .collect(),
        at: Position::Front,
    };
    let want = pack_plan(&verb, &b).expect("check: the board crate plans the add");
    assert_eq!(call_plan(&Call::from_verb(verb), &b), Ok(want));
}

#[test]
fn a_planning_refusal_is_its_row_at_input() {
    let b = board();
    let gone = Call::from_verb(Verb::Delete {
        id: id("s-9"),
        rev: Sha256::of(b"x"),
        cascade: false,
    });
    match call_plan(&gone, &b) {
        Err(Why::Report(r)) => {
            assert_eq!(r.id(), "MPC-E838", "a delete of what isn't there");
            assert_eq!(r.message(), "no object s-9");
        }
        other => panic!("no report for the delete of what isn't there: {other:?}"),
    }
    let vote = Call::from_verb(Verb::Vote {
        id: id("s-1"),
        delta: 1,
    });
    match call_plan(&vote, &b) {
        Err(Why::Report(r)) => assert_eq!(r.id(), "MPC-E836", "voting is off"),
        other => panic!("no report for a vote while voting is off: {other:?}"),
    }
}

#[test]
fn a_read_call_reads_as_the_board_crate_does() {
    let b = board();
    let view = verbs::read(&b, &id("s-1"))
        .expect("check: the board crate reads s-1")
        .expect("check: s-1 is there");
    let want = read_value(&view).expect("check: the view converts");
    assert_eq!(
        Call::from_read(ReadVerb::Read { id: id("s-1") }).read(&b),
        Ok(want)
    );
    let meta = verbs::meta(&b).expect("check: the settings read");
    let want = value_out(&meta.to_value()).expect("check: the settings convert");
    assert_eq!(Call::from_read(ReadVerb::Meta).read(&b), Ok(want));
    let listed = Call::from_read(ReadVerb::List {
        kind: Some(ObjectType::Connector),
    })
    .read(&b)
    .expect("check: the list reads");
    let k2_mpc_board_bridge::kit::errors::Value::List(items) = listed else {
        panic!("a list isn't a list: {listed:?}");
    };
    assert_eq!(items.len(), 1, "one connector");
    let refs = Call::from_read(ReadVerb::References {
        id: Some(id("s-2")),
    })
    .read(&b)
    .expect("check: the references read");
    let k2_mpc_board_bridge::kit::errors::Value::List(refs) = refs else {
        panic!("references aren't a list: {refs:?}");
    };
    assert_eq!(refs.len(), 1, "the connector's end names s-2");
}

#[test]
fn reading_what_is_not_there_is_its_row() {
    let b = board();
    match Call::from_read(ReadVerb::Read { id: id("s-9") }).read(&b) {
        Err(Why::Report(r)) => {
            assert_eq!(r.id(), "MPC-E838", "a read of what isn't there");
            assert_eq!(r.message(), "no object s-9");
        }
        other => panic!("no report for a read of what isn't there: {other:?}"),
    }
}

#[test]
fn a_read_call_has_no_plan_and_a_change_call_no_read() {
    let b = board();
    let read = Call::from_read(ReadVerb::Templates);
    match call_plan(&read, &b) {
        Err(Why::HostBug(HostBug::UnknownError { what })) => assert!(
            what.contains("templates reads the file"),
            "planning a read is the host's bug, naming the verb: {what}"
        ),
        other => panic!("planning a read isn't the host's bug: {other:?}"),
    }
    let change = Call::from_verb(Verb::Vote {
        id: id("s-1"),
        delta: 1,
    });
    match change.read(&b) {
        Err(Why::HostBug(HostBug::UnknownError { what })) => assert!(
            what.contains("vote changes the file"),
            "reading a change is the host's bug, naming the verb: {what}"
        ),
        other => panic!("reading a change isn't the host's bug: {other:?}"),
    }
}
