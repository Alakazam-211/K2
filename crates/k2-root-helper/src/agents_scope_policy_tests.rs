//! `agents setup` / `teardown` / `layout`: the bus policy and the daemon
//! unit's `Delegate=no` drop-in (kernel rows 1c, 2 and P4), over the temp
//! root with a stub `busctl`.

use super::*;
use k2_root_proto::agent_scope_policy::{bus_policy, delegate_dropin};

const POLICY: &str = "/etc/dbus-1/system.d/k2-agents.conf";
const DROPIN: &str = "/etc/systemd/system/k2-daemon.service.d/zz-k2-agents-delegate.conf";

/// systemd and the bus driver as `busctl` sees them: the unit's live
/// `Delegate` (`None`: unreadable) and `ReloadConfig`'s exit status.
fn systemd_bus(t: &T, delegate: Option<bool>, reload_status: i32) {
    let p: Program = Rc::new(move |args: &[String]| {
        let has = |w: &str| args.iter().any(|a| a == w);
        let out = |status: i32, o: &str, e: &str| ExecOut { status, stdout: o.as_bytes().to_vec(), stderr: e.as_bytes().to_vec() };
        if has("ReloadConfig") {
            return if reload_status == 0 { out(0, "", "") } else { out(reload_status, "", "Call failed: Access denied") };
        }
        if has("LoadUnit") {
            return out(0, r#"{"type":"o","data":["/org/freedesktop/systemd1/unit/k2_2ddaemon_2eservice"]}"#, "");
        }
        if has("Delegate") {
            return match delegate {
                Some(b) => out(0, &format!(r#"{{"type":"b","data":{b}}}"#), ""),
                None => out(1, "", "Unknown property"),
            };
        }
        out(1, "", "unexpected busctl call")
    });
    t.exec.0.borrow_mut().programs.insert(BUSCTL.into(), p);
}

fn reloads(t: &T) -> usize {
    busctl_calls(t).iter().filter(|c| c.iter().any(|x| x == "ReloadConfig")).count()
}

fn with_sleep(t: &T) {
    mkdirs(&t.root, &["/usr/bin"]);
    std::fs::write(t.p("/usr/bin/sleep"), b"elf").unwrap();
}

#[test]
fn agents_setup_writes_the_bus_policy_and_the_delegate_dropin_then_reloads_both() {
    let t = scope_box(true);
    with_sleep(&t);
    systemd_bus(&t, Some(false), 0);
    let o = t.call(&["agents", "setup"]);
    ok(&o);
    let v = json_of(&o);
    assert_eq!((v["busPolicyChanged"].clone(), v["delegateDropinChanged"].clone()), (serde_json::json!(true), serde_json::json!(true)), "{v}");
    assert_eq!(std::fs::read_to_string(t.p(POLICY)).expect("policy"), bus_policy("k2").expect("text"));
    assert_eq!(std::fs::read_to_string(t.p(DROPIN)).expect("drop-in"), delegate_dropin());
    assert_eq!(
        busctl_calls(&t),
        vec![sv(&["call", "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "ReloadConfig"])],
        "one bus reload, as root"
    );
    assert_eq!(systemctl_calls(&t).iter().filter(|c| c[0] == "daemon-reload").count(), 1, "{:?}", systemctl_calls(&t));
    // Again: nothing changed, no reload of either.
    let o = t.call(&["agents", "setup"]);
    ok(&o);
    assert_eq!(json_of(&o)["changed"], false);
    assert_eq!(reloads(&t), 1);
    assert_eq!(systemctl_calls(&t).iter().filter(|c| c[0] == "daemon-reload").count(), 1);
}

#[test]
fn agents_setup_rewrites_a_policy_that_differs_and_reloads_only_the_bus() {
    let t = scope_box(true);
    with_sleep(&t);
    systemd_bus(&t, Some(false), 0);
    ok(&t.call(&["agents", "setup"]));
    std::fs::write(t.p(POLICY), "<busconfig/>\n").unwrap();
    let o = t.call(&["agents", "setup"]);
    ok(&o);
    let v = json_of(&o);
    assert_eq!((v["busPolicyChanged"].clone(), v["delegateDropinChanged"].clone()), (serde_json::json!(true), serde_json::json!(false)), "{v}");
    assert_eq!(std::fs::read_to_string(t.p(POLICY)).unwrap(), bus_policy("k2").unwrap());
    assert_eq!(reloads(&t), 2);
    assert_eq!(systemctl_calls(&t).iter().filter(|c| c[0] == "daemon-reload").count(), 1);
}

#[test]
fn agents_setup_fails_loudly_when_the_bus_does_not_reload() {
    let t = scope_box(true);
    with_sleep(&t);
    systemd_bus(&t, Some(false), 1);
    let o = t.call(&["agents", "setup"]);
    assert_ne!(o.code, exit::OK);
    assert!(o.stderr.contains("the bus policy is written, but the system bus did not reload"), "{}", o.stderr);
}

#[test]
fn agents_setup_refuses_a_link_where_the_bus_policy_folder_goes() {
    let t = scope_box(true);
    with_sleep(&t);
    systemd_bus(&t, Some(false), 0);
    mkdirs(&t.root, &["/etc/dbus-1", "/elsewhere"]);
    symlink(t.p("/elsewhere"), t.p("/etc/dbus-1/system.d")).unwrap();
    let o = t.call(&["agents", "setup"]);
    assert_ne!(o.code, exit::OK);
    assert!(o.stderr.contains("system.d is not a directory"), "{}", o.stderr);
    assert!(!t.p("/elsewhere/k2-agents.conf").exists());
}

#[test]
fn agents_teardown_removes_the_bus_policy_and_the_dropin_then_reloads_both() {
    let t = scope_box(true);
    with_sleep(&t);
    systemd_bus(&t, Some(false), 0);
    ok(&t.call(&["agents", "setup"]));
    let before = systemctl_calls(&t).len();
    let o = t.call(&["agents", "teardown"]);
    ok(&o);
    let v = json_of(&o);
    assert_eq!((v["busPolicyRemoved"].clone(), v["delegateDropinRemoved"].clone(), v["ok"].clone()), (serde_json::json!(true), serde_json::json!(true), serde_json::json!(true)), "{v}");
    assert!(!t.p(POLICY).exists() && !t.p(DROPIN).exists());
    assert_eq!(reloads(&t), 2, "setup's and teardown's");
    assert_eq!(systemctl_calls(&t)[before..].iter().filter(|c| c[0] == "daemon-reload").count(), 1);
    // A policy left alone (no units, no drop-in) still goes, with a reload.
    std::fs::write(t.p(POLICY), bus_policy("k2").unwrap()).unwrap();
    let before = systemctl_calls(&t).len();
    let o = t.call(&["agents", "teardown"]);
    ok(&o);
    assert_eq!(json_of(&o)["busPolicyRemoved"], true);
    assert!(!t.p(POLICY).exists());
    assert_eq!(reloads(&t), 3);
    assert!(systemctl_calls(&t)[before..].iter().all(|c| c[0] != "daemon-reload"), "nothing of systemd's changed");
    // A failed bus reload is reported, not hidden.
    std::fs::write(t.p(POLICY), "x").unwrap();
    systemd_bus(&t, Some(false), 1);
    let o = t.call(&["agents", "teardown"]);
    ok(&o);
    let v = json_of(&o);
    assert_eq!(v["ok"], false, "{v}");
    assert!(v["busReloadError"].as_str().unwrap_or("").contains("did not reload"), "{v}");
}

/// A box after `agents setup`, with the anchors active, P1 refused and
/// systemd reporting `delegate` for the daemon's unit.
fn layout_box(delegate: Option<bool>) -> T {
    let t = scope_box(true);
    with_sleep(&t);
    systemd_bus(&t, delegate, 0);
    mkdirs(&t.root, &["/proc/sys/kernel"]);
    std::fs::write(t.p("/proc/sys/kernel/pid_max"), "4194304\n").unwrap();
    {
        let mut e = t.exec.0.borrow_mut();
        e.answers.insert(RUNUSER.into(), ExecOut { status: 1, stdout: vec![], stderr: b"Failed to start transient scope unit: Access denied".to_vec() });
        e.rules.push((SYSTEMCTL.into(), "is-active".into(), ExecOut { status: 0, stdout: b"active\n".to_vec(), stderr: vec![] }));
    }
    ok(&t.call(&["agents", "setup"]));
    t
}

fn layout_of(t: &T) -> serde_json::Value {
    let o = t.call(&["agents", "layout"]);
    ok(&o);
    json_of(&o)
}

#[test]
fn agents_layout_is_ready_with_the_policy_the_dropin_and_delegation_off() {
    let t = layout_box(Some(false));
    let v = layout_of(&t);
    assert_eq!(v["busPolicy"], "present", "{v}");
    assert_eq!(v["delegateDropin"], "present", "{v}");
    assert_eq!(v["delegation"], "off", "the unit's cgroup is root's: {v}");
    assert_eq!(v["unit"]["delegate"], false, "{v}");
    assert_eq!(v["p1"], "refused", "{v}");
    assert!(v.get("fullRestart").is_none(), "{v}");
    assert_eq!(v["ready"], true, "{v}");
}

#[test]
fn agents_layout_is_not_ready_without_the_bus_policy_or_the_dropin() {
    let t = layout_box(Some(false));
    std::fs::remove_file(t.p(POLICY)).unwrap();
    let v = layout_of(&t);
    assert_eq!((v["busPolicy"].clone(), v["ready"].clone()), (serde_json::json!("missing"), serde_json::json!(false)), "{v}");
    std::fs::write(t.p(POLICY), bus_policy("someone-else").unwrap()).unwrap();
    let v = layout_of(&t);
    assert_eq!((v["busPolicy"].clone(), v["ready"].clone()), (serde_json::json!("differs"), serde_json::json!(false)), "{v}");
    std::fs::write(t.p(POLICY), bus_policy("k2").unwrap()).unwrap();
    assert_eq!(layout_of(&t)["ready"], true);
    std::fs::remove_file(t.p(DROPIN)).unwrap();
    let v = layout_of(&t);
    assert_eq!((v["delegateDropin"].clone(), v["ready"].clone()), (serde_json::json!("missing"), serde_json::json!(false)), "{v}");
}

#[test]
fn agents_layout_reads_delegation_from_the_live_property_and_the_cgroups_owner() {
    // Delegate=no, but the unit's cgroup is not root's (ptyd kept it
    // through a plain restart): pending, with the command that ends it.
    let mut t = layout_box(Some(false));
    t.ctx.root_uid = t.ctx.root_uid.wrapping_add(1);
    let v = layout_of(&t);
    assert_eq!(v["delegation"], "pending-full-restart", "{v}");
    assert_eq!(
        v["fullRestart"],
        "sudo systemctl stop k2-daemon.service && sudo systemctl kill --kill-whom=all --signal=KILL k2-daemon.service; sudo systemctl start k2-daemon.service",
        "{v}"
    );
    // Still delegated (the drop-in not loaded, or a later one wins): on.
    let t = layout_box(Some(true));
    let v = layout_of(&t);
    assert_eq!((v["delegation"].clone(), v["ready"].clone()), (serde_json::json!("on"), serde_json::json!(false)), "{v}");
    // systemd's answer unreadable: unknown, never ready.
    let t = layout_box(None);
    let v = layout_of(&t);
    assert_eq!((v["delegation"].clone(), v["ready"].clone()), (serde_json::json!("unknown"), serde_json::json!(false)), "{v}");
}
