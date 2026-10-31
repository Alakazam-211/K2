//! MovableList and Map on Yrs: each op, the applied stream, `winner`, the forward-chain rule,
//! concurrency through real merges (kit-made bytes only), the stored state of overwrites,
//! the merged-update stream (a keyed diff) against the local one, and planted negatives
//! (mutant layouts that the checks catch). The pinned Yrs deviations of the CSV seam design
//! §5.3 are conformance rules (`crates/conformance`).

use std::collections::BTreeMap;

use crate::doc::{Doc, DocError, Edit};
use crate::elem::{
    chain, reorder, Anchor, Applied, Effect, ElemState, Facet, Key, ListApplied, ListOp,
    ListOpKind, MapApplied, MapOp, OpRefusal, Value, WriteId,
};
use crate::schema::{Kind, Schema};
use crate::text::diff;

const S: Schema = Schema {
    name: "grid",
    roots: &[
        ("rows", Kind::MovableList),
        ("cells", Kind::Map),
        ("notes", Kind::Text),
    ],
};

fn k(s: &str) -> Key {
    Key::new(s).unwrap()
}

fn after(s: &str) -> Anchor {
    Anchor::After(k(s))
}

fn before(s: &str) -> Anchor {
    Anchor::Before(k(s))
}

fn ins(key: &str, at: Anchor) -> ListOp {
    ListOp::Insert {
        key: k(key),
        at,
        value: Value::Null,
    }
}

fn ins_v(key: &str, at: Anchor, v: &str) -> ListOp {
    ListOp::Insert {
        key: k(key),
        at,
        value: Value::str(v),
    }
}

fn mv(key: &str, to: Anchor) -> ListOp {
    ListOp::Move { key: k(key), to }
}

fn del(key: &str) -> ListOp {
    ListOp::Delete { key: k(key) }
}

fn res(key: &str) -> ListOp {
    ListOp::Restore { key: k(key) }
}

fn set(key: &str, v: &str) -> ListOp {
    ListOp::Set {
        key: k(key),
        value: Value::str(v),
    }
}

fn mset(key: &str, v: &str) -> MapOp {
    MapOp::Set {
        key: key.into(),
        value: Value::str(v),
    }
}

/// Apply list ops to `rows` as `client`; the ops as applied.
fn rows(d: &mut Doc, client: u64, ops: Vec<ListOp>) -> Vec<ListApplied> {
    let e = d
        .edit_applied(
            client,
            &[Edit::List {
                root: "rows".into(),
                ops,
            }],
        )
        .unwrap();
    match e.applied.into_iter().next() {
        Some(Applied::List { ops, .. }) => ops,
        other => panic!("{other:?}"),
    }
}

fn cells(d: &mut Doc, client: u64, ops: Vec<MapOp>) -> Vec<MapApplied> {
    let e = d
        .edit_applied(
            client,
            &[Edit::Map {
                root: "cells".into(),
                ops,
            }],
        )
        .unwrap();
    match e.applied.into_iter().next() {
        Some(Applied::Map { ops, .. }) => ops,
        other => panic!("{other:?}"),
    }
}

