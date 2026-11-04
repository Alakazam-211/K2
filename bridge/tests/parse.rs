//! The argument parser: a command line to a call, and back. Every verb of the board crate's
//! `verbs::ARGS` parses from argv to the same verb, positionally and by flags; the JSON-only
//! arguments go through their JSON form; `apply` takes its ops wrapped or bare; `--<field> null`
//! removes a field; each usage refusal is its row, every key its message names set; and each verb's
//! usage line comes with an example that parses.

use k2_mpc_board_bridge::board::json::{self, Fields, Value};
use k2_mpc_board_bridge::board::verbs::{
    self, AlignTo, Axis, Ctx, End, Layer, Me, ObjRev, Planned, Removed, Side, Verb,
};
use k2_mpc_board_bridge::board::*;
use k2_mpc_board_bridge::kit::errors::ErrorReport;
use k2_mpc_board_bridge::parse::{self, parse, Call, CallKind, Names, ReadVerb};
use k2_mpc_board_bridge::report::conflict_report;
use k2_mpc_board_bridge::{Phase, Why};
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

fn rev_of(b: &Board, i: &str) -> Sha256 {
    b.objects
        .iter()
        .find(|o| o.id.as_str() == i)
        .unwrap_or_else(|| panic!("no object {i}"))
        .rev()
        .expect("a rev")
}

fn argv(words: &[&str]) -> Vec<String> {
    words.iter().map(|w| w.to_string()).collect()
}

fn run(words: &[&str]) -> Result<Call, Why> {
    parse(&argv(words), &Names::default(), &Limits::V1)
}

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

fn plan(call: &Call, b: &Board) -> Result<Planned, Why> {
    with_cx(|cx| call.plan(b, cx))
}

fn canonical(v: &Value) -> String {
    String::from_utf8(json::to_canonical(v).expect("canonical JSON")).expect("UTF-8")
}

fn board_json(b: &Board) -> String {
    String::from_utf8(b.to_canonical_json().expect("the board's JSON")).expect("UTF-8")
}

// --- Text forms, written the way a command line gives them -----------------------------------

fn num(n: f64) -> String {
    json::number_to_string(n).expect("a number")
}

fn ids(list: &[Id]) -> String {
    list.iter().map(Id::to_string).collect::<Vec<_>>().join(",")
}

fn obj_revs(list: &[ObjRev]) -> String {
    list.iter()
        .map(|o| format!("{}@{}", o.id, o.rev))
        .collect::<Vec<_>>()
        .join(",")
}

fn position(p: &Position) -> String {
    match p {
        Position::Front => "front".into(),
        Position::Back => "back".into(),
        Position::After(list) => format!("after:{}", ids(list)),
    }
}

fn layer(l: &Layer) -> String {
    match l {
        Layer::Front => "front".into(),
        Layer::Back => "back".into(),
        Layer::Forward => "forward".into(),
        Layer::Backward => "backward".into(),
        Layer::After(list) => format!("after:{}", ids(list)),
    }
}

fn end(e: &End) -> String {
    match e {
        End::Object { id, anchor: None } => id.to_string(),
        End::Object {
            id,
            anchor: Some(a),
        } => format!("{id}:{a}"),
        End::Point { x, y } => format!("{},{}", num(*x), num(*y)),
    }
}

fn field_text(v: &Value) -> String {
    match v {
        Value::Str(s) => s.clone(),
        Value::Num(n) => num(*n),
        Value::Bool(b) => b.to_string(),
        other => canonical(other),
    }
}

/// One argument of a verb, as text: `None` when an optional one is left out.
struct ArgText {
    arg: &'static str,
    text: Option<String>,
}

fn arg_text(arg: &'static str, text: Option<String>) -> ArgText {
    ArgText { arg, text }
}

/// One `--<field> <text>` pair.
struct FieldText {
    name: String,
    text: String,
}

