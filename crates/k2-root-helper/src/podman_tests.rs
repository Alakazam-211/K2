//! `apps podman install|uninstall` tests (protocol 8, lane P) over the temp
//! root and fakes of `tests.rs`. The happy path pins a small fixture
//! tarball (the real pins are checked in `k2-root-proto`); everything
//! else goes through the real verb with this build's pins.

use std::os::unix::fs::PermissionsExt;

use k2_root_proto::podman::*;
use k2_root_proto::verbs::exit;

use super::apps_tests::{bin, calls_of, json, read, rule, with_app_user, write, APP_UID};
use super::{box_root, ok, T};
use crate::podman as hp;

const NOBLE: &str = "PRETTY_NAME=\"Ubuntu 24.04.3 LTS\"\nNAME=\"Ubuntu\"\nVERSION_ID=\"24.04\"\nID=ubuntu\nID_LIKE=debian\n";
const TRIXIE: &str = "PRETTY_NAME=\"Debian GNU/Linux 13 (trixie)\"\nVERSION_ID=\"13\"\nID=debian\n";
const TOP: &str = "podman-linux-amd64";

/// A gzip tarball shaped like podman-static's: every member K2 takes,
/// plus files it must ignore. `link` makes that member a symlink.
fn tarball(skip: &[&str], link: Option<&str>) -> Vec<u8> {
    let mut b = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
    let mut add = |path: &str, body: &[u8]| {
        let mut h = tar::Header::new_gnu();
        h.set_size(body.len() as u64);
        h.set_mode(0o755);
        h.set_cksum();
        b.append_data(&mut h, format!("{TOP}/{path}"), body).unwrap();
    };
    for m in MEMBERS {
        if skip.contains(&m.src) || link == Some(m.src) {
            continue;
        }
        add(m.src, format!("fake {}\n", m.dest).as_bytes());
    }
    // Never written.
    add("usr/local/bin/runc", b"runc\n");
    add("etc/containers/policy.json", b"{}\n");
    add("usr/local/lib/systemd/system/podman.service", b"[Unit]\n");
    if let Some(src) = link {
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(tar::EntryType::Symlink);
        h.set_size(0);
        h.set_mode(0o777);
        b.append_link(&mut h, format!("{TOP}/{src}"), "/etc/shadow").unwrap();
    }
    b.into_inner().unwrap().finish().unwrap()
}

