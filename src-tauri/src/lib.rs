#[cfg(target_os = "macos")]
#[macro_use]
extern crate objc;

// `log_debug!` + `perf_timer!` + `perf_hist!` in scope across every child
// module. They live in k2-client (0.46.4: this crate no longer links
// k2-core; see Cargo.toml).
#[macro_use]
extern crate k2_client;

/// Flag to skip _exit(0) during relaunch (set by the frontend before process::relaunch)
static RELAUNCH_MODE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);


mod agent_hooks;
// The call's own window (pop out).
mod call_window;
mod commands;
mod tray;
// Editor discovery ("Open in …") lives in k2-client. Re-exported so
// `crate::editors::*` call sites keep working.
pub use k2_client::editors;
mod menu;
// `perf` lives in k2-client. Re-exported so `crate::perf_timer!` /
// `crate::perf_hist!` / `crate::perf::*` call sites keep working.
pub use k2_client::{perf, perf_hist, perf_timer};
// Phase 2 close-out (2026-05-23): `mod state` deleted. After Units 1-4
// reduced `AppState` to a single `watchers` field, the struct + the
// `.manage()` registration were pure ceremony around one
// `HashMap<String, RecommendedWatcher>`. Inlined into `watcher.rs`
// as a `LazyLock<Mutex<_>>` static so the only host-side process state
// lives next to the only consumer.
// Tauri-side HTTP client for the k2so-daemon. Routes state-mutating
// commands through the daemon's loopback HTTP instead of running them
// in-process.
mod daemon_client;
// Linux: restart the local k2-daemon user service when its version differs
// from the app's (the launchctl kickstart twin), with the gate's state.
mod linux_daemon;
// 0.46.4: the `agent_hook_sink` module (a Tauri AgentHookEventSink for
// k2-core's in-process `agent_hooks::emit`) is gone with the k2-core link.
// Every hook event the renderer listens to comes from the daemon's /events
// bus through `daemon_events.rs`; the one event this process raised itself
// (`k2so_chat_refresh_broadcast`) is emitted directly.
// Background subscriber for the daemon's /events WebSocket. Spawned in
// setup() once; reconnects forever so we survive daemon restarts.
mod daemon_events;
mod k2_app_window;
// Native voice calls (prd-k2-voice-rooms-v1 §14): Sew's plugin + K2's sign-in.
#[cfg(feature = "native-voice")]
mod native_voice;
mod watcher;
// Per-window renderer watchdog (Issue #6, Zen v2 UW32/UW56, 0.46.9 false reloads).
mod webview_watchdog;
mod window;
// macOS: field log for events vendored tao drops instead of aborting in
// `sendEvent:` (`~/.k2/client-event-faults.log`).
#[cfg(target_os = "macos")]
mod event_faults;

use tauri::Manager;

// 0.46.4: `prime_hook_config_from_daemon` removed. It copied the daemon's
// port + token into k2-core's `hook_config` for in-process Alacritty
// terminals; those moved to the daemon in Phase 2 Unit 3 and nothing in
// this process read `hook_config` since.

/// Detect a stale launchd-managed daemon on app startup and bounce it.
///
/// Background: the daemon is launchd-managed (`KeepAlive=true`), so a
/// drag-replace / AirDrop install of K2.app overwrites the binary on disk
/// while launchd keeps the OLD daemon process running with the deleted
/// inode — meaning a freshly-installed app talks to last-install's daemon
/// until reboot or Settings → Restart Daemon.
///
/// Two independent staleness signals (either is enough to kickstart):
/// 1. **Version string** mismatch (`daemon.version` ≠ app `CARGO_PKG_VERSION`).
/// 2. **On-disk binary newer than the running process** — same marketing
///    version (common for AirDrop test builds / hotfixes that keep
///    `0.40.x`) but the app bundle was replaced after the daemon started.
///    Without (2), `0.40.46`→`0.40.46` with new code never restarts and
///    you keep shipping `@` attribution / old federation bugs from the
///    live process.
///
/// Runs on a background thread because we must wait for the daemon to be reachable + we don't want
/// to block Tauri's setup hook on a synchronous HTTP round-trip. Polls
/// up to 10× at 500ms intervals; if the daemon stays unreachable we
/// log and bow out — bigger problem than version skew at that point.
///
/// On kickstart, the renderer-side ConnectionGate handles waiting +
/// "Connecting..." UX.
fn check_daemon_version_and_restart() {
    use std::time::Duration;
    std::thread::spawn(|| {
        let app_version = env!("CARGO_PKG_VERSION");
        for attempt in 0..10 {
            std::thread::sleep(Duration::from_millis(500));
            let client = match crate::daemon_client::DaemonClient::try_connect() {
                Ok(c) => c,
                Err(_) => continue,
            };
            let status = match client.status() {
                Ok(s) => s,
                Err(_) => continue,
            };
            let version_match = status.version == app_version;
            // Same-version drag-replace: disk binary mtime after process start.
            // Skip in debug — dev launches must not bounce the live daemon.
            let stale_binary = !cfg!(debug_assertions)
                && bundled_daemon_newer_than_running(status.uptime_secs);

            if version_match && !stale_binary {
                log_debug!(
                    "[version-check] daemon v{} matches app v{} and on-disk binary is not newer (attempt={})",
                    status.version,
                    app_version,
                    attempt
                );
                return;
            }

            // Dev builds (`tauri dev`) are always version-ahead of the
            // installed /Applications daemon, and the plist doesn't point
            // at the dev tree — a kickstart can never converge the
            // versions. It only bounces the user's live daemon (killing
            // PTY sessions) on every dev launch. Tolerate skew instead;
            // the release build keeps the drag-replace self-heal.
            if cfg!(debug_assertions) {
                log_debug!(
                    "[version-check] MISMATCH daemon=v{} app=v{} (attempt={}); dev build — tolerating skew, daemon left untouched",
                    status.version,
                    app_version,
                    attempt
                );
                return;
            }
            // Linux: systemd owns the daemon (user unit k2-daemon.service).
            // pacman/apt/dnf run as root and cannot restart a user unit, so
            // after a package upgrade the OLD daemon keeps answering and the
            // new app waits forever on the exact-version gate. Restart the
            // LOCAL unit ourselves and wait for the matching version
            // (linux_daemon.rs: loopback + same user + unit MainPID only;
            // explains drop-in overrides instead of editing them).
            // Windows NSIS: the bundled k2-daemon.exe sits next to k2.exe;
            // after an in-app update the OLD process is often still holding
            // the port while the NEW k2.exe waits forever in ConnectionGate
            // (exact version). Kill + respawn.
            #[cfg(target_os = "linux")]
            {
                log_debug!(
                    "[version-check] STALE daemon=v{} app=v{} version_match={} stale_binary={} (attempt={}); reconciling the k2-daemon user service",
                    status.version,
                    app_version,
                    version_match,
                    stale_binary,
                    attempt
                );
                if !version_match {
                    let report = crate::linux_daemon::reconcile(app_version);
                    log_debug!("[version-check] linux reconcile: {report:?}");
                }
                return;
            }
            #[cfg(windows)]
            {
                log_debug!(
                    "[version-check] STALE daemon=v{} app=v{} version_match={} stale_binary={} (attempt={}); stopping old k2-daemon and spawning bundled binary",
                    status.version,
                    app_version,
                    version_match,
                    stale_binary,
                    attempt
                );
                stop_bundled_daemon_processes();
                ensure_local_daemon_process();
                return;
            }
            #[cfg(target_os = "macos")]
            {
                if !version_match {
                    log_debug!(
                        "[version-check] MISMATCH daemon=v{} app=v{} (attempt={}); restarting daemon via launchctl kickstart",
                        status.version,
                        app_version,
                        attempt
                    );
                } else {
                    log_debug!(
                        "[version-check] STALE BINARY same version v{} but on-disk k2-daemon is newer than the running process (uptime={}s, attempt={}); restarting via launchctl kickstart",
                        status.version,
                        status.uptime_secs,
                        attempt
                    );
                }
                // #14: BEFORE kickstart, self-heal a plist whose baked-in
                // ProgramArguments[0] is stale/transient. Otherwise the
                // kickstart just respawns the SAME bad path and we never
                // converge. `heal_daemon_plist_program` is a no-op when the
                // recorded path is already correct, or when the current exe
                // is itself transient (can't be trusted to seed the plist).
                heal_daemon_plist_program();
                match crate::commands::daemon::kickstart_daemon() {
                    Ok(()) => log_debug!("[version-check] launchctl kickstart succeeded"),
                    Err(e) => log_debug!("[version-check] launchctl kickstart failed: {e}"),
                }
                return;
            }
        }
        log_debug!(
            "[version-check] daemon unreachable after 10 attempts; skipping version check"
        );
    });
}

