//! `desktop packages install` (protocol 16, `namespaces.desktop`; L5 of
//! `prd-linux-virtual-desktop-v1`) over the temp root and fakes of
//! `tests.rs`. Nothing here runs apt, pacman or dnf for real.

use std::rc::Rc;

use k2_root_proto::apps::{APT_GET, DNF, ENV, PACMAN};
use k2_root_proto::desktop::{ICEWM_SESSION, XVFB};
use k2_root_proto::verbs::{exit, Class, Verb};

use super::apps_tests::{bin, calls_of, json, rule};
use super::{box_root, ok, T};

const ARGV: &[&str] = &["desktop", "packages", "install"];

/// apt's update, simulation and install as the helper runs them for the
/// desktop: the closed list, never a recommend, never an upgrade of what
/// is there, a bounded wait for the dpkg lock.
const APT_OPTS: &str = "-q --no-install-recommends --no-upgrade -o DPkg::Lock::Timeout=120 -o Dpkg::Options::=--force-confdef -o Dpkg::Options::=--force-confold";
const FE: &str = "DEBIAN_FRONTEND=noninteractive /usr/bin/apt-get";
const APT_LIST: &str = "xvfb icewm fonts-dejavu-core";

fn apt_update() -> String {
    format!("{FE} update -q -o DPkg::Lock::Timeout=120")
}
fn apt_sim() -> String {
    format!("{FE} install -s {APT_OPTS} {APT_LIST}")
}
fn apt_install() -> String {
    format!("{FE} install -y {APT_OPTS} {APT_LIST}")
}

/// Installing through `prog` puts Xvfb and IceWM's session in place, like
/// the real packages do (only the real install, not a simulation).
fn installs_the_desktop(t: &T, prog: &str) {
    let root = t.root.clone();
    t.exec.0.borrow_mut().effects.push((
        prog.into(),
        Rc::new(move |args: &[String]| {
            let sim = args.iter().any(|a| a == "-s" || a == "--print");
            let installing = args.iter().any(|a| a == "install" || a == "-S");
            if installing && !sim {
                for abs in [XVFB, ICEWM_SESSION] {
                    let p = crate::ctx::under(&root, abs);
                    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                    std::fs::write(&p, "#!fake\n").unwrap();
                }
            }
        }),
    ));
}

const SIM_OK: &str = "Inst x11-xkb-utils (7.7+8build2 Ubuntu:24.04/noble [amd64])\n\
                      Inst xserver-common (2:21.1.12-1ubuntu1.4 Ubuntu:24.04/noble-updates [all])\n\
                      Inst xvfb (2:21.1.12-1ubuntu1.4 Ubuntu:24.04/noble-updates [amd64])\n\
                      Inst icewm (3.4.5-1build3 Ubuntu:24.04/noble [amd64])\n\
                      Conf xvfb (2:21.1.12-1ubuntu1.4 Ubuntu:24.04/noble-updates [amd64])\n";

#[test]
fn desktop_packages_install_takes_no_package_names() {
    assert_eq!(Verb::parse(ARGV), Ok(Verb::DesktopPackagesInstall));
    for extra in [
        &["desktop", "packages", "install", "xterm"][..],
        &["desktop", "packages", "install", "icewm"],
        &["desktop", "packages", "install", "--", "xvfb"],
        &["desktop", "packages", "install", "-o", "APT::Get::Remove=true"],
        &["desktop", "packages", "remove"],
        &["desktop", "packages"],
    ] {
        assert!(Verb::parse(extra).is_err(), "{extra:?} must not parse");
        let t = box_root();
        bin(&t, APT_GET);
        let o = t.call(extra);
        assert_eq!(o.code, exit::ARGV, "{extra:?}: {}", o.stderr);
        assert!(t.calls().is_empty(), "{extra:?} ran {:?}", t.calls());
    }
}

#[test]
fn desktop_packages_install_is_the_daemons_call_only() {
    assert_eq!(Verb::DesktopPackagesInstall.class(), Class::Daemon);
    assert_eq!(Verb::DesktopPackagesInstall.namespace(), "desktop");
    assert!(Verb::DesktopPackagesInstall.takes_lock());
    assert_eq!(Verb::DesktopPackagesInstall.stdin_cap(), None);
    assert!(k2_root_proto::verbs::catalog().contains(&Verb::DesktopPackagesInstall));
    // An agent (any caller but the daemon, which checks for an Owner's
    // yes) is refused before anything runs.
    let t = box_root();
    bin(&t, APT_GET);
    t.as_agent();
    let o = t.call(ARGV);
    assert_eq!(o.code, exit::NOT_DAEMON, "{}", o.stderr);
    assert!(t.non_query_calls().is_empty(), "{:?}", t.non_query_calls());
}

#[test]
fn desktop_packages_install_on_apt_simulates_then_installs_only_the_list() {
    let t = box_root();
    bin(&t, APT_GET);
    rule(&t, ENV, "-s", 0, SIM_OK);
    installs_the_desktop(&t, ENV);
    let o = t.call(ARGV);
    ok(&o);
    assert_eq!(calls_of(&t, ENV), vec![apt_update(), apt_sim(), apt_install()]);
    // Nothing else ran as root: no other package manager, no removal.
    assert!(calls_of(&t, DNF).is_empty() && calls_of(&t, PACMAN).is_empty());
    let v = json(&o);
    assert_eq!(v["packageManager"], "apt");
    assert_eq!(v["requested"], serde_json::json!(["xvfb", "icewm", "fonts-dejavu-core"]));
    assert_eq!(v["added"], serde_json::json!(["x11-xkb-utils", "xserver-common", "xvfb", "icewm"]));
    assert_eq!(v["xvfb"], true);
    assert_eq!(v["windowManager"], "icewm");
    // The audit line says which fixed packages went in.
    let audit = t.audit.0.borrow();
    let line = audit.last().unwrap();
    assert!(line.contains("\"verb\":\"desktop packages install\""), "{line}");
    assert!(line.contains("\"packages\":[\"xvfb\",\"icewm\",\"fonts-dejavu-core\"]"), "{line}");
}

