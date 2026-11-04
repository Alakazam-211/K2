//! The verbs that add, change and delete: each rule at its assert.

#[path = "support/verbs.rs"]
mod support;

use k2_mpc_board::json::{Fields, Value};
use k2_mpc_board::verbs::{self, End, Removed, Verb, VerbError};
use k2_mpc_board::*;
use support::*;

fn base() -> Board {
    board_of(
        "{\"id\":\"s-1\",\"later\":{\"keep\":1},\"text\":\"hello\",\"type\":\"sticky\",\"w\":100,\"x\":50,\"y\":70},\
{\"id\":\"s-2\",\"reactions\":{\"new\":[\"p-1\",\"p-2\"],\"x\":[\"p-1\"],\"z\":[\"p-2\"]},\"text\":\"two\",\"type\":\"sticky\",\"votes\":{\"p-1\":1,\"p-2\":4}},\
{\"from\":{\"id\":\"s-1\"},\"id\":\"c-1\",\"to\":{\"id\":\"s-2\"},\"type\":\"connector\"},\
{\"from\":{\"id\":\"s-2\"},\"id\":\"c-2\",\"locked\":true,\"to\":{\"x\":1,\"y\":2},\"type\":\"connector\"},\
{\"id\":\"m-1\",\"text\":\"root\",\"type\":\"mind\"},\
{\"id\":\"m-2\",\"parent\":\"m-1\",\"text\":\"kid\",\"type\":\"mind\"},\
{\"id\":\"m-3\",\"parent\":\"m-2\",\"text\":\"grandkid\",\"type\":\"mind\"},\
{\"id\":\"q-1\",\"target\":\"m-3\",\"text\":\"pin\",\"type\":\"comment\"},\
{\"id\":\"q-2\",\"replies\":[{\"at\":\"2026-11-03T10:00:00Z\",\"by\":\"p-2\",\"id\":\"r1\",\"pin\":true,\"text\":\"hi\"}],\"text\":\"free\",\"type\":\"comment\"},\
{\"id\":\"l-1\",\"locked\":true,\"text\":\"fixed\",\"type\":\"sticky\"},\
{\"checks\":[{\"done\":false,\"id\":\"c1\",\"mark\":\"keep\",\"text\":\"one\"},{\"done\":true,\"id\":\"c2\",\"text\":\"two\"}],\"id\":\"k-1\",\"type\":\"card\"},\
{\"columns\":[{\"id\":\"a\",\"tint\":\"keep\"},{\"id\":\"b\"}],\"id\":\"b-1\",\"rows\":[{\"cells\":{\"a\":\"x\",\"b\":\"y\"},\"id\":\"r1\",\"tall\":true},{\"id\":\"r2\"}],\"type\":\"table\"},\
{\"id\":\"f-1\",\"title\":\"One\",\"type\":\"frame\"},\
{\"id\":\"f-2\",\"locked\":true,\"title\":\"Two\",\"type\":\"frame\"},\
{\"frame\":\"f-1\",\"id\":\"s-3\",\"type\":\"sticky\"}",
        "{\"title\":\"T\",\"voting\":{\"active\":true,\"perPerson\":3}}",
        "[\"f-1\",\"f-2\"]",
    )
}

fn no_fields() -> Fields {
    Fields::new()
}

fn add(kind: ObjectType, f: Fields) -> Verb {
    Verb::Add {
        kind,
        fields: f,
        at: Position::Front,
    }
}

#[test]
fn add_mints_an_id_and_stamps_who_and_when_on_top() {
    let b = base();
    let v = add(ObjectType::Sticky, fields(&[("text", text("new"))]));
    let (next, planned, report) = run(&b, &v);
    let new = planned.ids[0].clone();
    assert_eq!(report.added, vec![new.clone()]);
    assert_eq!(next.objects.last().expect("an object").id, new, "on top");
    let o = obj(&next, new.as_str());
    assert_eq!(get(o, "text"), &text("new"));
    assert_eq!(
        get(o, "created"),
        &parse(&format!("{{\"at\":\"{NOW}\",\"by\":\"{ME}\"}}"))
    );
    assert!(new.as_str().starts_with("s-"), "{new}");
    assert_eq!(new.as_str().len(), 12, "{new}");
    assert_inverse(&b, &planned.group);
}

#[test]
fn add_with_no_actor_stamps_nothing() {
    let b = base();
    let v = add(ObjectType::Sticky, no_fields());
    let planned = plan_as(&b, Who::Nobody, 1, &v).expect("it plans");
    assert_eq!(planned.group.by, None);
    let Op::Add { object, .. } = &planned.group.ops[0] else {
        panic!("wanted an add, got {:?}", planned.group.ops);
    };
    assert!(!object.fields.contains_key("created"));
}

#[test]
fn an_actor_new_to_the_file_is_added_first() {
    let b = base();
    let planned = plan_as(&b, Who::New, 1, &add(ObjectType::Sticky, no_fields())).expect("plans");
    let Op::AddPerson { token, person } = &planned.group.ops[0] else {
        panic!("wanted the person first: {:?}", planned.group.ops);
    };
    assert_eq!(person.name.as_deref(), Some("Cy"));
    assert_eq!(person.kind.as_deref(), Some("agent"));
    assert_eq!(planned.group.by.as_ref(), Some(token));
    let mut next = b.clone();
    apply_group(&mut next, &planned.group).expect("it applies");
    assert!(next.people.contains_key(token));
    next.validate(&Limits::V1).expect("still valid");
}

#[test]
fn me_finds_a_person_by_name_and_kind_or_mints() {
    let b = base();
    let found = verbs::me(&b, Some("Ann"), Some("agent"), [1; 8]);
    assert_eq!(found.token, id(ME));
    assert_eq!(found.new, None);
    let other_kind = verbs::me(&b, Some("Ann"), Some("person"), [1; 8]);
    assert_eq!(other_kind.token, mint_person_token([1; 8]));
    assert_eq!(other_kind.new.expect("new").kind.as_deref(), Some("person"));
    let unnamed = verbs::me(&b, None, Some("agent"), [2; 8]);
    assert_eq!(unnamed.token, mint_person_token([2; 8]));
    let person = unnamed.new.expect("new");
    assert_eq!(person.name, None, "a stripped file doesn't get a name back");
    assert_eq!(person.kind.as_deref(), Some("agent"));
}

#[test]
fn add_refuses_an_id_or_type_field() {
    let b = base();
    for name in ["id", "type"] {
        let v = add(ObjectType::Sticky, fields(&[(name, text("x"))]));
        let Err(VerbError::Conflict(c)) = plan_as(&b, Who::Me, 1, &v) else {
            panic!("wanted a refusal for {name}");
        };
        assert_eq!(refusal_of(c), RefusalCode::FieldValue, "{name}");
    }
}

#[test]
fn a_new_table_gets_three_columns_and_rows_unless_it_brings_its_own() {
    let b = base();
    let (next, planned, _) = run(&b, &add(ObjectType::Table, no_fields()));
    let o = obj(&next, planned.ids[0].as_str());
    let Value::Arr(columns) = get(o, "columns") else {
        panic!("wanted columns");
    };
    let Value::Arr(rows) = get(o, "rows") else {
        panic!("wanted rows");
    };
    assert_eq!((columns.len(), rows.len()), (3, 3));
    let own = fields(&[("columns", parse("[{\"id\":\"only\"}]"))]);
    let (next, planned, _) = run(&b, &add(ObjectType::Table, own));
    let o = obj(&next, planned.ids[0].as_str());
    assert_eq!(get(o, "columns"), &parse("[{\"id\":\"only\"}]"));
    assert!(!o.fields.contains_key("rows"));
}

#[test]
fn an_added_frame_joins_the_presentation_order() {
    let b = base();
    let (next, planned, _) = run(&b, &add(ObjectType::Frame, no_fields()));
    assert_eq!(next.frames.last(), planned.ids.first());
    next.validate(&Limits::V1).expect("valid");
    assert_inverse(&b, &planned.group);
}

#[test]
fn a_minted_id_steps_past_one_in_use() {
    let b = board_of("{\"id\":\"s-0000000001\",\"type\":\"sticky\"}", "{}", "[]");
    let mut n = 0u8;
    let mut random = move || {
        n += 1;
        [0, 0, 0, 0, n.min(2) - 1 + 1, 0, 0, 0]
    };
    let mut cx = verbs::Ctx {
        group_id: "g".into(),
        by: None,
        at: None,
        random: &mut random,
    };
    let planned = verbs::plan(&b, &mut cx, &add(ObjectType::Sticky, no_fields())).expect("plans");
    assert_eq!(planned.ids[0].as_str(), "s-0000000002");
}

