//! "This computer" (§16.1): the node machine's own K2 daemon reads the
//! node's status and writes the machine owner's word: pause / drain /
//! stop / resume and policy. It is never in the job path.
//!
//! The owner's word lives in the node's config dir (`/etc/k2-node`), which
//! `k2node` can't write (its jobs run as the same uid, so anything
//! `k2-node` could write a job could rewrite). The installer makes the
//! dir `root:k2nodectl 0775` and adds the enabling human to `k2nodectl`,
//! so this daemon (running as that human) writes the two files directly;
//! `k2-node` polls them. Status comes from `<home>/run/status.json`,
//! which `k2-node` writes.

use std::path::{Path, PathBuf};

use super::proto::frames::Control;

/// Node home (`K2_NODE_HOME` overrides for tests and dev).
pub fn node_home() -> PathBuf {
    if let Some(p) = std::env::var_os("K2_NODE_HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(p);
    }
    if cfg!(target_os = "macos") {
        PathBuf::from("/var/k2node")
    } else {
        PathBuf::from("/var/lib/k2node")
    }
}

/// Node config dir (`K2_NODE_CONFIG_DIR` overrides).
pub fn config_dir() -> PathBuf {
    std::env::var_os("K2_NODE_CONFIG_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/etc/k2-node"))
}

fn read_toml(p: &Path) -> Option<toml::Table> {
    std::fs::read_to_string(p).ok()?.parse::<toml::Table>().ok()
}

fn write_atomic(p: &Path, body: &str) -> Result<(), String> {
    let dir = p.parent().ok_or("no parent dir")?;
    if !dir.is_dir() {
        return Err(format!(
            "{} doesn't exist: this computer isn't a compute node (see k2 compute local enable)",
            dir.display()
        ));
    }
    let tmp = dir.join(format!(".{}.tmp-{}", p.file_name().and_then(|s| s.to_str()).unwrap_or("f"), std::process::id()));
    std::fs::write(&tmp, body).map_err(|e| match e.kind() {
        std::io::ErrorKind::PermissionDenied => format!(
            "no permission to write {}: add yourself to the k2nodectl group (the installer does), or use sudo k2-node",
            dir.display()
        ),
        _ => format!("write {}: {e}", tmp.display()),
    })?;
    std::fs::rename(&tmp, p).map_err(|e| format!("replace {}: {e}", p.display()))
}

