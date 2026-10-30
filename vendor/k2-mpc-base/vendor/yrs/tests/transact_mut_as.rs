//! k2: `Doc::transact_mut_as(client)`, a transaction on the doc itself whose changes are made
//! as another client.
//!
//! - **Byte equality** ([`as_client_update_is_the_replica_update`]): random cases of text
//!   (plain and formatted inserts, removals, formatting), map and array ops (scalars and nested
//!   types), by several agents, interleaved with the doc's own edits and remote peers' updates
//!   (some typed on an out-of-date copy). For every agent op, the update `transact_mut_as(X)`
//!   makes on the doc is byte-identical to the one a throwaway replica makes (a fresh doc with
//!   client ID `X`, loaded from the doc's full state), and the two docs' state vectors and
//!   contents are then equal. `YRS_AS_CASES` cases (default 300; a node runs 10,000 or more).
//! - Observers fire with the as-client update ([`observers_see_the_as_client_update`]).
//! - The doc's own client ID comes back after commit, after a panic in the transaction's scope
//!   and after a panic during commit ([`the_client_id_is_restored_on_every_path`]).
//! - A subdoc added in an as-client transaction shares the doc's own client ID
//!   ([`a_subdoc_added_as_a_client_keeps_the_doc_id`]).
//!
//! Redo in an as-client transaction is a unit test of the undo manager (`undo.rs`).

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex};

use yrs::block::ClientID;
use yrs::types::text::TextEvent;
use yrs::types::{Attrs, Observable, ToJson};
use yrs::updates::decoder::Decode;
use yrs::{
    Any, Array, ArrayPrelim, ArrayRef, Doc, GetString, Map, MapPrelim, MapRef, OffsetKind, Options,
    Out, ReadTxn, StateVector, Text, TextPrelim, TextRef, Transact, TransactionMut, Update,
};

const OWN: u64 = 1;
const AGENTS: [u64; 4] = [1001, 1002, 1003, 1004];
const REMOTES: [u64; 2] = [2001, 2002];

fn doc(client: u64, skip_gc: bool) -> Doc {
    Doc::with_options(Options {
        client_id: ClientID::new(client),
        offset_kind: OffsetKind::Utf16,
        skip_gc,
        ..Options::default()
    })
}

struct Roots {
    t: TextRef,
    m: MapRef,
    a: ArrayRef,
}

fn roots(d: &Doc) -> Roots {
    Roots {
        t: d.get_or_insert_text("t"),
        m: d.get_or_insert_map("m"),
        a: d.get_or_insert_array("a"),
    }
}

