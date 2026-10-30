//! Process bootstrap shared by the daemon and the thin client: adopt the
//! login shell's PATH, raise the open-file limit, ignore SIGPIPE.
//!
//! Moved from `k2_core` (lib.rs / `terminal`) in the 0.46.4 build-speed
//! lane; k2-core re-exports every item at its old path.

/// Ignore SIGPIPE at process startup so writing to a dead PTY or socket
/// returns EPIPE instead of killing the whole process.
///
/// This is intentionally global: child processes spawned via Command::new()
/// inherit their own signal mask, so git/external tools are unaffected.
/// DB writes target local files (no pipe), and reqwest uses MSG_NOSIGNAL.
/// Zed uses the same approach.
#[cfg(unix)]
pub fn ignore_sigpipe() {
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
}

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
/// our capture, and the PATH is read from between the
/// [`crate::login_path::LOGIN_PATH_PROBE`] markers so anything the rc
/// files print to stdout is ignored.
///
/// Call this at the top of `main()` / `run()` in every binary that
/// might `posix_spawn` user-installed tools (the daemon spawns them
/// directly via alacritty's `tty::new`; the Tauri process spawns them
/// via the legacy `TerminalManager` and assorted `Command::new` call
/// sites).
#[cfg(unix)]
pub fn enrich_path_from_login_shell() {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    if let Some(captured) = login_shell_path(std::path::Path::new(&shell)) {
        if captured != std::env::var("PATH").unwrap_or_default() {
            std::env::set_var("PATH", captured);
        }
    }
}

/// The PATH `shell -ilc <probe>` prints between the probe's markers
/// (absolute + safe entries only), or `None` when the shell cannot run,
/// exits non-zero, or prints no marked PATH. Pure: reads and changes no
/// process env, so tests drive it with a fake shell.
#[cfg(unix)]
pub fn login_shell_path(shell: &std::path::Path) -> Option<String> {
    use std::process::Stdio;
    let out = crate::bg_proc::command(shell)
        .args(["-ilc", crate::login_path::LOGIN_PATH_PROBE])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    // The PATH between the probe's markers, absolute + safe entries only;
    // whatever the rc files print around it is ignored (0.45.0: zsh
    // printed completion-function source).
    crate::login_path::extract_login_path(&String::from_utf8_lossy(&out.stdout))
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
        // K-M13 (0.46.1): macOS reports an unlimited hard limit, and
        // setrlimit refuses a soft limit above kern.maxfilesperproc there
        // (EINVAL) — so the old "soft = hard" left launchd's 256 in place.
        let target = nofile_raise_target(hard);
        if prior_soft >= target {
            // Already at hard limit (or higher — shouldn't happen).
            log_debug!("[rlimit] RLIMIT_NOFILE soft={prior_soft} already at hard={hard}");
            return;
        }
        let new_rlim = libc::rlimit { rlim_cur: target, rlim_max: hard };
        if libc::setrlimit(libc::RLIMIT_NOFILE, &new_rlim) != 0 {
            log_debug!(
                "[rlimit] setrlimit RLIMIT_NOFILE failed (errno={}); leaving soft={prior_soft}",
                std::io::Error::last_os_error()
            );
            return;
        }
        log_debug!(
            "[rlimit] RLIMIT_NOFILE soft raised: {prior_soft} -> {target}",
        );
    }
}

/// The soft limit [`raise_nofile_limit`] aims for: the hard limit, except
/// where the kernel caps a process below an "unlimited" hard limit (macOS:
/// `kern.maxfilesperproc`, else `OPEN_MAX` 10240).
#[cfg(unix)]
#[doc(hidden)] // pub for k2-core's rlimit test
pub fn nofile_raise_target(hard: libc::rlim_t) -> libc::rlim_t {
    #[cfg(target_os = "macos")]
    {
        let mut per_proc: libc::c_int = 0;
        let mut len = std::mem::size_of::<libc::c_int>();
        let name = b"kern.maxfilesperproc\0";
        // SAFETY: valid NUL-terminated name, out-pointer and length.
        let rc = unsafe {
            libc::sysctlbyname(
                name.as_ptr() as *const libc::c_char,
                &mut per_proc as *mut libc::c_int as *mut libc::c_void,
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        let cap: libc::rlim_t = if rc == 0 && per_proc > 0 { per_proc as libc::rlim_t } else { 10_240 };
        hard.min(cap)
    }
    #[cfg(not(target_os = "macos"))]
    {
        hard
    }
}

/// This process's current soft `RLIMIT_NOFILE` (`None` where there is no
/// such limit, e.g. Windows, or `getrlimit` failed). An unlimited soft
/// limit reads as `u64::MAX`.
pub fn nofile_soft_limit() -> Option<u64> {
    #[cfg(unix)]
    {
        let mut rlim = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
        // SAFETY: valid out-pointer.
        if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut rlim) } != 0 {
            return None;
        }
        if rlim.rlim_cur == libc::RLIM_INFINITY {
            return Some(u64::MAX);
        }
        Some(rlim.rlim_cur as u64)
    }
    #[cfg(not(unix))]
    {
        None
    }
}

#[cfg(not(unix))]
pub fn raise_nofile_limit() {
    // Windows uses a different model (no equivalent to RLIMIT_NOFILE).
    // Per-process handle limits are usually high enough that this isn't
    // a concern in practice.
}