fn sha(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// A pin for a fixture tarball.
fn pin_for(bytes: &[u8]) -> PodmanArtifact {
    PodmanArtifact {
        arch: "x86_64",
        asset: "podman-linux-amd64.tar.gz",
        top: TOP,
        sha256: Box::leak(sha(bytes).into_boxed_str()),
        size: bytes.len() as u64,
    }
}

/// A box the bundle is for: Ubuntu 24.04, k2app, /etc/systemd, and the
/// bundle's podman answering its version once installed.
fn noble_box() -> T {
    let t = box_root();
    write(&t, OS_RELEASE, NOBLE);
    with_app_user(&t);
    rule(&t, PODMAN, "--version", 0, "podman version 5.8.8\n");
    t
}

fn nothing_installed(t: &T) {
    assert!(!t.p(OPT_K2).exists(), "/opt/k2 written");
    assert!(!t.p(USER_GENERATOR).exists(), "generator written");
    assert!(!t.p(ROOTLESS_CONF_PARENT).exists(), "rootless conf written");
    assert!(!t.p(APPARMOR_USERNS_FILE).exists(), "apparmor profile written");
}

#[test]
fn sha256_mismatch_is_refused_and_nothing_is_installed() {
    let t = noble_box();
    let o = t.call_in(&["apps", "podman", "install"], &tarball(&[], None));
    assert_eq!(o.code, exit::SIGNATURE, "{}", o.stderr);
    assert!(o.stderr.contains("sha256 mismatch"), "{}", o.stderr);
    nothing_installed(&t);
    // One flipped byte against a fixture pin: same refusal.
    let good = tarball(&[], None);
    let pin = pin_for(&good);
    let mut bad = good.clone();
    let n = bad.len();
    bad[n - 9] ^= 0x01;
    let e = hp::install_with(&t.ctx, &bad, &pin, "ubuntu 24.04".into()).unwrap_err();
    assert_eq!(e.code, exit::SIGNATURE);
    nothing_installed(&t);
    // The audit line names the refusal, never stdin.
    let audit = t.audit.0.borrow().join("\n");
    assert!(audit.contains("\"verb\":\"apps podman install\""), "{audit}");
    assert!(audit.contains("refused_or_failed"), "{audit}");
}

#[test]
fn only_the_boxes_the_bundle_is_for() {
    for (os, why) in [(TRIXIE, "debian 13"), ("ID=ubuntu\nVERSION_ID=\"26.04\"\n", "ubuntu 26.04"), ("", "unknown")] {
        let t = box_root();
        write(&t, OS_RELEASE, os);
        with_app_user(&t);
        let o = t.call_in(&["apps", "podman", "install"], &tarball(&[], None));
        assert_eq!(o.code, exit::STATE, "{why}");
        assert!(o.stderr.starts_with("podman_os_unsupported"), "{why}: {}", o.stderr);
        nothing_installed(&t);
    }
}

/// Ubuntu and Debian ship /etc/os-release as a link to
/// ../usr/lib/os-release (the VM e2e caught this): the link is never
/// followed, the /usr/lib file is read instead.
#[test]
fn a_linked_etc_os_release_reads_usr_lib() {
    let t = box_root();
    write(&t, OS_RELEASE_USR, NOBLE);
    std::os::unix::fs::symlink("../usr/lib/os-release", t.p(OS_RELEASE)).unwrap();
    assert_eq!(hp::supported(&t.ctx).unwrap(), "ubuntu 24.04");
    // A link to something else is never followed either.
    let t = box_root();
    write(&t, "/root/fake-os-release", NOBLE);
    std::os::unix::fs::symlink("/root/fake-os-release", t.p(OS_RELEASE)).unwrap();
    let e = hp::supported(&t.ctx).unwrap_err();
    assert!(e.msg.starts_with("podman_os_unsupported"), "{}", e.msg);
}

#[test]
fn an_agent_cannot_install_or_uninstall() {
    let t = noble_box();
    t.as_agent();
    for argv in [&["apps", "podman", "install"][..], &["apps", "podman", "uninstall"]] {
        let o = t.call_in(argv, &tarball(&[], None));
        assert_eq!(o.code, exit::NOT_DAEMON, "{argv:?}");
    }
    nothing_installed(&t);
}

#[test]
fn install_writes_only_the_members_then_the_k2app_config() {
    let t = noble_box();
    let tb = tarball(&[], None);
    let pin = pin_for(&tb);
    let v = hp::install_with(&t.ctx, &tb, &pin, "ubuntu 24.04".into()).unwrap();
    assert_eq!(v["installed"], BUNDLE_VERSION);
    assert_eq!(v["changed"], true);
    assert_eq!(v["k2appUid"], APP_UID);
    for m in MEMBERS {
        let p = t.p(&format!("{PREFIX}/{}", m.dest));
        assert_eq!(std::fs::read_to_string(&p).unwrap(), format!("fake {}\n", m.dest), "{m:?}");
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o755, "{m:?}");
    }
    for never in ["bin/runc", "etc", "lib/systemd", "usr"] {
        assert!(!t.p(&format!("{PREFIX}/{never}")).exists(), "{never}");
    }
    assert_eq!(read(&t, MARKER), marker("x86_64", pin.sha256));
    assert!(!t.p(&format!("{OPT_K2}/{STAGE_NAME}")).exists());
    assert!(!t.p(&format!("{OPT_K2}/{OLD_NAME}")).exists());
    assert_eq!(read(&t, &format!("{}/{ROOTLESS_CONF_FILE}", rootless_conf_dir(APP_UID))), rootless_conf());
    assert_eq!(read(&t, USER_GENERATOR), user_generator(APP_UID));
    assert_eq!(std::fs::metadata(t.p(USER_GENERATOR)).unwrap().permissions().mode() & 0o777, 0o755);
    // No user-namespace restriction here: no AppArmor profile.
    assert_eq!(v["apparmor"]["needed"], false);
    assert!(!t.p(APPARMOR_USERNS_FILE).exists());
    // The distro's Podman is never touched; k2app now runs the bundle.
    assert!(calls_of(&t, k2_root_proto::apps::APT_GET).is_empty());
    assert!(hp::installed(&t.ctx));
    assert_eq!(hp::effective_podman(&t.ctx), PODMAN);
    assert_eq!(hp::effective_crun(&t.ctx), CRUN);

    // Again: the tree is current, nothing changes.
    let v = hp::install_with(&t.ctx, &tb, &pin, "ubuntu 24.04".into()).unwrap();
    assert_eq!(v["changed"], false, "{v}");

    // apps status sees the bundle as the engine's Podman.
    let o = t.call(&["apps", "status"]);
    ok(&o);
    let s = json(&o);
    assert_eq!(s["engine"]["podmanSource"], "k2");
    assert_eq!(s["engine"]["podman"], "5.8.8");
    assert_eq!(s["engine"]["podmanOk"], true);
    assert_eq!(s["podmanBundle"]["installed"], true);
    assert_eq!(s["podmanBundle"]["conf"], "ok");
    assert_eq!(s["podmanBundle"]["generator"], "ok");
    assert_eq!(s["podmanBundle"]["osSupported"], "ubuntu 24.04");
    // The fixture isn't this build's pin, so the bundle isn't "current".
    assert_eq!(s["podmanBundle"]["current"], false);
}