/// A verb's arguments as text, by name, and its `--<field>` pairs. The match has no wildcard and
/// no `..`, so a variant or a field this doesn't write doesn't build.
fn texts(v: &Verb) -> (Vec<ArgText>, Vec<FieldText>) {
    let s = |t: &str| Some(t.to_string());
    let set_pairs = |set: &Fields, unset: &[String]| -> Vec<FieldText> {
        set.iter()
            .map(|(k, v)| FieldText {
                name: k.clone(),
                text: field_text(v),
            })
            .chain(unset.iter().map(|k| FieldText {
                name: k.clone(),
                text: "null".to_string(),
            }))
            .collect()
    };
    match v {
        Verb::Add { kind, fields, at } => (
            vec![
                arg_text("kind", s(kind.as_str())),
                arg_text("at", Some(position(at))),
            ],
            set_pairs(fields, &[]),
        ),
        Verb::Update {
            id,
            rev,
            set,
            unset,
        } => (
            vec![
                arg_text("id", s(id.as_str())),
                arg_text("rev", Some(rev.to_string())),
            ],
            set_pairs(set, unset),
        ),
        Verb::Delete { id, rev, cascade } => (
            vec![
                arg_text("id", s(id.as_str())),
                arg_text("rev", Some(rev.to_string())),
                arg_text("cascade", Some(cascade.to_string())),
            ],
            vec![],
        ),
        Verb::PutBack { removed } => (
            vec![arg_text("removed", Some(canonical(&removed.to_value())))],
            vec![],
        ),
        Verb::Connect { from, to, fields } => (
            vec![
                arg_text("from", Some(end(from))),
                arg_text("to", Some(end(to))),
            ],
            set_pairs(fields, &[]),
        ),
        Verb::React { id, emoji, on } => (
            vec![
                arg_text("id", s(id.as_str())),
                arg_text("emoji", s(emoji)),
                arg_text("on", on.map(|b| b.to_string())),
            ],
            vec![],
        ),
        Verb::Vote { id, delta } => (
            vec![
                arg_text("id", s(id.as_str())),
                arg_text("delta", Some(delta.to_string())),
            ],
            vec![],
        ),
        Verb::Comment {
            target,
            text,
            point,
        } => (
            vec![
                arg_text("target", target.as_ref().map(Id::to_string)),
                arg_text("text", s(text)),
                arg_text(
                    "point",
                    point.map(|(x, y)| format!("{},{}", num(x), num(y))),
                ),
            ],
            vec![],
        ),
        Verb::Reply { id, text } => (
            vec![arg_text("id", s(id.as_str())), arg_text("text", s(text))],
            vec![],
        ),
        Verb::Resolve { id, rev, resolved } => (
            vec![
                arg_text("id", s(id.as_str())),
                arg_text("rev", Some(rev.to_string())),
                arg_text("resolved", Some(resolved.to_string())),
            ],
            vec![],
        ),
        Verb::Meta { rev, set, unset } => (
            vec![arg_text("rev", Some(rev.to_string()))],
            set_pairs(set, unset),
        ),
        Verb::Lock { ids, locked } => (
            vec![
                arg_text("ids", Some(obj_revs(ids))),
                arg_text("locked", Some(locked.to_string())),
            ],
            vec![],
        ),
        Verb::CheckAdd { id, text } => (
            vec![arg_text("id", s(id.as_str())), arg_text("text", s(text))],
            vec![],
        ),
        Verb::CheckSet {
            id,
            rev,
            item,
            text,
            done,
        } => (
            vec![
                arg_text("id", s(id.as_str())),
                arg_text("rev", Some(rev.to_string())),
                arg_text("item", s(item)),
                arg_text("text", text.clone()),
                arg_text("done", done.map(|b| b.to_string())),
            ],
            vec![],
        ),
        Verb::CheckRemove { id, rev, item } => (
            vec![
                arg_text("id", s(id.as_str())),
                arg_text("rev", Some(rev.to_string())),
                arg_text("item", s(item)),
            ],
            vec![],
        ),
        Verb::TableCell {
            id,
            rev,
            row,
            column,
            text,
        } => (
            vec![
                arg_text("id", s(id.as_str())),
                arg_text("rev", Some(rev.to_string())),
                arg_text("row", s(row)),
                arg_text("column", s(column)),
                arg_text("text", s(text)),
            ],
            vec![],
        ),
        Verb::TableAddRow { id, after } => (
            vec![
                arg_text("id", s(id.as_str())),
                arg_text("after", after.clone()),
            ],
            vec![],
        ),
        Verb::TableRemoveRow { id, rev, row } => (
            vec![
                arg_text("id", s(id.as_str())),
                arg_text("rev", Some(rev.to_string())),
                arg_text("row", s(row)),
            ],
            vec![],
        ),
        Verb::TableAddColumn { id, after, width } => (
            vec![
                arg_text("id", s(id.as_str())),
                arg_text("after", after.clone()),
                arg_text("width", width.map(num)),
            ],
            vec![],
        ),
        Verb::TableRemoveColumn { id, rev, column } => (
            vec![
                arg_text("id", s(id.as_str())),
                arg_text("rev", Some(rev.to_string())),
                arg_text("column", s(column)),
            ],
            vec![],
        ),
        Verb::Move { ids, dx, dy } => (
            vec![
                arg_text("ids", Some(obj_revs(ids))),
                arg_text("dx", Some(num(*dx))),
                arg_text("dy", Some(num(*dy))),
            ],
            vec![],
        ),
        Verb::Order { ids: list, to } => (
            vec![
                arg_text("ids", Some(ids(list))),
                arg_text("to", Some(layer(to))),
            ],
            vec![],
        ),
        Verb::FrameOrder { id, to } => (
            vec![
                arg_text("id", s(id.as_str())),
                arg_text("to", Some(position(to))),
            ],
            vec![],
        ),
        Verb::Group { ids } => (vec![arg_text("ids", Some(obj_revs(ids)))], vec![]),
        Verb::Ungroup { ids } => (vec![arg_text("ids", Some(obj_revs(ids)))], vec![]),
        Verb::Align { ids, to } => (
            vec![
                arg_text("ids", Some(obj_revs(ids))),
                arg_text("to", s(to.as_str())),
            ],
            vec![],
        ),
        Verb::Distribute { ids, axis } => (
            vec![
                arg_text("ids", Some(obj_revs(ids))),
                arg_text("axis", s(axis.as_str())),
            ],
            vec![],
        ),
        Verb::Tidy { ids } => (vec![arg_text("ids", Some(obj_revs(ids)))], vec![]),
        Verb::CopyStyle { from, to } => (
            vec![
                arg_text("from", s(from.as_str())),
                arg_text("to", Some(obj_revs(to))),
            ],
            vec![],
        ),
        Verb::Duplicate { ids: list, dx, dy } => (
            vec![
                arg_text("ids", Some(ids(list))),
                arg_text("dx", Some(num(*dx))),
                arg_text("dy", Some(num(*dy))),
            ],
            vec![],
        ),
        Verb::Paste { objects, x, y } => (
            vec![
                arg_text(
                    "objects",
                    Some(canonical(&Value::Arr(
                        objects.iter().map(Object::to_value).collect(),
                    ))),
                ),
                arg_text("x", Some(num(*x))),
                arg_text("y", Some(num(*y))),
            ],
            vec![],
        ),
        Verb::ConnectedCopy { id, side } => (
            vec![
                arg_text("id", s(id.as_str())),
                arg_text("side", s(side.as_str())),
            ],
            vec![],
        ),
        Verb::MindChild { parent, text } => (
            vec![
                arg_text("parent", s(parent.as_str())),
                arg_text("text", s(text)),
            ],
            vec![],
        ),
        Verb::Template { name, x, y } => (
            vec![
                arg_text("name", s(name)),
                arg_text("x", Some(num(*x))),
                arg_text("y", Some(num(*y))),
            ],
            vec![],
        ),
        Verb::ClearBlob { hash } => (vec![arg_text("hash", Some(hash.to_string()))], vec![]),
    }
}

