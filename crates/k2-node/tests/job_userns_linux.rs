//! A job's syscall wall (0.46.18, [`k2_node::jobsec`]): the real
//! `job-exec` shim, run as a unit runs it, refuses user namespaces
//! (clone with `CLONE_NEWUSER`, clone3, unshare, `unshare -U true`) and
//! the denylist, while a normal fork and exec still work; `--userns`
//! (root's `K2_JOB_USERNS=allow`) skips only the user-namespace rules.
//!
//! The probes are this test binary itself, run by the shim as the job's
//! command (`K2_USERNS_PROBE=<probe>`), so each one is a real syscall
//! from inside the shim's filter. Nothing here needs root: the default
//! tests pass whether or not the machine running them lets this process
//! make user namespaces (a job on a 0.46.18 node can't). The `#[ignore]`d
//! ones need a process that may (a `k2 compute run --userns` job, or a
//! Linux box): they prove the allowed path and that the namespace limit
//! is the job's own, never the machine's.
#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::process::Command;

use k2_node::jobsec::{ENV_USERNS, MAX_USER_NAMESPACES, USERNS_ALLOW, USERNS_DENY, USERNS_DENY_LIMIT, WALL_EXIT};
use k2_node::wall::{write_exec_spec, ExecSpec};

const PROBE_ENV: &str = "K2_USERNS_PROBE";
const CLONE_NEWUSER: u64 = 0x1000_0000;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_k2-node"))
}

