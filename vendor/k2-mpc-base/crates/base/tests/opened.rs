//! The version a person's app opened (`note_opened`, `docs/SESSIONS.md`, "The version an app
//! opened"): the pin a host sets, kept with the doc's sync state across a restart and a store
//! rewrite, checked at every use, moved by an outside save, and the write-back hold after a
//! `FileNow` pin. Blob docs have no engine (the blob store holds them); a live doc's pin is
//! sync metadata no engine reads (no change is made from it in these rows), so they run on the
//! one engine sessions store docs on. Public API only.
//! Each plant of `MPC_OPENED_MUTANT` (`k2_mpc_base::differential::OpenedMutant`) fails the rows
//! named at it, at their own assertions.

#[path = "grid_pack/mod.rs"]
mod grid_pack;

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::doc::{Edit, Neutral};
use k2_mpc_base::elem::{Anchor, Key, ListOp, MapOp, Value};
use k2_mpc_base::schema::{Kind, Schema};
use k2_mpc_base::session::{
    AnyFile, AuditEvent, BlobMerge, BlobPack, BlobSession, Config, ConnId, DiskCheck, Host,
    Lineage, OpenedAt, OpenedVia, Participant, ParticipantKind, Rights, Session, SessionError,
    SessionManager, Stay, TextFilePack, EXTERNAL_PID,
};
use k2_mpc_base::session::{Declined, EditOptions, Pack, PackContent, PackError, Reconciled, View};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::tools::exit;

// ---------- harness ----------

struct TestHost {
    now: AtomicU64,
    rights: Mutex<HashMap<String, Rights>>,
    events: Mutex<Vec<AuditEvent>>,
}

impl Host for TestHost {
    fn rights(&self, who: &Participant, _key: &DocKey) -> Rights {
        self.rights
            .lock()
            .unwrap()
            .get(&who.pid)
            .copied()
            .unwrap_or(Rights::none())
    }
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn on_event(&self, ev: &AuditEvent) {
        self.events.lock().unwrap().push(ev.clone());
    }
}

struct TmpDir(PathBuf);
impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Fx {
    dir: TmpDir,
    ws: PathBuf,
    host: Arc<TestHost>,
    store: Arc<DocStore>,
    blobs: Arc<BlobStore>,
    mgr: SessionManager,
}

const T0: u64 = 1_793_111_400_000;
const NAME: &str = "paper.pdf";
const PDF1: &[u8] = b"%PDF-1.7\n1 0 obj << /Type /Catalog >> endobj\n%%EOF\n";
const PDF2: &[u8] = b"%PDF-1.7\n1 0 obj << /Type /Catalog /Lang (en) >> endobj\n%%EOF\n";
const PDF3: &[u8] = b"%PDF-1.7\n1 0 obj << /Type /Catalog /Title (agent) >> endobj\n%%EOF\n";

fn config() -> Config {
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    c
}

fn managers(dir: &Path, host: &Arc<TestHost>) -> (Arc<DocStore>, Arc<BlobStore>, SessionManager) {
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let h = host.clone();
    let blobs = Arc::new(
        BlobStore::open(
            store.clone(),
            dir.join("blobs"),
            BlobConfig::default(),
            Arc::new(move || h.now_ms()),
        )
        .unwrap(),
    );
    let mgr =
        SessionManager::new(store.clone(), host.clone(), config()).with_blob_store(blobs.clone());
    (store, blobs, mgr)
}

