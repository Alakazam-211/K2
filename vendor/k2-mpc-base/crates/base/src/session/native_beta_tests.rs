//! The native engine (a Beta) in sessions: a host chooses the engine when it makes a doc, the
//! Beta switch gates that choice (`MPC-E167`), and a doc on the native engine is one replica,
//! the host's, that people and agents edit together through the session's verbs, as on Yrs.
//!
//! Every row that has a Yrs twin runs the same body on both engines (the columns, [`cols`]),
//! so the native engine can't lose a behaviour unnoticed; a row is green only when each column
//! exercised its steps (the counts it asserts). The rows that need the engine's single-replica
//! mode (S at the head, the cut) are `#[ignore]`d with that reason.
//!
//! Plants (`MPC_BETA_PLANT`, `crate::beta_plant`), one per run, each caught at its row's assert:
//! `beta-gate-open`, `create-ignores-engine`, `native-commit-drops-writers`,
//! `replica-stores-engine-state`, `text-cap-off`, `beta-limits-changed`, `sync-path-open`,
//! `yjs-decoded-as-native`, `copy-via-yrs`.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use super::*;
use crate::doc::Edit;
use crate::store::{DocKey, DocStore, EntryBody};
use crate::text::diff::Hunk;
use crate::tools::session_exit;

struct H {
    now: AtomicU64,
}

impl Host for H {
    fn rights(&self, who: &Participant, _: &DocKey) -> Rights {
        Rights::full().with_owner(who.pid == "own")
    }
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn on_event(&self, _: &AuditEvent) {}
}

/// The text pack, with a file limit above the native engine's live-text cap, so the cap (not
/// the pack) is what a native doc meets.
const MD: TextFilePack = TextFilePack::new("markdown", 16 << 20);
const ROOT: &str = TextFilePack::ROOT;

/// An engine column: the same row on each engine a doc can be made on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Col {
    Yrs,
}

/// Every column this build has: Yrs, and with feature `native` the native engine.
fn cols() -> Vec<Col> {
    let yrs = [Col::Yrs];
    yrs.to_vec()
}

impl Col {
    fn opts(self) -> OpenOptions {
        OpenOptions::new()
    }

    fn native(self) -> bool {
        self != Col::Yrs
    }
}

struct Fx {
    dir: PathBuf,
    store: Arc<DocStore>,
    host: Arc<H>,
    mgr: SessionManager,
    config: Config,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = self.mgr.release();
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn config(beta: bool) -> Config {
    Config {
        edit_rate_per_s: 1_000_000,
        edit_burst: 1_000_000,
        idle_leave_ms: u64::MAX / 4,
        native_beta: beta,
        ..Config::default()
    }
}

fn manager(store: &Arc<DocStore>, host: &Arc<H>, c: Config) -> SessionManager {
    let m = SessionManager::new(store.clone(), host.clone(), c);
    m
}

fn fx(tag: &str, c: Config) -> Fx {
    fx_with(tag, c, false)
}

/// [`fx`] whose manager has a blob store (tier 0: demote and promote).
fn fx_blobs(tag: &str, c: Config) -> Fx {
    fx_with(tag, c, true)
}

fn fx_with(tag: &str, c: Config, blobs: bool) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-beta-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("ws")).unwrap();
    let dir = fs::canonicalize(dir).unwrap();
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let host = Arc::new(H {
        now: AtomicU64::new(1_000_000),
    });
    let mut mgr = manager(&store, &host, c.clone());
    if blobs {
        let h = host.clone();
        let b = crate::blob::BlobStore::open(
            store.clone(),
            dir.join("blobs"),
            crate::blob::BlobConfig::default(),
            Arc::new(move || h.now_ms()),
        )
        .unwrap();
        mgr = mgr.with_blob_store(Arc::new(b));
    }
    Fx {
        dir,
        store,
        host,
        mgr,
        config: c,
    }
}

impl Fx {
    /// Close every session and open the store with a new manager under `c` (a restart).
    fn restart(&mut self, c: Config) {
        let _ = self.mgr.release();
        self.mgr = manager(&self.store, &self.host, c.clone());
        self.config = c;
    }

    fn advance(&self, ms: u64) {
        self.host.now.fetch_add(ms, Ordering::SeqCst);
    }

    fn id(&self, k: &DocKey) -> u64 {
        self.store.find(k).unwrap().expect("the doc is stored")
    }

