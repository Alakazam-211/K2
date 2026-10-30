//! Disk sync (C4) as a host uses it, on real temp folders: write-back, outside edits, echo
//! suppression, renames, deletes, declines, reconciling after a restart, and a plain editor
//! working alongside agents. The clock is the host's (injected); the kit never sleeps.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use k2_mpc_base::session::{
    is_write_temp, AuditEvent, Config, ConnId, DiskCheck, DiskState, Host, Pack, Participant,
    ParticipantKind, Rights, Session, SessionError, SessionManager, Stay, SyncDrift, TextFilePack,
    UndoOutcome, EXTERNAL_PID,
};
use k2_mpc_base::store::{DocKey, DocStore, VersionReason};
use k2_mpc_base::text::merge::{
    self, closed, merge3_hunks, outside_merge_sides, outside_sides, Merge,
};
use proptest::prelude::*;

const MD: TextFilePack = TextFilePack::new("markdown", 4 << 20);
const ROOT: &str = TextFilePack::ROOT;

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
    /// The workspace: files live here.
    ws: PathBuf,
    host: Arc<TestHost>,
    store: Arc<DocStore>,
    mgr: SessionManager,
}

fn fx_with(tag: &str, config: Config) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let p = std::env::temp_dir().join(format!(
        "k2mb-disk-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = fs::remove_dir_all(&p);
    let ws = p.join("ws");
    fs::create_dir_all(&ws).unwrap();
    let ws = fs::canonicalize(ws).unwrap();
    let host = Arc::new(TestHost {
        now: AtomicU64::new(1_000_000),
        rights: Mutex::new(HashMap::new()),
        events: Mutex::new(Vec::new()),
    });
    let store = Arc::new(DocStore::open(p.join("docs.redb")).unwrap());
    let mgr = SessionManager::new(store.clone(), host.clone(), config);
    Fx {
        dir: TmpDir(p),
        ws,
        host,
        store,
        mgr,
    }
}

fn fx(tag: &str) -> Fx {
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    fx_with(tag, c)
}

fn key(p: &str) -> DocKey {
    DocKey {
        space: "ws".into(),
        path: p.into(),
    }
}

fn pack() -> Arc<dyn Pack> {
    Arc::new(MD)
}

impl Fx {
    fn path(&self, name: &str) -> PathBuf {
        self.ws.join(name)
    }
    fn write(&self, name: &str, bytes: &[u8]) {
        fs::write(self.path(name), bytes).unwrap();
    }
    fn bytes(&self, name: &str) -> Vec<u8> {
        fs::read(self.path(name)).unwrap()
    }
    fn open(&self, name: &str) -> Arc<Session> {
        self.mgr
            .open_file(pack(), &key(name), &self.path(name), &self.ws)
            .unwrap()
    }
    fn join(&self, s: &Session, pid: &str) -> ConnId {
        self.host
            .rights
            .lock()
            .unwrap()
            .insert(pid.into(), Rights::editor());
        s.join(
            Participant::new(pid, pid, "#123456", ParticipantKind::Agent, "cli"),
            Stay::Held,
        )
        .unwrap()
        .conn
    }
    fn advance(&self, ms: u64) {
        self.host.now.fetch_add(ms, Ordering::SeqCst);
    }
    fn tick(&self) -> k2_mpc_base::session::TickReport {
        let r = self.mgr.tick().unwrap();
        assert!(r.errors.is_empty(), "tick errors: {:?}", r.errors);
        r
    }
    fn events(&self) -> Vec<AuditEvent> {
        self.host.events.lock().unwrap().clone()
    }
    /// A restart: a new manager on the same store (the old one is dropped without flushing,
    /// like a crash, unless the test released it first).
    fn restart(&mut self) {
        let mgr = SessionManager::new(
            self.store.clone(),
            self.host.clone(),
            self.mgr.config().clone(),
        );
        self.mgr = mgr;
    }
    fn temps(&self) -> Vec<String> {
        fs::read_dir(&self.ws)
            .unwrap()
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter(|n| is_write_temp(n))
            .collect()
    }
}

fn text(s: &Session, c: ConnId) -> String {
    s.read(c).unwrap().text_by_root.remove(0).1
}

fn replace(s: &Session, c: ConnId, old: &str, new: &str) {
    let r = s
        .edit_with(c, ROOT, |t| match t.find(old) {
            Some(_) => Ok(t.replacen(old, new, 1)),
            None => Err(format!("{old:?} not in {t:?}")),
        })
        .unwrap();
    assert!(r.is_some(), "replace {old:?} changed nothing");
}

fn mtime(p: &Path) -> SystemTime {
    fs::metadata(p).unwrap().modified().unwrap()
}

fn disk(s: &Session) -> DiskState {
    s.status().unwrap().disk.expect("a file-backed session")
}

/// Replace `path` the way vim does: write a new file beside it, then rename over.
fn rename_save(path: &Path, bytes: &[u8]) {
    let tmp = path.with_extension("swp-new");
    fs::write(&tmp, bytes).unwrap();
    fs::rename(&tmp, path).unwrap();
}

#[cfg(unix)]
fn ino(p: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    fs::metadata(p).unwrap().ino()
}

// ---------- tests ----------

#[test]
fn untouched_open_and_close_never_write() {
    let mut f = fx("untouched");
    let original = b"\xEF\xBB\xBF# Title\r\n\r\nbody\r\n".to_vec();
    f.write("a.md", &original);
    let p = f.path("a.md");
    let t0 = mtime(&p);
    let s = f.open("a.md");
    assert_eq!(disk(&s), DiskState::Clean);
    let c = f.join(&s, "reader");
    assert_eq!(text(&s, c), "# Title\n\nbody\n");
    for _ in 0..10 {
        f.advance(2_000);
        let r = f.tick();
        assert!(r.written.is_empty());
    }
    s.leave(c).unwrap();
    drop(s);
    f.advance(60 * 60_000);
    assert_eq!(f.tick().unloaded, 1, "unloads, writing nothing");
    // reopen after a restart, and release
    f.restart();
    let s = f.open("a.md");
    assert_eq!(disk(&s), DiskState::Clean);
    drop(s);
    f.mgr.release().unwrap();
    assert_eq!(f.bytes("a.md"), original, "byte for byte");
    assert_eq!(mtime(&p), t0, "never rewritten");
    assert!(f.temps().is_empty());
}

#[test]
fn write_back_after_quiet_or_max_delay_atomically_keeping_the_mode() {
    let f = fx("writeback");
    f.write("a.md", b"one\ntwo\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(f.path("a.md"), fs::Permissions::from_mode(0o640)).unwrap();
    }
    let s = f.open("a.md");
    let a = f.join(&s, "agent");
    let t_edit = f.host.now_ms();
    replace(&s, a, "two", "TWO");
    assert_eq!(disk(&s), DiskState::Dirty { since_ms: t_edit });
    // not yet: 0.5 s of quiet
    f.advance(500);
    assert!(f.tick().written.is_empty());
    assert_eq!(f.bytes("a.md"), b"one\ntwo\n");
    // 1 s of quiet: written
    f.advance(500);
    assert_eq!(f.tick().written, vec![key("a.md")]);
    assert_eq!(f.bytes("a.md"), b"one\nTWO\n");
    assert_eq!(disk(&s), DiskState::Clean);
    assert!(f.temps().is_empty(), "no temp file left: {:?}", f.temps());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(f.path("a.md")).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o640, "mode kept");
    }
    // continuous typing (an edit every 400 ms) is written within 5 s of the first change
    let first = f.host.now_ms();
    let mut written_at = None;
    for i in 0..20 {
        replace(&s, a, "one", &format!("one{i}"));
        f.advance(400);
        if !f.tick().written.is_empty() {
            written_at = Some(f.host.now_ms());
            break;
        }
        replace(&s, a, &format!("one{i}"), "one");
    }
    let at = written_at.expect("written during continuous typing");
    assert!(at - first >= 5_000 && at - first < 5_400, "{}", at - first);
    // a change undone before the write-back writes nothing
    f.advance(2_000);
    f.tick();
    let before = mtime(&f.path("a.md"));
    replace(&s, a, "TWO", "two");
    replace(&s, a, "two", "TWO");
    f.advance(2_000);
    assert!(f.tick().written.is_empty());
    assert_eq!(mtime(&f.path("a.md")), before);
    assert_eq!(disk(&s), DiskState::Clean);
}

#[test]
fn own_writes_are_echoes_not_edits() {
    let f = fx("echo");
    f.write("a.md", b"a\n");
    let s = f.open("a.md");
    let c = f.join(&s, "agent");
    let watch = s.subscribe(c).unwrap();
    replace(&s, c, "a", "A");
    let v1 = s.flush(c).unwrap();
    assert!(v1.written);
    assert_eq!(v1.disk, Some(DiskState::Clean));
    let older = f.bytes("a.md");
    // the watcher reports our own write: nothing happens
    assert_eq!(s.file_changed().unwrap(), DiskCheck::Unchanged);
    replace(&s, c, "A", "AB");
    assert!(s.flush(c).unwrap().written);
    let version = s.status().unwrap().version;
    // an editor saves a stale buffer holding an older write of ours: not an edit; the
    // next write-back puts the doc back
    f.write("a.md", &older);
    assert_eq!(s.file_changed().unwrap(), DiskCheck::OwnWrite);
    assert_eq!(s.status().unwrap().version, version, "nothing ingested");
    assert!(matches!(disk(&s), DiskState::Dirty { .. }));
    f.advance(1_000);
    assert_eq!(f.tick().written.len(), 1);
    assert_eq!(f.bytes("a.md"), b"AB\n");
    let evs: Vec<_> = watch.try_iter().collect();
    assert!(evs.iter().all(|e| e.pid == "agent"), "no external change");
    // a second flush has nothing to write and reports the same hash
    let again = s.flush(c).unwrap();
    assert!(!again.written);
    assert_eq!(again.hash.len(), 64);
    // the suppressed echo was reported, not silent (v0.9.5)
    assert_eq!(
        f.events()
            .iter()
            .filter(|e| matches!(e, AuditEvent::EchoSuppressed { .. }))
            .count(),
        1
    );
}

