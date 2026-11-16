//! A store holding a native doc, opened by a build without the native engine: the store opens
//! and its other docs work, while the native doc alone is refused, per doc, with its own typed
//! error (`MPC-E169` `native_engine_missing`: [`SessionError::NativeEngineMissing`] in a
//! session, [`StoreError::NativeEngineMissing`] in the store), and none of its rows changes,
//! through a rewrite of the store and a purge of another doc too.
//!
//! This build can't make a native doc, so its rows are written by hand through redb, with the
//! table definitions a build with the native engine writes them under (here, not the kit's):
//! the native stamp, a generation row, a record and the record's nonce, the store stamped
//! format 8 with the native bit. A store the native engine itself wrote is checked by the
//! native engine's own N−1 run, outside this tree (`tools/n1/n1.sh` has no such mode).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use super::features::plant::{under, FormatPlant};
use super::*;
use crate::errors::Refusal as _;
use crate::session::{
    AuditEvent, Config, Host, Participant, ParticipantKind, PurgeRequest, PurgeScope, Rights,
    SessionError, SessionManager, Stay, TextFilePack,
};
use crate::tools::{exit, session_exit};

const MD: TextFilePack = TextFilePack::new("markdown", 1 << 20);
const ROOT: &str = TextFilePack::ROOT;

/// The native engine's stamp, as a build with it writes it on a native doc.
const NATIVE: (&str, u32) = ("mpce", 1);

/// The engine store's tables, as a build with the native engine defines them.
const GENS: TableDefinition<u64, &[u8]> = TableDefinition::new("native_gens");
const RECORDS: TableDefinition<(u64, u64, u64), &[u8]> = TableDefinition::new("native_records");
const NONCES: TableDefinition<&[u8], u64> = TableDefinition::new("native_nonces");

struct TmpDir(PathBuf);
impl TmpDir {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!(
            "k2mb-absent-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        TmpDir(p)
    }
    fn db(&self) -> PathBuf {
        self.0.join("docs.redb")
    }
    fn bytes(&self) -> Vec<u8> {
        std::fs::read(self.db()).unwrap()
    }
}
impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct H {
    now: AtomicU64,
}

impl Host for H {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::full().with_owner(true)
    }
    fn now_ms(&self) -> u64 {
        self.now.fetch_add(1, Ordering::SeqCst)
    }
    fn on_event(&self, _: &AuditEvent) {}
}

fn mgr(s: &Arc<DocStore>) -> SessionManager {
    let host = Arc::new(H {
        now: AtomicU64::new(1_000_000),
    });
    SessionManager::new(s.clone(), host, Config::default())
}

fn key(path: &str) -> DocKey {
    DocKey {
        space: "ws".into(),
        path: path.into(),
    }
}

/// A store at `t` holding a Yrs doc (`y.md`, imported through a session) and a native doc
/// (`n.md`, its rows by hand), stamped format 8 with the native bit; closed. Returns the
/// native doc's id.
fn mixed_store(t: &TmpDir) -> DocId {
    let s = Arc::new(DocStore::open(t.db()).unwrap());
    let m = mgr(&s);
    drop(m.open(Arc::new(MD), &key("y.md"), Some(b"yrs\n")).unwrap());
    m.release().unwrap();
    let id = s
        .create(&DocMeta {
            key: key("n.md"),
            pack: "markdown".into(),
            engine: NATIVE.0.into(),
            engine_format: NATIVE.1,
            codec: vec![],
            created_ms: 1_000,
            synced_hash: None,
            synced_seq: 0,
            declined: None,
        })
        .unwrap();
    drop(m);
    drop(s);
    let db = Database::open(t.db()).unwrap();
    let w = db.begin_write().unwrap();
    {
        let mut gens = w.open_table(GENS).unwrap();
        gens.insert(id, [2u8, 0, 1].as_slice()).unwrap();
        let mut records = w.open_table(RECORDS).unwrap();
        records
            .insert((id, 0, 0), b"an engine header record".as_slice())
            .unwrap();
        let mut nonces = w.open_table(NONCES).unwrap();
        nonces.insert([7u8; 16].as_slice(), id).unwrap();
        let mut meta = w.open_table(META).unwrap();
        meta.insert("store_format", STORE_FORMAT_FEATURES).unwrap();
        meta.insert(super::features::REQUIRED_FEATURES, FEATURE_NATIVE)
            .unwrap();
    }
    w.commit().unwrap();
    id
}