    fn engine_of(&self, k: &DocKey) -> String {
        self.store.meta(self.id(k)).unwrap().engine
    }

    fn file(&self, name: &str) -> PathBuf {
        self.dir.join("ws").join(name)
    }

    fn open_file(&self, k: &DocKey, name: &str, opts: &OpenOptions) -> Arc<Session> {
        self.mgr
            .open_file_with(
                Arc::new(MD),
                k,
                &self.file(name),
                &self.dir.join("ws"),
                opts,
            )
            .unwrap()
    }

    /// Make doc `k` from `bytes` on `col`'s engine; the doc is stamped with it and runs on it.
    fn make(&self, k: &DocKey, bytes: &[u8], col: Col) -> Arc<Session> {
        let s = self
            .mgr
            .open_with(Arc::new(MD), k, Some(bytes), &col.opts())
            .unwrap();
        self.made_on(k, &s, col);
        s
    }

    /// Make doc `k` from file `name` on `col`'s engine, as [`Fx::make`].
    fn make_file(&self, k: &DocKey, name: &str, col: Col) -> Arc<Session> {
        let s = self.open_file(k, name, &col.opts());
        self.made_on(k, &s, col);
        s
    }

    fn made_on(&self, k: &DocKey, s: &Session, col: Col) {
        assert_eq!(
            family_of(&self.engine_of(k)),
            family(col),
            "row {col:?}: the doc is stamped with the engine asked for at its create"
        );
        assert_eq!(
            s.is_native_for_tests().unwrap(),
            col.native(),
            "row {col:?}: the session holds the doc on that engine"
        );
    }
}

fn key(path: &str) -> DocKey {
    DocKey {
        space: "ws".into(),
        path: path.into(),
    }
}

fn person(pid: &str) -> Participant {
    Participant::new(pid, pid, "#123456", ParticipantKind::Person, "tab")
}

fn agent(pid: &str) -> Participant {
    Participant::new(pid, pid, "#654321", ParticipantKind::Agent, "cli")
}

fn text(s: &Session, c: ConnId) -> String {
    s.read(c).unwrap().text_by_root[0].1.clone()
}

fn ins(at: usize, t: &str) -> Hunk {
    Hunk {
        start: at,
        end: at,
        insert: t.into(),
    }
}

fn edit(s: &Session, c: ConnId, hunks: Vec<Hunk>) -> EditResult {
    s.edit(
        c,
        vec![Edit::Text {
            root: ROOT.into(),
            hunks,
        }],
        None,
    )
    .unwrap()
}

/// The stamp family a column's docs carry.
fn family(col: Col) -> &'static str {
    if col.native() {
        crate::engine::NATIVE_ENGINE_NAME
    } else {
        crate::engine::ENGINE_FAMILY
    }
}

fn family_of(engine: &str) -> &str {
    engine.split_once('-').map_or(engine, |(f, _)| f)
}

// ---------------------------------------------------------------------------------------------
// The switch and the choice

/// A doc's engine is chosen when it is made and only then: each column's create stamps its
/// engine, and opening the doc again asking for the other engine loads it on its own (stamp
/// and text unchanged). A native doc opens, edits and saves under a manager with the Beta
/// off.
#[test]
fn the_engine_is_chosen_only_at_create() {
    let mut exercised = 0;
    for col in cols() {
        let mut f = fx("choose", config(true));
        let k = key("a.md");
        let s = f.make(&k, b"one\n", col);
        drop(s);
        // open again, asking for the other engine: the stored doc's own engine
        let other = cols()
            .into_iter()
            .find(|c| *c != col)
            .unwrap_or(Col::Yrs)
            .opts();
        f.restart(config(true));
        let s = f.mgr.open_with(Arc::new(MD), &k, None, &other).unwrap();
        assert_eq!(family_of(&f.engine_of(&k)), family(col), "row {col:?}");
        assert_eq!(s.is_native_for_tests().unwrap(), col.native());
        drop(s);
        // the Beta off: a doc already made opens, edits and saves on its engine
        f.restart(config(false));
        let s = f.mgr.open(Arc::new(MD), &k, None).unwrap();
        let c = s.join(person("own"), Stay::Held).unwrap().conn;
        edit(&s, c, vec![ins(4, "two\n")]);
        drop(s);
        f.restart(config(false));
        let s = f.mgr.open(Arc::new(MD), &k, None).unwrap();
        let c = s.join(person("own"), Stay::Held).unwrap().conn;
        assert_eq!(text(&s, c), "one\ntwo\n", "row {col:?}: the edit was kept");
        assert_eq!(s.is_native_for_tests().unwrap(), col.native());
        exercised += 1;
    }
    assert_eq!(exercised, cols().len(), "row: every column ran");
}