#[test]
fn random_bytes_that_keep_repeating_are_a_refusal() {
    let b = board_of("{\"id\":\"s-0000000001\",\"type\":\"sticky\"}", "{}", "[]");
    let mut random = || [0, 0, 0, 0, 1, 0, 0, 0];
    let mut cx = verbs::Ctx {
        group_id: "g".into(),
        by: None,
        at: None,
        random: &mut random,
    };
    let e = verbs::plan(&b, &mut cx, &add(ObjectType::Sticky, no_fields())).expect_err("refused");
    assert_eq!(verb_code(e), "no_free_id");
}

#[test]
fn two_new_ids_in_one_plan_never_match() {
    let b = board_of("", "{}", "[]");
    let mut random = || [0, 0, 0, 0, 1, 0, 0, 0];
    let mut cx = verbs::Ctx {
        group_id: "g".into(),
        by: None,
        at: None,
        random: &mut random,
    };
    let v = Verb::Template {
        name: "mindmap".into(),
        x: 0.0,
        y: 0.0,
    };
    let e = verbs::plan(&b, &mut cx, &v).expect_err("refused");
    assert_eq!(verb_code(e), "no_free_id");
}

#[test]
fn a_created_the_caller_brings_is_kept() {
    let b = base();
    let mine = parse("{\"at\":\"2020-01-01T00:00:00Z\",\"by\":\"p-2\"}");
    let v = add(ObjectType::Sticky, fields(&[("created", mine.clone())]));
    let (next, planned, _) = run(&b, &v);
    assert_eq!(get(obj(&next, planned.ids[0].as_str()), "created"), &mine);
}

#[test]
fn update_changes_only_the_named_fields() {
    let b = base();
    let v = Verb::Update {
        id: id("s-1"),
        rev: rev(&b, "s-1"),
        set: fields(&[("text", text("new"))]),
        unset: vec!["w".to_string()],
    };
    let (next, planned, report) = run(&b, &v);
    assert_eq!(report.changed, vec![id("s-1")]);
    let mut want = obj(&b, "s-1").clone();
    want.fields.insert("text".into(), text("new"));
    want.fields.remove("w");
    assert_eq!(obj(&next, "s-1"), &want, "unknown fields and the rest stay");
    assert_inverse(&b, &planned.group);
}

#[test]
fn update_with_a_stale_revision_conflicts_and_changes_nothing() {
    let b = base();
    let v = Verb::Update {
        id: id("s-1"),
        rev: Sha256::of(b"old"),
        set: fields(&[("text", text("new"))]),
        unset: Vec::new(),
    };
    let planned = plan_as(&b, Who::Me, 1, &v).expect("plans");
    let mut next = b.clone();
    let e = apply_group(&mut next, &planned.group).expect_err("stale");
    assert!(matches!(e, Conflict::Changed { .. }), "{e:?}");
    assert_eq!(next, b);
}

fn delete(b: &Board, i: &str, cascade: bool) -> Verb {
    Verb::Delete {
        id: id(i),
        rev: rev(b, i),
        cascade,
    }
}

#[test]
fn a_cascade_takes_what_hangs_off_the_object() {
    let b = base();
    let (next, planned, report) = run(&b, &delete(&b, "m-1", true));
    for gone in ["m-1", "m-2", "m-3", "q-1"] {
        assert!(!has(&next, gone), "{gone} stayed");
    }
    let mut deleted: Vec<&str> = report.deleted.iter().map(Id::as_str).collect();
    deleted.sort_unstable();
    assert_eq!(deleted, ["m-1", "m-2", "m-3", "q-1"]);
    assert!(has(&next, "q-2"), "a comment on nothing stays");
    let removed: Vec<&str> = planned
        .removed
        .iter()
        .map(|r| r.object.id.as_str())
        .collect();
    assert_eq!(removed, ["m-1", "m-2", "m-3", "q-1"], "back to front");
    assert!(planned.orphans.is_empty(), "{:?}", planned.orphans);
    assert_inverse(&b, &planned.group);
}

#[test]
fn a_cascade_takes_connectors_attached_at_either_end() {
    let b = base();
    let (next, planned, _) = run(&b, &delete(&b, "s-1", true));
    assert!(!has(&next, "c-1"), "attached by its from");
    assert!(has(&next, "c-2"), "not attached to s-1");
    assert_inverse(&b, &planned.group);
    let (next, _, _) = run(&b, &delete(&b, "s-2", true));
    assert!(!has(&next, "c-1"), "attached by its to");
}

#[test]
fn a_locked_dependent_goes_and_the_report_names_it() {
    let b = base();
    assert_eq!(
        applies(&b, &delete(&b, "s-2", true)),
        Ok(()),
        "a locked dependent is unlocked first, so the cascade applies"
    );
    let (next, planned, report) = run(&b, &delete(&b, "s-2", true));
    assert!(!has(&next, "c-2"), "the locked connector stayed");
    assert_eq!(
        planned.removed_locked,
        vec![id("c-2")],
        "a locked object was deleted without being named"
    );
    assert!(report.deleted.contains(&id("c-2")));
}

#[test]
fn an_unlocked_dependent_is_not_named_as_locked() {
    let b = base();
    let (_, planned, _) = run(&b, &delete(&b, "s-1", true));
    assert!(planned.removed.iter().any(|r| r.object.id == id("c-1")));
    assert!(
        planned.removed_locked.is_empty(),
        "{:?}",
        planned.removed_locked
    );
}

#[test]
fn the_inverse_brings_a_locked_dependent_back_locked() {
    let b = base();
    let (next, planned, _) = run(&b, &delete(&b, "s-2", true));
    let back = verbs::inverse(&b, &next, "undo").expect("an inverse");
    let mut undone = next.clone();
    apply_group(&mut undone, &back).expect("it applies");
    assert!(has(&undone, "c-2"), "it didn't come back");
    assert_eq!(
        obj(&undone, "c-2").fields.get("locked"),
        Some(&Value::Bool(true)),
        "it came back unlocked"
    );
    assert_eq!(obj(&undone, "c-2"), obj(&b, "c-2"));
    assert_inverse(&b, &planned.group);
}

#[test]
fn put_back_restores_a_locked_dependent_locked_and_in_place() {
    let b = base();
    let (mut next, planned, _) = run(&b, &delete(&b, "s-2", true));
    for removed in &planned.removed {
        let v = Verb::PutBack {
            removed: removed.clone(),
        };
        let p = plan_as(&next, Who::Me, 5, &v).expect("plans");
        apply_group(&mut next, &p.group).expect("it applies");
    }
    assert_eq!(
        obj(&next, "c-2").fields.get("locked"),
        Some(&Value::Bool(true))
    );
    assert_eq!(order(&next), order(&b));
}

#[test]
fn deleting_a_locked_object_by_name_refuses() {
    let b = base();
    let planned = plan_as(&b, Who::Me, 1, &delete(&b, "l-1", true)).expect("plans");
    let mut next = b.clone();
    let e = apply_group(&mut next, &planned.group).expect_err("locked");
    assert_eq!(refusal_of(e), RefusalCode::ObjectLocked);
    assert_eq!(next, b);
}

#[test]
fn deleting_with_no_cascade_leaves_orphans_and_reports_them() {
    let b = base();
    let (next, planned, _) = run(&b, &delete(&b, "m-2", false));
    assert!(has(&next, "m-3"), "not cascaded");
    assert_eq!(planned.orphans.len(), 1, "{:?}", planned.orphans);
    assert_eq!(planned.orphans[0].from, id("m-3"));
    assert_eq!(planned.orphans[0].target, id("m-2"));
    assert_eq!(planned.orphans[0].state, RefState::Orphaned);
    assert_eq!(planned.removed.len(), 1);
    assert!(planned.removed_locked.is_empty());
    assert_inverse(&b, &planned.group);
}

#[test]
fn frame_members_stay_as_orphans_when_a_frame_goes() {
    let b = base();
    let (next, planned, _) = run(&b, &delete(&b, "f-1", true));
    assert!(has(&next, "s-3"), "a member isn't deleted with its frame");
    assert_eq!(planned.orphans.len(), 1, "{:?}", planned.orphans);
    assert_eq!(planned.orphans[0].field, "frame");
    next.validate(&Limits::V1).expect("valid");
    assert_inverse(&b, &planned.group);
}

/// A raw `del`, as a canvas sends it.
fn raw_del(i: &str) -> Group {
    Group {
        id: "g".into(),
        by: None,
        ops: vec![Op::Del {
            id: id(i),
            rev: Sha256::of(b"x"),
        }],
    }
}

#[test]
fn a_friendly_delete_of_what_is_gone_refuses() {
    let b = base();
    let v = Verb::Delete {
        id: id("nope"),
        rev: Sha256::of(b"x"),
        cascade: true,
    };
    let e = plan_as(&b, Who::Me, 1, &v).expect_err("deleting what isn't there refuses");
    assert_eq!(
        e,
        VerbError::Conflict(Conflict::Deleted { id: id("nope") }),
        "by type, naming what's gone"
    );
}

