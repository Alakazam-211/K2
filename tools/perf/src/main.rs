//! The kit's performance harness (tools/README.md: Performance harness): one Markdown doc grown through the
//! public session API to fixed sizes (10k, 100k, 1M versions), then every measured path
//! timed alone on a copy of each size, p50/p95/p99 per path. A job runs two builds (A and
//! B) interleaved A/B/A/B on one node (`tools/perf/pair.sh`); `summarize` takes the median
//! of the runs per side and the B/A ratio.
//!
//! Commands (each prints `RUNREC` lines, tools/evidence/README.md):
//!
//! - `grow --work DIR --out DIR --sizes 10000,100000,1000000 [--name h] [--side A]
//!   [--seed 1] [--lines 200] [--line-len 0] [--window 50000]`: grow one doc in `--work`
//!   and copy it to `--out/v<size>` at each size. `--lines`/`--line-len` set the text's
//!   shape (the large-doc fixture: about 39,000 lines of 80 bytes, about 3 MiB).
//! - `measure --fixture DIR --work DIR --name h10000 --size 10000 --side A --run 1
//!   --rows history|content|nosync --tsv FILE [--build LABEL] [--samples 10000]
//!   [--fast-cap-s 900] [--slow-cap-s 60] [--cold-cap-s 60] [--seed 1]`: copy the fixture
//!   to `--work`, time the rows, print one `RUNREC` per row and append it to `--tsv`.
//!   `nosync` (the `edit_nosync` row and the live-run count) only in the build with the
//!   `nosync` feature, which measures nothing else.
//! - `summarize --tsv FILE`: per fixture and row, the median of the runs per side, the
//!   B/A ratio, growth from the smallest to the largest size, and which p99s are
//!   indicative (fewer than 10,000 samples in a run).

mod alloc;

#[global_allocator]
static GLOBAL: alloc::Counting = alloc::Counting;

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use k2_mpc_base::session::{
    AuditEvent, Config, ConnId, DiskCheck, Host, LineRange, Pack, Participant, ParticipantKind,
    Rights, Session, SessionError, SessionManager, Stay, TextFilePack, UndoRequest, WriteOutcome,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::text::diff::Hunk;
use k2_mpc_base::tools::{ToolOptions, ToolRunError};
use k2_mpc_base::{Doc, Edit};

const ROOT: &str = TextFilePack::ROOT;
/// Markdown's live limit (as a host opens it).
const LIMIT: usize = 4 << 20;
/// The p99 of a run with fewer samples than this is marked indicative.
const P99_FULL: usize = 10_000;
/// Filler for long lines: no character `sed`'s replacement treats specially.
const LOREM: &str = "lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor incididunt ut labore et dolore magna aliqua ";

/// `--timers on|off`: the kit's timers (the `kit-timers` build; a kit without them says
/// `absent`, and `on` there is refused).
fn timers_switch(arg: &str) -> &'static str {
    let on = match arg {
        "on" => true,
        "off" => false,
        x => die(&format!("--timers {x}: on or off")),
    };
    timers_set(on)
}

#[cfg(feature = "kit-timers")]
fn timers_set(on: bool) -> &'static str {
    k2_mpc_base::timers::reset();
    k2_mpc_base::timers::set_enabled(on);
    if on {
        "on"
    } else {
        "off"
    }
}

#[cfg(not(feature = "kit-timers"))]
fn timers_set(on: bool) -> &'static str {
    if on {
        die("--timers on needs the kit-timers build (a kit with crates/base/src/timers.rs)");
    }
    "absent"
}

/// Whether the kit was built with its test hooks, as the kit itself says (`timers::HOOKS`);
/// `unknown` for a kit without timers.
#[cfg(feature = "kit-timers")]
fn kit_hooks() -> &'static str {
    if k2_mpc_base::timers::HOOKS {
        "yes"
    } else {
        "no"
    }
}

#[cfg(not(feature = "kit-timers"))]
fn kit_hooks() -> &'static str {
    "unknown"
}

fn features() -> &'static str {
    if cfg!(feature = "hooks") {
        "differential,conformance"
    } else if cfg!(feature = "nosync") {
        "conformance"
    } else {
        "none"
    }
}

fn die(msg: &str) -> ! {
    eprintln!("k2-mpc-perf: {msg}");
    std::process::exit(2)
}

// ---------------------------------------------------------------- arguments

struct Args {
    cmd: String,
    kv: BTreeMap<String, String>,
}

impl Args {
    fn parse() -> Self {
        let mut it = std::env::args().skip(1);
        let cmd = it.next().unwrap_or_default();
        let mut kv = BTreeMap::new();
        while let Some(k) = it.next() {
            let Some(k) = k.strip_prefix("--") else {
                die(&format!("expected --name value, got {k}"))
            };
            let v = it
                .next()
                .unwrap_or_else(|| die(&format!("--{k}: no value")));
            kv.insert(k.to_owned(), v);
        }
        Args { cmd, kv }
    }
    fn s(&self, k: &str) -> String {
        self.kv
            .get(k)
            .cloned()
            .unwrap_or_else(|| die(&format!("--{k} is required")))
    }
    fn s_or(&self, k: &str, d: &str) -> String {
        self.kv.get(k).cloned().unwrap_or_else(|| d.to_owned())
    }
    fn u(&self, k: &str, d: u64) -> u64 {
        self.kv.get(k).map_or(d, |v| {
            v.replace('_', "")
                .parse()
                .unwrap_or_else(|_| die(&format!("--{k}: a number")))
        })
    }
    fn f(&self, k: &str, d: f64) -> f64 {
        self.kv.get(k).map_or(d, |v| {
            v.parse()
                .unwrap_or_else(|_| die(&format!("--{k}: a number")))
        })
    }
    fn list(&self, k: &str) -> Vec<u64> {
        self.s(k)
            .split(',')
            .map(|x| {
                x.trim()
                    .replace('_', "")
                    .parse()
                    .unwrap_or_else(|_| die(&format!("--{k}: numbers")))
            })
            .collect()
    }
}

// ---------------------------------------------------------------- the seeded mix

/// SplitMix64: the harness's only randomness, so a seed gives the same calls in every
/// build.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed ^ 0x5DEE_CE66_D1CE_4E5B)
    }
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }
    fn range(&mut self, lo: usize, hi: usize) -> usize {
        lo + self.below(hi - lo)
    }
}

/// The text's shape: about `lines` lines; a line written is `line_len` bytes long (with
/// its newline) when that is longer than its token.
#[derive(Clone, Copy)]
struct Shape {
    lines: usize,
    line_len: usize,
}

