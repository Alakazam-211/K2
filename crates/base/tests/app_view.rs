//! Opaque handles for App viewers (K5): a viewer whose participant kind is `App` never sees a
//! pid. Its whole stream (the roster and its push, change events, read, blame, the log,
//! claims, reservations, activity, heads-up notes, approvals, refusals and their error
//! reports, the hunk channel's `edit_at_base` and `events_since`, `run_tool`'s output (its
//! fence, `--for-edit` claims and conflicts); for a tier-0 blob doc: the
//! roster, leases, reads, versions, the log, blame, approvals, and lease refusals) is
//! scanned for every pid in the doc, its own included; any one found fails. Handles are per
//! viewer, stable across its reconnects, and independent between viewers. No absolute path
//! reaches a viewer either (K7): a file session's disk error is shown relative to its
//! workspace. Public API only.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Mutex};

use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::session::AnyFile;
use k2_mpc_base::session::{
    At, AuditEvent, Config, ConnId, EditOptions, ForEdit, Host, LineCol, LineRange, LogQuery,
    Participant, ParticipantKind, Rights, RosterEvent, Session, SessionError, SessionManager, Stay,
    TextFilePack, Who,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::text::diff::Hunk;
use k2_mpc_base::tools::ToolOptions;

const ROOT: &str = TextFilePack::ROOT;
const DOC: &str = "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\n";

/// Pids that appear nowhere in the doc's text or the kit's own strings.
const ALICE: &str = "pid-alice-7f3a";
const AGENT: &str = "pid-agent-91c2";
const HELPER: &str = "pid-helper-c40d";
const APP: &str = "pid-app-55d1";
const APP2: &str = "pid-app-02e8";
const PIDS: [&str; 5] = [ALICE, AGENT, HELPER, APP, APP2];

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
        self.now.load(Ordering::SeqCst)
    }
    fn on_event(&self, _ev: &AuditEvent) {}
}

struct TmpDir(std::path::PathBuf);

impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Fx {
    _dir: TmpDir,
    host: Arc<TestHost>,
    mgr: SessionManager,
    s: Arc<Session>,
}

fn fx(tag: &str) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-appview-{tag}-{}-{}",
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
    let s = mgr
        .open(
            Arc::new(TextFilePack::new("markdown", 4 << 20)),
            &DocKey {
                space: "ws".into(),
                path: "notes.md".into(),
            },
            Some(DOC.as_bytes()),
        )
        .unwrap();
    Fx {
        _dir: TmpDir(dir),
        host,
        mgr,
        s,
    }
}

impl Fx {
    fn join(&self, pid: &str, name: &str, kind: ParticipantKind) -> ConnId {
        let p = Participant::new(pid, name, "#123456", kind, "tab");
        self.s.join(p, Stay::Held).unwrap().conn
    }
    fn advance(&self, ms: u64) {
        self.host.now.fetch_add(ms, Ordering::SeqCst);
    }
    fn tick(&self) {
        self.advance(300);
        self.mgr.flush_rosters();
    }
}

fn lines(first: usize, last: usize) -> LineRange {
    LineRange {
        root: ROOT.into(),
        first,
        last,
    }
}

fn lc(line: usize, col: usize) -> At {
    At::Line(LineCol { line, col })
}

fn drain<T>(rx: &Receiver<T>) -> Vec<T> {
    let mut v = Vec::new();
    loop {
        match rx.try_recv() {
            Ok(e) => v.push(e),
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => return v,
        }
    }
}

/// Everything one viewer was handed, as text.
#[derive(Default)]
struct Seen(String);

impl Seen {
    fn add(&mut self, what: &str, v: impl std::fmt::Debug) {
        self.0.push_str(&format!("\n== {what}\n{v:?}"));
    }
    /// A refusal: as the error, as its message, and as its report (plain and JSON).
    fn refusal(&mut self, what: &str, e: &SessionError) {
        self.add(what, e);
        let r = e.report();
        self.0
            .push_str(&format!("\n{e}\n{}\n{}", r.plain(), r.to_json()));
    }
    /// Fails on any pid of the doc in what was seen.
    fn assert_no_pid(&self) {
        for p in PIDS {
            if let Some(at) = self.0.find(p) {
                let from = at.saturating_sub(120);
                panic!(
                    "pid {p:?} reached an App viewer:\n...{}...",
                    &self.0[from..(at + p.len() + 40).min(self.0.len())]
                );
            }
        }
    }
}

