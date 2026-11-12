//! The worker's own limits: what it puts on itself as the very first thing in
//! `main`, before it reads a byte of its input.
//!
//! The host starts the worker with `posix_spawn` (no pre-exec hook: forking a
//! large daemon copies its page tables, 36–54 ms at ~1 GB, against ~1 ms), so
//! the limits a hook used to set travel on argv instead, as [`SelfLimits`],
//! computed by the host from its [`Limits`](crate::host::Limits). Nothing in
//! them comes from the request or the file.
//!
//! [`SelfLimits::apply`] goes in this order and stops at the first step that
//! fails (the worker then answers a typed `renderer_error` without reading
//! its input):
//!
//! 1. the parent: `PR_SET_PDEATHSIG` = SIGKILL (Linux), then `getppid()` must
//!    be the host's pid from argv (a host that died before step 1 would leave
//!    the worker re-parented, with no signal to come);
//! 2. signals: an empty mask and `SIGXCPU` at its default action (spawn keeps
//!    the host's mask and ignored signals, and `RLIMIT_CPU` speaks through
//!    `SIGXCPU`);
//! 3. inherited descriptors past stdio closed (spawn leaves open any the host
//!    didn't mark close-on-exec);
//! 4. rlimits: `RLIMIT_CORE` 0, `RLIMIT_FSIZE` 0, `RLIMIT_NOFILE` [`NOFILE`],
//!    `RLIMIT_CPU` (SIGXCPU at the soft limit, SIGKILL a second later),
//!    `RLIMIT_AS` (Linux; macOS doesn't enforce it), each capped at the hard
//!    limit the worker already has;
//! 5. `PR_SET_NO_NEW_PRIVS` (Linux);
//! 6. the seccomp no-network filter (Linux): `socket(2)` and
//!    `io_uring_setup(2)` fail with `EACCES`, any syscall of another ABI with
//!    `EPERM`.
//!
//! `main` then sets its allocator's cap (from argv too) and, on macOS, the
//! `no-network` sandbox profile (best effort, as before).

/// The flag that starts a worker run; its four `key=value` arguments follow.
pub const FLAG: &str = "--sandbox";

/// `RLIMIT_NOFILE` (system font files are opened one at a time).
pub const NOFILE: u64 = 64;

/// What the host tells the worker to put on itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelfLimits {
    /// The host's pid: the worker's parent must be this one.
    pub parent: u32,
    /// `RLIMIT_CPU`, soft, in seconds (hard: one more).
    pub cpu_secs: u64,
    /// The allocator's cap (0: none).
    pub memory_bytes: u64,
    /// `RLIMIT_AS` on Linux (0: none).
    pub address_space: u64,
}

/// Why the worker couldn't limit itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepFailed {
    pub step: &'static str,
    pub detail: String,
}

impl std::fmt::Display for StepFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the worker could not limit itself ({}): {}", self.step, self.detail)
    }
}

impl SelfLimits {
    /// The arguments after the binary.
    pub fn to_args(&self) -> Vec<String> {
        vec![
            FLAG.to_string(),
            format!("parent={}", self.parent),
            format!("cpu={}", self.cpu_secs),
            format!("memory={}", self.memory_bytes),
            format!("as={}", self.address_space),
        ]
    }

    /// The arguments after the binary, exactly as [`to_args`](Self::to_args) writes them.
    pub fn parse(args: &[String]) -> Result<SelfLimits, String> {
        let [flag, parent, cpu, memory, address_space] = args else {
            return Err(format!("the host starts this worker with {FLAG} parent=… cpu=… memory=… as=…"));
        };
        if flag != FLAG {
            return Err(format!("unknown flag {flag:?}"));
        }
        fn num(arg: &str, key: &str) -> Result<u64, String> {
            arg.strip_prefix(key)
                .and_then(|v| v.strip_prefix('='))
                .and_then(|v| v.parse::<u64>().ok())
                .ok_or_else(|| format!("expected {key}=<number>, got {arg:?}"))
        }
        let parent = u32::try_from(num(parent, "parent")?).map_err(|_| "parent is out of range".to_string())?;
        if parent == 0 {
            return Err("parent is 0".into());
        }
        Ok(SelfLimits {
            parent,
            cpu_secs: num(cpu, "cpu")?,
            memory_bytes: num(memory, "memory")?,
            address_space: num(address_space, "as")?,
        })
    }

