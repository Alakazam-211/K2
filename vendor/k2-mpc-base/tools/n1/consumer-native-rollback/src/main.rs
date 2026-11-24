//! A rollback to the previous release with a native doc in the store (tools/n1/n1.sh
//! `layout-native-rollback`): built against `<dir>/old`, the previous release as hosts ship it
//! (no native engine in it).
//!
//! - `read <dir>`: `<dir>/docs.redb` is a store this repo's `n1_native_rollback_producer` made
//!   with the native engine on, through files in `<dir>/ws`: `y1.md` and `y2.md` on Yrs, `n.md`
//!   native. First the store is copied to `space.redb` and `rename.redb`. Then, as a host on
//!   this release does after the rollback: `n.md` is refused on every path that reaches it,
//!   with `MPC-E169` `native_engine_missing` (exit 6, one line, its own session variant; the
//!   store's own variant on the store's paths): an open, an open with bytes, an open through
//!   its file as written, the same after an outside save of that file (the file kept as saved),
//!   a demote to tier 0 and a delete; after them no session of `n.md` is loaded, so no read,
//!   edit or verb has a handle to it, and a tier-0 open of its key is refused as another pack's
//!   (`MPC-E109`). The store's load, log size and compaction of `n.md` are refused too.
//!   Meanwhile `y1.md` is edited through its file, its history purged and written back, `y2.md`
//!   read, and the whole store rewritten; a new manager reads `y1.md`'s edit and is refused
//!   `n.md` again. On `space.redb`, the space's delete is refused (`MPC-E169`) and its docs
//!   stay; on `rename.redb`, a rename moves `n.md` to `moved/n.md` (no refusal: a rename moves
//!   only its key, which this repo's build reads back), every other field of its meta kept.
//!   This repo's `n1_native_rollback_after` then holds every row of `n.md` to its bytes.
//! - `produce <dir>`: this release's own store, Yrs only (`<dir>/docs.redb`, `p1.md` edited and
//!   `p2.md`, each through its file in `<dir>/ws`), for this repo's
//!   `n1_native_rollback_prev_after`.
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::errors::Refusal;
use k2_mpc_base::session::{
    AnyFile, AuditEvent, Config, Host, Participant, ParticipantKind, PurgeRequest, PurgeScope,
    Rights, SessionError, SessionManager, Stay, TextFilePack,
};
use k2_mpc_base::store::{DocKey, DocStore, StoreError, FEATURE_NATIVE};
use k2_mpc_base::tools::{exit, session_exit};

const MD: TextFilePack = TextFilePack::new("markdown", 1 << 20);
const ROOT: &str = TextFilePack::ROOT;

/// The row's texts (this repo's `n1_native_rollback_*` rows hold the same ones).
const RB_Y1: &str = "# Y1\n\nwritten by this build\n";
const RB_Y2: &str = "# Y2\n\nkept as written\n";
const RB_N: &str =
    "# Native\n\nwritten on the native engine\nedited by a person\nand by an agent\n";
const RB_Y1_PREV: &str = "# Y1\n\nwritten by this build\nedited by the previous release\n";
const RB_N_OUTSIDE: &str = "# Native\n\nsaved outside while the previous release ran\n";
const RB_P1: &str = "# P1\n\nwritten by the previous release\n";
const RB_P1_PREV: &str = "# P1\n\nwritten by the previous release\nand edited there\n";
const RB_P2_PREV: &str = "# P2\n\nwritten by the previous release\n";

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

/// A host's manager on `s`, with a blob store in `blobs` (tier 0: the demote path).
fn mgr(s: &Arc<DocStore>, blobs: &Path) -> SessionManager {
    let host = Arc::new(H {
        now: AtomicU64::new(1_950_000_000_000),
    });
    let h = host.clone();
    let b = BlobStore::open(
        s.clone(),
        blobs,
        BlobConfig::default(),
        Arc::new(move || h.now_ms()),
    )
    .unwrap();
    // the release's Config is non-exhaustive: set its fields one by one
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    SessionManager::new(s.clone(), host, c).with_blob_store(Arc::new(b))
}

fn reader() -> Participant {
    Participant::new("rd", "Reader", "#123456", ParticipantKind::Person, "cli")
}

/// The text of `name` through its file in `ws`.
fn text_of(m: &SessionManager, ws: &Path, name: &str) -> String {
    let s = m
        .open_file(Arc::new(MD), &key(name), &ws.join(name), ws)
        .unwrap();
    let c = s.join(reader(), Stay::Held).unwrap().conn;
    s.read(c).unwrap().text_by_root.remove(0).1
}

/// The native refusal's one line, its format aside.
#[track_caller]
fn native_line(line: &str, what: &str) {
    assert!(
        line.starts_with("this doc is on the native engine (format ")
            && line.ends_with("), which this build doesn't have")
            && !line.contains('\n'),
        "{what}: the native refusal's one line: {line:?}"
    );
}

