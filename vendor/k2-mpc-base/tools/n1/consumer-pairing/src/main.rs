//! N−1 of the carrier's pairing (K1b; tools/n1/n1.sh `layout-pairing`), built against the
//! previous release, which knows the carrier (v0.15.0 and later) but not the pairing. The store
//! comes from this repo's `n1_pairing_producer`: three live docs of the test pack `rows-minted`,
//! each with a carrier and a pairing (`carrier_pairings`). Its pack is the same file type as a
//! previous release has it: it keeps a carrier, has no pairing hooks, and writes with its own
//! `encode`.
//!
//! - **It ignores the pairing rows** and reads and writes the store as before: `a.rows` is
//!   written (a row inserted; the stored pairing is left naming this build's carrier: this
//!   repo's `n1_pairing_after` makes it again); `b.rows` is left alone.
//! - **It refuses a rewrite, and so a purge, before changing anything**, naming
//!   `carrier_pairings` (`StoreError::NewerData`, `MPC-E502`).
//! - Its doc delete (`c.rows`) leaves the doc's pairing rows, which this repo's open sweeps.
//! - **The other way:** it writes a store of its own (`old/`): a carrier and no pairing, which
//!   `n1_pairing_after` opens.
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::doc::{Edit, Neutral};
use k2_mpc_base::elem::{Anchor, Key, ListOp, Value};
use k2_mpc_base::schema::{Kind, Schema};
use k2_mpc_base::session::{
    AuditEvent, Config, Declined, EditOptions, Host, Pack, PackContent, PackError, Participant,
    ParticipantKind, Rights, Session, SessionManager, Stay,
};
use k2_mpc_base::store::{DocKey, DocStore, StoreError};

const T0: u64 = 1_000_000;
const HOUR: u64 = 60 * 60_000;

/// The file: one row per line with no id (its value trimmed), `#` lines opaque. Decode mints
/// r1..rn in file order; encode writes each row bare (dropping the opaque lines). It keeps a
/// carrier, with `encode_from`'s default (its own `encode`).
struct OldRows;
impl Pack for OldRows {
    fn schema(&self) -> Schema {
        Schema {
            name: "rows-minted",
            roots: &[("rows", Kind::MovableList)],
        }
    }
    fn decode(&self, b: &[u8]) -> Result<PackContent, Declined> {
        let s = std::str::from_utf8(b).map_err(|e| Declined {
            code: "malformed",
            message: e.to_string(),
        })?;
        let rows = s
            .lines()
            .filter(|l| !l.starts_with('#'))
            .enumerate()
            .map(|(i, l)| (Key::new(format!("r{}", i + 1)).unwrap(), Value::str(l.trim())))
            .collect();
        Ok(PackContent {
            roots: vec![("rows".into(), Neutral::List(rows))],
            codec: vec![1],
        })
    }
    fn encode(&self, c: &[(String, Neutral)], _: &[u8]) -> Result<Vec<u8>, PackError> {
        let [(r, Neutral::List(rows))] = c else {
            return Err(PackError("expects one list root `rows`".into()));
        };
        assert_eq!(r, "rows");
        Ok(rows
            .iter()
            .map(|(_, v)| format!("{}\n", v.as_str().unwrap_or("")))
            .collect::<String>()
            .into_bytes())
    }
    fn wants_carrier(&self) -> bool {
        true
    }
}

struct H(Arc<AtomicU64>);
impl Host for H {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::full().with_owner(true)
    }
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
    fn on_event(&self, _: &AuditEvent) {}
}

fn key(path: &str) -> DocKey {
    DocKey {
        space: "ws".into(),
        path: path.into(),
    }
}

fn open(dir: &str, now: &Arc<AtomicU64>) -> (Arc<DocStore>, Arc<BlobStore>, SessionManager) {
    let store = Arc::new(DocStore::open(format!("{dir}/docs.redb")).expect("the store opens"));
    let clock = now.clone();
    let blobs = Arc::new(
        BlobStore::open(
            store.clone(),
            format!("{dir}/blobs"),
            BlobConfig::default(),
            Arc::new(move || clock.load(Ordering::SeqCst)),
        )
        .unwrap(),
    );
    let mgr = SessionManager::new(store.clone(), Arc::new(H(now.clone())), Config::default())
        .with_blob_store(blobs.clone());
    (store, blobs, mgr)
}

/// Open `name` in `dir/ws`, insert a row `date` at the top as a person, then flush.
fn insert_date(mgr: &SessionManager, dir: &str, name: &str) {
    let ws = std::fs::canonicalize(format!("{dir}/ws")).unwrap();
    let s: Arc<Session> = mgr
        .open_file(Arc::new(OldRows), &key(name), &ws.join(name), &ws)
        .unwrap();
    let c = s
        .join(
            Participant::new("p", "P", "#111111", ParticipantKind::Person, "cli"),
            Stay::Held,
        )
        .unwrap()
        .conn;
    s.edit_ops(
        c,
        vec![Edit::List {
            root: "rows".into(),
            ops: vec![ListOp::Insert {
                key: Key::new("m1").unwrap(),
                at: Anchor::Start,
                value: Value::str("date"),
            }],
        }],
        &EditOptions::new(),
    )
    .unwrap();
    assert!(s.flush(c).unwrap().written, "the older kit wrote {name}");
}

fn main() {
    let now = Arc::new(AtomicU64::new(T0 + 2 * HOUR));
    let (store, blobs, mgr) = open(".", &now);

    // it reads and writes the store as before; b.rows is left alone
    let gc = blobs.gc().unwrap();
    assert_eq!(gc.removed, 0, "the older gc collected a carrier");
    insert_date(&mgr, ".", "a.rows");
    let a = std::fs::read_to_string("ws/a.rows").unwrap();
    assert_eq!(a, "date\napple\nfig\nbanana\ncherry\n", "its own encode");
    println!("the older kit wrote a.rows: {a:?}");
    mgr.release().unwrap();
    drop(mgr);

    // its rewrite, and so a purge, is refused naming the table it doesn't know
    match store.check_rewritable() {
        Err(StoreError::NewerData { table }) if table == "carrier_pairings" => {
            println!("rewrite refused before any change (table {table:?})");
        }
        other => panic!("expected the rewrite refused naming carrier_pairings, got {other:?}"),
    }
    // its doc delete leaves c.rows' pairing rows (this repo's open sweeps them)
    let c = store.find(&key("c.rows")).unwrap().expect("c.rows");
    store.delete_doc(c, now.load(Ordering::SeqCst)).unwrap();
    println!("doc delete: c.rows deleted, its pairing rows left");
    drop((store, blobs));

    // the other way: a store of its own, a carrier and no pairing
    std::fs::create_dir_all("old/ws").unwrap();
    std::fs::write(
        "old/ws/d.rows",
        "# rows v1\n  apple  \nbanana\n# about banana\n cherry\n# end\n",
    )
    .unwrap();
    let (_, _, mgr) = open("old", &now);
    insert_date(&mgr, "old", "d.rows");
    let d = std::fs::read_to_string("old/ws/d.rows").unwrap();
    assert_eq!(d, "date\napple\nbanana\ncherry\n");
    mgr.release().unwrap();
    println!("the older kit wrote its own store (old/)");
    println!("N-1 PAIRING (older side) OK");
}
