//! What a blame costs as a doc's history grows (v0.9.5).
//!
//! `blame_cost_at_sizes` (ignored; minutes in release: run it on a node) builds a doc's
//! history through a session, **1.2k, 40k and 300k changes** (`BLAME_COST_SIZES`,
//! comma-separated), and times `blame`. Public API only, so the same file measures a release
//! before this change and after it, on the same node.
//!
//! **The history.** A 300-line file, four agents taking turns: a new line in at the top, the
//! last line out (two changes). The text stays about 300 lines; the engine's state grows with
//! every change (each deleted line stays as a tombstone). The churn edits the doc's two ends,
//! so the engine's own position lookup (which walks from the start, tombstones included)
//! stays short and 300k changes build in minutes; blame's cost doesn't depend on where the
//! tombstones are.
//!
//! **What is timed** (each blame alone, `Instant`, release; the session's other work is not):
//! - `cold`: the first blame after the store is reopened: the provenance map is built from
//!   scratch (a throwaway replica of the doc), the same before and after;
//! - `steady`: blame of every line after a participant's edit (the map kept up to date);
//! - `range`: `blame -L 100,109` after an edit;
//! - `outside`: blame of every line after an outside edit (the file changed on disk). Until
//!   v0.9.5 an outside edit dropped the map, so this blame rebuilt it;
//! - `check` (v0.9.5 only, `blame_check_cost`): a blame with the runtime check sampled (one
//!   in `Config::blame_check_every`), which rebuilds and compares.
//!
//! ```sh
//! cargo test -p k2-mpc-base --release --test blame_cost -- --ignored --nocapture --test-threads 1
//! ```

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use k2_mpc_base::session::{
    AuditEvent, Config, ConnId, Host, LineRange, Participant, ParticipantKind, Rights, Session,
    SessionManager, Stay, TextFilePack,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::text::diff::Hunk;
use k2_mpc_base::Edit;

const MD: TextFilePack = TextFilePack::new("markdown", 64 << 20);
const ROOT: &str = TextFilePack::ROOT;
const AGENTS: usize = 4;

struct H(AtomicU64);
impl Host for H {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        self.0.fetch_add(1, Ordering::Relaxed)
    }
    fn on_event(&self, _: &AuditEvent) {}
}

