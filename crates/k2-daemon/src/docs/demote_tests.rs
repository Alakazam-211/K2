//! A doc stored as live text under a name that now routes to tier 0 (a
//! `.kicad_sch`, which text v0.2.22 moved to not-text) opens at tier 0
//! through K2's own tier-0 open ([`super::open_blob_or_demote`], what
//! `Runtime::open_blob` calls): the kit's demote, once, under the same key
//! and doc id. A temp store, no daemon runtime (the HTTP path is
//! `tests/docs_demote_integration.rs`).

use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::session::{
    AuditEvent, BlobSession, Config, ConnId, Host, LogItem, Participant, ParticipantKind, Rights, Session, SessionError,
    SessionManager, Stay, WriteOutcome, DEMOTED,
};
use k2_mpc_base::store::{DocKey, DocStore, VersionId, BLOB_DOC_PACK};
use k2_mpc_text::{PACK, ROOT};

struct TestHost {
    now: AtomicU64,
    events: Mutex<Vec<AuditEvent>>,
}

impl Host for TestHost {
    fn rights(&self, _who: &Participant, _key: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn on_event(&self, ev: &AuditEvent) {
        self.events.lock().expect("events").push(ev.clone());
    }
}

struct Fx {
    dir: PathBuf,
    ws: PathBuf,
    host: Arc<TestHost>,
    store: Arc<DocStore>,
    mgr: SessionManager,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = self.mgr.release();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn fx(tag: &str) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!("k2-demote-{tag}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&dir);
    let ws = dir.join("ws");
    std::fs::create_dir_all(&ws).expect("temp workspace");
    let ws = std::fs::canonicalize(ws).expect("canonical workspace");
    let host = Arc::new(TestHost { now: AtomicU64::new(1_000_000), events: Mutex::new(Vec::new()) });
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).expect("store"));
    let h = host.clone();
    let blobs = Arc::new(
        BlobStore::open(store.clone(), dir.join("blobs"), BlobConfig::default(), Arc::new(move || h.now_ms())).expect("blobs"),
    );
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    let mgr = SessionManager::new(store.clone(), host.clone(), c).with_blob_store(blobs);
    Fx { dir, ws, host, store, mgr }
}

fn key(name: &str) -> DocKey {
    DocKey { space: "ws".into(), path: name.into() }
}

fn who(pid: &str) -> Participant {
    Participant::new(pid, pid, "#123456", ParticipantKind::Agent, "cli")
}

impl Fx {
    fn path(&self, name: &str) -> PathBuf {
        self.ws.join(name)
    }
    fn bytes(&self, name: &str) -> Vec<u8> {
        std::fs::read(self.path(name)).expect("the file")
    }
    fn doc_id(&self, name: &str) -> k2_mpc_base::store::DocId {
        self.store.find(&key(name)).expect("find").expect("the doc is stored")
    }
    fn pack_of(&self, name: &str) -> String {
        self.store.meta(self.doc_id(name)).expect("meta").pack
    }
    /// The doc as it was before text v0.2.22: opened as live text of unknown
    /// type (the text pack's `PACK`).
    fn open_as_text(&self, name: &str) -> Arc<Session> {
        self.mgr.open_file(Arc::new(PACK), &key(name), &self.path(name), &self.ws).expect("a text open")
    }
}

/// An agent's whole-file write from the version it read, which must apply.
fn write(s: &Session, c: ConnId, to: &str) {
    let v = s.read(c).expect("read").version;
    let out = s.write_base(c, ROOT, v, to).expect("write");
    assert!(matches!(out, WriteOutcome::Applied(_)), "{out:?}");
}

fn blob_at(b: &BlobSession, c: ConnId, v: VersionId) -> Vec<u8> {
    let mut out = Vec::new();
    b.read_version(c, v).expect("read_version").reader.read_to_end(&mut out).expect("the version's bytes");
    out
}

fn blob_current(b: &BlobSession, c: ConnId) -> Vec<u8> {
    let mut out = Vec::new();
    b.read(c).expect("read").reader.read_to_end(&mut out).expect("the current bytes");
    out
}

