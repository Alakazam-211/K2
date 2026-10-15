//! Chat-pass key maintenance tick (S28 §7.3), daemon-owned so a headless
//! daemon rotates on schedule with no client attached (GH#22 lesson).
//!
//! Every `MAINT_TICK_SECS` (env `K2_CHAT_KEYS_TICK_SECS` overrides): run
//! the keyring's plan — promote `next` after 30 days once it has been
//! published ≥ 10 min, make a missing `next`, retire keys 10 min after
//! their last use. The tick never creates a keyring: a box that never
//! serves the chat JWKS or mints a pass stays clean. Mints and JWKS
//! requests run the same maintenance inline, so a late tick never lets an
//! overdue key sign.

use std::time::Duration;

use tokio::task::JoinHandle;

use super::keyring::{self, KeyError};

pub const MAINT_TICK_SECS: u64 = 300;

pub fn tick_interval() -> Duration {
    let secs = std::env::var("K2_CHAT_KEYS_TICK_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|&v| v > 0)
        .unwrap_or(MAINT_TICK_SECS);
    Duration::from_secs(secs)
}

/// What one tick did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TickOutcome {
    /// No keyring on this box yet (nothing to do; never created here).
    NoKeyring,
    Unchanged,
    Changed,
    /// The vault is closed; try again next tick.
    VaultClosed,
    Failed(String),
}

/// One maintenance pass on `ring` at `now`.
pub fn tick_once(ring: &keyring::Keyring, now: i64) -> TickOutcome {
    match ring.state() {
        Ok(None) => return TickOutcome::NoKeyring,
        Ok(Some(_)) => {}
        Err(e) => return TickOutcome::Failed(e.to_string()),
    }
    match ring.maintain(now, false) {
        Ok(true) => TickOutcome::Changed,
        Ok(false) => TickOutcome::Unchanged,
        Err(e) => TickOutcome::Failed(e.to_string()),
    }
}

/// One pass on the daemon's keyring.
pub fn tick_global(now: i64) -> TickOutcome {
    match keyring::global() {
        Ok(ring) => tick_once(&ring, now),
        Err(KeyError::VaultClosed(_)) => TickOutcome::VaultClosed,
        Err(e) => TickOutcome::Failed(e.to_string()),
    }
}

/// Spawn the tick (next to `session_reaper::spawn()` in the boot path).
pub fn spawn() -> JoinHandle<()> {
    tokio::spawn(async move {
        let tick = tick_interval();
        keyring::klog(format!("maintenance tick every {}s", tick.as_secs()));
        let mut last_failure: Option<String> = None;
        loop {
            let out = tokio::task::spawn_blocking(|| tick_global(keyring::now()))
                .await
                .unwrap_or_else(|e| TickOutcome::Failed(format!("join: {e}")));
            match out {
                TickOutcome::Failed(e) => {
                    // Corruption repeats every tick; say it once.
                    if last_failure.as_deref() != Some(e.as_str()) {
                        keyring::klog(format!("maintenance failed: {e}"));
                        last_failure = Some(e);
                    }
                }
                _ => last_failure = None,
            }
            tokio::time::sleep(tick).await;
        }
    })
}
