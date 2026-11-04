//! Groups of edits: the revision compare, all or nothing, merges, order and locks.

use k2_mpc_board::json::{Fields, Value};
use k2_mpc_board::*;

const P1: &str = "p-1";
const P2: &str = "p-2";

fn id(s: &str) -> Id {
    Id::new(s).expect("an id")
}

fn fields(pairs: &[(&str, Value)]) -> Fields {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

fn text(s: &str) -> Value {
    Value::Str(s.to_string())
}

fn parse(s: &str) -> Value {
    json::parse(s.as_bytes(), &Limits::V1).expect("JSON")
}

fn base() -> Board {
    let text = format!(
        "{{\"frames\":[\"f-1\",\"f-2\"],\"objects\":[\
{{\"id\":\"f-1\",\"title\":\"One\",\"type\":\"frame\"}},\
{{\"id\":\"f-2\",\"title\":\"Two\",\"type\":\"frame\"}},\
{{\"id\":\"s-1\",\"later\":{{\"keep\":1}},\"text\":\"hello\",\"type\":\"sticky\",\"x\":1.5}},\
{{\"id\":\"s-2\",\"reactions\":{{\"new\":[\"{P1}\",\"{P2}\"],\"x\":[\"{P2}\"]}},\"type\":\"sticky\",\"votes\":{{\"{P1}\":1,\"{P2}\":4}}}},\
{{\"checks\":[{{\"done\":false,\"id\":\"c1\",\"mark\":\"keep\",\"text\":\"one\"}},{{\"done\":false,\"id\":\"c2\",\"text\":\"two\"}}],\"id\":\"k-1\",\"type\":\"card\"}},\
{{\"columns\":[{{\"id\":\"a\",\"tint\":\"keep\",\"width\":10}},{{\"id\":\"b\"}}],\"id\":\"b-1\",\"rows\":[{{\"cells\":{{\"a\":\"x\"}},\"id\":\"r1\",\"tall\":true}},{{\"id\":\"r2\"}}],\"type\":\"table\"}},\
{{\"id\":\"q-1\",\"replies\":[{{\"at\":\"2026-11-03T10:00:00Z\",\"by\":\"{P1}\",\"id\":\"r1\",\"pin\":true,\"text\":\"hi\"}}],\"target\":\"s-1\",\"type\":\"comment\"}},\
{{\"id\":\"l-1\",\"locked\":true,\"text\":\"fixed\",\"type\":\"sticky\"}}\
],\"people\":{{\"{P1}\":{{\"name\":\"Ann\"}},\"{P2}\":{{\"name\":\"Bo\"}}}},\"settings\":{{\"keep\":true,\"title\":\"T\"}}}}"
    );
    Board::from_json(text.as_bytes(), &Limits::V1).expect("the base board")
}

fn obj<'b>(b: &'b Board, i: &str) -> &'b Object {
    b.objects
        .iter()
        .find(|o| o.id.as_str() == i)
        .expect("that object")
}

fn rev(b: &Board, i: &str) -> Sha256 {
    obj(b, i).rev().expect("a rev")
}

fn order(b: &Board) -> Vec<&str> {
    b.objects.iter().map(|o| o.id.as_str()).collect()
}

fn frames(b: &Board) -> Vec<&str> {
    b.frames.iter().map(|o| o.as_str()).collect()
}

fn group(ops: Vec<Op>) -> Group {
    Group {
        id: "g".to_string(),
        by: None,
        ops,
    }
}

fn apply(b: &mut Board, ops: Vec<Op>) -> Result<Report, Conflict> {
    apply_group(b, &group(ops))
}

fn apply_by(b: &mut Board, by: &str, ops: Vec<Op>) -> Result<Report, Conflict> {
    let mut g = group(ops);
    g.by = Some(id(by));
    apply_group(b, &g)
}

fn set(b: &Board, i: &str, pairs: &[(&str, Value)]) -> Op {
    Op::Set {
        id: id(i),
        rev: rev(b, i),
        set: fields(pairs),
        unset: Vec::new(),
    }
}

fn new_sticky(i: &str) -> Object {
    Object {
        id: id(i),
        kind: ObjectType::Sticky,
        fields: Fields::new(),
    }
}

fn stale() -> Sha256 {
    Sha256::of(b"something else")
}

fn refused(r: Result<Report, Conflict>) -> RefusalCode {
    match r {
        Err(Conflict::Refused(x)) => x.code,
        other => {
            assert_eq!(format!("{other:?}"), "a refusal");
            RefusalCode::IoError
        }
    }
}

#[test]
fn a_set_changes_only_the_named_field() {
    let mut b = base();
    let before = obj(&b, "s-1").clone();
    let op = set(&b, "s-1", &[("text", text("HELLO"))]);
    let report = apply(&mut b, vec![op]).expect("applies");
    assert_eq!(report.changed, vec![id("s-1")]);
    let after = obj(&b, "s-1");
    assert_eq!(after.fields.get("text"), Some(&text("HELLO")));
    assert_eq!(
        after.fields.get("x"),
        before.fields.get("x"),
        "a field it didn't name moved"
    );
    assert_eq!(
        after.fields.get("later"),
        before.fields.get("later"),
        "an unknown field moved"
    );
    b.validate(&Limits::V1).expect("still valid");
}

#[test]
fn unset_removes_a_field() {
    let mut b = base();
    let op = Op::Set {
        id: id("s-1"),
        rev: rev(&b, "s-1"),
        set: Fields::new(),
        unset: vec!["later".to_string()],
    };
    apply(&mut b, vec![op]).expect("applies");
    assert!(
        !obj(&b, "s-1").fields.contains_key("later"),
        "the field stayed"
    );
}