/// Every text version and the bytes its `file_at` gives, oldest first.
fn text_files(s: &Session, c: ConnId) -> Vec<(VersionId, Vec<u8>)> {
    s.versions(c).expect("versions").into_iter().map(|v| (v.id, s.file_at(c, v.id).expect("file_at").bytes)).collect()
}

const SCH: &str = "(kicad_sch (version 20231120) (generator eeschema)\n\
  (uuid \"5b0f6c1e-0d6a-4c39-9d43-1f0b3c6f5a10\")\n\
  (paper \"A4\")\n\
)\n";

/// `SCH` with these symbol lines before its closing paren.
fn sch_with(lines: &[&str]) -> String {
    let body = &SCH[..SCH.len() - 2];
    let mut out = body.to_string();
    for l in lines {
        out.push_str("  ");
        out.push_str(l);
        out.push('\n');
    }
    out.push_str(")\n");
    out
}

const R1: &str = "(symbol (lib_id \"Device:R\") (at 50 50 0) (uuid \"a1c2e3f4-0000-4000-8000-000000000001\"))";
const C1: &str = "(symbol (lib_id \"Device:C\") (at 80 50 0) (uuid \"a1c2e3f4-0000-4000-8000-000000000002\"))";
const D1: &str = "(symbol (lib_id \"Device:D\") (at 80 80 0) (uuid \"a1c2e3f4-0000-4000-8000-000000000003\"))";

