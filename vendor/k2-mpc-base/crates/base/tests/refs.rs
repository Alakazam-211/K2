//! The reference-state helper (`k2_mpc_base::refs`) on replicas of a synthetic keyed doc: a
//! MovableList `items`, a Map `props` (`<item>\x1fparent`) and a Map `links`
//! (`<link>\x1ffrom`, `<link>\x1fto`, `<link>\x1fhint`).
//!
//! `MPC_STRUCT_MUTANT` plants a fault for the whole suite (`k2_mpc_base::differential`, the
//! `differential` feature, on for these tests):
//! - `orphan-drops-link` is `RefMutant::OrphanDropsLink`, caught by
//!   `orphans_keep_their_links_and_hints` (and the proptest);
//! - `cycle-by-order` is `RefMutant::CycleByOrder`, caught by
//!   `a_cycle_is_cut_the_same_in_any_order` (and the proptest).
//!
//! Unset, the tests run as is. Any other value is refused.

use std::collections::BTreeMap;

use k2_mpc_base::conformance::replica::sync;
use k2_mpc_base::elem::{Anchor, KeyedRoot, KeyedState, ListOp, MapOp};
use k2_mpc_base::refs::{resolve_all, resolve_parents, Ref, RefState, SuspectWhy};
use k2_mpc_base::session::View;
use k2_mpc_base::{Doc, Edit, Key, Kind, Schema, Value};

fn under_struct_mutant<R>(f: impl FnOnce() -> R) -> R {
    use k2_mpc_base::differential::{with_ref_mutant, RefMutant};
    let m = std::env::var("MPC_STRUCT_MUTANT");
    println!("MPC_STRUCT_MUTANT={}", m.as_deref().unwrap_or("(unset)"));
    match m.as_deref() {
        Ok("orphan-drops-link") => with_ref_mutant(RefMutant::OrphanDropsLink, f),
        Ok("cycle-by-order") => with_ref_mutant(RefMutant::CycleByOrder, f),
        Ok(other) => panic!("MPC_STRUCT_MUTANT={other}: not a mutant this suite plants"),
        Err(_) => f(),
    }
}

const SHAPE: Schema = Schema {
    name: "shapes",
    roots: &[
        ("items", Kind::MovableList),
        ("props", Kind::Map),
        ("links", Kind::Map),
    ],
};

const SEP: char = '\u{1f}';

fn key(s: &str) -> Key {
    Key::new(s).unwrap()
}

fn any(_: &Value) -> bool {
    true
}

fn set(root: &str, k: String, v: &str) -> Edit {
    Edit::Map {
        root: root.into(),
        ops: vec![MapOp::Set {
            key: k,
            value: Value::str(v),
        }],
    }
}

fn list(op: ListOp) -> Edit {
    Edit::List {
        root: "items".into(),
        ops: vec![op],
    }
}

fn insert(k: &str) -> Edit {
    list(ListOp::Insert {
        key: key(k),
        at: Anchor::End,
        value: Value::str("box"),
    })
}

/// Every link end in `view`, as references to items, with the link's hint.
fn link_refs(view: &View) -> Vec<Ref> {
    let links = view.map("links").expect("a links map");
    let mut ids: Vec<&str> = links
        .keys()
        .filter_map(|k| k.split_once(SEP).map(|(id, _)| id))
        .collect();
    ids.dedup();
    let mut out = Vec::new();
    for id in ids {
        let hint = links
            .get(&format!("{id}{SEP}hint"))
            .and_then(|v| v.as_str())
            .map(str::to_owned);
        for end in ["from", "to"] {
            if let Some(v) = links
                .get(&format!("{id}{SEP}{end}"))
                .and_then(|v| v.as_str())
            {
                let r = Ref::new("items", key(v));
                out.push(match &hint {
                    Some(h) => r.with_hint(h.clone()),
                    None => r,
                });
            }
        }
    }
    out
}

fn parent_of(view: &View) -> impl Fn(&Key) -> Option<Key> + '_ {
    let props = view.map("props").expect("a props map");
    move |k: &Key| {
        props
            .get(&format!("{k}{SEP}parent"))
            .and_then(|v| v.as_str())
            .and_then(|s| Key::new(s).ok())
    }
}

/// `view` with `items`' live order reversed (the same elements, links and parents).
fn reversed(view: &View) -> View {
    let mut st: KeyedState = view.keyed().clone();
    for (r, k) in st.roots.iter_mut() {
        if let (true, KeyedRoot::List { live, .. }) = (r == "items", k) {
            live.reverse();
        }
    }
    View::new(st)
}