// ---------------------------------------------------------------------------------------------
// One replica, several participants

/// A person and an agent on one doc: each its own writer (distinct client IDs, each recorded as
/// its own pid's), each change logged with its author (a native doc's rows name the engine's
/// change by position), and blame names each line's writer, before and after a restart.
#[test]
fn a_person_and_an_agent_write_one_replica() {
    let mut exercised = 0;
    for col in cols() {
        let mut f = fx("two", config(true));
        let k = key("a.md");
        let s = f.make(&k, b"one\n", col);
        let p = s.join(person("ann"), Stay::Held).unwrap();
        let a = s.join(agent("bot"), Stay::Held).unwrap();
        assert_ne!(
            p.client, a.client,
            "row {col:?}: one writer per participant"
        );
        let base = s.read(p.conn).unwrap().version;
        edit(&s, a.conn, vec![ins(4, "bot line\n")]);
        // the person types at the version they saw: rebased past the agent's change
        let r = s
            .edit_at_base(
                p.conn,
                ROOT,
                base,
                vec![ins(0, "ann line\n")],
                &EditOptions::new(),
            )
            .unwrap();
        assert!(r.result.version > base);
        edit(&s, a.conn, vec![ins(0, "x")]);
        let want = "xann line\none\nbot line\n";
        assert_eq!(text(&s, p.conn), want, "row {col:?}");
        let id = f.id(&k);
        let owners = f.store.clients_of(id).unwrap();
        for (c, pid) in [(p.client, "ann"), (a.client, "bot")] {
            assert!(
                owners.iter().any(|(x, o)| *x == c && o == pid),
                "row {col:?}: client {c} recorded as {pid}: {owners:?}"
            );
        }
        let log = f.store.load(id).unwrap().updates;
        assert_eq!(log.len(), 4, "row {col:?}: the import and three changes");
        for (seq, e) in &log {
            assert_eq!(
                matches!(e.body, EntryBody::Native(_)),
                col.native(),
                "row {col:?}: entry {seq}'s body is its engine's"
            );
        }
        let authors: Vec<&str> = log[1..].iter().map(|(_, e)| e.author.as_str()).collect();
        assert_eq!(authors, ["bot", "ann", "bot"], "row {col:?}");
        let blamed = |s: &Session, c| -> Vec<String> {
            s.blame(c, ROOT, None)
                .unwrap()
                .lines
                .into_iter()
                .map(|l| l.author)
                .collect()
        };
        // line 1 holds the agent's "x" and the person's line: blame names its last writer
        let before = blamed(&s, p.conn);
        assert_eq!(before.len(), 3, "row {col:?}");
        assert_eq!(&before[1..], [IMPORT_PID, "bot"], "row {col:?}: {before:?}");
        drop(s);
        f.restart(config(true));
        let s = f.mgr.open(Arc::new(MD), &k, None).unwrap();
        let c = s.join(person("ann"), Stay::Held).unwrap().conn;
        assert_eq!(text(&s, c), want, "row {col:?}: after a restart");
        assert_eq!(blamed(&s, c), before, "row {col:?}: blame after a restart");
        exercised += log.len();
    }
    assert!(
        exercised >= 4 * cols().len(),
        "row: every column logged its changes"
    );
}

/// Person-writer reuse: a person's device writer row rides the change's commit on both engines
/// (a native commit too) and is there after a restart.
#[test]
fn person_writer_rows_ride_the_commit() {
    let mut exercised = 0;
    for col in cols() {
        let c = Config {
            reuse_person_writers: true,
            ..config(true)
        };
        let mut f = fx("writers", c.clone());
        let k = key("a.md");
        let s = f.make(&k, b"one\n", col);
        let j = s
            .join_with(
                person("ann"),
                Stay::Held,
                &JoinOptions::new().device("dev-1"),
            )
            .unwrap();
        assert!(
            j.lease.is_some(),
            "row {col:?}: a device join holds a lease"
        );
        edit(&s, j.conn, vec![ins(0, "a")]);
        let rows = f.store.person_writers(f.id(&k)).unwrap();
        assert!(
            rows.iter()
                .any(|(c, pid, _)| *c == j.client && pid == "ann"),
            "row {col:?}: the writer's row landed with its first change: {rows:?}"
        );
        drop(s);
        f.restart(c);
        let rows = f.store.person_writers(f.id(&k)).unwrap();
        assert!(rows.iter().any(|(c, _, _)| *c == j.client), "row {col:?}");
        exercised += 1;
    }
    assert_eq!(exercised, cols().len());
}

