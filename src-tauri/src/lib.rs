#[cfg(target_os = "macos")]
#[macro_use]
extern crate objc;

// Bring k2so-core's `log_debug!` + `perf_timer!` + `perf_hist!` into scope
// across every child module, matching the previous behavior of having
// them defined inline in this file.
#[macro_use]
extern crate k2_core;

/// Flag to skip _exit(0) during relaunch (set by the frontend before process::relaunch)
static RELAUNCH_MODE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);


mod agent_hooks;
mod commands;
mod tray;
// `companion` now lives in k2so-core. Re-exported so existing
// `crate::companion::*` paths (commands/companion.rs, agent_hooks.rs,
// commands/settings.rs) keep working.
pub use k2_core::companion;
// Modules opened for the benches at src-tauri/benches/perf.rs — the k2_lib
// crate is not published, so this is a no-op for real consumers. Revert to
// `mod` once the perf pass is over if we decide the benches' existence
// doesn't justify open modules.
// `db` now lives in k2so-core. Re-exported so callers can keep using
// `crate::db::shared()` etc. unchanged. Migrations are bundled into the
// k2so-core binary via include_str! from crates/k2so-core/drizzle_sql/.
pub use k2_core::db;
// `editors`, `fs_abstract`, `fs_atomic`, `project_config` now live in
// k2so-core (pure std + serde; no Tauri dep). Re-exported so existing
// `crate::editors::*` / `crate::fs_abstract::*` / etc. call sites keep
// working unchanged.
pub use k2_core::{editors, fs_abstract, fs_atomic, project_config};
// `git` module (libgit2 wrappers, worktree/branch/diff/merge) moved to
// k2so-core so k2so_agents_delegate + the daemon's future supervised-
// launch path can share the same code. Re-exported at the historical
// `crate::git::*` path so all existing call sites resolve unchanged.
pub use k2_core::git;
// Phase 2 Unit 2 — `llm` re-export removed. LLM inference + model
// management moved entirely to k2so-daemon; Tauri no longer touches
// llama.cpp or the model lifecycle. The renderer calls /cli/llm/* on
// the daemon directly.
mod menu;
// `perf` now lives in the k2so-core crate. Re-exported so existing
// `crate::perf_timer!` / `crate::perf_hist!` / `crate::perf::*` call sites
// keep working unchanged. See crates/k2so-core/src/perf.rs.
pub use k2_core::{perf, perf_hist, perf_timer};
// Phase 2 close-out (2026-05-23): `mod state` deleted. After Units 1-4
// reduced `AppState` to a single `watchers` field, the struct + the
// `.manage()` registration were pure ceremony around one
// `HashMap<String, RecommendedWatcher>`. Inlined into `watcher.rs`
// as a `LazyLock<Mutex<_>>` static so the only host-side process state
// lives next to the only consumer.
// Tauri-side HTTP client for the k2so-daemon. Routes state-mutating
// commands through the daemon's loopback HTTP instead of running them
// in-process. Small for now (ping + status); grows as daemon handlers
// land.
mod daemon_client;
// Phase 2 Unit 1 — companion bridges (settings + terminal + event
// sink + app event source) moved to k2so-daemon. The daemon now
// owns the ngrok tunnel and registers its own providers against
// k2so-core's companion ambient slots. Tauri no longer needs these
// shims because the renderer talks to `/cli/companion/*` directly.
// Tauri-backed AgentHookEventSink impl registered in setup() — routes
// agent-hook events (agent:lifecycle, agent:reply, sync:projects, …)
// back onto Tauri's event bus.
mod agent_hook_sink;
// Phase 2 Unit 7c — `workspace_regen_provider` module retired. The
// SKILL scaffolding moved to k2so-core in Unit 7b; build_launch now
// calls `workspace::write_workspace_skill_file` directly so the
// daemon-side and Tauri-side regen paths are identical.
// Background subscriber for the daemon's /events WebSocket. Spawned in
// setup() once; reconnects forever so we survive daemon restarts.
mod daemon_events;
// `terminal` now lives in k2so-core. Re-exported so existing
// `crate::terminal::*` paths keep working.
pub use k2_core::terminal;
// Phase 2 Unit 3 — `terminal_event_sink` module removed. The
// daemon owns the TerminalEventSink now (broadcasts events over
// /events WS). Tauri's `daemon_events.rs` re-emits them via
// AppHandle::emit so the renderer's listeners are unchanged.
mod watcher;
mod window;

use tauri::{Emitter, Manager};

/// H7: sync `k2_core::hook_config` with the daemon's port + token so
/// in-process Alacritty children emit `/hook/complete` requests at the
/// daemon (the sole HTTP server post-H7). Reads `~/.k2so/daemon.port`
/// + `~/.k2so/daemon.token` — written by the daemon on startup. Runs
/// off-thread with a small retry loop for the cold-start case where
/// Tauri wins the race against launchd's daemon spawn.
///
/// Best-effort: if the daemon files stay missing for the 5 retries,
/// Tauri boots without hook-config wired. New sessions will still
/// reach the daemon's `/cli/*` routes via CLI tools that read the
/// files dynamically; only the in-process Alacritty hook-script path
/// would be deaf, which is a degraded-but-usable state.
fn prime_hook_config_from_daemon() {
    std::thread::spawn(|| {
        use std::time::Duration;
        let Some(home) = dirs::home_dir() else { return };
        let port_path = home.join(".k2so/daemon.port");
        let token_path = home.join(".k2so/daemon.token");
        for attempt in 0..5 {
            let port_ok = std::fs::read_to_string(&port_path)
                .ok()
                .and_then(|s| s.trim().parse::<u16>().ok());
            let token_ok = std::fs::read_to_string(&token_path)
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            if let (Some(port), Some(token)) = (port_ok, token_ok) {
                k2_core::hook_config::set_port(port);
                k2_core::hook_config::set_token(token);
                log_debug!(
                    "[h7/hook-config] primed from daemon (port={}, attempt={})",
                    port,
                    attempt
                );
                return;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        log_debug!(
            "[h7/hook-config] daemon port/token files unavailable after 5 retries; \
             Alacritty children will boot with empty hook_config"
        );
    });
}

/// Detect daemon/Tauri version mismatch on app startup and bounce the
/// running daemon when they disagree.
///
/// Background: the daemon is launchd-managed (`KeepAlive=true`), so a
/// drag-replace install of K2SO.app overwrites the binary on disk while
/// launchd keeps the OLD daemon process running with the deleted inode
/// — meaning a freshly-installed K2SO talks to last-version's daemon
/// until the user reboots or manually clicks Settings → Restart Daemon.
/// `daemon_restart()` exists for that manual path; this function is
/// the automatic version of the same idea.
///
/// Runs on a background thread (mirrors `prime_hook_config_from_daemon`)
/// because we must wait for the daemon to be reachable + we don't want
/// to block Tauri's setup hook on a synchronous HTTP round-trip. Polls
/// up to 10× at 500ms intervals; if the daemon stays unreachable we
/// log and bow out — bigger problem than version skew at that point.
///
/// 0.39.2: on detected version mismatch, kickstarts the daemon and
/// returns. The renderer-side [`ConnectionGate`] handles waiting +
/// rendering — keeps the Rust shell simple (just signals "restart
/// needed") and centralizes the user-facing "Connecting..." UX +
/// retry logic in React. Same gate pattern is reusable for K2
/// Connect where remote daemons may be transiently unreachable.
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
            if status.version == app_version {
                log_debug!(
                    "[version-check] daemon v{} matches app v{} (attempt={})",
                    status.version,
                    app_version,
                    attempt
                );
                return;
            }
            log_debug!(
                "[version-check] MISMATCH daemon=v{} app=v{} (attempt={}); restarting daemon via launchctl kickstart",
                status.version,
                app_version,
                attempt
            );
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
        log_debug!(
            "[version-check] daemon unreachable after 10 attempts; skipping version check"
        );
    });
}

