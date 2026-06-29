//! `k2-vmm-worker` — the privilege-dropped libkrun VMM jail (Sandbox P2b).
//!
//! THIS IS THE SECURITY BOUNDARY. Get it wrong and a guest escape lands on the
//! host with host privileges = RCE. The whole file is written conservative >
//! clever, fail-closed on EVERY error path, and NEVER falls back to executing
//! the workload on the host (there is no `Passthrough` here).
//!
//! ## What it does (linux + `sandbox-microvm` only)
//!
//! 1. **Supervisor (parent, still root)**: parse argv → preflight hard-gates →
//!    create+join a cgroup v2 leaf → `clone()` a child into fresh mount + PID
//!    namespaces → forward signals → arm a cold-boot watchdog → wait → release
//!    the cgroup → propagate the child's exit status.
//! 2. **Jailed child**: builds THE HOST JAIL (§B steps 1-13, exact order) —
//!    `NO_NEW_PRIVS` → recursive-private mount → tmpfs root → overlay guest root
//!    → workspace bind → tool-root RO binds → `/dev/kvm` node bind → cell-socket
//!    bind → `pivot_root` + detach + rmdir oldroot → fresh `/proc` → drop all
//!    capabilities + `setgroups([kvm])` + `setgid`/`setuid` to a dedicated
//!    unprivileged uid → verify the drop → install a seccomp-bpf allowlist LAST
//!    → run the libkrun A-sequence (which `start_enter`s and never returns).
//!
//! The jail (steps 1-9) is the PRIMARY boundary; the cred-drop (11-12) means an
//! escape lands powerless; seccomp (13) is defense-in-depth.
//!
//! ## Build reality
//!
//! The real body is `#[cfg(all(target_os = "linux", feature = "sandbox-microvm"))]`
//! and links libkrun (`-lkrun`, see `build.rs`). On every other build this binary
//! is the stub `main` below that refuses to run. It is built + red-teamed ON the
//! Linux sandbox box (`k2-sandbox-01`); it cannot compile or run on macOS.

// ── Non-linux / feature-off stub ────────────────────────────────────────────
// On any build that is not (linux AND sandbox-microvm) this binary has no real
// body — it exists only so the `[[bin]]` target + Cargo wiring compile. It
// refuses to run rather than ever touching the host.
#[cfg(not(all(target_os = "linux", feature = "sandbox-microvm")))]
fn main() {
    eprintln!("k2-vmm-worker: linux+sandbox-microvm only");
    std::process::exit(1);
}

#[cfg(all(target_os = "linux", feature = "sandbox-microvm"))]
fn main() {
    worker::main();
}

// ── The real worker (linux + sandbox-microvm) ───────────────────────────────
#[cfg(all(target_os = "linux", feature = "sandbox-microvm"))]
mod worker {
    use std::ffi::CString;
    use std::os::fd::RawFd;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

    use nix::mount::{mount, umount2, MntFlags, MsFlags};
    use nix::sched::{clone, CloneFlags};
    use nix::sys::signal::{self, Signal};
    use nix::sys::wait::{waitpid, WaitStatus};
    use nix::unistd::{
        chdir, pivot_root, setgid, setgroups, setuid, Gid, Group, Pid, Uid, User,
    };

    // ── Pinned constants ────────────────────────────────────────────────────
    // Filesystem locations of the pre-staged guest base + libkrun runtime.
    // Assembled once by the on-box bootstrap (`p2b-onbox-bootstrap.md` §5b).
    const GUEST_BASE: &str = "/opt/k2/guest-base";
    const LIBKRUN_LIBDIR: &str = "/usr/local/lib64";
    const DEV_KVM: &str = "/dev/kvm";
    /// Dedicated unprivileged identity the jail drops to. An escape lands as
    /// THIS uid (powerless), never as the daemon/owner. Must exist on the box
    /// AND be a member of the `kvm` group (so 0660 `/dev/kvm` stays openable).
    const UNPRIV_USER: &str = "k2cell";
    const KVM_GROUP: &str = "kvm";

    // libkrun virtiofs root tag — PINNED from `/opt/libkrun/include/libkrun.h`
    // (`KRUN_FS_ROOT_TAG`). The guest kernel mounts the tag named "/dev/root"
    // as `/`. Asserted-by-construction; verify on-box if libkrun bumps it.
    const KRUN_FS_ROOT_TAG: &str = "/dev/root";

    // libkrun log enum ints — PINNED from /opt/libkrun/include/libkrun.h
    // (verified on-box 2026-06-29): target_fd is a SIGNED int and DEFAULT = -1
    // (stderr); INFO = 3; STYLE_ALWAYS = 1 (2 is NEVER).
    const KRUN_LOG_TARGET_DEFAULT: i32 = -1;
    const KRUN_LOG_LEVEL_INFO: u32 = 3;
    const KRUN_LOG_STYLE_ALWAYS: u32 = 1;

    // Hook channel vsock port (cell UDS forwarder). Channel itself is DEFERRED
    // to M1+ (needs an in-guest userland forwarder); we only wire the port.
    const HOOK_PORT: u32 = 1024;

    // RAM headroom over the guest's requested ram_mib for the VMM itself, and
    // the per-cell pids ceiling. Conservative.
    const CGROUP_RAM_HEADROOM_MIB: u64 = 128;
    const CGROUP_PIDS_MAX: u64 = 256;
    const CPU_PERIOD_US: u64 = 100_000;

    /// Cold-boot watchdog. Real boot is ~100-200ms; if the child has not
    /// signaled "jail+config OK" within this window it is wedged → SIGKILL.
    const COLD_BOOT_WATCHDOG_SECS: u64 = 30;

    /// Status byte the child writes to the status pipe right before it hands
    /// control to libkrun's blocking `start_enter`. Reading it = jail + libkrun
    /// config succeeded. EOF instead = the child died during setup.
    const STATUS_OK: u8 = b'K';

