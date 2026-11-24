//! A job's syscall wall (0.46.18): what a Linux job may never call, and
//! user namespaces only when the node's owner allowed them for the job
//! (`k2 compute run --userns`, [`k2_node_proto::userns`]).
//!
//! Two layers, the same rules:
//! - **The unit** (root's, authoritative): `systemd-run` properties
//!   ([`unit_props`]): `RestrictNamespaces=~user` (clone3 → `ENOSYS`,
//!   clone/unshare with `CLONE_NEWUSER` → `EPERM`), `setns` refused, the
//!   [`DENIED`] list refused (`EPERM`), `LockPersonality=yes`, and a user
//!   namespace of the job's own whose `max_user_namespaces` root sets to
//!   0 (that namespace only: the node's own sysctl never changes, since
//!   rootless Podman for Apps and the sandbox need it).
//! - **The `job-exec` shim**: before the job's command runs it installs
//!   the same rules as a seccomp filter of its own ([`install`]), so they
//!   hold even where systemd skips its seccomp properties (a systemd built
//!   without seccomp skips `RestrictNamespaces=` without a word).
//!   Root tells the shim which rules apply through the unit's own env
//!   ([`ENV_USERNS`]); a job's `--env` can never name a `K2_` variable.
//!
//! With `--userns` (the node owner's own server's trusted jobs) the job
//! skips the user-namespace and mount rules ([`DENIED_UNLESS_USERNS`]:
//! clone3, the `CLONE_NEWUSER` flag, `setns`, mount, umount2, pivot_root
//! and the new mount API, and its own namespace's limit), so it can build
//! containers in its own namespace; the [`DENIED`] list stays.

/// The unit's env (set by root with `systemd-run --setenv`) that tells the
/// shim which rules apply: [`USERNS_DENY_LIMIT`], [`USERNS_DENY`] or
/// [`USERNS_ALLOW`]. Unset (a job not started by a 0.46.18 root): the
/// shim denies, without waiting for a namespace limit.
pub const ENV_USERNS: &str = "K2_JOB_USERNS";
/// No user namespaces, and the job runs in one of its own whose
/// `max_user_namespaces` root sets to 0 right after the unit starts: the
/// shim waits (at most [`LIMIT_WAIT`]) until it reads 0.
pub const USERNS_DENY_LIMIT: &str = "deny-limit";
/// No user namespaces (no namespace of the job's own: the node can't make
/// one, so nothing in the job can either).
pub const USERNS_DENY: &str = "deny";
/// `--userns`: the job may make user namespaces.
pub const USERNS_ALLOW: &str = "allow";

/// The unit's env (root's) for a `--no-addr-filter` job:
/// [`NET_LO_ONLY`] = its network namespace must hold no interface but
/// `lo` ([`lo_only_check`]), the second wall when no address filter is.
pub const ENV_NET: &str = "K2_JOB_NET";
/// See [`ENV_NET`].
pub const NET_LO_ONLY: &str = "lo-only";

/// The interfaces of this process's network namespace (`/proc/self/net/dev`
/// is the namespace's own, unlike `/sys/class/net`).
pub fn netns_interfaces() -> Result<Vec<String>, String> {
    let t = std::fs::read_to_string("/proc/self/net/dev").map_err(|e| format!("read /proc/self/net/dev: {e}"))?;
    Ok(parse_net_dev(&t))
}

/// The names in a `/proc/net/dev` text (two header lines, then `name: …`).
pub fn parse_net_dev(t: &str) -> Vec<String> {
    t.lines().skip(2).filter_map(|l| l.split_once(':').map(|(n, _)| n.trim().to_string())).filter(|n| !n.is_empty()).collect()
}

