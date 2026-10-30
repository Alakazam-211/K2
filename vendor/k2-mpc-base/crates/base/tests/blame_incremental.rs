//! Blame's provenance map, kept up to date as changes apply, against a rebuild from scratch
//! (v0.9.5).
//!
//! Blame reads each live character's element and provenance root off the session's map. The
//! map is built from the engine and the store once (the cold path: a throwaway replica of
//! the doc), then every change keeps it exact as it applies: the session's own edits, undo,
//! redo, revert, restore, and outside edits. This file checks the kept map is the rebuild's:
//!
//! - **The differential test** (`blame_kept_map_equals_the_rebuild`): random docs and
//!   histories (multi-hunk edits by people and agents, an assistant's edits for a person and
//!   their approvals, `write --base`, undo, redo, revert, restore, purge, outside edits with
//!   and without a write-back first, compaction, restarts). After every step: the kept map
//!   equals a rebuild (runs and live restorations by root), and `blame` (every line, and a
//!   random `-L` range) equals blame from the rebuild in every field (author, change, kind,
//!   `restored_by`, `restored_in`, `erased`, `kit:unknown`...), and no mismatch was ever
//!   reported. `BLAME_DIFF_CASES` histories (default 256; at least 10^4 on a node),
//!   `BLAME_DIFF_STEPS` steps each (default 24), on `BLAME_DIFF_THREADS` threads, from seed
//!   `BLAME_DIFF_SEED`. A failure prints its seed and its steps.
//! - **Planted negatives** (`ProvMutant`): a missed deletion (the map keeps deleted
//!   elements), a missed deletion in the index of live restorations, and a restored element
//!   recorded as its own root (blamed to its restorer). Each must be caught by the
//!   differential check, and by the runtime check ([`Config::blame_check_every`] at 1):
//!   reported as `AuditEvent::ProvenanceMismatch`, and never served (every blame equals the
//!   rebuild's).
//!
//! ```sh
//! BLAME_DIFF_CASES=10000 BLAME_DIFF_THREADS=4 cargo test -p k2-mpc-base --release \
//!   --test blame_incremental -- --nocapture
//! ```

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use k2_mpc_base::differential::{with_blame_check_hook, with_prov_mutant, ProvMutant};
use k2_mpc_base::session::{
    ApprovalState, AuditEvent, Blame, Config, ConnId, Host, LineRange, Participant,
    ParticipantKind, PurgeRequest, PurgeScope, Rights, Session, SessionManager, Stay, TextFilePack,
    UndoMode, UndoRequest,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::text::diff::Hunk;
use k2_mpc_base::Edit;
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};

const MD: TextFilePack = TextFilePack::new("markdown", 4 << 20);
const ROOT: &str = TextFilePack::ROOT;
const T0: u64 = 1_793_111_400_000;

struct TestHost {
    now: AtomicU64,
    events: Mutex<Vec<AuditEvent>>,
}

impl Host for TestHost {
    fn rights(&self, who: &Participant, _key: &DocKey) -> Rights {
        if who.pid == "own" {
            Rights::full().with_owner(true)
        } else {
            Rights::full()
        }
    }
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn on_event(&self, ev: &AuditEvent) {
        self.events.lock().unwrap().push(ev.clone());
    }
}

impl TestHost {
    fn mismatches(&self) -> Vec<String> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                AuditEvent::ProvenanceMismatch { what, version, .. } => {
                    Some(format!("v{version}: {what}"))
                }
                _ => None,
            })
            .collect()
    }
}

/// The participants: `(pid, kind, on behalf of)`. `own` purges; `obs` only blames.
const WHO: &[(&str, ParticipantKind, Option<&str>)] = &[
    ("alice", ParticipantKind::Person, None),
    ("bob", ParticipantKind::Agent, None),
    ("aide", ParticipantKind::Agent, Some("alice")),
    ("own", ParticipantKind::Person, None),
    ("obs", ParticipantKind::Person, None),
];

/// What a run checks after each step.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    /// The differential test: the kept map against a rebuild, and blame from each.
    Differential,
    /// The runtime check at 1 in 1: only `blame`, which must equal the rebuild's.
    Runtime,
}

