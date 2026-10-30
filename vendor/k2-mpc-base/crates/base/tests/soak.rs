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
//! - **the file equals the doc** after a flush, and blame (by root) gives every line, with
//!   a writer who wrote in this doc;
//! - **claims and presence**: a new claim never overlaps another participant's (carried
//!   claims can, at a shared edge: counted), every claim belongs to a live connection, and the roster is exactly who is connected, with their
//!   connection counts;
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
//! It prints a progress line every `SOAK_PROGRESS_SECS` (default 30) and a summary.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::convert::Infallible;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use k2_mpc_base::session::{
    At, AuditEvent, ClaimPolicy, Config, ConnId, EditOptions, EditWithError, Host, LineRange,
    Participant, ParticipantKind, Rights, Session, SessionError, SessionManager, Stay,
    TextFilePack, UndoMode, UndoOutcome, UndoRequest, WriteOutcome,
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
}

impl Host for SoakHost {
    fn rights(&self, _who: &Participant, _key: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn on_event(&self, _ev: &AuditEvent) {}
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
    /// Checks (one per call) that saw two participants' carried claims overlap.
    overlap_checks: u64,
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

    fn token(&mut self, i: usize) -> String {
        let p = &mut self.parts[i];
        p.next += 1;
        let t = format!("{}:{}", p.pid, p.next);
        self.author.insert(t.clone(), p.pid.clone());
        t
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
                let req = UndoRequest::steps(self.rng.gen_range(1..=2)).mode(mode);
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
        assert_eq!(
            after_set.len(),
            after.len(),
            "{op}: a line twice: {after:?}"
        );
        let added: HashSet<String> = after_set
            .difference(&before_set)
            .map(|s| (*s).clone())
            .collect();
        let removed: HashSet<String> = before_set
            .difference(&after_set)
            .map(|s| (*s).clone())
            .collect();
        for l in &added {
            assert!(
                self.author.contains_key(l),
                "{op}: a line nobody wrote: {l:?}"
            );
        }
        let pid = self.parts[i].pid.clone();
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
            Expect::History(p) => {
                let mine = self.appeared_by.entry(p.clone()).or_default();
                for l in &removed {
                    assert!(
                        mine.contains(l),
                        "{op} by {p} removed {l} (by {}), which no change of {p}'s put there",
                        self.author[l]
                    );
                }
                let gone = self.removed_by.entry(p.clone()).or_default();
                for l in &added {
                    assert!(
                        gone.contains(l),
                        "{op} by {p} put back {l}, which {p} never removed"
                    );
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

    /// Claims and presence: every claim's connection is live; the roster is exactly who is
    /// connected; carried overlaps are counted (a new claim's are checked at `claim`).
    fn presence(&mut self) {
        let claims = self.claims();
        let mut overlaps = 0u64;
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
            // carried claims of two participants can come to overlap: once the text between
            // them is deleted they share an edge, and text typed there lands in both
            // (expanding anchors; tests/presence.rs, the ignored shared-edge test). Counted
            // and reported, not failed; a new claim never overlaps (checked at `claim`).
            for b in &claims[k + 1..] {
                if a.0 != b.0 && !(a.5 <= b.4 || b.5 <= a.4) {
                    overlaps += 1;
                }
            }
        }
        if overlaps > 0 {
            self.n.overlap_checks += 1;
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
        self.n.flushes += 1;
        let b = self.s.blame(self.obs(), ROOT, None).unwrap();
        assert_eq!(b.lines.len(), self.lines.len());
        let writers: HashSet<&String> = self.author.values().collect();
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

    fn run(&mut self, secs: u64, label: &str) {
        let progress = std::env::var("SOAK_PROGRESS_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(30u64);
        let start = Instant::now();
        let end = start + Duration::from_secs(secs);
        let mut next_progress = start + Duration::from_secs(progress);
        let restart_every = if secs <= 30 { 300 } else { 3_000 };
        while Instant::now() < end {
            self.advance();
            let claims_before = self.claims();
            let (op, i, policy, expect) = self.step();
            *self.n.by_op.entry(op).or_default() += 1;
            self.n.ops += 1;
            self.audit(op, i, policy, expect, &claims_before);
            self.presence();
            if self.n.ops % 250 == 0 {
                self.file_and_blame();
                self.sample_memory(start.elapsed().as_secs());
            }
            if self.n.ops % restart_every == 0 {
                self.restart();
            }
            if Instant::now() >= next_progress {
                next_progress += Duration::from_secs(progress);
                eprintln!(
                    "[soak {label}] {}s: {} ops ({} applied), v{}, {} lines, {} restarts, rss {} kB, est {} B",
                    start.elapsed().as_secs(),
                    self.n.ops,
                    self.n.applied,
                    self.version,
                    self.lines.len(),
                    self.n.restarts,
                    rss_kb(),
                    self.s.status().unwrap().estimated_bytes
                );
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
             {} restarts, {} blame+file checks ({} of {} lines blamed on the writer of their line break), {} joins, {} leaves, {} calls after which two participants' claims overlapped (shared edge); peak rss {} kB, peak kit estimate {} B \
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
            n.overlap_checks,
            n.peak_rss_kb,
            n.peak_estimate,
            self.config.memory_budget_bytes,
            sample(&n.rss_samples)
        );
        eprintln!("[soak {label}] by call: {:?}", n.by_op);
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
        ] {
            assert!(n.by_op.get(op).copied().unwrap_or(0) > 0, "{op} never ran");
        }
        assert!(
            n.applied * 3 > n.ops,
            "too few changes applied: {} of {}",
            n.applied,
            n.ops
        );
        assert!(n.restarts >= 1);
    }
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

/// CI: a few seconds of the same soak.
#[test]
fn soak_short() {
    let seed = env_u64("SOAK_SEED", 1);
    let mut w = Soak::new("short", seed);
    w.run(env_u64("SOAK_SHORT_SECS", 5), "short");
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
    let mut w = Soak::new("full", seed);
    w.run(secs, "32");
}