/// What every replica must agree on, and the helper's own invariants: one entry per link end,
/// each orphan with its link's hint; the parents cut the same in any list order.
fn check(view: &View) -> (Vec<(Ref, RefState)>, BTreeMap<Key, RefState>) {
    let refs = link_refs(view);
    let got = resolve_all(view, &refs, &any);
    assert_eq!(
        got.len(),
        refs.len(),
        "every link end keeps its entry, orphans included: {got:?}"
    );
    for ((r, s), want) in got.iter().zip(&refs) {
        assert_eq!(r, want, "in order, unchanged (hint included)");
        if *s == RefState::Orphaned {
            assert!(r.hint.is_some(), "an orphan keeps its hint: {r:?}");
        }
    }
    let parents = resolve_parents(view, "items", &parent_of(view));
    let rev = reversed(view);
    assert_eq!(
        resolve_parents(&rev, "items", &parent_of(&rev)),
        parents,
        "a cycle is cut at its smallest key, whatever the list's order"
    );
    (got, parents)
}

#[test]
fn orphans_keep_their_links_and_hints() {
    under_struct_mutant(|| {
        let mut d = Doc::new(SHAPE).unwrap();
        let c = 7;
        for k in ["a", "b", "c"] {
            d.edit_as(c, &[insert(k)]).unwrap();
        }
        let link = |id: &str, from: &str, to: &str, hint: &str| {
            vec![
                set("links", format!("{id}{SEP}from"), from),
                set("links", format!("{id}{SEP}to"), to),
                set("links", format!("{id}{SEP}hint"), hint),
            ]
        };
        d.edit_as(c, &link("l1", "a", "b", "10,20")).unwrap();
        d.edit_as(c, &link("l2", "b", "c", "30,40")).unwrap();
        d.edit_as(c, &[list(ListOp::Delete { key: key("b") })])
            .unwrap();
        let v = View::new(d.keyed_state().unwrap());
        let (got, _) = check(&v);
        let states: Vec<(&str, RefState)> = got.iter().map(|(r, s)| (r.key.as_str(), *s)).collect();
        assert_eq!(
            states,
            [
                ("a", RefState::Attached),
                ("b", RefState::Orphaned),
                ("b", RefState::Orphaned),
                ("c", RefState::Attached),
            ]
        );
        // a restore brings them back
        d.edit_as(c, &[list(ListOp::Restore { key: key("b") })])
            .unwrap();
        let v = View::new(d.keyed_state().unwrap());
        assert!(check(&v).0.iter().all(|(_, s)| *s == RefState::Attached));
    });
}

#[test]
fn a_cycle_is_cut_the_same_in_any_order() {
    under_struct_mutant(|| {
        let mut d = Doc::new(SHAPE).unwrap();
        let c = 7;
        // the list's order puts the cycle's smallest key last
        for k in ["z", "q", "m", "e"] {
            d.edit_as(c, &[insert(k)]).unwrap();
        }
        for (child, parent) in [("z", "q"), ("q", "m"), ("m", "z"), ("e", "m")] {
            d.edit_as(c, &[set("props", format!("{child}{SEP}parent"), parent)])
                .unwrap();
        }
        let v = View::new(d.keyed_state().unwrap());
        let (_, parents) = check(&v);
        assert_eq!(
            parents.get(&key("m")),
            Some(&RefState::Suspect(SuspectWhy::Cycle))
        );
        assert_eq!(parents.get(&key("z")), Some(&RefState::Attached));
        assert_eq!(parents.get(&key("q")), Some(&RefState::Attached));
        assert_eq!(parents.get(&key("e")), Some(&RefState::Attached));
    });
}

// ---- three replicas link, delete, restore, re-parent and sync at random

#[derive(Debug, Clone)]
enum Step {
    Add(usize),
    Delete(usize, usize),
    Restore(usize, usize),
    Link(usize, usize, usize),
    Parent(usize, usize, usize),
    Sync(usize, usize),
}

fn step() -> impl proptest::strategy::Strategy<Value = Step> {
    use proptest::prelude::*;
    let r = 0..3usize;
    prop_oneof![
        3 => r.clone().prop_map(Step::Add),
        2 => (r.clone(), any::<usize>()).prop_map(|(a, b)| Step::Delete(a, b)),
        1 => (r.clone(), any::<usize>()).prop_map(|(a, b)| Step::Restore(a, b)),
        3 => (r.clone(), any::<usize>(), any::<usize>()).prop_map(|(a, b, c)| Step::Link(a, b, c)),
        2 => (r.clone(), any::<usize>(), any::<usize>()).prop_map(|(a, b, c)| Step::Parent(a, b, c)),
        3 => (r.clone(), r).prop_map(|(a, b)| Step::Sync(a, b)),
    ]
}

