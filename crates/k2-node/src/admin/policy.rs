//! Remote policy (prd-k2-node-admin-v1 S3, §4.2, §7.2).
//!
//! - `policy.set`: the executor edits the machine owner's `policy.toml`.
//!   The class is computed **here, from the node's current file**: any
//!   field that widens what jobs may use makes it privileged (owner
//!   approval); pure narrowing is operate and applies at once.
//! - `policy.narrow`: the controller's own narrowing (`NodeLimits`),
//!   persisted root-owned in `narrow.toml` so it survives restarts and a
//!   job (same uid as the node) can't drop it. Today's in-memory
//!   `PolicyNarrow` frame still works on top.
//! - `policy.show`: the node answers (read).

use std::path::Path;

use k2_node_proto::admin::{code, AdminRefusal, Class};
use k2_node_proto::frames::NodeLimits;
use serde_json::{json, Value};

use crate::config::{self, AvailabilityMode, MachineFacts, OnOwnerReturn, Policy, PolicyFile};
use crate::paths::Layout;

/// Fields `policy.set` may name (exactly `PolicyFile`'s).
pub const FIELDS: &[&str] = &[
    "cpu_cap_percent",
    "mem_cap_gb",
    "disk_budget_gb",
    "disk_floor_gb",
    "availability",
    "window",
    "on_owner_return",
    "max_parallel",
    "workspaces",
    "foreign_locks",
    "write_smoke_lock",
    "network",
    "repo_toolchains",
    "gate_reserved_slots",
];

fn bad(m: impl Into<String>) -> AdminRefusal {
    AdminRefusal::new(code::BAD_ARGS, m)
}

fn json_to_toml(v: &Value) -> Result<toml::Value, AdminRefusal> {
    Ok(match v {
        Value::Bool(b) => toml::Value::Boolean(*b),
        Value::Number(n) => toml::Value::Integer(n.as_i64().ok_or_else(|| bad("numbers must be integers"))?),
        Value::String(s) => toml::Value::String(s.clone()),
        Value::Array(a) => toml::Value::Array(a.iter().map(json_to_toml).collect::<Result<_, _>>()?),
        Value::Object(m) => {
            let mut t = toml::Table::new();
            for (k, v) in m {
                t.insert(k.clone(), json_to_toml(v)?);
            }
            toml::Value::Table(t)
        }
        Value::Null => return Err(bad("null is only allowed at the top level (it resets a field)")),
    })
}

/// The current file as a table, the edited table, and both parsed.
struct Edit {
    table: toml::Table,
    old: Policy,
    new: Policy,
}

fn facts(l: &Layout) -> MachineFacts {
    crate::node::machine_facts(l)
}

fn edit(l: &Layout, args: &Value, m: &MachineFacts) -> Result<Edit, AdminRefusal> {
    let changes = args.as_object().ok_or_else(|| bad("args must be an object"))?;
    if changes.is_empty() {
        return Err(bad("policy.set needs at least one field"));
    }
    let text = match std::fs::read_to_string(l.policy()) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(AdminRefusal::new(code::FAILED, format!("read policy: {e}"))),
    };
    let mut table: toml::Table = toml::from_str(&text).map_err(|e| AdminRefusal::new(code::FAILED, format!("current policy.toml doesn't parse: {e}")))?;
    let old_file: PolicyFile = toml::from_str(&text).map_err(|e| AdminRefusal::new(code::FAILED, format!("current policy.toml: {e}")))?;
    let old = Policy::resolve(&old_file, m).map_err(|e| AdminRefusal::new(code::FAILED, format!("current policy: {e}")))?;
    for (k, v) in changes {
        if !FIELDS.contains(&k.as_str()) {
            return Err(bad(format!("unknown policy field {k:?}")));
        }
        // 0.46.4's reserved gate slots are gone (prd-compute-job-tiers-v1
        // R2): a value is refused, removing the key is fine.
        if k == "gate_reserved_slots" && !v.is_null() {
            return Err(AdminRefusal::new(
                code::DEPRECATED_KEY,
                "gate_reserved_slots is ignored since 0.46.5: no slot is reserved, CI takes the next free slot (k2 compute run --tier ci). Remove it with gate_reserved_slots=null",
            ));
        }
        if v.is_null() {
            table.remove(k);
        } else {
            table.insert(k.clone(), json_to_toml(v)?);
        }
    }
    let new_text = toml::to_string(&table).map_err(|e| bad(format!("encode: {e}")))?;
    let new_file: PolicyFile = toml::from_str(&new_text).map_err(|e| bad(format!("policy not valid: {e}")))?;
    let new = Policy::resolve(&new_file, m).map_err(bad)?;
    // Without the job wall nothing enforces `network = "none"`: refuse it
    // rather than let the policy claim a wall jobs don't have.
    if changes.get("network").and_then(Value::as_str) == Some("none") && !network_none_ok(l) {
        use k2_node_proto::refusal::{NETWORK_NONE_MESSAGE, NETWORK_NONE_NOT_ENFORCED};
        return Err(AdminRefusal::new(NETWORK_NONE_NOT_ENFORCED, NETWORK_NONE_MESSAGE));
    }
    Ok(Edit { table, old, new })
}

