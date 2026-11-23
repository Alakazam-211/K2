//! The `job-exec` shim's loopback check (0.46.17) against real network
//! namespaces. On nodes running k2-node 0.46.14, a connect to 127.0.0.1
//! inside a job never completed (a test that starts a local server and
//! connects to it hung). A 0.46.14 `network = none` unit had `IPAddressDeny=any`
//! and no allow for localhost; systemd's address filter covers `lo` and
//! drops what it denies, so the handshake's packets vanished and the
//! connect hung. The shim now checks loopback before the job's command and
//! fails the job at once, saying why.
//!
//! Each test runs the real binary in a namespace of its own:
//! - `unshare -Urn` (an unprivileged user + network namespace): `lo` down,
//!   `lo` up, and `lo` up with an nftables rule that drops every packet
//!   leaving through it (the same effect as the unit's address filter);
//! - as root, the exact 0.46.14 unit (`systemd-run`, `PrivateNetwork=yes`,
//!   `IPAddressDeny=any`) beside the current one (`IPAddressAllow=localhost`).
//!
//! They need user namespaces (Ubuntu 24.04's AppArmor refuses them to a
//! job user; Arch allows them), iproute2 and nft, or root and systemd, so
//! every test is `#[ignore]` and runs on purpose on a Linux box:
//!
//!   cargo test -p k2-node --test job_loopback_linux -- --ignored
//!
//! Nothing on the machine is changed: the namespaces and the transient
//! unit go with their processes.
#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use k2_node::wall::{read_rc, write_exec_spec, ExecSpec, EXEC_RC, LOOPBACK_EXIT};
use k2_node_proto::frames::ExitInfo;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_k2-node"))
}