#[test]
fn a_stale_set_conflicts_and_says_whats_there() {
    let mut b = base();
    let op = Op::Set {
        id: id("s-1"),
        rev: stale(),
        set: fields(&[("text", text("mine"))]),
        unset: Vec::new(),
    };
    let c = apply(&mut b, vec![op]).expect_err("a conflict");
    assert_eq!(
        c,
        Conflict::Changed {
            target: Target::Object(id("s-1")),
            theirs: obj(&b, "s-1").to_value(),
        }
    );
}

#[test]
fn a_set_on_a_gone_object_conflicts() {
    let mut b = base();
    let op = Op::Set {
        id: id("gone"),
        rev: stale(),
        set: Fields::new(),
        unset: Vec::new(),
    };
    assert_eq!(
        apply(&mut b, vec![op]).expect_err("a conflict"),
        Conflict::Deleted { id: id("gone") }
    );
}

#[test]
fn a_del_of_a_gone_object_is_a_noop() {
    let mut b = base();
    let before = b.to_canonical_json().expect("json");
    let report = apply(
        &mut b,
        vec![Op::Del {
            id: id("gone"),
            rev: stale(),
        }],
    )
    .expect("a no-op");
    assert_eq!(report.already_gone, vec![id("gone")]);
    assert!(report.is_noop(), "nothing changed, so nothing to write");
    assert_eq!(b.to_canonical_json().expect("json"), before);
}

#[test]
fn a_stale_del_conflicts() {
    let mut b = base();
    let c = apply(
        &mut b,
        vec![Op::Del {
            id: id("s-1"),
            rev: stale(),
        }],
    )
    .expect_err("a conflict");
    assert!(matches!(c, Conflict::Changed { .. }), "{c:?}");
}

#[test]
fn a_del_takes_a_frame_out_of_frames_and_leaves_references_orphaned() {
    let mut b = base();
    let (r1, r2) = (rev(&b, "f-1"), rev(&b, "s-1"));
    let report = apply(
        &mut b,
        vec![
            Op::Del {
                id: id("f-1"),
                rev: r1,
            },
            Op::Del {
                id: id("s-1"),
                rev: r2,
            },
        ],
    )
    .expect("applies");
    assert_eq!(report.deleted, vec![id("f-1"), id("s-1")]);
    assert_eq!(frames(&b), vec!["f-2"]);
    b.validate(&Limits::V1).expect("still valid");
    let target = b
        .references()
        .into_iter()
        .find(|r| r.field == "target")
        .expect("the comment's");
    assert_eq!(
        target.state,
        RefState::Orphaned,
        "what referred to it stays, orphaned"
    );
}

#[test]
fn an_add_with_an_id_in_use_conflicts() {
    let mut b = base();
    let c = apply(
        &mut b,
        vec![Op::Add {
            object: new_sticky("s-1"),
            at: Position::Front,
        }],
    )
    .expect_err("a conflict");
    assert_eq!(
        c,
        Conflict::Exists {
            target: Target::Object(id("s-1")),
            theirs: obj(&b, "s-1").to_value(),
        }
    );
}

#[test]
fn an_add_lands_where_it_says() {
    let mut b = base();
    let report = apply(
        &mut b,
        vec![
            Op::Add {
                object: new_sticky("top"),
                at: Position::Front,
            },
            Op::Add {
                object: new_sticky("bottom"),
                at: Position::Back,
            },
            Op::Add {
                object: new_sticky("mid"),
                at: Position::After(vec![id("s-1")]),
            },
        ],
    )
    .expect("applies");
    assert_eq!(report.added, vec![id("top"), id("bottom"), id("mid")]);
    let o = order(&b);
    assert_eq!(o[0], "bottom");
    assert_eq!(*o.last().expect("one"), "top");
    let at = o.iter().position(|x| *x == "s-1").expect("there");
    assert_eq!(o[at + 1], "mid");
}

#[test]
fn an_added_frame_joins_frames() {
    let mut b = base();
    let mut frame = new_sticky("f-3");
    frame.kind = ObjectType::Frame;
    apply(
        &mut b,
        vec![Op::Add {
            object: frame,
            at: Position::Back,
        }],
    )
    .expect("applies");
    assert_eq!(frames(&b), vec!["f-1", "f-2", "f-3"]);
    b.validate(&Limits::V1).expect("still valid");
}

#[test]
fn ops_in_a_group_see_each_other() {
    let mut b = base();
    let r = stale();
    let start = rev(&b, "s-1");
    let report = apply(
        &mut b,
        vec![
            Op::Add {
                object: new_sticky("new"),
                at: Position::Front,
            },
            Op::Set {
                id: id("new"),
                rev: r,
                set: fields(&[("text", text("a"))]),
                unset: Vec::new(),
            },
            Op::Set {
                id: id("s-1"),
                rev: start,
                set: fields(&[("text", text("b"))]),
                unset: Vec::new(),
            },
            Op::Set {
                id: id("s-1"),
                rev: start,
                set: fields(&[("x", Value::Num(9.0))]),
                unset: Vec::new(),
            },
        ],
    )
    .expect("a second op on one object names the revision the group started from");
    assert_eq!(obj(&b, "new").fields.get("text"), Some(&text("a")));
    assert_eq!(obj(&b, "s-1").fields.get("text"), Some(&text("b")));
    assert_eq!(obj(&b, "s-1").fields.get("x"), Some(&Value::Num(9.0)));
    assert_eq!(
        report.changed,
        vec![id("s-1")],
        "listed once, and an added object isn't changed"
    );
    assert_eq!(report.added, vec![id("new")]);
}