#[cfg(test)]
thread_local! {
    /// Tests pin the answer (the real one depends on the machine).
    pub static NETWORK_NONE_OVERRIDE: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

fn network_none_ok(l: &Layout) -> bool {
    #[cfg(test)]
    if let Some(v) = NETWORK_NONE_OVERRIDE.with(|c| c.get()) {
        return v;
    }
    crate::wall::network_none_supported(&l.config)
}

fn avail_rank(a: &AvailabilityMode) -> u8 {
    match a {
        AvailabilityMode::Always => 3,
        AvailabilityMode::Idle | AvailabilityMode::PluggedIn => 2,
        AvailabilityMode::IdleAndPluggedIn => 1,
        AvailabilityMode::Window => 0,
    }
}

/// The fields that widen (empty = pure narrowing or no-op). Anything we
/// can't order is counted as widening (doubt never allows).
pub fn widened(old: &Policy, new: &Policy) -> Vec<&'static str> {
    let mut w = Vec::new();
    if new.cpu_cap_percent > old.cpu_cap_percent {
        w.push("cpu_cap_percent");
    }
    if new.mem_cap_bytes > old.mem_cap_bytes {
        w.push("mem_cap_gb");
    }
    if new.disk_budget_bytes > old.disk_budget_bytes {
        w.push("disk_budget_gb");
    }
    if new.disk_floor_bytes < old.disk_floor_bytes {
        w.push("disk_floor_gb");
    }
    if new.max_parallel > old.max_parallel {
        w.push("max_parallel");
    }
    let (ro, rn) = (avail_rank(&old.availability), avail_rank(&new.availability));
    if rn > ro || (rn == ro && new.availability != old.availability) {
        w.push("availability");
    }
    if new.availability == AvailabilityMode::Window && new.window != old.window {
        w.push("window");
    }
    if old.on_owner_return == OnOwnerReturn::PauseNow && new.on_owner_return == OnOwnerReturn::Finish {
        w.push("on_owner_return");
    }
    let ws_wider = match (old.workspaces.is_empty(), new.workspaces.is_empty()) {
        (true, _) => false,      // everything allowed before
        (false, true) => true,   // a list became "any"
        (false, false) => !new.workspaces.iter().all(|x| old.workspaces.contains(x)),
    };
    if ws_wider {
        w.push("workspaces");
    }
    if old.foreign_locks.iter().any(|x| !new.foreign_locks.contains(x)) {
        w.push("foreign_locks");
    }
    if new.write_smoke_lock != old.write_smoke_lock {
        w.push("write_smoke_lock");
    }
    if old.network == "none" && new.network != "none" {
        w.push("network");
    }
    if !old.repo_toolchains && new.repo_toolchains {
        w.push("repo_toolchains");
    }
    w
}

