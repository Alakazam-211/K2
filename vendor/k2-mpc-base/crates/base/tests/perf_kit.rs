//! What the kit's own edit path costs as one doc's history grows (v0.9.8 investigation;
//! `docs/evidence/perf-kit-v0.9.8.md`).
//!
//! `perf_kit_at_sizes` (ignored: minutes in release, run it on a node) grows **one** doc,
//! opened with a file as a host opens Markdown, through the public session API with a seeded
//! mix of calls by a few people and agents: plain `edit`s (a line replaced, added or
//! deleted), agents' `run_tool` verbs (`replace --lines`, `insert --line`, `delete --lines`,
//! `sed -i`), agents' `edit_with`, undo and redo, reads, and the host's ticks and flushes.
//! The text stays about [`TARGET_LINES`] lines, so the doc's size is constant and only its
//! history grows. Nothing is timed while it grows, except the kit's own profile scopes per
//! window of `PERF_WINDOW` versions (default 5,000) where the kit has them. At each window's
//! end it also records the session's structure sizes, redb's statistics of the store file
//! and 50 probes of the store's own commit (`perfkit_window_<version>`), so a step in the
//! store's cost shows with the file's size, its B-tree height and the compactions run.
//!
//! At each size (`PERF_SIZES`, default `10000,50000,100000` versions) it then:
//!
//! - **reopens** the doc (a fresh store handle, manager and session, as after a restart) and
//!   times the open and the first edit after it (which builds what the session builds on
//!   first need, e.g. the provenance map);
//! - takes **warm samples** (`PERF_ROUNDS` rounds, default 500), each call timed alone: a
//!   person's `edit`, an agent's `edit_with`, `run_tool replace --lines`, `run_tool sed -i`,
//!   a person's `undo` (of an untimed edit just before), a `run_tool cat` read, and a probe of
//!   the store alone (one durable `host_put` of a few bytes: the store's fixed cost per
//!   commit at this file size);
//! - prints the session's structure sizes before and after the samples.
//!
//! Every figure is a `RUNREC` line (`tools/evidence/README.md`): `perfkit_<size>_<call>`
//! (`p50_us`, `p95_us`, `mean_us`, `max_us`, and per profile scope its mean ns per call,
//! `<scope>_ns`), `perfkit_<size>_cold`, `perfkit_<size>_sizes`, and per growth window
//! `perfkit_window_<version>` (each scope's ns per call made in the window and entries).
//!
//! ```sh
//! cargo test --release -p k2-mpc-base --test perf_kit -- --ignored --nocapture --test-threads 1
//! ```
//!
//! Knobs: `PERF_SIZES`, `PERF_ROUNDS`, `PERF_PEOPLE` (default 4), `PERF_AGENTS` (default 4),
//! `PERF_SEED` (default 1). Growing to 100k versions takes minutes on a node; the sizes are
//! the knob if a run must be shorter.

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use k2_mpc_base::session::{
    AuditEvent, Config, ConnId, Host, Participant, ParticipantKind, Rights, Session, SessionError,
    SessionManager, Stay, TextFilePack, UndoOutcome, UndoRequest,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::text::diff::Hunk;
use k2_mpc_base::tools::{ToolOptions, ToolRunError};
use k2_mpc_base::Edit;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

const ROOT: &str = TextFilePack::ROOT;
/// The text's size: inserts win below `TARGET_LINES - 20`, deletes above `TARGET_LINES + 20`.
const TARGET_LINES: usize = 200;
/// Growth windows (versions) for the profile.
/// Growth windows (versions) for the profile, the store's statistics and the probe of its
/// commit: `PERF_WINDOW`, default 5,000.
fn window() -> u64 {
    env_u64("PERF_WINDOW", 5_000)
}

/// The kit's profile scopes since the last call: `(scope, entries, seconds)`, inclusive
/// times. v0.9.7's `prof!` scopes (`differential::profile_take`); a kit without them
/// gives nothing.
fn prof_take() -> Vec<(&'static str, u64, f64)> {
    k2_mpc_base::differential::profile_take()
}

struct PerfHost {
    now: AtomicU64,
}

impl Host for PerfHost {
    fn rights(&self, _who: &Participant, _key: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn on_event(&self, _ev: &AuditEvent) {}
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn config() -> Config {
    let mut c = Config::default();
    // a loop types faster than any person: no rate refusals, no idle leaves
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    c.idle_leave_ms = u64::MAX / 4;
    c
}

struct Part {
    pid: String,
    kind: ParticipantKind,
    /// Its one connection (none before the first join).
    conn: Option<ConnId>,
    next: u64,
}

struct World {
    rng: StdRng,
    dir: std::path::PathBuf,
    file: std::path::PathBuf,
    key: DocKey,
    host: Arc<PerfHost>,
    /// The store, for the probe (taken while the doc reopens: one handle per file).
    store: Option<Arc<DocStore>>,
    mgr: SessionManager,
    s: Arc<Session>,
    parts: Vec<Part>,
    people: usize,
    ops: u64,
}

fn open(
    dir: &std::path::Path,
    file: &std::path::Path,
    key: &DocKey,
    host: &Arc<PerfHost>,
) -> (Arc<DocStore>, SessionManager, Arc<Session>) {
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let mgr = SessionManager::new(store.clone(), host.clone(), config());
    let s = mgr
        .open_file(
            Arc::new(TextFilePack::new("markdown", 4 << 20)),
            key,
            file,
            &dir.join("files"),
        )
        .unwrap();
    (store, mgr, s)
}

fn lines_of(text: &str) -> Vec<&str> {
    text.split_terminator('\n').collect()
}

/// Byte offsets of line `n` (0-based) in `text`: `(start, end)`, `end` past its newline.
fn line_span(text: &str, n: usize) -> (usize, usize) {
    let mut start = 0;
    for _ in 0..n {
        start += text[start..].find('\n').expect("line exists") + 1;
    }
    let end = start
        + text[start..]
            .find('\n')
            .map_or(text.len() - start, |k| k + 1);
    (start, end)
}

/// What one call did, for the timed samples.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Call {
    Edit,
    EditWith,
    ToolReplace,
    ToolSed,
    Undo,
    ToolCat,
    StoreProbe,
    Flush,
    Tick,
}

impl Call {
    fn name(self) -> &'static str {
        match self {
            Call::Edit => "edit",
            Call::EditWith => "edit_with",
            Call::ToolReplace => "tool_replace",
            Call::ToolSed => "tool_sed",
            Call::Undo => "undo",
            Call::ToolCat => "tool_cat",
            Call::StoreProbe => "store_probe",
            Call::Flush => "flush",
            Call::Tick => "tick",
        }
    }
}

impl World {
    fn new(seed: u64, people: usize, agents: usize) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "k2mb-perfkit-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("files")).unwrap();
        let file = dir.join("files").join("notes.md");
        let seed_text: String = (0..TARGET_LINES).map(|i| format!("seed:{i}\n")).collect();
        std::fs::write(&file, &seed_text).unwrap();
        let host = Arc::new(PerfHost {
            now: AtomicU64::new(1_000_000),
        });
        let key = DocKey {
            space: "ws".into(),
            path: "notes.md".into(),
        };
        let (store, mgr, s) = open(&dir, &file, &key, &host);
        let mut w = World {
            rng: StdRng::seed_from_u64(seed),
            dir,
            file,
            key,
            host,
            store: Some(store),
            mgr,
            s,
            parts: Vec::new(),
            people,
            ops: 0,
        };
        for i in 0..people + agents {
            let kind = if i < people {
                ParticipantKind::Person
            } else {
                ParticipantKind::Agent
            };
            let pid = format!("p{i:02}");
            w.parts.push(Part {
                pid,
                kind,
                conn: None,
                next: 0,
            });
        }
        w.join_all();
        w
    }

    fn join_all(&mut self) {
        for (i, p) in self.parts.iter_mut().enumerate() {
            let via = if p.kind == ParticipantKind::Person {
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
            p.conn = Some(self.s.join(who, Stay::Held).unwrap().conn);
        }
    }

    /// Release everything and open the doc again (a restart); returns the open's seconds.
    fn reopen(&mut self) -> f64 {
        self.mgr.release().unwrap();
        self.store = None;
        let t = Instant::now();
        let (store, mgr, s) = open(&self.dir, &self.file, &self.key, &self.host);
        let secs = t.elapsed().as_secs_f64();
        self.store = Some(store);
        self.mgr = mgr;
        self.s = s;
        self.join_all();
        secs
    }

    fn conn(&self, i: usize) -> ConnId {
        self.parts[i].conn.expect("joined")
    }

    fn token(&mut self, i: usize) -> String {
        let p = &mut self.parts[i];
        p.next += 1;
        format!("{}:{}", p.pid, p.next)
    }

    fn advance(&mut self) {
        let ms = self.rng.gen_range(50..300);
        self.host.now.fetch_add(ms, Ordering::SeqCst);
    }

    fn version(&self) -> u64 {
        self.s.status().unwrap().version
    }

    fn person(&mut self) -> usize {
        self.rng.gen_range(0..self.people)
    }

    fn agent(&mut self) -> usize {
        self.rng.gen_range(self.people..self.parts.len())
    }

    /// A person's plain `edit` (hunks from a read, guarded by its version): a line replaced
    /// (or added / deleted to keep the size near the target).
    fn person_edit(&mut self, i: usize, timed: bool) -> Option<f64> {
        let conn = self.conn(i);
        let r = self.s.read(conn).unwrap();
        let text = r.text_by_root[0].1.clone();
        let n = lines_of(&text).len();
        let t = self.token(i);
        let k = self.rng.gen_range(0..n.max(1));
        let pick = self.rng.gen_range(0..10);
        let hunk = if n < TARGET_LINES - 20 || (pick == 0 && n < TARGET_LINES + 20) || n == 0 {
            let at = if n == 0 { 0 } else { line_span(&text, k).0 };
            Hunk {
                start: at,
                end: at,
                insert: format!("{t}\n"),
            }
        } else if n > TARGET_LINES + 20 || pick == 1 {
            let (s, e) = line_span(&text, k);
            Hunk {
                start: s,
                end: e,
                insert: String::new(),
            }
        } else {
            let (s, e) = line_span(&text, k);
            Hunk {
                start: s,
                end: e,
                insert: format!("{t}\n"),
            }
        };
        let edits = vec![Edit::Text {
            root: ROOT.into(),
            hunks: vec![hunk],
        }];
        let t0 = Instant::now();
        let res = self.s.edit(conn, edits, Some(r.version));
        let secs = t0.elapsed().as_secs_f64();
        res.unwrap();
        timed.then_some(secs)
    }

    fn line_count(&self) -> usize {
        let r = self.s.read(self.conn(0)).unwrap();
        lines_of(&r.text_by_root[0].1).len()
    }

    /// A `run_tool` call; its time, or `None` when the tool refused it (a line the doc no
    /// longer has: the growth phase reads the size only now and then).
    fn tool(&mut self, i: usize, argv: &[String], stdin: &str) -> Option<f64> {
        let conn = self.conn(i);
        let t0 = Instant::now();
        let res = self
            .s
            .run_tool_with(conn, ROOT, argv, stdin.as_bytes(), &ToolOptions::new());
        let secs = t0.elapsed().as_secs_f64();
        match res {
            Ok(_) | Err(ToolRunError::Session(SessionError::Stale { .. })) => Some(secs),
            Err(ToolRunError::Tool(_)) => None,
            Err(e) => panic!("tool {argv:?}: {e:?}"),
        }
    }

    /// An agent's `run_tool`: replace, insert or delete one line (by the size), or `sed -i`.
    fn agent_tool(&mut self, i: usize, sed: bool, n: usize) -> Option<f64> {
        let a = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<String>>();
        let t = self.token(i);
        let k = self.rng.gen_range(1..=n.max(1));
        let ks = k.to_string();
        if sed {
            let script = format!("{k}s/.*/{t}/");
            return self.tool(i, &a(&["sed", "-i", &script]), "");
        }
        if n < TARGET_LINES - 20 {
            self.tool(i, &a(&["insert", "--line", &ks]), &format!("{t}\n"))
        } else if n > TARGET_LINES + 20 {
            let r = format!("{k},{k}");
            self.tool(i, &a(&["delete", "--lines", &r]), "")
        } else {
            let r = format!("{k},{k}");
            self.tool(i, &a(&["replace", "--lines", &r]), &format!("{t}\n"))
        }
    }

    /// An agent's `edit_with`: one line replaced through a closure on the live text.
    fn agent_edit_with(&mut self, i: usize) -> f64 {
        let conn = self.conn(i);
        let t = self.token(i);
        let pick: u64 = self.rng.gen();
        let t0 = Instant::now();
        let res = self.s.edit_with(conn, ROOT, move |live| {
            let mut ls: Vec<&str> = lines_of(live);
            if !ls.is_empty() {
                let k = (pick % ls.len() as u64) as usize;
                ls[k] = &t;
            }
            Ok::<_, Infallible>(ls.iter().map(|l| format!("{l}\n")).collect::<String>())
        });
        let secs = t0.elapsed().as_secs_f64();
        res.unwrap();
        secs
    }

    fn undo(&mut self, i: usize, redo: bool) -> f64 {
        let conn = self.conn(i);
        let req = UndoRequest::steps(1);
        let t0 = Instant::now();
        let res = if redo {
            self.s.redo_with(conn, &req)
        } else {
            self.s.undo_with(conn, &req)
        };
        let secs = t0.elapsed().as_secs_f64();
        // nothing to undo (an empty stack) is an answer, not a failure
        let _: UndoOutcome = res.unwrap_or_else(|e| panic!("undo: {e:?}"));
        secs
    }

    /// One growth step, chosen by the seed (nothing timed).
    fn step(&mut self, n_lines: usize) {
        self.advance();
        self.ops += 1;
        let pick = self.rng.gen_range(0..100);
        match pick {
            0..=39 => {
                let i = self.person();
                self.person_edit(i, false);
            }
            40..=59 => {
                let i = self.agent();
                self.agent_tool(i, false, n_lines);
            }
            60..=69 => {
                let i = self.agent();
                self.agent_tool(i, true, n_lines);
            }
            70..=79 => {
                let i = self.agent();
                self.agent_edit_with(i);
            }
            80..=89 => {
                let i = self.rng.gen_range(0..self.parts.len());
                self.undo(i, false);
            }
            90..=92 => {
                let i = self.rng.gen_range(0..self.parts.len());
                self.undo(i, true);
            }
            _ => {
                let i = self.agent();
                let argv = vec!["cat".to_owned()];
                self.tool(i, &argv, "");
            }
        }
        if self.ops % 25 == 0 {
            self.mgr.tick().unwrap();
        }
        if self.ops % 250 == 0 {
            let c = self.conn(0);
            self.s.flush(c).unwrap();
        }
    }

    fn sizes(&self) -> Vec<(&'static str, u64)> {
        let mut v = self.s.perf_sizes_for_tests().unwrap();
        let file = std::fs::metadata(self.dir.join("docs.redb")).map_or(0, |m| m.len());
        v.push(("store_file_bytes", file));
        let store = self.store.clone().expect("open");
        v.extend(store.redb_stats_for_tests().unwrap());
        v
    }
}

/// `n` durable commits of a few bytes on the store alone (no doc change): `(p50, mean)`
/// seconds, the store's fixed cost per commit at this file size.
fn probe(w: &World, n: usize) -> (f64, f64) {
    let store = w.store.clone().expect("open");
    let mut v = Vec::with_capacity(n);
    for k in 0..n {
        let key = format!("probe-{}", k % 8);
        let t0 = Instant::now();
        store.host_put("perf-kit", &key, b"probe").unwrap();
        v.push(t0.elapsed().as_secs_f64());
    }
    v.sort_by(f64::total_cmp);
    (quantile(&v, 0.5), v.iter().sum::<f64>() / n.max(1) as f64)
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A profile summed per scope: (entries, seconds).
type Prof = BTreeMap<&'static str, (u64, f64)>;

fn add_prof(acc: &mut Prof, p: Vec<(&'static str, u64, f64)>) {
    for (k, n, s) in p {
        let e = acc.entry(k).or_default();
        e.0 += n;
        e.1 += s;
    }
}

/// `session.prov_build` → `session_prov_build`: a field name the evidence tools take.
fn ident(scope: &str) -> String {
    scope
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

fn job() -> String {
    std::env::var("K2_COMPUTE_JOB")
        .ok()
        .filter(|j| !j.is_empty())
        .map_or_else(|| "null".to_owned(), |j| format!("\"{j}\""))
}

fn runrec(run: &str, fields: &[(String, u64)]) {
    let mut s = format!(
        "RUNREC {{\"job\":{},\"run\":\"{run}\",\"tool\":\"perf_kit\",\"status\":\"COMPLETE\"",
        job()
    );
    for (k, v) in fields {
        s.push_str(&format!(",\"{k}\":{v}"));
    }
    s.push('}');
    eprintln!("{s}");
}

fn quantile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let k = ((sorted.len() - 1) as f64 * q).round() as usize;
    sorted[k]
}

fn us(s: f64) -> u64 {
    (s * 1e6).round() as u64
}

fn ns(s: f64) -> u64 {
    (s * 1e9).round() as u64
}

/// Each scope's ns per call (`calls` calls) and entries per 1,000 calls.
fn prof_fields(p: &Prof, calls: u64) -> Vec<(String, u64)> {
    let c = calls.max(1) as f64;
    let mut out = Vec::new();
    for (k, (n, s)) in p {
        out.push((format!("{}_ns", ident(k)), ns(s / c)));
        out.push((
            format!("{}_per_kcalls", ident(k)),
            (*n as f64 * 1000.0 / c).round() as u64,
        ));
    }
    out
}

fn print_prof(label: &str, p: &Prof, calls: u64) {
    let mut v: Vec<(&&str, &(u64, f64))> = p.iter().collect();
    v.sort_by(|a, b| b.1 .1.total_cmp(&a.1 .1));
    let parts: Vec<String> = v
        .iter()
        .map(|(k, (n, s))| {
            format!(
                "{k} {:.1} µs/call ({n} entries)",
                s * 1e6 / calls.max(1) as f64
            )
        })
        .collect();
    if !parts.is_empty() {
        eprintln!("[perf_kit] {label}: {}", parts.join(", "));
    }
}

/// The measurements at one size.
fn measure(w: &mut World, size: u64, rounds: u64) {
    let v0 = w.version();
    eprintln!("[perf_kit] measuring at v{v0} (target {size})");
    let sizes = w.sizes();
    runrec(
        &format!("perfkit_{size}_sizes"),
        &sizes
            .iter()
            .map(|(k, v)| ((*k).to_owned(), *v))
            .collect::<Vec<_>>(),
    );
    eprintln!("[perf_kit] sizes at v{v0}: {sizes:?}");

    // cold: a restart, then the first calls
    let _ = prof_take();
    let open_s = w.reopen();
    let open_prof = {
        let mut p = Prof::new();
        add_prof(&mut p, prof_take());
        p
    };
    let i = w.person();
    let first_edit = w.person_edit(i, true).unwrap();
    let mut cold_prof = Prof::new();
    add_prof(&mut cold_prof, prof_take());
    let second_edit = w.person_edit(i, true).unwrap();
    let _ = prof_take();
    let n_lines = w.line_count();
    let a = w.agent();
    let first_tool = w.agent_tool(a, false, n_lines).expect("a line the doc has");
    let _ = prof_take();
    let first_undo = w.undo(i, false);
    let mut undo_prof = Prof::new();
    add_prof(&mut undo_prof, prof_take());
    let mut fields = vec![
        ("version".to_owned(), v0),
        ("open_us".to_owned(), us(open_s)),
        ("first_edit_us".to_owned(), us(first_edit)),
        ("second_edit_us".to_owned(), us(second_edit)),
        ("first_tool_us".to_owned(), us(first_tool)),
        ("first_undo_us".to_owned(), us(first_undo)),
    ];
    for (k, v) in prof_fields(&cold_prof, 1) {
        fields.push((format!("first_edit_{k}"), v));
    }
    for (k, v) in prof_fields(&undo_prof, 1) {
        fields.push((format!("first_undo_{k}"), v));
    }
    for (k, v) in prof_fields(&open_prof, 1) {
        fields.push((format!("open_{k}"), v));
    }
    runrec(&format!("perfkit_{size}_cold"), &fields);
    eprintln!(
        "[perf_kit] v{v0} cold: open {:.1} ms, first edit {:.0} µs, second {:.0} µs, first tool {:.0} µs, first undo {:.0} µs",
        open_s * 1e3,
        first_edit * 1e6,
        second_edit * 1e6,
        first_tool * 1e6,
        first_undo * 1e6
    );
    print_prof(&format!("v{v0} first edit after open"), &cold_prof, 1);

    // warm-up: 100 growth steps
    for _ in 0..100 {
        let n = w.line_count();
        w.step(n);
    }
    let _ = prof_take();

    // warm samples, each call timed alone, interleaved
    let mut times: BTreeMap<Call, Vec<f64>> = BTreeMap::new();
    let mut profs: BTreeMap<Call, Prof> = BTreeMap::new();
    let mut record = |c: Call, secs: f64, times: &mut BTreeMap<Call, Vec<f64>>| {
        times.entry(c).or_default().push(secs);
        add_prof(profs.entry(c).or_default(), prof_take());
    };
    for r in 0..rounds {
        w.advance();
        let _ = prof_take();
        let p = w.person();
        let s = w.person_edit(p, true).unwrap();
        record(Call::Edit, s, &mut times);
        let a = w.agent();
        let s = w.agent_edit_with(a);
        record(Call::EditWith, s, &mut times);
        // the size read just before each tool call (untimed), so its line exists
        let n_lines = w.line_count();
        let _ = prof_take();
        let a = w.agent();
        let s = w.agent_tool(a, false, n_lines).expect("a line the doc has");
        record(Call::ToolReplace, s, &mut times);
        let n_lines = w.line_count();
        let _ = prof_take();
        let a = w.agent();
        let s = w.agent_tool(a, true, n_lines).expect("a line the doc has");
        record(Call::ToolSed, s, &mut times);
        let q = w.person();
        w.person_edit(q, false);
        let _ = prof_take();
        let s = w.undo(q, false);
        record(Call::Undo, s, &mut times);
        let a = w.agent();
        let s = w.tool(a, &["cat".to_owned()], "").expect("a read");
        record(Call::ToolCat, s, &mut times);
        let key = format!("probe-{}", r % 8);
        let store = w.store.clone().expect("open");
        let t0 = Instant::now();
        store.host_put("perf-kit", &key, b"probe").unwrap();
        let s = t0.elapsed().as_secs_f64();
        record(Call::StoreProbe, s, &mut times);
        // every 10 rounds: the host's write-back (a flush) and, a minute later, its tick
        // (the disk clean, so the tick is the history duties: retention, the revert window)
        if r % 10 == 9 {
            let c = w.conn(0);
            let _ = prof_take();
            let t0 = Instant::now();
            w.s.flush(c).unwrap();
            let s = t0.elapsed().as_secs_f64();
            record(Call::Flush, s, &mut times);
            w.host.now.fetch_add(61_000, Ordering::SeqCst);
            let t0 = Instant::now();
            w.mgr.tick().unwrap();
            let s = t0.elapsed().as_secs_f64();
            record(Call::Tick, s, &mut times);
        }
    }
    let v1 = w.version();
    for (c, mut v) in times {
        v.sort_by(f64::total_cmp);
        let mean = v.iter().sum::<f64>() / v.len().max(1) as f64;
        let prof = profs.remove(&c).unwrap_or_default();
        let mut fields = vec![
            ("version".to_owned(), v0),
            ("version_after".to_owned(), v1),
            ("samples".to_owned(), v.len() as u64),
            ("p50_us".to_owned(), us(quantile(&v, 0.5))),
            ("p95_us".to_owned(), us(quantile(&v, 0.95))),
            ("mean_us".to_owned(), us(mean)),
            ("max_us".to_owned(), us(*v.last().unwrap_or(&0.0))),
        ];
        fields.extend(prof_fields(&prof, v.len() as u64));
        runrec(&format!("perfkit_{size}_{}", c.name()), &fields);
        eprintln!(
            "[perf_kit] v{v0} {}: p50 {:.0} µs, p95 {:.0} µs, mean {:.0} µs, max {:.0} µs ({} samples)",
            c.name(),
            quantile(&v, 0.5) * 1e6,
            quantile(&v, 0.95) * 1e6,
            mean * 1e6,
            v.last().unwrap_or(&0.0) * 1e6,
            v.len()
        );
        print_prof(&format!("v{v0} {}", c.name()), &prof, v.len() as u64);
    }
    let sizes = w.sizes();
    eprintln!("[perf_kit] sizes at v{v1}: {sizes:?}");
}

#[test]
#[ignore = "a measurement: run in release on a node with --nocapture"]
fn perf_kit_at_sizes() {
    let sizes: Vec<u64> = std::env::var("PERF_SIZES")
        .unwrap_or_else(|_| "10000,50000,100000".to_owned())
        .split(',')
        .map(|s| s.trim().parse().expect("PERF_SIZES: numbers"))
        .collect();
    let rounds = env_u64("PERF_ROUNDS", 500);
    let people = env_u64("PERF_PEOPLE", 4) as usize;
    let agents = env_u64("PERF_AGENTS", 4) as usize;
    let seed = env_u64("PERF_SEED", 1);
    let mut w = World::new(seed, people, agents);
    let start = Instant::now();
    let mut grow_s = 0.0;
    let mut window_prof = Prof::new();
    let mut window_ops = 0u64;
    let win = window();
    let mut next_window = win;
    let mut win_t = Instant::now();
    let mut compactions = 0u64;
    let mut last_log = w.s.status().unwrap().log_entries;
    for &size in &sizes {
        let t = Instant::now();
        let mut n_lines = w.line_count();
        let mut v = w.version();
        while v < size {
            w.step(n_lines);
            window_ops += 1;
            add_prof(&mut window_prof, prof_take());
            if w.ops % 16 == 0 {
                n_lines = w.line_count();
            }
            let st = w.s.status().unwrap();
            v = st.version;
            // the log is cut back to the snapshot by a compaction
            if st.log_entries < last_log {
                compactions += 1;
            }
            last_log = st.log_entries;
            if v >= next_window {
                let wall = win_t.elapsed().as_secs_f64();
                let (p50, mean) = probe(&w, 50);
                let mut fields = vec![
                    ("version".to_owned(), v),
                    ("calls".to_owned(), window_ops),
                    (
                        "us_per_call".to_owned(),
                        us(wall / window_ops.max(1) as f64),
                    ),
                    ("compactions".to_owned(), compactions),
                    ("probe_p50_us".to_owned(), us(p50)),
                    ("probe_mean_us".to_owned(), us(mean)),
                ];
                fields.extend(
                    w.sizes()
                        .iter()
                        .filter(|(k, _)| *k != "version")
                        .map(|(k, x)| ((*k).to_owned(), *x)),
                );
                fields.extend(prof_fields(&window_prof, window_ops));
                runrec(&format!("perfkit_window_{next_window}"), &fields);
                eprintln!(
                    "[perf_kit] window to v{v}: {:.0} µs per call, {compactions} compactions, store probe p50 {:.0} µs",
                    wall * 1e6 / window_ops.max(1) as f64,
                    p50 * 1e6
                );
                print_prof(&format!("window to v{v}"), &window_prof, window_ops);
                window_prof.clear();
                window_ops = 0;
                compactions = 0;
                next_window += win;
                let _ = prof_take();
                win_t = Instant::now();
            }
        }
        let g = t.elapsed().as_secs_f64();
        grow_s += g;
        eprintln!(
            "[perf_kit] grew to v{v} in {g:.1} s ({} calls so far, {:.0} s in all)",
            w.ops,
            start.elapsed().as_secs_f64()
        );
        measure(&mut w, size, rounds);
        // the windows time growth only
        let _ = prof_take();
        win_t = Instant::now();
        last_log = w.s.status().unwrap().log_entries;
    }
    // the store without its sync to disk (redb's Durability::None, test-only): what of a
    // commit is the sync and what the store's own work, at the last size
    if let Some(&last) = sizes.last() {
        w.store.clone().expect("open").unsynced_for_tests();
        let _ = prof_take();
        let mut v = Vec::new();
        let mut prof = Prof::new();
        for _ in 0..rounds {
            w.advance();
            let p = w.person();
            let _ = prof_take();
            v.push(w.person_edit(p, true).unwrap());
            add_prof(&mut prof, prof_take());
        }
        v.sort_by(f64::total_cmp);
        let mean = v.iter().sum::<f64>() / v.len().max(1) as f64;
        let mut fields = vec![
            ("version".to_owned(), w.version()),
            ("samples".to_owned(), v.len() as u64),
            ("p50_us".to_owned(), us(quantile(&v, 0.5))),
            ("p95_us".to_owned(), us(quantile(&v, 0.95))),
            ("mean_us".to_owned(), us(mean)),
        ];
        fields.extend(prof_fields(&prof, v.len() as u64));
        runrec(&format!("perfkit_{last}_edit_nosync"), &fields);
        eprintln!(
            "[perf_kit] edit without the store's sync: p50 {:.0} µs, mean {:.0} µs",
            quantile(&v, 0.5) * 1e6,
            mean * 1e6
        );
        print_prof("edit without the store's sync", &prof, v.len() as u64);
    }
    runrec(
        "perfkit_total",
        &[
            ("calls".to_owned(), w.ops),
            ("grow_s".to_owned(), grow_s.round() as u64),
            (
                "wall_s".to_owned(),
                start.elapsed().as_secs_f64().round() as u64,
            ),
            ("people".to_owned(), people as u64),
            ("agents".to_owned(), agents as u64),
            ("rounds".to_owned(), rounds),
        ],
    );
}
