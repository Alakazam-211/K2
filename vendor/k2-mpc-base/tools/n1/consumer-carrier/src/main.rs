//! N−1 of the carrier (K1; tools/n1/n1.sh `layout-carrier`), built against the previous release,
//! which knows nothing of it. The store comes from this repo's `n1_carrier_producer`: three live
//! docs of the test pack `deck-carrier`, each with a carrier (`live_carriers`, one count each on
//! its blob) and its import version's (`version_carriers`, one blob for the three). Its pack is
//! the same file type without the hook, as a previous release's pack is.
//!
//! - **Held:** past the grace, this build's blob GC keeps every carrier (the counts are in
//!   `blobs.refs`, which it reads).
//! - **It reads and writes the store as before:** `a.deck` is written with its own `encode`
//!   (the stored carrier is left naming bytes the file no longer holds: this repo's
//!   `n1_carrier_after` drops it and takes the file); `b.deck` is left alone.
//! - It refuses a rewrite, and so a purge, before changing anything (`live_carriers`), and its
//!   doc delete (`c.deck`) leaves the row and the count, which this repo's open gives back.
//! - **The other way:** it writes a store of its own (`old/`), with no carrier, which
//!   `n1_carrier_after` opens.
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::doc::{Edit, Neutral};
use k2_mpc_base::elem::{Key, MapOp, Value};
use k2_mpc_base::schema::{Kind, Schema};
use k2_mpc_base::session::{
    AuditEvent, Config, Declined, EditOptions, Host, Pack, PackContent, PackError, Participant,
    ParticipantKind, Rights, Session, SessionManager, Stay,
};
use k2_mpc_base::store::{DocKey, DocStore, StoreError};

const T0: u64 = 1_000_000;
const HOUR: u64 = 60 * 60_000;

/// The test pack without the hook: one line per shape (`key`, then `\t<field>=<value>`), `#`
/// lines opaque (dropped by decode and encode).
struct OldDeck;
impl Pack for OldDeck {
    fn schema(&self) -> Schema {
        Schema {
            name: "deck-carrier",
            roots: &[("shapes", Kind::MovableList), ("fields", Kind::Map)],
        }
    }
    fn decode(&self, b: &[u8]) -> Result<PackContent, Declined> {
        let bad = |message: String| Declined {
            code: "malformed",
            message,
        };
        let s = std::str::from_utf8(b).map_err(|e| bad(e.to_string()))?;
        let mut shapes = Vec::new();
        let mut fields = BTreeMap::new();
        for line in s.lines().filter(|l| !l.starts_with('#')) {
            let mut parts = line.split('\t');
            let k = parts.next().unwrap_or("");
            let key = Key::new(k).map_err(|e| bad(e.to_string()))?;
            for p in parts {
                let (f, v) = p
                    .split_once('=')
                    .ok_or_else(|| bad(format!("no = in {p:?}")))?;
                fields.insert(format!("{k}:{f}"), Value::str(v));
            }
            shapes.push((key, Value::Null));
        }
        Ok(PackContent {
            roots: vec![
                ("shapes".into(), Neutral::List(shapes)),
                ("fields".into(), Neutral::Map(fields)),
            ],
            codec: vec![1],
        })
    }
    fn encode(&self, c: &[(String, Neutral)], _: &[u8]) -> Result<Vec<u8>, PackError> {
        let mut shapes: &[(Key, Value)] = &[];
        let mut fields = &BTreeMap::new();
        for (r, n) in c {
            match n {
                Neutral::List(l) if r == "shapes" => shapes = l,
                Neutral::Map(m) if r == "fields" => fields = m,
                _ => return Err(PackError(format!("unexpected root {r}"))),
            }
        }
        let mut out = String::new();
        for (k, _) in shapes {
            out.push_str(k.as_str());
            let prefix = format!("{}:", k.as_str());
            for (fk, v) in fields.range(prefix.clone()..) {
                let Some(f) = fk.strip_prefix(&prefix) else {
                    break;
                };
                out.push('\t');
                out.push_str(f);
                out.push('=');
                out.push_str(v.as_str().unwrap_or(""));
            }
            out.push('\n');
        }
        Ok(out.into_bytes())
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

/// Open `name` in `dir/ws` and set `shape`'s `field` as a person, then flush.
fn write(mgr: &SessionManager, dir: &str, name: &str, shape: &str, field: &str, value: &str) {
    let ws = std::fs::canonicalize(format!("{dir}/ws")).unwrap();
    let s: Arc<Session> = mgr
        .open_file(Arc::new(OldDeck), &key(name), &ws.join(name), &ws)
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
        vec![Edit::Map {
            root: "fields".into(),
            ops: vec![MapOp::Set {
                key: format!("{shape}:{field}"),
                value: Value::str(value),
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

    // held
    let before = blobs.stats().unwrap().blobs;
    assert_eq!(
        before, 4,
        "the producer left three live carriers and the import's, shared by the versions"
    );
    let gc = blobs.gc().unwrap();
    assert_eq!(gc.removed, 0, "the older gc collected a carrier");
    println!("HELD: the older gc past the grace keeps every carrier ({before} blobs)");

    // it writes a.deck with its own encode; b.deck is left alone
    write(&mgr, ".", "a.deck", "s2", "title", "B-old");
    let a = std::fs::read_to_string("ws/a.deck").unwrap();
    assert_eq!(
        a, "s1\ttitle=A2-a\ns2\ttitle=B-old\tz=9\n",
        "its own encode"
    );
    println!("the older kit wrote a.deck: {a:?}");
    mgr.release().unwrap();
    drop(mgr);

    match store.check_rewritable() {
        Err(StoreError::NewerData { table })
            if table == "live_carriers" || table == "version_carriers" =>
        {
            println!("rewrite refused before any change (table {table:?})");
        }
        other => panic!("expected the rewrite refused, got {other:?}"),
    }
    let c = store.find(&key("c.deck")).unwrap().expect("c.deck");
    store.delete_doc(c, now.load(Ordering::SeqCst)).unwrap();
    now.fetch_add(2 * HOUR, Ordering::SeqCst);
    blobs.gc().unwrap();
    assert!(
        blobs.stats().unwrap().blobs >= 4,
        "its delete can't give back a count it doesn't know"
    );
    println!("doc delete: left the row and the count");
    drop((store, blobs));

    // the other way: a store of its own, with no carrier
    std::fs::create_dir_all("old/ws").unwrap();
    std::fs::write(
        "old/ws/d.deck",
        "# deck v1 (opaque)\ns1\ttitle=A\n# speaker note for s1\ns2\tz=9\ttitle=B\n# end\n",
    )
    .unwrap();
    let (_, _, mgr) = open("old", &now);
    write(&mgr, "old", "d.deck", "s1", "title", "D1");
    let d = std::fs::read_to_string("old/ws/d.deck").unwrap();
    assert_eq!(d, "s1\ttitle=D1\ns2\ttitle=B\tz=9\n");
    mgr.release().unwrap();
    println!("the older kit wrote its own store (old/)");
    println!("N-1 CARRIER (older side) OK");
}
