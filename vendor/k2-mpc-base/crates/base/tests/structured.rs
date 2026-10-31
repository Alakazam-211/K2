//! Structured kinds through the public API only: the applied stream replays.
//!
//! Property: for any sequence of changes (random list and map ops, some of them refused),
//! replaying each change's applied stream over the old state gives the new state, and so
//! does the stream a keyed diff derives for it (`Doc::applied_since`, how a merged update
//! reports what it did); the readers agree with each other, every write the stream reports
//! as winning is the register's `winner`, and the state survives a store round trip. A
//! planted negative shows the replay check fails on a corrupted stream.
//!
//! Also: the kit checks no value (NUL, size): that is the pack's per-op schema check (E10).
//!
//! `PROPTEST_CASES` scales it (256 by default; run more on a compute node).

use std::collections::{BTreeMap, HashMap};

use k2_mpc_base::elem::{
    Anchor, Effect, ElemState, Facet, ListApplied, ListOp, ListOpKind, MapApplied, MapOp,
    MapOpKind, WriteId,
};
use k2_mpc_base::{Applied, Doc, DocError, Edit, Key, Kind, Schema, Value};
use proptest::prelude::*;

const S: Schema = Schema {
    name: "grid",
    roots: &[("rows", Kind::MovableList), ("cells", Kind::Map)],
};

type List = Vec<(Key, Value)>;

/// Replay list ops over a live order (what a subscriber holding only the order does).
fn replay_list<W: std::fmt::Debug + PartialEq>(
    list: &mut List,
    ops: &[ListApplied<W>],
) -> Result<(), String> {
    for (i, o) in ops.iter().enumerate() {
        let bad = |why: &str| format!("op {i} ({o:?}): {why}");
        if o.effect != Effect::Visible {
            if o.index_before != o.index_after {
                return Err(bad("an op without a visible effect moved"));
            }
            continue;
        }
        let take = |list: &mut List, at: Option<usize>| -> Result<(), String> {
            let at = at.ok_or_else(|| bad("no index before"))?;
            match list.get(at) {
                Some((k, _)) if *k == o.key => {
                    list.remove(at);
                    Ok(())
                }
                other => Err(bad(&format!("index before holds {other:?}"))),
            }
        };
        let put = |list: &mut List, at: Option<usize>| -> Result<(), String> {
            let at = at.ok_or_else(|| bad("no index after"))?;
            if at > list.len() {
                return Err(bad("index after past the end"));
            }
            let v = o.value_after.clone().ok_or_else(|| bad("no value after"))?;
            list.insert(at, (o.key.clone(), v));
            Ok(())
        };
        match o.kind {
            ListOpKind::Insert | ListOpKind::Restore => put(list, o.index_after)?,
            ListOpKind::Delete => take(list, o.index_before)?,
            ListOpKind::Move => {
                take(list, o.index_before)?;
                put(list, o.index_after)?;
            }
            // a deleted element leaves for good: nothing shown changes
            ListOpKind::Drop => {}
            ListOpKind::Set => {
                let at = o.index_after.ok_or_else(|| bad("no index"))?;
                let slot = list.get_mut(at).ok_or_else(|| bad("index past the end"))?;
                if slot.0 != o.key {
                    return Err(bad("index holds another key"));
                }
                slot.1 = o.value_after.clone().ok_or_else(|| bad("no value after"))?;
            }
        }
    }
    Ok(())
}

fn replay_map<W: std::fmt::Debug>(
    map: &mut BTreeMap<String, Value>,
    ops: &[MapApplied<W>],
) -> Result<(), String> {
    for (i, o) in ops.iter().enumerate() {
        if map.get(&o.key) != o.before.as_ref() {
            return Err(format!(
                "op {i}: before is {:?}, not {:?}",
                o.before,
                map.get(&o.key)
            ));
        }
        match (o.kind, &o.after) {
            (MapOpKind::Set, Some(v)) => {
                map.insert(o.key.clone(), v.clone());
            }
            (MapOpKind::Delete, None) => {
                map.remove(&o.key);
            }
            _ => return Err(format!("op {i}: {o:?}")),
        }
    }
    Ok(())
}

/// Op recipes, resolved against the live keys at generation time (some become refused ops:
/// stale keys, self anchors, reused keys).
#[derive(Debug, Clone)]
enum R {
    Insert(u8, u8, bool),
    Move(u8, u8),
    Delete(u8),
    Restore(u8),
    Set(u8, bool),
    MapSet(u8, u8),
    MapDelete(u8),
    Chain(u8, u8, u8),
}

