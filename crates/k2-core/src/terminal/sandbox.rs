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
    /// Per-cell unique id for the microVM worker (M3). Emitted as
    /// `--session-id` by [`build_worker_invocation`] and used by the worker for
    /// BOTH the jail NEWROOT path and the cgroup leaf so concurrent cells never
    /// collide (inside `CLONE_NEWPID` the worker's `getpid()` is always 1).
    /// `None` ⇒ [`spawn_microvm`] generates a unique one. Unused by
    /// [`Passthrough`].
    pub session_id: Option<String>,
    /// P4-H6: the daemon-allocated per-session uid the worker must drop to
    /// (instead of the shared `k2cell`). Emitted as `--cell-uid <n>` by
    /// [`build_worker_invocation`]; the worker uses it as BOTH the drop uid and
    /// gid (per-session fs isolation) while keeping the kvm supplementary group.
    /// `None` (every non-microVM spawn, and any microVM spawn the daemon didn't
    /// allocate for) ⇒ no `--cell-uid` is emitted and the worker falls back to
    /// resolving `k2cell`. Unused by [`Passthrough`].
    pub cell_uid: Option<u32>,
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

/// libkrun microVM backend (P2b).
///
/// On a Linux build with `sandbox-microvm`, `spawn()` is the bridge from this
/// seam to the privilege-dropped `k2-vmm-worker` jail: it serializes the
/// [`SpawnRequest`] into the worker's argv via [`build_worker_invocation`],
/// hands the guest env to the worker on **fd 3** (so it never lands in the
/// worker's argv/environ — see [`spawn_microvm`]), and opens the PTY on the
/// worker process. On every other build the struct still exists (so the spec's
/// `Microvm` arm has a non-`Passthrough` thing to resolve to) but `spawn()`
/// fails closed — it NEVER runs the workload on the host.
pub struct Microvm;

impl SandboxBackend for Microvm {
    fn spawn(&self, req: SpawnRequest) -> io::Result<SpawnedChild> {
        #[cfg(all(target_os = "linux", feature = "sandbox-microvm"))]
        {
            spawn_microvm(req)
        }
        #[cfg(not(all(target_os = "linux", feature = "sandbox-microvm")))]
        {
            // Fail-closed placeholder: this arm is only ever reached on a build
            // that selected `Microvm` but cannot deliver it. Drop `req` and Err
            // — never a host spawn.
            let _ = req;
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "microvm backend not yet implemented (P2b)",
            ))
        }
    }

    fn name(&self) -> &'static str {
        "microvm"
    }
}