/// The verbs K2 uses, with a person and an agent on one doc, end at the same text on both
/// engines: live edits at a base (rebased past the other's), an agent's `write --base` merged
/// with a person's edit since, undo per participant (each undoes only its own change), redo, a
/// revert by the change's author, a restore of a version, and a reader's change events, which
/// rebuild the text exactly.
#[test]
fn the_verbs_end_alike_on_both_engines() {
    let mut finals = Vec::new();
    for col in cols() {
        let f = fx("verbs", config(true));
        let k = key("a.md");
        let start = "alpha\nbeta\ngamma\n";
        let s = f.make(&k, start.as_bytes(), col);
        let p = s.join(person("ann"), Stay::Held).unwrap();
        let a = s.join(agent("bot"), Stay::Held).unwrap();
        let reader = s.join(person("rex"), Stay::Held).unwrap();
        let events = s.subscribe(reader.conn).unwrap();
        let v0 = s.read(a.conn).unwrap().version;
        // the person's live edits, one at the base they saw
        let ann = edit(&s, p.conn, vec![ins(0, "ANN ")]).version;
        let r = s
            .edit_at_base(p.conn, ROOT, v0, vec![ins(5, "!")], &EditOptions::new())
            .unwrap();
        assert!(!r.hunks.is_empty(), "row {col:?}: the live edit applied");
        // the agent's whole-file write from the version it read: merged with the person's
        let mine = "alpha\nbeta\nGAMMA\ndelta\n";
        match s.write_base(a.conn, ROOT, v0, mine).unwrap() {
            WriteOutcome::Applied(_) => {}
            o => panic!("row {col:?}: the write merged: {o:?}"),
        }
        let merged = text(&s, p.conn);
        assert_eq!(
            merged, "ANN alpha!\nbeta\nGAMMA\ndelta\n",
            "row {col:?}: the write merged with the person's edits"
        );
        // undo is per participant: the person's undoes only the person's last change
        let before = text(&s, p.conn);
        match s.undo(p.conn, 1).unwrap() {
            UndoOutcome::Applied { steps: 1, .. } => {}
            o => panic!("row {col:?}: the person's undo: {o:?}"),
        }
        let after = text(&s, p.conn);
        assert_ne!(before, after, "row {col:?}");
        assert!(
            after.contains("GAMMA\ndelta\n"),
            "row {col:?}: the agent's kept"
        );
        match s.undo(a.conn, 1).unwrap() {
            UndoOutcome::Applied { steps: 1, .. } => {}
            o => panic!("row {col:?}: the agent's undo: {o:?}"),
        }
        assert!(
            text(&s, p.conn).starts_with("ANN "),
            "row {col:?}: the person's kept"
        );
        match s.redo(a.conn, 1).unwrap() {
            UndoOutcome::Applied { steps: 1, .. } => {}
            o => panic!("row {col:?}: the agent's redo: {o:?}"),
        }
        assert_eq!(
            text(&s, p.conn),
            "ANN alpha\nbeta\nGAMMA\ndelta\n",
            "row {col:?}"
        );
        // a revert of the person's first change, by its author, long after it
        match s.revert(p.conn, ann, false).unwrap() {
            UndoOutcome::Applied { .. } => {}
            o => panic!("row {col:?}: the revert: {o:?}"),
        }
        assert_eq!(
            text(&s, p.conn),
            "alpha\nbeta\nGAMMA\ndelta\n",
            "row {col:?}"
        );
        // a restore of the import's version, as a new change
        let import = s
            .versions(p.conn)
            .unwrap()
            .into_iter()
            .find(|v| v.reason == crate::store::VersionReason::Import)
            .expect("the import's version");
        s.restore(p.conn, import.id).unwrap();
        let end = text(&s, p.conn);
        assert_eq!(end, start, "row {col:?}: restored");
        // the reader's events rebuild the text from the start
        let mut rebuilt = start.to_owned();
        let mut n = 0;
        while let Ok(ev) = events.try_recv() {
            rebuilt = crate::text::diff::apply(&rebuilt, &ev.hunks);
            n += 1;
        }
        assert!(n >= 8, "row {col:?}: the reader saw every change ({n})");
        assert_eq!(rebuilt, end, "row {col:?}: the events rebuild the text");
        finals.push((col, end));
    }
    let want = &finals[0].1;
    for (col, t) in &finals {
        assert_eq!(t, want, "row: {col:?} ends where Yrs does");
    }
    assert_eq!(finals.len(), cols().len());
}

