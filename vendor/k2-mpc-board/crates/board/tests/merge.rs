//! The whole-object merge: every case per object, order, people, references, and through a file.

#[path = "support/old_merge.rs"]
mod old_merge;
#[path = "support/verbs.rs"]
mod support;

use k2_mpc_board::json::{Fields, Value};
use k2_mpc_board::*;
use std::io::Cursor;
use support::*;

fn base() -> Board {
    board_of(
        "{\"id\":\"f-1\",\"title\":\"One\",\"type\":\"frame\"},\
{\"id\":\"f-2\",\"title\":\"Two\",\"type\":\"frame\"},\
{\"id\":\"f-3\",\"title\":\"Three\",\"type\":\"frame\"},\
{\"id\":\"s-1\",\"later\":{\"keep\":1},\"text\":\"one\",\"type\":\"sticky\"},\
{\"id\":\"s-2\",\"text\":\"two\",\"type\":\"sticky\"},\
{\"id\":\"s-3\",\"text\":\"three\",\"type\":\"sticky\"},\
{\"from\":{\"id\":\"s-1\"},\"id\":\"c-1\",\"to\":{\"id\":\"s-2\"},\"type\":\"connector\"},\
{\"id\":\"m-1\",\"type\":\"mind\"},\
{\"id\":\"m-2\",\"type\":\"mind\"}",
        "{\"title\":\"T\"}",
        "[\"f-1\",\"f-2\",\"f-3\"]",
    )
}

fn set(b: &mut Board, i: &str, name: &str, v: Value) {
    let o = b
        .objects
        .iter_mut()
        .find(|o| o.id.as_str() == i)
        .expect("that object");
    o.fields.insert(name.to_string(), v);
}

fn gone(b: &mut Board, i: &str) {
    b.objects.retain(|o| o.id.as_str() != i);
    b.frames.retain(|f| f.as_str() != i);
}

fn added(i: &str, f: &[(&str, Value)]) -> Object {
    Object {
        id: id(i),
        kind: ObjectType::Sticky,
        fields: fields(f),
    }
}

fn clean(b: &Board, o: &Board, t: &Board) -> Merged {
    merge3(b, o, t).unwrap_or_else(|e| panic!("wanted a clean merge: {e:?}"))
}

fn conflicts(b: &Board, o: &Board, t: &Board) -> MergeConflicts {
    match merge3(b, o, t) {
        Err(MergeError::Conflicts(c)) => *c,
        other => panic!("wanted conflicts, got {other:?}"),
    }
}

fn ids(list: &[Id]) -> Vec<&str> {
    list.iter().map(Id::as_str).collect()
}

#[test]
fn nothing_changed_is_the_base() {
    let b = base();
    let m = clean(&b, &b, &b);
    assert_eq!(m.board, b);
    assert!(m.from_ours.is_empty() && m.from_theirs.is_empty() && m.deleted.is_empty());
    assert!(m.moved.is_empty() && m.orphans.is_empty());
}

#[test]
fn a_change_on_one_side_is_taken_whole() {
    let b = base();
    let mut o = b.clone();
    set(&mut o, "s-1", "text", text("ours"));
    let m = clean(&b, &o, &b);
    assert_eq!(
        obj(&m.board, "s-1"),
        obj(&o, "s-1"),
        "it kept ours, unknowns and all"
    );
    assert_eq!(ids(&m.from_ours), ["s-1"]);
    assert!(m.from_theirs.is_empty());

    let mut t = b.clone();
    set(&mut t, "s-1", "text", text("theirs"));
    let m = clean(&b, &b, &t);
    assert_eq!(obj(&m.board, "s-1"), obj(&t, "s-1"));
    assert_eq!(ids(&m.from_theirs), ["s-1"]);
    assert!(m.from_ours.is_empty());
}

#[test]
fn changes_to_different_objects_both_land() {
    let b = base();
    let (mut o, mut t) = (b.clone(), b.clone());
    set(&mut o, "s-1", "text", text("ours"));
    set(&mut t, "s-2", "text", text("theirs"));
    let m = clean(&b, &o, &t);
    assert_eq!(get(obj(&m.board, "s-1"), "text"), &text("ours"));
    assert_eq!(get(obj(&m.board, "s-2"), "text"), &text("theirs"));
    assert_eq!(ids(&m.from_ours), ["s-1"]);
    assert_eq!(ids(&m.from_theirs), ["s-2"]);
}

#[test]
fn the_same_change_on_both_sides_is_no_conflict() {
    let b = base();
    let (mut o, mut t) = (b.clone(), b.clone());
    set(&mut o, "s-1", "text", text("same"));
    set(&mut t, "s-1", "text", text("same"));
    let m = clean(&b, &o, &t);
    assert_eq!(get(obj(&m.board, "s-1"), "text"), &text("same"));
    assert_eq!(ids(&m.from_ours), ["s-1"]);
    assert!(m.from_theirs.is_empty(), "counted once");
}

#[test]
fn one_object_changed_on_both_sides_is_a_typed_conflict_with_every_side_kept() {
    let b = base();
    let (mut o, mut t) = (b.clone(), b.clone());
    set(&mut o, "s-1", "text", text("ours"));
    set(&mut t, "s-1", "text", text("theirs"));
    let c = conflicts(&b, &o, &t);
    assert_eq!(c.objects.len(), 1);
    let x = &c.objects[0];
    assert_eq!(x.id, id("s-1"));
    assert_eq!(x.kind, ConflictKind::BothChanged);
    assert_eq!(x.kind.as_str(), "both_changed");
    assert_eq!(x.base.as_ref(), Some(obj(&b, "s-1")));
    assert_eq!(x.ours.as_ref(), Some(obj(&o, "s-1")), "our edit is kept");
    assert_eq!(x.theirs.as_ref(), Some(obj(&t, "s-1")));
}

#[test]
fn different_fields_of_one_object_still_conflict_whole() {
    let b = base();
    let (mut o, mut t) = (b.clone(), b.clone());
    set(&mut o, "s-1", "text", text("ours"));
    set(&mut t, "s-1", "x", Value::Num(1.0));
    let c = conflicts(&b, &o, &t);
    assert_eq!(
        c.objects[0].kind,
        ConflictKind::BothChanged,
        "whole objects, by rule"
    );
}

#[test]
fn a_lock_on_one_side_and_an_edit_on_the_other_conflict() {
    let b = base();
    let (mut o, mut t) = (b.clone(), b.clone());
    set(&mut o, "s-1", "text", text("ours"));
    set(&mut t, "s-1", "locked", Value::Bool(true));
    let c = conflicts(&b, &o, &t);
    assert_eq!(c.objects[0].kind, ConflictKind::BothChanged);
}

