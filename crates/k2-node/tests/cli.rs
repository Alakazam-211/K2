//! The `k2-node` binary and the installer scripts. Temp dirs only; no real
//! dscl/useradd/systemctl/launchctl ever runs here.

use std::path::{Path, PathBuf};
use std::process::Command;

use k2_node_proto::frames::Control;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_k2-node")
}

fn run(args: &[&str]) -> (bool, String, String) {
    let o = Command::new(bin()).args(args).env_clear().env("PATH", "/usr/bin:/bin").output().unwrap();
    (o.status.success(), String::from_utf8_lossy(&o.stdout).into_owned(), String::from_utf8_lossy(&o.stderr).into_owned())
}

fn scripts() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/node");
    let mut v: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "sh"))
        .collect();
    v.sort();
    v
}

#[test]
fn every_installer_script_parses_and_is_executable() {
    use std::os::unix::fs::PermissionsExt;
    let all = scripts();
    let names: Vec<String> = all.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
    for want in ["install-node-linux.sh", "install-node-macos.sh", "uninstall-node-linux.sh", "uninstall-node-macos.sh", "install-rust-for-node.sh"] {
        assert!(names.iter().any(|n| n == want), "missing scripts/node/{want}: {names:?}");
    }
    for p in all {
        let o = Command::new("bash").arg("-n").arg(&p).output().unwrap();
        assert!(o.status.success(), "bash -n {}: {}", p.display(), String::from_utf8_lossy(&o.stderr));
        let mode = std::fs::metadata(&p).unwrap().permissions().mode();
        assert!(mode & 0o111 != 0, "{} is not executable", p.display());
    }
}

/// The S0 spike scripts (run by hand on the minis / z13flow) parse, are
/// executable, and refuse to start without their required arguments.
#[test]
fn spike_scripts_parse_and_refuse_without_arguments() {
    use std::os::unix::fs::PermissionsExt;
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/node/spikes");
    let mut names = Vec::new();
    for e in std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display())).flatten() {
        let p = e.path();
        if p.extension().is_none_or(|x| x != "sh") {
            continue;
        }
        names.push(p.file_name().unwrap().to_string_lossy().into_owned());
        let o = Command::new("bash").arg("-n").arg(&p).output().unwrap();
        assert!(o.status.success(), "bash -n {}: {}", p.display(), String::from_utf8_lossy(&o.stderr));
        assert!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o111 != 0, "{} not executable", p.display());
        // An unknown argument stops every spike before it does anything.
        let o = Command::new("bash").arg(&p).arg("--no-such-flag").env_clear().env("PATH", "/usr/bin:/bin").output().unwrap();
        assert_eq!(o.status.code(), Some(2), "{}: {}", p.display(), String::from_utf8_lossy(&o.stderr));
    }
    names.sort();
    assert_eq!(names, ["s0a-tests-as-hidden-user.sh", "s0b-linux-vm-on-mac.sh", "s0c-launchdaemon-probe.sh"]);
}

#[test]
fn installers_refuse_to_run_unprivileged_before_touching_anything() {
    // As a normal user every installer stops at its root check (or OS check).
    assert_ne!(unsafe { libc::geteuid() }, 0, "these tests must never run as root: the scripts would really install");
    for p in scripts() {
        let o = Command::new("bash").arg(&p).arg("--binary").arg(bin()).env_clear().env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin").output().unwrap();
        assert!(!o.status.success(), "{} succeeded as a normal user", p.display());
        let err = String::from_utf8_lossy(&o.stderr);
        assert!(err.contains("root") || err.contains("only") || err.contains("installer") || err.contains("unknown argument"), "{}: {err}", p.display());
    }
}

#[test]
fn version_and_usage() {
    let (ok, out, _) = run(&["version"]);
    assert!(ok);
    assert!(out.contains(&format!("protocol {}", k2_node_proto::PROTOCOL)), "{out}");
    let (ok, _, err) = run(&["nope"]);
    assert!(!ok);
    assert!(err.contains("k2-node run"), "{err}");
}