/// Every row of doc `id` this build can't read without the native engine, as the file holds
/// them (a table or row that isn't there gives none): its meta, its generation row and
/// records, and the store's nonces.
fn native_rows(t: &TmpDir, id: DocId) -> Vec<Vec<u8>> {
    let db = Database::open(t.db()).unwrap();
    let r = db.begin_read().unwrap();
    let mut out = Vec::new();
    if let Some(m) = r.open_table(DOCS).unwrap().get(id).unwrap() {
        out.push(m.value().to_vec());
    }
    if let Ok(gens) = r.open_table(GENS) {
        if let Some(g) = gens.get(id).unwrap() {
            out.push(g.value().to_vec());
        }
    }
    if let Ok(records) = r.open_table(RECORDS) {
        for kv in records
            .range((id, 0, 0)..=(id, u64::MAX, u64::MAX))
            .unwrap()
        {
            out.push(kv.unwrap().1.value().to_vec());
        }
    }
    if let Ok(nonces) = r.open_table(NONCES) {
        for kv in nonces.iter().unwrap() {
            let (k, v) = kv.unwrap();
            out.push([k.value(), &v.value().to_le_bytes()[..]].concat());
        }
    }
    out
}

/// The refusal a host sees for the native doc: its own session variant, with the doc's stamp,
/// `MPC-E169`, exit 6.
#[track_caller]
fn assert_native_missing(e: &SessionError, what: &str) {
    assert_eq!(
        e,
        &SessionError::NativeEngineMissing {
            stored: NATIVE.0.into(),
            stored_format: NATIVE.1,
        },
        "row: {what}: the host sees the native refusal's own variant"
    );
    let r = e.report();
    assert_eq!(
        (r.id(), r.exit(), session_exit(e)),
        ("MPC-E169", exit::STORE, exit::STORE),
        "row: {what}"
    );
}

/// A build without the native engine opens a store holding a native doc and a Yrs doc: the
/// Yrs doc opens, takes an edit and keeps it across a release; the native doc is refused on
/// open, delete and load, per doc, with `MPC-E169`, and its rows are byte for byte as written.
/// The bit and the engine stores are still checked against each other: the bit cleared by hand
/// while the rows are there is damage, refused with the bytes unchanged.
/// `MPC_FORMAT_PLANT=native-store-refused-whole` (the whole store refused, as before) fails at
/// the open's assert.
#[test]
fn a_native_doc_is_refused_per_doc_and_the_store_opens() {
    under(FormatPlant::NativeStoreRefusedWhole, || {
        let t = TmpDir::new("mixed");
        let n = mixed_store(&t);
        let before = native_rows(&t, n);
        assert_eq!(before.len(), 4, "control: the native doc's rows");
        let s = DocStore::open(t.db());
        assert!(
            s.is_ok(),
            "row: a store holding a native doc opens here: {:?}",
            s.err()
        );
        let s = Arc::new(s.unwrap());
        assert_eq!(s.required_features().unwrap(), FEATURE_NATIVE);
        let m = mgr(&s);
        let e = m.open(Arc::new(MD), &key("n.md"), None).err().unwrap();
        assert_native_missing(&e, "open");
        let e = m.delete_doc(&key("n.md")).err().unwrap();
        assert_native_missing(&e, "delete");
        let e = s.load(n).err().unwrap();
        assert_eq!(
            e,
            StoreError::NativeEngineMissing {
                stored: NATIVE.0.into(),
                stored_format: NATIVE.1,
            },
            "row: the store's load"
        );
        assert_eq!(e.report().id(), "MPC-E169");
        let sess = m.open(Arc::new(MD), &key("y.md"), None).unwrap();
        let who = Participant::new("a", "A", "#123456", ParticipantKind::Person, "cli");
        let c = sess.join(who.clone(), Stay::Held).unwrap().conn;
        assert_eq!(sess.read(c).unwrap().text_by_root[0].1, "yrs\n");
        sess.replace_text(c, ROOT, "yrs, edited\n").unwrap();
        drop(sess);
        m.release().unwrap();
        let m = mgr(&s);
        let sess = m.open(Arc::new(MD), &key("y.md"), None).unwrap();
        let c = sess.join(who, Stay::Held).unwrap().conn;
        assert_eq!(
            sess.read(c).unwrap().text_by_root[0].1,
            "yrs, edited\n",
            "row: the Yrs doc's edit is kept"
        );
        drop(sess);
        m.release().unwrap();
        drop(m);
        drop(s);
        assert!(
            native_rows(&t, n) == before,
            "row: the native doc's rows are unchanged"
        );
        // the bit and the engine stores are one fact here too
        let db = Database::open(t.db()).unwrap();
        let w = db.begin_write().unwrap();
        w.open_table(META)
            .unwrap()
            .insert(super::features::REQUIRED_FEATURES, 0)
            .unwrap();
        w.commit().unwrap();
        drop(db);
        let bytes = t.bytes();
        assert_eq!(
            DocStore::open(t.db()).err(),
            Some(StoreError::Corrupt(
                "engine records in a store without the native bit"
            )),
            "row: engine records without the bit are damage"
        );
        assert!(t.bytes() == bytes, "row: the refused store is unchanged");
        println!(
            "native doc per doc: 3 refusals (open, delete, load), 1 Yrs doc edited, {} rows kept",
            before.len()
        );
    });
}

