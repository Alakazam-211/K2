//! What a change's store commit costs at `CommitDurability::Immediate` (every commit waits for
//! the disk) and with grouped commits (`CommitDurability::grouped(32, 100)`), and what the
//! rest of each call costs beside it. Printed, not a gate.
//!
//! `commit_cost_grouped_against_immediate` is ignored: run it by name, in release, on a Linux
//! node (`--features native` adds the native engine's arms):
//!
//! ```text
//! cargo test --release -p k2-mpc-base --features native --test commit_cost \
//!     commit_cost_grouped_against_immediate -- --ignored --nocapture
//! ```
//!
//! **Shape.** One arm per engine (Yrs; the native engine with the feature) and durability
//! (`immediate`, `grouped_32_100`), each on a fresh store with one doc and no file (so no
//! write-back joins the figures): 2,000 one-character edits by a person (`Session::edit`) and
//! 2,000 agent verbs (`append` of a short line through `Session::run_tool`), taking turns. The
//! host's clock is the wall clock, so a group closes at 32 changes or 100 ms, whichever comes
//! first. Two rounds, the arms alternating within each, so a warm cache favours neither.
//!
//! **Printed,** one `MEASURE commit_cost` line per arm and one `MEASURE commit_cost_op` line
//! per arm and kind of call:
//! - `commit_p50_us`, `commit_p95_us`: the `commit.store` timer (a change's store
//!   transaction, its wait for the disk included), per call, read before and after it
//!   (single-threaded, so each difference is that call's own); `timer_p50_us`,
//!   `timer_p95_us`: the same from the timer's own histogram (within about 3%);
//! - `edit_p50_us`, `edit_p95_us`: each call's wall time less its `commit.store` time: the
//!   rest of the call, which waiting for the disk can slow down (a core gone cold);
//! - `edits_per_s` and `wall_ms`: the 4,000 calls, end to end;
//! - `not_durable`: the person's edits whose result said `durable: false`; `commits` and
//!   `durable_through`: the store's counts after the last call; `flush_us`: the store's
//!   `flush` that then made every commit durable.
//!
//! The row fails only when its shape didn't form: a call that didn't record exactly one
//! `commit.store`, an edit refused, the text not what the calls made, a grouped arm whose
//! results were all durable or an `Immediate` arm with one that wasn't.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use k2_mpc_base::session::{
    AuditEvent, Config, ConnId, Host, OpenOptions, Participant, ParticipantKind, Rights, Session,
    SessionManager, Stay, TextFilePack,
};
use k2_mpc_base::store::{CommitDurability, DocKey, DocStore, StoreSettings};
use k2_mpc_base::text::diff::Hunk;
use k2_mpc_base::timers::{self, Path};
use k2_mpc_base::Edit;

const MD: TextFilePack = TextFilePack::new("markdown", 16 << 20);
const ROOT: &str = TextFilePack::ROOT;
/// The doc at the start: the person types on its last, empty line.
const SEED: &str = "# Notes\n\n\n";
/// Each kind of call, per arm.
const CALLS: usize = 2_000;
const ROUNDS: usize = 2;

/// The host: full rights, the wall clock (ms).
struct H;

impl Host for H {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64)
    }
    fn on_event(&self, _: &AuditEvent) {}
}

/// Every engine this build has: Yrs, and the native engine with feature `native`.
fn engines() -> Vec<(&'static str, OpenOptions)> {
    let yrs = [("yrs", OpenOptions::new())];
    yrs.to_vec()
}

fn durabilities() -> Vec<(&'static str, CommitDurability)> {
    vec![
        ("immediate", CommitDurability::Immediate),
        (
            "grouped_32_100",
            CommitDurability::grouped(32, 100).expect("in range"),
        ),
    ]
}

fn manager(store: &Arc<DocStore>) -> SessionManager {
    let mut c = Config::default();
    c.edit_rate_per_s = 1 << 30;
    c.edit_burst = 1 << 30;
    c.idle_leave_ms = u64::MAX / 4;
    c.native_beta = true;
    with_test_keys(SessionManager::new(store.clone(), Arc::new(H), c))
}