#[test]
fn a_conflict_says_the_limit() {
    let b = base();
    let (mut o, mut t) = (b.clone(), b.clone());
    set(&mut o, "s-1", "text", text("ours"));
    set(&mut t, "s-1", "text", text("theirs"));
    let said = conflicts(&b, &o, &t).to_string();
    for words in [
        "one writer at a time",
        "never last-writer-wins",
        "both edits are kept",
    ] {
        assert!(said.contains(words), "{said:?} doesn't say {words:?}");
    }
}

#[test]
fn a_delete_on_one_side_of_an_untouched_object_lands() {
    let b = base();
    let mut o = b.clone();
    gone(&mut o, "s-3");
    let m = clean(&b, &o, &b);
    assert!(!has(&m.board, "s-3"));
    assert_eq!(ids(&m.deleted), ["s-3"]);
    let m = clean(&b, &b, &o);
    assert!(!has(&m.board, "s-3"), "and from theirs");
    assert_eq!(ids(&m.deleted), ["s-3"]);
}

#[test]
fn a_delete_on_both_sides_is_gone_once() {
    let b = base();
    let mut o = b.clone();
    gone(&mut o, "s-3");
    let m = clean(&b, &o, &o);
    assert!(!has(&m.board, "s-3"));
    assert_eq!(ids(&m.deleted), ["s-3"]);
}

#[test]
fn a_delete_against_an_edit_conflicts_either_way_with_the_edit_kept() {
    let b = base();
    let (mut o, mut t) = (b.clone(), b.clone());
    gone(&mut o, "s-3");
    set(&mut t, "s-3", "text", text("theirs"));
    let c = conflicts(&b, &o, &t);
    assert_eq!(c.objects[0].kind, ConflictKind::OursDeleted);
    assert_eq!(c.objects[0].ours, None);
    assert_eq!(c.objects[0].theirs.as_ref(), Some(obj(&t, "s-3")));

    let c = conflicts(&b, &t, &o);
    assert_eq!(c.objects[0].kind, ConflictKind::TheirsDeleted);
    assert_eq!(c.objects[0].ours.as_ref(), Some(obj(&t, "s-3")));
    assert_eq!(c.objects[0].theirs, None);
    assert_eq!(c.objects[0].kind.as_str(), "theirs_deleted");
}

#[test]
fn an_add_on_one_side_lands_and_the_same_add_on_both_is_one() {
    let b = base();
    let mut o = b.clone();
    o.objects.push(added("n-1", &[("text", text("new"))]));
    let m = clean(&b, &o, &b);
    assert_eq!(obj(&m.board, "n-1"), obj(&o, "n-1"));
    assert_eq!(ids(&m.from_ours), ["n-1"]);
    let m = clean(&b, &b, &o);
    assert_eq!(obj(&m.board, "n-1"), obj(&o, "n-1"));
    let m = clean(&b, &o, &o);
    assert_eq!(
        m.board.objects.iter().filter(|x| x.id == id("n-1")).count(),
        1
    );
}

#[test]
fn two_different_adds_with_one_id_conflict() {
    let b = base();
    let (mut o, mut t) = (b.clone(), b.clone());
    o.objects.push(added("n-1", &[("text", text("ours"))]));
    t.objects.push(added("n-1", &[("text", text("theirs"))]));
    let c = conflicts(&b, &o, &t);
    assert_eq!(c.objects[0].kind, ConflictKind::BothAdded);
    assert_eq!(c.objects[0].base, None);
}

#[test]
fn every_conflict_is_listed_and_nothing_is_merged() {
    let b = base();
    let (mut o, mut t) = (b.clone(), b.clone());
    set(&mut o, "s-1", "text", text("ours"));
    set(&mut t, "s-1", "text", text("theirs"));
    set(&mut o, "s-2", "text", text("ours"));
    set(&mut t, "s-2", "text", text("theirs"));
    o.settings.insert("title".into(), text("ours"));
    t.settings.insert("title".into(), text("theirs"));
    o.extra.insert("later".into(), text("ours"));
    t.extra.insert("later".into(), text("theirs"));
    let c = conflicts(&b, &o, &t);
    let listed: Vec<&Id> = c.objects.iter().map(|x| &x.id).collect();
    assert_eq!(listed, [&id("s-1"), &id("s-2")]);
    assert!(c.settings.is_some());
    assert_eq!(c.extra[0].name, "later");
}

/// One merge with conflicts of every kind: each is named, with both sides' values, in one pass.
#[test]
fn one_merge_names_every_conflict_with_its_values() {
    let mut b = base();
    b.people.insert(id("p-5"), person("Five"));
    b.people.insert(id("p-6"), person("Six"));
    b.extra.insert("ea".into(), text("base-a"));
    let (mut o, mut t) = (b.clone(), b.clone());
    // Objects: both changed, twice; ours deleted; theirs deleted; both added.
    for i in ["s-1", "s-2"] {
        set(&mut o, i, "text", text("ours"));
        set(&mut t, i, "text", text("theirs"));
    }
    gone(&mut o, "m-1");
    set(&mut t, "m-1", "text", text("theirs"));
    set(&mut o, "m-2", "text", text("ours"));
    gone(&mut t, "m-2");
    o.objects.push(added("n-1", &[("text", text("ours"))]));
    t.objects.push(added("n-1", &[("text", text("theirs"))]));
    // Settings.
    o.settings.insert("title".into(), text("ours"));
    t.settings.insert("title".into(), text("theirs"));
    // People: two tokens, one of them new on both sides.
    o.people.insert(id("p-5"), person("Ours"));
    t.people.insert(id("p-5"), person("Theirs"));
    o.people.insert(id("p-7"), person("Ours new"));
    t.people.insert(id("p-7"), person("Theirs new"));
    // Top-level unknowns: two, one of them new on both sides.
    o.extra.insert("ea".into(), text("ours-a"));
    t.extra.insert("ea".into(), text("theirs-a"));
    o.extra.insert("eb".into(), text("ours-b"));
    t.extra.insert("eb".into(), text("theirs-b"));

    let c = conflicts(&b, &o, &t);

    let object = |i: &str, kind, from_base: bool, ours: bool, theirs: bool| ObjectConflict {
        id: id(i),
        kind,
        base: from_base.then(|| obj(&b, i).clone()),
        ours: ours.then(|| obj(&o, i).clone()),
        theirs: theirs.then(|| obj(&t, i).clone()),
    };
    assert_eq!(
        c.objects,
        vec![
            object("m-1", ConflictKind::OursDeleted, true, false, true),
            object("m-2", ConflictKind::TheirsDeleted, true, true, false),
            object("n-1", ConflictKind::BothAdded, false, true, true),
            object("s-1", ConflictKind::BothChanged, true, true, true),
            object("s-2", ConflictKind::BothChanged, true, true, true),
        ],
        "every object, with its sides"
    );
    assert_eq!(
        c.settings,
        Some(SettingsConflict {
            base: b.settings.clone(),
            ours: o.settings.clone(),
            theirs: t.settings.clone(),
        }),
        "the settings, with their sides"
    );
    assert_eq!(
        c.people,
        vec![
            PersonConflict {
                token: id("p-5"),
                base: Some(person("Five")),
                ours: person("Ours"),
                theirs: person("Theirs"),
            },
            PersonConflict {
                token: id("p-7"),
                base: None,
                ours: person("Ours new"),
                theirs: person("Theirs new"),
            },
        ],
        "every person, with their sides"
    );
    assert_eq!(
        c.extra,
        vec![
            ExtraConflict {
                name: "ea".to_string(),
                base: Some(text("base-a")),
                ours: Some(text("ours-a")),
                theirs: Some(text("theirs-a")),
            },
            ExtraConflict {
                name: "eb".to_string(),
                base: None,
                ours: Some(text("ours-b")),
                theirs: Some(text("theirs-b")),
            },
        ],
        "every unknown field, with its sides"
    );
}

