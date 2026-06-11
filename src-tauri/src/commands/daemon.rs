//! Tauri command surface for the k2so-daemon.
//!
//! Exposes a tiny, frontend-facing view of the persistent-agent daemon's
//! lifecycle — enough for a Settings panel to show "daemon: running
//! (v0.33.0-dev, uptime 4m)" or "daemon: not installed" without the
//! frontend needing to know how the daemon is discovered or
//! authenticated. Wraps `crate::daemon_client::DaemonClient`.
//!
//! **Lifecycle commands** (`daemon_install` / `daemon_uninstall` /
//! `daemon_restart`) wrap `k2_core::wake` so the Settings pane can
//! install or remove the launch agent without the frontend knowing
//! `launchctl` exists. They all delegate to the same `DaemonPlist`
//! shape used by the first-launch migration in `lib.rs::setup()`, so
//! there's a single source of truth for what "canonical plist" means.

use std::path::PathBuf;
use std::process::Command;

use serde::Serialize;

use crate::daemon_client::{DaemonClient, DaemonStatus};

/// Shape returned to the frontend. Distinct variants rather than
/// Option<DaemonStatus> so the renderer can distinguish "daemon not
/// installed" (port file missing) from "daemon installed but crashed"
/// (port file stale).
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum DaemonStatusResponse {
    /// Daemon is reachable and authenticated; status payload included.
    Running {
        version: String,
        uptime_secs: u64,
        pid: u32,
        port: u16,
    },
    /// `~/.k2so/daemon.port` or `~/.k2so/daemon.token` not found. The
    /// install migration hasn't run yet, or the user uninstalled the
    /// plist.
    NotInstalled { reason: String },
    /// Files exist but the daemon didn't answer — almost certainly
    /// crashed between launchd restarts. Tauri could offer a "re-install
    /// daemon" button when this state appears.
    Unreachable { reason: String },
}

/// Returns the daemon's current state for the frontend Settings panel.
/// Never returns `Err` — instead encodes each failure mode in the
/// response variant so the frontend can render the right UI without
/// splitting try/catch.
#[tauri::command]
pub fn daemon_status() -> DaemonStatusResponse {
    let client = match DaemonClient::try_connect() {
        Ok(c) => c,
        Err(e) => return DaemonStatusResponse::NotInstalled { reason: e },
    };
    match client.status() {
        Ok(DaemonStatus {
            version,
            uptime_secs,
            pid,
            port,
        }) => DaemonStatusResponse::Running {
            version,
            uptime_secs,
            pid,
            port,
        },
        Err(e) => DaemonStatusResponse::Unreachable { reason: e },
    }
}

/// Locate the `k2so-daemon` binary bundled next to the current Tauri
/// executable. Matches the search path used by the first-launch
/// migration so Install / Reinstall operations agree on which binary
/// the plist should point at. Returns `Err` if the binary isn't
/// bundled (common in dev) or if we can't read the current exe path.
///
/// Pure path resolution + the `exists()` check live in
/// `k2_core::daemon_lifecycle::bundled_daemon_path`; this wrapper
/// adds the actual filesystem touch (Tauri context only) so a K2
/// Connect host can compute the same hypothetical path without
/// performing the FS probe.
fn locate_bundled_daemon() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("locate Tauri binary: {e}"))?;
    let candidate = k2_core::daemon_lifecycle::bundled_daemon_path(&exe)
        .ok_or_else(|| "Tauri binary has no parent dir".to_string())?;
    if !candidate.exists() {
        return Err(format!(
            "k2so-daemon not found at {} — dev builds don't bundle it; run from a release DMG",
            candidate.display()
        ));
    }
    Ok(candidate)
}

/// Install the launch agent plist and load it. Idempotent — an
/// existing loaded plist with the same label is unloaded first. Safe
/// to call as "reinstall" to repair a bad plist or a stale binary
/// path after the user moves K2SO.app.
///
/// Returns the plist path on success so the UI can show it in a
/// "View plist" affordance.
#[tauri::command]
pub fn daemon_install() -> Result<String, String> {
    let daemon_bin = locate_bundled_daemon()?;
    let plist = k2_core::wake::DaemonPlist::canonical(daemon_bin);
    k2_core::wake::install(&plist)
        .map(|p| p.to_string_lossy().to_string())
}

/// Unload the launch agent and delete the plist file. Does NOT delete
/// the user's `~/.k2so/` data directory — that stays across install
/// cycles on purpose. Also removes the `daemon.port` / `daemon.token`
/// files so the frontend's `daemon_status` immediately reports
/// `NotInstalled` instead of `Unreachable`.
#[tauri::command]
pub fn daemon_uninstall() -> Result<(), String> {
    // Uninstall is always safe — the plist might already be gone,
    // the daemon might never have been installed, etc. Any of those
    // paths are success. The only real failure is "can't locate
    // ~/Library/LaunchAgents" which `wake::uninstall` surfaces.
    //
    // We don't need the bundled binary path for uninstall — the
    // canonical plist builder only uses `program` for `write()`,
    // and `uninstall()` only cares about `label` + `plist_path()`.
    // Pass a placeholder so we don't fail on missing binary in dev.
    let plist = k2_core::wake::DaemonPlist::canonical(PathBuf::from("/nonexistent-uninstall"));
    k2_core::wake::uninstall(&plist)?;

    // Best-effort cleanup of the port/token/log files. Missing files
    // are fine.
    if let Some(dir) = dirs::home_dir().map(|h| h.join(".k2")) {
        for f in &["daemon.port", "daemon.token"] {
            let _ = std::fs::remove_file(dir.join(f));
        }
    }
    Ok(())
}

