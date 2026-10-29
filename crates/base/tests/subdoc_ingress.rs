//! The kit never uses subdocuments, so it refuses client updates that carry them (v0.9.5):
//! at every ingress, before anything is applied, with `DocError::Unaccepted` (exit 3). A
//! stored doc holding one is quarantined at load (`DocError::Quarantined`). Every update and
//! stored doc here is built through Yrs's normal API.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{
    Doc as YDoc, Map, OffsetKind, Options, ReadTxn, StateVector, Text, TextRef, Transact, Update,
};

use k2_mpc_base::session::{
    AuditEvent, Config, Host, Pack, Participant, Rights, SessionError, SessionManager, TextFilePack,
};
use k2_mpc_base::store::{DocKey, DocStore, LogEntry};
use k2_mpc_base::tools::{exit, session_exit};
use k2_mpc_base::{Doc, DocError, Kind, Schema};

const S: Schema = Schema {
    name: "test",
    roots: &[(ROOT, Kind::Text)],
};
const ROOT: &str = "source";
const MD: TextFilePack = TextFilePack::new("markdown", 1 << 20);

/// A raw yrs client holding what `from` holds.
fn client(id: u64, from: &Doc) -> (YDoc, TextRef) {
    let d = YDoc::with_options(Options {
        client_id: yrs::block::ClientID::new(id),
        offset_kind: OffsetKind::Utf16,
        ..Options::default()
    });
    let t = d.get_or_insert_text(ROOT);
    let u = from
        .encode_diff(&d.transact().state_vector().encode_v1())
        .unwrap();
    d.transact_mut()
        .apply_update(Update::decode_v1(&u).unwrap())
        .unwrap();
    (d, t)
}

/// What `c` has past `doc`'s state, as the client would send it.
fn since(c: &YDoc, doc: &Doc) -> Vec<u8> {
    let sv = StateVector::decode_v1(&doc.state_vector()).unwrap();
    c.transact().encode_state_as_update_v1(&sv)
}

fn base() -> Doc {
    let mut d = Doc::new(S).unwrap();
    d.replace_text_as(7, ROOT, "line 1\nline 2\n").unwrap();
    d
}

/// The doc's text, state vector and full state: all unchanged by a refused update.
fn snapshot(d: &Doc) -> (String, Vec<u8>, Vec<u8>) {
    (
        d.text(ROOT).unwrap(),
        d.state_vector(),
        d.encode_state().unwrap(),
    )
}

/// A client's change, made on its raw doc.
type MakeUpdate = fn(&YDoc, &TextRef);

#[test]
fn a_client_update_carrying_a_subdocument_is_refused_unapplied() {
    let cases: [(&str, MakeUpdate); 3] = [
        ("a subdocument in a map", |c, _| {
            let m = c.get_or_insert_map("m");
            m.insert(&mut c.transact_mut(), "sub", YDoc::new());
        }),
        ("with ordinary text in the same update", |c, t| {
            let m = c.get_or_insert_map("m");
            let mut txn = c.transact_mut();
            t.insert(&mut txn, 0, "typed ");
            m.insert(&mut txn, "sub", YDoc::new());
        }),
        ("nested in a map in a map", |c, _| {
            let m = c.get_or_insert_map("m");
            let mut txn = c.transact_mut();
            let inner = m.insert(&mut txn, "inner", yrs::MapPrelim::default());
            inner.insert(&mut txn, "sub", YDoc::new());
        }),
    ];
    for (what, make) in cases {
        let mut doc = base();
        let c = client(100, &doc);
        make(&c.0, &c.1);
        let u = since(&c.0, &doc);
        let before = snapshot(&doc);
        assert_eq!(
            doc.check_client_update(&u, 100),
            Err(DocError::Unaccepted),
            "{what}: checked"
        );
        assert_eq!(
            doc.apply_client_update(&u, 100),
            Err(DocError::Unaccepted),
            "{what}: applied"
        );
        assert_eq!(
            snapshot(&doc),
            before,
            "{what}: never integrated, nothing pending"
        );
        // the doc stays usable, and the client's ordinary edits still land
        let c2 = client(101, &doc);
        c2.1.insert(&mut c2.0.transact_mut(), 0, "ok ");
        doc.apply_client_update(&since(&c2.0, &doc), 101).unwrap();
        assert_eq!(doc.text(ROOT).unwrap(), "ok line 1\nline 2\n", "{what}");
    }
    // the refusal is plain, and exits 3 (refused)
    assert_eq!(
        DocError::Unaccepted.to_string(),
        "update contains content this host doesn't accept"
    );
    assert_eq!(
        session_exit(&SessionError::Doc(DocError::Unaccepted)),
        exit::REFUSED
    );
    assert_eq!(k2_mpc_base::doc::RESERVED_ERROR_ID_UNACCEPTED, None);
}

