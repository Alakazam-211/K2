//! Runs the hook installer for the daemon
//! (prd-daemon-activity-and-thread-working-v1 DA7, DA17; A9–A11; Q2).
//!
//! - [`spawn`]: after `/boot-status` is ready, one pass on the blocking
//!   pool, then one every 10 minutes (self-heal). Never on the boot path.
//! - [`run_once`]: one pass (also `POST /cli/hooks/install`).
//! - [`note_legacy_hook`]: a legacy `GET /hook/complete` means an older
//!   app bundle rewrote `notify.sh`; reinstall now (debounced), not in
//!   10 minutes (A10).
//!
//! Each pass honours the install gate (`K2_HOOK_INSTALL=0`, debug build on
//! the real home → skipped) and the `agentHooks` app setting (off → K2's
//! entries are removed). The first pass that finds a problem emits
//! `hooks_install_failed` once per boot (A11).

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use parking_lot::Mutex;
use serde::Serialize;

use k2_core::agent_hooks::install::{self, InstallReport, InstallRequest};
use k2_core::log_debug;

/// Self-heal cadence.
const TICK: Duration = Duration::from_secs(10 * 60);
/// Minimum gap between legacy-triggered reinstalls.
const LEGACY_DEBOUNCE_MS: i64 = 60_000;
/// A self-heal tick reuses the last `claude --version` this long, so the
/// daemon doesn't start Claude every 10 minutes. Other triggers re-probe.
const PROBE_REUSE_MS: i64 = 60 * 60_000;

/// Why a pass ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    Boot,
    Tick,
    Route,
    LegacyHook,
    Setting,
}

/// The last pass, for `k2 hooks status`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LastRun {
    pub at_ms: i64,
    pub trigger: Trigger,
    /// Set when the gate refused the pass (no report then).
    pub skipped: Option<String>,
    /// The `agentHooks` setting at the time.
    pub agent_hooks_setting: bool,
    pub report: Option<InstallReport>,
}

fn last() -> &'static Mutex<Option<LastRun>> {
    static LAST: OnceLock<Mutex<Option<LastRun>>> = OnceLock::new();
    LAST.get_or_init(|| Mutex::new(None))
}

/// The last pass as JSON (`null` before the first).
pub fn last_run_json() -> serde_json::Value {
    serde_json::to_value(last().lock().clone()).unwrap_or(serde_json::Value::Null)
}

/// The last report, if a pass has run.
pub fn last_report() -> Option<InstallReport> {
    last().lock().as_ref().and_then(|r| r.report.clone())
}

/// One installer pass. `remove` forces an uninstall (`k2 hooks uninstall`).
/// Blocking: probes `claude --version` (≤ 5 s) and writes files.
pub fn run_once(trigger: Trigger, remove: bool) -> LastRun {
    let at_ms = chrono::Utc::now().timestamp_millis();
    let setting = k2_core::app_settings::load().agent_hooks;
    let run = match dirs::home_dir() {
        None => LastRun {
            at_ms,
            trigger,
            skipped: Some("no home directory".to_string()),
            agent_hooks_setting: setting,
            report: None,
        },
        Some(home) => match install::install_gate_for_process(&home) {
            Err(reason) => LastRun {
                at_ms,
                trigger,
                skipped: Some(reason.to_string()),
                agent_hooks_setting: setting,
                report: None,
            },
            Ok(()) => {
                let remove = remove || !setting;
                let claude_version = if remove { None } else { claude_version(trigger, at_ms) };
                let report = install::run(&InstallRequest {
                    home: &home,
                    daemon_version: env!("CARGO_PKG_VERSION"),
                    claude_version,
                    remove,
                });
                LastRun { at_ms, trigger, skipped: None, agent_hooks_setting: setting, report: Some(report) }
            }
        },
    };
    match (&run.skipped, &run.report) {
        (Some(reason), _) => log_debug!("[agent-hooks] install pass skipped ({trigger:?}): {reason}"),
        (None, Some(r)) => log_debug!(
            "[agent-hooks] {} pass ({trigger:?}): script={:?} claude={:?}/{} clis={:?}",
            r.action,
            r.script.state,
            r.claude_version,
            r.claude_event_set,
            r.clis.iter().map(|c| (c.cli, &c.state)).collect::<Vec<_>>()
        ),
        (None, None) => {}
    }
    *last().lock() = Some(run.clone());
    if let Some(report) = &run.report {
        emit_failures_once(report);
    }
    run
}

/// `claude --version`, reused for [`PROBE_REUSE_MS`] on a self-heal tick.
fn claude_version(trigger: Trigger, now_ms: i64) -> Option<String> {
    static CACHE: Mutex<Option<(i64, Option<String>)>> = Mutex::new(None);
    if trigger == Trigger::Tick {
        if let Some((at, v)) = CACHE.lock().clone() {
            if now_ms - at < PROBE_REUSE_MS {
                return v;
            }
        }
    }
    let v = install::probe_claude_version();
    *CACHE.lock() = Some((now_ms, v.clone()));
    v
}

/// A11: one `hooks_install_failed` per boot, the first time a pass finds
/// an unreadable config or a write error.
fn emit_failures_once(report: &InstallReport) {
    static EMITTED: AtomicBool = AtomicBool::new(false);
    let failures = report.failures();
    if failures.is_empty() || EMITTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let _ = crate::session_events::emit(crate::session_events::SessionEvent::HooksInstallFailed {
        failures,
    });
}

/// Run a pass on the blocking pool without waiting for it.
pub fn request_run(trigger: Trigger) {
    if tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    tokio::task::spawn_blocking(move || {
        run_once(trigger, false);
    });
}

/// A legacy hook arrived from a live pane: the on-disk script is an older
/// app's. Reinstall now, at most once a minute.
pub fn note_legacy_hook() {
    static LAST_MS: AtomicI64 = AtomicI64::new(0);
    let now = chrono::Utc::now().timestamp_millis();
    let prev = LAST_MS.load(Ordering::SeqCst);
    if now - prev < LEGACY_DEBOUNCE_MS {
        return;
    }
    if LAST_MS.compare_exchange(prev, now, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
        log_debug!("[agent-hooks] legacy /hook/complete seen: reinstalling notify.sh");
        request_run(Trigger::LegacyHook);
    }
}

/// Start the boot pass + 10-minute self-heal. Call after `set_ready()`.
pub fn spawn() -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut trigger = Trigger::Boot;
        loop {
            let t = trigger;
            let _ = tokio::task::spawn_blocking(move || {
                run_once(t, false);
                crate::hook_ingest::sweep_live();
            })
            .await;
            trigger = Trigger::Tick;
            tokio::time::sleep(TICK).await;
        }
    })
}