pub fn classify_set(l: &Layout, args: &Value) -> Result<Class, AdminRefusal> {
    let e = edit(l, args, &facts(l))?;
    Ok(if widened(&e.old, &e.new).is_empty() { Class::Operate } else { Class::Privileged })
}

pub fn apply_set(l: &Layout, args: &Value) -> Result<Value, AdminRefusal> {
    apply_set_with(l, args, &facts(l))
}

pub fn apply_set_with(l: &Layout, args: &Value, m: &MachineFacts) -> Result<Value, AdminRefusal> {
    let e = edit(l, args, m)?;
    let w = widened(&e.old, &e.new);
    let body = toml::to_string(&e.table).map_err(|x| bad(format!("encode: {x}")))?;
    let text = format!("# k2-node policy (the machine owner's limits). Last written by the node admin executor.\n{body}");
    crate::util::atomic_write(&l.policy(), text.as_bytes(), 0o664).map_err(|x| AdminRefusal::new(code::FAILED, x))?;
    Ok(json!({"policy": e.new, "widened": w}))
}

/// `narrow.toml` (root 0644). Missing = no persisted narrowing.
pub fn read_narrow(config_dir: &Path) -> Result<NodeLimits, String> {
    let p = config_dir.join(super::config::NARROW_FILE);
    match std::fs::read_to_string(&p) {
        Ok(s) => {
            #[derive(serde::Deserialize)]
            #[serde(deny_unknown_fields)]
            struct F {
                cpu_millis: Option<u64>,
                mem_bytes: Option<u64>,
                max_parallel: Option<u32>,
            }
            let f: F = toml::from_str(&s).map_err(|e| format!("{}: {e}", p.display()))?;
            Ok(NodeLimits { cpu_millis: f.cpu_millis, mem_bytes: f.mem_bytes, max_parallel: f.max_parallel })
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(NodeLimits::default()),
        Err(e) => Err(format!("read {}: {e}", p.display())),
    }
}

pub fn apply_narrow(config_dir: &Path, args: &Value) -> Result<Value, AdminRefusal> {
    super::exec::only_keys(args, &["cpu_millis", "mem_bytes", "max_parallel"])?;
    let get = |k: &str| -> Result<Option<u64>, AdminRefusal> {
        match args.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => v.as_u64().map(Some).ok_or_else(|| bad(format!("{k} must be a positive integer"))),
        }
    };
    let n = NodeLimits {
        cpu_millis: get("cpu_millis")?,
        mem_bytes: get("mem_bytes")?,
        max_parallel: get("max_parallel")?.map(|p| p.clamp(1, 64) as u32),
    };
    let mut body = String::from("# Controller narrowing (node admin policy.narrow). Only ever tightens the owner's policy.\n");
    if let Some(c) = n.cpu_millis {
        body.push_str(&format!("cpu_millis = {c}\n"));
    }
    if let Some(m) = n.mem_bytes {
        body.push_str(&format!("mem_bytes = {m}\n"));
    }
    if let Some(p) = n.max_parallel {
        body.push_str(&format!("max_parallel = {p}\n"));
    }
    crate::util::atomic_write(&config_dir.join(super::config::NARROW_FILE), body.as_bytes(), 0o644).map_err(|e| AdminRefusal::new(code::FAILED, e))?;
    Ok(json!({"narrow": n}))
}