#[test]
fn bad_archives_are_refused_before_anything_is_written() {
    let req = MEMBERS.iter().find(|m| m.required && m.dest == "libexec/podman/quadlet").unwrap();
    for (tb, want) in [
        (tarball(&[req.src], None), "has no usr/local/libexec/podman/quadlet"),
        (tarball(&[], Some("usr/local/bin/crun")), "bin/crun is not a regular file"),
    ] {
        let t = noble_box();
        let pin = pin_for(&tb);
        let e = hp::install_with(&t.ctx, &tb, &pin, "ubuntu 24.04".into()).unwrap_err();
        assert_eq!(e.code, exit::ARGV, "{}", e.msg);
        assert!(e.msg.contains(want), "{}", e.msg);
        nothing_installed(&t);
    }
    // The optional member may be missing (arm64 has no pasta.avx2).
    let t = noble_box();
    let tb = tarball(&["usr/local/bin/pasta.avx2"], None);
    hp::install_with(&t.ctx, &tb, &pin_for(&tb), "ubuntu 24.04".into()).unwrap();
}

#[test]
fn no_k2app_no_install_and_a_broken_podman_fails_loudly() {
    let t = box_root();
    write(&t, OS_RELEASE, NOBLE);
    let tb = tarball(&[], None);
    let e = hp::install_with(&t.ctx, &tb, &pin_for(&tb), "ubuntu 24.04".into()).unwrap_err();
    assert!(e.msg.starts_with("box_not_prepared"), "{}", e.msg);
    nothing_installed(&t);

    // The installed podman doesn't answer 5.8.8: a failure, not a pass.
    let t = box_root();
    write(&t, OS_RELEASE, NOBLE);
    with_app_user(&t);
    rule(&t, PODMAN, "--version", 0, "podman version 4.9.3\n");
    let e = hp::install_with(&t.ctx, &tb, &pin_for(&tb), "ubuntu 24.04".into()).unwrap_err();
    assert!(e.msg.starts_with("podman_bundle_broken"), "{}", e.msg);
}