/// `verbs::ARGS`'s entry for a verb: each argument, and whether it may be left out.
fn args_of(verb: &str) -> Vec<(&'static str, bool)> {
    let (_, args) = verbs::ARGS
        .iter()
        .find(|(n, _)| *n == verb)
        .unwrap_or_else(|| panic!("verbs::ARGS has no {verb}"));
    args.iter()
        .map(|a| match a.strip_suffix('?') {
            Some(n) => (n, true),
            None => (*a, false),
        })
        .collect()
}

/// The verb as a command line: its required arguments positional in `verbs::ARGS`'s order (or,
/// with `flags`, every one as `--<arg>=<text>`), the optional ones as flags, then its fields.
fn to_argv(v: &Verb, flags: bool) -> Vec<String> {
    let (values, pairs) = texts(v);
    let mut out = vec![v.name().to_string()];
    for (arg, optional) in args_of(v.name()) {
        let Some(ArgText {
            text: Some(value), ..
        }) = values.iter().find(|a| a.arg == arg)
        else {
            continue;
        };
        if flags || optional {
            out.push(format!("--{arg}={value}"));
        } else {
            out.push(value.clone());
        }
    }
    for f in pairs {
        out.push(format!("--{}", f.name));
        out.push(f.text);
    }
    out
}

