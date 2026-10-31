//! In-crate tests of the grid session (C1c): the lost-edit audit's planted violations
//! (K-AUD-1, grid form: a dropped cell write, a silent cell overwrite, a row lost in a move),
//! planted through the live doc (kit-made Yrs updates only) and the store; and KC7 (a write
//! with no identity counts as someone else's in the per-facet base check).

use super::*;
use crate::elem::{Anchor, Key, ListOp, MapOp, Value};
use crate::session::{EditOptions, SessionManager};
use crate::store::{ElemFacet, ElemOpKind, ElemWrite};
use crate::table::{self, Field, Shape, Table};

struct H;
impl Host for H {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::full().with_owner(true)
    }
    fn now_ms(&self) -> u64 {
        1_000
    }
    fn on_event(&self, _: &AuditEvent) {}
}

/// A one-record-per-line table pack (no quoting), as the ITs' `csv-lite`.
struct Lite;
const LITE: Schema = Schema {
    name: "csv-lite",
    roots: table::ROOTS,
};
impl Pack for Lite {
    fn schema(&self) -> Schema {
        LITE
    }
    fn decode(&self, b: &[u8]) -> std::result::Result<super::super::PackContent, Declined> {
        let recs: Vec<Vec<Field>> = std::str::from_utf8(b)
            .unwrap()
            .lines()
            .map(|l| l.split(',').map(Field::new).collect())
            .collect();
        let (_, edits) = Table::import(&recs, false, Shape::detect(&recs)).unwrap();
        let mut d = Doc::new(LITE).unwrap();
        d.edit_as(2, &edits).unwrap();
        Ok(super::super::PackContent {
            roots: d.export_neutral().unwrap(),
            codec: vec![1],
        })
    }
    fn encode(&self, c: &[(String, Neutral)], _: &[u8]) -> std::result::Result<Vec<u8>, PackError> {
        let t = View::of_content(c)
            .table()
            .map_err(|e| PackError(e.to_string()))?;
        let mut out = String::new();
        for rec in t.render() {
            let f: Vec<&str> = rec.iter().map(|f| f.value.as_str()).collect();
            out.push_str(&f.join(","));
            out.push('\n');
        }
        Ok(out.into_bytes())
    }
}