/// A rewrite of a store holding a native doc copies the engine store's tables through, byte
/// for byte, as every table it knows: a purge of the Yrs doc's history (which rewrites the
/// file) and a whole-store rewrite leave every row of the native doc as it was, and the store
/// opens after each with the native doc still refused alone.
/// `MPC_FORMAT_PLANT=native-tables-dropped` (the rewrite leaves those tables out, as one that
/// drops the tables it doesn't know would) fails at the rows' assert.
#[test]
fn a_rewrite_and_a_purge_copy_the_native_tables_through() {
    under(FormatPlant::NativeTablesDropped, || {
        let t = TmpDir::new("rewrite");
        let n = mixed_store(&t);
        let before = native_rows(&t, n);
        assert_eq!(before.len(), 4, "control: the native doc's rows");
        {
            let s = DocStore::open(t.db());
            assert!(s.is_ok(), "row: the store opens: {:?}", s.err());
            let s = Arc::new(s.unwrap());
            let m = mgr(&s);
            let sess = m.open(Arc::new(MD), &key("y.md"), None).unwrap();
            let who = Participant::new("a", "A", "#123456", ParticipantKind::Person, "cli");
            let c = sess.join(who, Stay::Held).unwrap().conn;
            sess.replace_text(c, ROOT, "yrs, edited\n").unwrap();
            let purged = sess.purge(c, PurgeRequest::new(PurgeScope::History));
            assert!(purged.is_ok(), "row: the Yrs doc's purge runs: {purged:?}");
            assert_eq!(sess.read(c).unwrap().text_by_root[0].1, "yrs, edited\n");
            drop(sess);
            m.release().unwrap();
            let r = s.rewrite_file();
            assert!(r.is_ok(), "row: the store is rewritten: {r:?}");
        }
        assert!(
            native_rows(&t, n) == before,
            "row: the rewrites copied the native doc's rows through unchanged"
        );
        let s = Arc::new(DocStore::open(t.db()).unwrap());
        let m = mgr(&s);
        let e = m.open(Arc::new(MD), &key("n.md"), None).err().unwrap();
        assert_native_missing(&e, "open after the rewrites");
        m.release().unwrap();
        println!(
            "native tables through 2 rewrites (purge, rewrite): {} rows kept",
            before.len()
        );
    });
}