/// Blame's sampled check (every blame, here) runs on a copy of the doc on its own engine, off
/// the session's lock, and finds the kept map sound.
#[test]
fn the_blame_check_copies_on_the_docs_engine() {
    for col in cols() {
        let f = fx(
            "blame",
            Config {
                blame_check_every: 1,
                ..config(true)
            },
        );
        let k = key("a.md");
        let s = f.make(&k, b"one\ntwo\n", col);
        let p = s.join(person("ann"), Stay::Held).unwrap();
        let a = s.join(agent("bot"), Stay::Held).unwrap();
        edit(&s, a.conn, vec![ins(4, "bot\n")]);
        edit(&s, p.conn, vec![ins(0, "ann ")]);
        for _ in 0..3 {
            let b = s.blame(p.conn, ROOT, None);
            assert!(b.is_ok(), "row {col:?}: a sampled blame: {b:?}");
        }
        let checks = s.blame_checks_for_tests().unwrap();
        assert!(checks >= 2, "row {col:?}: the check ran ({checks})");
    }
}

// ---------------------------------------------------------------------------------------------
// The disk replica

/// A file-backed doc: edits are written back, and an outside save is merged as the outside
/// editor's change. A native doc's disk replica is the file's texts: its stored synced state is
/// the kit's text form, never engine bytes, and it reopens and merges again after a restart.
#[test]
fn outside_saves_merge_through_a_text_replica() {
    let mut exercised = 0;
    for col in cols() {
        let mut f = fx("disk", config(true));
        let k = key("a.md");
        fs::write(f.file("a.md"), "one\ntwo\n").unwrap();
        let s = f.make_file(&k, "a.md", col);
        let a = s.join(agent("bot"), Stay::Held).unwrap();
        edit(&s, a.conn, vec![ins(0, "bot\n")]);
        s.flush(a.conn).unwrap();
        assert_eq!(
            fs::read_to_string(f.file("a.md")).unwrap(),
            "bot\none\ntwo\n"
        );
        let state = |f: &Fx| {
            f.store
                .sync_states(f.id(&k))
                .unwrap()
                .0
                .expect("a synced state")
                .state
        };
        let synced = state(&f);
        assert_eq!(
            super::live::is_text_replica_for_tests(&synced),
            col.native(),
            "row {col:?}: a native doc's synced state is its texts, a Yrs doc's its state"
        );
        // an outside save while the agent's edit since isn't written: merged
        edit(&s, a.conn, vec![ins(0, "x")]);
        f.advance(10);
        fs::write(f.file("a.md"), "bot\none\ntwo\nthree\n").unwrap();
        match s.check_disk().unwrap() {
            DiskCheck::Ingested { .. } => {}
            o => panic!("row {col:?}: the save was merged: {o:?}"),
        }
        assert_eq!(text(&s, a.conn), "xbot\none\ntwo\nthree\n", "row {col:?}");
        let log = f.store.load(f.id(&k)).unwrap().updates;
        let (_, last) = log.last().unwrap();
        assert_eq!(last.author, EXTERNAL_PID, "row {col:?}");
        assert_eq!(matches!(last.body, EntryBody::Native(_)), col.native());
        s.flush(a.conn).unwrap();
        assert_eq!(
            fs::read_to_string(f.file("a.md")).unwrap(),
            "xbot\none\ntwo\nthree\n"
        );
        assert_eq!(
            super::live::is_text_replica_for_tests(&state(&f)),
            col.native(),
            "row {col:?}: after a merge and a write-back"
        );
        drop(s);
        // a restart: the stored replica opens, and the next outside save merges
        f.restart(config(true));
        fs::write(f.file("a.md"), "xbot\none\ntwo\nthree\nfour\n").unwrap();
        let s = f.open_file(&k, "a.md", &OpenOptions::new());
        let c = s.join(person("ann"), Stay::Held).unwrap().conn;
        assert_eq!(text(&s, c), "xbot\none\ntwo\nthree\nfour\n", "row {col:?}");
        assert!(
            s.audit_disk_sync().unwrap().is_empty(),
            "row {col:?}: in sync"
        );
        exercised += log.len();
    }
    assert!(exercised >= 4 * cols().len());
}