fn fx(tag: &str) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let p = std::env::temp_dir().join(format!(
        "k2mb-opened-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = fs::remove_dir_all(&p);
    let ws = p.join("ws");
    fs::create_dir_all(&ws).unwrap();
    let ws = fs::canonicalize(ws).unwrap();
    let host = Arc::new(TestHost {
        now: AtomicU64::new(T0),
        rights: Mutex::new(HashMap::new()),
        events: Mutex::new(Vec::new()),
    });
    let (store, blobs, mgr) = managers(&p, &host);
    fs::write(ws.join(NAME), PDF1).unwrap();
    for (pid, r) in [
        ("alice", Rights::full().with_owner(true)),
        ("viewer", Rights::read_only()),
    ] {
        host.rights.lock().unwrap().insert(pid.into(), r);
    }
    Fx {
        dir: TmpDir(p),
        ws,
        host,
        store,
        blobs,
        mgr,
    }
}

/// The host restarts: every handle goes, and the store and the blob folder open again.
fn restart(f: Fx) -> Fx {
    let Fx {
        dir,
        ws,
        host,
        store,
        blobs,
        mgr,
    } = f;
    mgr.release().unwrap();
    drop(mgr);
    drop(blobs);
    drop(store);
    let (store, blobs, mgr) = managers(&dir.0, &host);
    Fx {
        dir,
        ws,
        host,
        store,
        blobs,
        mgr,
    }
}

impl Fx {
    fn path(&self) -> PathBuf {
        self.ws.join(NAME)
    }
    fn open(&self) -> Arc<BlobSession> {
        self.mgr
            .open_blob_file(
                Arc::new(AnyFile),
                &DocKey {
                    space: "ws".into(),
                    path: NAME.into(),
                },
                &self.path(),
                &self.ws,
            )
            .unwrap()
    }
    fn advance(&self, ms: u64) {
        self.host.now.fetch_add(ms, Ordering::SeqCst);
    }
    fn events(&self) -> Vec<AuditEvent> {
        self.host.events.lock().unwrap().clone()
    }
}

fn join(s: &BlobSession, pid: &str, kind: ParticipantKind) -> ConnId {
    s.join(
        Participant::new(pid, pid, "#123456", kind, "cli"),
        Stay::Held,
    )
    .unwrap()
    .conn
}

fn app_save(path: &Path, bytes: &[u8]) {
    let tmp = path.with_file_name(".app-save.tmp");
    fs::write(&tmp, bytes).unwrap();
    fs::rename(&tmp, path).unwrap();
}

fn version(s: &BlobSession) -> u64 {
    s.status().unwrap().version
}

// ---------- the rows ----------

/// A pin outlives a restart: noted, the host restarts, and the doc's pin is the same.
#[test]
fn opened_pin_persists() {
    let f = fx("persists");
    let s = f.open();
    let a = join(&s, "alice", ParticipantKind::Person);
    let v = version(&s);
    let o = s.note_opened(a, OpenedAt::FileNow).expect("noted");
    assert_eq!((o.version, o.via), (v, OpenedVia::FileNow));
    assert_eq!(s.opened().unwrap(), Some(o));
    drop(s);
    let f = restart(f);
    let s = f.open();
    assert_eq!(
        s.opened().unwrap().map(|p| p.version),
        Some(v),
        "the pin outlives a restart"
    );
}

/// A pin outlives a store rewrite (a purge's or the host's): it is in the sync rows a rewrite
/// copies.
#[test]
fn pin_survives_rewrite() {
    let f = fx("rewrite");
    let s = f.open();
    let a = join(&s, "alice", ParticipantKind::Person);
    let v = version(&s);
    s.note_opened(a, OpenedAt::Version(v)).expect("noted");
    drop(s);
    let f = restart(f);
    f.store.rewrite_file().expect("the store rewrites");
    let f = restart(f);
    let s = f.open();
    assert_eq!(
        s.opened().unwrap().map(|p| p.version),
        Some(v),
        "the pin outlives a restart"
    );
}

/// A pin whose version is purged is dropped at its next use, with an event.
#[test]
fn pin_checked_at_use() {
    let f = fx("checked");
    let s = f.open();
    let a = join(&s, "alice", ParticipantKind::Person);
    let v1 = version(&s);
    s.note_opened(a, OpenedAt::Version(v1)).expect("noted");
    let w = s.write_bytes(a, PDF2, v1).expect("a write");
    assert_ne!(w.version, v1);
    s.purge(a, Some(v1)).expect("the older version purged");
    assert_eq!(
        s.opened().unwrap(),
        None,
        "a purged version's pin is dropped"
    );
    assert!(
        f.events().iter().any(|e| matches!(
            e,
            AuditEvent::OpenedDropped { version, why: "not_held", .. } if *version == v1
        )),
        "a purged version's pin is dropped"
    );
}

/// An outside save taken in moves the pin to the save's version: the app's buffer is its own
/// save now.
#[test]
fn pin_moves_with_outside_save() {
    let f = fx("moves");
    let s = f.open();
    let a = join(&s, "alice", ParticipantKind::Person);
    let v1 = version(&s);
    s.note_opened(a, OpenedAt::FileNow).expect("noted");
    app_save(&f.path(), PDF2);
    let saved = match s.file_changed().unwrap() {
        DiskCheck::Ingested { version } => version,
        other => panic!("the save is taken in: {other:?}"),
    };
    assert_ne!(saved, v1);
    let p = s.opened().unwrap().expect("a pin");
    assert_eq!(
        (p.version, p.via),
        (saved, OpenedVia::OutsideSave),
        "the pin follows the app's save"
    );
}

/// `FileNow` takes in a save the kit hasn't seen before it pins: the pin is the file's
/// version, not the stale one.
#[test]
fn file_now_takes_in_first() {
    let f = fx("takes-in");
    let s = f.open();
    let a = join(&s, "alice", ParticipantKind::Person);
    let v1 = version(&s);
    app_save(&f.path(), PDF2);
    let o = s.note_opened(a, OpenedAt::FileNow).expect("noted");
    assert_ne!(
        o.version, v1,
        "the pin is the file's version, not a stale one"
    );
    assert_eq!(o.version, version(&s));
}

/// After a `FileNow` pin the kit holds its write-backs for `opened_hold_ms`: a write lands in
/// the doc, not the file, until the hold ends.
#[test]
fn file_now_holds_write_backs() {
    let f = fx("hold");
    let s = f.open();
    let a = join(&s, "alice", ParticipantKind::Person);
    s.note_opened(a, OpenedAt::FileNow).expect("noted");
    let v = version(&s);
    let w = s.write_bytes(a, PDF3, v).expect("a write");
    assert!(!w.written, "held: the file waits");
    assert_eq!(fs::read(f.path()).unwrap(), PDF1, "held: the file waits");
    f.advance(Config::default().opened_hold_ms + 1);
    let out = s.flush(a).expect("a flush");
    assert!(out.written, "after the hold the write lands");
    assert_eq!(fs::read(f.path()).unwrap(), PDF3);
}

/// The refusals: a viewer can't pin (exit 3); a version the doc never had (exit 4); a purged
/// one (exit 8). `clear_opened` drops the pin, with an event.
#[test]
fn note_opened_rights_and_refusals() {
    let f = fx("refusals");
    let s = f.open();
    let a = join(&s, "alice", ParticipantKind::Person);
    let viewer = join(&s, "viewer", ParticipantKind::Person);
    let e = s.note_opened(viewer, OpenedAt::FileNow).unwrap_err();
    assert!(matches!(e, SessionError::Forbidden { .. }), "{e:?}");
    assert_eq!(k2_mpc_base::tools::session_exit(&e), exit::REFUSED);
    let e = s.note_opened(a, OpenedAt::Version(999)).unwrap_err();
    assert_eq!(e, SessionError::OpenedUnknown { version: 999 });
    assert_eq!(k2_mpc_base::tools::session_exit(&e), exit::NOT_FOUND);
    let v1 = version(&s);
    s.write_bytes(a, PDF2, v1).unwrap();
    s.purge(a, Some(v1)).unwrap();
    let e = s.note_opened(a, OpenedAt::Version(v1)).unwrap_err();
    assert_eq!(e, SessionError::OpenedGone { version: v1 });
    assert_eq!(k2_mpc_base::tools::session_exit(&e), exit::TOO_OLD);
    let v2 = version(&s);
    s.note_opened(a, OpenedAt::Version(v2)).unwrap();
    s.clear_opened(a).unwrap();
    assert_eq!(s.opened().unwrap(), None, "cleared");
    assert!(f
        .events()
        .iter()
        .any(|e| matches!(e, AuditEvent::OpenedDropped { why: "cleared", .. })));
}

// ---------- a live doc ----------

const MD: &str = "notes.md";
const TEXT: TextFilePack = TextFilePack::new("md", 1 << 20);

impl Fx {
    fn md_path(&self) -> PathBuf {
        self.ws.join(MD)
    }
    fn open_md(&self) -> Arc<Session> {
        self.mgr
            .open_file(
                Arc::new(TEXT),
                &DocKey {
                    space: "ws".into(),
                    path: MD.into(),
                },
                &self.md_path(),
                &self.ws,
            )
            .unwrap()
    }
}

fn md_fx(tag: &str) -> Fx {
    let f = fx(tag);
    fs::write(f.md_path(), "one\ntwo\n").unwrap();
    f
}

fn live_join(s: &Session, pid: &str) -> ConnId {
    s.join(
        Participant::new(pid, pid, "#123456", ParticipantKind::Person, "tab"),
        Stay::Held,
    )
    .unwrap()
    .conn
}

fn append(s: &Session, c: ConnId, text: &str) {
    let o = s
        .run_tool(
            c,
            TextFilePack::ROOT,
            &["append".to_owned()],
            text.as_bytes(),
        )
        .expect("an append");
    assert_eq!(o.exit, 0);
}

/// A live doc's pin outlives a restart, at the change seq the file held.
#[test]
fn live_opened_pin_persists() {
    let f = md_fx("live-persists");
    let s = f.open_md();
    let a = live_join(&s, "alice");
    let v = s.status().unwrap().version;
    let o = s.note_opened(a, OpenedAt::FileNow).expect("noted");
    assert_eq!((o.version, o.via), (v, OpenedVia::FileNow));
    drop(s);
    let f = restart(f);
    let s = f.open_md();
    assert_eq!(
        s.opened().unwrap().map(|p| p.version),
        Some(v),
        "the pin outlives a restart"
    );
}

/// An outside save taken in moves a live doc's pin to the save's change seq.
#[test]
fn live_pin_moves_with_outside_save() {
    let f = md_fx("live-moves");
    let s = f.open_md();
    let a = live_join(&s, "alice");
    s.note_opened(a, OpenedAt::FileNow).expect("noted");
    app_save(&f.md_path(), b"one\ntwo\nthree\n");
    assert!(matches!(
        s.file_changed().unwrap(),
        DiskCheck::Ingested { .. }
    ));
    let p = s.opened().unwrap().expect("a pin");
    assert_eq!(
        (p.version, p.via),
        (s.status().unwrap().version, OpenedVia::OutsideSave),
        "the pin follows the app's save"
    );
}

/// `FileNow` on a live doc takes in a save the kit hasn't seen before it pins.
#[test]
fn live_file_now_takes_in_first() {
    let f = md_fx("live-takes-in");
    let s = f.open_md();
    let a = live_join(&s, "alice");
    let v1 = s.status().unwrap().version;
    app_save(&f.md_path(), b"one\ntwo\nsaved\n");
    let o = s.note_opened(a, OpenedAt::FileNow).expect("noted");
    assert_ne!(
        o.version, v1,
        "the pin is the file's version, not a stale one"
    );
    assert_eq!(o.version, s.status().unwrap().version);
}

/// After a `FileNow` pin a live doc's edits wait to be written for `opened_hold_ms`.
#[test]
fn live_file_now_holds_write_backs() {
    let f = md_fx("live-hold");
    let s = f.open_md();
    let a = live_join(&s, "alice");
    s.note_opened(a, OpenedAt::FileNow).expect("noted");
    append(&s, a, "three\n");
    let out = s.flush(a).expect("a flush");
    assert!(!out.written, "held: the file waits");
    assert_eq!(
        fs::read_to_string(f.md_path()).unwrap(),
        "one\ntwo\n",
        "held: the file waits"
    );
    f.advance(Config::default().opened_hold_ms + 1);
    assert!(
        s.flush(a).expect("a flush").written,
        "after the hold the write lands"
    );
    assert_eq!(
        fs::read_to_string(f.md_path()).unwrap(),
        "one\ntwo\nthree\n"
    );
}

/// A live doc's refusals: a viewer can't pin; a seq past the doc's (exit 4); an earlier seq
/// with no stored version (exit 8); a doc with no file (exit 2).
#[test]
fn live_note_opened_refusals() {
    let f = md_fx("live-refusals");
    let s = f.open_md();
    let a = live_join(&s, "alice");
    let viewer = live_join(&s, "viewer");
    let e = s.note_opened(viewer, OpenedAt::FileNow).unwrap_err();
    assert!(matches!(e, SessionError::Forbidden { .. }), "{e:?}");
    append(&s, a, "three\n");
    let mid = s.status().unwrap().version;
    append(&s, a, "four\n");
    let now = s.status().unwrap().version;
    let e = s.note_opened(a, OpenedAt::Version(now + 5)).unwrap_err();
    assert_eq!(e, SessionError::OpenedUnknown { version: now + 5 });
    let e = s.note_opened(a, OpenedAt::Version(mid)).unwrap_err();
    assert_eq!(e, SessionError::OpenedGone { version: mid });
    assert_eq!(k2_mpc_base::tools::session_exit(&e), exit::TOO_OLD);
    let mem = f
        .mgr
        .open(
            Arc::new(TEXT),
            &DocKey {
                space: "ws".into(),
                path: "mem.md".into(),
            },
            Some(b"x\n".as_slice()),
        )
        .unwrap();
    let m = live_join(&mem, "alice");
    let e = mem.note_opened(m, OpenedAt::FileNow).unwrap_err();
    assert_eq!(e, SessionError::OpenedNoFile);
    assert_eq!(k2_mpc_base::tools::session_exit(&e), exit::USAGE);
}

/// A tier change drops the pin: a blob doc's pin doesn't come over when it is promoted to a
/// live doc (the store clears the doc's sync rows).
#[test]
fn pin_dropped_by_tier_change() {
    let f = md_fx("tier");
    let key = DocKey {
        space: "ws".into(),
        path: MD.into(),
    };
    let b = f
        .mgr
        .open_blob_file(Arc::new(AnyFile), &key, &f.md_path(), &f.ws)
        .unwrap();
    let a = join(&b, "alice", ParticipantKind::Person);
    b.note_opened(a, OpenedAt::FileNow).expect("noted");
    assert!(b.opened().unwrap().is_some(), "the blob doc's pin");
    b.leave(a).unwrap();
    drop(b);
    let (s, _) = f
        .mgr
        .promote_blob_file(Arc::new(TEXT), &key, &f.md_path(), &f.ws)
        .expect("promoted");
    assert_eq!(s.opened().unwrap(), None, "a tier change drops the pin");
}

// ---------- a blob pack's merge from the version the app opened ----------

/// A toy pack whose files are sets of lines, merged three ways: a line the save removed from
/// the base goes, a line it added comes in, and the doc's own changes stay. A save merged from
/// the wrong base removes what the doc added since the save's real base.
struct LineSets;
impl BlobPack for LineSets {
    fn can_merge(&self) -> bool {
        true
    }
    fn merge3_bytes(&self, base: &[u8], ours: &[u8], theirs: &[u8]) -> BlobMerge {
        let lines = |b: &[u8]| -> Vec<String> {
            String::from_utf8_lossy(b)
                .lines()
                .map(str::to_owned)
                .collect()
        };
        let (base, ours, theirs) = (lines(base), lines(ours), lines(theirs));
        let mut out: Vec<String> = ours
            .into_iter()
            .filter(|l| !base.contains(l) || theirs.contains(l))
            .collect();
        for l in theirs {
            if !base.contains(&l) && !out.contains(&l) {
                out.push(l);
            }
        }
        BlobMerge::Merged(format!("{}\n", out.join("\n")).into_bytes())
    }
}

const SET: &str = "set.txt";

impl Fx {
    fn set_path(&self) -> PathBuf {
        self.ws.join(SET)
    }
    fn open_set(&self) -> Arc<BlobSession> {
        self.mgr
            .open_blob_file(
                Arc::new(LineSets),
                &DocKey {
                    space: "ws".into(),
                    path: SET.into(),
                },
                &self.set_path(),
                &self.ws,
            )
            .unwrap()
    }
}

/// The flow both rows run: the file holds `a`; (the app opens it, noted when `pin`); an agent
/// writes `a w`, and the kit writes the file once the hold is over; the app then saves the
/// buffer it opened with its own change, `a p`. Returns what the save became and the file.
fn stale_save(f: &Fx, pin: bool) -> (DiskCheck, String) {
    fs::write(f.set_path(), "a\n").unwrap();
    let s = f.open_set();
    let a = join(&s, "alice", ParticipantKind::Person);
    if pin {
        s.note_opened(a, OpenedAt::FileNow).expect("noted");
    }
    let v = version(&s);
    s.write_bytes(a, b"a\nw\n", v).expect("the agent's change");
    f.advance(Config::default().opened_hold_ms + 1);
    s.flush(a).expect("a flush");
    assert_eq!(
        fs::read_to_string(f.set_path()).unwrap(),
        "a\nw\n",
        "the kit wrote the file"
    );
    app_save(&f.set_path(), b"a\np\n");
    let out = s.file_changed().unwrap();
    (out, fs::read_to_string(f.set_path()).unwrap())
}

/// With the version the app opened noted, its stale save merges from that version: the agent's
/// change the app never saw stays, and the person's comes in, with no copy.
#[test]
fn opened_save_merges() {
    let f = fx("merges");
    let (out, file) = stale_save(&f, true);
    assert!(
        matches!(out, DiskCheck::Merged { .. }),
        "merged from the opened version: {out:?}"
    );
    assert_eq!(file, "a\nw\np\n", "merged from the opened version");
}

/// With no pin, the same stale save keeps both, as before: the kit wrote the file since the
/// last outside save, and nothing says what the app opened.
#[test]
fn no_pin_keeps_today() {
    let f = fx("no-pin");
    let (out, file) = stale_save(&f, false);
    assert!(
        matches!(out, DiskCheck::Conflict { .. }),
        "no pin keeps both: {out:?}"
    );
    assert_eq!(file, "a\np\n", "no pin keeps both");
}

// ---------- text and grid docs: the merge's base is the version the app opened ----------

/// A text doc: the app opens `one two`; a person appends `agent`, written to the file once the
/// hold is over; the app then saves the buffer it opened with its own line. Merged from the
/// opened version, both lines are in the doc; merged from the file's (the kit's write), the save
/// would read as removing `agent`.
#[test]
fn text_save_merges_from_opened() {
    let f = md_fx("text-merge");
    let s = f.open_md();
    let a = live_join(&s, "alice");
    s.note_opened(a, OpenedAt::FileNow).expect("noted");
    append(&s, a, "agent\n");
    f.advance(Config::default().opened_hold_ms + 1);
    assert!(s.flush(a).unwrap().written, "the kit wrote the file");
    app_save(&f.md_path(), b"one\ntwo\nperson\n");
    s.file_changed().unwrap();
    let text = s.read(a).unwrap().text_by_root[0].1.clone();
    assert!(
        text.contains("agent\n") && text.contains("person\n"),
        "merged from the opened version: {text:?}"
    );
}

/// A text doc, a clash: the app opens `one two three`; a person rewrites `two`, written to the
/// file once the hold is over; the app then saves the buffer it opened with its own `two`.
/// Merged from the opened version, both lines are kept, and the save's change event says
/// where (`kept_pairs`: the live line, then the file's). Merged from the file's (the kit's
/// write), the save would read as replacing the person's line, and nothing would be kept.
#[test]
fn text_clash_from_opened_reports_kept_pairs() {
    let f = fx("text-kept");
    fs::write(f.md_path(), "one\ntwo\nthree\n").unwrap();
    let s = f.open_md();
    let a = live_join(&s, "alice");
    s.note_opened(a, OpenedAt::FileNow).expect("noted");
    let watch = s.subscribe(a).unwrap();
    let r = s
        .edit_with(a, TextFilePack::ROOT, |t| {
            Ok::<_, std::convert::Infallible>(t.replacen("two", "two (agent)", 1))
        })
        .unwrap();
    assert!(r.is_some(), "the person's edit applied");
    f.advance(Config::default().opened_hold_ms + 1);
    assert!(s.flush(a).unwrap().written, "the kit wrote the file");
    app_save(&f.md_path(), b"one\ntwo (person)\nthree\n");
    s.file_changed().unwrap();
    let text = s.read(a).unwrap().text_by_root[0].1.clone();
    assert_eq!(
        text, "one\ntwo (agent)\ntwo (person)\nthree\n",
        "merged from the opened version: {text:?}"
    );
    let evs: Vec<_> = watch.try_iter().collect();
    let ext = evs.last().expect("the save's change event");
    assert_eq!(ext.pid, EXTERNAL_PID);
    let kept: Vec<_> = ext.kept_pairs.iter().map(|p| (p.live, p.file)).collect();
    assert_eq!(
        kept,
        [((2, 2), (3, 3))],
        "the save's kept pairs: the live line, then the file's"
    );
}

const CSV: &str = "t.csv";

/// A grid doc: the app opens a table; a person sets a cell, written to the file once the hold
/// is over; the app then saves the table it opened with a new row. Merged from the opened
/// version, the cell keeps the person's value and the row comes in.
#[test]
fn grid_save_merges_from_opened() {
    let f = fx("grid-merge");
    let path = f.ws.join(CSV);
    fs::write(&path, "id,city\nk1,Oslo\n").unwrap();
    let s = f
        .mgr
        .open_file(
            Arc::new(grid_pack::GridLite),
            &DocKey {
                space: "ws".into(),
                path: CSV.into(),
            },
            &path,
            &f.ws,
        )
        .unwrap();
    let a = live_join(&s, "alice");
    s.note_opened(a, OpenedAt::FileNow).expect("noted");
    let argv: Vec<String> = "cell set --row r1 --col c2 --value Rome"
        .split(' ')
        .map(str::to_owned)
        .collect();
    let o = s
        .run_tool(a, k2_mpc_base::table::ROWS, &argv, b"")
        .expect("a cell set");
    assert_eq!(o.exit, 0);
    f.advance(Config::default().opened_hold_ms + 1);
    assert!(s.flush(a).unwrap().written, "the kit wrote the file");
    app_save(&path, b"id,city\nk1,Oslo\nk2,Lima\n");
    s.file_changed().unwrap();
    let text = String::from_utf8(s.read(a).unwrap().bytes).unwrap();
    assert!(
        text.contains("k1,Rome") && text.contains("k2,Lima"),
        "merged from the opened version: {text:?}"
    );
}

// ---------- a keyed doc: the field merge from the version the app opened ----------

const SHAPES: &str = "shapes";
const FIELDS: &str = "fields";
const SEP: char = '\u{1f}';
const DECK: Schema = Schema {
    name: "deck",
    roots: &[(SHAPES, Kind::MovableList), (FIELDS, Kind::Map)],
};

/// A keyed test pack: one line per shape, `<key>\t<field>=<value>...`; fields in a composite
/// map (`<shape>\x1f<field>`). Its rule (`check_merged_view`, when `checks`): no two shapes
/// at one `pos`.
struct Shapes {
    checks: bool,
}

fn bad(m: String) -> Declined {
    Declined {
        code: "malformed",
        message: m,
    }
}

impl Pack for Shapes {
    fn schema(&self) -> Schema {
        DECK
    }
    fn decode(&self, b: &[u8]) -> Result<PackContent, Declined> {
        let s = std::str::from_utf8(b).map_err(|e| bad(e.to_string()))?;
        let (mut list, mut fields) = (Vec::new(), BTreeMap::new());
        for line in s.lines() {
            let mut parts = line.split('\t');
            let key = Key::new(parts.next().unwrap_or("")).map_err(|e| bad(e.to_string()))?;
            for p in parts {
                let (f, v) = p
                    .split_once('=')
                    .ok_or_else(|| bad(format!("no = in {p:?}")))?;
                fields.insert(format!("{}{SEP}{f}", key.as_str()), Value::str(v));
            }
            list.push((key, Value::Null));
        }
        Ok(PackContent {
            roots: vec![
                (SHAPES.into(), Neutral::List(list)),
                (FIELDS.into(), Neutral::Map(fields)),
            ],
            codec: vec![1],
        })
    }
    fn encode(&self, c: &[(String, Neutral)], _: &[u8]) -> Result<Vec<u8>, PackError> {
        let view = View::of_content(c);
        let empty = BTreeMap::new();
        let fields = view.map(FIELDS).unwrap_or(&empty);
        let mut out = String::new();
        for (k, _) in view.list(SHAPES).unwrap_or(&[]) {
            out.push_str(k.as_str());
            let prefix = format!("{}{SEP}", k.as_str());
            for (fk, v) in fields.range(prefix.clone()..) {
                let Some(f) = fk.strip_prefix(&prefix) else {
                    break;
                };
                out.push_str(&format!("\t{f}={}", v.as_str().unwrap_or("")));
            }
            out.push('\n');
        }
        Ok(out.into_bytes())
    }
    fn reconcile(&self, synced: &View, bytes: &[u8], _: &[u8]) -> Result<Reconciled, Declined> {
        let content = self.decode(bytes)?;
        let new = View::of_content(&content.roots);
        let (old_l, new_l) = (
            synced.list(SHAPES).unwrap_or(&[]),
            new.list(SHAPES).unwrap_or(&[]),
        );
        let mut lops: Vec<ListOp> = old_l
            .iter()
            .filter(|(k, _)| !new_l.iter().any(|(n, _)| n == k))
            .map(|(k, _)| ListOp::Delete { key: k.clone() })
            .collect();
        lops.extend(
            new_l
                .iter()
                .filter(|(k, _)| !old_l.iter().any(|(o, _)| o == k))
                .map(|(k, v)| ListOp::Insert {
                    key: k.clone(),
                    at: Anchor::End,
                    value: v.clone(),
                }),
        );
        let empty = BTreeMap::new();
        let (old_m, new_m) = (
            synced.map(FIELDS).unwrap_or(&empty),
            new.map(FIELDS).unwrap_or(&empty),
        );
        let mut mops: Vec<MapOp> = old_m
            .keys()
            .filter(|k| !new_m.contains_key(*k))
            .map(|k| MapOp::Delete { key: k.clone() })
            .collect();
        mops.extend(
            new_m
                .iter()
                .filter(|(k, v)| old_m.get(*k) != Some(*v))
                .map(|(k, v)| MapOp::Set {
                    key: k.clone(),
                    value: v.clone(),
                }),
        );
        let mut edits = Vec::new();
        if !lops.is_empty() {
            edits.push(Edit::List {
                root: SHAPES.into(),
                ops: lops,
            });
        }
        if !mops.is_empty() {
            edits.push(Edit::Map {
                root: FIELDS.into(),
                ops: mops,
            });
        }
        Ok(Reconciled {
            edits,
            codec: content.codec,
            ..Reconciled::default()
        })
    }
    fn composite(&self) -> &'static [(&'static str, &'static str)] {
        &[(FIELDS, SHAPES)]
    }
    fn check_merged_view(&self, view: &View) -> Result<(), Declined> {
        if !self.checks {
            return Err(Declined {
                code: "merge_unchecked",
                message: "not checked".into(),
            });
        }
        let empty = BTreeMap::new();
        let mut seen = Vec::new();
        for (k, v) in view.map(FIELDS).unwrap_or(&empty) {
            if k.ends_with(&format!("{SEP}pos")) {
                let pos = v.as_str().unwrap_or("");
                if seen.contains(&pos) {
                    return Err(bad(format!("two shapes at {pos}")));
                }
                seen.push(pos);
            }
        }
        Ok(())
    }
}