fn samples(b: &Board) -> Vec<Verb> {
    let r1 = rev_of(b, "s-1");
    let r2 = rev_of(b, "s-2");
    let named = || {
        vec![
            ObjRev {
                id: id("s-1"),
                rev: r1,
            },
            ObjRev {
                id: id("s-2"),
                rev: r2,
            },
        ]
    };
    let fields = |pairs: &[(&str, Value)]| -> Fields {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    };
    let st = |t: &str| Value::Str(t.to_string());
    let pasted = Object {
        id: id("s-9"),
        kind: ObjectType::Sticky,
        fields: fields(&[("text", st("p"))]),
    };
    vec![
        Verb::Add {
            kind: ObjectType::Sticky,
            fields: fields(&[
                ("text", st("hi there")),
                ("x", Value::Num(-12.5)),
                ("tags", Value::Arr(vec![st("a"), st("b")])),
            ]),
            at: Position::After(vec![id("s-2")]),
        },
        Verb::Update {
            id: id("s-1"),
            rev: r1,
            set: fields(&[("text", st("new")), ("x", Value::Num(0.1))]),
            unset: vec!["link".to_string()],
        },
        Verb::Delete {
            id: id("s-1"),
            rev: r1,
            cascade: false,
        },
        Verb::PutBack {
            removed: Removed {
                object: pasted.clone(),
                after: vec![id("s-1")],
                frame_after: Vec::new(),
            },
        },
        Verb::Connect {
            from: End::Object {
                id: id("s-1"),
                anchor: Some("top".to_string()),
            },
            to: End::Point { x: 1.5, y: -2.0 },
            fields: fields(&[("label", st("l"))]),
        },
        Verb::React {
            id: id("s-1"),
            emoji: "+1".into(),
            on: Some(false),
        },
        Verb::Vote {
            id: id("s-1"),
            delta: -1,
        },
        Verb::Comment {
            target: Some(id("s-1")),
            text: "see --this".into(),
            point: Some((1.0, 2.5)),
        },
        Verb::Reply {
            id: id("q-1"),
            text: "yes".into(),
        },
        Verb::Resolve {
            id: id("q-1"),
            rev: r1,
            resolved: false,
        },
        Verb::Meta {
            rev: r1,
            set: fields(&[("title", st("T2"))]),
            unset: vec!["voting".to_string()],
        },
        Verb::Lock {
            ids: named(),
            locked: false,
        },
        Verb::CheckAdd {
            id: id("k-1"),
            text: "x".into(),
        },
        Verb::CheckSet {
            id: id("k-1"),
            rev: r1,
            item: "c1".into(),
            text: Some("y".into()),
            done: Some(true),
        },
        Verb::CheckRemove {
            id: id("k-1"),
            rev: r1,
            item: "c1".into(),
        },
        Verb::TableCell {
            id: id("b-1"),
            rev: r1,
            row: "r1".into(),
            column: "c1".into(),
            text: "cell".into(),
        },
        Verb::TableAddRow {
            id: id("b-1"),
            after: Some("r1".into()),
        },
        Verb::TableRemoveRow {
            id: id("b-1"),
            rev: r1,
            row: "r1".into(),
        },
        Verb::TableAddColumn {
            id: id("b-1"),
            after: None,
            width: Some(120.0),
        },
        Verb::TableRemoveColumn {
            id: id("b-1"),
            rev: r1,
            column: "c1".into(),
        },
        Verb::Move {
            ids: named(),
            dx: -10.5,
            dy: 0.0,
        },
        Verb::Order {
            ids: vec![id("s-1"), id("s-2")],
            to: Layer::After(vec![id("c-1")]),
        },
        Verb::FrameOrder {
            id: id("f-1"),
            to: Position::Back,
        },
        Verb::Group { ids: named() },
        Verb::Ungroup { ids: named() },
        Verb::Align {
            ids: named(),
            to: AlignTo::Center,
        },
        Verb::Distribute {
            ids: named(),
            axis: Axis::Vertical,
        },
        Verb::Tidy { ids: named() },
        Verb::CopyStyle {
            from: id("s-1"),
            to: named(),
        },
        Verb::Duplicate {
            ids: vec![id("s-1")],
            dx: 20.0,
            dy: 20.0,
        },
        Verb::Paste {
            objects: vec![pasted],
            x: 0.0,
            y: 300.0,
        },
        Verb::ConnectedCopy {
            id: id("s-1"),
            side: Side::Bottom,
        },
        Verb::MindChild {
            parent: id("m-1"),
            text: "kid".into(),
        },
        Verb::Template {
            name: "retro".into(),
            x: 5.0,
            y: 6.0,
        },
        Verb::ClearBlob { hash: r2 },
    ]
}

fn parsed_verb(words: &[String], b: &Board) -> Verb {
    let call = parse(words, &Names::default(), &Limits::V1)
        .unwrap_or_else(|e| panic!("{words:?} doesn't parse: {e:?}"));
    match call.verb(b) {
        Ok(Some(v)) => v,
        other => panic!("{words:?} gives no verb: {other:?}"),
    }
}

// --- 1. Every verb round-trips ---------------------------------------------------------------

#[test]
fn every_write_verb_round_trips_through_argv_positionally_and_by_flags() {
    let b = board();
    let all = samples(&b);
    let sampled: BTreeSet<&str> = all.iter().map(Verb::name).collect();
    let write: BTreeSet<&str> = verbs::WRITE_VERBS.iter().copied().collect();
    assert_eq!(sampled, write, "one sample of every verb plan takes");
    for v in &all {
        for flags in [false, true] {
            let words = to_argv(v, flags);
            assert_eq!(&parsed_verb(&words, &b), v, "{words:?} round-trips");
        }
    }
}

#[test]
fn every_read_verb_parses_to_its_read_call() {
    let cases: Vec<(Vec<&str>, ReadVerb)> = vec![
        (
            vec!["list", "--kind", "connector"],
            ReadVerb::List {
                kind: Some(ObjectType::Connector),
            },
        ),
        (vec!["list"], ReadVerb::List { kind: None }),
        (vec!["read", "s-1"], ReadVerb::Read { id: id("s-1") }),
        (
            vec!["search", "two words"],
            ReadVerb::Search {
                text: "two words".into(),
            },
        ),
        (
            vec!["fields", "--type=sticky"],
            ReadVerb::Fields {
                type_name: Some("sticky".into()),
            },
        ),
        (vec!["templates"], ReadVerb::Templates),
        (
            vec!["references", "--id", "s-2"],
            ReadVerb::References {
                id: Some(id("s-2")),
            },
        ),
        (vec!["meta"], ReadVerb::Meta),
    ];
    let covered: BTreeSet<&str> = cases.iter().map(|(w, _)| w[0]).collect();
    let mut want: BTreeSet<&str> = verbs::OTHER_VERBS
        .iter()
        .copied()
        .filter(|v| !matches!(*v, "apply" | "inverse"))
        .collect();
    want.insert("meta");
    assert_eq!(covered, want, "a case for every read verb");
    for (words, r) in cases {
        let call = run(&words).unwrap_or_else(|e| panic!("{words:?} doesn't parse: {e:?}"));
        assert_eq!(call, Call::from_read(r), "{words:?}");
        assert_eq!(call.kind(), CallKind::Read, "{words:?} reads");
    }
}