/// `--no-addr-filter`'s second wall: nothing but loopback here.
pub fn lo_only_check() -> Result<(), String> {
    let ifs = netns_interfaces()?;
    let other: Vec<&String> = ifs.iter().filter(|n| n.as_str() != "lo").collect();
    if !other.is_empty() {
        return Err(format!("its network namespace has interfaces besides loopback ({}); a job without the address filter runs only in a namespace with lo alone", other.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")));
    }
    if !ifs.iter().any(|n| n == "lo") {
        return Err("its network namespace has no loopback".into());
    }
    Ok(())
}

/// How long the shim waits for root to set its namespace's limit.
pub const LIMIT_WAIT: std::time::Duration = std::time::Duration::from_secs(20);

/// Where a process reads (and, with `CAP_SYS_RESOURCE` in the namespace,
/// sets) its own user namespace's limit.
pub const MAX_USER_NAMESPACES: &str = "/proc/sys/user/max_user_namespaces";

/// How `job-exec` exits when the syscall wall can't be put up (sysexits'
/// EX_OSERR): nothing was run.
pub const WALL_EXIT: u8 = 71;

/// Every job is refused these (`EPERM`), `--userns` or not: the usual
/// container denylist (eBPF, perf events, userfaultfd, the kernel keyring,
/// io_uring, kexec, kernel modules, file handles). Personality changes are
/// refused by `LockPersonality=yes` (and the shim's rule: only the default
/// `PER_LINUX` or the query `0xffffffff`).
pub const DENIED: &[&str] = &[
    "bpf",
    "perf_event_open",
    "userfaultfd",
    "keyctl",
    "add_key",
    "request_key",
    "io_uring_setup",
    "io_uring_enter",
    "io_uring_register",
    "kexec_load",
    "kexec_file_load",
    "init_module",
    "finit_module",
    "delete_module",
    "open_by_handle_at",
];

/// Refused unless the job has `--userns` (`EPERM`): mounts and joining
/// namespaces. `clone3` (its flags sit in memory a filter can't read)
/// answers `ENOSYS`, so libc falls back to `clone`, whose `CLONE_NEWUSER`
/// flag is refused; `unshare` likewise. With `--userns` the job may mount
/// in a namespace of its own (a container runtime's work); without it,
/// none of these.
pub const DENIED_UNLESS_USERNS: &[&str] = &[
    "mount",
    "umount",
    "umount2",
    "pivot_root",
    // The new mount API (Linux 5.2+; mount_setattr 5.12): without these a
    // mount deny can be gone around.
    "fsopen",
    "fsconfig",
    "fsmount",
    "fspick",
    "move_mount",
    "open_tree",
    "mount_setattr",
    "setns",
];

/// The unit's syscall properties (systemd). `errno_each`: systemd ≥ 247
/// takes `name:EPERM` per entry (refused with `EPERM`); an older one
/// gets the plain list (the process is killed with `SIGSYS`), still a
/// refusal. `userns`: the job's `--userns` (no `setns` rule; the
/// namespace rules are the caller's, with the team's).
pub fn unit_props(userns: bool, errno_each: bool) -> Vec<String> {
    let entry = |n: &str| if errno_each { format!("{n}:EPERM") } else { n.to_string() };
    let mut names: Vec<String> = DENIED.iter().map(|n| entry(*n)).collect();
    if !userns {
        names.extend(DENIED_UNLESS_USERNS.iter().map(|n| entry(*n)));
    }
    vec![format!("SystemCallFilter=~{}", names.join(" "))]
}

/// What the shim's filter does with a syscall.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    Errno(i32),
    /// Refused (`EPERM`) when `CLONE_NEWUSER` is set in the first argument.
    NewUserFlag,
    /// Only `PER_LINUX` (0) and the query (`0xffffffff`); anything else `EPERM`.
    Personality,
}

#[cfg(target_os = "linux")]
pub use filter::{install, program, Arch};

#[cfg(target_os = "linux")]
mod filter {
    use super::Rule;

    const AUDIT_ARCH_X86_64: u32 = 0xc000_003e;
    const AUDIT_ARCH_I386: u32 = 0x4000_0003;
    const AUDIT_ARCH_AARCH64: u32 = 0xc000_00b7;
    /// x32 syscalls carry this bit under `AUDIT_ARCH_X86_64`.
    const X32_SYSCALL_BIT: u32 = 0x4000_0000;
    const CLONE_NEWUSER: u32 = 0x1000_0000;

    const LD_W_ABS: u16 = 0x20; // BPF_LD | BPF_W | BPF_ABS
    const JEQ_K: u16 = 0x15; // BPF_JMP | BPF_JEQ | BPF_K
    const JGE_K: u16 = 0x35; // BPF_JMP | BPF_JGE | BPF_K
    const JSET_K: u16 = 0x45; // BPF_JMP | BPF_JSET | BPF_K
    const RET_K: u16 = 0x06; // BPF_RET | BPF_K

    const RET_ALLOW: u32 = 0x7fff_0000;
    const RET_ERRNO: u32 = 0x0005_0000;
    const RET_KILL_PROCESS: u32 = 0x8000_0000;

    // struct seccomp_data: nr (u32) at 0, arch (u32) at 4, args at 16 (u64
    // each; the low half first on these little-endian machines).
    const OFF_NR: u32 = 0;
    const OFF_ARCH: u32 = 4;
    const OFF_ARG0_LO: u32 = 16;

    /// A syscall ABI the filter knows: its audit arch and its numbers.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Arch {
        X86_64,
        /// i386 programs on an x86_64 kernel (`int 0x80`).
        I386,
        Aarch64,
    }

    impl Arch {
        fn audit(self) -> u32 {
            match self {
                Arch::X86_64 => AUDIT_ARCH_X86_64,
                Arch::I386 => AUDIT_ARCH_I386,
                Arch::Aarch64 => AUDIT_ARCH_AARCH64,
            }
        }

        /// A syscall's number in this ABI, or `None` when it has none.
        /// i386's are from the kernel's `syscall_32.tbl` (a 64-bit build has
        /// no `libc::SYS_*` for them).
        pub fn nr(self, name: &str) -> Option<u32> {
            // Syscalls added since Linux 5.1 have one number on every ABI
            // (asm-generic's table, also x86's and i386's).
            if let Some(n) = unified_nr(name) {
                return Some(n);
            }
            match self {
                Arch::I386 => Some(match name {
                    "mount" => 21,
                    "umount" => 22,
                    "umount2" => 52,
                    "clone" => 120,
                    "init_module" => 128,
                    "delete_module" => 129,
                    "personality" => 136,
                    "pivot_root" => 217,
                    "kexec_load" => 283,
                    "add_key" => 286,
                    "request_key" => 287,
                    "keyctl" => 288,
                    "unshare" => 310,
                    "perf_event_open" => 336,
                    "open_by_handle_at" => 342,
                    "setns" => 346,
                    "finit_module" => 350,
                    "bpf" => 357,
                    "userfaultfd" => 374,
                    "io_uring_setup" => 425,
                    "io_uring_enter" => 426,
                    "io_uring_register" => 427,
                    "clone3" => 435,
                    _ => return None,
                }),
                Arch::X86_64 | Arch::Aarch64 => native_nr(self, name),
            }
        }
    }

    /// The new mount API's numbers: the same on x86_64, i386 and aarch64.
    fn unified_nr(name: &str) -> Option<u32> {
        Some(match name {
            "open_tree" => 428,
            "move_mount" => 429,
            "fsopen" => 430,
            "fsconfig" => 431,
            "fsmount" => 432,
            "fspick" => 433,
            "mount_setattr" => 442,
            _ => return None,
        })
    }

    #[cfg(target_arch = "x86_64")]
    fn native_nr(a: Arch, name: &str) -> Option<u32> {
        if a != Arch::X86_64 {
            return None;
        }
        let n = match name {
            "mount" => libc::SYS_mount,
            "umount2" => libc::SYS_umount2,
            "clone" => libc::SYS_clone,
            "init_module" => libc::SYS_init_module,
            "delete_module" => libc::SYS_delete_module,
            "personality" => libc::SYS_personality,
            "pivot_root" => libc::SYS_pivot_root,
            "kexec_load" => libc::SYS_kexec_load,
            "kexec_file_load" => libc::SYS_kexec_file_load,
            "add_key" => libc::SYS_add_key,
            "request_key" => libc::SYS_request_key,
            "keyctl" => libc::SYS_keyctl,
            "unshare" => libc::SYS_unshare,
            "perf_event_open" => libc::SYS_perf_event_open,
            "open_by_handle_at" => libc::SYS_open_by_handle_at,
            "setns" => libc::SYS_setns,
            "finit_module" => libc::SYS_finit_module,
            "bpf" => libc::SYS_bpf,
            "userfaultfd" => libc::SYS_userfaultfd,
            "io_uring_setup" => libc::SYS_io_uring_setup,
            "io_uring_enter" => libc::SYS_io_uring_enter,
            "io_uring_register" => libc::SYS_io_uring_register,
            "clone3" => libc::SYS_clone3,
            _ => return None,
        };
        Some(n as u32)
    }

    #[cfg(target_arch = "aarch64")]
    fn native_nr(a: Arch, name: &str) -> Option<u32> {
        if a != Arch::Aarch64 {
            return None;
        }
        // aarch64 has no `umount` (only `umount2`).
        let n = match name {
            "mount" => libc::SYS_mount,
            "umount2" => libc::SYS_umount2,
            "clone" => libc::SYS_clone,
            "init_module" => libc::SYS_init_module,
            "delete_module" => libc::SYS_delete_module,
            "personality" => libc::SYS_personality,
            "pivot_root" => libc::SYS_pivot_root,
            "kexec_load" => libc::SYS_kexec_load,
            "kexec_file_load" => libc::SYS_kexec_file_load,
            "add_key" => libc::SYS_add_key,
            "request_key" => libc::SYS_request_key,
            "keyctl" => libc::SYS_keyctl,
            "unshare" => libc::SYS_unshare,
            "perf_event_open" => libc::SYS_perf_event_open,
            "open_by_handle_at" => libc::SYS_open_by_handle_at,
            "setns" => libc::SYS_setns,
            "finit_module" => libc::SYS_finit_module,
            "bpf" => libc::SYS_bpf,
            "userfaultfd" => libc::SYS_userfaultfd,
            "io_uring_setup" => libc::SYS_io_uring_setup,
            "io_uring_enter" => libc::SYS_io_uring_enter,
            "io_uring_register" => libc::SYS_io_uring_register,
            "clone3" => libc::SYS_clone3,
            _ => return None,
        };
        Some(n as u32)
    }

    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    fn native_nr(_a: Arch, _name: &str) -> Option<u32> {
        None
    }

    /// The ABIs a process of this build can call the kernel with: the
    /// native one, and i386 on x86_64. Any other ABI is killed.
    fn abis() -> Result<Vec<Arch>, String> {
        if cfg!(target_arch = "x86_64") {
            Ok(vec![Arch::X86_64, Arch::I386])
        } else if cfg!(target_arch = "aarch64") {
            Ok(vec![Arch::Aarch64])
        } else {
            Err("this CPU architecture has no job syscall filter yet".into())
        }
    }

    /// The rules for one ABI, by syscall number, in the filter's order.
    pub fn rules(a: Arch, userns: bool) -> Vec<(String, u32, Rule)> {
        let mut out = Vec::new();
        let mut push = |name: &str, rule: Rule| {
            if let Some(n) = a.nr(name) {
                out.push((name.to_string(), n, rule));
            }
        };
        for n in super::DENIED {
            push(*n, Rule::Errno(libc::EPERM));
        }
        if !userns {
            push("clone3", Rule::Errno(libc::ENOSYS));
            for n in super::DENIED_UNLESS_USERNS {
                push(*n, Rule::Errno(libc::EPERM));
            }
            push("clone", Rule::NewUserFlag);
            push("unshare", Rule::NewUserFlag);
        }
        push("personality", Rule::Personality);
        out
    }

    /// One classic-BPF instruction with symbolic jump targets.
    #[derive(Clone, Copy)]
    enum Ins {
        Ld(u32),
        /// if A == k goto t else goto f (label ids; `None` = next).
        Jeq(u32, Option<usize>, Option<usize>),
        Jge(u32, Option<usize>, Option<usize>),
        Jset(u32, Option<usize>, Option<usize>),
        Ret(u32),
        Label(usize),
    }

    const L_ALLOW: usize = 0;
    const L_EPERM: usize = 1;
    const L_ENOSYS: usize = 2;
    const L_KILL: usize = 3;
    const L_NEWUSER: usize = 4;
    const L_PERS: usize = 5;
    const L_ABI0: usize = 10;

    /// The filter, as `sock_filter` words (code, jt, jf, k).
    pub fn program(userns: bool) -> Result<Vec<(u16, u8, u8, u32)>, String> {
        let abis = abis()?;
        let mut p: Vec<Ins> = vec![Ins::Ld(OFF_ARCH)];
        for (i, a) in abis.iter().enumerate() {
            p.push(Ins::Jeq(a.audit(), Some(L_ABI0 + i), None));
        }
        p.push(Ins::Ret(RET_KILL_PROCESS));
        for (i, a) in abis.iter().enumerate() {
            p.push(Ins::Label(L_ABI0 + i));
            p.push(Ins::Ld(OFF_NR));
            if *a == Arch::X86_64 {
                // x32 (the same audit arch, numbers with bit 30): none.
                p.push(Ins::Jge(X32_SYSCALL_BIT, Some(L_ENOSYS), None));
            }
            for (_, nr, rule) in rules(*a, userns) {
                let target = match rule {
                    Rule::Errno(e) if e == libc::EPERM => L_EPERM,
                    Rule::Errno(e) if e == libc::ENOSYS => L_ENOSYS,
                    Rule::Errno(e) => return Err(format!("no return for errno {e}")),
                    Rule::NewUserFlag => L_NEWUSER,
                    Rule::Personality => L_PERS,
                };
                p.push(Ins::Jeq(nr, Some(target), None));
            }
            p.push(Ins::Ret(RET_ALLOW));
        }
        // The argument checks: clone's and unshare's flags are the first
        // argument on every ABI here (also i386's `CLONE_BACKWARDS`).
        p.push(Ins::Label(L_NEWUSER));
        p.push(Ins::Ld(OFF_ARG0_LO));
        p.push(Ins::Jset(CLONE_NEWUSER, Some(L_EPERM), Some(L_ALLOW)));
        p.push(Ins::Label(L_PERS));
        p.push(Ins::Ld(OFF_ARG0_LO));
        p.push(Ins::Jeq(0, Some(L_ALLOW), None));
        p.push(Ins::Jeq(0xffff_ffff, Some(L_ALLOW), Some(L_EPERM)));
        p.push(Ins::Label(L_ALLOW));
        p.push(Ins::Ret(RET_ALLOW));
        p.push(Ins::Label(L_EPERM));
        p.push(Ins::Ret(RET_ERRNO | libc::EPERM as u32));
        p.push(Ins::Label(L_ENOSYS));
        p.push(Ins::Ret(RET_ERRNO | libc::ENOSYS as u32));
        p.push(Ins::Label(L_KILL));
        p.push(Ins::Ret(RET_KILL_PROCESS));
        assemble(&p)
    }

    fn assemble(p: &[Ins]) -> Result<Vec<(u16, u8, u8, u32)>, String> {
        // Instruction index of each label.
        let mut at = std::collections::HashMap::new();
        let mut n = 0usize;
        for i in p {
            match i {
                Ins::Label(l) => {
                    at.insert(*l, n);
                }
                _ => n += 1,
            }
        }
        let mut out = Vec::with_capacity(n);
        let mut pc = 0usize;
        let off = |pc: usize, t: Option<usize>| -> Result<u8, String> {
            match t {
                None => Ok(0),
                Some(l) => {
                    let dst = *at.get(&l).ok_or_else(|| format!("no label {l}"))?;
                    let d = dst.checked_sub(pc + 1).ok_or_else(|| format!("label {l} jumps backwards"))?;
                    u8::try_from(d).map_err(|_| format!("label {l} is {d} instructions away"))
                }
            }
        };
        for i in p {
            let w = match *i {
                Ins::Label(_) => continue,
                Ins::Ld(k) => (LD_W_ABS, 0, 0, k),
                Ins::Jeq(k, t, f) => (JEQ_K, off(pc, t)?, off(pc, f)?, k),
                Ins::Jge(k, t, f) => (JGE_K, off(pc, t)?, off(pc, f)?, k),
                Ins::Jset(k, t, f) => (JSET_K, off(pc, t)?, off(pc, f)?, k),
                Ins::Ret(k) => (RET_K, 0, 0, k),
            };
            out.push(w);
            pc += 1;
        }
        Ok(out)
    }

    /// Install the filter on this process (every thread) and everything it
    /// starts from now on. `NoNewPrivileges` goes on first (the unit has it
    /// already). An `Err` means nothing may run.
    pub fn install(userns: bool) -> Result<(), String> {
        let words = program(userns)?;
        let mut f: Vec<libc::sock_filter> = words.iter().map(|&(code, jt, jf, k)| libc::sock_filter { code, jt, jf, k }).collect();
        let prog = libc::sock_fprog { len: u16::try_from(f.len()).map_err(|_| "the filter is too long".to_string())?, filter: f.as_mut_ptr() };
        // SAFETY: plain prctl with integer arguments.
        if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
            return Err(format!("no_new_privs: {}", std::io::Error::last_os_error()));
        }
        // SAFETY: `prog` points at `f`, alive for the call; the kernel copies it.
        let r = unsafe {
            libc::syscall(
                libc::SYS_seccomp,
                libc::SECCOMP_SET_MODE_FILTER,
                libc::SECCOMP_FILTER_FLAG_TSYNC,
                &prog as *const libc::sock_fprog,
            )
        };
        if r != 0 {
            return Err(format!("seccomp: {}", std::io::Error::last_os_error()));
        }
        Ok(())
    }
}

