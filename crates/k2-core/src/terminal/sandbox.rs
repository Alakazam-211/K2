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

/// Program + arguments to exec as the session's child.
///
/// A readable stand-in for alacritty's `tty::Shell` (whose `program`/`args`
/// fields are `pub(crate)` and therefore unreadable from this crate). P2a's
/// [`build_worker_invocation`] must read the program/args back out to serialize
/// the microVM worker argv, so the seam carries this instead of the opaque
/// `Shell`. [`Passthrough::spawn`] maps it into `tty::Shell::new` at the PTY
/// open, preserving byte-identical spawn behavior.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ShellSpec {
    /// Path / bare name of the program to run.
    pub program: String,
    /// Arguments passed to `program`.
    pub args: Vec<String>,
}

/// Everything a backend needs to open a child process attached to a PTY. The
/// fields are the env-enriched / login-shell-resolved values computed by
/// `DaemonPtySession::spawn` BEFORE this seam — the trait owns only the PTY
/// open itself, never the enrichment.
pub struct SpawnRequest {
    /// The program + args to run. `None` ⇒ the user's login shell (alacritty
    /// default), same as opening a terminal with no command override.
    pub shell: Option<ShellSpec>,
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
    /// Host paths exposed read-only inside the guest (toolchains, agent CLIs).
    /// **Plumbed in P2a, consumed in P2b** (the microVM backend bind-mounts
    /// each as a ro mount). Empty for [`Passthrough`].
    pub tool_roots: Vec<PathBuf>,
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
/// existing caller is byte-identical to pre-seam behavior.
///
/// `Microvm` (P2a) is present on ALL platforms so every `match` on a spec stays
/// exhaustive cross-platform — but it only resolves to a real (placeholder)
/// microVM backend on a Linux build with the `sandbox-microvm` feature. On
/// every other build it resolves [fail-closed](FailClosed), NEVER to
/// `Passthrough` (a silent isolation downgrade).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxSpec {
    /// No isolation — open the PTY directly on the host, exactly as the daemon
    /// always has.
    Passthrough,
    /// libkrun microVM jail. The real backend (the `k2-vmm-worker` invoker +
    /// host jail) is P2b, Linux-only, and not built in this slice; selecting
    /// this anywhere it can't be delivered resolves to [`FailClosed`].
    Microvm,
}

impl Default for SandboxSpec {
    fn default() -> Self {
        SandboxSpec::Passthrough
    }
}

impl SandboxSpec {
    /// Construct the backend object for this spec. One small heap alloc + dyn
    /// dispatch per spawn (negligible against an actual PTY open / fork).
    ///
    /// **Fail-closed (security-critical).** `Microvm` resolves to a real
    /// (placeholder) microVM backend ONLY on a Linux build with the
    /// `sandbox-microvm` feature. Everywhere else it resolves to [`FailClosed`]
    /// — it must NEVER fall through to [`Passthrough`], which would hand a
    /// caller that asked for a jail a raw host shell with no isolation.
    pub fn backend(&self) -> Box<dyn SandboxBackend> {
        match self {
            SandboxSpec::Passthrough => Box::new(Passthrough),
            SandboxSpec::Microvm => {
                #[cfg(all(target_os = "linux", feature = "sandbox-microvm"))]
                {
                    // P2b replaces this placeholder with the real libkrun worker
                    // invoker. Until then it Errs — but it is NEVER Passthrough,
                    // so no isolation downgrade can leak through.
                    Box::new(Microvm)
                }
                #[cfg(not(all(target_os = "linux", feature = "sandbox-microvm")))]
                {
                    Box::new(FailClosed)
                }
            }
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
        // Map the readable ShellSpec back into alacritty's `tty::Shell` at the
        // PTY open — same program + args as before, so spawn is byte-identical.
        let shell = req.shell.map(|s| Shell::new(s.program, s.args));
        let pty_options = TtyOptions {
            shell,
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

/// libkrun microVM backend — placeholder (P2a).
///
/// The real implementation (the `k2-vmm-worker` invoker + host jail) is P2b,
/// Linux-only, and intentionally NOT built in this slice. This struct exists so
/// the `Microvm` spec has a non-Passthrough thing to resolve to on a future
/// Linux build; its `spawn()` errors until P2b lands. P2b's real `spawn()` will
/// build its worker argv via [`build_worker_invocation`].
pub struct Microvm;

impl SandboxBackend for Microvm {
    fn spawn(&self, _req: SpawnRequest) -> io::Result<SpawnedChild> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "microvm backend not yet implemented (P2b)",
        ))
    }

    fn name(&self) -> &'static str {
        "microvm"
    }
}

