//! App failures (0.46.3): an agent maintains its own Apps.
//!
//! Every crash of a running instance, deploy that never went live (a failed
//! health gate, or an engine that couldn't start it) and failed build is
//! **recorded** in the App's failure history (`failures` in
//! `~/.k2/apps/app-audit.db`, read by `k2 app errors`), and the App's
//! workspace agent is **woken** through the alerts rail ([`super::alerts`]):
//!
//! - a crash wakes once per crash-loop breaker window (an hour): the first
//!   crash says what happened and that K2 is restarting it; further crashes
//!   in that hour are recorded and fold into the crash-loop alert
//!   (`supervisor::breaker`). An owner `k2 app start` after a crash loop
//!   clears it ([`clear_crash`]).
//! - a failed health gate, a deploy that couldn't start and a failed build
//!   wake at most once per App per kind every 5 minutes (the audit alerts'
//!   [`super::audit::ALERT_EVERY`]). A crash restart that fails its own
//!   gate counts as a crash, not a deploy.
//!
//! A wake carries ids and K2's own short reason, never App output or
//! secrets; the agent reads `k2 app logs` (framed as untrusted) for that.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use k2_core::app_audit::{self as store, Failure};
use k2_core::deploy::backoff::{BREAKER_RESTARTS, BREAKER_WINDOW_SECS};

/// Crashes wake once per this window (the breaker's).
pub const CRASH_WINDOW: Duration = Duration::from_secs(BREAKER_WINDOW_SECS as u64);
/// Other failures wake once per App per kind this often.
pub const ALERT_EVERY: Duration = super::audit::ALERT_EVERY;
/// Reasons in a wake and a record are cut to this many characters.
const REASON_CAP: usize = 300;

fn said() -> &'static Mutex<HashMap<(String, String), Instant>> {
    static S: OnceLock<Mutex<HashMap<(String, String), Instant>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

/// True (and marked) unless `group` was said for this App within `window`.
fn claim(app_id: &str, group: &str, window: Duration) -> bool {
    let mut s = said().lock().unwrap_or_else(|e| e.into_inner());
    let key = (app_id.to_string(), group.to_string());
    if s.get(&key).is_some_and(|t| t.elapsed() < window) {
        return false;
    }
    s.insert(key, Instant::now());
    true
}

/// The owner restarted an App after a crash loop: its next crash wakes.
pub fn clear_crash(app_id: &str) {
    said().lock().unwrap_or_else(|e| e.into_inner()).remove(&(app_id.to_string(), store::FAILURE_CRASH.to_string()));
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// K2's words, one line, no control characters, capped.
fn short(s: &str) -> String {
    let one = k2_core::deploy::framing::strip_controls(s).replace(['\n', '\r'], " ");
    let one = one.trim();
    if one.chars().count() <= REASON_CAP {
        return one.to_string();
    }
    let mut out: String = one.chars().take(REASON_CAP).collect();
    out.push('…');
    out
}

fn store_row(f: &Failure) {
    match store::open(&store::db_path()) {
        Ok(c) => {
            if let Err(e) = store::record_failure(&c, f) {
                k2_core::log_debug!("[apps/failures] record {} {}: {e}", f.app_id, f.kind);
            }
        }
        Err(e) => k2_core::log_debug!("[apps/failures] open app-audit.db: {e}"),
    }
}

fn owner_ws(app_id: &str) -> Option<String> {
    let h = k2_core::db::shared();
    let conn = h.lock();
    k2_core::deploy::store::get_app(&conn, app_id).ok().flatten().map(|a| a.owner_workspace)
}

/// A wake that went out: to whom, and what it said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Said {
    pub workspace: String,
    pub text: String,
}

fn say(app_id: &str, text: String) -> Option<Said> {
    let ws = owner_ws(app_id)?;
    super::alerts::notify(&ws, &text);
    Some(Said { workspace: ws, text })
}