const DECK_FILE: &str = "talk.deck";

/// The flow of the keyed rows: the file holds `start`; the app opens it (noted); a person sets
/// `field` of `shape` to `value` through the kit, written to the file once the hold is over;
/// the app saves the buffer it opened with its own change, `save`. Returns what the save became
/// and the doc as its file would hold it.
fn keyed_stale_save(
    f: &Fx,
    checks: bool,
    start: &str,
    (shape, field, value): (&str, &str, &str),
    save: &str,
) -> (DiskCheck, String) {
    let path = f.ws.join(DECK_FILE);
    fs::write(&path, start).unwrap();
    let s = f
        .mgr
        .open_file(
            Arc::new(Shapes { checks }),
            &DocKey {
                space: "ws".into(),
                path: DECK_FILE.into(),
            },
            &path,
            &f.ws,
        )
        .unwrap();
    let a = live_join(&s, "alice");
    s.note_opened(a, OpenedAt::FileNow).expect("noted");
    let key = format!("{shape}{SEP}{field}");
    let value = value.to_owned();
    s.edit_ops_with(a, &EditOptions::new(), move |_| {
        Ok::<_, std::convert::Infallible>(vec![Edit::Map {
            root: FIELDS.into(),
            ops: vec![MapOp::Set {
                key,
                value: Value::str(value),
            }],
        }])
    })
    .expect("the person's change");
    f.advance(Config::default().opened_hold_ms + 1);
    assert!(s.flush(a).unwrap().written, "the kit wrote the file");
    app_save(&path, save.as_bytes());
    let out = s.file_changed().unwrap();
    let doc = String::from_utf8(s.read(a).unwrap().bytes).unwrap();
    (out, doc)
}

