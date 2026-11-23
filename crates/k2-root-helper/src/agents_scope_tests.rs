//! `agents` namespace, protocol 19 (Agent Isolation M2 B4) over the temp
//! root and test doubles of `tests.rs`: the exact `StartTransientUnit` argv, the
//! refusals before it, adopt, stop, stop-all, layout's P1 verdict, setup
//! and teardown. `real_owners` is off here (no root on the test runner), so
//! the ownership checks are the kernel rows'.

use super::*;
use k2_root_proto::agent_scope::{anchor_unit, slice_unit, Level};
use k2_root_proto::apps::ENV;
use k2_root_proto::creds::BUSCTL;

const UNIT: &str = "/system.slice/k2-daemon.service";
const WS: &str = "ws-scope-test";
const SID: &str = "0123456789abcdef";
const EXEC_PID: i32 = 7000;
const START: u64 = 123_456_789;
const SCOPE: &str = "k2-agent-ws-scope-test:0123456789abcdef.scope";
const OPEN_DIR: &str = "/sys/fs/cgroup/k2agents.slice/k2agents-open.slice";
const FENCED_DIR: &str = "/sys/fs/cgroup/k2agents.slice/k2agents-fenced.slice";

fn exec_proc(t: &T, pid: i32, ruid: u32, cgroup: &str) {
    mkdirs(&t.root, &[&format!("/proc/{pid}")]);
    let stat = format!("{pid} (k2-daemon) S {}{START} 0 0\n", "1 ".repeat(18));
    std::fs::write(t.p(&format!("/proc/{pid}/stat")), stat).unwrap();
    std::fs::write(t.p(&format!("/proc/{pid}/cmdline")), b"/usr/local/bin/k2-daemon\0--agent-exec\0").unwrap();
    std::fs::write(t.p(&format!("/proc/{pid}/cgroup")), format!("0::{cgroup}\n")).unwrap();
    t.procs.0.borrow_mut().procs.insert(pid, P { exe: "/usr/local/bin/k2-daemon".into(), exe_owner: K2_UID, ppid: DAEMON_PID, ruid, sid: pid });
}

/// The daemon in its (not delegated) unit, the slices set up, the exec
/// helper (pid 7000) waiting in the daemon's leaf, and a systemd that moves
/// the pids it is given (`moves`) or doesn't.
fn scope_box(moves: bool) -> T {
    scope_box_with(moves, &[])
}

/// As [`scope_box`]; a pid in `reused` is another process by the time
/// systemd moves it (its start time changes with the move).
fn scope_box_with(moves: bool, reused: &[u32]) -> T {
    scope_box_full(moves, reused, &[])
}

