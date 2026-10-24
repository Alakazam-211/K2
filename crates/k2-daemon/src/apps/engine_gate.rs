//! The App engine as the supervisor sees it (prd-app-runtime-update-continuity-v1
//! S1: D12/A3/R33, R34, R43).
//!
//! - **Adopt before any start (R33).** At boot the supervisor asks the engine
//!   to `adopt` until it **answers**. Any error (a missing or stale socket,
//!   an engine mid-restart, `caller_not_daemon` before a hand-off's MAINPID
//!   move) is retried every [`ADOPT_RETRY`]. Until an answer arrives no
//!   instance of any App is started, so a surviving container is never
//!   doubled and leaked. Past [`NOT_ANSWERING_AFTER`]: one ERROR line,
//!   `appEngine.state = not_answering`, one owner notice, and it keeps
//!   waiting (the door answers 503 meanwhile).
//! - **What runs (R34).** Each `version` answer is recorded; a protocol
//!   that isn't this daemon's is a mismatch: an ERROR line and one owner
//!   notice per mismatch run.
//! - **Kick on an engine change (R43).** [`engine_changed`] (an engine
//!   install by any path, or a protocol that starts matching) bumps a
//!   generation; the supervisor's tick sees it and starts every pending
//!   K2-side restart at once with its backoff reset.
//!
//! The state is process-wide; the logic is [`GateCore`] and [`wait_adopt`],
//! which tests drive with their own clock and engine.

use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use k2_core::deploy::engine::RunHandle;
use serde_json::{json, Value};

/// How often boot asks the engine to adopt while it doesn't answer.
pub const ADOPT_RETRY: Duration = Duration::from_secs(2);
/// After this long without an answer: ERROR, `not_answering`, owner notice.
pub const NOT_ANSWERING_AFTER: Duration = Duration::from_secs(120);
/// An App waiting for the engine is retried at least this often (R6).
pub const WAITING_RETRY_SECS: i64 = 60;
/// While something waits on the engine, its `version` is polled this often
/// (T31: an App starts within 2 s of the engine answering).
pub const WATCH_EVERY: Duration = Duration::from_millis(1500);

/// The running engine's `version` answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Running {
    pub version: String,
    pub protocol: u64,
}

/// What one `version` observation changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Seen {
    /// The engine answers with this daemon's protocol after it didn't
    /// (another protocol, or no answer): R43's signal.
    NewlyMatching,
    /// Another protocol. `first` = the first observation of this mismatch
    /// run (R34 reports once per run).
    Mismatch { running: u64, first: bool },
    /// No answer.
    NotAnswering,
    /// Nothing changed.
    Same,
}

/// The gate's state and rules (pure; the process-wide copy is [`core`]).
#[derive(Debug, Clone, Default)]
pub struct GateCore {
    /// The engine answered `adopt` at least once in this process.
    pub adopt_answered: bool,
    /// Boot waited past [`NOT_ANSWERING_AFTER`] and still has no answer.
    pub not_answering: bool,
    pub last_error: Option<String>,
    pub running: Option<Running>,
    /// The last observation: answered with this daemon's protocol.
    pub matching: Option<bool>,
    mismatch_reported: bool,
    /// R43: bumped on every engine change.
    pub generation: u64,
    pub last_change: Option<String>,
}

impl GateCore {
    /// Record one `version` answer (or failure) against `want` protocol.
    pub fn observe(&mut self, ans: &Result<Value, String>, want: u32) -> Seen {
        let prev = self.matching;
        match ans {
            Ok(v) => {
                let protocol = v["protocol"].as_u64().unwrap_or(0);
                self.running = Some(Running { version: v["version"].as_str().unwrap_or("?").to_string(), protocol });
                self.last_error = None;
                let ok = protocol == want as u64;
                self.matching = Some(ok);
                if ok {
                    self.mismatch_reported = false;
                    if prev == Some(false) {
                        return Seen::NewlyMatching;
                    }
                    Seen::Same
                } else {
                    let first = !self.mismatch_reported;
                    self.mismatch_reported = true;
                    Seen::Mismatch { running: protocol, first }
                }
            }
            Err(e) => {
                self.running = None;
                self.last_error = Some(e.clone());
                self.matching = Some(false);
                Seen::NotAnswering
            }
        }
    }

