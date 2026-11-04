//! The verbs that place things, copy things and fill in templates.

#[path = "support/verbs.rs"]
mod support;

use k2_mpc_board::json::{Fields, Value};
use k2_mpc_board::verbs::{self, AlignTo, Axis, Layer, Side, Verb, VerbError};
use k2_mpc_board::*;
use support::*;

fn at(i: &str, kind: &str, x: f64, y: f64, w: f64, h: f64) -> String {
    format!("{{\"h\":{h},\"id\":\"{i}\",\"type\":\"{kind}\",\"w\":{w},\"x\":{x},\"y\":{y}}}")
}

fn board(objects: &[String]) -> Board {
    let frames: Vec<String> = objects
        .iter()
        .filter(|o| o.contains("\"type\":\"frame\""))
        .map(|o| {
            let i = o.split("\"id\":\"").nth(1).expect("an id");
            format!("\"{}\"", i.split('"').next().expect("an id"))
        })
        .collect();
    board_of(&objects.join(","), "{}", &format!("[{}]", frames.join(",")))
}

fn xy(b: &Board, i: &str) -> (f64, f64) {
    let o = obj(b, i);
    (num(o, "x"), num(o, "y"))
}

fn revs(b: &Board, ids: &[&str]) -> Vec<verbs::ObjRev> {
    ids.iter().map(|i| named(b, i)).collect()
}

#[test]
fn a_move_changes_only_x_and_y() {
    let b = board(&[
        "{\"id\":\"s-1\",\"later\":1,\"text\":\"t\",\"type\":\"sticky\",\"x\":1,\"y\":2}"
            .to_string(),
        at("s-2", "sticky", 5.0, 5.0, 1.0, 1.0),
    ]);
    let v = Verb::Move {
        ids: revs(&b, &["s-1"]),
        dx: 10.0,
        dy: -2.0,
    };
    let (next, planned, report) = run(&b, &v);
    assert_eq!(xy(&next, "s-1"), (11.0, 0.0));
    let mut want = obj(&b, "s-1").clone();
    want.fields.insert("x".into(), Value::Num(11.0));
    want.fields.insert("y".into(), Value::Num(0.0));
    assert_eq!(obj(&next, "s-1"), &want);
    assert_eq!(report.changed, vec![id("s-1")], "nothing else moved");
    assert_inverse(&b, &planned.group);
}

#[test]
fn a_move_does_not_round() {
    let b = board(&[at("s-1", "sticky", 1.25, 0.0, 1.0, 1.0)]);
    let v = Verb::Move {
        ids: revs(&b, &["s-1"]),
        dx: 0.5,
        dy: 0.0,
    };
    let (next, _, _) = run(&b, &v);
    assert_eq!(xy(&next, "s-1").0, 1.75);
}

#[test]
fn group_members_come_along_and_a_locked_one_stays_and_is_named() {
    let b = board(&[
        "{\"group\":\"g-1\",\"id\":\"s-1\",\"type\":\"sticky\",\"x\":1,\"y\":1}".to_string(),
        "{\"group\":\"g-1\",\"id\":\"s-2\",\"type\":\"sticky\",\"x\":2,\"y\":2}".to_string(),
        "{\"group\":\"g-1\",\"id\":\"s-3\",\"locked\":true,\"type\":\"sticky\",\"x\":3,\"y\":3}"
            .to_string(),
        "{\"group\":\"g-2\",\"id\":\"s-4\",\"type\":\"sticky\",\"x\":4,\"y\":4}".to_string(),
    ]);
    let v = Verb::Move {
        ids: revs(&b, &["s-1"]),
        dx: 10.0,
        dy: 0.0,
    };
    assert_eq!(
        applies(&b, &v),
        Ok(()),
        "a locked member stays put, so the move applies"
    );
    let (next, planned, _) = run(&b, &v);
    assert_eq!(xy(&next, "s-2"), (12.0, 2.0), "a member came along");
    assert_eq!(xy(&next, "s-3"), (3.0, 3.0), "the locked one stayed");
    assert_eq!(xy(&next, "s-4"), (4.0, 4.0), "another group stayed");
    assert_eq!(planned.skipped, vec![id("s-3")]);
    assert_inverse(&b, &planned.group);
}

#[test]
fn a_frames_contents_come_along_by_field_or_by_sitting_inside() {
    let b = board(&[
        at("f-1", "frame", 0.0, 0.0, 100.0, 100.0),
        at("s-1", "sticky", 10.0, 10.0, 20.0, 20.0),
        at("s-2", "sticky", 90.0, 90.0, 20.0, 20.0),
        "{\"frame\":\"f-1\",\"h\":10,\"id\":\"s-3\",\"type\":\"sticky\",\"w\":10,\"x\":500,\"y\":500}".to_string(),
        "{\"frame\":\"f-9\",\"h\":10,\"id\":\"s-4\",\"type\":\"sticky\",\"w\":10,\"x\":10,\"y\":10}".to_string(),
        at("f-2", "frame", 1.0, 1.0, 5.0, 5.0),
        "{\"frame\":\"f-1\",\"from\":{\"x\":1,\"y\":1},\"id\":\"c-1\",\"to\":{\"x\":2,\"y\":2},\"type\":\"connector\",\"x\":1,\"y\":1}".to_string(),
    ]);
    let v = Verb::Move {
        ids: revs(&b, &["f-1"]),
        dx: 1.0,
        dy: 1.0,
    };
    let (next, planned, _) = run(&b, &v);
    assert_eq!(xy(&next, "f-1"), (1.0, 1.0));
    assert_eq!(xy(&next, "s-1"), (11.0, 11.0), "inside by geometry");
    assert_eq!(xy(&next, "s-2"), (90.0, 90.0), "only partly inside");
    assert_eq!(xy(&next, "s-3"), (501.0, 501.0), "its frame field says so");
    assert_eq!(
        xy(&next, "s-4"),
        (10.0, 10.0),
        "its field names another frame"
    );
    assert_eq!(
        xy(&next, "f-2"),
        (1.0, 1.0),
        "a frame doesn't carry a frame"
    );
    assert_eq!(xy(&next, "c-1"), (1.0, 1.0), "a connector follows its ends");
    assert_inverse(&b, &planned.group);
}

