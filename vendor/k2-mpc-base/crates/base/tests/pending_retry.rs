//! A client's updates delivered to the kit out of order (v0.9.5): A writes `a` (A:0) and then,
//! independently, `z` at the end (A:1); B, having A:0, types `b` after `a` (B:0, origin A:0).
//! The kit's doc gets A:1, then B:0, then A:0.
//!
//! The kit never holds an update pending silently: a client update that skips ahead of a
//! client's history is refused (`MissingDeps`, nothing changes), and one that leaves anything
//! pending poisons the doc (`Pending`: it refuses to encode or edit, and a session reloads
//! it). The client resyncs by state vector, and every edit lands.

use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{Doc as YDoc, OffsetKind, Options, ReadTxn, Text, TextRef, Transact, Update};

use k2_mpc_base::{Doc, DocError, Kind, Schema};

const S: Schema = Schema {
    name: "test",
    roots: &[(ROOT, Kind::Text)],
};
const ROOT: &str = "source";

/// A raw yrs client (a Yjs stand-in) holding what `from` holds.
fn client(id: u64, from: &Doc) -> (YDoc, TextRef) {
    let d = YDoc::with_options(Options {
        client_id: yrs::block::ClientID::new(id),
        offset_kind: OffsetKind::Utf16,
        ..Options::default()
    });
    let t = d.get_or_insert_text(ROOT);
    catch_up(&d, from);
    (d, t)
}

fn catch_up(d: &YDoc, from: &Doc) {
    let u = from
        .encode_diff(&d.transact().state_vector().encode_v1())
        .unwrap();
    d.transact_mut()
        .apply_update(Update::decode_v1(&u).unwrap())
        .unwrap();
}

/// `s` typed at UTF-16 offset `at`, as one update.
fn typed(c: &(YDoc, TextRef), at: u32, s: &str) -> Vec<u8> {
    let sv = c.0.transact().state_vector();
    c.1.insert(&mut c.0.transact_mut(), at, s);
    c.0.transact().encode_state_as_update_v1(&sv)
}

/// What a client sends on reconnect: everything it has past the doc's state vector.
fn resync(c: &(YDoc, TextRef), doc: &Doc) -> Vec<u8> {
    let sv = yrs::StateVector::decode_v1(&doc.state_vector()).unwrap();
    c.0.transact().encode_state_as_update_v1(&sv)
}

#[test]
fn out_of_order_client_updates_are_refused_never_held_pending_and_resync_lands_them() {
    let mut doc = Doc::new(S).unwrap();
    doc.replace_text_as(7, ROOT, "xy\n").unwrap();
    let stored = doc.encode_state().unwrap();
    let a = client(100, &doc);
    let b = client(200, &doc);
    let a0 = typed(&a, 0, "a");
    let a1 = typed(&a, 4, "z"); // at the end: its origins are the base's, not A:0
    b.0.transact_mut()
        .apply_update(Update::decode_v1(&a0).unwrap())
        .unwrap();
    let b0 = typed(&b, 1, "b"); // after `a`: origin A:0

    // A:1 before A:0: refused, nothing changes, the doc stays usable
    assert_eq!(
        doc.apply_client_update(&a1, 100).map(|_| ()),
        Err(DocError::MissingDeps {
            client: 100,
            from: 1,
            have: 0
        })
    );
    assert_eq!(doc.text(ROOT).unwrap(), "xy\n");
    assert!(doc.encode_state().is_ok(), "nothing pending");
    // B:0 before A:0: it would wait for A:0; the doc refuses to hold it silently
    assert_eq!(
        doc.apply_client_update(&b0, 200).map(|_| ()),
        Err(DocError::Pending)
    );
    assert_eq!(
        doc.encode_state(),
        Err(DocError::Pending),
        "poisoned, not served"
    );
    assert_eq!(
        doc.apply_client_update(&a0, 100).map(|_| ()),
        Err(DocError::Pending),
        "a poisoned doc takes nothing more"
    );

    // the session reloads from its store; the clients resync by state vector, in any order
    let mut doc = Doc::load(S, &stored).unwrap();
    doc.apply_client_update(&resync(&b, &doc), 200)
        .map(|_| ())
        .expect_err("B's resync still needs A:0");
    let mut doc = Doc::load(S, &stored).unwrap();
    doc.apply_client_update(&resync(&a, &doc), 100).unwrap();
    doc.apply_client_update(&resync(&b, &doc), 200).unwrap();
    assert_eq!(doc.text(ROOT).unwrap(), "abxy\nz");
    assert!(doc.encode_state().is_ok(), "nothing pending");
    // and the clients converge with it
    catch_up(&a.0, &doc);
    catch_up(&b.0, &doc);
    for c in [&a, &b] {
        assert!(c.0.transact().store().pending_update().is_none());
        assert_eq!(yrs::GetString::get_string(&c.1, &c.0.transact()), "abxy\nz");
    }
}