#[test]
fn a_generator_k2_did_not_write_is_never_replaced() {
    let t = noble_box();
    write(&t, USER_GENERATOR, "#!/bin/sh\n# the admin's own\nexit 0\n");
    let tb = tarball(&[], None);
    let e = hp::install_with(&t.ctx, &tb, &pin_for(&tb), "ubuntu 24.04".into()).unwrap_err();
    assert!(e.msg.contains("K2 didn't write it"), "{}", e.msg);
    assert_eq!(read(&t, USER_GENERATOR), "#!/bin/sh\n# the admin's own\nexit 0\n");
}

#[test]
fn ubuntu_userns_restriction_gets_the_apparmor_profiles() {
    let t = noble_box();
    write(&t, USERNS_RESTRICT, "1\n");
    bin(&t, "/usr/sbin/apparmor_parser");
    let tb = tarball(&[], None);
    let v = hp::install_with(&t.ctx, &tb, &pin_for(&tb), "ubuntu 24.04".into()).unwrap();
    assert_eq!(v["apparmor"]["needed"], true);
    assert_eq!(read(&t, APPARMOR_USERNS_FILE), apparmor_userns_profiles());
    assert_eq!(
        calls_of(&t, "/usr/sbin/apparmor_parser"),
        vec!["--replace --write-cache /etc/apparmor.d/k2-podman"]
    );
    // Restricted but no parser: refused, not silently skipped.
    let t = noble_box();
    write(&t, USERNS_RESTRICT, "1\n");
    let e = hp::install_with(&t.ctx, &tb, &pin_for(&tb), "ubuntu 24.04".into()).unwrap_err();
    assert!(e.msg.starts_with("apparmor_missing"), "{}", e.msg);
}

#[test]
fn uninstall_waits_for_apps_then_removes_only_what_install_wrote() {
    let t = noble_box();
    write(&t, USERNS_RESTRICT, "1\n");
    bin(&t, "/usr/sbin/apparmor_parser");
    let tb = tarball(&[], None);
    hp::install_with(&t.ctx, &tb, &pin_for(&tb), "ubuntu 24.04".into()).unwrap();
    write(&t, k2_root_proto::apps::APPARMOR_PROFILES, "k2-podman (unconfined)\nk2-crun (unconfined)\n");
    // A distro file next to ours stays.
    write(&t, "/etc/containers/containers.conf", "# distro\n");

    // k2app's own D-Bus and engine never block it; another user's conmon
    // neither.
    let st = |name: &str, uid: u32| format!("Name:\t{name}\nUid:\t{uid}\t{uid}\t{uid}\t{uid}\n");
    write(&t, "/proc/4000/status", &st("dbus-daemon", APP_UID));
    write(&t, "/proc/4001/status", &st("k2-app-engine", APP_UID));
    write(&t, "/proc/4002/status", &st("conmon", 1000));
    // A container runs as k2app (its conmon): refused, nothing removed.
    write(&t, "/proc/4321/status", &st("conmon", APP_UID));
    let o = t.call(&["apps", "podman", "uninstall"]);
    assert_eq!(o.code, exit::STATE);
    assert!(o.stderr.starts_with("apps_running: 1 "), "{}", o.stderr);
    assert!(t.p(MARKER).exists());

    std::fs::remove_dir_all(t.p("/proc/4321")).unwrap();
    let o = t.call(&["apps", "podman", "uninstall"]);
    ok(&o);
    let v = json(&o);
    assert_eq!(v["changed"], true);
    assert!(!t.p(PREFIX).exists());
    assert!(!t.p(USER_GENERATOR).exists());
    assert!(!t.p(&rootless_conf_dir(APP_UID)).exists());
    assert!(!t.p(APPARMOR_USERNS_FILE).exists());
    assert!(t.p(OPT_K2).is_dir(), "/opt/k2 itself stays");
    assert_eq!(read(&t, "/etc/containers/containers.conf"), "# distro\n");
    assert!(calls_of(&t, "/usr/sbin/apparmor_parser").contains(&"--remove /etc/apparmor.d/k2-podman".to_string()));
    assert!(!hp::installed(&t.ctx));
    assert_eq!(hp::effective_podman(&t.ctx), "/usr/bin/podman");
    let audit = t.audit.0.borrow().join("\n");
    assert!(audit.contains("\"verb\":\"apps podman uninstall\""), "{audit}");
    assert!(audit.contains("\"removed\":["), "{audit}");

    // Twice: nothing left to remove.
    let o = t.call(&["apps", "podman", "uninstall"]);
    ok(&o);
    assert_eq!(json(&o)["changed"], false);

    // A generator K2 didn't write survives uninstall.
    write(&t, USER_GENERATOR, "#!/bin/sh\nexit 0\n");
    let o = t.call(&["apps", "podman", "uninstall"]);
    ok(&o);
    assert_eq!(read(&t, USER_GENERATOR), "#!/bin/sh\nexit 0\n");
    assert!(json(&o)["kept"][0].as_str().unwrap().contains("K2 didn't write it"));
}