/// Fail-closed backend.
///
/// Resolved when [`SandboxSpec::Microvm`] is selected on a build/platform that
/// cannot deliver a real microVM. Its `spawn()` ALWAYS errors — so a caller
/// that asked for isolation never silently gets a raw host shell.
pub struct FailClosed;

impl SandboxBackend for FailClosed {
    fn spawn(&self, _req: SpawnRequest) -> io::Result<SpawnedChild> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "microvm backend unavailable on this build/platform",
        ))
    }

    fn name(&self) -> &'static str {
        "failclosed"
    }
}

/// Resource caps for the microVM. Placeholders — the real values are an owner
/// decision (P2b). Defaults: 1 vCPU, 1 GiB RAM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VmCaps {
    pub vcpus: u32,
    pub ram_mib: u32,
}

impl Default for VmCaps {
    fn default() -> Self {
        Self {
            vcpus: 1,
            ram_mib: 1024,
        }
    }
}

/// Worker program name placed at `argv[0]` of a [`WorkerInvocation`].
pub const WORKER_PROGRAM: &str = "k2-vmm-worker";

/// A serialized invocation of the future (P2b, Linux) `k2-vmm-worker`.
///
/// Pure data — no exec. Produced by [`build_worker_invocation`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerInvocation {
    /// argv for the worker. `argv[0]` is the worker program name; the rest are
    /// `--flags` (+ the real shell program/args after a `--` separator).
    pub argv: Vec<String>,
    /// Environment forwarded INTO the guest (mirror of `req.env`, including the
    /// guest-side `K2_HOOK_SOCK`). Sorted for deterministic, testable output.
    pub guest_env: Vec<(String, String)>,
}