const START: &str = "s1\tcolor=blue\ns2\tcolor=red\n";

/// A keyed doc: the person's field and the app's field merge, from the version the app opened.
#[test]
fn keyed_save_merges_from_opened() {
    let f = fx("keyed-merge");
    let (out, doc) = keyed_stale_save(
        &f,
        true,
        START,
        ("s1", "color", "green"),
        "s1\tcolor=blue\ns2\tcolor=red\tsize=big\n",
    );
    assert!(
        matches!(out, DiskCheck::Ingested { .. } | DiskCheck::Merged { .. }),
        "merged from the opened version: {out:?}"
    );
    assert_eq!(
        doc, "s1\tcolor=green\ns2\tcolor=red\tsize=big\n",
        "merged from the opened version"
    );
}

/// A clash keeps both: both changed one field to different values; or the app's save inserts a
/// key the live doc already holds.
#[test]
fn keyed_clash_keeps_both() {
    let f = fx("keyed-clash");
    let (out, doc) = keyed_stale_save(
        &f,
        true,
        START,
        ("s1", "color", "green"),
        "s1\tcolor=yellow\ns2\tcolor=red\n",
    );
    assert!(
        matches!(out, DiskCheck::Conflict { .. }),
        "a clash keeps both: {out:?}"
    );
    assert_eq!(
        doc, "s1\tcolor=yellow\ns2\tcolor=red\n",
        "the save is current"
    );
}