/// What root asked the shim for ([`ENV_USERNS`] in its own env).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShimMode {
    DenyLimit,
    Deny,
    Allow,
}

/// [`ENV_USERNS`]'s value: unset = [`ShimMode::Deny`]; anything unknown is
/// an error (nothing runs).
pub fn shim_mode(v: Option<&str>) -> Result<ShimMode, String> {
    match v {
        None | Some(USERNS_DENY) => Ok(ShimMode::Deny),
        Some(USERNS_DENY_LIMIT) => Ok(ShimMode::DenyLimit),
        Some(USERNS_ALLOW) => Ok(ShimMode::Allow),
        Some(x) => Err(format!("{ENV_USERNS}={x:?} isn't a mode this k2-node knows")),
    }
}

/// The shim's half, run inside the job's unit before its command (Linux):
/// with [`ShimMode::DenyLimit`] wait until this namespace's
/// `max_user_namespaces` reads 0 (root sets it), then install the filter.
/// An `Err` is the whole message for the job's stderr; nothing may run.
pub fn shim_wall() -> Result<(), String> {
    if !cfg!(target_os = "linux") {
        return Ok(());
    }
    let mode = shim_mode(std::env::var(ENV_USERNS).ok().as_deref()).map_err(|e| no_wall(&e))?;
    if mode == ShimMode::DenyLimit {
        wait_for_limit(LIMIT_WAIT).map_err(|e| no_wall(&e))?;
    }
    match std::env::var(ENV_NET).ok().as_deref() {
        None => {}
        Some(NET_LO_ONLY) => lo_only_check().map_err(|e| no_wall(&e))?,
        Some(x) => return Err(no_wall(&format!("{ENV_NET}={x:?} isn't a mode this k2-node knows"))),
    }
    #[cfg(target_os = "linux")]
    install(mode == ShimMode::Allow).map_err(|e| no_wall(&format!("its syscall filter: {e}")))?;
    Ok(())
}