/// The doc's other participants do things an App viewer sees: join, carets, claims, notes,
/// edits made for a person, history.
fn busy_doc(f: &Fx) -> (ConnId, ConnId, ConnId) {
    let alice = f.join(ALICE, "Alice", ParticipantKind::Person);
    let agent = f.join(AGENT, "Builder", ParticipantKind::Agent);
    let helper = {
        let p = Participant::new(HELPER, "Helper", "#654321", ParticipantKind::Agent, "cli")
            .on_behalf_of(ALICE);
        f.s.join(p, Stay::Held).unwrap().conn
    };
    f.s.set_cursor(alice, ROOT, lc(2, 1), lc(2, 3)).unwrap();
    f.s.replace_text(alice, ROOT, &DOC.replace("one\n", "One\n"))
        .unwrap();
    f.s.replace_text(
        helper,
        ROOT,
        &DOC.replace("one\n", "One\n").replace("ten\n", "TEN\n"),
    )
    .unwrap();
    f.s.claim(agent, lines(4, 5), None, Some("rewriting".into()), false)
        .unwrap();
    (alice, agent, helper)
}

// ---------- tests ----------

/// The whole stream an App viewer gets holds no pid (its own included); a person viewer of
/// the same doc sees pids (the scan is not vacuous).
#[test]
fn an_app_viewers_whole_stream_holds_no_pid() {
    let f = fx("scan");
    let app = f.join(APP, "Guest", ParticipantKind::App);
    let roster_rx = f.s.subscribe_roster(app).unwrap();
    let change_rx = f.s.subscribe(app).unwrap();
    let v0 = f.s.read(app).unwrap().version;
    let (alice, agent, _helper) = busy_doc(&f);
    f.join(APP2, "Other guest", ParticipantKind::App);
    f.tick();

    let mut seen = Seen::default();
    seen.add("roster", f.s.roster(app).unwrap());
    seen.add("read", f.s.read(app).unwrap());
    seen.add("blame", f.s.blame(app, ROOT, None).unwrap());
    seen.add("log", f.s.log(app, &LogQuery::default()).unwrap());
    seen.add("claims", f.s.claims(app).unwrap());
    seen.add("reservations", f.s.reservations(app).unwrap());
    seen.add("activity", f.s.activity(app, 100).unwrap());
    let approvals = f.s.approvals(app).unwrap();
    assert!(
        !approvals.is_empty(),
        "the helper's change for Alice waits for her approval"
    );
    seen.add("approvals", &approvals);
    seen.add("approval", f.s.approval(app, approvals[0].0).unwrap());
    // a claim over the agent's: the refusal names its holder
    let e =
        f.s.claim(app, lines(4, 4), None, None, false)
            .expect_err("claimed");
    assert!(matches!(e, SessionError::Claimed { .. }), "{e:?}");
    seen.refusal("claim refused", &e);
    // the hunk channel (v0.9.7): a live edit at the import's version that competes with
    // alice's change (MPC-E111 names the change met and its author), one that applies, and
    // the events since that version (authors and on-behalf-of)
    let hunk = |start, end, insert: &str| Hunk {
        start,
        end,
        insert: insert.into(),
    };
    let e =
        f.s.edit_at_base(app, ROOT, v0, vec![hunk(0, 3, "uno")], &EditOptions::new())
            .expect_err("competes with alice's change");
    assert_eq!(e.id(), "MPC-E111");
    assert!(e.to_json().contains(" in change "), "{}", e.to_json());
    seen.add("edit_at_base refused", &e);
    seen.0
        .push_str(&format!("\n{}\n{}", e.plain(), e.to_json()));
    let ok =
        f.s.edit_at_base(app, ROOT, v0, vec![hunk(4, 4, "2\n")], &EditOptions::new())
            .unwrap();
    seen.add("edit_at_base", &ok);
    let since = f.s.events_since(app, v0).unwrap();
    assert!(since.len() >= 3, "{since:?}");
    assert!(since.iter().any(|e| e.on_behalf_of.is_some()), "{since:?}");
    seen.add("events_since", &since);
    // a read over the claimed and caret lines: heads-up notes
    f.advance(8_000);
    let argv: Vec<String> = ["view", "--lines", "1,6"]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    let out =
        f.s.run_tool_with(app, ROOT, &argv, b"", &ToolOptions::new())
            .unwrap();
    assert!(!out.notes.notes.is_empty(), "the view got notes");
    seen.add("tool notes", &out.notes);
    seen.0.push_str(&out.notes.to_string());
    // a write over a stale base that conflicts: who claimed or edited each region
    let report =
        f.s.write_base(app, ROOT, 1, &DOC.replace("four\n", "FOUR!\n"))
            .unwrap();
    seen.add("write --base", &report);
    // its own edit: change events, then the log again
    f.s.replace_text(
        app,
        ROOT,
        &f.s.read(app).unwrap().text_by_root[0]
            .1
            .replace("nine", "9"),
    )
    .unwrap();
    // the others leave; the agent's claim ends
    f.s.leave(agent).unwrap();
    f.s.leave(alice).unwrap();
    f.tick();
    seen.add("log after", f.s.log(app, &LogQuery::default()).unwrap());
    let roster_events = drain(&roster_rx);
    assert!(roster_events.len() > 5, "{roster_events:?}");
    seen.add("roster push", &roster_events);
    let changes = drain(&change_rx);
    assert!(changes.len() >= 3, "{changes:?}");
    seen.add("change events", &changes);
    seen.assert_no_pid();

    // handles are there instead
    assert!(roster_events.iter().any(|e| matches!(
        e,
        RosterEvent::Snapshot { members, .. } if members.iter().all(|m| matches!(m.who, Who::Handle(_)))
    )));
    assert!(
        changes.iter().all(|c| c.pid.starts_with('p')),
        "{changes:?}"
    );

    // the same doc through a person's eyes names pids
    let person = f.join("pid-person-viewer", "Viewer", ParticipantKind::Person);
    let mut plain = Seen::default();
    plain.add("blame", f.s.blame(person, ROOT, None).unwrap());
    plain.add("log", f.s.log(person, &LogQuery::default()).unwrap());
    assert!(
        plain.0.contains(ALICE) && plain.0.contains(HELPER),
        "{}",
        plain.0
    );
}