/// The pack's rules are checked on the merged doc: two shapes at one `pos` is refused, and the
/// save keeps both; a pack that doesn't check (the default) keeps both too.
#[test]
fn keyed_merge_checks_pack_rules() {
    let f = fx("keyed-rules");
    let (out, _) = keyed_stale_save(
        &f,
        true,
        "s1\tpos=1\ns2\tpos=2\n",
        ("s1", "pos", "3"),
        "s1\tpos=1\ns2\tpos=3\n",
    );
    assert!(
        matches!(out, DiskCheck::Conflict { .. }),
        "the pack refused the merged doc: {out:?}"
    );
    let f = fx("keyed-unchecked");
    let (out, _) = keyed_stale_save(
        &f,
        false,
        START,
        ("s1", "color", "green"),
        "s1\tcolor=blue\ns2\tcolor=red\tsize=big\n",
    );
    assert!(
        matches!(out, DiskCheck::Conflict { .. }),
        "a pack that doesn't check keeps both: {out:?}"
    );
}

// ---------- a save's own stamp names its base ----------

/// [`LineSets`] whose first line is a save stamp, `id=<n>` or `id=<n> on=<m>` (the save it was
/// made on).
struct StampedSets;
impl BlobPack for StampedSets {
    fn can_merge(&self) -> bool {
        true
    }
    fn merge3_bytes(&self, base: &[u8], ours: &[u8], theirs: &[u8]) -> BlobMerge {
        LineSets.merge3_bytes(base, ours, theirs)
    }
    fn save_lineage(&self, bytes: &[u8]) -> Option<Lineage> {
        let first = std::str::from_utf8(bytes).ok()?.lines().next()?;
        let mut words = first.split(' ');
        let this = words.next()?.strip_prefix("id=")?;
        let on = words.next().and_then(|w| w.strip_prefix("on="));
        Some(Lineage::new(
            this.as_bytes().to_vec(),
            on.map(|o| o.as_bytes().to_vec()),
        ))
    }
}

