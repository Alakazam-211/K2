//! A verb's own refusal carries what it's about as data (`verbs::Facts`): each place that refuses
//! sets exactly the facts its code names, and no others. One case per place that refuses.

#[path = "support/verbs.rs"]
mod support;

use k2_mpc_board::verbs::{self, AlignTo, Axis, Ctx, Facts, Me, Side, Verb, VerbCode, VerbError};
use k2_mpc_board::*;
use support::*;

fn base() -> Board {
    board_of(
        "{\"id\":\"s-1\",\"text\":\"one\",\"type\":\"sticky\",\"x\":0,\"y\":0},\
{\"id\":\"s-2\",\"text\":\"two\",\"type\":\"sticky\",\"x\":200,\"y\":0},\
{\"from\":{\"id\":\"s-1\"},\"id\":\"c-1\",\"to\":{\"id\":\"s-2\"},\"type\":\"connector\"},\
{\"id\":\"m-1\",\"text\":\"root\",\"type\":\"mind\"},\
{\"checks\":[{\"done\":false,\"id\":\"c1\",\"text\":\"one\"}],\"id\":\"k-1\",\"type\":\"card\"},\
{\"columns\":[{\"id\":\"a\"}],\"id\":\"b-1\",\"rows\":[{\"id\":\"r1\"}],\"type\":\"table\"},\
{\"id\":\"s-0000000000\",\"type\":\"sticky\"}",
        "{\"voting\":{\"active\":false}}",
        "[]",
    )
}

fn voting(per: u32) -> Board {
    board_of(
        "{\"id\":\"s-1\",\"type\":\"sticky\",\"votes\":{\"p-1\":3}}",
        &format!("{{\"voting\":{{\"active\":true,\"perPerson\":{per}}}}}"),
        "[]",
    )
}

/// The code and the facts of a verb's own refusal.
fn refused(e: VerbError) -> (VerbCode, Facts) {
    match e {
        VerbError::Refused { code, facts, .. } => (code, *facts),
        other => panic!("wanted a verb's refusal, got {other:?}"),
    }
}

fn plan_err(b: &Board, verb: &Verb) -> (VerbCode, Facts) {
    refused(plan_as(b, Who::Me, 1, verb).expect_err("the verb refuses"))
}

fn object(i: &str, type_name: &str) -> Facts {
    let mut f = Facts::default();
    f.id = Some(id(i));
    f.type_name = Some(type_name.to_string());
    f
}

fn min(n: u64) -> Facts {
    let mut f = Facts::default();
    f.min = Some(n);
    f
}

fn item(i: &str, it: &str) -> Facts {
    let mut f = Facts::default();
    f.id = Some(id(i));
    f.item = Some(it.to_string());
    f
}

fn of_type(t: &str) -> Facts {
    let mut f = Facts::default();
    f.type_name = Some(t.to_string());
    f
}

fn none() -> Facts {
    Facts::default()
}

// ---------------------------------------------------------------- wrong_type: id and type_name

#[test]
fn wrong_type_on_an_object_names_it_and_its_type() {
    let b = base();
    let v = Verb::Reply {
        id: id("s-1"),
        text: "hi".into(),
    };
    assert_eq!(
        plan_err(&b, &v),
        (VerbCode::WrongType, object("s-1", "sticky"))
    );
    let v = Verb::CheckAdd {
        id: id("m-1"),
        text: "x".into(),
    };
    assert_eq!(
        plan_err(&b, &v),
        (VerbCode::WrongType, object("m-1", "mind"))
    );
}

#[test]
fn wrong_type_for_a_box_names_the_connector() {
    let b = base();
    let v = Verb::Move {
        ids: vec![named(&b, "s-1"), named(&b, "c-1")],
        dx: 1.0,
        dy: 1.0,
    };
    assert_eq!(
        plan_err(&b, &v),
        (VerbCode::WrongType, object("c-1", "connector"))
    );
}