/// One viewer sees the same handle for the same pid across its reconnects; handles of two
/// viewers are independent (numbered in each one's order of first sight).
#[test]
fn handles_are_stable_per_viewer_and_independent_between_viewers() {
    let f = fx("stable");
    let alice = f.join(ALICE, "Alice", ParticipantKind::Person);
    let agent = f.join(AGENT, "Builder", ParticipantKind::Agent);
    f.s.claim(agent, lines(2, 2), None, None, false).unwrap();
    f.s.set_cursor(alice, ROOT, lc(1, 1), lc(1, 1)).unwrap();

    // the first App sees the roster first: Alice (an editor) is p1
    let app = f.join(APP, "Guest", ParticipantKind::App);
    let by_name = |rows: &[k2_mpc_base::session::RosterEntry]| -> HashMap<String, String> {
        rows.iter()
            .map(|e| (e.participant.name.clone(), e.participant.pid.clone()))
            .collect()
    };
    let first = by_name(&f.s.roster(app).unwrap());
    assert_eq!(first["Alice"], "p1");
    // the second App sees the agent first (claims), so its p1 is the agent
    let app2 = f.join(APP2, "Other guest", ParticipantKind::App);
    let claims = f.s.claims(app2).unwrap();
    assert_eq!(claims[0].pid, "p1");
    assert_eq!(by_name(&f.s.roster(app2).unwrap())["Builder"], "p1");
    assert_ne!(first["Builder"], "p1");

    // reconnecting: the same handles
    f.s.leave(app).unwrap();
    let again = f.join(APP, "Guest", ParticipantKind::App);
    let rejoined = by_name(&f.s.roster(again).unwrap());
    for (name, h) in &first {
        assert_eq!(&rejoined[name], h, "{name} after a reconnect");
    }

    // an App filters the log by its handles; a raw pid names nobody
    f.s.replace_text(alice, ROOT, &DOC.replace("two", "2"))
        .unwrap();
    let alice_h = first["Alice"].clone();
    let by_handle =
        f.s.log(again, &LogQuery::default().by(alice_h.clone()))
            .unwrap();
    assert!(!by_handle.is_empty());
    let by_pid = f.s.log(again, &LogQuery::default().by(ALICE)).unwrap();
    assert!(
        by_pid.is_empty(),
        "a raw pid must not match for an App: {by_pid:?}"
    );
}