/// Echo detection (v0.9.5): a save whose bytes equal a recent write of the kit's is an echo
/// only if no outside save was taken in since that write. Here a person's save brings back
/// the bytes of a kit write made before their previous save: it is their save, merged as
/// any other, never discarded and overwritten.
#[test]
fn a_save_that_recreates_a_kit_write_from_before_an_outside_save_is_kept() {
    let f = fx("echo-after-save");
    f.write("a.md", b"x y\n");
    let s = f.open("a.md");
    let a = f.join(&s, "agent");
    // an agent empties the line (not written back yet)
    replace(&s, a, "x y", "");
    // a person appends a line; it is merged, and the kit writes the result (its own write)
    f.advance(100);
    f.write("a.md", b"x y\nZ\n");
    assert!(matches!(
        s.file_changed().unwrap(),
        DiskCheck::Ingested { .. }
    ));
    assert!(s.flush(a).unwrap().written);
    assert_eq!(f.bytes("a.md"), b"\nZ\n");
    // the person deletes their line and saves
    f.advance(1_000);
    f.write("a.md", b"\n");
    assert!(matches!(
        s.file_changed().unwrap(),
        DiskCheck::Ingested { .. }
    ));
    assert_eq!(disk(&s), DiskState::Clean);
    // within the echo window, they put it back (an undo, or retyped) and save
    f.advance(1_000);
    f.write("a.md", b"\nZ\n");
    assert!(matches!(
        s.file_changed().unwrap(),
        DiskCheck::Ingested { .. }
    ));
    assert_eq!(text(&s, a), "\nZ\n", "the person's save is kept");
    f.advance(2_000);
    f.tick();
    assert_eq!(f.bytes("a.md"), b"\nZ\n", "never overwritten");
    assert!(!f
        .events()
        .iter()
        .any(|e| matches!(e, AuditEvent::EchoSuppressed { .. })));
    assert_eq!(s.audit_disk_sync().unwrap(), vec![]);
}

/// A true stale buffer is still an echo (v0.9.5): the kit writes W1, then W2, and an editor
/// that never saved in between saves its old buffer, W1. Not an edit: reported, and the
/// next write-back puts W2 back.
#[test]
fn a_stale_buffer_of_an_older_kit_write_is_still_an_echo() {
    let f = fx("echo-stale");
    f.write("a.md", b"one\n");
    let s = f.open("a.md");
    let a = f.join(&s, "agent");
    replace(&s, a, "one", "two");
    assert!(s.flush(a).unwrap().written);
    let w1 = f.bytes("a.md");
    replace(&s, a, "two", "three");
    assert!(s.flush(a).unwrap().written);
    f.advance(1_000);
    f.write("a.md", &w1);
    assert_eq!(s.file_changed().unwrap(), DiskCheck::OwnWrite);
    assert_eq!(text(&s, a), "three\n");
    assert_eq!(
        f.events()
            .iter()
            .filter(|e| matches!(e, AuditEvent::EchoSuppressed { .. }))
            .count(),
        1
    );
    f.advance(1_000);
    f.tick();
    assert_eq!(f.bytes("a.md"), b"three\n");
}

#[test]
fn an_outside_edit_merges_with_unwritten_agent_edits_between_versions() {
    let f = fx("external");
    let seed: String = (1..=8).map(|i| format!("line {i}\n")).collect();
    f.write("a.md", seed.as_bytes());
    let s = f.open("a.md");
    let a = f.join(&s, "agent");
    let watch = s.subscribe(a).unwrap();
    // the agent edits line 2; not written yet
    replace(&s, a, "line 2", "line 2 (agent)");
    assert!(matches!(disk(&s), DiskState::Dirty { .. }));
    // meanwhile a person edits line 7 in a plain editor (from the file as it was)
    let edited = seed.replace("line 7", "line 7 (person)");
    f.write("a.md", edited.as_bytes());
    let before = s.status().unwrap().version;
    let DiskCheck::Ingested { version } = s.file_changed().unwrap() else {
        panic!("expected an ingest");
    };
    assert_eq!(version, before + 1);
    let t = text(&s, a);
    assert!(
        t.contains("line 2 (agent)") && t.contains("line 7 (person)"),
        "{t}"
    );
    // attributed to the external participant, via disk, with versions around it
    let evs: Vec<_> = watch.try_iter().collect();
    let ext = evs.last().unwrap();
    assert_eq!(
        (ext.pid.as_str(), ext.via.as_str(), ext.version),
        (EXTERNAL_PID, "disk", version)
    );
    let id = f.store.find(&key("a.md")).unwrap().unwrap();
    let vs = f.store.versions(id).unwrap();
    let tail: Vec<_> = vs.iter().rev().take(2).rev().map(|(_, v)| v).collect();
    assert_eq!(tail[0].reason, VersionReason::BeforeExternal);
    assert_eq!(tail[0].seq, before);
    assert_eq!(tail[0].authors, vec!["agent".to_owned()]);
    assert_eq!(tail[1].reason, VersionReason::External);
    assert_eq!(tail[1].seq, version);
    assert_eq!(tail[1].authors, vec![EXTERNAL_PID.to_owned()]);
    let log = f.store.load(id).unwrap().updates;
    let (_, e) = log.last().unwrap();
    assert_eq!((e.author.as_str(), e.via.as_str()), (EXTERNAL_PID, "disk"));
    // the next write-back writes the merged doc
    assert!(matches!(disk(&s), DiskState::Dirty { .. }));
    f.advance(1_000);
    f.tick();
    assert_eq!(f.bytes("a.md"), s.read(a).unwrap().bytes);
    // a second outside edit right after: no "before" version (nothing changed in between)
    let n = f.store.versions(id).unwrap().len();
    let now = String::from_utf8(f.bytes("a.md")).unwrap();
    f.write("a.md", now.replace("line 1", "line one").as_bytes());
    assert!(matches!(
        s.file_changed().unwrap(),
        DiskCheck::Ingested { .. }
    ));
    assert_eq!(f.store.versions(id).unwrap().len(), n + 1);
    // the external participant keeps one client ID per doc
    let ext_events: Vec<_> = watch.try_iter().filter(|e| e.pid == EXTERNAL_PID).collect();
    assert_eq!(ext_events[0].client, ext.client);
    assert!(f.events().iter().any(|e| matches!(
        e,
        AuditEvent::ExternalEdit { seq, .. } if *seq == version
    )));
}

#[cfg(unix)]
#[test]
fn a_rename_save_is_seen_by_its_inode_alone() {
    let f = fx("vim");
    f.write("a.md", b"abc\n");
    let p = f.path("a.md");
    let s = f.open("a.md");
    let c = f.join(&s, "agent");
    let (i0, t0) = (ino(&p), mtime(&p));
    // same size, same mtime, new inode
    let tmp = f.path("a.md.new");
    fs::write(&tmp, b"xyz\n").unwrap();
    fs::File::options()
        .write(true)
        .open(&tmp)
        .unwrap()
        .set_modified(t0)
        .unwrap();
    fs::rename(&tmp, &p).unwrap();
    assert_ne!(ino(&p), i0);
    assert_eq!(mtime(&p), t0);
    assert!(matches!(
        s.check_disk().unwrap(),
        DiskCheck::Ingested { .. }
    ));
    assert_eq!(text(&s, c), "xyz\n");
    // and the stat backup finds nothing more
    assert_eq!(s.check_disk().unwrap(), DiskCheck::Unchanged);
    // the stat backup runs from tick while someone is present
    rename_save(&p, b"vim saved\n");
    f.advance(3_000);
    assert_eq!(f.tick().ingested.len(), 1);
    assert_eq!(text(&s, c), "vim saved\n");
}

#[test]
fn delete_goes_missing_writes_nothing_and_recreate_merges() {
    let f = fx("missing");
    f.write("a.md", b"keep\nme\n");
    let s = f.open("a.md");
    let c = f.join(&s, "agent");
    fs::remove_file(f.path("a.md")).unwrap();
    replace(&s, c, "me", "me too");
    assert_eq!(s.file_changed().unwrap(), DiskCheck::Missing);
    assert_eq!(disk(&s), DiskState::Missing);
    assert!(f.events().iter().any(|e| matches!(
        e,
        AuditEvent::FileMissing {
            unflushed: true,
            ..
        }
    )));
    for _ in 0..5 {
        f.advance(3_000);
        assert!(f.tick().written.is_empty());
    }
    let flushed = s.flush(c).unwrap();
    assert!(!flushed.written);
    assert_eq!(flushed.disk, Some(DiskState::Missing));
    assert!(!f.path("a.md").exists(), "never recreated by the kit");
    // replaced by a folder: still missing
    fs::create_dir(f.path("a.md")).unwrap();
    assert_eq!(s.file_changed().unwrap(), DiskCheck::Missing);
    fs::remove_dir(f.path("a.md")).unwrap();
    // recreated (git checkout) with an extra line: merged, and the edit made meanwhile kept
    f.write("a.md", b"keep\nme\nnew line\n");
    assert!(matches!(
        s.file_changed().unwrap(),
        DiskCheck::Ingested { .. }
    ));
    assert_eq!(text(&s, c), "keep\nme too\nnew line\n");
    assert!(f
        .events()
        .iter()
        .any(|e| matches!(e, AuditEvent::FileRestored { .. })));
    f.advance(1_000);
    f.tick();
    assert_eq!(f.bytes("a.md"), b"keep\nme too\nnew line\n");
    assert_eq!(disk(&s), DiskState::Clean);
}

