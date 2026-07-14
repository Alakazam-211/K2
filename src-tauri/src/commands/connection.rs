//! 0.40.48 connection resilience — the OUT-OF-WEBVIEW connection arbiter.
//!
//! THE INCIDENT: after a remote K2 server reboots, WKWebView can keep
//! reusing a poisoned pooled HTTP/2 connection whose every request fails at
//! the tunnel edge with HTTP 404 ("no route found"). The connection is
//! transport-healthy, so the pool never evicts it and JS has no eviction
//! lever — the renderer's own recovery poll rides the same pool and can
//! never tell "host is down" from "my pool is poisoned".
//!
//! `remote_boot_probe` is the tiebreaker: a FRESH `reqwest` client per call
//! (its own OS-level socket, `http1_only` so it can't join any coalesced
//! h2 connection, never reused) hits the same `/boot-status` the renderer
//! polls. If this probe sees the daemon 'ready' while the webview's probes
//! keep failing, the pool is proven poisoned and ConnectionGate escalates
//! (auto-reload once, then a user-initiated `restart_app`).

use std::time::Duration;

use serde::Serialize;
use tauri::AppHandle;

/// Raw result of the arbiter probe. The RENDERER owns the verdict logic
/// (ConnectionGate's `arbiterProvesHostReady` parses `body` for
/// `phase === 'ready'`); this command only reports what the fresh socket
/// saw, so the pure decision rule stays unit-testable in TS.
#[derive(Debug, Serialize)]
pub struct BootProbeResult {
    /// HTTP status code the daemon (or the tunnel edge) answered with.
    pub status: u16,
    /// Raw response body — the daemon's `/boot-status` JSON on success.
    pub body: String,
}

/// GET `<scheme>://<authority>/boot-status` on a FRESH, never-pooled HTTP/1
/// connection. `Err` = the probe failed at the network level (refused, DNS,
/// timeout) — a genuine-outage signal, NOT proof of a webview wedge.
///
/// The authority mirrors the renderer's rule (kessel/daemon-ws.ts
/// `authority()`): a secure host on 443 omits the port (`rosson.k2.dev`,
/// not `rosson.k2.dev:443`); everything else carries it explicitly.
#[tauri::command]
pub async fn remote_boot_probe(
    hostname: String,
    port: u16,
    secure: bool,
) -> Result<BootProbeResult, String> {
    // Blocking reqwest matches the crate's existing HTTP story
    // (daemon_client.rs); spawn_blocking keeps it off the UI thread.
    tauri::async_runtime::spawn_blocking(move || {
        let client = reqwest::blocking::Client::builder()
            // A fresh client per call is the whole point: its connection
            // can never be the webview's poisoned one. http1_only() also
            // guarantees it can't coalesce onto a shared h2 connection at
            // the tunnel edge — the exact mechanism that wedged the webview.
            .http1_only()
            .timeout(Duration::from_secs(4))
            .build()
            .map_err(|e| format!("build probe client: {e}"))?;
        let scheme = if secure { "https" } else { "http" };
        let authority = if secure && port == 443 {
            hostname.clone()
        } else {
            format!("{hostname}:{port}")
        };
        let url = format!("{scheme}://{authority}/boot-status");
        let response = client
            .get(&url)
            .send()
            .map_err(|e| format!("probe {url}: {e}"))?;
        let status = response.status().as_u16();
        let body = response.text().unwrap_or_default();
        Ok(BootProbeResult { status, body })
    })
    .await
    .map_err(|e| format!("probe task join: {e}"))?
}

/// Restart the app — the wedged-pool escape hatch (ALWAYS user-initiated:
/// ConnectionGate's banner button invokes this; nothing auto-restarts).
///
/// macOS deliberately does NOT use Tauri's `AppHandle::restart()` — it
/// spawns the bare binary instead of the `.app` bundle and trips the Metal
/// destructor crash (see `settings::relaunch_via_open`, which exists for
/// exactly those reasons). We set RELAUNCH_MODE (so the close handler skips
/// its hard `_exit`) and ride that proven helper-script relaunch. Other
/// platforms use the stock restart.
#[tauri::command]
pub fn restart_app(app: AppHandle) {
    crate::RELAUNCH_MODE.store(true, std::sync::atomic::Ordering::Relaxed);
    #[cfg(target_os = "macos")]
    {
        crate::commands::settings::relaunch_via_open(app);
    }
    #[cfg(not(target_os = "macos"))]
    {
        app.restart();
    }
}