/// #14 self-heal: if the on-disk daemon LaunchAgent plist records a
/// stale/transient `ProgramArguments[0]` (a path under a now-ejected DMG
/// or a vanished AppTranslocation mount, or a binary that no longer
/// exists), rewrite it to point at the daemon binary bundled next to the
/// CURRENT Tauri exe and reload the agent so it converges.
///
/// Thin IO shell over the pure decision logic in
/// `k2_core::daemon_lifecycle`:
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
///
/// Best-effort: every failure is logged and swallowed so this never
/// blocks startup or the kickstart that follows. Returns `true` when it
/// rewrote + reloaded the plist (caller can log), `false` otherwise.
fn heal_daemon_plist_program() -> bool {
    use k2_core::daemon_lifecycle as dl;

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
    let plist = k2_core::wake::DaemonPlist::canonical(std::path::PathBuf::from("/unused"));
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

    log_debug!(
        "[plist-heal] rewriting daemon plist program: recorded={} (exists={}) → desired={}",
        recorded.display(),
        recorded_exists,
        desired.display()
    );

    // Rewrite the plist to point at the desired (stable, bundled) binary.
    let new_plist = k2_core::wake::DaemonPlist::canonical(desired.clone());
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
    use std::process::Command;
    let uid = unsafe { libc::getuid() };
    let domain = format!("gui/{uid}");

    let bootout = Command::new("launchctl")
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

    let bootstrap = Command::new("launchctl")
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

#[cfg(not(target_os = "macos"))]
fn reload_daemon_launch_agent(_plist_path: &std::path::Path) {}

// Phase 2 Unit 2 — `llm_worker_main` moved to k2so-daemon's
// `llm_host::worker_main`. The daemon now spawns itself with
// `--llm-worker <payload_path>` for inference. Tauri is no longer
// an LLM host; deleted alongside `commands/assistant.rs` and the
// `--llm-worker` arm in `src-tauri/src/main.rs`.

/// 0.39.0: `warm_http_pool_async` (Kessel reqwest-runtime warmup) was
/// removed alongside the Kessel renderer. Kept as an empty stub so
/// `main.rs` doesn't fail to link — it now does nothing, which is
/// correct because the alacritty-v2 WS spawn path doesn't share the
/// blocking reqwest runtime the old Kessel spawn path needed warm.
pub fn warm_http_pool_async() {}

// ── 0.39.x (Issue #6): webview liveness watchdog ─────────────────────
//
// K2SO can land in a black, unresponsive window where the renderer JS
// isn't running — in TWO situations:
//   1. At LAUNCH (esp. after an auto-update over the running process):
//      WKWebView loads index.html but never executes the JS bundle.
//   2. MID-SESSION: the WKWebView content process dies and respawns
//      blank — most commonly when the laptop sleeps and wakes. This is
//      the case a user actually hit ("renderer crashed after my computer
//      took a nap").
// In both, right-click → Reload fixes it, but ordinary users won't know
// to do that, so the app reads as broken.
//
// This is UPSTREAM of every 0.39.2–0.39.5 black-screen defense
// (ConnectionGate, dynamic-import, /boot-status handshake, LocalPaired
// policy) — they all live in the renderer JS and cannot fire when the
// renderer JS itself isn't running. Recovery has to come from Rust.
//
// Mechanism: a renderer HEARTBEAT. `index.tsx` calls `renderer_heartbeat`
// the instant the bundle executes and then on a timer (~3s). A persistent
// Rust watchdog thread tracks the last heartbeat time; if the renderer
// goes silent past a threshold for a couple consecutive checks (so a
// normal sleep/wake where the renderer resumes within a tick doesn't
// false-trip it), it programmatically reloads the webview (the
// equivalent of the manual menu reload, which is known to work) up to
// MAX_RELOADS times, then surfaces a native error sheet. Whenever a
// heartbeat resumes, the watchdog re-arms — so it recovers launch
// failures AND mid-session content-process deaths.
//
// Wall-clock millis of the last renderer heartbeat (0 = none yet).
// SystemTime (not Instant) so a long sleep counts as elapsed time.
static LAST_HEARTBEAT_MS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

fn now_unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Renderer liveness heartbeat — invoked from `index.tsx` the moment the
/// bundle executes and then on a ~3s timer. Stamps the watchdog so it
/// knows the renderer JS is alive and never reloads a working window.
#[tauri::command]
fn renderer_heartbeat() {
    LAST_HEARTBEAT_MS.store(now_unix_millis(), std::sync::atomic::Ordering::SeqCst);
}

/// What the webview watchdog should do this tick. Pulled out as a pure
/// function so the (otherwise webview-bound) state machine — staleness +
/// the confirm-streak + the retry cap — is unit-testable without a real
/// WKWebView.
#[derive(Debug, PartialEq, Eq)]
enum WatchdogAction {
    /// Renderer is heartbeating — reset all counters, keep watching.
    Healthy,
    /// Stale but not yet confirmed (streak below threshold) — wait one
    /// more tick before acting. Guards against a sleep/wake race where
    /// the renderer resumes a beat late.
    Watch,
    /// Stale, confirmed, budget remains — reload now.
    Reload,
    /// Stale, confirmed, budget exhausted — give up + error sheet (once).
    GiveUp,
}

/// `is_stale` = no heartbeat within the staleness window; `stale_streak`
/// = consecutive stale ticks observed so far; `reloads_done` = reloads
/// issued in the current stale episode; `min_stale_streak` = ticks of
/// confirmed staleness required before the first reload; `max_reloads`
/// = reload cap before giving up.
fn watchdog_decision(
    is_stale: bool,
    stale_streak: u32,
    reloads_done: u32,
    min_stale_streak: u32,
    max_reloads: u32,
) -> WatchdogAction {
    if !is_stale {
        WatchdogAction::Healthy
    } else if stale_streak < min_stale_streak {
        WatchdogAction::Watch
    } else if reloads_done >= max_reloads {
        WatchdogAction::GiveUp
    } else {
        WatchdogAction::Reload
    }
}

/// Stage the bundled `frpc` tunnel client (shipped as a Tauri
/// externalBin sidecar at `Contents/MacOS/frpc`) out to
/// `~/.k2so/bin/frpc`, where the daemon's `resolve_frpc` finds it.
///
/// We locate the sidecar next to the current executable — the same
/// `current_exe().parent()` pattern used to find the bundled
/// `k2so-daemon` — which works identically for a release bundle
/// (`K2SO.app/Contents/MacOS/`) and for `tauri dev` (`target/debug/`).
///
/// Idempotent: only copies when the destination is missing or its bytes
/// differ from the sidecar (so app upgrades that bump frpc re-stage).
/// Fault-isolated: every failure is logged and swallowed — staging frpc
/// must never block app startup. Writing the bytes with our own process
/// (rather than the user downloading them) means the staged file carries
/// no `com.apple.quarantine` flag, so Gatekeeper lets it execute.
fn stage_bundled_frpc() {
    // Locate the sidecar next to the running executable.
    let sidecar = match std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("frpc")))
    {
        Some(p) if p.exists() => p,
        Some(p) => {
            log_debug!(
                "[k2so] bundled frpc sidecar not found at {} — skipping stage \
                 (dev build without externalBin, or older bundle)",
                p.display()
            );
            return;
        }
        None => {
            log_debug!("[k2so] could not resolve current_exe to find frpc sidecar");
            return;
        }
    };

    let home = match dirs::home_dir() {
        Some(h) => h,
        None => {
            log_debug!("[k2so] no home dir; cannot stage frpc");
            return;
        }
    };
    let dest = home.join(".k2so/bin/frpc");

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

    log_debug!("[k2so] staged bundled frpc -> {}", dest.display());
}

