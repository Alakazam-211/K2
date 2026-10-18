//! `front` namespace (protocol 8, front443) over the temp root and fakes
//! of `tests.rs`. Nothing runs the real systemctl.

use super::*;

const SOCKET: &str = "/etc/systemd/system/k2-front-443.socket";
const DROPIN: &str = "/etc/systemd/system/k2-daemon.service.d/k2-front.conf";

fn json(o: &crate::Outcome) -> serde_json::Value {
    ok(o);
    serde_json::from_slice(&o.stdout).unwrap()
}

fn count(t: &T, cmd: &str) -> usize {
    t.non_query_calls().iter().filter(|c| c.as_str() == format!("{SYSTEMCTL} {cmd}")).count()
}

#[test]
fn install_writes_the_canned_pair_reloads_once_and_starts_the_socket() {
    let t = box_root();
    let v = json(&t.call(&["front", "install"]));
    assert_eq!(v["unit"], "k2-daemon.service");
    assert_eq!(v["changed"], true);
    assert_eq!(v["active"], true);
    let sock = std::fs::read_to_string(t.p(SOCKET)).unwrap();
    assert_eq!(sock, k2_root_proto::templates::front_socket_unit("k2-daemon.service").unwrap());
    assert!(sock.contains("\nListenStream=[::]:443\n") && sock.contains("\nService=k2-daemon.service\n"));
    assert_eq!(std::fs::read_to_string(t.p(DROPIN)).unwrap(), k2_root_proto::templates::front_dropin());
    for p in [SOCKET, DROPIN] {
        assert_eq!(std::fs::metadata(t.p(p)).unwrap().permissions().mode() & 0o777, 0o644, "{p}");
    }
    assert_eq!(count(&t, "daemon-reload"), 1);
    assert_eq!(count(&t, "start k2-front-443.socket"), 1);
    // The daemon's unit is never restarted by the helper (the daemon does
    // its own supervisor restart), and nothing else is started.
    assert!(
        t.non_query_calls().iter().all(|c| !c.contains("restart") && !c.contains("k2-daemon.service")),
        "{:?}",
        t.non_query_calls()
    );

    // Again: same bytes, no reload; the socket is asserted started again.
    let v = json(&t.call(&["front", "install"]));
    assert_eq!(v["changed"], false);
    assert_eq!(count(&t, "daemon-reload"), 1);
    assert_eq!(count(&t, "start k2-front-443.socket"), 2);

    // A doctored socket unit is put back.
    std::fs::write(t.p(SOCKET), "[Socket]\nListenStream=0.0.0.0:22\nService=k2-daemon.service\n").unwrap();
    assert_eq!(json(&t.call(&["front", "install"]))["changed"], true);
    assert_eq!(
        std::fs::read_to_string(t.p(SOCKET)).unwrap(),
        k2_root_proto::templates::front_socket_unit("k2-daemon.service").unwrap()
    );

    let line = t.audit.0.borrow().last().unwrap().clone();
    let a: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(a["ns"], "front");
    assert_eq!(a["verb"], "front install");
    assert_eq!(a["class"], "daemon");
    assert_eq!(a["result"], "ok");
}

#[test]
fn a_caller_that_is_not_the_daemon_gets_77_and_nothing_changes() {
    let t = box_root();
    t.as_agent();
    for verb in ["install", "remove"] {
        let o = t.call(&["front", verb]);
        assert_eq!(o.code, exit::NOT_DAEMON, "{verb}: {}", o.stderr);
        let line = t.audit.0.borrow().last().unwrap().clone();
        assert!(line.contains("caller_not_daemon"), "{line}");
    }
    assert!(!t.p(SOCKET).exists() && !t.p(DROPIN).exists());
    assert!(t.non_query_calls().is_empty(), "{:?}", t.non_query_calls());
    // Status is open (read-only).
    let v = json(&t.call(&["front", "status"]));
    assert_eq!(v["socketFile"], "missing");
}