// ── 0.46.3: box prep's packages around the bundle (F3, F4, F5) ─────────

use super::apps_tests::{APT_BUNDLE, APT_DISTRO, APT_DISTRO_KEEP, APT_SIM, APT_Y};
use crate::apps::{apt_sim_refusal, select_packages, Pm, PodmanHave};
use k2_root_proto::apps as pa;

/// A noble box with K2's bundle installed and apt present.
fn noble_with_bundle() -> T {
    let t = noble_box();
    let tb = tarball(&[], None);
    hp::install_with(&t.ctx, &tb, &pin_for(&tb), "ubuntu 24.04".into()).unwrap();
    bin(&t, pa::APT_GET);
    t
}

fn env_calls(t: &T) -> Vec<String> {
    calls_of(t, pa::ENV)
}

/// F3: where the bundle runs k2app, apt installs only what the bundle
/// can't carry (no distro podman, crun, passt or catatonit); the distro
/// path keeps the full list; dnf/pacman never meet the bundle but follow
/// the same rule.
#[test]
fn f3_the_bundle_path_installs_only_what_the_bundle_cant_carry() {
    let t = noble_with_bundle();
    // skopeo's containers-common didn't land the policy: a clear stop.
    let o = t.call(&["apps", "packages", "install"]);
    assert_eq!(o.code, exit::STATE, "{}", o.stderr);
    assert!(o.stderr.starts_with("policy_missing"), "{}", o.stderr);
    write(&t, pa::CONTAINERS_POLICY, "{}\n");
    let o = t.call(&["apps", "packages", "install"]);
    ok(&o);
    let v = json(&o);
    assert_eq!(v["packages"]["path"], "bundle", "{v}");
    assert_eq!(v["podmanSource"], "k2");
    assert_eq!(v["podmanOk"], true);
    assert!(v["packages"]["boxPodman"].is_null());
    let calls = env_calls(&t);
    assert_eq!(calls.last().unwrap(), &format!("{APT_Y} {APT_BUNDLE}"));
    assert!(calls.contains(&format!("{APT_SIM} {APT_BUNDLE}")), "{calls:?}");
    for c in &calls {
        for distro_only in ["podman", "crun", "passt", "catatonit"] {
            assert!(!c.split(' ').any(|w| w == distro_only), "{distro_only} asked for on the bundle path: {c}");
        }
    }

    // The pure selection, every path.
    let b = |pm| select_packages(pm, true, &PodmanHave::None).unwrap().join(" ");
    assert_eq!(b(Pm::Apt), APT_BUNDLE);
    assert_eq!(b(Pm::Dnf), "shadow-utils skopeo");
    assert_eq!(b(Pm::Pacman), "skopeo");
    assert_eq!(select_packages(Pm::Apt, false, &PodmanHave::None).unwrap().join(" "), APT_DISTRO);
    assert_eq!(select_packages(Pm::Apt, false, &PodmanHave::Distro).unwrap().join(" "), APT_DISTRO_KEEP);
    assert_eq!(
        select_packages(Pm::Dnf, false, &PodmanHave::None).unwrap().join(" "),
        "podman crun passt shadow-utils skopeo catatonit"
    );
    // A box Podman never changes the bundle path's list.
    for have in [PodmanHave::Distro, PodmanHave::Local("/usr/local/bin/podman")] {
        assert_eq!(select_packages(Pm::Apt, true, &have).unwrap().join(" "), APT_BUNDLE, "{have:?}");
    }

    // The distro path (Debian 13: no bundle) on a box with no Podman: the full list.
    let t = box_root();
    write(&t, OS_RELEASE, TRIXIE);
    bin(&t, pa::APT_GET);
    let _ = t.call(&["apps", "packages", "install"]);
    assert_eq!(env_calls(&t).last().unwrap(), &format!("{APT_Y} {APT_DISTRO}"));
}

