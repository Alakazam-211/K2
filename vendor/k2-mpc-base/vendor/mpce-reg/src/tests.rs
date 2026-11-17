//! Fixtures for the read: the formal model's fixture rules and the existence-holder cases.

use super::*;
use std::collections::BTreeSet;

type Id = (u64, u64); // (agent, seq)
type R = Reg<Id, &'static str>;

/// an op of agent a, seq s, lamport l, with past `past`
fn op(a: u64, s: u64, l: u64, past: &[Id], kind: Kind<&'static str>) -> Op<Id, &'static str> {
    Op { id: (a, s), agent: a, lamport: l, past: past.iter().copied().collect(), kind, restores_write: None }
}

fn reg(ops: Vec<Op<Id, &'static str>>) -> R {
    let mut r = R::new();
    for o in ops {
        r.apply(o).unwrap();
    }
    r
}

const INS: Id = (0, 0);
fn ins() -> Op<Id, &'static str> {
    op(0, 0, 0, &[], Kind::Insert("v0"))
}

#[test]
fn no_insert_no_view() {
    assert_eq!(read(&reg(vec![op(1, 0, 1, &[INS], Kind::Set("v1"))])), None);
    let v = read(&reg(vec![ins()])).unwrap();
    assert_eq!((v.pos, v.value_write, v.value, v.existence, v.existence_holder), (INS, INS, "v0", Existence::Live, INS));
}

#[test]
fn value_and_position_lww_lamport_first_then_agent() {
    // concurrent sets at equal lamport: the higher agent wins; a higher lamport beats any agent
    let r = reg(vec![ins(), op(1, 0, 1, &[INS], Kind::Set("a")), op(2, 0, 1, &[INS], Kind::Set("b"))]);
    let v = read(&r).unwrap();
    assert_eq!((v.value_write, v.value), ((2, 0), "b"));
    let r = reg(vec![ins(), op(1, 0, 2, &[INS], Kind::Set("a")), op(2, 0, 1, &[INS], Kind::Set("b"))]);
    assert_eq!(read(&r).unwrap().value, "a");
    let r = reg(vec![ins(), op(1, 0, 1, &[INS], Kind::Move), op(2, 0, 1, &[INS], Kind::Move)]);
    assert_eq!(read(&r).unwrap().pos, (2, 0));
    // the losing writes stay in the set (history)
    assert_eq!(r.len(), 3);
    // the mutant tie-break flips the equal-lamport case
    assert_eq!(read_with(&r, ReadRule::AgentTiebreakReversed).unwrap().pos, (1, 0));
}

#[test]
fn delete_beats_concurrent_set_and_move() {
    // the list rule: a delete hides whatever higher-timestamp sets and moves are concurrent
    let r = reg(vec![ins(), op(1, 0, 1, &[INS], Kind::Delete), op(2, 0, 5, &[INS], Kind::Set("late")), op(2, 1, 6, &[INS, (2, 0)], Kind::Move)]);
    let v = read(&r).unwrap();
    assert_eq!(v.existence, Existence::Hidden);
    assert_eq!(v.existence_holder, (1, 0));
    // hiding keeps the registers
    assert_eq!((v.value, v.pos), ("late", (2, 1)));
    // teeth: the mutant lets the later set win
    assert_eq!(read_with(&r, ReadRule::DeleteLosesToSet).unwrap().existence, Existence::Live);
}

#[test]
fn undel_restores_with_converged_registers() {
    let r = reg(vec![
        ins(),
        op(1, 0, 1, &[INS], Kind::Set("a")),
        op(1, 1, 2, &[INS, (1, 0)], Kind::Delete),
        op(2, 0, 3, &[INS, (1, 0), (1, 1)], Kind::Undel),
    ]);
    let v = read(&r).unwrap();
    assert_eq!((v.existence, v.existence_holder, v.value), (Existence::Live, (2, 0), "a"));
}

#[test]
fn two_concurrent_deletes() {
    // holder: the higher (lamport, agent) delete
    let r = reg(vec![ins(), op(1, 0, 1, &[INS], Kind::Delete), op(2, 0, 1, &[INS], Kind::Delete)]);
    let v = read(&r).unwrap();
    assert_eq!((v.existence, v.existence_holder), (Existence::Hidden, (2, 0)));
}