/// Counts over a run (summed over cases).
#[derive(Default, Debug, Clone)]
struct Counts {
    cases: u64,
    steps: u64,
    /// Checks made with the map kept (not just rebuilt): the incremental path's.
    kept_checks: u64,
    /// Checks made right after the map was dropped (a restart or a purge): cold.
    cold_checks: u64,
    lines_compared: u64,
    ranges_compared: u64,
    /// Steps by kind, applied or refused.
    by_op: HashMap<&'static str, u64>,
    /// Lines blamed with a restoration, with an erased change, to kit:unknown, with an
    /// approval: that the fields the brief names were exercised.
    restored_lines: u64,
    erased_lines: u64,
    unknown_lines: u64,
    approval_lines: u64,
    /// Mismatches the session reported (`AuditEvent::ProvenanceMismatch`).
    reported: u64,
    /// Differences the differential check found (planted negatives only).
    found: u64,
}

impl Counts {
    fn add(&mut self, o: &Counts) {
        self.cases += o.cases;
        self.steps += o.steps;
        self.kept_checks += o.kept_checks;
        self.cold_checks += o.cold_checks;
        self.lines_compared += o.lines_compared;
        self.ranges_compared += o.ranges_compared;
        for (k, v) in &o.by_op {
            *self.by_op.entry(k).or_default() += v;
        }
        self.restored_lines += o.restored_lines;
        self.erased_lines += o.erased_lines;
        self.unknown_lines += o.unknown_lines;
        self.approval_lines += o.approval_lines;
        self.reported += o.reported;
        self.found += o.found;
    }
}

struct Case {
    rng: SmallRng,
    dir: PathBuf,
    ws: PathBuf,
    file: PathBuf,
    key: DocKey,
    host: Arc<TestHost>,
    config: Config,
    mgr: SessionManager,
    s: Arc<Session>,
    conns: Vec<ConnId>,
    /// Changes applied (for revert and purge).
    changes: Vec<u64>,
    /// A participant's last read: (version, text), for `write --base`.
    reads: HashMap<usize, (u64, String)>,
    log: Vec<String>,
    n: Counts,
}