/// `r` is the session's native refusal of `n.md` (`what` names the path).
#[track_caller]
fn refused<T>(r: Result<T, SessionError>, what: &str) {
    let e = match r {
        Ok(_) => panic!("{what} of n.md: not refused (a native doc opened on this release)"),
        Err(e) => e,
    };
    assert!(
        matches!(e, SessionError::NativeEngineMissing { .. }),
        "{what} of n.md: the native refusal's own session variant, not {e:?}"
    );
    let rep = e.report();
    assert_eq!(
        (rep.id(), rep.entry().name, rep.exit(), session_exit(&e)),
        ("MPC-E169", "native_engine_missing", exit::STORE, exit::STORE),
        "{what}"
    );
    let line = e.to_string();
    native_line(&line, what);
    println!(
        "N-1 NATIVE ROLLBACK {what} of n.md refused: {} {} exit {}: {line}",
        rep.id(),
        rep.entry().name,
        rep.exit()
    );
}

/// `r` is the store's native refusal of `n.md` (`what` names the call).
#[track_caller]
fn store_refused<T>(r: Result<T, StoreError>, what: &str) {
    let e = match r {
        Ok(_) => panic!("the store's {what} of n.md: not refused"),
        Err(e) => e,
    };
    assert!(
        matches!(e, StoreError::NativeEngineMissing { .. }),
        "the store's {what} of n.md: the native refusal, not {e:?}"
    );
    let rep = e.report();
    assert_eq!(
        (rep.id(), rep.entry().name, rep.exit()),
        ("MPC-E169", "native_engine_missing", exit::STORE),
        "{what}"
    );
    let line = e.to_string();
    native_line(&line, what);
    println!(
        "N-1 NATIVE ROLLBACK the store's {what} of n.md refused: {} {} exit {}: {line}",
        rep.id(),
        rep.entry().name,
        rep.exit()
    );
}