/// Conflicts of each kind, as many as asked: the settings or not, then `extra` unknown fields
/// `e-000` on, `people` tokens `p-000` on, and `objects` objects `x-000` on.
fn conflicting(
    settings: bool,
    extra: usize,
    people: usize,
    objects: usize,
) -> (Board, Board, Board) {
    let mut b = base();
    for i in 0..objects {
        b.objects
            .push(added(&format!("x-{i:03}"), &[("text", text("base"))]));
    }
    let (mut o, mut t) = (b.clone(), b.clone());
    if settings {
        o.settings.insert("title".into(), text("ours"));
        t.settings.insert("title".into(), text("theirs"));
    }
    for i in 0..extra {
        o.extra.insert(format!("e-{i:03}"), text("ours"));
        t.extra.insert(format!("e-{i:03}"), text("theirs"));
    }
    for i in 0..people {
        o.people.insert(id(&format!("p-{i:03}")), person("Ours"));
        t.people.insert(id(&format!("p-{i:03}")), person("Theirs"));
    }
    for i in 0..objects {
        let x = format!("x-{i:03}");
        set(&mut o, &x, "text", text("ours"));
        set(&mut t, &x, "text", text("theirs"));
    }
    (b, o, t)
}

fn names(n: usize, prefix: &str) -> Vec<String> {
    (0..n).map(|i| format!("{prefix}-{i:03}")).collect()
}

fn kept_extra(c: &MergeConflicts) -> Vec<String> {
    c.extra.iter().map(|x| x.name.clone()).collect()
}

fn kept_people(c: &MergeConflicts) -> Vec<String> {
    c.people
        .iter()
        .map(|x| x.token.as_str().to_string())
        .collect()
}

fn kept_objects(c: &MergeConflicts) -> Vec<String> {
    c.objects
        .iter()
        .map(|x| x.id.as_str().to_string())
        .collect()
}

/// One conflict past the cap, of every kind, with the cap falling among the objects.
#[test]
fn past_the_cap_the_first_conflicts_are_kept_in_order_and_the_rest_counted() {
    let n = MERGE_CONFLICTS_MAX + 1 - 5;
    let (b, o, t) = conflicting(true, 2, 2, n);
    let c = conflicts(&b, &o, &t);
    assert_eq!(
        c.totals,
        ConflictTotals {
            settings: 1,
            extra: 2,
            people: 2,
            objects: n as u64,
        },
        "every conflict is counted, kept or not"
    );
    assert!(c.truncated, "one conflict is counted and not kept");
    assert_eq!(c.kept(), MERGE_CONFLICTS_MAX, "the cap is kept in full");
    assert_eq!(
        c.settings,
        Some(SettingsConflict {
            base: b.settings.clone(),
            ours: o.settings.clone(),
            theirs: t.settings.clone(),
        }),
        "the settings come first"
    );
    assert_eq!(kept_extra(&c), names(2, "e"), "then unknown fields by name");
    assert_eq!(kept_people(&c), names(2, "p"), "then people by token");
    assert_eq!(
        kept_objects(&c),
        names(n - 1, "x"),
        "then objects by id, up to the cap: the last one is counted, not kept"
    );
    for x in &c.objects {
        let i = x.id.as_str();
        assert_eq!(
            (x.base.as_ref(), x.ours.as_ref(), x.theirs.as_ref()),
            (Some(obj(&b, i)), Some(obj(&o, i)), Some(obj(&t, i))),
            "{i} is kept in full"
        );
    }
    let said = c.to_string();
    for words in [
        format!("{} conflicts", MERGE_CONFLICTS_MAX + 1),
        format!("{n} objects, 2 people and 3 other things"),
        format!(
            "{} of {} are shown in full",
            MERGE_CONFLICTS_MAX,
            MERGE_CONFLICTS_MAX + 1
        ),
    ] {
        assert!(said.contains(&words), "{said:?} doesn't say {words:?}");
    }
}

/// The cap can fall in any kind: the kinds before it are kept whole, and the kinds after it are only
/// counted.
#[test]
fn the_cap_falls_in_each_kind_in_the_stated_order() {
    let over = MERGE_CONFLICTS_MAX + 40;

    // In the unknown fields: the settings and the first fields are kept, and no person or object.
    let (b, o, t) = conflicting(true, over, 3, 3);
    let c = conflicts(&b, &o, &t);
    assert_eq!(
        c.totals,
        ConflictTotals {
            settings: 1,
            extra: over as u64,
            people: 3,
            objects: 3,
        },
        "counted, with the cap in the unknown fields"
    );
    assert!(c.truncated, "truncated in the unknown fields");
    assert!(
        c.settings.is_some(),
        "the settings come before the unknown fields"
    );
    assert_eq!(
        kept_extra(&c),
        names(MERGE_CONFLICTS_MAX - 1, "e"),
        "the unknown fields by name, up to the cap"
    );
    assert_eq!(kept_people(&c), names(0, "p"), "no person past the cap");
    assert_eq!(kept_objects(&c), names(0, "x"), "no object past the cap");

    // In the people: the settings, every unknown field, the first people, and no object.
    let (b, o, t) = conflicting(true, 3, over, 3);
    let c = conflicts(&b, &o, &t);
    assert_eq!(
        c.totals,
        ConflictTotals {
            settings: 1,
            extra: 3,
            people: over as u64,
            objects: 3,
        },
        "counted, with the cap in the people"
    );
    assert!(c.truncated, "truncated in the people");
    assert!(c.settings.is_some(), "the settings come before the people");
    assert_eq!(
        kept_extra(&c),
        names(3, "e"),
        "the unknown fields come before the people"
    );
    assert_eq!(
        kept_people(&c),
        names(MERGE_CONFLICTS_MAX - 4, "p"),
        "the people by token, up to the cap"
    );
    assert_eq!(kept_objects(&c), names(0, "x"), "no object past the cap");

    // Only objects: the first by id.
    let (b, o, t) = conflicting(false, 0, 0, over);
    let c = conflicts(&b, &o, &t);
    assert_eq!(
        c.totals,
        ConflictTotals {
            objects: over as u64,
            ..ConflictTotals::default()
        },
        "counted, with only objects"
    );
    assert!(c.truncated, "truncated in the objects");
    assert_eq!(
        kept_objects(&c),
        names(MERGE_CONFLICTS_MAX, "x"),
        "the objects by id, up to the cap"
    );
}