// ---------------------------------------------------------------------------------------------
// The live-text cap

/// A native doc's live text stays within the engine's `string_bytes_per_op` (4 MiB at its
/// defaults): an edit that would grow it past is refused with the cap's own code,
/// `NativeTextCap` (`MPC-E909` `native_text_cap`, exit 2, as the standard engine's size limits)
/// with nothing applied, an edit that shrinks it isn't, and an outside save that would grow it past is
/// declined (`too_large`), the doc as it was. The Yrs twin takes the same edit: the cap is the
/// native engine's alone.
#[test]
fn a_native_docs_live_text_is_capped() {
    let mut exercised = 0;
    for col in cols() {
        let f = fx("cap", config(true));
        let k = key("a.md");
        fs::write(f.file("a.md"), "one\n").unwrap();
        let s = f.make_file(&k, "a.md", col);
        let a = s.join(agent("bot"), Stay::Held).unwrap();
        let cap = 4 << 20;
        let big = "y".repeat(cap);
        let v = s.read(a.conn).unwrap().version;
        let r = s.edit(
            a.conn,
            vec![Edit::Text {
                root: ROOT.into(),
                hunks: vec![ins(0, &big)],
            }],
            None,
        );
        if col.native() {
            let e = match r {
                Err(e) => e,
                Ok(_) => panic!("row {col:?}: an edit past the cap is refused"),
            };
            assert!(
                matches!(
                    e,
                    SessionError::NativeTextCap {
                        limit,
                        would_be,
                    } if limit == cap && would_be == cap + 4
                ),
                "row {col:?}: the cap's own refusal: {e:?}"
            );
            assert_eq!(e.report().id(), "MPC-E909", "row: native_text_cap");
            assert_eq!(session_exit(&e), 2, "row: exit 2");
            assert_eq!(s.read(a.conn).unwrap().version, v, "row: nothing applied");
            assert_eq!(text(&s, a.conn), "one\n", "row: nothing dropped");
            // a shrinking edit still applies
            edit(
                &s,
                a.conn,
                vec![Hunk {
                    start: 0,
                    end: 4,
                    insert: String::new(),
                }],
            );
            assert_eq!(text(&s, a.conn), "");
            edit(&s, a.conn, vec![ins(0, "one\n")]);
            // an outside save past the cap: declined, the doc as it was
            let mut file = String::from("one\n");
            file.push_str(&big);
            fs::write(f.file("a.md"), &file).unwrap();
            match s.check_disk().unwrap() {
                DiskCheck::Declined(d) => assert_eq!(d.code, "too_large", "row {col:?}"),
                o => panic!("row {col:?}: a save past the cap is declined: {o:?}"),
            }
            assert_eq!(text(&s, a.conn), "one\n", "row {col:?}: the doc as it was");
        } else {
            assert!(r.is_ok(), "row {col:?}: Yrs takes it: {r:?}");
            assert_eq!(text(&s, a.conn).len(), cap + 4);
        }
        exercised += 1;
    }
    assert_eq!(exercised, cols().len());
}

// ---------------------------------------------------------------------------------------------
// The cut cadence, a host setting