/// Non-macOS: if the local daemon is not answering `/boot-status` on the
/// published port (or no port file exists), spawn the bundled `k2-daemon`
/// next to this exe and wait briefly for readiness.
///
/// macOS uses launchd (`ensure_loaded`); Windows has no service unit yet,
/// so without this the thin client sits on "connecting" forever against a
/// stale `~/.k2/daemon.port` (SYN_SENT). Best-effort: log + return if the
/// binary is missing (dev builds).
#[cfg(not(target_os = "macos"))]
fn ensure_local_daemon_process() {
    use std::time::Duration;

    if local_daemon_boot_ready() {
        log_debug!("[k2so] local daemon already ready (boot-status)");
        return;
    }

    let Some(daemon_bin) = std::env::current_exe()
        .ok()
        .and_then(|p| k2_client::daemon_lifecycle::bundled_daemon_path(&p))
        .filter(|p| p.exists())
    else {
        log_debug!(
            "[k2so] no bundled k2-daemon next to thin client — cannot auto-start (install Program Files layout or run k2-daemon manually)"
        );
        return;
    };

    log_debug!(
        "[k2so] local daemon not ready; spawning {}",
        daemon_bin.display()
    );

    let mut cmd = k2_client::bg_proc::command(&daemon_bin);
    if let Some(dir) = daemon_bin.parent() {
        cmd.current_dir(dir);
    }
    // Windows: start it under its keep-alive supervisor (launchd's
    // KeepAlive), as the logon Run value does.
    #[cfg(windows)]
    cmd.arg(k2_client::win_autostart::KEEP_ALIVE_ARG);
    // Windows: hide console (k2-daemon is CUI unless built with windows
    // subsystem) and start a new process group so closing the UI does not
    // tear the daemon down. Prefer CREATE_NO_WINDOW over DETACHED_PROCESS —
    // DETACHED can still allocate a console for CUI children in some paths.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(k2_client::bg_proc::CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    match cmd.spawn() {
        Ok(child) => log_debug!("[k2so] spawned k2-daemon pid={}", child.id()),
        Err(e) => {
            log_debug!("[k2so] failed to spawn k2-daemon: {e}");
            return;
        }
    }

    // Wait up to ~8s for /boot-status phase=ready (same port-file reuse
    // path the daemon uses on restart).
    for _ in 0..40 {
        std::thread::sleep(Duration::from_millis(200));
        if local_daemon_boot_ready() {
            log_debug!("[k2so] local daemon became ready after spawn");
            return;
        }
    }
    log_debug!("[k2so] local daemon spawn timed out waiting for boot-status ready");
}

/// After an app update that did NOT install: start again the daemon that
/// `stop_bundled_daemon_for_update` stopped (Windows). macOS never stops it
/// (launchd keeps it), so there is nothing to do there.
pub(crate) fn restart_daemon_after_failed_update() {
    #[cfg(not(target_os = "macos"))]
    ensure_local_daemon_process();
}

/// Kill local `k2-daemon.exe` and orphaned `frpc.exe` (Windows file lock
/// + version heal). Best-effort; never panics. macOS uses launchd.
pub fn stop_bundled_daemon_processes() {
    #[cfg(windows)]
    {
        for image in ["k2-daemon.exe", "frpc.exe"] {
            let _ = k2_client::bg_proc::command("taskkill")
                .args(["/IM", image, "/F", "/T"])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
        // Brief pause so the file lock drops before NSIS/overwrite/spawn.
        std::thread::sleep(std::time::Duration::from_millis(400));
        log_debug!("[k2so] stop_bundled_daemon_processes: taskkill k2-daemon.exe frpc.exe");
    }
}

/// True when `~/.k2/daemon.port` points at a live daemon that answers
/// `/boot-status` with HTTP 2xx (token not required for that route).
#[cfg(not(target_os = "macos"))]
fn local_daemon_boot_ready() -> bool {
    use std::time::Duration;

    let home = k2_client::paths::k2_home();
    let port_path = home.join("daemon.port");
    let Ok(raw) = std::fs::read_to_string(&port_path) else {
        return false;
    };
    let Ok(port) = raw.trim().parse::<u16>() else {
        return false;
    };
    let url = format!("http://127.0.0.1:{port}/boot-status");
    let Ok(client) = reqwest::blocking::Client::builder()
        .timeout(Duration::from_millis(800))
        .build()
    else {
        return false;
    };
    match client.get(&url).send() {
        Ok(resp) if resp.status().is_success() => {
            // Prefer phase=ready when present; any 2xx means something is
            // listening and the client can progress past SYN_SENT.
            match resp.text() {
                Ok(body) if body.contains("\"phase\"") => {
                    body.contains("\"phase\":\"ready\"")
                        || body.contains("\"phase\": \"ready\"")
                }
                Ok(_) => true,
                Err(_) => true,
            }
        }
        _ => false,
    }
}

/// True when the bundled `k2-daemon` on disk was written **after** the
/// currently running daemon process started (plus a small clock skew).
///
/// That's the drag-replace / AirDrop case: same version string, new
/// bytes on disk, old process still holding the previous inode.
fn bundled_daemon_newer_than_running(uptime_secs: u64) -> bool {
    use std::time::SystemTime;

    let Some(daemon_bin) = std::env::current_exe()
        .ok()
        .and_then(|p| k2_client::daemon_lifecycle::bundled_daemon_path(&p))
        .filter(|p| p.exists())
    else {
        return false;
    };
    // Never treat a DMG / AppTranslocation path as authoritative — same
    // guard as the plist install path (#14).
    if k2_client::daemon_lifecycle::is_transient_exe_location(&daemon_bin) {
        return false;
    }
    let Ok(mtime) = std::fs::metadata(&daemon_bin).and_then(|m| m.modified()) else {
        return false;
    };
    is_running_daemon_stale_vs_disk(mtime, uptime_secs, SystemTime::now())
}

/// Pure clock compare used by [`bundled_daemon_newer_than_running`].
/// `binary_mtime` after `now - uptime` by more than 2s ⇒ process predates
/// the on-disk binary ⇒ restart.
fn is_running_daemon_stale_vs_disk(
    binary_mtime: std::time::SystemTime,
    uptime_secs: u64,
    now: std::time::SystemTime,
) -> bool {
    use std::time::{Duration, SystemTime};
    let process_started = now
        .checked_sub(Duration::from_secs(uptime_secs))
        .unwrap_or(SystemTime::UNIX_EPOCH);
    match binary_mtime.duration_since(process_started) {
        Ok(d) if d > Duration::from_secs(2) => true,
        _ => false,
    }
}

#[cfg(test)]
mod daemon_stale_binary_tests {
    use super::is_running_daemon_stale_vs_disk;
    use std::time::{Duration, SystemTime};

    #[test]
    fn same_version_disk_newer_than_process_is_stale() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        // Process has been up 1 hour; binary replaced 10 minutes ago.
        let mtime = now - Duration::from_secs(600);
        assert!(is_running_daemon_stale_vs_disk(mtime, 3600, now));
    }

    #[test]
    fn process_started_after_install_is_fresh() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        // Binary written, then process started 30s later (uptime 30).
        let mtime = now - Duration::from_secs(60);
        assert!(!is_running_daemon_stale_vs_disk(mtime, 30, now));
    }

    #[test]
    fn two_second_skew_is_not_stale() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let mtime = now - Duration::from_secs(10);
        // uptime 9s ⇒ process_started = now-9; mtime is 1s before start → fresh
        assert!(!is_running_daemon_stale_vs_disk(mtime, 9, now));
        // uptime 15s ⇒ process_started = now-15; mtime is 5s after start → stale
        assert!(is_running_daemon_stale_vs_disk(mtime, 15, now));
    }
}