#[test]
fn a_kicad_schematic_live_as_text_opens_at_tier_0_with_its_history() {
    let f = fx("kicad");
    let name = "board/main.kicad_sch";
    std::fs::create_dir_all(f.path("board")).expect("folder");
    std::fs::write(f.path(name), SCH).expect("the schematic");
    assert!(
        matches!(super::target::Tier::of(name), super::target::Tier::Blob),
        "a .kicad_sch routes to tier 0 today (text v0.2.22)"
    );

    // Stored as live text, as before: two named versions and a change the
    // file never got (the session still holds it, unwritten).
    let s = f.open_as_text(name);
    let a = s.join(who("agent-a"), Stay::Held).expect("join").conn;
    write(&s, a, &sch_with(&[R1]));
    s.flush(a).expect("flush");
    s.name_version(a, "R1 placed", None).expect("a named version");
    write(&s, a, &sch_with(&[R1, C1]));
    s.flush(a).expect("flush");
    s.name_version(a, "C1 placed", None).expect("a named version");
    let b = s.join(who("agent-b"), Stay::Held).expect("join").conn;
    let live = sch_with(&[R1, C1, D1]);
    write(&s, b, &live);
    assert_eq!(f.bytes(name), sch_with(&[R1, C1]).as_bytes(), "agent b's diode isn't written yet");
    let before = text_files(&s, a);
    assert!(before.len() >= 3, "the import and two named versions: {before:?}");
    let id = f.doc_id(name);
    assert_eq!(f.pack_of(name), "text");

    // K2's tier-0 open: no PackMismatch, a demotion.
    let (blob, d) = super::open_blob_or_demote(&f.mgr, &key(name), &f.path(name), &f.ws).expect("it opens at tier 0");
    let d = d.expect("this open demoted the text doc");
    assert!(!d.already, "{d:?}");
    assert_eq!(d.key, key(name));
    assert_eq!(f.doc_id(name), id, "the same doc id");
    assert_eq!(f.pack_of(name), BLOB_DOC_PACK, "a tier-0 doc now");
    let mut conns: Vec<ConnId> = d.connections.iter().map(|(c, _)| *c).collect();
    conns.sort();
    let mut joined = vec![a, b];
    joined.sort();
    assert_eq!(conns, joined, "the text session's connections, for their tabs");

    // Every earlier version, with the bytes its file_at gave.
    let r = blob.join(who("reader"), Stay::Held).expect("join the tier-0 doc").conn;
    assert_eq!(d.versions.len(), before.len(), "{d:?}");
    for ((tv, bytes), (from, to)) in before.iter().zip(&d.versions) {
        assert_eq!(tv, from, "{d:?}");
        assert_eq!(&blob_at(&blob, r, *to), bytes, "text version {tv} as tier-0 version {to}");
    }
    // The live state is current and on disk.
    assert!(!d.kept_both, "{d:?}");
    assert!(d.written, "the diode was written: {d:?}");
    assert_eq!(blob_current(&blob, r), live.as_bytes());
    assert_eq!(f.bytes(name), live.as_bytes(), "the file is the live state");
    // The change log is readable, both agents' changes in it.
    let authors: Vec<String> = blob
        .live_log(r)
        .expect("live_log")
        .into_iter()
        .filter_map(|i| match i {
            LogItem::Change { info, .. } => Some(info.author),
            _ => None,
        })
        .collect();
    assert_eq!(authors.len() as u64, d.changes, "{authors:?}");
    for pid in ["agent-a", "agent-b"] {
        assert!(authors.iter().any(|x| x == pid), "{pid} in the live log: {authors:?}");
    }
    // The text session is closed as demoted.
    match s.read(a) {
        Err(SessionError::Closed { reason }) => assert_eq!(reason, DEMOTED),
        other => panic!("the old text handle still answers: {:?}", other.map(|r| r.version)),
    }
    let demoted_events = f.host.events.lock().expect("events").iter().filter(|e| matches!(e, AuditEvent::Demoted { .. })).count();
    assert_eq!(demoted_events, 1);

    // A second open never demotes again: the same tier-0 doc, nothing moved.
    let (again, d2) = super::open_blob_or_demote(&f.mgr, &key(name), &f.path(name), &f.ws).expect("the second open");
    assert!(d2.is_none(), "a second open demoted again: {d2:?}");
    assert!(Arc::ptr_eq(&again, &blob), "the same tier-0 doc");
    drop(again);
    drop(blob);
    assert!(f.mgr.close(&key(name), "test").expect("close"), "the tier-0 doc was loaded");
    let (reloaded, d3) = super::open_blob_or_demote(&f.mgr, &key(name), &f.path(name), &f.ws).expect("a cold open");
    assert!(d3.is_none(), "a cold open demoted again: {d3:?}");
    let r = reloaded.join(who("reader"), Stay::Held).expect("join").conn;
    assert_eq!(blob_current(&reloaded, r), live.as_bytes());
    assert_eq!(f.doc_id(name), id);
    let demoted_events = f.host.events.lock().expect("events").iter().filter(|e| matches!(e, AuditEvent::Demoted { .. })).count();
    assert_eq!(demoted_events, 1, "one demotion in all");
}

/// Another program saved the file after the text doc's last sync: the save
/// is current and on disk, the live state a version and a conflict copy;
/// no version holds the two merged by lines.
#[test]
fn an_outside_save_since_the_last_sync_keeps_both() {
    let f = fx("kicad-save");
    let name = "main.kicad_sch";
    std::fs::write(f.path(name), SCH).expect("the schematic");
    let s = f.open_as_text(name);
    let a = s.join(who("agent-a"), Stay::Held).expect("join").conn;
    write(&s, a, &sch_with(&[R1]));
    s.flush(a).expect("flush");
    let live = sch_with(&[R1, C1]);
    write(&s, a, &live);
    // The schematic editor saves (write a temp file, rename it over).
    f.host.now.fetch_add(2_000, Ordering::SeqCst);
    let saved = sch_with(&[R1, D1]);
    let tmp = f.path(".editor-save.tmp");
    std::fs::write(&tmp, &saved).expect("the save");
    std::fs::rename(&tmp, f.path(name)).expect("rename over");
    let id = f.doc_id(name);

    let (blob, d) = super::open_blob_or_demote(&f.mgr, &key(name), &f.path(name), &f.ws).expect("it opens at tier 0");
    let d = d.expect("this open demoted the text doc");
    assert_eq!(f.doc_id(name), id, "the same doc id");
    assert!(d.kept_both && d.current != d.live, "{d:?}");
    assert!(!d.written, "the file is the other program's save: {d:?}");
    assert_eq!(f.bytes(name), saved.as_bytes(), "the save stays on disk");
    let r = blob.join(who("reader"), Stay::Held).expect("join").conn;
    assert_eq!(blob_current(&blob, r), saved.as_bytes(), "the save is current");
    assert_eq!(blob_at(&blob, r, d.live), live.as_bytes(), "the live state is a version");
    let copy = d.conflict_copy.clone().expect("a conflict copy");
    assert_eq!(std::fs::read(&copy).expect("the conflict copy"), live.as_bytes(), "the copy holds the live state");
    for (v, _) in blob.versions(r).expect("versions") {
        let t = String::from_utf8(blob_at(&blob, r, v)).expect("text");
        assert!(!(t.contains("Device:C") && t.contains("Device:D")), "version {v} merges the save by lines:\n{t}");
    }
    let conflicts = f.host.events.lock().expect("events").iter().filter(|e| matches!(e, AuditEvent::BlobConflict { .. })).count();
    assert_eq!(conflicts, 1, "one conflict notice");
}