pub fn run() {
    // Ignore SIGPIPE so writing to a dead PTY returns EPIPE instead of
    // killing the entire process.
    #[cfg(unix)]
    terminal::ignore_sigpipe();

    // launchd-launched .app processes inherit a sparse PATH that lacks
    // ~/.local/bin, /opt/homebrew/bin, and other user-installed prefixes.
    // Source the user's login shell once and adopt its PATH so legacy
    // alacritty spawns + every Command::new call site can resolve user
    // tools. See docs in k2_core::enrich_path_from_login_shell.
    #[cfg(unix)]
    k2_core::enrich_path_from_login_shell();

    // 0.40.0 rebrand — one-time ~/.k2so → ~/.k2 home migration. MUST run
    // before db::init_database() below: the DB now lives at ~/.k2/k2so.db,
    // and opening it first would create a fresh empty ~/.k2 (abandoning
    // the user's data in ~/.k2so and forcing the migration's Conflict
    // branch — caught by the convergence rig, 2026-06-10). The daemon
    // boot runs the same idempotent call; first process wins.
    match k2_core::migration_home::migrate_home_dir() {
        Ok(o) => eprintln!("[boot] home migration: {o:?}"),
        Err(e) => eprintln!("[boot] home migration failed: {e} — continuing on legacy layout"),
    }

    // Rustls 0.23 compiles both aws-lc-rs (via reqwest rustls-tls) and ring
    // (via ngrok) into the binary; it refuses to auto-pick and panics on
    // first TLS use unless a provider is explicitly installed.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    // Phase 2 Unit 1 — companion settings bridge moved to
    // k2so-daemon. The daemon now reads settings.json directly via
    // its own `DaemonCompanionSettingsProvider`. Tauri no longer
    // registers a provider; the renderer reaches the companion
    // tunnel via `/cli/companion/*` on the daemon, so this process
    // doesn't need one.

    let _ = perf_timer!("startup_db_init", {
        match db::init_database() {
            Ok(c) => c,
            Err(e) => {
                log_debug!("[k2so] FATAL: Failed to initialize database: {}", e);
                log_debug!("[k2so] The app will now exit. Check disk permissions and space at ~/.k2so/");
                std::process::exit(1);
            }
        }
    });

    // Phase 2 close-out (2026-05-23): `AppState` deleted entirely.
    // Units 1-4 had already moved every other field (companion, LLM,
    // terminal manager, DB connection) into the daemon; this commit
    // moves the last surviving field — `watchers` — into a
    // module-level `LazyLock<Mutex<_>>` static inside `watcher.rs`.
    // No `.manage()` registration needed; the filesystem watcher
    // commands read the static directly.

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_drag::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .menu(|handle| menu::create_menu(handle))
        .on_menu_event(menu::handle_menu_event)
        .setup(|app| {
            // 0.40.0 — retire com.k2so.* LaunchAgents (the daemon plist
            // re-installs below under dev.k2.daemon; heartbeat +
            // claude-auth re-ensure from the daemon's own boot).
            let swept = k2_core::migration_launchd::migrate_launchd_labels();
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

            // Phase 2 Unit 1 — companion terminal / event-sink /
            // app-event-source bridges moved to k2so-daemon. The
            // daemon registers daemon-owned impls
            // (`crates/k2so-daemon/src/companion_host.rs`) since the
            // tunnel + WS clients now live there. Tauri no longer
            // wires these bridges.

            // Agent-hook event sink: routes k2_core::agent_hooks::emit
            // onto AppHandle::emit. Registered before any hook HTTP
            // request can land.
            k2_core::agent_hooks::set_sink(Box::new(
                agent_hook_sink::TauriAgentHookEventSink::new(app.handle().clone()),
            ));

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
            if let Some(home) = dirs::home_dir() {
                let templates = home.join(".k2so/templates");
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

            // Install the k2so-daemon launchd agent if it isn't already
            // installed AND the daemon binary is bundled next to us.
            // Gated via `code_migrations` so this runs exactly once per
            // upgrade. In debug builds we opt out by default — the
            // `target/debug/k2so-daemon` path is volatile, and a dev
            // with `K2SO_INSTALL_DAEMON=1` can override.
            perf_timer!("startup_install_daemon_plist", {
                // v2 bump: v1 was burned during 0.33.0 RC testing when
                // dev launches with `K2SO_INSTALL_DAEMON=1` marked it
                // applied against an earlier k2so-daemon binary path.
                // Bumping the ID forces a re-install against the current
                // bundled daemon binary on the first 0.33.0 launch for
                // anyone carrying a stale v1 row. Safe for fresh users
                // (neither row present → runs as usual).
                const MIGRATION_ID: &str = "install_daemon_plist_v2";
                let needs_run = {
                    let db_arc = crate::db::shared();
                    let db = db_arc.lock();
                    !db::has_code_migration_applied(&db, MIGRATION_ID)
                };
                let opted_in = !cfg!(debug_assertions)
                    || std::env::var("K2SO_INSTALL_DAEMON").is_ok();
                if needs_run && opted_in {
                    // Locate k2so-daemon next to the current Tauri binary
                    // (inside K2SO.app/Contents/MacOS/). Skip install if
                    // it isn't bundled yet — earlier 0.33.x dev builds
                    // may ship without it.
                    let maybe_daemon = std::env::current_exe()
                        .ok()
                        .and_then(|p| k2_core::daemon_lifecycle::bundled_daemon_path(&p))
                        .filter(|p| p.exists());
                    match maybe_daemon {
                        Some(daemon_bin)
                            if k2_core::daemon_lifecycle::is_transient_exe_location(
                                &daemon_bin,
                            ) =>
                        {
                            // #14: first run straight from the mounted DMG
                            // (or a Gatekeeper-translocated copy) resolves a
                            // TRANSIENT daemon path. Baking that into a
                            // KeepAlive/RunAtLoad plist makes launchd respawn
                            // a binary that vanishes the moment the DMG is
                            // ejected — the "stuck Connecting / version
                            // mismatch / won't pair" trap. Do NOT register
                            // the plist; leave the migration unapplied so a
                            // later /Applications launch installs it for
                            // real. Never block startup.
                            log_debug!(
                                "[k2so] daemon binary is in a transient location ({}); \
                                 skipping plist install — move K2SO to /Applications and \
                                 relaunch to enable the background daemon",
                                daemon_bin.display()
                            );
                        }
                        Some(daemon_bin) => {
                            let plist = k2_core::wake::DaemonPlist::canonical(daemon_bin.clone());
                            match k2_core::wake::install(&plist) {
                                Ok(path) => {
                                    log_debug!(
                                        "[k2so] installed daemon plist at {} pointing at {}",
                                        path.display(),
                                        daemon_bin.display()
                                    );
                                    let db_arc = crate::db::shared();
                                    let db = db_arc.lock();
                                    db::mark_code_migration_applied(
                                        &db,
                                        MIGRATION_ID,
                                        Some(&format!("installed from {}", daemon_bin.display())),
                                    );
                                }
                                Err(e) => {
                                    // Don't mark applied — next launch will
                                    // retry. Common failure: launchctl
                                    // complaining about "Load failed: 5:
                                    // Input/output error" which usually
                                    // means a stale plist is already
                                    // loaded. User can resolve via
                                    // `launchctl unload ~/Library/LaunchAgents/dev.k2.daemon.plist`.
                                    log_debug!("[k2so] daemon plist install failed: {e}");
                                }
                            }
                        }
                        None => {
                            // Bundled daemon missing — common in pre-0.33
                            // dev builds. Leave the migration unapplied
                            // so a later launch (with the daemon bundled)
                            // completes it.
                            log_debug!(
                                "[k2so] daemon binary not found next to current exe; skipping plist install"
                            );
                        }
                    }
                }
            });

            // #14 self-heal at startup: if a previous bad DMG / translocated
            // launch baked a transient ProgramArguments[0] into the plist,
            // a normal /Applications upgrade may not trip the version-mismatch
            // path (the stale daemon could be the SAME version). Rewrite the
            // plist BEFORE the autostart `ensure_loaded` below so launchd
            // loads the corrected program path. No-op when the recorded path
            // is already correct, or when the current exe is itself transient.
            perf_timer!("startup_heal_daemon_plist", {
                if heal_daemon_plist_program() {
                    log_debug!("[k2so] daemon plist self-healed at startup (#14)");
                }
            });

            // Autostart: on every Tauri launch (not just first-install
            // migration), make sure the daemon plist is loaded. Covers
            // the case where the user red-buttoned with "keep server
            // running" OFF (which unloads the plist) and then relaunched
            // the app — they expect the daemon to be back without having
            // to click "Restart" in Settings. Fires regardless of the
            // toggle, in both debug and release builds.
            perf_timer!("startup_ensure_daemon_loaded", {
                let plist = k2_core::wake::DaemonPlist::canonical(
                    std::path::PathBuf::from("/unused"),
                );
                match k2_core::wake::ensure_loaded(&plist) {
                    Ok(k2_core::wake::LoadOutcome::AlreadyLoaded) => {
                        log_debug!("[k2so] daemon already loaded in launchctl");
                    }
                    Ok(k2_core::wake::LoadOutcome::Loaded) => {
                        log_debug!("[k2so] daemon plist loaded (was unloaded)");
                    }
                    Ok(k2_core::wake::LoadOutcome::NotInstalled) => {
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
                            .and_then(|p| k2_core::daemon_lifecycle::bundled_daemon_path(&p))
                            .filter(|p| p.exists())
                            .filter(|p| !k2_core::daemon_lifecycle::is_transient_exe_location(p));
                        match healed {
                            Some(daemon_bin) => {
                                let plist =
                                    k2_core::wake::DaemonPlist::canonical(daemon_bin.clone());
                                match k2_core::wake::install(&plist)
                                    .and_then(|_| k2_core::wake::ensure_loaded(&plist))
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
            });

            // Phase 2 Unit 4 — SKILL regeneration moved to the daemon's
            // boot sweep (`run_workspace_legacy_migrations_sweep` →
            // `ensure_all_skills_up_to_date`). The per-version
            // `skill_regen_version` gate this block used was replaced
            // by per-file checksum gates inside k2so-core's
            // `ensure_skill_up_to_date`, so a stale `skill_regen_version`
            // value is harmless — the writers don't touch disk if the
            // file already matches. Block deleted; daemon owns the pass.

            // Apply saved window state on startup
            if let Some(saved) = window::load_window_state(app.handle()) {
                if let Some(win) = app.get_webview_window("main") {
                    use tauri::PhysicalPosition;
                    use tauri::PhysicalSize;
                    let _ = win.set_position(PhysicalPosition::new(saved.x, saved.y));
                    let _ = win.set_size(PhysicalSize::new(saved.width, saved.height));
                    if saved.is_maximized {
                        let _ = win.maximize();
                    }
                }
            }
            // Native WebKit zoom is disabled via zoomHotkeysEnabled:false in tauri.conf.json.
            // App zoom is handled by transform:scale() in the frontend (App.tsx).

            // Save window state and clean up terminals on close
            let app_handle = app.handle().clone();
            if let Some(win) = app.get_webview_window("main") {
                let win_for_hide = win.clone();
                win.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        // Red close-button behavior is controlled by
                        // the "Keep Agent & Companion server running
                        // when K2SO quits" preference:
                        //
                        //   ON  → hide the window, keep Tauri + server
                        //         alive. Menubar icon stays visible so
                        //         the user can see what's still running.
                        //         Full quit happens only via Cmd+Q or
                        //         the menubar "Quit K2SO" item.
                        //   OFF → behave like a normal app quit: tear
                        //         down in-app companion + daemon plist
                        //         (if installed), then proceed to
                        //         destroy.
                        //
                        // Cmd+Q is deliberately NOT routed through here
                        // (NSApplication terminate: goes straight to
                        // RunEvent::ExitRequested) — it always closes
                        // everything regardless of the toggle.
                        // Phase 2 Unit 7c — read directly from
                        // k2so-core to drop the last Tauri-side
                        // `read_settings()` call site. Daemon-owned
                        // writes still synchronize because both
                        // sides read the same `~/.k2so/settings.json`
                        // through `k2_core::app_settings::load`.
                        let keep_running =
                            k2_core::app_settings::load().keep_daemon_on_quit;
                        if keep_running {
                            window::save_window_state(&app_handle);
                            api.prevent_close();
                            let _ = win_for_hide.hide();
                            log_debug!("[window] Close intercepted — keeping server alive per settings");
                            return;
                        }
                        // Toggle OFF — user wants red-dot to take
                        // everything down. Unload the daemon plist (if
                        // installed) so launchd stops respawning it,
                        // then fall through to the normal cleanup +
                        // destroy path below.
                        let plist = k2_core::wake::DaemonPlist::canonical(
                            std::path::PathBuf::from("/unused"),
                        );
                        if let Some(path) = plist.plist_path() {
                            if path.exists() {
                                let _ = k2_core::wake::launchctl_unload(&path);
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

                        // Use _exit() to skip C++ static destructors (ggml_metal).
                        // Without this, __cxa_finalize_ranges runs ggml's Metal cleanup
                        // which races against macOS Metal device teardown → SIGABRT.
                        // Skip _exit during relaunch so the process plugin can spawn
                        // the new process before this one exits.
                        if RELAUNCH_MODE.load(std::sync::atomic::Ordering::Relaxed) {
                            log_debug!("[shutdown] Relaunch mode — using normal exit");
                            std::process::exit(0);
                        } else {
                            unsafe { libc::_exit(0); }
                        }
                    }
                });
            }
            // Phase 4 H7: Tauri no longer runs its own HTTP listener.
            // The k2so-daemon (launchd-managed) is the sole server for
            // every /cli/* route + /hook/complete + /events WS, and
            // writes heartbeat.port + heartbeat.token itself on startup.
            //
            // What Tauri still does here:
            //   1. Write the notify.sh hook script to disk. The script
            //      reads heartbeat.port at exec time, so it doesn't
            //      need Tauri's port — just needs to exist on disk for
            //      `register_all_hooks` to point ~/.claude/settings.json
            //      at it.
            //   2. Register those hooks with claude/cursor/etc so their
            //      lifecycle events curl into the daemon's /hook/complete.
            //   3. Sync hook_config so in-process Alacritty children
            //      inject the daemon's port + token into child envs —
            //      handled by `prime_hook_config_from_daemon` below.
            //
            // What Tauri no longer does (moved to daemon):
            //   - Bind a TCP listener. The old `agent_hooks::start_server`
            //     call is gone; its 60+ /cli/* routes are all served by
            //     k2so-daemon now.
            //   - Write heartbeat.port / heartbeat.token. Daemon does
            //     it eagerly on startup.
            //   - Clean up stale heartbeat.port files. Same reasoning:
            //     daemon is the owner, not us.
            match agent_hooks::write_hook_script(0) {
                Ok(script_path) => {
                    agent_hooks::register_all_hooks(&app.handle().clone(), &script_path);
                    log_debug!("[agent-hooks] Hook scripts registered at {}", script_path);
                }
                Err(e) => {
                    log_debug!("[agent-hooks] Failed to write hook script: {}", e);
                    let _ = app.handle().emit(
                        "hook-injection-failed",
                        serde_json::json!({
                            "failures": [{"cli": "notify-script", "error": e}]
                        }),
                    );
                }
            }
            prime_hook_config_from_daemon();
            check_daemon_version_and_restart();
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

            // Phase 2 Unit 1 — companion ngrok tunnel autostart moved
            // to k2so-daemon. The daemon now reads `companion.auto_start`
            // from `~/.k2so/settings.json` on its own first boot and
            // brings the tunnel up itself (see
            // `crates/k2so-daemon/src/companion_host.rs::maybe_autostart`).
            //
            // Owning the tunnel daemon-side closes the K2SO Connect +
            // Mobile Companion gap: with Tauri closed (or running on a
            // different machine entirely), the daemon's tunnel stays
            // reachable.

            // Phase 2 Unit 2 — LLM stale-tmp cleanup, default-model
            // auto-download, and model auto-load moved to k2so-daemon's
            // first-boot pass (see `llm_host::maybe_first_boot_discover`).
            // The renderer kicks off downloads via /cli/llm/download-default
            // and polls /cli/llm/status for readiness. Tauri is no longer
            // involved in the LLM lifecycle.

            // Menubar / system tray icon. Pairs with the persistent-
            // agents feature: once Cmd+Q leaves the daemon running,
            // users need a surface that shows what's still active.
            // Failures here are non-fatal — the app works without a
            // tray, users just lose visibility into the daemon from
            // outside the main window.
            if let Err(e) = tray::install(&app.handle().clone()) {
                log_debug!("[tray] install failed: {e} (continuing without tray)");
            }

            // 0.39.x (Issue #6): webview liveness watchdog. See the
            // doc on `renderer_heartbeat` above. Persistent: it recovers
            // BOTH the launch black-screen (renderer JS never runs at
            // startup) AND mid-session content-process death (e.g. the
            // renderer crashing after the laptop sleeps + wakes), by
            // tracking the renderer's heartbeat and reloading the webview
            // from Rust when the heartbeat goes stale — then a native
            // error sheet if reloads don't bring it back.
            if let Some(win) = app.get_webview_window("main") {
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    use std::sync::atomic::Ordering;
                    // Check cadence. Recovery latency ≈ MIN_STALE_STREAK
                    // ticks once staleness begins (~6s).
                    const TICK: std::time::Duration = std::time::Duration::from_secs(3);
                    // No heartbeat for this long ⇒ stale. The renderer
                    // beats every ~3s, so 9s tolerates ~2 missed beats of
                    // ordinary main-thread jank without a false reload.
                    const STALE_MS: u64 = 9_000;
                    // Require this many consecutive stale ticks before the
                    // first reload — kills the sleep/wake race where the
                    // surviving renderer resumes a beat late.
                    const MIN_STALE_STREAK: u32 = 2;
                    const MAX_RELOADS: u32 = 3;

                    let mut stale_streak = 0u32;
                    let mut reloads = 0u32;
                    let mut gave_up = false;
                    loop {
                        std::thread::sleep(TICK);
                        let last = LAST_HEARTBEAT_MS.load(Ordering::SeqCst);
                        // last == 0 → renderer has never beaten yet (launch
                        // window). Treat as stale so the launch case is
                        // covered too.
                        let is_stale = last == 0
                            || now_unix_millis().saturating_sub(last) > STALE_MS;
                        match watchdog_decision(
                            is_stale,
                            stale_streak,
                            reloads,
                            MIN_STALE_STREAK,
                            MAX_RELOADS,
                        ) {
                            WatchdogAction::Healthy => {
                                // Heartbeat present — renderer is alive.
                                // Re-arm so a LATER crash (sleep/wake) is
                                // caught fresh.
                                if reloads > 0 {
                                    log_debug!(
                                        "[webview-watchdog] renderer recovered after {reloads} reload(s)"
                                    );
                                }
                                stale_streak = 0;
                                reloads = 0;
                                gave_up = false;
                            }
                            WatchdogAction::Watch => {
                                stale_streak += 1;
                            }
                            WatchdogAction::Reload => {
                                stale_streak += 1;
                                reloads += 1;
                                log_debug!(
                                    "[webview-watchdog] renderer heartbeat stale — reloading webview \
                                     (attempt {reloads}/{MAX_RELOADS})"
                                );
                                // Programmatic equivalent of right-click → Reload.
                                let _ = win.eval("window.location.reload()");
                            }
                            WatchdogAction::GiveUp => {
                                // Show the error sheet + log ONCE per stale
                                // episode (re-armed when a heartbeat
                                // resumes). Keep watching, but stop
                                // reloading so we never loop forever.
                                if !gave_up {
                                    gave_up = true;
                                    log_debug!(
                                        "[webview-watchdog] renderer still silent after {MAX_RELOADS} \
                                         reloads — giving up; surfacing error sheet"
                                    );
                                    if let Ok(home) = std::env::var("HOME") {
                                        let log_path = std::path::Path::new(&home)
                                            .join(".k2")
                                            .join("webview-watchdog.log");
                                        if let Ok(mut f) = std::fs::OpenOptions::new()
                                            .create(true)
                                            .append(true)
                                            .open(&log_path)
                                        {
                                            use std::io::Write;
                                            let _ = writeln!(
                                                f,
                                                "renderer JS unresponsive; {MAX_RELOADS} programmatic reloads failed",
                                            );
                                        }
                                    }
                                    use tauri_plugin_dialog::DialogExt;
                                    handle
                                        .dialog()
                                        .message(
                                            "K2SO stopped responding. Please quit and relaunch the app.\n\n\
                                             If this keeps happening, reinstalling the latest version usually fixes it.",
                                        )
                                        .title("K2SO couldn't load")
                                        .blocking_show();
                                }
                            }
                        }
                    }
                });
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // 0.39.x (Issue #6): webview liveness watchdog heartbeat.
            renderer_heartbeat,
            // Projects — host-only verbs (folder picker, icon upload,
            // OS-integration "open in" actions, editor discovery, focus
            // window). All DB-backed project CRUD moved off the Tauri
            // surface; the renderer reaches it host-aware via `/cli/*`.
            commands::projects::projects_pick_folder,
            commands::projects::projects_open_in_finder,
            commands::projects::projects_upload_icon,
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
            // 0.37.9 — macOS permissions surface for Settings UI
            commands::permissions::permissions_get_status,
            commands::permissions::permissions_request_full_disk_access,
            commands::permissions::permissions_request_accessibility,
            commands::permissions::permissions_request_microphone,
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
            commands::settings::set_relaunch_mode,
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
            commands::workspace_ops::workspace_split_pane,
            commands::workspace_ops::workspace_close_pane,
            commands::workspace_ops::workspace_open_document,
            commands::workspace_ops::workspace_open_terminal,
            commands::workspace_ops::workspace_new_tab,
            commands::workspace_ops::workspace_close_tab,
            commands::workspace_ops::workspace_arrange,
            // Phase 2 Unit 2 — `assistant_*` commands deleted. LLM
            // moved to k2so-daemon (/cli/llm/*); renderer calls the
            // daemon directly.
            // Phase 2 Unit 6 — `commands::chat_history::*` shims
            // deleted. Renderer hits `/cli/chat/*` on the daemon.
            // Plan B cleanup — `timer_*` command surface deleted. Routed
            // daemon data; the renderer reaches it host-aware via
            // `/cli/timer/*` on the active daemon.
            // Updater
            commands::updater::check_for_update,
            commands::updater::get_current_version,
            commands::updater::broadcast_sync,
            // Plan B cleanup — `workspace_layout_*` command surface
            // deleted. Routed daemon data; the renderer reaches it
            // host-aware via `/cli/workspace-layouts/*` on the active
            // daemon.
            // Claude Auth: Phase 2 Unit 5 — moved to daemon
            // (`/cli/claude-auth/*`). Renderer hits the daemon
            // directly; no Tauri command surface remains.
            // K2SO Agents
            commands::k2so_agents::k2so_agents_list,
            commands::k2so_agents::k2so_agents_create,
            commands::k2so_agents::k2so_agents_delete,
            commands::k2so_agents::k2so_agents_get_heartbeat,
            commands::k2so_agents::k2so_agents_set_heartbeat,
            commands::k2so_agents::k2so_agents_scheduler_tick,
            commands::k2so_agents::k2so_agents_heartbeat_noop,
            commands::k2so_agents::k2so_agents_heartbeat_action,
            commands::k2so_agents::k2so_agents_save_session_id,
            commands::k2so_agents::k2so_session_set_surfaced,
            commands::k2so_agents::k2so_chat_refresh_broadcast,
            commands::k2so_agents::k2so_agents_clear_session_id,
            // Plan B cleanup — `states_*` command surface deleted. Routed
            // daemon data; the renderer reaches it host-aware via
            // `/cli/states/*` on the active daemon.
            // Phase 2.1c Item 2 — workspace inbox primitive (replaces
            // the legacy `k2so_agents_work_*` + `k2so_agents_workspace_inbox_list`
            // calls). Thin wrappers around `k2_core::inbox::*` that
            // mirror the daemon-side `/cli/inbox/*` routes.
            commands::inbox::k2so_inbox_list,
            commands::inbox::k2so_inbox_count,
            commands::inbox::k2so_inbox_folders,
            commands::inbox::k2so_inbox_read,
            commands::inbox::k2so_inbox_search,
            commands::inbox::k2so_inbox_compose,
            commands::inbox::k2so_inbox_move,
            commands::inbox::k2so_inbox_archive,
            commands::inbox::k2so_inbox_delete,
            commands::inbox::k2so_inbox_respond,
            // Phase 2.1 wrap-up — generic worktree fs reader (powers
            // WorktreeDetailPane's Task tab; renders the worktree's
            // CLAUDE.md as Markdown). Path-canonicalized + traversal-
            // rejecting so it's safe to expose to the renderer.
            commands::worktree::read_worktree_file,
            // Phase 2.5b follow-up — Skills CRUD Tauri verbs. The
            // workspace settings "Skills" panel uses these to read /
            // write `.k2so/skills/<name>/SKILL.md` directly without
            // routing through the daemon HTTP surface. Thin forwards
            // to `k2_core::skills::crud::*`.
            commands::skills::k2so_skills_list,
            commands::skills::k2so_skills_profile,
            commands::skills::k2so_skills_create,
            commands::skills::k2so_skills_remove,
            // Legacy `k2so_agents_work_list` removed — its renderer
            // callers now use `k2so_inbox_list` above. The per-agent
            // work-queue surface is itself being retired with the
            // Phase 2.1 1:1 (workspace==agent) refactor.
            commands::k2so_agents::k2so_agents_delegate,
            // `k2so_agents_work_create` and `k2so_agents_work_move`
            // had zero frontend callers (confirmed via Phase 2.1c
            // audit) and are removed here.
            commands::k2so_agents::k2so_agents_get_profile,
            commands::k2so_agents::k2so_agents_regenerate_workspace_skill,
            commands::k2so_agents::k2so_onboarding_scan,
            // `k2so_onboarding_adopt` / `k2so_onboarding_skip` removed with
            // the canonical-agents PRD (§7): consent gate is gone now that
            // harness fan-out is off by default.
            commands::k2so_agents::k2so_onboarding_start_fresh,
            // Canonical Agent Flow (canonical-agents PRD §4 / §5.2 / §8.1).
            commands::k2so_agents::k2so_harness_fanout_enabled,
            commands::k2so_agents::k2so_set_harness_fanout_enabled,
            commands::k2so_agents::k2so_detect_canonical_state,
            commands::k2so_agents::k2so_write_opt_in_skill,
            // Back-compat aliases — retained during the 0.33.0 rename window so
            // stale React `invoke()` names keep working until every call site
            // has migrated to the canonical new names above.
            commands::k2so_agents::k2so_agents_teardown_workspace,
            commands::k2so_agents::k2so_agents_preview_workspace_ingest,
            commands::k2so_agents::k2so_agents_run_workspace_ingest,
            commands::k2so_agents::k2so_agents_generate_workspace_claude_md,
            commands::k2so_agents::k2so_agents_disable_workspace_claude_md,
            commands::k2so_agents::k2so_agents_build_launch,
            commands::k2so_agents::k2so_agents_review_queue,
            commands::k2so_agents::k2so_agents_review_approve,
            commands::k2so_agents::k2so_agents_review_reject,
            commands::k2so_agents::k2so_agents_review_request_changes,
            // `k2so_agents_workspace_inbox_list` and
            // `k2so_agents_workspace_inbox_create` removed (Phase 2.1c
            // Item 2) — renderer migrated to `k2so_inbox_*` above.
            // Phase 2.1 wrap-up (0.39.0f) also retired the core helper
            // `workspace_inbox_create` and its daemon caller
            // `workspace_msg::deliver_to_inbox` — the inbox-delivery
            // path is now `k2_core::inbox::compose` end-to-end.
            commands::k2so_agents::k2so_agents_lock,
            commands::k2so_agents::k2so_agents_triage_decide,
            commands::k2so_agents::k2so_agents_install_heartbeat,
            commands::k2so_agents::k2so_agents_uninstall_heartbeat,
            commands::k2so_agents::k2so_agents_apply_wake_scheduler,
            commands::k2so_agents::k2so_agents_update_heartbeat_projects,
            commands::k2so_agents::k2so_agents_preview_schedule,
            // Multi-heartbeat (agent_heartbeats table)
            commands::k2so_agents::k2so_heartbeat_add,
            commands::k2so_agents::k2so_heartbeat_list,
            commands::k2so_agents::k2so_heartbeat_list_all,
            commands::k2so_agents::k2so_heartbeat_fires_list_all,
            commands::k2so_agents::k2so_heartbeat_list_archived,
            commands::k2so_agents::k2so_heartbeat_archive,
            commands::k2so_agents::k2so_heartbeat_unarchive,
            commands::k2so_agents::k2so_heartbeat_remove,
            commands::k2so_agents::k2so_workspace_get_show_heartbeat_sessions,
            commands::k2so_agents::k2so_workspace_set_show_heartbeat_sessions,
            commands::k2so_agents::k2so_heartbeat_set_enabled,
            commands::k2so_agents::k2so_heartbeat_set_use_workspace_session,
            commands::k2so_agents::k2so_heartbeat_edit,
            commands::k2so_agents::k2so_heartbeat_rename,
            commands::k2so_agents::k2so_heartbeat_fires_list,
            agent_hooks::k2so_heartbeat_force_fire,
            agent_hooks::k2so_heartbeat_smart_launch,
            agent_hooks::k2so_heartbeat_active_session,
            agent_hooks::k2so_session_lookup_by_agent,
            agent_hooks::k2so_sessions_list_for_workspace,
            // Agent Sessions (DB-tracked)
            commands::k2so_agents::workspace_session_get,
            commands::k2so_agents::workspace_session_set_session_id,
            // Workspace Relations
            commands::k2so_agents::workspace_relations_list,
            commands::k2so_agents::workspace_relations_list_incoming,
            commands::k2so_agents::workspace_relations_create,
            commands::k2so_agents::workspace_relations_delete,
            // Agent Skills
            commands::k2so_agents::k2so_agents_regenerate_skills,
            // Agent Editor
            commands::k2so_agents::k2so_agents_get_editor_context,
            commands::k2so_agents::k2so_agents_save_agent_md,
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
            // Phase 2 Unit 1 — companion API commands deleted; the
            // renderer now calls `/cli/companion/{start,stop,status,
            // set-password,disconnect-session}` on the daemon
            // directly (see `CompanionSection.tsx`).
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
            // K2 Connect — client address book (non-secret host list)
            // persisted to ~/.k2so/connect-hosts.json.
            commands::connect_hosts::connect_hosts_read,
            commands::connect_hosts::connect_hosts_write,
        ])
        .build(tauri::generate_context!())
        .unwrap_or_else(|e| {
            // Pre-webview failure — we can't show a GUI error, so write to
            // stderr (visible in Console.app when launched from Finder) and
            // exit non-zero so the OS reports the crash cleanly. Previously
            // this used .expect which panicked and aborted with a stderr
            // message that failed on some sandboxes.
            use std::io::Write;
            let _ = writeln!(std::io::stderr(), "K2SO failed to build Tauri context: {}", e);
            std::process::exit(1);
        })
        .run(|app, event| {
            match event {
                // Cmd+Q / File → Quit / Menubar "Quit K2SO" /
                // NSApplication terminate: all land here. Semantic
                // choice ratified with rosson: these always kill
                // everything, regardless of the keep-running toggle.
                // That toggle ONLY controls the red close-button
                // behavior (handled in on_window_event above).
                //
                // So: unconditionally unload the daemon plist (if
                // installed), then let exit proceed. The in-app
                // companion server dies with the Tauri process.
                tauri::RunEvent::ExitRequested { .. } => {
                    let plist = k2_core::wake::DaemonPlist::canonical(
                        std::path::PathBuf::from("/unused"),
                    );
                    if let Some(path) = plist.plist_path() {
                        if path.exists() {
                            // Best-effort — errors swallowed so a
                            // hung launchctl can't block the quit.
                            let _ = k2_core::wake::launchctl_unload(&path);
                        }
                    }
                }
                tauri::RunEvent::Exit => {
                    if RELAUNCH_MODE.load(std::sync::atomic::Ordering::Relaxed) {
                        // Relaunch mode — use normal exit so the spawned process survives
                        std::process::exit(0);
                    } else {
                        // Use _exit() to skip C++ static destructors (ggml_metal).
                        // This handles Cmd+Q (NSApplication terminate:) which bypasses
                        // the window CloseRequested event and goes straight to exit().
                        unsafe { libc::_exit(0); }
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

#[cfg(test)]
mod webview_watchdog_tests {
    use super::{watchdog_decision, WatchdogAction};

    const MIN_STREAK: u32 = 2;
    const MAX_RELOADS: u32 = 3;

    #[test]
    fn healthy_when_not_stale_regardless_of_counters() {
        // A live heartbeat always resets — never reload a working window.
        assert_eq!(
            watchdog_decision(false, 0, 0, MIN_STREAK, MAX_RELOADS),
            WatchdogAction::Healthy
        );
        assert_eq!(
            watchdog_decision(false, 5, 3, MIN_STREAK, MAX_RELOADS),
            WatchdogAction::Healthy
        );
    }

    #[test]
    fn stale_below_streak_only_watches() {
        // First confirmed-staleness tick(s) wait — guards the sleep/wake
        // race where the surviving renderer resumes one beat late.
        assert_eq!(
            watchdog_decision(true, 0, 0, MIN_STREAK, MAX_RELOADS),
            WatchdogAction::Watch
        );
        assert_eq!(
            watchdog_decision(true, 1, 0, MIN_STREAK, MAX_RELOADS),
            WatchdogAction::Watch
        );
    }

    #[test]
    fn stale_confirmed_with_budget_reloads() {
        // Streak met, budget remains → reload.
        assert_eq!(
            watchdog_decision(true, 2, 0, MIN_STREAK, MAX_RELOADS),
            WatchdogAction::Reload
        );
        assert_eq!(
            watchdog_decision(true, 9, 2, MIN_STREAK, MAX_RELOADS),
            WatchdogAction::Reload
        );
    }

    #[test]
    fn stale_confirmed_at_cap_gives_up() {
        // Budget exhausted → give up (error sheet), not a 4th reload.
        assert_eq!(
            watchdog_decision(true, 2, 3, MIN_STREAK, MAX_RELOADS),
            WatchdogAction::GiveUp
        );
        assert_eq!(
            watchdog_decision(true, 9, 4, MIN_STREAK, MAX_RELOADS),
            WatchdogAction::GiveUp
        );
    }

    #[test]
    fn full_episode_watches_then_three_reloads_then_gives_up() {
        // Walk the loop's exact accounting for a renderer that stays
        // stale the whole time: Watch×(MIN_STREAK-1 extra), then 3
        // Reloads, then GiveUp — never a 4th reload.
        let mut stale_streak = 0u32;
        let mut reloads = 0u32;
        let mut actions = Vec::new();
        for _ in 0..8 {
            let a = watchdog_decision(true, stale_streak, reloads, MIN_STREAK, MAX_RELOADS);
            actions.push(format!("{a:?}"));
            match a {
                WatchdogAction::Watch => stale_streak += 1,
                WatchdogAction::Reload => {
                    stale_streak += 1;
                    reloads += 1;
                }
                WatchdogAction::GiveUp => break,
                WatchdogAction::Healthy => unreachable!(),
            }
        }
        assert_eq!(
            actions,
            vec!["Watch", "Watch", "Reload", "Reload", "Reload", "GiveUp"],
            "2 watch ticks then exactly 3 reloads then give up",
        );
    }

    #[test]
    fn recovery_resets_so_a_later_crash_is_caught_fresh() {
        // After reloads bring the renderer back (Healthy), the loop
        // resets its counters; a subsequent crash must start a fresh
        // episode (Watch → … → Reload), not jump straight to GiveUp.
        // Simulate: stale episode partway, then heartbeat resumes, then
        // stale again.
        let mut stale_streak = 2u32;
        let mut reloads = 2u32;
        // Heartbeat resumes:
        assert_eq!(
            watchdog_decision(false, stale_streak, reloads, MIN_STREAK, MAX_RELOADS),
            WatchdogAction::Healthy
        );
        stale_streak = 0;
        reloads = 0;
        // New crash later → fresh episode starts with Watch, not GiveUp.
        assert_eq!(
            watchdog_decision(true, stale_streak, reloads, MIN_STREAK, MAX_RELOADS),
            WatchdogAction::Watch
        );
    }
}