/// #14 self-heal: if the on-disk daemon LaunchAgent plist records a
/// stale/transient `ProgramArguments[0]` (a path under a now-ejected DMG
/// or a vanished AppTranslocation mount, or a binary that no longer
/// exists), rewrite it to point at the daemon binary bundled next to the
/// CURRENT Tauri exe and reload the agent so it converges.
///
/// Thin IO shell over the pure decision logic in
/// `k2_client::daemon_lifecycle`:
///   - classify current exe / recorded path (`is_transient_exe_location`)
///   - decide whether to rewrite (`should_rewrite_plist`)
///   - parse the recorded program out of the plist XML (`parse_plist_program`)
///
/// Conservative by construction:
///   - If the current exe is itself transient, we log and bail — we
///     can't trust the path we'd write.
///   - We only rewrite for transient/missing recorded paths, never just
///     because a different *stable + existing* path is recorded (the
///     dev-box `…/target/release/k2so-daemon` case).
///   - 0.43.2 migration: a plist that still records this app's old
///     `K2.app/Contents/MacOS/k2-daemon` is moved to the nested
///     `K2.app/Contents/Helpers/K2 Daemon.app/Contents/MacOS/k2-daemon`
///     and the agent is reloaded (bootout + bootstrap), which restarts the
///     daemon from the helper app. Runs at every app start and before the
///     version-mismatch kickstart.
///
/// Best-effort: every failure is logged and swallowed so this never
/// blocks startup or the kickstart that follows. Returns `true` when it
/// rewrote + reloaded the plist (caller can log), `false` otherwise.
///
/// macOS-only: the plist + launchctl machinery has no meaning elsewhere
/// (Linux daemons are systemd-user-unit-owned). Every caller is gated.
#[cfg(target_os = "macos")]
fn heal_daemon_plist_program() -> bool {
    use k2_client::daemon_lifecycle as dl;

    let Ok(current_exe) = std::env::current_exe() else {
        log_debug!("[plist-heal] current_exe() failed; skipping");
        return false;
    };
    let current_is_transient = dl::is_transient_exe_location(&current_exe);
    if current_is_transient {
        log_debug!(
            "[plist-heal] current exe is transient ({}); cannot trust it to seed the plist — skipping",
            current_exe.display()
        );
        return false;
    }

    let Some(desired) = dl::bundled_daemon_path(&current_exe) else {
        log_debug!("[plist-heal] current exe has no parent dir; skipping");
        return false;
    };

    // Locate the plist on disk.
    let plist = k2_client::wake::DaemonPlist::canonical(std::path::PathBuf::from("/unused"));
    let Some(plist_path) = plist.plist_path() else {
        log_debug!("[plist-heal] cannot locate ~/Library/LaunchAgents; skipping");
        return false;
    };
    if !plist_path.exists() {
        // No plist yet — install migration / autostart handles it.
        return false;
    }

    // Read the recorded program path out of the existing plist.
    let xml = match std::fs::read_to_string(&plist_path) {
        Ok(s) => s,
        Err(e) => {
            log_debug!("[plist-heal] read {} failed: {e}", plist_path.display());
            return false;
        }
    };
    let Some(recorded) = dl::parse_plist_program(&xml) else {
        log_debug!(
            "[plist-heal] no ProgramArguments[0] in {}; skipping",
            plist_path.display()
        );
        return false;
    };

    let recorded_exists = recorded.exists();
    if !dl::should_rewrite_plist(&recorded, &desired, recorded_exists, current_is_transient) {
        // Recorded path is fine (matches us, or is a different stable
        // existing dev path we must not churn).
        return false;
    }
    // 0.43.2 legacy-layout migration only makes sense when the nested
    // `K2 Daemon.app` is really in this bundle. Never swap a working
    // recorded binary for one that is not on disk (e.g. a bare
    // `tauri build` that skipped the helper-app step).
    if recorded_exists && !desired.exists() {
        log_debug!(
            "[plist-heal] recorded {} works and desired {} is missing; leaving the plist alone",
            recorded.display(),
            desired.display()
        );
        return false;
    }

    log_debug!(
        "[plist-heal] rewriting daemon plist program: recorded={} (exists={}) → desired={}",
        recorded.display(),
        recorded_exists,
        desired.display()
    );

    // Rewrite the plist to point at the desired (stable, bundled) binary.
    let new_plist = k2_client::wake::DaemonPlist::canonical(desired.clone());
    if let Err(e) = new_plist.write() {
        log_debug!("[plist-heal] write plist failed: {e}");
        return false;
    }

    // Validate the binary we just pointed at actually exists before we
    // bother reloading — if it doesn't, the rewrite at least leaves a
    // correct path for a future launch, but a reload would just fail.
    if !desired.exists() {
        log_debug!(
            "[plist-heal] desired daemon binary {} does not exist; wrote plist but skipping reload",
            desired.display()
        );
        return false;
    }

    // Reload the LaunchAgent (bootout + bootstrap) so launchd picks up
    // the new program path. Best-effort — kickstart follows regardless.
    reload_daemon_launch_agent(&plist_path);
    true
}

/// #14: bootout + bootstrap the daemon LaunchAgent so launchd re-reads a
/// freshly-rewritten plist. `launchctl load -w` won't re-read the
/// `ProgramArguments` of an already-bootstrapped service, so we have to
/// bootout the old definition first. Best-effort: all errors logged +
/// swallowed (a non-loaded service makes bootout fail harmlessly).
#[cfg(target_os = "macos")]
fn reload_daemon_launch_agent(plist_path: &std::path::Path) {
    let uid = unsafe { libc::getuid() };
    let domain = format!("gui/{uid}");

    let bootout = k2_client::bg_proc::command("launchctl")
        .arg("bootout")
        .arg(&domain)
        .arg(plist_path)
        .output();
    match bootout {
        Ok(o) if o.status.success() => log_debug!("[plist-heal] bootout ok"),
        Ok(o) => log_debug!(
            "[plist-heal] bootout non-zero (often harmless if not loaded): {}",
            String::from_utf8_lossy(&o.stderr).trim()
        ),
        Err(e) => log_debug!("[plist-heal] bootout spawn failed: {e}"),
    }

    let bootstrap = k2_client::bg_proc::command("launchctl")
        .arg("bootstrap")
        .arg(&domain)
        .arg(plist_path)
        .output();
    match bootstrap {
        Ok(o) if o.status.success() => log_debug!("[plist-heal] bootstrap ok"),
        Ok(o) => log_debug!(
            "[plist-heal] bootstrap non-zero: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        ),
        Err(e) => log_debug!("[plist-heal] bootstrap spawn failed: {e}"),
    }
}

/// 0.39.0: `warm_http_pool_async` (Kessel-T0 reqwest-runtime warmup) was
/// removed alongside the Kessel-T0 JSON-stream renderer. Kept as an empty
/// stub so `main.rs` doesn't fail to link — it now does nothing, which is
/// correct because the Kessel WS spawn path doesn't share the
/// blocking reqwest runtime the old Kessel-T0 spawn path needed warm.
pub fn warm_http_pool_async() {}

/// `k2 --version` / `k2 -V` on the app binary: the text to print, or `None`
/// to start the app normally.
///
/// `main` prints this and exits BEFORE Tauri starts. dev-build.sh and
/// release.sh exec the freshly signed, NOT installed bundle with `--version`
/// as their AMFI launch check (AMFI kills a bad signature at exec, before
/// `main`, so an early exit still proves the binary launches). Before this,
/// `--version` was ignored and the whole app started from the build folder:
/// `check_daemon_version_and_restart` saw that bundle's `k2-daemon` newer
/// than the running daemon (the mtime check) and kickstarted the user's live
/// daemon. A version check must never touch the running daemon.
pub fn version_flag_output<I, S>(args: I) -> Option<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    args.into_iter()
        .skip(1)
        .any(|a| matches!(a.as_ref(), "--version" | "-V"))
        .then(|| format!("k2 {}", env!("CARGO_PKG_VERSION")))
}

#[cfg(test)]
mod version_flag_tests {
    use super::version_flag_output;

    #[test]
    fn version_flag_prints_the_app_version_and_skips_startup() {
        let want = format!("k2 {}", env!("CARGO_PKG_VERSION"));
        assert_eq!(version_flag_output(["k2", "--version"]), Some(want.clone()));
        assert_eq!(version_flag_output(["k2", "-V"]), Some(want));
    }

    #[test]
    fn no_version_flag_starts_the_app() {
        assert_eq!(version_flag_output(["k2"]), None);
        assert_eq!(version_flag_output(["k2", "k2://open"]), None);
        // The program path itself is never read as the flag.
        assert_eq!(version_flag_output(["--version"]), None);
    }

    /// main.rs must check the flag before anything else runs (Tauri, the
    /// daemon version check, fd limits).
    #[test]
    fn main_checks_version_flag_first() {
        let main_src = include_str!("main.rs");
        let body = &main_src[main_src.find("fn main()").expect("fn main")..];
        let flag = body
            .find("version_flag_output")
            .expect("main must call version_flag_output");
        let run = body.find("k2_lib::run()").expect("main must call run");
        let nofile = body
            .find("raise_nofile_limit")
            .expect("main raises the fd limit");
        assert!(
            flag < run && flag < nofile,
            "--version must short-circuit before startup"
        );
    }
}

/// Stage the bundled `frpc` tunnel client (Tauri `externalBin` sidecar) out
/// to `~/.k2/bin/`, where the daemon's `resolve_frpc` finds it.
///
/// Sidecar locations (next to the running executable):
/// - macOS: `K2.app/Contents/MacOS/frpc`
/// - Windows NSIS: `%LOCALAPPDATA%\K2\frpc.exe` (Tauri names the peer `.exe`)
/// - `tauri dev`: `target/debug/frpc` / `frpc.exe`
///
/// Idempotent: only copies when the destination is missing or its bytes
/// differ from the sidecar (so app upgrades that bump frpc re-stage).
/// Fault-isolated: every failure is logged and swallowed — staging frpc
/// must never block app startup. Writing the bytes with our own process
/// (rather than the user downloading them) means the staged file carries
/// no `com.apple.quarantine` flag, so Gatekeeper lets it execute.
fn stage_bundled_frpc() {
    // Windows ships `frpc.exe`; Unix ships bare `frpc`. Probe both so a
    // mis-named peer still stages.
    let sidecar_names: &[&str] = if cfg!(windows) {
        &["frpc.exe", "frpc"]
    } else {
        &["frpc"]
    };

    let Some(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
    else {
        log_debug!("[k2so] could not resolve current_exe to find frpc sidecar");
        return;
    };

    let mut sidecar: Option<std::path::PathBuf> = None;
    for name in sidecar_names {
        let p = exe_dir.join(name);
        if p.exists() {
            sidecar = Some(p);
            break;
        }
    }
    let Some(sidecar) = sidecar else {
        log_debug!(
            "[k2so] bundled frpc sidecar not found next to {} — skipping stage \
             (dev build without externalBin, or older bundle)",
            exe_dir.display()
        );
        return;
    };

    // Stage with the platform-native name so resolve_frpc + CreateProcess agree.
    let dest_name = if cfg!(windows) { "frpc.exe" } else { "frpc" };
    let dest = k2_client::paths::k2_bin().join(dest_name);

    // Skip the copy when the staged bytes already match the sidecar.
    if dest.exists() {
        match (std::fs::read(&sidecar), std::fs::read(&dest)) {
            (Ok(a), Ok(b)) if a == b => {
                log_debug!("[k2so] frpc already staged at {} (up to date)", dest.display());
                return;
            }
            (Ok(_), Ok(_)) => {
                log_debug!("[k2so] staged frpc differs from bundle; re-staging");
            }
            // If we can't read one side, fall through and try to overwrite.
            _ => {}
        }
    }

    if let Some(parent) = dest.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            log_debug!("[k2so] failed to create {}: {e}", parent.display());
            return;
        }
    }

    if let Err(e) = std::fs::copy(&sidecar, &dest) {
        log_debug!(
            "[k2so] failed to stage frpc {} -> {}: {e}",
            sidecar.display(),
            dest.display()
        );
        return;
    }

    // Ensure the staged binary is executable (0755).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) =
            std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755))
        {
            log_debug!("[k2so] failed to chmod 0755 {}: {e}", dest.display());
            return;
        }
    }

    // Drop a bare `frpc` alias on Windows only if we staged `.exe` and the
    // legacy extensionless path is missing — older resolve probes used it.
    #[cfg(windows)]
    {
        let bare = k2_client::paths::k2_bin().join("frpc");
        if !bare.exists() {
            let _ = std::fs::copy(&dest, &bare);
        }
    }

    log_debug!("[k2so] staged bundled frpc -> {}", dest.display());
}

