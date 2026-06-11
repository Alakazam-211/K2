//! K2SO core library.
//!
//! Home of the device-local runtime that was previously embedded inside
//! `src-tauri/src/`: SQLite database, llama-cpp LLM integration, Alacritty
//! terminal backend, companion WebSocket server, heartbeat scheduler,
//! agent-lifecycle HTTP hooks, and the pluggable `PushTarget` interface for
//! notification delivery.
//!
//! Both the `k2so-daemon` binary and the `src-tauri` Tauri app link this
//! crate so the core logic executes in exactly one place — the daemon —
//! while the Tauri app stays a thin client that proxies state-mutating
//! commands over HTTP.
//!
//! Module migration from src-tauri lands incrementally.

/// Safe `eprintln!` replacement that silently ignores stderr write
/// failures. When K2SO is launched from Finder there's no tty attached and
/// the default `eprintln!` panics on broken-pipe, which then cascades into
/// abort(). This macro swallows the write result instead.
///
/// `#[macro_export]` so both k2so-core-internal modules and downstream
/// crates (src-tauri, k2so-daemon) can share one definition.
#[macro_export]
macro_rules! log_debug {
    ($($arg:tt)*) => {{
        use std::io::Write;
        let _ = writeln!(std::io::stderr(), $($arg)*);
    }};
}

pub mod active;
pub mod agent_hooks;
// Phase 2.5e: `agents` module retired. All historical homes for
// agent-scoped code redistributed across `workspace/`, `skills/`,
// `heartbeats/`, `deprecated/`, `migrations/`, top-level
// `connections.rs`, etc. The directory + module are deleted as of
// Phase 2.5e; downstream callers (daemon CLI + Tauri commands) use
// the canonical post-relocation paths directly.
pub mod app_settings;
pub mod chat_history;
// K2 Connect "Clone to" — the pure bundle engine (inventory the three
// state locations, scrub secrets, exclude bulk, tar/gz a manifest-driven
// bundle). Powers both the high-bar push (P2) and the README fallback
// (P3). HTTP/Tauri-free; see module docs + PRD k2-connect-clone-to.md.
pub mod clone;
pub mod companion;
// Phase 2.5e: workspace-to-workspace connections registry (relocated
// from `agents/connections.rs`). Powers `k2so connections list/add/
// remove`. Not workspace-scoped — operates on `workspace_relations`
// linking two projects together.
pub mod connections;
// K2SO #617: connect-user accounts (username+password, owner-provisioned)
// + in-memory login sessions. The auth boundary for the PUBLIC K2 Connect
// tunnel surface. See module docs.
pub mod connect_users;
// Headless-friendly daemon lifecycle abstractions (label, plist
// generation, launchctl arg vectors). Shared between Tauri's
// `daemon_install` / `daemon_restart` commands and K2 Connect's
// remote-daemon bootstrap path so both agree on the canonical
// label + plist shape.
pub mod daemon_lifecycle;
pub mod db;
pub mod migration_home;
pub mod migration_launchd;
pub mod workspace_dir;
pub use workspace_dir::workspace_dot_dir;
pub mod db_ops;
// Phase 2.5c: retired-but-preserved surface; modules here are
// `#[deprecated]` and exist only for the deprecation window.
pub mod deprecated;
pub mod editors;
pub mod fs_abstract;
pub mod fs_atomic;
pub mod fs_commands;
pub mod git;
// Phase 2.5c: workspace heartbeat schedules + launchd + cron live here
// post-refactor. Was [`agents::heartbeat`]; the `agents/` back-compat
// alias remains for one release cycle.
pub mod heartbeats;
// Phase 2.5c: workspace lifecycle + state + identity + launch helpers.
// Post-Phase-2.1 "workspace == agent" 1:1 means these are
// workspace-scoped concerns; was scattered under `agents/`.
pub mod workspace;
pub mod hook_config;
pub mod inbox;
// Phase 2.5c: historical migration helpers (one-shot, sentinel-gated).
pub mod migrations;
pub mod llm;
pub mod perf;
// Phase 2.5 follow-up — daemon port stability (sub-fix A of finding #547).
pub mod port_claim;
pub mod project_config;
pub mod projects_ops;
pub mod push;
pub mod review_checklist;
pub mod safe_delete;
// 0.39.0: test-aware trash wrapper for production call sites that are
// exercised by cargo tests + bash CLI sandbox tests. Bypasses the
// `trash` crate (and the macOS AppleEvents Touch ID prompt) for
// paths under `std::env::temp_dir()`. Production user-content paths
// keep going through `safe_delete::trash`. See module docs.
pub mod safe_delete_scratch;
pub mod scheduler;
pub mod skill_layers;
// Phase 2.5b — unified home for documentation profiles. Consolidates
// the three legacy folders (`.k2so/agents/`, `.k2so/agent-templates/`,
// bare-md `.k2so/skills/*.md`) into `.k2so/skills/<name>/SKILL.md`
// via a first-boot daemon migration. See [`skills`] module docs.
pub mod skills;
pub mod terminal;
pub mod themes;
// K2 Connect tunnel connector (open/MIT client side): renders an frpc
// config + supervises the `frpc` child that dials our hosted frps so the
// local daemon is reachable at https://<user>.k2.dev. See module docs +
// the proprietary `k2-connect` repo for the server contract.
pub mod tunnel;
pub mod wake;
pub mod whats_new;