    // ── extern "C" — libkrun FFI (§C3) ──────────────────────────────────────
    // Hand-rolled, ONLY in this worker. Signatures PINNED from libkrun.h
    // (>= 1.18 — needs the virtiofs3 read-only flag). All return i32; <0 = fail.
    // Verify each signature against the installed header on-box before trust.
    //
    // `#[link(name = "krun")]` ties the link directly to this extern block so
    // rustc positions `-lkrun` AFTER the referencing objects (past the default
    // `--as-needed`, which silently drops a build-script `-l` placed too early).
    // build.rs still supplies the search path + rpath (linux + feature only).
    #[link(name = "krun")]
    #[allow(non_snake_case, dead_code)]
    extern "C" {
        fn krun_init_log(target_fd: i32, level: u32, style: u32, options: u32) -> i32;
        fn krun_create_ctx() -> i32;
        fn krun_set_vm_config(ctx_id: u32, num_vcpus: u8, ram_mib: u32) -> i32;
        fn krun_add_virtiofs3(
            ctx_id: u32,
            c_tag: *const libc::c_char,
            c_path: *const libc::c_char,
            shm_size: u64,
            read_only: bool,
        ) -> i32;
        fn krun_add_virtiofs(
            ctx_id: u32,
            c_tag: *const libc::c_char,
            c_path: *const libc::c_char,
        ) -> i32;
        fn krun_set_workdir(ctx_id: u32, c_workdir: *const libc::c_char) -> i32;
        fn krun_set_exec(
            ctx_id: u32,
            c_exec_path: *const libc::c_char,
            c_argv: *const *const libc::c_char,
            c_envp: *const *const libc::c_char,
        ) -> i32;
        fn krun_add_virtio_console_default(
            ctx_id: u32,
            in_fd: i32,
            out_fd: i32,
            err_fd: i32,
        ) -> i32;
        fn krun_add_vsock_port2(
            ctx_id: u32,
            port: u32,
            c_path: *const libc::c_char,
            listen: bool,
        ) -> i32;
        fn krun_start_enter(ctx_id: u32) -> i32;
    }

    // ── Config + errors ─────────────────────────────────────────────────────
    /// Parsed worker invocation (see `build_worker_invocation` in
    /// `k2-core/.../sandbox.rs`, the EXACT producer of our argv).
    ///
    /// `cols`/`rows`/`drain_on_exit` are parsed now (the wire format is stable)
    /// but consumed only by a later console-geometry / drain milestone, hence
    /// the targeted dead-code allow.
    #[derive(Debug, Default)]
    #[allow(dead_code)]
    struct WorkerConfig {
        vcpus: u32,
        ram_mib: u32,
        cols: u16,
        rows: u16,
        drain_on_exit: bool,
        workspace_root: Option<PathBuf>,
        cell_socket: Option<PathBuf>,
        tool_roots: Vec<PathBuf>,
        cwd: Option<PathBuf>,
        /// The real guest program (exec path). Required.
        program: String,
        /// Args ONLY (NOT including the program) — libkrun sets argv[0] itself.
        program_args: Vec<String>,
        /// Guest env (NUL-delimited `KEY=VAL` arriving on fd 3). NEVER argv /
        /// environ (would leak same-uid via /proc/<pid>/{cmdline,environ}).
        guest_env: Vec<(String, String)>,

        /// Numeric drop identity, resolved by NAME in `preflight` (as root,
        /// BEFORE any unshare/mount/pivot_root — `/etc/passwd` + libnss are
        /// reachable there). The §B-step-11 drop uses ONLY these cached numbers;
        /// it must NOT call NSS (`getpwnam`/`getgrnam`) after `pivot_root`, where
        /// the jailed root has no `/etc/passwd`/`/etc/group`/nsswitch/libnss.
        /// `0` is a sentinel "unresolved" and is rejected fail-closed at drop.
        unpriv_uid: u32,
        unpriv_gid: u32,
        /// The `kvm` group's gid — supplementary group kept at drop so 0660
        /// `/dev/kvm` stays openable.
        kvm_gid: u32,
    }

    /// One catch-all error. We never branch on the variant — every error is
    /// fatal and fail-closed; the string is for the log only.
    type WErr = String;

    fn fail<T>(msg: impl Into<String>) -> Result<T, WErr> {
        Err(msg.into())
    }

    // ── A0 — supervisor control flow ────────────────────────────────────────
    /// Global handle to the cloned child so the async-signal-safe SIGTERM/INT
    /// handler can forward to it. 0 = no child yet.
    static CHILD_PID: AtomicI32 = AtomicI32::new(0);
    /// Set true once the child signals boot-complete; disarms the watchdog.
    static BOOTED: AtomicBool = AtomicBool::new(false);