/// As [`scope_box_with`]; `extra` pids are listed in every new scope too
/// (another process systemd put there).
fn scope_box_full(moves: bool, reused: &[u32], extra: &[u32]) -> T {
    let reused: Vec<String> = reused.iter().map(u32::to_string).collect();
    let extra: Vec<String> = extra.iter().map(u32::to_string).collect();
    let t = box_root();
    mkdirs(&t.root, &[&format!("/proc/{DAEMON_PID}"), &format!("/sys/fs/cgroup{UNIT}/daemon"), OPEN_DIR, FENCED_DIR]);
    std::fs::write(t.p(&format!("/proc/{DAEMON_PID}/cgroup")), format!("0::{UNIT}/daemon\n")).unwrap();
    exec_proc(&t, EXEC_PID, K2_UID, &format!("{UNIT}/daemon"));
    let root = t.root.clone();
    let busctl: Program = Rc::new(move |args: &[String]| {
        let has = |w: &str| args.iter().any(|a| a == w);
        let out = |s: &str| ExecOut { status: 0, stdout: s.as_bytes().to_vec(), stderr: vec![] };
        if has("StartTransientUnit") {
            let (name, slice) = (&args[6], &args[11]);
            let i = args.iter().position(|a| a == "PIDs").expect("PIDs");
            let n: usize = args[i + 2].parse().expect("count");
            let rel = format!("/k2agents.slice/{slice}/{name}");
            std::fs::create_dir_all(under(&root, &format!("/sys/fs/cgroup{rel}"))).unwrap();
            let mut listed: Vec<String> = if moves { args[i + 3..i + 3 + n].to_vec() } else { vec![] };
            listed.extend(extra.iter().cloned());
            let procs: String = listed.iter().map(|p| format!("{p}\n")).collect();
            std::fs::write(under(&root, &format!("/sys/fs/cgroup{rel}/cgroup.procs")), procs).unwrap();
            if moves {
                for p in &args[i + 3..i + 3 + n] {
                    let f = under(&root, &format!("/proc/{p}/cgroup"));
                    if f.exists() {
                        std::fs::write(f, format!("0::{rel}\n")).unwrap();
                    }
                    if reused.contains(p) {
                        let stat = format!("{p} (other) S {}{} 0 0\n", "1 ".repeat(18), START + 7);
                        std::fs::write(under(&root, &format!("/proc/{p}/stat")), stat).unwrap();
                    }
                }
            }
            return out(r#"{"type":"o","data":["/org/freedesktop/systemd1/job/7"]}"#);
        }
        if has("LoadUnit") {
            return out(r#"{"type":"o","data":["/org/freedesktop/systemd1/unit/k2_2ddaemon_2eservice"]}"#);
        }
        if has("Delegate") {
            return out(r#"{"type":"b","data":true}"#);
        }
        if has("ReloadConfig") {
            return out("");
        }
        ExecOut { status: 1, stdout: vec![], stderr: b"unexpected busctl call".to_vec() }
    });
    t.exec.0.borrow_mut().programs.insert(BUSCTL.into(), busctl);
    t
}

/// systemd ends a stopped scope: `systemctl stop … <name>` removes that
/// scope's cgroup under either slice (and stopping `k2agents.slice`
/// removes the whole tree).
fn systemd_stops(t: &T) {
    let root = t.root.clone();
    let effect: Effect = Rc::new(move |args: &[String]| {
        if args.first().map(String::as_str) == Some("stop") {
            let name = args.last().cloned().unwrap_or_default();
            if name == "k2agents.slice" {
                let _ = std::fs::remove_dir_all(under(&root, "/sys/fs/cgroup/k2agents.slice"));
            }
            for d in [OPEN_DIR, FENCED_DIR] {
                let _ = std::fs::remove_dir(under(&root, &format!("{d}/{name}")));
            }
        }
    });
    t.exec.0.borrow_mut().effects.push((SYSTEMCTL.into(), effect));
}

fn sv(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn json_of(o: &crate::Outcome) -> serde_json::Value {
    serde_json::from_slice(&o.stdout).expect("json answer")
}

fn busctl_calls(t: &T) -> Vec<Vec<String>> {
    t.calls().into_iter().filter(|(p, _, _)| p == BUSCTL).map(|(_, a, _)| a).collect()
}

fn systemctl_calls(t: &T) -> Vec<Vec<String>> {
    t.calls().into_iter().filter(|(p, a, _)| p == SYSTEMCTL && !a.iter().any(|x| x == "MainPID")).map(|(_, a, _)| a).collect()
}

fn scope_argv<'a>(kind: &'a str, caps: &[&'a str]) -> Vec<String> {
    let (pid, start) = (EXEC_PID.to_string(), START.to_string());
    let mut v: Vec<String> = ["agents", "scope", WS, SID, &pid, &start, kind].iter().map(|s| s.to_string()).collect();
    v.extend(caps.iter().map(|s| s.to_string()));
    v
}

fn call_v(t: &T, v: &[String]) -> crate::Outcome {
    let refs: Vec<&str> = v.iter().map(String::as_str).collect();
    t.call(&refs)
}

#[test]
fn agents_scope_starts_one_transient_unit_with_exactly_these_properties() {
    let t = scope_box(true);
    let o = call_v(&t, &scope_argv("open", &[]));
    ok(&o);
    let v = json_of(&o);
    assert_eq!(v["scope"], SCOPE, "{v}");
    assert_eq!(v["cgroup"], format!("/k2agents.slice/k2agents-open.slice/{SCOPE}"), "{v}");
    assert_eq!(v["pid"], EXEC_PID, "{v}");
    let want: Vec<String> = [
        "call",
        "org.freedesktop.systemd1",
        "/org/freedesktop/systemd1",
        "org.freedesktop.systemd1.Manager",
        "StartTransientUnit",
        "ssa(sv)a(sa(sv))",
        SCOPE,
        "fail",
        "6",
        "Slice",
        "s",
        "k2agents-open.slice",
        "PIDs",
        "au",
        "1",
        "7000",
        "Delegate",
        "b",
        "false",
        "CollectMode",
        "s",
        "inactive-or-failed",
        "TimeoutStopUSec",
        "t",
        "10000000",
        "Description",
        "s",
        "K2 agent scope",
        "0",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(busctl_calls(&t), vec![want]);
    assert!(systemctl_calls(&t).is_empty(), "nothing stopped: {:?}", systemctl_calls(&t));
}

#[test]
fn agents_scope_caps_become_memory_max_and_cpu_quota_numbers() {
    let t = scope_box(true);
    let o = call_v(&t, &scope_argv("fenced", &["mem=512", "cpu=150"]));
    ok(&o);
    assert_eq!(json_of(&o)["cgroup"], format!("/k2agents.slice/k2agents-fenced.slice/{SCOPE}"));
    let calls = busctl_calls(&t);
    assert_eq!(calls.len(), 1, "{calls:?}");
    let a = &calls[0];
    assert_eq!(a[8], "8", "six fixed properties and the two caps: {a:?}");
    assert_eq!(a[11], "k2agents-fenced.slice");
    let tail: Vec<&str> = a[a.len() - 13..].iter().map(String::as_str).collect();
    assert_eq!(
        tail,
        ["TimeoutStopUSec", "t", "10000000", "Description", "s", "K2 agent scope", "MemoryMax", "t", "536870912", "CPUQuotaPerSecUSec", "t", "1500000", "0"]
    );
    for tok in a {
        assert!(!tok.contains("mem=") && !tok.contains("cpu=") && !tok.contains('%'), "no caller token passes through: {tok}");
    }
}

fn refused(t: &T, argv: &[String], why: &str) {
    let o = call_v(t, argv);
    assert_ne!(o.code, exit::OK, "must be refused: {argv:?}");
    assert!(o.stderr.contains(why), "says why ({why}): {}", o.stderr);
    assert!(busctl_calls(t).is_empty(), "systemd never asked: {:?}", busctl_calls(t));
}

#[test]
fn agents_scope_refuses_a_process_whose_start_time_differs() {
    let t = scope_box(true);
    let mut a = scope_argv("open", &[]);
    a[5] = (START + 1).to_string();
    refused(&t, &a, "start time differs");
}

#[test]
fn agents_scope_refuses_a_process_not_in_the_exec_helper() {
    let t = scope_box(true);
    std::fs::write(t.p(&format!("/proc/{EXEC_PID}/cmdline")), b"/bin/sh\0-c\0true\0").unwrap();
    refused(&t, &scope_argv("open", &[]), "not waiting in the exec helper");
}

#[test]
fn agents_scope_refuses_a_process_of_another_user() {
    let t = scope_box(true);
    t.procs.0.borrow_mut().procs.get_mut(&EXEC_PID).unwrap().ruid = 0;
    refused(&t, &scope_argv("open", &[]), "not the daemon user's");
}

#[test]
fn agents_scope_refuses_a_process_outside_the_daemons_unit() {
    for cg in [
        "/system.slice/other.service".to_string(),
        "/user.slice/user-1001.slice/session-3.scope".to_string(),
        format!("{UNIT}/agents/a-ws-other"),
        format!("/k2agents.slice/k2agents-open.slice/k2-agent-ws-other:{SID}.scope"),
    ] {
        let t = scope_box(true);
        std::fs::write(t.p(&format!("/proc/{EXEC_PID}/cgroup")), format!("0::{cg}\n")).unwrap();
        refused(&t, &scope_argv("open", &[]), "outside the daemon's unit");
    }
}

#[test]
fn agents_scope_takes_a_process_from_the_unit_itself_or_its_own_old_cgroup() {
    for cg in [UNIT.to_string(), format!("{UNIT}/agents/a-{WS}")] {
        let t = scope_box(true);
        std::fs::write(t.p(&format!("/proc/{EXEC_PID}/cgroup")), format!("0::{cg}\n")).unwrap();
        ok(&call_v(&t, &scope_argv("open", &[])));
    }
}

#[test]
fn agents_scope_refuses_when_the_slice_is_missing() {
    let t = scope_box(true);
    std::fs::remove_dir(t.p(FENCED_DIR)).unwrap();
    refused(&t, &scope_argv("fenced", &[]), "agents setup");
    ok(&call_v(&t, &scope_argv("open", &[])));
}

#[test]
fn agents_scope_is_the_daemons_alone() {
    let t = scope_box(true);
    t.as_agent();
    let o = call_v(&t, &scope_argv("open", &[]));
    assert_eq!(o.code, exit::NOT_DAEMON, "{}", o.stderr);
    assert!(busctl_calls(&t).is_empty());
}

#[test]
fn agents_scope_stops_the_scope_when_the_process_did_not_land_in_it() {
    let t = scope_box(false);
    let o = call_v(&t, &scope_argv("open", &[]));
    assert_ne!(o.code, exit::OK);
    assert!(o.stderr.contains("not in its scope"), "{}", o.stderr);
    assert_eq!(systemctl_calls(&t), vec![sv(&["stop", "--no-block", SCOPE])]);
}

/// Review R3: systemd got a number; the process in the scope is another
/// one by then.
#[test]
fn agents_scope_stops_the_scope_when_the_start_time_changed_during_the_move() {
    let t = scope_box_with(true, &[EXEC_PID as u32]);
    let o = call_v(&t, &scope_argv("open", &[]));
    assert_ne!(o.code, exit::OK);
    assert!(o.stderr.contains("start time changed during the move"), "{}", o.stderr);
    assert_eq!(systemctl_calls(&t), vec![sv(&["stop", "--no-block", SCOPE])]);
}

/// Review N1: the scope lists a process besides the one checked.
#[test]
fn agents_scope_refuses_when_the_scope_lists_another_process() {
    let t = scope_box_full(true, &[], &[7300]);
    let o = call_v(&t, &scope_argv("open", &[]));
    assert_ne!(o.code, exit::OK);
    assert!(o.stderr.contains("a process other than the one checked"), "{}", o.stderr);
    assert_eq!(systemctl_calls(&t), vec![sv(&["stop", "--no-block", SCOPE])]);
    let audit = t.audit.0.borrow();
    let line = audit.iter().find(|l| l.contains("scope_member_mismatch")).expect("a loud audit line");
    let v: serde_json::Value = serde_json::from_str(line).unwrap();
    assert_eq!((v["ws"].clone(), v["scope"].clone()), (serde_json::json!(WS), serde_json::json!(SCOPE)), "{v}");
    assert_eq!(v["pids"][0]["pid"], 7300, "{v}");
}

/// The old cgroup with `pids` listed and each one a process of `ruid`.
fn old_cgroup(t: &T, pids: &[i32], ruid: u32) -> String {
    let old = format!("{UNIT}/agents/a-{WS}");
    mkdirs(&t.root, &[&format!("/sys/fs/cgroup{old}")]);
    for p in pids {
        exec_proc(t, *p, ruid, &old);
    }
    let list: String = pids.iter().map(|p| format!("{p}\n")).collect();
    std::fs::write(t.p(&format!("/sys/fs/cgroup{old}/cgroup.procs")), list).unwrap();
    old
}

#[test]
fn agents_adopt_refuses_an_old_cgroup_that_is_not_roots() {
    let mut t = scope_box(true);
    old_cgroup(&t, &[7101, 7102], K2_UID);
    // The runner's files stand for root's in tests; any other uid is not root.
    t.ctx.root_uid = t.ctx.root_uid.wrapping_add(1);
    let o = t.call(&["agents", "adopt", WS, SID, "open"]);
    assert_ne!(o.code, exit::OK);
    assert!(o.stderr.contains("old layout, not adopted"), "{}", o.stderr);
    assert!(busctl_calls(&t).is_empty(), "nothing moved: {:?}", busctl_calls(&t));
}

#[test]
fn agents_adopt_refuses_the_daemons_own_process_or_ptyd_in_the_old_cgroup() {
    // The daemon's main pid listed.
    let t = scope_box(true);
    let old = old_cgroup(&t, &[7101], K2_UID);
    std::fs::write(t.p(&format!("/sys/fs/cgroup{old}/cgroup.procs")), format!("7101\n{DAEMON_PID}\n")).unwrap();
    let o = t.call(&["agents", "adopt", WS, SID, "open"]);
    assert!(o.code != exit::OK && o.stderr.contains("daemon's own process"), "{}", o.stderr);
    assert!(busctl_calls(&t).is_empty(), "{:?}", busctl_calls(&t));
    // A process the unit's daemon/ leaf lists.
    let t = scope_box(true);
    old_cgroup(&t, &[7101, 7102], K2_UID);
    std::fs::write(t.p(&format!("/sys/fs/cgroup{UNIT}/daemon/cgroup.procs")), "7102\n").unwrap();
    let o = t.call(&["agents", "adopt", WS, SID, "open"]);
    assert!(o.code != exit::OK && o.stderr.contains("nothing moved"), "{}", o.stderr);
    assert!(busctl_calls(&t).is_empty(), "{:?}", busctl_calls(&t));
    // ptyd, by its name.
    let t = scope_box(true);
    old_cgroup(&t, &[7101, 7102], K2_UID);
    std::fs::write(t.p("/proc/7102/comm"), "k2-ptyd\n").unwrap();
    let o = t.call(&["agents", "adopt", WS, SID, "open"]);
    assert!(o.code != exit::OK && o.stderr.contains("ptyd"), "{}", o.stderr);
    assert!(busctl_calls(&t).is_empty(), "{:?}", busctl_calls(&t));
}

/// Review N1: a held process dies before the move; its number comes back
/// as another process with a new start time, and systemd moves that one
/// into the scope. The check looks at the scope, so adopt refuses it.
#[test]
fn agents_adopt_refuses_a_number_that_died_and_came_back_as_another_process() {
    let t = scope_box_with(true, &[7102]);
    old_cgroup(&t, &[7101, 7102], K2_UID);
    // Gone before the move (its held handle reports it), back after it.
    t.procs.0.borrow_mut().dead.insert(7102);
    let o = t.call(&["agents", "adopt", WS, SID, "open"]);
    assert_ne!(o.code, exit::OK);
    assert!(o.stderr.contains("\"mismatched\":[7102]"), "{}", o.stderr);
    assert!(o.stderr.contains("surviving sessions in it end too"), "{}", o.stderr);
    assert_eq!(systemctl_calls(&t), vec![sv(&["stop", "--no-block", SCOPE])]);
    let audit = t.audit.0.borrow();
    let line = audit.iter().find(|l| l.contains("scope_member_mismatch")).expect("a loud audit line");
    let v: serde_json::Value = serde_json::from_str(line).unwrap();
    assert_eq!(v["pids"], serde_json::json!([{"pid": 7102, "startBefore": START, "startNow": START + 7}]), "{v}");
}

/// A child a survivor forked after the move is in the scope too: adopted.
#[test]
fn agents_adopt_accepts_a_child_of_a_checked_process() {
    // A grandchild: 7151 → 7150 → 7101 (checked), all in the scope.
    let t = scope_box_full(true, &[], &[7150, 7151]);
    old_cgroup(&t, &[7101], K2_UID);
    child_proc(&t, 7150, 7101);
    child_proc(&t, 7151, 7150);
    ok(&t.call(&["agents", "adopt", WS, SID, "open"]));
    let lines: Vec<serde_json::Value> = t
        .audit
        .0
        .borrow()
        .iter()
        .filter(|l| l.contains("scope_member_child_accepted"))
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 2, "one line per child: {lines:?}");
    assert_eq!((lines[0]["pid"].clone(), lines[0]["via"].clone()), (serde_json::json!(7150), serde_json::json!([7101])), "{lines:?}");
    assert_eq!((lines[1]["pid"].clone(), lines[1]["via"].clone()), (serde_json::json!(7151), serde_json::json!([7150, 7101])), "{lines:?}");
    assert_eq!((lines[1]["ws"].clone(), lines[1]["scope"].clone()), (serde_json::json!(WS), serde_json::json!(SCOPE)), "{lines:?}");
    assert_eq!(lines[1]["verb"], "agents adopt", "{lines:?}");
    // An unrelated one is not.
    let t = scope_box_full(true, &[], &[7151]);
    old_cgroup(&t, &[7101], K2_UID);
    t.procs.0.borrow_mut().procs.insert(7151, P { exe: "/bin/sh".into(), exe_owner: 0, ppid: 1, ruid: K2_UID, sid: 7151 });
    let o = t.call(&["agents", "adopt", WS, SID, "open"]);
    assert!(o.code != exit::OK && o.stderr.contains("\"mismatched\":[7151]"), "{}", o.stderr);
}

fn child_proc(t: &T, pid: i32, ppid: i32) {
    t.procs.0.borrow_mut().procs.insert(pid, P { exe: "/bin/sh".into(), exe_owner: 0, ppid, ruid: K2_UID, sid: pid });
}

/// A parent chain that leaves the scope: 7161's parent 7160 is a child of
/// the checked 7101, but 7160 is not in the scope's member list.
#[test]
fn agents_adopt_refuses_a_child_whose_parent_chain_leaves_the_scope() {
    let t = scope_box_full(true, &[], &[7161]);
    old_cgroup(&t, &[7101], K2_UID);
    child_proc(&t, 7160, 7101);
    child_proc(&t, 7161, 7160);
    let o = t.call(&["agents", "adopt", WS, SID, "open"]);
    assert!(o.code != exit::OK && o.stderr.contains("\"mismatched\":[7161]"), "{}", o.stderr);
    assert_eq!(systemctl_calls(&t), vec![sv(&["stop", "--no-block", SCOPE])]);
    assert!(!t.audit.0.borrow().iter().any(|l| l.contains("scope_member_child_accepted")), "no child accepted");
}

/// A chain through a held pid that failed its own check is no chain: 7170's
/// parent is 7102, whose start time changed during the move (and 7102 is a
/// child of the checked 7101, so only the failed hop stands in the way).
#[test]
fn agents_adopt_refuses_a_child_through_a_held_pid_that_failed_its_check() {
    let t = scope_box_full(true, &[7102], &[7170]);
    old_cgroup(&t, &[7101, 7102], K2_UID);
    t.procs.0.borrow_mut().procs.get_mut(&7102).unwrap().ppid = 7101;
    child_proc(&t, 7170, 7102);
    let o = t.call(&["agents", "adopt", WS, SID, "open"]);
    assert_ne!(o.code, exit::OK);
    assert!(o.stderr.contains("\"mismatched\":[7102,7170]"), "{}", o.stderr);
    assert_eq!(systemctl_calls(&t), vec![sv(&["stop", "--no-block", SCOPE])]);
    assert!(!t.audit.0.borrow().iter().any(|l| l.contains("scope_member_child_accepted")), "no child accepted");
}

/// Review R3 for adopt: a moved number that is another process by then is
/// refused and named; the scope is stopped.
#[test]
fn agents_adopt_refuses_and_names_a_pid_whose_start_time_changed() {
    let t = scope_box_with(true, &[7102]);
    old_cgroup(&t, &[7101, 7102], K2_UID);
    let o = t.call(&["agents", "adopt", WS, SID, "open"]);
    assert_ne!(o.code, exit::OK);
    assert!(o.stderr.contains("\"mismatched\":[7102]"), "{}", o.stderr);
    assert_eq!(systemctl_calls(&t), vec![sv(&["stop", "--no-block", SCOPE])]);
}

#[test]
fn agents_adopt_moves_every_live_process_of_the_old_cgroup_into_one_scope() {
    let t = scope_box(true);
    let old = format!("{UNIT}/agents/a-{WS}");
    mkdirs(&t.root, &[&format!("/sys/fs/cgroup{old}")]);
    exec_proc(&t, 7101, K2_UID, &old);
    exec_proc(&t, 7102, K2_UID, &old);
    // 7103 is listed but exited since: skipped.
    std::fs::write(t.p(&format!("/sys/fs/cgroup{old}/cgroup.procs")), "7101\n7102\n7103\n").unwrap();
    let o = t.call(&["agents", "adopt", WS, SID, "open"]);
    ok(&o);
    let v = json_of(&o);
    assert_eq!(v["moved"], 2, "{v}");
    assert_eq!(v["pids"], serde_json::json!([7101, 7102]), "{v}");
    assert_eq!(v["scope"], SCOPE, "{v}");
    let calls = busctl_calls(&t);
    assert_eq!(calls.len(), 1, "one scope for them all: {calls:?}");
    let a: Vec<&str> = calls[0].iter().map(String::as_str).collect();
    let i = a.iter().position(|x| *x == "PIDs").expect("PIDs");
    assert_eq!(&a[i..i + 5], ["PIDs", "au", "2", "7101", "7102"]);
    assert!(!a.contains(&"MemoryMax"), "adopt sets no caps: {a:?}");
    let j = a.iter().position(|x| *x == "TimeoutStopUSec").expect("the stop timeout at birth");
    assert_eq!(&a[j..j + 6], ["TimeoutStopUSec", "t", "10000000", "Description", "s", "K2 agent scope (adopted)"]);
}

#[test]
fn agents_adopt_refuses_another_users_process_and_a_missing_or_empty_cgroup() {
    let t = scope_box(true);
    let r = t.call(&["agents", "adopt", WS, SID, "open"]);
    assert!(r.code != exit::OK && r.stderr.contains("no old-layout cgroup"), "{}", r.stderr);
    let old = format!("{UNIT}/agents/a-{WS}");
    mkdirs(&t.root, &[&format!("/sys/fs/cgroup{old}")]);
    std::fs::write(t.p(&format!("/sys/fs/cgroup{old}/cgroup.procs")), "").unwrap();
    let r = t.call(&["agents", "adopt", WS, SID, "open"]);
    assert!(r.code != exit::OK && r.stderr.contains("empty"), "{}", r.stderr);
    exec_proc(&t, 7101, K2_UID, &old);
    exec_proc(&t, 7102, 0, &old);
    std::fs::write(t.p(&format!("/sys/fs/cgroup{old}/cgroup.procs")), "7101\n7102\n").unwrap();
    let r = t.call(&["agents", "adopt", WS, SID, "open"]);
    assert!(r.code != exit::OK && r.stderr.contains("not the daemon user's"), "{}", r.stderr);
    assert!(busctl_calls(&t).is_empty(), "systemd never asked: {:?}", busctl_calls(&t));
}

#[test]
fn agents_stop_of_a_scope_that_is_gone_is_ok_and_runs_nothing() {
    let t = scope_box(true);
    let o = t.call(&["agents", "stop", WS, SID]);
    ok(&o);
    let v = json_of(&o);
    assert_eq!(v["gone"], true, "{v}");
    assert!(systemctl_calls(&t).is_empty(), "{:?}", systemctl_calls(&t));
    systemd_stops(&t);
    mkdirs(&t.root, &[&format!("{FENCED_DIR}/{SCOPE}")]);
    let o = t.call(&["agents", "stop", WS, SID]);
    ok(&o);
    let v = json_of(&o);
    assert_eq!((v["kind"].clone(), v["stopping"].clone()), (serde_json::json!("fenced"), serde_json::json!(true)), "{v}");
    assert_eq!(systemctl_calls(&t), vec![sv(&["stop", "--no-block", SCOPE])]);
}

/// Review R2: a scope whose process ignores SIGTERM never holds the
/// helper's lock. `stop` asks once and answers; nothing waits, nothing
/// kills (systemd's own stop timeout does).
#[test]
fn agents_stop_answers_at_once_without_waiting_for_the_scope_to_end() {
    let mut t = scope_box(true);
    t.ctx.agent_stop_wait_ms = 60_000;
    mkdirs(&t.root, &[&format!("{OPEN_DIR}/{SCOPE}")]);
    let began = std::time::Instant::now();
    let o = t.call(&["agents", "stop", WS, SID]);
    ok(&o);
    assert!(began.elapsed() < std::time::Duration::from_secs(5), "no wait: {:?}", began.elapsed());
    let v = json_of(&o);
    assert_eq!((v["gone"].clone(), v["stopping"].clone()), (serde_json::json!(false), serde_json::json!(true)), "{v}");
    assert_eq!(systemctl_calls(&t), vec![sv(&["stop", "--no-block", SCOPE])]);
    let began = std::time::Instant::now();
    ok(&t.call(&["agents", "stop-all"]));
    assert!(began.elapsed() < std::time::Duration::from_secs(5), "no wait: {:?}", began.elapsed());
    assert_eq!(systemctl_calls(&t), vec![sv(&["stop", "--no-block", SCOPE]), sv(&["stop", "--no-block", SCOPE])]);
}

#[test]
fn agents_stop_all_stops_only_names_the_classifier_accepts() {
    let t = scope_box(true);
    let other = format!("k2-agent-abc-def:{SID}.scope");
    mkdirs(
        &t.root,
        &[
            &format!("{OPEN_DIR}/{SCOPE}"),
            &format!("{OPEN_DIR}/k2agents-open-anchor.service"),
            &format!("{OPEN_DIR}/k2-agent-abc.scope"),
            &format!("{OPEN_DIR}/k2-agent-abc:XYZ.scope"),
            &format!("{FENCED_DIR}/{other}"),
            &format!("/sys/fs/cgroup/k2agents.slice/k2-agent-top:{SID}.scope"),
        ],
    );
    std::fs::write(t.p(&format!("{OPEN_DIR}/cgroup.procs")), "").unwrap();
    let o = t.call(&["agents", "stop-all"]);
    ok(&o);
    assert_eq!(json_of(&o)["stopping"], 2);
    assert_eq!(systemctl_calls(&t), vec![sv(&["stop", "--no-block", SCOPE]), sv(&["stop", "--no-block", &other])]);
}

fn layout_with_probe(status: i32, stderr: &str) -> (T, serde_json::Value) {
    let t = scope_box(true);
    mkdirs(&t.root, &[&format!("{OPEN_DIR}/{SCOPE}"), &format!("/sys/fs/cgroup{UNIT}/agents/a-ws-old")]);
    std::fs::write(t.p(&format!("{OPEN_DIR}/{SCOPE}/cgroup.procs")), "7000\n7001\n").unwrap();
    std::fs::write(t.p(&format!("/sys/fs/cgroup{UNIT}/agents/a-ws-old/cgroup.procs")), "7200\n").unwrap();
    mkdirs(&t.root, &["/proc/sys/kernel"]);
    std::fs::write(t.p("/proc/sys/kernel/pid_max"), "4194304\n").unwrap();
    {
        let mut e = t.exec.0.borrow_mut();
        e.rules.push((SYSTEMCTL.into(), "Description".into(), ExecOut { status: 0, stdout: b"K2 agent scope (adopted)\n".to_vec(), stderr: vec![] }));
        e.answers.insert(RUNUSER.into(), ExecOut { status, stdout: vec![], stderr: stderr.as_bytes().to_vec() });
        e.rules.push((SYSTEMCTL.into(), "is-active".into(), ExecOut { status: 0, stdout: b"active\n".to_vec(), stderr: vec![] }));
    }
    let o = t.call(&["agents", "layout"]);
    ok(&o);
    let v = json_of(&o);
    (t, v)
}

#[test]
fn agents_layout_reports_p1_refused_when_systemd_says_no() {
    let (t, v) = layout_with_probe(1, "Failed to start transient scope unit: Access denied");
    assert_eq!(v["p1"], "refused", "{v}");
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["slices"]["k2agents-open.slice"]["present"], true, "{v}");
    assert_eq!(v["anchors"]["k2agents-fenced-anchor.service"]["active"], true, "{v}");
    assert_eq!(v["unit"]["delegate"], true, "{v}");
    assert_eq!(v["unit"]["cgroup"], UNIT, "{v}");
    assert_eq!(v["scopes"], serde_json::json!([{"ws": WS, "id": SID, "kind": "open", "pids": 2, "adopted": true}]), "{v}");
    assert_eq!(v["pidMax"], 4_194_304, "{v}");
    assert_eq!(v["survivors"][0]["ws"], "ws-old", "{v}");
    assert_eq!(v["survivors"][0]["pids"], 1, "{v}");
    assert_eq!(v["survivors"][0]["owner"], t.ctx.root_uid, "{v}");
    assert_eq!(v["survivors"][0]["adoptable"], true, "a root-owned old cgroup: {v}");
    let probe: Vec<Vec<String>> = t.calls().into_iter().filter(|(p, _, _)| p == RUNUSER).map(|(_, a, _)| a).collect();
    assert_eq!(probe.len(), 1, "{probe:?}");
    let a: Vec<&str> = probe[0].iter().map(String::as_str).collect();
    assert_eq!(
        &a[..12],
        ["-u", "k2", "--", ENV, "LC_ALL=C", SYSTEMD_RUN, "--system", "--scope", "--quiet", "--collect", "--no-ask-password", "--slice=k2agents-open.slice"]
    );
    assert!(a[12].starts_with("--unit=k2-agent-probe:") && a[12].ends_with(".scope"), "{a:?}");
    assert_eq!(&a[13..], ["--", "true"]);
    // Report only: nothing started or stopped.
    assert!(busctl_calls(&t).iter().all(|c| !c.iter().any(|x| x == "StartTransientUnit")));
    assert!(systemctl_calls(&t).iter().all(|c| c[0] == "is-active" || c[0] == "show"), "{:?}", systemctl_calls(&t));
}

#[test]
fn agents_layout_reports_p1_allowed_as_not_ok() {
    let (_t, v) = layout_with_probe(0, "");
    assert_eq!(v["p1"], "ALLOWED", "{v}");
    assert_eq!(v["ok"], false, "{v}");
    assert_eq!(v["ready"], false, "{v}");
    let (_t, v) = layout_with_probe(1, "Failed to connect to bus: No such file or directory");
    assert_eq!(v["p1"], "unknown", "{v}");
    assert_eq!(v["ready"], false, "{v}");
}

#[test]
fn agents_setup_writes_the_units_once_and_teardown_removes_them() {
    let t = scope_box(true);
    mkdirs(&t.root, &["/usr/bin"]);
    std::fs::write(t.p("/usr/bin/sleep"), b"elf").unwrap();
    let o = t.call(&["agents", "setup"]);
    ok(&o);
    assert_eq!(json_of(&o)["changed"], true);
    for l in Level::ALL {
        let slice = std::fs::read_to_string(t.p(&format!("/etc/systemd/system/{}", l.slice()))).expect("slice unit");
        assert_eq!(slice, slice_unit(l));
        let anchor = std::fs::read_to_string(t.p(&format!("/etc/systemd/system/{}", l.anchor()))).expect("anchor unit");
        assert_eq!(anchor, anchor_unit(l, "/usr/bin/sleep").expect("anchor"));
    }
    let anchors = ["k2agents-anchor.service", "k2agents-open-anchor.service", "k2agents-fenced-anchor.service"];
    let mut enable = vec!["enable".to_string(), "--now".to_string()];
    enable.extend(anchors.iter().map(|s| s.to_string()));
    assert_eq!(systemctl_calls(&t), vec![vec!["daemon-reload".to_string()], enable.clone()]);
    // Again: nothing to write, no reload.
    let o = t.call(&["agents", "setup"]);
    ok(&o);
    assert_eq!(json_of(&o)["changed"], false);
    assert_eq!(systemctl_calls(&t).last(), Some(&enable));
    assert_eq!(systemctl_calls(&t).iter().filter(|c| c[0] == "daemon-reload").count(), 1);
    // Teardown: scopes first, then the anchors, then the files.
    systemd_stops(&t);
    mkdirs(&t.root, &[&format!("{OPEN_DIR}/{SCOPE}")]);
    let before = systemctl_calls(&t).len();
    let o = t.call(&["agents", "teardown"]);
    ok(&o);
    let v = json_of(&o);
    assert_eq!((v["stopped"].clone(), v["removed"].clone()), (serde_json::json!(1), serde_json::json!(6)), "{v}");
    assert_eq!((v["killed"].clone(), v["stillThere"].clone()), (serde_json::json!(0), serde_json::json!([])), "{v}");
    assert_eq!((v["ok"].clone(), v["sliceStopped"].clone()), (serde_json::json!(true), serde_json::json!(true)), "{v}");
    let mut disable = vec!["disable".to_string(), "--now".to_string()];
    disable.extend(anchors.iter().map(|s| s.to_string()));
    assert_eq!(
        systemctl_calls(&t)[before..].to_vec(),
        vec![sv(&["stop", "--no-block", SCOPE]), disable, sv(&["stop", "k2agents.slice"]), vec!["daemon-reload".to_string()]]
    );
    for l in Level::ALL {
        assert!(!t.p(&format!("/etc/systemd/system/{}", l.slice())).exists());
        assert!(!t.p(&format!("/etc/systemd/system/{}", l.anchor())).exists());
    }
    // And again: nothing left to do.
    let before = systemctl_calls(&t).len();
    ok(&t.call(&["agents", "teardown"]));
    assert_eq!(systemctl_calls(&t).len(), before, "idempotent: {:?}", &systemctl_calls(&t)[before..]);
}

#[test]
fn agents_setup_refuses_without_a_sleep_program() {
    let t = scope_box(true);
    let o = t.call(&["agents", "setup"]);
    assert_ne!(o.code, exit::OK);
    assert!(o.stderr.contains("no sleep program"), "{}", o.stderr);
    assert!(!t.p("/etc/systemd/system/k2agents.slice").exists());
}

#[path = "agents_scope_policy_tests.rs"]
mod policy_tests;