fn signal_name(sig: i32) -> String {
    match sig {
        6 => "SIGABRT".into(),
        9 => "SIGKILL".into(),
        11 => "SIGSEGV".into(),
        15 => "SIGTERM".into(),
        n => format!("signal {n}"),
    }
}

fn exit_words(code: Option<i32>, signal: Option<i32>) -> String {
    match (code, signal) {
        (_, Some(s)) => format!("was killed by {}", signal_name(s)),
        (Some(c), None) => format!("exited with code {c}"),
        (None, None) => "exited".into(),
    }
}

/// A running instance stopped working.
#[derive(Debug, Clone, Default)]
pub struct Crash<'a> {
    pub app_id: &'a str,
    pub deployment: &'a str,
    /// It exited (else it stopped answering its health checks).
    pub exited: bool,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    /// Restarts in the breaker window, this one counted.
    pub restart_count: u32,
    /// Seconds until K2 restarts it; `None` = the breaker tripped (the
    /// crash-loop alert speaks for it).
    pub restart_in_secs: Option<i64>,
    /// What happened, when it isn't an exit or failing checks (a crash
    /// restart that failed its gate).
    pub what: Option<&'a str>,
}

/// Record a crash; wake the App's agent for the first one in the window.
pub fn crash(c: &Crash<'_>) -> Option<Said> {
    let what = match c.what {
        Some(w) => w.to_string(),
        None if c.exited => exit_words(c.exit_code, c.signal),
        None => "stopped answering its health checks".to_string(),
    };
    let wake = c.restart_in_secs.is_some() && claim(c.app_id, store::FAILURE_CRASH, CRASH_WINDOW);
    let reason = match c.restart_in_secs {
        Some(0) => format!("{what}; restarted at once"),
        Some(n) => format!("{what}; restart in {n}s"),
        None => format!("{what}; crash-looping, K2 stopped restarting it"),
    };
    store_row(&Failure {
        at_ms: now_ms(),
        app_id: c.app_id.into(),
        deployment: Some(c.deployment.into()),
        kind: store::FAILURE_CRASH.into(),
        exit_code: c.exit_code,
        signal: c.signal,
        reason: short(&reason),
        restart_count: c.restart_count,
        rolled_back: false,
        woke: wake,
    });
    if !wake {
        return None;
    }
    let when = match c.restart_in_secs {
        Some(0) | None => "now".to_string(),
        Some(n) => format!("in {n}s"),
    };
    let app = c.app_id;
    let dep = c.deployment;
    say(
        app,
        format!(
            "[k2 apps] App {app} crashed: it {what} (deployment {dep}). K2 restarts it {when} (restart {} of {BREAKER_RESTARTS} \
             allowed in an hour; more crashes this hour won't wake you, and at {BREAKER_RESTARTS} K2 stops and tells you). \
             Last lines: k2 app logs {app} --deployment {dep} --tail 20 ; every failure: k2 app errors {app} --kind crash",
            c.restart_count
        ),
    )
}

/// The new deploy failed inside its watch window and K2 switched back to
/// the warm version. Recorded only: `supervisor::auto_rollback` wakes.
pub fn rolled_back(app_id: &str, deployment: &str, exited: bool, exit_code: Option<i32>, signal: Option<i32>) {
    let (kind, what) = if exited {
        (store::FAILURE_CRASH, exit_words(exit_code, signal))
    } else {
        (store::FAILURE_HEALTH, "failed its health checks".to_string())
    };
    store_row(&Failure {
        at_ms: now_ms(),
        app_id: app_id.into(),
        deployment: Some(deployment.into()),
        kind: kind.into(),
        exit_code,
        signal,
        reason: short(&format!("{what} inside the watch window; rolled back to the previous version")),
        restart_count: 0,
        rolled_back: true,
        woke: true,
    });
}