#[test]
fn restarts_reconcile_edits_made_while_closed_and_unwritten_changes() {
    let mut f = fx("restart");
    f.write("a.md", b"1\n2\n3\n");
    let s = f.open("a.md");
    let c = f.join(&s, "agent");
    replace(&s, c, "1", "one");
    drop(s);
    f.mgr.release().unwrap();
    assert_eq!(f.bytes("a.md"), b"one\n2\n3\n", "release writes back");
    // edited while no session was loaded
    f.write("a.md", b"one\n2\nthree\n");
    f.restart();
    let s = f.open("a.md");
    let c = f.join(&s, "agent");
    assert_eq!(text(&s, c), "one\n2\nthree\n");
    let id = f.store.find(&key("a.md")).unwrap().unwrap();
    assert_eq!(
        f.store.versions(id).unwrap().last().unwrap().1.reason,
        VersionReason::External
    );
    // a crash between a change and its write-back: the change is in the log...
    replace(&s, c, "2", "two");
    drop(s);
    // ...and the file is edited too before the next start
    f.write("a.md", b"ONE\n2\nthree\n");
    f.restart(); // the old manager is dropped without flushing
    let s = f.open("a.md");
    let c = f.join(&s, "agent");
    assert_eq!(text(&s, c), "ONE\ntwo\nthree\n", "both kept");
    assert!(matches!(disk(&s), DiskState::Dirty { .. }));
    f.advance(1_000);
    f.tick();
    assert_eq!(f.bytes("a.md"), b"ONE\ntwo\nthree\n");
    // a crash with no outside edit: the reconcile just writes the change
    replace(&s, c, "three", "3");
    drop(s);
    f.restart();
    let s = f.open("a.md");
    assert!(matches!(disk(&s), DiskState::Dirty { .. }));
    let c = f.join(&s, "agent");
    assert_eq!(text(&s, c), "ONE\ntwo\n3\n");
    assert!(s.flush(c).unwrap().written);
    assert_eq!(f.bytes("a.md"), b"ONE\ntwo\n3\n");
    // the file gone while closed: opens missing, keeps the doc
    drop(s);
    f.mgr.release().unwrap();
    fs::remove_file(f.path("a.md")).unwrap();
    f.restart();
    let s = f.open("a.md");
    assert_eq!(disk(&s), DiskState::Missing);
    // a doc that doesn't exist yet needs its file
    assert!(matches!(
        f.mgr
            .open_file(pack(), &key("none.md"), &f.path("none.md"), &f.ws),
        Err(SessionError::NotFound(_))
    ));
}

#[test]
fn crlf_and_bom_survive_edits_both_ways() {
    let f = fx("crlf");
    f.write("a.md", b"\xEF\xBB\xBFa\r\nb\r\n");
    let s = f.open("a.md");
    let c = f.join(&s, "agent");
    replace(&s, c, "b\n", "b\nc\r\n");
    assert_eq!(text(&s, c), "a\nb\nc\n", "the doc holds no \\r");
    assert!(s.flush(c).unwrap().written);
    assert_eq!(f.bytes("a.md"), b"\xEF\xBB\xBFa\r\nb\r\nc\r\n");
    // an outside CRLF edit comes in as text
    f.write("a.md", b"\xEF\xBB\xBFA\r\nb\r\nc\r\n");
    assert!(matches!(
        s.file_changed().unwrap(),
        DiskCheck::Ingested { .. }
    ));
    assert_eq!(text(&s, c), "A\nb\nc\n");
    // a tool that converts the file to LF without a BOM: same text, new format, adopted
    f.write("a.md", b"A\nb\nc\n");
    assert_eq!(s.file_changed().unwrap(), DiskCheck::Unchanged);
    replace(&s, c, "c", "C");
    assert!(s.flush(c).unwrap().written);
    assert_eq!(f.bytes("a.md"), b"A\nb\nC\n");
}

#[test]
fn a_file_that_stops_decoding_is_declined_until_it_decodes_again() {
    let mut cfg = Config::default();
    cfg.disk_read_max_bytes = 64;
    let f = fx_with("declined", cfg);
    f.write("a.md", b"text\n");
    let s = f.open("a.md");
    let c = f.join(&s, "agent");
    for (bad, code) in [
        (&b"bin\0ary\n"[..], "binary"),
        (&b"\xff\xfe"[..], "not_utf8"),
        (&b"a\r\nb\n"[..], "mixed_line_endings"),
        (&[b'x'; 100][..], "too_large"),
    ] {
        f.write("a.md", bad);
        match s.file_changed().unwrap() {
            DiskCheck::Declined(d) => assert_eq!(d.code, code),
            other => panic!("{code}: {other:?}"),
        }
        assert!(matches!(disk(&s), DiskState::Declined(_)), "{code}");
        assert!(s.status().unwrap().declined.is_some());
        assert!(matches!(
            s.replace_text(c, ROOT, "no"),
            Err(SessionError::Declined(_))
        ));
        f.advance(10_000);
        assert!(f.tick().written.is_empty());
        assert!(!s.flush(c).unwrap().written);
        assert_eq!(f.bytes("a.md"), bad, "never overwritten while declined");
    }
    assert!(f
        .events()
        .iter()
        .any(|e| matches!(e, AuditEvent::FileDeclined { code: "binary", .. })));
    // fixed: merged in, live again
    f.write("a.md", b"text\nfixed\n");
    assert!(matches!(
        s.file_changed().unwrap(),
        DiskCheck::Ingested { .. }
    ));
    assert_eq!(disk(&s), DiskState::Clean);
    assert_eq!(s.status().unwrap().declined, None);
    replace(&s, c, "fixed", "fixed again");
    assert!(s.flush(c).unwrap().written);
    assert_eq!(f.bytes("a.md"), b"text\nfixed again\n");
    // declined while closed: reopening keeps it declined until the file is fixed
    f.write("a.md", b"\0");
    assert!(matches!(s.file_changed().unwrap(), DiskCheck::Declined(_)));
    drop(s);
    f.mgr.release().unwrap();
    let mgr = SessionManager::new(f.store.clone(), f.host.clone(), f.mgr.config().clone());
    let s = mgr
        .open_file(pack(), &key("a.md"), &f.path("a.md"), &f.ws)
        .unwrap();
    assert!(matches!(disk(&s), DiskState::Declined(_)));
    f.write("a.md", b"text\nfixed again\n");
    assert_eq!(s.file_changed().unwrap(), DiskCheck::Unchanged);
    assert_eq!(disk(&s), DiskState::Clean, "back to the synced content");
}

