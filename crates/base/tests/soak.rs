//! The 32-participant soak: one doc in a file, 32 participants (16 people, 16 agents) on
//! several connections each, doing everything a session offers in a seeded, deterministic
//! mix (reads, every edit verb, `write --base`, guarded edits, undo / redo / `--partial`,
//! claims, cursors, leave and rejoin, ticks, flushes, full restarts) for a set time, with
//! the invariants checked as it goes:
//!
//! - **no panic** (any panic fails the test);
//! - **nothing acknowledged is lost** (K-AUD-1 style): every line is a unique token naming
//!   its writer. After every call, the lines that appeared and went are explained by that
//!   call alone: a refused or failed call (or a read, a claim, a leave, a tick, a restart)
//!   changes nothing; an acknowledged insert is there; a delete or replace removes only its
//!   target lines; an undo or redo removes only lines its participant's own changes made
//!   appear and puts back only lines its participant's own changes removed (the v0.9 promise);
//!   an agent's edit under `ClaimPolicy::Respect` never removes a line whose text another
//!   participant's claim holds; every applied change moves the version on;
//! - **store reload equals live state**: periodically every handle is released and the
//!   store reopened; the text and version must match, and everyone rejoins;
//! - **the file equals the doc** after a flush, and so does the disk replica, element for
//!   element (K-AUD-1's sync check, v0.9.4); blame (by root) gives every line, with a writer
//!   who wrote in this doc;
//! - **outside saves** (v0.9.5): a plain editor reads the file, edits it and saves it, often
//!   inside the write delay while the doc has changes the file doesn't: a line added or
//!   deleted, a line added and taken out again, a line deleted and typed back (two saves
//!   each), or an older write of the kit's saved back from a stale buffer. Each save must be
//!   taken in exactly (its lines there or gone, nothing else lost: the same line oracle), or,
//!   for a write of the kit's made since the last outside save, suppressed as an echo and
//!   reported; K-AUD-1 runs after every save;
//! - **claims and presence**: two participants' claims never overlap (a new one, and carried
//!   ones: text typed where two meet joins one of them; calls after which two shared an
//!   edge are counted), every claim belongs to a live connection, and the roster is exactly
//!   who is connected, with their connection counts;
//! - **bounded memory**: the kit's own estimate and the process's resident set are
//!   sampled; the estimate stays within the configured budget, and both are reported.
//!
//! `soak_short` runs in CI (5 s, `SOAK_SHORT_SECS`). `soak_32` is the full run, ignored:
//! `SOAK_SECS` (default 600) and `SOAK_SEED` (default 1); run it in release:
//!
//! ```sh
//! SOAK_SECS=600 cargo test -p k2-mpc-base --release --test soak -- --ignored --nocapture
//! ```
//!
//! It prints a progress line every `SOAK_PROGRESS_SECS` (default 30) and a summary. The
//! summary also gives the cost **at equal history** (v0.9.7): at every multiple of
//! `SOAK_MILESTONE` versions (default 10,000), the mean wall time per call and per `tick` over
//! the `SOAK_MILESTONE_CALLS` calls before it (default 1,000; a run record each), and the
//! least-squares slope of the per-call cost against the version. A run that gets further in
//! its time isn't penalised for it, as the first-window/last-150-s ratio is; the node's load
//! still scales every figure.
//!
//! **Shadow mode** (`SOAK_SHADOW=1`, v0.9.3): every engine edit the run makes (every verb,
//! undo, redo, outside edit) is also made through today's throwaway replica on a twin of the
//! doc, and any difference in the update, state, state vector or text fails the run
//! ([`k2_mpc_base::differential`]). It costs a full copy of the doc per edit, so the
//! throughput check is skipped in this mode.
//!
//! **The old write-back** (`SOAK_FULL_WRITEBACK=1`, v0.9.7): every file write-back re-encodes
//! the doc and reloads the disk replica from it, as before v0.9.7, instead of keeping the
//! replica in step. A control for the incremental write-back's effect on the soak.
//!
//! `SOAK_EDIT_CHECK_EVERY=N` sets the runtime checks' rate (default the kit's): with 1, every
//! outside save also computes its change at element identity in full, and a difference from
//! the merge's hunks (`AuditEvent::IdentityMismatch`) fails the run (as it does at any rate).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::convert::Infallible;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use k2_mpc_base::differential::{with_side, Path, Side};
use k2_mpc_base::session::{
    At, AuditEvent, ClaimPolicy, Config, ConnId, DiskCheck, EditOptions, EditWithError, Host,
    LineRange, Participant, ParticipantKind, Rights, Session, SessionError, SessionManager, Stay,
    TextFilePack, UndoMode, UndoOutcome, UndoRequest, WriteOutcome, EXTERNAL_PID,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::tools::{ToolOptions, ToolRunError};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

const ROOT: &str = TextFilePack::ROOT;
const PEOPLE: usize = 16;
const AGENTS: usize = 16;
const SEED_LINES: usize = 40;
/// Deletes win over inserts above this many lines, so the doc stays bounded.
const MAX_LINES: usize = 300;
const MIN_LINES: usize = 8;

// ---------- the host ----------

struct SoakHost {
    now: AtomicU64,
    /// Every runtime check's mismatch the kit reported (v0.9.7): any fails the run.
    mismatches: std::sync::Mutex<Vec<String>>,
}

impl Host for SoakHost {
    fn rights(&self, _who: &Participant, _key: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn on_event(&self, ev: &AuditEvent) {
        if matches!(
            ev,
            AuditEvent::EditPathMismatch { .. }
                | AuditEvent::TextCacheMismatch { .. }
                | AuditEvent::PositionIndexMismatch { .. }
                | AuditEvent::DiskReplicaMismatch { .. }
                | AuditEvent::IdentityMismatch { .. }
        ) {
            self.mismatches.lock().unwrap().push(format!("{ev:?}"));
        }
    }
}

// ---------- the world ----------

struct Part {
    pid: String,
    kind: ParticipantKind,
    conns: Vec<ConnId>,
    max_conns: usize,
    next: u64,
    /// The last read: (version, text), for `write --base` and guarded edits.
    last_read: Option<(u64, String)>,
}

/// What a call may have done to the lines (checked against what it did).
#[derive(Debug)]
enum Expect {
    /// Refused, failed, or not an edit: nothing changes.
    Nothing,
    /// Only these new lines appear (all of them, when it applied); nothing goes.
    Insert(Vec<String>),
    /// Only lines of `targets` go; only `new` lines appear.
    Remove {
        targets: HashSet<String>,
        new: Vec<String>,
    },
    /// An undo or redo by this participant.
    History(String),
    /// An outside save taken in (v0.9.5): exactly these lines go and these appear.
    Saved {
        targets: HashSet<String>,
        new: Vec<String>,
    },
}

#[derive(Default)]
struct Counts {
    ops: u64,
    applied: u64,
    refused: BTreeMap<String, u64>,
    by_op: BTreeMap<&'static str, u64>,
    restarts: u64,
    blames: u64,
    flushes: u64,
    joins: u64,
    leaves: u64,
    peak_estimate: u64,
    peak_rss_kb: u64,
    rss_samples: Vec<(u64, u64)>,
    max_lines: usize,
    blame_lines: u64,
    blame_newline: u64,
    /// Checks (one per call) that saw two participants' claims share an edge (never
    /// overlap: asserted).
    shared_edges: u64,
    /// Outside saves taken in, and older kit writes saved back and suppressed as echoes.
    outside_saves: u64,
    echoes: u64,
    /// Undo and redo calls whose mismatch with the line oracle was the known D1 gap
    /// (tolerated: see `Soak::d1`), and the lines they left written twice.
    d1_tolerated: u64,
    /// The calls at which they happened (the replay regression asserts them).
    d1_calls: Vec<u64>,
    dups: usize,
    /// Per progress window: (seconds since start at its end, calls, wall seconds).
    windows: Vec<(f64, u64, f64)>,
    /// The last calls' wall seconds (and the call's own seconds when it was a tick), for the
    /// milestones (v0.9.7).
    recent: std::collections::VecDeque<(f64, Option<f64>)>,
    next_milestone: u64,
    /// At each version milestone: the cost of the calls just before it.
    milestones: Vec<Milestone>,
    /// The kit's profile (its scopes' entries and seconds) between consecutive milestones,
    /// one per milestone (v0.9.7).
    profiles: Vec<Vec<(&'static str, u64, f64)>>,
    /// The engine's position index counters, summed over the session's loads.
    index: k2_mpc_base::differential::PositionIndexStats,
    /// The last calls, for a failure's dump (v0.9.7): (op number, version after, what).
    last_ops: std::collections::VecDeque<(u64, u64, String)>,
}

/// The soak's cost at a version milestone (v0.9.7): over the `SOAK_MILESTONE_CALLS` calls
/// (default 1,000) before the doc's version first reached it, the mean wall time per call
/// (every phase: the call, the audit, presence, blame, restarts) and per `tick` call (where
/// the file write-back runs). Measured at equal history, not equal time, so a run that gets
/// further in its 600 s isn't penalised for it; the node's load still scales every figure.
#[derive(Debug, Clone, Copy)]
struct Milestone {
    at: u64,
    version: u64,
    us_per_call: f64,
    us_per_tick: f64,
    ticks: usize,
    /// The doc's history in UTF-16 units when the version reached it (the canary's measure
    /// of its size, `Config::edit_check_scale_units`).
    history_units: u64,
}

impl Counts {
    /// Count a call (its wall seconds, and its own seconds if it was a tick) at `version`,
    /// recording every milestone (multiples of `every`) the version passed, with the doc's
    /// history (`units`, read only when a milestone is recorded).
    fn track(
        &mut self,
        wall: f64,
        tick: Option<f64>,
        version: u64,
        every: u64,
        keep: usize,
        units: impl Fn() -> u64,
    ) {
        self.recent.push_back((wall, tick));
        if self.recent.len() > keep {
            self.recent.pop_front();
        }
        if self.next_milestone == 0 {
            self.next_milestone = every;
        }
        while version >= self.next_milestone {
            if self.recent.len() == keep {
                let n = self.recent.len() as f64;
                let wall: f64 = self.recent.iter().map(|r| r.0).sum();
                let ticks: Vec<f64> = self.recent.iter().filter_map(|r| r.1).collect();
                self.milestones.push(Milestone {
                    at: self.next_milestone,
                    version,
                    us_per_call: wall / n * 1e6,
                    us_per_tick: if ticks.is_empty() {
                        0.0
                    } else {
                        ticks.iter().sum::<f64>() / ticks.len() as f64 * 1e6
                    },
                    ticks: ticks.len(),
                    history_units: units(),
                });
                self.profiles
                    .push(k2_mpc_base::differential::profile_take());
            }
            self.next_milestone += every;
        }
    }
}

/// The least-squares slope of µs per call against the version, in µs per 1,000 versions.
fn milestone_slope(m: &[Milestone]) -> Option<f64> {
    if m.len() < 2 {
        return None;
    }
    let n = m.len() as f64;
    let xs: Vec<f64> = m.iter().map(|x| x.at as f64 / 1000.0).collect();
    let mx = xs.iter().sum::<f64>() / n;
    let my = m.iter().map(|x| x.us_per_call).sum::<f64>() / n;
    let sxx: f64 = xs.iter().map(|x| (x - mx) * (x - mx)).sum();
    let sxy: f64 = xs
        .iter()
        .zip(m)
        .map(|(x, y)| (x - mx) * (y.us_per_call - my))
        .sum();
    (sxx > 0.0).then(|| sxy / sxx)
}

/// Where one progress window's time went: per phase of the loop (the harness's own work
/// apart from the kit call), and per kind of call (the kit call alone).
#[derive(Default)]
struct Window {
    ops: u64,
    /// Seconds: advance + claims, the call, audit, presence, file + blame, restart.
    phase: [f64; 6],
    /// Per kind of call: (calls, seconds in the call).
    by_op: BTreeMap<&'static str, (u64, f64)>,
}

const PHASES: [&str; 6] = ["pre", "call", "audit", "presence", "blame", "restart"];

impl Window {
    fn add(&mut self, op: &'static str, t: [Instant; 7]) {
        self.ops += 1;
        for k in 0..6 {
            self.phase[k] += (t[k + 1] - t[k]).as_secs_f64();
        }
        let e = self.by_op.entry(op).or_default();
        e.0 += 1;
        e.1 += (t[2] - t[1]).as_secs_f64();
    }

    fn wall(&self) -> f64 {
        self.phase.iter().sum()
    }

    /// `N calls/s; ms per call: pre a, call b, ...; µs per call by kind: insert x, ...`
    fn report(&self) -> String {
        let n = self.ops.max(1) as f64;
        let phases: Vec<String> = PHASES
            .iter()
            .zip(self.phase)
            .map(|(p, s)| format!("{p} {:.3}", s * 1e3 / n))
            .collect();
        let ops: Vec<String> = self
            .by_op
            .iter()
            .map(|(op, (c, s))| format!("{op} {:.0}", s * 1e6 / (*c).max(1) as f64))
            .collect();
        format!(
            "{:.0} calls/s; ms per call: {}; µs per call by kind: {}",
            self.ops as f64 / self.wall().max(1e-9),
            phases.join(", "),
            ops.join(", ")
        )
    }
}

struct Soak {
    rng: StdRng,
    dir: std::path::PathBuf,
    file: std::path::PathBuf,
    key: DocKey,
    host: Arc<SoakHost>,
    config: Config,
    mgr: SessionManager,
    s: Arc<Session>,
    parts: Vec<Part>,
    /// The checker's view: the live lines after the last call.
    lines: Vec<String>,
    version: u64,
    /// Token → its writer.
    author: HashMap<String, String>,
    /// Per pid: lines its changes made appear, and lines its changes removed.
    appeared_by: HashMap<String, HashSet<String>>,
    removed_by: HashMap<String, HashSet<String>>,
    n: Counts,
    /// The plain editor (v0.9.5): its next token, the file as last seen, and the kit's
    /// writes seen since the last outside save (bytes, when), the stale buffers it may save.
    editor_next: u64,
    last_file: Vec<u8>,
    kit_writes: Vec<(Vec<u8>, u64)>,
    /// Who made the call being audited, when not a participant (the outside editor).
    actor: Option<String>,
    /// The last calls, for a failure's dump: (op number, version after, what).
    last_ops: std::collections::VecDeque<(u64, u64, String)>,
    /// The undo or redo being audited had a region that is not whole lines (the known D1
    /// gap, amendment pending; v0.9.5). Every soak call writes whole lines, so such a region
    /// means the change's character diff reused characters already there, possibly another
    /// participant's: undoing it can change text someone else wrote. Only then is the undo
    /// promise's mismatch tolerated: counted and printed with its call number. The mode is
    /// `SOAK_KNOWN_D1` (default 1 in v0.9.5; 0 fails it); v0.9.7, which closes the gap,
    /// removes it.
    d1: bool,
    /// The run's knobs, from the environment by default (`SOAK_MAX_OPS`, `SOAK_OUTSIDE`,
    /// `SOAK_KNOWN_D1`); the replay regression sets its own, whatever the job's env.
    max_ops: u64,
    outside: bool,
    known_d1: bool,
}

fn lines_of(text: &str) -> Vec<String> {
    text.split_terminator('\n').map(str::to_owned).collect()
}

fn rss_kb() -> u64 {
    if let Ok(s) = std::fs::read_to_string("/proc/self/status") {
        for l in s.lines() {
            if let Some(v) = l.strip_prefix("VmRSS:") {
                return v.trim().trim_end_matches(" kB").trim().parse().unwrap_or(0);
            }
        }
        return 0;
    }
    std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

fn soak_config() -> Config {
    let mut c = Config::default();
    // 32 participants typing as fast as a loop: no rate refusals, no idle leaves; claims
    // and time otherwise as configured
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    c.idle_leave_ms = u64::MAX / 4;
    // diagnosis (v0.9.7): the position index's shadow check rate
    if let Ok(v) = std::env::var("SOAK_POSITION_CHECK_EVERY") {
        c.position_check_every = v.parse().expect("SOAK_POSITION_CHECK_EVERY: a number");
    }
    // SOAK_EDIT_CHECK_EVERY: the runtime checks' rate (edits and outside saves; default the
    // kit's, 1 in 1,000). 1 checks every outside save's change at identity in full.
    if let Ok(v) = std::env::var("SOAK_EDIT_CHECK_EVERY") {
        c.edit_check_every = v.parse().expect("SOAK_EDIT_CHECK_EVERY: a number");
    }
    c
}

impl Soak {
    fn new(tag: &str, seed: u64) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "k2mb-soak-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("files")).unwrap();
        let file = dir.join("files").join("notes.md");
        let seed_text: String = (0..SEED_LINES).map(|i| format!("seed:{i}\n")).collect();
        std::fs::write(&file, &seed_text).unwrap();
        let host = Arc::new(SoakHost {
            now: AtomicU64::new(1_000_000),
            mismatches: std::sync::Mutex::new(Vec::new()),
        });
        let key = DocKey {
            space: "ws".into(),
            path: "notes.md".into(),
        };
        let config = soak_config();
        let (mgr, s) = Self::open(&dir, &file, &key, &host, &config);
        let mut parts = Vec::new();
        for i in 0..PEOPLE + AGENTS {
            let person = i < PEOPLE;
            parts.push(Part {
                pid: format!("p{i:02}"),
                kind: if person {
                    ParticipantKind::Person
                } else {
                    ParticipantKind::Agent
                },
                conns: Vec::new(),
                // people: a tab and a CLI; agents: one or two
                max_conns: if person || i % 3 == 0 { 2 } else { 1 },
                next: 0,
                last_read: None,
            });
        }
        let mut author = HashMap::new();
        for i in 0..SEED_LINES {
            author.insert(format!("seed:{i}"), "kit:import".to_owned());
        }
        let mut w = Soak {
            rng: StdRng::seed_from_u64(seed),
            dir,
            file,
            key,
            host,
            config,
            mgr,
            s,
            parts,
            lines: lines_of(&seed_text),
            version: 0,
            author,
            appeared_by: HashMap::new(),
            removed_by: HashMap::new(),
            n: Counts::default(),
            editor_next: 0,
            last_file: seed_text.clone().into_bytes(),
            kit_writes: Vec::new(),
            actor: None,
            last_ops: std::collections::VecDeque::new(),
            d1: false,
            max_ops: env_u64("SOAK_MAX_OPS", 0),
            outside: env_u64("SOAK_OUTSIDE", 1) != 0,
            known_d1: env_u64("SOAK_KNOWN_D1", 1) != 0,
        };
        for i in 0..w.parts.len() {
            w.join(i);
        }
        w.version = w.s.status().unwrap().version;
        w
    }

    fn open(
        dir: &std::path::Path,
        file: &std::path::Path,
        key: &DocKey,
        host: &Arc<SoakHost>,
        config: &Config,
    ) -> (SessionManager, Arc<Session>) {
        let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
        let mgr = SessionManager::new(store, host.clone(), config.clone());
        let s = mgr
            .open_file(
                Arc::new(TextFilePack::new("markdown", 4 << 20)),
                key,
                file,
                &dir.join("files"),
            )
            .unwrap();
        (mgr, s)
    }

    fn join(&mut self, i: usize) {
        let p = &self.parts[i];
        let via = if p.conns.is_empty() && p.kind == ParticipantKind::Person {
            "tab"
        } else {
            "cli"
        };
        let who = Participant::new(
            p.pid.clone(),
            p.pid.to_uppercase(),
            format!("#{:06x}", (i as u32 + 1) * 0x0a0b0c % 0xffffff),
            p.kind,
            via,
        );
        let conn = self.s.join(who, Stay::Held).unwrap().conn;
        self.parts[i].conns.push(conn);
        self.n.joins += 1;
    }

    /// The observer: p00's first connection, never closed (re-made at each restart).
    fn obs(&self) -> ConnId {
        self.parts[0].conns[0]
    }

    fn live(&self) -> (Vec<String>, u64) {
        let r = self.s.read(self.obs()).unwrap();
        (lines_of(&r.text_by_root[0].1), r.version)
    }

    /// The next unique token of participant `i`. A token is never a text the doc already
    /// has had: a known D1 undo can rebuild characters into a text no one wrote yet (seed 3,
    /// call 25,877 rebuilt `p16:347` before p16 issued it), so such texts are skipped.
    fn token(&mut self, i: usize) -> String {
        let p = &mut self.parts[i];
        loop {
            p.next += 1;
            let t = format!("{}:{}", p.pid, p.next);
            if !self.author.contains_key(&t) {
                self.author.insert(t.clone(), p.pid.clone());
                return t;
            }
        }
    }

    /// The plain editor's next unique token (as [`Soak::token`]).
    fn editor_token(&mut self) -> String {
        loop {
            self.editor_next += 1;
            let t = format!("ed:{}", self.editor_next);
            if !self.author.contains_key(&t) {
                self.author.insert(t.clone(), EXTERNAL_PID.to_owned());
                return t;
            }
        }
    }

    fn advance(&mut self) {
        let ms = if self.rng.gen_ratio(1, 200) {
            // now and then a long pause: claims and for-edit holds run out
            self.rng.gen_range(30_000..120_000)
        } else {
            self.rng.gen_range(0..400)
        };
        self.host.now.fetch_add(ms, Ordering::SeqCst);
    }

    fn policy(&mut self, i: usize) -> ClaimPolicy {
        if self.parts[i].kind == ParticipantKind::Agent && self.rng.gen_bool(0.5) {
            ClaimPolicy::Respect
        } else {
            ClaimPolicy::Ignore
        }
    }

    fn refused(&mut self, what: String) {
        *self.n.refused.entry(what).or_default() += 1;
    }

    fn session_refusal(&mut self, e: &SessionError) {
        let name = format!("{e:?}");
        let name = name
            .split(|c: char| !c.is_alphanumeric())
            .next()
            .unwrap_or("?")
            .to_owned();
        match e {
            SessionError::Stale { .. }
            | SessionError::Claimed { .. }
            | SessionError::ClaimContended { .. }
            | SessionError::BadClaim(..)
            | SessionError::BadPosition(..) => self.refused(name),
            e => panic!("unexpected session error: {e:?}"),
        }
    }

    fn tool(
        &mut self,
        i: usize,
        conn: ConnId,
        argv: &[String],
        stdin: &str,
        policy: ClaimPolicy,
    ) -> bool {
        let opts = ToolOptions::new().policy(policy);
        match self
            .s
            .run_tool_with(conn, ROOT, argv, stdin.as_bytes(), &opts)
        {
            Ok(out) => {
                if out.changed.is_some() {
                    self.n.applied += 1;
                } else if argv[0] != "cat" && argv[0] != "head" && argv[0] != "grep" {
                    self.refused(format!("{} exit {}", argv[0], out.exit));
                }
                if out.fence.is_some() {
                    let r = self.s.read(conn).unwrap();
                    self.parts[i].last_read = Some((r.version, r.text_by_root[0].1.clone()));
                }
                true
            }
            Err(ToolRunError::Session(e)) => {
                self.session_refusal(&e);
                false
            }
            Err(e) => panic!("tool {argv:?}: {e:?}"),
        }
    }

    /// One call, chosen by the seed; returns its name, who made it (index) and what it may do.
    fn step(&mut self) -> (&'static str, usize, ClaimPolicy, Expect) {
        // the plain editor: SOAK_OUTSIDE=0 turns it off (the draw is still made)
        if self.rng.gen_ratio(1, 40) && self.outside {
            return self.outside_save();
        }
        let i = self.rng.gen_range(0..self.parts.len());
        if self.parts[i].conns.is_empty() {
            self.join(i);
            return ("rejoin", i, ClaimPolicy::Ignore, Expect::Nothing);
        }
        let conn = self.parts[i].conns[self.rng.gen_range(0..self.parts[i].conns.len())];
        let len = self.lines.len();
        let big = len > MAX_LINES;
        let small = len < MIN_LINES;
        let pick = self.rng.gen_range(0..100);
        let a = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<String>>();
        let policy = self.policy(i);
        match pick {
            0..=9 => {
                // a read: cat, head, grep, or an agent's `--for-edit`
                let n = self.rng.gen_range(1..=len.max(1)).to_string();
                let argv = match self.rng.gen_range(0..4) {
                    0 => a(&["cat"]),
                    1 => a(&["head", "-n", &n]),
                    2 => a(&["grep", "-c", "p0"]),
                    _ if self.parts[i].kind == ParticipantKind::Agent => {
                        a(&["head", "-n", &n, "--for-edit", "--ttl", "20000"])
                    }
                    _ => a(&["cat", "-n"]),
                };
                self.tool(i, conn, &argv, "", policy);
                ("read", i, policy, Expect::Nothing)
            }
            10..=24 if !big => {
                let t = self.token(i);
                let n = self.rng.gen_range(1..=len.max(1)).to_string();
                self.tool(
                    i,
                    conn,
                    &a(&["insert", "--line", &n]),
                    &format!("{t}\n"),
                    policy,
                );
                ("insert", i, policy, Expect::Insert(vec![t]))
            }
            25..=29 if !big => {
                let t = self.token(i);
                self.tool(i, conn, &a(&["append"]), &format!("{t}\n"), policy);
                ("append", i, policy, Expect::Insert(vec![t]))
            }
            30..=37 if len > 0 => {
                let t = self.token(i);
                let n = self.rng.gen_range(1..=len);
                let targets = HashSet::from([self.lines[n - 1].clone()]);
                let r = format!("{n},{n}");
                self.tool(
                    i,
                    conn,
                    &a(&["replace", "--lines", &r]),
                    &format!("{t}\n"),
                    policy,
                );
                (
                    "replace",
                    i,
                    policy,
                    Expect::Remove {
                        targets,
                        new: vec![t],
                    },
                )
            }
            38..=47 | 10..=29 if !small => {
                let first = self.rng.gen_range(1..=len);
                let last = (first + self.rng.gen_range(0..3)).min(len);
                let targets: HashSet<String> =
                    self.lines[first - 1..last].iter().cloned().collect();
                let r = format!("{first},{last}");
                self.tool(i, conn, &a(&["delete", "--lines", &r]), "", policy);
                (
                    "delete",
                    i,
                    policy,
                    Expect::Remove {
                        targets,
                        new: Vec::new(),
                    },
                )
            }
            48..=51 if len > 0 => {
                // sed -i with a version guard: current, or a stale one now and then
                let t = self.token(i);
                let n = self.rng.gen_range(1..=len);
                let targets = HashSet::from([self.lines[n - 1].clone()]);
                let v = if self.rng.gen_bool(0.7) {
                    self.version
                } else {
                    self.version.saturating_sub(1)
                };
                let script = format!("{n}s/.*/{t}/");
                self.tool(
                    i,
                    conn,
                    &a(&["sed", "-i", "--if-version", &v.to_string(), &script]),
                    "",
                    policy,
                );
                (
                    "sed -i",
                    i,
                    policy,
                    Expect::Remove {
                        targets,
                        new: vec![t],
                    },
                )
            }
            52..=58 => {
                // write --base V from this participant's last read (merges what came since),
                // or --base live
                let t = self.token(i);
                let live = self.rng.gen_bool(0.2);
                let (base, text) = match (&self.parts[i].last_read, live) {
                    (Some((v, text)), false) => (Some(*v), text.clone()),
                    _ => (None, self.lines.iter().map(|l| format!("{l}\n")).collect()),
                };
                let mut ls = lines_of(&text);
                let at = self.rng.gen_range(0..=ls.len());
                ls.insert(at, t.clone());
                let body: String = ls.iter().map(|l| format!("{l}\n")).collect();
                match base {
                    Some(v) => match self.s.write_base_with(conn, ROOT, v, &body, policy) {
                        Ok(r) => match r.outcome {
                            WriteOutcome::Applied(_) => self.n.applied += 1,
                            WriteOutcome::Conflicts(_) => self.refused("write conflict".into()),
                            WriteOutcome::BaseExpired => self.refused("base expired".into()),
                        },
                        Err(e) => self.session_refusal(&e),
                    },
                    None => match self.s.replace_text_with(conn, ROOT, &body, policy) {
                        Ok(_) => self.n.applied += 1,
                        Err(e) => self.session_refusal(&e),
                    },
                }
                ("write", i, policy, Expect::Insert(vec![t]))
            }
            59..=63 => {
                // the host's direct edit route: text from a read, guarded by its version
                let t = self.token(i);
                let v = match &self.parts[i].last_read {
                    Some((v, _)) => *v,
                    None => self.version,
                };
                let at = self.rng.gen_range(0..=len);
                let tok = t.clone();
                let opts = EditOptions::new().policy(policy).if_version(v);
                match self.s.edit_with_options(conn, ROOT, &opts, move |live| {
                    let mut ls = lines_of(live);
                    ls.insert(at.min(ls.len()), tok);
                    Ok::<_, Infallible>(ls.iter().map(|l| format!("{l}\n")).collect())
                }) {
                    Ok(Some(_)) => self.n.applied += 1,
                    Ok(None) => panic!("an insert changed nothing"),
                    Err(EditWithError::Session(e)) => self.session_refusal(&e),
                    Err(EditWithError::Compute(e)) => match e {},
                }
                ("edit_with_options", i, policy, Expect::Insert(vec![t]))
            }
            64..=73 => {
                let redo = pick >= 71;
                let mode = if self.rng.gen_ratio(1, 4) {
                    UndoMode::Partial
                } else {
                    UndoMode::All
                };
                let steps = self.rng.gen_range(1..=2);
                let req = UndoRequest::steps(steps).mode(mode);
                // the D1 signature: a region of the batches it undoes or redoes that isn't
                // whole lines
                let partial = |x: &str| !x.is_empty() && !x.ends_with('\n');
                self.d1 = self
                    .s
                    .undo_regions_for_tests(&self.parts[i].pid, redo, steps)
                    .unwrap()
                    .iter()
                    .any(|(b, a)| partial(b) || partial(a));
                let r = if redo {
                    self.s.redo_with(conn, &req)
                } else {
                    self.s.undo_with(conn, &req)
                };
                match r {
                    Ok(UndoOutcome::Applied { .. }) => self.n.applied += 1,
                    Ok(o) => self.refused(format!(
                        "{} {}",
                        if redo { "redo" } else { "undo" },
                        format!("{o:?}")
                            .split([' ', '{', '('])
                            .next()
                            .unwrap_or("?")
                    )),
                    Err(e) => self.session_refusal(&e),
                }
                (
                    if redo { "redo" } else { "undo" },
                    i,
                    ClaimPolicy::Ignore,
                    Expect::History(self.parts[i].pid.clone()),
                )
            }
            74..=80 if len > 0 => {
                let first = self.rng.gen_range(1..=len);
                let last = (first + self.rng.gen_range(0..4)).min(len);
                let ttl = self.rng.gen_range(1_000..60_000);
                let hold = self.rng.gen_ratio(1, 3);
                let range = LineRange {
                    root: ROOT.into(),
                    first,
                    last,
                };
                match self.s.claim(conn, range, Some(ttl), None, hold) {
                    // a new claim never overlaps another participant's
                    Ok(new) => {
                        for c in self.claims() {
                            if c.0 != new.pid {
                                assert!(
                                    new.end <= c.4 || c.5 <= new.start,
                                    "a new claim {new:?} overlaps {c:?}"
                                );
                            }
                        }
                    }
                    Err(e) => self.session_refusal(&e),
                }
                ("claim", i, policy, Expect::Nothing)
            }
            81..=83 => {
                self.s.release_claim(conn, None).unwrap();
                ("release", i, policy, Expect::Nothing)
            }
            84..=88 if len > 0 => {
                let n = self.rng.gen_range(0..len);
                let at: usize = self.lines[..n].iter().map(|l| l.len() + 1).sum();
                self.s
                    .set_cursor(conn, ROOT, At::Byte(at), At::Byte(at))
                    .unwrap();
                ("cursor", i, policy, Expect::Nothing)
            }
            89..=91 => {
                // leave one connection (never the observer's)
                if i == 0 && conn == self.obs() {
                    return ("noop", i, policy, Expect::Nothing);
                }
                assert!(self.s.leave(conn).unwrap(), "leave of a live connection");
                self.parts[i].conns.retain(|c| *c != conn);
                self.n.leaves += 1;
                ("leave", i, policy, Expect::Nothing)
            }
            92..=95 => {
                if self.parts[i].conns.len() < self.parts[i].max_conns {
                    self.join(i);
                    ("join", i, policy, Expect::Nothing)
                } else {
                    ("noop", i, policy, Expect::Nothing)
                }
            }
            _ => {
                self.mgr.tick().unwrap();
                ("tick", i, policy, Expect::Nothing)
            }
        }
    }

    /// The file changed since last seen, and not by the editor: a write of the kit's (the
    /// host's clock doesn't move within a loop turn, so `now` is when it was made).
    fn note_kit_write(&mut self) {
        let bytes = std::fs::read(&self.file).unwrap();
        if bytes != self.last_file {
            let now = self.host.now.load(Ordering::SeqCst);
            self.kit_writes.push((bytes.clone(), now));
            if self.kit_writes.len() > 8 {
                self.kit_writes.remove(0);
            }
            self.last_file = bytes;
        }
    }

    /// The plain editor saves `text` (in place, or by rename as vim does) and the host's
    /// watcher reports it. K-AUD-1 after every save.
    fn editor_save(&mut self, text: &str) -> DiskCheck {
        if self.rng.gen_bool(0.5) {
            std::fs::write(&self.file, text).unwrap();
        } else {
            let tmp = self.file.with_extension("swp-new");
            std::fs::write(&tmp, text).unwrap();
            std::fs::rename(&tmp, &self.file).unwrap();
        }
        let check = self.s.file_changed().unwrap();
        match check {
            DiskCheck::Ingested { .. } | DiskCheck::Unchanged => {
                self.n.outside_saves += 1;
                self.kit_writes.clear();
            }
            DiskCheck::OwnWrite => {}
            ref other => panic!("an outside save: {other:?}"),
        }
        self.last_file = text.as_bytes().to_vec();
        assert_eq!(
            self.s.audit_disk_sync().unwrap(),
            vec![],
            "K-AUD-1 after an outside save"
        );
        check
    }

    /// One outside save (or two), as a plain editor makes it on the file as it is now.
    fn outside_save(&mut self) -> (&'static str, usize, ClaimPolicy, Expect) {
        self.note_kit_write();
        let file = String::from_utf8(std::fs::read(&self.file).unwrap()).unwrap();
        let mut ls = lines_of(&file);
        let join = |ls: &[String]| -> String { ls.iter().map(|l| format!("{l}\n")).collect() };
        self.actor = Some(EXTERNAL_PID.to_owned());
        let ign = ClaimPolicy::Ignore;
        // a line the file and the doc both hold
        let shared: Vec<usize> = (0..ls.len())
            .filter(|&k| self.lines.contains(&ls[k]))
            .collect();
        let now = self.host.now.load(Ordering::SeqCst);
        let ttl = self.config.own_write_ttl_ms;
        let stale: Vec<Vec<u8>> = self
            .kit_writes
            .iter()
            .rev()
            .take(4)
            .filter(|(b, at)| now.saturating_sub(*at) + 5_000 <= ttl && *b != file.as_bytes())
            .map(|(b, _)| b.clone())
            .collect();
        let echo = |w: &mut Self, check: &DiskCheck| -> bool {
            if *check == DiskCheck::OwnWrite {
                // a write of the kit's since the last outside save (an undo of a line that
                // came and went, say): an echo by design; the write-back puts the doc back
                w.n.echoes += 1;
                w.s.flush(w.obs()).unwrap();
                true
            } else {
                false
            }
        };
        match self.rng.gen_range(0..5) {
            4 if !stale.is_empty() => {
                // a stale buffer: an older write of the kit's, made since the last outside save
                let w = stale[self.rng.gen_range(0..stale.len())].clone();
                let check = self.editor_save(std::str::from_utf8(&w).unwrap());
                assert_eq!(check, DiskCheck::OwnWrite, "a stale buffer of a kit write");
                echo(self, &check);
                ("outside stale", 0, ign, Expect::Nothing)
            }
            1..=3 if !shared.is_empty() => {
                let kind = self.rng.gen_range(1..=3);
                let k = shared[self.rng.gen_range(0..shared.len())];
                if kind == 2 {
                    // a line added, saved, then taken out again and saved
                    let t = self.editor_token();
                    ls.insert(k, t);
                    let check = self.editor_save(&join(&ls));
                    if echo(self, &check) {
                        return ("outside add-undo", 0, ign, Expect::Nothing);
                    }
                    ls.remove(k);
                    let check = self.editor_save(&join(&ls));
                    assert_ne!(
                        check,
                        DiskCheck::OwnWrite,
                        "no kit write since the last save"
                    );
                    return (
                        "outside add-undo",
                        0,
                        ign,
                        Expect::Saved {
                            targets: HashSet::new(),
                            new: Vec::new(),
                        },
                    );
                }
                let gone = ls.remove(k);
                let check = self.editor_save(&join(&ls));
                if echo(self, &check) {
                    return ("outside delete", 0, ign, Expect::Nothing);
                }
                if kind == 1 {
                    return (
                        "outside delete",
                        0,
                        ign,
                        Expect::Saved {
                            targets: HashSet::from([gone]),
                            new: Vec::new(),
                        },
                    );
                }
                // typed back where it was, and saved: the bytes as before
                ls.insert(k, gone);
                let check = self.editor_save(&join(&ls));
                assert_ne!(
                    check,
                    DiskCheck::OwnWrite,
                    "no kit write since the last save"
                );
                (
                    "outside delete-retype",
                    0,
                    ign,
                    Expect::Saved {
                        targets: HashSet::new(),
                        new: Vec::new(),
                    },
                )
            }
            _ => {
                let t = self.editor_token();
                let at = self.rng.gen_range(0..=ls.len());
                ls.insert(at, t.clone());
                let check = self.editor_save(&join(&ls));
                if echo(self, &check) {
                    return ("outside insert", 0, ign, Expect::Nothing);
                }
                (
                    "outside insert",
                    0,
                    ign,
                    Expect::Saved {
                        targets: HashSet::new(),
                        new: vec![t],
                    },
                )
            }
        }
    }

    /// The claims as they stand: (pid, conn, first, last, start, end).
    fn claims(&self) -> Vec<(String, ConnId, usize, usize, usize, usize)> {
        self.s
            .claims(self.obs())
            .unwrap()
            .into_iter()
            .map(|c| (c.pid, c.conn, c.lines.first, c.lines.last, c.start, c.end))
            .collect()
    }

    /// Check the call against the lines before and after it.
    fn audit(
        &mut self,
        op: &str,
        i: usize,
        policy: ClaimPolicy,
        expect: Expect,
        claims_before: &[(String, ConnId, usize, usize, usize, usize)],
    ) {
        let (after, version) = self.live();
        let before_set: HashSet<&String> = self.lines.iter().collect();
        let after_set: HashSet<&String> = after.iter().collect();
        {
            let who = self
                .actor
                .clone()
                .unwrap_or_else(|| self.parts[i].pid.clone());
            let mut ad: Vec<&String> = after_set.difference(&before_set).copied().collect();
            let mut rm: Vec<&String> = before_set.difference(&after_set).copied().collect();
            ad.sort();
            rm.sort();
            self.last_ops.push_back((
                self.n.ops,
                version,
                format!("{op} by {who}: +{ad:?} -{rm:?}"),
            ));
            if self.last_ops.len() > env_u64("SOAK_TRACE_KEEP", 64) as usize {
                self.last_ops.pop_front();
            }
        }
        let d1 =
            std::mem::take(&mut self.d1) && matches!(expect, Expect::History(_)) && self.known_d1;
        let mut d1_hit = false;
        let twice = after.len() - after_set.len();
        if twice > self.n.dups {
            assert!(d1, "{op}: a line twice: {after:?}");
            d1_hit = true;
        }
        self.n.dups = twice;
        let added: HashSet<String> = after_set
            .difference(&before_set)
            .map(|s| (*s).clone())
            .collect();
        let removed: HashSet<String> = before_set
            .difference(&after_set)
            .map(|s| (*s).clone())
            .collect();
        for l in &added {
            if !self.author.contains_key(l) {
                assert!(d1, "{op}: a line nobody wrote: {l:?}");
                d1_hit = true;
            }
        }
        let pid = self
            .actor
            .take()
            .unwrap_or_else(|| self.parts[i].pid.clone());
        let changed = !added.is_empty() || !removed.is_empty();
        if changed {
            assert!(
                version > self.version,
                "{op}: the text changed but the version didn't"
            );
        }
        match &expect {
            Expect::Nothing => {
                assert!(
                    !changed,
                    "{op} by {pid} changed the text: +{added:?} -{removed:?}"
                );
                assert_eq!(version, self.version, "{op}: the version moved");
            }
            Expect::Insert(new) => {
                assert!(removed.is_empty(), "{op} by {pid} removed {removed:?}");
                for l in &added {
                    assert!(new.contains(l), "{op} by {pid}: {l} appeared");
                }
                // acknowledged: there, or it applied nothing at all
                if version > self.version {
                    for t in new {
                        assert!(added.contains(t), "{op} by {pid} acked but {t} is missing");
                    }
                }
            }
            Expect::Remove { targets, new } => {
                for l in &removed {
                    assert!(
                        targets.contains(l),
                        "{op} by {pid} removed {l}, not its target"
                    );
                }
                for l in &added {
                    assert!(new.contains(l), "{op} by {pid}: {l} appeared");
                }
            }
            Expect::Saved { targets, new } => {
                for l in &removed {
                    assert!(
                        targets.contains(l),
                        "{op}: the save removed {l}, not its target"
                    );
                }
                for l in &added {
                    assert!(new.contains(l), "{op}: {l} appeared");
                }
                // taken in: every line it took out is gone, every line it added is there
                for l in targets {
                    assert!(!after_set.contains(l), "{op}: {l} survived the save");
                }
                for l in new {
                    assert!(after_set.contains(l), "{op}: the save's {l} is missing");
                }
            }
            Expect::History(p) => {
                let mine = self.appeared_by.entry(p.clone()).or_default();
                for l in &removed {
                    if !mine.contains(l) {
                        assert!(
                            d1,
                            "{op} by {p} removed {l} (by {}), which no change of {p}'s put there",
                            self.author[l]
                        );
                        d1_hit = true;
                    }
                }
                let gone = self.removed_by.entry(p.clone()).or_default();
                for l in &added {
                    if !gone.contains(l) {
                        assert!(d1, "{op} by {p} put back {l}, which {p} never removed");
                        d1_hit = true;
                    }
                }
            }
        }
        // an agent respecting claims never removes a line someone else claims
        if policy == ClaimPolicy::Respect && self.parts[i].kind == ParticipantKind::Agent {
            // a line is claimed when a claim's span holds all of its text. Claims hold at
            // the character: a claim that took in only part of a line (an expanding anchor
            // takes in text typed at its edges) protects those characters, and a delete's
            // minimal diff may remove the same text elsewhere (`p19:35\np` as `19:35\np`,
            // keeping the claimed `p`). Line-level, that reads as the line gone; it is not
            // a claimed character gone.
            let mut at = 0;
            for l in &self.lines {
                let (ls, le) = (at, at + l.len());
                at = le + 1;
                for c in claims_before {
                    let (cpid, start, end) = (&c.0, c.4, c.5);
                    if *cpid != pid && start <= ls && le <= end && ls < le {
                        assert!(
                            !removed.contains(l),
                            "{op} by agent {pid} under Respect removed {l}, claimed by {cpid}: \
                             claim {c:?}, line bytes {ls}..{le}, v{} -> v{version}",
                            self.version
                        );
                    }
                }
            }
        }
        if d1_hit {
            // the known D1 gap: counted; the lines it left are the doc's from now on
            self.n.d1_tolerated += 1;
            self.n.d1_calls.push(self.n.ops);
            let mut ad: Vec<&String> = added.iter().collect();
            let mut rm: Vec<&String> = removed.iter().collect();
            ad.sort();
            rm.sort();
            eprintln!(
                "[soak] known D1 violation at call {}: {op} by {pid}: +{ad:?} -{rm:?} (v{} -> v{version})",
                self.n.ops, self.version
            );
            for l in &added {
                self.author.entry(l.clone()).or_insert_with(|| pid.clone());
            }
        }
        self.appeared_by
            .entry(pid.clone())
            .or_default()
            .extend(added.iter().cloned());
        self.removed_by
            .entry(pid)
            .or_default()
            .extend(removed.iter().cloned());
        self.lines = after;
        self.version = version;
        self.n.max_lines = self.n.max_lines.max(self.lines.len());
    }

    /// Claims and presence: every claim's connection is live; two participants' claims never
    /// overlap (shared edges counted); the roster is exactly who is connected.
    fn presence(&mut self) {
        let claims = self.claims();
        let mut shared = 0u64;
        let conns: HashMap<ConnId, &str> = self
            .parts
            .iter()
            .flat_map(|p| p.conns.iter().map(move |c| (*c, p.pid.as_str())))
            .collect();
        for (k, a) in claims.iter().enumerate() {
            assert_eq!(
                conns.get(&a.1),
                Some(&a.0.as_str()),
                "a claim on a closed connection: {a:?}"
            );
            assert!(a.2 >= 1 && a.2 <= a.3, "claim lines {a:?}");
            assert!(a.4 <= a.5, "claim span {a:?}");
            // two participants' claims never overlap, carried ones included: text typed
            // where they share an edge joins one of them (v0.9.2; tests/presence.rs)
            for b in &claims[k + 1..] {
                assert!(
                    a.0 == b.0 || a.5 <= b.4 || b.5 <= a.4,
                    "two participants' claims overlap: {a:?} and {b:?}"
                );
                if a.0 != b.0 && (a.5 == b.4 || b.5 == a.4) {
                    shared += 1;
                }
            }
        }
        if shared > 0 {
            self.n.shared_edges += 1;
        }
        let roster = self.s.roster(self.obs()).unwrap();
        let mut want: BTreeMap<&str, usize> = BTreeMap::new();
        for p in &self.parts {
            if !p.conns.is_empty() {
                want.insert(&p.pid, p.conns.len());
            }
        }
        let got: BTreeMap<&str, usize> = roster
            .iter()
            .map(|r| (r.participant.pid.as_str(), r.connections))
            .collect();
        assert_eq!(got, want, "the roster is not who is connected");
        let st = self.s.status().unwrap();
        assert_eq!(st.participants, want.len());
        assert_eq!(st.connections, want.values().sum::<usize>());
    }

    /// The file holds the doc after a flush, and blame names each line's writer.
    fn file_and_blame(&mut self) {
        let f = self.s.flush(self.obs()).unwrap();
        assert!(
            matches!(f.disk, Some(k2_mpc_base::session::DiskState::Clean)),
            "{f:?}"
        );
        let on_disk = std::fs::read(&self.file).unwrap();
        assert_eq!(
            on_disk,
            self.s.read(self.obs()).unwrap().bytes,
            "the file isn't the doc"
        );
        // K-AUD-1's sync check (v0.9.4): the replica the next outside edit is made on is the
        // doc, on its elements (the file and the doc agreeing can't show a stale one)
        assert_eq!(
            self.s.audit_disk_sync().unwrap(),
            vec![],
            "the disk replica isn't the doc"
        );
        self.n.flushes += 1;
        let b = self.s.blame(self.obs(), ROOT, None).unwrap();
        assert_eq!(b.lines.len(), self.lines.len());
        let external = EXTERNAL_PID.to_owned();
        let mut writers: HashSet<&String> = self.author.values().collect();
        // a line the plain editor typed back is its, whoever wrote it first
        writers.insert(&external);
        for (bl, l) in b.lines.iter().zip(&self.lines) {
            assert_eq!(&bl.text, l);
            assert!(
                writers.contains(&bl.author),
                "blame names {} for {l}, who never wrote",
                bl.author
            );
            // a line goes to its newest character, and its `\n` can be a neighbour's (a
            // delete or a merge keeps one line's break for another): counted, not failed
            if bl.author != self.author[l] {
                self.n.blame_newline += 1;
            }
            self.n.blame_lines += 1;
        }
        self.n.blames += 1;
    }

    /// Release everything, reopen the store: the same text and version; everyone rejoins.
    fn restart(&mut self) {
        self.n.index += self.s.position_index_stats_for_tests().unwrap();
        let (text, version) = (self.s.read(self.obs()).unwrap().bytes, self.version);
        self.mgr.release().unwrap();
        let (mgr, s) = Self::open(&self.dir, &self.file, &self.key, &self.host, &self.config);
        self.mgr = mgr;
        self.s = s;
        for p in &mut self.parts {
            p.conns.clear();
            p.last_read = None;
        }
        // everyone rejoins on one connection (the observer first)
        for i in 0..self.parts.len() {
            self.join(i);
        }
        let r = self.s.read(self.obs()).unwrap();
        assert_eq!(r.bytes, text, "store reload: the text differs from live");
        assert_eq!(r.version, version, "store reload: the version differs");
        assert_eq!(lines_of(&r.text_by_root[0].1), self.lines);
        // the kit's own-write hashes are in memory: a new session has none
        self.last_file = std::fs::read(&self.file).unwrap();
        self.kit_writes.clear();
        self.n.restarts += 1;
    }

    fn sample_memory(&mut self, elapsed: u64) {
        let est = self.s.status().unwrap().estimated_bytes;
        self.n.peak_estimate = self.n.peak_estimate.max(est);
        assert!(
            est <= self.config.memory_budget_bytes,
            "the kit's estimate {est} is over the budget {}",
            self.config.memory_budget_bytes
        );
        let rss = rss_kb();
        self.n.peak_rss_kb = self.n.peak_rss_kb.max(rss);
        self.n.rss_samples.push((elapsed, rss));
    }

    /// What a failure needs for its analysis (v0.9.7): the last calls, the engine's text
    /// against the session's cached one (where they first differ, with the lines around),
    /// and participant `i`'s (and every participant's, when `i` is `None`) undo and redo
    /// stacks' top batches. The soak's own synthetic text only.
    fn dump(&self, label: &str, why: &str, i: Option<usize>) {
        eprintln!(
            "[soak {label}] DUMP ({why}) at op {}, v{}; the last calls (op, version after, call):",
            self.n.ops, self.version
        );
        for (n, v, w) in &self.n.last_ops {
            eprintln!("[soak {label}]   {n} v{v}: {w}");
        }
        // the audit's view of the same calls: the lines each added and removed (v0.9.5)
        for (n, v, what) in &self.last_ops {
            eprintln!("[soak {label}]   lines {n} v{v}: {what}");
        }
        match self.s.engine_and_cached_texts_for_tests() {
            Ok(texts) => {
                for (root, engine, cached) in texts {
                    if engine == cached {
                        eprintln!(
                            "[soak {label}] {root}: the engine's text equals the cached text ({} lines)",
                            engine.lines().count()
                        );
                        continue;
                    }
                    let (e, c): (Vec<&str>, Vec<&str>) =
                        (engine.lines().collect(), cached.lines().collect());
                    let k = e.iter().zip(&c).take_while(|(a, b)| a == b).count();
                    eprintln!(
                        "[soak {label}] {root}: the ENGINE'S TEXT DIFFERS from the cached text from line {k} ({} vs {} lines)",
                        e.len(),
                        c.len()
                    );
                    let lo = k.saturating_sub(3);
                    eprintln!(
                        "[soak {label}]   engine {lo}..: {:?}",
                        &e[lo..e.len().min(k + 6)]
                    );
                    eprintln!(
                        "[soak {label}]   cached {lo}..: {:?}",
                        &c[lo..c.len().min(k + 6)]
                    );
                }
            }
            Err(e) => eprintln!("[soak {label}] texts: {e}"),
        }
        let who: Vec<usize> = match i {
            Some(i) => vec![i],
            None => (0..self.parts.len()).collect(),
        };
        for i in who {
            let pid = &self.parts[i].pid;
            if let Ok((undo, redo)) = self.s.undo_stacks_for_tests(pid, 3) {
                eprintln!("[soak {label}] {pid}'s undo stack, top 3:\n{undo}");
                eprintln!("[soak {label}] {pid}'s redo stack, top 3:\n{redo}");
            }
        }
        // the lines written twice, with their blame (v0.9.5)
        if let Ok(r) = self.s.read(self.obs()) {
            let mut seen = HashSet::new();
            for l in lines_of(&r.text_by_root[0].1) {
                if !seen.insert(l.clone()) {
                    if let Ok(b) = self.s.blame(self.obs(), ROOT, None) {
                        for bl in b.lines.iter().filter(|bl| bl.text == l) {
                            eprintln!("[soak {label}] twice: {bl:?}");
                        }
                    }
                }
            }
        }
        if let Ok(b) = self.s.blame(self.obs(), ROOT, None) {
            eprintln!(
                "[soak {label}] blame now: {:?}",
                format!("{b:?}").chars().take(4000).collect::<String>()
            );
        }
    }

    fn run(&mut self, secs: u64, label: &str) {
        let progress = std::env::var("SOAK_PROGRESS_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(30u64);
        let start = Instant::now();
        let end = start + Duration::from_secs(secs);
        let mut next_progress = start + Duration::from_secs(progress);
        let restart_every = if secs <= 30 { 300 } else { 3_000 };
        let mut win = Window::default();
        let milestone_every = env_u64("SOAK_MILESTONE", 10_000).max(1);
        let milestone_calls = env_u64("SOAK_MILESTONE_CALLS", 1_000).max(1) as usize;
        // replay and diagnosis: stop after this many calls (`SOAK_MAX_OPS`, or the replay
        // regression's own; 0: no limit); dump the state before this call (1-based)
        let max_ops = self.max_ops;
        let dump_at = env_u64("SOAK_DUMP_AT_OP", 0);
        // how many of the last calls a dump lists (default 64)
        let dump_calls = env_u64("SOAK_DUMP_CALLS", 64).max(1) as usize;
        while Instant::now() < end && (max_ops == 0 || self.n.ops < max_ops) {
            let t0 = Instant::now();
            self.advance();
            let claims_before = self.claims();
            let t1 = Instant::now();
            if max_ops != 0 && self.n.ops >= max_ops {
                eprintln!("[soak {label}] stopped at SOAK_MAX_OPS={max_ops}");
                break;
            }
            if dump_at != 0 && self.n.ops + 1 == dump_at {
                self.dump(
                    label,
                    &format!("before op {dump_at} (SOAK_DUMP_AT_OP)"),
                    None,
                );
            }
            let stepped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.step()));
            let (op, i, policy, expect) = match stepped {
                Ok(s) => s,
                Err(e) => {
                    self.dump(label, "a panic in the call", None);
                    std::panic::resume_unwind(e);
                }
            };
            let t2 = Instant::now();
            *self.n.by_op.entry(op).or_default() += 1;
            self.n.ops += 1;
            // the outside editor's calls are made by no participant (v0.9.5)
            let who = self
                .actor
                .clone()
                .unwrap_or_else(|| self.parts[i].pid.clone());
            let what = format!("{op} by {who} {:?} {policy:?}", expect);
            let what: String = what.chars().take(240).collect();
            let audited = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.audit(op, i, policy, expect, &claims_before)
            }));
            self.n.last_ops.push_back((self.n.ops, self.version, what));
            if self.n.last_ops.len() > dump_calls {
                self.n.last_ops.pop_front();
            }
            if let Err(e) = audited {
                self.dump(label, "an invariant failed", Some(i));
                std::panic::resume_unwind(e);
            }
            let found = std::mem::take(&mut *self.host.mismatches.lock().unwrap());
            if !found.is_empty() {
                self.dump(label, "a runtime check reported a mismatch", Some(i));
                panic!(
                    "op {} (v{}): the kit reported {} mismatch(es): {:?}",
                    self.n.ops,
                    self.version,
                    found.len(),
                    found
                );
            }
            let t3 = Instant::now();
            self.presence();
            let t4 = Instant::now();
            if self.n.ops % 250 == 0 {
                self.file_and_blame();
                self.sample_memory(start.elapsed().as_secs());
            }
            let t5 = Instant::now();
            if self.n.ops % restart_every == 0 {
                self.restart();
            }
            self.note_kit_write();
            let t6 = Instant::now();
            win.add(op, [t0, t1, t2, t3, t4, t5, t6]);
            let tick = (op == "tick").then(|| (t2 - t1).as_secs_f64());
            let version = self.version;
            let s = &self.s;
            self.n.track(
                (t6 - t0).as_secs_f64(),
                tick,
                version,
                milestone_every,
                milestone_calls,
                || s.history_units_for_tests().unwrap(),
            );
            if Instant::now() >= next_progress {
                next_progress += Duration::from_secs(progress);
                let at = start.elapsed().as_secs_f64();
                eprintln!(
                    "[soak {label}] {}s: {} ops ({} applied), v{}, {} lines, {} restarts, rss {} kB, est {} B",
                    at as u64,
                    self.n.ops,
                    self.n.applied,
                    self.version,
                    self.lines.len(),
                    self.n.restarts,
                    rss_kb(),
                    self.s.status().unwrap().estimated_bytes
                );
                eprintln!("[soak {label}] {}s: {}", at as u64, win.report());
                self.n.windows.push((at, win.ops, win.wall()));
                win = Window::default();
            }
        }
        // the final state: the file, blame, and one more restart
        self.file_and_blame();
        self.restart();
        self.sample_memory(start.elapsed().as_secs());
        let wall = start.elapsed().as_secs_f64();
        let n = &self.n;
        eprintln!(
            "[soak {label}] done: {:.0}s, {} ops ({:.0}/s), {} applied, v{}, {} lines now (max {}), \
             {} restarts, {} blame+file checks ({} of {} lines blamed on the writer of their line break), {} joins, {} leaves, {} calls after which two participants' claims shared an edge (never overlapping); peak rss {} kB, peak kit estimate {} B \
             (budget {}); rss samples (s, kB): {:?}",
            wall,
            n.ops,
            n.ops as f64 / wall,
            n.applied,
            self.version,
            self.lines.len(),
            n.max_lines,
            n.restarts,
            n.blames,
            n.blame_newline,
            n.blame_lines,
            n.joins,
            n.leaves,
            n.shared_edges,
            n.peak_rss_kb,
            n.peak_estimate,
            self.config.memory_budget_bytes,
            sample(&n.rss_samples)
        );
        eprintln!("[soak {label}] by call: {:?}", n.by_op);
        eprintln!(
            "[soak {label}] outside saves: {} taken in, {} older kit writes saved back and suppressed as echoes",
            n.outside_saves, n.echoes
        );
        eprintln!(
            "[soak {label}] undo/redo mismatches with the line oracle tolerated as the known D1 gap (a region that isn't whole lines; amendment pending): {}; lines written twice now: {}",
            n.d1_tolerated, n.dups
        );
        let rates: Vec<(u64, u64)> = n
            .windows
            .iter()
            .map(|(at, c, w)| (*at as u64, (*c as f64 / w.max(1e-9)) as u64))
            .collect();
        eprintln!("[soak {label}] calls/s by window (s, calls/s): {rates:?}");
        // the cost at equal history (v0.9.7): one record per milestone, and the slope
        let job = k2_mpc_base::differential::runrec_job();
        for m in &n.milestones {
            eprintln!(
                "[soak {label}] at v{}: {:.0} µs per call, {:.0} µs per tick ({} ticks) over the {milestone_calls} calls before it; history {} units",
                m.at, m.us_per_call, m.us_per_tick, m.ticks, m.history_units
            );
            eprintln!(
                "RUNREC {{\"job\":{job},\"run\":\"soak_{label}_v{}\",\"tool\":\"soak\",\"status\":\"COMPLETE\",\"milestone\":{},\"version\":{},\"calls\":{milestone_calls},\"us_per_call\":{},\"us_per_tick\":{},\"ticks\":{},\"history_units\":{},\"full_writeback\":{}}}",
                m.at,
                m.at,
                m.version,
                m.us_per_call.round() as u64,
                m.us_per_tick.round() as u64,
                m.ticks,
                m.history_units,
                full_writeback_on()
            );
        }
        // the profile between milestones (v0.9.7): each scope's mean time per entry, the
        // largest first, with its entries per call (how often it runs)
        for (m, prof) in n.milestones.iter().zip(&n.profiles) {
            let mut v: Vec<&(&str, u64, f64)> = prof.iter().collect();
            v.sort_by(|a, b| b.2.total_cmp(&a.2));
            let parts: Vec<String> = v
                .iter()
                .map(|(k, c, secs)| format!("{k} {:.0}µs x{c}", secs / (*c).max(1) as f64 * 1e6))
                .collect();
            eprintln!(
                "[soak {label}] profile up to v{} (scope, µs per entry, entries): {}",
                m.at,
                parts.join(", ")
            );
        }
        let ix = n.index;
        eprintln!(
            "[soak {label}] position index: {} lookups, {} from a marker ({:.1}%), {} walks, {} shadow checks, {} mismatches, {} stale markers (summed over every load of the doc in the run)",
            ix.lookups,
            ix.marker_hits,
            ix.marker_hits as f64 * 100.0 / ix.lookups.max(1) as f64,
            ix.walks,
            ix.shadow_checks,
            ix.mismatches,
            ix.stale_markers
        );
        let slope = milestone_slope(&n.milestones);
        let growth = match (n.milestones.first(), n.milestones.last()) {
            (Some(a), Some(b)) if n.milestones.len() >= 2 && a.us_per_call > 0.0 => {
                Some(b.us_per_call / a.us_per_call)
            }
            _ => None,
        };
        if let (Some(sl), Some(g)) = (slope, growth) {
            eprintln!(
                "[soak {label}] cost per call grows {sl:.1} µs per 1,000 versions (least squares over {} milestones); last milestone / first: {g:.2}x",
                n.milestones.len()
            );
        }
        // throughput over time: the first window against the last 150 s (G-gate evidence:
        // a call must not cost more as history grows; SOAK_MAX_SLOWDOWN, default 2)
        // the run's record (tools/evidence/), before the checks below can fail it
        let (first, tail) = slowdown(&n.windows, secs as f64, 150.0).unwrap_or((0.0, 0.0));
        let max_x = std::env::var("SOAK_MAX_SLOWDOWN")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(2.0);
        let ratio = first / tail.max(1e-9);
        let checked = !shadow_on() && tail > 0.0;
        let opt = |v: Option<f64>, k: f64| {
            v.map_or("null".to_owned(), |x| format!("{}", (x * k).round() as i64))
        };
        eprintln!(
            "RUNREC {{\"job\":{job},\"run\":\"soak_{label}\",\"tool\":\"soak\",\"status\":\"{}\",\"secs\":{secs},\"calls\":{},\"applied\":{},\"version\":{},\"restarts\":{},\"first_calls_per_s\":{},\"tail_calls_per_s\":{},\"slowdown_percent\":{},\"max_slowdown_percent\":{},\"peak_rss_kb\":{},\"peak_estimate_bytes\":{},\"shadow\":{},\"full_writeback\":{},\"milestones\":{},\"slope_ns_per_call_per_kversion\":{},\"milestone_growth_percent\":{},\"index_lookups\":{},\"index_marker_hits\":{},\"index_walks\":{},\"index_mismatches\":{}}}",
            if !checked || ratio <= max_x { "COMPLETE" } else { "VIOLATION" },
            n.ops,
            n.applied,
            self.version,
            n.restarts,
            first.round() as u64,
            tail.round() as u64,
            if checked { (ratio * 100.0).round() as u64 } else { 0 },
            (max_x * 100.0).round() as u64,
            n.peak_rss_kb,
            n.peak_estimate,
            shadow_on(),
            full_writeback_on(),
            n.milestones.len(),
            opt(slope, 1000.0),
            opt(growth, 100.0),
            ix.lookups,
            ix.marker_hits,
            ix.walks,
            ix.mismatches
        );
        if shadow_on() {
            eprintln!("[soak {label}] throughput check skipped: shadow mode");
        } else if self.max_ops != 0 {
            eprintln!("[soak {label}] throughput check skipped: a call-limited replay");
        } else if let Some((first, tail)) = slowdown(&n.windows, secs as f64, 150.0) {
            let max = std::env::var("SOAK_MAX_SLOWDOWN")
                .ok()
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(2.0);
            let ratio = first / tail.max(1e-9);
            eprintln!(
                "[soak {label}] throughput: {first:.0} calls/s in the first window, {tail:.0} in the last 150 s: {ratio:.2}x slower (most {max})"
            );
            assert!(
                ratio <= max,
                "throughput fell {ratio:.2}x over the run ({first:.0} -> {tail:.0} calls/s; most {max}x)"
            );
        }
        eprintln!("[soak {label}] refusals and no-ops: {:?}", n.refused);
        // it did real work: every kind of call ran, most edits applied
        for op in [
            "read",
            "insert",
            "append",
            "replace",
            "delete",
            "sed -i",
            "write",
            "edit_with_options",
            "undo",
            "redo",
            "claim",
            "release",
            "cursor",
            "leave",
            "join",
            "tick",
            "outside insert",
            "outside delete",
        ] {
            if op.starts_with("outside") && !self.outside {
                continue;
            }
            assert!(n.by_op.get(op).copied().unwrap_or(0) > 0, "{op} never ran");
        }
        assert!(
            n.applied * 3 > n.ops,
            "too few changes applied: {} of {}",
            n.applied,
            n.ops
        );
        assert!(n.restarts >= 1);
        // the report's last line (v0.9.5's soak gate)
        eprintln!("[soak {label}] known D1 violations: {}", n.d1_tolerated);
    }
}