#[test]
fn exactly_the_cap_keeps_everything() {
    let n = MERGE_CONFLICTS_MAX - 5;
    let (b, o, t) = conflicting(true, 2, 2, n);
    let c = conflicts(&b, &o, &t);
    assert_eq!(
        c.totals,
        ConflictTotals {
            settings: 1,
            extra: 2,
            people: 2,
            objects: n as u64,
        },
        "every conflict is counted"
    );
    assert!(!c.truncated, "nothing is cut at exactly the cap");
    assert_eq!(c.kept(), MERGE_CONFLICTS_MAX, "every conflict is kept");
    assert_eq!(kept_objects(&c), names(n, "x"), "every object is kept");
    let said = c.to_string();
    let words = format!("{0} of {0} are shown in full", MERGE_CONFLICTS_MAX);
    assert!(said.contains(&words), "{said:?} doesn't say {words:?}");
}

#[test]
fn settings_merge_whole() {
    let b = base();
    let (mut o, mut t) = (b.clone(), b.clone());
    o.settings.insert("title".into(), text("ours"));
    let m = clean(&b, &o, &t);
    assert_eq!(m.board.settings["title"], text("ours"));
    let m = clean(&b, &t, &o);
    assert_eq!(m.board.settings["title"], text("ours"), "from theirs too");
    t.settings.insert("keep".into(), text("x"));
    let c = conflicts(&b, &o, &t);
    assert!(c.settings.is_some(), "whole settings, by rule");
    let m = clean(&b, &o, &o);
    assert_eq!(m.board.settings, o.settings, "the same on both sides");
}

fn person(name: &str) -> Person {
    Person {
        name: Some(name.to_string()),
        ..Person::default()
    }
}

#[test]
fn people_merge_by_union() {
    let b = base();
    let (mut o, mut t) = (b.clone(), b.clone());
    o.people.insert(id("p-7"), person("Ours"));
    t.people.insert(id("p-8"), person("Theirs"));
    let m = clean(&b, &o, &t);
    assert!(m.board.people.contains_key(&id("p-7")));
    assert!(m.board.people.contains_key(&id("p-8")));
    assert!(
        m.board.people.contains_key(&id(ME)),
        "and the ones already there"
    );
}

#[test]
fn a_token_one_side_dropped_stays() {
    let b = base();
    let mut o = b.clone();
    o.people.remove(&id(OTHER));
    let m = clean(&b, &o, &b);
    assert!(m.board.people.contains_key(&id(OTHER)));
    let m = clean(&b, &b, &o);
    assert!(m.board.people.contains_key(&id(OTHER)));
    let m = clean(&b, &o, &o);
    assert!(
        m.board.people.contains_key(&id(OTHER)),
        "gone from both, kept from the base"
    );
}

#[test]
fn a_person_changed_on_one_side_is_taken_and_on_both_conflicts() {
    let b = base();
    let (mut o, mut t) = (b.clone(), b.clone());
    o.people.insert(id(OTHER), person("Ours"));
    let m = clean(&b, &o, &t);
    assert_eq!(m.board.people[&id(OTHER)].name.as_deref(), Some("Ours"));
    t.people.insert(id(OTHER), person("Theirs"));
    let c = conflicts(&b, &o, &t);
    assert_eq!(c.people[0].token, id(OTHER));
    assert_eq!(c.people[0].ours, person("Ours"));
    assert_eq!(c.people[0].theirs, person("Theirs"));
    // One token, added on both sides for two people.
    let (mut o, mut t) = (b.clone(), b.clone());
    o.people.insert(id("p-7"), person("Ours"));
    t.people.insert(id("p-7"), person("Theirs"));
    assert_eq!(conflicts(&b, &o, &t).people.len(), 1);
}

#[test]
fn top_level_unknown_fields_merge_three_ways() {
    let mut b = base();
    b.extra.insert("old".into(), text("x"));
    let (mut o, mut t) = (b.clone(), b.clone());
    o.extra.insert("later".into(), text("ours"));
    t.extra.remove("old");
    let m = clean(&b, &o, &t);
    assert_eq!(
        m.board.extra.get("later"),
        Some(&text("ours")),
        "a field ours added is kept"
    );
    assert!(!m.board.extra.contains_key("old"), "gone from theirs");
    t.extra.insert("later".into(), text("ours"));
    assert_eq!(
        clean(&b, &o, &t).board.extra.get("later"),
        Some(&text("ours")),
        "the same on both"
    );
}

fn move_up(b: &mut Board, i: &str) {
    let at = b
        .objects
        .iter()
        .position(|o| o.id.as_str() == i)
        .expect("there");
    let o = b.objects.remove(at);
    b.objects.push(o);
}

fn move_down(b: &mut Board, i: &str) {
    let at = b
        .objects
        .iter()
        .position(|o| o.id.as_str() == i)
        .expect("there");
    let o = b.objects.remove(at);
    b.objects.insert(0, o);
}

const START: [&str; 9] = [
    "f-1", "f-2", "f-3", "s-1", "s-2", "s-3", "c-1", "m-1", "m-2",
];

#[test]
fn nobody_moving_keeps_the_order() {
    let b = base();
    assert_eq!(order(&b), START);
    assert_eq!(order(&clean(&b, &b, &b).board), START);
}

#[test]
fn a_move_on_either_side_lands() {
    let b = base();
    let mut o = b.clone();
    move_up(&mut o, "s-1");
    let m = clean(&b, &o, &b);
    assert_eq!(order(&m.board), order(&o));
    assert_eq!(m.moved.len(), 1, "{:?}", m.moved);
    assert_eq!(m.moved[0].id, id("s-1"));
    assert_eq!(m.moved[0].list, OrderList::Objects);
    assert!(!m.moved[0].over_theirs);
    let m = clean(&b, &b, &o);
    assert_eq!(order(&m.board), order(&o), "theirs stands as it is");
    assert!(m.moved.is_empty(), "ours moved nothing: {:?}", m.moved);
}

#[test]
fn moves_of_different_objects_both_land() {
    let b = base();
    let (mut o, mut t) = (b.clone(), b.clone());
    move_up(&mut o, "s-1");
    move_down(&mut t, "m-2");
    let m = clean(&b, &o, &t);
    assert_eq!(
        order(&m.board),
        ["m-2", "f-1", "f-2", "f-3", "s-2", "s-3", "c-1", "m-1", "s-1"]
    );
}

