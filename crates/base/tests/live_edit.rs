//! The hunk channel (kit v0.9.7, K2's Live tab): `Session::edit_at_base` (amendment E-LIVE
//! r2) and `Session::events_since` (K3), public API only.
//!
//! - **Each rule as a test**, through a session: the window and its refusals (E112), the
//!   tie (the change applied first stays left), deletion vs deletion, competing text (E111
//!   with the region), a deletion split around others' inserts, a replacement's text at the
//!   left end, validation first (E151), a refusal applies nothing (P4), attribution and the
//!   event, undo of the edit as applied.
//! - **The property** (`live_clients_converge`): 3 clients, each the JS client's model (one
//!   change in flight, the rest buffered; incoming changes carried through its pending ones
//!   by the client mirror), typing random hunks at random times with random delivery and ack
//!   timing, against a real session, with agent edits, undo and outside edits in the window.
//!   Checked: at every ack the client's own prediction of its rebased change is the server's
//!   (P1); every client's text converges on the server's, which is the serial replay of the
//!   events (P1); a refusal changes nothing (P4) and the client predicted it; others'
//!   inserted text (unique tokens) inserted after a base is never deleted by an edit made at
//!   it (P3); streams of inserts only, or deletions only, are never refused (P2). **r2.3:**
//!   every live edit is also judged by an ID-level oracle that never uses the positional
//!   rebase (K2 Engine's: P2, a refusal only on a conflict by intent; P2c, every conflict by
//!   intent refused), in every stream, and a stream of mixed edits at overlapping ranges
//!   reaches the shapes r2.2's rule (1) got wrong (an insertion touching its own deletion, a
//!   split replacement, a deletion of replaced text).
//!   `LIVE_CASES` histories (default 128; ≥ 10^4 on a node), `LIVE_STEPS` steps each.
//! - **The mutants** (`LiveMutant`: r2's six, r2.1's `window-holds-expanded-hunks`, and
//!   r2.3's rule (1) on the server only (caught by P1) and on both sides (P2, P2c)) are each
//!   caught, and each catching history passes without its mutant.
//! - **Clarification r2.1:** clients typing in the same multi-author word never bounce (F1:
//!   the window holds the authored hunks); the window is exactly the hunk sets broadcast,
//!   every kind of change (F2 (a), checked at the end of every history).
//! - **Shared vectors** for the JS client (`tests/vectors/live_rebase.json`): written by
//!   `LIVE_VECTORS_WRITE=1`, else checked (a stale file fails).
//! - **The trace hook** for K2 Engine's exhaustive model (`LIVE_TRACES=<file>`): cases in the
//!   vectors' format, replayed through the rebase.
//! - **Parity:** a person's live edit and the agent verbs for the same rows (P9 `insert
//!   --at`, P10 `replace --range`, P11 `delete --lines`) give the same text and attribution.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use k2_mpc_base::differential::{with_live_mutant, LiveMutant};
use k2_mpc_base::errors::ErrorReport;
use k2_mpc_base::session::rebase::{rebase_incoming, rebase_ours, Carried};
use k2_mpc_base::session::{
    AuditEvent, ChangeEvent, Config, ConnId, EditOptions, Host, Participant, ParticipantKind,
    Rights, Session, SessionManager, Stay, TextFilePack, UndoOutcome,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::text::diff::{self, Hunk};
use k2_mpc_base::Edit;
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};

const MD: TextFilePack = TextFilePack::new("markdown", 4 << 20);
const ROOT: &str = TextFilePack::ROOT;

// ---------- harness ----------

struct TestHost {
    now: AtomicU64,
    events: Mutex<Vec<AuditEvent>>,
}

impl Host for TestHost {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        self.now.fetch_add(1, Ordering::SeqCst)
    }
    fn on_event(&self, ev: &AuditEvent) {
        self.events.lock().unwrap().push(ev.clone());
    }
}

struct Fx {
    dir: std::path::PathBuf,
    file: std::path::PathBuf,
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
    c.write_quiet_ms = 1 << 40;
    c.write_max_delay_ms = 1 << 40;
    c
}

fn fx_with(tag: &str, text: &str, c: Config) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-live-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let ws = dir.join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    let ws = std::fs::canonicalize(ws).unwrap();
    let file = ws.join("d.md");
    std::fs::write(&file, text).unwrap();
    let host = Arc::new(TestHost {
        now: AtomicU64::new(1_000_000),
        events: Mutex::new(Vec::new()),
    });
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    store.unsynced_for_tests();
    let mgr = SessionManager::new(store, host, c);
    let key = DocKey {
        space: "ws".into(),
        path: "d.md".into(),
    };
    let s = mgr.open_file(Arc::new(MD), &key, &file, &ws).unwrap();
    Fx { dir, file, mgr, s }
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

fn h(start: usize, end: usize, insert: &str) -> Hunk {
    Hunk {
        start,
        end,
        insert: insert.into(),
    }
}

fn text(s: &Session, c: ConnId) -> String {
    s.read(c).unwrap().text_by_root[0].1.clone()
}

fn version(s: &Session, c: ConnId) -> u64 {
    s.read(c).unwrap().version
}

fn at(
    s: &Session,
    c: ConnId,
    base: u64,
    hunks: Vec<Hunk>,
) -> Result<k2_mpc_base::session::EditAtBase, ErrorReport> {
    s.edit_at_base(c, ROOT, base, hunks, &EditOptions::new())
}

fn apply(t: &str, hs: &[Hunk]) -> String {
    diff::validate(t, hs).unwrap_or_else(|e| panic!("{e:?}: {t:?} {hs:?}"));
    diff::apply(t, hs)
}

// ---------- each rule through a session ----------

/// At the current version it is a plain edit: applied, attributed, broadcast, undoable.
#[test]
fn at_the_current_version_it_is_an_edit() {
    let f = fx("plain", "hello\n");
    let a = join(&f.s, "ann", ParticipantKind::Person);
    let rx = f.s.subscribe(a).unwrap();
    let v = version(&f.s, a);
    let r = at(&f.s, a, v, vec![h(5, 5, " world")]).unwrap();
    assert_eq!(text(&f.s, a), "hello world\n");
    assert_eq!(r.result.version, v + 1);
    assert_eq!(r.hunks, vec![h(5, 5, " world")]);
    let ev = rx.try_recv().unwrap();
    assert_eq!(
        (ev.pid.as_str(), ev.version, ev.hunks.clone()),
        ("ann", v + 1, r.hunks)
    );
    // the authored range is the hunk: undo takes it out
    assert!(matches!(
        f.s.undo(a, 1).unwrap(),
        UndoOutcome::Applied { .. }
    ));
    assert_eq!(text(&f.s, a), "hello\n");
}

/// Rule 3 through a session: two people type at the same point at the same base; the
/// change applied first stays left. Rule 4 (a): both delete the same word; no refusal.
#[test]
fn ties_and_shared_deletions_never_bounce() {
    let f = fx("tie", "ab\n");
    let (a, b) = (
        join(&f.s, "ann", ParticipantKind::Person),
        join(&f.s, "bob", ParticipantKind::Person),
    );
    let v = version(&f.s, a);
    at(&f.s, a, v, vec![h(1, 1, "X")]).unwrap();
    at(&f.s, b, v, vec![h(1, 1, "Y")]).unwrap();
    assert_eq!(text(&f.s, a), "aXYb\n");
    let v = version(&f.s, a);
    at(&f.s, a, v, vec![h(1, 3, "")]).unwrap();
    let r = at(&f.s, b, v, vec![h(1, 3, "")]).unwrap();
    assert_eq!(text(&f.s, a), "ab\n");
    // nothing left to delete: nothing applied, no change
    assert!(r.hunks.is_empty());
    assert_eq!(r.result.version, v + 1);
}

/// Rule 4 (b): competing text refuses with MPC-E111 and the region; nothing applies (P4).
#[test]
fn competing_text_refuses_with_the_region() {
    let f = fx("e111", "one two three\n");
    let (a, b) = (
        join(&f.s, "ann", ParticipantKind::Person),
        join(&f.s, "bob", ParticipantKind::Person),
    );
    let v = version(&f.s, a);
    at(&f.s, a, v, vec![h(4, 7, "2")]).unwrap();
    let (t0, v0) = (text(&f.s, a), version(&f.s, a));
    let e = at(&f.s, b, v, vec![h(4, 7, "deux")]).unwrap_err();
    assert_eq!(e.id(), "MPC-E111");
    assert_eq!(session_exit_of(&e), 1);
    let json = e.to_json();
    assert!(
        json.contains("\"why\":\"replacement\"") && json.contains("ann in change"),
        "{json}"
    );
    assert_eq!(
        (text(&f.s, a), version(&f.s, a)),
        (t0, v0),
        "nothing applied"
    );
    // ... and inserting strictly inside a range someone deleted
    let v = version(&f.s, a);
    at(&f.s, a, v, vec![h(0, 4, "")]).unwrap();
    let e = at(&f.s, b, v, vec![h(2, 2, "!")]).unwrap_err();
    assert!(
        e.to_json().contains("insert_inside_deleted"),
        "{}",
        e.to_json()
    );
}