struct Fx {
    dir: std::path::PathBuf,
    store: Arc<DocStore>,
    s: Arc<Session>,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn fx(tag: &str, body: &str) -> Fx {
    let dir = std::env::temp_dir().join(format!("k2mb-gridunit-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let c = Config {
        edit_burst: 10_000,
        edit_rate_per_s: 10_000,
        ..Config::default()
    };
    let mgr = SessionManager::new(store.clone(), Arc::new(H), c);
    let key = DocKey {
        space: "ws".into(),
        path: format!("{tag}.csv"),
    };
    let s = mgr
        .open(Arc::new(Lite), &key, Some(body.as_bytes()))
        .unwrap();
    Fx { dir, store, s }
}

fn k(s: &str) -> Key {
    Key::new(s).unwrap()
}

fn join(s: &Session, pid: &str, kind: ParticipantKind) -> ConnId {
    s.join(
        Participant::new(pid, pid.to_uppercase(), "#111111", kind, "cli"),
        Stay::Held,
    )
    .unwrap()
    .conn
}

fn set(
    s: &Session,
    c: ConnId,
    r: &str,
    col: &str,
    v: &str,
    opts: &EditOptions,
) -> Result<EditResult> {
    s.edit_ops(
        c,
        vec![Edit::Map {
            root: table::CELLS.into(),
            ops: vec![MapOp::Set {
                key: table::cell_key(&k(r), &k(col)),
                value: Value::str(v),
            }],
        }],
        opts,
    )
}

/// An update a foreign client makes on a copy of the live doc: kit-made bytes, never in
/// the log, never recorded (what a lost or silent write looks like to the audit).
fn foreign(s: &Session, edits: &[Edit]) -> Vec<u8> {
    let st = s.lock().unwrap();
    let mut d = Doc::load(s.schema, &st.doc.encode_state().unwrap()).unwrap();
    d.edit_as(777_777, edits).unwrap().unwrap()
}

#[test]
fn audit_is_clean_on_an_honest_doc() {
    let f = fx("honest", "a,b\nc,d\n");
    let c = join(&f.s, "p", ParticipantKind::Person);
    set(&f.s, c, "r1", "c1", "A", &EditOptions::new()).unwrap();
    let rep = f.s.audit().unwrap();
    assert!(rep.is_clean(), "{rep:?}");
}

#[test]
fn audit_catches_a_silent_cell_overwrite() {
    let f = fx("silent", "a,b\nc,d\n");
    let c = join(&f.s, "p", ParticipantKind::Person);
    set(&f.s, c, "r1", "c1", "A", &EditOptions::new()).unwrap();
    let u = foreign(
        &f.s,
        &[Edit::Map {
            root: table::CELLS.into(),
            ops: vec![MapOp::Set {
                key: table::cell_key(&k("r1"), &k("c1")),
                value: Value::str("X"),
            }],
        }],
    );
    f.s.apply_raw_for_tests(&u).unwrap();
    let rep = f.s.audit().unwrap();
    assert!(
        rep.violations.contains(&AuditViolation::SilentWrite {
            root: table::CELLS.into(),
            key: table::cell_key(&k("r1"), &k("c1")),
            facet: crate::elem::Facet::Value,
        }),
        "{rep:?}"
    );
    assert!(rep.violations.contains(&AuditViolation::LogReplay));
}

#[test]
fn audit_catches_a_dropped_cell_write() {
    let f = fx("dropped", "a,b\nc,d\n");
    let c = join(&f.s, "p", ParticipantKind::Person);
    let before = f.s.lock().unwrap().doc.encode_state().unwrap();
    let r = set(&f.s, c, "r2", "c2", "D", &EditOptions::new()).unwrap();
    // the engine "loses" the acknowledged write: the live doc goes back to before it
    {
        let mut st = f.s.lock().unwrap();
        st.doc = Doc::load(f.s.schema, &before).unwrap();
    }
    let rep = f.s.audit().unwrap();
    assert!(
        rep.violations.contains(&AuditViolation::DroppedWrite {
            root: table::CELLS.into(),
            key: table::cell_key(&k("r2"), &k("c2")),
            facet: crate::elem::Facet::Value,
            change: r.version,
        }),
        "{rep:?}"
    );
}

#[test]
fn audit_catches_a_row_lost_in_a_move() {
    let f = fx("lost", "a\nb\nc\n");
    let c = join(&f.s, "p", ParticipantKind::Person);
    let r =
        f.s.edit_ops(
            c,
            vec![Edit::List {
                root: table::ROWS.into(),
                ops: vec![ListOp::Move {
                    key: k("r2"),
                    to: Anchor::Start,
                }],
            }],
            &EditOptions::new(),
        )
        .unwrap();
    // the row vanishes: a delete nobody recorded
    let u = foreign(
        &f.s,
        &[Edit::List {
            root: table::ROWS.into(),
            ops: vec![ListOp::Delete { key: k("r2") }],
        }],
    );
    f.s.apply_raw_for_tests(&u).unwrap();
    let rep = f.s.audit().unwrap();
    assert!(
        rep.violations.iter().any(|v| matches!(
            v,
            AuditViolation::LostElement { root, key, .. } if root == table::ROWS && key == "r2"
        )),
        "{rep:?}"
    );
    let _ = r;
}

#[test]
fn audit_catches_an_index_that_lost_an_acknowledged_write() {
    let f = fx("index", "a,b\n");
    let c = join(&f.s, "p", ParticipantKind::Person);
    let first = set(&f.s, c, "r1", "c1", "A", &EditOptions::new()).unwrap();
    let second = set(&f.s, c, "r1", "c1", "B", &EditOptions::new()).unwrap();
    let id = f.s.doc_id;
    // the index still names the first write: the record of the second has no row
    let w = f
        .store
        .elem_write(
            id,
            table::CELLS,
            &table::cell_key(&k("r1"), &k("c1")),
            ElemFacet::Value,
        )
        .unwrap()
        .unwrap();
    assert_eq!(w.seq, second.version);
    f.store
        .put_elem_write_for_tests(
            id,
            table::CELLS,
            &table::cell_key(&k("r1"), &k("c1")),
            ElemFacet::Value,
            &ElemWrite {
                seq: first.version,
                ..w
            },
        )
        .unwrap();
    let rep = f.s.audit().unwrap();
    assert!(
        rep.violations.iter().any(|v| matches!(
            v,
            AuditViolation::DroppedWrite { change, .. } if *change == second.version
        )),
        "{rep:?}"
    );
}

/// KC7: on this engine a merged restore or delete carries no writer identity; the base
/// check counts such a write as someone else's, so it is a conflict, never an overwrite,
/// even when the change was the caller's own.
#[test]
fn a_write_with_no_identity_counts_as_someone_elses() {
    let f = fx("kc7", "a,b\n");
    let a = join(&f.s, "agent-a", ParticipantKind::Agent);
    let base = f.s.read(a).unwrap().version;
    let opts = EditOptions::new().base(base);
    let mine = set(&f.s, a, "r1", "c1", "A", &opts).unwrap();
    // the caller's own write since its base doesn't refuse it
    set(&f.s, a, "r1", "c1", "AA", &opts).unwrap();
    // the same change, with no write identity (as a merged restore or delete has here)
    let key = table::cell_key(&k("r1"), &k("c1"));
    f.store
        .put_elem_write_for_tests(
            f.s.doc_id,
            table::CELLS,
            &key,
            ElemFacet::Value,
            &ElemWrite {
                seq: mine.version,
                write: None,
                kind: ElemOpKind::MapDelete,
                restores: None,
            },
        )
        .unwrap();
    let e = set(&f.s, a, "r1", "c1", "Z", &opts).unwrap_err();
    let SessionError::FacetConflict(cs) = &e else {
        panic!("{e:?}")
    };
    assert_eq!(cs[0].by.as_deref(), Some("agent-a"));
    assert_eq!(cs[0].change, mine.version);
    assert_eq!(crate::tools::session_exit(&e), crate::tools::exit::NO);
}

/// An entry with no recorded author isn't anyone's own: it keeps its audit event. An entry only
/// the caller held goes; one held by someone else too stays, without the caller.
#[test]
fn drop_own_keeps_an_entry_with_no_recorded_author() {
    let entry = |by: &[&str]| grid::Overridden {
        rule: "cell",
        region: "r1/c1".to_owned(),
        row: Some("r1".to_owned()),
        line: None,
        col: None,
        yours: String::new(),
        file: String::new(),
        by: by.iter().map(|b| (*b).to_owned()).collect(),
    };
    let mut v = vec![
        entry(&[]),
        entry(&["me"]),
        entry(&["me", "you"]),
        entry(&["kit:import"]),
    ];
    grid::drop_own(&mut v, "me");
    let left: Vec<Vec<String>> = v.iter().map(|o| o.by.clone()).collect();
    assert_eq!(
        left,
        [
            vec![],
            vec!["you".to_owned()],
            vec!["kit:import".to_owned()]
        ]
    );
}