#[cfg(unix)]
#[test]
fn paths_are_jailed_and_symlinks_resolved() {
    let f = fx("paths");
    f.write("real.md", b"r\n");
    std::os::unix::fs::symlink(f.path("real.md"), f.path("link.md")).unwrap();
    let outside = f.dir.0.join("outside.md");
    fs::write(&outside, b"o\n").unwrap();
    std::os::unix::fs::symlink(&outside, f.path("escape.md")).unwrap();
    fs::create_dir(f.path("folder.md")).unwrap();
    let open = |n: &str| {
        f.mgr
            .open_file(pack(), &key(n), &f.path(n), &f.ws)
            .map(|_| ())
    };
    assert!(matches!(open("escape.md"), Err(SessionError::BadPath(_))));
    assert!(matches!(open("folder.md"), Err(SessionError::BadPath(_))));
    assert!(matches!(
        f.mgr
            .open_file(pack(), &key("x"), &f.path("real.md"), &f.ws.join("nope")),
        Err(SessionError::BadPath(_))
    ));
    // a symlink inside the root: write-back goes to its target; the link stays a link
    let s = f.open("link.md");
    let c = f.join(&s, "agent");
    replace(&s, c, "r", "R");
    assert!(s.flush(c).unwrap().written);
    assert!(fs::symlink_metadata(f.path("link.md"))
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(f.bytes("real.md"), b"R\n");
    assert_eq!(fs::read(&outside).unwrap(), b"o\n");
    // re-pointed outside the root later: treated as gone, never written through
    fs::remove_file(f.path("link.md")).unwrap();
    std::os::unix::fs::symlink(&outside, f.path("link.md")).unwrap();
    replace(&s, c, "R", "RR");
    assert_eq!(s.file_changed().unwrap(), DiskCheck::Missing);
    assert!(!s.flush(c).unwrap().written);
    assert_eq!(fs::read(&outside).unwrap(), b"o\n");
}

#[test]
fn a_paired_rename_moves_the_sync() {
    let f = fx("moved");
    f.write("old.md", b"x\n");
    let s = f.open("old.md");
    let c = f.join(&s, "agent");
    fs::rename(f.path("old.md"), f.path("new.md")).unwrap();
    f.mgr.rekey(&key("old.md"), key("new.md")).unwrap();
    assert_eq!(
        s.file_moved(&f.path("new.md")).unwrap(),
        DiskCheck::Unchanged
    );
    assert_eq!(s.file_path().unwrap(), Some(f.path("new.md")));
    replace(&s, c, "x", "y");
    assert!(s.flush(c).unwrap().written);
    assert_eq!(f.bytes("new.md"), b"y\n");
    assert!(!f.path("old.md").exists());
    assert!(matches!(
        s.file_moved(&f.path("nowhere.md")),
        Err(SessionError::BadPath(_))
    ));
    // sessions without a file have no disk state and nothing to flush
    let mem = f.mgr.open(pack(), &key("mem.md"), Some(b"m\n")).unwrap();
    assert_eq!(mem.status().unwrap().disk, None);
    let mc = f.join(&mem, "agent");
    let out = mem.flush(mc).unwrap();
    assert!(!out.written && out.disk.is_none());
    assert_eq!(mem.file_changed().unwrap(), DiskCheck::Unchanged);
}

#[test]
fn flush_needs_read() {
    let f = fx("flush-rights");
    f.write("a.md", b"a\n");
    let s = f.open("a.md");
    let c = f.join(&s, "agent");
    f.host
        .rights
        .lock()
        .unwrap()
        .insert("agent".into(), Rights::none());
    assert!(matches!(s.flush(c), Err(SessionError::Forbidden { .. })));
}

/// A tiny deterministic generator (xorshift), so a failing seed reproduces.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// A change that leaves the bytes as they were (an edit and its undo, or an edit and its
/// inverse) between write-backs, then an outside edit: the outside edit is kept, exactly.
/// The file's previous bytes are the doc's rendering, so the outside edit must land as the
/// diff from them, whatever the doc's history in between. Until v0.9.4 the disk replica kept
/// the text on its old elements (the doc had put it back as new ones), and the edit landed
/// beside the copy: `LINE three3`, `line 3\nLINE three4`, `LINEline 3three`.
#[test]
fn an_outside_edit_after_a_change_that_moved_no_byte_is_kept() {
    let seed: String = (1..=6).map(|i| format!("line {i}\n")).collect();
    type Change = fn(&Session, ConnId);
    let changes: [(&str, Change); 4] = [
        ("type-undo", |s, a| {
            replace(s, a, "line 3", "line 3 X");
            s.undo(a, 1).unwrap();
        }),
        ("delete-undo", |s, a| {
            replace(s, a, "line 3\n", "");
            s.undo(a, 1).unwrap();
        }),
        ("delete-inverse", |s, a| {
            replace(s, a, "line 3\n", "");
            replace(s, a, "line 4", "line 3\nline 4");
        }),
        ("rewrite-inverse", |s, a| {
            replace(s, a, "line 3", "other");
            replace(s, a, "other", "line 3");
        }),
    ];
    let mut lost = Vec::new();
    for (tag, change) in changes {
        for flushed in [false, true] {
            let f = fx(&format!("nobyte-{tag}-{flushed}"));
            f.write("a.md", seed.as_bytes());
            let s = f.open("a.md");
            let a = f.join(&s, "agent");
            change(&s, a);
            assert_eq!(text(&s, a), seed, "{tag}: the change moved no byte");
            if flushed {
                // a write-back runs and finds nothing to write
                assert!(!s.flush(a).unwrap().written, "{tag}");
            }
            f.advance(2_000);
            assert!(f.tick().written.is_empty(), "{tag}: nothing to write");
            assert_eq!(disk(&s), DiskState::Clean, "{tag}");
            assert_eq!(s.audit_disk_sync().unwrap(), vec![], "{tag}: K-AUD-1");
            // a person edits the changed line in a plain editor
            let edited = seed.replace("line 3", "LINE three");
            f.advance(1_000);
            f.write("a.md", edited.as_bytes());
            assert!(
                matches!(s.file_changed().unwrap(), DiskCheck::Ingested { .. }),
                "{tag}"
            );
            let got = text(&s, a);
            f.advance(2_000);
            f.tick();
            let file = String::from_utf8(f.bytes("a.md")).unwrap();
            if got != edited || file != edited {
                lost.push(format!(
                    "{tag} (flushed {flushed}): doc {got:?}, file {file:?}"
                ));
            }
        }
    }
    assert!(
        lost.is_empty(),
        "outside edits lost (want {:?}):\n{}",
        seed.replace("line 3", "LINE three"),
        lost.join("\n")
    );
}

/// K-AUD-1's sync check (v0.9.4) with planted violations: a disk replica holding the doc's
/// text on other elements (the stale base an outside edit was corrupted on), and one holding
/// another text, are both reported while the doc is clean; the next write-back that finds
/// nothing to write makes the replica the doc again.
#[test]
fn the_sync_audit_reports_a_stale_or_wrong_disk_replica() {
    let f = fx("audit-plant");
    f.write("a.md", b"one\ntwo\n");
    let s = f.open("a.md");
    let a = f.join(&s, "agent");
    assert_eq!(s.audit_disk_sync().unwrap(), vec![], "as imported");
    replace(&s, a, "two", "2");
    assert!(s.flush(a).unwrap().written);
    assert_eq!(s.audit_disk_sync().unwrap(), vec![], "after a write-back");
    let root = || ROOT.to_owned();
    // the stale base: the doc's text on fresh elements
    s.plant_disk_replica_for_tests(ROOT, "one\n2\n").unwrap();
    assert_eq!(
        s.audit_disk_sync().unwrap(),
        vec![SyncDrift::Elements { root: root() }]
    );
    // a write-back with nothing to write brings it up to the doc
    assert!(!s.flush(a).unwrap().written);
    assert_eq!(s.audit_disk_sync().unwrap(), vec![], "re-synced");
    // a wrong text
    s.plant_disk_replica_for_tests(ROOT, "one\n").unwrap();
    assert_eq!(
        s.audit_disk_sync().unwrap(),
        vec![SyncDrift::Text { root: root() }]
    );
    // only a clean doc is audited: unwritten changes are the replica's to merge
    replace(&s, a, "one", "1");
    assert!(matches!(disk(&s), DiskState::Dirty { .. }));
    assert_eq!(s.audit_disk_sync().unwrap(), vec![]);
    assert!(s.flush(a).unwrap().written);
    assert_eq!(s.audit_disk_sync().unwrap(), vec![]);
    // a session without a file has nothing to audit
    let plain = f.mgr.open(pack(), &key("plain.md"), Some(b"x\n")).unwrap();
    assert_eq!(plain.audit_disk_sync().unwrap(), vec![]);
}

/// A change that moves no byte, by every route a doc offers.
#[derive(Debug, Clone, Copy)]
enum NoByte {
    EditUndo,
    DeleteUndo,
    DeleteRetype,
    RewriteBack,
    Revert,
    Restore,
}

/// What happens between that change and the outside edit.
#[derive(Debug, Clone, Copy)]
enum Between {
    Nothing,
    Tick,
    Flush,
    /// A crash: the manager is dropped without a write-back, and the doc reopened.
    Restart,
    FlushRestart,
}

/// `[lo, hi)` of `t` from two fractions (ASCII text: every index is a boundary).
fn span(t: &str, p: u16, q: u16) -> (usize, usize) {
    let at = |x: u16| (x as usize * t.len()) / (u16::MAX as usize + 1);
    let (a, b) = (at(p), at(q));
    (a.min(b), a.max(b))
}

fn set(s: &Session, c: ConnId, new: &str) -> k2_mpc_base::session::EditResult {
    s.edit_with(c, ROOT, |_| Ok::<_, ()>(new.to_owned()))
        .unwrap()
        .expect("the edit changes the text")
}

/// `t` with `[lo, hi)` replaced by `ins`, changed for sure.
fn spliced(t: &str, (lo, hi): (usize, usize), ins: &str) -> String {
    let mut ins = ins.to_owned();
    if t[lo..hi] == ins {
        ins.push('Z');
    }
    format!("{}{ins}{}", &t[..lo], &t[hi..])
}

/// The property behind `an_outside_edit_after_a_change_that_moved_no_byte_is_kept`: a text
/// doc (lines from a small vocabulary, so text repeats), a random change that moves no byte,
/// then nothing, a tick, a flush or a restart, then a random outside edit. The doc and the
/// file must both end as the outside edit applied to the text before the change, and the
/// sync audit must find nothing. `NO_BYTE_CASES` sets the count (default 256; 10^4 on a
/// node).
///
/// Its twin for grid docs (a header turned off and on, a cell set back to its value) lands
/// with C1, which brings grid docs to the kit.
fn no_byte_cases() -> u32 {
    std::env::var("NO_BYTE_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(256)
}

fn no_byte_doc() -> impl Strategy<Value = String> {
    prop::collection::vec(
        prop::sample::select(vec!["a", "b", "line 3", "x y", "", "line 3 x"]),
        2..10,
    )
    .prop_map(|ls| ls.iter().map(|l| format!("{l}\n")).collect())
}

fn no_byte_insert() -> impl Strategy<Value = String> {
    proptest::string::string_regex("[ab3 \n]{0,6}").unwrap()
}

fn no_byte_change() -> impl Strategy<Value = NoByte> {
    prop::sample::select(vec![
        NoByte::EditUndo,
        NoByte::DeleteUndo,
        NoByte::DeleteRetype,
        NoByte::RewriteBack,
        NoByte::Revert,
        NoByte::Restore,
    ])
}

fn no_byte_between() -> impl Strategy<Value = Between> {
    prop::sample::select(vec![
        Between::Nothing,
        Between::Tick,
        Between::Flush,
        Between::Restart,
        Between::FlushRestart,
    ])
}

#[allow(clippy::too_many_arguments)]
fn no_byte_case(
    seed: &str,
    change: NoByte,
    at: (u16, u16),
    ins: &str,
    between: Between,
    out_at: (u16, u16),
    out_ins: &str,
) {
    let why = format!("{change:?} then {between:?}");
    let mut f = fx("nobyte-prop");
    f.write("a.md", seed.as_bytes());
    let mut s = f.open("a.md");
    let a = f.join(&s, "agent");
    f.host
        .rights
        .lock()
        .unwrap()
        .insert("agent".into(), Rights::full());
    let (lo, hi) = span(seed, at.0, at.1);
    match change {
        NoByte::EditUndo => {
            set(&s, a, &spliced(seed, (lo, hi), ins));
            assert!(matches!(s.undo(a, 1).unwrap(), UndoOutcome::Applied { .. }));
        }
        NoByte::DeleteUndo | NoByte::DeleteRetype => {
            let hi = if hi == lo { lo + 1 } else { hi };
            let gone = &seed[lo..hi];
            set(&s, a, &format!("{}{}", &seed[..lo], &seed[hi..]));
            if matches!(change, NoByte::DeleteUndo) {
                assert!(matches!(s.undo(a, 1).unwrap(), UndoOutcome::Applied { .. }));
            } else {
                s.edit_with(a, ROOT, |t| {
                    Ok::<_, ()>(format!("{}{gone}{}", &t[..lo], &t[lo..]))
                })
                .unwrap();
            }
        }
        NoByte::RewriteBack => {
            set(&s, a, &spliced(seed, (lo, hi), ins));
            set(&s, a, seed);
        }
        NoByte::Revert => {
            let r = set(&s, a, &spliced(seed, (lo, hi), ins));
            let out = s.revert(a, r.version, false).unwrap();
            assert!(matches!(out, UndoOutcome::Applied { .. }), "{why}: {out:?}");
        }
        NoByte::Restore => {
            let v = s.name_version(a, "before", None).unwrap();
            set(&s, a, &spliced(seed, (lo, hi), ins));
            s.restore(a, v).unwrap();
        }
    }
    assert_eq!(text(&s, a), seed, "{why}: the change moved no byte");
    if matches!(between, Between::Flush | Between::FlushRestart) {
        assert!(!s.flush(a).unwrap().written, "{why}: nothing to write");
        assert_eq!(s.audit_disk_sync().unwrap(), vec![], "{why}: K-AUD-1");
    }
    if matches!(between, Between::Tick) {
        f.advance(2_000);
        assert!(f.tick().written.is_empty(), "{why}: nothing to write");
        assert_eq!(s.audit_disk_sync().unwrap(), vec![], "{why}: K-AUD-1");
    }
    let mut a = a;
    if matches!(between, Between::Restart | Between::FlushRestart) {
        drop(s);
        f.restart();
        s = f.open("a.md");
        a = f.join(&s, "agent");
        assert_eq!(disk(&s), DiskState::Clean, "{why}: reopened clean");
        assert_eq!(s.audit_disk_sync().unwrap(), vec![], "{why}: K-AUD-1");
    }
    assert_eq!(
        f.bytes("a.md"),
        seed.as_bytes(),
        "{why}: the file untouched"
    );
    // a plain editor changes the file
    let edited = spliced(seed, span(seed, out_at.0, out_at.1), out_ins);
    f.advance(1_000);
    f.write("a.md", edited.as_bytes());
    let check = s.file_changed().unwrap();
    assert!(
        matches!(check, DiskCheck::Ingested { .. }),
        "{why}: {check:?}"
    );
    assert_eq!(text(&s, a), edited, "{why}: the doc is the outside edit");
    f.advance(2_000);
    f.tick();
    s.flush(a).unwrap();
    let file = String::from_utf8(f.bytes("a.md")).unwrap();
    assert_eq!(file, edited, "{why}: the file is the outside edit");
    assert_eq!(disk(&s), DiskState::Clean, "{why}");
    assert_eq!(
        s.audit_disk_sync().unwrap(),
        vec![],
        "{why}: K-AUD-1 at the end"
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(no_byte_cases()))]
    #[test]
    fn an_outside_edit_after_any_change_that_moved_no_byte_is_kept(
        seed in no_byte_doc(),
        change in no_byte_change(),
        at in (any::<u16>(), any::<u16>()),
        ins in no_byte_insert(),
        between in no_byte_between(),
        out_at in (any::<u16>(), any::<u16>()),
        out_ins in no_byte_insert(),
    ) {
        no_byte_case(&seed, change, at, &ins, between, out_at, &out_ins);
    }
}

/// The v0.9.4 re-sync ran only when the doc rendered the synced bytes (at a write-back, an
/// outside edit or a load). A change that moves no byte followed by **another edit not yet
/// written back** left the doc dirty, so nothing re-synced the replica, and an outside edit
/// arriving before the write-back (`write_quiet_ms`, at most `write_max_delay_ms`) was made
/// on the stale replica and corrupted as before v0.9.4 (`LINE three3`). Fixed in v0.9.5: an
/// outside save is a three-way merge onto the live text; the replica's elements aren't used.
#[test]
fn an_outside_edit_after_a_no_byte_change_and_an_unwritten_edit_is_kept() {
    let f = fx("nobyte-dirty");
    let seed: String = (1..=6).map(|i| format!("line {i}\n")).collect();
    f.write("a.md", seed.as_bytes());
    let s = f.open("a.md");
    let a = f.join(&s, "agent");
    // a change that moves no byte, then an edit elsewhere, not yet written back
    replace(&s, a, "line 3\n", "");
    s.undo(a, 1).unwrap();
    replace(&s, a, "line 6", "line 6 Y");
    assert!(matches!(disk(&s), DiskState::Dirty { .. }));
    // a plain editor changes the re-created line
    f.advance(100);
    f.write("a.md", seed.replace("line 3", "LINE three").as_bytes());
    assert!(matches!(
        s.file_changed().unwrap(),
        DiskCheck::Ingested { .. }
    ));
    let want = seed
        .replace("line 3", "LINE three")
        .replace("line 6", "line 6 Y");
    assert_eq!(text(&s, a), want);
    assert!(s.flush(a).unwrap().written);
    assert_eq!(String::from_utf8(f.bytes("a.md")).unwrap(), want);
    assert_eq!(s.audit_disk_sync().unwrap(), vec![], "K-AUD-1");
}

/// The other order (v0.9.5): a line deleted, another edited (not written back), then the
/// delete undone. The doc never renders the synced bytes at a commit, yet the line lives on
/// new elements; an outside edit to it must land exactly, before and after a restart.
#[test]
fn an_outside_edit_after_an_undo_behind_an_unwritten_edit_is_kept() {
    let seed: String = (1..=6).map(|i| format!("line {i}\n")).collect();
    for restart in [false, true] {
        let mut f = fx(&format!("undo-behind-{restart}"));
        f.write("a.md", seed.as_bytes());
        let mut s = f.open("a.md");
        let a = f.join(&s, "agent");
        let b = f.join(&s, "other");
        replace(&s, a, "line 3\n", "");
        replace(&s, b, "line 6", "line 6 Y");
        assert!(matches!(s.undo(a, 1).unwrap(), UndoOutcome::Applied { .. }));
        assert!(matches!(disk(&s), DiskState::Dirty { .. }));
        let mut a = a;
        if restart {
            drop(s);
            f.restart();
            s = f.open("a.md");
            a = f.join(&s, "agent");
            assert!(matches!(disk(&s), DiskState::Dirty { .. }));
        }
        f.advance(100);
        f.write("a.md", seed.replace("line 3", "LINE three").as_bytes());
        assert!(matches!(
            s.file_changed().unwrap(),
            DiskCheck::Ingested { .. }
        ));
        let want = seed
            .replace("line 3", "LINE three")
            .replace("line 6", "line 6 Y");
        assert_eq!(text(&s, a), want, "restart {restart}");
        assert!(s.flush(a).unwrap().written);
        assert_eq!(String::from_utf8(f.bytes("a.md")).unwrap(), want);
        assert_eq!(s.audit_disk_sync().unwrap(), vec![], "K-AUD-1");
    }
}

/// An outside save that overlaps a change not yet written back keeps both, as the CRDT merge
/// of an outside edit did before v0.9.5 (kept explicitly in v0.9.5, `merge3_keep_both`), at
/// line granularity: what either side deleted is gone and every line either side wrote
/// lands whole, the live one first at the same point. Never a conflict and never a conflict
/// copy (those are tier 0's).
#[test]
fn an_overlapping_outside_save_keeps_both_sides() {
    let seed: String = (1..=6).map(|i| format!("line {i}\n")).collect();
    let f = fx("overlap");
    f.write("a.md", seed.as_bytes());
    let s = f.open("a.md");
    let a = f.join(&s, "agent");
    // the agent rewrites line 3 and deletes line 5; neither is written back
    replace(&s, a, "line 3", "AAA");
    replace(&s, a, "line 5\n", "");
    f.advance(100);
    // the plain editor rewrites line 3 too, and changes line 5's text
    let saved = seed.replace("line 3", "ZZZ").replace("line 5", "QQQQQQ");
    f.write("a.md", saved.as_bytes());
    assert!(matches!(
        s.file_changed().unwrap(),
        DiskCheck::Ingested { .. }
    ));
    // both rewrites of line 3, whole lines, the live one first; line 5's new text kept as
    // its own line, the line the agent deleted gone
    let want = seed
        .replace("line 3", "AAA\nZZZ")
        .replace("line 5\n", "QQQQQQ\n");
    assert_eq!(text(&s, a), want);
    assert_eq!(
        want,
        keep_both_model(
            &seed,
            &saved,
            &seed.replace("line 3", "AAA").replace("line 5\n", "")
        )
    );
    assert!(s.flush(a).unwrap().written);
    assert_eq!(String::from_utf8(f.bytes("a.md")).unwrap(), want);
    assert_eq!(s.audit_disk_sync().unwrap(), vec![], "K-AUD-1");
}

/// The disk replica's marker is the doc's change counter (v0.9.5): synced at version
/// `synced_seq`, stale once the version moves past it, a delete-only change included (v0.9.4
/// compared state vectors, which a delete-only change leaves equal: harmless for text, where
/// such a change always moves bytes, but in a structured doc deleting an empty cell moves
/// none). The counter is stored with the sync state, so a restart neither re-syncs a replica
/// that is up to date nor skips one that isn't.
#[test]
fn the_disk_replica_follows_the_change_counter_across_restarts() {
    let seed: String = (1..=6).map(|i| format!("line {i}\n")).collect();
    let mut f = fx("counter");
    f.write("a.md", seed.as_bytes());
    let mut s = f.open("a.md");
    let mut a = f.join(&s, "agent");
    let point = |s: &Session| s.disk_sync_point_for_tests().unwrap().unwrap();
    let synced = |s: &Session| {
        let (p, v) = point(s);
        p == v
    };
    assert!(synced(&s), "as imported");
    // a delete-only change: stale until written back
    replace(&s, a, "line 3\n", "");
    assert!(!synced(&s));
    assert!(s.flush(a).unwrap().written);
    assert!(synced(&s), "the write-back syncs it");
    assert_eq!(s.audit_disk_sync().unwrap(), vec![]);
    // a change that moves no byte: the write-back writes nothing and re-syncs it
    replace(&s, a, "line 4\n", "");
    s.undo(a, 1).unwrap();
    assert!(!synced(&s));
    assert!(!s.flush(a).unwrap().written);
    assert!(synced(&s), "re-synced");
    assert_eq!(s.audit_disk_sync().unwrap(), vec![]);
    // a restart keeps the point: nothing to re-sync
    let before = point(&s);
    drop(s);
    f.restart();
    s = f.open("a.md");
    a = f.join(&s, "agent");
    assert_eq!(point(&s), before, "reopened at the same sync point");
    assert_eq!(s.audit_disk_sync().unwrap(), vec![]);
    // a change that moves no byte, then a crash before its write-back: re-synced on open
    replace(&s, a, "line 5", "x");
    replace(&s, a, "x", "line 5");
    assert!(!synced(&s));
    drop(s);
    f.restart();
    s = f.open("a.md");
    a = f.join(&s, "agent");
    assert!(synced(&s), "re-synced when opened");
    assert_eq!(disk(&s), DiskState::Clean);
    assert_eq!(s.audit_disk_sync().unwrap(), vec![]);
    // an outside save the doc holds more than: synced at the save's version, dirty, and the
    // write-back makes the replica the doc
    replace(&s, a, "line 1", "line 1 A");
    let on_disk = String::from_utf8(f.bytes("a.md")).unwrap();
    f.advance(10);
    f.write("a.md", on_disk.replace("line 6", "line 6 O").as_bytes());
    assert!(matches!(
        s.file_changed().unwrap(),
        DiskCheck::Ingested { .. }
    ));
    assert!(synced(&s));
    assert!(matches!(disk(&s), DiskState::Dirty { .. }));
    drop(s);
    f.restart();
    s = f.open("a.md");
    a = f.join(&s, "agent");
    assert!(synced(&s), "the point survives a restart");
    assert!(matches!(disk(&s), DiskState::Dirty { .. }));
    assert!(s.flush(a).unwrap().written);
    assert!(synced(&s));
    assert_eq!(
        text(&s, a),
        seed.replace("line 3\n", "")
            .replace("line 1", "line 1 A")
            .replace("line 6", "line 6 O")
    );
    assert_eq!(s.audit_disk_sync().unwrap(), vec![]);
}

/// An outside save whose text is what the doc's file already held (the editor deleted text
/// and typed it again, or rewrote the file with the same content; in place or by rename, or
/// the same text in other line endings) changes no element of the doc: the merge's hunks on
/// an unchanged root are none, so nothing is applied, logged, carried or recorded (v0.9.5).
/// With and without a change of the doc's not yet written back.
#[test]
fn an_outside_save_of_identical_text_touches_nothing() {
    for dirty in [false, true] {
        let f = fx(&format!("same-text-{dirty}"));
        f.write("a.md", b"one\nxy two\nthree\n");
        let s = f.open("a.md");
        let a = f.join(&s, "agent");
        f.host
            .rights
            .lock()
            .unwrap()
            .insert("agent".into(), Rights::full());
        replace(&s, a, "three", "three (agent)");
        assert!(s.flush(a).unwrap().written);
        if dirty {
            replace(&s, a, "one", "one (unwritten)");
        }
        let watch = s.subscribe(a).unwrap();
        let id = f.store.find(&key("a.md")).unwrap().unwrap();
        let state = |s: &Session| {
            (
                text(s, a),
                s.status().unwrap().version,
                // element identity: each line's writer and change
                s.blame(a, ROOT, None).unwrap().lines,
                // no change committed: so no log entry, revert record or undo region
                f.store.load(id).unwrap().updates.len(),
                f.store.versions(id).unwrap().len(),
            )
        };
        let before = state(&s);
        let file = f.bytes("a.md");
        // the same bytes: in place, then by rename (a delete and a retype of `xy` in an
        // editor, saved)
        f.advance(100);
        f.write("a.md", &file);
        assert_eq!(
            s.file_changed().unwrap(),
            DiskCheck::Unchanged,
            "dirty {dirty}"
        );
        f.advance(100);
        rename_save(&f.path("a.md"), &file);
        assert_eq!(
            s.file_changed().unwrap(),
            DiskCheck::Unchanged,
            "dirty {dirty}"
        );
        assert_eq!(state(&s), before, "dirty {dirty}: same bytes");
        // the same text in other bytes (CRLF): adopted as the file's format, the doc untouched
        let crlf = String::from_utf8(file.clone())
            .unwrap()
            .replace('\n', "\r\n");
        f.advance(100);
        f.write("a.md", crlf.as_bytes());
        assert_eq!(
            s.file_changed().unwrap(),
            DiskCheck::Unchanged,
            "dirty {dirty}"
        );
        assert_eq!(state(&s), before, "dirty {dirty}: same text");
        assert!(
            watch.try_iter().next().is_none(),
            "dirty {dirty}: no change event"
        );
        assert_eq!(s.audit_disk_sync().unwrap(), vec![], "dirty {dirty}");
    }
}

/// One step of [`mix_case`]: a change that moves no byte, a delete undone behind another
/// participant's edit, a real edit, time passing (a tick: a write-back if due), a flush, a
/// crash and reopen, or an outside save (with the session open, or while it is closed).
#[derive(Debug, Clone)]
enum Mix {
    NoByte(NoByte, (u16, u16), String),
    UndoBehind((u16, u16), (u16, u16), String),
    Edit((u16, u16), String),
    Wait(u64),
    Flush,
    Restart,
    Outside((u16, u16), String, bool),
}

fn mix_step() -> impl Strategy<Value = Mix> {
    let at = || (any::<u16>(), any::<u16>());
    prop_oneof![
        3 => (no_byte_change(), at(), no_byte_insert()).prop_map(|(c, a, i)| Mix::NoByte(c, a, i)),
        2 => (at(), at(), no_byte_insert()).prop_map(|(a, b, i)| Mix::UndoBehind(a, b, i)),
        3 => (at(), no_byte_insert()).prop_map(|(a, i)| Mix::Edit(a, i)),
        // mostly inside the write delay (quiet 1 s, at most 5 s), sometimes past it
        2 => (0u64..6_000).prop_map(Mix::Wait),
        1 => Just(Mix::Flush),
        1 => Just(Mix::Restart),
        3 => (at(), no_byte_insert(), prop::bool::weighted(0.2))
            .prop_map(|(a, i, c)| Mix::Outside(a, i, c)),
    ]
}

/// Every step list ends with an outside save.
fn mix_steps() -> impl Strategy<Value = Vec<Mix>> {
    (
        prop::collection::vec(mix_step(), 1..8),
        (any::<u16>(), any::<u16>()),
        no_byte_insert(),
    )
        .prop_map(|(mut v, a, i)| {
            v.push(Mix::Outside(a, i, false));
            v
        })
}

/// The keep-both rule character by character (the model of an outside save's merge, v0.9.7:
/// on the closed texts' sides, whole lines): at each offset of the closed `base`, the
/// insertions there, then the base character unless either side deletes it; a change made
/// identically on both sides is taken once. Then the end: no final "\n" when the side that
/// wrote the last line left its text so; for a base line, or a change both made, as the
/// side that changed the base's ending (the file first).
fn keep_both_model(base: &str, file: &str, live: &str) -> String {
    use k2_mpc_base::text::Hunk;
    let open = |t: &str| !t.is_empty() && !t.ends_with('\n');
    let cb = closed(base);
    // the sides as the merge takes them: whole lines of the closed texts
    let (a, b, _) = outside_merge_sides(base, file, live);
    let both: Vec<Hunk> = b.iter().filter(|y| a.contains(y)).cloned().collect();
    let a: Vec<Hunk> = a.into_iter().filter(|x| !b.contains(x)).collect();
    // who wrote the last character: 0 live, 1 file, 2 both or the base
    let mut author = 2;
    let mut out = String::new();
    let mut at: Vec<(usize, Option<char>)> = cb.char_indices().map(|(i, c)| (i, Some(c))).collect();
    at.push((cb.len(), None));
    for (p, c) in at {
        // pure insertions, then replacements; the live side first within each (as merge3
        // places a clean merge's)
        let mut here: Vec<(bool, usize, &Hunk)> = b
            .iter()
            .map(|h| (0, h))
            .chain(a.iter().map(|h| (1, h)))
            .filter(|(_, h)| h.start == p)
            .map(|(side, h)| (!h.is_insertion(), side, h))
            .collect();
        here.sort_by_key(|(r, side, _)| (*r, *side));
        for (_, side, h) in here {
            if !h.insert.is_empty() {
                out.push_str(&h.insert);
                author = if both.contains(h) { 2 } else { side };
            }
        }
        if let Some(c) = c {
            if !b.iter().chain(&a).any(|h| h.start <= p && p < h.end) {
                out.push(c);
                author = 2;
            }
        }
    }
    let end_open = match author {
        0 => open(live),
        1 => open(file),
        _ if open(file) != open(base) => open(file),
        _ => open(live),
    };
    // an empty last line can't be left open (that would delete it)
    if end_open && out.ends_with('\n') && out != "\n" && !out.ends_with("\n\n") {
        out.pop();
    }
    out
}

/// Each insertion of `base` → `new` (on the closed texts), in order, as disjoint pieces of
/// `got` (closed).
fn inserts_kept(base: &str, new: &str, got: &str) -> bool {
    let got = closed(got);
    let mut from = 0;
    for h in outside_sides(base, new, base).0 {
        match got[from..].find(&h.insert) {
            Some(i) => from += i + h.insert.len(),
            None => return false,
        }
    }
    true
}

fn no_pending(s: &Session) -> bool {
    !s.audit_disk_sync()
        .unwrap()
        .iter()
        .any(|d| matches!(d, SyncDrift::Pending { .. }))
}

/// The property behind v0.9.5's fix: a text doc, a random mix of changes that move no byte
/// and real ones (some written back, some not), time passing inside and past the write
/// delay, restarts, and outside saves at random points. At each outside save, with S the
/// synced text (the file before it), D the live text and O the file's new text:
/// 1. when the three-way merge is clean, the doc is its result: `merge3_hunks` (the pair
///    rules of `write --base`) on the two sides S→O and S→D, each diffed at whole lines
///    (`outside_sides`; v0.9.7, before that with its pure line insertions slid to line
///    starts, as outside edits always were); the
///    save is a `write --base` of O on S, up to the order of two appends at the very end;
///    except that, in an outside save, a line break separates text one side left without a
///    final line break (when the original text ended with one) from a change of the other
///    side that is inserted at the end of the text;
/// 2. no outside change is lost and 3. no unwritten doc change is lost: the doc is the
///    keep-both model (every deletion of either side, every insertion of both), and each
///    side's insertions are in it in order;
/// 4. no update is left pending: the ingest succeeds (an apply that leaves one poisons the
///    doc and fails), K-AUD-1 finds no [`SyncDrift::Pending`], and a reopen from the store
///    gives the same text.
///
/// At the end the file is the doc, the doc is clean, and K-AUD-1 finds nothing.
/// `NO_BYTE_CASES` sets the count (default 256; 10^4 on nodes, sharded).
#[allow(clippy::too_many_lines)]
fn mix_case(seed: &str, steps: &[Mix]) {
    let mut f = fx("mix");
    f.write("a.md", seed.as_bytes());
    let mut s = f.open("a.md");
    let join = |f: &Fx, s: &Session| {
        let a = f.join(s, "agent");
        let b = f.join(s, "other");
        f.host
            .rights
            .lock()
            .unwrap()
            .insert("agent".into(), Rights::full());
        (a, b)
    };
    let (mut a, mut b) = join(&f, &s);
    for (k, step) in steps.iter().enumerate() {
        let why = format!("step {k} {step:?} of {steps:?}");
        let t = text(&s, a);
        match step {
            Mix::NoByte(change, at, ins) if !t.is_empty() => {
                let (lo, hi) = span(&t, at.0, at.1);
                match change {
                    NoByte::EditUndo => {
                        set(&s, a, &spliced(&t, (lo, hi), ins));
                        assert!(matches!(s.undo(a, 1).unwrap(), UndoOutcome::Applied { .. }));
                    }
                    NoByte::DeleteUndo | NoByte::DeleteRetype => {
                        let (lo, hi) = match (lo, hi) {
                            (l, h) if h > l => (l, h),
                            (l, _) if l < t.len() => (l, l + 1),
                            (l, _) => (l - 1, l),
                        };
                        let gone = t[lo..hi].to_owned();
                        set(&s, a, &format!("{}{}", &t[..lo], &t[hi..]));
                        if matches!(change, NoByte::DeleteUndo) {
                            assert!(matches!(s.undo(a, 1).unwrap(), UndoOutcome::Applied { .. }));
                        } else {
                            s.edit_with(a, ROOT, |x| {
                                Ok::<_, ()>(format!("{}{gone}{}", &x[..lo], &x[lo..]))
                            })
                            .unwrap();
                        }
                    }
                    NoByte::RewriteBack => {
                        set(&s, a, &spliced(&t, (lo, hi), ins));
                        set(&s, a, &t);
                    }
                    NoByte::Revert => {
                        let r = set(&s, a, &spliced(&t, (lo, hi), ins));
                        let out = s.revert(a, r.version, false).unwrap();
                        assert!(matches!(out, UndoOutcome::Applied { .. }), "{why}: {out:?}");
                    }
                    NoByte::Restore => {
                        let v = s.name_version(a, "before", None).unwrap();
                        set(&s, a, &spliced(&t, (lo, hi), ins));
                        s.restore(a, v).unwrap();
                    }
                }
                assert_eq!(text(&s, a), t, "{why}: the change moved no byte");
            }
            Mix::UndoBehind(at, at2, ins) if !t.is_empty() => {
                let (lo, hi) = span(&t, at.0, at.1);
                let (lo, hi) = if hi > lo {
                    (lo, hi)
                } else if lo < t.len() {
                    (lo, lo + 1)
                } else {
                    (lo - 1, lo)
                };
                set(&s, a, &format!("{}{}", &t[..lo], &t[hi..]));
                let t1 = text(&s, b);
                set(&s, b, &spliced(&t1, span(&t1, at2.0, at2.1), ins));
                // undone behind the other edit (applied, or partly when they overlap)
                s.undo_with(
                    a,
                    &k2_mpc_base::session::UndoRequest::steps(1)
                        .mode(k2_mpc_base::session::UndoMode::Partial),
                )
                .unwrap();
            }
            Mix::NoByte(..) | Mix::UndoBehind(..) | Mix::Edit(..) => {
                let (at, ins) = match step {
                    Mix::Edit(at, ins) | Mix::NoByte(_, at, ins) | Mix::UndoBehind(at, _, ins) => {
                        (at, ins)
                    }
                    _ => unreachable!(),
                };
                set(&s, b, &spliced(&t, span(&t, at.0, at.1), ins));
            }
            Mix::Wait(ms) => {
                f.advance(*ms);
                f.tick();
            }
            Mix::Flush => {
                s.flush(a).unwrap();
            }
            Mix::Restart => {
                drop(s);
                f.restart();
                s = f.open("a.md");
                (a, b) = join(&f, &s);
                assert_eq!(text(&s, a), t, "{why}: the store kept every change");
            }
            Mix::Outside(at, ins, closed) => {
                let synced = String::from_utf8(f.bytes("a.md")).unwrap();
                let live = t.clone();
                let saved = spliced(&synced, span(&synced, at.0, at.1), ins);
                f.advance(10);
                let check = if *closed {
                    // saved while the doc isn't loaded (a crash first): merged on open
                    drop(s);
                    f.restart();
                    f.write("a.md", saved.as_bytes());
                    s = f.open("a.md");
                    (a, b) = join(&f, &s);
                    None
                } else {
                    f.write("a.md", saved.as_bytes());
                    Some(s.file_changed().unwrap())
                };
                if matches!(check, Some(DiskCheck::OwnWrite)) {
                    // an older write of the kit's came back: not an edit (documented); the
                    // write-back puts the doc back
                    assert_eq!(text(&s, a), live, "{why}: an echo changes nothing");
                    assert!(s.flush(a).unwrap().written, "{why}");
                    continue;
                }
                assert!(
                    matches!(
                        check,
                        None | Some(DiskCheck::Ingested { .. } | DiskCheck::Unchanged)
                    ),
                    "{why}: {check:?}"
                );
                let got = text(&s, a);
                // 1. a clean merge is merge3's on the closed texts (v0.9.7), but for its final
                // "\n" (the end is merged on its own)
                let (o, d) = outside_sides(&synced, &saved, &live);
                let cs = merge::closed(&synced);
                if let Merge::Clean(c) = merge3_hunks(&cs, &o, &d, !cs.is_empty()) {
                    assert_eq!(
                        *merge::closed(&got),
                        c.text,
                        "{why}: S {synced:?} O {saved:?} D {live:?}"
                    );
                }
                // 2 and 3. nothing either side changed is lost
                assert_eq!(
                    got,
                    keep_both_model(&synced, &saved, &live),
                    "{why}: S {synced:?} O {saved:?} D {live:?}"
                );
                assert!(inserts_kept(&synced, &saved, &got), "{why}: outside lost");
                assert!(inserts_kept(&synced, &live, &got), "{why}: unwritten lost");
                // 4. nothing pending
                assert!(no_pending(&s), "{why}: pending");
            }
        }
        assert!(no_pending(&s), "{why}: pending");
        assert!(
            s.audit_disk_sync().unwrap().is_empty(),
            "{why}: K-AUD-1 {:?}",
            s.audit_disk_sync()
        );
    }
    let live = text(&s, a);
    s.flush(a).unwrap();
    assert_eq!(
        String::from_utf8(f.bytes("a.md")).unwrap(),
        live,
        "the file is the doc"
    );
    assert_eq!(disk(&s), DiskState::Clean);
    assert_eq!(s.audit_disk_sync().unwrap(), vec![], "K-AUD-1 at the end");
    // 4. the store replays to the same doc
    drop(s);
    f.restart();
    let s = f.open("a.md");
    let a = f.join(&s, "agent");
    assert_eq!(text(&s, a), live, "reopened from the store");
    assert_eq!(disk(&s), DiskState::Clean);
    assert!(no_pending(&s));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(no_byte_cases()))]
    #[test]
    fn an_outside_save_amid_any_mix_of_changes_is_merged(
        seed in no_byte_doc(),
        steps in mix_steps(),
    ) {
        mix_case(&seed, &steps);
    }
}