fn session_exit_of(e: &ErrorReport) -> u8 {
    e.exit()
}

/// Rule 5: a deletion over others' later insertion is split around it; a replacement's
/// text goes at the left end.
#[test]
fn others_inserts_survive_deletions_and_replacements() {
    let f = fx("split", "abcdef\n");
    let (a, b) = (
        join(&f.s, "ann", ParticipantKind::Person),
        join(&f.s, "bob", ParticipantKind::Person),
    );
    let v = version(&f.s, a);
    at(&f.s, a, v, vec![h(3, 3, "XY")]).unwrap();
    let r = at(&f.s, b, v, vec![h(1, 5, "")]).unwrap();
    assert_eq!(text(&f.s, a), "aXYf\n");
    assert_eq!(r.hunks, vec![h(1, 3, ""), h(5, 7, "")]);
    let v = version(&f.s, a);
    at(&f.s, a, v, vec![h(2, 2, "Q")]).unwrap();
    at(&f.s, b, v, vec![h(1, 3, "Z")]).unwrap();
    assert_eq!(text(&f.s, a), "aZQf\n");
}

/// Rule 1: the window. A base past it, or a version not reached, is MPC-E112 (exit 8); a
/// base inside it works across a restart (the window is the store's).
#[test]
fn the_window_and_base_expired() {
    let mut c = config();
    c.live_window_events = 5;
    let f = fx_with("window", "x\n", c);
    let a = join(&f.s, "ann", ParticipantKind::Person);
    let v0 = version(&f.s, a);
    for i in 0..6 {
        let v = version(&f.s, a);
        at(&f.s, a, v, vec![h(0, 0, &i.to_string())]).unwrap();
    }
    let e = at(&f.s, a, v0, vec![h(0, 0, "!")]).unwrap_err();
    assert_eq!((e.id(), e.exit()), ("MPC-E112", 8));
    assert!(f.s.events_since(a, v0).is_err());
    let e = at(&f.s, a, version(&f.s, a) + 1, vec![h(0, 0, "!")]).unwrap_err();
    assert_eq!(e.id(), "MPC-E112");
    // inside the window, after a restart: the same events, the same rebase
    let v1 = v0 + 2;
    let before = f.s.events_since(a, v1).unwrap();
    assert_eq!(before.len(), 4);
    f.mgr.release().unwrap();
    let f2 = reopen(&f);
    let a2 = join(&f2, "ann", ParticipantKind::Person);
    let after = f2.events_since(a2, v1).unwrap();
    assert_eq!(after, before);
    let r = at(&f2, a2, v1, vec![h(0, 0, "!")]).unwrap();
    assert_eq!(
        r.hunks,
        vec![h(4, 4, "!")],
        "after the four digits typed since"
    );
}

fn reopen(f: &Fx) -> Arc<Session> {
    let store = Arc::new(DocStore::open(f.dir.join("docs.redb")).unwrap());
    let host = Arc::new(TestHost {
        now: AtomicU64::new(9_000_000),
        events: Mutex::new(Vec::new()),
    });
    let mgr = SessionManager::new(store, host, config());
    let ws = f.file.parent().unwrap().to_owned();
    let key = DocKey {
        space: "ws".into(),
        path: "d.md".into(),
    };
    let s = mgr.open_file(Arc::new(MD), &key, &f.file, &ws).unwrap();
    std::mem::forget(mgr);
    s
}

/// Rule 7: hunks are checked against the base's text first (E151): out of order, out of
/// range, or off a character boundary there, even when they would fit the text now.
#[test]
fn hunks_are_validated_at_the_base() {
    let f = fx("e151", "é\n");
    let (a, b) = (
        join(&f.s, "ann", ParticipantKind::Person),
        join(&f.s, "bob", ParticipantKind::Person),
    );
    let v = version(&f.s, a);
    at(&f.s, a, v, vec![h(0, 0, "abcdef")]).unwrap();
    // offset 4 fits the text now, but the base's text is 3 bytes
    let e = at(&f.s, b, v, vec![h(4, 4, "x")]).unwrap_err();
    assert_eq!((e.id(), e.exit()), ("MPC-E151", 2));
    // inside "é" at the base
    let e = at(&f.s, b, v, vec![h(1, 1, "x")]).unwrap_err();
    assert_eq!(e.id(), "MPC-E151");
    let e = at(&f.s, b, v, vec![h(2, 2, "x"), h(0, 0, "y")]).unwrap_err();
    assert_eq!(e.id(), "MPC-E151");
}

/// K3: the events since a version replay onto that version's text to the doc's (the serial
/// replay), every kind of change included.
#[test]
fn events_since_replays_every_kind_of_change() {
    let f = fx("k3", "alpha\nbeta\n");
    let a = join(&f.s, "ann", ParticipantKind::Person);
    let g = join(&f.s, "agent", ParticipantKind::Agent);
    let v0 = version(&f.s, a);
    let t0 = text(&f.s, a);
    at(&f.s, a, v0, vec![h(5, 5, "!")]).unwrap();
    f.s.replace_text(g, ROOT, "alpha!\nbeta\ngamma\n").unwrap();
    f.s.undo(a, 1).unwrap();
    std::fs::write(&f.file, "alpha\nBETA\ngamma\n").unwrap();
    f.s.file_changed().unwrap();
    let evs = f.s.events_since(a, v0).unwrap();
    assert!(evs.len() >= 4, "{evs:?}");
    let mut t = t0;
    for e in &evs {
        t = apply(&t, &e.hunks);
    }
    assert_eq!(t, text(&f.s, a));
    assert!(f.s.events_since(a, version(&f.s, a)).unwrap().is_empty());
}

// ---------- parity ----------

/// A person's live edit has the agent verbs of the same rows as its equivalent: P9 `insert
/// --at`, P10 `replace --range`, P11 `delete --lines` give the same text, one change each,
/// attributed to the one who made it.
#[test]
fn parity_live_edit_and_agent_verbs() {
    let body = "# t\n\nfirst line\nsecond line\n";
    let cases: [(&[&str], &str, Vec<Hunk>); 3] = [
        (&["insert", "--at", "3:1"], "new ", vec![h(5, 5, "new ")]),
        (
            &["replace", "--range", "3:1-3:6"],
            "1st",
            vec![h(5, 10, "1st")],
        ),
        (&["delete", "--lines", "4,4"], "", vec![h(16, 28, "")]),
    ];
    for (argv, stdin, hunks) in cases {
        let fp = fx("parity-p", body);
        let fa = fx("parity-a", body);
        let p = join(&fp.s, "pat", ParticipantKind::Person);
        let g = join(&fa.s, "agt", ParticipantKind::Agent);
        let v = version(&fp.s, p);
        at(&fp.s, p, v, hunks).unwrap();
        let argv: Vec<String> = argv.iter().map(|s| (*s).to_owned()).collect();
        let out = fa.s.run_tool(g, ROOT, &argv, stdin.as_bytes()).unwrap();
        assert_eq!(out.exit, 0, "{argv:?}: {out:?}");
        assert_eq!(text(&fp.s, p), text(&fa.s, g), "{argv:?}");
        assert_eq!(version(&fp.s, p), version(&fa.s, g));
        let lp = fp.s.blame(p, ROOT, None).unwrap();
        let la = fa.s.blame(g, ROOT, None).unwrap();
        let who = |b: &k2_mpc_base::session::Blame, me: &str| {
            b.lines.iter().map(|l| l.author == me).collect::<Vec<_>>()
        };
        assert_eq!(who(&lp, "pat"), who(&la, "agt"), "{argv:?}");
    }
}

// ---------- the property: clients converge ----------

/// Pieces of random text: plain bits, line breaks, multi-byte characters.
const BITS: &[&str] = &["a", "b", "c", " ", "\n", "é", "日"];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Stream {
    /// Every kind of hunk, and the other channels in the window.
    Mixed,
    /// Pure insertions only: never refused (P2).
    InsertsOnly,
    /// Pure deletions only: never refused (P2).
    DeletesOnly,
    /// r2.3's classes: insertions, deletions and replacements of one or two characters on a
    /// short text, so ranges overlap (an insertion coming to touch its own change's deletion,
    /// a replacement split around others' text, a deletion of text another replaced); the
    /// other channels too. Judged by the ID-level oracle (P2, P2c).
    Overlapping,
}