#[test]
fn one_object_moved_on_both_sides_takes_ours_and_says_so() {
    let b = base();
    let (mut o, mut t) = (b.clone(), b.clone());
    move_up(&mut o, "s-1");
    move_down(&mut t, "s-1");
    let m = clean(&b, &o, &t);
    assert_eq!(
        *order(&m.board).last().expect("an object"),
        "s-1",
        "ours stands"
    );
    assert_eq!(m.moved.len(), 1);
    assert!(m.moved[0].over_theirs, "and that's reported");
    let again = clean(&b, &o, &t);
    assert_eq!(again.board, m.board, "the same every time");
}

#[test]
fn what_ours_put_on_top_stays_on_top_whatever_theirs_did_below() {
    let b = base();
    let (mut o, mut t) = (b.clone(), b.clone());
    move_up(&mut o, "s-1");
    o.objects.push(added("n-1", &[]));
    move_up(&mut t, "m-1");
    let m = clean(&b, &o, &t);
    let got = order(&m.board);
    assert_eq!(&got[got.len() - 2..], ["s-1", "n-1"], "{got:?}");
}

#[test]
fn an_added_object_goes_above_its_neighbour_or_the_nearest_still_there() {
    let b = base();
    let mut o = b.clone();
    o.objects.insert(5, added("n-1", &[]));
    let m = clean(&b, &o, &b);
    assert_eq!(order(&m.board), order(&o), "above s-2");
    let mut t = b.clone();
    gone(&mut t, "s-2");
    let m = clean(&b, &o, &t);
    assert_eq!(
        order(&m.board),
        ["f-1", "f-2", "f-3", "s-1", "n-1", "s-3", "c-1", "m-1", "m-2"],
        "s-2 went, so above s-1"
    );
    let mut o = b.clone();
    o.objects.insert(0, added("n-1", &[]));
    let m = clean(&b, &o, &b);
    assert_eq!(order(&m.board)[0], "n-1", "no neighbour below: the bottom");
}

#[test]
fn frames_merge_their_own_order() {
    let b = base();
    let (mut o, mut t) = (b.clone(), b.clone());
    o.frames = vec![id("f-2"), id("f-1"), id("f-3")];
    let m = clean(&b, &o, &t);
    assert_eq!(ids(&m.board.frames), ["f-2", "f-1", "f-3"]);
    assert_eq!(m.moved[0].list, OrderList::Frames);
    t.frames = vec![id("f-1"), id("f-3"), id("f-2")];
    let m = clean(&b, &o, &t);
    m.board.validate(&Limits::V1).expect("valid");
    assert_eq!(m.board.frames.len(), 3);
    gone(&mut t, "f-3");
    let m = clean(&b, &o, &t);
    assert!(
        !m.board.frames.contains(&id("f-3")),
        "a deleted frame leaves the list"
    );
    m.board.validate(&Limits::V1).expect("valid");
}

#[test]
fn a_frame_ours_deleted_leaves_theirs_list() {
    let b = base();
    let mut o = b.clone();
    gone(&mut o, "f-3");
    let frames = merge3(&b, &o, &b).map(|m| m.board.frames);
    assert_eq!(
        frames,
        Ok(vec![id("f-1"), id("f-2")]),
        "a frame one side deleted leaves the list, and the merge is clean"
    );
    let m = clean(&b, &o, &b);
    assert!(!order(&m.board).contains(&"f-3"));
}

#[test]
fn the_order_is_not_symmetric_and_ours_is_favoured() {
    let b = base();
    let (mut x, mut y) = (b.clone(), b.clone());
    move_up(&mut x, "s-1");
    move_down(&mut y, "s-1");
    let xy = clean(&b, &x, &y);
    let yx = clean(&b, &y, &x);
    assert_eq!(
        *order(&xy.board).last().expect("an object"),
        "s-1",
        "x is ours: on top"
    );
    assert_eq!(order(&yx.board)[0], "s-1", "y is ours: at the bottom");
    assert_ne!(
        order(&xy.board),
        order(&yx.board),
        "swapping the sides changes the order"
    );
    assert!(
        xy.moved[0].over_theirs && yx.moved[0].over_theirs,
        "and each says so"
    );
    let fields_of = |m: &Merged| {
        let mut all = m.board.objects.clone();
        all.sort_by(|p, q| p.id.cmp(&q.id));
        all
    };
    assert_eq!(fields_of(&xy), fields_of(&yx), "only the order differs");
}

#[test]
fn a_frame_added_on_one_side_joins_the_list() {
    let b = base();
    let mut o = b.clone();
    o.objects.push(Object {
        id: id("f-9"),
        kind: ObjectType::Frame,
        fields: Fields::new(),
    });
    o.frames.insert(1, id("f-9"));
    let m = clean(&b, &o, &b);
    assert_eq!(ids(&m.board.frames), ["f-1", "f-9", "f-2", "f-3"]);
}

#[test]
fn a_connector_edited_while_its_end_goes_conflicts_only_if_it_went_too() {
    let b = base();
    let (mut o, mut t) = (b.clone(), b.clone());
    gone(&mut o, "s-1");
    gone(&mut o, "c-1");
    set(&mut t, "c-1", "label", text("kept"));
    let c = conflicts(&b, &o, &t);
    assert_eq!(c.objects[0].id, id("c-1"));
    assert_eq!(c.objects[0].kind, ConflictKind::OursDeleted);

    let mut o = b.clone();
    gone(&mut o, "s-1");
    let m = clean(&b, &o, &t);
    assert!(has(&m.board, "c-1"), "no delete of it: it stays");
    assert_eq!(m.orphans.len(), 1, "{:?}", m.orphans);
    assert_eq!(m.orphans[0].from, id("c-1"));
    assert_eq!(m.orphans[0].state, RefState::Orphaned);
}

#[test]
fn a_new_connector_to_a_deleted_object_stays_and_is_reported() {
    let b = base();
    let (mut o, mut t) = (b.clone(), b.clone());
    gone(&mut t, "s-3");
    o.objects.push(Object {
        id: id("c-9"),
        kind: ObjectType::Connector,
        fields: fields(&[
            ("from", parse("{\"id\":\"s-3\"}")),
            ("to", parse("{\"x\":1,\"y\":2}")),
        ]),
    });
    let m = clean(&b, &o, &t);
    assert!(has(&m.board, "c-9"));
    assert_eq!(m.orphans.len(), 1, "{:?}", m.orphans);
    assert_eq!(m.orphans[0].from, id("c-9"));
}

#[test]
fn an_orphan_one_side_already_had_is_not_news() {
    let b = board_of(
        "{\"from\":{\"id\":\"gone\"},\"id\":\"c-1\",\"to\":{\"x\":1,\"y\":2},\"type\":\"connector\"},{\"id\":\"s-1\",\"type\":\"sticky\"}",
        "{}",
        "[]",
    );
    let mut o = b.clone();
    set(&mut o, "s-1", "text", text("x"));
    assert!(clean(&b, &o, &b).orphans.is_empty());
}