#[test]
fn a_named_connector_or_locked_object_refuses_a_move() {
    let b = board(&[
        "{\"id\":\"c-1\",\"type\":\"connector\"}".to_string(),
        "{\"id\":\"l-1\",\"locked\":true,\"type\":\"sticky\",\"x\":1}".to_string(),
    ]);
    let v = Verb::Move {
        ids: revs(&b, &["c-1"]),
        dx: 1.0,
        dy: 0.0,
    };
    assert_eq!(
        verb_code(plan_as(&b, Who::Me, 1, &v).expect_err("type")),
        "wrong_type"
    );
    let v = Verb::Move {
        ids: revs(&b, &["l-1"]),
        dx: 1.0,
        dy: 0.0,
    };
    let p = plan_as(&b, Who::Me, 1, &v).expect("plans");
    let e = apply_group(&mut b.clone(), &p.group).expect_err("locked");
    assert_eq!(refusal_of(e), RefusalCode::ObjectLocked);
}

#[test]
fn a_stale_revision_stops_a_whole_move() {
    let b = board(&[
        at("s-1", "sticky", 0.0, 0.0, 1.0, 1.0),
        at("s-2", "sticky", 0.0, 0.0, 1.0, 1.0),
    ]);
    let mut ids = revs(&b, &["s-1", "s-2"]);
    ids[1].rev = Sha256::of(b"old");
    let v = Verb::Move {
        ids,
        dx: 1.0,
        dy: 0.0,
    };
    let p = plan_as(&b, Who::Me, 1, &v).expect("plans");
    let mut next = b.clone();
    let e = apply_group(&mut next, &p.group).expect_err("stale");
    assert!(matches!(e, Conflict::Changed { .. }), "{e:?}");
    assert_eq!(next, b);
}

fn stack() -> Board {
    board(&[
        at("a", "sticky", 0.0, 0.0, 10.0, 10.0),
        at("b", "sticky", 100.0, 100.0, 10.0, 10.0),
        at("c", "sticky", 5.0, 5.0, 10.0, 10.0),
        at("d", "sticky", 200.0, 200.0, 10.0, 10.0),
        at("e", "sticky", 6.0, 6.0, 10.0, 10.0),
        at("f-1", "frame", 0.0, 0.0, 500.0, 500.0),
    ])
}

fn order_of(b: &Board, ids: &[&str], to: Layer) -> Vec<String> {
    let v = Verb::Order {
        ids: ids.iter().map(|i| id(i)).collect(),
        to,
    };
    let (next, planned, _) = run(b, &v);
    assert_inverse(b, &planned.group);
    order(&next).iter().map(|s| s.to_string()).collect()
}

#[test]
fn front_and_back_keep_their_order_among_themselves() {
    let b = stack();
    assert_eq!(
        order_of(&b, &["c", "a"], Layer::Front),
        ["b", "d", "e", "f-1", "a", "c"]
    );
    assert_eq!(
        order_of(&b, &["e", "c"], Layer::Back),
        ["c", "e", "a", "b", "d", "f-1"]
    );
}

#[test]
fn after_goes_directly_above_the_first_still_there() {
    let b = stack();
    assert_eq!(
        order_of(&b, &["a"], Layer::After(vec![id("nope"), id("d")])),
        ["b", "c", "d", "a", "e", "f-1"]
    );
}

#[test]
fn forward_steps_past_the_nearest_overlap_above() {
    let b = stack();
    assert_eq!(
        order_of(&b, &["a"], Layer::Forward),
        ["b", "c", "a", "d", "e", "f-1"]
    );
    assert_eq!(
        order_of(&b, &["c"], Layer::Forward),
        ["a", "b", "d", "e", "c", "f-1"]
    );
}

#[test]
fn forward_with_nothing_overlapping_above_stays_and_a_frame_does_not_count() {
    let b = stack();
    assert_eq!(
        order_of(&b, &["e"], Layer::Forward),
        ["a", "b", "c", "d", "e", "f-1"]
    );
    assert_eq!(
        order_of(&b, &["d"], Layer::Forward),
        ["a", "b", "c", "d", "e", "f-1"]
    );
}

#[test]
fn backward_steps_past_the_nearest_overlap_below() {
    let b = stack();
    assert_eq!(
        order_of(&b, &["e"], Layer::Backward),
        ["a", "b", "e", "c", "d", "f-1"]
    );
    assert_eq!(
        order_of(&b, &["c"], Layer::Backward),
        ["c", "a", "b", "d", "e", "f-1"]
    );
    assert_eq!(
        order_of(&b, &["a"], Layer::Backward),
        ["a", "b", "c", "d", "e", "f-1"]
    );
}

#[test]
fn several_objects_step_together() {
    let b = stack();
    assert_eq!(
        order_of(&b, &["a", "c"], Layer::Forward),
        ["b", "d", "e", "a", "c", "f-1"]
    );
}

#[test]
fn an_order_of_an_object_that_is_gone_conflicts_and_a_locked_one_refuses() {
    let b = stack();
    let v = Verb::Order {
        ids: vec![id("nope")],
        to: Layer::Front,
    };
    assert_eq!(
        plan_as(&b, Who::Me, 1, &v).expect_err("gone"),
        VerbError::Conflict(Conflict::Deleted { id: id("nope") })
    );
    let locked = board(&[
        "{\"id\":\"l\",\"locked\":true,\"type\":\"sticky\"}".to_string(),
        at("a", "sticky", 0.0, 0.0, 1.0, 1.0),
    ]);
    let v = Verb::Order {
        ids: vec![id("l")],
        to: Layer::Front,
    };
    let p = plan_as(&locked, Who::Me, 1, &v).expect("plans");
    let e = apply_group(&mut locked.clone(), &p.group).expect_err("locked");
    assert_eq!(refusal_of(e), RefusalCode::ObjectLocked);
}

#[test]
fn frame_order_moves_in_the_presentation_order() {
    let b = board(&[
        at("f-1", "frame", 0.0, 0.0, 1.0, 1.0),
        at("f-2", "frame", 0.0, 0.0, 1.0, 1.0),
    ]);
    let v = Verb::FrameOrder {
        id: id("f-1"),
        to: Position::After(vec![id("f-2")]),
    };
    let (next, planned, _) = run(&b, &v);
    assert_eq!(next.frames, vec![id("f-2"), id("f-1")]);
    assert_inverse(&b, &planned.group);
}