fn boundary(rng: &mut SmallRng, t: &str) -> usize {
    let mut x = rng.gen_range(0..=t.len());
    while !t.is_char_boundary(x) {
        x -= 1;
    }
    x
}

/// Random canonical hunks on `t`; inserted text sometimes a unique token `<n>` (P3).
fn random_hunks(rng: &mut SmallRng, t: &str, stream: Stream, tokens: &AtomicU64) -> Vec<Hunk> {
    let n = rng.gen_range(1..=3);
    let mut cuts: Vec<usize> = (0..2 * n).map(|_| boundary(rng, t)).collect();
    cuts.sort_unstable();
    let mut out: Vec<Hunk> = Vec::new();
    for c in cuts.chunks(2) {
        let (s, mut e) = (c[0], c[1]);
        if e - s > 6 && rng.gen_bool(0.6) {
            e = s;
        }
        let ins = match stream {
            Stream::DeletesOnly => String::new(),
            _ if rng.gen_bool(0.3) => format!("<{}>", tokens.fetch_add(1, Ordering::SeqCst)),
            _ => (0..rng.gen_range(0..3))
                .map(|_| BITS[rng.gen_range(0..BITS.len())])
                .collect(),
        };
        let (s, e) = match stream {
            Stream::InsertsOnly => (s, s),
            Stream::Overlapping => {
                // zero, one or two characters from `s`
                let k = rng.gen_range(0..=2);
                (
                    s,
                    t[s..].char_indices().nth(k).map_or(t.len(), |(i, _)| s + i),
                )
            }
            _ => (s, e),
        };
        if out.last().is_some_and(|l: &Hunk| s <= l.end) {
            continue;
        }
        if s == e && ins.is_empty() {
            continue;
        }
        out.push(h(s, e, &ins));
    }
    out
}

/// One client: the JS client's model (K1).
struct Client {
    conn: ConnId,
    rx: Receiver<ChangeEvent>,
    inbox: VecDeque<ChangeEvent>,
    /// Every event received, as broadcast, and the version it joined at.
    seen: Vec<ChangeEvent>,
    joined: u64,
    /// The server's text at `version` (every event applied in order).
    confirmed: String,
    version: u64,
    /// Sent, unacknowledged: its base and hunks (carried through everything since, each hunk
    /// as sent (r2.3), in the coordinates of `confirmed`).
    inflight: Option<(u64, Carried)>,
    /// Typed while one is in flight (coordinates: `confirmed` + `inflight`).
    buffer: Option<Carried>,
    /// The editor's text.
    display: String,
    /// The server's answer to the request in flight, when it has one: `Ok(version, applied
    /// hunks)` or `Err(version at the refusal)`.
    answer: Option<Result<(u64, Vec<Hunk>), u64>>,
    /// The client predicted the server will refuse its request in flight (rolled back).
    doomed: bool,
    /// Ready to send (`(base, hunks)`), not yet delivered.
    outbox: Option<(u64, Vec<Hunk>)>,
}

impl Client {
    fn drain(&mut self) {
        while let Ok(e) = self.rx.try_recv() {
            self.seen.push(e.clone());
            self.inbox.push_back(e);
        }
    }

    /// P1, at every step: the editor's text is the confirmed text plus what is pending.
    fn check(&self, why: &str) -> Result<(), String> {
        let mut t = self.confirmed.clone();
        if !self.doomed {
            if let Some((_, p)) = &self.inflight {
                t = apply(&t, &p.hunks());
            }
            if let Some(b) = &self.buffer {
                t = apply(&t, &b.hunks());
            }
        }
        if self.display != t {
            return Err(format!(
                "P1 ({why}): the editor reads {:?}, confirmed + pending is {t:?}",
                self.display
            ));
        }
        Ok(())
    }

    /// Type: hunks on the editor's text, sent if nothing is in flight, else buffered (one).
    fn type_(&mut self, hs: Vec<Hunk>, own_bug: bool) {
        self.display = apply(&self.display, &hs);
        if self.inflight.is_none() {
            // planted (own-change-in-window): a base one behind the client's last ack
            let base = if own_bug && self.version > 1 {
                self.version - 1
            } else {
                self.version
            };
            self.inflight = Some((base, Carried::new(&hs)));
            self.outbox = Some((base, hs));
        } else {
            self.buffer = Some(Carried::new(&hs));
        }
    }

    /// The next thing the client can process: an event, or the answer once every event
    /// before it is in. `Err` with why when the model's checks fail.
    fn step(&mut self, own_bug: bool) -> Result<bool, String> {
        self.drain();
        // the answer to our request, when the stream has reached it
        if let Some(ans) = self.answer.clone() {
            let ready = match &ans {
                Ok((v, _)) | Err(v) => self.version >= *v,
            };
            match ans {
                Err(_) if ready => {
                    if !self.doomed {
                        return Err("refused, though the client predicted no conflict".into());
                    }
                    self.answer = None;
                    self.doomed = false;
                    self.inflight = None;
                    self.display = self.confirmed.clone();
                    return Ok(true);
                }
                Ok((v, applied)) if applied.is_empty() && ready => {
                    // nothing applied: the edit was already in the text
                    if self.doomed {
                        return Err("accepted, though the client predicted a conflict".into());
                    }
                    let p = self.inflight.take().expect("in flight").1.hunks();
                    if !p.is_empty() {
                        return Err(format!("predicted {p:?}, the server applied nothing"));
                    }
                    let _ = v;
                    self.answer = None;
                    self.send_buffer(own_bug);
                    return Ok(true);
                }
                _ => {}
            }
        }
        let Some(e) = self.inbox.pop_front() else {
            return Ok(false);
        };
        if e.version != self.version + 1 {
            return Err(format!("a gap: {} after {}", e.version, self.version));
        }
        self.confirmed = apply(&self.confirmed, &e.hunks);
        self.version = e.version;
        if let Some(Ok((v, applied))) = &self.answer {
            // a no-op answer has no event of its own (it is taken above)
            if *v == e.version && !applied.is_empty() {
                // our own change: the ack
                if self.doomed {
                    return Err("accepted, though the client predicted a conflict".into());
                }
                let p = self.inflight.take().expect("in flight").1.hunks();
                if p != *applied || e.hunks != *applied {
                    return Err(format!(
                        "P1: the client predicted {p:?}, the server applied {applied:?}"
                    ));
                }
                self.answer = None;
                self.send_buffer(own_bug);
                self.check("after the ack")?;
                return Ok(true);
            }
        }
        // someone else's change: carried through ours (client mirror, against ours' gap
        // form), ours through it (each hunk as sent, r2.3)
        let mut inc = e.hunks.clone();
        if !self.doomed {
            if let Some((base, p)) = self.inflight.take() {
                let shown = Carried::new(&inc).mirrored(&p.gaps()).hunks();
                match p.through(&inc, 0) {
                    Ok(p2) => {
                        self.inflight = Some((base, p2));
                        inc = shown;
                    }
                    Err(c) => {
                        if std::env::var("LIVE_DEBUG").is_ok() {
                            eprintln!("[debug] conn {:?} doomed: inflight {p:?} (base {base}) through v{} {inc:?}: {c:?}", self.conn, e.version);
                        }
                        // the server will refuse it: roll back what isn't confirmed
                        self.inflight = Some((base, p));
                        self.doomed = true;
                        self.buffer = None;
                        self.display = self.confirmed.clone();
                        return Ok(true);
                    }
                }
            }
            if let Some(b) = self.buffer.take() {
                let shown = Carried::new(&inc).mirrored(&b.gaps()).hunks();
                match b.through(&inc, 0) {
                    Ok(b2) => {
                        self.buffer = Some(b2);
                        inc = shown;
                    }
                    Err(_) => {
                        // drop the buffered edit: the editor shows confirmed + in flight
                        let mut t = self.confirmed.clone();
                        if let Some((_, p)) = &self.inflight {
                            t = apply(&t, &p.hunks());
                        }
                        self.display = t;
                        self.check("after dropping the buffer")?;
                        return Ok(true);
                    }
                }
            }
            self.display = apply(&self.display, &inc);
        } else {
            self.display = self.confirmed.clone();
        }
        self.check("after an incoming change")?;
        Ok(true)
    }

    fn send_buffer(&mut self, own_bug: bool) {
        if let Some(b) = self.buffer.take() {
            let base = if own_bug && self.version > 1 {
                self.version - 1
            } else {
                self.version
            };
            // sent as it reads now: its hunks as sent from here on
            let b = b.hunks();
            self.inflight = Some((base, Carried::new(&b)));
            self.outbox = Some((base, b));
        }
    }
}