#[test]
fn a_group_against_a_delete_conflicts_on_that_member() {
    let b = base();
    let (mut o, mut t) = (b.clone(), b.clone());
    set(&mut o, "s-1", "group", text("g-1"));
    set(&mut o, "s-2", "group", text("g-1"));
    gone(&mut t, "s-2");
    let c = conflicts(&b, &o, &t);
    assert_eq!(c.objects.len(), 1);
    assert_eq!(c.objects[0].id, id("s-2"));
    assert_eq!(c.objects[0].kind, ConflictKind::TheirsDeleted);
}

#[test]
fn two_edits_that_make_a_loop_merge_and_read_as_suspect() {
    let b = base();
    let (mut o, mut t) = (b.clone(), b.clone());
    set(&mut o, "m-1", "parent", text("m-2"));
    set(&mut t, "m-2", "parent", text("m-1"));
    let m = clean(&b, &o, &t);
    m.board.validate(&Limits::V1).expect("valid");
    let states: Vec<RefState> = m
        .board
        .references()
        .into_iter()
        .filter(|r| r.field == "parent")
        .map(|r| r.state)
        .collect();
    assert!(states.contains(&RefState::Suspect), "{states:?}");
}

#[test]
fn a_member_of_a_frame_deleted_on_the_other_side_stays() {
    let b = base();
    let (mut o, mut t) = (b.clone(), b.clone());
    set(&mut o, "s-1", "frame", text("f-1"));
    gone(&mut t, "f-1");
    let m = clean(&b, &o, &t);
    assert!(has(&m.board, "s-1"));
    assert_eq!(m.orphans[0].field, "frame");
}

#[test]
fn a_result_over_a_limit_is_refused_typed() {
    let b = board_of("", "{}", "[]");
    let (mut o, mut t) = (b.clone(), b.clone());
    let many = |tag: &str, n: u32| -> Vec<Object> {
        (0..n)
            .map(|i| Object {
                id: id(&format!("{tag}{i}")),
                kind: ObjectType::Sticky,
                fields: Fields::new(),
            })
            .collect()
    };
    o.objects = many("a", 60_000);
    t.objects = many("b", 60_000);
    match merge3(&b, &o, &t) {
        Err(MergeError::Refused(r)) => assert_eq!(r.code, RefusalCode::LimitObjects),
        other => panic!(
            "wanted a refusal: {:?}",
            other.map(|m| m.board.objects.len())
        ),
    }
}

/// Both sides, swapped: what's in the board doesn't depend on who merges.
#[test]
fn the_objects_do_not_depend_on_who_merges() {
    let b = base();
    let mut seen = 0;
    for (i, a) in START.iter().enumerate() {
        for c in &START[i + 1..] {
            let (mut o, mut t) = (b.clone(), b.clone());
            set(&mut o, a, "x", Value::Num(1.0));
            gone(&mut t, c);
            if *c != "s-3" {
                set(&mut t, "s-3", "y", Value::Num(2.0));
            }
            let (Ok(m1), Ok(m2)) = (merge3(&b, &o, &t), merge3(&b, &t, &o)) else {
                continue;
            };
            let mut x: Vec<Object> = m1.board.objects.clone();
            let mut y: Vec<Object> = m2.board.objects.clone();
            x.sort_by(|p, q| p.id.cmp(&q.id));
            y.sort_by(|p, q| p.id.cmp(&q.id));
            assert_eq!(x, y, "{a} against {c}");
            seen += 1;
        }
    }
    assert!(seen > 20, "only {seen} pairs merged cleanly");
}

fn file_of(b: &Board, base_of: Option<&Board>, parts: &mut MemParts) -> Vec<u8> {
    let mut meta = WriteMeta::new(Lineage::from_bytes([5; 16]));
    if let Some(from) = base_of {
        meta.base = Some(Base::new(Sha256::of(
            &from.to_canonical_json().expect("canonical"),
        )));
    }
    let mut out = Vec::new();
    write(&mut out, &meta, b, parts, &Limits::V1).expect("a file");
    out
}

#[test]
fn a_copy_says_what_it_came_from() {
    let b = base();
    let mut copy = b.clone();
    set(&mut copy, "s-1", "text", text("ours"));
    let bytes = file_of(&copy, Some(&b), &mut MemParts::new());
    let pkg = Package::open(Cursor::new(&bytes[..]), &Limits::V1).expect("it opens");
    assert!(base_matches(pkg.manifest(), &b).expect("a compare"));
    assert!(!base_matches(pkg.manifest(), &copy).expect("a compare"));
    let plain = file_of(&copy, None, &mut MemParts::new());
    let pkg = Package::open(Cursor::new(&plain[..]), &Limits::V1).expect("it opens");
    assert!(
        !base_matches(pkg.manifest(), &b).expect("a compare"),
        "it names nothing"
    );
}

#[test]
fn a_merge_saves_behind_the_guard_with_the_copys_blobs() {
    let b = base();
    let on_disk = {
        let mut t = b.clone();
        set(&mut t, "s-2", "text", text("theirs"));
        file_of(&t, None, &mut MemParts::new())
    };
    let mut copy_parts = MemParts::new();
    let hash = copy_parts.add_blob("image/png", b"picture".to_vec());
    let mut copy = b.clone();
    set(&mut copy, "s-1", "text", text("ours"));
    copy.objects.push(Object {
        id: id("i-1"),
        kind: ObjectType::Image,
        fields: fields(&[("src", parse(&format!("{{\"blob\":\"{hash}\"}}")))]),
    });
    let mut f = BoardFile::open(
        FileTarget::new("plan.k5board").expect("a path"),
        Cursor::new(&on_disk[..]),
        &Limits::V1,
    )
    .expect("it opens");
    let merged = f
        .merge(&b, &copy, &mut copy_parts)
        .expect("a clean merge")
        .value;
    assert_eq!(merged.from_ours.len(), 2, "{:?}", merged.from_ours);
    assert!(f.is_changed());
    let mut out = Vec::new();
    let saved = f
        .save(Cursor::new(&on_disk[..]), &mut out)
        .expect("check: a merged save writes, with the copy's blobs")
        .value;
    assert!(matches!(saved, Saved::Wrote { .. }), "{saved:?}");
    let mut again = Package::open(Cursor::new(&out[..]), &Limits::V1).expect("it reads back");
    assert_eq!(get(obj(again.board(), "s-1"), "text"), &text("ours"));
    assert_eq!(get(obj(again.board(), "s-2"), "text"), &text("theirs"));
    assert_eq!(
        again.read_blob(&hash).expect("the blob").bytes,
        b"picture".to_vec()
    );
    assert_eq!(
        again.manifest().lineage,
        Lineage::from_bytes([5; 16]),
        "the file's own"
    );
}