#[test]
fn a_group_shares_one_new_id_and_ungroup_takes_it_off() {
    let b = stack();
    let v = Verb::Group {
        ids: revs(&b, &["a", "b"]),
    };
    let (next, planned, _) = run(&b, &v);
    assert_inverse(&b, &planned.group);
    let g = get(obj(&next, "a"), "group").clone();
    assert_eq!(get(obj(&next, "b"), "group"), &g);
    let Value::Str(g) = g else {
        panic!("text");
    };
    assert!(g.starts_with("g-"), "{g}");
    let v = Verb::Ungroup {
        ids: revs(&next, &["a", "c"]),
    };
    let (after, planned, _) = run(&next, &v);
    assert_inverse(&next, &planned.group);
    assert!(!obj(&after, "a").fields.contains_key("group"));
    assert!(obj(&after, "b").fields.contains_key("group"), "not named");
    assert_eq!(planned.skipped, vec![id("c")], "in no group");
    let v = Verb::Group {
        ids: revs(&b, &["a"]),
    };
    assert_eq!(
        verb_code(plan_as(&b, Who::Me, 1, &v).expect_err("few")),
        "too_few"
    );
}

fn three() -> Board {
    board(&[
        at("a", "sticky", 0.0, 0.0, 10.0, 20.0),
        at("b", "sticky", 30.0, 40.0, 20.0, 10.0),
        at("c", "sticky", 100.0, 80.0, 30.0, 30.0),
    ])
}

fn aligned(to: AlignTo) -> Board {
    let b = three();
    let v = Verb::Align {
        ids: revs(&b, &["a", "b", "c"]),
        to,
    };
    let (next, planned, _) = run(&b, &v);
    assert_inverse(&b, &planned.group);
    next
}

#[test]
fn align_lines_up_every_edge_and_centre() {
    let n = aligned(AlignTo::Left);
    assert_eq!(
        (xy(&n, "a").0, xy(&n, "b").0, xy(&n, "c").0),
        (0.0, 0.0, 0.0)
    );
    let n = aligned(AlignTo::Right);
    assert_eq!(
        (xy(&n, "a").0, xy(&n, "b").0, xy(&n, "c").0),
        (120.0, 110.0, 100.0)
    );
    let n = aligned(AlignTo::Center);
    assert_eq!(
        (xy(&n, "a").0, xy(&n, "b").0, xy(&n, "c").0),
        (60.0, 55.0, 50.0)
    );
    let n = aligned(AlignTo::Top);
    assert_eq!(
        (xy(&n, "a").1, xy(&n, "b").1, xy(&n, "c").1),
        (0.0, 0.0, 0.0)
    );
    let n = aligned(AlignTo::Bottom);
    assert_eq!(
        (xy(&n, "a").1, xy(&n, "b").1, xy(&n, "c").1),
        (90.0, 100.0, 80.0)
    );
    let n = aligned(AlignTo::Middle);
    assert_eq!(
        (xy(&n, "a").1, xy(&n, "b").1, xy(&n, "c").1),
        (45.0, 50.0, 40.0)
    );
}

#[test]
fn align_moves_only_its_axis() {
    let n = aligned(AlignTo::Left);
    assert_eq!(xy(&n, "b").1, 40.0);
    let n = aligned(AlignTo::Top);
    assert_eq!(xy(&n, "b").0, 30.0);
}

#[test]
fn a_box_that_is_already_there_is_not_written() {
    let b = three();
    let v = Verb::Align {
        ids: revs(&b, &["a", "b"]),
        to: AlignTo::Left,
    };
    let planned = plan_as(&b, Who::Me, 1, &v).expect("plans");
    assert_eq!(
        planned.group.ops.len(),
        1,
        "only b moves: {:?}",
        planned.group.ops
    );
}

#[test]
fn a_box_with_defaults_and_a_pen_box_from_points_both_align() {
    let b = board(&[
        "{\"id\":\"s-1\",\"type\":\"sticky\",\"x\":0,\"y\":0}".to_string(),
        "{\"id\":\"p-9\",\"points\":[[10,10],[20,30]],\"type\":\"pen\",\"x\":300,\"y\":0}"
            .to_string(),
    ]);
    let v = Verb::Align {
        ids: revs(&b, &["s-1", "p-9"]),
        to: AlignTo::Right,
    };
    let (next, _, _) = run(&b, &v);
    // The sticky's box is 200 wide by default. The pen's box starts at 310 and is 10 wide.
    assert_eq!(xy(&next, "p-9").0, 300.0, "it's the rightmost");
    assert_eq!(xy(&next, "s-1").0, 120.0, "320 - 200");
    let v = Verb::Align {
        ids: revs(&b, &["s-1", "p-9"]),
        to: AlignTo::Left,
    };
    let (next, _, _) = run(&b, &v);
    assert_eq!(xy(&next, "p-9").0, -10.0, "its points start 10 in");
}

#[test]
fn align_refuses_too_few_and_a_connector() {
    let b = three();
    let v = Verb::Align {
        ids: revs(&b, &["a"]),
        to: AlignTo::Left,
    };
    assert_eq!(
        verb_code(plan_as(&b, Who::Me, 1, &v).expect_err("few")),
        "too_few"
    );
    let b = board(&[
        at("a", "sticky", 0.0, 0.0, 1.0, 1.0),
        "{\"id\":\"c\",\"type\":\"connector\"}".to_string(),
    ]);
    let v = Verb::Align {
        ids: revs(&b, &["a", "c"]),
        to: AlignTo::Left,
    };
    assert_eq!(
        verb_code(plan_as(&b, Who::Me, 1, &v).expect_err("type")),
        "wrong_type"
    );
}

#[test]
fn distribute_makes_even_gaps_on_its_axis() {
    let b = board(&[
        at("a", "sticky", 0.0, 7.0, 10.0, 10.0),
        at("b", "sticky", 12.0, 7.0, 20.0, 10.0),
        at("c", "sticky", 100.0, 7.0, 10.0, 10.0),
    ]);
    let v = Verb::Distribute {
        ids: revs(&b, &["c", "a", "b"]),
        axis: Axis::Horizontal,
    };
    let (next, planned, _) = run(&b, &v);
    assert_inverse(&b, &planned.group);
    assert_eq!(xy(&next, "a"), (0.0, 7.0));
    assert_eq!(xy(&next, "b"), (45.0, 7.0), "(110 - 40) / 2 = 35 gaps");
    assert_eq!(xy(&next, "c"), (100.0, 7.0));
    let b = board(&[
        at("a", "sticky", 7.0, 0.0, 10.0, 10.0),
        at("b", "sticky", 7.0, 12.0, 10.0, 20.0),
        at("c", "sticky", 7.0, 100.0, 10.0, 10.0),
    ]);
    let v = Verb::Distribute {
        ids: revs(&b, &["a", "b", "c"]),
        axis: Axis::Vertical,
    };
    let (next, _, _) = run(&b, &v);
    assert_eq!(xy(&next, "b"), (7.0, 45.0));
    let v = Verb::Distribute {
        ids: revs(&b, &["a", "b"]),
        axis: Axis::Vertical,
    };
    assert_eq!(
        verb_code(plan_as(&b, Who::Me, 1, &v).expect_err("few")),
        "too_few"
    );
}