#[test]
fn a_deleted_id_can_come_back_in_one_group() {
    let mut b = base();
    let r = rev(&b, "s-1");
    apply(
        &mut b,
        vec![
            Op::Del {
                id: id("s-1"),
                rev: r,
            },
            Op::Add {
                object: new_sticky("s-1"),
                at: Position::Back,
            },
        ],
    )
    .expect("applies");
    assert_eq!(order(&b)[0], "s-1");
    assert!(obj(&b, "s-1").fields.is_empty());
}

fn failing_groups(b: &Board) -> Vec<(&'static str, Vec<Op>)> {
    let good = set(b, "s-1", &[("text", text("changed first"))]);
    let good_add = Op::Add {
        object: new_sticky("added"),
        at: Position::Front,
    };
    let good_del = Op::Del {
        id: id("k-1"),
        rev: rev(b, "k-1"),
    };
    let good_z = Op::Z {
        id: id("s-2"),
        to: Position::Back,
    };
    let good_frames = Op::FrameOrder {
        id: id("f-2"),
        to: Position::Back,
    };
    let good_meta = Op::Meta {
        rev: b.settings_rev().expect("rev"),
        set: fields(&[("title", text("New"))]),
        unset: Vec::new(),
    };
    let good_person = Op::AddPerson {
        token: id("p-9"),
        person: Person::default(),
    };
    let good_clear = Op::ClearBlob { hash: stale() };
    let all_good = vec![
        good,
        good_add,
        good_del,
        good_z,
        good_frames,
        good_meta,
        good_person,
        good_clear,
    ];
    let with = |last: Op| {
        let mut ops = all_good.clone();
        ops.push(last);
        ops
    };
    vec![
        (
            "stale set",
            with(Op::Set {
                id: id("s-2"),
                rev: stale(),
                set: Fields::new(),
                unset: Vec::new(),
            }),
        ),
        (
            "gone set",
            with(Op::Set {
                id: id("gone"),
                rev: stale(),
                set: Fields::new(),
                unset: Vec::new(),
            }),
        ),
        (
            "stale del",
            with(Op::Del {
                id: id("s-2"),
                rev: stale(),
            }),
        ),
        (
            "add in use",
            with(Op::Add {
                object: new_sticky("s-2"),
                at: Position::Back,
            }),
        ),
        (
            "stale meta",
            with(Op::Meta {
                rev: stale(),
                set: Fields::new(),
                unset: Vec::new(),
            }),
        ),
        (
            "bad value",
            with(set(b, "s-2", &[("text", Value::Num(1.0))])),
        ),
        (
            "unknown person",
            with(set(b, "s-2", &[("votes", parse("{\"p-nobody\":1}"))])),
        ),
        ("locked", with(set(b, "l-1", &[("text", text("no"))]))),
        (
            "gone move",
            with(Op::Z {
                id: id("gone"),
                to: Position::Front,
            }),
        ),
        (
            "bad settings",
            with(Op::Meta {
                rev: b.settings_rev().expect("rev"),
                set: fields(&[("title", Value::Num(1.0))]),
                unset: Vec::new(),
            }),
        ),
    ]
}

#[test]
fn a_failed_group_leaves_the_board_byte_identical() {
    let start = base();
    let before = start.to_canonical_json().expect("json");
    for (name, ops) in failing_groups(&start) {
        let mut b = start.clone();
        let r = apply(&mut b, ops);
        assert!(r.is_err(), "{name}: wanted a failure");
        assert!(
            b.to_canonical_json().expect("json") == before,
            "{name}: the failed group changed the board"
        );
        assert_eq!(b, start, "{name}: the failed group changed the model");
    }
}

#[test]
fn the_rest_go_on_after_a_failed_group() {
    let mut b = base();
    let good = set(&b, "s-1", &[("text", text("kept"))]);
    assert!(apply(
        &mut b,
        vec![Op::Del {
            id: id("s-2"),
            rev: stale()
        }]
    )
    .is_err());
    apply(&mut b, vec![good]).expect("the next group applies");
    assert_eq!(obj(&b, "s-1").fields.get("text"), Some(&text("kept")));
}

#[test]
fn settings_name_their_revision() {
    let mut b = base();
    let c = apply(
        &mut b,
        vec![Op::Meta {
            rev: stale(),
            set: fields(&[("title", text("x"))]),
            unset: Vec::new(),
        }],
    )
    .expect_err("a conflict");
    assert_eq!(
        c,
        Conflict::Changed {
            target: Target::Settings,
            theirs: Value::Obj(b.settings.clone()),
        }
    );
    let r = b.settings_rev().expect("rev");
    let report = apply(
        &mut b,
        vec![Op::Meta {
            rev: r,
            set: fields(&[("title", text("x"))]),
            unset: vec!["keep".to_string()],
        }],
    )
    .expect("applies");
    assert!(report.settings_changed);
    assert_eq!(b.settings.get("title"), Some(&text("x")));
    assert!(!b.settings.contains_key("keep"), "unset didn't remove it");
}

#[test]
fn a_meta_that_changes_nothing_is_a_noop() {
    let mut b = base();
    let r = b.settings_rev().expect("rev");
    let report = apply(
        &mut b,
        vec![Op::Meta {
            rev: r,
            set: fields(&[("title", text("T"))]),
            unset: Vec::new(),
        }],
    )
    .expect("applies");
    assert!(report.is_noop(), "{report:?}");
}

#[test]
fn reactions_change_only_the_callers_entries() {
    let mut b = base();
    // The caller's view lacks the emoji it can't show, and P2's entry under "x".
    let op = set(
        &b,
        "s-2",
        &[("reactions", parse(&format!("{{\"y\":[\"{P1}\"]}}")))],
    );
    apply_by(&mut b, P1, vec![op]).expect("applies");
    assert_eq!(
        obj(&b, "s-2").fields.get("reactions"),
        Some(&parse(&format!(
            "{{\"new\":[\"{P2}\"],\"x\":[\"{P2}\"],\"y\":[\"{P1}\"]}}"
        ))),
        "someone else's reaction went, or P1's stayed"
    );
    b.validate(&Limits::V1).expect("sorted and unique");
}

