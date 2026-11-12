//! The worker's own limits: what it puts on itself as the very first thing in
//! `main`, before it reads a byte of its input.
//!
//! The host starts the worker with `posix_spawn` (no pre-exec hook: forking a
//! large daemon copies its page tables, 36–54 ms at ~1 GB in K2 Render's lab,
//! against ~1 ms), so the limits a hook would set travel on argv instead, as
//! [`Args`], computed by the host from its [`Limits`](crate::host::Limits).
//! Nothing in them comes from the request or the file.
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
//! `main` then sets its allocator's cap (from argv too), on macOS puts itself
//! in the `no-network` sandbox profile (best effort) and watches its parent.
//!
//! The same design as k2-deckrender's spawn change, kept self-contained here
//! so a later commit can share one copy.

use crate::proto::default_ssaa;
use crate::scene::InputLimits;

/// The flag that starts a worker run; its `key=value` arguments follow, in [`KEYS`] order.
pub const FLAG: &str = "--sandbox";

/// `RLIMIT_NOFILE`: the worker opens no file itself (headroom for the
/// platform: macOS's sandbox profile, Linux's `/proc` reads in tests).
pub const NOFILE: u64 = 64;

/// What the host tells the worker to put on itself (the process limits).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelfLimits {
    /// The host's pid: the worker's parent must be this one.
    pub parent: u32,
    /// `RLIMIT_CPU`, soft, in seconds (hard: one more; 0: none).
    pub cpu_secs: u64,
    /// The allocator's cap (0: none).
    pub memory_bytes: u64,
    /// `RLIMIT_AS` on Linux (0: none).
    pub address_space: u64,
}

/// What the worker accepts (the render limits), also from the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Caps {
    /// The widest image.
    pub max_width: u32,
    /// The longest side (the height too) and the most pixels of one image.
    pub max_side: u32,
    pub max_pixels: u64,
    /// Samples per axis, at most (1-4).
    pub max_ssaa: u32,
    /// Render threads, at most.
    pub max_threads: u32,
    /// Views in one request, at most.
    pub max_views: u32,
    /// All the images of a request together, at most.
    pub max_output_bytes: u64,
    pub input: InputLimits,
}

impl Caps {
    /// The SSAA a view gets: the request's, else the default for its width,
    /// never above the cap.
    pub fn ssaa_for(&self, asked: Option<u32>, width: u32) -> u32 {
        asked.unwrap_or_else(|| default_ssaa(width).min(self.max_ssaa.max(1)))
    }
}

/// The worker's whole command line after the binary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Args {
    pub limits: SelfLimits,
    pub caps: Caps,
}

/// The keys after [`FLAG`], in order.
pub const KEYS: [&str; 19] = [
    "parent",
    "cpu",
    "memory",
    "as",
    "width",
    "side",
    "pixels",
    "ssaa",
    "threads",
    "views",
    "output",
    "input",
    "header",
    "meshes",
    "objects",
    "cameras",
    "vertices",
    "triangles",
    "drawn",
];

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

impl Args {
    /// The arguments after the binary.
    pub fn to_args(&self) -> Vec<String> {
        let (l, c, i) = (&self.limits, &self.caps, &self.caps.input);
        // drawn: vertices and triangles share one cap on the wire (K2's limit is
        // "8M triangles or vertices drawn").
        let drawn = i.max_drawn_triangles.max(i.max_drawn_vertices);
        let vals: [u64; 19] = [
            l.parent as u64,
            l.cpu_secs,
            l.memory_bytes,
            l.address_space,
            c.max_width as u64,
            c.max_side as u64,
            c.max_pixels,
            c.max_ssaa as u64,
            c.max_threads as u64,
            c.max_views as u64,
            c.max_output_bytes,
            i.max_input_bytes,
            i.max_header_bytes as u64,
            i.max_meshes as u64,
            i.max_objects as u64,
            i.max_cameras as u64,
            i.max_vertices,
            i.max_triangles,
            drawn,
        ];
        let mut out = vec![FLAG.to_string()];
        for (k, v) in KEYS.iter().zip(vals) {
            out.push(format!("{k}={v}"));
        }
        out
    }