/// One op on a doc's roots, as data, so the same op can be applied to two docs.
#[derive(Debug, Clone)]
enum Op {
    TextInsert(u32, String),
    TextInsertFormatted(u32, String, &'static str, bool),
    TextRemove(u32, u32),
    TextFormat(u32, u32, &'static str, bool),
    MapSet(String, i64),
    MapSetText(String, String),
    MapSetMap(String, String, i64),
    MapRemove(String),
    ArrayInsert(u32, Vec<i64>),
    ArrayInsertNested(u32, Vec<i64>),
    ArrayRemove(u32, u32),
}

/// BMP-only pieces (one UTF-16 unit per char), so any index is a character boundary.
const PIECES: &[&str] = &["a", "bc", "é", "日本", " ", "\n", "xyz", "Ω"];
const KEYS: &[&str] = &["k1", "k2", "k3", "k4", "k5"];
/// Formatting marks. A case uses one (75%) or both. With both, yrs itself orders the format
/// items an insert or format makes by iterating a std `HashMap` of attributes (each with its
/// own random hasher state), so two replicas making the same op can order them differently:
/// those cases compare what the update holds (block IDs, delete set) and the results, not its
/// bytes. With one mark, every update is compared byte for byte.
const MARKS: &[&str] = &["bold", "italic"];

fn piece(rng: &mut fastrand::Rng) -> String {
    (0..rng.usize(1..4))
        .map(|_| PIECES[rng.usize(..PIECES.len())])
        .collect()
}

fn random_op(rng: &mut fastrand::Rng, d: &Doc, r: &Roots, marks: usize) -> Op {
    let txn = d.transact();
    let tl = r.t.len(&txn);
    let al = r.a.len(&txn);
    let key = KEYS[rng.usize(..KEYS.len())].to_owned();
    match rng.u32(0..100) {
        0..=29 => Op::TextInsert(rng.u32(0..=tl), piece(rng)),
        30..=37 => Op::TextInsertFormatted(
            rng.u32(0..=tl),
            piece(rng),
            MARKS[rng.usize(..marks)],
            rng.bool(),
        ),
        38..=51 if tl > 0 => {
            let at = rng.u32(0..tl);
            Op::TextRemove(at, rng.u32(1..=(tl - at).min(4)))
        }
        52..=59 if tl > 0 => {
            let at = rng.u32(0..tl);
            Op::TextFormat(
                at,
                rng.u32(1..=(tl - at)),
                MARKS[rng.usize(..marks)],
                rng.bool(),
            )
        }
        60..=67 => Op::MapSet(key, rng.i64(-50..50)),
        68..=71 => Op::MapSetText(key, piece(rng)),
        72..=74 => Op::MapSetMap(key, piece(rng), rng.i64(0..9)),
        75..=79 => Op::MapRemove(key),
        80..=89 => Op::ArrayInsert(
            rng.u32(0..=al),
            (0..rng.usize(1..4)).map(|_| rng.i64(0..99)).collect(),
        ),
        90..=93 => Op::ArrayInsertNested(rng.u32(0..=al), vec![rng.i64(0..9), rng.i64(0..9)]),
        94..=99 if al > 0 => {
            let at = rng.u32(0..al);
            Op::ArrayRemove(at, rng.u32(1..=(al - at).min(3)))
        }
        _ => Op::TextInsert(rng.u32(0..=tl), piece(rng)),
    }
}

fn attrs(mark: &str, on: bool) -> Attrs {
    let mut a = Attrs::new();
    a.insert(
        Arc::from(mark),
        if on { Any::Bool(true) } else { Any::Null },
    );
    a
}

fn apply(op: &Op, txn: &mut TransactionMut, r: &Roots) {
    match op {
        Op::TextInsert(at, s) => r.t.insert(txn, *at, s),
        Op::TextInsertFormatted(at, s, mark, on) => {
            r.t.insert_with_attributes(txn, *at, s, attrs(mark, *on))
        }
        Op::TextRemove(at, n) => r.t.remove_range(txn, *at, *n),
        Op::TextFormat(at, n, mark, on) => r.t.format(txn, *at, *n, attrs(mark, *on)),
        Op::MapSet(k, v) => {
            r.m.insert(txn, k.as_str(), *v);
        }
        Op::MapSetText(k, s) => {
            r.m.insert(txn, k.as_str(), TextPrelim::new(s.as_str()));
        }
        Op::MapSetMap(k, s, v) => {
            // one key per nested map: a MapPrelim is a HashMap, iterated in its own random
            // order, so two built alike may integrate their keys in different orders
            let mut inner = MapPrelim::default();
            inner.insert(Arc::from(format!("{s}{v}").as_str()), (*v).into());
            r.m.insert(txn, k.as_str(), inner);
        }
        Op::MapRemove(k) => {
            r.m.remove(txn, k);
        }
        Op::ArrayInsert(at, vs) => {
            r.a.insert_range(txn, *at, vs.clone());
        }
        Op::ArrayInsertNested(at, vs) => {
            r.a.insert(txn, *at, ArrayPrelim::from(vs.clone()));
        }
        Op::ArrayRemove(at, n) => r.a.remove_range(txn, *at, *n),
    }
}

fn sv(d: &Doc) -> Vec<(u64, u32)> {
    let mut v: Vec<(u64, u32)> = d
        .transact()
        .state_vector()
        .iter()
        .map(|(c, k)| (c.get(), *k))
        .collect();
    v.sort_unstable();
    v
}

/// The roots' contents: the text with its formatting, and the map and array as values (a
/// map compares by entries, whatever its order).
fn content(d: &Doc, r: &Roots) -> (Vec<yrs::types::text::Diff<()>>, Any, Any) {
    let txn = d.transact();
    (r.t.diff(&txn, |_| ()), r.m.to_json(&txn), r.a.to_json(&txn))
}

fn apply_update(d: &Doc, u: &[u8]) {
    d.transact_mut()
        .apply_update(Update::decode_v1(u).unwrap())
        .unwrap();
}

/// One random case: `(agent ops compared, of which byte for byte, steps)`; panics at the
/// first difference.
fn case(seed: u64) -> (usize, usize, usize) {
    let mut rng = fastrand::Rng::with_seed(seed);
    let skip_gc = rng.u32(0..4) == 0;
    let marks = if rng.u32(0..4) == 0 { 2 } else { 1 };
    let main = doc(OWN, skip_gc);
    let mr = roots(&main);
    let remotes: Vec<(Doc, Roots)> = REMOTES
        .iter()
        .map(|c| {
            let d = doc(*c, skip_gc);
            let r = roots(&d);
            (d, r)
        })
        .collect();
    let steps = rng.usize(10..60);
    let (mut compared, mut strict) = (0, 0);
    for step in 0..steps {
        match rng.u32(0..100) {
            // an agent's op: as X on the doc, and on a throwaway replica with client X
            0..=54 => {
                let x = AGENTS[rng.usize(..AGENTS.len())];
                let op = random_op(&mut rng, &main, &mr, marks);
                let replica = doc(x, skip_gc);
                let rr = roots(&replica);
                let full = main
                    .transact()
                    .encode_state_as_update_v1(&StateVector::default());
                apply_update(&replica, &full);
                let want = {
                    let mut txn = replica.transact_mut();
                    apply(&op, &mut txn, &rr);
                    txn.encode_update_v1()
                };
                let got = {
                    let mut txn = main.transact_mut_as(ClientID::new(x));
                    apply(&op, &mut txn, &mr);
                    txn.encode_update_v1()
                };
                if marks == 1 {
                    assert_eq!(
                        got, want,
                        "seed {seed} step {step}: the update of {op:?} as {x} differs from the replica's"
                    );
                    strict += 1;
                } else {
                    let (g, w) = (
                        Update::decode_v1(&got).unwrap(),
                        Update::decode_v1(&want).unwrap(),
                    );
                    assert_eq!(
                        (g.insertions(true), g.delete_set()),
                        (w.insertions(true), w.delete_set()),
                        "seed {seed} step {step}: the blocks of {op:?} as {x} differ from the replica's"
                    );
                }
                assert_eq!(
                    sv(&main),
                    sv(&replica),
                    "seed {seed} step {step}: state vectors"
                );
                assert_eq!(
                    content(&main, &mr),
                    content(&replica, &rr),
                    "seed {seed} step {step}: contents"
                );
                assert_eq!(main.client_id(), ClientID::new(OWN));
                compared += 1;
            }
            // the doc's own edit
            55..=69 => {
                let op = random_op(&mut rng, &main, &mr, marks);
                apply(&op, &mut main.transact_mut(), &mr);
            }
            // a remote peer's edit (on a synced copy, or concurrent on an out-of-date one)
            _ => {
                let (rd, rroots) = &remotes[rng.usize(..remotes.len())];
                if rng.u32(0..10) < 7 {
                    let s = rd.transact().state_vector();
                    let diff = main.transact().encode_diff_v1(&s);
                    apply_update(rd, &diff);
                }
                let op = random_op(&mut rng, rd, rroots, marks);
                let u = {
                    let mut txn = rd.transact_mut();
                    apply(&op, &mut txn, rroots);
                    txn.encode_update_v1()
                };
                apply_update(&main, &u);
            }
        }
        let txn = main.transact();
        assert!(
            txn.store().pending_update().is_none(),
            "seed {}: pending",
            seed
        );
    }
    (compared, strict, steps)
}

#[test]
fn as_client_update_is_the_replica_update() {
    let cases: u64 = std::env::var("YRS_AS_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(300);
    let first: u64 = std::env::var("YRS_AS_SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let (mut compared, mut strict, mut steps) = (0, 0, 0);
    for seed in first..first + cases {
        let (c, b, s) = case(seed);
        compared += c;
        strict += b;
        steps += s;
    }
    eprintln!(
        "[transact_mut_as] {cases} cases (seeds {first}..{}), {steps} steps, {compared} agent ops \
         ({strict} byte for byte; the rest two-mark: same blocks and delete set); state vectors \
         and contents equal after each",
        first + cases
    );
    assert!(
        strict * 2 > compared,
        "too few byte-for-byte comparisons: {}",
        strict
    );
    assert!(
        compared as u64 > cases * 5,
        "too few agent ops: {}",
        compared
    );
}

#[test]
fn observers_see_the_as_client_update() {
    let d = doc(OWN, false);
    let t = d.get_or_insert_text("t");
    t.insert(&mut d.transact_mut(), 0, "base ");
    let updates: Arc<Mutex<Vec<Vec<u8>>>> = Arc::default();
    let u2 = updates.clone();
    let _sub = d
        .observe_update_v1("k2", move |_, e| u2.lock().unwrap().push(e.update.clone()))
        .unwrap();
    let deltas: Arc<Mutex<Vec<String>>> = Arc::default();
    let d2 = deltas.clone();
    t.observe("k2", move |txn, e: &TextEvent| {
        d2.lock().unwrap().push(format!("{:?}", e.delta(txn)));
    });
    let mine = {
        let mut txn = d.transact_mut_as(ClientID::new(77));
        t.insert(&mut txn, 5, "as 77");
        t.remove_range(&mut txn, 0, 1);
        txn.encode_update_v1()
    };
    let seen = updates.lock().unwrap().clone();
    assert_eq!(
        seen,
        vec![mine.clone()],
        "the update observer saw the as-77 update"
    );
    let u = Update::decode_v1(&mine).unwrap();
    let clients: Vec<u64> = u.state_vector().iter().map(|(c, _)| c.get()).collect();
    assert_eq!(clients, vec![77], "its new blocks are 77's");
    assert_eq!(
        deltas.lock().unwrap().len(),
        1,
        "the text observer fired once"
    );
    assert!(deltas.lock().unwrap()[0].contains("as 77"));
    assert_eq!(d.client_id(), ClientID::new(OWN));
}

/// The doc's own ID is back after commit, after a panic while the transaction is open (it
/// commits as it unwinds) and after a panic in an observer during commit.
#[test]
fn the_client_id_is_restored_on_every_path() {
    let d = doc(OWN, false);
    let t = d.get_or_insert_text("t");
    let own_next = |d: &Doc, t: &TextRef| {
        let before = sv(d);
        t.insert(&mut d.transact_mut(), 0, "o");
        let after = sv(d);
        (before, after)
    };
    let clock = |v: &[(u64, u32)], c: u64| v.iter().find(|x| x.0 == c).map_or(0, |x| x.1);
    // 1. commit on drop
    {
        let mut txn = d.transact_mut_as(ClientID::new(70));
        t.insert(&mut txn, 0, "a");
    }
    let (b, a) = own_next(&d, &t);
    assert_eq!(clock(&a, OWN), clock(&b, OWN) + 1, "the own client wrote");
    assert_eq!(clock(&a, 70), 1);
    // 2. a panic while the transaction is open
    let r = catch_unwind(AssertUnwindSafe(|| {
        let mut txn = d.transact_mut_as(ClientID::new(71));
        t.insert(&mut txn, 0, "b");
        panic!("in the transaction's scope");
    }));
    assert!(r.is_err());
    let (b, a) = own_next(&d, &t);
    assert_eq!(clock(&a, OWN), clock(&b, OWN) + 1);
    assert_eq!(clock(&a, 71), 1, "71's insert committed as it unwound");
    // 3. a panic in an observer, during commit
    t.observe("boom", |_, _: &TextEvent| panic!("observer"));
    let r = catch_unwind(AssertUnwindSafe(|| {
        let mut txn = d.transact_mut_as(ClientID::new(72));
        t.insert(&mut txn, 0, "c");
    }));
    assert!(r.is_err(), "the observer's panic carries on");
    assert!(t.unobserve("boom"));
    let (b, a) = own_next(&d, &t);
    assert_eq!(
        clock(&a, OWN),
        clock(&b, OWN) + 1,
        "the own client wrote after the panic"
    );
    assert_eq!(clock(&a, 72), 1);
    assert_eq!(d.client_id(), ClientID::new(OWN));
    // and a normal transaction made after all this is the doc's own
    let u = {
        let mut txn = d.transact_mut();
        t.insert(&mut txn, 0, "z");
        txn.encode_update_v1()
    };
    let u = Update::decode_v1(&u).unwrap();
    assert_eq!(
        u.state_vector_lower()
            .iter()
            .map(|(c, _)| c.get())
            .collect::<Vec<_>>(),
        vec![OWN]
    );
    assert_eq!(t.get_string(&d.transact()).len(), 7);
}

/// Step 10 of commit gives each subdoc added in the transaction the store's client ID. The
/// override ends before it: a subdoc added as client 80 shares the doc's own ID (as it would
/// in any transaction), while the item that holds it is 80's.
#[test]
fn a_subdoc_added_as_a_client_keeps_the_doc_id() {
    let d = doc(OWN, false);
    let m = d.get_or_insert_map("m");
    {
        let mut txn = d.transact_mut_as(ClientID::new(80));
        m.insert(&mut txn, "sub", Doc::new());
    }
    let sub = match m.get(&d.transact(), "sub") {
        Some(Out::YDoc(s)) => s,
        other => panic!("not a subdoc: {:?}", other),
    };
    assert_eq!(
        sub.client_id(),
        ClientID::new(OWN),
        "the subdoc has the doc's ID, not 80"
    );
    assert_eq!(sv(&d), vec![(80, 1)], "the item holding it is 80's");
    // its own edits are the doc's client's
    let st = sub.get_or_insert_text("s");
    let u = {
        let mut txn = sub.transact_mut();
        st.insert(&mut txn, 0, "x");
        txn.encode_update_v1()
    };
    let u = Update::decode_v1(&u).unwrap();
    assert_eq!(
        u.state_vector()
            .iter()
            .map(|(c, _)| c.get())
            .collect::<Vec<_>>(),
        vec![OWN]
    );
    // the control: added in an ordinary transaction, the same
    {
        let mut txn = d.transact_mut();
        m.insert(&mut txn, "sub2", Doc::new());
    }
    let sub2 = match m.get(&d.transact(), "sub2") {
        Some(Out::YDoc(s)) => s,
        other => panic!("not a subdoc: {:?}", other),
    };
    assert_eq!(sub2.client_id(), ClientID::new(OWN));
}