#[test]
fn a_raw_del_of_what_is_gone_is_a_noop_the_report_names() {
    let b = base();
    let mut next = b.clone();
    let report =
        apply_group(&mut next, &raw_del("nope")).expect("a raw del of what's gone applies");
    assert_eq!(report.already_gone, vec![id("nope")]);
    assert_eq!(next, b);
}

#[test]
fn a_raw_del_of_what_is_gone_leaves_what_named_it() {
    let b = board_of(
        "{\"from\":{\"id\":\"nope\"},\"id\":\"c-1\",\"to\":{\"x\":1,\"y\":1},\"type\":\"connector\"}",
        "{}",
        "[]",
    );
    let mut next = b.clone();
    apply_group(&mut next, &raw_del("nope")).expect("it applies");
    assert!(has(&next, "c-1"), "an orphan stays");
}

#[test]
fn a_stale_revision_on_the_named_object_stops_the_whole_delete() {
    let b = base();
    let v = Verb::Delete {
        id: id("m-1"),
        rev: Sha256::of(b"old"),
        cascade: true,
    };
    let planned = plan_as(&b, Who::Me, 1, &v).expect("plans");
    let mut next = b.clone();
    let e = apply_group(&mut next, &planned.group).expect_err("stale");
    assert!(matches!(e, Conflict::Changed { .. }), "{e:?}");
    assert_eq!(next, b);
}

#[test]
fn a_put_back_frame_returns_to_its_place_locked() {
    let b = base();
    let (mut next, planned, _) = run(
        &b,
        &Verb::Update {
            id: id("f-2"),
            rev: rev(&b, "f-2"),
            set: fields(&[("locked", Value::Bool(false))]),
            unset: Vec::new(),
        },
    );
    let _ = planned;
    let v = delete(&next, "f-1", false);
    let p = plan_as(&next, Who::Me, 9, &v).expect("plans");
    let removed: Removed = p.removed[0].clone();
    apply_group(&mut next, &p.group).expect("it applies");
    assert_eq!(next.frames, vec![id("f-2")]);

    let mut locked = removed.clone();
    locked
        .object
        .fields
        .insert("locked".into(), Value::Bool(true));
    let p = plan_as(&next, Who::Me, 9, &Verb::PutBack { removed: locked }).expect("plans");
    apply_group(&mut next, &p.group).expect("check: a locked frame goes back");
    assert_eq!(next.frames, vec![id("f-1"), id("f-2")], "first again");
    assert_eq!(
        obj(&next, "f-1").fields.get("locked"),
        Some(&Value::Bool(true))
    );
}

#[test]
fn a_put_back_frame_returns_between_its_neighbours() {
    let b = board_of(
        "{\"id\":\"f-1\",\"type\":\"frame\"},{\"id\":\"f-2\",\"type\":\"frame\"},{\"id\":\"f-3\",\"type\":\"frame\"}",
        "{}",
        "[\"f-3\",\"f-2\",\"f-1\"]",
    );
    let (mut next, planned, _) = run(&b, &delete(&b, "f-2", false));
    assert_eq!(next.frames, vec![id("f-3"), id("f-1")]);
    assert_eq!(planned.removed[0].frame_after, vec![id("f-3")]);
    let v = Verb::PutBack {
        removed: planned.removed[0].clone(),
    };
    let p = plan_as(&next, Who::Me, 5, &v).expect("plans");
    apply_group(&mut next, &p.group).expect("it applies");
    assert_eq!(next.frames, b.frames, "between its neighbours");
    assert_eq!(order(&next), order(&b), "and in its place in z-order");
}

#[test]
fn connect_attaches_ends_that_are_there() {
    let b = base();
    let v = Verb::Connect {
        from: End::Object {
            id: id("s-1"),
            anchor: None,
        },
        to: End::Object {
            id: id("s-2"),
            anchor: Some("top".into()),
        },
        fields: fields(&[("label", text("via"))]),
    };
    assert_eq!(
        applies(&b, &v),
        Ok(()),
        "an end with no anchor gets auto, so the connector applies"
    );
    let (next, planned, _) = run(&b, &v);
    let o = obj(&next, planned.ids[0].as_str());
    assert_eq!(o.kind, ObjectType::Connector);
    assert_eq!(
        get(o, "from"),
        &parse("{\"anchor\":\"auto\",\"id\":\"s-1\"}")
    );
    assert_eq!(get(o, "to"), &parse("{\"anchor\":\"top\",\"id\":\"s-2\"}"));
    assert_eq!(get(o, "label"), &text("via"));
    assert_inverse(&b, &planned.group);
}

#[test]
fn connect_takes_a_free_point() {
    let b = base();
    let v = Verb::Connect {
        from: End::Point { x: 1.5, y: 2.0 },
        to: End::Object {
            id: id("s-2"),
            anchor: None,
        },
        fields: no_fields(),
    };
    let (next, planned, _) = run(&b, &v);
    let o = obj(&next, planned.ids[0].as_str());
    assert_eq!(get(o, "from"), &parse("{\"x\":1.5,\"y\":2}"));
}

#[test]
fn connect_to_an_object_that_is_gone_conflicts() {
    let b = base();
    let v = Verb::Connect {
        from: End::Object {
            id: id("s-1"),
            anchor: None,
        },
        to: End::Object {
            id: id("nope"),
            anchor: None,
        },
        fields: no_fields(),
    };
    let e = plan_as(&b, Who::Me, 1, &v).expect_err("refused");
    assert_eq!(e, VerbError::Conflict(Conflict::Deleted { id: id("nope") }));
}

#[test]
fn connect_refuses_ends_in_its_fields() {
    let b = base();
    for name in ["from", "to"] {
        let v = Verb::Connect {
            from: End::Point { x: 0.0, y: 0.0 },
            to: End::Point { x: 1.0, y: 1.0 },
            fields: fields(&[(name, parse("{\"x\":1,\"y\":2}"))]),
        };
        let Err(VerbError::Conflict(c)) = plan_as(&b, Who::Me, 1, &v) else {
            panic!("wanted a refusal for {name}");
        };
        assert_eq!(refusal_of(c), RefusalCode::FieldValue);
    }
}

#[test]
fn a_friendly_connect_refuses_an_unknown_field_and_raw_ops_keep_it() {
    let b = base();
    let connect = |f: Fields| Verb::Connect {
        from: End::Point { x: 0.0, y: 0.0 },
        to: End::Point { x: 1.0, y: 1.0 },
        fields: f,
    };
    for (name, why) in [
        ("colr", "a field no type has"),
        ("rows", "another type's field"),
    ] {
        let v = connect(fields(&[(name, text("x"))]));
        let code = plan_as(&b, Who::Me, 1, &v).map(|_| ()).map_err(verb_code);
        assert_eq!(code, Err("unknown_field"), "{why} refuses: {name}");
    }
    run(&b, &connect(fields(&[("label", text("via"))])));
    let raw = "{\"ops\":[{\"op\":\"add\",\"obj\":{\"colr\":\"red\",\"from\":{\"x\":0,\"y\":0},\
\"id\":\"n\",\"to\":{\"x\":1,\"y\":1},\"type\":\"connector\"}}]}";
    let planned = labelled(&b, raw).expect("raw ops plan");
    let mut kept = b.clone();
    apply_group(&mut kept, &planned.group).expect("raw ops keep what they don't know");
    assert_eq!(
        get(obj(&kept, planned.ids[0].as_str()), "colr"),
        &text("red")
    );
}

#[test]
fn a_reaction_changes_only_the_callers_entries() {
    let b = base();
    let on = Verb::React {
        id: id("s-2"),
        emoji: "fresh".into(),
        on: Some(true),
    };
    let (next, planned, _) = run(&b, &on);
    assert_eq!(
        get(obj(&next, "s-2"), "reactions"),
        &parse("{\"fresh\":[\"p-1\"],\"new\":[\"p-1\",\"p-2\"],\"x\":[\"p-1\"],\"z\":[\"p-2\"]}")
    );
    assert_inverse(&b, &planned.group);
    let off = Verb::React {
        id: id("s-2"),
        emoji: "new".into(),
        on: Some(false),
    };
    let (next, _, _) = run(&b, &off);
    assert_eq!(
        get(obj(&next, "s-2"), "reactions"),
        &parse("{\"new\":[\"p-2\"],\"x\":[\"p-1\"],\"z\":[\"p-2\"]}"),
        "my others stay, and everyone else's"
    );
}

#[test]
fn a_reaction_passes_a_lock_and_needs_an_actor() {
    let b = base();
    let v = Verb::React {
        id: id("l-1"),
        emoji: "y".into(),
        on: Some(true),
    };
    let (next, _, _) = run(&b, &v);
    assert_eq!(
        get(obj(&next, "l-1"), "reactions"),
        &parse("{\"y\":[\"p-1\"]}")
    );
    let e = plan_as(&b, Who::Nobody, 1, &v).expect_err("refused");
    assert_eq!(verb_code(e), "needs_actor");
}