/// A tier-0 blob doc: the roster, the lease, reads, versions, the log, blame, approvals and a
/// lease refusal, as an App viewer gets them, hold no pid.
#[test]
fn an_app_viewer_of_a_blob_doc_sees_no_pid() {
    let f = fx("blob");
    let key = DocKey {
        space: "ws".into(),
        path: "deck.pdf".into(),
    };
    let s = f
        .mgr
        .open_blob(Arc::new(AnyFile), &key, Some(&mut &b"pdf-1"[..]))
        .unwrap();
    let join = |p: Participant| s.join(p, Stay::Held).unwrap().conn;
    let alice = join(Participant::new(
        ALICE,
        "Alice",
        "#1",
        ParticipantKind::Person,
        "tab",
    ));
    let helper = join(
        Participant::new(HELPER, "Helper", "#2", ParticipantKind::Agent, "cli").on_behalf_of(ALICE),
    );
    let app = join(Participant::new(
        APP,
        "Guest",
        "#3",
        ParticipantKind::App,
        "app",
    ));
    let v1 = s.status().unwrap().version;
    let w = s.write_bytes(helper, b"pdf-2 by the helper", v1).unwrap();
    s.acquire_lease(alice, None).unwrap();

    let mut seen = Seen::default();
    seen.add("roster", s.roster(app).unwrap());
    let r = s.read(app).unwrap();
    seen.add("read", (&r.version, &r.info));
    seen.add("versions", s.versions(app).unwrap());
    seen.add("log", s.log(app).unwrap());
    seen.add("blame", s.blame(app).unwrap());
    let approvals = s.approvals(app).unwrap();
    assert!(
        !approvals.is_empty(),
        "the helper's version waits for Alice"
    );
    seen.add("approvals", &approvals);
    seen.add("approval", s.approval(app, w.version).unwrap());
    // Alice holds the lease: the App's lease and write are refused, naming the holder
    let e = s.acquire_lease(app, None).expect_err("leased");
    assert!(matches!(e, SessionError::Leased { .. }), "{e:?}");
    seen.refusal("lease refused", &e);
    let e = s.write_bytes(app, b"pdf-3", w.version).expect_err("leased");
    assert!(matches!(e, SessionError::Leased { .. }), "{e:?}");
    seen.refusal("write refused", &e);
    // free again: the App's own lease
    s.release_lease(alice).unwrap();
    seen.add("lease", s.acquire_lease(app, None).unwrap());
    seen.assert_no_pid();

    // a person's view of the same doc names pids
    let person = join(Participant::new(
        "pid-person-viewer",
        "Viewer",
        "#4",
        ParticipantKind::Person,
        "tab",
    ));
    let mut plain = Seen::default();
    plain.add("log", s.log(person).unwrap());
    assert!(plain.0.contains(HELPER), "{}", plain.0);
}

