//! Tauri command surface for the k2so-daemon.
//!
//! Exposes a tiny, frontend-facing view of the persistent-agent daemon's
//! lifecycle — enough for a Settings panel to show "daemon: running
//! (v0.33.0-dev, uptime 4m)" or "daemon: not installed" without the
//! frontend needing to know how the daemon is discovered or
//! authenticated. Wraps `crate::daemon_client::DaemonClient`.
//!
//! **Lifecycle commands** (`daemon_install` / `daemon_uninstall` /
//! `daemon_restart`) wrap `k2_client::wake` so the Settings pane can
//! install or remove the launch agent without the frontend knowing
//! `launchctl` exists. They all delegate to the same `DaemonPlist`
//! shape used by the first-launch migration in `lib.rs::setup()`, so
//! there's a single source of truth for what "canonical plist" means.

use std::path::PathBuf;
#[cfg(target_os = "macos")]
use std::process::Command;

use serde::Serialize;

use crate::daemon_client::{off_main, DaemonClient, DaemonStatus};

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
    /// `~/.k2/daemon.port` or `~/.k2/daemon.token` not found. The
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
///
/// `async` + `off_main` (0.46.4): the `/status` call (10 s reqwest
/// timeout) used to run on the main thread and froze the window while the
/// daemon was slow. The answer is the same; a blocking-pool failure (it
/// panicked) reads as Unreachable rather than an Err.
#[tauri::command]
pub async fn daemon_status() -> DaemonStatusResponse {
    match off_main(|| Ok(daemon_status_blocking())).await {
        Ok(status) => status,
        Err(reason) => DaemonStatusResponse::Unreachable { reason },
    }
}

/// [`daemon_status`] on the calling thread, for callers that already run
/// off the main thread (the tray's refresh loop).
pub fn daemon_status_blocking() -> DaemonStatusResponse {
    status_from(DaemonClient::try_connect())
}

/// The status for a `.k2` dir (tests use a temp dir, never `~/.k2`).
#[cfg(test)]
fn daemon_status_in(k2_dir: &std::path::Path) -> DaemonStatusResponse {
    status_from(DaemonClient::try_connect_in(k2_dir))
}