#[test]
fn a_callers_entry_sorts_in_and_an_emptied_reaction_goes() {
    let mut b = base();
    let op = set(
        &b,
        "s-2",
        &[("reactions", parse(&format!("{{\"x\":[\"{P1}\"]}}")))],
    );
    apply_by(&mut b, P1, vec![op]).expect("applies");
    assert_eq!(
        obj(&b, "s-2").fields.get("reactions"),
        Some(&parse(&format!(
            "{{\"new\":[\"{P2}\"],\"x\":[\"{P1}\",\"{P2}\"]}}"
        ))),
    );
    let op = set(&b, "s-2", &[("reactions", parse("{}"))]);
    apply_by(&mut b, P2, vec![op]).expect("applies");
    assert_eq!(
        obj(&b, "s-2").fields.get("reactions"),
        Some(&parse(&format!("{{\"x\":[\"{P1}\"]}}"))),
        "an emptied reaction stayed"
    );
    let op = set(&b, "s-2", &[("reactions", parse("{}"))]);
    apply_by(&mut b, P1, vec![op]).expect("applies");
    assert!(
        !obj(&b, "s-2").fields.contains_key("reactions"),
        "an empty field stayed"
    );
}

#[test]
fn with_no_caller_reactions_and_votes_are_replaced_whole() {
    let mut b = base();
    let op = set(
        &b,
        "s-2",
        &[
            ("reactions", parse(&format!("{{\"z\":[\"{P1}\"]}}"))),
            ("votes", parse(&format!("{{\"{P1}\":7}}"))),
        ],
    );
    apply(&mut b, vec![op]).expect("applies");
    assert_eq!(
        obj(&b, "s-2").fields.get("reactions"),
        Some(&parse(&format!("{{\"z\":[\"{P1}\"]}}")))
    );
    assert_eq!(
        obj(&b, "s-2").fields.get("votes"),
        Some(&parse(&format!("{{\"{P1}\":7}}")))
    );
}

#[test]
fn votes_change_only_the_callers_entry() {
    let mut b = base();
    let op = set(
        &b,
        "s-2",
        &[("votes", parse(&format!("{{\"{P1}\":3,\"{P2}\":99}}")))],
    );
    apply_by(&mut b, P1, vec![op]).expect("applies");
    assert_eq!(
        obj(&b, "s-2").fields.get("votes"),
        Some(&parse(&format!("{{\"{P1}\":3,\"{P2}\":4}}"))),
        "someone else's votes moved"
    );
    let op = set(&b, "s-2", &[("votes", parse("{}"))]);
    apply_by(&mut b, P1, vec![op]).expect("applies");
    assert_eq!(
        obj(&b, "s-2").fields.get("votes"),
        Some(&parse(&format!("{{\"{P2}\":4}}"))),
        "the caller's votes stayed"
    );
    let op = Op::Set {
        id: id("s-2"),
        rev: rev(&b, "s-2"),
        set: Fields::new(),
        unset: vec!["votes".to_string(), "reactions".to_string()],
    };
    apply_by(&mut b, P1, vec![op]).expect("applies");
    assert_eq!(
        obj(&b, "s-2").fields.get("votes"),
        Some(&parse(&format!("{{\"{P2}\":4}}"))),
        "unset with a caller removes only the caller's"
    );
    assert!(
        obj(&b, "s-2").fields.contains_key("reactions"),
        "others' reactions went"
    );
}

#[test]
fn lists_merge_by_id_and_keep_unknown_sub_fields() {
    let mut b = base();
    let op = set(
        &b,
        "k-1",
        &[("checks", parse("[{\"done\":true,\"id\":\"c3\",\"text\":\"new\"},{\"done\":true,\"id\":\"c1\",\"text\":\"ONE\"}]"))],
    );
    apply(&mut b, vec![op]).expect("applies");
    assert_eq!(
        obj(&b, "k-1").fields.get("checks"),
        Some(&parse("[{\"done\":true,\"id\":\"c3\",\"text\":\"new\"},{\"done\":true,\"id\":\"c1\",\"mark\":\"keep\",\"text\":\"ONE\"}]")),
        "the caller's list sets the order and members; an item keeps its unknown fields"
    );
}

#[test]
fn replies_rows_and_columns_merge_the_same_way() {
    let mut b = base();
    let reply = format!(
        "[{{\"at\":\"2026-11-03T10:00:00Z\",\"by\":\"{P1}\",\"id\":\"r1\",\"text\":\"edited\"}}]"
    );
    let op = set(&b, "q-1", &[("replies", parse(&reply))]);
    apply(&mut b, vec![op]).expect("applies");
    assert_eq!(
        obj(&b, "q-1").fields.get("replies"),
        Some(&parse(&format!("[{{\"at\":\"2026-11-03T10:00:00Z\",\"by\":\"{P1}\",\"id\":\"r1\",\"pin\":true,\"text\":\"edited\"}}]")))
    );
    let op = set(
        &b,
        "b-1",
        &[
            ("rows", parse("[{\"cells\":{\"b\":\"y\"},\"id\":\"r1\"}]")),
            (
                "columns",
                parse("[{\"id\":\"a\",\"width\":20},{\"id\":\"b\"}]"),
            ),
        ],
    );
    apply(&mut b, vec![op]).expect("applies");
    assert_eq!(
        obj(&b, "b-1").fields.get("rows"),
        Some(&parse(
            "[{\"cells\":{\"b\":\"y\"},\"id\":\"r1\",\"tall\":true}]"
        ))
    );
    assert_eq!(
        obj(&b, "b-1").fields.get("columns"),
        Some(&parse(
            "[{\"id\":\"a\",\"tint\":\"keep\",\"width\":20},{\"id\":\"b\"}]"
        ))
    );
}