/// Counts over a run.
#[derive(Default, Debug, Clone)]
struct Counts {
    cases: u64,
    live_edits: u64,
    accepted: u64,
    refused: u64,
    noops: u64,
    rebased_through: u64,
    others: u64,
    acks_checked: u64,
    /// P3's exposure: bytes others inserted after a base that an edit at it split its
    /// deletion around (kept).
    tokens_checked: u64,
    /// P2, P2c (r2.3): live edits judged by the ID-level oracle, and those it found in
    /// conflict by intent.
    p2_judged: u64,
    id_conflicts: u64,
    /// r2.3's shapes: a deletion of text someone else deleted or replaced since (S5); a
    /// replacement with others' text inserted inside it since (rule 5's split); an insertion
    /// that came to touch its own change's deletion (r2.2's rule (1) merged them).
    del_over_deleted: u64,
    split_replacement: u64,
    touch_joined: u64,
}

impl Counts {
    fn add(&mut self, o: &Counts) {
        self.cases += o.cases;
        self.live_edits += o.live_edits;
        self.accepted += o.accepted;
        self.refused += o.refused;
        self.noops += o.noops;
        self.rebased_through += o.rebased_through;
        self.others += o.others;
        self.acks_checked += o.acks_checked;
        self.tokens_checked += o.tokens_checked;
        self.p2_judged += o.p2_judged;
        self.id_conflicts += o.id_conflicts;
        self.del_over_deleted += o.del_over_deleted;
        self.split_replacement += o.split_replacement;
        self.touch_joined += o.touch_joined;
    }
}

/// The ID-level oracle (K2 Engine's, `g1-live-draft.md` §2; E-LIVE r2.3): every character
/// ever, tombstones kept, in document order. An insertion at visible gap `g` goes right after
/// visible character `g − 1`, before any tombstones there. Built from the changes as
/// broadcast, every channel; it never uses the positional rebase.
struct Shadow {
    next: u64,
    /// Every element ever: (id, alive).
    els: Vec<(u64, bool)>,
    /// The visible ids at each version seen.
    at: std::collections::BTreeMap<u64, Vec<u64>>,
}

impl Shadow {
    fn new(text: &str, version: u64) -> Self {
        let els: Vec<(u64, bool)> = (0..text.chars().count() as u64)
            .map(|i| (i, true))
            .collect();
        let vis = els.iter().map(|e| e.0).collect();
        Shadow {
            next: els.len() as u64,
            els,
            at: std::collections::BTreeMap::from([(version, vis)]),
        }
    }

    /// A change (canonical hunks on `before`, the text at the version before it).
    fn apply(&mut self, before: &str, hunks: &[Hunk], version: u64) {
        let vis: Vec<usize> = (0..self.els.len()).filter(|&i| self.els[i].1).collect();
        for h in hunks.iter().rev() {
            let cs = before[..h.start].chars().count();
            let ce = cs + before[h.start..h.end].chars().count();
            for &i in &vis[cs..ce] {
                self.els[i].1 = false;
            }
            let at = if cs == 0 { 0 } else { vis[cs - 1] + 1 };
            let new: Vec<(u64, bool)> = h
                .insert
                .chars()
                .map(|_| {
                    self.next += 1;
                    (self.next, true)
                })
                .collect();
            self.els.splice(at..at, new);
        }
        let now = self.els.iter().filter(|e| e.1).map(|e| e.0).collect();
        self.at.insert(version, now);
    }

    /// Does `ours` (as sent at `base`, whose text is `base_text`) conflict by intent with the
    /// changes since (P2, P2c)? (i) a replacement some of whose deleted ids are gone now;
    /// (ii) an insertion, and some change since deletes the visible neighbours of both of its
    /// tie-break placements just before it ([`inside_both`]; r2.3 S3).
    fn conflict(
        &self,
        base: u64,
        base_text: &str,
        ours: &[Hunk],
        since: &[ChangeEvent],
    ) -> Option<&'static str> {
        let vb = &self.at[&base];
        let now = self.at.values().next_back().expect("a version");
        let alive: std::collections::HashSet<u64> = now.iter().copied().collect();

        for h in Carried::new(ours).hunks() {
            let cs = base_text[..h.start].chars().count();
            let ce = cs + base_text[h.start..h.end].chars().count();
            if h.insert.is_empty() {
                continue;
            }
            if cs < ce {
                if vb[cs..ce].iter().any(|id| !alive.contains(id)) {
                    return Some("replacement");
                }
                continue;
            }
            if inside_both(vb, base_text, ce, since) {
                return Some("insert_inside_deleted");
            }
        }
        None
    }

    /// r2.3's shapes in `ours` at `base`, judged now: a deletion (or replacement) of ids
    /// someone else deleted since (S5), and a replacement with others' text inserted since
    /// between its deleted ids (split by rule 5).
    fn shapes(&self, base: u64, base_text: &str, ours: &[Hunk]) -> (bool, bool) {
        let vb = &self.at[&base];
        let base_ids: std::collections::HashSet<u64> = vb.iter().copied().collect();
        let now: std::collections::HashSet<u64> = self
            .at
            .values()
            .next_back()
            .unwrap()
            .iter()
            .copied()
            .collect();
        let pos: HashMap<u64, usize> = self.els.iter().enumerate().map(|(i, e)| (e.0, i)).collect();
        let (mut del_over, mut split) = (false, false);
        for h in Carried::new(ours).hunks() {
            let cs = base_text[..h.start].chars().count();
            let ce = cs + base_text[h.start..h.end].chars().count();
            if cs == ce {
                continue;
            }
            del_over |= vb[cs..ce].iter().any(|id| !now.contains(id));
            if !h.insert.is_empty() && ce - cs >= 2 {
                let (a, b) = (pos[&vb[cs]], pos[&vb[ce - 1]]);
                split |= self.els[a..b]
                    .iter()
                    .any(|e| e.1 && !base_ids.contains(&e.0));
            }
        }
        (del_over, split)
    }
}

/// (ii) of [`Shadow::conflict`], by IDs (K2 Engine's markers): ours' two tie-break placements
/// are invisible markers at char gap `gap` of the base (visible ids `base_ids`, text
/// `base_text`), the left one first; each change since is replayed by IDs: an insertion at
/// visible gap g goes right after visible character g − 1, except in a gap holding a marker:
/// right after the left one, else right before the right one (so others' insertions at ours'
/// point land between the markers). True when some change deletes the visible neighbours of
/// both markers just before it.
fn inside_both(base_ids: &[u64], base_text: &str, gap: usize, since: &[ChangeEvent]) -> bool {
    // Some(id, alive) for a character; None for a marker (left, then right)
    const L: u64 = u64::MAX;
    const R: u64 = u64::MAX - 1;
    let mut els: Vec<(u64, bool)> = base_ids.iter().map(|&i| (i, true)).collect();
    els.splice(gap..gap, [(L, false), (R, false)]);
    let mut text = base_text.to_owned();
    let mut fresh = u64::MAX / 2;
    for e in since {
        let vis: Vec<usize> = (0..els.len())
            .filter(|&i| els[i].1 && els[i].0 < R)
            .collect();
        let mut deleted = std::collections::HashSet::new();
        for h in &e.hunks {
            let cs = text[..h.start].chars().count();
            let ce = cs + text[h.start..h.end].chars().count();
            deleted.extend(vis[cs..ce].iter().copied());
        }
        let neighbours_deleted = |m: u64| {
            let k = els.iter().position(|x| x.0 == m).expect("marker");
            let left = (0..k).rev().find(|&i| els[i].1 && els[i].0 < R);
            let right = (k + 1..els.len()).find(|&i| els[i].1 && els[i].0 < R);
            matches!((left, right), (Some(a), Some(b)) if deleted.contains(&a) && deleted.contains(&b))
        };
        if neighbours_deleted(L) && neighbours_deleted(R) {
            return true;
        }
        for h in e.hunks.iter().rev() {
            let cs = text[..h.start].chars().count();
            let ce = cs + text[h.start..h.end].chars().count();
            for &i in &vis[cs..ce] {
                els[i].1 = false;
            }
            // the gap between visible characters cs − 1 and cs (tombstones and markers): after
            // the left marker if it is there, else before the right one, else at its front
            let lo = if cs == 0 { 0 } else { vis[cs - 1] + 1 };
            let hi = vis.get(cs).copied().unwrap_or(els.len());
            let at = match (
                (lo..hi).find(|&x| els[x].0 == L),
                (lo..hi).find(|&x| els[x].0 == R),
            ) {
                (Some(x), _) => x + 1,
                (None, Some(x)) => x,
                _ => lo,
            };
            let new: Vec<(u64, bool)> = h
                .insert
                .chars()
                .map(|_| {
                    fresh += 1;
                    (fresh, true)
                })
                .collect();
            els.splice(at..at, new);
        }
        text = apply(&text, &e.hunks);
    }
    false
}