#[test]
fn every_ordinary_client_update_is_still_accepted() {
    let mut doc = base();
    let c = client(100, &doc);
    // an insert, a delete, a replace, each its own update
    c.1.insert(&mut c.0.transact_mut(), 0, "a ");
    let u = since(&c.0, &doc);
    assert!(!doc.apply_client_update(&u, 100).unwrap().delete_only);
    c.1.remove_range(&mut c.0.transact_mut(), 0, 2);
    let u =
        c.0.transact()
            .encode_state_as_update_v1(&StateVector::default());
    assert!(doc.apply_client_update(&u, 100).unwrap().delete_only);
    {
        let mut txn = c.0.transact_mut();
        c.1.remove_range(&mut txn, 0, 4);
        c.1.insert(&mut txn, 0, "LINE");
    }
    let u = since(&c.0, &doc);
    doc.apply_client_update(&u, 100).unwrap();
    // formatted text and an embedded plain value carry no subdocument
    {
        let mut txn = c.0.transact_mut();
        let attrs: HashMap<Arc<str>, yrs::Any> =
            HashMap::from([(Arc::from("bold"), yrs::Any::Bool(true))]);
        c.1.insert_with_attributes(&mut txn, 0, "b", attrs);
        c.1.insert_embed(&mut txn, 0, yrs::Any::from("x"));
    }
    let u = since(&c.0, &doc);
    doc.apply_client_update(&u, 100).unwrap();
    assert!(doc.encode_state().is_ok(), "nothing pending");
    assert!(doc.text(ROOT).unwrap().contains("LINE 1\nline 2\n"));
}

// ---------- the store-load scan ----------

struct H(AtomicU64, Mutex<Vec<AuditEvent>>);

impl Host for H {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
    fn on_event(&self, ev: &AuditEvent) {
        self.1.lock().unwrap().push(ev.clone());
    }
}

/// A stored doc that holds subdocument content (written to the store through its own API,
/// as a log entry or as a compacted state) is quarantined: never loaded as live.
#[test]
fn a_stored_doc_holding_a_subdocument_is_quarantined_at_load() {
    for compacted in [false, true] {
        let dir =
            std::env::temp_dir().join(format!("k2mb-subdoc-{}-{compacted}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
        let host = Arc::new(H(AtomicU64::new(1_000), Mutex::new(Vec::new())));
        let key = DocKey {
            space: "ws".into(),
            path: "a.md".into(),
        };
        let pack = || Arc::new(MD);
        let mgr = SessionManager::new(store.clone(), host.clone(), Config::default());
        let s = mgr.open(pack(), &key, Some(b"line 1\n")).unwrap();
        let text_before = s.status().unwrap().version;
        drop(s);
        mgr.release().unwrap();
        // the stored doc, as a raw client sees it, plus a subdocument
        let id = store.find(&key).unwrap().unwrap();
        let stored = Doc::from_store(MD.schema(), &store.load(id).unwrap()).unwrap();
        let c = YDoc::with_client_id(4242);
        c.transact_mut()
            .apply_update(Update::decode_v2(&stored.encode_state().unwrap()).unwrap())
            .unwrap();
        let sv = c.transact().state_vector();
        c.get_or_insert_map("m")
            .insert(&mut c.transact_mut(), "sub", YDoc::new());
        if compacted {
            let state = c
                .transact()
                .encode_state_as_update_v2(&StateVector::default());
            store.compact(id, text_before, &state).unwrap();
        } else {
            let update = c.transact().encode_state_as_update_v1(&sv);
            store
                .append(
                    id,
                    &[LogEntry {
                        author: "someone".into(),
                        client: 4242,
                        at_ms: 1_000,
                        via: "cli".into(),
                        update,
                        on_behalf_of: None,
                    }],
                )
                .unwrap();
        }
        // the kit's load refuses it, typed; so does a session open
        assert_eq!(
            Doc::from_store(MD.schema(), &store.load(id).unwrap()).err(),
            Some(DocError::Quarantined),
            "compacted {compacted}"
        );
        let mgr = SessionManager::new(store.clone(), host.clone(), Config::default());
        match mgr.open(pack(), &key, None) {
            Err(SessionError::Doc(DocError::Quarantined)) => {}
            other => panic!("compacted {compacted}: {:?}", other.map(|_| ())),
        }
        assert_eq!(
            session_exit(&SessionError::Doc(DocError::Quarantined)),
            exit::REFUSED
        );
        drop(mgr);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