/// The stamped flow: the file holds `id=1 a`; the app opens it (noted); a person writes
/// `id=2 on=1 a w`, written to the file after the hold; the app then saves `save`.
fn stamped_save(f: &Fx, save: &str) -> (DiskCheck, String) {
    let path = f.ws.join("stamped.txt");
    fs::write(&path, "id=1\na\n").unwrap();
    let s = f
        .mgr
        .open_blob_file(
            Arc::new(StampedSets),
            &DocKey {
                space: "ws".into(),
                path: "stamped.txt".into(),
            },
            &path,
            &f.ws,
        )
        .unwrap();
    let a = join(&s, "alice", ParticipantKind::Person);
    s.note_opened(a, OpenedAt::FileNow).expect("noted");
    let v = version(&s);
    s.write_bytes(a, b"id=2 on=1\na\nw\n", v)
        .expect("the kit's write");
    f.advance(Config::default().opened_hold_ms + 1);
    s.flush(a).expect("a flush");
    app_save(&path, save.as_bytes());
    let out = s.file_changed().unwrap();
    (out, fs::read_to_string(&path).unwrap())
}

/// The save's stamp names the version it was made on, ahead of the pin: the app re-read the
/// kit's write (stamp `on=2`), removed `w` and added `p`, and the save is taken from that
/// version, so `w` stays removed (a merge from the pinned version would bring it back).
#[test]
fn stamped_base_first() {
    let f = fx("stamped");
    let (out, file) = stamped_save(&f, "id=3 on=2\na\np\n");
    let lines: Vec<&str> = file.lines().collect();
    assert!(
        lines.contains(&"p") && !lines.contains(&"w"),
        "the save's stamp decides: {out:?} {file:?}"
    );
}