/// One history. `Err`: a check failed (the mutant tests count these). `p2_oracle`: judge
/// every refusal and acceptance by the ID-level oracle ([`Shadow`]).
fn one(
    seed: u64,
    steps: u64,
    stream: Stream,
    own_bug: bool,
    p2_oracle: bool,
) -> Result<Counts, String> {
    let mut rng = SmallRng::seed_from_u64(seed);
    let len = if stream == Stream::Overlapping { 6 } else { 12 };
    let start: String = (0..rng.gen_range(0..len))
        .map(|_| BITS[rng.gen_range(0..BITS.len())])
        .collect();
    let f = fx("prop", &start);
    let tokens_n = AtomicU64::new(0);
    let agent = join(&f.s, "agent", ParticipantKind::Agent);
    let mut n = Counts {
        cases: 1,
        ..Counts::default()
    };
    let mut cs: Vec<Client> = (0..3)
        .map(|k| {
            let conn = join(&f.s, &format!("p{k}"), ParticipantKind::Person);
            let rx = f.s.subscribe(conn).unwrap();
            let r = f.s.read(conn).unwrap();
            Client {
                conn,
                rx,
                inbox: VecDeque::new(),
                seen: Vec::new(),
                joined: r.version,
                confirmed: r.text_by_root[0].1.clone(),
                version: r.version,
                inflight: None,
                buffer: None,
                display: r.text_by_root[0].1.clone(),
                answer: None,
                doomed: false,
                outbox: None,
            }
        })
        .collect();
    // the text at every version (the P3 oracle's bases), from a subscription of its own
    let orx = f.s.subscribe(agent).unwrap();
    type Texts = (u64, HashMap<u64, String>, Shadow);
    let texts: Mutex<Texts> = Mutex::new({
        let r = f.s.read(agent).unwrap();
        let t = r.text_by_root[0].1.clone();
        (
            r.version,
            HashMap::from([(r.version, t.clone())]),
            Shadow::new(&t, r.version),
        )
    });
    let snap = || {
        let mut g = texts.lock().unwrap();
        while let Ok(e) = orx.try_recv() {
            let last = g.1[&g.0].clone();
            g.0 = e.version;
            g.2.apply(&last, &e.hunks, e.version);
            g.1.insert(e.version, apply(&last, &e.hunks));
        }
    };
    let deliver = |c: &mut Client, n: &mut Counts| -> Result<(), String> {
        let Some((base, hs)) = c.outbox.take() else {
            return Ok(());
        };
        n.live_edits += 1;
        let before = (text(&f.s, agent), version(&f.s, agent));
        let since = f.s.events_since(agent, base).map_err(|e| e.plain())?;
        n.rebased_through += since.len() as u64;
        // P2, P2c: the ID-level verdict on ours as sent, against the changes since
        snap();
        let verdict = {
            let g = texts.lock().unwrap();
            let bt = g.1.get(&base).cloned().unwrap_or_default();
            if g.2.at.get(&base).map(Vec::len) != Some(bt.chars().count()) {
                return Err(format!("the ID oracle has no text at version {base}"));
            }
            if diff::validate(&bt, &hs).is_err() {
                // not hunks on the base's text (a client's fault, as the planted
                // own-change-in-window): no intent to judge; the session refuses or P1 fails
                return Err(format!(
                    "the client sent {hs:?}, not hunks on version {base}'s text"
                ));
            }
            let (del_over, split) = g.2.shapes(base, &bt, &hs);
            n.del_over_deleted += u64::from(del_over);
            n.split_replacement += u64::from(split);
            g.2.conflict(base, &bt, &hs, &since)
        };
        n.p2_judged += 1;
        n.id_conflicts += u64::from(verdict.is_some());
        // an insertion that came to touch its own change's deletion (r2.2's rule (1) merged
        // them): the pieces as carried join only in the hunks applied
        let since_h: Vec<Vec<Hunk>> = since.iter().map(|e| e.hunks.clone()).collect();
        let mut carried = Some(Carried::new(&hs));
        for (k, t) in since_h.iter().enumerate() {
            carried = carried.and_then(|c| c.through(t, k).ok());
        }
        if carried.is_some_and(|c| c.hunks().len() < c.gaps().len()) {
            n.touch_joined += 1;
        }
        let res =
            f.s.edit_at_base(c.conn, ROOT, base, hs.clone(), &EditOptions::new());
        if std::env::var("LIVE_DEBUG").is_ok() {
            eprintln!(
                "[debug] conn {:?} at base {base} {hs:?} since {:?} -> {:?}",
                c.conn,
                since
                    .iter()
                    .map(|e| (e.version, e.hunks.clone()))
                    .collect::<Vec<_>>(),
                res.as_ref()
                    .map(|r| (r.result.version, r.hunks.clone()))
                    .map_err(|e| e.id())
            );
        }
        if p2_oracle {
            match (&res, verdict) {
                (Ok(_), Some(why)) => {
                    return Err(format!(
                        "P2c: accepted {hs:?} at base {base} ({:?}), which conflicts by intent \
                         ({why})",
                        texts.lock().unwrap().1.get(&base)
                    ))
                }
                (Err(e), None) if e.id() == "MPC-E111" => {
                    return Err(format!(
                        "P2: refused {hs:?} at base {base}, with no conflict by intent: {}",
                        e.plain()
                    ))
                }
                _ => {}
            }
        }
        match res {
            Ok(r) => {
                n.accepted += 1;
                if r.hunks.is_empty() {
                    n.noops += 1;
                }
                // P3, by identity: the bytes the changes since the base inserted (labelled by
                // replaying them onto the base's text) are never in a range ours deletes
                snap();
                let base_text = texts
                    .lock()
                    .unwrap()
                    .1
                    .get(&base)
                    .cloned()
                    .ok_or_else(|| format!("no text recorded at version {base}"))?;
                let mut t = base_text;
                let mut fresh = vec![false; t.len()];
                for e in &since {
                    let mut nf = Vec::with_capacity(fresh.len());
                    let mut at = 0;
                    for hk in &e.hunks {
                        nf.extend_from_slice(&fresh[at..hk.start]);
                        nf.extend(std::iter::repeat(true).take(hk.insert.len()));
                        at = hk.end;
                    }
                    nf.extend_from_slice(&fresh[at..]);
                    fresh = nf;
                    t = apply(&t, &e.hunks);
                }
                if t != before.0 {
                    return Err("the oracle's replay of the window isn't the text".into());
                }
                for hk in r.hunks.iter().filter(|x| x.start < x.end) {
                    if let Some(k) = fresh[hk.start..hk.end].iter().position(|x| *x) {
                        return Err(format!(
                            "P3: byte {} of the text, inserted after base {base}, was deleted by {hk:?}",
                            hk.start + k
                        ));
                    }
                }
                // exposure: others' fresh bytes kept in a gap between two deletions of ours
                // (rule 5's split at work)
                for w in r.hunks.windows(2) {
                    let gap = &fresh[w[0].end..w[1].start];
                    if w[0].start < w[0].end && w[1].start < w[1].end && gap.iter().all(|x| *x) {
                        n.tokens_checked += gap.len() as u64;
                    }
                }
                c.answer = Some(Ok((r.result.version, r.hunks)));
            }
            Err(e) if e.id() == "MPC-E111" => {
                n.refused += 1;
                if matches!(stream, Stream::InsertsOnly | Stream::DeletesOnly) {
                    return Err(format!("P2: refused in a {stream:?} stream: {}", e.plain()));
                }
                // P4
                if (text(&f.s, agent), version(&f.s, agent)) != before {
                    return Err("P4: a refusal changed the doc".into());
                }
                c.answer = Some(Err(before.1));
            }
            Err(e) => return Err(format!("unexpected refusal {}", e.to_json())),
        }
        Ok(())
    };
    for _ in 0..steps {
        let k = rng.gen_range(0..cs.len());
        match rng.gen_range(0..100) {
            0..=39 => {
                let c = &mut cs[k];
                if c.buffer.is_none() && !c.doomed {
                    let hs = random_hunks(&mut rng, &c.display.clone(), stream, &tokens_n);
                    if !hs.is_empty() {
                        c.type_(hs, own_bug);
                    }
                }
            }
            40..=64 => deliver(&mut cs[k], &mut n)?,
            65..=89 => {
                for _ in 0..rng.gen_range(1..4) {
                    cs[k].step(own_bug)?;
                }
            }
            90..=95 if matches!(stream, Stream::Mixed | Stream::Overlapping) => {
                // an agent's edit at the current version
                let t = text(&f.s, agent);
                let hs = random_hunks(&mut rng, &t, stream, &tokens_n);
                if !hs.is_empty() {
                    f.s.edit(
                        agent,
                        vec![Edit::Text {
                            root: ROOT.into(),
                            hunks: hs,
                        }],
                        None,
                    )
                    .unwrap();
                    n.others += 1;
                }
            }
            96..=97 if matches!(stream, Stream::Mixed | Stream::Overlapping) => {
                let _ = f.s.undo(agent, 1);
                n.others += 1;
            }
            98..=99 if matches!(stream, Stream::Mixed | Stream::Overlapping) => {
                // an outside edit: the file (as last written) with a line added
                let _ = f.s.flush(agent);
                let t = std::fs::read_to_string(&f.file).unwrap_or_default();
                std::fs::write(&f.file, format!("{t}out{}\n", rng.gen_range(0..9))).unwrap();
                let _ = f.s.file_changed();
                n.others += 1;
            }
            _ => {}
        }
        for c in cs.iter_mut() {
            c.drain();
        }
    }
    // quiesce: deliver, process, until nothing moves
    for _ in 0..10_000 {
        let mut moved = false;
        for c in cs.iter_mut() {
            if c.outbox.is_some() {
                deliver(c, &mut n)?;
                moved = true;
            }
            while c.step(own_bug)? {
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }
    let server = text(&f.s, agent);
    for (k, c) in cs.iter().enumerate() {
        if c.inflight.is_some() || c.buffer.is_some() || c.answer.is_some() {
            return Err(format!("client {k} didn't settle"));
        }
        if c.display != server || c.confirmed != server {
            return Err(format!(
                "P1: client {k} reads {:?}, the server {server:?}",
                c.display
            ));
        }
        n.acks_checked += 1;
        // F2 (a): the window holds exactly the hunk sets broadcast, every kind of change
        let window = f.s.events_since(agent, c.joined).map_err(|e| e.plain())?;
        if window != c.seen {
            return Err(format!(
                "F2: the window since {} isn't what was broadcast:\n window {window:?}\n seen {:?}",
                c.joined, c.seen
            ));
        }
    }
    Ok(n)
}

/// Clarification r2.1 (F1), the case class: clients typing letters into (and backspacing
/// in) the same word, which several people have written, all at one base, never bounce.
fn word_case(seed: u64) -> Result<u64, String> {
    let mut rng = SmallRng::seed_from_u64(seed);
    let words = ["alpha", "beta", "gamma", "delta"];
    let start = format!("{}\n", words.join(" "));
    let f = fx("word", &start);
    let cs: Vec<ConnId> = (0..3)
        .map(|k| join(&f.s, &format!("w{k}"), ParticipantKind::Person))
        .collect();
    let w = rng.gen_range(0..words.len());
    let ws: usize = words[..w].iter().map(|x| x.len() + 1).sum();
    // someone else's letter inside the word first: a multi-author word
    let v = version(&f.s, cs[0]);
    at(&f.s, cs[0], v, vec![h(ws + 2, ws + 2, "X")]).map_err(|e| e.plain())?;
    let mut edits = 0;
    for _ in 0..rng.gen_range(2..6) {
        let base = version(&f.s, cs[0]);
        let t = text(&f.s, cs[0]);
        let we = ws
            + t[ws..]
                .find(|c: char| !c.is_alphanumeric())
                .unwrap_or(t.len() - ws);
        for &c in cs.iter().skip(1).chain(std::iter::once(&cs[0])) {
            // a letter strictly inside the word, or a backspace of one of its letters
            let hs = if rng.gen_bool(0.7) || we - ws < 3 {
                let p = rng.gen_range(ws + 1..we);
                vec![h(p, p, ["q", "r", "s"][rng.gen_range(0..3)])]
            } else {
                let p = rng.gen_range(ws + 1..we - 1);
                vec![h(p, p + 1, "")]
            };
            at(&f.s, c, base, hs)
                .map_err(|e| format!("P2: bounced in a shared word: {}", e.plain()))?;
            edits += 1;
        }
    }
    Ok(edits)
}

#[test]
fn same_multi_author_word_never_bounces() {
    let cases = env("LIVE_WORD_CASES", 128);
    let mut edits = 0;
    for i in 0..cases {
        edits += word_case(7_000_000 + i).unwrap_or_else(|e| panic!("seed {}: {e}", 7_000_000 + i));
    }
    eprintln!(
        "RUNREC {{\"job\":null,\"run\":\"live_shared_word\",\"tool\":\"live_edit\",\"status\":\"COMPLETE\",\"cases\":{cases},\"live_edits\":{edits}}}"
    );
}

fn env(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn run(cases: u64, seed: u64, stream: Stream, threads: u64) -> Counts {
    let next = Arc::new(AtomicU64::new(0));
    let total = Arc::new(Mutex::new(Counts::default()));
    let steps = env("LIVE_STEPS", 60);
    let ws: Vec<_> = (0..threads.max(1))
        .map(|_| {
            let (next, total) = (next.clone(), total.clone());
            std::thread::spawn(move || loop {
                let i = next.fetch_add(1, Ordering::SeqCst);
                if i >= cases {
                    break;
                }
                match one(seed + i, steps, stream, false, true) {
                    Ok(n) => total.lock().unwrap().add(&n),
                    Err(why) => panic!("seed {}: {why}", seed + i),
                }
            })
        })
        .collect();
    for w in ws {
        w.join().unwrap();
    }
    let n = total.lock().unwrap().clone();
    n
}

/// P1–P4 over random histories of 3 clients and the other channels (module docs).
#[test]
fn live_clients_converge() {
    let cases = env("LIVE_CASES", 128);
    let seed = env("LIVE_SEED", 0);
    let threads = env("LIVE_THREADS", 1);
    let t0 = std::time::Instant::now();
    let n = run(cases, seed, Stream::Mixed, threads);
    eprintln!("[live] mixed: {n:?}");
    assert_eq!(n.cases, cases);
    // that the run exercised what it checks (a large enough run)
    if cases >= 32 {
        assert!(
            n.refused > 0 && n.accepted > n.refused && n.rebased_through > 0,
            "{n:?}"
        );
        assert!(n.tokens_checked > 0 && n.others > 0, "{n:?}");
    }
    let ins = run(
        cases / 4 + 1,
        seed + 1_000_000,
        Stream::InsertsOnly,
        threads,
    );
    let del = run(
        cases / 4 + 1,
        seed + 2_000_000,
        Stream::DeletesOnly,
        threads,
    );
    // r2.3's classes (P2, P2c by intent): mixed edits at overlapping ranges
    let ovl = run(cases, seed + 3_000_000, Stream::Overlapping, threads);
    eprintln!(
        "[live] inserts only: {ins:?}\n[live] deletes only: {del:?}\n[live] overlapping: {ovl:?}"
    );
    assert_eq!(ins.refused + del.refused, 0);
    if cases >= 32 {
        assert!(
            ovl.id_conflicts > 0
                && ovl.refused > 0
                && ovl.del_over_deleted > 0
                && ovl.split_replacement > 0
                && ovl.touch_joined > 0,
            "r2.3's classes not all reached: {ovl:?}"
        );
    }
    eprintln!(
        "RUNREC {{\"job\":null,\"run\":\"live_converge\",\"tool\":\"live_edit\",\"status\":\"COMPLETE\",\"cases\":{},\"live_edits\":{},\"accepted\":{},\"refused\":{},\"noops\":{},\"rebased_through\":{},\"others\":{},\"tokens_checked\":{},\"p2_cases\":{},\"p2_edits\":{},\"ovl_cases\":{},\"ovl_live_edits\":{},\"ovl_judged\":{},\"ovl_id_conflicts\":{},\"ovl_refused\":{},\"ovl_del_over_deleted\":{},\"ovl_split_replacement\":{},\"ovl_touch_joined\":{},\"mixed_judged\":{},\"mixed_id_conflicts\":{},\"seed\":{seed},\"wall_s\":{}}}",
        n.cases,
        n.live_edits,
        n.accepted,
        n.refused,
        n.noops,
        n.rebased_through,
        n.others,
        n.tokens_checked,
        ins.cases + del.cases,
        ins.live_edits + del.live_edits,
        ovl.cases,
        ovl.live_edits,
        ovl.p2_judged,
        ovl.id_conflicts,
        ovl.refused,
        ovl.del_over_deleted,
        ovl.split_replacement,
        ovl.touch_joined,
        n.p2_judged,
        n.id_conflicts,
        t0.elapsed().as_secs()
    );
}

/// The six mutants of amendment E-LIVE r2, r2.1's window-holds-expanded-hunks and r2.3's
/// two rule-(1) variants, each caught by the property within
/// `LIVE_MUTANT_CASES` histories (default 64).
#[test]
fn live_mutants_are_caught() {
    let cases = env("LIVE_MUTANT_CASES", 64);
    let steps = env("LIVE_STEPS", 60);
    let all = [
        (
            LiveMutant::TieLeftForOurs,
            "tie_left_for_ours",
            Stream::InsertsOnly,
        ),
        (
            LiveMutant::DeleteSwallowsInsert,
            "delete_swallows_insert",
            Stream::Mixed,
        ),
        (LiveMutant::O5ForLive, "o5_for_live", Stream::InsertsOnly),
        (
            LiveMutant::ReplacementTextPlacedRight,
            "replacement_text_placed_right",
            Stream::Mixed,
        ),
        (
            LiveMutant::DeleteDeleteConflicts,
            "delete_delete_conflicts",
            Stream::DeletesOnly,
        ),
        (
            LiveMutant::OwnChangeInWindow,
            "own_change_in_window",
            Stream::Mixed,
        ),
        (
            LiveMutant::WindowHoldsExpandedHunks,
            "window_holds_expanded_hunks",
            Stream::InsertsOnly,
        ),
        // r2.3: rule (1) on the server only must fail P1 (judged without the ID oracle, so
        // the catch is the client's and the server's disagreement); on both sides, P2 or P2c
        (
            LiveMutant::RecanonServerOnly,
            "recanon_server_only",
            Stream::Overlapping,
        ),
        (
            LiveMutant::RecanonBothSides,
            "recanon_both_sides",
            Stream::Overlapping,
        ),
        // r2.3 S3: the right placement alone (P2, by the ID oracle's both placements)
        (
            LiveMutant::InsideByTieBreak,
            "inside_by_tie_break",
            Stream::Overlapping,
        ),
    ];
    for (m, name, stream) in all {
        let own = m == LiveMutant::OwnChangeInWindow;
        // F1's mutant is caught by its own case class
        let words = m == LiveMutant::WindowHoldsExpandedHunks;
        let p2_oracle = m != LiveMutant::RecanonServerOnly;
        let run_one = |i: u64, own: bool| -> Result<(), String> {
            if words {
                word_case(7_000_000 + i).map(|_| ())
            } else {
                one(5_000_000 + i, steps, stream, own, p2_oracle).map(|_| ())
            }
        };
        let mut caught = None;
        for i in 0..cases {
            let r = with_live_mutant(m, || {
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_one(i, own)))
            });
            let failed = match r {
                Ok(Ok(_)) => None,
                Ok(Err(why)) => Some(why),
                Err(_) => Some("a panic".to_owned()),
            };
            if let Some(why) = failed {
                caught = Some((i + 1, why));
                break;
            }
        }
        let (k, why) = caught.unwrap_or_else(|| panic!("{name}: never caught in {cases}"));
        // the same history without the mutant passes: the catch is the mutant's
        if let Err(e) = run_one(k - 1, false) {
            panic!("{name}: history {k} fails without the mutant too: {e}");
        }
        if m == LiveMutant::RecanonServerOnly {
            // one-sided rule (1): the client's prediction and the server part (P1)
            assert!(
                why.starts_with("P1") || why.contains("predicted"),
                "{name}: caught, but not by P1: {why}"
            );
        }
        eprintln!("[live mutant {name}] caught in history {k}: {why}");
        eprintln!(
            "RUNREC {{\"job\":null,\"run\":\"live_mutant_{name}\",\"tool\":\"live_edit\",\"status\":\"CAUGHT\",\"histories\":{k}}}"
        );
    }
}

// ---------- shared vectors for the JS client, and the trace hook ----------

fn js(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn hunks_json(hs: &[Hunk]) -> String {
    let v: Vec<String> = hs
        .iter()
        .map(|x| format!("[{},{},{}]", x.start, x.end, js(&x.insert)))
        .collect();
    format!("[{}]", v.join(","))
}

/// The vectors: hand-picked cases for each rule (the client-mirror tie and the replacement
/// placement among them), then seeded random ones. Each: the base text, ours, the changes
/// since (server order), the server's result (or the conflict), and the client mirror's
/// result for the first change carried through ours.
type Case = (&'static str, Vec<Hunk>, Vec<Vec<Hunk>>);

fn vectors() -> String {
    let hand: Vec<Case> = vec![
        ("ab", vec![h(1, 1, "X")], vec![vec![h(1, 1, "Y")]]),
        ("abcd", vec![h(1, 1, "X")], vec![vec![h(1, 3, "Y")]]),
        ("abcdef", vec![h(1, 4, "")], vec![vec![h(2, 5, "")]]),
        ("abcdef", vec![h(1, 4, "X")], vec![vec![h(2, 5, "")]]),
        ("abcde", vec![h(2, 2, "X")], vec![vec![h(1, 4, "")]]),
        ("abcdef", vec![h(1, 5, "")], vec![vec![h(3, 3, "XY")]]),
        ("abcdef", vec![h(1, 5, "Z")], vec![vec![h(3, 3, "XY")]]),
        ("aé日b", vec![h(1, 6, "")], vec![vec![h(3, 3, "Z")]]),
        (
            "hello world",
            vec![h(5, 5, ",")],
            vec![vec![h(0, 0, ">> ")], vec![h(9, 14, "there")]],
        ),
    ];
    let mut cases: Vec<(String, Vec<Hunk>, Vec<Vec<Hunk>>)> = hand
        .into_iter()
        .map(|(b, o, s)| (b.to_owned(), o, s))
        .collect();
    let mut rng = SmallRng::seed_from_u64(42);
    let tok = AtomicU64::new(0);
    for _ in 0..200 {
        let base: String = (0..rng.gen_range(0..10))
            .map(|_| BITS[rng.gen_range(0..BITS.len())])
            .collect();
        let ours = random_hunks(&mut rng, &base, Stream::Mixed, &tok);
        let mut since = Vec::new();
        let mut t = base.clone();
        for _ in 0..rng.gen_range(1..3) {
            let c = random_hunks(&mut rng, &t, Stream::Mixed, &tok);
            t = apply(&t, &c);
            since.push(c);
        }
        cases.push((base, ours, since));
    }
    // r2.3 (appended, so the cases above keep their numbers): K2 Engine's P2 and P2c
    // counterexamples to r2.2's rule (1), an insertion at each edge of a deleted range (S3),
    // and a change fully absorbed (S4: an empty result)
    let tail: Vec<Case> = vec![
        (
            "ab ",
            vec![h(1, 1, "C"), h(2, 3, "")],
            vec![vec![h(1, 2, "A")], vec![h(2, 3, "B")]],
        ),
        (
            "ab ",
            vec![h(0, 2, "B")],
            vec![vec![h(1, 1, "C")], vec![h(2, 3, "D")]],
        ),
        ("abcde", vec![h(1, 1, "X")], vec![vec![h(1, 4, "")]]),
        ("abcde", vec![h(4, 4, "X")], vec![vec![h(1, 4, "")]]),
        ("abcde", vec![h(1, 3, "")], vec![vec![h(0, 4, "")]]),
        // r2.3 S3 (214): inside under the right placement only (ours went after `B`): no
        // conflict
        (
            "ab ",
            vec![h(0, 0, "A")],
            vec![vec![h(0, 0, "B")], vec![h(0, 2, "")]],
        ),
    ];
    cases.extend(tail.into_iter().map(|(b, o, s)| (b.to_owned(), o, s)));
    let mut out = String::from(
        "{\"about\":\"k2-mpc-base live-edit rebase vectors (E-LIVE r2.3: each hunk carried as sent): server = rebase_ours(ours, since); mirror = rebase_incoming(since[0], ours). Hunks are [start, end, insert] in UTF-8 bytes.\",\"cases\":[\n",
    );
    for (k, (base, ours, since)) in cases.iter().enumerate() {
        let server = match rebase_ours(ours, since) {
            Ok(r) => format!("{{\"ok\":{}}}", hunks_json(&r)),
            Err(c) => format!(
                "{{\"conflict\":{{\"ours\":{},\"step\":{},\"why\":{}}}}}",
                c.ours,
                c.step,
                js(c.why)
            ),
        };
        let mirror = hunks_json(&rebase_incoming(&since[0], ours));
        let since_j: Vec<String> = since.iter().map(|s| hunks_json(s)).collect();
        out.push_str(&format!(
            "{{\"base\":{},\"ours\":{},\"since\":[{}],\"server\":{server},\"mirror\":{mirror}}}{}\n",
            js(base),
            hunks_json(ours),
            since_j.join(","),
            if k + 1 < cases.len() { "," } else { "" }
        ));
    }
    out.push_str("]}\n");
    out
}

/// The shared vectors are what the rules say now: `LIVE_VECTORS_WRITE=1` writes them; a
/// stale file fails here.
#[test]
fn shared_vectors_are_current() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/vectors/live_rebase.json");
    let now = vectors();
    if std::env::var("LIVE_VECTORS_WRITE").is_ok() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &now).unwrap();
    }
    let kept = std::fs::read_to_string(&path)
        .expect("tests/vectors/live_rebase.json: write it with LIVE_VECTORS_WRITE=1");
    assert!(
        kept == now,
        "the live rebase vectors are stale: regenerate with LIVE_VECTORS_WRITE=1"
    );
}