/// Serialize a [`SpawnRequest`] + [`VmCaps`] into the future worker's argv/env.
///
/// Pure, platform-independent host-side construction (NO exec, NO libkrun) so it
/// compiles + unit-tests on macOS. This is exactly what P2b's real
/// `Microvm::spawn` will hand to the worker process: the workspace_root rw
/// mount, the cell_socket host path, the tool-roots ro mounts, the real shell +
/// args, the guest env (incl. `K2_HOOK_SOCK`), the window size, and the
/// vcpu/ram caps.
pub fn build_worker_invocation(req: &SpawnRequest, caps: &VmCaps) -> WorkerInvocation {
    let mut argv = vec![WORKER_PROGRAM.to_string()];

    // VM resource caps.
    argv.push("--vcpus".to_string());
    argv.push(caps.vcpus.to_string());
    argv.push("--ram-mib".to_string());
    argv.push(caps.ram_mib.to_string());

    // Initial PTY geometry.
    argv.push("--cols".to_string());
    argv.push(req.window_size.num_cols.to_string());
    argv.push("--rows".to_string());
    argv.push(req.window_size.num_lines.to_string());

    if req.drain_on_exit {
        argv.push("--drain-on-exit".to_string());
    }

    // Workspace root → read-WRITE mount inside the guest.
    if let Some(ws) = &req.workspace_root {
        argv.push("--workspace-root".to_string());
        argv.push(ws.to_string_lossy().into_owned());
    }

    // Per-cell hook socket host path → bind-mounted into the guest.
    if let Some(sock) = &req.cell_socket {
        argv.push("--cell-socket".to_string());
        argv.push(sock.to_string_lossy().into_owned());
    }

    // Tool roots → read-only mounts inside the guest (stable order).
    for root in &req.tool_roots {
        argv.push("--tool-root".to_string());
        argv.push(root.to_string_lossy().into_owned());
    }

    // Working directory inside the guest.
    if let Some(cwd) = &req.cwd {
        argv.push("--cwd".to_string());
        argv.push(cwd.to_string_lossy().into_owned());
    }

    // The real shell program + args, after a `--` separator so the worker's own
    // flag parser stops here and the rest is the child command verbatim.
    if let Some(shell) = &req.shell {
        argv.push("--".to_string());
        argv.push(shell.program.clone());
        for a in &shell.args {
            argv.push(a.clone());
        }
    }

    // Guest env: forward everything the caller staged (including the guest-side
    // K2_HOOK_SOCK). Sorted so the output is deterministic for tests.
    let mut guest_env: Vec<(String, String)> = req
        .env
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    guest_env.sort();

    WorkerInvocation { argv, guest_env }
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

        let shell = ShellSpec {
            program: "sh".to_string(),
            args: vec![
                "-c".to_string(),
                format!("printf '%s' \"$K2_SANDBOX_SENTINEL\" > {}", out.display()),
            ],
        };
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
            tool_roots: vec![],
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

    // ── P2a — Microvm / FailClosed / build_worker_invocation ──────────────

    fn win(cols: u16, rows: u16) -> WindowSize {
        WindowSize {
            num_cols: cols,
            num_lines: rows,
            cell_width: 10,
            cell_height: 20,
        }
    }

    /// A request with sentinel values in every field so the worker
    /// serialization test can prove each one is carried through.
    fn sentinel_request() -> SpawnRequest {
        let mut env = HashMap::new();
        env.insert("K2_HOOK_SOCK".to_string(), "/guest/run/hook.sock".to_string());
        env.insert("SENTINEL_FOO".to_string(), "bar".to_string());
        SpawnRequest {
            shell: Some(ShellSpec {
                program: "/sentinel/bin/shell".to_string(),
                args: vec!["--login".to_string(), "-c".to_string(), "claude".to_string()],
            }),
            cwd: Some(PathBuf::from("/sentinel/cwd")),
            env,
            window_size: win(111, 222),
            drain_on_exit: true,
            cell_socket: Some(PathBuf::from("/sentinel/host/cell.sock")),
            workspace_root: Some(PathBuf::from("/sentinel/workspace")),
            tool_roots: vec![
                PathBuf::from("/sentinel/tool-a"),
                PathBuf::from("/sentinel/tool-b"),
            ],
        }
    }

    /// Value following `flag` in `argv` (first occurrence).
    fn flag_value<'a>(argv: &'a [String], flag: &str) -> Option<&'a str> {
        argv.iter()
            .position(|a| a == flag)
            .and_then(|i| argv.get(i + 1))
            .map(|s| s.as_str())
    }

    #[test]
    fn failclosed_spawn_errors_no_silent_passthrough() {
        // FailClosed must ERROR — proving a caller that asked for isolation
        // never silently gets a raw host shell. `.err().expect(..)` rather than
        // `expect_err` since SpawnedChild can't derive Debug (Pty isn't Debug).
        let err = FailClosed
            .spawn(sentinel_request())
            .err()
            .expect("FailClosed::spawn must return Err, never a host spawn");
        assert_eq!(err.kind(), io::ErrorKind::Unsupported);
        assert!(
            err.to_string().contains("unavailable"),
            "unexpected FailClosed error message: {err}"
        );
    }

    #[test]
    fn microvm_spec_backend_spawn_errors_on_this_build() {
        // On this (macOS, feature-off) build the Microvm spec resolves to
        // FailClosed; under linux+feature it resolves to the Microvm
        // placeholder. EITHER WAY spawn() must Err — never a real host spawn.
        let backend = SandboxSpec::Microvm.backend();
        let name = backend.name();
        assert!(
            name == "failclosed" || name == "microvm",
            "Microvm spec must resolve to failclosed or microvm placeholder, got {name}"
        );
        let err = backend
            .spawn(sentinel_request())
            .err()
            .expect("Microvm spec must not perform a real host spawn on this build");
        assert_eq!(err.kind(), io::ErrorKind::Unsupported);
    }

    /// On macOS with the feature off (the build this slice targets), the
    /// Microvm spec must resolve to the fail-closed backend specifically —
    /// never Passthrough (no silent isolation downgrade).
    #[cfg(not(all(target_os = "linux", feature = "sandbox-microvm")))]
    #[test]
    fn microvm_spec_is_failclosed_on_non_linux_or_feature_off() {
        assert_eq!(SandboxSpec::Microvm.backend().name(), "failclosed");
    }

    #[test]
    fn vmcaps_defaults_are_1_vcpu_1gib() {
        let caps = VmCaps::default();
        assert_eq!(caps.vcpus, 1);
        assert_eq!(caps.ram_mib, 1024);
    }

    #[test]
    fn build_worker_invocation_carries_every_field() {
        let req = sentinel_request();
        let caps = VmCaps {
            vcpus: 3,
            ram_mib: 4096,
        };
        let inv = build_worker_invocation(&req, &caps);

        assert_eq!(inv.argv[0], WORKER_PROGRAM);

        assert_eq!(flag_value(&inv.argv, "--vcpus"), Some("3"));
        assert_eq!(flag_value(&inv.argv, "--ram-mib"), Some("4096"));
        assert_eq!(flag_value(&inv.argv, "--cols"), Some("111"));
        assert_eq!(flag_value(&inv.argv, "--rows"), Some("222"));
        assert!(inv.argv.iter().any(|a| a == "--drain-on-exit"));
        assert_eq!(
            flag_value(&inv.argv, "--workspace-root"),
            Some("/sentinel/workspace")
        );
        assert_eq!(
            flag_value(&inv.argv, "--cell-socket"),
            Some("/sentinel/host/cell.sock")
        );
        assert_eq!(flag_value(&inv.argv, "--cwd"), Some("/sentinel/cwd"));

        // Both tool roots present (ro mounts).
        let tool_roots: Vec<&String> = inv
            .argv
            .iter()
            .enumerate()
            .filter(|(i, a)| *a == "--tool-root" && inv.argv.get(i + 1).is_some())
            .map(|(i, _)| &inv.argv[i + 1])
            .collect();
        assert_eq!(tool_roots.len(), 2, "both tool roots must be emitted");
        assert!(tool_roots.iter().any(|p| *p == "/sentinel/tool-a"));
        assert!(tool_roots.iter().any(|p| *p == "/sentinel/tool-b"));

        // Real shell + args after the `--` separator, in order.
        let sep = inv
            .argv
            .iter()
            .position(|a| a == "--")
            .expect("shell must follow a -- separator");
        assert_eq!(inv.argv.get(sep + 1).map(String::as_str), Some("/sentinel/bin/shell"));
        assert_eq!(inv.argv.get(sep + 2).map(String::as_str), Some("--login"));
        assert_eq!(inv.argv.get(sep + 3).map(String::as_str), Some("-c"));
        assert_eq!(inv.argv.get(sep + 4).map(String::as_str), Some("claude"));

        // Guest env carries the guest hook sock + sentinel var, sorted.
        assert!(inv
            .guest_env
            .iter()
            .any(|(k, v)| k == "K2_HOOK_SOCK" && v == "/guest/run/hook.sock"));
        assert!(inv
            .guest_env
            .iter()
            .any(|(k, v)| k == "SENTINEL_FOO" && v == "bar"));
        let mut sorted = inv.guest_env.clone();
        sorted.sort();
        assert_eq!(inv.guest_env, sorted, "guest_env must be deterministically sorted");
    }

    #[test]
    fn build_worker_invocation_omits_absent_optional_fields() {
        let req = SpawnRequest {
            shell: None,
            cwd: None,
            env: HashMap::new(),
            window_size: win(80, 24),
            drain_on_exit: false,
            cell_socket: None,
            workspace_root: None,
            tool_roots: vec![],
        };
        let inv = build_worker_invocation(&req, &VmCaps::default());
        assert!(flag_value(&inv.argv, "--workspace-root").is_none());
        assert!(flag_value(&inv.argv, "--cell-socket").is_none());
        assert!(flag_value(&inv.argv, "--cwd").is_none());
        assert!(!inv.argv.iter().any(|a| a == "--tool-root"));
        assert!(!inv.argv.iter().any(|a| a == "--"));
        assert!(!inv.argv.iter().any(|a| a == "--drain-on-exit"));
        assert_eq!(flag_value(&inv.argv, "--vcpus"), Some("1"));
        assert_eq!(flag_value(&inv.argv, "--ram-mib"), Some("1024"));
    }
}
