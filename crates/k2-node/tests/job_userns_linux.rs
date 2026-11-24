//! A job's syscall wall (0.46.18, [`k2_node::jobsec`]): the real
//! `job-exec` shim, run as a unit runs it, refuses user namespaces
//! (clone with `CLONE_NEWUSER`, clone3, unshare, `unshare -U true`) and
//! the denylist, while a normal fork and exec still work; `--userns`
//! (root's `K2_JOB_USERNS=allow`) skips only the user-namespace rules.
//!
//! The probes are `k2-node wall-probe --syscall <probe>`, run by the shim
//! as the job's command, so each one is a real syscall from inside the
//! shim's filter. A team tenant's unit tells the shim `deny`/`deny-limit`
//! like the owner's (never `allow`: root refuses `--userns` for any
//! tenant slot), so the refusals below are a team job's too. A
//! `--no-addr-filter` job's shim also refuses to run in a network
//! namespace with anything but `lo`. Nothing here needs root: the default
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


fn bin() -> PathBuf {
    // A copied box run (scripts/ci/node-userns-root-e2e.sh) names its copy.
    std::env::var_os("K2_NODE_E2E_BIN").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_k2-node")))
}

fn root(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("k2-job-userns-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A spec whose command is `k2-node wall-probe --syscall <probe>`.
fn probe_spec(root: &Path, probe: &str) -> PathBuf {
    let s = ExecSpec {
        argv: vec![bin().display().to_string(), "wall-probe".into(), "--syscall".into(), probe.into()],
        env: vec![("PATH".into(), "/usr/bin:/bin:/usr/sbin:/sbin".into())],
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
    // wall-probe's JSON: {"syscall": {"<probe>": "<outcome>"}, …}, as a PROBE line.
    let line = serde_json::from_str::<serde_json::Value>(stdout.trim())
        .ok()
        .and_then(|v| v["syscall"][probe].as_str().map(|s| format!("PROBE {probe}={s}")))
        .unwrap_or_default();
    let _ = std::fs::remove_dir_all(&d);
    (out.status.code(), line, String::from_utf8_lossy(&out.stderr).to_string())
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
        "personality_change",
    ];
    for mode in [None, Some(USERNS_ALLOW)] {
        for p in denied {
            expect("deny", mode, p, "EPERM");
        }
        if mode.is_none() {
            // Mounts and the new mount API: refused without the grant (with
            // it, a job may mount in its own namespace: the ignored test).
            for p in ["mount", "umount2", "pivot_root", "fsopen_tmpfs", "fsconfig", "fsmount", "fspick", "move_mount", "open_tree", "mount_setattr"] {
                expect("deny-mnt", mode, p, "EPERM");
            }
            expect("deny-mntns", mode, "userns_mount_tmpfs", "unshare:EPERM");
            expect("deny-fsns", mode, "userns_fsopen_tmpfs", "unshare:EPERM");
        }
        // The default personality stays settable (systemd's LockPersonality
        // allows only the current one, not even the 0xffffffff query).
        expect("persd", mode, "personality_default", "ok");
        expect("forkexec", mode, "fork_exec", "ok");
    }
}

/// `--no-addr-filter`'s second wall: in a namespace with more than `lo`
/// (any K2 job's, or a plain box's: they have their own interfaces, or a
/// test node's real ones) the shim runs nothing; in one with `lo` alone it
/// runs the job. The second half needs a fresh network namespace.
#[test]
fn a_job_without_the_address_filter_runs_only_beside_loopback_alone() {
    let ifs = k2_node::jobsec::netns_interfaces().unwrap();
    assert!(ifs.iter().any(|n| n == "lo"), "{ifs:?}");
    let d = root("lo-only");
    let spec = probe_spec(&d, "fork_exec");
    let out = Command::new(bin()).arg("job-exec").arg(&spec).env_clear().env("PATH", "/usr/bin:/bin").env(k2_node::jobsec::ENV_NET, k2_node::jobsec::NET_LO_ONLY).output().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    if ifs.iter().any(|n| n != "lo") {
        // More than loopback here: nothing runs.
        assert_eq!(out.status.code(), Some(i32::from(WALL_EXIT)), "{err}");
        assert!(err.contains("besides loopback"), "{err}");
        assert!(!stdout.contains("syscall"), "nothing ran");
    } else {
        // A loopback-only namespace (a `--network none` job): it runs.
        assert_eq!(out.status.code(), Some(0), "{err}");
        assert!(stdout.contains(r#""fork_exec":"ok""#), "{stdout}");
    }
    let _ = std::fs::remove_dir_all(&d);
    let (code, _, err) = {
        let d = root("net-unknown");
        let spec = probe_spec(&d, "fork_exec");
        let out = Command::new(bin()).arg("job-exec").arg(&spec).env_clear().env("PATH", "/usr/bin:/bin").env(k2_node::jobsec::ENV_NET, "open").output().unwrap();
        (out.status.code(), (), String::from_utf8_lossy(&out.stderr).to_string())
    };
    assert_eq!(code, Some(i32::from(WALL_EXIT)), "an unknown K2_JOB_NET runs nothing: {err}");
}

/// Run the shim under an `unshare -Ur` parent: the job is root in a user
/// namespace of its own, with every capability there, so only the filter
/// stands between it and the mount calls. `(probe, outcome)` per probe.
fn shim_with_caps(tag: &str, mode: &str, probes: &[&str]) -> Vec<(String, String)> {
    let d = root(tag);
    let mut out = Vec::new();
    for p in probes {
        let spec = probe_spec(&d, p);
        let o = Command::new("unshare")
            .args(["-Ur", "sh", "-c", "exec \"$0\" job-exec \"$1\""])
            .arg(bin())
            .arg(&spec)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env(ENV_USERNS, mode)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&o.stdout);
        assert!(o.status.success(), "{p}: {stdout} {}", String::from_utf8_lossy(&o.stderr));
        let v: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| panic!("{p}: {e}: {stdout}"));
        out.push((p.to_string(), v["syscall"][*p].as_str().unwrap_or("missing").to_string()));
    }
    let _ = std::fs::remove_dir_all(&d);
    out
}

/// The fail-first test for the new mount API (Linux 5.2+): a job that
/// holds CAP_SYS_ADMIN in its own namespace still can't fsopen("tmpfs")
/// (or mount any other way) without the grant. Without these in the
/// filter, fsopen/fsmount/move_mount would mount where mount(2) is denied.
#[test]
#[ignore = "needs a process that may make user namespaces with capabilities (not under AppArmor's userns restriction; a Linux box or a node that allows it): run with --ignored"]
fn the_mount_api_is_refused_even_to_a_job_with_capabilities() {
    assert!(may_make_user_namespaces(), "this process can't make user namespaces: run it where it may");
    // The control: root in its own namespace, no shim filter, gets one.
    let control = Command::new("unshare").args(["-Ur", bin().to_str().unwrap(), "wall-probe", "--syscall", "fsopen_tmpfs"]).output().unwrap();
    assert!(String::from_utf8_lossy(&control.stdout).contains(r#""fsopen_tmpfs":"ok""#), "control: {}", String::from_utf8_lossy(&control.stdout));
    for (p, got) in shim_with_caps("mntapi", USERNS_DENY, &["fsopen_tmpfs", "mount", "fsmount", "move_mount", "open_tree", "fspick", "mount_setattr", "pivot_root", "umount2"]) {
        assert_eq!(got, "EPERM", "{p} without the grant, holding CAP_SYS_ADMIN in its own namespace");
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

/// `--userns`: the user-namespace rules are gone (the always-denied list stays: the
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

/// `--userns` lets the owner's trusted jobs build containers in their own
/// namespace: a user + mount namespace of the job's own, tmpfs mounted
/// there with mount(2) and with the new mount API; keyctl and bpf stay
/// refused even so. (Without the grant: the default tests above.)
#[test]
#[ignore = "needs a process that may make user namespaces with capabilities (not under AppArmor's userns restriction; a Linux box or a node that allows it): run with --ignored"]
fn with_userns_a_job_mounts_tmpfs_in_its_own_namespace() {
    assert!(may_make_user_namespaces(), "this process can't make user namespaces: run it where it may");
    expect("a-mnt", Some(USERNS_ALLOW), "userns_mount_tmpfs", "ok");
    expect("a-fsmnt", Some(USERNS_ALLOW), "userns_fsopen_tmpfs", "ok");
    expect("a-keyctl", Some(USERNS_ALLOW), "keyctl", "EPERM");
    expect("a-bpf", Some(USERNS_ALLOW), "bpf", "EPERM");
    // And with CAP_SYS_ADMIN in its own namespace the grant's filter still
    // refuses keyctl and bpf, and lets fsopen through.
    for (p, got) in shim_with_caps("a-caps", USERNS_ALLOW, &["fsopen_tmpfs", "keyctl", "bpf"]) {
        let want = if p == "fsopen_tmpfs" { "ok" } else { "EPERM" };
        assert_eq!(got, want, "{p} with the grant, holding CAP_SYS_ADMIN in its own namespace");
    }
}

/// The limit is the job's own: what root does for a unit (a user
/// namespace of the job's own, its `max_user_namespaces` set to 0 from
/// outside), done here by an `unshare -Ur` parent. Inside: 0, and no
/// user namespace even past the filter's flags; outside, before and
/// after: the machine's value, unchanged. A shim whose limit never comes
/// runs nothing.
#[test]
#[ignore = "needs a plain Linux box (not a K2 job: its /proc/sys is read-only; not under AppArmor's userns restriction) where this process may make user namespaces and set their limit: run with --ignored"]
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
    assert!(stdout.contains(r#""max_user_namespaces":"0""#), "inside the job: {stdout}");
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
    assert!(!String::from_utf8_lossy(&out.stdout).contains("syscall"), "nothing ran");
    let _ = std::fs::remove_dir_all(&d);
}