fn recipe() -> impl Strategy<Value = R> {
    prop_oneof![
        3 => (any::<u8>(), any::<u8>(), any::<bool>()).prop_map(|(a, b, c)| R::Insert(a, b, c)),
        3 => (any::<u8>(), any::<u8>()).prop_map(|(a, b)| R::Move(a, b)),
        2 => any::<u8>().prop_map(R::Delete),
        2 => any::<u8>().prop_map(R::Restore),
        2 => (any::<u8>(), any::<bool>()).prop_map(|(a, b)| R::Set(a, b)),
        2 => (any::<u8>(), any::<u8>()).prop_map(|(a, b)| R::MapSet(a, b)),
        1 => any::<u8>().prop_map(R::MapDelete),
        1 => (any::<u8>(), any::<u8>(), any::<u8>()).prop_map(|(a, b, c)| R::Chain(a, b, c)),
    ]
}

struct Gen {
    next_key: u32,
    /// Every key ever inserted (live or deleted), in insert order.
    known: Vec<Key>,
}

impl Gen {
    fn pick(&self, n: u8) -> Key {
        if self.known.is_empty() || n == 255 {
            // sometimes a key that was never inserted
            return Key::new(format!("ghost{n}")).unwrap();
        }
        self.known[n as usize % self.known.len()].clone()
    }

    fn anchor(&self, n: u8) -> Anchor {
        match n % 4 {
            0 => Anchor::Start,
            1 => Anchor::End,
            2 => Anchor::After(self.pick(n / 4)),
            _ => Anchor::Before(self.pick(n / 4)),
        }
    }

    fn edits(&mut self, rs: &[R]) -> Vec<Edit> {
        let mut list = Vec::new();
        let mut map = Vec::new();
        for r in rs {
            match r {
                R::Insert(a, b, v) => {
                    let key = if *b == 254 && !self.known.is_empty() {
                        self.pick(*a) // a reused key: refused
                    } else {
                        self.next_key += 1;
                        let k = Key::new(format!("r{}", self.next_key)).unwrap();
                        self.known.push(k.clone());
                        k
                    };
                    list.push(ListOp::Insert {
                        key,
                        at: self.anchor(*a),
                        value: if *v {
                            Value::str(format!("v{b}"))
                        } else {
                            Value::Null
                        },
                    });
                }
                R::Move(a, b) => list.push(ListOp::Move {
                    key: self.pick(*a),
                    to: self.anchor(*b),
                }),
                R::Delete(a) => list.push(ListOp::Delete { key: self.pick(*a) }),
                R::Restore(a) => list.push(ListOp::Restore { key: self.pick(*a) }),
                R::Set(a, null) => list.push(ListOp::Set {
                    key: self.pick(*a),
                    value: if *null {
                        Value::Null
                    } else {
                        Value::str(format!("s{a}"))
                    },
                }),
                R::MapSet(a, b) => map.push(MapOp::Set {
                    key: format!("r{}\u{1f}c{}", a % 8, b % 3),
                    value: Value::str(if b % 5 == 0 {
                        String::new()
                    } else {
                        format!("x{b}")
                    }),
                }),
                R::MapDelete(a) => map.push(MapOp::Delete {
                    key: format!("r{}\u{1f}c{}", a % 8, a % 3),
                }),
                R::Chain(a, n, b) => {
                    let mut keys: Vec<Key> = Vec::new();
                    for i in 0..(n % 4 + 1) {
                        let k = self.pick(a.wrapping_add(i * 7));
                        if !keys.contains(&k) {
                            keys.push(k);
                        }
                    }
                    list.extend(k2_mpc_base::elem::chain(&keys, self.anchor(*b)));
                }
            }
        }
        let mut out = Vec::new();
        if !list.is_empty() {
            out.push(Edit::List {
                root: "rows".into(),
                ops: list,
            });
        }
        if !map.is_empty() {
            out.push(Edit::Map {
                root: "cells".into(),
                ops: map,
            });
        }
        out
    }
}