/// F4: the box's own Podman is never upgraded and never gets a second one
/// next to it: a distro Podman is left off the list (and apt runs
/// --no-upgrade), a source-built one in /usr/local stops the distro path
/// before apt runs, and an apt plan that would upgrade, add or remove
/// anyway is refused before the real install.
#[test]
fn f4_the_boxs_own_podman_is_never_upgraded_or_duplicated() {
    // Source-built Podman, no bundle for this OS: nothing runs.
    let t = box_root();
    write(&t, OS_RELEASE, TRIXIE);
    bin(&t, pa::APT_GET);
    bin(&t, "/usr/local/bin/podman");
    let o = t.call(&["apps", "packages", "install"]);
    assert_eq!(o.code, exit::STATE);
    assert!(o.stderr.starts_with("customer_podman") && o.stderr.contains("/usr/local/bin/podman"), "{}", o.stderr);
    assert!(env_calls(&t).is_empty(), "apt never ran: {:?}", env_calls(&t));
    assert!(!t.p(pa::PODMAN).exists());

    // Source-built Podman on a bundle box: the bundle for k2app, theirs untouched.
    let t = noble_with_bundle();
    bin(&t, "/usr/local/sbin/podman");
    write(&t, pa::CONTAINERS_POLICY, "{}\n");
    let o = t.call(&["apps", "packages", "install"]);
    ok(&o);
    assert_eq!(env_calls(&t).last().unwrap(), &format!("{APT_Y} {APT_BUNDLE}"));
    assert_eq!(json(&o)["packages"]["boxPodman"], "/usr/local/sbin/podman");

    // A distro Podman 5 (kept): not on the list, apt --no-upgrade.
    let t = box_root();
    write(&t, OS_RELEASE, TRIXIE);
    bin(&t, pa::APT_GET);
    bin(&t, pa::PODMAN);
    bin(&t, pa::PASTA);
    rule(&t, pa::PODMAN, "--version", 0, "podman version 5.4.2\n");
    let o = t.call(&["apps", "packages", "install"]);
    ok(&o);
    let calls = env_calls(&t);
    assert_eq!(calls[1], format!("{APT_SIM} {APT_DISTRO_KEEP}"));
    assert_eq!(calls[2], format!("{APT_Y} {APT_DISTRO_KEEP}"));
    assert!(calls[1..].iter().all(|c| c.contains("--no-upgrade")), "{calls:?}");
    assert_eq!(json(&o)["packages"]["boxPodman"], pa::PODMAN);

    // apt's plan would upgrade that Podman anyway (a dependency): refused,
    // and the real install never runs.
    let t = box_root();
    write(&t, OS_RELEASE, TRIXIE);
    bin(&t, pa::APT_GET);
    bin(&t, pa::PODMAN);
    rule(&t, pa::ENV, "-s", 0, "Inst libgpgme11 (1.18.0-4) []\nInst podman [5.4.2+ds1-1] (5.4.2+ds1-2 Debian:13/stable [amd64])\n");
    let o = t.call(&["apps", "packages", "install"]);
    assert_eq!(o.code, exit::STATE);
    assert!(o.stderr.starts_with("customer_podman: apt would upgrade podman"), "{}", o.stderr);
    assert!(!env_calls(&t).iter().any(|c| c.contains(" -y ")), "no real install: {:?}", env_calls(&t));

    // ...or remove something: refused the same way.
    let t = box_root();
    write(&t, OS_RELEASE, TRIXIE);
    bin(&t, pa::APT_GET);
    rule(&t, pa::ENV, "-s", 0, "Remv docker-ce [27.1.1]\nInst podman (5.4.2 Debian:13/stable [amd64])\n");
    let o = t.call(&["apps", "packages", "install"]);
    assert_eq!(o.code, exit::STATE);
    assert!(o.stderr.starts_with("apt_would_remove") && o.stderr.contains("docker-ce"), "{}", o.stderr);
    assert!(!env_calls(&t).iter().any(|c| c.contains(" -y ")));

    // The pure check.
    let listed = ["uidmap", "skopeo"];
    assert!(apt_sim_refusal("Inst podman:amd64 (4.9.3 Ubuntu:24.04/noble [amd64])\n", &listed).unwrap().contains("install podman"));
    assert!(apt_sim_refusal("Inst podman (4.9.3)\n", &["podman", "skopeo"]).is_none(), "asked for on an empty box");
    assert!(apt_sim_refusal("Inst skopeo (1.13)\nConf skopeo (1.13)\n", &listed).is_none());
    assert!(apt_sim_refusal("", &listed).is_none());
}

