//! The `k2-node` binary and the installer scripts. Temp dirs only; no real
//! dscl/useradd/systemctl/launchctl ever runs here.
// pending k2-node Windows port: k2-node (the compute node runtime) is Unix-only
// until its Windows port (Phase 3: Job Objects instead of uids and cgroups).
#![cfg(unix)]

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
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } == 0 {
        // Root bypasses directory mode bits, so the 0555 folder does not stop
        // the write: root is a machine owner. The Linux soak and release gate
        // run tests as root, so assert that real behaviour instead.
        assert!(ok, "root ignores 0555, so pause must succeed: {err}");
        let c = k2_node::config::read_control(&ro.join("control.toml")).unwrap();
        assert_eq!(c.control, Control::Paused);
        return;
    }
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

/// MT37: `enroll --force` on a shared node no longer drops the other
/// servers in silence: it refuses before dialing and points at `servers
/// owner`; `--drop-servers` goes ahead on purpose.
#[test]
fn enroll_force_on_a_shared_node_points_at_servers_owner() {
    use k2_node_proto::share::{Network, ServersFile, ShareLimits, Trust};
    let d = k2_node::util::temp_dir("cli-enroll-shared");
    let layout = k2_node::paths::Layout::new(d.join("home"), d.join("cfg"));
    layout.ensure().unwrap();
    std::fs::create_dir_all(&layout.config).unwrap();
    let k = k2_node_proto::crypto::SigningKey::generate().unwrap();
    let pin = k2_node::identity::Pin {
        controller_fp: k.fingerprint(),
        controller_public_key_pem: k.spki_pem(),
        routes: vec!["https://rosson.k2.dev".into()],
        node_id: "n0".into(),
        name: "mini-1".into(),
        enrolled_at: 1,
        sas: "000 000".into(),
        revoked: false,
        revoked_reason: None,
        labels: Default::default(),
        confirmed: true,
    };
    k2_node::identity::write_pin(&layout.pin(), &pin).unwrap();
    let mut f = ServersFile::new(&k.fingerprint(), "rosson.k2.dev", ShareLimits::owner(1, 1, 1, 1, Network::On), 3, 1, "local:test");
    let mut b = f.servers[0].clone();
    b.slot = 1;
    b.label = "z3thon.k2.dev".into();
    b.fp_prefix = "0123456789abcdef".into();
    b.set_limits(&ShareLimits::new_server(Trust::Own, false));
    f.servers.push(b);
    k2_node::admin::servers::write_file(&layout.config, &f).unwrap();
    let (home, cfg) = (layout.home.display().to_string(), layout.config.display().to_string());
    let mut args = vec!["enroll", "--controller", "https://127.0.0.1:9", "--enroll", "ABCDE-FGHJK.0123456789abcdef", "--name", "mini-1", "--home", &home, "--config-dir", &cfg, "--force"];
    let (ok, _, err) = run(&args);
    assert!(!ok);
    assert!(err.contains("z3thon.k2.dev") && err.contains("sudo k2-node servers owner") && err.contains("--drop-servers"), "{err}");
    assert_eq!(k2_node::identity::read_pin(&layout.pin()).unwrap().unwrap().controller_fp, k.fingerprint(), "nothing changed");
    args.push("--drop-servers");
    let (ok, _, err) = run(&args);
    assert!(!ok, "nobody answers at 127.0.0.1:9");
    assert!(!err.contains("servers owner"), "past the check, it dials: {err}");
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

/// A real install never runs `--dev` (the `--dev` sharing and no-sudo
/// exemptions exist for tests only): not in any installer script, nor in
/// the service files the binary writes for either OS.
#[test]
fn no_install_path_ever_runs_dev() {
    for s in scripts() {
        let body = std::fs::read_to_string(&s).unwrap();
        assert!(!body.contains("--dev"), "{} passes --dev", s.display());
    }
    for os in ["linux", "macos"] {
        let d = k2_node::util::temp_dir("cli-nodev");
        let out = d.join("out");
        let (ok, _, err) = run(&[
            "install-files", "--os", os, "--binary", "/usr/local/libexec/k2-node", "--home", "/var/lib/k2node",
            "--config-dir", "/etc/k2-node", "--user", "k2node", "--group", "k2node", "--out", out.to_str().unwrap(),
        ]);
        assert!(ok, "{os}: {err}");
        let mut seen = 0;
        for f in std::fs::read_dir(&out).unwrap().flatten() {
            let body = std::fs::read_to_string(f.path()).unwrap();
            assert!(!body.contains("--dev"), "{os}: {} passes --dev", f.path().display());
            seen += 1;
        }
        assert!(seen >= 2, "{os}: install-files wrote {seen} files");
    }
    // And the exemption itself exists only in debug builds, and never for
    // an installed node's config (it has admin.toml), even in a debug one.
    let cfg = k2_node::util::temp_dir("cli-dev-exempt");
    assert_eq!(k2_node::node::dev_test_exemption(true, &cfg), cfg!(debug_assertions));
    assert!(!k2_node::node::dev_test_exemption(false, &cfg));
    std::fs::write(cfg.join("admin.toml"), "remote_admin = \"off\"\n").unwrap();
    assert!(!k2_node::node::dev_test_exemption(true, &cfg), "an installed node's config never takes the --dev exemption");
}

/// z3mbpZ e2e fix 3: re-running the macOS installer with --binary pointing
/// at the installed k2-node (an upgrade in place with nothing to copy) is a
/// no-op copy, not `install`'s "are the same file" stopping the script
/// under `set -e`. The script's own `install_binary` is run on its own.
#[test]
fn the_macos_installer_takes_the_installed_binary_as_is() {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/node/install-node-macos.sh");
    let text = std::fs::read_to_string(&script).unwrap();
    let start = text.find("install_binary() {").expect("install_binary in the installer");
    let end = start + text[start..].find("\n}\n").expect("its end") + 3;
    let func = &text[start..end];
    assert!(text.contains("install_binary \"$BINARY\" \"$BIN_DIR/k2-node\""), "the installer copies through it");
    let d = k2_node::util::temp_dir("installer-same-file");
    let bin = d.join("k2-node");
    std::fs::write(&bin, "#!/bin/sh\n").unwrap();
    let link = d.join("via-link");
    std::os::unix::fs::symlink(&bin, &link).unwrap();
    for src in [&bin, &link] {
        let out = Command::new("bash")
            .args(["-c", &format!("set -euo pipefail\n{func}\ninstall_binary \"$1\" \"$2\"\necho AFTER"), "x", src.to_str().unwrap(), bin.to_str().unwrap()])
            .output()
            .unwrap();
        let said = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success() && said.contains("already that binary") && said.contains("AFTER"), "{said} {}", String::from_utf8_lossy(&out.stderr));
    }
}