#[test]
fn a_move_goes_to_its_nearest_named_neighbour() {
    let mut b = base();
    let report = apply(
        &mut b,
        vec![Op::Z {
            id: id("s-1"),
            to: Position::After(vec![id("gone"), id("b-1")]),
        }],
    )
    .expect("applies");
    let o = order(&b);
    let at = o.iter().position(|x| *x == "b-1").expect("there");
    assert_eq!(
        o[at + 1],
        "s-1",
        "it isn't above the first neighbour still there"
    );
    assert_eq!(
        report.moved,
        vec![Moved {
            id: id("s-1"),
            list: OrderList::Objects,
            after: Some(id("b-1")),
            exact: false
        }]
    );
}

#[test]
fn a_move_with_no_neighbour_left_goes_to_the_bottom() {
    let mut b = base();
    let report = apply(
        &mut b,
        vec![Op::Z {
            id: id("s-1"),
            to: Position::After(vec![id("gone")]),
        }],
    )
    .expect("applies");
    assert_eq!(order(&b)[0], "s-1");
    assert_eq!(report.moved[0].after, None);
    assert!(!report.moved[0].exact);
}

#[test]
fn a_move_ignores_itself_as_a_neighbour() {
    let mut b = base();
    let report = apply(
        &mut b,
        vec![Op::Z {
            id: id("s-1"),
            to: Position::After(vec![id("s-1"), id("f-1")]),
        }],
    )
    .expect("applies");
    let o = order(&b);
    assert_eq!(&o[..2], &["f-1", "s-1"]);
    assert!(report.moved[0].exact, "the first other neighbour was there");
}

#[test]
fn a_move_that_changes_nothing_is_a_noop() {
    let mut b = base();
    let before = b.to_canonical_json().expect("json");
    let report = apply(
        &mut b,
        vec![Op::Z {
            id: id("f-1"),
            to: Position::Back,
        }],
    )
    .expect("applies");
    assert!(report.is_noop(), "{report:?}");
    assert_eq!(b.to_canonical_json().expect("json"), before);
}

#[test]
fn the_later_save_wins_on_position_and_nothing_is_lost() {
    // Two saves from one read move the same object: no conflict, the later position stands.
    let mut b = base();
    let mut ids = order(&b).iter().map(|s| s.to_string()).collect::<Vec<_>>();
    apply(
        &mut b,
        vec![Op::Z {
            id: id("s-1"),
            to: Position::Front,
        }],
    )
    .expect("the first save");
    let report = apply(
        &mut b,
        vec![Op::Z {
            id: id("s-1"),
            to: Position::Back,
        }],
    )
    .expect("the later save");
    assert_eq!(
        order(&b)[0],
        "s-1",
        "the later save's position doesn't stand"
    );
    assert_eq!(report.moved.len(), 1, "the later move isn't reported");
    let mut now = order(&b).iter().map(|s| s.to_string()).collect::<Vec<_>>();
    ids.sort();
    now.sort();
    assert_eq!(ids, now, "something was lost or duplicated");
}

#[test]
fn the_same_moves_give_the_same_order() {
    let run = || {
        let mut b = base();
        apply(
            &mut b,
            vec![
                Op::Z {
                    id: id("s-2"),
                    to: Position::After(vec![id("s-1")]),
                },
                Op::Z {
                    id: id("f-1"),
                    to: Position::Front,
                },
                Op::Add {
                    object: new_sticky("n"),
                    at: Position::After(vec![id("s-2")]),
                },
            ],
        )
        .expect("applies");
        b.to_canonical_json().expect("json")
    };
    assert_eq!(run(), run());
}

#[test]
fn a_move_of_a_gone_object_conflicts() {
    let mut b = base();
    assert_eq!(
        apply(
            &mut b,
            vec![Op::Z {
                id: id("gone"),
                to: Position::Front
            }]
        )
        .expect_err("a conflict"),
        Conflict::Deleted { id: id("gone") }
    );
    assert_eq!(
        apply(
            &mut b,
            vec![Op::FrameOrder {
                id: id("gone"),
                to: Position::Front
            }]
        )
        .expect_err("a conflict"),
        Conflict::Deleted { id: id("gone") }
    );
}

#[test]
fn frame_order_moves_within_frames_only() {
    let mut b = base();
    let before = order(&b).iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let report = apply(
        &mut b,
        vec![Op::FrameOrder {
            id: id("f-2"),
            to: Position::Back,
        }],
    )
    .expect("applies");
    assert_eq!(frames(&b), vec!["f-2", "f-1"]);
    assert_eq!(
        order(&b).iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        before,
        "z-order moved"
    );
    assert_eq!(
        report.moved,
        vec![Moved {
            id: id("f-2"),
            list: OrderList::Frames,
            after: None,
            exact: true
        }]
    );
    assert_eq!(
        refused(apply(
            &mut b,
            vec![Op::FrameOrder {
                id: id("s-1"),
                to: Position::Front
            }]
        )),
        RefusalCode::FramesMismatch,
        "a sticky isn't a frame"
    );
}

#[test]
fn a_locked_object_refuses_edits() {
    let b = base();
    let attempts = vec![
        set(&b, "l-1", &[("text", text("no"))]),
        Op::Del {
            id: id("l-1"),
            rev: rev(&b, "l-1"),
        },
        Op::Z {
            id: id("l-1"),
            to: Position::Back,
        },
        set(&b, "l-1", &[("locked", Value::Bool(true))]),
        Op::Set {
            id: id("l-1"),
            rev: rev(&b, "l-1"),
            set: Fields::new(),
            unset: vec!["text".to_string()],
        },
        set(
            &b,
            "l-1",
            &[
                ("locked", Value::Bool(false)),
                ("text", text("both in one op")),
            ],
        ),
    ];
    for op in attempts {
        let mut t = b.clone();
        assert_eq!(
            refused(apply(&mut t, vec![op.clone()])),
            RefusalCode::ObjectLocked,
            "{op:?}"
        );
        assert_eq!(t, b, "{op:?} changed the board");
    }
}

