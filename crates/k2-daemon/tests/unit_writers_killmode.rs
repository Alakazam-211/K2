//! k2-ptyd P87: every writer of the daemon's systemd unit sets
//! `KillMode=process`, so a restart, update or crash never kills k2-ptyd
//! and its agents with the unit's cgroup (with `control-group` the daemon
//! keeps sessions in-process: `ptyd: off (killmode_control-group)`). Pins
//! each writer, so a new one (or a reverted line) fails here.

use std::path::Path;

const WRITERS: &[&str] = &[
    "scripts/install-daemon.sh",
    "scripts/provision-k2-server.sh",
    "scripts/bootstrap-k2-dedicated.sh",
    "scripts/package-daemon-deb.sh",
    "packaging/arch/k2-daemon.service",
    "cli/k2",
];

#[test]
fn every_daemon_unit_writer_sets_killmode_process() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for w in WRITERS {
        let src = std::fs::read_to_string(root.join(w)).unwrap_or_else(|e| panic!("read {w}: {e}"));
        let modes: Vec<&str> = src.lines().filter_map(|l| l.trim().strip_prefix("KillMode=")).collect();
        assert_eq!(modes, vec!["process"], "{w}: the daemon unit must set exactly KillMode=process (P87), found {modes:?}");
        // The k2-ptyd and runtime-dir lines live side by side in each unit.
        assert!(src.contains("OOMPolicy=continue"), "{w}: OOMPolicy=continue missing");
    }
}

/// Every daemon unit writer carries the ExecStop that waits for the main
/// process, exactly as the daemon checks for it before a hand-off
/// (`handoff::EXEC_STOP_WAIT_MAINPID`). Without it a hand-off successor
/// (not systemd's child) is SIGKILLed at stop/restart, so a stop would
/// leave k2-ptyd agents running; the daemon then refuses hand-offs and a
/// reload is a plain restart.
#[test]
fn every_daemon_unit_writer_waits_for_the_main_process_at_stop() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let want = k2_daemon::handoff::EXEC_STOP_WAIT_MAINPID;
    assert!(k2_daemon::handoff::exec_stop_waits_for_main(want));
    // The unquoted shell heredocs escape `$` (the file gets `$$MAINPID`).
    let want_escaped = want.replace('$', "\\$");
    for w in WRITERS {
        let src = std::fs::read_to_string(root.join(w)).unwrap_or_else(|e| panic!("read {w}: {e}"));
        let stops: Vec<&str> = src.lines().filter(|l| l.trim_start().starts_with("ExecStop=")).collect();
        assert_eq!(stops.len(), 1, "{w}: exactly one ExecStop in the daemon unit, found {stops:?}");
        assert!(stops[0] == want || stops[0] == want_escaped, "{w}: ExecStop differs from the daemon's:\n  {}\n  {want}", stops[0]);
    }
    // What `systemctl show -p ExecStop` prints for it.
    assert!(k2_daemon::handoff::exec_stop_waits_for_main(
        "{ path=/bin/sh ; argv[]=/bin/sh -c kill -TERM \"$MAINPID\" 2>/dev/null; while kill -0 \"$MAINPID\" 2>/dev/null; do sleep 0.2; done k2-wait-mainpid ; ignore_errors=no ; start_time=[n/a] ; stop_time=[n/a] ; pid=0 ; code=(null) ; status=0/0 }"
    ));
    assert!(!k2_daemon::handoff::exec_stop_waits_for_main(""), "no ExecStop");
}
