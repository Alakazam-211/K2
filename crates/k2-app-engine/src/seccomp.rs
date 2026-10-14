//! K2's seccomp profile for guarded Apps (`prd-k2-app-runtime-v1` §4.2
//! item 3, stage W §5.6): Podman's default profile from the distro
//! (`containers-common`, so it tracks new syscalls with the distro's
//! Podman) **plus** K2's denies. Versioned by [`PROFILE_VERSION`]; the
//! engine writes the result under its own state dir at start.
//!
//! W measured that the default lets `unshare -U` through. The denies:
//! - `unshare`, `keyctl`, `add_key`, `request_key`, `bpf`, `userfaultfd`,
//!   `perf_event_open`: `EPERM`;
//! - `clone` only without `CLONE_NEWUSER` (the default's own `clone` rules
//!   are replaced);
//! - `clone3`: `ENOSYS`, so libc falls back to `clone` (whose flags a
//!   filter can read; `clone3` passes them in memory).
//!
//! A rule is added only where the profile's default action wouldn't
//! already give the same answer (libseccomp refuses a rule whose action
//! equals the default).

use serde_json::{json, Value};

pub const PROFILE_VERSION: u32 = 1;
/// Where the distro ships Podman's default.
pub const DISTRO_DEFAULT: &str = "/usr/share/containers/seccomp.json";

pub const CLONE_NEWUSER: u64 = 0x1000_0000;
pub const EPERM: u64 = 1;
pub const ENOSYS: u64 = 38;

/// Denied outright (`EPERM`).
pub const DENY: &[&str] = &["unshare", "keyctl", "add_key", "request_key", "bpf", "userfaultfd", "perf_event_open"];

/// The profile file name for this version.
pub fn file_name() -> String {
    format!("seccomp-k2-v{PROFILE_VERSION}.json")
}

/// `Some(errno)` when the profile's default action is `SCMP_ACT_ERRNO`.
fn default_errno(p: &Value) -> Option<u64> {
    if p.get("defaultAction").and_then(Value::as_str) != Some("SCMP_ACT_ERRNO") {
        return None;
    }
    // libseccomp/runc/crun: no defaultErrnoRet means EPERM.
    Some(p.get("defaultErrnoRet").and_then(Value::as_u64).unwrap_or(EPERM))
}