#[test]
fn every_json_only_argument_is_the_board_crates_and_parses_as_json() {
    let mut json_args = BTreeSet::new();
    for (verb, arg, why) in verbs::JSON_ARGS {
        assert!(!why.trim().is_empty(), "{verb} {arg}: JSON-only, and why");
        assert!(
            args_of(verb).iter().any(|(a, _)| a == arg),
            "{verb} has an argument {arg}"
        );
        let line = parse::usage(verb).unwrap_or_else(|| panic!("no usage for {verb}"));
        assert!(
            line.contains(&format!("<{arg}: JSON>")),
            "{verb}'s usage names {arg} as JSON: {line}"
        );
        json_args.insert((*verb, *arg));
    }
    for (verb, _) in verbs::ARGS {
        let line = parse::usage(verb).unwrap_or_else(|| panic!("no usage for {verb}"));
        for (arg, _) in args_of(verb) {
            if line.contains(&format!("<{arg}: JSON>")) {
                assert!(
                    json_args.contains(&(*verb, arg)),
                    "{verb}'s {arg} is JSON in its usage, and not in verbs::JSON_ARGS"
                );
            }
        }
    }
    let wanted: BTreeSet<(&str, &str)> = [
        ("put-back", "removed"),
        ("paste", "objects"),
        ("inverse", "before"),
        ("inverse", "after"),
        ("apply", "ops"),
    ]
    .into_iter()
    .collect();
    assert_eq!(json_args, wanted, "the JSON-only arguments");
}

#[test]
fn apply_and_inverse_parse_to_their_calls_and_change_the_file() {
    let b = board();
    let ops = "[{\"op\":\"add\",\"obj\":{\"id\":\"a\",\"type\":\"sticky\"}}]";
    let call = run(&["apply", ops]).expect("check: apply parses");
    assert_eq!(call, Call::apply(ops, &Limits::V1));
    assert_eq!(call.kind(), CallKind::Change, "apply changes the file");
    let before = board_json(&b);
    let call =
        run(&["inverse", &before, &before, "--group_id", "u1"]).expect("check: inverse parses");
    assert_eq!(call, Call::inverse(b.clone(), b.clone(), "u1"));
    assert_eq!(call.kind(), CallKind::Change, "inverse changes the file");
}

// --- The compound forms ----------------------------------------------------------------------

#[test]
fn every_layer_position_and_end_form_round_trips() {
    let b = board();
    let layers = [
        Layer::Front,
        Layer::Back,
        Layer::Forward,
        Layer::Backward,
        Layer::After(vec![id("s-1"), id("c-1")]),
    ];
    for to in layers {
        let v = Verb::Order {
            ids: vec![id("s-2")],
            to,
        };
        assert_eq!(parsed_verb(&to_argv(&v, false), &b), v);
    }
    let positions = [
        Position::Front,
        Position::Back,
        Position::After(vec![id("s-1"), id("s-2")]),
    ];
    for to in positions {
        let v = Verb::FrameOrder { id: id("f-1"), to };
        assert_eq!(parsed_verb(&to_argv(&v, false), &b), v);
    }
    let ends = [
        End::Object {
            id: id("s-1"),
            anchor: None,
        },
        End::Object {
            id: id("s-1"),
            anchor: Some("left".into()),
        },
        End::Point { x: -3.0, y: 0.5 },
    ];
    for from in ends {
        let v = Verb::Connect {
            from,
            to: End::Object {
                id: id("s-2"),
                anchor: None,
            },
            fields: Fields::new(),
        };
        assert_eq!(parsed_verb(&to_argv(&v, false), &b), v);
    }
}

#[test]
fn every_word_of_align_axis_and_side_round_trips() {
    let b = board();
    let r = rev_of(&b, "s-1");
    let one = || {
        vec![ObjRev {
            id: id("s-1"),
            rev: r,
        }]
    };
    for to in [
        AlignTo::Left,
        AlignTo::Center,
        AlignTo::Right,
        AlignTo::Top,
        AlignTo::Middle,
        AlignTo::Bottom,
    ] {
        let v = Verb::Align { ids: one(), to };
        assert_eq!(parsed_verb(&to_argv(&v, true), &b), v);
    }
    for axis in [Axis::Horizontal, Axis::Vertical] {
        let v = Verb::Distribute { ids: one(), axis };
        assert_eq!(parsed_verb(&to_argv(&v, true), &b), v);
    }
    for side in [Side::Top, Side::Right, Side::Bottom, Side::Left] {
        let v = Verb::ConnectedCopy {
            id: id("s-1"),
            side,
        };
        assert_eq!(parsed_verb(&to_argv(&v, true), &b), v);
    }
}

