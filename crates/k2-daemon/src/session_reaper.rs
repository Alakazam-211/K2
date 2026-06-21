//! Daemon-canonical connect-session reaper (K2 Connect #4, PRD
//! `.k2/prds/secure-persistent-connect-sessions.md` §1d).
//!
//! A single long-lived tokio task that evicts EXPIRED persisted K2 Connect
//! login sessions from `~/.k2/connect-sessions.json`, independent of any
//! client. This is the GH#22 lesson applied to sessions: the sweep that
//! enforces expiry must run on the DAEMON, not a client renderer — a
//! headless daemon (no webview attached) must still reap, and two clients
//! must never disagree about which sessions are live.
//!
//! Note the reaper is a hygiene backstop, NOT the security boundary:
//! `validate_session` already rejects (and lazily deletes) an expired or
//! revoked record on every call. The reaper bounds the on-disk record set
//! for sessions that are simply never presented again after they expire.
//!
//! Cadence is low-frequency (default 5 min) — expiry is a 30-day-scale
//! event, so there is no need for a tight loop. The first pass runs at
//! boot so a long-down daemon's accumulated expired records are cleared
//! promptly on restart.

use std::time::Duration;

use tokio::task::JoinHandle;

use k2_core::log_debug;

/// Reaper cadence. Overridable via `K2_SESSION_REAPER_TICK_SECS` for
/// tests/tuning. Default 300s (5 min): expiry is coarse-grained.
const TICK_INTERVAL_SECS: u64 = 300;

fn tick_interval() -> Duration {
    let secs = std::env::var("K2_SESSION_REAPER_TICK_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|&v| v > 0)
        .unwrap_or(TICK_INTERVAL_SECS);
    Duration::from_secs(secs)
}

/// Spawn the connect-session reaper as a background tokio task. Returns
/// the JoinHandle (the task exits when the runtime drops). Mirrors
/// `active_reaper::spawn`'s shape.
pub fn spawn() -> JoinHandle<()> {
    tokio::spawn(async move { run().await })
}

async fn run() {
    let tick = tick_interval();
    log_debug!("[session-reaper] started — tick={}s", tick.as_secs());
    loop {
        // The store read+write is sync IO under a process mutex — run it
        // off the async reactor on a blocking thread.
        let reaped = tokio::task::spawn_blocking(k2_core::connect_users::reap_expired_sessions)
            .await
            .unwrap_or(0);
        if reaped > 0 {
            log_debug!("[session-reaper] evicted {reaped} expired connect session(s)");
        }
        tokio::time::sleep(tick).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_interval_default_when_unset() {
        std::env::remove_var("K2_SESSION_REAPER_TICK_SECS");
        assert_eq!(tick_interval(), Duration::from_secs(TICK_INTERVAL_SECS));
    }

    #[test]
    fn tick_interval_env_override() {
        std::env::set_var("K2_SESSION_REAPER_TICK_SECS", "7");
        assert_eq!(tick_interval(), Duration::from_secs(7));
        std::env::remove_var("K2_SESSION_REAPER_TICK_SECS");
    }
}