#[test]
fn a_reaction_on_an_object_that_is_gone_conflicts() {
    let b = base();
    let v = Verb::React {
        id: id("nope"),
        emoji: "y".into(),
        on: Some(true),
    };
    let e = plan_as(&b, Who::Me, 1, &v).expect_err("refused");
    assert_eq!(e, VerbError::Conflict(Conflict::Deleted { id: id("nope") }));
}

fn vote(i: &str, delta: i32) -> Verb {
    Verb::Vote { id: id(i), delta }
}

#[test]
fn a_vote_adds_to_the_callers_count_only() {
    let b = base();
    let (next, planned, _) = run(&b, &vote("s-2", 1));
    assert_eq!(
        get(obj(&next, "s-2"), "votes"),
        &parse("{\"p-1\":2,\"p-2\":4}")
    );
    assert_inverse(&b, &planned.group);
}

#[test]
fn votes_stop_at_the_settings_limit_across_the_board() {
    let b = base();
    assert_eq!(
        applies(&b, &vote("s-1", 2)),
        Ok(()),
        "votes up to the limit are allowed"
    );
    let (next, _, _) = run(&b, &vote("s-1", 2));
    assert_eq!(get(obj(&next, "s-1"), "votes"), &parse("{\"p-1\":2}"));
    let e = plan_as(&next, Who::Me, 1, &vote("s-1", 1)).expect_err("used up");
    assert_eq!(verb_code(e), "no_votes_left");
}

#[test]
fn taking_back_a_vote_removes_the_entry_at_zero() {
    let b = base();
    assert_eq!(
        applies(&b, &vote("s-2", -1)),
        Ok(()),
        "a vote taken back to zero removes the entry, so the board stays valid"
    );
    let (next, _, _) = run(&b, &vote("s-2", -1));
    assert_eq!(get(obj(&next, "s-2"), "votes"), &parse("{\"p-2\":4}"));
    let (next, _, report) = run(&b, &vote("s-1", -1));
    assert_eq!(obj(&next, "s-1"), obj(&b, "s-1"));
    assert!(report.is_noop(), "{report:?}");
}

#[test]
fn taking_back_ignores_the_limit() {
    let b = board_of(
        "{\"id\":\"s-1\",\"type\":\"sticky\",\"votes\":{\"p-1\":9}}",
        "{\"voting\":{\"active\":true,\"perPerson\":1}}",
        "[]",
    );
    assert_eq!(
        applies(&b, &vote("s-1", -1)),
        Ok(()),
        "taking back ignores the limit"
    );
    let (next, _, _) = run(&b, &vote("s-1", -1));
    assert_eq!(get(obj(&next, "s-1"), "votes"), &parse("{\"p-1\":8}"));
}

#[test]
fn voting_has_to_be_on() {
    for settings in ["{}", "{\"voting\":{\"active\":false}}"] {
        let b = board_of("{\"id\":\"s-1\",\"type\":\"sticky\"}", settings, "[]");
        let e = plan_as(&b, Who::Me, 1, &vote("s-1", 1)).expect_err("off");
        assert_eq!(verb_code(e), "voting_off", "{settings}");
    }
}

#[test]
fn voting_with_no_limit_has_no_limit_and_a_zero_vote_is_refused() {
    let b = board_of(
        "{\"id\":\"s-1\",\"type\":\"sticky\"}",
        "{\"voting\":{\"active\":true}}",
        "[]",
    );
    let (next, _, _) = run(&b, &vote("s-1", 50));
    assert_eq!(get(obj(&next, "s-1"), "votes"), &parse("{\"p-1\":50}"));
    let e = plan_as(&b, Who::Me, 1, &vote("s-1", 0)).expect_err("zero");
    assert_eq!(verb_code(e), "bad_value");
    let e = plan_as(&b, Who::Nobody, 1, &vote("s-1", 1)).expect_err("nobody");
    assert_eq!(verb_code(e), "needs_actor");
}

#[test]
fn a_pinned_comment_sits_by_its_target() {
    let b = base();
    let v = Verb::Comment {
        target: Some(id("s-1")),
        text: "look".into(),
        point: None,
    };
    let (next, planned, _) = run(&b, &v);
    let o = obj(&next, planned.ids[0].as_str());
    assert_eq!(get(o, "target"), &text("s-1"));
    assert_eq!(num(o, "x"), 138.0, "x + w - 12");
    assert_eq!(num(o, "y"), 50.0, "y - 20");
    assert!(o.fields.contains_key("created"));
    assert_inverse(&b, &planned.group);
}

#[test]
fn a_comment_can_sit_at_a_point_and_needs_text_and_a_target_that_is_there() {
    let b = base();
    let v = Verb::Comment {
        target: None,
        text: "free".into(),
        point: Some((3.0, 4.0)),
    };
    let (next, planned, _) = run(&b, &v);
    let o = obj(&next, planned.ids[0].as_str());
    assert_eq!((num(o, "x"), num(o, "y")), (3.0, 4.0));
    assert!(!o.fields.contains_key("target"));
    let empty = Verb::Comment {
        target: None,
        text: String::new(),
        point: None,
    };
    assert_eq!(
        verb_code(plan_as(&b, Who::Me, 1, &empty).expect_err("empty")),
        "bad_value"
    );
    let gone = Verb::Comment {
        target: Some(id("nope")),
        text: "x".into(),
        point: Some((1.0, 1.0)),
    };
    assert_eq!(
        plan_as(&b, Who::Me, 1, &gone).expect_err("gone"),
        VerbError::Conflict(Conflict::Deleted { id: id("nope") })
    );
}

#[test]
fn a_reply_adds_to_the_list_and_keeps_what_is_there() {
    let b = base();
    let v = Verb::Reply {
        id: id("q-2"),
        text: "me too".into(),
    };
    let (next, planned, _) = run(&b, &v);
    let Value::Arr(replies) = get(obj(&next, "q-2"), "replies") else {
        panic!("wanted replies");
    };
    assert_eq!(replies.len(), 2);
    let Value::Obj(old) = &replies[0] else {
        panic!("an object");
    };
    assert_eq!(old["pin"], Value::Bool(true), "an unknown sub-field stays");
    let Value::Obj(new) = &replies[1] else {
        panic!("an object");
    };
    assert_eq!(new["by"], text(ME));
    assert_eq!(new["at"], text(NOW));
    assert_eq!(new["text"], text("me too"));
    assert_inverse(&b, &planned.group);
}

#[test]
fn a_reply_needs_a_comment_an_actor_and_a_time() {
    let b = base();
    let v = Verb::Reply {
        id: id("s-1"),
        text: "x".into(),
    };
    assert_eq!(
        verb_code(plan_as(&b, Who::Me, 1, &v).expect_err("type")),
        "wrong_type"
    );
    let v = Verb::Reply {
        id: id("q-2"),
        text: "x".into(),
    };
    assert_eq!(
        verb_code(plan_as(&b, Who::Nobody, 1, &v).expect_err("who")),
        "needs_actor"
    );
    let mut random = || [1; 8];
    let mut cx = verbs::Ctx {
        group_id: "g".into(),
        by: Some(verbs::me(&b, Some("Ann"), Some("agent"), [1; 8])),
        at: None,
        random: &mut random,
    };
    assert_eq!(
        verb_code(verbs::plan(&b, &mut cx, &v).expect_err("when")),
        "needs_actor"
    );
}

#[test]
fn resolve_names_a_revision_and_only_fits_a_comment() {
    let b = base();
    let v = Verb::Resolve {
        id: id("q-2"),
        rev: rev(&b, "q-2"),
        resolved: true,
    };
    let (next, planned, _) = run(&b, &v);
    assert_eq!(get(obj(&next, "q-2"), "resolved"), &Value::Bool(true));
    assert_inverse(&b, &planned.group);
    let stale = Verb::Resolve {
        id: id("q-2"),
        rev: Sha256::of(b"old"),
        resolved: true,
    };
    let p = plan_as(&b, Who::Me, 1, &stale).expect("plans");
    let e = apply_group(&mut b.clone(), &p.group).expect_err("stale");
    assert!(matches!(e, Conflict::Changed { .. }), "{e:?}");
    let v = Verb::Resolve {
        id: id("s-1"),
        rev: rev(&b, "s-1"),
        resolved: true,
    };
    assert_eq!(
        verb_code(plan_as(&b, Who::Me, 1, &v).expect_err("type")),
        "wrong_type"
    );
}

#[test]
fn meta_names_the_settings_revision() {
    let b = base();
    let v = Verb::Meta {
        rev: b.settings_rev().expect("a rev"),
        set: fields(&[("title", text("New"))]),
        unset: vec!["voting".to_string()],
    };
    let (next, planned, report) = run(&b, &v);
    assert!(report.settings_changed);
    assert_eq!(next.settings["title"], text("New"));
    assert!(!next.settings.contains_key("voting"));
    assert_inverse(&b, &planned.group);
    let stale = Verb::Meta {
        rev: Sha256::of(b"old"),
        set: Fields::new(),
        unset: Vec::new(),
    };
    let p = plan_as(&b, Who::Me, 1, &stale).expect("plans");
    let e = apply_group(&mut b.clone(), &p.group).expect_err("stale");
    assert!(
        matches!(
            e,
            Conflict::Changed {
                target: Target::Settings,
                ..
            }
        ),
        "{e:?}"
    );
}

