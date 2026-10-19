//! K2 Connect tunnel auto-start retry (0.46.2).
//!
//! Before 0.46.2 the boot auto-start tried `start_tunnel` once. When that
//! failed (cert broker, network, frps) it logged "will retry" and nothing
//! did: rosson.k2.dev stayed dark for 14 minutes on 2026-10-19 until the
//! owner re-enabled the tunnel by hand. This module is the retry: after a
//! failed start it waits on a capped backoff ([`AUTOSTART_BACKOFF`]) and
//! tries again, whatever the cause, until the tunnel runs or the owner no
//! longer wants it (disabled, auto-start off, released, air-gap). One retry
//! loop per process ([`RetryClaim`]), so retries never stack.

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use k2_core::log_debug;

/// Waits after the 1st, 2nd, … failed auto-start: 15 s, 30 s, 1 m, 2 m,
/// then every 5 m (the last step repeats).
pub(crate) const AUTOSTART_BACKOFF: [Duration; 5] = [
    Duration::from_secs(15),
    Duration::from_secs(30),
    Duration::from_secs(60),
    Duration::from_secs(120),
    Duration::from_secs(300),
];

/// `steps[failures - 1]`, the last step repeating (the cap). `failures` 0
/// is treated as 1. An empty table means "no wait".
pub(crate) fn backoff_step(steps: &[Duration], failures: u32) -> Duration {
    let Some(last) = steps.last() else {
        return Duration::ZERO;
    };
    let i = (failures.max(1) - 1) as usize;
    *steps.get(i).unwrap_or(last)
}

/// Whether another attempt should run (checked before every retry).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Gate {
    /// Still wanted and not running: try again.
    Go,
    /// Already running (someone else started it, e.g. `k2 tunnel enable`).
    Running,
    /// No longer wanted; the reason is logged.
    Stop(String),
}

/// How a retry run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RetryOutcome {
    /// An attempt succeeded (its message), after `attempts` tries.
    Started { attempts: u32, detail: String },
    /// The tunnel was found running before the next attempt.
    AlreadyRunning { attempts: u32 },
    /// The gate stopped the retries.
    Stopped { attempts: u32, reason: String },
}

/// Run `attempt` until it succeeds, sleeping `backoff_step(steps, n)` after
/// the n-th failure and asking `gate` before each retry (not before the
/// first attempt — the caller gated that). Uses `tokio::time`, so a test
/// with a paused clock sees the exact waits.
pub(crate) async fn retry_until_started<A, F, G>(
    steps: &[Duration],
    mut attempt: A,
    mut gate: G,
) -> RetryOutcome
where
    A: FnMut() -> F,
    F: Future<Output = Result<String, String>>,
    G: FnMut() -> Gate,
{
    let mut failures: u32 = 0;
    loop {
        match attempt().await {
            Ok(detail) => {
                return RetryOutcome::Started { attempts: failures + 1, detail };
            }
            Err(e) => {
                failures = failures.saturating_add(1);
                let wait = backoff_step(steps, failures);
                log_debug!(
                    "[daemon/tunnel] auto-start failed (attempt {failures}; tunnel not running; \
                     daemon unaffected; retrying in {wait:?}): {e}"
                );
                tokio::time::sleep(wait).await;
            }
        }
        match gate() {
            Gate::Go => {}
            Gate::Running => return RetryOutcome::AlreadyRunning { attempts: failures },
            Gate::Stop(reason) => {
                return RetryOutcome::Stopped { attempts: failures, reason };
            }
        }
    }
}

/// The production gate: re-reads `tunnel.json` (the owner's latest word)
/// and the live connector before every retry.
pub(crate) fn autostart_gate() -> Gate {
    if k2_core::airgap::enabled() {
        return Gate::Stop("air-gap is on".to_string());
    }
    let cfg = match k2_core::tunnel::config::load() {
        Ok(c) => c,
        Err(e) => return Gate::Stop(format!("tunnel.json unreadable: {e}")),
    };
    if !cfg.enabled {
        return Gate::Stop("the tunnel was disabled".to_string());
    }
    if !cfg.auto_start {
        return Gate::Stop("auto-start was turned off".to_string());
    }
    if k2_core::tunnel::unpair::identity_released(&cfg).is_some() {
        return Gate::Stop("this device's tunnel identity was released".to_string());
    }
    if k2_core::tunnel::tunnel_status().running {
        return Gate::Running;
    }
    // A vault-held token that can't be read right now is not "unpaired":
    // keep retrying (the attempt fails softly until it reads).
    if !cfg.token_unreadable() && !cfg.is_connectable() {
        return Gate::Stop("config is no longer connectable (missing token/server)".to_string());
    }
    Gate::Go
}

static RETRY_ACTIVE: AtomicBool = AtomicBool::new(false);

/// The one-per-process auto-start retry slot; released on drop.
pub(crate) struct RetryClaim(());

impl RetryClaim {
    /// `None` when a retry loop already runs (never stack retries).
    pub(crate) fn try_claim() -> Option<Self> {
        RETRY_ACTIVE
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .ok()
            .map(|_| RetryClaim(()))
    }
}

impl Drop for RetryClaim {
    fn drop(&mut self) {
        RETRY_ACTIVE.store(false, Ordering::SeqCst);
    }
}