/// `policy.show` (node side).
pub fn show(l: &Layout, m: &MachineFacts, session_narrow: &NodeLimits) -> Value {
    let file = config::read_policy_file(&l.policy());
    let resolved = file.as_ref().map_err(|e| e.clone()).and_then(|f| Policy::resolve(f, m));
    let notes: Vec<&str> = file.as_ref().ok().filter(|f| f.gate_reserved_slots.is_some()).map(|_| config::GATE_RESERVED_IGNORED).into_iter().collect();
    json!({
        "policy": resolved.as_ref().ok(),
        "error": resolved.as_ref().err(),
        "notes": notes,
        "file": file.ok(),
        "narrow": read_narrow(&l.config).ok(),
        "session_narrow": session_narrow,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::GIB;

    fn rig() -> (Layout, MachineFacts) {
        let d = crate::util::temp_dir("policy-admin");
        let l = Layout::new(d.join("home"), d.join("etc"));
        std::fs::create_dir_all(&l.config).unwrap();
        (l, MachineFacts { cores: 8, mem_bytes: 16 * GIB, disk_total_bytes: 500 * GIB, has_battery: false, is_macos: false })
    }

    fn class(l: &Layout, m: &MachineFacts, args: Value) -> Class {
        let e = edit(l, &args, m).unwrap();
        if widened(&e.old, &e.new).is_empty() {
            Class::Operate
        } else {
            Class::Privileged
        }
    }

    /// prd-compute-job-tiers-v1 §3.7: a 0.46.4 file that reserves gate
    /// slots still reads (the key is ignored); `policy.set` refuses a value
    /// with `deprecated_key` but may remove the key; `policy.show` says why.
    #[test]
    fn gate_reserved_slots_is_ignored_and_setting_it_is_deprecated() {
        let (l, m) = rig();
        std::fs::write(l.policy(), "max_parallel = 3\ngate_reserved_slots = 1\n").unwrap();
        let f = config::read_policy_file(&l.policy()).expect("a 0.46.4 policy.toml stays valid");
        assert_eq!(f.gate_reserved_slots, Some(1));
        assert_eq!(Policy::resolve(&f, &m).unwrap().max_parallel, 3);
        let shown = show(&l, &m, &NodeLimits::default());
        assert_eq!(shown["notes"], json!([config::GATE_RESERVED_IGNORED]));
        for v in [json!(2), json!(0)] {
            let e = edit(&l, &json!({"gate_reserved_slots": v}), &m).err().expect("refused");
            assert_eq!(e.code, code::DEPRECATED_KEY);
        }
        assert_eq!(class(&l, &m, json!({"gate_reserved_slots": null})), Class::Operate, "removing an ignored key changes nothing");
        apply_set_with(&l, &json!({"gate_reserved_slots": null}), &m).unwrap();
        assert_eq!(config::read_policy_file(&l.policy()).unwrap().gate_reserved_slots, None);
        assert_eq!(show(&l, &m, &NodeLimits::default())["notes"], json!([]));
    }

    #[test]
    fn narrowing_is_operate_widening_is_privileged() {
        let (l, m) = rig();
        // Defaults: mem 12 GiB (3/4 of 16), cpu 75 %, parallel 2.
        assert_eq!(class(&l, &m, json!({"mem_cap_gb": 6})), Class::Operate);
        assert_eq!(class(&l, &m, json!({"mem_cap_gb": 13})), Class::Privileged);
        assert_eq!(class(&l, &m, json!({"cpu_cap_percent": 50, "max_parallel": 1})), Class::Operate);
        assert_eq!(class(&l, &m, json!({"cpu_cap_percent": 50, "max_parallel": 3})), Class::Privileged, "one widening field is enough");
        assert_eq!(class(&l, &m, json!({"disk_floor_gb": 10})), Class::Privileged, "a lower floor lets jobs fill more");
        assert_eq!(class(&l, &m, json!({"workspaces": ["k2"]})), Class::Operate, "any list narrows 'all'");
        assert_eq!(class(&l, &m, json!({"availability": "plugged_in"})), Class::Operate);
        assert_eq!(class(&l, &m, json!({"foreign_locks": []})), Class::Privileged, "dropping a lock widens");
        // Writing a narrowed file, then widening back from it.
        apply_set_with(&l, &json!({"mem_cap_gb": 6, "workspaces": ["k2", "sew"]}), &m).unwrap();
        assert_eq!(class(&l, &m, json!({"mem_cap_gb": null})), Class::Privileged, "reset to the default widens 6 → 12");
        assert_eq!(class(&l, &m, json!({"workspaces": ["k2"]})), Class::Operate);
        assert_eq!(class(&l, &m, json!({"workspaces": ["k2", "other"]})), Class::Privileged);
        assert_eq!(class(&l, &m, json!({"workspaces": null})), Class::Privileged);
    }

    /// Without the job wall `network = "none"` isn't enforced: policy.set
    /// refuses it with the stable code and leaves the file alone. A node
    /// whose file already says none (hand-edited) may still go back to
    /// full, and that counts as a widening.
    #[test]
    fn network_none_is_refused_without_the_wall() {
        NETWORK_NONE_OVERRIDE.with(|c| c.set(Some(false)));
        let (l, m) = rig();
        apply_set_with(&l, &json!({"mem_cap_gb": 6}), &m).unwrap();
        let before = std::fs::read_to_string(l.policy()).unwrap();
        let e = apply_set_with(&l, &json!({"network": "none"}), &m).unwrap_err();
        assert_eq!(e.code, "network_none_not_enforced_yet");
        assert!(e.message.contains("isn't enforced on this node"), "{}", e.message);
        assert!(classify_set(&l, &json!({"network": "none"})).is_err());
        assert_eq!(std::fs::read_to_string(l.policy()).unwrap(), before);
        std::fs::write(l.policy(), "network = \"none\"\n").unwrap();
        assert_eq!(class(&l, &m, json!({"network": "full"})), Class::Privileged);
        assert_eq!(class(&l, &m, json!({"mem_cap_gb": 6})), Class::Operate, "other edits still work on a none file");
    }

    /// A walled node takes `network = "none"`; full → none narrows (the
    /// owner's approval isn't needed), none → full widens.
    #[test]
    fn network_none_is_a_narrowing_on_a_walled_node() {
        NETWORK_NONE_OVERRIDE.with(|c| c.set(Some(true)));
        let (l, m) = rig();
        assert_eq!(class(&l, &m, json!({"network": "none"})), Class::Operate);
        apply_set_with(&l, &json!({"network": "none"}), &m).unwrap();
        assert_eq!(config::read_policy_file(&l.policy()).unwrap().network.as_deref(), Some("none"));
        assert_eq!(class(&l, &m, json!({"network": "full"})), Class::Privileged);
    }

    #[test]
    fn bad_fields_and_values_refuse_and_leave_the_file() {
        let (l, m) = rig();
        apply_set_with(&l, &json!({"mem_cap_gb": 6}), &m).unwrap();
        let before = std::fs::read_to_string(l.policy()).unwrap();
        for bad in [json!({}), json!({"cpu_cap": 5}), json!({"max_parallel": 0}), json!({"availability": "sometimes"}), json!({"mem_cap_gb": "x"})] {
            assert!(edit(&l, &bad, &m).is_err(), "{bad}");
        }
        assert_eq!(std::fs::read_to_string(l.policy()).unwrap(), before);
        let pol = config::read_policy_file(&l.policy()).unwrap();
        assert_eq!(pol.mem_cap_gb, Some(6));
    }

    #[test]
    fn narrow_persists_and_reads_back() {
        let (l, _) = rig();
        assert_eq!(read_narrow(&l.config).unwrap(), NodeLimits::default());
        apply_narrow(&l.config, &json!({"mem_bytes": 4 * GIB, "max_parallel": 1})).unwrap();
        let n = read_narrow(&l.config).unwrap();
        assert_eq!((n.mem_bytes, n.max_parallel, n.cpu_millis), (Some(4 * GIB), Some(1), None));
        assert!(apply_narrow(&l.config, &json!({"mem_bytes": -1})).is_err());
        assert!(apply_narrow(&l.config, &json!({"disk": 1})).is_err());
        apply_narrow(&l.config, &json!({})).unwrap();
        assert_eq!(read_narrow(&l.config).unwrap(), NodeLimits::default(), "an empty narrow clears it");
    }
}