#[test]
fn a_locked_frame_keeps_its_place_in_frames() {
    let mut b = base();
    let f = rev(&b, "f-1");
    apply(
        &mut b,
        vec![Op::Set {
            id: id("f-1"),
            rev: f,
            set: fields(&[("locked", Value::Bool(true))]),
            unset: Vec::new(),
        }],
    )
    .expect("locks");
    assert_eq!(
        refused(apply(
            &mut b,
            vec![Op::FrameOrder {
                id: id("f-1"),
                to: Position::Back
            }]
        )),
        RefusalCode::ObjectLocked
    );
}

#[test]
fn a_locked_object_can_be_unlocked_voted_on_and_reacted_to() {
    let b = base();
    for pairs in [
        vec![("locked", Value::Bool(false))],
        vec![("votes", parse(&format!("{{\"{P1}\":1}}")))],
        vec![("reactions", parse(&format!("{{\"x\":[\"{P1}\"]}}")))],
    ] {
        let mut t = b.clone();
        let op = set(&t, "l-1", &pairs);
        apply_by(&mut t, P1, vec![op]).expect("the lock doesn't cover these");
    }
    let mut t = b.clone();
    let op = Op::Set {
        id: id("l-1"),
        rev: rev(&t, "l-1"),
        set: Fields::new(),
        unset: vec!["locked".to_string()],
    };
    apply(&mut t, vec![op]).expect("removing the lock unlocks");
    assert!(!obj(&t, "l-1").fields.contains_key("locked"));
}

#[test]
fn locks_are_tracked_through_the_group() {
    let mut b = base();
    let r = rev(&b, "l-1");
    apply(
        &mut b,
        vec![
            Op::Set {
                id: id("l-1"),
                rev: r,
                set: fields(&[("locked", Value::Bool(false))]),
                unset: Vec::new(),
            },
            Op::Set {
                id: id("l-1"),
                rev: r,
                set: fields(&[("text", text("edited"))]),
                unset: Vec::new(),
            },
        ],
    )
    .expect("unlock, then edit, in one group");
    assert_eq!(obj(&b, "l-1").fields.get("text"), Some(&text("edited")));

    let mut b = base();
    let r = rev(&b, "s-1");
    assert_eq!(
        refused(apply(
            &mut b,
            vec![
                Op::Set {
                    id: id("s-1"),
                    rev: r,
                    set: fields(&[("locked", Value::Bool(true))]),
                    unset: Vec::new()
                },
                Op::Set {
                    id: id("s-1"),
                    rev: r,
                    set: fields(&[("text", text("no"))]),
                    unset: Vec::new()
                },
            ],
        )),
        RefusalCode::ObjectLocked,
        "lock, then edit, in one group"
    );
}

#[test]
fn a_stale_edit_of_a_locked_object_reports_the_conflict_first() {
    let mut b = base();
    let c = apply(
        &mut b,
        vec![Op::Set {
            id: id("l-1"),
            rev: stale(),
            set: fields(&[("text", text("no"))]),
            unset: Vec::new(),
        }],
    )
    .expect_err("a conflict");
    assert!(matches!(c, Conflict::Changed { .. }), "{c:?}");
}

#[test]
fn people_can_be_added_in_the_group_that_uses_them() {
    let mut b = base();
    let token = mint_person_token([1; 8]);
    let person = Person {
        name: Some("New".to_string()),
        kind: Some("agent".to_string()),
        extra: Fields::new(),
    };
    let op = set(
        &b,
        "s-1",
        &[("votes", parse(&format!("{{\"{token}\":1}}")))],
    );
    let report = apply_by(
        &mut b,
        token.as_str(),
        vec![
            Op::AddPerson {
                token: token.clone(),
                person: person.clone(),
            },
            op,
        ],
    )
    .expect("applies");
    assert_eq!(report.people_added, vec![token.clone()]);
    assert_eq!(b.people.get(&token), Some(&person));
    b.validate(&Limits::V1).expect("valid");

    let report = apply(
        &mut b,
        vec![Op::AddPerson {
            token: token.clone(),
            person: person.clone(),
        }],
    )
    .expect("the same person again");
    assert!(report.is_noop(), "{report:?}");

    let other = Person {
        name: Some("Other".to_string()),
        ..person
    };
    let c = apply(
        &mut b,
        vec![Op::AddPerson {
            token: token.clone(),
            person: other,
        }],
    )
    .expect_err("a conflict");
    assert!(
        matches!(&c, Conflict::Exists { target: Target::Person(t), .. } if *t == token),
        "{c:?}"
    );
}

#[test]
fn a_person_outside_the_rules_is_refused() {
    let mut b = base();
    let long = Person {
        name: Some("a".repeat(81)),
        ..Person::default()
    };
    assert_eq!(
        refused(apply(
            &mut b,
            vec![Op::AddPerson {
                token: id("p-7"),
                person: long
            }]
        )),
        RefusalCode::LimitField
    );
    let odd = Person {
        kind: Some("Big Kind".to_string()),
        ..Person::default()
    };
    assert_eq!(
        refused(apply(
            &mut b,
            vec![Op::AddPerson {
                token: id("p-7"),
                person: odd
            }]
        )),
        RefusalCode::FieldValue
    );
}