/// A build without the native engine has no key service to give.
fn with_test_keys(m: SessionManager) -> SessionManager {
    m
}

/// The `q` quantile (0 < q <= 1) of `v` by rank `ceil(q * n)`, as the timers take it.
fn pct(v: &mut [u64], q: f64) -> u64 {
    v.sort_unstable();
    let n = v.len() as u64;
    let rank = ((q * n as f64).ceil() as u64).clamp(1, n);
    v[(rank - 1) as usize]
}

fn us(ns: u64) -> f64 {
    ns as f64 / 1_000.0
}

/// One kind of call's figures, ns per call.
#[derive(Default)]
struct Calls {
    commit: Vec<u64>,
    rest: Vec<u64>,
}

impl Calls {
    /// Time `call`, with its `commit.store` time apart; exactly one commit per call.
    fn time<R>(&mut self, what: &str, call: impl FnOnce() -> R) -> R {
        let (n0, t0) = timers::read(Path::CommitStore);
        let start = Instant::now();
        let out = call();
        let wall = u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX);
        let (n1, t1) = timers::read(Path::CommitStore);
        assert_eq!(
            n1 - n0,
            1,
            "{what}: one commit.store per call, counted {}",
            n1 - n0
        );
        let commit = t1 - t0;
        self.commit.push(commit);
        self.rest.push(wall.saturating_sub(commit));
        out
    }
}

struct Arm {
    dir: PathBuf,
    store: Arc<DocStore>,
    mgr: SessionManager,
    session: Arc<Session>,
}

fn arm(engine: &str, opts: &OpenOptions, durability: &str, d: CommitDurability) -> Arm {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-commit-cost-{engine}-{durability}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let settings = StoreSettings::DEFAULT.with_commit_durability(d);
    let store = Arc::new(DocStore::open_with(dir.join("d.redb"), settings).unwrap());
    let mgr = manager(&store);
    let key = DocKey {
        space: "ws".into(),
        path: format!("{engine}-{durability}.md"),
    };
    let session = mgr
        .open_with(Arc::new(MD), &key, Some(SEED.as_bytes()), opts)
        .unwrap();
    Arm {
        dir,
        store,
        mgr,
        session,
    }
}

fn join(s: &Session, pid: &str, kind: ParticipantKind) -> ConnId {
    s.join(
        Participant::new(pid, pid, "#2563eb", kind, "cli"),
        Stay::Held,
    )
    .unwrap()
    .conn
}