/// Staged helper binary, and whether this call replaced the bytes.
#[cfg(target_os = "macos")]
struct MenubarStage {
    path: std::path::PathBuf,
    replaced: bool,
}

/// Copy `k2-menubar` from beside this exe to `~/.k2/bin/k2-menubar`.
/// Same shape as [`stage_bundled_frpc`]. The LaunchAgent points at the
/// copy, not at `Contents/MacOS`.
#[cfg(target_os = "macos")]
fn stage_bundled_menubar() -> Option<MenubarStage> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))?;
    let sidecar = exe_dir.join(k2_client::wake::MENU_BAR_HELPER_BIN);
    if !sidecar.exists() {
        log_debug!(
            "[menubar] {} not next to {} — skipping stage",
            k2_client::wake::MENU_BAR_HELPER_BIN,
            exe_dir.display()
        );
        return None;
    }
    let dest = k2_client::paths::k2_bin().join(k2_client::wake::MENU_BAR_HELPER_BIN);
    let mut replaced = !dest.exists();
    if dest.exists() {
        match (std::fs::read(&sidecar), std::fs::read(&dest)) {
            (Ok(a), Ok(b)) if a == b => {
                log_debug!(
                    "[menubar] already staged at {} (up to date)",
                    dest.display()
                );
                return Some(MenubarStage {
                    path: dest,
                    replaced: false,
                });
            }
            (Ok(_), Ok(_)) => {
                log_debug!("[menubar] staged binary differs; re-staging");
                replaced = true;
            }
            _ => {
                replaced = true;
            }
        }
    }
    if let Some(parent) = dest.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            log_debug!("[menubar] failed to create {}: {e}", parent.display());
            return None;
        }
    }
    if let Err(e) = std::fs::copy(&sidecar, &dest) {
        log_debug!(
            "[menubar] failed to stage {} -> {}: {e}",
            sidecar.display(),
            dest.display()
        );
        return None;
    }
    use std::os::unix::fs::PermissionsExt;
    if let Err(e) = std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755)) {
        log_debug!("[menubar] failed to chmod 0755 {}: {e}", dest.display());
        return None;
    }
    log_debug!(
        "[menubar] staged {} -> {}",
        sidecar.display(),
        dest.display()
    );
    Some(MenubarStage {
        path: dest,
        replaced,
    })
}

/// Write and `launchctl load -w` the helper LaunchAgent when this open
/// is not a transient DMG / AppTranslocation launch and the staged binary
/// exists. Does not unload `dev.k2.daemon`.
#[cfg(target_os = "macos")]
pub(crate) fn install_menu_bar_helper() {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            log_debug!("[menubar] current_exe: {e}");
            return;
        }
    };
    if k2_client::daemon_lifecycle::is_transient_exe_location(&exe) {
        log_debug!(
            "[menubar] transient launch ({}); skipping helper install",
            exe.display()
        );
        return;
    }
    let Some(staged) = stage_bundled_menubar() else {
        return;
    };
    let plist = k2_client::wake::DaemonPlist::menu_bar_helper(staged.path.clone());
    let outcome = match k2_client::wake::ensure_loaded(&plist) {
        Ok(outcome) => outcome,
        Err(e) => {
            log_debug!("[menubar] ensure_loaded failed: {e}");
            return;
        }
    };
    if !staged.replaced && outcome != k2_client::wake::LoadOutcome::NotInstalled {
        log_debug!("[menubar] helper launch agent {outcome:?}");
        return;
    }
    match k2_client::wake::install(&plist) {
        Ok(path) => log_debug!(
            "[menubar] installed helper plist at {} -> {}",
            path.display(),
            staged.path.display()
        ),
        Err(e) => log_debug!("[menubar] helper install failed: {e}"),
    }
}

/// Zen v2 S0 spike builds only (`VITE_K2_ZEN_SPIKE=s0` at build time,
/// prd-zen-user-widgets-v2 §15): the renderer shows the sealed-frame spike
/// instead of the app, so the shell leaves this machine's daemon, its
/// launchd plist and the menu-bar helper alone. Always false in a normal
/// build.
fn zen_spike_build() -> bool {
    option_env!("VITE_K2_ZEN_SPIKE") == Some("s0")
}