#[test]
fn the_inverse_takes_off_a_setting_the_group_added() {
    let b = base();
    let v = Verb::Meta {
        rev: b.settings_rev().expect("a rev"),
        set: fields(&[("soon", text("new"))]),
        unset: Vec::new(),
    };
    let (_, planned, _) = run(&b, &v);
    assert_inverse(&b, &planned.group);
}

#[test]
fn lock_sets_and_clears_and_leaves_one_already_that_way() {
    let b = base();
    let v = Verb::Lock {
        ids: vec![named(&b, "s-1"), named(&b, "l-1")],
        locked: true,
    };
    assert_eq!(
        applies(&b, &v),
        Ok(()),
        "one already locked is left alone, so the lock applies"
    );
    let (next, planned, _) = run(&b, &v);
    assert_eq!(get(obj(&next, "s-1"), "locked"), &Value::Bool(true));
    assert_eq!(planned.skipped, vec![id("l-1")], "already locked");
    assert_inverse(&b, &planned.group);
    let v = Verb::Lock {
        ids: vec![named(&b, "l-1"), named(&b, "s-1")],
        locked: false,
    };
    assert_eq!(
        applies(&b, &v),
        Ok(()),
        "one already unlocked is left alone, so the unlock applies"
    );
    let (next, planned, _) = run(&b, &v);
    assert_eq!(get(obj(&next, "l-1"), "locked"), &Value::Bool(false));
    assert_eq!(planned.skipped, vec![id("s-1")], "already unlocked");
}

#[test]
fn check_items_add_set_and_remove_and_keep_unknown_sub_fields() {
    let b = base();
    let (next, planned, _) = run(
        &b,
        &Verb::CheckAdd {
            id: id("k-1"),
            text: "three".into(),
        },
    );
    assert_inverse(&b, &planned.group);
    let Value::Arr(list) = get(obj(&next, "k-1"), "checks") else {
        panic!("wanted checks");
    };
    assert_eq!(list.len(), 3);
    let Value::Obj(new) = &list[2] else {
        panic!("an object");
    };
    assert_eq!(new["text"], text("three"));
    assert_eq!(new["done"], Value::Bool(false));

    let v = Verb::CheckSet {
        id: id("k-1"),
        rev: rev(&b, "k-1"),
        item: "c1".into(),
        text: Some("uno".into()),
        done: Some(true),
    };
    let (next, planned, _) = run(&b, &v);
    assert_inverse(&b, &planned.group);
    assert_eq!(
        get(obj(&next, "k-1"), "checks"),
        &parse("[{\"done\":true,\"id\":\"c1\",\"mark\":\"keep\",\"text\":\"uno\"},{\"done\":true,\"id\":\"c2\",\"text\":\"two\"}]")
    );

    let v = Verb::CheckSet {
        id: id("k-1"),
        rev: rev(&b, "k-1"),
        item: "c1".into(),
        text: None,
        done: Some(true),
    };
    let (next, _, _) = run(&b, &v);
    assert_eq!(
        get(obj(&next, "k-1"), "checks"),
        &parse("[{\"done\":true,\"id\":\"c1\",\"mark\":\"keep\",\"text\":\"one\"},{\"done\":true,\"id\":\"c2\",\"text\":\"two\"}]"),
        "the text stays when only done is given"
    );

    let v = Verb::CheckRemove {
        id: id("k-1"),
        rev: rev(&b, "k-1"),
        item: "c1".into(),
    };
    let (next, planned, _) = run(&b, &v);
    assert_inverse(&b, &planned.group);
    assert_eq!(
        get(obj(&next, "k-1"), "checks"),
        &parse("[{\"done\":true,\"id\":\"c2\",\"text\":\"two\"}]")
    );
}

#[test]
fn check_items_refuse_a_missing_item_and_a_type_without_them() {
    let b = base();
    let v = Verb::CheckSet {
        id: id("k-1"),
        rev: rev(&b, "k-1"),
        item: "nope".into(),
        text: None,
        done: None,
    };
    assert_eq!(
        verb_code(plan_as(&b, Who::Me, 1, &v).expect_err("item")),
        "no_such_item"
    );
    let v = Verb::CheckRemove {
        id: id("k-1"),
        rev: rev(&b, "k-1"),
        item: "nope".into(),
    };
    assert_eq!(
        verb_code(plan_as(&b, Who::Me, 1, &v).expect_err("item")),
        "no_such_item"
    );
    let v = Verb::CheckAdd {
        id: id("s-1"),
        text: "x".into(),
    };
    assert_eq!(
        verb_code(plan_as(&b, Who::Me, 1, &v).expect_err("type")),
        "wrong_type"
    );
}

#[test]
fn a_cell_changes_only_itself() {
    let b = base();
    let v = Verb::TableCell {
        id: id("b-1"),
        rev: rev(&b, "b-1"),
        row: "r1".into(),
        column: "b".into(),
        text: "new".into(),
    };
    let (next, planned, _) = run(&b, &v);
    assert_inverse(&b, &planned.group);
    assert_eq!(
        get(obj(&next, "b-1"), "rows"),
        &parse(
            "[{\"cells\":{\"a\":\"x\",\"b\":\"new\"},\"id\":\"r1\",\"tall\":true},{\"id\":\"r2\"}]"
        )
    );
    let v = Verb::TableCell {
        id: id("b-1"),
        rev: rev(&b, "b-1"),
        row: "r2".into(),
        column: "a".into(),
        text: "first".into(),
    };
    let (next, _, _) = run(&b, &v);
    assert_eq!(
        get(obj(&next, "b-1"), "rows"),
        &parse("[{\"cells\":{\"a\":\"x\",\"b\":\"y\"},\"id\":\"r1\",\"tall\":true},{\"cells\":{\"a\":\"first\"},\"id\":\"r2\"}]"),
        "a row with no cells gets its first"
    );
}

#[test]
fn a_cell_needs_its_row_and_column_and_a_table() {
    let b = base();
    let cell = |row: &str, column: &str, on: &str| Verb::TableCell {
        id: id(on),
        rev: rev(&b, on),
        row: row.into(),
        column: column.into(),
        text: "x".into(),
    };
    assert_eq!(
        verb_code(plan_as(&b, Who::Me, 1, &cell("nope", "a", "b-1")).expect_err("row")),
        "no_such_item"
    );
    assert_eq!(
        verb_code(plan_as(&b, Who::Me, 1, &cell("r1", "nope", "b-1")).expect_err("col")),
        "no_such_item"
    );
    assert_eq!(
        verb_code(plan_as(&b, Who::Me, 1, &cell("r1", "a", "s-1")).expect_err("type")),
        "wrong_type"
    );
}

fn ids_of(list: &Value) -> Vec<String> {
    let Value::Arr(items) = list else {
        panic!("wanted a list");
    };
    items
        .iter()
        .map(|v| match v {
            Value::Obj(f) => match &f["id"] {
                Value::Str(s) => s.clone(),
                other => panic!("an id: {other:?}"),
            },
            other => panic!("an object: {other:?}"),
        })
        .collect()
}

#[test]
fn rows_go_where_they_are_asked_and_a_removed_one_takes_its_cells() {
    let b = base();
    let v = Verb::TableAddRow {
        id: id("b-1"),
        after: Some("r1".into()),
    };
    let (next, planned, _) = run(&b, &v);
    assert_inverse(&b, &planned.group);
    let rows = ids_of(get(obj(&next, "b-1"), "rows"));
    assert_eq!(rows.len(), 3);
    assert_eq!(
        (rows[0].as_str(), rows[2].as_str()),
        ("r1", "r2"),
        "{rows:?}"
    );
    let v = Verb::TableAddRow {
        id: id("b-1"),
        after: None,
    };
    let (next, _, _) = run(&b, &v);
    let rows = ids_of(get(obj(&next, "b-1"), "rows"));
    assert_eq!(rows[1], "r1", "first with none named: {rows:?}");
    let v = Verb::TableRemoveRow {
        id: id("b-1"),
        rev: rev(&b, "b-1"),
        row: "r1".into(),
    };
    let (next, planned, _) = run(&b, &v);
    assert_inverse(&b, &planned.group);
    assert_eq!(get(obj(&next, "b-1"), "rows"), &parse("[{\"id\":\"r2\"}]"));
    let v = Verb::TableAddRow {
        id: id("b-1"),
        after: Some("nope".into()),
    };
    assert_eq!(
        verb_code(plan_as(&b, Who::Me, 1, &v).expect_err("row")),
        "no_such_item"
    );
}