    /// The arguments after the binary, exactly as [`to_args`](Self::to_args) writes them.
    pub fn parse(args: &[String]) -> Result<Args, String> {
        let Some((flag, rest)) = args.split_first() else {
            return Err(format!("the host starts this worker with {FLAG} {}=…", KEYS.join("=… ")));
        };
        if flag != FLAG {
            return Err(format!("unknown flag {flag:?}"));
        }
        if rest.len() != KEYS.len() {
            return Err(format!("{FLAG} takes {} arguments, got {}", KEYS.len(), rest.len()));
        }
        let mut v = [0u64; 19];
        for (i, (arg, key)) in rest.iter().zip(KEYS).enumerate() {
            v[i] = arg
                .strip_prefix(key)
                .and_then(|x| x.strip_prefix('='))
                .and_then(|x| x.parse::<u64>().ok())
                .ok_or_else(|| format!("expected {key}=<number>, got {arg:?}"))?;
        }
        let u32_of = |i: usize| u32::try_from(v[i]).map_err(|_| format!("{} is out of range", KEYS[i]));
        let parent = u32_of(0)?;
        if parent == 0 {
            return Err("parent is 0".into());
        }
        let caps = Caps {
            max_width: u32_of(4)?,
            max_side: u32_of(5)?,
            max_pixels: v[6],
            max_ssaa: u32_of(7)?.clamp(1, 4),
            max_threads: u32_of(8)?.max(1),
            max_views: u32_of(9)?,
            max_output_bytes: v[10],
            input: InputLimits {
                max_input_bytes: v[11],
                max_header_bytes: u32_of(12)?,
                max_meshes: u32_of(13)?,
                max_objects: u32_of(14)?,
                max_cameras: u32_of(15)?,
                max_vertices: v[16],
                max_triangles: v[17],
                max_drawn_vertices: v[18],
                max_drawn_triangles: v[18],
            },
        };
        Ok(Args { limits: SelfLimits { parent, cpu_secs: v[1], memory_bytes: v[2], address_space: v[3] }, caps })
    }
}

impl SelfLimits {
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
        // SAFETY: a zeroed sigset_t filled by sigemptyset; plain calls.
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
/// between): the rlimits (`soft/hard`, `inf` for none), the open fds past
/// stdio, and on Linux `no_new_privs`, `seccomp` (the `Seccomp:` line of
/// `/proc/self/status`: 2 is a filter) and `pdeathsig`.
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
        let open = (3..256).filter(|&fd| {
            // SAFETY: fcntl F_GETFD on any number; EBADF when not open.
            unsafe { libc::fcntl(fd, libc::F_GETFD) != -1 }
        });
        out.push(format!("open_fds={}", open.count()));
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
        // NOFILE is small: this read is one fd, closed at once.
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

    fn args() -> Args {
        Args {
            limits: SelfLimits { parent: 4242, cpu_secs: 25, memory_bytes: 1 << 30, address_space: (1 << 30) + (512 << 20) },
            caps: Caps {
                max_width: 1920,
                max_side: 4096,
                max_pixels: 16 << 20,
                max_ssaa: 2,
                max_threads: 4,
                max_views: 64,
                max_output_bytes: 64 << 20,
                input: InputLimits::default(),
            },
        }
    }

    #[test]
    fn the_arguments_round_trip_and_nothing_else_parses() {
        let a = args();
        assert_eq!(Args::parse(&a.to_args()), Ok(a));
        let good = a.to_args();
        let bad = |v: Vec<String>| Args::parse(&v).is_err();
        assert!(bad(vec![]));
        assert!(bad(vec!["--version".into()]));
        assert!(bad(good[..good.len() - 1].to_vec()), "one short");
        let mut extra = good.clone();
        extra.push("x=1".into());
        assert!(bad(extra), "one more");
        let mut swapped = good.clone();
        swapped.swap(1, 2);
        assert!(bad(swapped), "out of order");
        let mut zero = good.clone();
        zero[1] = "parent=0".into();
        assert!(bad(zero));
        let mut neg = good.clone();
        neg[2] = "cpu=-1".into();
        assert!(bad(neg));
        let mut other = good;
        other[0] = "--other".into();
        assert!(bad(other));
    }

    #[test]
    fn ssaa_defaults_by_width_under_the_cap() {
        let c = args().caps;
        assert_eq!(c.ssaa_for(None, 320), 2, "4 by default, capped at 2");
        assert_eq!(c.ssaa_for(None, 1920), 2);
        assert_eq!(c.ssaa_for(Some(3), 320), 3, "an explicit value is the request's (checked later)");
        let thumb = Caps { max_ssaa: 4, ..c };
        assert_eq!(thumb.ssaa_for(None, 320), 4);
    }
}