#[test]
fn a_number_takes_the_formats_number_rule() {
    let b = board();
    let r = rev_of(&b, "s-1").to_string();
    let v = parsed_verb(
        &argv(&["move", &format!("s-1@{r}"), "0.10000000000000001", "-0"]),
        &b,
    );
    let Verb::Move { dx, dy, .. } = v else {
        panic!("move gives no move: {v:?}");
    };
    assert_eq!(dx, 0.1, "the nearest double");
    assert_eq!(dy, 0.0, "-0 is 0");
}

#[test]
fn positional_text_after_a_double_dash_may_start_like_a_flag() {
    let b = board();
    let v = parsed_verb(&argv(&["reply", "q-1", "--", "--looks like a flag"]), &b);
    assert_eq!(
        v,
        Verb::Reply {
            id: id("q-1"),
            text: "--looks like a flag".into()
        }
    );
}

// --- 3. Usage refusals -----------------------------------------------------------------------

/// Every `{key}` the row's message names is in the details, and nothing is left unfilled.
fn keys_hold(r: &ErrorReport) {
    let mut rest = r.entry().message;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        let close = after.find('}').expect("check: a template key closes");
        let key = &after[..close];
        assert!(
            r.detail(key).is_some(),
            "{}: details lack {key}: {:?}",
            r.id(),
            r.details()
        );
        rest = &after[close + 1..];
    }
    assert!(
        !r.message().contains('{'),
        "{}: {:?} has a {{ left",
        r.id(),
        r.message()
    );
}

/// The call's refusal, which must be the row `want`, with every key its message names set.
fn refused(words: &[&str], want: &str) -> ErrorReport {
    match run(words) {
        Err(Why::Report(r)) => {
            assert_eq!(r.id(), want, "{words:?}: wanted {want}, got {}", r.id());
            keys_hold(&r);
            r
        }
        other => panic!("{words:?}: wanted {want}, got no row: {other:?}"),
    }
}

fn detail<'r>(r: &'r ErrorReport, key: &str) -> &'r str {
    r.detail(key)
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| panic!("{}: no text detail {key}: {:?}", r.id(), r.details()))
}

#[test]
fn an_unknown_verb_is_tool_unsupported() {
    let r = refused(&["frobnicate", "s-1"], "MPC-E120");
    assert_eq!(r.exit(), 2);
    assert_eq!(detail(&r, "flag"), "frobnicate");
    assert!(!detail(&r, "hint").is_empty(), "a hint");
}

#[test]
fn an_unknown_flag_is_tool_unsupported() {
    let r = refused(&["read", "s-1", "--bogus", "x"], "MPC-E120");
    assert_eq!(r.exit(), 2);
    assert_eq!(detail(&r, "flag"), "--bogus");
    assert_eq!(detail(&r, "verb"), "read");
    assert!(
        detail(&r, "hint").contains("--id"),
        "the hint names read's flags"
    );
    let r = refused(&["templates", "--kind", "x"], "MPC-E120");
    assert_eq!(detail(&r, "flag"), "--kind");
}

#[test]
fn a_missing_argument_is_tool_usage() {
    let r = refused(&["read"], "MPC-E122");
    assert_eq!(r.exit(), 2);
    assert_eq!(detail(&r, "arg"), "id");
    let r = refused(&["move", "s-1@x"], "MPC-E122");
    assert_eq!(detail(&r, "arg"), "dx", "the first one missing, in order");
    refused(&[], "MPC-E122");
}

#[test]
fn a_bad_value_is_tool_usage_with_the_value_apart() {
    let b = board();
    let rev = rev_of(&b, "s-1").to_string();
    let cases: Vec<(Vec<String>, &str)> = vec![
        (argv(&["move", "s-1@nope", "1", "2"]), "ids"),
        (argv(&["move", "s-1", "1", "2"]), "ids"),
        (argv(&["move", &format!("s-1@{rev}"), "east", "2"]), "dx"),
        (argv(&["move", &format!("s-1@{rev}"), "1e400", "2"]), "dx"),
        (argv(&["read", "S 1"]), "id"),
        (argv(&["delete", "s-1", &rev, "yes"]), "cascade"),
        (argv(&["vote", "s-1", "1.5"]), "delta"),
        (argv(&["order", "s-1", "sideways"]), "to"),
        (argv(&["frame-order", "f-1", "after:"]), "to"),
        (argv(&["align", &format!("s-1@{rev}"), "diagonal"]), "to"),
        (argv(&["comment", "hi", "--point", "1"]), "point"),
        (argv(&["connect", "1,2:top", "s-2"]), "from"),
        (argv(&["connect", "s-1:", "s-2"]), "from"),
        (argv(&["put-back", "{"]), "removed"),
        (argv(&["paste", "{}", "0", "0"]), "objects"),
        (argv(&["inverse", "[]", "{}", "u"]), "before"),
        (argv(&["add", "Sticky!", "front"]), "kind"),
    ];
    for (words, arg) in cases {
        let w: Vec<&str> = words.iter().map(String::as_str).collect();
        let r = refused(&w, "MPC-E122");
        assert_eq!(detail(&r, "arg"), arg, "{words:?}");
        assert!(r.detail("value").is_some(), "{words:?}: the value, apart");
        assert!(
            !r.message().contains("S 1") && !r.message().contains("Sticky!"),
            "{words:?}: the message names no value"
        );
    }
    let r = refused(&["move", &format!("s-1@{rev}"), "1e400", "2"], "MPC-E122");
    assert_eq!(
        detail(&r, "code"),
        "json_number_range",
        "the board crate's code"
    );
}