#[test]
fn pause_drain_resume_write_the_control_file() {
    let d = k2_node::util::temp_dir("cli-control");
    let cfg = d.to_str().unwrap();
    for (args, want) in [
        (vec!["pause", "--by", "rosson"], Control::Paused),
        (vec!["pause", "--now", "--by", "rosson"], Control::Stopped),
        (vec!["drain", "--by", "rosson"], Control::Draining),
        (vec!["resume", "--by", "rosson"], Control::Active),
    ] {
        let mut a = args.clone();
        a.extend(["--config-dir", cfg]);
        let (ok, out, err) = run(&a);
        assert!(ok, "{a:?}: {err}");
        assert!(out.contains(want.as_str()), "{out}");
        let c = k2_node::config::read_control(&d.join("control.toml")).unwrap();
        assert_eq!((c.control, c.by.as_deref()), (want, Some("rosson")));
    }
}

#[test]
fn control_writes_fail_loudly_without_permission() {
    let d = k2_node::util::temp_dir("cli-ro");
    let ro = d.join("ro");
    std::fs::create_dir_all(&ro).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555)).unwrap();
    let (ok, _, err) = run(&["pause", "--config-dir", ro.to_str().unwrap()]);
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(!ok);
    assert!(err.contains("k2nodectl"), "the error says who may write it: {err}");
}

#[test]
fn policy_set_and_show() {
    let d = k2_node::util::temp_dir("cli-policy");
    let cfg = d.to_str().unwrap();
    let home = d.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let (ok, out, err) = run(&["policy", "set", "cpu_cap_percent=40", "availability=window", "--config-dir", cfg, "--home", home.to_str().unwrap()]);
    assert!(!ok, "window without a [[window]] is refused: {out}");
    assert!(err.contains("window"), "{err}");
    let (ok, out, err) = run(&["policy", "set", "cpu_cap_percent=40", "max_parallel=3", "--config-dir", cfg, "--home", home.to_str().unwrap()]);
    assert!(ok, "{err}");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!((v["cpu_cap_percent"].as_u64(), v["max_parallel"].as_u64()), (Some(40), Some(3)));
    let (ok, out, _) = run(&["policy", "show", "--config-dir", cfg, "--home", home.to_str().unwrap()]);
    assert!(ok);
    assert!(out.contains("\"cpu_cap_percent\": 40"), "{out}");
}

#[test]
fn check_home_refuses_a_k2_users_home() {
    let d = k2_node::util::temp_dir("cli-home");
    assert!(run(&["check-home", d.to_str().unwrap()]).0);
    std::fs::create_dir_all(d.join(".k2")).unwrap();
    std::fs::write(d.join(".k2/daemon.port"), "1234").unwrap();
    let (ok, _, err) = run(&["check-home", d.to_str().unwrap()]);
    assert!(!ok);
    assert!(err.contains("runs K2"), "{err}");
}

#[test]
fn enroll_rejects_a_bad_enroll_string_before_dialing() {
    let d = k2_node::util::temp_dir("cli-enroll");
    let (ok, _, err) = run(&["enroll", "--controller", "https://x.k2.dev", "--enroll", "nonsense", "--name", "mini-1", "--home", d.to_str().unwrap()]);
    assert!(!ok);
    assert!(err.contains("ABCDE-FGHJK"), "{err}");
    let (ok, _, err) = run(&["enroll", "--controller", "http://8.8.8.8", "--enroll", "ABCDE-FGHJK.0123456789abcdef", "--name", "mini-1", "--home", d.to_str().unwrap()]);
    assert!(!ok);
    assert!(err.contains("private network"), "plaintext to a public host is refused: {err}");
}

#[test]
fn install_files_writes_the_service_and_default_config() {
    let d = k2_node::util::temp_dir("cli-install");
    let out = d.join("out");
    let (ok, stdout, err) = run(&[
        "install-files", "--os", "linux", "--binary", "/usr/local/libexec/k2-node", "--home", "/var/lib/k2node",
        "--config-dir", "/etc/k2-node", "--user", "k2node", "--group", "k2node", "--out", out.to_str().unwrap(),
    ]);
    assert!(ok, "{err}");
    assert_eq!(stdout.lines().count(), 3);
    let unit = std::fs::read_to_string(out.join("k2-node.service")).unwrap();
    assert!(unit.contains("User=k2node") && unit.contains("Delegate=yes"), "{unit}");
}