/// Open a PTY on the `k2-vmm-worker` jail for `req` (Linux + `sandbox-microvm`).
///
/// This is the "Wiring" section of the P2b spec — the bridge from the seam to
/// the worker. It does NOT itself build the jail or touch libkrun (that is all
/// inside the worker process, post privilege-drop); it only:
///
/// 1. serializes `req` → worker argv via [`build_worker_invocation`];
/// 2. resolves the `k2-vmm-worker` binary path (alongside this daemon binary);
/// 3. writes the guest env (NUL-delimited `KEY=VAL`) to a pipe whose read end
///    the worker inherits as **fd 3** — keeping it out of the worker's
///    argv/environ so a same-uid `/proc/<pid>/{cmdline,environ}` read can't
///    leak it;
/// 4. opens the PTY on the worker via alacritty's `tty::new`, exactly like
///    [`Passthrough`], and captures the worker's direct child pid.
///
/// Every failure path Errs (fail-closed) — there is no host-exec fallback.
#[cfg(all(target_os = "linux", feature = "sandbox-microvm"))]
fn spawn_microvm(req: SpawnRequest) -> io::Result<SpawnedChild> {
    use std::os::fd::FromRawFd;
    use std::os::unix::io::RawFd;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;

    // C2: serialize the clear-CLOEXEC(read_fd)→fork window. The env-pipe read
    // end is CLOEXEC except for the brief window in which we clear it so THIS
    // worker can inherit it across exec; holding this lock across that window →
    // tty::new (fork) keeps any concurrent spawn / unrelated fork from inheriting
    // it. (Each spawn now uses its OWN natural fd — no shared fixed-fd contract —
    // so there is no cross-wire even without the lock; the lock just bounds the
    // transient non-CLOEXEC leak window.)
    static SPAWN_FD3_LOCK: Mutex<()> = Mutex::new(());
    // Per-daemon-process monotonic suffix for generated session ids.
    static SPAWN_SEQ: AtomicU64 = AtomicU64::new(0);

    // M3: ensure a unique session id so the worker's NEWROOT + cgroup leaf can't
    // collide across concurrent cells (inside CLONE_NEWPID the worker is always
    // pid 1, so it cannot self-generate a unique one).
    let mut req = req;
    if req.session_id.is_none() {
        let seq = SPAWN_SEQ.fetch_add(1, Ordering::Relaxed);
        req.session_id = Some(format!("{}-{}", std::process::id(), seq));
    }

    // 1. Serialize the request into the worker's argv + guest env.
    let caps = VmCaps::default();
    let invocation = build_worker_invocation(&req, &caps);
    // argv[0] is the worker program NAME (for the worker's own logging); the
    // real exec path is resolved below. The flags + `-- <program> <args>` are
    // argv[1..].
    let worker_args: Vec<String> = invocation.argv[1..].to_vec();

    // 2. Resolve the worker binary: it ships next to the daemon binary.
    let worker_path = resolve_worker_path()?;

    // 3. Guest env → a pipe whose read end the worker inherits. Build the
    //    NUL-delimited blob. A short blob fits the pipe buffer (64 KiB); for
    //    safety against a huge env we write from a detached thread so we never
    //    deadlock on a full pipe before the worker drains it.
    let mut blob: Vec<u8> = Vec::new();
    for (k, v) in &invocation.guest_env {
        blob.extend_from_slice(k.as_bytes());
        blob.push(b'=');
        blob.extend_from_slice(v.as_bytes());
        blob.push(0);
    }

    // pipe2(O_CLOEXEC): BOTH ends CLOEXEC by default so neither can leak into an
    // unrelated fork. The write end NEVER reaches the worker; the read end's
    // CLOEXEC is cleared only inside the locked fork window below so it survives
    // exec into THIS worker (and no other).
    //
    // CRITICAL (bug #1): the previous design `dup2(read_fd, 3)` to hand the env
    // on a FIXED fd 3 clobbered whatever the daemon already had at fd 3 — which
    // is tokio's I/O-driver epoll instance (`anon_inode:[eventpoll]`). Replacing
    // then closing it made the next `epoll_wait` return EBADF → tokio aborts the
    // WHOLE daemon on cell exit. We now keep the pipe on its NATURAL fd (never
    // dup2 over a fixed number) and tell the worker which fd via the
    // `K2_GUEST_ENV_FD` env var — nothing in the daemon's fd table is touched.
    let mut fds = [0 as libc::c_int; 2];
    // SAFETY: fds is a valid 2-int buffer.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let (read_fd, write_fd): (RawFd, RawFd) = (fds[0], fds[1]);

    // Writer thread: push the blob then close, so the worker's read sees EOF.
    // Detached — it finishes on its own and never blocks spawn.
    // SAFETY: we own write_fd; the thread takes sole ownership.
    let mut writer = unsafe { std::fs::File::from_raw_fd(write_fd) };
    std::thread::spawn(move || {
        use std::io::Write;
        let _ = writer.write_all(&blob);
        let _ = writer.flush();
        drop(writer); // closes write_fd → reader sees EOF
    });

    // Host env for the WORKER process (NOT the guest):
    //  - LD_LIBRARY_PATH so the dynamic linker resolves libkrunfw at libkrun's
    //    runtime dlopen (the launchd daemon won't have it, and alacritty
    //    inherits-then-augments — it does NOT clear — so we add it explicitly);
    //  - K2_GUEST_ENV_FD = the inherited read-end fd number the worker reads the
    //    guest env from (kept out of the guest env, which rides that fd's bytes).
    let mut worker_env = HashMap::new();
    worker_env.insert("LD_LIBRARY_PATH".to_string(), "/usr/local/lib64".to_string());
    worker_env.insert("K2_GUEST_ENV_FD".to_string(), read_fd.to_string());

    // 4. Fork the worker under a global lock that serializes the
    //    clear-CLOEXEC(read_fd) → fork window, so a concurrent spawn (or any
    //    other fork in the daemon during this window) can't inherit this env
    //    pipe. The parent closes read_fd BEFORE releasing the lock.
    let pty = {
        let _guard = SPAWN_FD3_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        // Clear CLOEXEC on the read end so it survives the worker's exec (only
        // this worker inherits it; the lock bounds the window). SAFETY: ours.
        unsafe { libc::fcntl(read_fd, libc::F_SETFD, 0) };

        // The child the daemon supervises IS the worker (which then clones the
        // jailed VMM internally). It inherits read_fd; K2_GUEST_ENV_FD tells it
        // which number to read.
        let shell = Shell::new(worker_path, worker_args);
        let pty_options = TtyOptions {
            shell: Some(shell),
            working_directory: None, // the worker sets the guest cwd, not the host
            drain_on_exit: req.drain_on_exit,
            env: worker_env,
            #[cfg(target_os = "windows")]
            escape_args: false,
        };
        let result = tty::new(&pty_options, req.window_size, 0);
        // Parent no longer needs read_fd (the worker has its own inherited copy)
        // — close it BEFORE releasing the lock, on BOTH ok and err paths.
        // SAFETY: read_fd is ours.
        unsafe { libc::close(read_fd) };
        result
    }?;

    let child_pid: Option<i32> = Some(pty.child().id() as i32);
    Ok(SpawnedChild { pty, child_pid })
}