/// The trace hook (K2 Engine's exhaustive model): `LIVE_TRACES=<file>` holds cases in the
/// vectors' format (one JSON object per line, with `base`, `ours`, `since`, the expected
/// `server` and `mirror`); each is replayed through the rebase here: the server's result
/// (the hunks, or the conflict's ours and step) and the mirror's (`rebase_incoming(since[0],
/// ours)`, when there is a change since). Every disagreement is counted and the first ones
/// shown; there must be none. Without the variable it checks the vectors file itself.
#[test]
fn trace_hook_replays_cases() {
    let path = std::env::var("LIVE_TRACES")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/vectors/live_rebase.json")
        });
    let body = std::fs::read_to_string(&path).unwrap();
    let (mut n, mut server_bad, mut mirror_bad) = (0, Vec::new(), Vec::new());
    // a file exported under r2.2 (rule (1), re-canonicalize at every step): a disagreement
    // the kit doesn't have with rule (1) planted is rule (1)'s alone
    let r22 = std::env::var("LIVE_TRACES_R22").is_ok();
    let mut rule1 = 0;
    for (k, line) in body
        .lines()
        .filter(|l| l.starts_with("{\"base\""))
        .enumerate()
    {
        let v = json::parse(line.trim_end_matches(','));
        let base = v.get("base").str();
        let ours = v.get("ours").hunks();
        let since: Vec<Vec<Hunk>> = v.get("since").list().iter().map(|x| x.hunks()).collect();
        let expect = v.get("server");
        let agrees = || match rebase_ours(&ours, &since) {
            Ok(r) => {
                let mut t = base.to_owned();
                for c in &since {
                    t = apply(&t, c);
                }
                apply(&t, &r);
                expect.has("ok") && r == expect.get("ok").hunks()
            }
            Err(c) => {
                expect.has("conflict") && {
                    let e = expect.get("conflict");
                    (c.ours, c.step) == (e.get("ours").num(), e.get("step").num())
                }
            }
        };
        if !agrees() {
            if r22 && with_live_mutant(LiveMutant::RecanonBothSides, agrees) {
                rule1 += 1;
            } else {
                server_bad.push(k);
            }
        }
        if let Some(first) = since.first() {
            if rebase_incoming(first, &ours) != v.get("mirror").hunks() {
                mirror_bad.push(k);
            }
        }
        n += 1;
    }
    assert!(n > 0, "no cases in {}", path.display());
    let lines: Vec<&str> = body
        .lines()
        .filter(|l| l.starts_with("{\"base\""))
        .collect();
    for &k in server_bad.iter().chain(&mirror_bad).take(6) {
        eprintln!("[trace hook] case {k}: {}", lines[k]);
    }
    eprintln!(
        "RUNREC {{\"job\":null,\"run\":\"live_trace_hook_{}\",\"tool\":\"live_edit\",\"status\":\"COMPLETE\",\"file\":{},\"cases\":{n},\"server_disagree\":{},\"mirror_disagree\":{},\"r22_rule1_only\":{rule1}}}",
        // the run is named after the file: `live_trace_hook_<stem>`
        path.file_stem()
            .unwrap()
            .to_string_lossy()
            .replace(|c: char| !c.is_ascii_alphanumeric(), "_"),
        js(&path.file_name().unwrap().to_string_lossy()),
        server_bad.len(),
        mirror_bad.len()
    );
    assert!(
        server_bad.is_empty() && mirror_bad.is_empty(),
        "{}: {} server and {} mirror disagreements of {n} (cases {:?} …)",
        path.display(),
        server_bad.len(),
        mirror_bad.len(),
        server_bad
            .iter()
            .chain(&mirror_bad)
            .take(10)
            .collect::<Vec<_>>()
    );
}