fn pair(docs: &mut [Doc], a: usize, b: usize) -> (&Doc, &mut Doc) {
    if a < b {
        let (x, y) = docs.split_at_mut(b);
        (&x[a], &mut y[0])
    } else {
        let (x, y) = docs.split_at_mut(a);
        (&y[0], &mut x[b])
    }
}

fn run(steps: &[Step]) {
    let clients = [11u64, 22, 33];
    let mut docs: Vec<Doc> = (0..3).map(|_| Doc::new(SHAPE).unwrap()).collect();
    let mut made = [0usize; 3];
    let live = |d: &Doc| -> Vec<Key> {
        d.list("items")
            .unwrap()
            .into_iter()
            .map(|(k, _)| k)
            .collect()
    };
    let hidden = |d: &Doc| -> Vec<Key> {
        match d
            .keyed_state()
            .unwrap()
            .roots
            .into_iter()
            .find(|(r, _)| r == "items")
        {
            Some((_, KeyedRoot::List { hidden, .. })) => hidden.into_keys().collect(),
            _ => Vec::new(),
        }
    };
    for s in steps {
        match *s {
            Step::Add(r) => {
                made[r] += 1;
                let k = format!("x{r}n{}", made[r]);
                docs[r].edit_as(clients[r], &[insert(&k)]).unwrap();
            }
            Step::Delete(r, p) => {
                let l = live(&docs[r]);
                if !l.is_empty() {
                    let k = l[p % l.len()].clone();
                    docs[r]
                        .edit_as(clients[r], &[list(ListOp::Delete { key: k })])
                        .unwrap();
                }
            }
            Step::Restore(r, p) => {
                let h = hidden(&docs[r]);
                if !h.is_empty() {
                    let k = h[p % h.len()].clone();
                    docs[r]
                        .edit_as(clients[r], &[list(ListOp::Restore { key: k })])
                        .unwrap();
                }
            }
            Step::Link(r, a, b) => {
                let l = live(&docs[r]);
                if !l.is_empty() {
                    made[r] += 1;
                    let id = format!("l{r}n{}", made[r]);
                    let (from, to) = (&l[a % l.len()], &l[b % l.len()]);
                    let edits = [
                        set("links", format!("{id}{SEP}from"), from.as_str()),
                        set("links", format!("{id}{SEP}to"), to.as_str()),
                        set("links", format!("{id}{SEP}hint"), &format!("{a},{b}")),
                    ];
                    docs[r].edit_as(clients[r], &edits).unwrap();
                }
            }
            Step::Parent(r, a, b) => {
                let l = live(&docs[r]);
                if !l.is_empty() {
                    let (child, parent) = (&l[a % l.len()], &l[b % l.len()]);
                    docs[r]
                        .edit_as(
                            clients[r],
                            &[set("props", format!("{child}{SEP}parent"), parent.as_str())],
                        )
                        .unwrap();
                }
            }
            Step::Sync(a, b) if a != b => {
                let (from, to) = pair(&mut docs, a, b);
                sync(from, to).unwrap();
            }
            Step::Sync(..) => {}
        }
        // every replica's states hold the invariants at every step, not only converged
        for d in &docs {
            check(&View::new(d.keyed_state().unwrap()));
        }
    }
    for _ in 0..2 {
        for a in 0..3 {
            for b in 0..3 {
                if a != b {
                    let (from, to) = pair(&mut docs, a, b);
                    sync(from, to).unwrap();
                }
            }
        }
    }
    let views: Vec<View> = docs
        .iter()
        .map(|d| View::new(d.keyed_state().unwrap()))
        .collect();
    let first = check(&views[0]);
    for v in &views[1..] {
        assert_eq!(check(v), first, "every replica computes the same states");
    }
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config::with_cases(64))]
    /// Three replicas add, delete, restore, link and re-parent items concurrently and sync in
    /// any order: every replica computes identical states, every link end keeps its entry and
    /// an orphan its hint, and cycles are cut the same in any list order.
    #[test]
    fn replicas_agree_on_every_reference(steps in proptest::collection::vec(step(), 1..50)) {
        under_struct_mutant(|| run(&steps));
    }
}