/// The first progress window's call rate and the rate over the last `tail` seconds of a
/// `secs` run; `None` when the run is too short to have both apart (under four windows).
fn slowdown(windows: &[(f64, u64, f64)], secs: f64, tail: f64) -> Option<(f64, f64)> {
    if windows.len() < 4 {
        return None;
    }
    let first = windows[0].1 as f64 / windows[0].2.max(1e-9);
    let cut = (secs - tail).max(windows[0].0);
    let (calls, wall) = windows
        .iter()
        .filter(|w| w.0 > cut)
        .fold((0u64, 0f64), |(c, s), w| (c + w.1, s + w.2));
    (wall > 0.0).then(|| (first, calls as f64 / wall))
}

/// At most ~12 evenly spaced samples, for the summary line.
fn sample(v: &[(u64, u64)]) -> Vec<(u64, u64)> {
    let step = v.len().div_ceil(12).max(1);
    let mut out: Vec<(u64, u64)> = v.iter().step_by(step).copied().collect();
    if let Some(l) = v.last() {
        if out.last() != Some(l) {
            out.push(*l);
        }
    }
    out
}

impl Drop for Soak {
    fn drop(&mut self) {
        let _ = self.mgr.release();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// The engine's edit path the soak runs: production's (v0.9.3: on the doc itself).
const SOAK_PATH: Path = Path::MainDoc;

fn shadow_on() -> bool {
    env_u64("SOAK_SHADOW", 0) != 0
}

fn full_writeback_on() -> bool {
    env_u64("SOAK_FULL_WRITEBACK", 0) != 0
}

/// Run `f` (the whole soak) with its edits through [`SOAK_PATH`]; in shadow mode, each also
/// checked against the throwaway replica (any difference panics).
fn soak_side<R>(label: &str, f: impl FnOnce() -> R) -> R {
    // replay: the kit's client IDs from a seed (with the seeded calls and clock, a run
    // replays exactly)
    let client_seed = std::env::var("SOAK_CLIENT_SEED")
        .ok()
        .map(|v| v.parse().expect("SOAK_CLIENT_SEED: a number"));
    soak_side_with(label, client_seed, shadow_on(), f)
}

/// [`soak_side`] with the client seed and shadow mode given (the replay regression's).
fn soak_side_with<R>(
    label: &str,
    client_seed: Option<u64>,
    shadow: bool,
    f: impl FnOnce() -> R,
) -> R {
    let mut side = Side::new(SOAK_PATH);
    if let Some(seed) = client_seed {
        side = side.with_client_seed(seed);
        eprintln!("[soak {label}] client IDs seeded from {seed}: the run replays exactly");
    }
    if env_u64("SOAK_NO_POSINDEX", 0) != 0 {
        side = side.no_position_index();
        eprintln!("[soak {label}] the engine's position index is off: every lookup walks");
    }
    if full_writeback_on() {
        side = side.full_writeback();
        eprintln!("[soak {label}] the old write-back: re-encode and reload the disk replica");
    }
    if shadow {
        side = side.shadowed_by(Path::Throwaway);
    }
    let r = with_side(&mut side, f);
    if shadow {
        eprintln!(
            "[soak {label}] shadow: {} edits made through {:?} and checked against the throwaway replica: byte-identical",
            side.shadow_checks(),
            side.path()
        );
        assert!(side.shadow_checks() > 0, "shadow mode checked nothing");
    }
    r
}

/// CI: a few seconds of the same soak.
#[test]
fn soak_short() {
    let seed = env_u64("SOAK_SEED", 1);
    soak_side("short", || {
        let mut w = Soak::new("short", seed);
        w.run(env_u64("SOAK_SHORT_SECS", 5), "short");
    });
}

/// The full soak (G-gate evidence): `SOAK_SECS` (default 600), release build, on a node.
#[test]
#[ignore = "10 minutes: run on a compute node (tools/tests/soak.sh)"]
fn soak_32() {
    let seed = env_u64("SOAK_SEED", 1);
    let secs = env_u64("SOAK_SECS", 600);
    eprintln!(
        "[soak 32] seed {seed}, {secs}s, {} people + {} agents",
        PEOPLE, AGENTS
    );
    soak_side("32", || {
        let mut w = Soak::new("full", seed);
        w.run(secs, "32");
    });
}

/// The D1 replay regression (v0.9.5): `SOAK_SEED=3`, client IDs from seed 1, the plain editor
/// on, the known-D1 mode on, 27,000 calls, whatever the job's environment. Call 25,877 is a
/// known D1 violation: p30's undo rebuilds characters into `p16:347` before p16 issues that
/// token (p16 does at call 26,450, which once read as a line written twice). The run must see
/// exactly that one known violation and no other invariant failure (any fails the test).
/// About two minutes in release: `tools/tests/soak.sh` runs it.
#[test]
#[ignore = "a 27,000-call seeded replay: run in release (tools/tests/soak.sh)"]
fn soak_replay_seed_3_known_d1() {
    let calls = soak_side_with("replay 3", Some(1), false, || {
        let mut w = Soak::new("replay3", 3);
        w.max_ops = 27_000;
        w.outside = true;
        w.known_d1 = true;
        w.run(3_600, "replay 3");
        assert_eq!(w.n.ops, 27_000, "the replay ran to its call limit");
        w.n.d1_calls.clone()
    });
    assert_eq!(
        calls,
        vec![25_877],
        "exactly the known D1 violation at call 25,877"
    );
}
