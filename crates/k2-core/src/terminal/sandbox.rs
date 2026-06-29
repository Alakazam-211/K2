//! Sandbox seam for daemon PTY spawn (Sandbox P1 — SandboxBackend seam).
//!
//! ## What this is
//!
//! The single backend-specific step of [`crate::terminal::daemon_pty::DaemonPtySession::spawn`]
//! — "build the `tty::Options`, open the PTY, capture the child PID" — is
//! extracted behind a trait so a future microVM backend (P2, libkrun on Linux)
//! can replace ONLY that step without touching the Term / EventLoop / Arc /
//! label / kill machinery that wraps it. Everything else in `spawn` is
//! backend-agnostic and stays put.
//!
//! ## Default-OFF / prod byte-identical
//!
//! [`SandboxSpec::default()`] is [`SandboxSpec::Passthrough`], and
//! [`Passthrough::spawn`] is a VERBATIM extraction of the original spawn body
//! (same `tty::Options` field set + order, same `tty::new(&opts, window_size,
//! 0)`, same `#[cfg(unix)]` PID capture). With the default spec every existing
//! path produces byte-identical behavior — the trait adds one `Box<dyn>`
//! indirection and nothing else. The real microVM backend (P2) lives behind a
//! default-OFF `K2_SANDBOX` env and is unconstructible in P1: the only variant
//! is `Passthrough`.

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;

use alacritty_terminal::event::WindowSize;
use alacritty_terminal::tty::{self, Options as TtyOptions, Shell};

/// Everything a backend needs to open a child process attached to a PTY. The
/// fields are the env-enriched / login-shell-resolved values computed by
/// `DaemonPtySession::spawn` BEFORE this seam — the trait owns only the PTY
/// open itself, never the enrichment.
pub struct SpawnRequest {
    /// The program + args to run. `None` ⇒ the user's login shell (alacritty
    /// default), same as opening a terminal with no command override.
    pub shell: Option<Shell>,
    /// Working directory for the child. `None` ⇒ inherit.
    pub cwd: Option<PathBuf>,
    /// The fully-enriched child environment (PATH augmentation + TERM /
    /// COLORTERM / TERM_PROGRAM already applied upstream).
    pub env: HashMap<String, String>,
    /// Initial PTY window size. `Copy`.
    pub window_size: WindowSize,
    /// Drain pending child output before teardown on child exit.
    pub drain_on_exit: bool,
    /// Per-cell hook UDS path (#58), derived from `K2_HOOK_SOCK` in the env.
    /// **Plumbed in P1, consumed in P2** (the microVM backend forwards the
    /// hook channel into the guest). Unused by [`Passthrough`].
    pub cell_socket: Option<PathBuf>,
    /// The agent's own workspace root (the spawn cwd). **Plumbed in P1,
    /// consumed in P2** (the microVM backend mounts it into the guest).
    /// Unused by [`Passthrough`].
    pub workspace_root: Option<PathBuf>,
}

/// A spawned child + its captured direct-child PID. Returned from a backend's
/// [`SandboxBackend::spawn`] and unpacked by `DaemonPtySession::spawn`, which
/// then hands the `pty` to alacritty's `EventLoop::new` exactly as before.
pub struct SpawnedChild {
    pub pty: tty::Pty,
    /// Direct child PID (captured while we still own the `Pty`, before
    /// `EventLoop::new` consumes it). `None` only when capture is unavailable
    /// (non-unix).
    pub child_pid: Option<i32>,
}

/// A spawn backend: opens a PTY for a [`SpawnRequest`] and returns the
/// [`SpawnedChild`]. The trait deliberately returns a concrete `tty::Pty` (the
/// type `EventLoop::new` already consumes) — no event-loop generalization.
pub trait SandboxBackend: Send + Sync {
    fn spawn(&self, req: SpawnRequest) -> io::Result<SpawnedChild>;
    fn name(&self) -> &'static str;
}

/// Which backend a spawn should use. `Default` = [`Passthrough`] so every
/// existing caller is byte-identical to pre-seam behavior. P2 adds `Microvm`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxSpec {
    /// No isolation — open the PTY directly on the host, exactly as the daemon
    /// always has. The only P1 variant.
    Passthrough,
}

impl Default for SandboxSpec {
    fn default() -> Self {
        SandboxSpec::Passthrough
    }
}