impl Shape {
    fn body(&self, tok: &str) -> String {
        let mut s = String::with_capacity(self.line_len.max(tok.len() + 1));
        s.push_str(tok);
        if self.line_len > tok.len() + 2 {
            s.push(' ');
            let start = (tok.len() * 7) % LOREM.len();
            let mut it = LOREM.bytes().cycle().skip(start);
            while s.len() + 1 < self.line_len {
                s.push(it.next().unwrap_or(b'x') as char);
            }
        }
        s
    }
    fn line(&self, tok: &str) -> String {
        let mut s = self.body(tok);
        s.push('\n');
        s
    }
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

/// The session config: defaults, except that a loop types faster than any person (no
/// rate refusals, no idle leaves or unloads) and the pool budget never unloads the doc.
fn config() -> Config {
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    c.idle_leave_ms = u64::MAX / 4;
    c.idle_unload_ms = u64::MAX / 4;
    c.memory_budget_bytes = u64::MAX / 4;
    c
}

struct Part {
    pid: String,
    kind: ParticipantKind,
    conn: Option<ConnId>,
    next: u64,
}

fn lines_of(text: &str) -> Vec<&str> {
    text.split_terminator('\n').collect()
}

/// Byte offsets of line `n` (0-based): `(start, end)`, `end` past its newline.
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

fn copy_dir(src: &Path, dst: &Path) {
    let _ = std::fs::remove_dir_all(dst);
    std::fs::create_dir_all(dst).expect("create dir");
    for e in std::fs::read_dir(src).expect("read dir") {
        let e = e.expect("dir entry");
        let to = dst.join(e.file_name());
        if e.file_type().expect("file type").is_dir() {
            copy_dir(&e.path(), &to);
        } else {
            std::fs::copy(e.path(), &to).expect("copy");
        }
    }
}

fn file_len(p: &Path) -> u64 {
    std::fs::metadata(p).map_or(0, |m| m.len())
}

/// One doc, opened with a file as a host opens Markdown, and its participants.
struct World {
    rng: Rng,
    work: PathBuf,
    file: PathBuf,
    key: DocKey,
    host: Arc<PerfHost>,
    store: Option<Arc<DocStore>>,
    mgr: Option<SessionManager>,
    sess: Option<Arc<Session>>,
    parts: Vec<Part>,
    people: usize,
    ops: u64,
    shape: Shape,
    /// The `nosync` build: every open turns the store's sync off.
    unsynced: bool,
    /// The last open's internal paths (the kit's timers).
    last_open: Vec<(&'static str, u64)>,
}

impl World {
    fn base(work: &Path, seed: u64, people: usize, agents: usize, shape: Shape, now: u64) -> Self {
        let parts = (0..people + agents)
            .map(|i| Part {
                pid: format!("p{i:02}"),
                kind: if i < people {
                    ParticipantKind::Person
                } else {
                    ParticipantKind::Agent
                },
                conn: None,
                next: 0,
            })
            .collect();
        World {
            rng: Rng::new(seed),
            work: work.to_owned(),
            file: work.join("files").join("notes.md"),
            key: DocKey {
                space: "ws".into(),
                path: "notes.md".into(),
            },
            host: Arc::new(PerfHost {
                now: AtomicU64::new(now),
            }),
            store: None,
            mgr: None,
            sess: None,
            parts,
            people,
            ops: 0,
            shape,
            unsynced: false,
            last_open: Vec::new(),
        }
    }

    /// A new doc in `work`: the seed text, then opened.
    fn create(work: &Path, seed: u64, shape: Shape) -> Self {
        let _ = std::fs::remove_dir_all(work);
        std::fs::create_dir_all(work.join("files")).expect("work dir");
        let mut w = World::base(work, seed, 4, 4, shape, 1_000_000);
        let text: String = (0..shape.lines)
            .map(|i| shape.line(&format!("seed:{i}")))
            .collect();
        std::fs::write(&w.file, text).expect("seed file");
        w.open();
        w
    }

    /// A doc a `grow` left in `work` (its `harness.txt`: the host's clock and the shape).
    fn attach(work: &Path, seed: u64) -> Self {
        let meta = std::fs::read_to_string(work.join("harness.txt")).expect("harness.txt");
        let get = |k: &str| -> u64 {
            meta.lines()
                .find_map(|l| l.strip_prefix(k).and_then(|r| r.strip_prefix('=')))
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or_else(|| die(&format!("harness.txt: no {k}")))
        };
        let shape = Shape {
            lines: get("lines") as usize,
            line_len: get("line_len") as usize,
        };
        World::base(work, seed, 4, 4, shape, get("host_now"))
    }

    fn s(&self) -> &Arc<Session> {
        self.sess.as_ref().expect("open")
    }

    fn mgr(&self) -> &SessionManager {
        self.mgr.as_ref().expect("open")
    }