#[test]
fn tidy_makes_rows_in_order_and_leaves_what_can_not_move() {
    let b = board(&[
        at("a", "sticky", 50.0, 3.0, 10.0, 10.0),
        at("b", "sticky", 0.0, 0.0, 20.0, 10.0),
        at("c", "sticky", 7.0, 100.0, 10.0, 30.0),
        at("f", "frame", 0.0, 0.0, 5.0, 5.0),
        "{\"id\":\"q\",\"type\":\"comment\"}".to_string(),
    ]);
    let v = Verb::Tidy {
        ids: revs(&b, &["a", "b", "c", "f", "q"]),
    };
    let (next, planned, _) = run(&b, &v);
    assert_inverse(&b, &planned.group);
    assert_eq!(xy(&next, "b"), (0.0, 0.0), "first in the first row");
    assert_eq!(xy(&next, "a"), (44.0, 0.0), "20 wide, then a gap of 24");
    assert_eq!(
        xy(&next, "c"),
        (0.0, 34.0),
        "the next row: 10 tall, then a gap of 24"
    );
    assert_eq!(planned.skipped, vec![id("f"), id("q")]);
    let v = Verb::Tidy {
        ids: revs(&b, &["a", "f"]),
    };
    assert_eq!(
        verb_code(plan_as(&b, Who::Me, 1, &v).expect_err("few")),
        "too_few"
    );
}

#[test]
fn tidy_orders_a_row_left_to_right_whatever_its_heights() {
    let b = board(&[
        at("l", "sticky", 0.0, 3.0, 10.0, 10.0),
        at("r", "sticky", 50.0, 0.0, 10.0, 10.0),
    ]);
    let v = Verb::Tidy {
        ids: revs(&b, &["r", "l"]),
    };
    let (next, _, _) = run(&b, &v);
    assert_eq!(xy(&next, "l").0, 0.0, "the left one stays first");
    assert_eq!(xy(&next, "r").0, 34.0, "10 wide, then a gap of 24");
}

#[test]
fn tidy_goes_by_the_drawn_box_of_a_rotated_object() {
    let b = board(&[
        "{\"h\":10,\"id\":\"a\",\"rot\":90,\"type\":\"sticky\",\"w\":30,\"x\":0,\"y\":10}"
            .to_string(),
        at("b", "sticky", 100.0, 0.0, 10.0, 30.0),
    ]);
    let v = Verb::Tidy {
        ids: revs(&b, &["a", "b"]),
    };
    let (next, _, _) = run(&b, &v);
    // Drawn, a is 10 wide and 30 tall, at (10, 0).
    assert_eq!(xy(&next, "b").0, 44.0, "10 + 10 wide + a gap of 24");
    assert_eq!(
        xy(&next, "a").0,
        0.0,
        "its stored x keeps its offset from what's drawn"
    );
}

#[test]
fn copy_style_carries_only_fields_of_the_same_kind() {
    let b = board(&[
        "{\"color\":\"pink\",\"fill\":\"red\",\"id\":\"h-1\",\"stroke\":\"blue\",\"strokeWidth\":5,\"text\":\"keep\",\"type\":\"shape\"}".to_string(),
        "{\"color\":\"yellow\",\"id\":\"s-1\",\"type\":\"sticky\"}".to_string(),
        "{\"id\":\"c-1\",\"type\":\"connector\"}".to_string(),
        "{\"id\":\"h-2\",\"text\":\"mine\",\"type\":\"shape\"}".to_string(),
    ]);
    let v = Verb::CopyStyle {
        from: id("h-1"),
        to: revs(&b, &["s-1", "c-1", "h-2"]),
    };
    let (next, planned, _) = run(&b, &v);
    assert_inverse(&b, &planned.group);
    assert_eq!(
        get(obj(&next, "s-1"), "color"),
        &text("yellow"),
        "a sticky's palette isn't an ink"
    );
    assert_eq!(planned.skipped, vec![id("s-1")]);
    let c = obj(&next, "c-1");
    assert_eq!(get(c, "stroke"), &text("blue"));
    assert_eq!(num(c, "strokeWidth"), 5.0);
    assert!(!c.fields.contains_key("fill"), "a connector has no fill");
    let h = obj(&next, "h-2");
    assert_eq!(get(h, "color"), &text("pink"));
    assert_eq!(get(h, "fill"), &text("red"));
    assert_eq!(get(h, "text"), &text("mine"), "the look, not the content");
}

#[test]
fn copy_style_leaves_a_look_that_is_already_the_same() {
    let b = board(&[
        "{\"color\":\"pink\",\"id\":\"h-1\",\"type\":\"shape\"}".to_string(),
        "{\"color\":\"pink\",\"id\":\"h-2\",\"type\":\"shape\"}".to_string(),
    ]);
    let v = Verb::CopyStyle {
        from: id("h-1"),
        to: revs(&b, &["h-2"]),
    };
    let planned = plan_as(&b, Who::Me, 1, &v).expect("plans");
    assert!(planned.group.ops.is_empty(), "{:?}", planned.group.ops);
    assert_eq!(planned.skipped, vec![id("h-2")]);
}