#[test]
fn columns_go_where_they_are_asked_and_a_removed_one_takes_its_cells() {
    let b = base();
    let v = Verb::TableAddColumn {
        id: id("b-1"),
        after: Some("a".into()),
        width: Some(40.0),
    };
    let (next, planned, _) = run(&b, &v);
    assert_inverse(&b, &planned.group);
    let columns = ids_of(get(obj(&next, "b-1"), "columns"));
    assert_eq!(
        (columns[0].as_str(), columns[2].as_str()),
        ("a", "b"),
        "{columns:?}"
    );
    let Value::Arr(list) = get(obj(&next, "b-1"), "columns") else {
        panic!("a list");
    };
    let Value::Obj(new) = &list[1] else {
        panic!("an object");
    };
    assert_eq!(new["width"], Value::Num(40.0));
    let v = Verb::TableRemoveColumn {
        id: id("b-1"),
        rev: rev(&b, "b-1"),
        column: "a".into(),
    };
    assert_eq!(
        applies(&b, &v),
        Ok(()),
        "a removed column takes its cells, so the board stays valid"
    );
    let (next, planned, _) = run(&b, &v);
    assert_inverse(&b, &planned.group);
    assert_eq!(
        get(obj(&next, "b-1"), "columns"),
        &parse("[{\"id\":\"b\"}]")
    );
    assert_eq!(
        get(obj(&next, "b-1"), "rows"),
        &parse("[{\"cells\":{\"b\":\"y\"},\"id\":\"r1\",\"tall\":true},{\"id\":\"r2\"}]")
    );
    next.validate(&Limits::V1)
        .expect("no cell names a gone column");
    let v = Verb::TableRemoveColumn {
        id: id("b-1"),
        rev: rev(&b, "b-1"),
        column: "nope".into(),
    };
    assert_eq!(
        verb_code(plan_as(&b, Who::Me, 1, &v).expect_err("col")),
        "no_such_item"
    );
}

#[test]
fn clear_blob_clears_a_reference() {
    let h = Sha256::of(b"pic");
    let b = board_of(
        &format!("{{\"id\":\"i-1\",\"src\":{{\"blob\":\"{h}\"}},\"type\":\"image\"}}"),
        "{}",
        "[]",
    );
    let (next, planned, report) = run(&b, &Verb::ClearBlob { hash: h });
    assert_eq!(report.blobs_cleared, vec![h]);
    assert!(!obj(&next, "i-1").fields.contains_key("src"));
    assert_inverse(&b, &planned.group);
}

const REV: &str = "sha256-0000000000000000000000000000000000000000000000000000000000000000";

#[test]
fn raw_json_reads_every_op() {
    let text = format!(
        "{{\"id\":\"g9\",\"by\":\"p-1\",\"ops\":[\
{{\"op\":\"add\",\"obj\":{{\"id\":\"s-9\",\"type\":\"sticky\",\"text\":\"t\"}},\"at\":\"back\"}},\
{{\"op\":\"add\",\"obj\":{{\"id\":\"s-8\",\"type\":\"sticky\"}},\"at\":{{\"after\":[\"s-9\"]}}}},\
{{\"op\":\"add\",\"obj\":{{\"id\":\"s-7\",\"type\":\"sticky\"}}}},\
{{\"op\":\"set\",\"id\":\"s-1\",\"rev\":\"{REV}\",\"f\":{{\"text\":\"n\"}},\"unset\":[\"w\"]}},\
{{\"op\":\"del\",\"id\":\"s-2\",\"rev\":\"{REV}\"}},\
{{\"op\":\"meta\",\"rev\":\"{REV}\",\"f\":{{\"title\":\"x\"}}}},\
{{\"op\":\"z\",\"id\":\"s-1\",\"to\":\"front\"}},\
{{\"op\":\"frame-order\",\"id\":\"f-1\",\"to\":{{\"after\":[\"f-2\"]}}}},\
{{\"op\":\"person\",\"token\":\"p-9\",\"person\":{{\"name\":\"N\",\"kind\":\"agent\",\"more\":1}}}},\
{{\"op\":\"clear-blob\",\"hash\":\"{REV}\"}}]}}"
    );
    let g = verbs::group_from_json(text.as_bytes(), &Limits::V1).expect("it reads");
    assert_eq!(g.id, "g9");
    assert_eq!(g.by, Some(id("p-1")));
    let rev0 = Sha256::parse(REV).expect("a hash");
    let want = vec![
        Op::Add {
            object: Object {
                id: id("s-9"),
                kind: ObjectType::Sticky,
                fields: fields(&[("text", support::text("t"))]),
            },
            at: Position::Back,
        },
        Op::Add {
            object: Object {
                id: id("s-8"),
                kind: ObjectType::Sticky,
                fields: Fields::new(),
            },
            at: Position::After(vec![id("s-9")]),
        },
        Op::Add {
            object: Object {
                id: id("s-7"),
                kind: ObjectType::Sticky,
                fields: Fields::new(),
            },
            at: Position::Front,
        },
        Op::Set {
            id: id("s-1"),
            rev: rev0,
            set: fields(&[("text", support::text("n"))]),
            unset: vec!["w".to_string()],
        },
        Op::Del {
            id: id("s-2"),
            rev: rev0,
        },
        Op::Meta {
            rev: rev0,
            set: fields(&[("title", support::text("x"))]),
            unset: Vec::new(),
        },
        Op::Z {
            id: id("s-1"),
            to: Position::Front,
        },
        Op::FrameOrder {
            id: id("f-1"),
            to: Position::After(vec![id("f-2")]),
        },
        Op::AddPerson {
            token: id("p-9"),
            person: Person {
                name: Some("N".to_string()),
                kind: Some("agent".to_string()),
                extra: fields(&[("more", Value::Num(1.0))]),
            },
        },
        Op::ClearBlob { hash: rev0 },
    ];
    assert_eq!(g.ops, want);
}

#[test]
fn raw_json_refuses_what_it_does_not_know() {
    for (what, text) in [
        ("a top-level member", "{\"ops\":[],\"extra\":1}".to_string()),
        ("no ops", "{}".to_string()),
        ("ops of the wrong kind", "{\"ops\":{}}".to_string()),
        (
            "a misspelt member",
            format!("{{\"ops\":[{{\"op\":\"del\",\"id\":\"a\",\"rev\":\"{REV}\",\"revs\":1}}]}}"),
        ),
        (
            "a misspelt set member",
            format!(
                "{{\"ops\":[{{\"op\":\"set\",\"id\":\"a\",\"rev\":\"{REV}\",\"fields\":{{}}}}]}}"
            ),
        ),
        ("an unknown op", "{\"ops\":[{\"op\":\"zap\"}]}".to_string()),
        ("an op of the wrong kind", "{\"ops\":[1]}".to_string()),
        (
            "a bad position",
            "{\"ops\":[{\"op\":\"z\",\"id\":\"a\",\"to\":\"sideways\"}]}".to_string(),
        ),
        (
            "a bad after",
            "{\"ops\":[{\"op\":\"z\",\"id\":\"a\",\"to\":{\"after\":\"b\"}}]}".to_string(),
        ),
        (
            "a bad revision",
            "{\"ops\":[{\"op\":\"del\",\"id\":\"a\",\"rev\":\"nope\"}]}".to_string(),
        ),
        (
            "a missing revision",
            "{\"ops\":[{\"op\":\"del\",\"id\":\"a\"}]}".to_string(),
        ),
        (
            "an add with no type",
            "{\"ops\":[{\"op\":\"add\",\"obj\":{\"id\":\"a\"}}]}".to_string(),
        ),
        (
            "an unset of the wrong kind",
            format!(
                "{{\"ops\":[{{\"op\":\"set\",\"id\":\"a\",\"rev\":\"{REV}\",\"unset\":[1]}}]}}"
            ),
        ),
        (
            "a group id of the wrong kind",
            "{\"id\":1,\"ops\":[]}".to_string(),
        ),
        (
            "a by of the wrong kind",
            "{\"by\":1,\"ops\":[]}".to_string(),
        ),
    ] {
        let r = verbs::group_from_json(text.as_bytes(), &Limits::V1);
        assert!(r.is_err(), "{what} was accepted: {r:?}");
        assert_eq!(
            r.expect_err("refused").code,
            RefusalCode::FieldValue,
            "{what}"
        );
    }
    let r = verbs::group_from_json(
        b"{\"ops\":[{\"op\":\"del\",\"id\":\"BAD\",\"rev\":\"x\"}]}",
        &Limits::V1,
    );
    assert_eq!(r.expect_err("refused").code, RefusalCode::IdInvalid);
}

