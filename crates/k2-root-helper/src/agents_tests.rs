//! `agents` namespace (protocol 18, Agent Isolation M1b) over the temp root
//! and fakes of `tests.rs`: `agents place` moves exactly the process the
//! daemon started, and refuses one whose start time differs (a pid reused
//! since the spawn, review B5). `real_owners` is off here (no root on the
//! test runner), so the ownership checks are the kernel test's.

use super::*;

const UNIT: &str = "/system.slice/k2-daemon.service";
const WS: &str = "ws-agents-test";
const EXEC_PID: i32 = 7000;
const START: u64 = 123_456_789;

/// The daemon in its unit, the agents' leaf, and the exec helper (pid 7000)
/// waiting in the leaf with start time `START`.
fn agents_box() -> T {
    let t = box_root();
    mkdirs(&t.root, &[&format!("/proc/{DAEMON_PID}"), &format!("/proc/{EXEC_PID}"), &format!("/sys/fs/cgroup{UNIT}/agents")]);
    std::fs::write(t.p(&format!("/proc/{DAEMON_PID}/cgroup")), format!("0::{UNIT}/daemon\n")).unwrap();
    std::fs::write(t.p(&format!("/sys/fs/cgroup{UNIT}/agents/cgroup.procs")), "").unwrap();
    let stat = format!("{EXEC_PID} (k2-daemon) S {}{START} 0 0\n", "1 ".repeat(18));
    std::fs::write(t.p(&format!("/proc/{EXEC_PID}/stat")), stat).unwrap();
    std::fs::write(t.p(&format!("/proc/{EXEC_PID}/cmdline")), b"/usr/local/bin/k2-daemon\0--agent-exec\0").unwrap();
    std::fs::write(t.p(&format!("/proc/{EXEC_PID}/cgroup")), format!("0::{UNIT}/agents\n")).unwrap();
    t.procs.0.borrow_mut().procs.insert(
        EXEC_PID,
        P { exe: "/usr/local/bin/k2-daemon".into(), exe_owner: K2_UID, ppid: DAEMON_PID, ruid: K2_UID, sid: EXEC_PID },
    );
    t
}

fn agent_cgroup(t: &T) -> PathBuf {
    t.p(&format!("/sys/fs/cgroup{UNIT}/agents/a-{WS}"))
}

#[test]
fn agents_place_moves_the_process_with_the_start_time_the_daemon_read() {
    let t = agents_box();
    let o = t.call(&["agents", "place", WS, &EXEC_PID.to_string(), &START.to_string()]);
    ok(&o);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["cgroup"], format!("{UNIT}/agents/a-{WS}"), "{v}");
    let procs = std::fs::read_to_string(agent_cgroup(&t).join("cgroup.procs")).expect("the move wrote the pid");
    assert_eq!(procs.trim(), EXEC_PID.to_string());
}

#[test]
fn agents_place_refuses_a_process_whose_start_time_differs() {
    let t = agents_box();
    let o = t.call(&["agents", "place", WS, &EXEC_PID.to_string(), &(START + 1).to_string()]);
    assert_ne!(o.code, exit::OK, "a reused pid must not be placed: {}", o.stderr);
    assert!(o.stderr.contains("start time differs"), "says why: {}", o.stderr);
    assert!(!agent_cgroup(&t).exists(), "nothing made or moved for a refused place");
}
