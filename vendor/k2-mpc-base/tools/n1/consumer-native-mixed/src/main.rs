//! A store holding a native doc and a Yrs doc, opened by a build without the native engine
//! (tools/n1/n1.sh `layout-native-mixed`): built against `<dir>/old`, a kit that refuses native
//! docs one by one, with the native feature off.
//!
//! - `read <dir>`: `<dir>/mixed.redb` is a store this repo's `n1_native_mixed_producer` wrote
//!   with the native engine: `y.md` on Yrs, `n.md` native. It opens here; `y.md` opens, takes an
//!   edit and keeps it across a release; `n.md` alone is refused, on open and on delete, with
//!   `MPC-E169` `native_engine_missing` as its own session variant (exit 6); a purge of `y.md`'s
//!   history and a whole-store rewrite run; every row of `n.md` (its meta, its engine store,
//!   the store's nonces, its log rows: the newest ones its cut kept) is byte for byte what the
//!   producer wrote.
//!   `<dir>/last.redb` (the same store after `n.md`'s delete there) opens at format 8 with no
//!   native bit, and `y.md` reads.
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use k2_mpc_base::errors::Refusal;
use k2_mpc_base::session::{
    AuditEvent, Config, Host, Participant, ParticipantKind, PurgeRequest, PurgeScope, Rights,
    SessionError, SessionManager, Stay, TextFilePack,
};
use k2_mpc_base::store::{DocKey, DocStore, FEATURE_NATIVE};
use k2_mpc_base::tools::{exit, session_exit};
use redb::{Database, ReadableTable, TableDefinition};

const MD: TextFilePack = TextFilePack::new("markdown", 1 << 20);

const PATHS: TableDefinition<&str, u64> = TableDefinition::new("paths");
const DOCS: TableDefinition<u64, &[u8]> = TableDefinition::new("docs");
const GENS: TableDefinition<u64, &[u8]> = TableDefinition::new("native_gens");
const RECORDS: TableDefinition<(u64, u64, u64), &[u8]> = TableDefinition::new("native_records");
const NONCES: TableDefinition<&[u8], u64> = TableDefinition::new("native_nonces");
const UPDATES: TableDefinition<(u64, u64), &[u8]> = TableDefinition::new("updates");

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

fn key(path: &str) -> DocKey {
    DocKey {
        space: "ws".into(),
        path: path.into(),
    }
}

fn mgr(s: &Arc<DocStore>) -> SessionManager {
    let host = Arc::new(H {
        now: AtomicU64::new(1_900_000_000_000),
    });
    SessionManager::new(s.clone(), host, Config::default())
}

/// Every row of the native doc `n.md`, read through redb itself: its meta, its generation row,
/// its records, and the store's nonce table.
fn native_rows(p: &Path) -> Vec<Vec<u8>> {
    let db = Database::open(p).unwrap();
    let r = db.begin_read().unwrap();
    let id = r
        .open_table(PATHS)
        .unwrap()
        .get("ws\u{0}n.md")
        .unwrap()
        .expect("n.md is in the store")
        .value();
    let mut out = vec![r.open_table(DOCS).unwrap().get(id).unwrap().unwrap().value().to_vec()];
    out.push(r.open_table(GENS).unwrap().get(id).unwrap().unwrap().value().to_vec());
    let records = r.open_table(RECORDS).unwrap();
    for kv in records.range((id, 0, 0)..=(id, u64::MAX, u64::MAX)).unwrap() {
        out.push(kv.unwrap().1.value().to_vec());
    }
    // its log rows: this build's cut trimmed them to the newest (a gap below), and nothing here
    // reads, moves or renumbers them
    let log = r.open_table(UPDATES).unwrap();
    let mut rows = 0;
    for kv in log.range((id, 0)..=(id, u64::MAX)).unwrap() {
        let (k, v) = kv.unwrap();
        out.push([&k.value().1.to_le_bytes()[..], v.value()].concat());
        rows += 1;
    }
    assert!(rows > 1_000, "n.md's trimmed log holds its newest rows: {rows}");
    let nonces = r.open_table(NONCES).unwrap();
    for kv in nonces.iter().unwrap() {
        let (k, v) = kv.unwrap();
        out.push([k.value(), &v.value().to_le_bytes()[..]].concat());
    }
    assert!(out.len() >= 4, "n.md holds an engine store: {} rows", out.len());
    out
}

fn text_of(m: &SessionManager, path: &str) -> String {
    let s = m.open(Arc::new(MD), &key(path), None).unwrap();
    let who = Participant::new("a", "A", "#123456", ParticipantKind::Person, "cli");
    let c = s.join(who, Stay::Held).unwrap().conn;
    s.read(c).unwrap().text_by_root.remove(0).1
}

#[track_caller]
fn native_missing(e: &SessionError, what: &str) {
    assert!(
        matches!(e, SessionError::NativeEngineMissing { .. }),
        "{what}: the host sees the native refusal's own variant: {e:?}"
    );
    let r = e.report();
    assert_eq!(
        (r.id(), r.exit(), session_exit(e)),
        ("MPC-E169", exit::STORE, exit::STORE),
        "{what}"
    );
    println!("N-1 NATIVE MIXED {what} of n.md refused: {} ({e})", r.id());
}

fn read(dir: &Path) {
    let p = dir.join("mixed.redb");
    let before = native_rows(&p);
    {
        let s = Arc::new(DocStore::open(&p).expect("a store holding a native doc opens"));
        assert_eq!(s.required_features().unwrap() & FEATURE_NATIVE, FEATURE_NATIVE);
        let m = mgr(&s);
        let e = m.open(Arc::new(MD), &key("n.md"), None).err().unwrap();
        native_missing(&e, "open");
        let e = m.delete_doc(&key("n.md")).err().unwrap();
        native_missing(&e, "delete");
        let y = text_of(&m, "y.md");
        assert_eq!(y, "written by the native build\n");
        let sess = m.get(&key("y.md")).unwrap();
        let who = Participant::new("b", "B", "#654321", ParticipantKind::Person, "cli");
        let c = sess.join(who, Stay::Held).unwrap().conn;
        sess.replace_text(c, TextFilePack::ROOT, "edited without the native engine\n")
            .unwrap();
        sess.purge(c, PurgeRequest::new(PurgeScope::History))
            .expect("a purge of y.md's history (a rewrite of the store)");
        drop(sess);
        m.release().unwrap();
        let m = mgr(&s);
        assert_eq!(text_of(&m, "y.md"), "edited without the native engine\n");
        m.release().unwrap();
        s.rewrite_file().expect("a whole-store rewrite");
        println!("N-1 NATIVE MIXED y.md opened, edited, purged and kept; the store rewritten");
    }
    assert!(native_rows(&p) == before, "n.md's rows changed");
    println!("N-1 NATIVE MIXED n.md's {} rows unchanged", before.len());
    let p = dir.join("last.redb");
    let s = Arc::new(DocStore::open(&p).expect("the store after its last native doc's delete"));
    assert_eq!(s.format().unwrap(), 8);
    assert_eq!(s.required_features().unwrap() & FEATURE_NATIVE, 0);
    let m = mgr(&s);
    assert_eq!(text_of(&m, "y.md"), "written by the native build\n");
    m.release().unwrap();
    println!("N-1 NATIVE MIXED last.redb opened: format 8, no native bit, y.md read");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = Path::new(args.get(2).expect("usage: read <dir>"));
    match args.get(1).map(String::as_str) {
        Some("read") => read(dir),
        _ => panic!("usage: read <dir>"),
    }
}