fn root(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("k2-job-userns-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A spec whose command is this binary's probe `probe`.
fn probe_spec(root: &Path, probe: &str) -> PathBuf {
    let me = std::env::current_exe().unwrap();
    let s = ExecSpec {
        argv: vec![me.display().to_string(), "--exact".into(), "probe".into(), "--ignored".into(), "--nocapture".into(), "--test-threads=1".into()],
        env: vec![("PATH".into(), "/usr/bin:/bin:/usr/sbin:/sbin".into()), (PROBE_ENV.into(), probe.into())],
        cwd: root.to_path_buf(),
        stdin_env: false,
        rc_file: None,
    };
    write_exec_spec(root, &s).unwrap()
}

/// Run the shim (with root's `K2_JOB_USERNS` = `mode`, `None` = unset) on
/// `probe`: its exit code and the probe's `PROBE` line.
fn shim(tag: &str, mode: Option<&str>, probe: &str) -> (Option<i32>, String, String) {
    let d = root(tag);
    let spec = probe_spec(&d, probe);
    let mut c = Command::new(bin());
    c.arg("job-exec").arg(&spec).env_clear().env("PATH", "/usr/bin:/bin");
    if let Some(m) = mode {
        c.env(ENV_USERNS, m);
    }
    let out = c.output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let line = stdout.lines().find(|l| l.starts_with("PROBE ")).unwrap_or("").to_string();
    let _ = std::fs::remove_dir_all(&d);
    (out.status.code(), line, String::from_utf8_lossy(&out.stderr).to_string())
}

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

fn run_probe(name: &str) -> String {
    // SAFETY (all arms): raw syscalls with harmless arguments; every
    // pointer is null or to a local that outlives the call.
    unsafe {
        match name {
            "clone_newuser" => {
                let r = libc::syscall(libc::SYS_clone, CLONE_NEWUSER | libc::SIGCHLD as u64, 0u64, 0u64, 0u64, 0u64);
                let s = said(r);
                reap_if_child(r);
                s
            }
            "clone3_newuser" | "clone3_plain" => {
                // struct clone_args (v0, 64 bytes): flags, pidfd, child_tid,
                // parent_tid, exit_signal, stack, stack_size, tls.
                let mut a = [0u64; 8];
                a[0] = if name == "clone3_newuser" { CLONE_NEWUSER } else { 0 };
                a[4] = libc::SIGCHLD as u64;
                let r = libc::syscall(libc::SYS_clone3, a.as_mut_ptr(), std::mem::size_of_val(&a));
                let s = said(r);
                reap_if_child(r);
                s
            }
            // In a child: a multithreaded caller (the test harness) gets
            // EINVAL from unshare(CLONE_NEWUSER) whatever the filter says.
            "unshare_newuser" => {
                let pid = libc::fork();
                if pid == 0 {
                    let r = libc::syscall(libc::SYS_unshare, CLONE_NEWUSER);
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
            "unshare_cmd" => match Command::new("unshare").args(["-U", "true"]).status() {
                Ok(s) if s.success() => "ok".into(),
                Ok(s) => format!("exit{}", s.code().unwrap_or(-1)),
                Err(e) => format!("no-unshare:{e}"),
            },
            // EBADF without a filter: the filter answers before the fd is looked at.
            "setns" => said(libc::syscall(libc::SYS_setns, -1i64, 0i64)),
            "fork_exec" => match Command::new("/bin/sh").args(["-c", "echo child-ran"]).output() {
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
            // ADDR_NO_RANDOMIZE: a change away from the default.
            "personality_change" => said(libc::syscall(libc::SYS_personality, 0x0040000u64)),
            "personality_query" => said(libc::syscall(libc::SYS_personality, 0xffff_ffffu64)),
            "max_user_namespaces" => std::fs::read_to_string(MAX_USER_NAMESPACES).map(|s| s.trim().to_string()).unwrap_or_else(|e| format!("read:{e}")),
            other => format!("unknown-probe:{other}"),
        }
    }
}

/// The probe entry the shim runs (never on its own: it does nothing
/// without `K2_USERNS_PROBE`).
#[test]
#[ignore = "the shim's probe: run by the other tests, inside job-exec"]
fn probe() {
    let Ok(name) = std::env::var(PROBE_ENV) else { return };
    println!("PROBE {name}={}", run_probe(&name));
}

fn expect(tag: &str, mode: Option<&str>, probe: &str, want: &str) {
    let (code, line, err) = shim(tag, mode, probe);
    assert_eq!(code, Some(0), "{probe} under {mode:?}: the shim or the probe failed: {line} {err}");
    assert_eq!(line, format!("PROBE {probe}={want}"), "{probe} under {mode:?}: stderr {err}");
}

/// The fail-first test: no user namespace from inside a job, however it's
/// asked for; a normal fork and exec still work.
#[test]
fn a_job_cant_make_a_user_namespace() {
    for mode in [None, Some(USERNS_DENY)] {
        expect("clone", mode, "clone_newuser", "EPERM");
        expect("clone3", mode, "clone3_newuser", "ENOSYS");
        // clone3 can't be inspected, so it's gone whole: libc falls back to clone.
        expect("clone3p", mode, "clone3_plain", "ENOSYS");
        expect("unshare", mode, "unshare_newuser", "EPERM");
        expect("unsharecmd", mode, "unshare_cmd", "exit1");
        expect("setns", mode, "setns", "EPERM");
        expect("forkexec", mode, "fork_exec", "ok");
        expect("fork", mode, "fork", "ok");
    }
}

/// The usual container denylist, `--userns` or not.
#[test]
fn a_job_is_refused_the_denylist_with_or_without_userns() {
    let denied = [
        "keyctl",
        "add_key",
        "request_key",
        "io_uring_setup",
        "io_uring_enter",
        "io_uring_register",
        "bpf",
        "perf_event_open",
        "userfaultfd",
        "kexec_load",
        "kexec_file_load",
        "init_module",
        "finit_module",
        "delete_module",
        "open_by_handle_at",
        "mount",
        "umount2",
        "pivot_root",
        "personality_change",
    ];
    for mode in [None, Some(USERNS_ALLOW)] {
        for p in denied {
            expect("deny", mode, p, "EPERM");
        }
        expect("persq", mode, "personality_query", "ok");
        expect("forkexec", mode, "fork_exec", "ok");
    }
}

/// An unknown mode from root runs nothing.
#[test]
fn an_unknown_mode_runs_nothing() {
    let (code, line, err) = shim("unknown", Some("maybe"), "fork_exec");
    assert_eq!(code, Some(i32::from(WALL_EXIT)), "{line} {err}");
    assert!(line.is_empty(), "nothing ran: {line}");
    assert!(err.contains("K2_JOB_USERNS"), "{err}");
}

/// Whether this process may make a user namespace (a job on a 0.46.18 node
/// without `--userns` may not).
fn may_make_user_namespaces() -> bool {
    Command::new("unshare").args(["-U", "true"]).status().map(|s| s.success()).unwrap_or(false)
}

/// `--userns`: the user-namespace rules are gone (the denylist stays: the
/// test above).
#[test]
#[ignore = "needs a process that may make user namespaces (k2 compute run --userns, or a Linux box): run with --ignored"]
fn with_userns_a_job_may_make_a_user_namespace() {
    assert!(may_make_user_namespaces(), "this process can't make user namespaces: run it where it may");
    expect("a-clone", Some(USERNS_ALLOW), "clone_newuser", "ok");
    expect("a-clone3", Some(USERNS_ALLOW), "clone3_newuser", "ok");
    expect("a-unshare", Some(USERNS_ALLOW), "unshare_newuser", "ok");
    expect("a-unsharecmd", Some(USERNS_ALLOW), "unshare_cmd", "ok");
    expect("a-setns", Some(USERNS_ALLOW), "setns", "EBADF");
}

/// The limit is the job's own: what root does for a unit (a user
/// namespace of the job's own, its `max_user_namespaces` set to 0 from
/// outside), done here by an `unshare -Ur` parent. Inside: 0, and no
/// user namespace even past the filter's flags; outside, before and
/// after: the machine's value, unchanged. A shim whose limit never comes
/// runs nothing.
#[test]
#[ignore = "needs a process that may make user namespaces and set their limit (not under AppArmor's userns restriction): run with --ignored"]
fn the_namespace_limit_is_the_jobs_own_never_the_machines() {
    assert!(may_make_user_namespaces(), "this process can't make user namespaces: run it where it may");
    let machine = std::fs::read_to_string(MAX_USER_NAMESPACES).unwrap().trim().to_string();
    assert_ne!(machine, "0", "the machine itself allows user namespaces");
    let d = root("limit");
    let spec = probe_spec(&d, "max_user_namespaces");
    let out = Command::new("unshare")
        .args(["-Ur", "sh", "-c", &format!("echo 0 > {MAX_USER_NAMESPACES} && exec \"$0\" job-exec \"$1\"")])
        .arg(bin())
        .arg(&spec)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env(ENV_USERNS, USERNS_DENY_LIMIT)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout} {}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.lines().any(|l| l == "PROBE max_user_namespaces=0"), "inside the job: {stdout}");
    let after = std::fs::read_to_string(MAX_USER_NAMESPACES).unwrap().trim().to_string();
    assert_eq!(after, machine, "the machine's limit never changes");
    // A shim in a namespace whose limit nobody sets waits, then runs nothing.
    let spec = probe_spec(&d, "fork_exec");
    let out = Command::new("unshare")
        .args(["-Ur", "sh", "-c", "exec \"$0\" job-exec \"$1\""])
        .arg(bin())
        .arg(&spec)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env(ENV_USERNS, USERNS_DENY_LIMIT)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(i32::from(WALL_EXIT)), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(!String::from_utf8_lossy(&out.stdout).contains("PROBE"), "nothing ran");
    let _ = std::fs::remove_dir_all(&d);
}