/// Every reader agrees: `elem` with the order, `elem_value` with the values, `winner` with
/// the last write the streams reported as winning.
fn readers_agree(d: &Doc, gen: &Gen, wins: &HashMap<(String, Facet), WriteId>) {
    let list = d.list("rows").unwrap();
    for (i, (k, v)) in list.iter().enumerate() {
        assert_eq!(d.elem("rows", k).unwrap(), ElemState::Live(i));
        assert_eq!(d.elem_value("rows", k).unwrap().as_ref(), Some(v));
    }
    for k in &gen.known {
        if !list.iter().any(|(x, _)| x == k) {
            assert_eq!(d.elem("rows", k).unwrap(), ElemState::Deleted, "{k}");
        }
    }
    for ((key, facet), w) in wins {
        let root = if key.contains('\u{1f}') {
            "cells"
        } else {
            "rows"
        };
        assert_eq!(
            d.winner(root, key, *facet).unwrap(),
            Some(*w),
            "{root} {key:?} {facet:?}"
        );
    }
}

fn run(changes: Vec<Vec<R>>) -> Result<(), TestCaseError> {
    let mut d = Doc::new(S).unwrap();
    let mut gen = Gen {
        next_key: 0,
        known: Vec::new(),
    };
    let mut wins: HashMap<(String, Facet), WriteId> = HashMap::new();
    for (n, rs) in changes.iter().enumerate() {
        let edits = gen.edits(rs);
        let (old_list, old_map) = (d.list("rows").unwrap(), d.map("cells").unwrap());
        let state = d.encode_state().unwrap();
        let keyed = d.keyed_state().unwrap();
        let client = 10 + (n as u64 % 3);
        match d.edit_applied(client, &edits) {
            Err(DocError::Op { .. }) => {
                prop_assert_eq!(d.encode_state().unwrap(), state, "a refused change applied");
                // keys minted for a refused change were never inserted
                gen.known.retain(|k| {
                    old_list.iter().any(|(x, _)| x == k)
                        || d.elem("rows", k).unwrap() != ElemState::Unknown
                });
                continue;
            }
            Err(e) => return Err(TestCaseError::fail(format!("change {n}: {e}"))),
            Ok(e) => {
                let mut list = old_list.clone();
                let mut map = old_map.clone();
                for a in &e.applied {
                    match a {
                        Applied::List { ops, .. } => {
                            replay_list(&mut list, ops).map_err(TestCaseError::fail)?;
                            for o in ops {
                                let facet = match o.kind {
                                    ListOpKind::Insert | ListOpKind::Move => Some(Facet::Position),
                                    ListOpKind::Delete if o.effect == Effect::Visible => {
                                        Some(Facet::Existence)
                                    }
                                    ListOpKind::Set => Some(Facet::Value),
                                    _ => None,
                                };
                                if let Some(f) = facet {
                                    wins.insert((o.key.to_string(), f), o.write);
                                }
                                if o.kind == ListOpKind::Insert
                                    && o.value_after != Some(Value::Null)
                                {
                                    wins.insert((o.key.to_string(), Facet::Value), o.write);
                                }
                                if o.kind == ListOpKind::Restore && o.effect == Effect::Visible {
                                    wins.remove(&(o.key.to_string(), Facet::Existence));
                                }
                            }
                        }
                        Applied::Map { ops, .. } => {
                            replay_map(&mut map, ops).map_err(TestCaseError::fail)?;
                            for o in ops {
                                if o.kind == MapOpKind::Set {
                                    wins.insert((o.key.clone(), Facet::Value), o.write);
                                } else if o.effect == Effect::Visible {
                                    wins.remove(&(o.key.clone(), Facet::Value));
                                }
                            }
                        }
                        other => return Err(TestCaseError::fail(format!("{other:?}"))),
                    }
                }
                prop_assert_eq!(&list, &d.list("rows").unwrap(), "change {}: list replay", n);
                prop_assert_eq!(&map, &d.map("cells").unwrap(), "change {}: map replay", n);
                // the stream a keyed diff derives for the same change replays too
                let (mut dl, mut dm) = (old_list.clone(), old_map.clone());
                for a in d.applied_since(&keyed).unwrap() {
                    match a {
                        Applied::List { ops, .. } => {
                            replay_list(&mut dl, &ops).map_err(TestCaseError::fail)?
                        }
                        Applied::Map { ops, .. } => {
                            replay_map(&mut dm, &ops).map_err(TestCaseError::fail)?
                        }
                        other => return Err(TestCaseError::fail(format!("{other:?}"))),
                    }
                }
                prop_assert_eq!(&dl, &list, "change {}: derived list replay", n);
                prop_assert_eq!(&dm, &map, "change {}: derived map replay", n);
                if list != old_list || map != old_map {
                    prop_assert!(
                        e.update.is_some(),
                        "change {}: a visible change wrote nothing",
                        n
                    );
                }
                if e.update.is_none() {
                    prop_assert_eq!(d.encode_state().unwrap(), state, "change {}", n);
                }
            }
        }
        readers_agree(&d, &gen, &wins);
    }
    let again = Doc::load(S, &d.encode_state().unwrap()).unwrap();
    prop_assert_eq!(again.export_neutral().unwrap(), d.export_neutral().unwrap());
    readers_agree(&again, &gen, &wins);
    Ok(())
}