/// Everything the local status route shows.
pub fn status() -> serde_json::Value {
    let home = node_home();
    let cfg = config_dir();
    let status: Option<serde_json::Value> = std::fs::read_to_string(home.join("run/status.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok());
    let control = read_toml(&cfg.join("control.toml"));
    let policy = read_toml(&cfg.join("policy.toml"));
    serde_json::json!({
        "installed": cfg.is_dir(),
        "home": home.display().to_string(),
        "configDir": cfg.display().to_string(),
        "node": status,
        "control": control.map(|t| serde_json::to_value(t).unwrap_or_default()),
        "policy": policy.map(|t| serde_json::to_value(t).unwrap_or_default()),
    })
}

/// Write `control.toml`.
pub fn set_control(state: Control, by: &str) -> Result<(), String> {
    let by: String = by.chars().filter(|c| !c.is_control() && *c != '"' && *c != '\\').take(64).collect();
    let body = format!("state = \"{}\"\nby = \"{}\"\nat = {}\n", state.as_str(), by, super::now());
    write_atomic(&config_dir().join("control.toml"), &body)
}

/// Policy keys this route may set, with their type.
const POLICY_KEYS: &[(&str, &str)] = &[
    ("cpu_cap_percent", "int"),
    ("mem_cap_gb", "int"),
    ("disk_budget_gb", "int"),
    ("disk_floor_gb", "int"),
    ("availability", "enum"),
    ("on_owner_return", "enum"),
    ("max_parallel", "int"),
    ("network", "enum"),
];

fn enum_ok(key: &str, v: &str) -> bool {
    match key {
        "availability" => matches!(v, "always" | "idle" | "plugged_in" | "idle_and_plugged_in" | "window"),
        "on_owner_return" => matches!(v, "finish" | "pause_now"),
        "network" => matches!(v, "full" | "none"),
        _ => false,
    }
}

/// Merge `updates` into `policy.toml`. Unknown keys and bad values are
/// refused before anything is written.
pub fn set_policy(updates: &[(String, String)]) -> Result<toml::Table, String> {
    let p = config_dir().join("policy.toml");
    let mut t = read_toml(&p).unwrap_or_default();
    for (k, v) in updates {
        let Some((_, kind)) = POLICY_KEYS.iter().find(|(name, _)| name == k) else {
            return Err(format!("unknown policy key '{k}'"));
        };
        let val = match *kind {
            "int" => {
                let n: i64 = v.trim().trim_end_matches(['%', 'G', 'g']).parse().map_err(|_| format!("{k} needs a number"))?;
                let max = if k == "cpu_cap_percent" { 100 } else { 1 << 20 };
                if n < 0 || n > max || (k == "cpu_cap_percent" && n == 0) {
                    return Err(format!("{k} is out of range"));
                }
                toml::Value::Integer(n)
            }
            _ => {
                let s = v.trim().replace('-', "_");
                if !enum_ok(k, &s) {
                    return Err(format!("{k} can't be '{v}'"));
                }
                toml::Value::String(s)
            }
        };
        t.insert(k.clone(), val);
    }
    let body = toml::to_string(&t).map_err(|e| e.to_string())?;
    write_atomic(&p, &body)?;
    Ok(t)
}

/// What enabling this computer takes (dogfood A: a CLI one-liner run as
/// root; the Settings sheet and the one-click account path come later).
pub fn enable_steps(controller: &str, enroll: &str, name: &str) -> serde_json::Value {
    let os = if cfg!(target_os = "macos") { "macos" } else { "linux" };
    let line = format!(
        "sudo scripts/node/install-node-{os}.sh --binary <path to k2-node> --controller {controller} --enroll {enroll} --name {name} --human \"$USER\""
    );
    serde_json::json!({
        "os": os,
        "rootNeeded": true,
        "command": line,
        "message": "Run this once as an admin. It creates the hidden k2node user and the service, enrolls the node and prints the confirmation code to check on the controller.",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn with_dirs(f: impl FnOnce(&Path, &Path)) {
        let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let base = std::env::temp_dir().join(format!("k2-compute-local-{}-{}", std::process::id(), super::super::now()));
        let home = base.join("home");
        let cfg = base.join("etc");
        std::fs::create_dir_all(home.join("run")).unwrap();
        std::fs::create_dir_all(&cfg).unwrap();
        std::env::set_var("K2_NODE_HOME", &home);
        std::env::set_var("K2_NODE_CONFIG_DIR", &cfg);
        f(&home, &cfg);
        std::env::remove_var("K2_NODE_HOME");
        std::env::remove_var("K2_NODE_CONFIG_DIR");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn control_and_policy_round_trip_through_status() {
        with_dirs(|home, cfg| {
            std::fs::write(home.join("run/status.json"), r#"{"state":"online","name":"mini-1"}"#).unwrap();
            set_control(Control::Paused, "rosson\"evil").unwrap();
            let c = std::fs::read_to_string(cfg.join("control.toml")).unwrap();
            let t: toml::Table = c.parse().unwrap();
            assert_eq!(t["state"].as_str(), Some("paused"));
            assert_eq!(t["by"].as_str(), Some("rossonevil"), "quotes can't break the file");
            let p = set_policy(&[("cpu_cap_percent".into(), "60%".into()), ("availability".into(), "plugged-in".into())]).unwrap();
            assert_eq!(p["cpu_cap_percent"].as_integer(), Some(60));
            assert_eq!(p["availability"].as_str(), Some("plugged_in"));
            assert!(set_policy(&[("foreign_locks".into(), "x".into())]).is_err(), "only listed keys");
            assert!(set_policy(&[("cpu_cap_percent".into(), "0".into())]).is_err());
            assert!(set_policy(&[("availability".into(), "sometimes".into())]).is_err());
            let s = status();
            assert_eq!(s["installed"], true);
            assert_eq!(s["node"]["state"], "online");
            assert_eq!(s["control"]["state"], "paused");
            assert_eq!(s["policy"]["cpu_cap_percent"], 60);
        });
    }

    #[test]
    fn writing_without_a_config_dir_says_not_a_node() {
        with_dirs(|_, cfg| {
            std::fs::remove_dir_all(cfg).unwrap();
            let e = set_control(Control::Paused, "x").unwrap_err();
            assert!(e.contains("isn't a compute node"), "{e}");
            assert_eq!(status()["installed"], false);
        });
    }
}
