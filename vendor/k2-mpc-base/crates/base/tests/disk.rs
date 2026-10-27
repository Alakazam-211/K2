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
    ParticipantKind, Rights, Session, SessionError, SessionManager, Stay, TextFilePack,
    EXTERNAL_PID,
};
use k2_mpc_base::store::{DocKey, DocStore, VersionReason};

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
