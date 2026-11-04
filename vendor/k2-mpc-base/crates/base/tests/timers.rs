//! The production timers (v0.10.1, `k2_mpc_base::timers`; `docs/SESSIONS.md`, "Timers"):
//! each timed path records once per call while the timers are on, and nothing while they are
//! off. Public API only.
//!
//! One scenario drives every path through the call that reaches it, reading each path's count
//! before and after the call. The scenario's table must name every [`Path`]: a path added
//! without a row here fails `every_path_records_once_per_call`. Every row is checked and every
//! mismatch reported together.
//!
//! The registry is process-wide, so the tests in this file take one lock (this binary is its
//! own process: no other test file's calls reach it).
//!
//! `calibrate_and_lookup_cost` prints what a scope costs on the machine, and the mean time of
//! a provenance lookup, as a `RUNREC` line (`--nocapture`).

#[path = "grid_pack/mod.rs"]
mod grid_pack;

use std::collections::BTreeSet;
use std::path::{Path as FsPath, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use grid_pack::GridLite;
use k2_mpc_base::doc::{Doc, Edit};
use k2_mpc_base::session::{
    AuditEvent, Config, ConnId, DiskCheck, GridWriteOptions, Host, Pack, Participant,
    ParticipantKind, Rights, Session, SessionManager, Stay, TextFilePack, UndoOutcome,
    WriteOutcome,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::table::ROWS;
use k2_mpc_base::text::diff::Hunk;
use k2_mpc_base::timers::{self, Path};

const ROOT: &str = TextFilePack::ROOT;
const V0: &str = "intro\nmiddle\nend\n";

static LOCK: Mutex<()> = Mutex::new(());

fn lock() -> MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

struct H {
    now: AtomicU64,
}

impl Host for H {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn on_event(&self, _: &AuditEvent) {}
}

/// A temp folder with a store and one file, removed at the end.
struct Dir(PathBuf);

impl Dir {
    fn new(tag: &str) -> Dir {
        static N: AtomicU64 = AtomicU64::new(0);
        let d = std::env::temp_dir().join(format!(
            "k2mb-timers-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("ws")).unwrap();
        Dir(d)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Fx {
    host: Arc<H>,
    store: Arc<DocStore>,
    mgr: SessionManager,
    s: Arc<Session>,
    key: DocKey,
    file: PathBuf,
    // last: dropped after the store and the manager
    dir: Dir,
}

fn open(
    dir: &FsPath,
    host: &Arc<H>,
    pack: Arc<dyn Pack>,
    key: &DocKey,
    file: &FsPath,
) -> (Arc<DocStore>, SessionManager, Arc<Session>) {
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let mgr = SessionManager::new(store.clone(), host.clone(), Config::default());
    let s = mgr.open_file(pack, key, file, &dir.join("ws")).unwrap();
    (store, mgr, s)
}

fn fx(tag: &str, pack: Arc<dyn Pack>, name: &str, initial: &str) -> Fx {
    let dir = Dir::new(tag);
    let file = dir.0.join("ws").join(name);
    std::fs::write(&file, initial).unwrap();
    let host = Arc::new(H {
        now: AtomicU64::new(1_000_000),
    });
    let key = DocKey {
        space: "ws".into(),
        path: name.into(),
    };
    let (store, mgr, s) = open(&dir.0, &host, pack, &key, &file);
    Fx {
        host,
        store,
        mgr,
        s,
        key,
        file,
        dir,
    }
}

impl Fx {
    fn join(&self, pid: &str, kind: ParticipantKind) -> ConnId {
        self.s
            .join(Participant::new(pid, pid, "#1", kind, "cli"), Stay::Held)
            .unwrap()
            .conn
    }
    /// The host's clock moves on (an ordinary pace: no rate limits, no bursts).
    fn advance(&self) {
        self.host.now.fetch_add(1_000, Ordering::SeqCst);
    }
    fn version(&self) -> u64 {
        self.s.status().unwrap().version
    }
    fn text(&self, c: ConnId) -> String {
        self.s.read(c).unwrap().text_by_root[0].1.clone()
    }
    /// Release everything (the file is written back), close the store, and open the doc again
    /// from it.
    fn reopen(self, pack: Arc<dyn Pack>) -> Fx {
        let Fx {
            host,
            store,
            mgr,
            s,
            key,
            file,
            dir,
        } = self;
        mgr.release().unwrap();
        drop(s);
        drop(mgr);
        drop(store);
        let (store, mgr, s) = open(&dir.0, &host, pack, &key, &file);
        Fx {
            host,
            store,
            mgr,
            s,
            key,
            file,
            dir,
        }
    }
}

fn markdown() -> Arc<dyn Pack> {
    Arc::new(TextFilePack::new("markdown", 4 << 20))
}

/// `text` appended by `c`.
fn append(f: &Fx, c: ConnId, text: &str) -> u64 {
    f.advance();
    let live = f.text(c);
    f.s.edit(
        c,
        vec![Edit::Text {
            root: ROOT.to_owned(),
            hunks: vec![Hunk {
                start: live.len(),
                end: live.len(),
                insert: text.to_owned(),
            }],
        }],
        None,
    )
    .unwrap();
    f.version()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Want {
    Exactly(u64),
    /// Several per call by design (`prov.lookup`: one per region planned).
    AtLeast(u64),
}

impl Want {
    fn holds(self, got: u64) -> bool {
        match self {
            Want::Exactly(n) => got == n,
            Want::AtLeast(n) => got >= n,
        }
    }
}

/// One row: the path, the call that reached it, what it should count, what it counted.
struct Row {
    path: Path,
    call: &'static str,
    want: Want,
    got: u64,
}

#[derive(Default)]
struct Table {
    rows: Vec<Row>,
}

impl Table {
    /// Run `call`, then record each of `paths`' count deltas against what it wants.
    fn call<R>(
        &mut self,
        name: &'static str,
        paths: &[(Path, Want)],
        call: impl FnOnce() -> R,
    ) -> R {
        let before: Vec<u64> = paths.iter().map(|(p, _)| timers::read(*p).0).collect();
        let out = call();
        for ((p, want), b) in paths.iter().zip(before) {
            self.rows.push(Row {
                path: *p,
                call: name,
                want: *want,
                got: timers::read(*p).0 - b,
            });
        }
        out
    }
}

const ONE: Want = Want::Exactly(1);

/// Every path through the call that reaches it.
fn scenario() -> Table {
    let mut t = Table::default();

    // ---- an engine doc alone
    let schema = TextFilePack::new("markdown", 4 << 20).schema();
    let mut d = Doc::new(schema).unwrap();
    t.call("Doc::edit_as", &[(Path::EditAs, ONE)], || {
        d.edit_as(
            7,
            &[Edit::Text {
                root: ROOT.to_owned(),
                hunks: vec![Hunk {
                    start: 0,
                    end: 0,
                    insert: "x\n".to_owned(),
                }],
            }],
        )
        .unwrap()
    });

    // ---- a text doc with a file
    let f = fx("text", markdown(), "notes.md", V0);
    let p = t.call("Session::join", &[(Path::Join, ONE)], || {
        f.join("pat", ParticipantKind::Person)
    });
    let a = f.join("agent", ParticipantKind::Agent);
    let v1 = t.call(
        "Session::edit",
        &[(Path::Commit, ONE), (Path::StoreTxnCommit, ONE)],
        || append(&f, p, "pat's line\n"),
    );
    t.call("Doc::from_store", &[(Path::LoadDoc, ONE)], || {
        let id = f.store.find(&f.key).unwrap().expect("stored");
        let loaded = f.store.load(id).unwrap();
        Doc::from_store(schema, &loaded).unwrap()
    });
    t.call("Session::blame", &[(Path::Blame, ONE)], || {
        f.s.blame(p, ROOT, None).unwrap()
    });
    t.call(
        "Session::events_since",
        &[(Path::EventsSince, ONE), (Path::EventsSinceWindow, ONE)],
        || {
            let ev = f.s.events_since(p, v1 - 1).unwrap();
            assert_eq!(ev.len(), 1, "one change since");
        },
    );
    f.advance();
    let base = f.version();
    let ours = f.text(a).replace("middle\n", "middle, by the agent\n");
    t.call(
        "Session::write_base",
        &[
            (Path::WriteBase, ONE),
            (Path::WriteBaseBaseText, ONE),
            (Path::WriteBaseMerge, ONE),
        ],
        || {
            let out = f.s.write_base(a, ROOT, base, &ours).unwrap();
            assert!(matches!(out, WriteOutcome::Applied(_)), "{out:?}");
        },
    );
    append(&f, p, "pat again\n");
    t.call(
        "Session::undo",
        &[
            (Path::Undo, ONE),
            (Path::UndoPlan, ONE),
            (Path::ProvLookup, Want::AtLeast(1)),
        ],
        || {
            let out = f.s.undo(p, 1).unwrap();
            assert!(matches!(out, UndoOutcome::Applied { .. }), "{out:?}");
        },
    );
    f.advance();
    t.call(
        "Session::revert",
        &[(Path::Revert, ONE), (Path::UndoPlan, ONE)],
        || {
            let out = f.s.revert(p, v1, false).unwrap();
            assert!(matches!(out, UndoOutcome::Applied { .. }), "{out:?}");
        },
    );
    append(&f, p, "for the file\n");
    t.call(
        "Session::flush",
        &[(Path::WriteBack, ONE), (Path::WriteBackFile, ONE)],
        || f.s.flush(p).unwrap(),
    );
    // an outside save replaces the agent's line: the merge, and a note for its author
    f.advance();
    let argv: Vec<String> = vec!["insert".into(), "--line".into(), "2".into()];
    f.s.run_tool(a, ROOT, &argv, b"the agent's line\n").unwrap();
    f.s.flush(a).unwrap();
    f.host.now.fetch_add(10_000, Ordering::SeqCst);
    let on_disk = std::fs::read_to_string(&f.file).unwrap();
    let saved = on_disk.replace("the agent's line\n", "") + "edited in the editor\n";
    std::fs::write(&f.file, saved).unwrap();
    t.call(
        "Session::file_changed",
        &[
            (Path::Ingest, ONE),
            (Path::IngestDiff, ONE),
            (Path::IngestMerge, ONE),
            (Path::IngestCanon, ONE),
        ],
        || {
            let out = f.s.file_changed().unwrap();
            assert!(matches!(out, DiskCheck::Ingested { .. }), "{out:?}");
        },
    );
    t.call(
        "Session::take_notes",
        &[(Path::Notes, ONE), (Path::NotesDeliver, ONE)],
        || {
            let n = f.s.take_notes(a).unwrap();
            assert!(!n.notes.is_empty(), "the agent is told what went");
        },
    );
    f.host.now.fetch_add(61_000, Ordering::SeqCst);
    t.call(
        "SessionManager::tick",
        &[
            (Path::Tick, ONE),
            (Path::TickHistory, ONE),
            (Path::TickDisk, ONE),
        ],
        || f.mgr.tick().unwrap(),
    );
    let f = t.call(
        "SessionManager::open_file (stored)",
        &[(Path::Load, ONE), (Path::LoadStoreRead, ONE)],
        move || f.reopen(markdown()),
    );
    let p = f.join("pat", ParticipantKind::Person);
    t.call(
        "Session::edit (first after a load)",
        &[(Path::ProvRebuild, ONE)],
        || append(&f, p, "after the load\n"),
    );
    drop(f);

    // ---- a grid doc with a file
    let g = fx(
        "grid",
        Arc::new(GridLite),
        "t.csv",
        "id,city\nk1,Oslo\nk2,Rome\nk3,Lima\n",
    );
    let p = g.join("pat", ParticipantKind::Person);
    g.advance();
    let base = g.version();
    t.call(
        "Session::write_base_grid",
        &[(Path::GridWriteBase, ONE)],
        || {
            let r =
                g.s.write_base_grid(
                    p,
                    base,
                    b"id,city\nk1,Oslo\nk2,Milano\nk3,Lima\n",
                    &GridWriteOptions::default(),
                )
                .unwrap();
            assert_eq!(r.exit(), 0);
        },
    );
    g.advance();
    let argv: Vec<String> = "cell set --row #1 --col city --value Bergen"
        .split(' ')
        .map(str::to_owned)
        .collect();
    let out = g.s.run_tool(p, ROWS, &argv, b"").unwrap();
    assert_eq!(out.exit, 0, "{}", String::from_utf8_lossy(&out.stdout));
    g.advance();
    t.call("Session::undo (grid)", &[(Path::GridUndoPlan, ONE)], || {
        let out = g.s.undo(p, 1).unwrap();
        assert!(matches!(out, UndoOutcome::Applied { .. }), "{out:?}");
    });
    g.s.flush(p).unwrap();
    g.host.now.fetch_add(10_000, Ordering::SeqCst);
    let on_disk = std::fs::read_to_string(&g.file).unwrap();
    std::fs::write(&g.file, on_disk.replace("Lima", "Quito")).unwrap();
    t.call(
        "Session::file_changed (grid)",
        &[(Path::GridIngest, ONE)],
        || {
            let out = g.s.file_changed().unwrap();
            assert!(matches!(out, DiskCheck::Ingested { .. }), "{out:?}");
        },
    );
    t
}

fn report(t: &Table, ok: impl Fn(&Row) -> bool) -> Vec<String> {
    t.rows
        .iter()
        .filter(|r| !ok(r))
        .map(|r| {
            format!(
                "{} via {}: counted {}, wanted {:?}",
                r.path.name(),
                r.call,
                r.got,
                r.want
            )
        })
        .collect()
}

#[test]
fn every_path_records_once_per_call() {
    let _l = lock();
    timers::set_enabled(true);
    let t = scenario();
    timers::set_enabled(false);
    let mut bad = report(&t, |r| r.want.holds(r.got));
    let named: BTreeSet<Path> = t.rows.iter().map(|r| r.path).collect();
    for p in Path::ALL {
        if !named.contains(p) {
            bad.push(format!("{}: no row in this test's scenario", p.name()));
        }
    }
    assert!(
        bad.is_empty(),
        "timers that didn't count as they should ({} of {} rows; {} paths):\n  {}",
        bad.len(),
        t.rows.len(),
        Path::ALL.len(),
        bad.join("\n  ")
    );
}

#[test]
fn off_records_nothing() {
    let _l = lock();
    timers::set_enabled(false);
    let before = timers::snapshot();
    let t = scenario();
    let bad = report(&t, |r| r.got == 0);
    assert!(
        bad.is_empty(),
        "timers counted while off:\n  {}",
        bad.join("\n  ")
    );
    let after = timers::snapshot();
    assert_eq!(
        before.paths, after.paths,
        "a figure moved while the timers were off"
    );
    assert!(!after.enabled);
}

#[test]
fn snapshot_reset_and_the_manager_s_view() {
    let _l = lock();
    timers::set_enabled(true);
    let t = scenario();
    timers::set_enabled(false);
    assert!(!t.rows.is_empty());
    let snap = timers::snapshot();
    assert!(!snap.enabled);
    assert_eq!(snap.hooks, timers::HOOKS);
    assert_eq!(
        snap.paths.iter().map(|p| p.path).collect::<Vec<_>>(),
        Path::ALL.to_vec(),
        "one entry per path, in order"
    );
    for s in &snap.paths {
        assert_eq!((s.count, s.total_ns), timers::read(s.path), "{}", s.name);
        assert_eq!(s.name, s.path.name());
        assert!(s.count > 0, "{} counted nothing in the scenario", s.name);
        assert_eq!(
            s.buckets.iter().map(|b| b.1).sum::<u64>(),
            s.count,
            "{}",
            s.name
        );
        assert!(
            s.p50_ns <= s.p95_ns && s.p95_ns <= s.p99_ns && s.p99_ns <= s.max_ns,
            "{s:?}"
        );
        assert!(s.max_ns <= s.total_ns, "{s:?}");
    }
    // the manager's view is the same registry
    let f = fx("view", markdown(), "v.md", V0);
    assert_eq!(f.mgr.timers().paths, timers::snapshot().paths);
    drop(f);
    timers::reset();
    let z = timers::snapshot();
    assert!(z.paths.iter().all(|s| s.count == 0
        && s.total_ns == 0
        && s.max_ns == 0
        && s.p99_ns == 0
        && s.buckets.is_empty()));
}

/// What a scope costs here, and the mean time of `prov.lookup` (one region's lookups): the
/// scope's share of it stays small (a single lookup was about 160 ns on a release build,
/// near a running scope's own cost, which is why the scope covers the region's block).
#[test]
fn calibrate_and_lookup_cost() {
    let _l = lock();
    let c = timers::calibrate(1_000_000);
    timers::reset();
    timers::set_enabled(true);
    let f = fx("lookup", markdown(), "l.md", V0);
    let p = f.join("pat", ParticipantKind::Person);
    for k in 0..200 {
        append(&f, p, &format!("line {k}\n"));
        if k % 2 == 1 {
            let _ = f.s.undo(p, 1).unwrap();
        }
    }
    timers::set_enabled(false);
    let (n, ns) = timers::read(Path::ProvLookup);
    let mean = if n == 0 { 0.0 } else { ns as f64 / n as f64 };
    eprintln!(
        "RUNREC {{\"run\":\"timers_calibrate\",\"tool\":\"timers_test\",\"status\":\"COMPLETE\",\
         \"debug\":{},\"hooks\":{},\"off_ns\":{:.2},\"on_ns\":{:.2},\"rounds\":{},\
         \"prov_lookup_calls\":{n},\"prov_lookup_mean_ns\":{mean:.1},\"scope_share\":{:.3}}}",
        cfg!(debug_assertions),
        timers::HOOKS,
        c.off_ns,
        c.on_ns,
        c.rounds,
        if mean > 0.0 { c.on_ns / mean } else { 0.0 }
    );
    assert!(n > 0, "the undos looked provenance up");
    assert!(c.on_ns > 0.0);
    drop(f);
    timers::reset();
}