/// A minimal JSON reader for the hook (the kit has no serde).
mod json {
    use super::{h, Hunk};

    #[derive(Debug, Clone)]
    pub enum V {
        S(String),
        N(f64),
        L(Vec<V>),
        O(Vec<(String, V)>),
        Other,
    }

    impl V {
        pub fn get(&self, k: &str) -> &V {
            match self {
                V::O(m) => m
                    .iter()
                    .find(|(x, _)| x == k)
                    .map(|(_, v)| v)
                    .unwrap_or(&V::Other),
                _ => &V::Other,
            }
        }
        pub fn has(&self, k: &str) -> bool {
            matches!(self, V::O(m) if m.iter().any(|(x, _)| x == k))
        }
        pub fn str(&self) -> &str {
            match self {
                V::S(s) => s,
                _ => panic!("not a string: {self:?}"),
            }
        }
        pub fn num(&self) -> usize {
            match self {
                V::N(n) => *n as usize,
                _ => panic!("not a number: {self:?}"),
            }
        }
        pub fn list(&self) -> &[V] {
            match self {
                V::L(l) => l,
                _ => panic!("not a list: {self:?}"),
            }
        }
        pub fn hunks(&self) -> Vec<Hunk> {
            self.list()
                .iter()
                .map(|x| {
                    let l = x.list();
                    h(l[0].num(), l[1].num(), l[2].str())
                })
                .collect()
        }
    }