proptest! {
    #[test]
    fn replaying_the_applied_stream_gives_the_new_state(
        changes in prop::collection::vec(prop::collection::vec(recipe(), 1..8), 1..24)
    ) {
        run(changes)?;
    }
}

fn k(s: &str) -> Key {
    Key::new(s).unwrap()
}

/// Planted negative: a stream with one index off, or one op dropped, doesn't replay.
#[test]
fn planted_a_corrupted_stream_does_not_replay() {
    let mut d = Doc::new(S).unwrap();
    let e = d
        .edit_applied(
            7,
            &[Edit::List {
                root: "rows".into(),
                ops: vec![
                    ListOp::Insert {
                        key: k("a"),
                        at: Anchor::End,
                        value: Value::Null,
                    },
                    ListOp::Insert {
                        key: k("b"),
                        at: Anchor::End,
                        value: Value::Null,
                    },
                    ListOp::Insert {
                        key: k("c"),
                        at: Anchor::Start,
                        value: Value::Null,
                    },
                    ListOp::Move {
                        key: k("a"),
                        to: Anchor::End,
                    },
                ],
            }],
        )
        .unwrap();
    let Applied::List { ops, .. } = &e.applied[0] else {
        panic!()
    };
    let now = d.list("rows").unwrap();
    let mut good = Vec::new();
    replay_list(&mut good, ops).unwrap();
    assert_eq!(good, now);
    let mut off = ops.clone();
    off[3].index_after = Some(0);
    let mut l = Vec::new();
    assert!(replay_list(&mut l, &off).is_err() || l != now);
    let mut dropped = ops.clone();
    dropped.remove(2);
    let mut l = Vec::new();
    assert!(replay_list(&mut l, &dropped).is_err() || l != now);
}

/// E10: value checks are per op and belong to the pack's schema hook (the table layer), not
/// to the kit: a NUL in a value, and a value over the CSV contract's 64 KiB cap, are stored
/// and read back as given (list values and map values), here and after a reload. Map *keys*
/// are the kit's (`check_map_key`: no NUL, at most 256 bytes).
#[test]
fn the_kit_leaves_value_checks_to_the_pack() {
    let nul = Value::str("a\0b");
    let big = Value::Str("x".repeat(64 * 1024 + 1));
    let mut d = Doc::new(S).unwrap();
    d.edit_as(
        7,
        &[
            Edit::List {
                root: "rows".into(),
                ops: vec![
                    ListOp::Insert {
                        key: k("a"),
                        at: Anchor::End,
                        value: nul.clone(),
                    },
                    ListOp::Insert {
                        key: k("b"),
                        at: Anchor::End,
                        value: Value::Null,
                    },
                    ListOp::Set {
                        key: k("b"),
                        value: big.clone(),
                    },
                ],
            },
            Edit::Map {
                root: "cells".into(),
                ops: vec![
                    MapOp::Set {
                        key: "n".into(),
                        value: nul.clone(),
                    },
                    MapOp::Set {
                        key: "g".into(),
                        value: big.clone(),
                    },
                ],
            },
        ],
    )
    .unwrap()
    .unwrap();
    let again = Doc::load(S, &d.encode_state().unwrap()).unwrap();
    for doc in [&d, &again] {
        assert_eq!(
            doc.list("rows").unwrap(),
            vec![(k("a"), nul.clone()), (k("b"), big.clone())]
        );
        assert_eq!(doc.map_get("cells", "n").unwrap(), Some(nul.clone()));
        assert_eq!(doc.map_get("cells", "g").unwrap(), Some(big.clone()));
    }
    // a map key is the kit's to check
    let e = d.edit_as(
        7,
        &[Edit::Map {
            root: "cells".into(),
            ops: vec![MapOp::Set {
                key: "a\0b".into(),
                value: Value::Null,
            }],
        }],
    );
    assert!(matches!(e, Err(DocError::Op { .. })), "{e:?}");
}