#[test]
fn a_socket_that_cannot_listen_is_rolled_back() {
    let t = box_root();
    t.exec.0.borrow_mut().rules.push((
        SYSTEMCTL.into(),
        "start".into(),
        crate::ctx::ExecOut {
            status: 1,
            stdout: vec![],
            stderr: b"Job for k2-front-443.socket failed (Address already in use)".to_vec(),
        },
    ));
    let o = t.call(&["front", "install"]);
    assert_eq!(o.code, exit::STATE, "{}", o.stderr);
    assert!(o.stderr.contains(":443 could not be taken"), "{}", o.stderr);
    assert!(!t.p(SOCKET).exists(), "socket unit rolled back");
    assert!(!t.p(DROPIN).exists(), "drop-in rolled back");
    assert_eq!(count(&t, "daemon-reload"), 2, "once to load, once after the rollback");
}

#[test]
fn another_daemons_front_is_never_taken_over() {
    let t = box_root();
    let toml = "[[daemon]]\nuser = \"k2\"\nunit = \"k2-daemon.service\"\nscope = \"system\"\n\
                [[daemon]]\nuser = \"k2b\"\nunit = \"k2-daemon-b.service\"\nscope = \"system\"\n";
    std::fs::write(t.p(DAEMON_TOML), toml).unwrap();
    let theirs = k2_root_proto::templates::front_socket_unit("k2-daemon-b.service").unwrap();
    std::fs::write(t.p(SOCKET), &theirs).unwrap();
    for verb in ["install", "remove"] {
        let o = t.call(&["front", verb]);
        assert_eq!(o.code, exit::STATE, "{verb}: {}", o.stderr);
        assert!(o.stderr.contains("another K2 daemon"), "{}", o.stderr);
    }
    assert_eq!(std::fs::read_to_string(t.p(SOCKET)).unwrap(), theirs);
    assert!(!t.p(DROPIN).exists());
    let v = json(&t.call(&["front", "status"]));
    assert_eq!(v["socketFile"], "canned");
    assert_eq!(v["heldFor"], "k2-daemon-b.service");

    // A leftover naming a unit no configured daemon has is replaced.
    let stale = k2_root_proto::templates::front_socket_unit("k2-gone.service").unwrap();
    std::fs::write(t.p(SOCKET), stale).unwrap();
    ok(&t.call(&["front", "install"]));
    assert!(std::fs::read_to_string(t.p(SOCKET)).unwrap().contains("\nService=k2-daemon.service\n"));
}

