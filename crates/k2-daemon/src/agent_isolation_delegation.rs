//! Agent Isolation M2, B4: the daemon unit's delegation and the bus policy
//! as `k2 agent isolation layout on|status` shows them (kernel rows 1c, 2
//! and P4; `k2_root_proto::agent_scope_policy`).
//!
//! `agents setup` writes the bus policy and the unit's `Delegate=no`
//! drop-in. From systemd's reload on, it refuses to move processes into
//! the daemon's unit (row P4), but the unit's cgroup stays the server
//! user's until a full stop and start of the unit: k2-ptyd keeps it alive
//! through a plain restart. That restart ends every agent session, so it is
//! the owner's step: the route names it, with how many sessions it would
//! end. Everything here is pure over `agents layout`'s answer.

use serde_json::{json, Value};

/// `agents layout`'s `busPolicy` (`active`, `present-but-not-active`,
/// `missing`): only `active` means the bus enforces it.
pub fn bus_policy(layout: &Value) -> Option<&str> {
    layout["busPolicy"].as_str()
}

/// `agents layout`'s `delegation` (`off`, `pending-full-restart`, `on`,
/// `unknown`).
pub fn delegation(layout: &Value) -> Option<&str> {
    layout["delegation"].as_str()
}

/// The A2 ruling's box kind: the helper's answer (`daemon.toml`'s scope)
/// when there is one, else this daemon's own cgroup
/// (`k2_core::agent_cgroup::box_kind`).
pub fn box_kind(layout: Option<&Value>) -> String {
    layout
        .and_then(|l| l["boxKind"].as_str())
        .map(str::to_string)
        .unwrap_or_else(|| k2_core::agent_cgroup::box_kind().to_string())
}

/// The A2 ruling's line: the helper's words, else the same words.
pub fn open_agents(layout: Option<&Value>) -> String {
    layout
        .and_then(|l| l["openAgents"].as_str())
        .unwrap_or(k2_root_proto::agent_scope_policy::OPEN_AGENTS)
        .to_string()
}

/// What the owner still has to do for `delegation` to read `off`: `None`
/// when it does already (or the helper reports no delegation).
/// `running_sessions`: the agent sessions such a restart would end now.
pub fn full_restart(layout: &Value, running_sessions: usize) -> Option<Value> {
    let d = delegation(layout).filter(|d| *d != "off")?;
    let unit = layout["unit"]["name"].as_str().unwrap_or_default();
    let command = layout["fullRestart"]
        .as_str()
        .map(str::to_string)
        .or_else(|| k2_root_proto::agent_scope_policy::full_restart_command(unit))?;
    let message = match d {
        "pending-full-restart" => format!(
            "systemd already refuses to move processes into the daemon's unit (Delegate=no), but the unit's cgroup stays the server \
             user's until a full stop and start of the unit, the daemon and k2-ptyd together: {command}. That ends every running agent \
             session ({running_sessions} now). Until then delegation reads pending-full-restart"
        ),
        "on" => format!(
            "the daemon's unit is still delegated (Delegate=yes): the drop-in agents setup wrote is not in effect. A full stop and \
             start of the unit ({command}) ends every running agent session ({running_sessions} now)"
        ),
        _ => format!(
            "systemd's Delegate for the daemon's unit could not be read. A full stop and start of the unit ({command}) ends every \
             running agent session ({running_sessions} now)"
        ),
    };
    Some(json!({
        "needed": true,
        "delegation": d,
        "command": command,
        "endsSessions": running_sessions,
        "message": message,
    }))
}