// Session Stream primitives — landed in 0.34.0 behind a feature flag;
// the flag was retired in 0.39.0e (these are always-on now). See
// .k2so/prds/session-stream-and-awareness-bus.md for the original rollout.
pub mod awareness;
pub mod session;
pub mod term;

/// Replace this process's `PATH` with the one a fresh login shell
/// would produce.
///
/// Why: macOS launchd does not source `.zshrc` / `.bash_profile` when
/// it starts jobs. Both K2SO's Tauri `.app` and `k2so-daemon` are
/// launchd children, so they inherit the kernel's sparse default
/// (`/usr/bin:/bin:/usr/sbin:/sbin`). User binaries installed under
/// `~/.local/bin`, `/opt/homebrew/bin`, `/usr/local/bin`, or any
/// language-runtime prefix (`~/.cargo/bin`, `~/.bun/bin`, npm globals,
/// etc.) live outside that and are unfindable by `posix_spawn` of a
/// bare command name.
///
/// The standard macOS-GUI-app remedy: ask the user's login shell to
/// print its `PATH` once at startup and adopt it. Children spawned
/// later inherit the rich PATH naturally — no per-spawn shell wrapper
/// or per-spawn lookup needed.
///
/// Costs ~30-100ms once at startup (one shell exec; the upper bound
/// covers users with heavy `.zshrc` plugins like oh-my-zsh / p10k).
/// Best-effort: silently leaves the existing PATH alone if the shell
/// exec fails or returns nothing useful, so we never block startup
/// on a misconfigured shell.
///
/// Why **`-ilc`** instead of just `-lc`: zsh sources `~/.zshrc` only
/// for *interactive* shells, not login-only ones. Many users (Rosson
/// included) set `export PATH="$HOME/.local/bin:$PATH"` in `~/.zshrc`,
/// not in `~/.zprofile`. Without `-i`, `~/.local/bin` and tool-manager
/// dirs (`~/.bun/bin`, `~/.cargo/bin`, npm globals, etc.) are missing
/// from the captured PATH — exactly the bug 0.35.1 shipped. We use
/// `-ilc` to source the full chain (zshenv → zprofile → zshrc → zlogin
/// for zsh). Stderr is dropped so noisy `.zshrc` plugins don't pollute
/// our capture, and we take only the last line of stdout in case the
/// rc files emit anything before our `printf`.
///
/// Call this at the top of `main()` / `run()` in every binary that
/// might `posix_spawn` user-installed tools (the daemon spawns them
/// directly via alacritty's `tty::new`; the Tauri process spawns them
/// via the legacy `TerminalManager` and assorted `Command::new` call
/// sites).
#[cfg(unix)]
pub fn enrich_path_from_login_shell() {
    use std::process::Stdio;
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let output = std::process::Command::new(&shell)
        .args(["-ilc", "printf %s \"$PATH\""])
        .stderr(Stdio::null())
        .output();
    if let Ok(out) = output {
        if out.status.success() {
            // Take the last non-empty line of stdout. If the user's
            // rc files print anything before us, our `printf %s` (no
            // newline) lands at the end — split-rsplit-find-non-empty
            // recovers our payload regardless of preamble noise.
            let stdout = String::from_utf8_lossy(&out.stdout);
            let captured = stdout
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("")
                .trim()
                .to_string();
            if !captured.is_empty()
                && captured.contains('/')
                && captured != std::env::var("PATH").unwrap_or_default()
            {
                std::env::set_var("PATH", captured);
            }
        }
    }
}

