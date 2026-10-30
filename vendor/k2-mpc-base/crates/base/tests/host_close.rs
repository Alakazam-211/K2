//! The host closes a doc's live session, members or not (`SessionManager::close`,
//! `close_where`; v0.9.7.2): a host switching a file type off, for instance. Nothing acknowledged
//! is lost, every later call gets `SessionError::Closed`, connected clients get
//! `RosterEvent::Closed` last, the audit names the connections, waiting outside-save notes are
//! kept for the next open, and a save made while closed is taken in by that open.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::TryRecvError;
use std::sync::{Arc, Mutex};

use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::session::{
    AnyFile, AuditEvent, Config, ConnId, Host, NoteKind, Participant, ParticipantKind, Rights,
    RosterEvent, Session, SessionError, SessionManager, Stay, TextFilePack,
};
use k2_mpc_base::store::{DocKey, DocStore, BLOB_DOC_PACK};

const OFF: &str = "core_off";

struct H {
    now: AtomicU64,
    events: Mutex<Vec<AuditEvent>>,
}

impl Host for H {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn on_event(&self, ev: &AuditEvent) {
        self.events.lock().unwrap().push(ev.clone());
    }
}

struct Fx {
    dir: PathBuf,
    host: Arc<H>,
    mgr: SessionManager,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = self.mgr.release();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

static NEXT: AtomicU64 = AtomicU64::new(0);

fn fx() -> Fx {
    let n = NEXT.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("k2mb-close-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("ws")).unwrap();
    let host = Arc::new(H {
        now: AtomicU64::new(1_000_000),
        events: Mutex::new(Vec::new()),
    });
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
    let mgr = SessionManager::new(store, host.clone(), Config::default()).with_blob_store(blobs);
    Fx { dir, host, mgr }
}

fn key(path: &str) -> DocKey {
    DocKey {
        space: "ws".into(),
        path: path.into(),
    }
}

impl Fx {
    fn file(&self, name: &str) -> PathBuf {
        self.dir.join("ws").join(name)
    }
    fn open(&self, name: &str, pack: &'static str) -> Arc<Session> {
        let f = self.file(name);
        if !f.exists() {
            std::fs::write(&f, "intro\nend\n").unwrap();
        }
        self.mgr
            .open_file(
                Arc::new(TextFilePack::new(pack, 4 << 20)),
                &key(name),
                &f,
                &self.dir.join("ws"),
            )
            .unwrap()
    }
    fn advance(&self, ms: u64) {
        self.host.now.fetch_add(ms, Ordering::SeqCst);
    }
    fn closed_events(&self) -> Vec<AuditEvent> {
        self.host
            .events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| matches!(e, AuditEvent::SessionClosed { .. }))
            .cloned()
            .collect()
    }
}

fn join(s: &Session, pid: &str) -> ConnId {
    s.join(
        Participant::new(pid, pid, "#1", ParticipantKind::Agent, "cli"),
        Stay::Held,
    )
    .unwrap()
    .conn
}

fn insert(s: &Session, c: ConnId, at: usize, text: &str) {
    let argv: Vec<String> = vec!["insert".into(), "--line".into(), at.to_string()];
    s.run_tool(c, TextFilePack::ROOT, &argv, text.as_bytes())
        .unwrap();
}

fn text(s: &Session, c: ConnId) -> String {
    s.read(c).unwrap().text_by_root[0].1.clone()
}

fn is_closed<T: std::fmt::Debug>(r: Result<T, SessionError>) -> bool {
    matches!(r, Err(SessionError::Closed { reason: OFF }))
}

/// An acknowledged edit the file doesn't have yet (inside the write delay) is written before the
/// session closes.
#[test]
fn a_pending_write_back_is_flushed_then_the_file_holds_it() {
    let f = fx();
    let s = f.open("a.md", "markdown");
    let c = join(&s, "agent");
    insert(&s, c, 2, "written just before the close\n");
    assert!(!std::fs::read_to_string(f.file("a.md"))
        .unwrap()
        .contains("written just before"));
    assert!(f.mgr.close(&key("a.md"), OFF).unwrap());
    assert_eq!(
        std::fs::read_to_string(f.file("a.md")).unwrap(),
        "intro\nwritten just before the close\nend\n"
    );
}

/// Every call on the closed session fails with `Closed`, `file_changed` included.
#[test]
fn every_op_afterwards_gets_closed() {
    let f = fx();
    let s = f.open("a.md", "markdown");
    let c = join(&s, "agent");
    assert!(f.mgr.close(&key("a.md"), OFF).unwrap());
    assert!(is_closed(s.read(c)));
    assert!(is_closed(
        s.run_tool(c, TextFilePack::ROOT, &["cat".to_owned()], b"")
            .map_err(|e| match e {
                k2_mpc_base::tools::ToolRunError::Session(e) => e,
                other => panic!("{other:?}"),
            })
    ));
    assert!(is_closed(s.file_changed()));
    assert!(is_closed(s.check_disk()));
    assert!(is_closed(s.flush(c)));
    assert!(is_closed(s.take_notes(c)));
    assert!(is_closed(s.subscribe_roster(c)));
    assert!(is_closed(s.join(
        Participant::new("late", "late", "#2", ParticipantKind::Person, "app"),
        Stay::Held
    )));
    let e = s.read(c).unwrap_err();
    assert_eq!(
        e.to_string(),
        "the host closed this doc's live session (core_off)"
    );
    assert_eq!(k2_mpc_base::errors::Refusal::report(&e).id(), "MPC-E519");
    // a second close of the same key finds nothing loaded
    assert!(!f.mgr.close(&key("a.md"), OFF).unwrap());
}

/// Members' connections are told: every roster subscription's last event is `Closed`, then it
/// ends; the audit names the connections.
#[test]
fn members_get_the_closing_event_and_the_audit_names_them() {
    let f = fx();
    let s = f.open("a.md", "markdown");
    let a = join(&s, "agent");
    let b = s
        .join(
            Participant::new("sam", "Sam", "#2", ParticipantKind::Person, "app"),
            Stay::Held,
        )
        .unwrap()
        .conn;
    let ra = s.subscribe_roster(a).unwrap();
    let rb = s.subscribe_roster(b).unwrap();
    assert!(f.mgr.close(&key("a.md"), OFF).unwrap());
    for rx in [&ra, &rb] {
        let all: Vec<RosterEvent> = rx.try_iter().collect();
        assert!(
            matches!(all.last(), Some(RosterEvent::Closed { reason: OFF })),
            "{all:?}"
        );
        assert_eq!(rx.try_recv(), Err(TryRecvError::Disconnected));
    }
    let ev = f.closed_events();
    assert_eq!(ev.len(), 1);
    match &ev[0] {
        AuditEvent::SessionClosed {
            key: k,
            connections,
            reason,
        } => {
            assert_eq!(k, &key("a.md"));
            assert_eq!(*reason, OFF);
            let mut got: Vec<(ConnId, String)> = connections.clone();
            got.sort();
            let mut want = vec![(a, "agent".to_owned()), (b, "sam".to_owned())];
            want.sort();
            assert_eq!(got, want);
        }
        e => panic!("{e:?}"),
    }
}

/// The host opens it again when it allows: everything is there.
#[test]
fn a_reopen_later_works() {
    let f = fx();
    let s = f.open("a.md", "markdown");
    let c = join(&s, "agent");
    insert(&s, c, 2, "kept\n");
    assert!(f.mgr.close(&key("a.md"), OFF).unwrap());
    let s2 = f.open("a.md", "markdown");
    assert!(!Arc::ptr_eq(&s, &s2));
    let c2 = join(&s2, "agent");
    assert_eq!(text(&s2, c2), "intro\nkept\nend\n");
    insert(&s2, c2, 1, "after the reopen\n");
    assert!(text(&s2, c2).starts_with("after the reopen\n"));
}

/// A note waiting when the session closes is delivered by the next open.
#[test]
fn waiting_notes_survive_and_deliver_on_reopen() {
    let f = fx();
    let s = f.open("a.md", "markdown");
    let c = join(&s, "agent");
    insert(&s, c, 2, "the agent's line\n");
    s.flush(c).unwrap();
    f.advance(10_000);
    std::fs::write(f.file("a.md"), "intro\nend\nan editor's line\n").unwrap();
    s.file_changed().unwrap();
    assert!(f.mgr.close(&key("a.md"), OFF).unwrap());
    let s2 = f.open("a.md", "markdown");
    let c2 = join(&s2, "agent");
    let notes = s2.take_notes(c2).unwrap();
    let got: Vec<_> = notes
        .notes
        .iter()
        .filter(|n| n.kind == NoteKind::Overwritten)
        .filter_map(|n| n.overwrite.clone())
        .collect();
    assert_eq!(got.len(), 1, "{notes:?}");
    assert_eq!(got[0].yours, "the agent's line\n");
}

/// A save made while the session is closed isn't merged then; the next open takes it in as an
/// ordinary outside save, and nothing acknowledged before the close is lost.
#[test]
fn an_outside_save_after_the_close_is_taken_in_by_the_next_open() {
    let f = fx();
    let s = f.open("a.md", "markdown");
    let c = join(&s, "agent");
    insert(&s, c, 2, "acknowledged before the close\n");
    assert!(f.mgr.close(&key("a.md"), OFF).unwrap());
    // the file was written at the close; an editor adds a line while it's closed
    let on_disk = std::fs::read_to_string(f.file("a.md")).unwrap();
    assert!(on_disk.contains("acknowledged before the close"));
    f.advance(10_000);
    std::fs::write(f.file("a.md"), format!("{on_disk}saved while closed\n")).unwrap();
    // nothing merges it: the old handle refuses, the manager's tick doesn't reach it
    assert!(is_closed(s.file_changed()));
    let t = f.mgr.tick().unwrap();
    assert!(t.ingested.is_empty(), "{:?}", t.ingested);
    // the host opens it again: an ordinary outside save, by kit:external, with nothing lost
    let s2 = f.open("a.md", "markdown");
    let c2 = join(&s2, "agent");
    assert_eq!(
        text(&s2, c2),
        "intro\nacknowledged before the close\nend\nsaved while closed\n"
    );
    let b = s2.blame(c2, TextFilePack::ROOT, None).unwrap();
    let last = b.lines.last().unwrap();
    assert_eq!(last.text, "saved while closed");
    assert_eq!(last.author, "kit:external");
    assert_eq!(
        b.lines[1].author, "agent",
        "the agent's line stays the agent's"
    );
}

/// `close_where` closes the docs it picks, by key and pack, text and tier 0, and no others.
#[test]
fn close_where_closes_what_it_picks_only() {
    let f = fx();
    let md1 = f.open("one.md", "markdown");
    let md2 = f.open("two.md", "markdown");
    let txt = f.open("notes.txt", "text");
    std::fs::write(f.file("pic.bin"), [0u8, 1, 2, 3]).unwrap();
    let blob = f
        .mgr
        .open_blob_file(
            Arc::new(AnyFile),
            &key("pic.bin"),
            &f.file("pic.bin"),
            &f.dir.join("ws"),
        )
        .unwrap();
    let cb = blob
        .join(
            Participant::new("p", "p", "#3", ParticipantKind::Person, "app"),
            Stay::Held,
        )
        .unwrap()
        .conn;
    let closed = f
        .mgr
        .close_where(OFF, |_, pack| pack == "markdown" || pack == BLOB_DOC_PACK)
        .unwrap();
    assert_eq!(closed, vec![key("one.md"), key("pic.bin"), key("two.md")]);
    let c = join(&txt, "agent");
    assert!(txt.read(c).is_ok(), "text docs stay open");
    for s in [&md1, &md2] {
        assert!(is_closed(s.status()));
    }
    assert!(matches!(
        blob.read(cb),
        Err(SessionError::Closed { reason: OFF })
    ));
    assert!(f.closed_events().iter().any(
        |e| matches!(e, AuditEvent::SessionClosed { key: k, connections, .. }
            if *k == key("pic.bin") && connections.len() == 1)
    ));
}