/// The sync audit's cost on a realistic doc (5,000 lines, ~340 KB, 1,000 changes since the
/// import), and a re-sync's (one full encode and load). Ignored; run it in release:
/// `cargo test -p k2-mpc-base --release --test disk -- --ignored --nocapture audit_cost`.
#[test]
#[ignore]
fn audit_cost() {
    use std::time::Instant;
    let f = fx("audit-cost");
    let seed: String = (0..5_000)
        .map(|i| format!("{i:05} Some prose in a markdown file, a line of a realistic length.\n"))
        .collect();
    f.write("a.md", seed.as_bytes());
    let s = f.open("a.md");
    let a = f.join(&s, "agent");
    let mut rng = Rng(7);
    for k in 0..1_000 {
        let line = rng.below(5_000);
        let _ = s.edit_with(a, ROOT, |t| {
            let at = t.find(&format!("{line:05} ")).map_or(0, |i| i + 6);
            Ok::<_, ()>(format!("{}edit {k} {}", &t[..at], &t[at..]))
        });
    }
    assert!(s.flush(a).unwrap().written);
    let bytes = text(&s, a).len();
    let n = 20;
    let t0 = Instant::now();
    for _ in 0..n {
        assert_eq!(s.audit_disk_sync().unwrap(), vec![]);
    }
    let audit = t0.elapsed() / n;
    // a change that moves no byte, then the write-back that re-syncs the replica
    let mut resync = std::time::Duration::ZERO;
    let mut noop = std::time::Duration::ZERO;
    for _ in 0..n {
        replace(&s, a, "edit 999 ", "");
        s.undo(a, 1).unwrap();
        let t0 = Instant::now();
        assert!(!s.flush(a).unwrap().written);
        resync += t0.elapsed();
        let t0 = Instant::now();
        assert!(!s.flush(a).unwrap().written);
        noop += t0.elapsed();
    }
    println!(
        "audit_cost: {bytes} bytes, 1,001 changes: audit {audit:?} per call; \
         write-back with nothing to write: {:?} after a change that moved no byte \
         (re-sync), {:?} with nothing changed",
        resync / n,
        noop / n
    );
}