/// F5: below 5 GiB free on / or /var, neither packages nor the bundle
/// start: nothing installed, nothing staged, a `low_disk` line that says
/// where and how much.
#[test]
fn f5_low_disk_refuses_before_any_package_or_bundle_step() {
    let mut t = noble_box();
    bin(&t, pa::APT_GET);
    t.ctx.free_bytes = |p| Ok(if p.ends_with("var") { 3 * 1024 * 1024 * 1024 } else { u64::MAX });
    let o = t.call(&["apps", "packages", "install"]);
    assert_eq!(o.code, exit::STATE);
    assert!(o.stderr.starts_with("low_disk: 3.0 GiB free on /var;"), "{}", o.stderr);
    assert!(o.stderr.contains("5.0 GiB free on / and /var"), "{}", o.stderr);
    assert!(env_calls(&t).is_empty(), "apt never ran");
    let tb = tarball(&[], None);
    let e = hp::install_with(&t.ctx, &tb, &pin_for(&tb), "ubuntu 24.04".into()).unwrap_err();
    assert!(e.msg.starts_with("low_disk"), "{}", e.msg);
    nothing_installed(&t);
    // Exactly at the floor is enough; one byte under isn't.
    assert!(pa::low_disk(&[("/".into(), pa::MIN_FREE_BYTES)]).is_none());
    assert!(pa::low_disk(&[("/".into(), pa::MIN_FREE_BYTES - 1)]).is_some());
    t.ctx.free_bytes = |_| Ok(pa::MIN_FREE_BYTES);
    let o = t.call(&["apps", "packages", "install"]);
    assert!(!o.stderr.starts_with("low_disk"), "{}", o.stderr);
    assert_eq!(env_calls(&t)[0], "DEBIAN_FRONTEND=noninteractive /usr/bin/apt-get update -q", "apt ran this time");
}