/// Why `agents layout` is not ready, in the owner's words: the first of the
/// P1 probe, the bus policy, the drop-in, the delegation, then the slices.
pub fn not_ready_reason(v: &Value) -> String {
    if let Some(p) = bus_policy(v).filter(|p| *p != "active") {
        let path = v["busPolicyPath"].as_str().unwrap_or(k2_root_proto::agent_scope_policy::BUS_POLICY_PATH);
        let probe = v["busPolicyProbe"].as_str().map(|w| format!(" (the probe: {w})")).unwrap_or_default();
        return format!(
            "the bus policy that stops the server user asking systemd to move processes ({path}): {p}{probe}; agents setup writes it and \
             checks the bus enforces it"
        );
    }
    if let Some(d) = v["delegateDropin"].as_str().filter(|d| *d != "present") {
        return format!("the daemon unit's Delegate=no drop-in ({}): {d}; agents setup writes it", v["delegateDropinPath"].as_str().unwrap_or("?"));
    }
    match delegation(v) {
        Some("on") => {
            return "the daemon's unit is still delegated (Delegate=yes after the drop-in): another drop-in or the unit file wins".to_string();
        }
        Some("unknown") => return "systemd's Delegate for the daemon's unit could not be read".to_string(),
        _ => {}
    }
    format!("the agents slices aren't set up (agents layout: {v})")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(delegation: &str) -> Value {
        json!({
            "ready": true,
            "busPolicy": "active",
            "delegateDropin": "present",
            "delegation": delegation,
            "unit": { "name": "k2-daemon.service" },
        })
    }

    #[test]
    fn a_full_restart_is_named_with_the_sessions_it_ends_until_delegation_is_off() {
        assert_eq!(full_restart(&layout("off"), 3), None);
        assert_eq!(full_restart(&json!({ "unit": { "name": "k2-daemon.service" } }), 3), None, "an older helper reports none");
        let mut pending = layout("pending-full-restart");
        pending["fullRestart"] = json!("the helper's command");
        let r = full_restart(&pending, 3).expect("needed");
        assert_eq!((r["needed"].as_bool(), r["delegation"].as_str(), r["endsSessions"].as_u64()), (Some(true), Some("pending-full-restart"), Some(3)), "{r}");
        assert_eq!(r["command"], "the helper's command");
        let m = r["message"].as_str().expect("message");
        assert!(m.contains("the daemon and k2-ptyd") && m.contains("(3 now)") && m.contains("the helper's command"), "{m}");
        let r = full_restart(&layout("on"), 0).expect("needed");
        assert_eq!(
            r["command"],
            "sudo systemctl stop k2-daemon.service && sudo systemctl kill --kill-whom=all --signal=KILL k2-daemon.service; sudo systemctl start k2-daemon.service"
        );
        assert!(r["message"].as_str().expect("message").contains("still delegated"), "{r}");
    }

    #[test]
    fn the_box_kind_and_the_open_agents_line_come_from_the_helper_else_this_daemon() {
        let l = json!({ "boxKind": "server", "openAgents": "the helper's words" });
        assert_eq!(box_kind(Some(&l)), "server");
        assert_eq!(open_agents(Some(&l)), "the helper's words");
        let own = k2_core::agent_cgroup::box_kind();
        assert!(own == "server" || own == "desktop", "{own}");
        assert_eq!(box_kind(None), own);
        assert_eq!(box_kind(Some(&json!({}))), own, "an older helper says nothing");
        assert_eq!(open_agents(None), "open agents are not fenced; only fenced sessions are held");
    }

    #[test]
    fn the_reason_names_what_is_missing_first() {
        let mut v = layout("off");
        v["ready"] = json!(false);
        v["busPolicy"] = json!("missing");
        v["busPolicyPath"] = json!("/etc/dbus-1/system.d/k2-agents.conf");
        assert!(not_ready_reason(&v).contains("bus policy") && not_ready_reason(&v).contains("(/etc/dbus-1/system.d/k2-agents.conf): missing"), "{}", not_ready_reason(&v));
        v["busPolicy"] = json!("present-but-not-active");
        v["busPolicyProbe"] = json!("NotDenied: Unit k2-agents-probe.service not loaded.");
        assert!(not_ready_reason(&v).contains("present-but-not-active (the probe: NotDenied"), "{}", not_ready_reason(&v));
        v["busPolicy"] = json!("active");
        v["delegateDropin"] = json!("differs");
        assert!(not_ready_reason(&v).contains("Delegate=no drop-in") && not_ready_reason(&v).contains(": differs"), "{}", not_ready_reason(&v));
        v["delegateDropin"] = json!("present");
        v["delegation"] = json!("on");
        assert!(not_ready_reason(&v).contains("still delegated"), "{}", not_ready_reason(&v));
        v["delegation"] = json!("unknown");
        assert!(not_ready_reason(&v).contains("could not be read"), "{}", not_ready_reason(&v));
        v["delegation"] = json!("pending-full-restart");
        assert!(not_ready_reason(&v).contains("slices aren't set up"), "{}", not_ready_reason(&v));
    }
}