#[test]
fn desktop_packages_install_refuses_a_simulation_that_removes_anything() {
    let t = box_root();
    bin(&t, APT_GET);
    let sim = format!("{SIM_OK}Remv xserver-xephyr:amd64 [2:21.1.12-1ubuntu1.4]\n");
    rule(&t, ENV, "-s", 0, &sim);
    installs_the_desktop(&t, ENV);
    let o = t.call(ARGV);
    assert_eq!(o.code, exit::STATE, "{}", o.stderr);
    assert!(o.stderr.starts_with("apt_would_remove:"), "{}", o.stderr);
    assert!(o.stderr.contains("xserver-xephyr"), "{}", o.stderr);
    assert!(o.stderr.contains("nothing changed"), "{}", o.stderr);
    // The simulation ran; the install never did.
    assert_eq!(calls_of(&t, ENV), vec![apt_update(), apt_sim()]);
    assert!(!t.p(XVFB).exists());
}

#[test]
fn desktop_packages_install_with_no_known_package_manager_installs_nothing_and_says_how() {
    let t = box_root();
    let o = t.call(ARGV);
    assert_eq!(o.code, exit::STATE, "{}", o.stderr);
    assert!(o.stderr.starts_with("no_package_manager:"), "{}", o.stderr);
    assert!(o.stderr.contains("nothing was installed"), "{}", o.stderr);
    assert!(o.stderr.contains("Install Xvfb, IceWM and a DejaVu font"), "{}", o.stderr);
    assert!(t.non_query_calls().is_empty(), "{:?}", t.non_query_calls());
}

#[test]
fn desktop_packages_install_on_pacman_and_dnf_uses_their_fixed_lists() {
    let t = box_root();
    bin(&t, PACMAN);
    rule(&t, PACMAN, "--print", 0, "imlib2\nicewm\nttf-dejavu\n");
    installs_the_desktop(&t, PACMAN);
    let o = t.call(ARGV);
    ok(&o);
    assert_eq!(
        calls_of(&t, PACMAN),
        vec![
            "-S --needed --print --print-format %n xorg-server-xvfb icewm ttf-dejavu".to_string(),
            "-S --needed --noconfirm xorg-server-xvfb icewm ttf-dejavu".to_string(),
        ]
    );
    let v = json(&o);
    assert_eq!(v["packageManager"], "pacman");
    assert_eq!(v["added"], serde_json::json!(["imlib2", "icewm", "ttf-dejavu"]));

    let t = box_root();
    bin(&t, DNF);
    installs_the_desktop(&t, DNF);
    let o = t.call(ARGV);
    ok(&o);
    // dnf never removes without --allowerasing, which K2 never passes.
    assert_eq!(
        calls_of(&t, DNF),
        vec!["install -y -q --setopt=install_weak_deps=False xorg-x11-server-Xvfb icewm dejavu-sans-fonts"]
    );
    assert!(calls_of(&t, DNF).iter().all(|c| !c.contains("allowerasing")));
}

#[test]
fn desktop_packages_install_refuses_a_nearly_full_disk_before_apt() {
    let mut t = box_root();
    bin(&t, APT_GET);
    t.ctx.free_bytes = |_| Ok(k2_root_proto::desktop::DESKTOP_MIN_FREE_BYTES - 1);
    let o = t.call(ARGV);
    assert_eq!(o.code, exit::STATE, "{}", o.stderr);
    assert!(o.stderr.starts_with("low_disk:"), "{}", o.stderr);
    assert!(calls_of(&t, ENV).is_empty(), "{:?}", calls_of(&t, ENV));
}

#[test]
fn desktop_packages_install_fails_when_xvfb_is_still_missing_afterwards() {
    let t = box_root();
    bin(&t, APT_GET);
    rule(&t, ENV, "-s", 0, SIM_OK);
    // apt answers 0 but nothing lands (a held or diverted package).
    let o = t.call(ARGV);
    assert_eq!(o.code, exit::STATE, "{}", o.stderr);
    assert!(o.stderr.starts_with("xvfb_missing:"), "{}", o.stderr);
}

#[test]
fn desktop_packages_install_reports_openbox_when_only_it_is_there() {
    let t = box_root();
    bin(&t, APT_GET);
    bin(&t, XVFB);
    bin(&t, k2_root_proto::desktop::OPENBOX);
    rule(&t, ENV, "-s", 0, "");
    // Everything was already there: apt adds nothing; IceWM's package is
    // listed but a box that has openbox alone still gets a desktop.
    let o = t.call(ARGV);
    ok(&o);
    let v = json(&o);
    assert_eq!(v["added"], serde_json::json!([]));
    assert_eq!(v["windowManager"], "openbox");
}

#[test]
fn helper_version_says_it_speaks_the_desktop_namespace() {
    let t = box_root();
    let o = t.call(&["helper", "version"]);
    ok(&o);
    let v = json(&o);
    assert_eq!(v["protocol"], 16);
    assert_eq!(v["namespaces"]["desktop"], true);
    assert_eq!(k2_root_proto::version::DESKTOP_MIN_PROTOCOL, 16);
}
