//! k2: `Update::has_subdoc_content`, a decode-only check an embedder can run on an update from
//! an untrusted peer before applying it. Every update here is built through the normal API.

use yrs::updates::decoder::Decode;
use yrs::{Doc, Map, ReadTxn, StateVector, Text, Transact, Update};

fn update_of(doc: &Doc) -> Vec<u8> {
    doc.transact().encode_state_as_update_v1(&StateVector::default())
}

#[test]
fn plain_text_and_map_updates_have_no_subdoc_content() {
    let doc = Doc::with_client_id(1);
    let text = doc.get_or_insert_text("t");
    let map = doc.get_or_insert_map("m");
    {
        let mut txn = doc.transact_mut();
        text.insert(&mut txn, 0, "hello");
        map.insert(&mut txn, "k", "v");
        map.insert(&mut txn, "n", 3);
    }
    let u = Update::decode_v1(&update_of(&doc)).unwrap();
    assert!(!u.has_subdoc_content());
}

#[test]
fn an_update_with_a_subdocument_is_detected() {
    let doc = Doc::with_client_id(1);
    let map = doc.get_or_insert_map("m");
    {
        let mut txn = doc.transact_mut();
        map.insert(&mut txn, "plain", "v");
        map.insert(&mut txn, "sub", Doc::new());
    }
    let u = Update::decode_v1(&update_of(&doc)).unwrap();
    assert!(u.has_subdoc_content());
}

#[test]
fn a_subdocument_nested_in_a_map_is_detected() {
    let doc = Doc::with_client_id(1);
    let outer = doc.get_or_insert_map("m");
    {
        let mut txn = doc.transact_mut();
        let inner = outer.insert(&mut txn, "inner", yrs::MapPrelim::default());
        inner.insert(&mut txn, "sub", Doc::new());
    }
    let u = Update::decode_v1(&update_of(&doc)).unwrap();
    assert!(u.has_subdoc_content());
}

#[test]
fn checking_does_not_change_a_document() {
    // decode + check only: the receiving doc is untouched until the caller applies
    let src = Doc::with_client_id(1);
    let map = src.get_or_insert_map("m");
    map.insert(&mut src.transact_mut(), "sub", Doc::new());
    let bytes = update_of(&src);
    let dst = Doc::with_client_id(2);
    let before = dst.transact().state_vector();
    let u = Update::decode_v1(&bytes).unwrap();
    assert!(u.has_subdoc_content());
    drop(u);
    assert_eq!(dst.transact().state_vector(), before);
}