    /// Put the limits on this process (steps 1–6 above), in order. Call it
    /// once, first thing in `main`, single-threaded.
    pub fn apply(&self) -> Result<(), StepFailed> {
        #[cfg(unix)]
        {
            imp::parent(self.parent)?;
            imp::signals()?;
            imp::close_inherited();
            imp::rlimits(self.cpu_secs, self.address_space)?;
            #[cfg(target_os = "linux")]
            {
                imp::no_new_privs()?;
                seccomp::install(&seccomp::no_network_filter())?;
            }
        }
        Ok(())
    }
}

#[cfg(unix)]
mod imp {
    use super::StepFailed;

    fn os(step: &'static str) -> StepFailed {
        StepFailed { step, detail: std::io::Error::last_os_error().to_string() }
    }

    pub fn parent(expected: u32) -> Result<(), StepFailed> {
        #[cfg(target_os = "linux")]
        // SAFETY: prctl with integer arguments.
        if unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0) } != 0 {
            return Err(os("PR_SET_PDEATHSIG"));
        }
        // SAFETY: getppid has no preconditions.
        let now = unsafe { libc::getppid() };
        if u32::try_from(now).ok() != Some(expected) {
            return Err(StepFailed { step: "parent", detail: format!("the parent is {now}, not the host {expected}") });
        }
        Ok(())
    }

    pub fn signals() -> Result<(), StepFailed> {
        // SAFETY: a zeroed sigset_t filled by sigemptyset; plain syscalls.
        unsafe {
            let mut none: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut none);
            if libc::pthread_sigmask(libc::SIG_SETMASK, &none, std::ptr::null_mut()) != 0 {
                return Err(StepFailed { step: "signal mask", detail: "pthread_sigmask failed".into() });
            }
            if libc::signal(libc::SIGXCPU, libc::SIG_DFL) == libc::SIG_ERR {
                return Err(os("SIGXCPU"));
            }
        }
        Ok(())
    }

    /// Best effort: an fd that can't be closed was not open.
    pub fn close_inherited() {
        #[cfg(target_os = "linux")]
        // SAFETY: close_range (5.9+) on fds this process doesn't use yet.
        if unsafe { libc::syscall(libc::SYS_close_range, 3 as libc::c_uint, libc::c_uint::MAX, 0 as libc::c_uint) } == 0 {
            return;
        }
        for fd in 3..4096 {
            // SAFETY: as above; EBADF for fds that aren't open.
            unsafe { libc::close(fd) };
        }
    }

    fn set(res: libc::c_int, name: &'static str, soft: u64, hard: u64) -> Result<(), StepFailed> {
        // SAFETY: getrlimit/setrlimit on plain structs.
        unsafe {
            let mut now = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
            if libc::getrlimit(res as _, &mut now) != 0 {
                return Err(os(name));
            }
            // Never above the hard limit the worker already has (only root may raise it).
            let max = (hard as libc::rlim_t).min(now.rlim_max);
            let cur = (soft as libc::rlim_t).min(max);
            if libc::setrlimit(res as _, &libc::rlimit { rlim_cur: cur, rlim_max: max }) != 0 {
                return Err(os(name));
            }
        }
        Ok(())
    }

    pub fn rlimits(cpu_secs: u64, address_space: u64) -> Result<(), StepFailed> {
        set(libc::RLIMIT_CORE as _, "RLIMIT_CORE", 0, 0)?;
        // It writes no files (pipes aren't affected).
        set(libc::RLIMIT_FSIZE as _, "RLIMIT_FSIZE", 0, 0)?;
        set(libc::RLIMIT_NOFILE as _, "RLIMIT_NOFILE", super::NOFILE, super::NOFILE)?;
        if cpu_secs > 0 {
            set(libc::RLIMIT_CPU as _, "RLIMIT_CPU", cpu_secs, cpu_secs.saturating_add(1))?;
        }
        #[cfg(target_os = "linux")]
        if address_space > 0 {
            set(libc::RLIMIT_AS as _, "RLIMIT_AS", address_space, address_space)?;
        }
        #[cfg(not(target_os = "linux"))]
        let _ = address_space;
        Ok(())
    }

    #[cfg(target_os = "linux")]
    pub fn no_new_privs() -> Result<(), StepFailed> {
        // SAFETY: prctl with integer arguments.
        if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
            return Err(os("PR_SET_NO_NEW_PRIVS"));
        }
        Ok(())
    }
}