fn run_arm(round: usize, engine: &str, opts: &OpenOptions, name: &str, d: CommitDurability) {
    let a = arm(engine, opts, name, d);
    let person = join(&a.session, "pat", ParticipantKind::Person);
    let agent = join(&a.session, "agent", ParticipantKind::Agent);
    let append = ["append".to_owned()];
    let (mut by_person, mut by_agent) = (Calls::default(), Calls::default());
    let mut not_durable = 0usize;
    timers::reset();
    timers::set_enabled(true);
    let start = Instant::now();
    for k in 0..CALLS {
        // the person types at the end of their own line, the seed's last
        let at = SEED.len() - 1 + k;
        let r = by_person.time("the person's edit", || {
            a.session.edit(
                person,
                vec![Edit::Text {
                    root: ROOT.to_owned(),
                    hunks: vec![Hunk {
                        start: at,
                        end: at,
                        insert: "a".to_owned(),
                    }],
                }],
                None,
            )
        });
        if !r.expect("the person's edit lands").durable {
            not_durable += 1;
        }
        let out = by_agent.time("the agent's verb", || {
            a.session.run_tool(agent, ROOT, &append, b"agent line\n")
        });
        let out = out.expect("the agent's verb runs");
        assert_eq!(
            out.exit,
            0,
            "append: {}",
            String::from_utf8_lossy(&out.stdout)
        );
    }
    let wall = start.elapsed();
    timers::set_enabled(false);
    let snap = timers::snapshot();
    let timer = snap.get(Path::CommitStore).expect("listed");
    assert_eq!(timer.count, 2 * CALLS as u64, "commit.store calls");
    let (commits, durable_through) = (a.store.commits(), a.store.durable_through());
    let t = Instant::now();
    a.store.flush().unwrap();
    let flush = t.elapsed();
    assert_eq!(a.store.durable_through(), a.store.commits());

    // the shape formed: every call landed, and the results said what the setting promises
    let r = a.session.read(person).unwrap();
    let text = &r.text_by_root[0].1;
    let want_len = SEED.len() + CALLS + CALLS * "agent line\n".len();
    assert_eq!(text.len(), want_len, "the text holds every call's change");
    let typed = format!("{}{}\n", &SEED[..SEED.len() - 1], "a".repeat(CALLS));
    assert!(text.starts_with(&typed), "the person's line comes first");
    match d {
        CommitDurability::Immediate => assert_eq!(not_durable, 0, "Immediate: all durable"),
        _ => assert!(not_durable > 0, "grouped: some results not yet durable"),
    }

    let n = 2 * CALLS;
    let mut commit: Vec<u64> = by_person
        .commit
        .iter()
        .chain(&by_agent.commit)
        .copied()
        .collect();
    let mut rest: Vec<u64> = by_person
        .rest
        .iter()
        .chain(&by_agent.rest)
        .copied()
        .collect();
    let secs = wall.as_secs_f64();
    let (c50, c95) = (pct(&mut commit, 0.50), pct(&mut commit, 0.95));
    let (e50, e95) = (pct(&mut rest, 0.50), pct(&mut rest, 0.95));
    println!(
        "MEASURE commit_cost round={round} engine={engine} commit={name} calls={n} \
         commit_p50_us={:.1} commit_p95_us={:.1} timer_p50_us={:.1} timer_p95_us={:.1} \
         edit_p50_us={:.1} edit_p95_us={:.1} edits_per_s={:.0} wall_ms={:.0} \
         not_durable={not_durable} commits={commits} durable_through={durable_through} \
         flush_us={:.0}",
        us(c50),
        us(c95),
        us(timer.p50_ns),
        us(timer.p95_ns),
        us(e50),
        us(e95),
        n as f64 / secs,
        secs * 1e3,
        flush.as_secs_f64() * 1e6,
    );
    for (op, c) in [
        ("person_edit", &mut by_person),
        ("agent_verb", &mut by_agent),
    ] {
        let (c50, c95) = (pct(&mut c.commit, 0.50), pct(&mut c.commit, 0.95));
        let (e50, e95) = (pct(&mut c.rest, 0.50), pct(&mut c.rest, 0.95));
        println!(
            "MEASURE commit_cost_op round={round} engine={engine} commit={name} op={op} \
             calls={} commit_p50_us={:.1} commit_p95_us={:.1} edit_p50_us={:.1} \
             edit_p95_us={:.1}",
            c.commit.len(),
            us(c50),
            us(c95),
            us(e50),
            us(e95),
        );
    }
    let dir = a.dir.clone();
    a.mgr.release().unwrap();
    drop(a);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "printed, not of record: run by name: crates/base/tests/commit_cost.rs"]
fn commit_cost_grouped_against_immediate() {
    println!(
        "MEASURE commit_cost_build release={} hooks={} calls_per_kind={CALLS} rounds={ROUNDS}",
        !cfg!(debug_assertions),
        timers::HOOKS
    );
    for round in 1..=ROUNDS {
        for (engine, opts) in engines() {
            let mut ds = durabilities();
            // alternate which arm runs first, so a warm cache favours neither
            if round % 2 == 0 {
                ds.reverse();
            }
            for (name, d) in ds {
                run_arm(round, engine, &opts, name, d);
            }
        }
    }
}