#[test]
fn react_with_no_on_toggles() {
    let b = base();
    let toggle = |emoji: &str| Verb::React {
        id: id("s-2"),
        emoji: emoji.into(),
        on: None,
    };
    let (next, _, _) = run(&b, &toggle("fresh"));
    assert_eq!(
        get(obj(&next, "s-2"), "reactions"),
        &parse("{\"fresh\":[\"p-1\"],\"new\":[\"p-1\",\"p-2\"],\"x\":[\"p-1\"],\"z\":[\"p-2\"]}"),
        "one the caller hasn't is put on"
    );
    let (next, _, _) = run(&b, &toggle("new"));
    assert_eq!(
        get(obj(&next, "s-2"), "reactions"),
        &parse("{\"new\":[\"p-2\"],\"x\":[\"p-1\"],\"z\":[\"p-2\"]}"),
        "one the caller has is taken off, and only theirs"
    );
    let (next, _, _) = run(&b, &toggle("z"));
    assert_eq!(
        get(obj(&next, "s-2"), "reactions"),
        &parse("{\"new\":[\"p-1\",\"p-2\"],\"x\":[\"p-1\"],\"z\":[\"p-1\",\"p-2\"]}"),
        "someone else's doesn't count as the caller's"
    );
}

#[test]
fn a_friendly_add_refuses_an_unknown_field_or_type() {
    let b = base();
    let misspelt = add(ObjectType::Sticky, fields(&[("colr", text("red"))]));
    let e = plan_as(&b, Who::Me, 1, &misspelt).expect_err("a field no type has refuses");
    assert_eq!(verb_code(e), "unknown_field");
    let other = add(ObjectType::Sticky, fields(&[("route", text("straight"))]));
    let e = plan_as(&b, Who::Me, 1, &other).expect_err("another type's field refuses");
    assert_eq!(verb_code(e), "unknown_field");
    let widget = add(ObjectType::parse("widget").expect("a name"), no_fields());
    let e = plan_as(&b, Who::Me, 1, &widget).expect_err("a type the format lacks refuses");
    assert_eq!(verb_code(e), "unknown_type");
    let known = add(
        ObjectType::Sticky,
        fields(&[("x", Value::Num(1.0)), ("color", text("pink"))]),
    );
    run(&b, &known);
    let raw =
        "{\"ops\":[{\"op\":\"add\",\"obj\":{\"colr\":\"red\",\"id\":\"n\",\"type\":\"widget\"}}]}";
    let planned = labelled(&b, raw).expect("raw ops plan");
    let mut kept = b.clone();
    apply_group(&mut kept, &planned.group).expect("raw ops keep what they don't know");
    let o = obj(&kept, planned.ids[0].as_str());
    assert_eq!(o.kind.as_str(), "widget");
    assert_eq!(get(o, "colr"), &text("red"));
}

#[test]
fn a_friendly_update_refuses_an_unknown_field_or_type() {
    let b = board_of(
        "{\"id\":\"s-1\",\"later\":1,\"text\":\"a\",\"type\":\"sticky\"},{\"id\":\"w-1\",\"type\":\"widget\",\"x\":1}",
        "{}",
        "[]",
    );
    let update = |i: &str, set: Fields, unset: &[&str]| Verb::Update {
        id: id(i),
        rev: rev(&b, i),
        set,
        unset: unset.iter().map(|u| u.to_string()).collect(),
    };
    let e = plan_as(
        &b,
        Who::Me,
        1,
        &update("s-1", fields(&[("colr", text("red"))]), &[]),
    )
    .expect_err("a field no type has refuses");
    assert_eq!(verb_code(e), "unknown_field");
    let e = plan_as(
        &b,
        Who::Me,
        1,
        &update("s-1", fields(&[("route", text("straight"))]), &[]),
    )
    .expect_err("another type's field refuses");
    assert_eq!(verb_code(e), "unknown_field");
    let e = plan_as(&b, Who::Me, 1, &update("s-1", Fields::new(), &["later"]))
        .expect_err("removing a field the format lacks refuses");
    assert_eq!(verb_code(e), "unknown_field");
    let e = plan_as(
        &b,
        Who::Me,
        1,
        &update("w-1", fields(&[("x", Value::Num(2.0))]), &[]),
    )
    .expect_err("an object of a type the format lacks refuses");
    assert_eq!(verb_code(e), "unknown_type");
    run(
        &b,
        &update("s-1", fields(&[("x", Value::Num(2.0))]), &["text"]),
    );
}

#[test]
fn flags_turn_text_into_set_and_null_into_unset() {
    let pairs = [
        ("text", "hi"),
        ("x", "12.5"),
        ("locked", "true"),
        ("link", "null"),
        ("tags", "[\"a\"]"),
    ];
    let got = verbs::flags("sticky", &pairs, &Limits::V1).expect("the flags read");
    assert_eq!(
        got.set,
        fields(&[
            ("text", text("hi")),
            ("x", Value::Num(12.5)),
            ("locked", Value::Bool(true)),
            ("tags", parse("[\"a\"]")),
        ]),
        "each value by its kind"
    );
    assert_eq!(got.unset, vec!["link".to_string()], "null unsets");
    let refused =
        |t: &str, pairs: &[(&str, &str)]| verbs::flags(t, pairs, &Limits::V1).expect_err("refused");
    assert_eq!(
        verb_code(refused("sticky", &[("colr", "red")])),
        "unknown_field"
    );
    assert_eq!(
        verb_code(refused("sticky", &[("route", "null")])),
        "unknown_field"
    );
    assert_eq!(verb_code(refused("widget", &[("x", "1")])), "unknown_type");
    assert_eq!(
        verb_code(refused("sticky", &[("x", "1"), ("x", "null")])),
        "bad_value"
    );
    let VerbError::Conflict(c) = refused("sticky", &[("x", "wide")]) else {
        panic!("wanted the format's refusal");
    };
    assert_eq!(
        refusal_of(c),
        RefusalCode::JsonSyntax,
        "a number's text is read as the format reads a number"
    );
    let b = base();
    let v = Verb::Update {
        id: id("s-1"),
        rev: rev(&b, "s-1"),
        set: got.set,
        unset: got.unset,
    };
    let (next, _, _) = run(&b, &v);
    assert_eq!(get(obj(&next, "s-1"), "text"), &text("hi"));
}

#[test]
fn search_finds_check_items() {
    let b = board_of(
        "{\"checks\":[{\"done\":false,\"id\":\"c1\",\"text\":\"Buy milk\"}],\"id\":\"k-1\",\"type\":\"card\"},{\"id\":\"s-1\",\"text\":\"other\",\"type\":\"sticky\"}",
        "{}",
        "[]",
    );
    let found: Vec<String> = verbs::search(&b, "MILK")
        .expect("a search")
        .iter()
        .map(|v| v.object.id.to_string())
        .collect();
    assert_eq!(found, ["k-1"], "a check item's text, any case");
}

#[test]
fn meta_reads_the_settings_and_their_revision() {
    let b = base();
    let m = verbs::meta(&b).expect("it reads");
    assert_eq!(m.settings, b.settings, "the settings as they are");
    assert_eq!(
        m.rev,
        b.settings_rev().expect("a rev"),
        "the rev meta names"
    );
    let Value::Obj(v) = m.to_value() else {
        panic!("wanted an object");
    };
    assert_eq!(
        v.get("rev"),
        Some(&text(&m.rev.to_string())),
        "meta's json holds the rev"
    );
    assert_eq!(
        v.get("settings"),
        Some(&Value::Obj(b.settings.clone())),
        "meta's json holds the settings"
    );
    let change = Verb::Meta {
        rev: m.rev,
        set: fields(&[("title", text("New"))]),
        unset: Vec::new(),
    };
    let (next, _, _) = run(&b, &change);
    assert_eq!(next.settings["title"], text("New"));
}

#[test]
fn templates_lists_every_template_by_name_and_title() {
    let names: Vec<&str> = verbs::templates().iter().map(|(n, _)| *n).collect();
    assert_eq!(
        names,
        [
            "kickoff",
            "kanban",
            "retro",
            "flowchart",
            "storymap",
            "brainstorm",
            "mindmap"
        ]
    );
    for (name, title) in verbs::templates() {
        assert!(!title.is_empty(), "{name} has a title");
        let v = Verb::Template {
            name: name.to_string(),
            x: 0.0,
            y: 0.0,
        };
        let planned = plan_as(&base(), Who::Me, 1, &v).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(!planned.ids.is_empty(), "{name} adds objects");
    }
}

#[test]
fn apply_takes_update_and_delete_and_a_bare_list() {
    let b = base();
    let (r1, r3) = (rev(&b, "s-1"), rev(&b, "s-3"));
    let ops = format!(
        "[{{\"op\":\"update\",\"id\":\"s-1\",\"rev\":\"{r1}\",\"f\":{{\"text\":\"n\"}}}},\
{{\"op\":\"delete\",\"id\":\"s-3\",\"rev\":\"{r3}\"}}]"
    );
    let wrapped = format!("{{\"ops\":{ops}}}");
    let want = vec![
        Op::Set {
            id: id("s-1"),
            rev: r1,
            set: fields(&[("text", text("n"))]),
            unset: Vec::new(),
        },
        Op::Del {
            id: id("s-3"),
            rev: r3,
        },
    ];
    let g = verbs::group_from_json(wrapped.as_bytes(), &Limits::V1)
        .expect("check: update and delete read");
    assert_eq!(g.ops, want, "update is set, and delete is del");
    let g = verbs::group_from_json(ops.as_bytes(), &Limits::V1).expect("check: a bare list reads");
    assert_eq!(g.ops, want, "a bare list is the ops");
    let planned = labelled(&b, &wrapped).expect("check: update and delete read, labelled");
    assert_eq!(planned.group.ops, want);
    let planned = labelled(&b, &ops).expect("check: a bare list reads, labelled");
    assert_eq!(planned.group.ops, want, "a bare list is the ops, labelled");
}