pub fn run() {
    // Before any window exists: a caught ObjC exception or panic in tao's
    // `sendEvent:` writes one line to ~/.k2/client-event-faults.log.
    #[cfg(target_os = "macos")]
    event_faults::install();

    // Ignore SIGPIPE so writing to a dead PTY returns EPIPE instead of
    // killing the entire process.
    #[cfg(unix)]
    k2_client::process::ignore_sigpipe();

    // launchd-launched .app processes inherit a sparse PATH that lacks
    // ~/.local/bin, /opt/homebrew/bin, and other user-installed prefixes.
    // Source the user's login shell once and adopt its PATH so legacy
    // alacritty spawns + every Command::new call site can resolve user
    // tools. See docs in k2_client::process::enrich_path_from_login_shell.
    #[cfg(unix)]
    k2_client::process::enrich_path_from_login_shell();

    // 0.40.0 rebrand — one-time ~/.k2so → ~/.k2 home migration. MUST run
    // before anything creates ~/.k2 (a fresh empty ~/.k2 would abandon the
    // user's data in ~/.k2so and force the migration's Conflict branch —
    // caught by the convergence rig, 2026-06-10). The daemon boot runs
    // the same idempotent call; first process wins.
    match k2_client::migration_home::migrate_home_dir() {
        Ok(o) => eprintln!("[boot] home migration: {o:?}"),
        Err(e) => eprintln!("[boot] home migration failed: {e} — continuing on legacy layout"),
    }

    // Rustls 0.23 refuses to auto-pick when more than one CryptoProvider
    // is compiled in and panics on first TLS use unless one is installed.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    // 0.46.4: this process no longer opens (or migrates) ~/.k2/k2.db.
    // The daemon is the only DB owner and runs the migrations on its own
    // boot; every renderer read and write reaches it over /cli/*.

    // Phase 2 close-out (2026-05-23): `AppState` deleted entirely.
    // Units 1-4 had already moved every other field (companion, LLM,
    // terminal manager, DB connection) into the daemon; this commit
    // moves the last surviving field — `watchers` — into a
    // module-level `LazyLock<Mutex<_>>` static inside `watcher.rs`.
    // No `.manage()` registration needed; the filesystem watcher
    // commands read the static directly.

    let builder = tauri::Builder::default();
    // Windows: one K2 app per person. Starting K2 again (Start menu,
    // taskbar, a k2:// link) brings the running window back instead of
    // a second app beside a hidden one. Registered first, as the plugin
    // asks; its `deep-link` feature hands a k2:// launch to the running
    // app. macOS (LaunchServices) and Linux are unchanged.
    #[cfg(windows)]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
        tray::show_main_window(app);
    }));
    let builder = builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_drag::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_notification::init());
    #[cfg(feature = "native-voice")]
    let builder = builder.plugin(native_voice::plugin());

    // App menu bar is macOS-only. Win/Linux use the in-app "Menu" button
    // (frameless chrome). Tray menu is installed separately and stays.
    #[cfg(target_os = "macos")]
    let builder = builder
        .menu(|handle| menu::create_menu(handle))
        .on_menu_event(menu::handle_menu_event);

    builder
        .setup(|app| {
            // 0.40.1 — CLI symlink heal. Re-link `/usr/local/bin/k2`
            // (+ the k2so shim) when the symlink is missing, broken, or points
            // at a different bundle than the one running (e.g. a user replaced
            // K2SO.app with a fresh K2.app, leaving a dangling link → `CLI
            // Version: v?`).
            //
            // 0.40.10 (#56) — this boot heal is now SILENT: it only attempts
            // the direct (non-admin) symlink. It must NEVER fire the osascript
            // admin password dialog, because a background heal that prompts on
            // every launch/update was the "K2 needs your password" nag. The
            // admin prompt now happens ONLY from an explicit Settings → Install
            // CLI click. If the heal needs admin (root-owned /usr/local/bin),
            // it defers silently and the user installs from Settings once.
            // Combined with the canonicalized needs-heal check, a correctly
            // installed CLI reports no heal needed and stays quiet.
            // Browser Tab (S1): registry for child browser webviews.
            crate::commands::browser_webviews::init(&app.handle().clone());

            if crate::commands::settings::cli_symlink_needs_heal() {
                std::thread::spawn(|| {
                    match crate::commands::settings::cli_heal_silent() {
                        Ok(true) => eprintln!("[boot] CLI heal: k2 + k2so symlinks (re)linked silently"),
                        Ok(false) => eprintln!("[boot] CLI heal deferred: /usr/local/bin needs admin — install from Settings → Install CLI"),
                        Err(e) => eprintln!("[boot] CLI heal skipped: {e}"),
                    }
                });
            }

            // 0.40.0 — retire com.k2so.* LaunchAgents (the daemon plist
            // re-installs below under dev.k2.daemon; heartbeat +
            // claude-auth re-ensure from the daemon's own boot).
            let swept = k2_client::migration_launchd::migrate_launchd_labels();
            if !swept.is_empty() {
                eprintln!("[boot] launchd labels migrated: {swept:?}");
            }
            let __setup_start = std::time::Instant::now();
            struct SetupGuard(std::time::Instant);
            impl Drop for SetupGuard {
                fn drop(&mut self) {
                    if crate::perf::is_enabled() {
                        use std::io::Write;
                        let _ = writeln!(
                            std::io::stderr(),
                            "[perf] startup_setup_total — {}µs",
                            self.0.elapsed().as_micros()
                        );
                    }
                }
            }
            let _setup_guard = SetupGuard(__setup_start);

            // 0.46.4: no in-process agent-hook sink. Hook events reach
            // the renderer from the daemon's /events bus (daemon_events).

            // Phase 2 Unit 7c — workspace regen bridge retired.
            // `k2_core::workspace::agent_launch` now calls
            // `workspace::write_workspace_skill_file` directly (the
            // SKILL scaffolding moved into k2so-core during Unit 7b),
            // so the WorkspaceRegenProvider trait + Tauri impl that
            // used to forward eager regens back into src-tauri are
            // gone. Both daemon and Tauri contexts hit the same body.

            // Subscribe to the daemon's /events WebSocket. Daemon-
            // originated hook events arrive here and are re-emitted via
            // AppHandle::emit exactly as if agent_hooks.rs had handled
            // them locally. No-op until the daemon is running; reconnects
            // forever so we survive launchctl unload/load cycles.
            daemon_events::spawn_subscriber(app.handle().clone());

            // Phase 2 Unit 4 — window-state JSON→SQLite migration was
            // dead code as of 2026-05-23 (the source file was already
            // cleaned up by the migration itself ages ago). Deleted.
            //
            // Phase 2 Unit 4 — workspace_layouts settings.json→SQLite
            // migration moved into the daemon's first-boot pass
            // (`k2_core::db_ops::migrate_workspace_layouts_to_db`)
            // so K2SO Connect and headless daemons pick it up.

            // Create skill layer template directories if they don't exist
            {
                let templates = k2_client::paths::k2_home().join("templates");
                let _ = std::fs::create_dir_all(templates.join("manager"));
                let _ = std::fs::create_dir_all(templates.join("agent-template"));
                let _ = std::fs::create_dir_all(templates.join("custom-agent"));
            }

            // Stage the bundled `frpc` tunnel client to ~/.k2so/bin/frpc so
            // a fresh HOST machine needs zero manual setup for K2 Connect.
            // The binary ships INSIDE the notarized app as a Tauri
            // externalBin (sidecar in Contents/MacOS/frpc), so it's signed
            // with our Developer ID + hardened runtime. Writing it out with
            // our own process means no com.apple.quarantine flag, so
            // Gatekeeper lets it run (unlike a network-downloaded binary).
            //
            // The daemon (a separate process) resolves ~/.k2so/bin/frpc via
            // the existing `resolve_frpc` common-locations probe — no
            // resolver change needed. Idempotent + fault-isolated: logs and
            // continues on any failure, never blocks startup.
            perf_timer!("startup_stage_frpc", {
                stage_bundled_frpc();
            });

            // 0.39.0 K2 Connect prep: the `legacy_agent_types_v1`
            // AGENT.md frontmatter rewrite (pod-member → agent-template,
            // pod-leader → manager) moved to the daemon's first-boot
            // pass (`run_legacy_agent_types_v1_migration` in
            // `crates/k2so-daemon/src/main.rs`). Headless / remote
            // daemons now pick it up without Tauri being present.

            // Linux/other platforms: no launchd. Linux package owns a
            // systemd user unit; Windows has neither yet — spawn the
            // bundled k2-daemon next to this exe if nothing answers
            // /boot-status (fixes "perpetually connecting" when the
            // thin client launches alone).
            #[cfg(not(target_os = "macos"))]
            {
                log_debug!(
                    "[k2so] daemon self-heal: launchd unavailable — ensure local k2-daemon process"
                );
                if !zen_spike_build() {
                    ensure_local_daemon_process();
                }
            }

            // 0.46.4: the one-time `install_daemon_plist_v2` install that
            // used to run here was gated on a `code_migrations` row in
            // ~/.k2/k2.db, and this process no longer opens the DB. Every
            // machine since 0.33 already has that row (so the block was a
            // no-op there), and a fresh install is covered by the
            // every-boot heal below: `ensure_loaded` → NotInstalled →
            // install + load from the bundled daemon, with the same
            // transient-location (#14) guard. The one case that differs:
            // a plist that exists while the DB row is missing (DB wiped,
            // LaunchAgent kept) is no longer force-rewritten once; the
            // heal below still fixes a stale program path and loads it.

            // #14 self-heal at startup: if a previous bad DMG / translocated
            // launch baked a transient ProgramArguments[0] into the plist,
            // a normal /Applications upgrade may not trip the version-mismatch
            // path (the stale daemon could be the SAME version). Rewrite the
            // plist BEFORE the autostart `ensure_loaded` below so launchd
            // loads the corrected program path. No-op when the recorded path
            // is already correct, or when the current exe is itself transient.
            #[cfg(target_os = "macos")]
            perf_timer!("startup_heal_daemon_plist", {
                if !zen_spike_build() && heal_daemon_plist_program() {
                    log_debug!("[k2so] daemon plist self-healed at startup (#14)");
                }
            });

            // Windows: the daemon starts at logon, as the macOS launchd
            // agent does (k2_client::win_autostart: a per-user Run value
            // naming the installed k2-daemon.exe). Never from a dev build
            // or a transient location; checked on every launch so a moved
            // install heals.
            #[cfg(windows)]
            perf_timer!("startup_ensure_daemon_autostart", {
                let daemon_bin = std::env::current_exe()
                    .ok()
                    .and_then(|p| k2_client::daemon_lifecycle::bundled_daemon_path(&p))
                    .filter(|p| p.exists())
                    .filter(|p| !k2_client::daemon_lifecycle::is_transient_exe_location(p));
                match daemon_bin {
                    Some(bin) if !cfg!(debug_assertions) => match k2_client::win_autostart::ensure(&bin) {
                        Ok(true) => log_debug!("[k2] daemon logon autostart set -> {}", bin.display()),
                        Ok(false) => {}
                        Err(e) => log_debug!("[k2] daemon logon autostart failed: {e}"),
                    },
                    _ => log_debug!("[k2] daemon logon autostart skipped (dev build or no installed daemon)"),
                }
            });

            // Autostart: on every Tauri launch (not just first-install
            // migration), make sure the daemon plist is loaded. Covers
            // the case where the user red-buttoned with "keep server
            // running" OFF (which unloads the plist) and then relaunched
            // the app — they expect the daemon to be back without having
            // to click "Restart" in Settings. Fires regardless of the
            // toggle, in both debug and release builds.
            #[cfg(target_os = "macos")]
            perf_timer!("startup_ensure_daemon_loaded", {
              if zen_spike_build() {
                log_debug!("[zen-spike] S0 spike build: daemon autostart skipped");
              } else {
                let plist = k2_client::wake::DaemonPlist::canonical(
                    std::path::PathBuf::from("/unused"),
                );
                match k2_client::wake::ensure_loaded(&plist) {
                    Ok(k2_client::wake::LoadOutcome::AlreadyLoaded) => {
                        log_debug!("[k2so] daemon already loaded in launchctl");
                    }
                    Ok(k2_client::wake::LoadOutcome::Loaded) => {
                        log_debug!("[k2so] daemon plist loaded (was unloaded)");
                    }
                    Ok(k2_client::wake::LoadOutcome::NotInstalled) => {
                        // Self-heal EVERY boot, not just first install: the
                        // 0.40 launchd sweep retires com.k2so.k2so-daemon,
                        // and on updated machines install_daemon_plist_v1 is
                        // already marked applied in the DB — deferring to
                        // "the install migration" here would leave the
                        // machine daemonless forever (caught by the
                        // convergence rig, 2026-06-10). Re-install from the
                        // bundled daemon and load, same transient-location
                        // guard as the migration path (#14).
                        let healed = std::env::current_exe()
                            .ok()
                            .and_then(|p| k2_client::daemon_lifecycle::bundled_daemon_path(&p))
                            .filter(|p| p.exists())
                            .filter(|p| !k2_client::daemon_lifecycle::is_transient_exe_location(p));
                        match healed {
                            Some(daemon_bin) => {
                                let plist =
                                    k2_client::wake::DaemonPlist::canonical(daemon_bin.clone());
                                match k2_client::wake::install(&plist)
                                    .and_then(|_| k2_client::wake::ensure_loaded(&plist))
                                {
                                    Ok(_) => log_debug!(
                                        "[k2so] daemon plist healed + loaded (was missing) → {}",
                                        daemon_bin.display()
                                    ),
                                    Err(e) => log_debug!(
                                        "[k2so] daemon plist heal failed: {e}"
                                    ),
                                }
                            }
                            None => log_debug!(
                                "[k2so] daemon plist missing and no bundled daemon to heal from                                  (dev build or transient location) — daemon stays down"
                            ),
                        }
                    }
                    Err(e) => {
                        log_debug!("[k2so] daemon autostart failed: {e}");
                    }
                }
              }
            });

            // Menu-bar helper plist, same GUI open as the daemon agent.
            // The once-only migration is already applied on upgraded
            // machines, so this is not inside that row: a missing helper
            // plist is installed here (and from Settings daemon_install).
            // A transient DMG / AppTranslocation launch returns without
            // writing it. Debug is not special-cased — there is nothing
            // to stage until `k2-menubar` sits next to this exe.
            #[cfg(target_os = "macos")]
            {
                if !zen_spike_build() {
                    install_menu_bar_helper();
                }
            }

            // Phase 2 Unit 4 — SKILL regeneration moved to the daemon's
            // boot sweep (`run_workspace_legacy_migrations_sweep` →
            // `ensure_all_skills_up_to_date`). The per-version
            // `skill_regen_version` gate this block used was replaced
            // by per-file checksum gates inside k2so-core's
            // `ensure_skill_up_to_date`, so a stale `skill_regen_version`
            // value is harmless — the writers don't touch disk if the
            // file already matches. Block deleted; daemon owns the pass.

            // Conf window is `create: false` so we can attach `on_navigation`
            // (builder-only). Must exist before restore / close / watchdog.
            crate::k2_app_window::create_main_k2_window(app.handle())?;

            // Restore last good frame, or center a default 1400×900 if
            // the saved rect is missing / tiny / off every current display.
            window::apply_restored_frame(app.handle());
            if window::load_window_state(app.handle()).is_none() {
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    for _ in 0..10 {
                        std::thread::sleep(std::time::Duration::from_millis(200));
                        if window::load_window_state(&handle).is_some() {
                            window::apply_restored_frame(&handle);
                            return;
                        }
                    }
                });
            }
            // Native WebKit zoom is disabled via zoomHotkeysEnabled:false in tauri.conf.json.
            // App zoom is handled by transform:scale() in the frontend (App.tsx).

            // Save window state and clean up terminals on close
            let app_handle = app.handle().clone();
            if let Some(win) = app.get_webview_window("main") {
                let win_for_hide = win.clone();
                win.on_window_event(move |event| {
                    match event {
                        tauri::WindowEvent::Moved(_) | tauri::WindowEvent::Resized(_) => {
                            window::schedule_save_window_state(&app_handle);
                            return;
                        }
                        tauri::WindowEvent::CloseRequested { api, .. } => {
                        // Red close-button behavior is controlled by
                        // the "Keep server running when the window is
                        // closed" preference:
                        //
                        //   ON  → hide the window. The app process stays.
                        //         The daemon stays. The menu-bar helper
                        //         is a separate LaunchAgent and stays.
                        //   OFF → unload the daemon plist, then exit.
                        //
                        // Cmd+Q is not this path. It lands in
                        // RunEvent::ExitRequested, which exits the app
                        // and does not unload the daemon.
                        // Read from the daemon-owned `~/.k2/settings.json`
                        // (k2-client's read-only view; same field, same
                        // default as `k2_core::app_settings::load`), so it
                        // works even while the daemon is down.
                        let keep_running =
                            k2_client::app_settings_view::load().keep_daemon_on_quit;
                        if keep_running {
                            window::save_window_state(&app_handle);
                            api.prevent_close();
                            let _ = win_for_hide.hide();
                            log_debug!("[window] Close intercepted — keeping server alive per settings");
                            return;
                        }
                        // Toggle OFF — user wants red-dot to take
                        // everything down: an explicit daemon STOP, which
                        // ends its agents (k2-ptyd P83, Q9). Daemon-first:
                        // `POST /cli/daemon/stop` (the daemon ends its
                        // agents and unloads its own launchd job). Only
                        // when the daemon is unreachable: write the stop
                        // intent and unload the plist here, so launchd
                        // stops respawning it. Then fall through to the
                        // normal cleanup + destroy path below.
                        #[cfg(target_os = "macos")]
                        {
                            let asked = crate::daemon_client::DaemonClient::try_connect()
                                .and_then(|c| c.cli_post_query("/cli/daemon/stop", &[]))
                                .is_ok();
                            if !asked {
                                let plist = k2_client::wake::DaemonPlist::canonical(
                                    std::path::PathBuf::from("/unused"),
                                );
                                if let Some(path) = plist.plist_path() {
                                    if path.exists() {
                                        let _ = k2_client::wake::write_stop_intent(
                                            &k2_client::paths::k2_home(),
                                            "app",
                                        );
                                        let _ = k2_client::wake::launchctl_unload(&path);
                                    }
                                }
                            }
                        }
                        window::save_window_state(&app_handle);

                        // Phase 2 Unit 3 — terminal PTY lifecycle moved
                        // to the daemon. Tauri quit MUST NOT kill the
                        // PTYs anymore: K2SO Connect (daemon on remote
                        // machine) and Mobile Companion both require
                        // PTYs to survive the local Tauri quitting.
                        // The daemon process owns the TerminalManager
                        // and reaps dead PTYs in its own watchdog (see
                        // `crates/k2so-daemon/src/watchdog.rs`).
                        //
                        // Phase 2 Unit 2 — the LLM lives in the daemon
                        // now; Tauri no longer owns a model handle, so
                        // there's nothing to unload here either.

                        // Phase 4 H7: the daemon is the sole writer of
                        // ~/.k2so/heartbeat.port since Tauri retired
                        // its HTTP listener. Do NOT remove the file on
                        // Tauri quit — the daemon process keeps running
                        // (launchd-managed) and the CLI still needs a
                        // valid port file to find it.

                        // The red-close path never reaches ExitRequested.
                        commands::native_voice::leave_on_quit(&app_handle);

                        // Use _exit() to skip C++ static destructors (ggml_metal).
                        // Without this, __cxa_finalize_ranges runs ggml's Metal cleanup
                        // which races against macOS Metal device teardown → SIGABRT.
                        // Skip _exit during relaunch so the process plugin can spawn
                        // the new process before this one exits.
                        if RELAUNCH_MODE.load(std::sync::atomic::Ordering::Relaxed) {
                            log_debug!("[shutdown] Relaunch mode — using normal exit");
                            std::process::exit(0);
                        } else {
                            #[cfg(unix)]
                            unsafe {
                                libc::_exit(0);
                            }
                            #[cfg(not(unix))]
                            std::process::exit(0);
                        }
                        }
                        _ => {}
                    }
                });
            }
            // Phase 4 H7: Tauri no longer runs its own HTTP listener.
            // The k2so-daemon (launchd-managed) is the sole server for
            // every /cli/* route + /hook/* + /events WS, and writes
            // heartbeat.port + heartbeat.token itself on startup.
            //
            // Hook installation (notify.sh + the claude/cursor/gemini
            // configs) is the DAEMON's job too since
            // prd-daemon-activity-and-thread-working-v1 DA7: it runs after
            // boot and every 10 minutes, so a headless daemon has hooks
            // and this app never rewrites the script. Install problems
            // reach the renderer as the daemon's `hooks_install_failed`.
            //
            // (0.46.4: the hook_config sync that used to run here fed
            // in-process terminals this app no longer has; removed.)
            if !zen_spike_build() {
                check_daemon_version_and_restart();
            }
            // Phase 2 Unit 7b: the per-workspace legacy migrations
            // (filename uppercase, CLAUDE.md harvest, heartbeat
            // promote/repair, orphan archive) + `ensure_all_skills_up_to_date`
            // now run in `k2so-daemon::main::run_workspace_legacy_migrations_sweep`.
            // The daemon executes the same idempotent sweep on its
            // own boot, so this Tauri-side thread is gone. Remote
            // daemons (K2SO Connect) and headless boots now pick up
            // these migrations without Tauri being present.
            //
            // Phase 4 H7: the old 60s heartbeat.port watchdog used to
            // periodically rewrite heartbeat.port with Tauri's own port.
            // Post-H7 the daemon owns heartbeat.port and runs its own
            // re-claim loop (see `run_heartbeat_port_watchdog` in
            // k2so-daemon). Tauri re-writing the file would fight the
            // daemon for ownership, so this loop is gone.

            // Linux and Windows keep the in-window tray. macOS does not
            // call tray::install — the status item is the k2-menubar
            // LaunchAgent, and a second icon would come from this window.
            // The macOS binding keeps tray.rs type-checked here; it does
            // not install an icon.
            #[cfg(not(target_os = "macos"))]
            {
                if let Err(e) = tray::install(&app.handle().clone()) {
                    log_debug!("[tray] install failed: {e} (continuing without tray)");
                }
            }
            #[cfg(target_os = "macos")]
            {
                const _: fn(&tauri::AppHandle<tauri::Wry>) -> Result<(), String> = tray::install;
            }

            // 0.39.x (Issue #6): webview liveness watchdog, per window
            // (Zen v2 UW32, UW56); `webview_watchdog.rs`.
            webview_watchdog::spawn(app);

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // Win/Linux App Menu — open secondary window (macOS uses native menu).
            menu::window_new,
            // Windows: the title bar's right-click system menu.
            menu::window_system_menu,
            // Tickets "Open in window" — one window per ticket.
            menu::window_open_ticket,
            // A voice call's own window (pop out); closing it never hangs up.
            call_window::window_open_call,
            // prd-zen-mode-v1 Z53: the View menu's Enter/Exit Zen Mode text.
            menu::set_zen_menu_label,
            // 0.39.x (Issue #6): webview liveness watchdog heartbeat
            // (per window since Zen v2 UW56) and its reload note (UW32).
            webview_watchdog::renderer_heartbeat,
            webview_watchdog::watchdog_take_reload_note,
            // 0.40.48 connection resilience — out-of-webview boot-status
            // arbiter (poisoned-pool tiebreaker) + user-initiated restart.
            commands::connection::remote_boot_probe,
            commands::connection::restart_app,
            commands::connection::open_app_devtools,
            // Style System — traffic lights follow the style's window inset.
            commands::traffic_lights::set_traffic_light_inset,
            // Square style — private NSWindow corner radius. Non-mac no-op.
            commands::window_corners::set_window_corner_radius,
            commands::linux_desktop::linux_desktop_session,
            // Cmd+L — `makeFirstResponder:` BOOL. `set_focus` drops it.
            commands::ui_webview_focus::ui_webview_make_first_responder,
            // Embedded Browser Tab (S1 spike) — child-webview lifecycle.
            commands::browser_webviews::browser_create,
            commands::browser_webviews::browser_set_bounds,
            commands::browser_webviews::browser_set_visible,
            commands::browser_webviews::browser_navigate,
            commands::browser_webviews::browser_current_url,
            commands::browser_webviews::browser_close,
            commands::browser_webviews::browser_devtools,
            commands::browser_webviews::browser_back,
            commands::browser_webviews::browser_forward,
            commands::browser_webviews::browser_history_state,
            commands::browser_webviews::browser_snapshot,
            commands::browser_webviews::browser_set_appearance,
            // Design mode picker (Design tabs only; host eval, no child IPC).
            commands::browser_webviews::browser_pick_arm,
            commands::browser_webviews::browser_pick_disarm,
            commands::browser_webviews::browser_pick_poll,
            commands::browser_webviews::browser_design_badges,
            // Remote Gmail OAuth — client-local loopback capture.
            commands::oauth_loopback::oauth_loopback_bind,
            commands::oauth_loopback::oauth_loopback_wait,
            commands::oauth_loopback::oauth_loopback_cancel,
            // Projects — host-only verbs (folder picker, icon upload,
            // OS-integration "open in" actions, editor discovery, focus
            // window). All DB-backed project CRUD moved off the Tauri
            // surface; the renderer reaches it host-aware via `/cli/*`.
            commands::projects::projects_pick_folder,
            commands::projects::pick_local_files,
            commands::projects::projects_open_in_finder,
            commands::projects::open_folder,
            commands::projects::projects_open_in_editor,
            commands::projects::projects_open_in_terminal,
            commands::projects::projects_get_editors,
            commands::projects::projects_get_all_editors,
            commands::projects::projects_refresh_editors,
            commands::projects::projects_open_focus_window,
            // Plan B cleanup — `workspaces_*`, `focus_groups_*`,
            // `sections_*`, and `presets_*` (agents) command surfaces
            // deleted. All routed daemon data; the renderer reaches it
            // host-aware via `/cli/*` on the active daemon.
            // Phase 2 Unit 6 — `commands::filesystem::*` shims
            // deleted. Renderer hits `/cli/fs/*` on the daemon.
            // K2 Connect remote-files Phase 2 — read a LOCAL dropped
            // file's bytes (base64) for upload to the remote daemon.
            commands::local_upload::read_local_file_base64,
            // GH #3 — streaming "Clone to" for large bundles: size probe +
            // ranged read so the renderer can chunk a multi-GB bundle to
            // `/cli/fs/upload-chunk` without holding it in memory.
            commands::local_upload::local_file_size,
            commands::local_upload::read_local_file_range,
            // 0.40.22 — streaming download-to-local: the renderer loops the
            // daemon's `GET /cli/fs/read-range` and lands each chunk in
            // ~/Downloads via ordered .part appends + atomic finalize.
            commands::local_download::local_download_chunk,
            commands::local_download::local_download_abort,
            // 0.37.9 — macOS permissions surface for Settings UI
            commands::permissions::permissions_get_status,
            commands::permissions::permissions_request_full_disk_access,
            commands::permissions::permissions_request_accessibility,
            commands::permissions::permissions_request_microphone,
            commands::permissions::permissions_open_microphone_settings,
            // Video in voice rooms (behind videoRoomsPreview): the camera and
            // screen recording privacy panes, for the call panel's refusals.
            commands::permissions::permissions_open_camera_settings,
            commands::permissions::permissions_open_screen_settings,
            commands::native_voice::voice_native_status,
            commands::permissions::permissions_request_apple_events,
            commands::permissions::permissions_request_local_network,
            // Phase 2 Unit 6 — `commands::whats_new::*` shims
            // deleted. Renderer hits `/cli/whats_new*` on the daemon.
            // Memory watcher (0.38.12 — leak telemetry)
            commands::memory_watcher::renderer_memory_status,
            // Filesystem watcher
            watcher::fs_watch_dir,
            watcher::fs_unwatch_dir,
            // Settings — host-only verbs (CLI symlink install, document-
            // edited flag, relaunch). `settings_{get,update,reset}` moved
            // off the Tauri surface; the renderer reaches settings data
            // host-aware via `/cli/settings/*`.
            commands::settings::cli_install_status,
            commands::settings::cli_install,
            commands::settings::cli_uninstall,
            commands::settings::set_document_edited,
            commands::settings::set_board_file_unsaved,
            commands::settings::board_file_quit_now,
            commands::settings::set_relaunch_mode,
            commands::settings::stop_bundled_daemon_for_update,
            commands::settings::relaunch_via_open,
            // Phase 2 Unit 6 — `commands::project_config::*` shims
            // deleted. Renderer hits `/cli/project-config/*` on the
            // daemon.
            // Phase 2 Unit 3 — `commands::terminal::*` shims deleted.
            // PTY lifecycle now owned by the daemon; the renderer
            // hits `/cli/terminal/{create,kill,resize,scroll,log,
            // kill-foreground,set-focus,active-count,foreground-cmd,
            // exists,get-grid,list-running}` and the legacy
            // `lifecycle-write` route on k2so-daemon directly. The
            // daemon's TerminalEventSink broadcasts grid/title/exit/
            // bell events over /events WS; daemon_events.rs re-emits
            // via AppHandle::emit so the renderer's
            // `listen('terminal:grid:<id>')` subscribers see the
            // same events as before.
            // Plan B cleanup — the entire `git_*` command surface
            // (info/branches/worktrees/diff/staging/commit/merge) deleted.
            // All routed daemon data; the renderer reaches it host-aware
            // via `/cli/git/*` on the active daemon.
            // Workspace Ops
            // Phase 2 Unit 6 — `commands::chat_history::*` shims
            // deleted. Renderer hits `/cli/chat/*` on the daemon.
            // Plan B cleanup — `timer_*` command surface deleted. Routed
            // daemon data; the renderer reaches it host-aware via
            // `/cli/timer/*` on the active daemon.
            // Updater
            commands::updater::check_for_update,
            commands::updater::get_current_version,
            commands::updater::probe_install_update,
            // SS4: app updates verified with the K2 update keys.
            commands::updater::app_update_check,
            commands::updater::app_update_download,
            commands::updater::app_update_install,
            commands::updater::broadcast_sync,
            commands::linux_pkg_update::install_linux_package_update,
            commands::linux_pkg_update::relaunch_after_linux_package_update,
            commands::linux_pkg_update::local_daemon_update_state,
            // Plan B cleanup — `workspace_layout_*` command surface
            // deleted. Routed daemon data; the renderer reaches it
            // host-aware via `/cli/workspace-layouts/*` on the active
            // daemon.
            // Claude Auth: Phase 2 Unit 5 — moved to daemon
            // (`/cli/claude-auth/*`). Renderer hits the daemon
            // directly; no Tauri command surface remains.
            // K2SO Agents
            commands::k2so_agents::k2so_agents_list,
            commands::k2so_agents::k2so_chat_refresh_broadcast,
            // Plan B cleanup — `states_*` command surface deleted. Routed
            // daemon data; the renderer reaches it host-aware via
            // `/cli/states/*` on the active daemon.
            // Phase 2.1c Item 2 — workspace inbox primitive (replaces
            // the legacy `k2so_agents_work_*` + `k2so_agents_workspace_inbox_list`
            // calls). The badge count forwards to `/cli/inbox/count`
            // (0.46.4; the other inbox shims had no renderer caller).
            commands::inbox::k2so_inbox_count,
            // Phase 2.1 wrap-up — generic worktree fs reader (powers
            // WorktreeDetailPane's Task tab; renders the worktree's
            // CLAUDE.md as Markdown). Path-canonicalized + traversal-
            // rejecting so it's safe to expose to the renderer.
            commands::worktree::read_worktree_file,
            // Phase 2.5b follow-up — Skills panel list; forwards to
            // `/cli/skills/list` (0.46.4).
            commands::skills::k2so_skills_list,
            // Legacy `k2so_agents_work_list` removed — its renderer
            // callers now use `k2so_inbox_list` above. The per-agent
            // work-queue surface is itself being retired with the
            // Phase 2.1 1:1 (workspace==agent) refactor.
            // `k2so_agents_work_create` and `k2so_agents_work_move`
            // had zero frontend callers (confirmed via Phase 2.1c
            // audit) and are removed here.
            // `k2so_onboarding_adopt` / `k2so_onboarding_skip` removed with
            // the canonical-agents PRD (§7): consent gate is gone now that
            // harness fan-out is off by default.
            // Canonical Agent Flow (canonical-agents PRD §4 / §5.2 / §8.1).
            commands::k2so_agents::k2so_detect_canonical_state,
            // Back-compat aliases — retained during the 0.33.0 rename window so
            // stale React `invoke()` names keep working until every call site
            // has migrated to the canonical new names above.
            commands::k2so_agents::k2so_agents_teardown_workspace,
            commands::k2so_agents::k2so_agents_build_launch,
            commands::k2so_agents::k2so_agents_review_queue,
            // `k2so_agents_workspace_inbox_list` and
            // `k2so_agents_workspace_inbox_create` removed (Phase 2.1c
            // Item 2) — renderer migrated to `k2so_inbox_*` above.
            // Phase 2.1 wrap-up (0.39.0f) also retired the core helper
            // `workspace_inbox_create` and its daemon caller
            // `workspace_msg::deliver_to_inbox` — the inbox-delivery
            // path is now `k2_core::inbox::compose` end-to-end.
            commands::k2so_agents::k2so_agents_lock,
            commands::k2so_agents::k2so_agents_install_heartbeat,
            commands::k2so_agents::k2so_agents_uninstall_heartbeat,
            commands::k2so_agents::k2so_agents_apply_wake_scheduler,
            commands::k2so_agents::k2so_agents_update_heartbeat_projects,
            commands::k2so_agents::k2so_agents_preview_schedule,
            // Multi-heartbeat (agent_heartbeats table)
            commands::k2so_agents::k2so_heartbeat_list,
            commands::k2so_agents::k2so_heartbeat_list_all,
            commands::k2so_agents::k2so_heartbeat_fires_list_all,
            commands::k2so_agents::k2so_workspace_get_show_heartbeat_sessions,
            commands::k2so_agents::k2so_heartbeat_set_session,
            agent_hooks::k2so_heartbeat_force_fire,
            agent_hooks::k2so_heartbeat_smart_launch,
            agent_hooks::k2so_heartbeat_active_session,
            agent_hooks::k2so_session_lookup_by_agent,
            agent_hooks::k2so_sessions_list_for_workspace,
            // Agent Sessions (DB-tracked)
            commands::k2so_agents::workspace_session_get,
            commands::k2so_agents::workspace_session_set_session_id,
            // Workspace Relations
            // Agent Skills
            // Agent Editor
            commands::k2so_agents::k2so_agents_get_editor_context,
            // Phase 2 Unit 6 — `commands::review_checklist::*` shims
            // deleted. Renderer hits `/cli/review-checklist/*` on the
            // daemon.
            // Format
            commands::format::format_file,
            commands::format::format_file_check,
            // Phase 2 Unit 6 — `commands::themes::*` shims deleted.
            // Renderer hits `/cli/themes/*` on the daemon.
            // Phase 2 Unit 6 — `commands::skill_layers::*` shims
            // deleted. Renderer hits `/cli/skill-layers/*` on the
            // daemon.
            // k2so-daemon lifecycle (Settings panel reads this to show
            // "daemon: running / not installed / unreachable") and
            // controls the launch agent install/uninstall/restart.
            commands::daemon::daemon_status,
            commands::daemon::daemon_install,
            commands::daemon::daemon_uninstall,
            commands::daemon::daemon_restart,
            commands::daemon::daemon_log_path,
            commands::daemon::daemon_log_tail,
            commands::daemon::daemon_ws_url,
            // Plan B cleanup — `get/set_keep_daemon_on_quit` and the
            // `set_active_daemon` proxy chokepoint deleted. The keep-on-
            // quit toggle is read/written host-aware via `/cli/settings/*`;
            // the global daemon override is gone (DaemonClient always
            // resolves the local daemon, and daemon-data routing is
            // host-aware in the renderer).
            // K2 Connect — cross-platform keychain for remembered
            // remote-host tokens (macOS/Linux/Windows via `keyring`).
            commands::secrets::k2_secret_set,
            commands::secrets::k2_secret_get,
            commands::secrets::k2_secret_delete,
            commands::secrets::k2_account_session_set,
            commands::secrets::k2_account_session_get,
            commands::secrets::k2_account_session_clear,
            // K2 Connect — client address book (non-secret host list)
            // persisted to ~/.k2so/connect-hosts.json.
            commands::connect_hosts::connect_hosts_read,
            commands::connect_hosts::connect_hosts_write,
            commands::connect_hosts::connect_cli_token_upsert,
            commands::connect_hosts::connect_cli_token_delete,
        ])
        .build(tauri::generate_context!())
        .unwrap_or_else(|e| {
            // Pre-webview failure — we can't show a GUI error, so write to
            // stderr (visible in Console.app when launched from Finder) and
            // exit non-zero so the OS reports the crash cleanly. Previously
            // this used .expect which panicked and aborted with a stderr
            // message that failed on some sandboxes.
            use std::io::Write;
            let _ = writeln!(std::io::stderr(), "K2 failed to build Tauri context: {}", e);
            std::process::exit(1);
        })
        .run(|app, event| {
            // Non-mac: `app` is only consumed by the mac-gated Reopen arm.
            #[cfg(not(target_os = "macos"))]
            let _ = &app;
            match event {
                // Cmd+Q / File → Quit / NSApplication terminate land
                // here. Exit the app. Do not unload dev.k2.daemon — the
                // menu-bar helper is a different LaunchAgent and stays,
                // and the row stays Running. The keep-daemon-off unload
                // is only the red-close path above.
                tauri::RunEvent::ExitRequested { .. } => {
                    window::save_window_state(app);
                    // A call in progress: the app `_exit`s below, so nothing
                    // later would tell Sew (vs-live V36). At most a second.
                    commands::native_voice::leave_on_quit(app);
                }
                tauri::RunEvent::Exit => {
                    if RELAUNCH_MODE.load(std::sync::atomic::Ordering::Relaxed) {
                        // Relaunch mode — use normal exit so the spawned process survives
                        std::process::exit(0);
                    } else {
                        // Use _exit() to skip C++ static destructors (ggml_metal).
                        // This handles Cmd+Q (NSApplication terminate:) which bypasses
                        // the window CloseRequested event and goes straight to exit().
                        #[cfg(unix)]
                        unsafe {
                            libc::_exit(0);
                        }
                        #[cfg(not(unix))]
                        std::process::exit(0);
                    }
                }
                // macOS: user clicked the Dock icon while the window was
                // hidden (e.g. they had closed it with the red button and
                // we kept the app alive for heartbeat agents). Re-show
                // the main window instead of opening a new one.
                #[cfg(target_os = "macos")]
                tauri::RunEvent::Reopen { has_visible_windows, .. } => {
                    if !has_visible_windows {
                        if let Some(win) = app.get_webview_window("main") {
                            let _ = win.show();
                            let _ = win.set_focus();
                        }
                    }
                }
                _ => {}
            }
        });
}

// Phase 2 Unit 4: `any_heartbeat_enabled` was dead code (no callers
// in lib.rs after the red-button refactor) — deleted.
// `migrate_workspace_layouts_to_db` moved to
// `k2_core::db_ops::migrate_workspace_layouts_to_db`; the daemon
// runs it on its own first boot now.