/// A stamp that names no version the kit holds is another writer's line: the save keeps both,
/// pin or not.
#[test]
fn unknown_stamp_keeps_both() {
    let f = fx("stamp-unknown");
    let (out, _) = stamped_save(&f, "id=9 on=7\na\np\n");
    assert!(
        matches!(out, DiskCheck::Conflict { .. }),
        "the save's stamp decides: {out:?}"
    );
}

// ---------- N−1: the previous release carries the pin (tools/n1/n1.sh node-opened) ----------

fn n1_dir() -> PathBuf {
    PathBuf::from(std::env::var("K2MB_N1_OUT").expect("K2MB_N1_OUT"))
}

/// This build's store in `dir` (docs.redb, blobs/, the files in ws/), as the N−1 halves use it.
fn n1_fx(dir: &Path) -> Fx {
    let host = Arc::new(TestHost {
        now: AtomicU64::new(T0),
        rights: Mutex::new(HashMap::new()),
        events: Mutex::new(Vec::new()),
    });
    host.rights
        .lock()
        .unwrap()
        .insert("alice".into(), Rights::full().with_owner(true));
    let ws = dir.join("ws");
    fs::create_dir_all(&ws).unwrap();
    let ws = fs::canonicalize(ws).unwrap();
    let (store, blobs, mgr) = managers(dir, &host);
    Fx {
        dir: TmpDir(PathBuf::new()),
        ws,
        host,
        store,
        blobs,
        mgr,
    }
}