/// Restart the running daemon in-place without reinstalling the
/// plist. Uses `launchctl kickstart -k gui/<uid>/<label>` which sends
/// SIGTERM to the running daemon and lets launchd respawn it (because
/// `KeepAlive: true`). Preferred over unload+load because the plist
/// config stays untouched.
///
/// Internal-call-friendly version (no Tauri command attribute) used by
/// the version-mismatch auto-restart path in `lib.rs`. The
/// `#[tauri::command]` wrapper just delegates to this.
///
/// Arg vector construction lives in
/// `k2_core::daemon_lifecycle::launchctl_kickstart_args` so K2
/// Connect (which runs the same kickstart via SSH on a remote host)
/// produces the same exact arg shape.
pub fn kickstart_daemon() -> Result<(), String> {
    let uid = unsafe { libc::getuid() };
    let args = k2_core::daemon_lifecycle::launchctl_kickstart_args(uid);
    let out = Command::new("launchctl")
        .args(&args)
        .output()
        .map_err(|e| format!("launchctl kickstart: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        // "Could not find" means the plist isn't loaded — not an
        // error state from the user's perspective; they wanted a
        // restart, the daemon wasn't running, nothing to do.
        if stderr.contains("Could not find") {
            return Ok(());
        }
        return Err(format!(
            "launchctl kickstart failed (exit {}): {}",
            out.status.code().unwrap_or(-1),
            stderr.trim(),
        ));
    }
    Ok(())
}

#[tauri::command]
pub fn daemon_restart() -> Result<(), String> {
    kickstart_daemon()
}

/// Return the path to the daemon's stdout log file, regardless of
/// whether the daemon is currently running. The frontend uses this
/// to reveal the file in Finder or tail it via a child process.
#[tauri::command]
pub fn daemon_log_path() -> Result<String, String> {
    let dir = dirs::home_dir()
        .ok_or_else(|| "home dir unavailable".to_string())?
        .join(".k2");
    Ok(dir.join("daemon.stdout.log").to_string_lossy().to_string())
}

/// Return the last N lines of the daemon's stdout log. Defaults to
/// the tail of the file if it's shorter than `lines`.
#[tauri::command]
pub fn daemon_log_tail(lines: Option<u32>) -> Result<String, String> {
    let lines = lines.unwrap_or(200).clamp(1, 5000) as usize;
    let path_str = daemon_log_path()?;
    let path = PathBuf::from(&path_str);
    if !path.exists() {
        return Ok(String::new());
    }
    let content = std::fs::read_to_string(&path)
        .map_err(|e| format!("read {}: {e}", path.display()))?;
    let collected: Vec<&str> = content.lines().collect();
    let start = collected.len().saturating_sub(lines);
    Ok(collected[start..].join("\n"))
}

/// Phase 4.5 I1 — expose the daemon's port + auth token to the
/// renderer so React can compose the Session Stream WebSocket URL
/// (`ws://127.0.0.1:<port>/cli/sessions/subscribe?session=<uuid>&token=<token>`).
///
/// Reads `~/.k2so/heartbeat.port` + `~/.k2so/heartbeat.token` —
/// the Phase 4 H7 files the daemon eagerly writes on startup.
/// Intentionally reads the `heartbeat.*` pair (the public CLI-
/// facing address) rather than `daemon.*` (the Tauri-internal
/// pair) so future thin clients on other devices use the same
/// discovery convention.
///
/// Returns `NotInstalled { reason }` when either file is missing
/// — same "tri-state" shape as `daemon_status` so the renderer
/// can mirror the Settings pane's UX for daemon-unavailable.
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum DaemonWsUrlResponse {
    /// Port + token both readable; renderer can compose the URL.
    Available { port: u16, token: String },
    /// One or both files missing — daemon not running / not
    /// installed. Matches `daemon_status`'s NotInstalled variant.
    NotInstalled { reason: String },
}

#[tauri::command]
pub fn daemon_ws_url() -> DaemonWsUrlResponse {
    let Some(home) = dirs::home_dir() else {
        return DaemonWsUrlResponse::NotInstalled {
            reason: "home directory unavailable".to_string(),
        };
    };
    daemon_ws_url_in(&home)
}

/// Resolve the WebSocket URL response from an explicit home directory.
///
/// Split out from `daemon_ws_url()` so tests can point the lookup at a
/// temp dir they fully control rather than the live `~/.k2so`. The real
/// directory can be mutated by a running agent-heartbeat daemon between
/// a test's existence-check and its assertion (TOCTOU), which made the
/// old `ws_url_not_installed_when_files_missing` test flaky.
fn daemon_ws_url_in(home: &std::path::Path) -> DaemonWsUrlResponse {
    let port_path = home.join(".k2so/heartbeat.port");
    let token_path = home.join(".k2so/heartbeat.token");

    let port_raw = match std::fs::read_to_string(&port_path) {
        Ok(s) => s,
        Err(e) => {
            return DaemonWsUrlResponse::NotInstalled {
                reason: format!("read {}: {e}", port_path.display()),
            };
        }
    };
    let port: u16 = match port_raw.trim().parse() {
        Ok(p) => p,
        Err(e) => {
            return DaemonWsUrlResponse::NotInstalled {
                reason: format!("parse port {:?}: {e}", port_raw.trim()),
            };
        }
    };

    let token = match std::fs::read_to_string(&token_path) {
        Ok(s) => s.trim().to_string(),
        Err(e) => {
            return DaemonWsUrlResponse::NotInstalled {
                reason: format!("read {}: {e}", token_path.display()),
            };
        }
    };
    if token.is_empty() {
        return DaemonWsUrlResponse::NotInstalled {
            reason: "token file empty".to_string(),
        };
    }

    DaemonWsUrlResponse::Available { port, token }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Meaty integration lives in scripts/ — these tests cover the pure
    // Serialize shape so frontend contract changes are loud at build.
    #[test]
    fn running_variant_serializes_with_state_tag() {
        let r = DaemonStatusResponse::Running {
            version: "0.33.0-dev".to_string(),
            uptime_secs: 42,
            pid: 1234,
            port: 58211,
        };
        let json = serde_json::to_string(&r).expect("serialize");
        assert!(json.contains("\"state\":\"running\""), "got: {json}");
        assert!(json.contains("\"version\":\"0.33.0-dev\""), "got: {json}");
        assert!(json.contains("\"uptime_secs\":42"), "got: {json}");
    }

    #[test]
    fn not_installed_variant_serializes_with_state_tag() {
        let r = DaemonStatusResponse::NotInstalled {
            reason: "port file missing".to_string(),
        };
        let json = serde_json::to_string(&r).expect("serialize");
        assert!(json.contains("\"state\":\"not_installed\""), "got: {json}");
        assert!(json.contains("\"reason\":\"port file missing\""), "got: {json}");
    }

    #[test]
    fn unreachable_variant_serializes_with_state_tag() {
        let r = DaemonStatusResponse::Unreachable {
            reason: "connection refused".to_string(),
        };
        let json = serde_json::to_string(&r).expect("serialize");
        assert!(json.contains("\"state\":\"unreachable\""), "got: {json}");
    }

    #[test]
    fn ws_url_available_serializes_with_port_and_token() {
        let r = DaemonWsUrlResponse::Available {
            port: 58211,
            token: "abc123".to_string(),
        };
        let json = serde_json::to_string(&r).expect("serialize");
        assert!(json.contains("\"state\":\"available\""), "got: {json}");
        assert!(json.contains("\"port\":58211"), "got: {json}");
        assert!(json.contains("\"token\":\"abc123\""), "got: {json}");
    }

    #[test]
    fn ws_url_not_installed_when_files_missing() {
        // Point the lookup at a fresh temp dir we fully control, so the
        // result can't depend on the live `~/.k2so` — a running
        // agent-heartbeat daemon can create/refresh `heartbeat.{port,token}`
        // between an existence-check and the assertion (TOCTOU), which is
        // exactly the flake this test used to hit. A brand-new temp dir
        // has no `.k2so/heartbeat.*`, so `daemon_ws_url_in` must report
        // NotInstalled deterministically regardless of any live daemon.
        let unique = format!(
            "k2so-ws-url-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let temp_home = std::env::temp_dir().join(unique);
        std::fs::create_dir_all(&temp_home).expect("create temp home");

        let result = daemon_ws_url_in(&temp_home);

        // Clean up before asserting so a panic doesn't leak the temp dir.
        let _ = std::fs::remove_dir_all(&temp_home);

        match result {
            DaemonWsUrlResponse::NotInstalled { .. } => {}
            other => panic!("expected NotInstalled, got {other:?}"),
        }
    }

    #[test]
    fn daemon_status_with_no_files_returns_not_installed() {
        // `daemon_status()` reads ~/.k2so/daemon.port. If it's not
        // there (common in dev — nobody's installed the plist), the
        // command should quietly return NotInstalled instead of
        // panicking or returning an Err.
        let k2so_dir = dirs::home_dir().unwrap().join(".k2");
        let port_file = k2so_dir.join("daemon.port");
        let token_file = k2so_dir.join("daemon.token");
        // Only run if neither file happens to exist in the dev env.
        if port_file.exists() || token_file.exists() {
            eprintln!("[test] daemon files present; skipping NotInstalled assertion");
            return;
        }
        match daemon_status() {
            DaemonStatusResponse::NotInstalled { .. } => {}
            other => panic!("expected NotInstalled, got {other:?}"),
        }
    }
}