fn dangling_board(hash: &Sha256) -> Board {
    let text = format!(
        "{{\"frames\":[],\"later\":{{\"blob\":\"{hash}\"}},\"objects\":[\
{{\"id\":\"i-1\",\"locked\":true,\"src\":{{\"blob\":\"{hash}\"}},\"type\":\"image\"}},\
{{\"id\":\"s-1\",\"keep\":{{\"a\":[{{\"blob\":\"{hash}\"}}]}},\"type\":\"sticky\"}},\
{{\"id\":\"s-2\",\"type\":\"sticky\"}}],\
\"people\":{{\"p-1\":{{\"later\":{{\"blob\":\"{hash}\"}}}}}},\"settings\":{{\"later\":{{\"blob\":\"{hash}\"}}}}}}"
    );
    Board::from_json(text.as_bytes(), &Limits::V1).expect("board")
}

#[test]
fn clearing_a_blob_clears_every_reference_to_it() {
    let hash = Sha256::of(b"gone");
    let mut b = dangling_board(&hash);
    let untouched = rev(&b, "s-2");
    let report = apply(&mut b, vec![Op::ClearBlob { hash }]).expect("applies, locked or not");
    assert!(
        b.blob_refs().is_empty(),
        "a reference is left: {:?}",
        b.blob_refs()
    );
    assert_eq!(report.blobs_cleared, vec![hash]);
    assert_eq!(report.changed, vec![id("i-1"), id("s-1")]);
    assert!(
        !obj(&b, "i-1").fields.contains_key("src"),
        "a known field is removed"
    );
    assert_eq!(
        obj(&b, "s-1").fields.get("keep"),
        Some(&parse("{\"a\":[null]}")),
        "an unknown one keeps its shape"
    );
    assert_eq!(
        rev(&b, "s-2"),
        untouched,
        "an object with no reference changed"
    );
    assert_eq!(b.settings.get("later"), Some(&Value::Null));
    assert_eq!(b.extra.get("later"), Some(&Value::Null));
    assert_eq!(b.people[&id("p-1")].extra.get("later"), Some(&Value::Null));
    b.validate(&Limits::V1).expect("valid");
    let mut out = Vec::new();
    write(
        &mut out,
        &WriteMeta::new(Lineage::from_bytes([1; 16])),
        &b,
        &mut MemParts::new(),
        &Limits::V1,
    )
    .expect("it writes now");
}

#[test]
fn clearing_one_blob_leaves_another() {
    let (gone, kept) = (Sha256::of(b"gone"), Sha256::of(b"kept"));
    let text = format!(
        "{{\"frames\":[],\"objects\":[{{\"id\":\"a\",\"src\":{{\"blob\":\"{gone}\"}},\"type\":\"image\"}},{{\"id\":\"b\",\"src\":{{\"blob\":\"{kept}\"}},\"type\":\"image\"}}],\"people\":{{}},\"settings\":{{}}}}"
    );
    let mut b = Board::from_json(text.as_bytes(), &Limits::V1).expect("board");
    apply(&mut b, vec![Op::ClearBlob { hash: gone }]).expect("applies");
    assert_eq!(b.blob_refs().into_iter().collect::<Vec<_>>(), vec![kept]);
}

#[test]
fn clearing_a_blob_nothing_names_is_a_noop() {
    let mut b = base();
    let report = apply(&mut b, vec![Op::ClearBlob { hash: stale() }]).expect("applies");
    assert!(report.is_noop(), "{report:?}");
}

#[test]
fn id_and_type_cant_be_set() {
    let mut b = base();
    for name in ["id", "type"] {
        let op = set(&b, "s-1", &[(name, text("x"))]);
        assert_eq!(
            refused(apply(&mut b, vec![op])),
            RefusalCode::FieldValue,
            "{name}"
        );
        let op = Op::Set {
            id: id("s-1"),
            rev: rev(&b, "s-1"),
            set: Fields::new(),
            unset: vec![name.to_string()],
        };
        assert_eq!(
            refused(apply(&mut b, vec![op])),
            RefusalCode::FieldValue,
            "{name}"
        );
    }
}

#[test]
fn a_set_that_changes_nothing_is_a_noop() {
    let mut b = base();
    let before = b.to_canonical_json().expect("json");
    let op = set(&b, "s-1", &[("text", text("hello"))]);
    let report = apply(&mut b, vec![op]).expect("applies");
    assert!(report.is_noop(), "{report:?}");
    assert!(apply(&mut b, Vec::new()).expect("an empty group").is_noop());
    assert_eq!(b.to_canonical_json().expect("json"), before);
}

#[test]
fn a_group_cant_pass_the_object_limit() {
    let mut b = Board::empty();
    for i in 0..Limits::V1.objects {
        b.objects.push(new_sticky(&format!("o{i}")));
    }
    assert_eq!(
        refused(apply(
            &mut b,
            vec![Op::Add {
                object: new_sticky("one-more"),
                at: Position::Front
            }]
        )),
        RefusalCode::LimitObjects
    );
    assert_eq!(b.objects.len() as u64, Limits::V1.objects);
}

#[test]
fn an_added_object_outside_the_rules_is_refused() {
    let mut b = base();
    let mut bad = new_sticky("n");
    bad.fields.insert("text".to_string(), Value::Num(1.0));
    assert_eq!(
        refused(apply(
            &mut b,
            vec![Op::Add {
                object: bad,
                at: Position::Front
            }]
        )),
        RefusalCode::FieldKind
    );
    let mut odd = new_sticky("n");
    odd.kind = ObjectType::Unknown("Not A Name".to_string());
    assert_eq!(
        refused(apply(
            &mut b,
            vec![Op::Add {
                object: odd,
                at: Position::Front
            }]
        )),
        RefusalCode::TypeInvalid
    );
}