    /// R43: an engine change (install, or the protocol starts matching).
    pub fn changed(&mut self, why: &str) {
        self.generation += 1;
        self.last_change = Some(why.to_string());
    }
}

fn core() -> &'static Mutex<GateCore> {
    static G: OnceLock<Mutex<GateCore>> = OnceLock::new();
    G.get_or_init(|| Mutex::new(GateCore::default()))
}

fn with<R>(f: impl FnOnce(&mut GateCore) -> R) -> R {
    f(&mut core().lock().unwrap_or_else(|e| e.into_inner()))
}

/// A copy of the process-wide gate.
pub fn snapshot() -> GateCore {
    with(|g| g.clone())
}

/// R43: an engine was installed (any path: the boot heal, the owner's
/// `k2 app engine install`, auto-prepare) or its protocol now matches.
/// The supervisor's next tick kicks every pending K2-side restart.
pub fn engine_changed(why: &str) {
    k2_core::log_debug!("[apps] engine changed ({why}): pending restarts run now");
    with(|g| g.changed(why));
}

/// The change generation (the supervisor kicks when it moves).
pub fn generation() -> u64 {
    with(|g| g.generation)
}

/// Record a `version` observation in the process-wide gate and act on it:
/// R34's line and notice on a new mismatch, R43's kick on a newly
/// matching engine.
pub fn observe(ans: &Result<Value, String>) -> Seen {
    let want = k2_root_proto::engine::ENGINE_PROTOCOL;
    let seen = with(|g| g.observe(ans, want));
    match &seen {
        Seen::NewlyMatching => engine_changed(&format!("the App engine answers with protocol {want} now")),
        Seen::Mismatch { running, first: true } => report_mismatch(*running, want),
        _ => {}
    }
    seen
}

/// R34: the ERROR line and the owner notice for a protocol mismatch.
pub fn report_mismatch(running: u64, want: u32) {
    let next = if k2_core::deploy::engine_heal_enabled() {
        "K2 installs the matching engine now (engine heal; k2 app engine status)"
    } else {
        "the engine heal is off (appsEngineHeal): the owner runs k2 app engine install"
    };
    super::runtime_notes::error(&format!("engine protocol {running} vs {want}: {next}"));
    super::runtime_notes::owner_notice(
        "K2's App engine doesn't match this K2",
        &format!(
            "The running k2-app-engine speaks protocol {running} and this K2 speaks {want}, so Apps can't start or \
             restart on it. {next}. Apps that should run wait for the engine (they don't count as crashing) and start \
             as soon as it matches."
        ),
    );
}

/// What boot's adopt wait needs (tests pass a fake clock and engine).
pub trait WaitOps {
    /// Ask the engine(s) to adopt: `Ok` = it answered (maybe with nothing).
    fn adopt(&self) -> Result<Vec<RunHandle>, String>;
    /// Time since the wait started.
    fn elapsed(&self) -> Duration;
    fn sleep(&self, d: Duration);
    /// Past [`NOT_ANSWERING_AFTER`]: called once per wait.
    fn not_answering(&self, waited: Duration, last_error: &str);
}

/// R33: adopt until the engine answers; never returns before it does.
pub fn wait_adopt(ops: &dyn WaitOps) -> Vec<RunHandle> {
    let mut reported = false;
    loop {
        match ops.adopt() {
            Ok(v) => {
                with(|g| {
                    g.adopt_answered = true;
                    g.not_answering = false;
                });
                if reported {
                    k2_core::log_debug!("[apps] the App engine answered adopt after {}s", ops.elapsed().as_secs());
                }
                return v;
            }
            Err(e) => {
                let waited = ops.elapsed();
                if !reported && waited >= NOT_ANSWERING_AFTER {
                    reported = true;
                    with(|g| {
                        g.not_answering = true;
                        g.last_error = Some(e.clone());
                    });
                    ops.not_answering(waited, &e);
                }
                ops.sleep(ADOPT_RETRY);
            }
        }
    }
}