fn status_from(client: Result<DaemonClient, String>) -> DaemonStatusResponse {
    let client = match client {
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
/// `k2_client::daemon_lifecycle::bundled_daemon_path`; this wrapper
/// adds the actual filesystem touch (Tauri context only) so a K2
/// Connect host can compute the same hypothetical path without
/// performing the FS probe.
fn locate_bundled_daemon() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("locate Tauri binary: {e}"))?;
    let candidate = k2_client::daemon_lifecycle::bundled_daemon_path(&exe)
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
///
/// `async`: launchctl and the plist write run on the blocking pool, not
/// the main thread.
#[tauri::command]
pub async fn daemon_install() -> Result<String, String> {
    off_main(daemon_install_blocking).await
}

fn daemon_install_blocking() -> Result<String, String> {
    let daemon_bin = locate_bundled_daemon()?;
    let plist = k2_client::wake::DaemonPlist::canonical(daemon_bin);
    let path = k2_client::wake::install(&plist)?;
    // Same GUI moment as the daemon plist. Not `k2 daemon install`.
    #[cfg(target_os = "macos")]
    crate::install_menu_bar_helper();
    Ok(path.to_string_lossy().to_string())
}

/// Unload the launch agent and delete the plist file. Does NOT delete
/// the user's `~/.k2/` data directory — that stays across install
/// cycles on purpose. Also removes the `daemon.port` / `daemon.token`
/// files so the frontend's `daemon_status` immediately reports
/// `NotInstalled` instead of `Unreachable`.
#[tauri::command]
pub async fn daemon_uninstall() -> Result<(), String> {
    off_main(daemon_uninstall_blocking).await
}

fn daemon_uninstall_blocking() -> Result<(), String> {
    // Uninstall is always safe — the plist might already be gone,
    // the daemon might never have been installed, etc. Any of those
    // paths are success. The only real failure is "can't locate
    // ~/Library/LaunchAgents" which `wake::uninstall` surfaces.
    //
    // We don't need the bundled binary path for uninstall — the
    // canonical plist builder only uses `program` for `write()`,
    // and `uninstall()` only cares about `label` + `plist_path()`.
    // Pass a placeholder so we don't fail on missing binary in dev.
    let plist = k2_client::wake::DaemonPlist::canonical(PathBuf::from("/nonexistent-uninstall"));
    k2_client::wake::uninstall(&plist)?;

    // Best-effort cleanup of the port/token/log files. Missing files
    // are fine.
    if let Some(dir) = k2_client::paths::home_dir().map(|h| h.join(".k2")) {
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
/// `k2_client::daemon_lifecycle::launchctl_kickstart_args` so K2
/// Connect (which runs the same kickstart via SSH on a remote host)
/// produces the same exact arg shape.
///
/// macOS-only: Windows/Linux have no launchd. Non-mac stubs succeed so
/// version-mismatch auto-restart does not hard-fail the thin client;
/// operators restart `k2-daemon` via service/CLI instead.
#[cfg(target_os = "macos")]
pub fn kickstart_daemon() -> Result<(), String> {
    let uid = unsafe { libc::getuid() };
    let args = k2_client::daemon_lifecycle::launchctl_kickstart_args(uid);
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

#[cfg(not(target_os = "macos"))]
pub fn kickstart_daemon() -> Result<(), String> {
    Ok(())
}

/// `async`: `launchctl kickstart -k` waits for the old daemon to stop, so
/// it runs on the blocking pool, not the main thread.
#[tauri::command]
pub async fn daemon_restart() -> Result<(), String> {
    off_main(kickstart_daemon).await
}

/// Return the path to the daemon's stdout log file, regardless of
/// whether the daemon is currently running. The frontend uses this
/// to reveal the file in Finder or tail it via a child process.
#[tauri::command]
pub fn daemon_log_path() -> Result<String, String> {
    let dir = k2_client::paths::home_dir()
        .ok_or_else(|| "home dir unavailable".to_string())?
        .join(".k2");
    Ok(dir.join("daemon.stdout.log").to_string_lossy().to_string())
}

/// Return the last N lines of the daemon's stdout log. Defaults to
/// the tail of the file if it's shorter than `lines`.
///
/// `async`: the log can be large; it is read on the blocking pool.
#[tauri::command]
pub async fn daemon_log_tail(lines: Option<u32>) -> Result<String, String> {
    off_main(move || log_tail_of(&PathBuf::from(daemon_log_path()?), lines)).await
}

fn log_tail_of(path: &std::path::Path, lines: Option<u32>) -> Result<String, String> {
    let lines = lines.unwrap_or(200).clamp(1, 5000) as usize;
    if !path.exists() {
        return Ok(String::new());
    }
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("read {}: {e}", path.display()))?;
    let collected: Vec<&str> = content.lines().collect();
    let start = collected.len().saturating_sub(lines);
    Ok(collected[start..].join("\n"))
}

/// Phase 4.5 I1 — expose the daemon's port + auth token to the
/// renderer so React can compose the Session Stream WebSocket URL
/// (`ws://127.0.0.1:<port>/cli/sessions/subscribe?session=<uuid>&token=<token>`).
///
/// Reads `~/.k2/heartbeat.port` + `~/.k2/heartbeat.token` —
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
    let Some(home) = k2_client::paths::home_dir() else {
        return DaemonWsUrlResponse::NotInstalled {
            reason: "home directory unavailable".to_string(),
        };
    };
    daemon_ws_url_in(&home)
}

/// Resolve the WebSocket URL response from an explicit home directory.
///
/// Split out from `daemon_ws_url()` so tests can point the lookup at a
/// temp dir they fully control rather than the live `~/.k2`. The real
/// directory can be mutated by a running agent-heartbeat daemon between
/// a test's existence-check and its assertion (TOCTOU), which made the
/// old `ws_url_not_installed_when_files_missing` test flaky.
fn daemon_ws_url_in(home: &std::path::Path) -> DaemonWsUrlResponse {
    let port_path = home.join(".k2/heartbeat.port");
    let token_path = home.join(".k2/heartbeat.token");

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
        // result can't depend on the live `~/.k2` — a running
        // agent-heartbeat daemon can create/refresh `heartbeat.{port,token}`
        // between an existence-check and the assertion (TOCTOU), which is
        // exactly the flake this test used to hit. A brand-new temp dir
        // has no `.k2/heartbeat.*`, so `daemon_ws_url_in` must report
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
        // A temp `.k2` with no daemon.port / daemon.token (never the live
        // ~/.k2, which a running daemon owns): NotInstalled, not a panic
        // or an Err.
        let dir = tempfile::tempdir().expect("tempdir");
        match daemon_status_in(dir.path()) {
            DaemonStatusResponse::NotInstalled { reason } => {
                assert!(reason.contains("daemon.port"), "got: {reason}")
            }
            other => panic!("expected NotInstalled, got {other:?}"),
        }
    }

    fn write_port_token(dir: &std::path::Path, port: u16) {
        std::fs::write(dir.join("daemon.port"), format!("{port}\n")).expect("port");
        std::fs::write(dir.join("daemon.token"), "tok\n").expect("token");
    }

    #[test]
    fn daemon_status_reports_running_from_the_status_route() {
        use crate::daemon_client::test_support::{pairs, FakeDaemon};
        let fake = FakeDaemon::serve(vec![(
            200,
            r#"{"version":"0.46.4","uptime_secs":7,"pid":42,"port":5}"#,
        )]);
        let port: u16 = fake.base.rsplit(':').next().expect("port").parse().expect("u16");
        let dir = tempfile::tempdir().expect("tempdir");
        write_port_token(dir.path(), port);
        let k2 = dir.path().to_path_buf();
        // Through off_main, as the command runs it.
        let got = tauri::async_runtime::block_on(off_main(move || Ok(daemon_status_in(&k2)))).expect("ran");
        match got {
            DaemonStatusResponse::Running { version, uptime_secs, pid, port: p } => {
                assert_eq!((version.as_str(), uptime_secs, pid, p), ("0.46.4", 7, 42, 5));
            }
            other => panic!("expected Running, got {other:?}"),
        }
        let seen = fake.next();
        assert_eq!((seen.method.as_str(), seen.path.as_str()), ("GET", "/status"));
        assert_eq!(seen.query, pairs(&[("token", "tok")]));
    }

    #[test]
    fn daemon_status_reports_unreachable_when_nothing_answers() {
        // Bind then drop a listener: the port is (almost surely) closed.
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
            l.local_addr().expect("addr").port()
        };
        let dir = tempfile::tempdir().expect("tempdir");
        write_port_token(dir.path(), port);
        match daemon_status_in(dir.path()) {
            DaemonStatusResponse::Unreachable { reason } => {
                assert!(reason.starts_with("daemon /status"), "got: {reason}")
            }
            other => panic!("expected Unreachable, got {other:?}"),
        }
    }

    #[test]
    fn log_tail_returns_the_last_lines_and_empty_for_no_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log = dir.path().join("daemon.stdout.log");
        assert_eq!(log_tail_of(&log, None), Ok(String::new()));
        std::fs::write(&log, "a\nb\nc\nd\n").expect("write log");
        let p = log.clone();
        let got = tauri::async_runtime::block_on(off_main(move || log_tail_of(&p, Some(2))));
        assert_eq!(got, Ok("c\nd".to_string()));
        // 0 clamps to 1 line.
        assert_eq!(log_tail_of(&log, Some(0)), Ok("d".to_string()));
    }
}