fn labelled(b: &Board, text: &str) -> Result<verbs::Planned, VerbError> {
    let mut n = 500u64;
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
    verbs::labelled_from_json(b, &mut cx, text.as_bytes(), &Limits::V1)
}

#[test]
fn labels_become_fresh_ids_and_everything_naming_them_follows() {
    let b = base();
    let rev_s1 = rev(&b, "s-1");
    let text = format!(
        "{{\"ops\":[\
{{\"op\":\"add\",\"obj\":{{\"id\":\"a\",\"type\":\"sticky\"}}}},\
{{\"op\":\"add\",\"obj\":{{\"id\":\"b\",\"type\":\"frame\"}},\"at\":{{\"after\":[\"a\"]}}}},\
{{\"op\":\"add\",\"obj\":{{\"id\":\"c\",\"type\":\"connector\",\"from\":{{\"id\":\"a\"}},\"to\":{{\"id\":\"s-1\"}}}}}},\
{{\"op\":\"add\",\"obj\":{{\"id\":\"m\",\"type\":\"mind\",\"parent\":\"a\"}}}},\
{{\"op\":\"add\",\"obj\":{{\"id\":\"q\",\"type\":\"comment\",\"target\":\"a\",\"frame\":\"b\"}}}},\
{{\"op\":\"add\",\"obj\":{{\"type\":\"sticky\"}}}},\
{{\"op\":\"set\",\"id\":\"s-1\",\"rev\":\"{rev_s1}\",\"f\":{{\"frame\":\"b\"}}}},\
{{\"op\":\"z\",\"id\":\"a\",\"to\":{{\"after\":[\"s-1\"]}}}}]}}"
    );
    let planned = labelled(&b, &text).expect("it plans");
    assert_eq!(planned.ids.len(), 6);
    assert_eq!(
        planned.labels.len(),
        5,
        "the last has no label: {:?}",
        planned.labels
    );
    let mut next = b.clone();
    assert_eq!(
        apply_group(&mut next, &planned.group).map(|_| ()),
        Ok(()),
        "an op that names a label follows it, so the group applies"
    );
    next.validate(&Limits::V1).expect("valid");
    let a = planned.labels["a"].as_str().to_string();
    let f = planned.labels["b"].as_str().to_string();
    assert_ne!(a, "a");
    let c = obj(&next, planned.labels["c"].as_str());
    assert_eq!(get(c, "from"), &parse(&format!("{{\"id\":\"{a}\"}}")));
    assert_eq!(get(c, "to"), &parse("{\"id\":\"s-1\"}"), "a real id stays");
    assert_eq!(
        get(obj(&next, planned.labels["m"].as_str()), "parent"),
        &support::text(&a)
    );
    let q = obj(&next, planned.labels["q"].as_str());
    assert_eq!(get(q, "target"), &support::text(&a));
    assert_eq!(get(q, "frame"), &support::text(&f));
    assert_eq!(
        get(obj(&next, "s-1"), "frame"),
        &support::text(&f),
        "a set follows"
    );
    assert!(get(obj(&next, &a), "created") != &Value::Null, "stamped");
    let s1 = next
        .objects
        .iter()
        .position(|o| o.id.as_str() == "s-1")
        .expect("s-1");
    assert_eq!(
        next.objects[s1 + 1].id.as_str(),
        a,
        "a z follows: {:?}",
        order(&next)
    );
    assert_inverse(&b, &planned.group);
}

#[test]
fn a_label_on_a_position_follows() {
    let b = board_of(
        "{\"id\":\"s-1\",\"type\":\"sticky\"},{\"id\":\"s-2\",\"type\":\"sticky\"}",
        "{}",
        "[]",
    );
    let text = "{\"ops\":[\
{\"op\":\"add\",\"obj\":{\"id\":\"a\",\"type\":\"sticky\"},\"at\":\"back\"},\
{\"op\":\"add\",\"obj\":{\"id\":\"b\",\"type\":\"sticky\"},\"at\":{\"after\":[\"a\"]}},\
{\"op\":\"z\",\"id\":\"s-2\",\"to\":{\"after\":[\"b\"]}}]}";
    let planned = labelled(&b, text).expect("it plans");
    let mut next = b.clone();
    apply_group(&mut next, &planned.group).expect("it applies");
    assert_eq!(next.objects[0].id, planned.labels["a"]);
    assert_eq!(
        next.objects[1].id,
        planned.labels["b"],
        "{:?}",
        order(&next)
    );
    assert_eq!(
        next.objects[2].id,
        id("s-2"),
        "a move above a label: {:?}",
        order(&next)
    );
}

#[test]
fn a_label_used_twice_is_refused_and_so_is_a_stray_member() {
    let b = base();
    let text = "{\"ops\":[{\"op\":\"add\",\"obj\":{\"id\":\"a\",\"type\":\"sticky\"}},{\"op\":\"add\",\"obj\":{\"id\":\"a\",\"type\":\"text\"}}]}";
    let Err(VerbError::Conflict(c)) = labelled(&b, text) else {
        panic!("wanted a refusal");
    };
    assert_eq!(refusal_of(c), RefusalCode::FieldValue);
    let Err(VerbError::Conflict(c)) = labelled(&b, "{\"ops\":[],\"id\":\"x\"}") else {
        panic!("wanted a refusal");
    };
    assert_eq!(refusal_of(c), RefusalCode::FieldValue);
}

#[test]
fn a_field_from_text_goes_by_the_schemas_kind_and_never_rounds() {
    let l = &Limits::V1;
    let ok = |t: &str, f: &str, s: &str| verbs::field_from_text(t, f, s, l);
    assert_eq!(
        ok("sticky", "text", "12"),
        Ok(support::text("12")),
        "text stays text"
    );
    assert_eq!(ok("sticky", "x", "1.25"), Ok(Value::Num(1.25)));
    assert_eq!(ok("sticky", "locked", "true"), Ok(Value::Bool(true)));
    assert_eq!(ok("sticky", "locked", "false"), Ok(Value::Bool(false)));
    assert_eq!(ok("sticky", "color", "pink"), Ok(support::text("pink")));
    assert_eq!(
        ok("sticky", "link", "https://a.example"),
        Ok(support::text("https://a.example"))
    );
    assert_eq!(ok("sticky", "frame", "f-1"), Ok(support::text("f-1")));
    assert_eq!(ok("sticky", "group", "g-1"), Ok(support::text("g-1")));
    assert_eq!(ok("sticky", "tags", "[\"a\"]"), Ok(parse("[\"a\"]")));
    assert_eq!(
        ok("connector", "from", "{\"x\":1,\"y\":2}"),
        Ok(parse("{\"x\":1,\"y\":2}"))
    );
    assert_eq!(
        ok("sticky", "later", "plain"),
        Ok(support::text("plain")),
        "unknown: text"
    );
    assert_eq!(
        ok("sticky", "later", "{\"a\":1}"),
        Ok(parse("{\"a\":1}")),
        "unknown: JSON"
    );
    assert_eq!(
        ok("sticky", "title", "T"),
        Ok(support::text("T")),
        "a setting's kind"
    );
    assert_eq!(
        ok("sticky", "title", "[x"),
        Ok(support::text("[x")),
        "a setting takes its kind: text, even if it starts like a list"
    );
    let code = |t: &str, f: &str, s: &str| verbs::field_from_text(t, f, s, l).map_err(|e| e.code);
    assert_eq!(code("sticky", "x", "abc"), Err(RefusalCode::JsonSyntax));
    assert_eq!(code("sticky", "x", "\"1\""), Err(RefusalCode::FieldValue));
    assert_eq!(
        code("sticky", "x", "0.1000000000000000055"),
        Err(RefusalCode::JsonNumberInexact)
    );
    assert_eq!(
        code("sticky", "locked", "yes"),
        Err(RefusalCode::FieldValue)
    );
    assert_eq!(
        code("sticky", "tags", "not json"),
        Err(RefusalCode::JsonSyntax)
    );
}

#[test]
fn every_verb_has_its_name_listed() {
    let b = base();
    let v = add(ObjectType::Sticky, no_fields());
    assert_eq!(v.name(), "add");
    let names = verbs::WRITE_VERBS;
    let mut sorted: Vec<&str> = names.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), names.len(), "a name twice");
    assert!(names.contains(&delete(&b, "s-1", false).name()));
}