struct Fx {
    dir: PathBuf,
    ws: PathBuf,
    file: PathBuf,
    key: DocKey,
    host: Arc<H>,
    config: Config,
    mgr: SessionManager,
    s: Arc<Session>,
    agents: Vec<ConnId>,
    obs: ConnId,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn open(
    fx_dir: &std::path::Path,
    ws: &std::path::Path,
    file: &std::path::Path,
    key: &DocKey,
    host: &Arc<H>,
    config: &Config,
) -> (SessionManager, Arc<Session>, Vec<ConnId>, ConnId) {
    let store = Arc::new(DocStore::open(fx_dir.join("docs.redb")).unwrap());
    let mgr = SessionManager::new(store, host.clone(), config.clone());
    let s = mgr.open_file(Arc::new(MD), key, file, ws).unwrap();
    let agents = (0..AGENTS)
        .map(|k| {
            let p = Participant::new(
                format!("agent{k}"),
                format!("Agent {k}"),
                "#123456",
                ParticipantKind::Agent,
                "cli",
            );
            s.join(p, Stay::Held).unwrap().conn
        })
        .collect();
    let obs = s
        .join(
            Participant::new("obs", "Obs", "#654321", ParticipantKind::Person, "tab"),
            Stay::Held,
        )
        .unwrap()
        .conn;
    (mgr, s, agents, obs)
}

impl Fx {
    fn new(tag: &str, config: Config) -> Self {
        let dir =
            std::env::temp_dir().join(format!("k2mb-blame-cost-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let ws = dir.join("ws");
        fs::create_dir_all(&ws).unwrap();
        let ws = fs::canonicalize(ws).unwrap();
        let file = ws.join("cost.md");
        let seed: String = (0..300).map(|i| format!("seed line {i}\n")).collect();
        fs::write(&file, seed).unwrap();
        let key = DocKey {
            space: "ws".into(),
            path: "cost.md".into(),
        };
        let host = Arc::new(H(AtomicU64::new(1_793_111_400_000)));
        let (mgr, s, agents, obs) = open(&dir, &ws, &file, &key, &host, &config);
        Fx {
            dir,
            ws,
            file,
            key,
            host,
            config,
            mgr,
            s,
            agents,
            obs,
        }
    }

    fn text(&self) -> String {
        self.s.read(self.obs).unwrap().text_by_root[0].1.clone()
    }

    fn hunk(&self, k: usize, start: usize, end: usize, insert: String) {
        self.s
            .edit(
                self.agents[k % AGENTS],
                vec![Edit::Text {
                    root: ROOT.into(),
                    hunks: vec![Hunk { start, end, insert }],
                }],
                None,
            )
            .unwrap();
    }

    /// One churn step (two changes) by agent `k`: a line in at the top, the last line out.
    fn churn(&self, k: usize, n: u64) {
        self.hunk(k, 0, 0, format!("line {n} by agent {}\n", k % AGENTS));
        let t = self.text();
        let last = t[..t.len() - 1].rfind('\n').map_or(0, |i| i + 1);
        self.hunk(k, last, t.len(), String::new());
    }

    fn reopen(&mut self) {
        self.mgr.release().unwrap();
        let (mgr, s, agents, obs) = open(
            &self.dir,
            &self.ws,
            &self.file,
            &self.key,
            &self.host,
            &self.config,
        );
        (self.mgr, self.s, self.agents, self.obs) = (mgr, s, agents, obs);
    }

    fn time_blame(&self, lines: Option<LineRange>) -> Duration {
        let t = Instant::now();
        let b = self.s.blame(self.obs, ROOT, lines).unwrap();
        let d = t.elapsed();
        assert!(!b.lines.is_empty());
        d
    }

    /// An outside edit: the file written back, then one line changed on disk and ingested.
    fn outside_edit(&self, n: u64) {
        self.s.flush(self.obs).unwrap();
        let t = fs::read_to_string(&self.file).unwrap();
        let at = t.len() / 2;
        let at = t[..at].rfind('\n').map_or(0, |i| i + 1);
        let new = format!("{}outside {n}\n{}", &t[..at], &t[at..]);
        fs::write(&self.file, new).unwrap();
        self.s.file_changed().unwrap();
    }
}

struct Stats {
    p50: f64,
    mean: f64,
    min: f64,
    max: f64,
    n: usize,
}

fn stats(mut v: Vec<Duration>) -> Stats {
    v.sort();
    let us: Vec<f64> = v.iter().map(|d| d.as_secs_f64() * 1e6).collect();
    Stats {
        p50: us[us.len() / 2],
        mean: us.iter().sum::<f64>() / us.len() as f64,
        min: us[0],
        max: us[us.len() - 1],
        n: us.len(),
    }
}

fn report(label: &str, changes: u64, what: &str, st: &Stats, version: u64, est: u64) {
    eprintln!(
        "[blame cost {label}] {changes} changes, {what}: p50 {:.0} us, mean {:.0} us, min {:.0} us, max {:.0} us ({} blames; v{version}, est {est} B)",
        st.p50, st.mean, st.min, st.max, st.n
    );
    eprintln!(
        "RUNREC {{\"job\":null,\"run\":\"blame_cost_{label}_{changes}_{what}\",\"tool\":\"blame_cost\",\"status\":\"COMPLETE\",\"changes\":{changes},\"version\":{version},\"estimated_bytes\":{est},\"blames\":{},\"p50_us\":{:.0},\"mean_us\":{:.0},\"min_us\":{:.0},\"max_us\":{:.0}}}",
        st.n, st.p50, st.mean, st.min, st.max
    );
}

fn sizes() -> Vec<u64> {
    std::env::var("BLAME_COST_SIZES")
        .unwrap_or_else(|_| "1200,40000,300000".into())
        .split(',')
        .map(|v| v.trim().parse().expect("BLAME_COST_SIZES: numbers"))
        .collect()
}

fn label() -> String {
    std::env::var("BLAME_COST_LABEL").unwrap_or_else(|_| "this".into())
}

fn config() -> Config {
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000_000;
    c.edit_burst = 1_000_000_000;
    // the build is timed nowhere; history kept as a host keeps it by default
    c
}

/// Build `n` changes of history (two per churn step) after the import.
fn build(fx: &Fx, n: u64) {
    let t0 = Instant::now();
    let mut k = 0u64;
    while 2 * k < n {
        fx.churn(k as usize, k);
        k += 1;
        if k % 20_000 == 0 {
            eprintln!(
                "[blame cost] built {} changes in {:.0} s",
                2 * k,
                t0.elapsed().as_secs_f64()
            );
        }
    }
    eprintln!(
        "[blame cost] built {n} changes in {:.1} s",
        t0.elapsed().as_secs_f64()
    );
}

#[test]
#[ignore = "a measurement: run in release with --nocapture, on a node"]
fn blame_cost_at_sizes() {
    let label = label();
    for n in sizes() {
        let mut fx = Fx::new(&format!("{label}-{n}"), config());
        build(&fx, n);
        let status = |fx: &Fx| {
            let s = fx.s.status().unwrap();
            (s.version, s.estimated_bytes)
        };
        // warm-up: the map built, the store's pages read
        fx.time_blame(None);
        // cold: the first blame after a reopen (5)
        let mut cold = Vec::new();
        for _ in 0..5 {
            fx.reopen();
            cold.push(fx.time_blame(None));
        }
        let (v, est) = status(&fx);
        report(&label, n, "cold", &stats(cold), v, est);
        fx.time_blame(None);
        // steady: after an edit, every line (30), and a 10-line range (30)
        let (mut steady, mut range) = (Vec::new(), Vec::new());
        for i in 0..30u64 {
            fx.churn(i as usize, n + i);
            steady.push(fx.time_blame(None));
            fx.churn(i as usize + 1, n + 100 + i);
            range.push(fx.time_blame(Some(LineRange {
                root: ROOT.into(),
                first: 100,
                last: 109,
            })));
        }
        let (v, est) = status(&fx);
        report(&label, n, "steady", &stats(steady), v, est);
        report(&label, n, "range", &stats(range), v, est);
        // after an outside edit (10): the write-back and the ingest aren't timed
        let mut outside = Vec::new();
        for i in 0..10u64 {
            fx.outside_edit(n + i);
            outside.push(fx.time_blame(None));
        }
        let (v, est) = status(&fx);
        report(&label, n, "outside", &stats(outside), v, est);
    }
}

/// The runtime check's cost (v0.9.5): a blame with the check sampled (1 in 1 here), at each
/// size; and how long it keeps the session's lock (v0.9.7), as another participant sees it:
/// a second thread calls `status` (which takes the lock and does almost nothing) in a loop,
/// and each call that starts while a sampled blame runs is timed. The longest such wait,
/// per blame, is the lock hold the blame imposed.
#[test]
#[ignore = "a measurement: run in release with --nocapture, on a node"]
fn blame_check_cost() {
    use std::sync::atomic::AtomicBool;
    let label = label();
    for n in sizes() {
        let mut c = config();
        c.blame_check_every = 1;
        let fx = Fx::new(&format!("{label}-check-{n}"), c);
        build(&fx, n);
        fx.time_blame(None);
        let in_blame = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let worst = Arc::new(std::sync::Mutex::new(Duration::ZERO));
        let probe = {
            let (s, in_blame, stop, worst) =
                (fx.s.clone(), in_blame.clone(), stop.clone(), worst.clone());
            std::thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    let during = in_blame.load(Ordering::SeqCst);
                    let t = Instant::now();
                    s.status().unwrap();
                    let d = t.elapsed();
                    if during {
                        let mut w = worst.lock().unwrap();
                        *w = (*w).max(d);
                    }
                    std::thread::sleep(Duration::from_micros(50));
                }
            })
        };
        let (mut checked, mut holds) = (Vec::new(), Vec::new());
        for i in 0..10u64 {
            fx.churn(i as usize, n + i);
            std::thread::sleep(Duration::from_millis(2));
            *worst.lock().unwrap() = Duration::ZERO;
            in_blame.store(true, Ordering::SeqCst);
            checked.push(fx.time_blame(None));
            in_blame.store(false, Ordering::SeqCst);
            // let a probe that started during the blame finish
            std::thread::sleep(Duration::from_millis(1));
            holds.push(*worst.lock().unwrap());
        }
        stop.store(true, Ordering::SeqCst);
        probe.join().unwrap();
        let s = fx.s.status().unwrap();
        report(
            &label,
            n,
            "check",
            &stats(checked),
            s.version,
            s.estimated_bytes,
        );
        report(
            &label,
            n,
            "check_lock_wait",
            &stats(holds),
            s.version,
            s.estimated_bytes,
        );
    }
}
