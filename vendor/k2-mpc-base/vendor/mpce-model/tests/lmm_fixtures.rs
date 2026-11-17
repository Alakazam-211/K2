//! Fixtures (CT) for list, mlist and map: the Quint models' fixture rules, the undel cases, the
//! drop and existence holder, and the applied stream.

use mpce_model::list;
use mpce_model::map::{self, Effect as KEffect};
use mpce_model::mlist::{Doc, Effect, Intent, OpKind};
use mpce_model::text::Id;
use mpce_reg::{Existence, Facet};
use std::collections::BTreeSet;

fn seeded(names: &[&str]) -> Doc {
    let mut d = Doc::new();
    for (k, n) in names.iter().enumerate() {
        d.insert(0, k, n).unwrap();
    }
    d
}
fn values(d: &Doc) -> Vec<String> {
    d.view().into_iter().map(|(_, v)| v).collect()
}
fn sync(to: &mut Doc, from: &Doc) {
    to.merge_from(from).unwrap();
}
const A: Id = Id { agent: 0, seq: 0 };

#[test]
fn concurrent_moves_leave_one_copy_at_the_winner() {
    let s = seeded(&["A", "B", "C"]);
    let (mut d1, mut d2) = (s.clone(), s.clone());
    d1.move_elem(1, 0, 2).unwrap(); // A to the end: B C A
    d2.move_elem(2, 0, 1).unwrap(); // A after B: B A C
    sync(&mut d1, &d2);
    sync(&mut d2, &d1);
    // equal lamport: agent 2 wins
    assert_eq!(values(&d1), ["B", "A", "C"]);
    assert_eq!(d1.view(), d2.view());
    assert_eq!(d1.slots(), d2.slots());
}

#[test]
fn move_vs_delete_then_undel_restores_at_the_moved_position() {
    let s = seeded(&["A", "B", "C"]);
    let (mut d1, mut d2) = (s.clone(), s.clone());
    d1.move_elem(1, 0, 2).unwrap(); // B C A
    d1.set(1, 2, "a2").unwrap();
    d2.delete(2, 0).unwrap();
    sync(&mut d1, &d2);
    sync(&mut d2, &d1);
    assert_eq!(values(&d1), ["B", "C"]);
    assert_eq!(d1.read(A).unwrap().existence, Existence::Hidden);
    d2.undel(2, 0).unwrap();
    // the same element, with the position and value it converged to
    assert_eq!(values(&d2), ["B", "C", "a2"]);
    assert_eq!(d2.visible()[2], A);
}

#[test]
fn undel_concurrent_with_a_second_delete_stays_hidden() {
    let s = seeded(&["A", "B"]);
    let (mut d1, mut d2, mut d3) = (s.clone(), s.clone(), s.clone());
    let del1 = d1.delete(1, 0).unwrap();
    sync(&mut d2, &d1);
    let u = d2.undel(2, 0).unwrap();
    let del3 = d3.delete(3, 0).unwrap();
    for d in [&mut d1, &mut d2] {
        d.deliver_all(&[del1.clone(), u.clone(), del3.clone()]).unwrap();
    }
    sync(&mut d3, &d1);
    for d in [&d1, &d2, &d3] {
        assert_eq!(values(d), ["B"]);
        let v = d.read(A).unwrap();
        assert_eq!((v.existence, v.existence_holder), (Existence::Hidden, del3.id));
        assert_eq!(d.max_existence(A), [u.id, del3.id].into_iter().collect());
    }
    // an undel that has seen both restores
    d1.undel(1, 0).unwrap();
    assert_eq!(values(&d1), ["A", "B"]);
    assert_eq!(d1.winner(A, Facet::Existence), Some(d1.log().last().unwrap().id));
}

#[test]
fn branch_undel_meets_a_stable_delete() {
    // an old branch's undel (it had seen the first delete only) meets a later delete
    let mut d = seeded(&["A", "B"]);
    d.delete(1, 0).unwrap();
    let v1: BTreeSet<Id> = d.frontier().clone();
    d.undel(1, 0).unwrap();
    d.delete(1, 0).unwrap();
    let e = d.branch(1, &v1, Intent::Undel { index: 0 }, None).unwrap();
    assert_eq!(e.parents, v1);
    assert_eq!(values(&d), ["B"]);
}

#[test]
fn drop_is_absorbing() {
    let mut d = seeded(&["A", "B"]);
    d.delete(1, 0).unwrap();
    let before_drop = d.frontier().clone();
    let dr = d.drop_elem(1, A).unwrap();
    assert_eq!(d.read(A).unwrap().existence, Existence::Dropped);
    assert!(d.hidden_slots().is_empty());
    // an undel from an old branch (where A was only hidden) cannot revive it
    d.branch(2, &before_drop, Intent::Undel { index: 0 }, None).unwrap();
    assert_eq!(values(&d), ["B"]);
    assert_eq!(d.winner(A, Facet::Existence), Some(dr.id));
    // its slot and ID stay
    assert!(d.slots().contains(&A));
    assert!(d.drop_elem(1, A).is_err());
}

