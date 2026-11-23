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
//! With `--userns` the job skips the user-namespace rules (clone3, the
//! `CLONE_NEWUSER` flag, `setns`, its own namespace's limit); every other
//! denial stays.

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
/// io_uring, kexec, kernel modules, file handles, mounts). Personality
/// changes are refused by `LockPersonality=yes` (and the shim's rule: only
/// the default `PER_LINUX` or the query `0xffffffff`).
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
    "mount",
    "umount",
    "umount2",
    "pivot_root",
];

/// Refused unless the job has `--userns` (`EPERM`). `clone3` (its flags sit
/// in memory a filter can't read) answers `ENOSYS`, so libc falls back to
/// `clone`, whose `CLONE_NEWUSER` flag is refused; `unshare` likewise.
pub const DENIED_UNLESS_USERNS: &[&str] = &["setns"];

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
    let _ = mode;
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
        assert!(!u[0].contains("setns"), "--userns keeps setns: {u:?}");
        assert!(u[0].contains("bpf:EPERM") && u[0].contains("mount:EPERM"), "the other denials stay: {u:?}");
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