/// K7 (v0.9.8): no absolute path reaches a viewer. A file session whose write-back fails
/// (the file's name is so long that the write's temp file can't be named, which fails for
/// every user) refuses the App's `flush` with a disk error that names the file relative to
/// the workspace: the workspace's absolute path appears nowhere in the refusal, its message,
/// its report or its JSON.
#[test]
fn no_absolute_path_reaches_an_app_viewer() {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-appview-paths-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let ws = dir.join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    let _guard = TmpDir(dir.clone());
    let name = format!("{}.md", "n".repeat(240));
    let file = ws.join(&name);
    std::fs::write(&file, DOC).unwrap();
    let host = Arc::new(TestHost {
        now: AtomicU64::new(1_000_000),
        rights: Mutex::new(HashMap::new()),
    });
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let mgr = SessionManager::new(store, host, Config::default());
    let key = DocKey {
        space: "ws".into(),
        path: name.clone(),
    };
    let s = mgr
        .open_file(
            Arc::new(TextFilePack::new("markdown", 4 << 20)),
            &key,
            &file,
            &ws,
        )
        .unwrap();
    let join = |pid: &str, kind| {
        s.join(
            Participant::new(pid, "X", "#123456", kind, "tab"),
            Stay::Held,
        )
        .unwrap()
        .conn
    };
    let app = join(APP, ParticipantKind::App);
    let alice = join(ALICE, ParticipantKind::Person);
    s.replace_text(alice, ROOT, &DOC.replace("two\n", "Two\n"))
        .unwrap();
    let e = s
        .flush(app)
        .expect_err("the write's temp file can't be named");
    assert!(matches!(e, SessionError::Disk(_)), "{e:?}");
    let mut seen = Seen::default();
    seen.refusal("flush", &e);
    seen.add("read", s.read(app).unwrap());
    let canon = std::fs::canonicalize(&dir).unwrap();
    for abs in [dir.to_string_lossy(), canon.to_string_lossy()] {
        assert!(
            !seen.0.contains(abs.as_ref()),
            "an absolute path reached the App: {}",
            seen.0
        );
    }
    // the file is named, relative to the workspace: the error had the path, redacted
    assert!(e.to_string().contains(&name[..40]), "{e}");
    assert!(!e.to_string().contains("<outside>"), "{e}");
    seen.assert_no_pid();
}

/// What `run_tool` hands an App viewer holds no pid (its own included): a read's fence
/// (contributors since its last read), a `--for-edit` conflict with someone's claim, and its
/// own `--for-edit` claim; a person's same reads name pids (the scan is not vacuous).
#[test]
fn an_app_viewers_tool_output_holds_no_pid() {
    let f = fx("tool");
    let app = f.join(APP, "Guest", ParticipantKind::App);
    // its last read, before the others change anything
    f.s.read(app).unwrap();
    busy_doc(&f);
    let argv = |a: &str| -> Vec<String> { a.split(' ').map(str::to_owned).collect() };
    let mut seen = Seen::default();
    let mut tool = |what: &str, a: &str| {
        let o =
            f.s.run_tool_with(app, ROOT, &argv(a), b"", &ToolOptions::new())
                .unwrap();
        seen.add(what, &o);
        if let Some(fe) = &o.for_edit {
            seen.0.push_str(&format!("\n{fe}"));
        }
        o
    };
    // the fence names who changed the doc since its last read: alice and her helper
    let o = tool("cat", "cat");
    let fence = o.fence.expect("a read's fence");
    assert_eq!(fence.contributors.len(), 2, "{fence:?}");
    assert!(
        fence.contributors.iter().all(|c| c
            .strip_prefix('p')
            .is_some_and(|n| n.parse::<u32>().is_ok())),
        "handles: {fence:?}"
    );
    // lines the agent claimed: a conflict naming the claim's holder
    let o = tool("view --for-edit (claimed)", "view --lines 4,5 --for-edit");
    assert!(
        matches!(o.for_edit.as_deref(), Some(ForEdit::Conflict(_))),
        "{o:?}"
    );
    // lines nobody holds: its own claim
    let o = tool("view --for-edit (its own)", "view --lines 8,9 --for-edit");
    assert!(
        matches!(o.for_edit.as_deref(), Some(ForEdit::Claimed(_))),
        "{o:?}"
    );
    seen.assert_no_pid();

    // a person's same reads name pids
    let person = f.join("pid-person-viewer", "Viewer", ParticipantKind::Person);
    let o =
        f.s.run_tool_with(
            person,
            ROOT,
            &argv("view --lines 4,5 --for-edit"),
            b"",
            &ToolOptions::new(),
        )
        .unwrap();
    assert!(format!("{o:?}").contains(AGENT), "{o:?}");
}