#[test]
fn wrong_type_for_a_mind_child_names_the_parent() {
    let b = base();
    let v = Verb::MindChild {
        parent: id("s-1"),
        text: "x".into(),
    };
    assert_eq!(
        plan_err(&b, &v),
        (VerbCode::WrongType, object("s-1", "sticky"))
    );
}

#[test]
fn wrong_type_for_a_connected_copy_names_the_object() {
    let b = base();
    let v = Verb::ConnectedCopy {
        id: id("m-1"),
        side: Side::Right,
    };
    assert_eq!(
        plan_err(&b, &v),
        (VerbCode::WrongType, object("m-1", "mind"))
    );
}

// ---------------------------------------------------------------- too_few: min

#[test]
fn too_few_names_the_fewest_each_verb_takes() {
    let b = base();
    let one = vec![named(&b, "s-1")];
    let two = vec![named(&b, "s-1"), named(&b, "s-2")];
    assert_eq!(
        plan_err(&b, &Verb::Group { ids: one.clone() }),
        (VerbCode::TooFew, min(2)),
        "group"
    );
    assert_eq!(
        plan_err(
            &b,
            &Verb::Align {
                ids: one.clone(),
                to: AlignTo::Left
            }
        ),
        (VerbCode::TooFew, min(2)),
        "align"
    );
    assert_eq!(
        plan_err(
            &b,
            &Verb::Distribute {
                ids: two,
                axis: Axis::Horizontal
            }
        ),
        (VerbCode::TooFew, min(3)),
        "distribute"
    );
    assert_eq!(
        plan_err(&b, &Verb::Tidy { ids: one }),
        (VerbCode::TooFew, min(2)),
        "tidy"
    );
    let paste = Verb::Paste {
        objects: Vec::new(),
        x: 0.0,
        y: 0.0,
    };
    assert_eq!(plan_err(&b, &paste), (VerbCode::TooFew, min(1)), "paste");
}

// ---------------------------------------------------------------- no_template: name

#[test]
fn no_template_names_the_template() {
    let v = Verb::Template {
        name: "nope".into(),
        x: 0.0,
        y: 0.0,
    };
    let mut want = Facts::default();
    want.name = Some("nope".into());
    assert_eq!(plan_err(&base(), &v), (VerbCode::NoTemplate, want));
}

// ---------------------------------------------------------------- unknown_field, unknown_type

#[test]
fn unknown_field_names_the_type_and_the_field() {
    let v = Verb::Add {
        kind: ObjectType::Sticky,
        fields: fields(&[("text", text("a")), ("nope", text("b"))]),
        at: Position::Front,
    };
    let mut want = of_type("sticky");
    want.field = Some("nope".into());
    assert_eq!(
        plan_err(&base(), &v),
        (VerbCode::UnknownField, want.clone())
    );
    let e = verbs::flags("sticky", &[("nope", "1")], &Limits::V1).expect_err("flags refuse");
    assert_eq!(refused(e), (VerbCode::UnknownField, want), "flags");
}

#[test]
fn unknown_type_names_the_type() {
    let v = Verb::Add {
        kind: ObjectType::parse("zzz").expect("check: a type name the format may hold"),
        fields: fields(&[]),
        at: Position::Front,
    };
    assert_eq!(
        plan_err(&base(), &v),
        (VerbCode::UnknownType, of_type("zzz"))
    );
    let e = verbs::flags("zzz", &[("x", "1")], &Limits::V1).expect_err("flags refuse");
    assert_eq!(refused(e), (VerbCode::UnknownType, of_type("zzz")), "flags");
    let e = verbs::fields(Some("nope")).expect_err("fields refuses");
    assert_eq!(
        refused(e),
        (VerbCode::UnknownType, of_type("nope")),
        "fields"
    );
}

// ---------------------------------------------------------------- no_such_item: id and item