/// Resolve the `k2-vmm-worker` binary path: it is installed next to the running
/// daemon binary (same `target/` dir in dev, same install dir in prod). Errs if
/// the daemon's own path can't be determined or the worker is absent — never a
/// silent fallback.
#[cfg(all(target_os = "linux", feature = "sandbox-microvm"))]
fn resolve_worker_path() -> io::Result<String> {
    let exe = std::env::current_exe()?;
    let dir = exe.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, "daemon exe has no parent dir")
    })?;
    let worker = dir.join(WORKER_PROGRAM);
    if !worker.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("{WORKER_PROGRAM} not found next to daemon at {}", worker.display()),
        ));
    }
    Ok(worker.to_string_lossy().into_owned())
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

/// Guest-side path of the per-cell hook socket (Sandbox B2). The HOST cell
/// socket lives under `/run/k2/cells/<sid>.sock` and is unreachable from inside
/// the guest; the worker bridges it in as a vsock→UDS forwarder listening at
/// THIS in-guest path, so the guest's `K2_HOOK_SOCK` must point here (never at
/// the host path). The in-guest `k2`/`claude` connect here; the forwarder
/// relays each connection over `HOOK_PORT` (vsock 1024) to the host socket.
pub const GUEST_HOOK_SOCK: &str = "/run/k2-hook.sock";