/// The cut cadence is a host setting (`Config::native_cut_cadence`), on every build: by default
/// the Beta's, `min(200, max(100, n / 30))`, listed among the settings in effect; a host's own
/// is checked by `CutCadence::new`, which refuses a floor of 0, a floor over the ceiling, a
/// ceiling past `CEILING_MOST` and a divisor of 0, and takes the bounds themselves.
/// `MPC_BETA_PLANT=cadence-unchecked` fails at the first refusal's assert.
#[test]
fn the_cut_cadence_is_checked_and_defaults_to_the_betas() {
    let d = Config::default().native_cut_cadence;
    assert_eq!(
        (d.floor(), d.ceiling(), d.divisor()),
        (100, 200, 30),
        "row: the default is the Beta's"
    );
    assert_eq!(d, CutCadence::BETA);
    let mut rows = 0;
    for (n, want) in [
        (0, 100),
        (2_999, 100),
        (3_000, 100),
        (3_030, 101),
        (6_000, 200),
        (6_030, 200),
        (1_000_000, 200),
    ] {
        assert_eq!(d.every(n), want, "row: the threshold at {n} changes");
        rows += 1;
    }
    assert_eq!(d.live_cut_most(), 400, "row: twice the ceiling");
    assert_eq!(CutCadence::new(100, 200, 30), Ok(CutCadence::BETA));
    for (f, c, v, want) in [
        (0, 500, 30, CadenceError::FloorZero),
        (
            600,
            500,
            30,
            CadenceError::FloorOverCeiling {
                floor: 600,
                ceiling: 500,
            },
        ),
        (
            100,
            CutCadence::CEILING_MOST + 1,
            30,
            CadenceError::CeilingTooHigh {
                ceiling: CutCadence::CEILING_MOST + 1,
                most: CutCadence::CEILING_MOST,
            },
        ),
        (100, 500, 0, CadenceError::DivisorZero),
    ] {
        assert_eq!(
            CutCadence::new(f, c, v),
            Err(want),
            "row: ({f}, {c}, {v}) is refused"
        );
        rows += 1;
    }
    let edge = CutCadence::new(1, CutCadence::CEILING_MOST, 1);
    assert!(edge.is_ok(), "row: the bounds themselves: {edge:?}");
    let listed = Config::default().limits();
    for (name, want) in [
        ("native_cut_floor", 100),
        ("native_cut_ceiling", 200),
        ("native_cut_divisor", 30),
    ] {
        assert!(
            listed.contains(&(name, want)),
            "row: {name} listed among the settings"
        );
    }
    println!("CADENCE rows={rows}");
}

/// A host's cadence reaches its native docs: a doc made under it runs under it, and after a
/// restart under another cadence the reopened doc runs under the new one (a doc takes the
/// setting at its next open). The Yrs twin has no cadence.
/// `MPC_BETA_PLANT=cadence-not-taken` fails at the made doc's assert.
#[test]
fn a_hosts_cut_cadence_reaches_its_native_docs() {
    let mut exercised = 0;
    for col in cols() {
        let first = CutCadence::new(10, 20, 5).unwrap();
        let mut f = fx(
            "cadence",
            Config {
                native_cut_cadence: first,
                ..config(true)
            },
        );
        let k = key("a.md");
        let s = f.make(&k, b"one\n", col);
        drop(s);
        let second = CutCadence::new(50, 60, 2).unwrap();
        f.restart(Config {
            native_cut_cadence: second,
            ..config(true)
        });
        let s = f.mgr.open(Arc::new(MD), &k, None).unwrap();
        drop(s);
        exercised += 1;
    }
    assert_eq!(exercised, cols().len());
}

// ---------------------------------------------------------------------------------------------
// Keys: by the store's salt and the doc's id

// ---------------------------------------------------------------------------------------------
// Purge

/// An owner's purge of a native doc's history runs as on Yrs: the live text stays, and the
/// text a change inserted and another deleted is gone from everything the kit holds, the
/// doc's own engine records included (each held it before: the control). The native engine's
/// purge is one change, logged as a change of no text by the owner (the version moves by one;
/// on Yrs it doesn't), and the doc reopens after a restart with its live text. A second purge,
/// with nothing deleted since, adds no change.
/// `MPC_MPCE_PLANT=purge-leaves-deleted` (the engine erases no deleted element) fails at the
/// records' assert.
#[test]
fn an_owners_purge_erases_a_native_docs_history() {
    fn under<R>(_: (), f: impl FnOnce() -> R) -> R {
        f()
    }
    let plant = ();
    const SECRET: &str = "a secret line, typed then deleted\n";
    under(plant, || {
        let mut exercised = 0;
        for col in cols() {
            let mut f = fx("purge", config(true));
            let k = key("a.md");
            let s = f.make(&k, b"keep\n", col);
            let own = s.join(person("own"), Stay::Held).unwrap();
            let bot = s.join(agent("bot"), Stay::Held).unwrap();
            edit(&s, bot.conn, vec![ins(5, SECRET)]);
            edit(
                &s,
                bot.conn,
                vec![Hunk {
                    start: 5,
                    end: 5 + SECRET.len(),
                    insert: String::new(),
                }],
            );
            let v = s.read(own.conn).unwrap().version;
            let out = s.purge(own.conn, PurgeRequest::new(PurgeScope::History));
            assert!(
                out.is_ok(),
                "row {col:?}: the owner's purge runs: {:?}",
                out.err()
            );
            assert_eq!(
                text(&s, own.conn),
                "keep\n",
                "row {col:?}: the live text stays"
            );
            let after = s.read(own.conn).unwrap().version;
            assert_eq!(
                after,
                v + u64::from(col.native()),
                "row {col:?}: the native engine's purge is one change"
            );
            drop(s);
            let c = f.config.clone();
            f.restart(c);
            let s = f.mgr.open(Arc::new(MD), &k, None).unwrap();
            let r = s.join(agent("reader"), Stay::Held).unwrap();
            assert_eq!(text(&s, r.conn), "keep\n", "row {col:?}: it reopens");
            drop(s);
            exercised += 1;
        }
        assert_eq!(exercised, cols().len());
    });
}