    pub fn parse(s: &str) -> V {
        let mut p = P {
            b: s.as_bytes(),
            i: 0,
        };
        p.value()
    }

    struct P<'a> {
        b: &'a [u8],
        i: usize,
    }

    impl P<'_> {
        fn ws(&mut self) {
            while self.i < self.b.len() && (self.b[self.i] as char).is_whitespace() {
                self.i += 1;
            }
        }
        fn value(&mut self) -> V {
            self.ws();
            match self.b[self.i] {
                b'"' => V::S(self.string()),
                b'[' => {
                    self.i += 1;
                    let mut l = Vec::new();
                    loop {
                        self.ws();
                        if self.b[self.i] == b']' {
                            self.i += 1;
                            break;
                        }
                        l.push(self.value());
                        self.ws();
                        if self.b[self.i] == b',' {
                            self.i += 1;
                        }
                    }
                    V::L(l)
                }
                b'{' => {
                    self.i += 1;
                    let mut m = Vec::new();
                    loop {
                        self.ws();
                        if self.b[self.i] == b'}' {
                            self.i += 1;
                            break;
                        }
                        let k = self.string();
                        self.ws();
                        self.i += 1; // ':'
                        m.push((k, self.value()));
                        self.ws();
                        if self.b[self.i] == b',' {
                            self.i += 1;
                        }
                    }
                    V::O(m)
                }
                _ => {
                    let st = self.i;
                    while self.i < self.b.len() && !b",]} ".contains(&self.b[self.i]) {
                        self.i += 1;
                    }
                    let t = std::str::from_utf8(&self.b[st..self.i]).unwrap();
                    t.parse().map(V::N).unwrap_or(V::Other)
                }
            }
        }
        fn string(&mut self) -> String {
            self.i += 1;
            let mut out = Vec::new();
            loop {
                let c = self.b[self.i];
                self.i += 1;
                match c {
                    b'"' => break,
                    b'\\' => {
                        let e = self.b[self.i];
                        self.i += 1;
                        match e {
                            b'n' => out.push(b'\n'),
                            b'u' => {
                                let hex = std::str::from_utf8(&self.b[self.i..self.i + 4]).unwrap();
                                self.i += 4;
                                let ch =
                                    char::from_u32(u32::from_str_radix(hex, 16).unwrap()).unwrap();
                                let mut buf = [0u8; 4];
                                out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                            }
                            other => out.push(other),
                        }
                    }
                    other => out.push(other),
                }
            }
            String::from_utf8(out).unwrap()
        }
    }
}