/// K-DISK-5/9/11: a person edits the file in a plain editor (rewriting it in place or by
/// rename, sometimes telling the kit, sometimes leaving it to the stat backup) while three
/// agents edit through the session, on the host's clock with write-backs in between. Every
/// agent's change and every editor change must survive, and the file must end equal to the
/// session.
#[test]
fn plain_editor_and_three_agents_lose_nothing() {
    let seeds: u64 = std::env::var("SIM_SEEDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8);
    if let Some(one) = std::env::var("SIM_SEED").ok().and_then(|v| v.parse().ok()) {
        return simulate(one, 120);
    }
    // seeds run in parallel: the run is bound by fsync (each change is a durable commit,
    // each write-back an fsynced file), not by CPU
    let threads: Vec<_> = (1..=seeds)
        .map(|seed| std::thread::spawn(move || simulate(seed, 120)))
        .collect();
    let failed: Vec<u64> = threads
        .into_iter()
        .zip(1..)
        .filter_map(|(t, seed)| t.join().is_err().then_some(seed))
        .collect();
    assert!(
        failed.is_empty(),
        "seeds failed: {failed:?} (rerun one with SIM_SEED=n)"
    );
}

fn simulate(seed: u64, ticks: usize) {
    let f = fx(&format!("sim{seed}"));
    let base: String = (0..12).map(|i| format!("base {i}\n")).collect();
    f.write("doc.md", base.as_bytes());
    let s = f.open("doc.md");
    let agents: Vec<ConnId> = (0..3).map(|k| f.join(&s, &format!("agent{k}"))).collect();
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let mut expect: Vec<String> = Vec::new();
    let mut base_edits: Vec<String> = Vec::new();
    for t in 0..ticks {
        // agents insert their own lines after random lines
        for (k, &c) in agents.iter().enumerate() {
            if rng.below(3) == 0 {
                continue;
            }
            let line = format!("agent{k}-{t}");
            let after = rng.below(1_000) as usize;
            s.edit_with(c, ROOT, |txt| {
                let mut lines: Vec<&str> = txt.lines().collect();
                let at = if lines.is_empty() {
                    0
                } else {
                    after % (lines.len() + 1)
                };
                lines.insert(at, &line);
                Ok::<_, ()>(lines.join("\n") + "\n")
            })
            .unwrap();
            expect.push(line);
        }
        // the plain editor: reads the file, adds a line or edits a base line, saves
        if rng.below(4) == 0 {
            let on_disk = String::from_utf8(f.bytes("doc.md")).unwrap();
            let mut lines: Vec<String> = on_disk.lines().map(str::to_owned).collect();
            let line = format!("editor-{t}");
            if rng.below(2) == 0 {
                let at = rng.below(lines.len() as u64 + 1) as usize;
                lines.insert(at, line.clone());
                expect.push(line);
            } else {
                // edit a base line in place (agents only insert around them)
                let i = rng.below(12);
                let target = format!("base {i}");
                if let Some(l) = lines.iter_mut().find(|l| **l == target) {
                    *l = format!("base {i} {line}");
                    base_edits.push(format!("base {i} {line}"));
                }
            }
            let new = lines.join("\n") + "\n";
            if rng.below(2) == 0 {
                rename_save(&f.path("doc.md"), new.as_bytes());
            } else {
                f.write("doc.md", new.as_bytes());
            }
            if rng.below(2) == 0 {
                s.file_changed().unwrap();
            }
        }
        f.advance(200 + rng.below(1_500));
        let r = f.tick();
        if std::env::var("SIM_DEBUG").is_ok() {
            let live = text(&s, agents[0]);
            for e in expect.iter().filter(|e| e.starts_with("agent")) {
                if !live.lines().any(|l| l == e) {
                    panic!("tick {t}: {e:?} lost after tick {r:?}\n{live}");
                }
            }
        }
    }
    let c = agents[0];
    let out = s.flush(c).unwrap();
    assert_eq!(out.disk, Some(DiskState::Clean), "seed {seed}");
    let live = text(&s, c);
    let file = String::from_utf8(f.bytes("doc.md")).unwrap();
    assert_eq!(file, live, "seed {seed}: file == session");
    assert_eq!(s.audit_disk_sync().unwrap(), vec![], "seed {seed}: K-AUD-1");
    for e in &expect {
        assert!(
            live.lines().any(|l| l == e),
            "seed {seed}: {e:?} lost\n{live}"
        );
    }
    for e in &base_edits {
        assert!(live.contains(e.as_str()), "seed {seed}: {e:?} lost");
    }
    for i in 0..12 {
        assert!(live.contains(&format!("base {i}")), "seed {seed}: base {i}");
    }
    assert_eq!(
        live.lines().count(),
        12 + expect.len(),
        "seed {seed}: nothing duplicated\n{live}"
    );
    assert!(f.temps().is_empty());
}

// ---------- racy clean (v0.9.5): an equal stat can't vouch for a file saved inside its mtime tick

/// An in-place save (same inode, same size) that leaves the file's mtime at `mtime`: a
/// second save inside one mtime tick, made deterministic.
fn save_in_place_at(path: &Path, bytes: &[u8], mtime: SystemTime) {
    fs::write(path, bytes).unwrap();
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(mtime)
        .unwrap();
}

/// An mtime the kit can't trust yet (in the future): every stat it takes is racy.
fn racy_mtime() -> SystemTime {
    SystemTime::now() + std::time::Duration::from_secs(3600)
}

/// (c) The poll on tick ingests a same-size save made inside the tick of the kit's last look.
#[test]
fn racy_the_poll_ingests_a_save_inside_the_tick() {
    let f = fx("racy-poll");
    let at = racy_mtime();
    let p = f.path("a.md");
    save_in_place_at(&p, b"# One\n\nbody\n", at);
    let s = f.open("a.md");
    let c = f.join(&s, "reader");
    assert_eq!(text(&s, c), "# One\n\nbody\n");
    save_in_place_at(&p, b"# Two\n\nbody\n", at);
    f.advance(3_000);
    let r = f.tick();
    assert_eq!(r.ingested.len(), 1, "ingested on the next tick: {r:?}");
    assert_eq!(text(&s, c), "# Two\n\nbody\n");
}

/// `check_disk` (the stat backup) too.
#[test]
fn racy_check_disk_ingests_a_save_inside_the_tick() {
    let f = fx("racy-check");
    let at = racy_mtime();
    let p = f.path("a.md");
    save_in_place_at(&p, b"alpha\n", at);
    let s = f.open("a.md");
    let c = f.join(&s, "reader");
    save_in_place_at(&p, b"omega\n", at);
    assert!(matches!(
        s.check_disk().unwrap(),
        DiskCheck::Ingested { .. }
    ));
    assert_eq!(text(&s, c), "omega\n");
}

/// The host's watcher (`file_changed`) always reads and hashes: no stat shortcut.
#[test]
fn racy_file_changed_always_hashes() {
    let f = fx("racy-fc");
    let at = racy_mtime();
    let p = f.path("a.md");
    save_in_place_at(&p, b"alpha\n", at);
    let s = f.open("a.md");
    let c = f.join(&s, "reader");
    save_in_place_at(&p, b"omega\n", at);
    assert!(matches!(
        s.file_changed().unwrap(),
        DiskCheck::Ingested { .. }
    ));
    assert_eq!(text(&s, c), "omega\n");
}

/// The soak's "outside delete-retype" duplicate (seed 1, call 176,147), trimmed. The file
/// ends `p22:2150`, `p10:2205`; unwritten live edits took out `p22:2150` and added two lines
/// after `p10:2205`. The plain editor deletes `p10:2205` and saves, then types it back and
/// saves again (the bytes as before). The line must not end up in the doc twice.
///
/// The text diff from the synced text to the live text pairs the lines up shifted
/// (`p22:2150` rewritten to `p10:2205`, `p10:2205` to the two new lines), so the save's
/// deletion of `p10:2205` meets a live "rewrite" of it, and keep-both keeps the live copy:
/// the doc doesn't change. The replica, though, takes the file's text (without the line), so
/// the retype reads as a new line and lands next to the one the doc kept.
#[test]
fn an_outside_delete_and_retype_over_unwritten_edits_never_duplicates_a_line() {
    let synced = "p05:2216\np22:2150\np10:2205\n";
    let live = "p05:2216\np10:2205\np23:2229\np25:2184\n";
    let deleted = "p05:2216\np22:2150\n";
    let f = fx("delete-retype");
    f.write("a.md", synced.as_bytes());
    let s = f.open("a.md");
    let a = f.join(&s, "agent");
    set(&s, a, live);
    f.advance(100);
    // the editor deletes the line and saves
    f.write("a.md", deleted.as_bytes());
    s.file_changed().unwrap();
    assert_eq!(
        s.audit_disk_sync().unwrap(),
        vec![],
        "K-AUD-1 after the delete"
    );
    let after_delete = text(&s, a);
    // ... then types it back where it was and saves: the bytes as before the delete
    f.advance(100);
    f.write("a.md", synced.as_bytes());
    s.file_changed().unwrap();
    assert_eq!(
        s.audit_disk_sync().unwrap(),
        vec![],
        "K-AUD-1 after the retype"
    );
    let t = text(&s, a);
    let mut seen = std::collections::HashSet::new();
    for l in t.lines() {
        assert!(
            seen.insert(l),
            "{l:?} twice after the retype: {t:?} (after the delete: {after_delete:?})"
        );
    }
}

/// The merge under [`an_outside_delete_and_retype_over_unwritten_edits_never_duplicates_a_line`]:
/// the live side never touched `p10:2205` (it took out `p22:2150` and added two lines after
/// it), so the save's deletion of that line lands. The live side's diff must not read as a
/// rewrite of it.
#[test]
fn an_outside_delete_of_a_line_the_live_side_kept_lands() {
    let synced = "p05:2216\np22:2150\np10:2205\n";
    let live = "p05:2216\np10:2205\np23:2229\np25:2184\n";
    let deleted = "p05:2216\np22:2150\n";
    let line = synced.find("p10:2205").unwrap();
    let (_, theirs) = outside_sides(synced, deleted, live);
    assert!(
        theirs
            .iter()
            .all(|h| h.end <= line || h.start >= synced.len()),
        "the live side's diff rewrites the line it kept: {theirs:?}"
    );
    assert_eq!(
        k2_mpc_base::text::merge::merge3_keep_both(synced, deleted, live).text,
        "p05:2216\np23:2229\np25:2184\n"
    );
}

/// v0.9.8: a reopened doc keeps its disk replica encoded until the disk path needs it. An
/// outside save right after the reopen still merges on the synced text (here with an agent's
/// unwritten edit), the next write-back writes the merge, and the sync audit is clean; a save
/// made while the doc was closed is reconciled at the open as before.
#[cfg(unix)]
#[test]
fn an_outside_edit_right_after_a_reopen_merges_on_the_synced_text() {
    let mut f = fx("lazy-replica");
    let seed: String = (1..=8).map(|i| format!("line {i}\n")).collect();
    f.write("a.md", seed.as_bytes());
    {
        let s = f.open("a.md");
        let c = f.join(&s, "agent");
        replace(&s, c, "line 1", "line one");
        assert!(s.flush(c).unwrap().written);
    }
    f.mgr.release().unwrap();
    f.restart();
    // right after the reopen: an outside save, and an agent's edit before it is seen
    let s = f.open("a.md");
    let c = f.join(&s, "agent");
    let synced = String::from_utf8(f.bytes("a.md")).unwrap();
    f.write(
        "a.md",
        synced.replace("line 7", "line 7 (person)").as_bytes(),
    );
    replace(&s, c, "line 2", "line 2 (agent)");
    assert!(matches!(
        s.file_changed().unwrap(),
        DiskCheck::Ingested { .. }
    ));
    let t = text(&s, c);
    assert_eq!(
        t,
        seed.replace("line 1", "line one")
            .replace("line 2", "line 2 (agent)")
            .replace("line 7", "line 7 (person)")
    );
    assert!(s.flush(c).unwrap().written);
    assert_eq!(f.bytes("a.md"), t.as_bytes());
    assert!(s.audit_disk_sync().unwrap().is_empty());
    // saved while closed: reconciled at the open
    drop(s);
    f.mgr.release().unwrap();
    f.restart();
    f.write("a.md", t.replace("line 8", "line eight").as_bytes());
    let s = f.open("a.md");
    let c = f.join(&s, "agent");
    assert_eq!(text(&s, c), t.replace("line 8", "line eight"));
    s.flush(c).unwrap();
    assert!(s.audit_disk_sync().unwrap().is_empty());
}