// ---------------------------------------------------------------------------------------------
// Tier 0: no move to or from it

/// In the Beta a doc on the native engine doesn't move to or from tier 0. A promotion asked at
/// its key is refused `NativeNoTier0` (`MPC-E721`, exit 4), as is its demotion, nothing changed:
/// still on the native engine, its rows' tables and its log as they were, its text as it was.
/// The Yrs twin: a promotion asked at a text doc's key is the standard refusal, `PackMismatch`
/// (exit 4, the same class), and the doc demotes to tier 0.
/// `MPC_BETA_PLANT=native-demotes` (the demote's refusal skipped) fails at the demote's assert.
#[test]
fn a_native_doc_neither_demotes_nor_promotes() {
    let mut exercised = 0;
    for col in cols() {
        let f = fx_blobs("tier0", config(true));
        let k = key("a.md");
        fs::write(f.file("a.md"), "one\n").unwrap();
        let s = f.make_file(&k, "a.md", col);
        let a = s.join(agent("bot"), Stay::Held).unwrap();
        edit(&s, a.conn, vec![ins(4, "two\n")]);
        s.leave(a.conn).unwrap();
        drop(s);
        let id = f.id(&k);
        let tables = f.store.tables_holding(id).unwrap();
        let log = f.store.load(id).unwrap().updates.len();
        let p = f.mgr.promote_blob(Arc::new(MD), &k);
        let pe = match p {
            Err(e) => e,
            Ok(_) => panic!("row {col:?}: a promotion at a text doc's key is refused"),
        };
        assert_eq!(
            session_exit(&pe),
            4,
            "row {col:?}: the promotion's exit: {pe:?}"
        );
        let d = f.mgr.demote_text_file(
            Arc::new(MD),
            Arc::new(AnyFile),
            &k,
            &f.file("a.md"),
            &f.dir.join("ws"),
        );
        if col.native() {
            assert!(
                matches!(pe, SessionError::NativeNoTier0 { verb: "promote" }),
                "row {col:?}: a promotion at a native doc's key: {pe:?}"
            );
            assert_eq!(pe.report().id(), "MPC-E721", "row: native_no_tier0");
            let de = match d {
                Err(e) => e,
                Ok(_) => panic!("row {col:?}: a native doc's demotion is refused"),
            };
            assert!(
                matches!(de, SessionError::NativeNoTier0 { verb: "demote" }),
                "row {col:?}: {de:?}"
            );
            assert_eq!(de.report().id(), "MPC-E721", "row: native_no_tier0");
            assert_eq!(session_exit(&de), 4, "row: exit 4");
            assert_eq!(
                family_of(&f.engine_of(&k)),
                family(col),
                "row: still on the native engine"
            );
            assert_eq!(
                f.store.tables_holding(id).unwrap(),
                tables,
                "row: its tables"
            );
            assert_eq!(f.store.load(id).unwrap().updates.len(), log, "row: its log");
            let s = f.mgr.open(Arc::new(MD), &k, None).unwrap();
            let r = s.join(agent("reader"), Stay::Held).unwrap();
            assert_eq!(text(&s, r.conn), "one\ntwo\n", "row: its text");
        } else {
            assert!(
                matches!(pe, SessionError::PackMismatch { .. }),
                "row {col:?}: the standard refusal: {pe:?}"
            );
            let done = d.map(|(_, d)| d);
            assert!(done.is_ok(), "row {col:?}: the doc demotes: {done:?}");
        }
        exercised += 1;
    }
    assert_eq!(exercised, cols().len());
}

// ---------------------------------------------------------------------------------------------
// One replica: no sync bytes

// ---------------------------------------------------------------------------------------------
// Waiting for the engine's single-replica mode