    /// Open the store, the manager and the doc, and join everyone: `(open seconds, each
    /// join's seconds)`.
    fn open(&mut self) -> (f64, Vec<(f64, Vec<(&'static str, u64)>)>) {
        let t = clock();
        let store = Arc::new(DocStore::open(self.work.join("docs.redb")).expect("store"));
        let mgr = SessionManager::new(store.clone(), self.host.clone(), config());
        let s = mgr
            .open_file(
                Arc::new(TextFilePack::new("markdown", LIMIT)),
                &self.key,
                &self.file,
                &self.work.join("files"),
            )
            .expect("open_file");
        let open_s = t.secs();
        self.last_open = take_last();
        #[cfg(feature = "nosync")]
        if self.unsynced {
            store.unsynced_for_tests();
        }
        self.store = Some(store);
        self.mgr = Some(mgr);
        self.sess = Some(s);
        let joins = self.join_all();
        (open_s, joins)
    }

    /// Release everything (the file is written back) and record the host's clock and the
    /// shape beside the store.
    fn close(&mut self) {
        if let Some(m) = self.mgr.take() {
            m.release().expect("release");
        }
        self.sess = None;
        self.store = None;
        let meta = format!(
            "host_now={}\nlines={}\nline_len={}\n",
            self.host.now.load(Ordering::SeqCst),
            self.shape.lines,
            self.shape.line_len
        );
        std::fs::write(self.work.join("harness.txt"), meta).expect("harness.txt");
    }

    fn join_all(&mut self) -> Vec<(f64, Vec<(&'static str, u64)>)> {
        let s = self.sess.clone().expect("open");
        let mut v = Vec::with_capacity(self.parts.len());
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
            let t = clock();
            let j = s.join(who, Stay::Held).expect("join");
            v.push((t.secs(), take_last()));
            p.conn = Some(j.conn);
        }
        v
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
        let ms = self.rng.range(50, 300) as u64;
        self.host.now.fetch_add(ms, Ordering::SeqCst);
    }

    fn version(&self) -> u64 {
        self.s().status().expect("status").version
    }

    fn person(&mut self) -> usize {
        self.rng.below(self.people)
    }

    fn agent(&mut self) -> usize {
        self.rng.range(self.people, self.parts.len())
    }

    fn text(&self) -> String {
        let r = self.s().read(self.conn(0)).expect("read");
        r.text_by_root
            .into_iter()
            .next()
            .map(|x| x.1)
            .unwrap_or_default()
    }

    fn line_count(&self) -> usize {
        lines_of(&self.text()).len()
    }

    /// A person's plain `edit` (hunks from a read, guarded by its version): a line
    /// replaced, or added / deleted to keep the size near the shape's. `(seconds, version
    /// after)`.
    fn person_edit(&mut self, i: usize) -> (f64, u64) {
        let conn = self.conn(i);
        let r = self.s().read(conn).expect("read");
        let version = r.version;
        let text = r
            .text_by_root
            .into_iter()
            .next()
            .map(|x| x.1)
            .unwrap_or_default();
        let n = lines_of(&text).len();
        let target = self.shape.lines;
        let t = self.token(i);
        let k = self.rng.below(n.max(1));
        let pick = self.rng.below(10);
        let hunk = if n + 20 < target || (pick == 0 && n < target + 20) || n == 0 {
            let at = if n == 0 { 0 } else { line_span(&text, k).0 };
            Hunk {
                start: at,
                end: at,
                insert: self.shape.line(&t),
            }
        } else if n > target + 20 || pick == 1 {
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
                insert: self.shape.line(&t),
            }
        };
        let edits = vec![Edit::Text {
            root: ROOT.into(),
            hunks: vec![hunk],
        }];
        let t0 = clock();
        let res = self.s().edit(conn, edits, Some(version));
        let secs = t0.secs();
        let r = res.unwrap_or_else(|e| panic!("edit: {e:?}"));
        (secs, r.version)
    }

    /// A `run_tool` call: its seconds, or `None` when the tool refused it (a line the doc
    /// doesn't have now).
    fn tool(&mut self, i: usize, argv: &[String], stdin: &str) -> Option<f64> {
        let conn = self.conn(i);
        let t0 = clock();
        let res = self
            .s()
            .run_tool_with(conn, ROOT, argv, stdin.as_bytes(), &ToolOptions::new());
        let secs = t0.secs();
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
        let k = self.rng.range(1, n.max(1) + 1);
        let ks = k.to_string();
        let target = self.shape.lines;
        if sed {
            let script = format!("{k}s/.*/{}/", self.shape.body(&t));
            return self.tool(i, &a(&["sed", "-i", &script]), "");
        }
        let line = self.shape.line(&t);
        if n + 20 < target {
            self.tool(i, &a(&["insert", "--line", &ks]), &line)
        } else if n > target + 20 {
            let r = format!("{k},{k}");
            self.tool(i, &a(&["delete", "--lines", &r]), "")
        } else {
            let r = format!("{k},{k}");
            self.tool(i, &a(&["replace", "--lines", &r]), &line)
        }
    }

    /// An agent's `edit_with`: one line replaced through a closure on the live text.
    fn agent_edit_with(&mut self, i: usize) -> f64 {
        let conn = self.conn(i);
        let t = self.token(i);
        let body = self.shape.body(&t);
        let pick = self.rng.next();
        let t0 = clock();
        let res = self.s().edit_with(conn, ROOT, move |live| {
            let mut ls: Vec<&str> = lines_of(live);
            if !ls.is_empty() {
                let k = (pick % ls.len() as u64) as usize;
                ls[k] = body.as_str();
            }
            let mut out = String::with_capacity(live.len() + body.len());
            for l in ls {
                out.push_str(l);
                out.push('\n');
            }
            Ok::<_, Infallible>(out)
        });
        let secs = t0.secs();
        res.unwrap_or_else(|e| panic!("edit_with: {e:?}"));
        secs
    }

    fn undo(&mut self, i: usize, redo: bool) -> f64 {
        let conn = self.conn(i);
        let req = UndoRequest::steps(1);
        let t0 = clock();
        let res = if redo {
            self.s().redo_with(conn, &req)
        } else {
            self.s().undo_with(conn, &req)
        };
        let secs = t0.secs();
        // nothing to undo (an empty stack) is an answer, not a failure
        res.unwrap_or_else(|e| panic!("undo: {e:?}"));
        secs
    }

    /// One growth step, chosen by the seed (nothing timed); the host's tick every 25 calls
    /// and write-back every 250, as `crates/base/tests/perf_kit.rs`.
    fn step(&mut self, n_lines: usize) {
        self.advance();
        self.ops += 1;
        match self.rng.below(100) {
            0..=39 => {
                let i = self.person();
                self.person_edit(i);
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
                let i = self.rng.below(self.parts.len());
                self.undo(i, false);
            }
            90..=92 => {
                let i = self.rng.below(self.parts.len());
                self.undo(i, true);
            }
            _ => {
                let i = self.agent();
                self.tool(i, &["cat".to_owned()], "");
            }
        }
        if self.ops % 25 == 0 {
            self.mgr().tick().expect("tick");
        }
        if self.ops % 250 == 0 {
            let c = self.conn(0);
            self.s().flush(c).expect("flush");
        }
    }

    /// An engine doc loaded from the store as the session's is (snapshot, then the log):
    /// `(the doc, its heap bytes)`.
    fn engine_doc(&self) -> (Doc, u64) {
        let store = self.store.clone().expect("open");
        let id = store.find(&self.key).expect("find").expect("the doc");
        let schema = TextFilePack::new("markdown", LIMIT).schema();
        let h0 = alloc::live();
        let d = {
            let loaded = store.load(id).expect("load");
            Doc::from_store(schema, &loaded).expect("from_store")
        };
        let h1 = alloc::live();
        (d, h1.saturating_sub(h0))
    }
}

// ---------------------------------------------------------------- run records

fn job() -> String {
    std::env::var("K2_COMPUTE_JOB")
        .ok()
        .filter(|j| !j.is_empty())
        .map_or_else(|| "null".to_owned(), |j| format!("\"{j}\""))
}

enum F {
    U(u64),
    S(String),
}

/// The node's class, as the submitter names it (`PERF_NODE_CLASS`: a disk and CPU class,
/// never a host name): absolute figures are the node's as much as the kit's (a durable
/// commit costs 30x more on one class of build node than another).
fn node_class() -> String {
    std::env::var("PERF_NODE_CLASS")
        .ok()
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| "unnamed".to_owned())
        .replace(['"', '\\'], "_")
}

fn runrec(run: &str, fields: &[(&str, F)]) {
    let mut s = format!(
        "RUNREC {{\"job\":{},\"run\":\"{run}\",\"tool\":\"perf_harness\",\"status\":\"COMPLETE\",\"node_class\":\"{}\"",
        job(),
        node_class()
    );
    for (k, v) in fields {
        match v {
            F::U(x) => s.push_str(&format!(",\"{k}\":{x}")),
            F::S(x) => s.push_str(&format!(",\"{k}\":\"{}\"", x.replace(['"', '\\'], "_"))),
        }
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

fn ns(s: f64) -> u64 {
    (s * 1e9).round() as u64
}

// ---------------------------------------------------------------- the kit's timers

/// A sample's clock: the wall time of one call and, in the `kit-timers` build with the
/// kit's timers on, what each of the kit's internal paths took inside it (exact deltas of
/// `timers::read`, read outside the timed window).
struct Clock {
    #[cfg(feature = "kit-timers")]
    before: Option<Vec<(u64, u64)>>,
    t0: Instant,
}

#[cfg(feature = "kit-timers")]
fn read_timers() -> Vec<(u64, u64)> {
    k2_mpc_base::timers::Path::ALL
        .iter()
        .map(|p| k2_mpc_base::timers::read(*p))
        .collect()
}

std::thread_local! {
    /// The last timed call's internal paths: `(path name, ns)` for each path it entered.
    static LAST: std::cell::RefCell<Vec<(&'static str, u64)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

fn clock() -> Clock {
    Clock {
        #[cfg(feature = "kit-timers")]
        before: k2_mpc_base::timers::enabled().then(read_timers),
        t0: Instant::now(),
    }
}

impl Clock {
    /// The call's seconds; its internal paths go to [`take_last`].
    fn secs(self) -> f64 {
        let secs = self.t0.elapsed().as_secs_f64();
        #[allow(unused_mut)]
        let mut d: Vec<(&'static str, u64)> = Vec::new();
        #[cfg(feature = "kit-timers")]
        if let Some(before) = self.before {
            let after = read_timers();
            for ((p, b), a) in k2_mpc_base::timers::Path::ALL
                .iter()
                .zip(&before)
                .zip(&after)
            {
                if a.0 > b.0 {
                    d.push((p.name(), a.1.saturating_sub(b.1)));
                }
            }
        }
        LAST.with(|l| *l.borrow_mut() = d);
        secs
    }
}

fn take_last() -> Vec<(&'static str, u64)> {
    LAST.with(|l| std::mem::take(&mut *l.borrow_mut()))
}

/// A row name made at run time (`<row>.<timer>`), kept for the process.
fn intern(s: String) -> &'static str {
    std::thread_local! {
        static NAMES: std::cell::RefCell<BTreeMap<String, &'static str>> =
            const { std::cell::RefCell::new(BTreeMap::new()) };
    }
    NAMES.with(|n| {
        let mut n = n.borrow_mut();
        if let Some(x) = n.get(&s) {
            return *x;
        }
        let x: &'static str = Box::leak(s.clone().into_boxed_str());
        n.insert(s, x);
        x
    })
}

/// The rows whose durable commit the timers can take out (`<row>.no_commit`: the call
/// less its `store.txn_commit`).
const NO_COMMIT_ROWS: [&str; 8] = [
    "edit",
    "edit_with",
    "tool_replace",
    "tool_sed",
    "undo",
    "revert",
    "write_base",
    "ingest",
];

// ---------------------------------------------------------------- grow

fn grow(a: &Args) {
    let work = PathBuf::from(a.s("work"));
    let out = PathBuf::from(a.s("out"));
    let mut sizes = a.list("sizes");
    sizes.sort_unstable();
    let name = a.s_or("name", "h");
    let side = a.s_or("side", "A");
    let seed = a.u("seed", 1);
    let shape = Shape {
        lines: a.u("lines", 200) as usize,
        line_len: a.u("line-len", 0) as usize,
    };
    let window = a.u("window", 50_000).max(1);
    let mut w = World::create(&work, seed, shape);
    let start = Instant::now();
    let mut win_t = Instant::now();
    let mut win_ops = 0u64;
    let mut next_window = window;
    let mut n_lines = w.line_count();
    for &size in &sizes {
        let mut v = w.version();
        while v < size {
            w.step(n_lines);
            win_ops += 1;
            if w.ops % 16 == 0 {
                n_lines = w.line_count();
            }
            v = w.version();
            if v >= next_window {
                let wall = win_t.elapsed().as_secs_f64();
                runrec(
                    &format!("perfh_grow_{name}_{side}_w{next_window}"),
                    &[
                        ("side", F::S(side.clone())),
                        ("version", F::U(v)),
                        ("calls", F::U(win_ops)),
                        ("ns_per_call", F::U(ns(wall / win_ops.max(1) as f64))),
                        ("wall_s", F::U(start.elapsed().as_secs())),
                        ("store_bytes", F::U(file_len(&work.join("docs.redb")))),
                        ("heap_bytes", F::U(alloc::live())),
                    ],
                );
                eprintln!(
                    "[perf] {name} {side}: v{v}, {:.0} µs per call in the window, {:.0} s so far",
                    wall * 1e6 / win_ops.max(1) as f64,
                    start.elapsed().as_secs_f64()
                );
                win_ops = 0;
                next_window += window;
                win_t = Instant::now();
            }
        }
        let grow_s = start.elapsed().as_secs();
        w.close();
        let t = Instant::now();
        let dst = out.join(format!("v{size}"));
        copy_dir(&work, &dst);
        let copy_s = t.elapsed().as_secs_f64();
        let fields = [
            ("side", F::S(side.clone())),
            ("features", F::S(features().to_owned())),
            ("version", F::U(v)),
            ("calls", F::U(w.ops)),
            ("grow_s", F::U(grow_s)),
            ("copy_ms", F::U((copy_s * 1e3).round() as u64)),
            ("store_bytes", F::U(file_len(&dst.join("docs.redb")))),
            (
                "content_bytes",
                F::U(file_len(&dst.join("files").join("notes.md"))),
            ),
            ("lines", F::U(shape.lines as u64)),
            ("line_len", F::U(shape.line_len as u64)),
            ("seed", F::U(seed)),
        ];
        runrec(&format!("perfh_grow_{name}{size}_{side}"), &fields);
        eprintln!(
            "[perf] {name}{size} {side}: fixture at v{v} after {grow_s} s ({} calls)",
            w.ops
        );
        w.open();
        win_t = Instant::now();
    }
    w.close();
}

// ---------------------------------------------------------------- measure

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Fast,
    Slow,
    Cold,
}

#[derive(Default)]
struct Row {
    /// Seconds per sample, or bytes for a memory row.
    v: Vec<f64>,
    spent: f64,
    refused: u64,
    extra: BTreeMap<&'static str, u64>,
}

struct Meas {
    rows: BTreeMap<&'static str, Row>,
    samples: usize,
    fast_cap: f64,
    slow_cap: f64,
    cold_cap: f64,
    /// Mutating calls since the fixture was last copied in.
    mutating: u64,
    /// Copy the fixture back in after this many (the size stays within about 10 %).
    batch: u64,
    restores: u64,
    version_max: u64,
    /// Untimed warm-ups after an open ([`warm_up`]).
    warm_ups: u64,
}

const MEMORY_ROWS: [&str; 4] = [
    "mem_open_bytes",
    "mem_warm_bytes",
    "mem_estimate_bytes",
    "engine_doc_bytes",
];

fn class_of(row: &str) -> Class {
    match row {
        "flush" | "tick" | "ingest" | "notes" => Class::Slow,
        "open" | "join" | "first_edit" => Class::Cold,
        _ => Class::Fast,
    }
}

impl Meas {
    fn cap(&self, row: &str) -> f64 {
        match class_of(row) {
            Class::Fast => self.fast_cap,
            Class::Slow => self.slow_cap,
            Class::Cold => self.cold_cap,
        }
    }
    fn want(&self, row: &str) -> bool {
        self.rows
            .get(row)
            // refusals count too: a row its setup can never sample mustn't run forever
            .map_or(true, |r| {
                r.v.len() + (r.refused as usize) < self.samples && r.spent < self.cap(row)
            })
    }
    fn add(&mut self, row: &'static str, secs: f64) {
        let d = take_last();
        self.add_with(row, secs, d);
    }
    /// A sample, with the internal paths the call entered: each its own row
    /// (`<row>.<timer>`), and `<row>.no_commit` where the row's durable commit is known.
    fn add_with(&mut self, row: &'static str, secs: f64, paths: Vec<(&'static str, u64)>) {
        let r = self.rows.entry(row).or_default();
        r.v.push(secs);
        r.spent += secs;
        let mut commit = None;
        for (name, x) in paths {
            if name == "store.txn_commit" {
                commit = Some(commit.unwrap_or(0) + x);
            }
            let sub = intern(format!("{row}.{name}"));
            let r = self.rows.entry(sub).or_default();
            r.v.push(x as f64 / 1e9);
            r.spent += x as f64 / 1e9;
        }
        if let Some(c) = commit.filter(|_| NO_COMMIT_ROWS.contains(&row)) {
            let sub = intern(format!("{row}.no_commit"));
            let r = self.rows.entry(sub).or_default();
            let x = (secs - c as f64 / 1e9).max(0.0);
            r.v.push(x);
            r.spent += x;
        }
    }
    fn value(&mut self, row: &'static str, x: u64) {
        self.rows.entry(row).or_default().v.push(x as f64);
    }
    fn refuse(&mut self, row: &'static str) {
        self.rows.entry(row).or_default().refused += 1;
    }
    fn extra(&mut self, row: &'static str, k: &'static str, by: u64) {
        *self
            .rows
            .entry(row)
            .or_default()
            .extra
            .entry(k)
            .or_default() += by;
    }
}

struct Ctx {
    fixture: PathBuf,
    /// The in-memory engine doc for `doc_edit_as` (reloaded with the fixture).
    doc: Option<Doc>,
    doc_client: u64,
    /// The rows a warm-up calls once after each open.
    warm_fast: Vec<&'static str>,
    warm_slow: Vec<&'static str>,
    wb_gap: usize,
}

/// Copy the fixture back in (untimed) and reopen.
fn restore(w: &mut World, m: &mut Meas, c: &mut Ctx, want_doc: bool) {
    w.close();
    copy_dir(&c.fixture, &w.work);
    w.open();
    m.restores += 1;
    m.mutating = 0;
    c.doc = None;
    if want_doc {
        let (d, bytes) = w.engine_doc();
        m.value("engine_doc_bytes", bytes);
        c.doc = Some(d);
    }
    warm_up(w, m, c);
}

/// After an open, one untimed call of every row (nothing recorded): what a doc builds on
/// first need after a load (the provenance map, the revert window, ...) is the cold rows'
/// cost, never a warm row's. Without it a warm row's first sample after each copy-back
/// carries that cost (at 10k versions, 1.4 % of `edit`'s samples: its p99).
fn warm_up(w: &mut World, m: &mut Meas, c: &mut Ctx) {
    let mut scratch = Meas {
        rows: BTreeMap::new(),
        samples: 1,
        fast_cap: f64::MAX,
        slow_cap: f64::MAX,
        cold_cap: f64::MAX,
        mutating: 0,
        batch: u64::MAX,
        restores: 0,
        version_max: 0,
        warm_ups: 0,
    };
    let (fast, slow, gap) = (c.warm_fast.clone(), c.warm_slow.clone(), c.wb_gap);
    fast_round(w, &mut scratch, c, &fast, gap);
    slow_round(w, &mut scratch, &slow);
    // each participant's own first calls too: a row picks one at random, so the others would
    // pay theirs in a timed sample
    if !fast.contains(&"edit_nosync") {
        for i in 0..w.parts.len() {
            let n = w.line_count();
            if w.parts[i].kind == ParticipantKind::Person {
                w.person_edit(i);
            } else {
                w.agent_edit_with(i);
                w.agent_tool(i, false, n);
                w.agent_tool(i, true, n);
                w.tool(i, &["cat".to_owned()], "");
            }
            let (_, v) = w.person_edit(0);
            let _ = w.s().revert(w.conn(0), v, false);
            w.undo(i, false);
            scratch.mutating += 6;
        }
    }
    let _ = take_last();
    m.mutating += scratch.mutating;
    m.warm_ups += 1;
}

/// One cold sample: release, reopen (timed), each join (timed), the first edit after it
/// (timed), and the heap they hold.
fn cold(w: &mut World, m: &mut Meas) {
    w.close();
    let h0 = alloc::live();
    let (open_s, joins) = w.open();
    let open_paths = std::mem::take(&mut w.last_open);
    let h_open = alloc::live();
    let i = w.person();
    let (first, _) = w.person_edit(i);
    let first_paths = take_last();
    m.mutating += 1;
    let h_warm = alloc::live();
    m.add_with("open", open_s, open_paths);
    for (j, d) in joins {
        m.add_with("join", j, d);
    }
    m.add_with("first_edit", first, first_paths);
    m.value("mem_open_bytes", h_open.saturating_sub(h0));
    m.value("mem_warm_bytes", h_warm.saturating_sub(h0));
    let est = w.s().status().expect("status").estimated_bytes;
    m.value("mem_estimate_bytes", est);
}

fn note_version(w: &World, m: &mut Meas) {
    let v = w.version();
    m.version_max = m.version_max.max(v);
}

/// The fast rows of the history fixtures, one round: each row once (while it wants
/// samples), timed alone.
fn fast_round(w: &mut World, m: &mut Meas, c: &mut Ctx, rows: &[&'static str], wb_gap: usize) {
    w.advance();
    // a seeded order per round: a kit's periodic cost (a sampled check every N calls) can't always
    // land on one row, and a copy-back's next call isn't always the same row's
    let mut order: Vec<&'static str> = rows.to_vec();
    for i in (1..order.len()).rev() {
        let j = w.rng.below(i + 1);
        order.swap(i, j);
    }
    for &row in &order {
        if !m.want(row) {
            continue;
        }
        match row {
            "edit" => {
                let p = w.person();
                let (s, _) = w.person_edit(p);
                m.add(row, s);
                m.mutating += 1;
            }
            "edit_nosync" => {
                // the same call as `edit`, in the build whose store skips the sync
                let p = w.person();
                let (s, _) = w.person_edit(p);
                m.add(row, s);
                m.mutating += 1;
            }
            "edit_with" => {
                let a = w.agent();
                let s = w.agent_edit_with(a);
                m.add(row, s);
                m.mutating += 1;
            }
            "tool_replace" | "tool_sed" => {
                let n = w.line_count();
                let a = w.agent();
                match w.agent_tool(a, row == "tool_sed", n) {
                    Some(s) => m.add(row, s),
                    None => m.refuse(row),
                }
                m.mutating += 1;
            }
            "doc_edit_as" => {
                let Some(d) = c.doc.as_mut() else { continue };
                let text = d.text(ROOT).expect("doc text");
                let n = lines_of(&text).len();
                let k = w.rng.below(n.max(1));
                let t = format!("e:{}", w.rng.next() % 1_000_000);
                let (s, e) = if n == 0 { (0, 0) } else { line_span(&text, k) };
                let edits = [Edit::Text {
                    root: ROOT.into(),
                    hunks: vec![Hunk {
                        start: s,
                        end: e,
                        insert: w.shape.line(&t),
                    }],
                }];
                let t0 = clock();
                let r = d.edit_as(c.doc_client, &edits);
                let secs = t0.secs();
                r.unwrap_or_else(|e| panic!("edit_as: {e:?}"));
                m.add(row, secs);
            }
            "undo" => {
                let q = w.person();
                w.person_edit(q);
                let s = w.undo(q, false);
                m.add(row, s);
                m.mutating += 2;
            }
            "revert" => {
                // revert by root (U22) of the participant's own change just made
                let q = w.person();
                let (_, v) = w.person_edit(q);
                let conn = w.conn(q);
                let t0 = clock();
                let r = w.s().revert(conn, v, false);
                let secs = t0.secs();
                match r {
                    Ok(_) => m.add(row, secs),
                    Err(e) => {
                        if m.rows.get(row).map_or(0, |r| r.refused) == 0 {
                            eprintln!("[perf] revert refused (counted): {e:?}");
                        }
                        m.refuse(row);
                    }
                }
                m.mutating += 2;
            }
            "blame_line" | "blame_doc" => {
                let n = w.line_count();
                let conn = w.conn(0);
                let lines = (row == "blame_line").then(|| {
                    let k = w.rng.range(1, n.max(1) + 1);
                    LineRange {
                        root: ROOT.into(),
                        first: k,
                        last: k,
                    }
                });
                let t0 = clock();
                let r = w.s().blame(conn, ROOT, lines);
                let secs = t0.secs();
                r.unwrap_or_else(|e| panic!("blame: {e:?}"));
                m.add(row, secs);
            }
            "cat" => {
                let a = w.agent();
                let s = w.tool(a, &["cat".to_owned()], "").expect("a read");
                m.add(row, s);
            }
            "events_500" => {
                let v = w.version();
                if v <= 500 {
                    m.refuse(row);
                    continue;
                }
                let conn = w.conn(0);
                let t0 = clock();
                let r = w.s().events_since(conn, v - 500);
                let secs = t0.secs();
                match r {
                    Ok(ev) => {
                        m.add(row, secs);
                        m.extra(row, "events", ev.len() as u64);
                    }
                    Err(_) => m.refuse(row),
                }
            }
            "store_probe" => {
                let store = w.store.clone().expect("open");
                let key = format!("probe-{}", w.rng.below(8));
                let t0 = clock();
                store
                    .host_put("perf-harness", &key, b"probe")
                    .expect("host_put");
                m.add(row, t0.secs());
            }
            "write_base" => write_base_sample(w, m, wb_gap),
            "join_warm" => {
                // a participant joining the loaded doc (then leaving, untimed)
                let n = w.rng.next() % 1_000_000_000;
                let who = Participant::new(
                    format!("w{n}"),
                    format!("W{n}"),
                    "#123456".to_owned(),
                    ParticipantKind::Agent,
                    "cli",
                );
                let t0 = clock();
                let j = w.s().join(who, Stay::Held).expect("join");
                let secs = t0.secs();
                m.add(row, secs);
                w.s().leave(j.conn).expect("leave");
            }
            _ => die(&format!("no fast row {row}")),
        }
    }
    note_version(w, m);
}

/// `write --base`: an agent reads, `gap` others' edits land, then the agent writes its
/// read text with one line changed (a line the edits since left alone where one is found).
fn write_base_sample(w: &mut World, m: &mut Meas, gap: usize) {
    const ROW: &str = "write_base";
    let a = w.agent();
    let conn = w.conn(a);
    let r = w.s().read(conn).expect("read");
    let base = r.version;
    let base_text = r
        .text_by_root
        .into_iter()
        .next()
        .map(|x| x.1)
        .unwrap_or_default();
    for _ in 0..gap {
        let p = w.person();
        w.person_edit(p);
    }
    let live = w.text();
    let base_lines = lines_of(&base_text);
    let live_lines = lines_of(&live);
    let mut pick = None;
    for _ in 0..20 {
        let j = w.rng.below(base_lines.len().max(1));
        if j >= base_lines.len() {
            break;
        }
        let l: &str = base_lines[j];
        let once = |ls: &[&str]| ls.iter().filter(|x| **x == l).count() == 1;
        if once(&base_lines) && once(&live_lines) {
            pick = Some(j);
            break;
        }
    }
    let j = pick.unwrap_or_else(|| w.rng.below(base_lines.len().max(1)));
    let t = w.token(a);
    let mut ours = String::with_capacity(base_text.len() + 64);
    for (k, l) in base_lines.iter().enumerate() {
        if k == j {
            ours.push_str(&w.shape.line(&t));
        } else {
            ours.push_str(l);
            ours.push('\n');
        }
    }
    let t0 = clock();
    let res = w.s().write_base(conn, ROOT, base, &ours);
    let secs = t0.secs();
    m.mutating += gap as u64 + 1;
    match res {
        Ok(WriteOutcome::Applied(_)) => {
            m.add(ROW, secs);
            m.extra(ROW, "applied", 1);
        }
        Ok(WriteOutcome::Conflicts(_)) => {
            m.add(ROW, secs);
            m.extra(ROW, "conflicts", 1);
        }
        Ok(WriteOutcome::BaseExpired) => {
            m.refuse(ROW);
            m.extra(ROW, "base_expired", 1);
        }
        Err(e) => panic!("write_base: {e:?}"),
    }
    m.extra(ROW, "changes_since", gap as u64);
}

/// The slow rows, one round: a write-back after an edit, the tick a minute of host time
/// later, an outside save and its ingest, and the note it leaves its line's author.
fn slow_round(w: &mut World, m: &mut Meas, rows: &[&'static str]) {
    w.advance();
    for &row in rows {
        match row {
            "flush" if m.want(row) => {
                let p = w.person();
                w.person_edit(p);
                let conn = w.conn(0);
                let t0 = clock();
                w.s().flush(conn).expect("flush");
                m.add(row, t0.secs());
                m.mutating += 1;
            }
            "tick" if m.want(row) => {
                w.host.now.fetch_add(61_000, Ordering::SeqCst);
                let t0 = clock();
                w.mgr().tick().expect("tick");
                m.add(row, t0.secs());
            }
            "ingest" if m.want(row) => {
                let conn = w.conn(0);
                w.s().flush(conn).expect("flush");
                let text = std::fs::read_to_string(&w.file).expect("file");
                let mut ls: Vec<String> = lines_of(&text).iter().map(|x| (*x).to_owned()).collect();
                if ls.is_empty() {
                    m.refuse(row);
                    continue;
                }
                let k = w.rng.below(ls.len());
                let author = ls[k].split(':').next().unwrap_or("").to_owned();
                let n = w.rng.next() % 1_000_000;
                ls[k] = w.shape.body(&format!("ext:{n}"));
                let mut out = ls.join("\n");
                out.push('\n');
                std::fs::write(&w.file, out).expect("outside save");
                let t0 = clock();
                let r = w.s().file_changed();
                let secs = t0.secs();
                match r {
                    Ok(DiskCheck::Ingested { .. }) => m.add(row, secs),
                    Ok(_) => m.refuse(row),
                    Err(e) => panic!("file_changed: {e:?}"),
                }
                m.mutating += 1;
                // the note an outside save leaves the author of the line it replaced
                if m.want("notes") {
                    if let Some(i) = w.parts.iter().position(|p| p.pid == author) {
                        w.host.now.fetch_add(6_000, Ordering::SeqCst);
                        let conn = w.conn(i);
                        let t0 = clock();
                        let r = w.s().take_notes(conn);
                        let secs = t0.secs();
                        let notes = r.unwrap_or_else(|e| panic!("take_notes: {e:?}"));
                        m.add("notes", secs);
                        m.extra("notes", "delivered", notes.notes.len() as u64);
                        m.extra("notes", "held", notes.held as u64);
                    }
                }
            }
            _ => {}
        }
    }
    note_version(w, m);
}

/// The live runs of the text and of its provenance roots (the `nosync` build:
/// `Session::char_blame`): runs of characters with consecutive engine IDs.
#[cfg(feature = "nosync")]
fn runs(w: &World) -> (u64, u64, u64) {
    let cb = w.s().char_blame(ROOT).expect("char_blame");
    let (mut live, mut prov) = (0u64, 0u64);
    // clocks as u64 whatever width this kit's CharBlame gives them
    let mut prev: Option<(u64, u64, u64, u64, u64)> = None;
    for c in &cb {
        let len = c.ch.len_utf16() as u64;
        #[allow(clippy::unnecessary_cast)]
        let (client, clock, rclient, rclock) = (
            c.client as u64,
            c.clock as u64,
            c.root_client as u64,
            c.root_clock as u64,
        );
        match prev {
            Some((pc, pk, rc, rk, pl)) => {
                if !(client == pc && clock == pk + pl) {
                    live += 1;
                }
                if !(rclient == rc && rclock == rk + pl) {
                    prov += 1;
                }
            }
            None => {
                live += 1;
                prov += 1;
            }
        }
        prev = Some((client, clock, rclient, rclock, len));
    }
    (live, prov, cb.len() as u64)
}

fn measure(a: &Args) {
    let fixture = PathBuf::from(a.s("fixture"));
    let work = PathBuf::from(a.s("work"));
    let name = a.s("name");
    let size = a.u("size", 0);
    let side = a.s_or("side", "A");
    let run = a.u("run", 1);
    let build = a.s_or("build", "unknown");
    let rows_kind = a.s("rows");
    let tsv = PathBuf::from(a.s("tsv"));
    let seed = a.u("seed", 1);
    let wb_gap = a.u("wb-gap", 3) as usize;
    let max_s = a.f("max-s", 3600.0);
    let nosync = rows_kind == "nosync";
    let timers = timers_switch(&a.s_or("timers", "off"));
    if nosync != cfg!(feature = "nosync") {
        die("the nosync rows run only in the build with the `nosync` feature, and that build runs nothing else");
    }
    let mut m = Meas {
        rows: BTreeMap::new(),
        samples: a.u("samples", 10_000) as usize,
        fast_cap: a.f("fast-cap-s", 900.0),
        slow_cap: a.f("slow-cap-s", 60.0),
        cold_cap: a.f("cold-cap-s", 60.0),
        mutating: 0,
        batch: a.u("batch", (size / 10).max(1_000)),
        restores: 0,
        version_max: 0,
        warm_ups: 0,
    };
    let start = Instant::now();
    copy_dir(&fixture, &work);
    let mut w = World::attach(&work, seed.wrapping_mul(0x1000_0000_01B3) ^ run);
    w.unsynced = nosync;
    let content_bytes = file_len(&w.file);
    let store_bytes = file_len(&work.join("docs.redb"));
    let mut c = Ctx {
        fixture: fixture.clone(),
        doc: None,
        doc_client: 0x0012_3456_789a,
        warm_fast: Vec::new(),
        warm_slow: Vec::new(),
        wb_gap,
    };
    let (fast, slow, cold_rows): (Vec<&'static str>, Vec<&'static str>, bool) =
        match rows_kind.as_str() {
            "history" => (
                vec![
                    "edit",
                    "edit_with",
                    "tool_replace",
                    "tool_sed",
                    "doc_edit_as",
                    "undo",
                    "revert",
                    "blame_line",
                    "blame_doc",
                    "cat",
                    "events_500",
                    "store_probe",
                    "write_base",
                    "join_warm",
                ],
                vec!["flush", "tick", "ingest"],
                true,
            ),
            "content" => (
                vec!["edit", "doc_edit_as", "blame_doc"],
                vec!["flush"],
                true,
            ),
            "nosync" => (vec!["edit_nosync"], vec![], false),
            k => die(&format!("--rows {k}: history, content or nosync")),
        };
    // the first open of the fixture (untimed: the cold rows come last), then a warm-up
    w.open();
    let version_start = w.version();
    m.version_max = version_start;
    if fast.contains(&"doc_edit_as") {
        let (d, bytes) = w.engine_doc();
        m.value("engine_doc_bytes", bytes);
        c.doc = Some(d);
    }
    #[cfg(feature = "nosync")]
    {
        let (live, prov, chars) = runs(&w);
        m.value("live_runs", live);
        m.value("prov_runs", prov);
        m.value("chars", chars);
        // the fixture's data beside its file length (`store_bytes`): same-seed files differ in
        // free space only (tools/README.md, Performance harness, known issues)
        let st = w
            .store
            .clone()
            .expect("open")
            .redb_stats_for_tests()
            .expect("stats");
        let get = |k: &str| st.iter().find(|(n, _)| *n == k).map_or(0, |(_, v)| *v);
        m.value("store_data_bytes", get("redb_stored_bytes"));
        m.value("store_free_bytes", get("redb_fragmented_bytes"));
    }
    c.warm_fast = fast.clone();
    c.warm_slow = slow.clone();
    warm_up(&mut w, &mut m, &mut c);
    let want_doc = fast.contains(&"doc_edit_as");
    let over = || start.elapsed().as_secs_f64() > max_s;
    // 1. the fast rows, interleaved
    while fast.iter().any(|r| m.want(r)) && !over() {
        fast_round(&mut w, &mut m, &mut c, &fast, wb_gap);
        if m.mutating >= m.batch {
            restore(&mut w, &mut m, &mut c, want_doc);
        }
    }
    // 2. the slow rows, interleaved
    while slow
        .iter()
        .chain(std::iter::once(&"notes").filter(|_| slow.contains(&"ingest")))
        .any(|r| m.want(r))
        && !over()
    {
        slow_round(&mut w, &mut m, &slow);
        if m.mutating >= m.batch {
            restore(&mut w, &mut m, &mut c, want_doc);
        }
        // a row whose setup can't give it samples (no note left) mustn't hold the phase
        if !slow.iter().any(|r| m.want(r)) {
            break;
        }
    }
    // 3. cold: reopen, join, first edit, memory
    if cold_rows {
        while m.want("open") && !over() {
            cold(&mut w, &mut m);
            if m.mutating >= m.batch {
                restore(&mut w, &mut m, &mut c, want_doc);
            }
        }
    }
    note_version(&w, &mut m);
    let wall_s = start.elapsed().as_secs();
    let capped_run = over();
    w.close();
    // the records
    let mut tsv_out = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&tsv)
        .expect("tsv");
    runrec(
        &format!("perfh_{name}_run_{side}{run}"),
        &[
            ("side", F::S(side.clone())),
            ("run_index", F::U(run)),
            ("build", F::S(build.clone())),
            ("features", F::S(features().to_owned())),
            ("rows", F::S(rows_kind.clone())),
            ("size", F::U(size)),
            ("version_start", F::U(version_start)),
            ("version_max", F::U(m.version_max)),
            ("restores", F::U(m.restores)),
            ("warm_ups", F::U(m.warm_ups)),
            ("batch", F::U(m.batch)),
            ("content_bytes", F::U(content_bytes)),
            ("store_bytes", F::U(store_bytes)),
            ("wall_s", F::U(wall_s)),
            ("max_s_hit", F::U(capped_run as u64)),
            ("timers", F::S(timers.to_owned())),
            ("kit_hooks", F::S(kit_hooks().to_owned())),
        ],
    );
    for (row, r) in &m.rows {
        let mut v = r.v.clone();
        v.sort_by(f64::total_cmp);
        let n = v.len();
        let mean = v.iter().sum::<f64>() / n.max(1) as f64;
        let bytes = MEMORY_ROWS.contains(row)
            || matches!(
                *row,
                "live_runs" | "prov_runs" | "chars" | "store_data_bytes" | "store_free_bytes"
            );
        let conv = |x: f64| if bytes { x.round() as u64 } else { ns(x) };
        let (p50, p95, p99, max) = (
            conv(quantile(&v, 0.5)),
            conv(quantile(&v, 0.95)),
            conv(quantile(&v, 0.99)),
            conv(v.last().copied().unwrap_or(0.0)),
        );
        let unit = if bytes { "count" } else { "ns" };
        let capped = !bytes && n < m.samples;
        let mut fields = vec![
            ("side", F::S(side.clone())),
            ("run_index", F::U(run)),
            ("build", F::S(build.clone())),
            ("features", F::S(features().to_owned())),
            ("unit", F::S(unit.to_owned())),
            ("samples", F::U(n as u64)),
            ("p50", F::U(p50)),
            ("p95", F::U(p95)),
            ("p99", F::U(p99)),
            ("mean", F::U(conv(mean))),
            ("max", F::U(max)),
            ("p99_indicative", F::U((!bytes && n < P99_FULL) as u64)),
            ("capped", F::U(capped as u64)),
            ("refused", F::U(r.refused)),
            ("version_start", F::U(version_start)),
            ("version_max", F::U(m.version_max)),
            ("content_bytes", F::U(content_bytes)),
            ("store_bytes", F::U(store_bytes)),
        ];
        for (k, x) in &r.extra {
            fields.push((*k, F::U(*x)));
        }
        runrec(&format!("perfh_{name}_{row}_{side}{run}"), &fields);
        writeln!(
            tsv_out,
            "{name}\t{row}\t{side}\t{run}\t{n}\t{p50}\t{p95}\t{p99}\t{}\t{unit}\t{build}\t{}",
            conv(mean),
            features()
        )
        .expect("tsv write");
        if bytes {
            eprintln!("[perf] {name} {side}{run} {row}: median {p50} (n={n})");
        } else {
            let note = if n < P99_FULL {
                format!(" p99 indicative (n={n})")
            } else {
                String::new()
            };
            eprintln!(
                "[perf] {name} {side}{run} {row}: p50 {:.1} µs, p95 {:.1} µs, p99 {:.1} µs (n={n}, refused {}){note}",
                p50 as f64 / 1e3,
                p95 as f64 / 1e3,
                p99 as f64 / 1e3,
                r.refused
            );
        }
    }
}

// ---------------------------------------------------------------- summarize

fn median(v: &mut [u64]) -> u64 {
    v.sort_unstable();
    let n = v.len();
    if n == 0 {
        0
    } else if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2] + 1) / 2
    }
}

#[derive(Default)]
struct Side {
    p50: Vec<u64>,
    p95: Vec<u64>,
    p99: Vec<u64>,
    n: Vec<u64>,
    builds: Vec<String>,
    features: Vec<String>,
}

/// The size a fixture name ends with (`h100000` → 100000).
fn size_of(fixture: &str) -> Option<u64> {
    let d: String = fixture
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .collect();
    d.parse().ok()
}

fn summarize(a: &Args) {
    let text = std::fs::read_to_string(a.s("tsv")).expect("tsv");
    // (fixture, row) -> side -> runs
    let mut by: BTreeMap<(String, String), BTreeMap<String, Side>> = BTreeMap::new();
    let mut units: BTreeMap<String, String> = BTreeMap::new();
    for l in text.lines() {
        let f: Vec<&str> = l.split('\t').collect();
        if f.len() < 12 {
            continue;
        }
        let num = |i: usize| f[i].parse::<u64>().unwrap_or(0);
        let s = by
            .entry((f[0].to_owned(), f[1].to_owned()))
            .or_default()
            .entry(f[2].to_owned())
            .or_default();
        s.n.push(num(4));
        s.p50.push(num(5));
        s.p95.push(num(6));
        s.p99.push(num(7));
        s.builds.push(f[10].to_owned());
        s.features.push(f[11].to_owned());
        units.insert(f[1].to_owned(), f[9].to_owned());
    }
    // per row and side: (fixture size, median p50, median p99) for the growth
    let mut growth: BTreeMap<(String, String, String), Vec<(u64, u64, u64)>> = BTreeMap::new();
    println!("node class: {}", node_class());
    println!("| fixture | row | side | runs | p50 | p95 | p99 | n (min) | build | features |");
    println!("|---|---|---|---|---|---|---|---|---|---|");
    for ((fixture, row), sides) in &mut by {
        let unit = units.get(row).cloned().unwrap_or_default();
        let mut med = BTreeMap::new();
        for (side, s) in sides.iter_mut() {
            let (p50, p95, p99) = (median(&mut s.p50), median(&mut s.p95), median(&mut s.p99));
            let n_min = s.n.iter().copied().min().unwrap_or(0);
            let mut b = s.builds.clone();
            b.dedup();
            let mut fe = s.features.clone();
            fe.dedup();
            let ind = unit == "ns" && (n_min as usize) < P99_FULL;
            let p99_s = if ind {
                format!("{p99} indicative (n={n_min})")
            } else {
                p99.to_string()
            };
            println!(
                "| {fixture} | {row} | {side} | {} | {p50} | {p95} | {p99_s} | {n_min} | {} | {} |",
                s.n.len(),
                b.join(","),
                fe.join(",")
            );
            med.insert(side.clone(), (p50, p95, p99, n_min, s.n.len()));
            let key = (
                fixture
                    .chars()
                    .take_while(|c| !c.is_ascii_digit())
                    .collect::<String>(),
                row.clone(),
                side.clone(),
            );
            if let Some(sz) = size_of(fixture) {
                growth.entry(key).or_default().push((sz, p50, p99));
            }
            let mut fields = vec![
                ("side", F::S(side.clone())),
                ("unit", F::S(unit.clone())),
                ("runs", F::U(s.n.len() as u64)),
                ("p50", F::U(p50)),
                ("p95", F::U(p95)),
                ("p99", F::U(p99)),
                ("n_min", F::U(n_min)),
                ("p99_indicative", F::U(ind as u64)),
                ("build", F::S(b.join(","))),
                ("features", F::S(fe.join(","))),
            ];
            if unit == "ns" && s.n.len() < 5 {
                fields.push(("under_5_runs", F::U(1)));
            }
            runrec(&format!("perfh_med_{fixture}_{row}_{side}"), &fields);
        }
        // the hooked build (side H, label-only) against the plain one on the same fixture
        if let (Some(a_), Some(h_)) = (med.get("A"), med.get("H")) {
            let r = |b: u64, a: u64| if a == 0 { 0 } else { (b * 1000 + a / 2) / a };
            runrec(
                &format!("perfh_hookgap_{fixture}_{row}"),
                &[
                    ("unit", F::S(unit.clone())),
                    ("plain_p50", F::U(a_.0)),
                    ("hooked_p50", F::U(h_.0)),
                    ("plain_p99", F::U(a_.2)),
                    ("hooked_p99", F::U(h_.2)),
                    ("hooked_over_plain_p50_milli", F::U(r(h_.0, a_.0))),
                    ("hooked_over_plain_p99_milli", F::U(r(h_.2, a_.2))),
                    ("n_min", F::U(a_.3.min(h_.3))),
                ],
            );
            eprintln!(
                "[perf] {fixture} {row}: hooked/plain p50 {:.3}×, p99 {:.3}×",
                r(h_.0, a_.0) as f64 / 1000.0,
                r(h_.2, a_.2) as f64 / 1000.0
            );
        }
        if let (Some(a_), Some(b_)) = (med.get("A"), med.get("B")) {
            let r = |b: u64, a: u64| if a == 0 { 0 } else { (b * 1000 + a / 2) / a };
            runrec(
                &format!("perfh_ratio_{fixture}_{row}"),
                &[
                    ("unit", F::S(unit.clone())),
                    ("ratio_p50_milli", F::U(r(b_.0, a_.0))),
                    ("ratio_p95_milli", F::U(r(b_.1, a_.1))),
                    ("ratio_p99_milli", F::U(r(b_.2, a_.2))),
                    ("n_min", F::U(a_.3.min(b_.3))),
                    (
                        "p99_indicative",
                        F::U((unit == "ns" && (a_.3.min(b_.3) as usize) < P99_FULL) as u64),
                    ),
                    ("runs_a", F::U(a_.4 as u64)),
                    ("runs_b", F::U(b_.4 as u64)),
                ],
            );
            eprintln!(
                "[perf] {fixture} {row}: B/A p50 {:.3}×, p99 {:.3}×{}",
                r(b_.0, a_.0) as f64 / 1000.0,
                r(b_.2, a_.2) as f64 / 1000.0,
                if row == "edit_nosync" {
                    " (the C19 ratio row)"
                } else {
                    ""
                }
            );
        }
    }
    for ((prefix, row, side), mut v) in growth {
        if v.len() < 2 {
            continue;
        }
        v.sort_unstable();
        let (lo, hi) = (v[0], v[v.len() - 1]);
        let r = |b: u64, a: u64| if a == 0 { 0 } else { (b * 1000 + a / 2) / a };
        runrec(
            &format!("perfh_growth_{prefix}_{row}_{side}"),
            &[
                ("from_size", F::U(lo.0)),
                ("to_size", F::U(hi.0)),
                ("growth_p50_milli", F::U(r(hi.1, lo.1))),
                ("growth_p99_milli", F::U(r(hi.2, lo.2))),
            ],
        );
    }
    // the slots with no Yrs path: named, never zeros
    for (slot, row) in [("fork(V)", "E-PERF-35"), ("changed_since(V)", "E-PERF-34")] {
        println!("{row} {slot}: no Yrs path (an empty slot until the native engine)");
    }
}

/// `store-stats --store FILE --name NAME` (the `nosync` build): redb's statistics of a store
/// file, to see what of a fixture's size is data and what is the file's allocation.
#[cfg(feature = "nosync")]
fn store_stats(a: &Args) {
    let path = PathBuf::from(a.s("store"));
    let name = a.s("name");
    let file = file_len(&path);
    let store = DocStore::open(&path).expect("store");
    let mut fields = vec![("file_bytes", F::U(file))];
    for (k, v) in store.redb_stats_for_tests().expect("stats") {
        fields.push((k, F::U(v)));
    }
    runrec(&format!("perfh_store_{name}"), &fields);
}

#[cfg(not(feature = "nosync"))]
fn store_stats(_a: &Args) {
    die("store-stats needs the nosync build");
}

/// `timers-check --work DIR`: in this build (no kit features), switch the kit's timers on,
/// make each measured call once on a small doc, and print each timed path's count
/// (`perfh_timers_check`). Fails when no path counted anything, or when the build has
/// the kit's test hooks.
#[cfg(not(feature = "kit-timers"))]
fn timers_check(_a: &Args) {
    die("timers-check needs the kit-timers build");
}

#[cfg(feature = "kit-timers")]
fn timers_check(a: &Args) {
    {
        use k2_mpc_base::timers::{self, Path};
        let work = PathBuf::from(a.s("work"));
        let shape = Shape {
            lines: 200,
            line_len: 0,
        };
        let mut w = World::create(&work, 7, shape);
        // enough history for events_since(500)
        let mut n = w.line_count();
        while w.version() < 700 {
            w.step(n);
            if w.ops % 16 == 0 {
                n = w.line_count();
            }
        }
        timers::reset();
        timers::set_enabled(true);
        let mut m = Meas {
            rows: BTreeMap::new(),
            samples: 1,
            fast_cap: 60.0,
            slow_cap: 60.0,
            cold_cap: 60.0,
            mutating: 0,
            batch: u64::MAX,
            restores: 0,
            version_max: 0,
            warm_ups: 0,
        };
        let (d, _) = w.engine_doc();
        let mut c = Ctx {
            fixture: work.clone(),
            doc: Some(d),
            doc_client: 0x0012_3456_789a,
            warm_fast: Vec::new(),
            warm_slow: Vec::new(),
            wb_gap: 3,
        };
        let fast = [
            "edit",
            "edit_with",
            "tool_replace",
            "tool_sed",
            "doc_edit_as",
            "undo",
            "revert",
            "blame_line",
            "blame_doc",
            "cat",
            "events_500",
            "write_base",
            "join_warm",
        ];
        fast_round(&mut w, &mut m, &mut c, &fast, 3);
        slow_round(&mut w, &mut m, &["flush", "tick", "ingest"]);
        cold(&mut w, &mut m);
        w.close();
        let mut counted = 0;
        let mut zero = Vec::new();
        for p in Path::ALL {
            let (count, total) = timers::read(*p);
            runrec(
                &format!("perfh_timers_check_{}", p.name()),
                &[
                    ("calls", F::U(count)),
                    ("total_ns", F::U(total)),
                    ("kit_hooks", F::S(kit_hooks().to_owned())),
                ],
            );
            if count > 0 {
                counted += 1;
            } else {
                zero.push(p.name());
            }
        }
        eprintln!(
            "[perf] timers-check: {counted} of {} paths counted; none on: {}",
            Path::ALL.len(),
            zero.join(", ")
        );
        if counted == 0 || timers::HOOKS {
            die("timers-check failed: no path counted, or the kit's test hooks are in this build");
        }
    }
}

fn main() {
    let a = Args::parse();
    match a.cmd.as_str() {
        "grow" => {
            if cfg!(feature = "nosync") {
                die("grow runs in the plain build only");
            }
            grow(&a)
        }
        "measure" => measure(&a),
        "summarize" => summarize(&a),
        "timers-check" => timers_check(&a),
        "store-stats" => store_stats(&a),
        _ => die("usage: k2-mpc-perf grow|measure|summarize --name value ... (tools/README.md: Performance harness)"),
    }
}