    pub fn main() {
        // Parse first so a bad invocation never touches the host.
        let args: Vec<String> = std::env::args().skip(1).collect();
        let mut cfg = match parse_args(&args) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("k2-vmm-worker: bad invocation: {e}");
                std::process::exit(2);
            }
        };
        // Guest env rides fd 3 (NUL-delimited). Absent fd 3 ⇒ empty (hand tests).
        cfg.guest_env = read_guest_env_fd3();

        // Preflight hard gates (each fail → nonzero exit, host untouched). Takes
        // `&mut` to cache the numeric drop ids while NSS is still reachable.
        if let Err(e) = preflight(&mut cfg) {
            eprintln!("k2-vmm-worker: preflight failed: {e}");
            std::process::exit(3);
        }

        // cgroup v2: create the leaf, set limits, join the PARENT before clone
        // so the cloned child inherits membership.
        let cgroup_dir = match cgroup_create_and_join(&cfg) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("k2-vmm-worker: cgroup setup failed: {e}");
                std::process::exit(4);
            }
        };

        // Status pipe: child writes STATUS_OK right before start_enter; EOF = it
        // died in setup. pipe2(0): both ends inheritable across clone().
        let (status_r, status_w) = match pipe2_raw() {
            Ok(p) => p,
            Err(e) => {
                eprintln!("k2-vmm-worker: pipe2 failed: {e}");
                let _ = std::fs::remove_dir_all(&cgroup_dir);
                std::process::exit(5);
            }
        };

        // clone() into fresh mount + PID namespaces. NOT NEWUSER (rootless
        // user-ns maps escape back to the owner on a single-user box — rejected;
        // we drop to a real dedicated uid instead). NOT NEWNET (libkrun TSI
        // needs host sockets; net jail = P4). SIGCHLD so waitpid sees it.
        let flags = CloneFlags::CLONE_NEWNS | CloneFlags::CLONE_NEWPID;
        // 1 MiB child stack (the child does mounts + a blocking start_enter).
        let mut stack = vec![0u8; 1024 * 1024];
        let child_cfg = std::mem::take(&mut cfg);
        let child_status_w = status_w;
        let child = {
            let cb = Box::new(move || jailed_child(&child_cfg, child_status_w));
            // SAFETY: `clone` is unsafe (the child runs with a fresh, partly
            // shared address space until it execs/exits). Our child callback
            // only does mounts + cred-drop + a blocking libkrun call and never
            // returns to Rust on success, which is sound for this usage.
            match unsafe { clone(cb, &mut stack, flags, Some(libc::SIGCHLD)) } {
                Ok(pid) => pid,
                Err(e) => {
                    eprintln!("k2-vmm-worker: clone failed: {e}");
                    let _ = close_raw(status_r);
                    let _ = close_raw(status_w);
                    let _ = std::fs::remove_dir_all(&cgroup_dir);
                    std::process::exit(6);
                }
            }
        };
        CHILD_PID.store(child.as_raw(), Ordering::SeqCst);

        // Parent no longer needs the write end (else it never sees EOF).
        let _ = close_raw(status_w);

        // Forward SIGTERM/SIGINT to the child; arm the cold-boot watchdog.
        install_signal_forwarding();
        spawn_cold_boot_watchdog(child);

        // Block on the status pipe: STATUS_OK = booted; EOF = died in setup.
        let booted = read_status_byte(status_r) == Some(STATUS_OK);
        let _ = close_raw(status_r);
        BOOTED.store(true, Ordering::SeqCst); // disarm watchdog either way
        if !booted {
            eprintln!("k2-vmm-worker: child died during jail/config (no boot)");
            // Reap so it isn't a zombie; the cgroup is released below.
        }

        // Reap the child + propagate its exit status faithfully.
        let exit_code = match waitpid(child, None) {
            Ok(WaitStatus::Exited(_, code)) => code,
            Ok(WaitStatus::Signaled(_, sig, _)) => 128 + sig as i32,
            Ok(other) => {
                eprintln!("k2-vmm-worker: unexpected wait status: {other:?}");
                70
            }
            Err(e) => {
                eprintln!("k2-vmm-worker: waitpid failed: {e}");
                71
            }
        };

        // Best-effort cgroup release (the daemon also rmdirs on ChildExit; this
        // is belt-and-braces — never fatal).
        let _ = std::fs::remove_dir_all(&cgroup_dir);

        std::process::exit(exit_code);
    }

    // ── A1 — argv parser (hand-rolled positional scanner; NOT clap) ──────────
    fn parse_args(args: &[String]) -> Result<WorkerConfig, WErr> {
        let mut cfg = WorkerConfig::default();
        let mut i = 0;
        let mut saw_vcpus = false;
        let mut saw_ram = false;

        // Helper: the value following a flag, or a parse error.
        macro_rules! val {
            ($flag:expr) => {{
                i += 1;
                match args.get(i) {
                    Some(v) => v,
                    None => return fail(format!("missing value for {}", $flag)),
                }
            }};
        }

        while i < args.len() {
            let a = args[i].as_str();
            match a {
                "--" => {
                    // Everything after `--` is the guest command, verbatim.
                    i += 1;
                    if let Some(prog) = args.get(i) {
                        cfg.program = prog.clone();
                        cfg.program_args = args[(i + 1)..].to_vec();
                    }
                    break;
                }
                "--vcpus" => {
                    cfg.vcpus = val!("--vcpus")
                        .parse()
                        .map_err(|_| "invalid --vcpus".to_string())?;
                    saw_vcpus = true;
                }
                "--ram-mib" => {
                    cfg.ram_mib = val!("--ram-mib")
                        .parse()
                        .map_err(|_| "invalid --ram-mib".to_string())?;
                    saw_ram = true;
                }
                "--cols" => {
                    cfg.cols = val!("--cols")
                        .parse()
                        .map_err(|_| "invalid --cols".to_string())?;
                }
                "--rows" => {
                    cfg.rows = val!("--rows")
                        .parse()
                        .map_err(|_| "invalid --rows".to_string())?;
                }
                "--drain-on-exit" => cfg.drain_on_exit = true,
                "--workspace-root" => {
                    cfg.workspace_root = Some(PathBuf::from(val!("--workspace-root")));
                }
                "--cell-socket" => {
                    cfg.cell_socket = Some(PathBuf::from(val!("--cell-socket")));
                }
                "--tool-root" => {
                    cfg.tool_roots.push(PathBuf::from(val!("--tool-root")));
                }
                "--cwd" => {
                    cfg.cwd = Some(PathBuf::from(val!("--cwd")));
                }
                other => return fail(format!("unknown flag: {other}")),
            }
            i += 1;
        }

        // Fail-closed validation.
        if !saw_vcpus || cfg.vcpus == 0 {
            return fail("vcpus must be >= 1");
        }
        if !saw_ram || cfg.ram_mib < 256 {
            return fail("ram-mib must be >= 256");
        }
        if cfg.program.is_empty() {
            return fail("no program after `--`");
        }
        Ok(cfg)
    }

    /// Read NUL-delimited `KEY=VAL` pairs from fd 3 (the guest env channel).
    /// Absent / empty fd 3 ⇒ empty env (tolerated for hand tests). Malformed
    /// entries (no `=`) are dropped, never fatal — env is not security state.
    fn read_guest_env_fd3() -> Vec<(String, String)> {
        use std::io::Read;
        // Is fd 3 actually open? fcntl(F_GETFD) fails with EBADF if not.
        if unsafe { libc::fcntl(3, libc::F_GETFD) } < 0 {
            return Vec::new();
        }
        // SAFETY: we own fd 3 by the worker contract; wrap to read then drop.
        let mut f = unsafe { <std::fs::File as std::os::fd::FromRawFd>::from_raw_fd(3) };
        let mut buf = Vec::new();
        if f.read_to_end(&mut buf).is_err() {
            return Vec::new();
        }
        buf.split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .filter_map(|s| {
                let s = String::from_utf8_lossy(s);
                s.split_once('=')
                    .map(|(k, v)| (k.to_string(), v.to_string()))
            })
            .collect()
    }

    // ── A2 — preflight hard gates ───────────────────────────────────────────
    // Takes `&mut` so it can CACHE the numeric drop ids it resolves by name
    // here (as root, pre-pivot). The jail's §B-step-11 drop reads ONLY those
    // cached numbers — never NSS — because after `pivot_root` the jailed root
    // has no passwd/group/nsswitch/libnss and a name lookup would ENOENT.
    fn preflight(cfg: &mut WorkerConfig) -> Result<(), WErr> {
        // /dev/kvm openable RW. CLOSE immediately — a dirfd/devfd kept open
        // across pivot_root would be a re-traversal handle to the host (§F).
        {
            let kvm = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(DEV_KVM)
                .map_err(|e| format!("/dev/kvm not openable RW: {e}"))?;
            drop(kvm);
        }

        // libkrunfw.so resolvable in the libkrun libdir.
        let krunfw_present = std::fs::read_dir(LIBKRUN_LIBDIR)
            .map_err(|e| format!("libkrun libdir {LIBKRUN_LIBDIR}: {e}"))?
            .filter_map(|e| e.ok())
            .any(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("libkrunfw.so")
            });
        if !krunfw_present {
            return fail(format!("libkrunfw.so not found in {LIBKRUN_LIBDIR}"));
        }

        // Base guest image dir present (the RO overlay lowerdir).
        if !Path::new(GUEST_BASE).is_dir() {
            return fail(format!("guest base image dir missing: {GUEST_BASE}"));
        }

        // Dedicated unprivileged uid/gid exist + uid is a member of `kvm`.
        // Resolve by NAME here (root, pre-pivot) and CACHE the numeric ids on
        // cfg — the drop step uses ONLY the cached numbers (no post-pivot NSS).
        let user = User::from_name(UNPRIV_USER)
            .map_err(|e| format!("lookup user {UNPRIV_USER}: {e}"))?
            .ok_or_else(|| format!("dedicated user {UNPRIV_USER} does not exist"))?;
        let kvm = Group::from_name(KVM_GROUP)
            .map_err(|e| format!("lookup group {KVM_GROUP}: {e}"))?
            .ok_or_else(|| format!("group {KVM_GROUP} does not exist"))?;
        let in_kvm = user.gid == kvm.gid || kvm.mem.iter().any(|m| m == UNPRIV_USER);
        if !in_kvm {
            return fail(format!("{UNPRIV_USER} is not a member of the {KVM_GROUP} group"));
        }
        // Fail-closed: the drop identity must NOT be root (a misconfigured
        // k2cell=uid 0 would make the "drop" a no-op, leaving an escape on the
        // owner). Reject before we ever build the jail.
        if user.uid.as_raw() == 0 || user.gid.as_raw() == 0 {
            return fail(format!("{UNPRIV_USER} resolves to uid/gid 0 — refusing to run"));
        }
        // Cache the NUMERIC ids for the post-pivot drop (no NSS in the jail).
        cfg.unpriv_uid = user.uid.as_raw();
        cfg.unpriv_gid = user.gid.as_raw();
        cfg.kvm_gid = kvm.gid.as_raw();

        // libkrun >= 1.18: there is no runtime version symbol; the link itself
        // pins it (build.rs links the installed lib, and add_virtiofs3's
        // read_only flag only exists >= 1.18).
        // TODO(on-box): assert `pkg-config --modversion libkrun >= 1.18` in
        // build.rs / bootstrap; record in /opt/k2-sandbox-versions.txt.

        // Sanity: a workspace/tool/cell path that does not exist is fatal here
        // rather than mid-jail (cleaner failure, no half-built namespace).
        if let Some(ws) = &cfg.workspace_root {
            if !ws.is_dir() {
                return fail(format!("workspace-root not a dir: {}", ws.display()));
            }
        }
        for t in &cfg.tool_roots {
            if !t.exists() {
                return fail(format!("tool-root missing: {}", t.display()));
            }
        }
        if let Some(cs) = &cfg.cell_socket {
            if !cs.exists() {
                return fail(format!("cell-socket missing: {}", cs.display()));
            }
        }
        Ok(())
    }

    // ── A3 — cgroup v2 (direct cgroupfs writes, no crate) ───────────────────
    /// Create `/sys/fs/cgroup/k2cells/<leaf>/`, set the limits, and join the
    /// PARENT (so the cloned child inherits). Returns the leaf path; the daemon
    /// also rmdirs it on ChildExit (best-effort here on supervisor exit).
    ///
    /// Leaf = `cell-<worker-pid>` (unique per worker). The daemon captures the
    /// worker's child pid and derives the same path for its own rmdir.
    /// TODO(on-box): consider an explicit `--session-id` so the daemon names
    /// the cgroup deterministically rather than deriving from pid.
    fn cgroup_create_and_join(cfg: &WorkerConfig) -> Result<PathBuf, WErr> {
        let leaf = format!("cell-{}", std::process::id());
        let parent = Path::new("/sys/fs/cgroup/k2cells");
        let dir = parent.join(&leaf);

        // Ensure the parent exists + delegates the controllers we set on the
        // leaf. Best-effort on subtree_control (it fails harmlessly if already
        // enabled); the leaf-limit writes below are the fail-closed gate.
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create cgroup parent: {e}"))?;
        let _ = std::fs::write(
            parent.join("cgroup.subtree_control"),
            b"+memory +pids +cpu",
        );
        std::fs::create_dir_all(&dir).map_err(|e| format!("create cgroup leaf: {e}"))?;

        // memory.max = requested + headroom (in bytes); no swap; pids ceiling.
        let mem_bytes = (cfg.ram_mib as u64 + CGROUP_RAM_HEADROOM_MIB) * 1024 * 1024;
        write_cgroup(&dir, "memory.max", &mem_bytes.to_string())?;
        write_cgroup(&dir, "memory.swap.max", "0")?;
        write_cgroup(&dir, "pids.max", &CGROUP_PIDS_MAX.to_string())?;
        // cpu.max = "<quota> <period>"; quota = vcpus * period.
        let quota = cfg.vcpus as u64 * CPU_PERIOD_US;
        write_cgroup(&dir, "cpu.max", &format!("{quota} {CPU_PERIOD_US}"))?;

        // Join the PARENT (this supervisor) BEFORE clone — child inherits.
        write_cgroup(&dir, "cgroup.procs", &std::process::id().to_string())?;
        Ok(dir)
    }

    fn write_cgroup(dir: &Path, file: &str, val: &str) -> Result<(), WErr> {
        std::fs::write(dir.join(file), val)
            .map_err(|e| format!("write cgroup {file}={val}: {e}"))
    }

    // ── A4/§B — THE HOST JAIL + libkrun handoff (runs in the cloned child) ───
    /// The cloned child entry point. Builds the jail (§B), then runs libkrun
    /// (§A5) which `start_enter`s and never returns. ANY error → write nothing
    /// to the status pipe (EOF tells the parent we died in setup) → exit
    /// nonzero. NEVER execs the workload on the host.
    ///
    /// Returns `isize` (the clone callback contract); in practice every path
    /// either `exit()`s or libkrun blocks forever.
    fn jailed_child(cfg: &WorkerConfig, status_w: RawFd) -> isize {
        if let Err(e) = host_jail(cfg) {
            eprintln!("k2-vmm-worker[child]: jail failed: {e}");
            // Do NOT write STATUS_OK; closing on exit gives the parent EOF.
            let _ = close_raw(status_w);
            std::process::exit(40);
        }
        // From here the host fs is unreachable and we are unprivileged +
        // seccomp-confined. Hand off to libkrun (never returns on success).
        run_libkrun(cfg, status_w)
    }

    /// §B — THE HOST JAIL, exact ordered sequence (security core).
    fn host_jail(cfg: &WorkerConfig) -> Result<(), WErr> {
        // 1. NO_NEW_PRIVS FIRST — required for a non-root seccomp filter and
        //    neuters any setuid bits we might traverse.
        if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
            return fail("prctl(PR_SET_NO_NEW_PRIVS) failed");
        }

        // 2. Make our mount namespace recursively PRIVATE — nothing we mount
        //    below propagates back to the host ns. BEFORE all binds.
        mount(
            None::<&str>,
            "/",
            None::<&str>,
            MsFlags::MS_REC | MsFlags::MS_PRIVATE,
            None::<&str>,
        )
        .map_err(|e| format!("mount / MS_REC|MS_PRIVATE: {e}"))?;

        // 3. Fresh private tmpfs NEWROOT (nosuid,nodev,0700). Its size bounds
        //    the guest's writable disk/inode usage via the overlay upper.
        let newroot = PathBuf::from(format!("/run/k2cell-{}", std::process::id()));
        std::fs::create_dir_all(&newroot)
            .map_err(|e| format!("mkdir NEWROOT: {e}"))?;
        // tmpfs size = guest ram budget (a generous ceiling for scratch);
        // TODO(on-box): tune independently of ram if cells need bigger scratch.
        let tmpfs_opts = format!("size={}m,mode=0700", cfg.ram_mib);
        mount(
            Some("tmpfs"),
            &newroot,
            Some("tmpfs"),
            MsFlags::MS_NOSUID | MsFlags::MS_NODEV,
            Some(tmpfs_opts.as_str()),
        )
        .map_err(|e| format!("mount tmpfs NEWROOT: {e}"))?;

        // 4. Overlay guest root at NEWROOT/guest: lower = RO base image,
        //    upper+work = throwaway dirs on the per-cell tmpfs → RW root that
        //    shares the RO base but discards all writes per cell.
        let guest = newroot.join("guest");
        let upper = newroot.join(".overlay-upper");
        let work = newroot.join(".overlay-work");
        for d in [&guest, &upper, &work] {
            std::fs::create_dir_all(d)
                .map_err(|e| format!("mkdir {}: {e}", d.display()))?;
        }
        let ov = format!(
            "lowerdir={},upperdir={},workdir={}",
            GUEST_BASE,
            upper.display(),
            work.display()
        );
        mount(
            Some("overlay"),
            &guest,
            Some("overlay"),
            MsFlags::MS_NOSUID | MsFlags::MS_NODEV,
            Some(ov.as_str()),
        )
        .map_err(|e| format!("mount overlay guest root: {e}"))?;

        // 5. Workspace → NEWROOT/workspace-src, RW. CANONICALIZE + assert leaf
        //    dir first (TOCTOU). Initial bind, then a SEPARATE remount to apply
        //    nosuid,nodev (the kernel ignores those on the first bind).
        if let Some(ws) = &cfg.workspace_root {
            let ws = ws
                .canonicalize()
                .map_err(|e| format!("canonicalize workspace: {e}"))?;
            if !ws.is_dir() {
                return fail("workspace-root is not a directory after canonicalize");
            }
            let dst = newroot.join("workspace-src");
            std::fs::create_dir_all(&dst)
                .map_err(|e| format!("mkdir workspace-src: {e}"))?;
            mount(
                Some(ws.as_path()),
                &dst,
                None::<&str>,
                MsFlags::MS_BIND | MsFlags::MS_REC,
                None::<&str>,
            )
            .map_err(|e| format!("bind workspace: {e}"))?;
            mount(
                None::<&str>,
                &dst,
                None::<&str>,
                MsFlags::MS_BIND
                    | MsFlags::MS_REMOUNT
                    | MsFlags::MS_NOSUID
                    | MsFlags::MS_NODEV,
                None::<&str>,
            )
            .map_err(|e| format!("remount workspace nosuid,nodev: {e}"))?;
        }

        // 6. Tool roots → NEWROOT/tools/<i>, READ-ONLY. Initial bind ignores
        //    MS_RDONLY → a SEPARATE remount actually applies ro,nosuid,nodev.
        for (idx, root) in cfg.tool_roots.iter().enumerate() {
            let src = root
                .canonicalize()
                .map_err(|e| format!("canonicalize tool-root {idx}: {e}"))?;
            let dst = newroot.join(format!("tools/{idx}"));
            std::fs::create_dir_all(&dst)
                .map_err(|e| format!("mkdir tool dst {idx}: {e}"))?;
            mount(
                Some(src.as_path()),
                &dst,
                None::<&str>,
                MsFlags::MS_BIND | MsFlags::MS_REC,
                None::<&str>,
            )
            .map_err(|e| format!("bind tool-root {idx}: {e}"))?;
            mount(
                None::<&str>,
                &dst,
                None::<&str>,
                MsFlags::MS_BIND
                    | MsFlags::MS_REMOUNT
                    | MsFlags::MS_RDONLY
                    | MsFlags::MS_NOSUID
                    | MsFlags::MS_NODEV,
                None::<&str>,
            )
            .map_err(|e| format!("remount tool-root {idx} ro: {e}"))?;
        }

        // 7. /dev/kvm: mkdir NEWROOT/dev; bind ONLY the single device node
        //    (NEVER the full host /dev). Target must be a file before binding.
        let dev = newroot.join("dev");
        std::fs::create_dir_all(&dev).map_err(|e| format!("mkdir NEWROOT/dev: {e}"))?;
        let kvm_dst = dev.join("kvm");
        std::fs::File::create(&kvm_dst).map_err(|e| format!("touch dev/kvm: {e}"))?;
        mount(
            Some(DEV_KVM),
            &kvm_dst,
            None::<&str>,
            MsFlags::MS_BIND,
            None::<&str>,
        )
        .map_err(|e| format!("bind /dev/kvm node: {e}"))?;

        // 8. Cell socket → NEWROOT/cell.sock (M1 binds it; the channel itself is
        //    deferred — the in-guest forwarder + SO_PEERCRED adaptation land
        //    with the channel milestone, see §F).
        if let Some(sock) = &cfg.cell_socket {
            let dst = newroot.join("cell.sock");
            std::fs::File::create(&dst).map_err(|e| format!("touch cell.sock: {e}"))?;
            mount(
                Some(sock.as_path()),
                &dst,
                None::<&str>,
                MsFlags::MS_BIND,
                None::<&str>,
            )
            .map_err(|e| format!("bind cell socket: {e}"))?;
        }

        // 9. pivot_root — THE control. Make the host fs entirely unreachable so
        //    any virtio-fs symlink-escape resolves to nothing. Detach + rmdir
        //    oldroot so there is no re-traversal path back.
        let oldroot = newroot.join("oldroot");
        std::fs::create_dir_all(&oldroot)
            .map_err(|e| format!("mkdir oldroot: {e}"))?;
        pivot_root(&newroot, &oldroot)
            .map_err(|e| format!("pivot_root: {e}"))?;
        chdir("/").map_err(|e| format!("chdir / after pivot: {e}"))?;
        umount2("/oldroot", MntFlags::MNT_DETACH)
            .map_err(|e| format!("detach oldroot: {e}"))?;
        std::fs::remove_dir("/oldroot")
            .map_err(|e| format!("rmdir oldroot: {e}"))?;

        // 10. Fresh /proc (nosuid,nodev,noexec). No /sys.
        std::fs::create_dir_all("/proc").map_err(|e| format!("mkdir /proc: {e}"))?;
        mount(
            Some("proc"),
            "/proc",
            Some("proc"),
            MsFlags::MS_NOSUID | MsFlags::MS_NODEV | MsFlags::MS_NOEXEC,
            None::<&str>,
        )
        .map_err(|e| format!("mount /proc: {e}"))?;

        // 11. DROP PRIVS, in order. capbset drop → setgroups([kvm]) → setgid →
        //     setuid. setgroups MUST be exactly [kvm] (NOT [] — that loses the
        //     kvm group → 0660 /dev/kvm becomes unopenable). Drop BEFORE seccomp
        //     so seccomp can forbid the setuid family entirely.
        drop_privileges(cfg)?;

        // 12. VERIFY the drop actually took (fail-closed if anything is wrong).
        verify_drop(cfg)?;

        // 13. seccomp-bpf LAST. Defense-in-depth; steps 1-9 are the primary
        //     boundary. See build_and_apply_seccomp for the allowlist + the
        //     LOG-first → KILL on-box derivation note.
        build_and_apply_seccomp()?;

        Ok(())
    }

    /// §B.11 — drop all capabilities then the uid/gid, in the safe order.
    ///
    /// Uses ONLY the numeric ids cached on `cfg` by `preflight` (resolved by
    /// name while still root + pre-pivot). It deliberately does NOT call NSS
    /// (`getpwnam`/`getgrnam`) — after `pivot_root` the jailed root has no
    /// passwd/group/nsswitch/libnss, so a name lookup here would ENOENT and the
    /// drop would fail (the exact bug this guards against).
    fn drop_privileges(cfg: &WorkerConfig) -> Result<(), WErr> {
        // Belt-and-braces: `preflight` already resolved these (and rejected
        // uid/gid 0); refuse the sentinel "unresolved" 0 so a bypassed preflight
        // can never silently make the drop a no-op.
        if cfg.unpriv_uid == 0 || cfg.unpriv_gid == 0 || cfg.kvm_gid == 0 {
            return fail("drop ids unresolved (preflight not run) — refusing to drop");
        }

        // Drop every capability from the bounding set so nothing can be
        // re-acquired across the (already NNP-blocked) exec surface.
        // CAP_LAST_CAP is kernel-version dependent; iterate generously and
        // ignore EINVAL past the real last cap.
        for cap in 0..=63 {
            let r = unsafe { libc::prctl(libc::PR_CAPBSET_DROP, cap as libc::c_ulong, 0, 0, 0) };
            // EINVAL = cap number above CAP_LAST_CAP → fine; anything else on a
            // valid cap is a hard failure (we must not run with caps retained).
            if r != 0 {
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() != Some(libc::EINVAL) {
                    return fail(format!("PR_CAPBSET_DROP({cap}): {err}"));
                }
            }
        }

        // Supplementary groups = EXACTLY [kvm] (cached numeric gid).
        setgroups(&[Gid::from_raw(cfg.kvm_gid)])
            .map_err(|e| format!("setgroups([kvm]): {e}"))?;
        // gid before uid (you can't setgid after dropping uid).
        setgid(Gid::from_raw(cfg.unpriv_gid))
            .map_err(|e| format!("setgid: {e}"))?;
        setuid(Uid::from_raw(cfg.unpriv_uid))
            .map_err(|e| format!("setuid: {e}"))?;
        Ok(())
    }

    /// §B.12 — assert the drop took: real+effective uid/gid are the unpriv id,
    /// and a re-elevation attempt fails with EPERM. Compares against the cached
    /// numeric ids (no post-pivot NSS).
    fn verify_drop(cfg: &WorkerConfig) -> Result<(), WErr> {
        let uid = cfg.unpriv_uid;
        let gid = cfg.unpriv_gid;

        let ruid = unsafe { libc::getuid() };
        let euid = unsafe { libc::geteuid() };
        let rgid = unsafe { libc::getgid() };
        if ruid != uid || euid != uid {
            return fail(format!("uid not dropped (r={ruid} e={euid} want={uid})"));
        }
        if rgid != gid {
            return fail(format!("gid not dropped (r={rgid} want={gid})"));
        }
        // setuid(0) MUST fail with EPERM now (proves we cannot re-elevate).
        if unsafe { libc::setuid(0) } == 0 {
            return fail("re-elevation to uid 0 SUCCEEDED — drop is broken");
        }
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() != Some(libc::EPERM) {
            return fail(format!("setuid(0) failed with non-EPERM: {err}"));
        }
        Ok(())
    }

    // ── §B.13 — seccomp allowlist ───────────────────────────────────────────
    //
    // TODO(on-box): DERIVE THE EXACT SYSCALL SET via `strace -f` on a real boot
    // and SHIP `SeccompAction::Log` FIRST (record zero violations across the
    // acceptance suite) THEN flip SECCOMP_FAIL_ACTION to KillProcess. The list
    // below is a CONSERVATIVE-BUT-UNVALIDATED starting point; it WILL be missing
    // or over-granting syscalls until validated on the box. Do NOT trust it as
    // the boundary — steps 1-9 are.
    //
    /// Default action for a syscall NOT on the allowlist. Starts as `Log` so the
    /// first on-box boot records the real syscall set without killing the VMM;
    /// FLIP to `SeccompAction::KillProcess` once the allowlist is validated.
    fn seccomp_fail_action() -> seccompiler::SeccompAction {
        // TODO(on-box): change to SeccompAction::KillProcess after strace.
        seccompiler::SeccompAction::Log
    }

    fn build_and_apply_seccomp() -> Result<(), WErr> {
        use seccompiler::{SeccompAction, SeccompFilter, TargetArch};
        use std::collections::BTreeMap;

        // Allowlist: VMM runtime only. Each entry maps a syscall number to an
        // empty rule vec (= allow unconditionally). ioctl is allowed broadly
        // for now; TODO(on-box): CONSTRAIN ioctl arg1 to the KVM request set
        // (KVM_RUN, KVM_SET_*, etc.) via a SeccompCondition once enumerated.
        let allow: &[libc::c_long] = &[
            // I/O the VMM + virtio do constantly.
            libc::SYS_read,
            libc::SYS_write,
            libc::SYS_readv,
            libc::SYS_writev,
            libc::SYS_pread64,
            libc::SYS_pwrite64,
            libc::SYS_close,
            libc::SYS_dup,
            libc::SYS_dup3,
            libc::SYS_fcntl,
            // ioctl — KVM control surface (TODO constrain arg1 to KVM reqs).
            libc::SYS_ioctl,
            // Memory.
            libc::SYS_mmap,
            libc::SYS_munmap,
            libc::SYS_mprotect,
            libc::SYS_madvise,
            libc::SYS_brk,
            // Sync / scheduling.
            libc::SYS_futex,
            libc::SYS_sched_yield,
            // Event loop.
            libc::SYS_epoll_create1,
            libc::SYS_epoll_ctl,
            libc::SYS_epoll_pwait,
            libc::SYS_eventfd2,
            libc::SYS_ppoll,
            libc::SYS_poll,
            // Signals.
            libc::SYS_rt_sigaction,
            libc::SYS_rt_sigprocmask,
            libc::SYS_rt_sigreturn,
            // Time.
            libc::SYS_clock_nanosleep,
            libc::SYS_nanosleep,
            // Exit / restart.
            libc::SYS_exit,
            libc::SYS_exit_group,
            libc::SYS_restart_syscall,
            // RNG.
            libc::SYS_getrandom,
            // Thread creation for the vCPU + IO threads.
            libc::SYS_clone,
            libc::SYS_clone3,
            libc::SYS_set_robust_list,
            // TSI (libkrun's transparent socket impl — net stays host-side, M1).
            libc::SYS_socket,
            libc::SYS_connect,
            libc::SYS_sendto,
            libc::SYS_recvfrom,
            libc::SYS_accept4,
            libc::SYS_bind,
            libc::SYS_getsockname,
            libc::SYS_setsockopt,
        ];
        // EXPLICITLY NOT on the list (would be denied / logged): ptrace,
        // process_vm_*, mount, pivot_root, setns, unshare, keyctl, bpf,
        // userfaultfd, io_uring_*. Those are the escape-amplifying syscalls.

        let mut rules: BTreeMap<i64, Vec<seccompiler::SeccompRule>> = BTreeMap::new();
        for &nr in allow {
            rules.insert(nr as i64, vec![]);
        }

        let filter = SeccompFilter::new(
            rules,
            seccomp_fail_action(),  // mismatch action (Log first, then KILL)
            SeccompAction::Allow,   // matched action = allow
            TargetArch::x86_64,
        )
        .map_err(|e| format!("build seccomp filter: {e}"))?;

        let prog: seccompiler::BpfProgram = filter
            .try_into()
            .map_err(|e| format!("compile seccomp filter: {e}"))?;
        seccompiler::apply_filter(&prog)
            .map_err(|e| format!("apply seccomp filter: {e}"))?;
        Ok(())
    }

    // ── A5 — libkrun A-sequence (after jail+drop; all paths jail-relative) ───
    /// Configure + boot the microVM. `krun_start_enter` BLOCKS until the guest
    /// exits, then we propagate its code. Never returns on success.
    fn run_libkrun(cfg: &WorkerConfig, status_w: RawFd) -> isize {
        match run_libkrun_inner(cfg, status_w) {
            Ok(_) => unreachable!("krun_start_enter returned without exiting"),
            Err(e) => {
                eprintln!("k2-vmm-worker[child]: libkrun config failed: {e}");
                // No STATUS_OK written → parent sees EOF = died in setup.
                let _ = close_raw(status_w);
                std::process::exit(41);
            }
        }
    }

    fn run_libkrun_inner(cfg: &WorkerConfig, status_w: RawFd) -> Result<(), WErr> {
        // Keep every CString alive until after start_enter.
        let mut keep: Vec<CString> = Vec::new();
        let cstr = |s: &str, keep: &mut Vec<CString>| -> Result<*const libc::c_char, WErr> {
            let c = CString::new(s).map_err(|_| format!("NUL in string: {s}"))?;
            let p = c.as_ptr();
            keep.push(c);
            Ok(p)
        };

        // Logging.
        let r = unsafe {
            krun_init_log(
                KRUN_LOG_TARGET_DEFAULT,
                KRUN_LOG_LEVEL_INFO,
                KRUN_LOG_STYLE_ALWAYS,
                0,
            )
        };
        if r < 0 {
            return fail(format!("krun_init_log: {r}"));
        }

        // Context.
        let ctx = unsafe { krun_create_ctx() };
        if ctx < 0 {
            return fail(format!("krun_create_ctx: {ctx}"));
        }
        let ctx = ctx as u32;

        // VM resources. num_vcpus is u8 in libkrun; clamp defensively.
        let vcpus: u8 = cfg.vcpus.min(u8::MAX as u32) as u8;
        let r = unsafe { krun_set_vm_config(ctx, vcpus, cfg.ram_mib) };
        if r < 0 {
            return fail(format!("krun_set_vm_config: {r}"));
        }

        // RW root = the overlay at jail-relative /guest, tagged "/dev/root".
        let tag = cstr(KRUN_FS_ROOT_TAG, &mut keep)?;
        let guest = cstr("/guest", &mut keep)?;
        let r = unsafe { krun_add_virtiofs3(ctx, tag, guest, 0, false) };
        if r < 0 {
            return fail(format!("krun_add_virtiofs3 root: {r}"));
        }

        // Workspace = RW virtiofs, tag "workspace", jail-relative source.
        if cfg.workspace_root.is_some() {
            let wtag = cstr("workspace", &mut keep)?;
            let wsrc = cstr("/workspace-src", &mut keep)?;
            let r = unsafe { krun_add_virtiofs3(ctx, wtag, wsrc, 0, false) };
            if r < 0 {
                return fail(format!("krun_add_virtiofs3 workspace: {r}"));
            }
        }

        // Tool roots = RO virtiofs, tags "tool{i}", jail-relative /tools/{i}.
        for idx in 0..cfg.tool_roots.len() {
            let ttag = cstr(&format!("tool{idx}"), &mut keep)?;
            let tsrc = cstr(&format!("/tools/{idx}"), &mut keep)?;
            let r = unsafe { krun_add_virtiofs3(ctx, ttag, tsrc, 0, true) };
            if r < 0 {
                return fail(format!("krun_add_virtiofs3 tool{idx}: {r}"));
            }
        }

        // Working directory inside the guest.
        let wd = cfg
            .cwd
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| "/workspace".to_string());
        let wdc = cstr(&wd, &mut keep)?;
        let r = unsafe { krun_set_workdir(ctx, wdc) };
        if r < 0 {
            return fail(format!("krun_set_workdir: {r}"));
        }

        // Cell socket → vsock port (forwarder = guest userland, DEFERRED M1).
        if cfg.cell_socket.is_some() {
            let csock = cstr("/cell.sock", &mut keep)?;
            let r = unsafe { krun_add_vsock_port2(ctx, HOOK_PORT, csock, false) };
            if r < 0 {
                return fail(format!("krun_add_vsock_port2: {r}"));
            }
        }

        // Console on our PTY slave (the fds alacritty handed us as 0,1,2).
        let r = unsafe { krun_add_virtio_console_default(ctx, 0, 1, 2) };
        if r < 0 {
            return fail(format!("krun_add_virtio_console_default: {r}"));
        }

        // Exec: program + ARGS-ONLY (libkrun sets argv[0]=program) + guest env.
        let exec = cstr(&cfg.program, &mut keep)?;
        // argv: args only, NULL-terminated.
        let mut argv: Vec<*const libc::c_char> = Vec::new();
        for a in &cfg.program_args {
            argv.push(cstr(a, &mut keep)?);
        }
        argv.push(std::ptr::null());
        // envp: KEY=VAL, NULL-terminated.
        let mut envp: Vec<*const libc::c_char> = Vec::new();
        for (k, v) in &cfg.guest_env {
            envp.push(cstr(&format!("{k}={v}"), &mut keep)?);
        }
        envp.push(std::ptr::null());
        let r = unsafe { krun_set_exec(ctx, exec, argv.as_ptr(), envp.as_ptr()) };
        if r < 0 {
            return fail(format!("krun_set_exec: {r}"));
        }

        // Jail + config OK → tell the supervisor it booted, THEN block forever.
        let _ = write_status_ok(status_w);
        let _ = close_raw(status_w);

        // BLOCKS until the guest exits; then propagates the guest's code.
        let code = unsafe { krun_start_enter(ctx) };
        // Reachable only after the guest exits. <0 = libkrun start error.
        if code < 0 {
            return fail(format!("krun_start_enter: {code}"));
        }
        // Keep CStrings alive across the call.
        drop(keep);
        std::process::exit(code);
    }

    // ── A6 — signals + cold-boot watchdog ───────────────────────────────────
    /// Forward SIGTERM/SIGINT to the cloned child so a daemon teardown tears the
    /// VM down cleanly. `libc::kill` is async-signal-safe.
    extern "C" fn forward_signal(sig: libc::c_int) {
        let pid = CHILD_PID.load(Ordering::SeqCst);
        if pid > 0 {
            unsafe { libc::kill(pid, sig) };
        }
    }

    fn install_signal_forwarding() {
        let handler = signal::SigHandler::Handler(forward_signal);
        let action = signal::SigAction::new(
            handler,
            signal::SaFlags::empty(),
            signal::SigSet::empty(),
        );
        unsafe {
            let _ = signal::sigaction(Signal::SIGTERM, &action);
            let _ = signal::sigaction(Signal::SIGINT, &action);
        }
    }

    /// Cold-boot watchdog: boot is ~100-200ms; if the child hasn't signaled
    /// boot-complete (`BOOTED`) within the window, it is wedged pre-start_enter
    /// → SIGKILL it so the supervisor's waitpid returns and we tear down.
    fn spawn_cold_boot_watchdog(child: Pid) {
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(COLD_BOOT_WATCHDOG_SECS));
            if !BOOTED.load(Ordering::SeqCst) {
                eprintln!(
                    "k2-vmm-worker: cold-boot watchdog fired ({COLD_BOOT_WATCHDOG_SECS}s) — \
                     SIGKILL child"
                );
                let _ = signal::kill(child, Signal::SIGKILL);
            }
        });
    }

    // ── small raw-fd helpers ────────────────────────────────────────────────
    fn pipe2_raw() -> Result<(RawFd, RawFd), WErr> {
        let mut fds = [0 as libc::c_int; 2];
        // No CLOEXEC: the write end must survive into the cloned child.
        if unsafe { libc::pipe2(fds.as_mut_ptr(), 0) } != 0 {
            return fail(format!("pipe2: {}", std::io::Error::last_os_error()));
        }
        Ok((fds[0], fds[1]))
    }

    fn close_raw(fd: RawFd) -> Result<(), WErr> {
        if unsafe { libc::close(fd) } != 0 {
            return fail(format!("close({fd}): {}", std::io::Error::last_os_error()));
        }
        Ok(())
    }

    fn write_status_ok(fd: RawFd) -> Result<(), WErr> {
        let b = [STATUS_OK];
        let n = unsafe { libc::write(fd, b.as_ptr() as *const libc::c_void, 1) };
        if n != 1 {
            return fail("short write on status pipe");
        }
        Ok(())
    }

    fn read_status_byte(fd: RawFd) -> Option<u8> {
        let mut b = [0u8; 1];
        let n = unsafe { libc::read(fd, b.as_mut_ptr() as *mut libc::c_void, 1) };
        if n == 1 {
            Some(b[0])
        } else {
            None // 0 = EOF (child died in setup); <0 = error
        }
    }
}