#[test]
fn a_merged_save_still_stops_at_a_stale_guard() {
    let b = base();
    let on_disk = file_of(&b, None, &mut MemParts::new());
    let mut copy = b.clone();
    set(&mut copy, "s-1", "text", text("ours"));
    let mut f = BoardFile::open(
        FileTarget::new("plan.k5board").expect("a path"),
        Cursor::new(&on_disk[..]),
        &Limits::V1,
    )
    .expect("it opens");
    f.merge(&b, &copy, &mut MemParts::new())
        .expect("a clean merge");
    let mut moved = b.clone();
    set(&mut moved, "s-3", "text", text("later"));
    let later = file_of(&moved, None, &mut MemParts::new());
    let mut out = Vec::new();
    let e = f
        .save(Cursor::new(&later[..]), &mut out)
        .expect_err("stale")
        .value;
    assert!(matches!(e, SaveError::Stale { .. }), "{e:?}");
    assert!(out.is_empty());
}

#[test]
fn a_conflict_leaves_the_file_and_keeps_the_edit() {
    let b = base();
    let mut t = b.clone();
    set(&mut t, "s-1", "text", text("theirs"));
    let on_disk = file_of(&t, None, &mut MemParts::new());
    let mut copy = b.clone();
    set(&mut copy, "s-1", "text", text("ours"));
    let mut f = BoardFile::open(
        FileTarget::new("plan.k5board").expect("a path"),
        Cursor::new(&on_disk[..]),
        &Limits::V1,
    )
    .expect("it opens");
    let e = f
        .merge(&b, &copy, &mut MemParts::new())
        .expect_err("a conflict")
        .value;
    let MergeError::Conflicts(c) = e else {
        panic!("wanted conflicts: {e:?}");
    };
    assert_eq!(c.objects[0].ours.as_ref(), Some(obj(&copy, "s-1")));
    assert!(!f.is_changed(), "the board is as it was");
    let mut out = Vec::new();
    assert_eq!(
        f.save(Cursor::new(&on_disk[..]), &mut out)
            .expect("saves")
            .value,
        Saved::Nothing
    );
}

/// A copy's parts that don't match their listing, as a file that lies would give.
struct Lying(MemParts);

impl PartSource for Lying {
    fn list(&self) -> Vec<Part> {
        self.0.list()
    }

    fn read_part(&mut self, _name: &str) -> Result<Vec<u8>, Refusal> {
        Ok(b"something else".to_vec())
    }
}

#[test]
fn blobs_in_a_merge_are_checked_and_a_file_blob_is_not_replaced() {
    let b = base();
    let on_disk = file_of(&b, None, &mut MemParts::new());
    let mut parts = MemParts::new();
    let hash = parts.add_blob("image/png", b"picture".to_vec());
    let mut copy = b.clone();
    copy.objects.push(Object {
        id: id("i-1"),
        kind: ObjectType::Image,
        fields: fields(&[("src", parse(&format!("{{\"blob\":\"{hash}\"}}")))]),
    });
    let mut f = BoardFile::open(
        FileTarget::new("plan.k5board").expect("a path"),
        Cursor::new(&on_disk[..]),
        &Limits::V1,
    )
    .expect("it opens");
    let e = f
        .merge(&b, &copy, &mut Lying(parts.clone()))
        .expect_err("refused")
        .value;
    let MergeError::Refused(r) = e else {
        panic!("wanted a refusal: {e:?}");
    };
    assert_eq!(r.code, RefusalCode::PartHash);
    assert!(!f.is_changed(), "nothing changed");

    // A blob no side supplies stays dangling, and the save refuses it, typed.
    let mut f = BoardFile::open(
        FileTarget::new("plan.k5board").expect("a path"),
        Cursor::new(&on_disk[..]),
        &Limits::V1,
    )
    .expect("it opens");
    f.merge(&b, &copy, &mut MemParts::new())
        .expect("a clean merge");
    let mut out = Vec::new();
    let e = f
        .save(Cursor::new(&on_disk[..]), &mut out)
        .expect_err("refused")
        .value;
    assert!(
        matches!(e, SaveError::Refused(ref r) if r.code == RefusalCode::BlobMissing),
        "{e:?}"
    );

    // A blob the file holds isn't asked of the copy, so a copy that lies about it changes nothing.
    let mut own = MemParts::new();
    own.add_blob("image/png", b"picture".to_vec());
    let holding = file_of(&copy, None, &mut own);
    let mut f = BoardFile::open(
        FileTarget::new("plan.k5board").expect("a path"),
        Cursor::new(&holding[..]),
        &Limits::V1,
    )
    .expect("it opens");
    let mut later = copy.clone();
    set(&mut later, "s-1", "text", text("later"));
    let stands = f
        .merge(&copy, &later, &mut Lying(parts.clone()))
        .map(|_| ());
    assert_eq!(
        stands,
        Ok(()),
        "the file's own blob stands, and the copy isn't asked for it"
    );

    // A file that holds it keeps its own listing.
    let mut own = MemParts::new();
    own.add_blob("image/webp", b"picture".to_vec());
    let holding = file_of(&copy, None, &mut own);
    let mut theirs_changed = copy.clone();
    set(&mut theirs_changed, "s-1", "text", text("ours"));
    let mut f = BoardFile::open(
        FileTarget::new("plan.k5board").expect("a path"),
        Cursor::new(&holding[..]),
        &Limits::V1,
    )
    .expect("it opens");
    f.merge(&copy, &theirs_changed, &mut parts)
        .expect("a clean merge");
    let mut out = Vec::new();
    f.save(Cursor::new(&holding[..]), &mut out).expect("saves");
    let again = Package::open(Cursor::new(&out[..]), &Limits::V1).expect("reads");
    let part = again
        .manifest()
        .parts
        .iter()
        .find(|p| p.name == hash.blob_name())
        .expect("listed");
    assert_eq!(part.media_type, "image/webp");
}

/// A small generator with a fixed seed, so a failure names its case.
struct Rng(u64);

impl Rng {
    fn next(&mut self, below: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % below.max(1) as u64) as usize
    }
}

/// One side's random edits: moves, deletes and adds, of objects and frames.
fn wander(b: &Board, rng: &mut Rng, tag: &str) -> Board {
    let mut out = b.clone();
    for step in 0..rng.next(12) {
        let n = out.objects.len();
        match rng.next(5) {
            0 | 1 if n > 0 => {
                let o = out.objects.remove(rng.next(n));
                let at = rng.next(n);
                out.objects.insert(at, o);
            }
            2 if n > 0 => {
                let i = out.objects[rng.next(n)].id.clone();
                gone(&mut out, i.as_str());
            }
            3 if !out.frames.is_empty() => {
                let f = out.frames.remove(rng.next(out.frames.len()));
                let at = rng.next(out.frames.len() + 1);
                out.frames.insert(at, f);
            }
            _ => {
                let i = id(&format!("{tag}-{step}"));
                let kind = if rng.next(3) == 0 {
                    out.frames.insert(rng.next(out.frames.len() + 1), i.clone());
                    ObjectType::Frame
                } else {
                    ObjectType::Sticky
                };
                let at = rng.next(n + 1);
                out.objects.insert(
                    at,
                    Object {
                        id: i,
                        kind,
                        fields: Fields::new(),
                    },
                );
            }
        }
    }
    out
}