/// Podman's default + K2's denies. `Err` when the input isn't a profile
/// with a deny-by-default action (K2 never loosens a profile, and a
/// default-allow profile can't be fixed by appending).
pub fn transform(default_profile: &str) -> Result<String, String> {
    let mut p: Value = serde_json::from_str(default_profile).map_err(|e| format!("default seccomp profile unreadable: {e}"))?;
    let derr = default_errno(&p).ok_or("the default seccomp profile is not deny-by-default (SCMP_ACT_ERRNO)")?;
    let syscalls = p
        .get_mut("syscalls")
        .and_then(Value::as_array_mut)
        .ok_or("the default seccomp profile has no syscalls list")?;
    let drop_names = |n: &str| DENY.contains(&n) || n == "clone" || n == "clone3";
    let mut kept = Vec::with_capacity(syscalls.len());
    for mut rule in syscalls.drain(..) {
        if let Some(names) = rule.get_mut("names").and_then(Value::as_array_mut) {
            names.retain(|n| !n.as_str().is_some_and(drop_names));
            if names.is_empty() {
                continue;
            }
        } else if rule.get("name").and_then(Value::as_str).is_some_and(drop_names) {
            continue;
        }
        kept.push(rule);
    }
    // clone without CLONE_NEWUSER: (flags & CLONE_NEWUSER) == 0. Flags are
    // argument 0 on x86_64 and aarch64 (the arches K2 ships).
    kept.push(json!({
        "names": ["clone"],
        "action": "SCMP_ACT_ALLOW",
        "args": [{"index": 0, "value": CLONE_NEWUSER, "valueTwo": 0, "op": "SCMP_CMP_MASKED_EQ"}],
        "comment": "K2: clone, never a new user namespace",
    }));
    if derr != ENOSYS {
        kept.push(json!({
            "names": ["clone3"],
            "action": "SCMP_ACT_ERRNO",
            "errnoRet": ENOSYS,
            "comment": "K2: clone3 hides its flags; ENOSYS makes libc use clone",
        }));
    }
    if derr != EPERM {
        kept.push(json!({
            "names": DENY,
            "action": "SCMP_ACT_ERRNO",
            "errnoRet": EPERM,
            "comment": "K2: no user namespaces, keyrings, bpf, userfaultfd or perf for Apps",
        }));
    }
    *syscalls = kept;
    serde_json::to_string_pretty(&p).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "defaultAction": "SCMP_ACT_ERRNO",
      "defaultErrnoRet": 38,
      "archMap": [{"architecture": "SCMP_ARCH_X86_64", "subArchitectures": ["SCMP_ARCH_X86"]}],
      "syscalls": [
        {"names": ["read", "write", "unshare", "clone", "keyctl", "setns"], "action": "SCMP_ACT_ALLOW"},
        {"names": ["clone"], "action": "SCMP_ACT_ALLOW", "args": [{"index": 0, "value": 2114060288, "op": "SCMP_CMP_MASKED_EQ"}], "excludes": {"caps": ["CAP_SYS_ADMIN"]}},
        {"names": ["clone3"], "action": "SCMP_ACT_ERRNO", "errnoRet": 38},
        {"names": ["bpf", "perf_event_open"], "action": "SCMP_ACT_ALLOW", "includes": {"caps": ["CAP_SYS_ADMIN"]}}
      ]
    }"#;

    fn names_with_action(p: &Value, action: &str) -> Vec<String> {
        p["syscalls"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["action"] == action)
            .flat_map(|r| r["names"].as_array().unwrap().iter().map(|n| n.as_str().unwrap().to_string()))
            .collect()
    }

    #[test]
    fn denies_land_and_nothing_else_moves() {
        let out: Value = serde_json::from_str(&transform(SAMPLE).unwrap()).unwrap();
        let allow = names_with_action(&out, "SCMP_ACT_ALLOW");
        for kept in ["read", "write", "setns"] {
            assert!(allow.contains(&kept.to_string()), "{kept}");
        }
        for gone in ["unshare", "keyctl", "bpf", "perf_event_open", "clone3"] {
            assert!(!allow.contains(&gone.to_string()), "{gone} still allowed");
        }
        // Exactly one clone rule: ours, masked on CLONE_NEWUSER.
        let clones: Vec<&Value> = out["syscalls"].as_array().unwrap().iter().filter(|r| r["names"].as_array().unwrap().iter().any(|n| n == "clone")).collect();
        assert_eq!(clones.len(), 1);
        assert_eq!(clones[0]["args"][0]["value"], CLONE_NEWUSER);
        assert_eq!(clones[0]["args"][0]["op"], "SCMP_CMP_MASKED_EQ");
        // Default ENOSYS already covers clone3; EPERM denies are explicit.
        assert!(!out["syscalls"].as_array().unwrap().iter().any(|r| r["names"].as_array().unwrap().iter().any(|n| n == "clone3")));
        let eperm = names_with_action(&out, "SCMP_ACT_ERRNO");
        for d in DENY {
            assert!(eperm.contains(&d.to_string()), "{d}");
        }
        assert_eq!(out["archMap"], serde_json::from_str::<Value>(SAMPLE).unwrap()["archMap"]);
    }

    #[test]
    fn an_eperm_default_gets_an_explicit_clone3_enosys() {
        let s = SAMPLE.replace("\"defaultErrnoRet\": 38,", "");
        let out: Value = serde_json::from_str(&transform(&s).unwrap()).unwrap();
        let c3: Vec<&Value> = out["syscalls"].as_array().unwrap().iter().filter(|r| r["names"].as_array().unwrap().iter().any(|n| n == "clone3")).collect();
        assert_eq!(c3.len(), 1);
        assert_eq!(c3[0]["errnoRet"], ENOSYS);
        assert!(!out["syscalls"].as_array().unwrap().iter().any(|r| r["names"].as_array().unwrap().iter().any(|n| n == "unshare")), "EPERM is the default: removal is the deny");
    }

    #[test]
    fn refuses_what_it_cant_tighten() {
        assert!(transform("{}").is_err());
        assert!(transform(r#"{"defaultAction":"SCMP_ACT_ALLOW","syscalls":[]}"#).is_err());
        assert!(transform("not json").is_err());
    }
}