#[test]
fn delete_concurrent_with_undel_wins_and_holds() {
    // d1, then u (sees d1) concurrent with d2 (does not see u): hidden, holder d2
    let r = reg(vec![
        ins(),
        op(1, 0, 1, &[INS], Kind::Delete),
        op(1, 1, 5, &[INS, (1, 0)], Kind::Undel),
        op(2, 0, 2, &[INS, (1, 0)], Kind::Delete),
    ]);
    let v = read(&r).unwrap();
    assert_eq!((v.existence, v.existence_holder), (Existence::Hidden, (2, 0)));
    // the undel has the highest timestamp: the LWW mutant shows it
    assert_eq!(read_with(&r, ReadRule::UndelLww).unwrap().existence, Existence::Live);
    // the maximal existence ops are u and d2
    assert_eq!(max_existence(&r), [(1, 1), (2, 0)].into_iter().collect());
}

#[test]
fn undel_covering_two_deletes() {
    let r = reg(vec![
        ins(),
        op(1, 0, 1, &[INS], Kind::Delete),
        op(2, 0, 1, &[INS], Kind::Delete),
        op(3, 0, 2, &[INS, (1, 0), (2, 0)], Kind::Undel),
    ]);
    let v = read(&r).unwrap();
    assert_eq!((v.existence, v.existence_holder), (Existence::Live, (3, 0)));
}

/// The case: A (agent 2) restores having seen only d1, B (agent 1) only d2.
/// d1 < uA, d2 < uB, crosswise concurrent; equal lamports, so A's undel is the higher.
fn two_covering_undels() -> R {
    reg(vec![
        ins(),
        op(1, 0, 1, &[INS], Kind::Delete),       // d2, by B
        op(2, 0, 1, &[INS], Kind::Delete),       // d1, by A
        op(2, 1, 2, &[INS, (2, 0)], Kind::Undel), // uA covers d1
        op(1, 1, 2, &[INS, (1, 0)], Kind::Undel), // uB covers d2
    ])
}

#[test]
fn two_concurrent_undels_covering_different_deletes() {
    // live (every delete is covered); the holder is the higher covering undel (r3), not the
    // insert (r2's wording: no undel covers every delete)
    let r = two_covering_undels();
    let v = read(&r).unwrap();
    assert_eq!((v.existence, v.existence_holder), (Existence::Live, (2, 1)));
    assert_eq!(covering_undels(&r), [(1, 1), (2, 1)].into_iter().collect());
    // r2's wording gave the insert
    assert_eq!(read_with(&r, ReadRule::HolderCoversEveryDelete).unwrap().existence_holder, INS);
}

#[test]
fn undel_with_no_delete_holds() {
    let r = reg(vec![ins(), op(1, 0, 1, &[INS], Kind::Undel), op(2, 0, 1, &[INS], Kind::Undel)]);
    assert_eq!(winner(&r, Facet::Existence), Some((2, 0)));
}

#[test]
fn undoing_a_restore_is_own_only_if_every_other_covering_undel_is_in_its_past() {
    let r = two_covering_undels();
    let ua = r.get((2, 1)).unwrap();
    let batch: BTreeSet<Id> = [ua.id].into_iter().collect();
    // A's undo of its restore: B's concurrent undel is outside A's past, so it conflicts
    // (otherwise A would silently re-hide what B restored)
    assert!(!restore_undo_is_own(&r, &batch, &ua.past));
    // the planted mutant own-by-holder-only fails this: A's undel is the holder
    assert_eq!(winner(&r, Facet::Existence), Some(ua.id));
    assert!(restore_undo_is_own_with(&r, &batch, &ua.past, OwnRule::ByHolderOnly));
    // had A seen B's undel (it is in A's batch's past), the undo is A's own
    let mut seen = ua.past.clone();
    seen.insert((1, 1));
    assert!(restore_undo_is_own(&r, &batch, &seen));
    // B's undo: A's undel (the holder) is outside B's past, so it conflicts too; the mutant
    // also says so (B's undel is not the holder): own-by-holder-only errs only one way
    let ub = r.get((1, 1)).unwrap();
    let bb: BTreeSet<Id> = [ub.id].into_iter().collect();
    assert!(!restore_undo_is_own(&r, &bb, &ub.past));
    assert!(!restore_undo_is_own_with(&r, &bb, &ub.past, OwnRule::ByHolderOnly));
    // one restore covering both deletes: its undo is own
    let one = reg(vec![
        ins(),
        op(1, 0, 1, &[INS], Kind::Delete),
        op(2, 0, 1, &[INS], Kind::Delete),
        op(3, 0, 2, &[INS, (1, 0), (2, 0)], Kind::Undel),
    ]);
    let u = one.get((3, 0)).unwrap();
    assert!(restore_undo_is_own(&one, &[u.id].into_iter().collect(), &u.past));
}