#[test]
fn duplicate_gives_fresh_ids_and_keeps_links_inside_the_set() {
    let b = board_of(
        "{\"created\":{\"at\":\"2020-01-01T00:00:00Z\",\"by\":\"p-2\"},\"group\":\"g-1\",\"id\":\"s-1\",\"reactions\":{\"y\":[\"p-2\"]},\"type\":\"sticky\",\"votes\":{\"p-2\":1},\"x\":1.4,\"y\":2},\
{\"group\":\"g-1\",\"id\":\"s-2\",\"type\":\"sticky\",\"x\":10,\"y\":2},\
{\"from\":{\"anchor\":\"top\",\"hint\":{\"x\":1,\"y\":1},\"id\":\"s-1\"},\"id\":\"c-1\",\"to\":{\"id\":\"s-2\"},\"type\":\"connector\"},\
{\"from\":{\"id\":\"s-1\"},\"id\":\"c-2\",\"to\":{\"id\":\"s-9\"},\"type\":\"connector\"},\
{\"h\":10,\"id\":\"s-9\",\"type\":\"sticky\",\"w\":20,\"x\":100,\"y\":100},\
{\"id\":\"m-1\",\"type\":\"mind\"},\
{\"id\":\"m-2\",\"parent\":\"m-1\",\"type\":\"mind\"},\
{\"id\":\"m-3\",\"parent\":\"s-9\",\"type\":\"mind\"},\
{\"id\":\"q-1\",\"target\":\"s-1\",\"type\":\"comment\"},\
{\"id\":\"q-2\",\"target\":\"s-9\",\"type\":\"comment\"}",
        "{}",
        "[]",
    );
    let v = Verb::Duplicate {
        ids: ["s-1", "s-2", "m-1", "m-2", "m-3", "q-1", "q-2"]
            .iter()
            .map(|i| id(i))
            .collect(),
        dx: 24.0,
        dy: 24.0,
    };
    let (next, planned, _) = run(&b, &v);
    assert_inverse(&b, &planned.group);
    next.validate(&Limits::V1).expect("valid");
    assert_eq!(
        planned.ids.len(),
        8,
        "the seven and the connector inside: {:?}",
        planned.ids
    );
    let copy = |nth: usize| obj(&next, planned.ids[nth].as_str());
    let (s1, s2, c1) = (copy(0), copy(1), copy(2));
    assert_eq!(
        (num(s1, "x"), num(s1, "y")),
        (25.0, 26.0),
        "placed whole, as a person's is"
    );
    assert!(!s1.fields.contains_key("reactions"));
    assert!(!s1.fields.contains_key("votes"));
    assert_eq!(
        get(s1, "created"),
        &parse(&format!("{{\"at\":\"{NOW}\",\"by\":\"{ME}\"}}")),
        "the copy is mine, now"
    );
    assert_eq!(get(s1, "group"), get(s2, "group"));
    assert_ne!(get(s1, "group"), &text("g-1"), "a new group");
    assert_eq!(c1.kind, ObjectType::Connector);
    assert_eq!(
        get(c1, "from"),
        &parse(&format!("{{\"anchor\":\"top\",\"id\":\"{}\"}}", s1.id)),
        "follows, with its hint gone"
    );
    assert_eq!(get(c1, "to"), &parse(&format!("{{\"id\":\"{}\"}}", s2.id)));
    assert!(!has(&next, planned.ids[7].as_str()) || copy(7).kind != ObjectType::Connector);
    let (m1, m2, m3) = (copy(3), copy(4), copy(5));
    assert_eq!(get(m2, "parent"), &text(m1.id.as_str()));
    assert!(
        !m3.fields.contains_key("parent"),
        "its parent isn't in the set"
    );
    assert_eq!(get(copy(6), "target"), &text(s1.id.as_str()));
    assert!(
        !copy(7).fields.contains_key("target"),
        "its target isn't in the set"
    );
    assert_eq!(order(&next).len(), b.objects.len() + 8);
    let _ = c1;
    let originals = order(&b);
    assert_eq!(
        &order(&next)[..originals.len()],
        &originals[..],
        "copies go on top"
    );
}

#[test]
fn a_named_connector_with_an_end_outside_gets_a_free_point() {
    let b = board(&[
        at("s-1", "sticky", 0.0, 0.0, 10.0, 10.0),
        at("s-2", "sticky", 100.0, 100.0, 20.0, 10.0),
        "{\"from\":{\"id\":\"s-1\"},\"id\":\"c-1\",\"to\":{\"id\":\"s-2\"},\"type\":\"connector\"}".to_string(),
        "{\"from\":{\"id\":\"s-1\"},\"id\":\"c-2\",\"to\":{\"x\":1,\"y\":2},\"type\":\"connector\"}".to_string(),
        "{\"from\":{\"id\":\"gone\"},\"id\":\"c-3\",\"to\":{\"id\":\"s-1\"},\"type\":\"connector\"}".to_string(),
    ]);
    let v = Verb::Duplicate {
        ids: vec![id("s-1"), id("c-1"), id("c-2"), id("c-3")],
        dx: 5.0,
        dy: 5.0,
    };
    let (next, planned, _) = run(&b, &v);
    let s1 = planned.ids[0].clone();
    let c1 = obj(&next, planned.ids[1].as_str());
    assert_eq!(get(c1, "from"), &parse(&format!("{{\"id\":\"{s1}\"}}")));
    assert_eq!(
        get(c1, "to"),
        &parse("{\"x\":115,\"y\":110}"),
        "its object's centre plus the offset"
    );
    let c2 = obj(&next, planned.ids[2].as_str());
    assert_eq!(
        get(c2, "to"),
        &parse("{\"x\":6,\"y\":7}"),
        "a free end moves by the offset"
    );
    let c3 = obj(&next, planned.ids[3].as_str());
    assert_eq!(
        get(c3, "from"),
        &parse("{\"x\":0,\"y\":0}"),
        "its object is gone"
    );
}

#[test]
fn a_connector_keeps_x_and_y_in_a_copy() {
    let b = board(&[
        "{\"from\":{\"x\":1,\"y\":1},\"id\":\"c-1\",\"to\":{\"x\":2,\"y\":2},\"type\":\"connector\",\"x\":3}".to_string(),
    ]);
    let v = Verb::Duplicate {
        ids: vec![id("c-1")],
        dx: 5.0,
        dy: 5.0,
    };
    let (next, planned, _) = run(&b, &v);
    let c = obj(&next, planned.ids[0].as_str());
    assert_eq!(num(c, "x"), 3.0, "a connector's ends place it");
}

