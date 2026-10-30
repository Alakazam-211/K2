//! k2: a subdocument whose item is deleted and then garbage collected (its content dropped, the
//! item merged into a neighbour or replaced by a GC block) is detached from that item first.
//! After the commit it has no parent and the parent doc no longer lists it. Every case is built
//! through the normal API.

use std::sync::{Arc, Mutex};
use yrs::updates::decoder::Decode;
use yrs::{
    ClientID, Doc, GetString, Map, MapPrelim, MapRef, Options, Out, ReadTxn, StateVector, Text,
    Transact, Update,
};

type Events = Arc<Mutex<Vec<(Vec<String>, Vec<String>)>>>;

/// records (added, removed) subdoc guids of every subdocs event on `doc`
fn record(doc: &Doc) -> Events {
    let events: Events = Arc::default();
    let sink = events.clone();
    doc.observe_subdocs("k2-test", move |_, e| {
        let added = e.added().map(|d| d.guid().to_string()).collect();
        let removed = e.removed().map(|d| d.guid().to_string()).collect();
        sink.lock().unwrap().push((added, removed));
    })
    .unwrap();
    events
}

fn removed_guids(events: &Events) -> Vec<String> {
    let e = events.lock().unwrap();
    e.iter().flat_map(|(_, r)| r.iter().cloned()).collect()
}

fn subdoc_guids(doc: &Doc) -> Vec<String> {
    doc.transact()
        .subdocs()
        .map(|d| d.guid().to_string())
        .collect()
}

fn update(doc: &Doc, sv: &StateVector) -> Vec<u8> {
    doc.transact().encode_state_as_update_v1(sv)
}

fn apply(doc: &Doc, u: &[u8]) {
    doc.transact_mut()
        .apply_update(Update::decode_v1(u).unwrap())
        .unwrap();
}

/// a sender that never collects garbage, so only the receiver frees anything
fn sender() -> Doc {
    let mut o = Options::with_client_id(ClientID::new(1));
    o.skip_gc = true;
    Doc::with_options(o)
}

fn assert_detached(sub: &Doc) {
    assert!(sub.parent_doc().is_none(), "subdoc still has a parent doc");
    assert!(sub.branch_id().is_none(), "subdoc still has a branch id");
}

fn map_value(doc: &Doc, map: &MapRef, key: &str) -> Option<Out> {
    map.get(&doc.transact(), key)
}

/// a still-integrated subdoc is attached: the baseline the other cases are checked against
#[test]
fn a_live_subdocument_is_attached() {
    let d = Doc::with_client_id(1);
    let m = d.get_or_insert_map("m");
    let sub = m.insert(&mut d.transact_mut(), "k", Doc::new());
    assert!(sub.parent_doc().is_some());
    assert!(sub.branch_id().is_some());
    assert_eq!(subdoc_guids(&d), vec![sub.guid().to_string()]);
}

/// m[k] = 1; m[k] = subdoc; m[k] = 2, a transaction each: the subdoc's item is GC'd and
/// merged into its deleted neighbour in the third commit, before the subdoc is destroyed
#[test]
fn local_overwrite_then_squash() {
    let d = Doc::with_client_id(1);
    let events = record(&d);
    let m = d.get_or_insert_map("m");
    m.insert(&mut d.transact_mut(), "k", 1);
    let sub = m.insert(&mut d.transact_mut(), "k", Doc::new());
    m.insert(&mut d.transact_mut(), "k", 2);

    assert_detached(&sub);
    assert!(subdoc_guids(&d).is_empty());
    assert_eq!(removed_guids(&events), vec![sub.guid().to_string()]);
    assert_eq!(map_value(&d, &m, "k"), Some(Out::from(2)));
}

/// the subdoc is inserted and overwritten in the transaction that also deleted its neighbour:
/// it is never reported, and a clone the caller kept is detached
#[test]
fn local_insert_and_overwrite_in_one_transaction() {
    let d = Doc::with_client_id(1);
    let events = record(&d);
    let m = d.get_or_insert_map("m");
    let sub = Doc::new();
    {
        let mut t = d.transact_mut();
        m.insert(&mut t, "k", 1);
        m.insert(&mut t, "k", sub.clone());
        m.insert(&mut t, "k", 2);
    }
    assert_detached(&sub);
    assert!(subdoc_guids(&d).is_empty());
    assert!(removed_guids(&events).is_empty());
    assert_eq!(map_value(&d, &m, "k"), Some(Out::from(2)));
}