#[test]
fn a_command_line_that_does_not_hold_is_tool_usage() {
    let cases: Vec<Vec<&str>> = vec![
        vec!["read", "s-1", "s-2"],
        vec!["read", "--id", "s-1", "--id", "s-2"],
        vec!["read", "--id"],
        vec![
            "meta",
            "--rev",
            "sha256-0000000000000000000000000000000000000000000000000000000000000000",
        ],
        vec!["meta", "--title", "T"],
        vec!["add", "sticky", "front", "--text", "null"],
        vec!["connect", "s-1", "s-2", "--label", "null"],
    ];
    for words in cases {
        refused(&words, "MPC-E122");
    }
}

#[test]
fn a_fields_value_is_the_board_crates_to_refuse_at_input() {
    refused(&["add", "sticky", "front", "--bogus", "x"], "MPC-E850");
    refused(&["add", "widget", "front"], "MPC-E851");
    refused(&["add", "sticky", "front", "--x", "abc"], "MPC-E847");
    let r = refused(&["add", "sticky", "front", "--tags", "["], "MPC-E847");
    assert_eq!(detail(&r, "code"), "json_syntax");
    refused(
        &[
            "meta",
            "--rev",
            &rev_of(&board(), "s-1").to_string(),
            "--bogus",
            "x",
        ],
        "MPC-E850",
    );
}

// --- 4. apply: wrapped and bare --------------------------------------------------------------

#[test]
fn apply_takes_its_ops_wrapped_or_bare_to_the_same_group() {
    let b = board();
    let ops = "[{\"op\":\"add\",\"obj\":{\"id\":\"a\",\"type\":\"sticky\",\"text\":\"n\"}},\
{\"op\":\"add\",\"obj\":{\"id\":\"b\",\"type\":\"connector\",\"from\":{\"id\":\"a\"},\"to\":{\"id\":\"s-1\"}}}]";
    let wrapped = format!("{{\"ops\":{ops}}}");
    let bare =
        plan(&run(&["apply", ops]).expect("check: bare parses"), &b).expect("check: bare plans");
    let wrap = plan(
        &run(&["apply", &wrapped]).expect("check: wrapped parses"),
        &b,
    )
    .expect("check: wrapped plans");
    assert_eq!(bare, wrap, "the same group");
    assert_eq!(bare.group.ops.len(), 2, "two adds");
    assert_eq!(bare.labels.len(), 2, "each add's id a label");
}

#[test]
fn an_ops_refusal_is_the_ops_row_and_a_flag_refusal_the_input_row() {
    let b = board();
    match plan(&run(&["apply", "["]).expect("check: apply parses"), &b) {
        Err(Why::Report(r)) => {
            keys_hold(&r);
            assert_eq!(r.id(), "MPC-E849", "bad_ops, at Ops");
            assert_eq!(detail(&r, "code"), "json_syntax");
        }
        other => panic!("ops that don't parse aren't refused with a row: {other:?}"),
    }
    refused(&["add", "sticky", "front", "--tags", "["], "MPC-E847");
}

// --- 5. --<field> null unsets ----------------------------------------------------------------

#[test]
fn a_field_given_null_is_unset() {
    let b = board();
    let rev = rev_of(&b, "s-1").to_string();
    let v = parsed_verb(
        &argv(&["update", "s-1", &rev, "--text", "null", "--x", "3"]),
        &b,
    );
    assert_eq!(
        v,
        Verb::Update {
            id: id("s-1"),
            rev: rev_of(&b, "s-1"),
            set: [("x".to_string(), Value::Num(3.0))].into_iter().collect(),
            unset: vec!["text".to_string()],
        }
    );
    let v = parsed_verb(&argv(&["meta", &rev, "--voting", "null"]), &b);
    assert_eq!(
        v,
        Verb::Meta {
            rev: rev_of(&b, "s-1"),
            set: Fields::new(),
            unset: vec!["voting".to_string()],
        }
    );
}

#[test]
fn an_update_reads_its_fields_by_the_objects_type_on_the_board() {
    let b = board();
    let rev = rev_of(&b, "c-1").to_string();
    let call = run(&["update", "c-1", &rev, "--route", "straight"]).expect("check: it parses");
    assert!(call.verb(&b).is_ok(), "route is a connector's field");
    let rev = rev_of(&b, "s-1").to_string();
    let call = run(&["update", "s-1", &rev, "--route", "straight"]).expect("check: it parses");
    match call.verb(&b) {
        Err(Why::Report(r)) => assert_eq!(r.id(), "MPC-E850", "a sticky has no route"),
        other => panic!("a sticky's route isn't refused: {other:?}"),
    }
}