/// TEST ONLY: the limits in force right now, as `key=value` pairs (`;`
/// between): the rlimits (`soft/hard`, `inf` for none), what `socket(2)`
/// does (`open` or `errno<n>`), and on Linux
/// `no_new_privs`, `seccomp` (the `Seccomp:` line of `/proc/self/status`: 2 is
/// a filter) and `pdeathsig`.
#[cfg(feature = "test-hostile")]
#[doc(hidden)]
pub fn observe() -> String {
    let mut out: Vec<String> = Vec::new();
    #[cfg(unix)]
    {
        let rl = |res: libc::c_int| {
            let mut r = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
            // SAFETY: getrlimit fills a plain struct.
            if unsafe { libc::getrlimit(res as _, &mut r) } != 0 {
                return "err".to_string();
            }
            let v = |x: libc::rlim_t| if x == libc::RLIM_INFINITY { "inf".to_string() } else { x.to_string() };
            format!("{}/{}", v(r.rlim_cur), v(r.rlim_max))
        };
        out.push(format!("core={}", rl(libc::RLIMIT_CORE as _)));
        out.push(format!("fsize={}", rl(libc::RLIMIT_FSIZE as _)));
        out.push(format!("nofile={}", rl(libc::RLIMIT_NOFILE as _)));
        out.push(format!("cpu={}", rl(libc::RLIMIT_CPU as _)));
        out.push(format!("as={}", rl(libc::RLIMIT_AS as _)));
        // SAFETY: socket(2), closed if it opened.
        let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0) };
        if fd >= 0 {
            // SAFETY: our own fd.
            unsafe { libc::close(fd) };
            out.push("socket=open".into());
        } else {
            out.push(format!("socket=errno{}", std::io::Error::last_os_error().raw_os_error().unwrap_or(0)));
        }
    }
    #[cfg(target_os = "linux")]
    {
        // SAFETY: prctl with integer arguments / a pointer to a c_int it fills.
        let nnp = unsafe { libc::prctl(libc::PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) };
        out.push(format!("no_new_privs={nnp}"));
        let mut sig: libc::c_int = -1;
        // SAFETY: as above.
        unsafe { libc::prctl(libc::PR_GET_PDEATHSIG, &mut sig as *mut libc::c_int as libc::c_ulong, 0, 0, 0) };
        out.push(format!("pdeathsig={sig}"));
        let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
        let mode = status.lines().find_map(|l| l.strip_prefix("Seccomp:")).map(str::trim).unwrap_or("?");
        out.push(format!("seccomp={mode}"));
    }
    out.join(";")
}

/// Linux: no network for the worker. A classic-BPF seccomp filter makes
/// `socket(2)` and `io_uring_setup(2)` fail with `EACCES` (no socket can be
/// made, so nothing can connect, bind or send), and any syscall of another ABI
/// (`int 0x80`, x32) fail with `EPERM`. Installed after `PR_SET_NO_NEW_PRIVS`,
/// so no privilege is needed.
#[cfg(target_os = "linux")]
mod seccomp {
    use super::StepFailed;

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct SockFilter {
        code: u16,
        jt: u8,
        jf: u8,
        k: u32,
    }

    #[repr(C)]
    struct SockFprog {
        len: u16,
        filter: *const SockFilter,
    }

    const LD_W_ABS: u16 = 0x20; // BPF_LD | BPF_W | BPF_ABS
    const JEQ_K: u16 = 0x15; // BPF_JMP | BPF_JEQ | BPF_K
    #[cfg(target_arch = "x86_64")]
    const JGE_K: u16 = 0x35; // BPF_JMP | BPF_JGE | BPF_K
    const RET_K: u16 = 0x06; // BPF_RET | BPF_K
    const RET_ALLOW: u32 = 0x7fff_0000;
    const RET_ERRNO: u32 = 0x0005_0000;
    const OFF_NR: u32 = 0;
    const OFF_ARCH: u32 = 4;
    const SYS_IO_URING_SETUP: u32 = 425;
    const PR_SET_SECCOMP: libc::c_int = 22;
    const SECCOMP_MODE_FILTER: libc::c_ulong = 2;
    #[cfg(target_arch = "x86_64")]
    const ARCH: u32 = 0xC000_003E; // AUDIT_ARCH_X86_64
    #[cfg(target_arch = "aarch64")]
    const ARCH: u32 = 0xC000_00B7; // AUDIT_ARCH_AARCH64