/// The live keys, space-separated.
fn order(d: &Doc) -> String {
    d.list("rows")
        .unwrap()
        .iter()
        .map(|(k, _)| k.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

fn doc_with(keys: &str) -> Doc {
    let mut d = Doc::new(S).unwrap();
    let mut at = Anchor::Start;
    let mut ops = Vec::new();
    for key in keys.split(' ') {
        ops.push(ins(key, at));
        at = after(key);
    }
    rows(&mut d, 50, ops);
    d
}

/// A replica of `d` (its state, loaded), as a participant's device would hold it.
fn replica(d: &Doc) -> Doc {
    Doc::load(S, &d.encode_state().unwrap()).unwrap()
}

/// Full sync of every doc with every other.
fn sync_all(docs: &mut [&mut Doc]) {
    for i in 0..docs.len() {
        for j in 0..docs.len() {
            if i != j {
                let (a, b) = if i < j {
                    let (x, y) = docs.split_at_mut(j);
                    (&mut *x[i], &*y[0])
                } else {
                    let (x, y) = docs.split_at_mut(i);
                    (&mut *y[0], &*x[j])
                };
                a.merge_from(b).unwrap();
            }
        }
    }
    let first = order(docs[0]);
    for d in docs.iter() {
        assert_eq!(order(d), first, "replicas converge");
        assert_eq!(d.map("cells").unwrap(), docs[0].map("cells").unwrap());
    }
}

fn refusal(r: Result<impl std::fmt::Debug, DocError>) -> OpRefusal {
    match r {
        Err(DocError::Op { refusal, .. }) => refusal,
        other => panic!("expected a refused op, got {other:?}"),
    }
}

// ---- each op ----

#[test]
fn insert_at_every_anchor() {
    let mut d = doc_with("a b c");
    let got = rows(
        &mut d,
        7,
        vec![
            ins("s", Anchor::Start),
            ins("e", Anchor::End),
            ins("x", after("a")),
            ins_v("y", before("c"), "why"),
        ],
    );
    assert_eq!(order(&d), "s a x b y c e");
    let idx: Vec<_> = got
        .iter()
        .map(|o| (o.index_before, o.index_after))
        .collect();
    // indices as each op left the order: s at 0; e at 4 of [s a b c e]; x at 2; y at 4
    assert_eq!(
        idx,
        vec![
            (None, Some(0)),
            (None, Some(4)),
            (None, Some(2)),
            (None, Some(4))
        ]
    );
    assert!(got.iter().all(|o| o.kind == ListOpKind::Insert
        && o.effect == Effect::Visible
        && o.value_before.is_none()));
    assert_eq!(got[3].value_after, Some(Value::str("why")));
    assert_eq!(
        d.elem_value("rows", &k("y")).unwrap(),
        Some(Value::str("why"))
    );
    assert_eq!(d.elem_value("rows", &k("x")).unwrap(), Some(Value::Null));
    assert_eq!(d.elem("rows", &k("y")).unwrap(), ElemState::Live(4));
    assert_eq!(d.elem("rows", &k("zz")).unwrap(), ElemState::Unknown);
    assert_eq!(d.elem_value("rows", &k("zz")).unwrap(), None);
}

#[test]
fn keys_are_never_reused_live_or_deleted() {
    let mut d = doc_with("a b");
    assert_eq!(
        refusal(d.edit_applied(
            7,
            &[Edit::List {
                root: "rows".into(),
                ops: vec![ins("a", Anchor::End)]
            }]
        )),
        OpRefusal::KeyExists("a".into())
    );
    rows(&mut d, 7, vec![del("b")]);
    assert_eq!(
        refusal(d.edit_applied(
            7,
            &[Edit::List {
                root: "rows".into(),
                ops: vec![ins("b", Anchor::End)]
            }]
        )),
        OpRefusal::KeyExists("b".into())
    );
    // inserted earlier in the same change counts too
    assert_eq!(
        refusal(d.edit_applied(
            7,
            &[Edit::List {
                root: "rows".into(),
                ops: vec![ins("n", Anchor::End), ins("n", Anchor::Start)]
            }]
        )),
        OpRefusal::KeyExists("n".into())
    );
}

#[test]
fn a_refused_op_refuses_the_whole_change() {
    let mut d = doc_with("a b c");
    let state = d.encode_state().unwrap();
    let r = d.edit_applied(
        7,
        &[
            Edit::Text {
                root: "notes".into(),
                hunks: diff::hunks("", "hello"),
            },
            Edit::Map {
                root: "cells".into(),
                ops: vec![mset("a\x1fc1", "v")],
            },
            Edit::List {
                root: "rows".into(),
                ops: vec![mv("a", Anchor::End), mv("ghost", Anchor::Start)],
            },
        ],
    );
    assert_eq!(
        r,
        Err(DocError::Op {
            root: "rows".into(),
            index: 1,
            refusal: OpRefusal::UnknownKey("ghost".into())
        })
    );
    assert_eq!(d.encode_state().unwrap(), state, "nothing applied");
    assert_eq!(order(&d), "a b c");
    assert_eq!(d.text("notes").unwrap(), "");
    for bad in [
        vec![mv("a", after("a"))],
        vec![mv("a", before("a"))],
        vec![ins("x", after("ghost"))],
        vec![del("ghost")],
        vec![res("ghost")],
        vec![set("ghost", "v")],
    ] {
        assert!(
            matches!(
                d.edit_applied(
                    7,
                    &[Edit::List {
                        root: "rows".into(),
                        ops: bad.clone()
                    }]
                ),
                Err(DocError::Op { .. })
            ),
            "{bad:?}"
        );
    }
    assert_eq!(
        refusal(d.edit_applied(
            7,
            &[Edit::List {
                root: "rows".into(),
                ops: vec![mv("a", after("a"))]
            }]
        )),
        OpRefusal::SelfAnchor("a".into())
    );
    for bad in ["a\0b".to_owned(), "x".repeat(257)] {
        assert!(matches!(
            refusal(d.edit_applied(
                7,
                &[Edit::Map {
                    root: "cells".into(),
                    ops: vec![mset(&bad, "v")]
                }]
            )),
            OpRefusal::BadMapKey(_)
        ));
    }
    assert_eq!(d.encode_state().unwrap(), state);
}

#[test]
fn edits_must_match_the_root_kind() {
    let mut d = Doc::new(S).unwrap();
    let wrong = [
        Edit::List {
            root: "cells".into(),
            ops: vec![],
        },
        Edit::Map {
            root: "rows".into(),
            ops: vec![],
        },
        Edit::Text {
            root: "rows".into(),
            hunks: vec![],
        },
    ];
    for e in wrong {
        assert!(
            matches!(
                d.edit_applied(7, std::slice::from_ref(&e)),
                Err(DocError::WrongKind { .. })
            ),
            "{e:?}"
        );
    }
    assert!(matches!(
        d.edit_applied(
            7,
            &[Edit::List {
                root: "nope".into(),
                ops: vec![]
            }]
        ),
        Err(DocError::NoRoot { .. })
    ));
    assert!(matches!(d.list("cells"), Err(DocError::WrongKind { .. })));
    assert!(matches!(d.map("rows"), Err(DocError::WrongKind { .. })));
    assert!(matches!(d.text("rows"), Err(DocError::WrongKind { .. })));
    assert!(matches!(
        d.winner("notes", "x", Facet::Value),
        Err(DocError::WrongKind { .. })
    ));
}

#[test]
fn move_forward_back_and_in_place() {
    let mut d = doc_with("a b c d");
    let got = rows(&mut d, 7, vec![mv("a", after("c"))]);
    assert_eq!(order(&d), "b c a d");
    assert_eq!(
        (got[0].index_before, got[0].index_after),
        (Some(0), Some(2))
    );
    let got = rows(&mut d, 7, vec![mv("d", Anchor::Start)]);
    assert_eq!(order(&d), "d b c a");
    assert_eq!(
        (got[0].index_before, got[0].index_after),
        (Some(3), Some(0))
    );
    // a move to where it is still moves (a new place, a new position write)
    let w0 = d.winner("rows", "b", Facet::Position).unwrap();
    let got = rows(&mut d, 7, vec![mv("b", after("d"))]);
    assert_eq!(order(&d), "d b c a");
    assert_eq!(got[0].effect, Effect::Visible);
    assert_eq!(
        (got[0].index_before, got[0].index_after),
        (Some(1), Some(1))
    );
    let w1 = d.winner("rows", "b", Facet::Position).unwrap();
    assert_ne!(w0, w1);
    assert_eq!(w1, Some(got[0].write));
    assert_eq!(got[0].value_before, Some(Value::Null));
}

#[test]
fn delete_hides_and_keeps_value_and_place() {
    let mut d = doc_with("a b c");
    rows(&mut d, 7, vec![set("b", "bee")]);
    let got = rows(&mut d, 8, vec![del("b")]);
    assert_eq!(order(&d), "a c");
    assert_eq!(got[0].index_before, Some(1));
    assert_eq!(got[0].index_after, None);
    assert_eq!(got[0].effect, Effect::Visible);
    assert_eq!(got[0].value_after, Some(Value::str("bee")));
    assert_eq!(d.elem("rows", &k("b")).unwrap(), ElemState::Deleted);
    assert_eq!(d.elem("rows", &k("c")).unwrap(), ElemState::Live(1));
    assert_eq!(
        d.winner("rows", "b", Facet::Existence).unwrap(),
        Some(got[0].write)
    );
    // deleting again does nothing, and says who deleted it
    let again = rows(&mut d, 9, vec![del("b")]);
    assert_eq!(
        again[0].effect,
        Effect::None {
            winner: Some(got[0].write)
        }
    );
    // an insert next to the deleted element keeps its place
    rows(&mut d, 9, vec![ins("x", after("b"))]);
    assert_eq!(order(&d), "a x c");
    let back = rows(&mut d, 9, vec![res("b")]);
    assert_eq!(order(&d), "a b x c");
    assert_eq!(back[0].index_after, Some(1));
    assert_eq!(back[0].value_after, Some(Value::str("bee")));
    assert_eq!(d.winner("rows", "b", Facet::Existence).unwrap(), None);
    // restoring a live element does nothing
    let again = rows(&mut d, 9, vec![res("b")]);
    assert_eq!(again[0].effect, Effect::None { winner: None });
    assert_eq!(
        (again[0].index_before, again[0].index_after),
        (Some(1), Some(1))
    );
}

#[test]
fn set_move_of_a_deleted_element_are_hidden_and_kept() {
    let mut d = doc_with("a b c");
    rows(&mut d, 7, vec![del("a")]);
    let got = rows(&mut d, 8, vec![set("a", "later"), mv("a", Anchor::End)]);
    assert!(got.iter().all(|o| o.effect == Effect::Hidden
        && o.index_before.is_none()
        && o.index_after.is_none()));
    assert_eq!(order(&d), "b c");
    rows(&mut d, 9, vec![res("a")]);
    assert_eq!(order(&d), "b c a", "it comes back where it was moved");
    assert_eq!(d.list("rows").unwrap()[2], (k("a"), Value::str("later")));
}

#[test]
fn set_replaces_the_value_and_records_its_writer() {
    let mut d = doc_with("a b");
    let got = rows(&mut d, 7, vec![set("a", "1"), set("a", "2")]);
    assert_eq!(got[0].value_before, Some(Value::Null));
    assert_eq!(got[1].value_before, Some(Value::str("1")));
    assert_eq!(got[1].value_after, Some(Value::str("2")));
    assert_eq!(
        (got[1].index_before, got[1].index_after),
        (Some(0), Some(0))
    );
    assert_eq!(
        d.winner("rows", "a", Facet::Value).unwrap(),
        Some(got[1].write)
    );
    assert_eq!(d.winner("rows", "b", Facet::Value).unwrap(), None);
    // an identical value written again is a new write (ABA: undo compares writes)
    let same = rows(&mut d, 8, vec![set("a", "2")]);
    assert_eq!(
        d.winner("rows", "a", Facet::Value).unwrap(),
        Some(same[0].write)
    );
    assert_ne!(same[0].write, got[1].write);
    // back to Null is a write too
    rows(
        &mut d,
        8,
        vec![ListOp::Set {
            key: k("a"),
            value: Value::Null,
        }],
    );
    assert_eq!(d.list("rows").unwrap()[0].1, Value::Null);
    assert!(d.winner("rows", "a", Facet::Value).unwrap().is_some());
}

#[test]
fn map_set_delete_get() {
    let mut d = Doc::new(S).unwrap();
    let got = cells(
        &mut d,
        7,
        vec![
            mset("r1\x1fc1", "x"),
            mset("r1\x1fc2", ""),
            mset("r1\x1fc1", "y"),
        ],
    );
    assert_eq!(got[2].before, Some(Value::str("x")));
    assert_eq!(got[2].after, Some(Value::str("y")));
    assert_eq!(
        d.map_get("cells", "r1\x1fc2").unwrap(),
        Some(Value::str("")),
        "an empty value is present (a cell clear is a set to \"\", E5)"
    );
    assert_eq!(
        d.winner("cells", "r1\x1fc1", Facet::Value).unwrap(),
        Some(got[2].write)
    );
    let gone = cells(
        &mut d,
        8,
        vec![MapOp::Delete {
            key: "r1\x1fc1".into(),
        }],
    );
    assert_eq!(gone[0].before, Some(Value::str("y")));
    assert_eq!(gone[0].effect, Effect::Visible);
    assert_eq!(d.map_get("cells", "r1\x1fc1").unwrap(), None);
    assert_eq!(d.winner("cells", "r1\x1fc1", Facet::Value).unwrap(), None);
    let again = cells(
        &mut d,
        8,
        vec![MapOp::Delete {
            key: "r1\x1fc1".into(),
        }],
    );
    assert_eq!(again[0].effect, Effect::None { winner: None });
    assert_eq!(
        d.map("cells").unwrap(),
        BTreeMap::from([("r1\x1fc2".to_owned(), Value::str(""))])
    );
    assert_eq!(
        d.winner("cells", "r1\x1fc2", Facet::Position).unwrap(),
        None
    );
}

#[test]
fn no_op_changes_write_nothing_and_report_why() {
    let mut d = doc_with("a b");
    rows(&mut d, 7, vec![del("a")]);
    let state = d.encode_state().unwrap();
    let e = d
        .edit_applied(
            8,
            &[
                Edit::List {
                    root: "rows".into(),
                    ops: vec![del("a"), res("b")],
                },
                Edit::Map {
                    root: "cells".into(),
                    ops: vec![MapOp::Delete { key: "z".into() }],
                },
                Edit::Text {
                    root: "notes".into(),
                    hunks: vec![],
                },
            ],
        )
        .unwrap();
    assert_eq!(e.update, None);
    assert_eq!(e.applied.len(), 3);
    assert_eq!(d.encode_state().unwrap(), state);
    // a set then a delete of a key absent before: also nothing
    let e = d
        .edit_applied(
            8,
            &[Edit::Map {
                root: "cells".into(),
                ops: vec![mset("t", "v"), MapOp::Delete { key: "t".into() }],
            }],
        )
        .unwrap();
    assert_eq!(e.update, None);
    // and a delete then restore of a live element
    assert_eq!(
        d.edit_as(
            8,
            &[Edit::List {
                root: "rows".into(),
                ops: vec![del("b"), res("b")]
            }]
        )
        .unwrap(),
        None
    );
    assert_eq!(d.encode_state().unwrap(), state);
}

#[test]
fn text_lists_and_maps_in_one_change_and_text_clocks_unmoved() {
    let mut d = Doc::new(S).unwrap();
    d.replace_text_as(500, "notes", "one\n").unwrap();
    let edits = vec![
        Edit::List {
            root: "rows".into(),
            ops: vec![ins("r1", Anchor::Start)],
        },
        Edit::Text {
            root: "notes".into(),
            hunks: diff::hunks("one\n", "one\ntwo\n"),
        },
        Edit::Map {
            root: "cells".into(),
            ops: vec![mset("r1\x1fc1", "v")],
        },
    ];
    let clock = d.clock_of(600);
    let want = super::insert_clocks(clock, &edits);
    let e = d.edit_applied(600, &edits).unwrap();
    assert!(e.update.is_some());
    assert!(matches!(e.applied[1], Applied::Text { .. }));
    assert_eq!(d.text("notes").unwrap(), "one\ntwo\n");
    assert_eq!(order(&d), "r1");
    // the text took the change's first clocks, as `insert_clocks` says
    let runs = d.blame_runs("notes").unwrap();
    assert_eq!(runs.last().unwrap().client, 600);
    assert_eq!(runs.last().unwrap().clock, want[1][0]);
    assert_eq!(want[1][0], clock);
    let all = d.export_neutral().unwrap();
    assert_eq!(all.len(), 3);
    let l = Doc::load(S, &d.encode_state().unwrap()).unwrap();
    assert_eq!(l.export_neutral().unwrap(), all, "store round trip");
    assert_eq!(l.elem("rows", &k("r1")).unwrap(), ElemState::Live(0));
}

#[test]
fn root_names_are_checked() {
    for roots in [
        &[("a/slots", Kind::Text)][..],
        &[("rows", Kind::MovableList), ("rows", Kind::Map)][..],
        &[("", Kind::Map)][..],
    ] {
        let s = Schema { name: "x", roots };
        assert!(
            matches!(Doc::new(s), Err(DocError::RootName(_))),
            "{roots:?}"
        );
    }
    // List (without move) is not built yet
    let s = Schema {
        name: "x",
        roots: &[("l", Kind::List)],
    };
    assert!(matches!(
        Doc::new(s),
        Err(DocError::Unsupported(Kind::List))
    ));
}

#[test]
fn write_ids_are_unique_even_for_changes_that_only_remove() {
    let mut d = doc_with("a b c");
    rows(&mut d, 7, vec![del("a"), del("b")]);
    cells(&mut d, 7, vec![mset("k", "v")]);
    // three changes by one client that insert nothing: each removes
    let r1 = rows(&mut d, 7, vec![res("a")]);
    let r2 = rows(&mut d, 7, vec![res("b")]);
    let r3 = cells(&mut d, 7, vec![MapOp::Delete { key: "k".into() }]);
    let next = rows(&mut d, 7, vec![set("c", "x")]);
    let ids = [r1[0].write, r2[0].write, r3[0].write, next[0].write];
    for i in 0..ids.len() {
        for j in i + 1..ids.len() {
            assert_ne!(ids[i], ids[j], "{i} {j}");
            assert!(!ids[i].same_change(&ids[j]));
        }
    }
    // ops of one change share it
    let two = rows(&mut d, 7, vec![set("a", "1"), set("b", "2")]);
    assert!(two[0].write.same_change(&two[1].write));
    assert_ne!(two[0].write, two[1].write);
    assert_eq!(two[0].write.client(), 7);
}

/// The live order is cached per root and dropped only by a change to that root (or a
/// merge), and what it says stays right.
#[test]
fn the_order_cache_follows_changes_to_its_root_only() {
    const TWO: Schema = Schema {
        name: "two",
        roots: &[
            ("rows", Kind::MovableList),
            ("cols", Kind::MovableList),
            ("cells", Kind::Map),
        ],
    };
    let mut d = Doc::new(TWO).unwrap();
    let ins_in = |d: &mut Doc, root: &str, key: &str| {
        d.edit_as(
            7,
            &[Edit::List {
                root: root.into(),
                ops: vec![ins(key, Anchor::End)],
            }],
        )
        .unwrap()
        .unwrap();
    };
    ins_in(&mut d, "rows", "r1");
    ins_in(&mut d, "cols", "c1");
    assert!(d.engine().cached_orders().is_empty());
    assert_eq!(d.elem("rows", &k("r1")).unwrap(), ElemState::Live(0));
    assert_eq!(d.elem("cols", &k("c1")).unwrap(), ElemState::Live(0));
    assert_eq!(d.engine().cached_orders(), vec!["cols", "rows"]);
    cells(&mut d, 7, vec![mset("r1\x1fc1", "v")]);
    assert_eq!(
        d.engine().cached_orders(),
        vec!["cols", "rows"],
        "a map change keeps both"
    );
    ins_in(&mut d, "rows", "r0");
    assert_eq!(
        d.engine().cached_orders(),
        vec!["cols"],
        "a rows change drops rows only"
    );
    assert_eq!(d.elem("rows", &k("r0")).unwrap(), ElemState::Live(1));
    let other = replica(&d);
    d.merge_from(&other).unwrap();
    assert!(
        d.engine().cached_orders().is_empty(),
        "a merge drops them all"
    );
    assert_eq!(d.elem("cols", &k("c1")).unwrap(), ElemState::Live(0));
}

// ---- the move rule: forward chains ----

/// The slots made by `change` (a write of it), as positions in the slots array and engine
/// clocks.
fn slots_of(d: &Doc, w: &WriteId) -> Vec<(usize, u32)> {
    d.engine()
        .slot_items("rows")
        .into_iter()
        .enumerate()
        .filter(|(_, (entry, _, _))| {
            let id = entry.split('\x1f').next().unwrap();
            WriteId::decode(id).is_some_and(|x| x.same_change(w))
        })
        .map(|(i, (_, _, clock))| (i, clock))
        .collect()
}

/// True when `slots` are one run: adjacent in the array and consecutive engine IDs.
fn one_run(slots: &[(usize, u32)]) -> bool {
    slots
        .windows(2)
        .all(|w| w[1].0 == w[0].0 + 1 && w[1].1 == w[0].1 + 1)
}

fn sorted_desc(n: usize) -> (Doc, Vec<Key>) {
    let keys: Vec<String> = (0..n).map(|i| format!("r{i}")).collect();
    let d = doc_with(&keys.join(" "));
    let target: Vec<Key> = keys.iter().rev().map(|s| k(s)).collect();
    (d, target)
}

#[test]
fn a_sort_as_a_forward_chain_is_one_run() {
    let (mut d, target) = sorted_desc(200);
    let current: Vec<Key> = d
        .list("rows")
        .unwrap()
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    let ops = reorder(&current, &target).unwrap();
    assert_eq!(ops.len(), 200);
    let got = rows(&mut d, 7, ops);
    let now: Vec<Key> = d
        .list("rows")
        .unwrap()
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    assert_eq!(now, target);
    let slots = slots_of(&d, &got[0].write);
    assert_eq!(slots.len(), 200);
    assert!(
        one_run(&slots),
        "the sort's live slots are one run: {slots:?}"
    );
    // and the slots it killed (the import's run) are one deleted range
    let import: Vec<_> = d
        .engine()
        .deleted_ranges()
        .into_iter()
        .filter(|(c, _)| *c == 50)
        .collect();
    assert_eq!(import.len(), 1, "{import:?}");
    // a second sort, back again: one more run
    let back: Vec<Key> = target.iter().rev().cloned().collect();
    let got = rows(&mut d, 7, reorder(&now, &back).unwrap());
    assert!(one_run(&slots_of(&d, &got[0].write)));
}

/// Planted negative: the same sort generated backwards (each move to the start, last key
/// first) gives the same order, but its slots are not a run, and the check says so.
#[test]
fn planted_a_backward_sort_is_not_one_run() {
    let (mut d, target) = sorted_desc(50);
    let ops: Vec<ListOp> = target
        .iter()
        .rev()
        .map(|key| ListOp::Move {
            key: key.clone(),
            to: Anchor::Start,
        })
        .collect();
    let got = rows(&mut d, 7, ops);
    let now: Vec<Key> = d
        .list("rows")
        .unwrap()
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    assert_eq!(now, target, "same order");
    assert!(!one_run(&slots_of(&d, &got[0].write)));
}

#[test]
fn a_block_move_is_one_run_and_keeps_its_order() {
    let mut d = doc_with("a b c d e f g");
    let block = [k("b"), k("c"), k("d")];
    let got = rows(&mut d, 7, chain(&block, after("f")));
    assert_eq!(order(&d), "a e f b c d g");
    assert!(one_run(&slots_of(&d, &got[0].write)));
}

/// E9: two writers each move a block into one gap, concurrently: the blocks stay
/// contiguous (forward chains don't interleave).
#[test]
fn concurrent_block_moves_into_one_gap_stay_contiguous() {
    let mut base = doc_with("a b c d e f g h");
    let mut x = replica(&base);
    let mut y = replica(&base);
    rows(&mut x, 21, chain(&[k("a"), k("b"), k("c")], after("h")));
    rows(&mut y, 22, chain(&[k("d"), k("e"), k("f")], after("h")));
    sync_all(&mut [&mut base, &mut x, &mut y]);
    let o = order(&base);
    assert!(
        o.contains("a b c") && o.contains("d e f"),
        "blocks interleaved: {o}"
    );
}

// ---- concurrency through merges ----

#[test]
fn concurrent_moves_never_duplicate() {
    let mut base = doc_with("a b c d");
    let mut x = replica(&base);
    let mut y = replica(&base);
    rows(&mut x, 21, vec![mv("a", Anchor::End)]);
    rows(&mut y, 22, vec![mv("a", after("b"))]);
    sync_all(&mut [&mut base, &mut x, &mut y]);
    let o = order(&base);
    assert_eq!(o.matches('a').count(), 1, "{o}");
    assert_eq!(o.split(' ').count(), 4);
    // and the elem index agrees with the order after a merge (the cache was dropped)
    let i = o.split(' ').position(|s| s == "a").unwrap();
    assert_eq!(base.elem("rows", &k("a")).unwrap(), ElemState::Live(i));
}

/// E-MLIST-2: a delete concurrent with a move wins; the element comes back (restore) at the
/// moved place.
#[test]
fn delete_wins_over_a_concurrent_move() {
    let mut base = doc_with("a b c");
    let mut x = replica(&base);
    let mut y = replica(&base);
    rows(&mut x, 21, vec![mv("a", Anchor::End)]);
    rows(&mut y, 22, vec![del("a")]);
    sync_all(&mut [&mut base, &mut x, &mut y]);
    assert_eq!(order(&base), "b c");
    rows(&mut base, 23, vec![res("a")]);
    assert_eq!(order(&base), "b c a");
}

/// E1: a delete concurrent with a restore wins.
#[test]
fn delete_wins_over_a_concurrent_restore() {
    for (rx, ry) in [(21, 22), (22, 21)] {
        let mut base = doc_with("a b c");
        rows(&mut base, 20, vec![del("b")]);
        let mut x = replica(&base);
        let mut y = replica(&base);
        rows(&mut x, rx, vec![res("b")]);
        sync_all(&mut [&mut base, &mut x]);
        // y deleted b again concurrently with x's restore (it never saw it)
        rows(&mut y, ry, vec![res("b"), del("b")]);
        sync_all(&mut [&mut base, &mut x, &mut y]);
        assert_eq!(order(&base), "a c", "clients {rx}/{ry}");
    }
}

/// E-MLIST-2: an insert next to an element moved concurrently lands at the old spot.
#[test]
fn insert_beside_a_moved_element_stays_at_the_old_spot() {
    let mut base = doc_with("a b c d");
    let mut x = replica(&base);
    let mut y = replica(&base);
    rows(&mut x, 21, vec![mv("b", Anchor::End)]);
    rows(&mut y, 22, vec![ins("n", after("b"))]);
    sync_all(&mut [&mut base, &mut x, &mut y]);
    assert_eq!(order(&base), "a n c d b");
}

#[test]
fn a_move_and_a_set_both_apply() {
    let mut base = doc_with("a b c");
    let mut x = replica(&base);
    let mut y = replica(&base);
    rows(&mut x, 21, vec![mv("a", Anchor::End)]);
    rows(&mut y, 22, vec![set("a", "v")]);
    sync_all(&mut [&mut base, &mut x, &mut y]);
    assert_eq!(base.list("rows").unwrap()[2], (k("a"), Value::str("v")));
}

// ---- the merged-update stream (a keyed diff, design §4.2) ----

/// x makes one change; y merges it. y's stream for the merge (`applied_since`) is x's local
/// stream, op for op, writes included, except where yrs keeps no identity for a removal (a
/// restore, a map delete: `None`; the conformance rule `stream.merged_writes` pins it).
#[test]
fn a_merged_change_reports_what_the_local_one_did() {
    type Case = (&'static str, Vec<Edit>, bool);
    let l = |ops| Edit::List {
        root: "rows".into(),
        ops,
    };
    let m = |ops| Edit::Map {
        root: "cells".into(),
        ops,
    };
    let cases: Vec<Case> = vec![
        ("insert", vec![l(vec![ins_v("n", after("c"), "nv")])], true),
        ("move", vec![l(vec![mv("a", after("d"))])], true),
        ("move back", vec![l(vec![mv("e", Anchor::Start)])], true),
        ("delete", vec![l(vec![del("c")])], true),
        ("restore", vec![l(vec![res("b")])], false),
        ("set", vec![l(vec![set("d", "v")])], true),
        ("hidden set", vec![l(vec![set("b", "h")])], true),
        ("map set", vec![m(vec![mset("k", "1")])], true),
        ("map add", vec![m(vec![mset("n", "1")])], true),
        (
            "map delete",
            vec![m(vec![MapOp::Delete { key: "k".into() }])],
            false,
        ),
    ];
    for (what, edits, named) in cases {
        let mut base = doc_with("a b c d e");
        rows(&mut base, 50, vec![del("b")]);
        cells(&mut base, 50, vec![mset("k", "0")]);
        let mut x = replica(&base);
        let mut y = replica(&base);
        let local = x.edit_applied(21, &edits).unwrap().applied;
        let before = y.keyed_state().unwrap();
        y.merge_from(&x).unwrap();
        let merged = y.applied_since(&before).unwrap();
        let want: Vec<Applied<Option<WriteId>>> = local
            .into_iter()
            .map(|a| a.map_write(|w| named.then_some(w)))
            .collect();
        assert_eq!(merged, want, "{what}");
    }
}

/// A sort merged: the derived stream moves fewer elements than the sort did (a minimal set),
/// and both replay to the same order.
#[test]
fn a_merged_sort_is_a_minimal_set_of_moves() {
    let base = doc_with("a b c d e f");
    let mut x = replica(&base);
    let mut y = replica(&base);
    let now: Vec<Key> = "a b c d e f".split(' ').map(k).collect();
    let target: Vec<Key> = "a c d e f b".split(' ').map(k).collect();
    let local = rows(&mut x, 21, reorder(&now, &target).unwrap());
    assert_eq!(
        local.len(),
        5,
        "the sort moves every element after the first"
    );
    let before = y.keyed_state().unwrap();
    y.merge_from(&x).unwrap();
    let merged = y.applied_since(&before).unwrap();
    let [Applied::List { ops, .. }] = merged.as_slice() else {
        panic!("{merged:?}")
    };
    assert_eq!(ops.len(), 1, "{ops:?}");
    assert_eq!(
        (ops[0].key.as_str(), ops[0].index_before, ops[0].index_after),
        ("b", Some(1), Some(5))
    );
    assert_eq!(ops[0].write, Some(local[4].write), "b's move is the sort's");
    assert_eq!(order(&y), "a c d e f b");
}

// ---- the pinned Yrs deviations (CSV seam design §5.3) ----
//
// `mlist.move_lww`, `mlist.yata_separating_case`, `map.lww`, `table.cell_lww`,
// `map.delete_vs_set` and `map.steady_bounded` are conformance rules now, in the kit Doc's
// `KNOWN_DEVIATIONS` (`crate::conformance`), each asserted to still happen by the generic
// suite (`crates/conformance`, `tests/structured.rs`).

/// `map.steady_bounded` (E-MAP-3: memory O(1) in overwrites of one key) deviates in memory,
/// live and after a reload (pinned in the conformance suite, `k2-mpc-conformance`, which
/// counts allocations). The stored state is another matter: the v2 encoding compresses a
/// regular run of overwrite stubs (two writers alternating, 1000 more overwrites) to a few
/// bytes.
#[test]
fn map_overwrites_compress_in_the_stored_state() {
    let state_after = |n: usize| {
        let mut x = Doc::new(S).unwrap();
        let mut y = replica(&x);
        for i in 0..n {
            let (d, c) = if i % 2 == 0 {
                (&mut x, 21)
            } else {
                (&mut y, 22)
            };
            cells(d, c, vec![mset("k", &format!("{i}"))]);
            let (a, b) = if i % 2 == 0 {
                (&mut y, &x)
            } else {
                (&mut x, &y)
            };
            a.merge_from(b).unwrap();
        }
        x.encode_state().unwrap().len()
    };
    let (small, big) = (state_after(10), state_after(1010));
    assert!(
        big < small + 200,
        "1000 more overwrites of one key grew the stored state by {} B",
        big - small
    );
}

// ---- planted negatives: mutant layouts the checks catch ----

/// Mutant `move-duplicates` (design §5.1 approach A): a move as remove + re-insert in a
/// `YArray` of keys. Two concurrent moves of one key leave two copies; the kit's layout
/// (above) never does.
#[test]
fn planted_move_as_reinsert_duplicates() {
    use yrs::updates::decoder::Decode;
    use yrs::{Array, Doc as YDoc, Options, ReadTxn, StateVector, Transact, Update};
    let mk = |c: u64| {
        YDoc::with_options(Options {
            client_id: yrs::block::ClientID::new(c),
            ..Options::default()
        })
    };
    let base = mk(50);
    let arr = base.get_or_insert_array("rows");
    arr.insert_range(&mut base.transact_mut(), 0, ["a", "b", "c", "d"]);
    let full = base
        .transact()
        .encode_state_as_update_v1(&StateVector::default());
    let fork = |c| {
        let d = mk(c);
        let a = d.get_or_insert_array("rows");
        d.transact_mut()
            .apply_update(Update::decode_v1(&full).unwrap())
            .unwrap();
        (d, a)
    };
    let (x, xa) = fork(21);
    let (y, ya) = fork(22);
    {
        let mut t = x.transact_mut();
        xa.remove(&mut t, 0);
        xa.insert(&mut t, 3, "a");
    }
    {
        let mut t = y.transact_mut();
        ya.remove(&mut t, 0);
        ya.insert(&mut t, 1, "a");
    }
    let ux = x
        .transact()
        .encode_state_as_update_v1(&StateVector::default());
    y.transact_mut()
        .apply_update(Update::decode_v1(&ux).unwrap())
        .unwrap();
    let got: Vec<String> = ya
        .iter(&y.transact())
        .map(|o| o.to_string(&y.transact()))
        .collect();
    assert_eq!(
        got.iter().filter(|s| *s == "a").count(),
        2,
        "the mutant must duplicate: {got:?}"
    );
}

/// Mutant `undel-lww` (E1): restore as a set of a liveness flag. A restore concurrent with a
/// delete then wins by client ID; the kit's restore (a flag removal) never does
/// (`delete_wins_over_a_concurrent_restore`).
#[test]
fn planted_restore_as_a_flag_set_beats_a_delete() {
    use yrs::updates::decoder::Decode;
    use yrs::{Doc as YDoc, Map, Options, ReadTxn, StateVector, Transact, Update};
    let mk = |c: u64| {
        YDoc::with_options(Options {
            client_id: yrs::block::ClientID::new(c),
            ..Options::default()
        })
    };
    let base = mk(50);
    let m = base.get_or_insert_map("dead");
    m.insert(&mut base.transact_mut(), "b", "dead");
    let full = base
        .transact()
        .encode_state_as_update_v1(&StateVector::default());
    let fork = |c| {
        let d = mk(c);
        let m = d.get_or_insert_map("dead");
        d.transact_mut()
            .apply_update(Update::decode_v1(&full).unwrap())
            .unwrap();
        (d, m)
    };
    // the restorer has the higher client ID; the deleter deletes again concurrently
    let (x, xm) = fork(22);
    let (y, ym) = fork(21);
    xm.insert(&mut x.transact_mut(), "b", "live");
    ym.insert(&mut y.transact_mut(), "b", "dead");
    let uy = y
        .transact()
        .encode_state_as_update_v1(&StateVector::default());
    x.transact_mut()
        .apply_update(Update::decode_v1(&uy).unwrap())
        .unwrap();
    let v = xm.get(&x.transact(), "b").unwrap().to_string(&x.transact());
    assert_eq!(v, "live", "the mutant must let the restore win");
}

/// Mutant: a visible order that ignores the position register (every slot shows). After
/// two concurrent moves the slots array holds both new slots, so the mutant shows the key
/// twice; the real order shows it once (`concurrent_moves_never_duplicate`).
#[test]
fn planted_order_without_the_position_register_duplicates() {
    let mut base = doc_with("a b c d");
    let mut x = replica(&base);
    let mut y = replica(&base);
    rows(&mut x, 21, vec![mv("a", Anchor::End)]);
    rows(&mut y, 22, vec![mv("a", after("b"))]);
    sync_all(&mut [&mut base, &mut x, &mut y]);
    let every_slot: Vec<String> = base
        .engine()
        .slot_items("rows")
        .into_iter()
        .map(|(e, _, _)| e.split('\x1f').nth(1).unwrap().to_owned())
        .collect();
    assert_eq!(every_slot.iter().filter(|s| *s == "a").count(), 2);
    assert_eq!(order(&base).matches('a').count(), 1);
}

// ---- drop (`rows drop-deleted`) and places (C1c) ----

fn drp(key: &str) -> ListOp {
    ListOp::Drop { key: k(key) }
}

fn places(d: &Doc) -> String {
    let s = d.keyed_state().unwrap();
    let (_, r) = s.roots.into_iter().find(|(r, _)| r == "rows").unwrap();
    match r {
        crate::elem::KeyedRoot::List { places, .. } => {
            places.iter().map(Key::as_str).collect::<Vec<_>>().join(" ")
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn places_keep_deleted_elements_where_they_were() {
    let mut d = doc_with("a b c d");
    rows(&mut d, 7, vec![del("b"), mv("d", Anchor::Start)]);
    assert_eq!(order(&d), "d a c");
    assert_eq!(places(&d), "d a b c", "b is hidden at its place");
    // a restore shows it there, as places said
    rows(&mut d, 7, vec![res("b")]);
    assert_eq!(order(&d), "d a b c");
}

#[test]
fn drop_removes_a_deleted_element_for_good() {
    let mut d = doc_with("a b c");
    rows(&mut d, 7, vec![set("b", "x"), del("b")]);
    let got = rows(&mut d, 8, vec![drp("b")]);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].kind, ListOpKind::Drop);
    assert_eq!(got[0].effect, Effect::Hidden);
    assert_eq!(got[0].value_before, Some(Value::str("x")));
    assert_eq!((got[0].index_before, got[0].index_after), (None, None));
    assert_eq!(order(&d), "a c");
    assert_eq!(places(&d), "a c");
    assert_eq!(d.elem("rows", &k("b")).unwrap(), ElemState::Unknown);
    assert_eq!(d.elem_value("rows", &k("b")).unwrap(), None);
    assert_eq!(d.winner("rows", "b", Facet::Value).unwrap(), None);
    // it can't come back from the list
    assert_eq!(
        refusal(d.edit_applied(
            9,
            &[Edit::List {
                root: "rows".into(),
                ops: vec![res("b")]
            }]
        )),
        OpRefusal::UnknownKey("b".into())
    );
    // the state survives a reload, and a replica agrees
    let r = replica(&d);
    assert_eq!(order(&r), "a c");
    assert_eq!(places(&r), "a c");
}

#[test]
fn drop_refuses_a_live_element_and_changes_nothing() {
    let mut d = doc_with("a b");
    let before = d.encode_state().unwrap();
    assert_eq!(
        refusal(d.edit_applied(
            9,
            &[Edit::List {
                root: "rows".into(),
                ops: vec![del("a"), drp("b")]
            }]
        )),
        OpRefusal::NotDeleted("b".into())
    );
    assert_eq!(d.encode_state().unwrap(), before, "all or nothing");
    // and a key dropped in a change can't be inserted again in it
    rows(&mut d, 9, vec![del("b")]);
    assert_eq!(
        refusal(d.edit_applied(
            9,
            &[Edit::List {
                root: "rows".into(),
                ops: vec![drp("b"), ins("b", Anchor::End)]
            }]
        )),
        OpRefusal::KeyExists("b".into())
    );
}

#[test]
fn a_drop_merges_into_a_replica_that_moved_its_neighbour() {
    let mut a = doc_with("a b c");
    rows(&mut a, 7, vec![del("b")]);
    let mut b = replica(&a);
    rows(&mut a, 7, vec![drp("b")]);
    rows(
        &mut b,
        8,
        vec![mv("c", Anchor::Start), ins("n", after("b"))],
    );
    sync_all(&mut [&mut a, &mut b]);
    // the insert anchored on the dropped element still lands at its old place
    assert_eq!(order(&a), "c a n");
    assert_eq!(d_known(&a, "b"), ElemState::Unknown);
}

fn d_known(d: &Doc, key: &str) -> ElemState {
    d.elem("rows", &k(key)).unwrap()
}