#[test]
fn a_member_of_a_frame_in_the_set_follows_the_copy() {
    let b = board(&[
        at("f-1", "frame", 0.0, 0.0, 100.0, 100.0),
        "{\"frame\":\"f-1\",\"id\":\"s-1\",\"type\":\"sticky\",\"x\":1,\"y\":1}".to_string(),
    ]);
    let v = Verb::Duplicate {
        ids: vec![id("f-1"), id("s-1")],
        dx: 1.0,
        dy: 1.0,
    };
    let (next, planned, _) = run(&b, &v);
    assert_eq!(
        get(obj(&next, planned.ids[1].as_str()), "frame"),
        &text(planned.ids[0].as_str())
    );
    next.validate(&Limits::V1).expect("valid");
}

#[test]
fn paste_centres_objects_on_the_point() {
    let source = board(&[
        at("a", "sticky", 0.0, 0.0, 10.0, 10.0),
        at("b", "sticky", 30.0, 30.0, 10.0, 10.0),
        "{\"from\":{\"id\":\"a\"},\"id\":\"c\",\"to\":{\"id\":\"b\"},\"type\":\"connector\"}"
            .to_string(),
    ]);
    let target = board(&[at("z", "sticky", 0.0, 0.0, 1.0, 1.0)]);
    let v = Verb::Paste {
        objects: source.objects.clone(),
        x: 100.0,
        y: 200.0,
    };
    let (next, planned, _) = run(&target, &v);
    assert_inverse(&target, &planned.group);
    assert_eq!(xy(&next, planned.ids[0].as_str()), (80.0, 180.0));
    assert_eq!(xy(&next, planned.ids[1].as_str()), (110.0, 210.0));
    let c = obj(&next, planned.ids[2].as_str());
    assert_eq!(
        get(c, "from"),
        &parse(&format!("{{\"id\":\"{}\"}}", planned.ids[0]))
    );
    let v = Verb::Paste {
        objects: Vec::new(),
        x: 0.0,
        y: 0.0,
    };
    assert_eq!(
        verb_code(plan_as(&target, Who::Me, 1, &v).expect_err("empty")),
        "too_few"
    );
}

#[test]
fn a_connected_copy_is_empty_on_its_side_and_joined() {
    let b = board_of(
        "{\"assignee\":\"Bo\",\"checks\":[{\"done\":false,\"id\":\"c\",\"text\":\"t\"}],\"color\":\"red\",\"h\":100,\"id\":\"k-1\",\"later\":1,\"link\":\"https://a.example\",\"tags\":[\"t\"],\"text\":\"t\",\"title\":\"T\",\"type\":\"card\",\"votes\":{\"p-2\":1},\"w\":200,\"x\":0,\"y\":0}",
        "{}",
        "[]",
    );
    let v = Verb::ConnectedCopy {
        id: id("k-1"),
        side: Side::Right,
    };
    let (next, planned, _) = run(&b, &v);
    assert_inverse(&b, &planned.group);
    assert_eq!(planned.ids.len(), 2);
    let copy = obj(&next, planned.ids[0].as_str());
    assert_eq!(copy.kind, ObjectType::Card);
    assert_eq!(
        (num(copy, "x"), num(copy, "y")),
        (280.0, 0.0),
        "200 wide, then a gap of 80"
    );
    assert_eq!(get(copy, "title"), &text(""));
    assert_eq!(get(copy, "text"), &text(""));
    assert_eq!(get(copy, "assignee"), &text(""));
    assert_eq!(get(copy, "color"), &text("red"), "the look stays");
    for gone in ["checks", "link", "tags", "votes", "later"] {
        assert!(!copy.fields.contains_key(gone), "{gone} came along");
    }
    let line = obj(&next, planned.ids[1].as_str());
    assert_eq!(
        get(line, "from"),
        &parse("{\"anchor\":\"right\",\"id\":\"k-1\"}")
    );
    assert_eq!(
        get(line, "to"),
        &parse(&format!("{{\"anchor\":\"left\",\"id\":\"{}\"}}", copy.id))
    );
}

#[test]
fn a_connected_copy_steps_past_anything_in_the_way_on_each_side() {
    let base = |extra: &str| {
        board(&[
            "{\"h\":100,\"id\":\"s-1\",\"type\":\"sticky\",\"w\":100,\"x\":0,\"y\":0}".to_string(),
            extra.to_string(),
        ])
    };
    let cases = [
        (
            Side::Right,
            at("z", "sticky", 180.0, 0.0, 10.0, 10.0),
            (360.0, 0.0),
        ),
        (
            Side::Left,
            at("z", "sticky", -180.0, 0.0, 10.0, 10.0),
            (-360.0, 0.0),
        ),
        (
            Side::Bottom,
            at("z", "sticky", 0.0, 180.0, 10.0, 10.0),
            (0.0, 360.0),
        ),
        (
            Side::Top,
            at("z", "sticky", 0.0, -180.0, 10.0, 10.0),
            (0.0, -360.0),
        ),
    ];
    for (side, extra, want) in cases {
        let b = base(&extra);
        let (next, planned, _) = run(
            &b,
            &Verb::ConnectedCopy {
                id: id("s-1"),
                side,
            },
        );
        assert_eq!(xy(&next, planned.ids[0].as_str()), want, "{side:?}");
    }
    let frame_is_no_block = base(&at("f", "frame", 180.0, 0.0, 10.0, 10.0));
    let (next, planned, _) = run(
        &frame_is_no_block,
        &Verb::ConnectedCopy {
            id: id("s-1"),
            side: Side::Right,
        },
    );
    assert_eq!(xy(&next, planned.ids[0].as_str()), (180.0, 0.0));
}

#[test]
fn a_connected_copy_refuses_other_types_and_a_locked_object() {
    let b = board(&[
        at("t-1", "text", 0.0, 0.0, 10.0, 10.0),
        "{\"id\":\"s-1\",\"locked\":true,\"type\":\"sticky\"}".to_string(),
    ]);
    let v = Verb::ConnectedCopy {
        id: id("t-1"),
        side: Side::Top,
    };
    assert_eq!(
        verb_code(plan_as(&b, Who::Me, 1, &v).expect_err("type")),
        "wrong_type"
    );
    let v = Verb::ConnectedCopy {
        id: id("s-1"),
        side: Side::Top,
    };
    let Err(VerbError::Conflict(c)) = plan_as(&b, Who::Me, 1, &v) else {
        panic!("wanted a refusal");
    };
    assert_eq!(refusal_of(c), RefusalCode::ObjectLocked);
}

