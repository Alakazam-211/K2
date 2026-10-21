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
fn install_writes_the_canned_pair_reloads_once_and_never_starts_the_socket() {
    let t = box_root();
    let v = json(&t.call(&["front", "install"]));
    assert_eq!(v["unit"], "k2-daemon.service");
    assert_eq!(v["changed"], true);
    let sock = std::fs::read_to_string(t.p(SOCKET)).unwrap();
    assert_eq!(sock, k2_root_proto::templates::front_socket_unit("k2-daemon.service").unwrap());
    assert!(sock.contains("\nListenStream=[::]:443\n") && sock.contains("\nService=k2-daemon.service\n"));
    assert_eq!(std::fs::read_to_string(t.p(DROPIN)).unwrap(), k2_root_proto::templates::front_dropin());
    for p in [SOCKET, DROPIN] {
        assert_eq!(std::fs::metadata(t.p(p)).unwrap().permissions().mode() & 0o777, 0o644, "{p}");
    }
    assert_eq!(count(&t, "daemon-reload"), 1);
    // Never started here: systemd refuses to start a socket whose service
    // (the live daemon, the caller) is active. It starts with the daemon's
    // restart. The daemon's unit is never restarted by the helper.
    assert_eq!(count(&t, "start k2-front-443.socket"), 0);
    assert_eq!(v["active"], false);
    // ...and schedules the daemon unit's one restart (systemd then starts
    // the socket first and hands the daemon its fd): the unit is
    // daemon.toml's, a few seconds out so the daemon answers first.
    assert_eq!(v["restartScheduled"], true);
    assert_eq!(v["restartInSecs"], 3);
    let run: Vec<String> = t.non_query_calls().into_iter().filter(|c| c.starts_with(SYSTEMD_RUN)).collect();
    assert_eq!(
        run,
        vec![format!(
            "{SYSTEMD_RUN} --unit=k2-front-restart --on-active=3 --timer-property=AccuracySec=100ms --collect --quiet {SYSTEMCTL} restart k2-daemon.service"
        )]
    );
    assert!(
        t.non_query_calls().iter().all(|c| !c.starts_with(&format!("{SYSTEMCTL} start")) && !c.starts_with(&format!("{SYSTEMCTL} restart"))),
        "{:?}",
        t.non_query_calls()
    );

    // Again: same bytes, no reload.
    let v = json(&t.call(&["front", "install"]));
    assert_eq!(v["changed"], false);
    assert_eq!(count(&t, "daemon-reload"), 1);
    assert_eq!(count(&t, "start k2-front-443.socket"), 0);

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

/// /proc/net/tcp6 with a LISTEN on [::]:443 (Stalwart's tls-alpn) and an
/// established connection on another port.
const TCP6_443: &str = "  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
   0: 00000000000000000000000000000000:01BB 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000   997        0 81234 1 0000000000000000 100 0 0 10 0\n\
   1: 0000000000000000FFFF00000100007F:1F90 0000000000000000FFFF00000100007F:D2A4 01 00000000:00000000 00:00000000 00000000  1001        0 81299 1 0000000000000000 20 4 30 10 -1\n";

#[test]
fn a_taken_port_443_is_refused_before_anything_is_written_and_names_the_holder() {
    let t = box_root();
    std::fs::create_dir_all(t.p("/proc/net")).unwrap();
    std::fs::write(t.p("/proc/net/tcp6"), TCP6_443).unwrap();
    std::fs::create_dir_all(t.p("/proc/3953928/fd")).unwrap();
    std::fs::write(t.p("/proc/3953928/comm"), "stalwart\n").unwrap();
    symlink("socket:[81234]", t.p("/proc/3953928/fd/44")).unwrap();
    let o = t.call(&["front", "install"]);
    assert_eq!(o.code, exit::STATE, "{}", o.stderr);
    assert!(o.stderr.contains("port 443 is already in use on this box by [::]:443 (stalwart[3953928])"), "{}", o.stderr);
    assert!(o.stderr.contains("--mail"), "{}", o.stderr);
    assert!(!t.p(SOCKET).exists() && !t.p(DROPIN).exists(), "nothing written");
    assert_eq!(count(&t, "daemon-reload"), 0);

    // Our own socket already holding :443 (a re-run) is not "taken".
    t.exec.0.borrow_mut().rules.push((
        SYSTEMCTL.into(),
        "is-active".into(),
        crate::ctx::ExecOut { status: 0, stdout: b"active\n".to_vec(), stderr: vec![] },
    ));
    let v = json(&t.call(&["front", "install"]));
    assert_eq!(v["active"], true);
}

#[test]
fn proc_net_listeners_parse_both_families() {
    let six = crate::front::listeners_on(TCP6_443, 443);
    assert_eq!(six, vec![("[::]:443".to_string(), 81234)]);
    let four = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
    0: 0100007F:01BB 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 555 1 0 100 0 0 10 0\n\
    1: 00000000:0050 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 556 1 0 100 0 0 10 0\n";
    assert_eq!(crate::front::listeners_on(four, 443), vec![("127.0.0.1:443".to_string(), 555)]);
    assert!(crate::front::listeners_on(four, 8443).is_empty());
    assert!(crate::front::listeners_on("garbage\nnot a table", 443).is_empty());
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
    // systemd's own words for the socket (why it did or didn't start).
    assert!(v["unit"]["journal"].is_array(), "{v}");
    assert!(t.calls().iter().any(|(p, a, _)| p == JOURNALCTL && a.iter().any(|x| x == "k2-front-443.socket")));
    std::fs::write(t.p(DROPIN), "[Service]\nAmbientCapabilities=CAP_NET_BIND_SERVICE\n").unwrap();
    assert_eq!(json(&t.call(&["front", "status"]))["dropins"][0]["dropin"], "other");
}

#[test]
fn version_announces_the_front_namespace_on_protocol_8() {
    let t = box_root();
    let v = json(&t.call(&["helper", "version"]));
    assert_eq!(v["protocol"], 9, "front came in 8; 9 is 0.46.3's one bump");
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