/// Start the tunnel on `daemon_port` (blocking pool), retrying on the
/// production backoff until it runs or the owner no longer wants it.
pub(crate) async fn start_with_retry(daemon_port: u16) {
    let Some(_claim) = RetryClaim::try_claim() else {
        log_debug!("[daemon/tunnel] auto-start retry already running — not starting another");
        return;
    };
    let attempt = || async move {
        match tokio::task::spawn_blocking(move || {
            k2_core::tunnel::start_tunnel(None, daemon_port)
        })
        .await
        {
            Ok(Ok(st)) => Ok(format!("running={} url={:?}", st.running, st.public_url)),
            Ok(Err(e)) => Err(e),
            Err(e) => Err(format!("start task panicked/cancelled: {e}")),
        }
    };
    match retry_until_started(&AUTOSTART_BACKOFF, attempt, autostart_gate).await {
        RetryOutcome::Started { attempts, detail } => {
            if attempts > 1 {
                log_debug!("[daemon/tunnel] auto-started on attempt {attempts}: {detail}");
            } else {
                log_debug!("[daemon/tunnel] auto-started: {detail}");
            }
        }
        RetryOutcome::AlreadyRunning { attempts } => log_debug!(
            "[daemon/tunnel] auto-start retry done after {attempts} failed attempt(s): the \
             tunnel is running (started elsewhere)"
        ),
        RetryOutcome::Stopped { attempts, reason } => log_debug!(
            "[daemon/tunnel] auto-start retry stopped after {attempts} failed attempt(s): {reason}"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn backoff_steps_then_caps() {
        let s = &AUTOSTART_BACKOFF;
        let got: Vec<u64> = (1..=8).map(|n| backoff_step(s, n).as_secs()).collect();
        assert_eq!(got, vec![15, 30, 60, 120, 300, 300, 300, 300]);
        assert_eq!(backoff_step(s, 0).as_secs(), 15, "0 failures is treated as 1");
        assert_eq!(backoff_step(&[], 3), Duration::ZERO);
    }

    /// (d) The real driver waits exactly 15 s, 30 s, 1 m, 2 m, 5 m, 5 m
    /// between failed attempts (paused tokio clock, no real waiting).
    #[tokio::test(start_paused = true)]
    async fn retry_waits_follow_the_backoff() {
        let t0 = tokio::time::Instant::now();
        let at: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
        let at2 = at.clone();
        let outcome = retry_until_started(
            &AUTOSTART_BACKOFF,
            move || {
                let at = at2.clone();
                async move {
                    let mut v = at.lock().unwrap();
                    v.push(t0.elapsed().as_secs());
                    if v.len() < 7 {
                        Err("broker down".to_string())
                    } else {
                        Ok("up".to_string())
                    }
                }
            },
            || Gate::Go,
        )
        .await;
        assert_eq!(
            outcome,
            RetryOutcome::Started { attempts: 7, detail: "up".to_string() }
        );
        // Attempt times: 0, +15, +30, +60, +120, +300, +300.
        assert_eq!(*at.lock().unwrap(), vec![0, 15, 45, 105, 225, 525, 825]);
    }

    /// The gate is asked after every failure; `Running` ends the retries
    /// without another attempt.
    #[tokio::test(start_paused = true)]
    async fn retry_ends_when_the_tunnel_is_running() {
        let attempts = Arc::new(Mutex::new(0u32));
        let a2 = attempts.clone();
        let outcome = retry_until_started(
            &AUTOSTART_BACKOFF,
            move || {
                let a = a2.clone();
                async move {
                    *a.lock().unwrap() += 1;
                    Err::<String, _>("frps refused".to_string())
                }
            },
            || Gate::Running,
        )
        .await;
        assert_eq!(outcome, RetryOutcome::AlreadyRunning { attempts: 1 });
        assert_eq!(*attempts.lock().unwrap(), 1);
    }

    /// (c) Disabling the tunnel stops the retries: the production gate
    /// re-reads tunnel.json, and no attempt runs after `enabled: false`.
    #[tokio::test(start_paused = true)]
    async fn no_retry_after_the_tunnel_is_disabled() {
        let _home = crate::test_support::TempHome::new();
        let _air = k2_core::test_env::EnvVar::remove("K2_AIRGAP");
        k2_core::tunnel::config::save(&k2_core::tunnel::config::TunnelConfig {
            token: "tok".to_string(),
            subdomain: "rosson".to_string(),
            server_addr: "relay.example".to_string(),
            auto_start: true,
            enabled: true,
            ..Default::default()
        })
        .expect("seed tunnel.json");
        assert_eq!(autostart_gate(), Gate::Go, "a wanted, stopped tunnel retries");

        let attempts = Arc::new(Mutex::new(0u32));
        let a2 = attempts.clone();
        let outcome = retry_until_started(
            &AUTOSTART_BACKOFF,
            move || {
                let a = a2.clone();
                async move {
                    let n = {
                        let mut g = a.lock().unwrap();
                        *g += 1;
                        *g
                    };
                    if n == 2 {
                        // The owner disables the tunnel while we back off.
                        k2_core::tunnel::config::update(|c| c.enabled = false)
                            .expect("disable");
                    }
                    Err::<String, _>("cert broker: issuance_failed".to_string())
                }
            },
            autostart_gate,
        )
        .await;
        assert_eq!(
            outcome,
            RetryOutcome::Stopped {
                attempts: 2,
                reason: "the tunnel was disabled".to_string()
            }
        );
        assert_eq!(*attempts.lock().unwrap(), 2, "no attempt may run after disable");

        // Auto-start off stops it too.
        k2_core::tunnel::config::update(|c| {
            c.enabled = true;
            c.auto_start = false;
        })
        .expect("auto_start off");
        assert_eq!(autostart_gate(), Gate::Stop("auto-start was turned off".to_string()));
    }

    #[test]
    fn retries_never_stack() {
        let first = RetryClaim::try_claim().expect("first claim");
        assert!(RetryClaim::try_claim().is_none(), "a second retry loop must not start");
        drop(first);
        let again = RetryClaim::try_claim().expect("claim frees on drop");
        drop(again);
    }
}