/// Rewrite the guest-bound `K2_HOOK_SOCK` to the in-guest forwarder path
/// ([`GUEST_HOOK_SOCK`]), leaving every other variable — crucially
/// `K2_HOOK_TOKEN` (the per-cell scoped bearer) — untouched.
///
/// **Why (Sandbox B2):** `DaemonPtySession::spawn` stages `K2_HOOK_SOCK` as the
/// HOST cell-socket path (so the bare-PTY hook path works). For a microVM cell
/// that host path does not exist inside the guest, so the guest copy must be
/// rewritten to the forwarder path before it is handed to the worker via the
/// `K2_GUEST_ENV_FD` pipe. Pure + total (a no-op when the key is absent) so it
/// is unit-testable on macOS without the libkrun path, and it touches ONLY the
/// guest env — the host `worker_env` is assembled separately and is unaffected.
fn rewrite_guest_hook_sock(guest_env: &mut [(String, String)]) {
    for (k, v) in guest_env.iter_mut() {
        if k == "K2_HOOK_SOCK" {
            *v = GUEST_HOOK_SOCK.to_string();
        }
    }
}

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

    // Per-cell unique id → worker NEWROOT path + cgroup leaf (M3). Emitted when
    // present; the worker falls back to its own (supervisor) pid otherwise.
    if let Some(sid) = &req.session_id {
        argv.push("--session-id".to_string());
        argv.push(sid.clone());
    }

    // P4-H6: the daemon-allocated per-session drop uid. Emitted when present so
    // the worker drops to THIS uid (its own host identity) instead of the shared
    // `k2cell`. Absent ⇒ no flag → the worker falls back to `k2cell` (legacy /
    // hand-test path). The daemon's microVM spawn door always passes it.
    if let Some(uid) = req.cell_uid {
        argv.push("--cell-uid".to_string());
        argv.push(uid.to_string());
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

    // Guest env: forward everything the caller staged, but REWRITE
    // `K2_HOOK_SOCK` to the in-guest forwarder path (Sandbox B2) — the staged
    // value is the HOST cell-socket path, unreachable from inside the guest.
    // `K2_HOOK_TOKEN` (the per-cell scoped bearer) is left untouched. Sorted so
    // the output is deterministic for tests.
    let mut guest_env: Vec<(String, String)> = req
        .env
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    rewrite_guest_hook_sock(&mut guest_env);
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
            session_id: None,
            cell_uid: None,
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
            session_id: Some("sentinel-sid".to_string()),
            cell_uid: Some(60042),
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
        assert_eq!(flag_value(&inv.argv, "--session-id"), Some("sentinel-sid"));
        // P4-H6: the per-session drop uid is emitted as --cell-uid.
        assert_eq!(flag_value(&inv.argv, "--cell-uid"), Some("60042"));
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

        // Guest env carries the REWRITTEN guest hook sock (Sandbox B2 — the
        // staged host path is replaced with the in-guest forwarder path) +
        // the sentinel var, sorted.
        assert!(inv
            .guest_env
            .iter()
            .any(|(k, v)| k == "K2_HOOK_SOCK" && v == GUEST_HOOK_SOCK));
        assert!(inv
            .guest_env
            .iter()
            .any(|(k, v)| k == "SENTINEL_FOO" && v == "bar"));
        let mut sorted = inv.guest_env.clone();
        sorted.sort();
        assert_eq!(inv.guest_env, sorted, "guest_env must be deterministically sorted");
    }

    /// B3a — a staged `ANTHROPIC_API_KEY` (the per-workspace key the daemon's
    /// spawn door puts into `req.env`) is mirrored verbatim into `guest_env`,
    /// reaching the in-cell Claude Code. `build_worker_invocation` only rewrites
    /// `K2_HOOK_SOCK`, so the key passes through untouched — no change needed
    /// here. This pins that contract.
    #[test]
    fn build_worker_invocation_carries_anthropic_api_key() {
        let mut req = sentinel_request();
        req.env.insert(
            "ANTHROPIC_API_KEY".to_string(),
            "sk-ant-workspace-key".to_string(),
        );
        let inv = build_worker_invocation(&req, &VmCaps::default());
        assert!(
            inv.guest_env
                .iter()
                .any(|(k, v)| k == "ANTHROPIC_API_KEY" && v == "sk-ant-workspace-key"),
            "the staged per-workspace ANTHROPIC_API_KEY must reach the guest env verbatim",
        );
    }

    /// Sandbox B2: the guest `K2_HOOK_SOCK` is rewritten to the in-guest
    /// forwarder path, `K2_HOOK_TOKEN` is preserved verbatim, and the source
    /// `req.env` (the host-side staged env) is left unaffected.
    #[test]
    fn build_worker_invocation_rewrites_guest_hook_sock_keeps_token() {
        let mut env = HashMap::new();
        // Staged with the HOST cell-socket path (what DaemonPtySession sets).
        env.insert(
            "K2_HOOK_SOCK".to_string(),
            "/run/k2/cells/abc.sock".to_string(),
        );
        env.insert("K2_HOOK_TOKEN".to_string(), "scoped-bearer-xyz".to_string());
        let req = SpawnRequest {
            shell: None,
            cwd: None,
            env,
            window_size: win(80, 24),
            drain_on_exit: false,
            cell_socket: Some(PathBuf::from("/run/k2/cells/abc.sock")),
            workspace_root: None,
            tool_roots: vec![],
            session_id: None,
            cell_uid: None,
        };

        let inv = build_worker_invocation(&req, &VmCaps::default());

        // Guest hook sock REWRITTEN to the in-guest forwarder path.
        assert_eq!(
            inv.guest_env
                .iter()
                .find(|(k, _)| k == "K2_HOOK_SOCK")
                .map(|(_, v)| v.as_str()),
            Some(GUEST_HOOK_SOCK),
            "guest K2_HOOK_SOCK must be the in-guest forwarder path"
        );
        assert_eq!(GUEST_HOOK_SOCK, "/run/k2-hook.sock");

        // Token carried through UNCHANGED.
        assert_eq!(
            inv.guest_env
                .iter()
                .find(|(k, _)| k == "K2_HOOK_TOKEN")
                .map(|(_, v)| v.as_str()),
            Some("scoped-bearer-xyz"),
            "the per-cell scoped bearer must be preserved verbatim"
        );

        // Host-side source env is untouched (rewrite operates on the guest
        // copy only) — the original host cell-socket path still stands.
        assert_eq!(
            req.env.get("K2_HOOK_SOCK").map(String::as_str),
            Some("/run/k2/cells/abc.sock"),
            "the source req.env (host side) must be unaffected by the rewrite"
        );
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
            session_id: None,
            cell_uid: None,
        };
        let inv = build_worker_invocation(&req, &VmCaps::default());
        assert!(flag_value(&inv.argv, "--workspace-root").is_none());
        assert!(flag_value(&inv.argv, "--cell-socket").is_none());
        assert!(flag_value(&inv.argv, "--cwd").is_none());
        assert!(!inv.argv.iter().any(|a| a == "--tool-root"));
        assert!(!inv.argv.iter().any(|a| a == "--"));
        assert!(!inv.argv.iter().any(|a| a == "--drain-on-exit"));
        assert!(flag_value(&inv.argv, "--session-id").is_none());
        assert!(flag_value(&inv.argv, "--cell-uid").is_none());
        assert_eq!(flag_value(&inv.argv, "--vcpus"), Some("1"));
        assert_eq!(flag_value(&inv.argv, "--ram-mib"), Some("1024"));
    }
}
