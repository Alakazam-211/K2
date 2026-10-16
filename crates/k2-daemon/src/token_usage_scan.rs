//! Token-ledger scans. Full pass at boot and every 15 minutes. Idle
//! scans one workspace (Claude hash dir, Grok encoded dir, today's
//! matching Codex rollouts, slot homes included) — not every transcript
//! tree, and not the renderer's 5-second idle.
//!
//! Usage v2: idle notes are debounced per workspace with a **trailing**
//! run (B13), so the last turn of a quick burst is read within the
//! debounce, not at the next full pass. A scan's newest Codex
//! `rate_limits` become live limits ([`crate::usage_live`]). The full
//! pass also refreshes the cost table once a day (off in air-gap).

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::session_events::SessionEvent;

static IDLE_TX: OnceLock<mpsc::UnboundedSender<String>> = OnceLock::new();

pub const IDLE_DEBOUNCE: Duration = Duration::from_secs(20);

/// A session's lead went idle (prd-daemon-activity-and-thread-working-v1
/// A29: called by `activity_events` on every activity-store turn end).
/// Unregister is observed on the bus inside [`spawn`].
pub fn note_idle_workspace(workspace_path: &str) {
    if workspace_path.is_empty() {
        return;
    }
    if let Some(tx) = IDLE_TX.get() {
        let _ = tx.send(workspace_path.to_string());
    }
}

fn after_scan(what: &str, stats: &k2_core::token_usage::ScanStats) {
    k2_core::log_debug!(
        "[token-usage] {what} seen={} read={} skipped={} rows={}",
        stats.files_seen,
        stats.files_read,
        stats.files_skipped,
        stats.rows
    );
    for seen in &stats.codex_limits {
        crate::usage_live::ingest_codex_rollout(seen);
    }
    // A pass that read at least one file may have written new rows; nudge
    // the page. A skip-only pass changes nothing.
    if stats.files_read > 0 {
        crate::session_events::emit_token_usage_changed();
    }
}

fn run_full() {
    match k2_core::token_usage::scan_host() {
        Ok(stats) => after_scan("full scan", &stats),
        Err(e) => k2_core::log_debug!("[token-usage] full scan failed: {e}"),
    }
    refresh_prices_if_due();
}

fn run_workspace(cwd: String) {
    match k2_core::token_usage::scan_workspace_host(&cwd) {
        Ok(stats) => after_scan(&format!("idle scan cwd={cwd}"), &stats),
        Err(e) => k2_core::log_debug!("[token-usage] idle scan failed: {e}"),
    }
}

/// Once a day: fetch LiteLLM's public price table and keep the trimmed
/// copy (`~/.k2/usage/prices.json`). A failure keeps the old table.
fn refresh_prices_if_due() {
    if k2_core::airgap::enabled() || std::env::var_os("K2_USAGE_PRICES_FETCH").is_some_and(|v| v == "0") {
        return;
    }
    let now = chrono::Utc::now();
    if !k2_core::usage_prices::refresh_due(now) {
        return;
    }
    let body = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .and_then(|c| c.get(k2_core::usage_prices::LITELLM_URL).send())
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.text());
    match body {
        Ok(text) => match k2_core::usage_prices::store_refreshed(&text, now) {
            Ok(n) => k2_core::log_debug!("[usage-prices] refreshed: {n} priced models"),
            Err(e) => k2_core::log_debug!("[usage-prices] refresh rejected: {e}"),
        },
        Err(e) => k2_core::log_debug!("[usage-prices] refresh failed: {e}"),
    }
}

/// Per-workspace trailing debounce: the first note runs at once; notes
/// inside the window mark it dirty, and one more scan runs when the
/// window closes (B13).
#[derive(Debug, Default)]
pub struct Debounce {
    last_run: HashMap<String, Instant>,
    pending: HashMap<String, Instant>,
}

impl Debounce {
    /// A note for `cwd` at `now`: `true` = scan now.
    pub fn note(&mut self, cwd: &str, now: Instant) -> bool {
        match self.last_run.get(cwd) {
            Some(prev) if now.duration_since(*prev) < IDLE_DEBOUNCE => {
                self.pending.insert(cwd.to_string(), *prev + IDLE_DEBOUNCE);
                false
            }
            _ => {
                self.last_run.insert(cwd.to_string(), now);
                self.pending.remove(cwd);
                true
            }
        }
    }

    /// The trailing runs due at `now` (each starts a new window).
    pub fn due(&mut self, now: Instant) -> Vec<String> {
        let ready: Vec<String> = self.pending.iter().filter(|(_, at)| **at <= now).map(|(k, _)| k.clone()).collect();
        for k in &ready {
            self.pending.remove(k);
            self.last_run.insert(k.clone(), now);
        }
        ready
    }

    /// When the next trailing run is due.
    pub fn next_due(&self) -> Option<Instant> {
        self.pending.values().min().copied()
    }
}

fn scan_workspace_soon(cwd: String) {
    tokio::spawn(async move {
        let _ = tokio::task::spawn_blocking(move || run_workspace(cwd)).await;
    });
}

pub fn spawn() {
    let (tx, mut notes) = mpsc::unbounded_channel();
    let _ = IDLE_TX.set(tx);

    tokio::spawn(async move {
        loop {
            let _ = tokio::task::spawn_blocking(run_full).await;
            tokio::time::sleep(k2_core::token_usage::FULL_SCAN_INTERVAL).await;
        }
    });

    tokio::spawn(async move {
        let mut events = crate::session_events::subscribe();
        let mut debounce = Debounce::default();
        loop {
            let wake = debounce.next_due();
            let cwd = tokio::select! {
                ev = events.recv() => match ev {
                    Ok(SessionEvent::SessionRemoved { workspace_path, .. }) => Some(workspace_path),
                    Ok(_) => None,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => None,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                },
                note = notes.recv() => note,
                _ = async {
                    match wake {
                        Some(at) => tokio::time::sleep_until(at).await,
                        None => std::future::pending::<()>().await,
                    }
                } => None,
            };
            let now = Instant::now();
            for due in debounce.due(now) {
                scan_workspace_soon(due);
            }
            let Some(cwd) = cwd.filter(|c| !c.is_empty()) else { continue };
            if debounce.note(&cwd, now) {
                scan_workspace_soon(cwd);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_debounce_runs_the_trailing_edge_once() {
        let t0 = Instant::now();
        let mut d = Debounce::default();
        assert!(d.note("/ws", t0), "the first note scans at once");
        assert!(!d.note("/ws", t0 + Duration::from_secs(5)), "inside the window: deferred");
        assert!(!d.note("/ws", t0 + Duration::from_secs(9)), "still one pending run");
        assert!(d.note("/other", t0 + Duration::from_secs(9)), "another workspace is independent");
        assert!(d.due(t0 + Duration::from_secs(19)).is_empty(), "not before the window closes");
        assert_eq!(d.next_due(), Some(t0 + IDLE_DEBOUNCE));
        assert_eq!(d.due(t0 + IDLE_DEBOUNCE), vec!["/ws".to_string()], "the trailing turn end is scanned (B13)");
        assert!(d.due(t0 + Duration::from_secs(60)).is_empty(), "exactly once");
        assert!(d.next_due().is_none());
        // The trailing run opened a new window.
        assert!(!d.note("/ws", t0 + IDLE_DEBOUNCE + Duration::from_secs(1)));
        assert!(d.note("/ws", t0 + IDLE_DEBOUNCE * 3));
    }
}