fn mind() -> Board {
    board_of(
        "{\"color\":\"violet\",\"h\":60,\"id\":\"m-1\",\"text\":\"root\",\"type\":\"mind\",\"w\":200,\"x\":400,\"y\":100},\
{\"color\":\"blue\",\"h\":48,\"id\":\"m-2\",\"parent\":\"m-1\",\"type\":\"mind\",\"w\":160,\"x\":100,\"y\":50},\
{\"color\":\"green\",\"h\":48,\"id\":\"m-3\",\"parent\":\"m-1\",\"type\":\"mind\",\"w\":160,\"x\":700,\"y\":80},\
{\"h\":10,\"id\":\"m-4\",\"type\":\"mind\",\"w\":10,\"x\":0,\"y\":0},\
{\"id\":\"s-1\",\"type\":\"sticky\"}",
        "{}",
        "[]",
    )
}

fn child(b: &Board, parent: &str) -> (Board, Object) {
    let v = Verb::MindChild {
        parent: id(parent),
        text: "new".into(),
    };
    let (next, planned, _) = run(b, &v);
    assert_inverse(b, &planned.group);
    let o = obj(&next, planned.ids[0].as_str()).clone();
    (next, o)
}

#[test]
fn a_mind_child_sits_beside_its_parent_on_the_lighter_side() {
    let b = mind();
    let (_, o) = child(&b, "m-1");
    assert_eq!(get(&o, "parent"), &text("m-1"));
    assert_eq!(get(&o, "text"), &text("new"));
    // One child on each side: not fewer on the left, so right.
    assert_eq!(num(&o, "x"), 680.0, "400 + 200 + 80");
    assert_eq!(
        num(&o, "y"),
        148.0,
        "below its lowest sibling: 80 + 48 + 20"
    );
    assert_eq!(
        get(&o, "color"),
        &text("orange"),
        "the third colour for the third child"
    );
}

#[test]
fn a_deeper_child_stays_on_its_parents_side_and_takes_its_colour() {
    let b = mind();
    let (_, left) = child(&b, "m-2");
    assert_eq!(
        num(&left, "x"),
        -140.0,
        "100 - 240: m-2 is left of the root"
    );
    assert_eq!(num(&left, "y"), 50.0, "beside it: 50 + 24 - 24");
    assert_eq!(
        left.fields.get("color"),
        Some(&text("blue")),
        "a deeper child takes its parent's colour"
    );
    let (_, right) = child(&b, "m-3");
    assert_eq!(num(&right, "x"), 940.0, "700 + 160 + 80");
    assert_eq!(get(&right, "color"), &text("green"));
}

#[test]
fn the_first_child_of_a_root_goes_right_with_the_first_colour() {
    let b = mind();
    let (_, o) = child(&b, "m-4");
    assert_eq!(num(&o, "x"), 90.0, "0 + 10 + 80");
    assert_eq!(num(&o, "y"), -19.0, "0 + 5 - 24");
    assert_eq!(get(&o, "color"), &text("blue"));
}

#[test]
fn a_mind_child_goes_left_when_more_of_them_sit_right() {
    let b = board_of(
        "{\"h\":10,\"id\":\"m-1\",\"type\":\"mind\",\"w\":10,\"x\":0,\"y\":0},\
{\"h\":10,\"id\":\"m-2\",\"parent\":\"m-1\",\"type\":\"mind\",\"w\":10,\"x\":90,\"y\":0}",
        "{}",
        "[]",
    );
    let (_, o) = child(&b, "m-1");
    assert_eq!(num(&o, "x"), -240.0);
}

#[test]
fn a_mind_child_needs_a_mind_parent_that_is_there() {
    let b = mind();
    let v = Verb::MindChild {
        parent: id("s-1"),
        text: "x".into(),
    };
    assert_eq!(
        verb_code(plan_as(&b, Who::Me, 1, &v).expect_err("type")),
        "wrong_type"
    );
    let v = Verb::MindChild {
        parent: id("nope"),
        text: "x".into(),
    };
    assert_eq!(
        plan_as(&b, Who::Me, 1, &v).expect_err("gone"),
        VerbError::Conflict(Conflict::Deleted { id: id("nope") })
    );
}

#[test]
fn a_parent_loop_does_not_hang_a_mind_child() {
    let b = board_of(
        "{\"h\":10,\"id\":\"m-1\",\"parent\":\"m-2\",\"type\":\"mind\",\"w\":10,\"x\":0,\"y\":0},\
{\"h\":10,\"id\":\"m-2\",\"parent\":\"m-1\",\"type\":\"mind\",\"w\":10,\"x\":50,\"y\":0}",
        "{}",
        "[]",
    );
    // With no stop, this would never come back: the assert below says so.
    let (done, wait) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let (_, o) = child(&b, "m-1");
        done.send(o).expect("the test is waiting");
    });
    let o = wait.recv_timeout(std::time::Duration::from_secs(120)).ok();
    assert!(o.is_some(), "a parent loop never ended");
    assert_eq!(get(&o.expect("it came back"), "parent"), &text("m-1"));
}

fn empty() -> Board {
    board_of("", "{}", "[]")
}

fn template(name: &str) -> (Board, verbs::Planned) {
    let b = empty();
    let v = Verb::Template {
        name: name.to_string(),
        x: 1000.0,
        y: 2000.0,
    };
    let (next, planned, _) = run(&b, &v);
    (next, planned)
}

#[test]
fn every_template_makes_a_valid_board_placed_at_its_point() {
    let want = [
        ("kickoff", 39),
        ("kanban", 21),
        ("retro", 18),
        ("flowchart", 13),
        ("storymap", 20),
        ("brainstorm", 8),
        ("mindmap", 6),
    ];
    assert_eq!(verbs::TEMPLATES.len(), want.len());
    for (name, count) in want {
        assert!(
            verbs::TEMPLATES.iter().any(|(n, _)| *n == name),
            "{name} isn't listed"
        );
        let (next, planned) = template(name);
        next.validate(&Limits::V1)
            .unwrap_or_else(|r| panic!("{name}: {r}"));
        assert_eq!(next.objects.len(), count, "{name}");
        assert_eq!(planned.ids.len(), count, "{name}");
        assert_eq!(
            next.objects[0].kind,
            ObjectType::Frame,
            "{name} starts with its frame"
        );
        assert_eq!(
            xy(&next, next.objects[0].id.as_str()),
            (1000.0, 2000.0),
            "{name}"
        );
        assert_inverse(&empty(), &planned.group);
        for r in next.references() {
            assert_eq!(r.state, RefState::Attached, "{name}: {r:?}");
        }
    }
}

