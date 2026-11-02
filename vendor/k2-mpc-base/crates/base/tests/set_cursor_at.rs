//! `Session::set_cursor_at` (kit v0.10.1): a caret given in byte offsets of a root's text at a
//! version (a live client's `cursor` frame) is carried through the changes since on that root
//! (`position::transform`, `Bias::After`) and set, or dropped (`Ahead`, `PastWindow`) with
//! nothing set. Public API only.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use k2_mpc_base::session::position::{self, Bias};
use k2_mpc_base::session::{
    At, AuditEvent, Config, ConnId, CursorAtVersion, CursorDrop, EditOptions, Host, Participant,
    ParticipantKind, Rights, Session, SessionManager, Stay, TextFilePack,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::text::diff::Hunk;

const ROOT: &str = TextFilePack::ROOT;

// ---------- harness ----------

struct TestHost {
    now: AtomicU64,
    rights: Mutex<HashMap<String, Rights>>,
}

impl Host for TestHost {
    fn rights(&self, who: &Participant, _key: &DocKey) -> Rights {
        self.rights
            .lock()
            .unwrap()
            .get(&who.pid)
            .copied()
            .unwrap_or(Rights::full())
    }
    fn now_ms(&self) -> u64 {
        self.now.fetch_add(1, Ordering::SeqCst)
    }
    fn on_event(&self, _: &AuditEvent) {}
}

struct Fx {
    dir: std::path::PathBuf,
    host: Arc<TestHost>,
    mgr: SessionManager,
    s: Arc<Session>,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn config() -> Config {
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    c
}

fn key() -> DocKey {
    DocKey {
        space: "ws".into(),
        path: "d.md".into(),
    }
}

fn open(
    store: Arc<DocStore>,
    host: Arc<TestHost>,
    c: Config,
    initial: Option<&[u8]>,
) -> (SessionManager, Arc<Session>) {
    let mgr = SessionManager::new(store, host, c);
    let s = mgr
        .open(
            Arc::new(TextFilePack::new("markdown", 4 << 20)),
            &key(),
            initial,
        )
        .unwrap();
    (mgr, s)
}

fn fx_with(tag: &str, text: &str, c: Config) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-cursor-at-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let host = Arc::new(TestHost {
        now: AtomicU64::new(1_000_000),
        rights: Mutex::new(HashMap::new()),
    });
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let (mgr, s) = open(store, host.clone(), c, Some(text.as_bytes()));
    Fx { dir, host, mgr, s }
}

fn fx(tag: &str, text: &str) -> Fx {
    fx_with(tag, text, config())
}

fn join(s: &Session, pid: &str, kind: ParticipantKind) -> ConnId {
    s.join(
        Participant::new(pid, pid.to_uppercase(), "#123456", kind, "tab"),
        Stay::Held,
    )
    .unwrap()
    .conn
}

fn version(s: &Session, c: ConnId) -> u64 {
    s.read(c).unwrap().version
}

fn text(s: &Session, c: ConnId) -> String {
    s.read(c).unwrap().text_by_root[0].1.clone()
}

/// `c` inserts `insert` at byte `at` of the text now.
fn insert(s: &Session, c: ConnId, at: usize, insert: &str) {
    let v = version(s, c);
    s.edit_at_base(
        c,
        ROOT,
        v,
        vec![Hunk {
            start: at,
            end: at,
            insert: insert.into(),
        }],
        &EditOptions::new(),
    )
    .unwrap();
}

/// `pid`'s caret in the roster, as `(anchor, head)` bytes.
fn caret(s: &Session, viewer: ConnId, pid: &str) -> Option<(usize, usize)> {
    s.roster(viewer)
        .unwrap()
        .into_iter()
        .find(|e| e.participant.pid == pid)
        .and_then(|e| e.cursor)
        .map(|c| (c.cursor.anchor, c.cursor.head))
}

fn set(c: CursorAtVersion) -> (String, usize, usize) {
    match c {
        CursorAtVersion::Set(c) => (c.root, c.anchor, c.head),
        other => panic!("expected a caret set, got {other:?}"),
    }
}

// ---------- the calls ----------

/// Another participant's edits before the caret, at its anchor and after it: the caret moves
/// as the kit carries one (`Bias::After`), the same as a host mapping it by hand through
/// `events_since`, and lands on the same text.
#[test]
fn maps_through_edits_before_at_and_after_the_caret() {
    let f = fx("map", "hello world\n");
    let ann = join(&f.s, "ann", ParticipantKind::Person);
    let bob = join(&f.s, "bob", ParticipantKind::Person);
    let v0 = version(&f.s, ann);
    // ann's frame at v0: "wo" selected
    let (a0, h0) = (6, 8);
    assert_eq!(&text(&f.s, ann)[a0..h0], "wo");
    insert(&f.s, bob, 0, "AB"); // before: "ABhello world\n"
    insert(&f.s, bob, 8, "X"); // at the anchor: "ABhello Xworld\n"
    insert(&f.s, bob, 14, "!"); // after: "ABhello Xworld!\n"
    let now = text(&f.s, ann);
    assert_eq!(now, "ABhello Xworld!\n");

    // what WIRE.md told a host to do by hand
    let (mut a, mut h) = (a0, h0);
    for e in f.s.events_since(ann, v0).unwrap() {
        if e.root == ROOT {
            a = position::transform(a, &e.hunks, Bias::After);
            h = position::transform(h, &e.hunks, Bias::After);
        }
    }

    let got = set(f.s.set_cursor_at(ann, ROOT, v0, a0, h0).unwrap());
    assert_eq!(
        got,
        (ROOT.to_owned(), 9, 11),
        "pushed past the X typed at it"
    );
    assert_eq!((got.1, got.2), (a, h), "the same as the mapping by hand");
    assert_eq!(&now[got.1..got.2], "wo", "on the same text");
    assert_eq!(
        caret(&f.s, bob, "ann"),
        Some((9, 11)),
        "set, and others see it"
    );
}

/// At the doc's version: no mapping, set as given.
#[test]
fn at_the_docs_version_it_sets_as_given() {
    let f = fx("same", "hello world\n");
    let ann = join(&f.s, "ann", ParticipantKind::Person);
    let bob = join(&f.s, "bob", ParticipantKind::Person);
    insert(&f.s, bob, 0, "AB");
    let v = version(&f.s, ann);
    let got = set(f.s.set_cursor_at(ann, ROOT, v, 2, 7).unwrap());
    assert_eq!(got, (ROOT.to_owned(), 2, 7));
    assert_eq!(caret(&f.s, bob, "ann"), Some((2, 7)));
}

/// A version ahead of the doc's: dropped (`Ahead`), the caret as it was.
#[test]
fn ahead_is_dropped_and_sets_nothing() {
    let f = fx("ahead", "hello world\n");
    let ann = join(&f.s, "ann", ParticipantKind::Person);
    let v = version(&f.s, ann);
    assert_eq!(
        f.s.set_cursor_at(ann, ROOT, v + 1, 3, 3).unwrap(),
        CursorAtVersion::Dropped(CursorDrop::Ahead)
    );
    assert_eq!(caret(&f.s, ann, "ann"), None, "nothing set");
    set(f.s.set_cursor_at(ann, ROOT, v, 1, 1).unwrap());
    assert_eq!(
        f.s.set_cursor_at(ann, ROOT, v + 5, 4, 4).unwrap(),
        CursorAtVersion::Dropped(CursorDrop::Ahead)
    );
    assert_eq!(caret(&f.s, ann, "ann"), Some((1, 1)), "the caret as it was");
}

/// Behind by more than the live window holds: dropped (`PastWindow`), as `events_since`
/// refuses; nothing set.
#[test]
fn behind_past_the_window_is_dropped_and_sets_nothing() {
    let mut c = config();
    c.live_window_events = 3;
    let f = fx_with("window", "hello world\n", c);
    let ann = join(&f.s, "ann", ParticipantKind::Person);
    let bob = join(&f.s, "bob", ParticipantKind::Person);
    let v0 = version(&f.s, ann);
    for i in 0..4 {
        insert(&f.s, bob, 0, &i.to_string());
    }
    assert!(f.s.events_since(ann, v0).is_err(), "past the window");
    assert_eq!(
        f.s.set_cursor_at(ann, ROOT, v0, 2, 2).unwrap(),
        CursorAtVersion::Dropped(CursorDrop::PastWindow)
    );
    assert_eq!(caret(&f.s, bob, "ann"), None, "nothing set");
    // inside the window it maps
    let v1 = v0 + 1;
    assert!(f.s.events_since(ann, v1).is_ok());
    let got = set(f.s.set_cursor_at(ann, ROOT, v1, 1, 1).unwrap());
    assert_eq!(
        (got.1, got.2),
        (4, 4),
        "past the three digits typed at 0 since"
    );
}

/// An offset off a character boundary after the mapping (inside a multibyte character): the
/// refusal `set_cursor` gives (`MPC-E114`), nothing set.
#[test]
fn off_a_boundary_after_mapping_is_e114() {
    let f = fx("boundary", "é\n");
    let ann = join(&f.s, "ann", ParticipantKind::Person);
    let bob = join(&f.s, "bob", ParticipantKind::Person);
    let v0 = version(&f.s, ann);
    insert(&f.s, bob, 0, "x"); // "xé\n": byte 2 is inside the é
    let e = f.s.set_cursor_at(ann, ROOT, v0, 1, 1).unwrap_err();
    assert_eq!(e.report().id(), "MPC-E114", "{e:?}");
    let e =
        f.s.set_cursor(ann, ROOT, At::Byte(2), At::Byte(2))
            .unwrap_err();
    assert_eq!(
        e.report().id(),
        "MPC-E114",
        "set_cursor's own refusal there"
    );
    assert_eq!(caret(&f.s, bob, "ann"), None, "nothing set");
    // past the end after mapping: the same
    let e = f.s.set_cursor_at(ann, ROOT, v0, 9, 9).unwrap_err();
    assert_eq!(e.report().id(), "MPC-E114", "{e:?}");
}

/// A reader is refused as `set_cursor` refuses them, whatever the version (admission first).
#[test]
fn a_reader_is_refused() {
    let f = fx("reader", "hello world\n");
    f.host
        .rights
        .lock()
        .unwrap()
        .insert("rita".into(), Rights::read_only());
    let rita = join(&f.s, "rita", ParticipantKind::Person);
    let bob = join(&f.s, "bob", ParticipantKind::Person);
    let v0 = version(&f.s, rita);
    insert(&f.s, bob, 0, "AB");
    let want =
        f.s.set_cursor(rita, ROOT, At::Byte(0), At::Byte(0))
            .unwrap_err()
            .report()
            .id()
            .to_owned();
    assert_eq!(want, "MPC-E102");
    for v in [v0, v0 + 1, v0 + 9] {
        let e = f.s.set_cursor_at(rita, ROOT, v, 0, 0).unwrap_err();
        assert_eq!(e.report().id(), want, "version {v}: {e:?}");
    }
    assert_eq!(caret(&f.s, bob, "rita"), None);
}

/// After a restart the changes since come from the store: the caret maps the same.
#[test]
fn maps_across_a_restart_from_the_store() {
    let f = fx("restart", "hello world\n");
    let ann = join(&f.s, "ann", ParticipantKind::Person);
    let bob = join(&f.s, "bob", ParticipantKind::Person);
    let v0 = version(&f.s, ann);
    insert(&f.s, bob, 0, "AB");
    insert(&f.s, bob, 8, "X");
    f.mgr.release().unwrap();
    let store = Arc::new(DocStore::open(f.dir.join("docs.redb")).unwrap());
    let (mgr, s) = open(store, f.host.clone(), config(), None);
    let ann = join(&s, "ann", ParticipantKind::Person);
    assert_eq!(text(&s, ann), "ABhello Xworld\n");
    assert_eq!(version(&s, ann), v0 + 2);
    let got = set(s.set_cursor_at(ann, ROOT, v0, 6, 8).unwrap());
    assert_eq!((got.1, got.2), (9, 11));
    assert_eq!(caret(&s, ann, "ann"), Some((9, 11)));
    mgr.release().unwrap();
}