impl Drop for Case {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn open(
    dir: &std::path::Path,
    ws: &std::path::Path,
    file: &std::path::Path,
    key: &DocKey,
    host: &Arc<TestHost>,
    config: &Config,
) -> (SessionManager, Arc<Session>) {
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    store.unsynced_for_tests();
    let mgr = SessionManager::new(store, host.clone(), config.clone());
    let s = mgr.open_file(Arc::new(MD), key, file, ws).unwrap();
    (mgr, s)
}

/// Pieces random text is made of: ASCII, line breaks, two- three- and four-byte characters
/// (the last two UTF-16 units: the engine's clocks).
const BITS: &[&str] = &["a", "b", "c", "\n", "é", "日", "😀", "xy\n", "ab", "\n\n"];

fn random_text(rng: &mut SmallRng, max: usize) -> String {
    let n = rng.gen_range(0..=max);
    (0..n).map(|_| BITS[rng.gen_range(0..BITS.len())]).collect()
}

/// A random char boundary of `t`.
fn boundary(rng: &mut SmallRng, t: &str) -> usize {
    let mut at = rng.gen_range(0..=t.len());
    while !t.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// Up to `max` random sorted, non-overlapping hunks on `t`.
fn random_hunks(rng: &mut SmallRng, t: &str, max: usize) -> Vec<Hunk> {
    let k = rng.gen_range(1..=max);
    let mut cuts: Vec<usize> = (0..2 * k).map(|_| boundary(rng, t)).collect();
    cuts.sort_unstable();
    let mut out = Vec::new();
    for p in cuts.chunks(2) {
        let (a, mut b) = (p[0], p[1]);
        // mostly short deletions
        if b - a > 12 && rng.gen_bool(0.7) {
            b = a;
        }
        if let Some(l) = out.last() {
            let l: &Hunk = l;
            if a < l.end {
                continue;
            }
        }
        out.push(Hunk {
            start: a,
            end: b,
            insert: random_text(rng, 3),
        });
    }
    out
}

fn apply(t: &str, hunks: &[Hunk]) -> String {
    let mut out = String::new();
    let mut at = 0;
    for h in hunks {
        out.push_str(&t[at..h.start]);
        out.push_str(&h.insert);
        at = h.end;
    }
    out.push_str(&t[at..]);
    out
}

impl Case {
    fn new(seed: u64, config: &Config) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "k2mb-blame-incr-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&dir);
        let ws = dir.join("ws");
        fs::create_dir_all(&ws).unwrap();
        let ws = fs::canonicalize(ws).unwrap();
        let mut rng = SmallRng::seed_from_u64(seed);
        let file = ws.join("d.md");
        let seed_text: String = (0..rng.gen_range(0..6))
            .map(|i| format!("line {i} {}\n", random_text(&mut rng, 2).replace('\n', "")))
            .collect();
        fs::write(&file, &seed_text).unwrap();
        let key = DocKey {
            space: "ws".into(),
            path: "d.md".into(),
        };
        let host = Arc::new(TestHost {
            now: AtomicU64::new(T0),
            events: Mutex::new(Vec::new()),
        });
        let mut config = config.clone();
        // compaction now and then (it keeps the map)
        if rng.gen_bool(0.3) {
            config.compact_entries = rng.gen_range(3..10);
        }
        let (mgr, s) = open(&dir, &ws, &file, &key, &host, &config);
        let mut c = Case {
            rng,
            dir,
            ws,
            file,
            key,
            host,
            config,
            mgr,
            s,
            conns: Vec::new(),
            changes: Vec::new(),
            reads: HashMap::new(),
            log: vec![format!("seed {seed}: {seed_text:?}")],
            n: Counts::default(),
        };
        c.join_all();
        c
    }

    fn join_all(&mut self) {
        self.conns = WHO
            .iter()
            .map(|(pid, kind, behalf)| {
                let mut p = Participant::new(*pid, pid.to_uppercase(), "#123456", *kind, "cli");
                if let Some(b) = behalf {
                    p = p.on_behalf_of(*b);
                }
                self.s.join(p, Stay::Held).unwrap().conn
            })
            .collect();
    }

    fn obs(&self) -> ConnId {
        self.conns[4]
    }

    fn text(&self) -> String {
        self.s.read(self.obs()).unwrap().text_by_root[0].1.clone()
    }

    fn count(&mut self, op: &'static str) {
        *self.n.by_op.entry(op).or_default() += 1;
    }

    /// One random step. Refusals are fine (a nothing-to-undo, a stale base...).
    fn step(&mut self, mutant: bool) {
        self.host
            .now
            .fetch_add(self.rng.gen_range(1..50), Ordering::SeqCst);
        // an editor: alice, bob or aide (and now and then the owner)
        let who = match self.rng.gen_range(0..10) {
            0..=3 => 0,
            4..=6 => 1,
            7..=8 => 2,
            _ => 3,
        };
        let conn = self.conns[who];
        let pick = self.rng.gen_range(0..100);
        match pick {
            0..=34 => {
                let t = self.text();
                let hunks = random_hunks(&mut self.rng, &t, 3);
                self.log.push(format!("{} edit {hunks:?}", WHO[who].0));
                let r = self.s.edit(
                    conn,
                    vec![Edit::Text {
                        root: ROOT.into(),
                        hunks,
                    }],
                    None,
                );
                if let Ok(r) = r {
                    self.changes.push(r.version);
                }
                self.count("edit");
            }
            35..=39 => {
                // write --base from this participant's last read, else a read now
                match self.reads.remove(&who) {
                    Some((v, base)) => {
                        let hunks = random_hunks(&mut self.rng, &base, 2);
                        let body = apply(&base, &hunks);
                        self.log
                            .push(format!("{} write --base {v} {body:?}", WHO[who].0));
                        let _ = self.s.write_base(conn, ROOT, v, &body);
                        self.count("write_base");
                    }
                    None => {
                        let r = self.s.read(conn).unwrap();
                        self.log.push(format!("{} read v{}", WHO[who].0, r.version));
                        self.reads
                            .insert(who, (r.version, r.text_by_root[0].1.clone()));
                        self.count("read");
                    }
                }
            }
            40..=51 => {
                // under a planted fault, one change per call: the check runs between changes
                let steps = if mutant { 1 } else { self.rng.gen_range(1..=2) };
                let mode = if self.rng.gen_bool(0.3) {
                    UndoMode::Partial
                } else {
                    UndoMode::All
                };
                self.log
                    .push(format!("{} undo {steps} {mode:?}", WHO[who].0));
                let _ = self
                    .s
                    .undo_with(conn, &UndoRequest::steps(steps).mode(mode));
                self.count("undo");
            }
            52..=59 => {
                let steps = if mutant { 1 } else { self.rng.gen_range(1..=2) };
                self.log.push(format!("{} redo {steps}", WHO[who].0));
                let _ = self.s.redo_with(conn, &UndoRequest::steps(steps));
                self.count("redo");
            }
            60..=66 if !self.changes.is_empty() => {
                let c = self.changes[self.rng.gen_range(0..self.changes.len())];
                let partial = self.rng.gen_bool(0.5);
                self.log
                    .push(format!("{} revert {c} partial={partial}", WHO[who].0));
                let _ = self.s.revert(conn, c, partial);
                self.count("revert");
            }
            67..=70 => {
                let vs = self.s.versions(conn).unwrap();
                if !vs.is_empty() {
                    let v = vs[self.rng.gen_range(0..vs.len())].id;
                    self.log.push(format!("{} restore {v:?}", WHO[who].0));
                    if let Ok(r) = self.s.restore(conn, v) {
                        self.changes.push(r.version);
                    }
                }
                self.count("restore");
            }
            71..=73 if !self.changes.is_empty() => {
                let own = self.conns[3];
                let req = if self.rng.gen_bool(0.6) {
                    let a = self.changes[self.rng.gen_range(0..self.changes.len())];
                    PurgeRequest::new(PurgeScope::Changes { from: a, to: a })
                } else {
                    let t = self.text();
                    let a = boundary(&mut self.rng, &t);
                    if a + 9 > t.len() {
                        return;
                    }
                    let mut b = a + 9;
                    while !t.is_char_boundary(b) {
                        b += 1;
                    }
                    let r = PurgeRequest::new(PurgeScope::Text(t[a..b].to_owned()));
                    if self.rng.gen_bool(0.5) {
                        r.and_remove()
                    } else {
                        r
                    }
                };
                self.log.push(format!("own purge {:?}", req.scope));
                let _ = self.s.purge(own, req);
                self.count("purge");
            }
            74..=84 => {
                // an outside edit: on the file as it is (after a write-back, or not)
                if self.rng.gen_bool(0.5) {
                    let _ = self.s.flush(self.obs());
                }
                let t = fs::read_to_string(&self.file).unwrap_or_default();
                let hunks = random_hunks(&mut self.rng, &t, 3);
                let new = apply(&t, &hunks);
                self.log.push(format!("outside edit {t:?} -> {new:?}"));
                fs::write(&self.file, &new).unwrap();
                let _ = self.s.file_changed();
                self.count("outside");
            }
            85..=87 => {
                // an assistant's change approved or declined by the person
                if let Some(&c) = self.changes.last() {
                    let st = if self.rng.gen_bool(0.5) {
                        ApprovalState::Approved
                    } else {
                        ApprovalState::Declined
                    };
                    self.log.push(format!("alice approval {c} {st:?}"));
                    let _ = self.s.set_approval(self.conns[0], c, st);
                }
                self.count("approval");
            }
            88..=90 => {
                self.log.push("flush".into());
                let _ = self.s.flush(self.obs());
                self.count("flush");
            }
            91..=93 => {
                self.log.push("restart".into());
                self.restart();
                self.count("restart");
            }
            _ => {
                self.log.push("tick".into());
                let _ = self.mgr.tick();
                self.count("tick");
            }
        }
        self.n.steps += 1;
    }

    fn restart(&mut self) {
        self.mgr.release().unwrap();
        let (mgr, s) = open(
            &self.dir,
            &self.ws,
            &self.file,
            &self.key,
            &self.host,
            &self.config,
        );
        self.mgr = mgr;
        self.s = s;
        self.reads.clear();
        self.join_all();
    }

    fn tally(&mut self, b: &Blame) {
        for l in &b.lines {
            self.n.restored_lines += u64::from(l.restored_by.is_some());
            self.n.erased_lines += u64::from(l.erased);
            self.n.unknown_lines += u64::from(l.author == "kit:unknown");
            self.n.approval_lines += u64::from(l.approval.is_some());
        }
    }

    /// The differential check after a step. `Err`: where the kept path and the rebuild
    /// differ.
    fn check(&mut self) -> Result<(), String> {
        let kept = self.s.prov_built_for_tests().unwrap();
        if kept {
            self.n.kept_checks += 1;
        } else {
            self.n.cold_checks += 1;
        }
        if let Some(d) = self.s.prov_differs_for_tests().unwrap() {
            return Err(format!("the kept map differs from a rebuild: {d}"));
        }
        let all = self.s.blame(self.obs(), ROOT, None).unwrap();
        let oracle = self.s.blame_rebuilt_for_tests(ROOT, None).unwrap();
        if all != oracle {
            return Err(format!(
                "blame differs:\n kept {all:?}\n rebuilt {oracle:?}"
            ));
        }
        self.n.lines_compared += all.lines.len() as u64;
        self.tally(&all);
        let n = all.lines.len();
        if n > 0 {
            let first = self.rng.gen_range(1..=n);
            let last = self.rng.gen_range(first..=n + 2);
            let r = || {
                Some(LineRange {
                    root: ROOT.into(),
                    first,
                    last,
                })
            };
            let a = self.s.blame(self.obs(), ROOT, r()).unwrap();
            let b = self.s.blame_rebuilt_for_tests(ROOT, r()).unwrap();
            if a != b {
                return Err(format!(
                    "blame -L {first},{last} differs:\n kept {a:?}\n rebuilt {b:?}"
                ));
            }
            self.n.ranges_compared += 1;
        }
        let m = self.host.mismatches();
        if !m.is_empty() {
            return Err(format!("a mismatch was reported: {m:?}"));
        }
        Ok(())
    }

    /// The runtime check after a step: blame (checked at 1 in 1) must be the rebuild's.
    fn served(&mut self) -> Result<(), String> {
        let all = self.s.blame(self.obs(), ROOT, None).unwrap();
        let oracle = self.s.blame_rebuilt_for_tests(ROOT, None).unwrap();
        if all != oracle {
            return Err(format!(
                "a wrong blame was served:\n served {all:?}\n rebuilt {oracle:?}"
            ));
        }
        self.n.lines_compared += all.lines.len() as u64;
        Ok(())
    }

    fn fail(&self, seed: u64, why: &str) -> ! {
        panic!("seed {seed}: {why}\nsteps:\n  {}", self.log.join("\n  "));
    }
}

fn env(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn config(check_every: u32) -> Config {
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    c.blame_check_every = check_every;
    c
}

/// Run `cases` histories from `seed0` on `threads` threads.
fn run(
    seed0: u64,
    cases: u64,
    steps: u64,
    threads: u64,
    mode: Mode,
    mutant: Option<ProvMutant>,
) -> Counts {
    let next = Arc::new(AtomicU64::new(0));
    let total = Arc::new(Mutex::new(Counts::default()));
    let workers: Vec<_> = (0..threads.max(1))
        .map(|_| {
            let (next, total) = (next.clone(), total.clone());
            std::thread::spawn(move || loop {
                let k = next.fetch_add(1, Ordering::SeqCst);
                if k >= cases {
                    break;
                }
                let seed = seed0.wrapping_add(k);
                let n = one(seed, steps, mode, mutant);
                total.lock().unwrap().add(&n);
            })
        })
        .collect();
    for w in workers {
        w.join().unwrap();
    }
    let n = total.lock().unwrap().clone();
    n
}

/// One history. Without a mutant, any difference fails; with one, a difference is counted
/// and the history stops (`found`).
fn one(seed: u64, steps: u64, mode: Mode, mutant: Option<ProvMutant>) -> Counts {
    let cfg = config(match mode {
        Mode::Differential => 0,
        Mode::Runtime => 1,
    });
    let mut c = Case::new(seed, &cfg);
    let body = |c: &mut Case| {
        for _ in 0..steps {
            c.step(mutant.is_some());
            let r = match mode {
                Mode::Differential => c.check(),
                Mode::Runtime => c.served(),
            };
            match (r, mutant, mode) {
                (Ok(()), _, _) => {}
                (Err(why), None, _) | (Err(why), Some(_), Mode::Runtime) => c.fail(seed, &why),
                (Err(_), Some(_), Mode::Differential) => {
                    c.n.found += 1;
                    return;
                }
            }
        }
    };
    match mutant {
        Some(m) => with_prov_mutant(m, || body(&mut c)),
        None => body(&mut c),
    }
    c.n.reported += c.host.mismatches().len() as u64;
    c.n.cases += 1;
    c.n.clone()
}

/// The differential test: the kept map and blame from it equal the rebuild's after every
/// step of every history.
#[test]
fn blame_kept_map_equals_the_rebuild() {
    let cases = env("BLAME_DIFF_CASES", 256);
    let steps = env("BLAME_DIFF_STEPS", 24);
    let threads = env("BLAME_DIFF_THREADS", 1);
    let seed = env("BLAME_DIFF_SEED", 0);
    let t0 = std::time::Instant::now();
    let n = run(seed, cases, steps, threads, Mode::Differential, None);
    let secs = t0.elapsed().as_secs();
    eprintln!("[blame diff] {n:?}");
    assert_eq!(n.cases, cases);
    assert_eq!(n.reported, 0);
    // the incremental path is what was checked, and the brief's fields were exercised
    assert!(n.kept_checks > n.cold_checks, "{n:?}");
    if cases >= 64 {
        for op in [
            "edit", "undo", "redo", "revert", "restore", "purge", "outside", "restart",
        ] {
            assert!(
                n.by_op.get(op).copied().unwrap_or(0) > 0,
                "no {op} step: {n:?}"
            );
        }
        assert!(n.restored_lines > 0 && n.erased_lines > 0, "{n:?}");
        assert!(n.approval_lines > 0, "{n:?}");
    }
    eprintln!(
        "RUNREC {{\"job\":null,\"run\":\"blame_diff\",\"tool\":\"blame_diff\",\"status\":\"COMPLETE\",\"cases\":{},\"steps\":{},\"kept_checks\":{},\"cold_checks\":{},\"lines_compared\":{},\"ranges_compared\":{},\"restored_lines\":{},\"erased_lines\":{},\"unknown_lines\":{},\"approval_lines\":{},\"outside_edits\":{},\"reported\":{},\"seed\":{seed},\"wall_s\":{secs}}}",
        n.cases,
        n.steps,
        n.kept_checks,
        n.cold_checks,
        n.lines_compared,
        n.ranges_compared,
        n.restored_lines,
        n.erased_lines,
        n.unknown_lines,
        n.approval_lines,
        n.by_op.get("outside").copied().unwrap_or(0),
        n.reported,
    );
}

/// Histories the differential test once failed, kept as regressions:
/// - 5518 (v0.9.5, before its fix): an outside edit merged into a doc with an unsynced
///   edit left the text as it was on new elements (`xy` deleted and typed again by the
///   outside edit); the root's text was unchanged, so the map wasn't updated.
const REGRESSIONS: &[u64] = &[5518];

#[test]
fn blame_regression_seeds() {
    for &seed in REGRESSIONS {
        let n = one(seed, 24, Mode::Differential, None);
        assert_eq!(n.reported, 0, "seed {seed}: {n:?}");
    }
}

const MUTANTS: &[(ProvMutant, &str)] = &[
    (ProvMutant::MissDeletion, "miss_deletion"),
    (ProvMutant::MissDeletionInIndex, "miss_deletion_in_index"),
    (ProvMutant::RestoredAsRestorer, "restored_as_restorer"),
];

/// Planted negatives, differential side: each planted fault is found, in most histories
/// long enough to make it show (`BLAME_MUTANT_CASES`, default 64).
#[test]
fn blame_differential_catches_planted_faults() {
    let cases = env("BLAME_MUTANT_CASES", 64);
    for &(m, name) in MUTANTS {
        let n = run(1_000_000, cases, 40, 1, Mode::Differential, Some(m));
        eprintln!(
            "[blame mutant {name}] found in {} of {} histories",
            n.found, n.cases
        );
        assert!(n.found > 0, "{name}: never found ({n:?})");
        eprintln!(
            "RUNREC {{\"job\":null,\"run\":\"blame_mutant_diff_{name}\",\"tool\":\"blame_diff\",\"status\":\"CAUGHT\",\"cases\":{},\"found\":{}}}",
            n.cases, n.found
        );
    }
}

/// Planted negatives, runtime side: with the check at 1 in 1, each planted fault is
/// reported (`AuditEvent::ProvenanceMismatch`) and never served: every blame equals the
/// rebuild's (a wrong one fails the history).
#[test]
fn blame_runtime_check_catches_planted_faults() {
    let cases = env("BLAME_MUTANT_CASES", 64);
    for &(m, name) in MUTANTS {
        let n = run(2_000_000, cases, 40, 1, Mode::Runtime, Some(m));
        eprintln!(
            "[blame mutant {name}] runtime check: {} mismatches reported over {} histories, \
             {} lines served, all equal to the rebuild's",
            n.reported, n.cases, n.lines_compared
        );
        assert!(n.reported > 0, "{name}: never reported ({n:?})");
        eprintln!(
            "RUNREC {{\"job\":null,\"run\":\"blame_mutant_runtime_{name}\",\"tool\":\"blame_diff\",\"status\":\"CAUGHT\",\"cases\":{},\"reported\":{},\"lines_served\":{}}}",
            n.cases, n.reported, n.lines_compared
        );
    }
}

/// The runtime check's sampling: at 1 in 3, a planted fault that shows is reported on the
/// third blame, not before (the length check aside), and blame is the rebuild's from then.
/// And a sound map is never reported.
#[test]
fn blame_runtime_check_samples() {
    // restored-as-restorer: the runs stay aligned, so only the sampled check sees it
    let cfg = config(3);
    let c = Case::new(7, &cfg);
    let a = c.conns[0];
    let obs = c.obs();
    with_prov_mutant(ProvMutant::RestoredAsRestorer, || {
        c.s.edit(
            a,
            vec![Edit::Text {
                root: ROOT.into(),
                hunks: vec![Hunk {
                    start: 0,
                    end: 0,
                    insert: "alice wrote this\n".into(),
                }],
            }],
            None,
        )
        .unwrap();
        // the first blame builds the map (cold): it is exact
        let b1 = c.s.blame(obs, ROOT, None).unwrap();
        assert_eq!(b1.lines[0].author, "alice");
        // alice deletes her line and undoes it: the planted fault records the restored
        // text as her undo's own
        let t = c.text();
        let end = t.find('\n').unwrap() + 1;
        c.s.edit(
            a,
            vec![Edit::Text {
                root: ROOT.into(),
                hunks: vec![Hunk {
                    start: 0,
                    end,
                    insert: String::new(),
                }],
            }],
            None,
        )
        .unwrap();
        c.s.undo(a, 1).unwrap();
        // blame 2: not sampled, the wrong map is read (no restoration named)
        let b2 = c.s.blame(obs, ROOT, None).unwrap();
        assert_eq!(b2.lines[0].restored_by, None);
        assert!(c.host.mismatches().is_empty());
        // blame 3: sampled; the rebuild is served (restored by alice) and the mismatch reported
        let b3 = c.s.blame(obs, ROOT, None).unwrap();
        assert_eq!(b3.lines[0].restored_by.as_deref(), Some("alice"));
        assert_eq!(b3, c.s.blame_rebuilt_for_tests(ROOT, None).unwrap());
        let m = c.host.mismatches();
        assert_eq!(m.len(), 1, "{m:?}");
        assert!(m[0].contains("runs differ"), "{m:?}");
    });
    // a sound map, checked at 1 in 1 through a history: never reported
    let n = run(3_000_000, 16, 30, 1, Mode::Runtime, None);
    assert_eq!(n.reported, 0, "{n:?}");
}

/// The sampled check runs off the session's lock (v0.9.7): another participant's edit or a
/// purge can land between the snapshot it checks and its verdict. With a planted fault that
/// shows (restored-as-restorer), and the check at 1 in 1:
/// - an edit lands meanwhile: the map checked is still the kept one, changed since, so it is
///   reported and dropped (not replaced by a rebuild of an older doc), and the blame served
///   is rebuilt from the doc as it is now (the edit included);
/// - a purge lands meanwhile (it replaces the map): the verdict is about a map that is gone,
///   so nothing is reported, and the blame served is the rebuild's.
#[test]
fn blame_check_off_the_lock() {
    for purge in [false, true] {
        let cfg = config(1);
        let c = Case::new(11, &cfg);
        let (a, obs, own) = (c.conns[0], c.obs(), c.conns[3]);
        let line = |s: &Session, start: usize, end: usize, insert: &str| {
            s.edit(
                a,
                vec![Edit::Text {
                    root: ROOT.into(),
                    hunks: vec![Hunk {
                        start,
                        end,
                        insert: insert.into(),
                    }],
                }],
                None,
            )
            .unwrap()
            .version
        };
        let v1 = line(&c.s, 0, 0, "alice wrote this\n");
        // cold: builds the map, not sampled
        c.s.blame(obs, ROOT, None).unwrap();
        with_prov_mutant(ProvMutant::RestoredAsRestorer, || {
            line(&c.s, 0, 17, "");
            c.s.undo(a, 1).unwrap();
        });
        let s2 = c.s.clone();
        let hook = move || {
            if purge {
                s2.purge(
                    own,
                    PurgeRequest::new(PurgeScope::Changes { from: v1, to: v1 }),
                )
                .unwrap();
            } else {
                s2.edit(
                    a,
                    vec![Edit::Text {
                        root: ROOT.into(),
                        hunks: vec![Hunk {
                            start: 0,
                            end: 0,
                            insert: "meanwhile\n".into(),
                        }],
                    }],
                    None,
                )
                .unwrap();
            }
        };
        let b = with_blame_check_hook(hook, || c.s.blame(obs, ROOT, None).unwrap());
        assert_eq!(b, c.s.blame_rebuilt_for_tests(ROOT, None).unwrap());
        let alice = b
            .lines
            .iter()
            .find(|l| l.text == "alice wrote this")
            .unwrap();
        assert_eq!(alice.restored_by.as_deref(), Some("alice"), "{b:?}");
        let m = c.host.mismatches();
        if purge {
            assert!(m.is_empty(), "{m:?}");
        } else {
            assert_eq!(b.lines[0].text, "meanwhile", "{b:?}");
            assert_eq!(m.len(), 1, "{m:?}");
        }
        // the map kept from here on is sound
        assert_eq!(c.s.prov_differs_for_tests().unwrap(), None);
    }
}

/// The adaptive rate (v0.9.7): below `blame_check_large_at` changes, one blame in
/// `blame_check_every` is checked; from it on, one in `blame_check_every_large`. A cold blame
/// (the map built from scratch) is never also checked.
#[test]
fn blame_check_rate_adapts_to_history() {
    let mut cfg = config(2);
    cfg.blame_check_large_at = 12;
    cfg.blame_check_every_large = 5;
    let c = Case::new(21, &cfg);
    let (a, obs) = (c.conns[0], c.obs());
    let edit = |n: usize| {
        for i in 0..n {
            c.s.edit(
                a,
                vec![Edit::Text {
                    root: ROOT.into(),
                    hunks: vec![Hunk {
                        start: 0,
                        end: 0,
                        insert: format!("line {i}\n"),
                    }],
                }],
                None,
            )
            .unwrap();
        }
    };
    edit(1);
    let v = |c: &Case| c.s.read(c.obs()).unwrap().version;
    assert!(v(&c) < 12);
    // blame 1 builds the map (cold, sampled or not: no check), then 2..=8: every 2nd
    for _ in 0..8 {
        c.s.blame(obs, ROOT, None).unwrap();
    }
    assert_eq!(
        c.s.blame_checks_for_tests().unwrap(),
        4,
        "blames 2, 4, 6, 8"
    );
    // past 12 changes: every 5th blame (blames 9..=28: 10, 15, 20, 25)
    edit(12);
    assert!(v(&c) >= 12);
    for _ in 0..20 {
        c.s.blame(obs, ROOT, None).unwrap();
    }
    assert_eq!(
        c.s.blame_checks_for_tests().unwrap(),
        4 + 4,
        "blames 10, 15, 20, 25"
    );
    assert!(c.host.mismatches().is_empty());
    // the defaults
    let d = Config::default();
    assert_eq!(
        (
            d.blame_check_every,
            d.blame_check_large_at,
            d.blame_check_every_large
        ),
        (100, 20_000, 1_000)
    );
}