/// A name that still routes to live text never demotes: a text doc served at
/// tier 0 only because its pack refuses the file now (it turned binary, say)
/// keeps its live history, and the tier-0 open stays the kit's
/// `PackMismatch`.
#[test]
fn a_text_name_is_never_demoted() {
    let f = fx("txt");
    let name = "notes.txt";
    std::fs::write(f.path(name), "one\n").expect("notes");
    let s = f.open_as_text(name);
    let a = s.join(who("agent-a"), Stay::Held).expect("join").conn;
    write(&s, a, "one\ntwo\n");
    s.flush(a).expect("flush");
    match super::open_blob_or_demote(&f.mgr, &key(name), &f.path(name), &f.ws) {
        Err(SessionError::PackMismatch { stored, pack }) => {
            assert_eq!((stored.as_str(), pack.as_str()), ("text", BLOB_DOC_PACK))
        }
        other => panic!("a tier-0 open of a text name: {:?}", other.map(|(_, d)| d)),
    }
    assert_eq!(f.pack_of(name), "text", "still a text doc");
    s.read(a).expect("the text session still answers");
}

/// Drill, timeline and FCPXML files were live text before their cores (the
/// text pack takes an unknown name as text when its bytes pass): a doc
/// stored that way opens at tier 0 through the same demote, once.
#[test]
fn drill_timeline_and_fcpxml_files_live_as_text_open_at_tier_0() {
    for (name, body) in [
        ("board.drl", "M48\nMETRIC\nT01C0.300\n%\nT01\nX1.0Y1.0\nM30\n"),
        ("cut.otio", "{\"OTIO_SCHEMA\": \"Timeline.1\", \"name\": \"cut\"}\n"),
        ("edit.fcpxml", "<?xml version=\"1.0\"?>\n<fcpxml version=\"1.10\"/>\n"),
    ] {
        let f = fx("packfile");
        std::fs::write(f.path(name), body).expect("the file");
        assert!(
            !matches!(super::target::Tier::of(name), super::target::Tier::Markdown | super::target::Tier::Text(_)),
            "{name} is never live text now"
        );
        let s = f.open_as_text(name);
        assert_eq!(f.pack_of(name), "text", "{name}");
        let (blob, d) = super::open_blob_or_demote(&f.mgr, &key(name), &f.path(name), &f.ws).expect("it opens at tier 0");
        assert!(d.is_some_and(|d| !d.already), "{name}: this open demoted it");
        assert_eq!(f.pack_of(name), BLOB_DOC_PACK, "{name}: a tier-0 doc now");
        let r = blob.join(who("reader"), Stay::Held).expect("join").conn;
        assert_eq!(blob_current(&blob, r), body.as_bytes(), "{name}: the same bytes");
        drop(s);
    }
}