#[test]
fn drop_over_undel_is_absorbing() {
    // a drop, then an undel causally after it: still dropped; holder the highest drop
    let r = reg(vec![
        ins(),
        op(1, 0, 1, &[INS], Kind::Delete),
        op(1, 1, 2, &[INS, (1, 0)], Kind::Drop),
        op(2, 0, 3, &[INS, (1, 0), (1, 1)], Kind::Undel),
        op(3, 0, 1, &[INS], Kind::Drop),
    ]);
    let v = read(&r).unwrap();
    assert_eq!((v.existence, v.existence_holder), (Existence::Dropped, (1, 1)));
    assert_eq!(winner(&r, Facet::Existence), Some((1, 1)));
}

#[test]
fn winner_resolved_follows_restores_write() {
    let mut undo = op(2, 0, 3, &[INS, (1, 0)], Kind::Set("v0"));
    undo.restores_write = Some(INS);
    let r = reg(vec![ins(), op(1, 0, 1, &[INS], Kind::Set("a")), undo]);
    assert_eq!(winner(&r, Facet::Value), Some((2, 0)));
    assert_eq!(winner_resolved(&r, Facet::Value), Some(INS));
    assert_eq!(winner_resolved(&r, Facet::Position), Some(INS));
}

#[test]
fn apply_is_union_in_any_order() {
    let ops = vec![
        ins(),
        op(1, 0, 1, &[INS], Kind::Delete),
        op(1, 1, 5, &[INS, (1, 0)], Kind::Undel),
        op(2, 0, 2, &[INS, (1, 0)], Kind::Delete),
        op(2, 1, 3, &[INS, (1, 0), (2, 0)], Kind::Set("z")),
    ];
    let a = reg(ops.clone());
    let b = reg(ops.into_iter().rev().collect());
    assert_eq!(a, b);
    assert_eq!(read(&a), read(&b));
    let mut c = a.clone();
    assert_eq!(c.merge(&b), Ok(0));
    assert_eq!(c.apply(op(1, 0, 9, &[INS], Kind::Delete)), Err(RegError::Conflict));
}

#[test]
fn key_lww() {
    use crate::key::*;
    let w = |a: u64, s: u64, l: u64, kind: KeyKind<&'static str>| KeyWrite { id: (a, s), agent: a, lamport: l, kind, restores_write: None };
    let mut k: KeyReg<Id, &str> = KeyReg::new();
    k.apply(w(1, 0, 0, KeyKind::Set("a"))).unwrap();
    k.apply(w(2, 0, 0, KeyKind::Delete)).unwrap();
    assert_eq!(read_key(&k).map(|x| (x.id, x.live())), Some(((2, 0), false)));
    assert_eq!(read_key_with(&k, KeyRule::NoAgentTiebreak).map(|x| x.id), Some((1, 0)));
    k.apply(w(1, 1, 1, KeyKind::Set("b"))).unwrap();
    assert_eq!(read_key(&k).map(|x| (x.id, x.live())), Some(((1, 1), true)));
    assert_eq!(read_key_with(&k, KeyRule::DeleteWins).map(|x| x.id), Some((2, 0)));
}