    const fn op(code: u16, jt: u8, jf: u8, k: u32) -> SockFilter {
        SockFilter { code, jt, jf, k }
    }

    /// The program.
    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    pub fn no_network_filter() -> Vec<SockFilter> {
        let deny_abi = RET_ERRNO | libc::EPERM as u32;
        let deny = RET_ERRNO | libc::EACCES as u32;
        let socket = libc::SYS_socket as u32;
        let mut p = vec![op(LD_W_ABS, 0, 0, OFF_ARCH)];
        // Another ABI: to the last instruction (EPERM), patched below.
        p.push(op(JEQ_K, 0, 0, ARCH));
        p.push(op(LD_W_ABS, 0, 0, OFF_NR));
        #[cfg(target_arch = "x86_64")]
        p.push(op(JGE_K, 0, 0, 0x4000_0000)); // x32: to EPERM, patched below
        p.push(op(JEQ_K, 0, 0, socket)); // to EACCES, patched below
        p.push(op(JEQ_K, 0, 0, SYS_IO_URING_SETUP)); // to EACCES, patched below
        p.push(op(RET_K, 0, 0, RET_ALLOW));
        let eacces = p.len();
        p.push(op(RET_K, 0, 0, deny));
        let eperm = p.len();
        p.push(op(RET_K, 0, 0, deny_abi));
        // Jumps are relative to the next instruction.
        let rel = |from: usize, to: usize| (to - from - 1) as u8;
        p[1].jf = rel(1, eperm);
        // The syscall checks follow the second load: [x32 check,] socket, io_uring_setup.
        let socket_at = eacces - 3;
        #[cfg(target_arch = "x86_64")]
        {
            p[3].jt = rel(3, eperm);
        }
        p[socket_at].jt = rel(socket_at, eacces);
        p[socket_at + 1].jt = rel(socket_at + 1, eacces);
        p
    }

    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    pub fn no_network_filter() -> Vec<SockFilter> {
        Vec::new()
    }

    /// Install the filter (after NO_NEW_PRIVS). An empty filter (another
    /// architecture) installs nothing.
    pub fn install(filter: &[SockFilter]) -> Result<(), StepFailed> {
        if filter.is_empty() {
            return Ok(());
        }
        let prog = SockFprog { len: filter.len() as u16, filter: filter.as_ptr() };
        // SAFETY: prog points at `filter`, alive for the call; the kernel copies it.
        if unsafe { libc::prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &prog as *const SockFprog as libc::c_ulong, 0, 0) } != 0 {
            return Err(StepFailed { step: "seccomp", detail: std::io::Error::last_os_error().to_string() });
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn every_jump_lands_inside_the_program() {
            let p = super::no_network_filter();
            assert!(!p.is_empty());
            for (i, ins) in p.iter().enumerate() {
                if ins.code == super::JEQ_K || ins.code == 0x35 {
                    assert!(i + 1 + (ins.jt as usize) < p.len(), "jt of {i}");
                    assert!(i + 1 + (ins.jf as usize) < p.len(), "jf of {i}");
                }
            }
            assert_eq!(p.last().map(|i| i.code), Some(super::RET_K));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_arguments_round_trip_and_nothing_else_parses() {
        let l = SelfLimits { parent: 4242, cpu_secs: 25, memory_bytes: 1 << 30, address_space: (1 << 30) + (512 << 20) };
        assert_eq!(SelfLimits::parse(&l.to_args()), Ok(l));
        let bad = |v: &[&str]| SelfLimits::parse(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>()).is_err();
        assert!(bad(&[]));
        assert!(bad(&["--version"]));
        assert!(bad(&[FLAG, "parent=1", "cpu=1", "memory=1"]));
        assert!(bad(&[FLAG, "parent=0", "cpu=1", "memory=1", "as=1"]));
        assert!(bad(&[FLAG, "cpu=1", "parent=1", "memory=1", "as=1"]));
        assert!(bad(&[FLAG, "parent=1", "cpu=-1", "memory=1", "as=1"]));
        assert!(bad(&[FLAG, "parent=1", "cpu=1", "memory=1", "as=1", "extra"]));
        assert!(bad(&["--other", "parent=1", "cpu=1", "memory=1", "as=1"]));
    }
}