/// Writes the store the previous release's consumer reads: a blob doc (`paper.pdf`) and two live
/// docs (`keep.md`, `gone.md`), each with a pin (`opened.txt`: each one's pinned version).
#[test]
#[ignore = "tools/n1/n1.sh: writes the store for the previous release's consumer"]
fn n1_opened_producer() {
    let dir = n1_dir();
    let f = n1_fx(&dir);
    fs::write(f.path(), PDF1).unwrap();
    let b = f.open();
    let a = join(&b, "alice", ParticipantKind::Person);
    let pdf = b.note_opened(a, OpenedAt::FileNow).expect("noted").version;
    let mut lines = format!("paper.pdf {pdf}\n");
    for name in ["keep.md", "gone.md"] {
        let path = f.ws.join(name);
        fs::write(&path, "one\ntwo\n").unwrap();
        let s = f
            .mgr
            .open_file(
                Arc::new(TEXT),
                &DocKey {
                    space: "ws".into(),
                    path: name.into(),
                },
                &path,
                &f.ws,
            )
            .unwrap();
        let c = live_join(&s, "alice");
        let v = s.note_opened(c, OpenedAt::FileNow).expect("noted").version;
        lines.push_str(&format!("{name} {v}\n"));
    }
    fs::write(dir.join("opened.txt"), lines).unwrap();
    f.mgr.release().unwrap();
}

/// Reads the store the previous release left: it rewrote the file and deleted `gone.md`, and
/// the pins of `paper.pdf` and `keep.md` are there, as this build wrote them.
#[test]
#[ignore = "tools/n1/n1.sh: reads the store the previous release's consumer left"]
fn n1_opened_after() {
    let dir = n1_dir();
    let want: HashMap<String, u64> = fs::read_to_string(dir.join("opened.txt"))
        .unwrap()
        .lines()
        .map(|l| {
            let (n, v) = l.split_once(' ').unwrap();
            (n.to_owned(), v.parse().unwrap())
        })
        .collect();
    let f = n1_fx(&dir);
    let b = f.open();
    assert_eq!(
        b.opened().unwrap().map(|p| p.version),
        Some(want["paper.pdf"]),
        "the blob doc's pin, through the previous release's rewrite"
    );
    let open = |name: &str| {
        f.mgr
            .open_file(
                Arc::new(TEXT),
                &DocKey {
                    space: "ws".into(),
                    path: name.into(),
                },
                &f.ws.join(name),
                &f.ws,
            )
            .unwrap()
    };
    assert_eq!(
        open("keep.md").opened().unwrap().map(|p| p.version),
        Some(want["keep.md"]),
        "the live doc's pin, through the previous release's rewrite"
    );
    assert_eq!(
        open("gone.md").opened().unwrap(),
        None,
        "a deleted doc's pin went with it"
    );
    f.mgr.release().unwrap();
    println!("N-1 OPENED (this build) OK");
}