/// Raise `RLIMIT_NOFILE` (soft limit) up to the kernel's hard limit so
/// the process can hold enough open file descriptors for many PTY
/// sessions + file watchers + WebSocket sockets concurrently.
///
/// **Why this matters:** macOS apps launched via `open` or LaunchAgent
/// inherit launchd's default soft limit, which is typically **256 or
/// 1024** — much lower than the user's shell `ulimit -n`. Each v2
/// terminal pane takes ~2 fds for the PTY (master + slave), plus WS
/// sockets and file watchers, so 10–20 panes is enough to saturate at
/// the default and start failing spawns with:
///
/// > `spawn failed: ... Too many open files (os error 24)`
///
/// The kernel hard limit on macOS is `OPEN_MAX` (10240) or higher.
/// Raising the soft limit to match the hard limit is a no-op when
/// they're already equal, and is safe + standard for daemon-style apps
/// (Postgres, Redis, browsers all do this). The change persists for
/// the lifetime of the process; children inherit the new soft limit.
#[cfg(unix)]
pub fn raise_nofile_limit() {
    use std::mem::MaybeUninit;
    unsafe {
        let mut rlim: MaybeUninit<libc::rlimit> = MaybeUninit::uninit();
        if libc::getrlimit(libc::RLIMIT_NOFILE, rlim.as_mut_ptr()) != 0 {
            log_debug!("[rlimit] getrlimit RLIMIT_NOFILE failed; leaving default");
            return;
        }
        let rlim = rlim.assume_init();
        let prior_soft = rlim.rlim_cur;
        let hard = rlim.rlim_max;
        if prior_soft >= hard {
            // Already at hard limit (or higher — shouldn't happen).
            log_debug!("[rlimit] RLIMIT_NOFILE soft={prior_soft} already at hard={hard}");
            return;
        }
        let new_rlim = libc::rlimit { rlim_cur: hard, rlim_max: hard };
        if libc::setrlimit(libc::RLIMIT_NOFILE, &new_rlim) != 0 {
            log_debug!(
                "[rlimit] setrlimit RLIMIT_NOFILE failed (errno={}); leaving soft={prior_soft}",
                std::io::Error::last_os_error()
            );
            return;
        }
        log_debug!(
            "[rlimit] RLIMIT_NOFILE soft raised: {prior_soft} -> {hard}",
        );
    }
}

#[cfg(not(unix))]
pub fn raise_nofile_limit() {
    // Windows uses a different model (no equivalent to RLIMIT_NOFILE).
    // Per-process handle limits are usually high enough that this isn't
    // a concern in practice.
}

#[doc(hidden)]
pub fn __scaffolding_marker() {}

#[cfg(all(test, unix))]
mod path_enrichment_tests {
    //! Regression tests for the launchd-PATH gap that broke v0.35.0
    //! production: launchd hands `/Applications/K2SO.app` and the daemon
    //! a sparse PATH (`/usr/bin:/bin:/usr/sbin:/sbin`). Spawns of user-
    //! installed tools like `claude` failed with ENOENT until the
    //! `enrich_path_from_login_shell` helper landed in 0.35.1.
    //!
    //! These tests run in `cargo test`'s shell-inherited (rich) PATH
    //! by default, so we can't rely on the ambient env to reproduce the
    //! bug — we deliberately pave PATH down to the launchd default
    //! before calling the helper, then assert it widens.
    use super::*;
    use std::sync::Mutex;

    /// `std::env::set_var` is not thread-safe and these tests mutate it
    /// directly. Serialize them so they don't race each other when the
    /// test runner uses multiple threads.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Lock + recover from poison so a panic in one test doesn't
    /// fail the next one with an opaque PoisonError.
    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn with_sparse_path<F: FnOnce()>(f: F) {
        let _g = lock_env();
        let original = std::env::var("PATH").ok();
        // Standard launchd default — what `/Applications/K2SO.app` and
        // the daemon actually see in production after a fresh install.
        std::env::set_var("PATH", "/usr/bin:/bin:/usr/sbin:/sbin");
        f();
        match original {
            Some(p) => std::env::set_var("PATH", p),
            None => std::env::remove_var("PATH"),
        }
    }

    #[test]
    fn enrich_path_widens_sparse_launchd_default() {
        with_sparse_path(|| {
            let before = std::env::var("PATH").unwrap();
            assert_eq!(before, "/usr/bin:/bin:/usr/sbin:/sbin");
            enrich_path_from_login_shell();
            let after = std::env::var("PATH").unwrap();
            // The login shell's PATH is set in the user's rc files. We
            // assert the helper produced *something different and longer*
            // — the exact contents are user-environment-specific.
            assert_ne!(
                before, after,
                "expected enrich_path_from_login_shell to widen PATH; got the same launchd-default value"
            );
            assert!(
                after.len() > before.len(),
                "expected widened PATH to be longer than launchd default; got before={before} after={after}"
            );
        });
    }

    #[test]
    fn enrich_path_safe_to_call_multiple_times() {
        // Production calls the helper exactly once at startup, so
        // strict idempotency isn't required — but it must be safe to
        // invoke repeatedly without crashing or producing an empty
        // PATH. (Some users' rc files reorder dirs across invocations,
        // so equality across calls is too strict an assertion.)
        let _g = lock_env();
        let before = std::env::var("PATH").unwrap_or_default();
        enrich_path_from_login_shell();
        let after_one = std::env::var("PATH").unwrap_or_default();
        enrich_path_from_login_shell();
        let after_two = std::env::var("PATH").unwrap_or_default();
        assert!(
            !after_one.is_empty(),
            "PATH must not become empty after first enrich (before={before})"
        );
        assert!(
            !after_two.is_empty(),
            "PATH must not become empty after second enrich"
        );
        // Bound any drift: a second call shouldn't massively grow the
        // string (e.g. by re-prepending user dirs over and over). 2x
        // headroom catches real runaway growth without flaking on
        // legitimate reordering.
        assert!(
            after_two.len() < after_one.len() * 2,
            "second enrich call doubled PATH length — likely a runaway prepend"
        );
    }
}