/// Wait until this process's user namespace allows no user namespaces.
pub fn wait_for_limit(timeout: std::time::Duration) -> Result<(), String> {
    let t0 = std::time::Instant::now();
    loop {
        let v = std::fs::read_to_string(MAX_USER_NAMESPACES).map_err(|e| format!("read {MAX_USER_NAMESPACES}: {e}"))?;
        if v.trim() == "0" {
            return Ok(());
        }
        if t0.elapsed() >= timeout {
            return Err(format!(
                "this job's own user namespace still allows {} user namespaces after {}s (root sets it to 0 as the job starts)",
                v.trim(),
                timeout.as_secs()
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

fn no_wall(why: &str) -> String {
    format!("k2-node job-exec: this job's sandbox isn't complete, so nothing was run: {why}. This is the node's job sandbox, not the job: run it on another node, and tell the node's owner.")
}

/// Test probes (`k2-node wall-probe --syscall NAME`): one syscall from this
/// process, answered `ok` or the errno's name. Each is harmless: a clone
/// that succeeds exits at once; arguments are null or invalid.
#[cfg(target_os = "linux")]
pub use probes::syscall_probe;

/// Off Linux there's nothing to probe.
#[cfg(not(target_os = "linux"))]
pub fn syscall_probe(_name: &str) -> String {
    "not-linux".into()
}

#[cfg(target_os = "linux")]
mod probes {
    const NEWUSER: u64 = 0x1000_0000;

    /// What a probe's syscall said: `ok` or the errno's name.
    fn said(r: libc::c_long) -> String {
        if r >= 0 {
            return "ok".into();
        }
        match std::io::Error::last_os_error().raw_os_error() {
            Some(libc::EPERM) => "EPERM".into(),
            Some(libc::ENOSYS) => "ENOSYS".into(),
            Some(libc::EINVAL) => "EINVAL".into(),
            Some(libc::EBADF) => "EBADF".into(),
            Some(libc::EFAULT) => "EFAULT".into(),
            Some(libc::ENOSPC) => "ENOSPC".into(),
            Some(e) => format!("errno{e}"),
            None => "?".into(),
        }
    }

    /// A fork-like clone (no new stack): the child exits at once; the parent
    /// reaps it.
    fn reap_if_child(r: libc::c_long) {
        if r == 0 {
            // SAFETY: the child of a fork-like clone leaves at once.
            unsafe { libc::_exit(0) };
        }
        if r > 0 {
            let mut st = 0;
            // SAFETY: waiting for our own child.
            unsafe { libc::waitpid(r as libc::pid_t, &mut st, 0) };
        }
    }

    /// The errnos a namespace probe tells apart (code = index + 1; 0 = none
    /// of them).
    const ERRNO_CODES: [(i32, &str); 6] = [
        (libc::EPERM, "EPERM"),
        (libc::EACCES, "EACCES"),
        (libc::EINVAL, "EINVAL"),
        (libc::ENOSPC, "ENOSPC"),
        (libc::ENOSYS, "ENOSYS"),
        (libc::ENODEV, "ENODEV"),
    ];

    /// Fork; the child makes a user + mount namespace, maps itself to root
    /// there, and mounts tmpfs (mount(2), or fsopen/fsmount). Its exit code
    /// is the errno of the first step that failed (0 = mounted).
    fn userns_child(new_api: bool) -> String {
        // SAFETY: fork, then only syscalls and _exit in the child.
        unsafe {
            let uid = libc::getuid();
            let gid = libc::getgid();
            let dir = std::env::temp_dir().join(format!("k2-probe-mnt-{}", std::process::id()));
            let _ = std::fs::create_dir_all(&dir);
            let c_dir = std::ffi::CString::new(dir.display().to_string()).unwrap_or_default();
            let maps = [
                (c"/proc/self/setgroups", "deny".to_string()),
                (c"/proc/self/uid_map", format!("0 {uid} 1")),
                (c"/proc/self/gid_map", format!("0 {gid} 1")),
            ];
            let pid = libc::fork();
            if pid == 0 {
                /// Exit `step * 8 + errno's code` ([`ERRNO_CODES`]).
                fn fail(step: i32) -> ! {
                    let e = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
                    let k = ERRNO_CODES.iter().position(|(n, _)| *n == e).map_or(ERRNO_CODES.len() + 1, |i| i + 1) as i32;
                    // SAFETY: leaving the forked child.
                    unsafe { libc::_exit(step * 8 + k) }
                }
                if libc::syscall(libc::SYS_unshare, (libc::CLONE_NEWUSER | libc::CLONE_NEWNS) as u64) != 0 {
                    fail(1);
                }
                for (path, text) in &maps {
                    let f = libc::open(path.as_ptr(), libc::O_WRONLY);
                    if f < 0 || libc::write(f, text.as_ptr().cast(), text.len()) != text.len() as isize {
                        fail(2);
                    }
                    libc::close(f);
                }
                if new_api {
                    let fs = libc::syscall(430, c"tmpfs".as_ptr(), 0u64);
                    if fs < 0 {
                        fail(3);
                    }
                    // FSCONFIG_CMD_CREATE = 6
                    if libc::syscall(431, fs, 6u64, 0u64, 0u64, 0u64) != 0 {
                        fail(4);
                    }
                    if libc::syscall(432, fs, 0u64, 0u64) < 0 {
                        fail(5);
                    }
                } else if libc::mount(c"tmpfs".as_ptr(), c_dir.as_ptr(), c"tmpfs".as_ptr(), 0, std::ptr::null()) != 0 {
                    fail(3);
                }
                libc::_exit(0);
            }
            let mut st = 0;
            libc::waitpid(pid, &mut st, 0);
            let _ = std::fs::remove_dir(&dir);
            match libc::WEXITSTATUS(st) {
                0 => "ok".into(),
                c => {
                    let (step, k) = ((c / 8) as usize, (c % 8) as usize);
                    let step = ["?", "unshare", "map", "mount", "fsconfig", "fsmount"].get(step).copied().unwrap_or("?");
                    let e = ERRNO_CODES.get(k.wrapping_sub(1)).map_or("other-errno", |(_, n)| *n);
                    format!("{step}:{e}")
                }
            }
        }
    }

    pub fn syscall_probe(name: &str) -> String {
        // SAFETY (all arms): raw syscalls with harmless arguments; every
        // pointer is null or to a local that outlives the call.
        unsafe {
            match name {
                "clone_newuser" => {
                    let r = libc::syscall(libc::SYS_clone, NEWUSER | libc::SIGCHLD as u64, 0u64, 0u64, 0u64, 0u64);
                    let s = said(r);
                    reap_if_child(r);
                    s
                }
                "clone3_newuser" | "clone3_plain" => {
                    // struct clone_args (v0, 64 bytes): flags, pidfd, child_tid,
                    // parent_tid, exit_signal, stack, stack_size, tls.
                    let mut a = [0u64; 8];
                    a[0] = if name == "clone3_newuser" { NEWUSER } else { 0 };
                    a[4] = libc::SIGCHLD as u64;
                    let r = libc::syscall(libc::SYS_clone3, a.as_mut_ptr(), std::mem::size_of_val(&a));
                    let s = said(r);
                    reap_if_child(r);
                    s
                }
                // In a child: a multithreaded caller (the test harness) gets
                // EINVAL from unshare(NEWUSER) whatever the filter says.
                "unshare_newuser" => {
                    let pid = libc::fork();
                    if pid == 0 {
                        let r = libc::syscall(libc::SYS_unshare, NEWUSER);
                        let e = if r == 0 { 0 } else { std::io::Error::last_os_error().raw_os_error().unwrap_or(255) };
                        libc::_exit(e & 0xff);
                    }
                    let mut st = 0;
                    libc::waitpid(pid, &mut st, 0);
                    match libc::WEXITSTATUS(st) {
                        0 => "ok".into(),
                        e if e == libc::EPERM => "EPERM".into(),
                        e if e == libc::EINVAL => "EINVAL".into(),
                        e if e == libc::ENOSPC => "ENOSPC".into(),
                        e => format!("errno{e}"),
                    }
                }
                "unshare_cmd" => match std::process::Command::new("unshare").args(["-U", "true"]).status() {
                    Ok(s) if s.success() => "ok".into(),
                    Ok(s) => format!("exit{}", s.code().unwrap_or(-1)),
                    Err(e) => format!("no-unshare:{e}"),
                },
                // EBADF without a filter: the filter answers before the fd is looked at.
                "setns" => said(libc::syscall(libc::SYS_setns, -1i64, 0i64)),
                "fork_exec" => match std::process::Command::new("/bin/sh").args(["-c", "echo child-ran"]).output() {
                    Ok(o) if o.status.success() && String::from_utf8_lossy(&o.stdout).contains("child-ran") => "ok".into(),
                    Ok(o) => format!("exit{}", o.status.code().unwrap_or(-1)),
                    Err(e) => format!("spawn:{e}"),
                },
                "fork" => {
                    let r = libc::fork() as libc::c_long;
                    let s = said(r);
                    reap_if_child(r);
                    s
                }
                // keyctl(KEYCTL_GET_KEYRING_ID, KEY_SPEC_SESSION_KEYRING, 0): any user may.
                "keyctl" => said(libc::syscall(libc::SYS_keyctl, 0i64, -3i64, 0i64)),
                "add_key" => said(libc::syscall(libc::SYS_add_key, 0u64, 0u64, 0u64, 0u64, -3i64)),
                "request_key" => said(libc::syscall(libc::SYS_request_key, 0u64, 0u64, 0u64, 0i64)),
                "io_uring_setup" => said(libc::syscall(libc::SYS_io_uring_setup, 0u64, 0u64)),
                "io_uring_enter" => said(libc::syscall(libc::SYS_io_uring_enter, -1i64, 0u64, 0u64, 0u64, 0u64, 0u64)),
                "io_uring_register" => said(libc::syscall(libc::SYS_io_uring_register, -1i64, 0u64, 0u64, 0u64)),
                "bpf" => said(libc::syscall(libc::SYS_bpf, 0i64, 0u64, 0u64)),
                "perf_event_open" => said(libc::syscall(libc::SYS_perf_event_open, 0u64, 0i64, -1i64, -1i64, 0u64)),
                // UFFD_USER_MODE_ONLY: any user may (Linux 5.11+).
                "userfaultfd" => said(libc::syscall(libc::SYS_userfaultfd, 1u64)),
                "kexec_load" => said(libc::syscall(libc::SYS_kexec_load, 0u64, 0u64, 0u64, 0u64)),
                "kexec_file_load" => said(libc::syscall(libc::SYS_kexec_file_load, -1i64, -1i64, 0u64, 0u64, 0u64)),
                "init_module" => said(libc::syscall(libc::SYS_init_module, 0u64, 0u64, 0u64)),
                "finit_module" => said(libc::syscall(libc::SYS_finit_module, -1i64, 0u64, 0u64)),
                "delete_module" => said(libc::syscall(libc::SYS_delete_module, 0u64, 0u64)),
                "open_by_handle_at" => said(libc::syscall(libc::SYS_open_by_handle_at, -1i64, 0u64, 0u64)),
                "mount" => said(libc::syscall(libc::SYS_mount, 0u64, 0u64, 0u64, 0u64, 0u64)),
                "umount2" => said(libc::syscall(libc::SYS_umount2, 0u64, 0u64)),
                "pivot_root" => said(libc::syscall(libc::SYS_pivot_root, 0u64, 0u64)),
                // The new mount API (one number on every ABI). fsopen of a
                // real filesystem: what a process with CAP_SYS_ADMIN in its
                // namespace gets a context for; the rest with bad fds.
                "fsopen_tmpfs" => {
                    let fs = c"tmpfs";
                    let r = libc::syscall(430, fs.as_ptr(), 0u64);
                    let s = said(r);
                    if r >= 0 {
                        libc::close(r as libc::c_int);
                    }
                    s
                }
                "fsconfig" => said(libc::syscall(431, -1i64, 0u64, 0u64, 0u64, 0u64)),
                "fsmount" => said(libc::syscall(432, -1i64, 0u64, 0u64)),
                "fspick" => said(libc::syscall(433, -1i64, 0u64, 0u64)),
                "move_mount" => said(libc::syscall(429, -1i64, 0u64, -1i64, 0u64, 0u64)),
                "open_tree" => said(libc::syscall(428, -1i64, 0u64, 0u64)),
                "mount_setattr" => said(libc::syscall(442, -1i64, 0u64, 0u64, 0u64, 0u64)),
                // In a child: a user and mount namespace of its own (as
                // root there), then tmpfs over a fresh folder, with mount(2)
                // or with the new API. What a container runtime does.
                "userns_mount_tmpfs" | "userns_fsopen_tmpfs" => userns_child(name == "userns_fsopen_tmpfs"),
                // ADDR_NO_RANDOMIZE: a change away from the default.
                "personality_change" => said(libc::syscall(libc::SYS_personality, 0x0040000u64)),
                "personality_default" => said(libc::syscall(libc::SYS_personality, 0u64)),
                "max_user_namespaces" => std::fs::read_to_string(super::MAX_USER_NAMESPACES).map(|s| s.trim().to_string()).unwrap_or_else(|e| format!("read:{e}")),
                other => format!("unknown-probe:{other}"),
            }
        }
    }

}

/// The denylist as a reviewer reads it: one line per syscall, what the
/// unit and the shim do with it, with and without `--userns`.
pub fn denylist_table() -> String {
    let mut s = String::from("syscall              default          --userns\n");
    let mut row = |n: &str, d: &str, u: &str| s.push_str(&format!("{n:<20} {d:<16} {u}\n"));
    for n in DENIED {
        row(n, "EPERM", "EPERM");
    }
    row("clone3", "ENOSYS", "allowed");
    row("clone", "EPERM if NEWUSER", "allowed");
    row("unshare", "EPERM if NEWUSER", "allowed");
    for n in DENIED_UNLESS_USERNS {
        row(n, "EPERM", "allowed");
    }
    row("personality", "default only", "default only");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_filter_lists_every_denial_and_setns_only_without_userns() {
        let d = unit_props(false, true);
        assert_eq!(d.len(), 1);
        for n in DENIED.iter().chain(DENIED_UNLESS_USERNS) {
            assert!(d[0].split(['~', ' ']).any(|w| w == format!("{n}:EPERM")), "{n} missing: {d:?}");
        }
        let u = unit_props(true, true);
        let words: Vec<&str> = u[0].trim_start_matches("SystemCallFilter=~").split(' ').collect();
        for n in DENIED_UNLESS_USERNS {
            assert!(!words.contains(&format!("{n}:EPERM").as_str()), "--userns keeps {n}: {u:?}");
        }
        for n in DENIED {
            assert!(words.contains(&format!("{n}:EPERM").as_str()), "{n} stays denied with --userns: {u:?}");
        }
        let old = unit_props(false, false);
        assert!(old[0].starts_with("SystemCallFilter=~bpf perf_event_open"), "{old:?}");
        assert!(!old[0].contains(':'), "{old:?}");
    }

    #[test]
    fn shim_mode_reads_roots_env_and_refuses_unknown() {
        assert_eq!(shim_mode(None), Ok(ShimMode::Deny));
        assert_eq!(shim_mode(Some("deny")), Ok(ShimMode::Deny));
        assert_eq!(shim_mode(Some("deny-limit")), Ok(ShimMode::DenyLimit));
        assert_eq!(shim_mode(Some("allow")), Ok(ShimMode::Allow));
        assert!(shim_mode(Some("yes")).is_err());
    }

    #[test]
    fn net_dev_names_are_read_from_the_namespace_file() {
        let t = "Inter-|   Receive\n face |bytes\n    lo: 1 2 3\n  eth0: 4 5 6\nveth1@if2: 0\n";
        assert_eq!(parse_net_dev(t), ["lo", "eth0", "veth1@if2"]);
        assert_eq!(parse_net_dev("h\nh\n    lo: 0 0\n"), ["lo"]);
    }

    #[test]
    fn table_names_every_rule() {
        let t = denylist_table();
        for n in DENIED.iter().chain(DENIED_UNLESS_USERNS).chain(&["clone3", "clone", "unshare", "personality"]) {
            assert!(t.lines().any(|l| l.split_whitespace().next() == Some(*n)), "{n}: {t}");
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn filter_assembles_with_every_rule_for_each_abi() {
        for userns in [false, true] {
            let p = program(userns).unwrap();
            assert!(p.len() < 200, "{}", p.len());
            // Last instruction returns; the first loads the arch.
            assert_eq!(p[0], (0x20, 0, 0, 4));
        }
        let native = if cfg!(target_arch = "x86_64") { Arch::X86_64 } else { Arch::Aarch64 };
        let deny = filter_rules(native, false);
        for n in DENIED {
            if *n == "umount" && native != Arch::I386 {
                continue; // x86_64 and aarch64 have only umount2
            }
            assert!(deny.iter().any(|(m, _, r)| m == n && *r == Rule::Errno(libc::EPERM)), "{n}");
        }
        assert!(deny.iter().any(|(m, _, r)| m == "clone3" && *r == Rule::Errno(libc::ENOSYS)));
        assert!(deny.iter().any(|(m, _, r)| m == "clone" && *r == Rule::NewUserFlag));
        assert!(deny.iter().any(|(m, _, r)| m == "unshare" && *r == Rule::NewUserFlag));
        assert!(deny.iter().any(|(m, _, r)| m == "setns" && *r == Rule::Errno(libc::EPERM)));
        let allow = filter_rules(native, true);
        assert!(!allow.iter().any(|(m, _, _)| ["clone3", "clone", "unshare", "setns"].contains(&m.as_str())), "{allow:?}");
        assert!(allow.iter().any(|(m, _, _)| m == "bpf"));
        if cfg!(target_arch = "x86_64") {
            let i386 = filter_rules(Arch::I386, false);
            assert!(i386.iter().any(|(m, nr, _)| m == "unshare" && *nr == 310));
            assert!(i386.iter().any(|(m, nr, _)| m == "umount" && *nr == 22));
        }
    }

    #[cfg(target_os = "linux")]
    fn filter_rules(a: Arch, userns: bool) -> Vec<(String, u32, Rule)> {
        super::filter::rules(a, userns)
    }
}