#[test]
fn every_success_leaves_a_valid_board() {
    let mut b = base();
    let start = b.settings_rev().expect("rev");
    let ops = vec![
        set(&b, "s-1", &[("text", text("a"))]),
        Op::Del {
            id: id("q-1"),
            rev: rev(&b, "q-1"),
        },
        Op::Add {
            object: new_sticky("n"),
            at: Position::Back,
        },
        Op::Z {
            id: id("n"),
            to: Position::Front,
        },
        Op::Meta {
            rev: start,
            set: fields(&[("title", text("x"))]),
            unset: Vec::new(),
        },
    ];
    apply(&mut b, ops).expect("applies");
    b.validate(&Limits::V1).expect("valid");
}

#[test]
fn an_object_deleted_and_added_back_has_no_old_revision() {
    let mut b = base();
    let r = rev(&b, "s-1");
    apply(
        &mut b,
        vec![
            Op::Del {
                id: id("s-1"),
                rev: r,
            },
            Op::Add {
                object: new_sticky("s-1"),
                at: Position::Back,
            },
            Op::Set {
                id: id("s-1"),
                rev: stale(),
                set: fields(&[("text", text("again"))]),
                unset: Vec::new(),
            },
        ],
    )
    .expect("the added object has nothing to compare with");
    assert_eq!(obj(&b, "s-1").fields.get("text"), Some(&text("again")));
}

#[test]
fn clearing_a_blob_reaches_what_the_group_already_touched() {
    let hash = Sha256::of(b"gone");
    let mut b = dangling_board(&hash);
    let op = set(&b, "s-1", &[("x", Value::Num(1.0))]);
    apply(&mut b, vec![op, Op::ClearBlob { hash }]).expect("applies");
    assert!(
        b.blob_refs().is_empty(),
        "a reference is left: {:?}",
        b.blob_refs()
    );
    assert_eq!(obj(&b, "s-1").fields.get("x"), Some(&Value::Num(1.0)));
}

#[test]
fn a_group_cant_pass_the_people_limit() {
    let mut b = Board::empty();
    for i in 0..Limits::V1.people {
        b.people.insert(id(&format!("p{i}")), Person::default());
    }
    let r = apply(
        &mut b,
        vec![Op::AddPerson {
            token: id("one-more"),
            person: Person::default(),
        }],
    );
    assert_eq!(refused(r), RefusalCode::LimitPeople);
}

#[test]
fn the_last_vote_taken_back_removes_the_field() {
    let mut b = base();
    let op = Op::Set {
        id: id("s-2"),
        rev: rev(&b, "s-2"),
        set: fields(&[("votes", parse(&format!("{{\"{P1}\":1}}")))]),
        unset: Vec::new(),
    };
    apply(&mut b, vec![op]).expect("one voter left");
    let op = set(&b, "s-2", &[("votes", parse("{}"))]);
    apply_by(&mut b, P1, vec![op]).expect("takes it back");
    assert!(
        !obj(&b, "s-2").fields.contains_key("votes"),
        "an empty field stayed"
    );
}

#[test]
fn a_report_says_noop_only_when_nothing_changed() {
    let start = base();
    let settings = start.settings_rev().expect("rev");
    let hash = Sha256::of(b"gone");
    let cases: Vec<(&str, Board, Vec<Op>)> = vec![
        (
            "add",
            start.clone(),
            vec![Op::Add {
                object: new_sticky("n"),
                at: Position::Front,
            }],
        ),
        (
            "set",
            start.clone(),
            vec![set(&start, "s-1", &[("text", text("x"))])],
        ),
        (
            "del",
            start.clone(),
            vec![Op::Del {
                id: id("s-1"),
                rev: rev(&start, "s-1"),
            }],
        ),
        (
            "z",
            start.clone(),
            vec![Op::Z {
                id: id("s-1"),
                to: Position::Front,
            }],
        ),
        (
            "frame order",
            start.clone(),
            vec![Op::FrameOrder {
                id: id("f-2"),
                to: Position::Back,
            }],
        ),
        (
            "meta",
            start.clone(),
            vec![Op::Meta {
                rev: settings,
                set: fields(&[("title", text("x"))]),
                unset: Vec::new(),
            }],
        ),
        (
            "person",
            start.clone(),
            vec![Op::AddPerson {
                token: id("p-9"),
                person: Person::default(),
            }],
        ),
        ("clear", dangling_board(&hash), vec![Op::ClearBlob { hash }]),
    ];
    for (name, mut b, ops) in cases {
        let before = b.to_canonical_json().expect("json");
        let report = apply(&mut b, ops).expect("applies");
        assert_ne!(
            b.to_canonical_json().expect("json"),
            before,
            "{name} changed nothing"
        );
        assert!(
            !report.is_noop(),
            "{name} changed the board, and the report says it didn't: a host would skip the save"
        );
    }
}

#[test]
fn clearing_a_reference_only_outside_the_objects_is_still_a_change() {
    let hash = Sha256::of(b"gone");
    for text in [
        format!("{{\"frames\":[],\"later\":{{\"blob\":\"{hash}\"}},\"objects\":[],\"people\":{{}},\"settings\":{{}}}}"),
        format!("{{\"frames\":[],\"objects\":[],\"people\":{{\"p-1\":{{\"later\":{{\"blob\":\"{hash}\"}}}}}},\"settings\":{{}}}}"),
    ] {
        let mut b = Board::from_json(text.as_bytes(), &Limits::V1).expect("board");
        let report = apply(&mut b, vec![Op::ClearBlob { hash }]).expect("applies");
        assert!(b.blob_refs().is_empty(), "a reference is left");
        assert!(!report.is_noop(), "the board changed, and the report says it didn't: {report:?}");
    }
}