/// a peer applies the same history, one update per transaction
#[test]
fn remote_overwrite_then_squash() {
    let a = sender();
    let m = a.get_or_insert_map("m");
    {
        let mut t = a.transact_mut();
        m.insert(&mut t, "k", 1);
        m.insert(&mut t, "k", Doc::new());
    }
    let u1 = update(&a, &StateVector::default());
    let sv = a.transact().state_vector();
    m.insert(&mut a.transact_mut(), "k", 2);
    let u2 = update(&a, &sv);

    let b = Doc::with_client_id(2);
    let events = record(&b);
    let mb = b.get_or_insert_map("m");
    apply(&b, &u1);
    let sub = match map_value(&b, &mb, "k") {
        Some(Out::YDoc(sub)) => sub,
        other => panic!("expected a subdoc at m.k, got {:?}", other),
    };
    assert!(sub.parent_doc().is_some());
    apply(&b, &u2);

    assert_detached(&sub);
    assert!(subdoc_guids(&b).is_empty());
    assert_eq!(removed_guids(&events), vec![sub.guid().to_string()]);
    assert_eq!(map_value(&b, &mb, "k"), Some(Out::from(2)));

    // the receiver's collected state still converges with a fresh peer
    let c = Doc::with_client_id(3);
    apply(&c, &update(&b, &StateVector::default()));
    let mc = c.get_or_insert_map("m");
    assert_eq!(map_value(&c, &mc, "k"), Some(Out::from(2)));
}

/// a subdoc inside a nested map; the outer entry is overwritten, so the subdoc's item is GC'd
/// as a child of a deleted type and replaced by a GC block
#[test]
fn local_nested_parent_collected() {
    let d = Doc::with_client_id(1);
    let events = record(&d);
    let m = d.get_or_insert_map("m");
    let inner = m.insert(&mut d.transact_mut(), "n", MapPrelim::default());
    let sub = inner.insert(&mut d.transact_mut(), "s", Doc::new());
    m.insert(&mut d.transact_mut(), "n", 0);

    assert_detached(&sub);
    assert!(subdoc_guids(&d).is_empty());
    assert_eq!(removed_guids(&events), vec![sub.guid().to_string()]);
    assert_eq!(map_value(&d, &m, "n"), Some(Out::from(0)));
}

/// the nested case on a receiver
#[test]
fn remote_nested_parent_collected() {
    let a = sender();
    let m = a.get_or_insert_map("m");
    {
        let mut t = a.transact_mut();
        let inner = m.insert(&mut t, "n", MapPrelim::default());
        inner.insert(&mut t, "s", Doc::new());
    }
    let u1 = update(&a, &StateVector::default());
    let sv = a.transact().state_vector();
    m.insert(&mut a.transact_mut(), "n", 0);
    let u2 = update(&a, &sv);

    let b = Doc::with_client_id(2);
    let events = record(&b);
    apply(&b, &u1);
    assert_eq!(subdoc_guids(&b).len(), 1);
    let sub = b.transact().subdocs().next().unwrap().clone();
    apply(&b, &u2);

    assert_detached(&sub);
    assert!(subdoc_guids(&b).is_empty());
    assert_eq!(removed_guids(&events), vec![sub.guid().to_string()]);
}

/// with GC off nothing is freed: the removed subdoc is destroyed as before (and detached), and
/// a later explicit GC of the whole doc collects the item without reading anything freed
#[test]
fn skip_gc_then_explicit_gc() {
    let d = sender();
    let events = record(&d);
    let m = d.get_or_insert_map("m");
    m.insert(&mut d.transact_mut(), "k", 1);
    let sub = m.insert(&mut d.transact_mut(), "k", Doc::new());
    m.insert(&mut d.transact_mut(), "k", 2);
    assert_detached(&sub);
    assert_eq!(removed_guids(&events), vec![sub.guid().to_string()]);

    d.transact_mut().gc(None);
    d.transact_mut().gc(None);
    assert!(subdoc_guids(&d).is_empty());
    assert_eq!(map_value(&d, &m, "k"), Some(Out::from(2)));
}

/// a subdoc that is still live keeps its parent and keeps working after a neighbouring subdoc
/// is collected
#[test]
fn a_live_subdocument_survives_its_neighbours_collection() {
    let d = Doc::with_client_id(1);
    let m = d.get_or_insert_map("m");
    let gone = m.insert(&mut d.transact_mut(), "a", Doc::new());
    let kept = m.insert(&mut d.transact_mut(), "b", Doc::new());
    m.insert(&mut d.transact_mut(), "a", 1);
    assert_detached(&gone);
    assert!(kept.parent_doc().is_some());
    assert_eq!(subdoc_guids(&d), vec![kept.guid().to_string()]);

    kept.load(&mut d.transact_mut());
    let t = kept.get_or_insert_text("t");
    t.insert(&mut kept.transact_mut(), 0, "still here");
    assert_eq!(t.get_string(&kept.transact()), "still here");
}