impl SandboxSpec {
    /// Construct the backend object for this spec. One small heap alloc + dyn
    /// dispatch per spawn (negligible against an actual PTY open / fork).
    pub fn backend(&self) -> Box<dyn SandboxBackend> {
        match self {
            SandboxSpec::Passthrough => Box::new(Passthrough),
        }
    }
}

/// The host-direct backend. Its [`spawn`](SandboxBackend::spawn) body is a
/// VERBATIM extraction of the original `daemon_pty.rs` spawn lines (build
/// `tty::Options` in the same field order, `tty::new(&opts, window_size, 0)`,
/// `#[cfg(unix)]` PID capture) so the default path is unchanged.
/// `cell_socket` / `workspace_root` are unused here (consumed by the P2
/// microVM backend).
pub struct Passthrough;

impl SandboxBackend for Passthrough {
    fn spawn(&self, req: SpawnRequest) -> io::Result<SpawnedChild> {
        let pty_options = TtyOptions {
            shell: req.shell,
            working_directory: req.cwd,
            drain_on_exit: req.drain_on_exit,
            env: req.env,
            #[cfg(target_os = "windows")]
            escape_args: false,
        };

        // Window ID is used on macOS/Windows to associate the PTY with a
        // specific OS window for controlling-terminal semantics. The daemon has
        // no window, so we pass 0.
        let pty = tty::new(&pty_options, req.window_size, 0)?;

        // Capture the direct child PID NOW, while we still own the Pty —
        // `EventLoop::new` consumes it upstream. alacritty's `Pty` exposes
        // `child() -> &std::process::Child`; `.id()` is the PID as a u32. The
        // child is a setsid() session leader (its own process group), so
        // killpg on its pgid is daemon-safe.
        #[cfg(unix)]
        let child_pid: Option<i32> = Some(pty.child().id() as i32);
        #[cfg(not(unix))]
        let child_pid: Option<i32> = None;

        Ok(SpawnedChild { pty, child_pid })
    }

    fn name(&self) -> &'static str {
        "passthrough"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_spec_is_passthrough() {
        assert_eq!(SandboxSpec::default(), SandboxSpec::Passthrough);
    }

    #[test]
    fn passthrough_backend_name() {
        assert_eq!(SandboxSpec::Passthrough.backend().name(), "passthrough");
    }

    /// The env handed to a backend genuinely reaches the spawned child. We run
    /// a tiny `sh -c 'printf "$SENTINEL" > file'` through `Passthrough::spawn`
    /// with a sentinel env var and assert the child wrote the value — proving
    /// the env round-trips `SpawnRequest.env` → `tty::Options.env` → child.
    #[cfg(unix)]
    #[test]
    fn passthrough_spawn_request_round_trips_env() {
        let dir = std::env::temp_dir().join(format!(
            "k2-passthrough-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let out = dir.join("sentinel.out");

        let mut env = HashMap::new();
        env.insert("K2_SANDBOX_SENTINEL".to_string(), "round-trip-OK".to_string());

        let shell = Shell::new(
            "sh".to_string(),
            vec![
                "-c".to_string(),
                format!("printf '%s' \"$K2_SANDBOX_SENTINEL\" > {}", out.display()),
            ],
        );
        let req = SpawnRequest {
            shell: Some(shell),
            cwd: Some(dir.clone()),
            env,
            window_size: WindowSize {
                num_cols: 80,
                num_lines: 24,
                cell_width: 10,
                cell_height: 20,
            },
            drain_on_exit: true,
            cell_socket: None,
            workspace_root: None,
        };

        let child = SandboxSpec::Passthrough
            .backend()
            .spawn(req)
            .expect("passthrough spawn must succeed");
        assert!(
            child.child_pid.is_some(),
            "unix spawn must capture the child pid"
        );

        // Hold the Pty (in `child`) so the child isn't SIGHUP'd by Pty::Drop
        // before it writes; poll for the file the child produces.
        let mut got = String::new();
        for _ in 0..250 {
            if let Ok(s) = std::fs::read_to_string(&out) {
                if !s.is_empty() {
                    got = s;
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        drop(child);
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(
            got, "round-trip-OK",
            "the child must observe the sentinel env passed through Passthrough::spawn"
        );
    }
}
