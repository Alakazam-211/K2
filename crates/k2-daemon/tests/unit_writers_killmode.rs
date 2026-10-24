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