// --- inverse ---------------------------------------------------------------------------------

#[test]
fn inverse_after_an_edit_gives_the_board_back() {
    let before = board();
    let mut after = before.clone();
    let edit = Call::from_verb(Verb::Update {
        id: id("s-1"),
        rev: rev_of(&before, "s-1"),
        set: [("text".to_string(), Value::Str("changed".into()))]
            .into_iter()
            .collect(),
        unset: vec![],
    });
    let p = plan(&edit, &after).expect("check: the edit plans");
    apply_group(&mut after, &p.group).expect("check: the edit applies");
    let undo = run(&["inverse", &board_json(&before), &board_json(&after), "u1"])
        .expect("check: inverse parses");
    let p = plan(&undo, &after).expect("check: the inverse plans");
    let mut back = after.clone();
    apply_group(&mut back, &p.group).expect("check: the inverse applies");
    assert_eq!(
        board_json(&back),
        board_json(&before),
        "the board as it was"
    );
}

#[test]
fn inverse_after_someone_else_changed_it_is_a_conflict() {
    let before = board();
    let mut after = before.clone();
    let set = |text: &str, b: &Board| {
        Call::from_verb(Verb::Update {
            id: id("s-1"),
            rev: rev_of(b, "s-1"),
            set: [("text".to_string(), Value::Str(text.into()))]
                .into_iter()
                .collect(),
            unset: vec![],
        })
    };
    let p = plan(&set("mine", &after), &after).expect("check: the edit plans");
    apply_group(&mut after, &p.group).expect("check: the edit applies");
    let undo = run(&["inverse", &board_json(&before), &board_json(&after), "u1"])
        .expect("check: inverse parses");
    let mut now = after.clone();
    let p = plan(&set("theirs", &now), &now).expect("check: their edit plans");
    apply_group(&mut now, &p.group).expect("check: their edit applies");
    let p = plan(&undo, &now).expect("check: the inverse plans");
    let mut tried = now.clone();
    let c = apply_group(&mut tried, &p.group).expect_err("the undo conflicts with their edit");
    let r = conflict_report(&c, Phase::Input).expect("check: a conflict has a row");
    assert_eq!(r.id(), "MPC-E830", "conflict");
    assert_eq!(tried, now, "nothing changed");
}

// --- Usage lines ---------------------------------------------------------------------------

#[test]
fn every_verb_has_a_usage_line_naming_its_arguments_and_an_example_that_parses() {
    let b = board();
    for (verb, _) in verbs::ARGS {
        let line = parse::usage(verb).unwrap_or_else(|| panic!("no usage for {verb}"));
        assert!(
            line.starts_with(&format!("{verb} ")) || line == *verb,
            "{line}"
        );
        for (arg, optional) in args_of(verb) {
            if matches!(arg, "fields" | "set" | "unset") {
                assert!(
                    line.contains("> <text>|null]…"),
                    "{verb} takes fields: {line}"
                );
                continue;
            }
            let named = if optional {
                line.contains(&format!("[--{arg} <{arg}"))
            } else {
                line.contains(&format!(" <{arg}>")) || line.contains(&format!(" <{arg}: "))
            };
            assert!(named, "{verb}'s usage names {arg}: {line}");
        }
        let words = parse::example(verb).unwrap_or_else(|| panic!("no example for {verb}"));
        assert_eq!(words[0], *verb, "the example runs {verb}");
        let call = parse(&words, &Names::default(), &Limits::V1)
            .unwrap_or_else(|e| panic!("{verb}'s example {words:?} doesn't parse: {e:?}"));
        assert_eq!(call.name(), *verb, "{words:?}");
        assert!(call.verb(&b).is_ok(), "{verb}'s example builds: {words:?}");
    }
    assert_eq!(parse::usage("frobnicate"), None);
    assert_eq!(parse::example("frobnicate"), None);
    assert!(
        parse::usage("apply").is_some_and(|l| l.contains("bare list")),
        "apply's usage says both forms"
    );
}

#[test]
fn every_example_uses_every_flag_its_usage_line_names() {
    for (verb, _) in verbs::ARGS {
        let line = parse::usage(verb).unwrap_or_else(|| panic!("no usage for {verb}"));
        let words = parse::example(verb).unwrap_or_else(|| panic!("no example for {verb}"));
        let mut named = 0;
        for part in line.split("[--").skip(1) {
            let flag = part.split([' ', ']']).next().unwrap_or("");
            if flag.starts_with('<') {
                // `[--<field> <text>|null]…`: the example gives one field.
                assert!(
                    words.iter().skip(1).any(|w| w.starts_with("--")),
                    "{verb}'s example gives a field: {words:?}"
                );
                continue;
            }
            named += 1;
            assert!(
                words.iter().any(|w| *w == format!("--{flag}")),
                "{verb}'s example uses --{flag}, as its usage line names: {words:?}"
            );
        }
        let optional = args_of(verb).iter().filter(|(_, o)| *o).count();
        assert_eq!(
            named, optional,
            "{verb}: a flag in the usage line for each optional argument"
        );
    }
}