#[test]
fn no_such_item_names_the_object_and_the_item() {
    let b = base();
    let v = Verb::CheckSet {
        id: id("k-1"),
        rev: rev(&b, "k-1"),
        item: "c9".into(),
        text: None,
        done: Some(true),
    };
    assert_eq!(plan_err(&b, &v), (VerbCode::NoSuchItem, item("k-1", "c9")));
    let v = Verb::TableCell {
        id: id("b-1"),
        rev: rev(&b, "b-1"),
        row: "r1".into(),
        column: "z9".into(),
        text: "x".into(),
    };
    assert_eq!(
        plan_err(&b, &v),
        (VerbCode::NoSuchItem, item("b-1", "z9")),
        "a column"
    );
    let v = Verb::TableCell {
        id: id("b-1"),
        rev: rev(&b, "b-1"),
        row: "r9".into(),
        column: "a".into(),
        text: "x".into(),
    };
    assert_eq!(
        plan_err(&b, &v),
        (VerbCode::NoSuchItem, item("b-1", "r9")),
        "a row"
    );
    let v = Verb::TableAddRow {
        id: id("b-1"),
        after: Some("r9".into()),
    };
    assert_eq!(
        plan_err(&b, &v),
        (VerbCode::NoSuchItem, item("b-1", "r9")),
        "after a row"
    );
}

// ---------------------------------------------------------------- the codes with no facts

#[test]
fn voting_and_values_carry_no_facts() {
    let b = base();
    let vote = |d| Verb::Vote {
        id: id("s-1"),
        delta: d,
    };
    assert_eq!(plan_err(&b, &vote(1)), (VerbCode::VotingOff, none()));
    assert_eq!(plan_err(&b, &vote(0)), (VerbCode::BadValue, none()));
    assert_eq!(
        plan_err(&voting(3), &vote(1)),
        (VerbCode::NoVotesLeft, none())
    );
    let v = Verb::Comment {
        target: None,
        text: String::new(),
        point: None,
    };
    assert_eq!(plan_err(&b, &v), (VerbCode::BadValue, none()));
    let e = verbs::flags("sticky", &[("text", "a"), ("text", "b")], &Limits::V1)
        .expect_err("flags refuse");
    assert_eq!(refused(e), (VerbCode::BadValue, none()), "flags");
}

#[test]
fn needs_actor_and_no_free_id_carry_no_facts() {
    let b = base();
    let v = Verb::React {
        id: id("s-1"),
        emoji: "+1".into(),
        on: None,
    };
    let e = plan_as(&b, Who::Nobody, 1, &v).expect_err("no one is acting");
    assert_eq!(refused(e), (VerbCode::NeedsActor, none()), "react");
    // a reply needs the time too
    let mut random = || [0u8; 8];
    let mut cx = Ctx {
        group_id: "g".into(),
        by: Some(Me {
            token: id(ME),
            new: None,
        }),
        at: None,
        random: &mut random,
    };
    let comment = board_of(
        "{\"id\":\"q-1\",\"text\":\"c\",\"type\":\"comment\"}",
        "{}",
        "[]",
    );
    let v = Verb::Reply {
        id: id("q-1"),
        text: "hi".into(),
    };
    let e = verbs::plan(&comment, &mut cx, &v).expect_err("a reply needs the time");
    assert_eq!(refused(e), (VerbCode::NeedsActor, none()), "reply");
    // every random id lands on s-0000000000
    let mut same = || [0u8; 8];
    let mut cx = Ctx {
        group_id: "g".into(),
        by: None,
        at: None,
        random: &mut same,
    };
    let v = Verb::Add {
        kind: ObjectType::Sticky,
        fields: fields(&[]),
        at: Position::Front,
    };
    let e = verbs::plan(&b, &mut cx, &v).expect_err("no free id");
    assert_eq!(refused(e), (VerbCode::NoFreeId, none()));
}