fn read(dir: &Path) {
    let p = dir.join("docs.redb");
    let ws = fs::canonicalize(dir.join("ws")).unwrap();
    let plant = std::env::var("MPC_N1_PLANT").unwrap_or_default();
    println!(
        "N-1 NATIVE ROLLBACK the previous release opens the store (plant: {})",
        if plant.is_empty() { "none" } else { &plant }
    );
    for copy in ["space.redb", "rename.redb"] {
        fs::copy(&p, dir.join(copy)).unwrap();
    }
    let n = key("n.md");
    let s = Arc::new(DocStore::open(&p).expect("a store holding a native doc opens"));
    assert_eq!(s.format().unwrap(), 8);
    assert_eq!(
        s.required_features().unwrap() & FEATURE_NATIVE,
        FEATURE_NATIVE
    );
    let id = s.find(&n).unwrap().expect("n.md is in the store");
    {
        let m = mgr(&s, &dir.join("blobs"));
        assert_eq!(fs::read_to_string(ws.join("n.md")).unwrap(), RB_N);
        refused(m.open(Arc::new(MD), &n, None), "open");
        refused(
            m.open(Arc::new(MD), &n, Some(RB_N.as_bytes())),
            "open with bytes",
        );
        refused(
            m.open_file(Arc::new(MD), &n, &ws.join("n.md"), &ws),
            "open through its file",
        );
        assert_eq!(fs::read_to_string(ws.join("n.md")).unwrap(), RB_N);
        // the person edits the file in another editor while this release runs
        fs::write(ws.join("n.md"), RB_N_OUTSIDE).unwrap();
        refused(
            m.open_file(Arc::new(MD), &n, &ws.join("n.md"), &ws),
            "open through its file after an outside save",
        );
        refused(
            m.demote_text_file(Arc::new(MD), Arc::new(AnyFile), &n, &ws.join("n.md"), &ws),
            "demote to tier 0",
        );
        refused(m.delete_doc(&n), "delete");
        assert_eq!(
            fs::read_to_string(ws.join("n.md")).unwrap(),
            RB_N_OUTSIDE,
            "the outside save is the file's"
        );
        assert!(m.get(&n).is_none() && !m.loaded_keys().contains(&n));
        println!("N-1 NATIVE ROLLBACK no session of n.md is loaded: no read, edit or verb has a handle to it; its file kept as saved");
        match m.open_blob(Arc::new(AnyFile), &n, None) {
            Err(e @ SessionError::PackMismatch { .. }) => {
                assert_eq!(e.report().id(), "MPC-E109");
                println!(
                    "N-1 NATIVE ROLLBACK a tier-0 open of n.md's key refused as another pack's: {} ({e})",
                    e.report().id()
                );
            }
            Err(e) => panic!("a tier-0 open of n.md's key: {e:?}"),
            Ok(_) => panic!("a tier-0 open of n.md's key opened"),
        }
        store_refused(s.load(id), "load");
        store_refused(s.log_size(id), "log size");
        store_refused(s.compact(id, 1, &[]), "compaction");
        // the Yrs docs, as usual
        assert_eq!(text_of(&m, &ws, "y2.md"), RB_Y2);
        let y1 = m
            .open_file(Arc::new(MD), &key("y1.md"), &ws.join("y1.md"), &ws)
            .unwrap();
        let who = Participant::new("bo", "Bo", "#654321", ParticipantKind::Person, "tab");
        let c = y1.join(who, Stay::Held).unwrap().conn;
        assert_eq!(y1.read(c).unwrap().text_by_root.remove(0).1, RB_Y1);
        y1.replace_text(c, ROOT, RB_Y1_PREV).unwrap();
        y1.purge(c, PurgeRequest::new(PurgeScope::History))
            .expect("a purge of y1.md's history (a rewrite of the store)");
        drop(y1);
        m.release().unwrap();
        assert_eq!(
            fs::read_to_string(ws.join("y1.md")).unwrap(),
            RB_Y1_PREV,
            "y1.md's edit written back"
        );
        s.rewrite_file().expect("a whole-store rewrite");
        println!("N-1 NATIVE ROLLBACK y1.md opened, edited, purged and written back; y2.md read; the store rewritten");
    }
    {
        let m = mgr(&s, &dir.join("blobs"));
        assert_eq!(text_of(&m, &ws, "y1.md"), RB_Y1_PREV);
        refused(m.open(Arc::new(MD), &n, None), "open after the rewrite");
        m.release().unwrap();
    }
    assert_eq!(
        s.required_features().unwrap() & FEATURE_NATIVE,
        FEATURE_NATIVE,
        "the native bit is kept"
    );
    drop(s);
    {
        let s = Arc::new(DocStore::open(dir.join("space.redb")).unwrap());
        let m = mgr(&s, &dir.join("blobs-space"));
        refused(m.delete_space("ws"), "the space delete");
        for d in ["y1.md", "y2.md", "n.md"] {
            assert!(s.find(&key(d)).unwrap().is_some(), "space.redb keeps {d}");
        }
        m.release().unwrap();
    }
    {
        let s = Arc::new(DocStore::open(dir.join("rename.redb")).unwrap());
        let m = mgr(&s, &dir.join("blobs-rename"));
        let id = s.find(&n).unwrap().unwrap();
        let before = s.meta(id).unwrap();
        m.rekey(&n, key("moved/n.md"))
            .expect("a rename of n.md moves its key");
        assert_eq!(s.find(&key("moved/n.md")).unwrap(), Some(id));
        assert_eq!(s.find(&n).unwrap(), None);
        let mut after = s.meta(id).unwrap();
        assert_eq!(after.key, key("moved/n.md"));
        after.key = n.clone();
        assert_eq!(after, before, "a rename changes n.md's key alone");
        refused(
            m.open(Arc::new(MD), &key("moved/n.md"), None),
            "open after a rename",
        );
        m.release().unwrap();
        println!("N-1 NATIVE ROLLBACK rename.redb: n.md renamed to moved/n.md, its meta but the key unchanged");
    }
    println!("N-1 NATIVE ROLLBACK PREVIOUS RELEASE OK");
}

fn produce(dir: &Path) {
    let _ = fs::remove_dir_all(dir);
    fs::create_dir_all(dir.join("ws")).unwrap();
    let ws = fs::canonicalize(dir.join("ws")).unwrap();
    fs::write(ws.join("p1.md"), RB_P1).unwrap();
    fs::write(ws.join("p2.md"), RB_P2_PREV).unwrap();
    let s = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let m = mgr(&s, &dir.join("blobs"));
    assert_eq!(text_of(&m, &ws, "p2.md"), RB_P2_PREV);
    let p1 = m
        .open_file(Arc::new(MD), &key("p1.md"), &ws.join("p1.md"), &ws)
        .unwrap();
    let c = p1.join(reader(), Stay::Held).unwrap().conn;
    p1.replace_text(c, ROOT, RB_P1_PREV).unwrap();
    drop(p1);
    m.release().unwrap();
    assert_eq!(fs::read_to_string(ws.join("p1.md")).unwrap(), RB_P1_PREV);
    assert_eq!(s.required_features().unwrap() & FEATURE_NATIVE, 0);
    println!(
        "N-1 NATIVE ROLLBACK the previous release wrote its own store: format {}, p1.md edited, p2.md",
        s.format().unwrap()
    );
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = Path::new(args.get(2).expect("usage: read <dir> | produce <dir>"));
    match args.get(1).map(String::as_str) {
        Some("read") => read(dir),
        Some("produce") => produce(dir),
        _ => panic!("usage: read <dir> | produce <dir>"),
    }
}