#[test]
fn stream_reports_every_applied_op() {
    let s = seeded(&["A", "B", "C"]);
    let (mut d1, mut d2) = (s.clone(), s.clone());
    d1.take_stream();
    let lo = d1.set(1, 0, "lo").unwrap();
    let mv = d1.move_elem(1, 0, 2).unwrap();
    let hi = {
        // a concurrent set with a higher lamport
        d2.set(2, 1, "x").unwrap();
        d2.set(2, 0, "hi").unwrap()
    };
    let st = d1.take_stream();
    assert_eq!((st[0].kind, st[0].index_before, st[0].value_before.as_deref(), st[0].value_after.as_deref(), st[0].effect), (OpKind::Set, Some(0), Some("A"), Some("lo"), Effect::Visible));
    assert_eq!((st[1].kind, st[1].index_before, st[1].index_after, st[1].winner), (OpKind::Move, Some(0), Some(2), Some(mv.id)));
    assert!(st.iter().all(|a| a.local));
    sync(&mut d1, &d2);
    let st = d1.take_stream();
    let last = st.iter().find(|a| a.event == hi.id).unwrap();
    assert_eq!((last.effect, last.winner, last.local), (Effect::Visible, Some(hi.id), false));
    // the losing set arrives at d2: nothing visible changes, and it reports the current winner
    d2.take_stream();
    sync(&mut d2, &d1);
    let st = d2.take_stream();
    let l = st.iter().find(|a| a.event == lo.id).unwrap();
    assert_eq!((l.effect, l.winner), (Effect::None, Some(hi.id)));
    // a delete: the element leaves the view
    let del = d2.delete(2, 2).unwrap();
    let st = d2.take_stream();
    assert_eq!((st[0].event, st[0].index_before, st[0].index_after, st[0].existence_after), (del.id, Some(2), None, Some(Existence::Hidden)));
}

#[test]
fn winner_resolved_follows_restores_write() {
    let mut d = seeded(&["A"]);
    let s1 = d.set(1, 0, "x").unwrap();
    let undo = d.local(1, Intent::Set { index: 0, value: "A".into() }, Some(A)).unwrap();
    assert_eq!(d.winner(A, Facet::Value), Some(undo.id));
    assert_eq!(d.winner_resolved(A, Facet::Value), Some(A));
    assert_ne!(s1.id, undo.id);
}

#[test]
fn list_delete_beats_a_concurrent_set() {
    let mut s = list::Doc::new();
    s.insert(0, 0, "a").unwrap();
    let (mut d1, mut d2) = (s.clone(), s.clone());
    d1.delete(1, 0).unwrap();
    d2.set(2, 0, "b").unwrap();
    d2.set(2, 0, "c").unwrap(); // higher lamport than the delete
    d1.deliver_all(&d2.log()).unwrap();
    d2.deliver_all(&d1.log()).unwrap();
    assert!(d1.view().is_empty() && d2.view().is_empty());
    // the register keeps converging behind the tombstone, and both sets stay in history
    assert_eq!(d1.read(A).unwrap().value, "c");
    assert_eq!(d1.as_mlist().reg(A).unwrap().len(), 4);
    assert!(d1.deliver(&d1.as_mlist().log()[0]).is_ok());
}

#[test]
fn map_lww_resurrect_and_replaced_root() {
    let (mut d1, mut d2) = (map::Doc::new(), map::Doc::new());
    let r1 = d1.set_root(1, "k").unwrap();
    d2.deliver(&r1).unwrap();
    // agent 2 edits the root while agent 1 replaces it
    let edit = d2.nest(2, "k").unwrap();
    let r2 = d1.set_root(1, "k").unwrap();
    d1.take_stream();
    d1.deliver(&edit).unwrap();
    let st = d1.take_stream();
    assert!(st[0].into_replaced_root && st[0].effect == KEffect::None);
    assert_eq!(d1.nested()[&r1.id], [edit.id].into_iter().collect());
    assert_eq!(d1.view()["k"], (r2.id, Some(BTreeSet::new())));
    // delete, then a later set resurrects; concurrent sets: higher (lamport, agent)
    let del = d1.delete(1, "k").unwrap();
    assert!(!d1.is_live("k"));
    d2.deliver(&r2).unwrap();
    d2.deliver(&del).unwrap();
    let s = d2.set(2, "k", "v").unwrap();
    assert!(d2.is_live("k"));
    assert_eq!(d2.winner("k"), Some(s.id));
    assert_eq!(d2.cells().len(), 1);
}