#[test]
fn links_and_odd_files_are_refused_with_nothing_written_through_them() {
    let t = box_root();
    symlink(t.p("/etc/shadow"), t.p(SOCKET)).unwrap();
    let o = t.call(&["front", "install"]);
    assert_eq!(o.code, exit::STATE, "{}", o.stderr);
    assert!(!t.p("/etc/shadow").exists(), "never written through the link");
    assert!(!t.p(DROPIN).exists());
    std::fs::remove_file(t.p(SOCKET)).unwrap();

    // A symlinked drop-in dir: refused, and the socket unit it wrote first
    // is taken back out.
    let elsewhere = t.p("/var/lib/elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    symlink(&elsewhere, t.p("/etc/systemd/system/k2-daemon.service.d")).unwrap();
    let o = t.call(&["front", "install"]);
    assert_eq!(o.code, exit::STATE, "{}", o.stderr);
    assert!(std::fs::read_dir(&elsewhere).unwrap().next().is_none());
    assert!(!t.p(SOCKET).exists(), "no half install");
    assert_eq!(count(&t, "start k2-front-443.socket"), 0);

    // A directory where the socket unit should be.
    std::fs::remove_file(t.p("/etc/systemd/system/k2-daemon.service.d")).unwrap();
    std::fs::create_dir_all(t.p(SOCKET)).unwrap();
    for verb in ["install", "remove"] {
        assert_eq!(t.call(&["front", verb]).code, exit::STATE, "{verb}");
    }
    assert_eq!(json(&t.call(&["front", "status"]))["socketFile"], "not-a-file");
}

#[test]
fn a_user_scope_daemon_is_refused() {
    let t = box_root();
    let toml = "[[daemon]]\nuser = \"k2\"\nunit = \"k2-daemon.service\"\nscope = \"user\"\n";
    std::fs::write(t.p(DAEMON_TOML), toml).unwrap();
    let o = t.call(&["front", "install"]);
    assert_eq!(o.code, exit::STATE, "{}", o.stderr);
    assert!(o.stderr.contains("user-scope"), "{}", o.stderr);
    assert!(!t.p(SOCKET).exists());
}

#[test]
fn remove_stops_and_removes_both_files_once() {
    let t = box_root();
    ok(&t.call(&["front", "install"]));
    let v = json(&t.call(&["front", "remove"]));
    assert_eq!(v["changed"], true);
    assert!(!t.p(SOCKET).exists() && !t.p(DROPIN).exists());
    assert_eq!(count(&t, "stop k2-front-443.socket"), 1);
    assert_eq!(count(&t, "daemon-reload"), 2);
    // Nothing left: no stop, no reload.
    let v = json(&t.call(&["front", "remove"]));
    assert_eq!(v["changed"], false);
    assert_eq!(count(&t, "stop k2-front-443.socket"), 1);
    assert_eq!(count(&t, "daemon-reload"), 2);
}

#[test]
fn status_reports_files_and_the_socket_state() {
    let t = box_root();
    t.exec.0.borrow_mut().rules.push((
        SYSTEMCTL.into(),
        "is-active".into(),
        crate::ctx::ExecOut { status: 0, stdout: b"active\n".to_vec(), stderr: vec![] },
    ));
    ok(&t.call(&["front", "install"]));
    let v = json(&t.call(&["front", "status"]));
    assert_eq!(v["socketFile"], "canned");
    assert_eq!(v["heldFor"], "k2-daemon.service");
    assert_eq!(v["active"], "active");
    assert_eq!(v["dropins"][0]["unit"], "k2-daemon.service");
    assert_eq!(v["dropins"][0]["dropin"], "canned");
    std::fs::write(t.p(DROPIN), "[Service]\nAmbientCapabilities=CAP_NET_BIND_SERVICE\n").unwrap();
    assert_eq!(json(&t.call(&["front", "status"]))["dropins"][0]["dropin"], "other");
}

#[test]
fn version_announces_the_front_namespace_on_protocol_8() {
    let t = box_root();
    let v = json(&t.call(&["helper", "version"]));
    assert_eq!(v["protocol"], 8);
    assert_eq!(v["namespaces"]["front"], true);
    assert_eq!(v["namespaces"]["sew"], true, "protocols are additive");
}

#[test]
fn argv_beyond_the_verb_is_refused_before_anything_runs() {
    let t = box_root();
    for bad in [
        &["front", "install", "k2-daemon.service"][..],
        &["front", "install", "8443"],
        &["front", "remove", "/etc/systemd/system/k2-front-443.socket"],
        &["front", "status", "--json"],
    ] {
        let o = t.call(bad);
        assert_eq!(o.code, exit::ARGV, "{bad:?}");
    }
    assert!(t.non_query_calls().is_empty());
    assert!(!t.p(SOCKET).exists());
}

#[test]
fn another_daemons_leftover_drop_in_blocks_install() {
    let t = box_root();
    let toml = "[[daemon]]\nuser = \"k2\"\nunit = \"k2-daemon.service\"\nscope = \"system\"\n\
                [[daemon]]\nuser = \"k2b\"\nunit = \"k2-daemon-b.service\"\nscope = \"system\"\n";
    std::fs::write(t.p(DAEMON_TOML), toml).unwrap();
    std::fs::create_dir_all(t.p("/etc/systemd/system/k2-daemon-b.service.d")).unwrap();
    std::fs::write(t.p("/etc/systemd/system/k2-daemon-b.service.d/k2-front.conf"), k2_root_proto::templates::front_dropin()).unwrap();
    let o = t.call(&["front", "install"]);
    assert_eq!(o.code, exit::STATE, "{}", o.stderr);
    assert!(o.stderr.contains("another K2 daemon"), "{}", o.stderr);
    assert!(!t.p(SOCKET).exists() && !t.p(DROPIN).exists());
}

#[test]
fn a_failed_reload_takes_the_new_files_back_out() {
    let t = box_root();
    t.exec.0.borrow_mut().rules.push((
        SYSTEMCTL.into(),
        "daemon-reload".into(),
        crate::ctx::ExecOut { status: 1, stdout: vec![], stderr: b"Failed to reload daemon".to_vec() },
    ));
    let o = t.call(&["front", "install"]);
    assert_ne!(o.code, exit::OK);
    assert!(!t.p(SOCKET).exists() && !t.p(DROPIN).exists(), "nothing left half-installed");
    assert_eq!(count(&t, "start k2-front-443.socket"), 0);
}