/// The live `not_answering` report (R33/R34): ERROR line + owner notice.
pub fn report_not_answering(waited: Duration, last_error: &str) {
    super::runtime_notes::error(&format!(
        "the App engine hasn't answered adopt for {}s ({last_error}); no App starts until it does; still waiting",
        waited.as_secs()
    ));
    super::runtime_notes::owner_notice(
        "K2's App engine isn't answering",
        &format!(
            "K2 has asked k2-app-engine to report the running Apps for {}s and it hasn't answered ({last_error}). \
             K2 starts no App until it does, so nothing runs twice; their addresses answer 503 meanwhile. \
             Check `k2 app engine status` and `systemctl --user -M k2app@ status k2-app-engine`; the owner can run \
             `k2 app engine install`.",
            waited.as_secs()
        ),
    );
}

/// `{adoptAnswered, notAnswering, running, matching, lastError}` for status.
pub fn json() -> Value {
    let g = snapshot();
    json!({
        "adoptAnswered": g.adopt_answered,
        "notAnswering": g.not_answering,
        "running": g.running.as_ref().map(|r| json!({ "version": r.version, "protocol": r.protocol })),
        "matching": g.matching,
        "lastError": g.last_error,
        "lastChange": g.last_change,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    #[test]
    fn observe_reports_a_mismatch_once_and_a_newly_matching_engine() {
        let mut g = GateCore::default();
        let v = |p: u64| Ok(json!({ "version": "0.46.0", "protocol": p }));
        assert_eq!(g.observe(&v(3), 3), Seen::Same, "a first matching answer is no change");
        assert_eq!(g.observe(&v(2), 3), Seen::Mismatch { running: 2, first: true });
        assert_eq!(g.observe(&v(2), 3), Seen::Mismatch { running: 2, first: false });
        assert_eq!(g.observe(&v(3), 3), Seen::NewlyMatching);
        assert_eq!(g.running, Some(Running { version: "0.46.0".into(), protocol: 3 }));
        assert_eq!(g.observe(&Err("gone".into()), 3), Seen::NotAnswering);
        assert_eq!(g.running, None);
        assert_eq!(g.observe(&v(3), 3), Seen::NewlyMatching, "answering again after silence is a change");
        assert_eq!(g.observe(&v(2), 3), Seen::Mismatch { running: 2, first: true }, "a new mismatch run reports again");
        let before = g.generation;
        g.changed("install");
        assert_eq!((g.generation, g.last_change.as_deref()), (before + 1, Some("install")));
    }

    struct Fake {
        fails: Cell<u32>,
        calls: Cell<u32>,
        clock: Cell<Duration>,
        reported: RefCell<Vec<(u64, String)>>,
    }

    impl WaitOps for Fake {
        fn adopt(&self) -> Result<Vec<RunHandle>, String> {
            self.calls.set(self.calls.get() + 1);
            if self.fails.get() > 0 {
                self.fails.set(self.fails.get() - 1);
                return Err("App engine: caller_not_daemon".into());
            }
            Ok(vec![RunHandle { app_id: "a".into(), deployment_id: "d-1".into(), port: 41000, pid: Some(7), engine_ref: "podman:x".into() }])
        }
        fn elapsed(&self) -> Duration {
            self.clock.get()
        }
        fn sleep(&self, d: Duration) {
            self.clock.set(self.clock.get() + d);
        }
        fn not_answering(&self, waited: Duration, e: &str) {
            self.reported.borrow_mut().push((waited.as_secs(), e.to_string()));
        }
    }

    #[test]
    fn wait_adopt_retries_every_two_seconds_and_reports_once_past_120s() {
        // 10 s of caller_not_daemon: 5 refusals, no report.
        let f = Fake { fails: Cell::new(5), calls: Cell::new(0), clock: Cell::new(Duration::ZERO), reported: RefCell::new(vec![]) };
        let got = wait_adopt(&f);
        assert_eq!((got.len(), f.calls.get(), f.clock.get()), (1, 6, Duration::from_secs(10)));
        assert!(f.reported.borrow().is_empty());
        // 200 s without an answer: one report at 120 s, then it keeps waiting.
        let f = Fake { fails: Cell::new(100), calls: Cell::new(0), clock: Cell::new(Duration::ZERO), reported: RefCell::new(vec![]) };
        let got = wait_adopt(&f);
        assert_eq!(got[0].deployment_id, "d-1", "it still adopts when the engine finally answers");
        assert_eq!(f.calls.get(), 101);
        assert_eq!(*f.reported.borrow(), vec![(120, "App engine: caller_not_daemon".to_string())], "exactly one report");
    }
}