fn ids_of(list: &[Id]) -> Vec<&Id> {
    list.iter().collect()
}

/// The shipped order against the first one, for one case. Says which case and which ids differ.
fn same_as_the_first(case: &str, b: &Board, o: &Board, t: &Board) -> bool {
    let Ok(m) = merge3(b, o, t) else {
        return false;
    };
    let (bo, oo, to) = (
        b.objects.iter().map(|x| x.id.clone()).collect::<Vec<_>>(),
        o.objects.iter().map(|x| x.id.clone()).collect::<Vec<_>>(),
        t.objects.iter().map(|x| x.id.clone()).collect::<Vec<_>>(),
    );
    let keep: std::collections::BTreeSet<&Id> = m.board.objects.iter().map(|x| &x.id).collect();
    let mut notes = Vec::new();
    let objects =
        old_merge::merge_order(&ids_of(&bo), &ids_of(&oo), &ids_of(&to), &keep, &mut notes);
    let frames = old_merge::merge_order(
        &ids_of(&b.frames),
        &ids_of(&o.frames),
        &ids_of(&t.frames),
        &keep,
        &mut notes,
    );
    let got: Vec<Id> = m.board.objects.iter().map(|x| x.id.clone()).collect();
    if let Some(i) = (0..got.len().max(objects.len())).find(|i| got.get(*i) != objects.get(*i)) {
        panic!(
            "{case}: objects differ at {i}: shipped {:?}, first {:?}\nbase {:?}\nours {:?}\ntheirs {:?}",
            got.get(i),
            objects.get(i),
            order(b),
            order(o),
            order(t)
        );
    }
    assert_eq!(
        m.board.frames, frames,
        "{case}: frames\nbase {:?}\nours {:?}\ntheirs {:?}",
        b.frames, o.frames, t.frames
    );
    let moved: Vec<(Id, bool)> = m
        .moved
        .iter()
        .map(|x| (x.id.clone(), x.over_theirs))
        .collect();
    assert_eq!(moved, notes, "{case}: what's reported as moved");
    true
}

#[test]
fn the_order_matches_the_first_version_on_generated_boards() {
    let mut compared = 0;
    for seed in 1..=3000u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let n = rng.next(40);
        let frames = rng.next(6);
        let mut objects = Vec::new();
        for i in 0..n {
            objects.push(format!("{{\"id\":\"o-{i}\",\"type\":\"sticky\"}}"));
        }
        let mut listed = Vec::new();
        for i in 0..frames {
            objects.insert(
                rng.next(objects.len() + 1),
                format!("{{\"id\":\"f-{i}\",\"type\":\"frame\"}}"),
            );
            listed.push(format!("\"f-{i}\""));
        }
        let b = board_of(&objects.join(","), "{}", &format!("[{}]", listed.join(",")));
        let o = wander(&b, &mut rng, "wx");
        let t = wander(&b, &mut rng, "wy");
        if same_as_the_first(&format!("generated board, seed {seed}"), &b, &o, &t) {
            compared += 1;
        }
    }
    assert!(compared > 2000, "only {compared} boards merged cleanly");
}

#[test]
fn the_order_matches_the_first_version_on_the_corpus() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../vectors/fidelity");
    let mut compared = 0;
    let mut names: Vec<std::path::PathBuf> = std::fs::read_dir(&root)
        .expect("the corpus")
        .map(|e| e.expect("an entry").path())
        .filter(|p| !p.to_string_lossy().ends_with(".out.json"))
        .collect();
    names.sort();
    for (n, path) in names.iter().enumerate() {
        let bytes = std::fs::read(path).expect("a case");
        let b = Board::from_json(&bytes, &Limits::V1).expect("the corpus reads");
        for seed in 1..=40u64 {
            let mut rng = Rng((seed << 8 | n as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
            let o = wander(&b, &mut rng, "wx");
            let t = wander(&b, &mut rng, "wy");
            let case = format!("{} seed {seed}", path.display());
            if same_as_the_first(&case, &b, &o, &t) {
                compared += 1;
            }
        }
    }
    assert!(compared > 300, "only {compared} merged cleanly");
}

#[test]
fn the_order_matches_the_first_version_on_the_hard_shapes() {
    let objects: Vec<String> = (0..300)
        .map(|i| format!("{{\"id\":\"aaaaaaaaaaaaaaaaaa{i:06}\",\"type\":\"frame\"}}"))
        .collect();
    let listed: Vec<String> = (0..300)
        .map(|i| format!("\"aaaaaaaaaaaaaaaaaa{i:06}\""))
        .collect();
    let b = board_of(&objects.join(","), "{}", &format!("[{}]", listed.join(",")));
    let mut reversed = b.clone();
    reversed.objects.reverse();
    reversed.frames.reverse();
    assert!(same_as_the_first(
        "reversed, theirs the base",
        &b,
        &reversed,
        &b
    ));
    assert!(same_as_the_first(
        "reversed on both",
        &b,
        &reversed,
        &reversed
    ));
    let mut rotated = b.clone();
    rotated.objects.rotate_left(150);
    rotated.frames.rotate_left(150);
    assert!(same_as_the_first(
        "reversed, theirs rotated",
        &b,
        &reversed,
        &rotated
    ));
    assert!(same_as_the_first(
        "rotated, theirs reversed",
        &b,
        &rotated,
        &reversed
    ));
}

/// The worst shape in `examples/merge_cost.rs`, at the format's limit. It runs under a host's writer
/// lock, so its cost is held here: well over what it takes on a loaded build node, and far under
/// what scanning for each moved object would.
#[test]
fn the_worst_order_at_the_limit_ends_in_bounded_time() {
    let mut objects = Vec::with_capacity(100_000);
    let mut frames = Vec::with_capacity(100_000);
    for i in 0..100_000 {
        let i = id(&format!("aaaaaaaaaaaaaaaaaa{i:06}"));
        frames.push(i.clone());
        objects.push(Object {
            id: i,
            kind: ObjectType::Frame,
            fields: Fields::new(),
        });
    }
    let b = Board {
        objects,
        frames,
        ..Board::empty()
    };
    let mut o = b.clone();
    o.objects.reverse();
    o.frames.reverse();
    let (done, wait) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let moved = merge3(&b, &o, &b).map(|m| m.moved.len());
        done.send(moved).expect("the test is waiting");
    });
    let moved = wait.recv_timeout(std::time::Duration::from_secs(120));
    assert!(moved.is_ok(), "the worst order took over 120 s");
    assert_eq!(moved.expect("it ended").expect("clean"), 2 * 99_999);
}