#[test]
fn templates_hold_what_a_person_gets() {
    let (next, _) = template("flowchart");
    let connectors = next
        .objects
        .iter()
        .filter(|o| o.kind == ObjectType::Connector)
        .count();
    assert_eq!(connectors, 5);
    let yes = next
        .objects
        .iter()
        .find(|o| o.fields.get("label") == Some(&text("Yes")))
        .expect("a Yes line");
    let Value::Obj(from) = get(yes, "from") else {
        panic!("an end");
    };
    assert_eq!(from["anchor"], text("bottom"));

    let (next, _) = template("mindmap");
    let kids = next
        .objects
        .iter()
        .filter(|o| o.fields.contains_key("parent"))
        .count();
    assert_eq!(kids, 4);

    let (next, _) = template("kanban");
    let first = next
        .objects
        .iter()
        .find(|o| o.kind == ObjectType::Card)
        .expect("a card");
    assert_eq!(get(first, "title"), &text("Plan the next sprint"));
    assert_eq!(get(first, "assignee"), &text("You"));

    let (next, _) = template("brainstorm");
    let top = next
        .objects
        .iter()
        .find(|o| o.kind == ObjectType::Sticky)
        .expect("a sticky");
    assert_eq!(
        xy(&next, top.id.as_str()),
        (1420.0, 2040.0),
        "one decimal, straight above"
    );
    let second = next
        .objects
        .iter()
        .filter(|o| o.kind == ObjectType::Sticky)
        .nth(1)
        .expect("a second sticky");
    assert_eq!(
        xy(&next, second.id.as_str()),
        (1714.4, 2155.0),
        "a position worked out with sines keeps one decimal"
    );

    let (next, _) = template("storymap");
    let line = next
        .objects
        .iter()
        .find(|o| o.fields.get("shape") == Some(&text("rect")))
        .expect("the line");
    assert_eq!(num(line, "w"), 992.0, "4 * 240 + 32");

    let (next, _) = template("kickoff");
    let star = next
        .objects
        .iter()
        .find(|o| o.fields.get("shape") == Some(&text("star")))
        .expect("the last milestone");
    assert_eq!(get(star, "text"), &text("Launch"));
}

#[test]
fn an_unknown_template_is_refused() {
    let v = Verb::Template {
        name: "nope".into(),
        x: 0.0,
        y: 0.0,
    };
    let e = plan_as(&empty(), Who::Me, 1, &v).expect_err("refused");
    assert_eq!(verb_code(e), "no_template");
}

#[test]
fn the_inverse_conflicts_if_someone_changed_it_since() {
    let b = three();
    let v = Verb::Move {
        ids: revs(&b, &["a"]),
        dx: 1.0,
        dy: 0.0,
    };
    let (mut next, _, _) = run(&b, &v);
    let back = verbs::inverse(&b, &next, "undo").expect("an inverse");
    let later = Verb::Update {
        id: id("a"),
        rev: rev(&next, "a"),
        set: Fields::from([("text".to_string(), text("later"))]),
        unset: Vec::new(),
    };
    let p = plan_as(&next, Who::Me, 7, &later).expect("plans");
    apply_group(&mut next, &p.group).expect("it applies");
    let before = next.clone();
    let e = apply_group(&mut next, &back).expect_err("someone changed it");
    assert!(matches!(e, Conflict::Changed { .. }), "{e:?}");
    assert_eq!(next, before);
}

#[test]
fn the_inverse_of_nothing_is_nothing() {
    let b = three();
    let g = verbs::inverse(&b, &b, "undo").expect("an inverse");
    assert!(g.ops.is_empty(), "{:?}", g.ops);
    assert_eq!(g.id, "undo");
}

#[test]
fn the_inverse_restores_a_lock_taken_off_and_one_put_on() {
    let b = board(&[
        "{\"id\":\"l\",\"locked\":true,\"text\":\"t\",\"type\":\"sticky\"}".to_string(),
        at("a", "sticky", 0.0, 0.0, 1.0, 1.0),
    ]);
    let unlock_and_edit = Group {
        id: "g".into(),
        by: None,
        ops: vec![
            Op::Set {
                id: id("l"),
                rev: rev(&b, "l"),
                set: Fields::from([("locked".to_string(), Value::Bool(false))]),
                unset: Vec::new(),
            },
            Op::Set {
                id: id("l"),
                rev: rev(&b, "l"),
                set: Fields::from([("text".to_string(), text("edited"))]),
                unset: Vec::new(),
            },
            Op::Z {
                id: id("l"),
                to: Position::Front,
            },
        ],
    };
    assert_inverse(&b, &unlock_and_edit);
    let edit_and_lock = Group {
        id: "g".into(),
        by: None,
        ops: vec![
            Op::Z {
                id: id("a"),
                to: Position::Back,
            },
            Op::Set {
                id: id("a"),
                rev: rev(&b, "a"),
                set: Fields::from([
                    ("locked".to_string(), Value::Bool(true)),
                    ("text".to_string(), text("x")),
                ]),
                unset: Vec::new(),
            },
        ],
    };
    assert_inverse(&b, &edit_and_lock);
}

#[test]
fn the_inverse_restores_a_locked_frame_and_its_place() {
    let b = board(&[
        "{\"id\":\"f-1\",\"locked\":true,\"type\":\"frame\"}".to_string(),
        at("f-2", "frame", 0.0, 0.0, 1.0, 1.0),
    ]);
    let g = Group {
        id: "g".into(),
        by: None,
        ops: vec![
            Op::Set {
                id: id("f-1"),
                rev: rev(&b, "f-1"),
                set: Fields::from([("locked".to_string(), Value::Bool(false))]),
                unset: Vec::new(),
            },
            Op::Del {
                id: id("f-1"),
                rev: rev(&b, "f-1"),
            },
        ],
    };
    assert_inverse(&b, &g);
}

#[test]
fn the_inverse_takes_out_a_locked_object_a_group_added() {
    let b = three();
    let g = Group {
        id: "g".into(),
        by: None,
        ops: vec![Op::Add {
            object: Object {
                id: id("l"),
                kind: ObjectType::Sticky,
                fields: Fields::from([("locked".to_string(), Value::Bool(true))]),
            },
            at: Position::Back,
        }],
    };
    assert_inverse(&b, &g);
}