fn root(tag: &str, under: &Path) -> PathBuf {
    let d = under.join(format!("k2-job-loopback-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A spec whose command writes `ran` to a file: the proof it ran.
fn spec(root: &Path) -> PathBuf {
    let s = ExecSpec {
        argv: vec!["/bin/sh".into(), "-c".into(), format!("echo ran > {}", root.join("ran").display())],
        env: vec![("PATH".into(), "/usr/bin:/bin".into())],
        cwd: root.to_path_buf(),
        stdin_env: false,
        rc_file: Some(root.join(EXEC_RC)),
    };
    write_exec_spec(root, &s).unwrap()
}

/// `unshare -Urn sh -c '<setup>; exec k2-node job-exec <spec>'`.
fn in_own_netns(setup: &str, spec: &Path) -> (Output, Duration) {
    let script = format!("{setup}exec \"$0\" job-exec \"$1\"");
    let t0 = Instant::now();
    let out = Command::new("unshare").args(["-Urn", "sh", "-c", &script]).arg(bin()).arg(spec).output().expect("unshare (util-linux)");
    (out, t0.elapsed())
}

fn ip() -> &'static str {
    ["/usr/sbin/ip", "/sbin/ip", "/usr/bin/ip", "/bin/ip"].into_iter().find(|p| Path::new(p).exists()).expect("ip (iproute2)")
}

fn nft() -> &'static str {
    ["/usr/sbin/nft", "/sbin/nft", "/usr/bin/nft", "/bin/nft"].into_iter().find(|p| Path::new(p).exists()).expect("nft (nftables)")
}

/// The control: user namespaces work here and loopback up runs the job.
#[test]
#[ignore = "user namespaces + iproute2: run on a Linux box with --ignored"]
fn with_loopback_up_the_job_runs() {
    let d = root("up", &std::env::temp_dir());
    let (out, _) = in_own_netns(&format!("{} link set lo up && ", ip()), &spec(&d));
    assert!(out.status.success(), "{out:?}");
    assert_eq!(std::fs::read_to_string(d.join("ran")).unwrap(), "ran\n");
    assert_eq!(read_rc(&d.join(EXEC_RC)), Some(ExitInfo { code: Some(0), signal: None }));
}

/// `lo` down (a namespace nobody set up): the job fails at once, says
/// loopback isn't up, and its command never runs.
#[test]
#[ignore = "user namespaces: run on a Linux box with --ignored"]
fn with_loopback_down_the_job_fails_at_once_and_says_why() {
    let d = root("down", &std::env::temp_dir());
    let (out, took) = in_own_netns("", &spec(&d));
    assert_eq!(out.status.code(), Some(i32::from(LOOPBACK_EXIT)), "{out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("this job's loopback doesn't work, so nothing was run") && err.contains("loopback (lo) isn't up"), "{err}");
    assert!(took < Duration::from_secs(2), "no wait for a down lo: {took:?}");
    assert!(!d.join("ran").exists(), "the job's command never ran");
    assert_eq!(read_rc(&d.join(EXEC_RC)), Some(ExitInfo { code: Some(i32::from(LOOPBACK_EXIT)), signal: None }));
}

/// `lo` up but its packets dropped (the reported hang): the job fails
/// within the check's timeout, saying the packets are dropped; its
/// command never runs.
#[test]
#[ignore = "user namespaces + iproute2 + nft: run on a Linux box with --ignored"]
fn with_loopback_packets_dropped_the_job_fails_in_bounded_time() {
    let d = root("drop", &std::env::temp_dir());
    let setup = format!("{} link set lo up && {nft} add table inet k2t && {nft} add chain inet k2t out '{{ type filter hook output priority 0; policy drop; }}' && ", ip(), nft = nft());
    let (out, took) = in_own_netns(&setup, &spec(&d));
    assert_eq!(out.status.code(), Some(i32::from(LOOPBACK_EXIT)), "{out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("got no answer") && err.contains("loopback packets are dropped") && err.contains("127.0.0.1:"), "{err}");
    assert!(took >= k2_node::wall::LOOPBACK_CHECK_TIMEOUT && took < k2_node::wall::LOOPBACK_CHECK_TIMEOUT * 3, "bounded, not a hang: {took:?}");
    assert!(!d.join("ran").exists(), "the job's command never ran");
    assert_eq!(read_rc(&d.join(EXEC_RC)), Some(ExitInfo { code: Some(i32::from(LOOPBACK_EXIT)), signal: None }));
}

/// The exact unit shapes, as root: 0.46.14's `none` unit (a deny, no
/// localhost allow) fails the job with the loopback error; the current one (with
/// `IPAddressAllow=localhost`) runs it. The shim runs as root here, in a
/// dir under /var/lib (a unit's /tmp is its own).
#[test]
#[ignore = "root + systemd: run on a Linux box with --ignored"]
fn the_0_46_14_none_unit_fails_loudly_and_todays_runs() {
    assert_eq!(unsafe { libc::geteuid() }, 0, "run as root");
    let base = root("unit", Path::new("/var/lib"));
    let unit_run = |tag: &str, allow: bool| -> (Output, PathBuf) {
        let d = root(tag, &base);
        let p = spec(&d);
        let unit = format!("k2-loopback-test-{tag}-{}.service", std::process::id());
        let mut a: Vec<String> = vec![format!("--unit={unit}"), "--quiet".into(), "--wait".into(), "--collect".into(), "--pipe".into()];
        let mut props = vec!["PrivateNetwork=yes".to_string(), "IPAddressDeny=any".into(), "RuntimeMaxSec=60".into()];
        if allow {
            props.push(format!("IPAddressAllow={}", k2_node::admin::jobs::LOOPBACK_ALLOW));
        }
        for x in props {
            a.push("-p".into());
            a.push(x);
        }
        a.extend(["--".into(), bin().display().to_string(), "job-exec".into(), p.display().to_string()]);
        let out = Command::new("systemd-run").args(&a).output().expect("systemd-run");
        (out, d)
    };
    let (old, d_old) = unit_run("old", false);
    assert_eq!(old.status.code(), Some(i32::from(LOOPBACK_EXIT)), "{old:?}");
    let err = String::from_utf8_lossy(&old.stderr);
    assert!(err.contains("loopback packets are dropped"), "{err}");
    assert!(!d_old.join("ran").exists());
    let (new, d_new) = unit_run("new", true);
    assert!(new.status.success(), "{new:?}");
    assert_eq!(std::fs::read_to_string(d_new.join("ran")).unwrap(), "ran\n");
    let _ = std::fs::remove_dir_all(&base);
}
