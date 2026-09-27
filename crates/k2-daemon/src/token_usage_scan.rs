//! Token-ledger scans. Full pass every 15 minutes after ready. Idle
//! scans one workspace (Claude hash dir, Grok encoded dir, today's
//! matching Codex rollouts) — not every transcript tree, and not the
//! renderer's 5-second idle.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;

use tokio::sync::mpsc;

use crate::session_events::SessionEvent;

static IDLE_TX: OnceLock<mpsc::UnboundedSender<String>> = OnceLock::new();

const IDLE_DEBOUNCE: Duration = Duration::from_secs(20);

/// Child exit / channel close. `SessionActivityChanged` idle and
/// unregister are observed on the bus inside [`spawn`].
pub fn note_idle_workspace(workspace_path: &str) {
    if workspace_path.is_empty() {
        return;
    }
    if let Some(tx) = IDLE_TX.get() {
        let _ = tx.send(workspace_path.to_string());
    }
}

fn run_full() {
    match k2_core::token_usage::scan_host() {
        Ok(stats) => {
            k2_core::log_debug!(
                "[token-usage] full scan seen={} read={} skipped={}",
                stats.files_seen,
                stats.files_read,
                stats.files_skipped
            );
            // A pass that read at least one file may have written new
            // turns; nudge the live log. A skip-only pass changes nothing.
            if stats.files_read > 0 {
                crate::session_events::emit_token_usage_changed();
            }
        }
        Err(e) => k2_core::log_debug!("[token-usage] full scan failed: {e}"),
    }
}

fn run_workspace(cwd: String) {
    match k2_core::token_usage::scan_workspace_host(&cwd) {
        Ok(stats) => {
            k2_core::log_debug!(
                "[token-usage] idle scan cwd={cwd} seen={} read={} skipped={}",
                stats.files_seen,
                stats.files_read,
                stats.files_skipped
            );
            if stats.files_read > 0 {
                crate::session_events::emit_token_usage_changed();
            }
        }
        Err(e) => k2_core::log_debug!("[token-usage] idle scan failed: {e}"),
    }
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
        let mut last: HashMap<String, tokio::time::Instant> = HashMap::new();
        loop {
            let cwd = tokio::select! {
                ev = events.recv() => match ev {
                    Ok(SessionEvent::SessionActivityChanged { status, workspace_path, .. })
                        if status == "idle" =>
                    {
                        Some(workspace_path)
                    }
                    Ok(SessionEvent::SessionRemoved { workspace_path, .. }) => Some(workspace_path),
                    Ok(_) => None,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => None,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                },
                note = notes.recv() => note,
            };
            let Some(cwd) = cwd else { continue };
            if cwd.is_empty() {
                continue;
            }
            let now = tokio::time::Instant::now();
            if let Some(prev) = last.get(&cwd) {
                if now.duration_since(*prev) < IDLE_DEBOUNCE {
                    continue;
                }
            }
            last.insert(cwd.clone(), now);
            tokio::spawn(async move {
                let _ = tokio::task::spawn_blocking(move || run_workspace(cwd)).await;
            });
        }
    });
}