/// A deploy (or rollback, start, restart) that never went live.
#[derive(Debug, Clone, Default)]
pub struct DeployFail<'a> {
    pub app_id: &'a str,
    pub deployment: &'a str,
    /// `deploy` | `rollback` | `start` | `restart`.
    pub deploy_kind: &'a str,
    pub actor: &'a str,
    /// The failure code (`health_failed`, `engine_unavailable`, …).
    pub code: &'a str,
    /// K2's message (for a health gate: the check, the timeout, the last
    /// probe).
    pub reason: &'a str,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
}

/// Record a deploy that never went live; wake the App's agent (at most
/// once per App per kind every 5 minutes). The supervisor never calls this
/// for a crash restart's own failure: that is a crash.
pub fn deploy_failed(d: &DeployFail<'_>) -> Option<Said> {
    let health = d.code == k2_core::deploy::codes::HEALTH_FAILED;
    let kind = if health { store::FAILURE_HEALTH } else { store::FAILURE_DEPLOY };
    let wake = claim(d.app_id, kind, ALERT_EVERY);
    let reason = short(&format!("{}: {}", d.code, d.reason));
    store_row(&Failure {
        at_ms: now_ms(),
        app_id: d.app_id.into(),
        deployment: Some(d.deployment.into()),
        kind: kind.into(),
        exit_code: d.exit_code,
        signal: d.signal,
        reason: reason.clone(),
        restart_count: 0,
        rolled_back: false,
        woke: wake,
    });
    if !wake {
        return None;
    }
    let app = d.app_id;
    let dep = d.deployment;
    let failed = if health { "failed its health gate" } else { "couldn't start" };
    let exit = if d.exit_code.is_some() || d.signal.is_some() { format!(" The instance {}.", exit_words(d.exit_code, d.signal)) } else { String::new() };
    let serving = if super::router::live(app).is_some() { "the previous version keeps serving" } else { "nothing is live now" };
    say(
        app,
        format!(
            "[k2 apps] App {app}: {} {dep} by {} {failed}: {reason}.{exit} It never went live, so nothing was rolled back; {serving}. \
             Last lines: k2 app logs {app} --deployment {dep} --tail 20 ; every failure: k2 app errors {app} --kind {kind}",
            d.deploy_kind, d.actor
        ),
    )
}

/// A build for a deploy that failed.
#[derive(Debug, Clone, Default)]
pub struct BuildFail<'a> {
    pub app_id: &'a str,
    pub actor: &'a str,
    pub code: &'a str,
    /// K2's message (never the build's output).
    pub reason: &'a str,
    /// The manifest's build step, when it ran.
    pub step: Option<&'a str>,
    /// The build log's id (`b-…`), when the build printed anything.
    pub log: Option<&'a str>,
}

/// Record a failed build; wake the App's agent (at most once per App every
/// 5 minutes).
pub fn build_failed(b: &BuildFail<'_>) -> Option<Said> {
    let wake = claim(b.app_id, store::FAILURE_BUILD, ALERT_EVERY);
    let step = b.step.map(|s| format!(" at `{}`", short(s).chars().take(80).collect::<String>())).unwrap_or_default();
    let reason = short(&format!("{}{step}: {}", b.code, b.reason));
    store_row(&Failure {
        at_ms: now_ms(),
        app_id: b.app_id.into(),
        deployment: b.log.map(str::to_string),
        kind: store::FAILURE_BUILD.into(),
        exit_code: None,
        signal: None,
        reason: reason.clone(),
        restart_count: 0,
        rolled_back: false,
        woke: wake,
    });
    if !wake {
        return None;
    }
    let app = b.app_id;
    let out = match b.log {
        Some(l) => format!("Build output: k2 app logs {app} --deployment {l}"),
        None => "It stopped before the build printed anything".to_string(),
    };
    let serving = if super::router::live(app).is_some() { "the live version keeps serving" } else { "nothing is live" };
    say(
        app,
        format!(
            "[k2 apps] App {app}: a build for a deploy by {} failed ({reason}). Nothing was deployed; {serving}. \
             {out} ; every failure: k2 app errors {app} --kind build",
            b.actor
        ),
    )
}

#[cfg(test)]
#[path = "failures_tests.rs"]
mod tests;
