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
        "platform": platform(),
        "enrollResult": enroll_result(),
        "enrollRequestPending": cfg.join("enroll-request.toml").exists(),
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
    ("repo_toolchains", "bool"),
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
            "bool" => match v.trim() {
                "true" | "on" | "yes" | "1" => toml::Value::Boolean(true),
                "false" | "off" | "no" | "0" => toml::Value::Boolean(false),
                _ => return Err(format!("{k} is true or false")),
            },
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

/// `linux` | `macos`.
pub fn platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

/// Spots in a home that jobs could read when the home is open to others.
pub const HOME_SPOTS: &[&str] = &[".ssh", ".k2", ".env", ".config", ".aws", ".gnupg", "DevProjects", "Projects", "projects", "src", "code", "Documents", "Desktop"];

/// The home-folder check the enable sheet shows (§5.1 step 3): is the
/// human's home readable by other users (and so by jobs), and what's in it.
pub fn home_check(home: &Path) -> Option<serde_json::Value> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(home).ok()?.permissions().mode() & 0o7777;
    let private = mode & 0o005 == 0;
    let readable: Vec<String> = if private {
        Vec::new()
    } else {
        HOME_SPOTS.iter().map(|s| home.join(s)).filter(|p| p.exists()).map(|p| p.display().to_string()).collect()
    };
    Some(serde_json::json!({
        "home": home.display().to_string(),
        "mode": format!("{mode:o}"),
        "private": private,
        "readable": readable,
    }))
}

/// Ask the running node to enroll itself (`enroll-request.toml` in the
/// config dir; `k2-node` reads it within seconds). Exactly one of
/// `enroll` (a code) or `ticket`. Returns the request id.
pub fn write_enroll_request(
    controller: &str,
    enroll: Option<&str>,
    ticket: Option<&str>,
    name: &str,
    by: &str,
) -> Result<String, String> {
    if enroll.is_some() == ticket.is_some() {
        return Err("pass an enroll code or a ticket".into());
    }
    if !super::enroll::valid_name(name) {
        return Err("node names are 1–32 of a-z, 0-9 and '-'".into());
    }
    if !(controller.starts_with("https://") || controller.starts_with("http://")) || controller.contains(['"', '\n', '\\']) {
        return Err("controller must be an http(s) URL".into());
    }
    let id = format!("k2-{}", super::proto::crypto::random_id()?);
    let mut t = toml::Table::new();
    t.insert("id".into(), toml::Value::String(id.clone()));
    t.insert("controller".into(), toml::Value::String(controller.trim_end_matches('/').to_string()));
    if let Some(e) = enroll {
        t.insert("enroll".into(), toml::Value::String(e.trim().to_string()));
    }
    if let Some(tk) = ticket {
        t.insert("ticket".into(), toml::Value::String(tk.trim().to_string()));
    }
    t.insert("name".into(), toml::Value::String(name.to_string()));
    let by: String = by.chars().filter(|c| !c.is_control()).take(64).collect();
    t.insert("requested_by".into(), toml::Value::String(by));
    t.insert("requested_at".into(), toml::Value::Integer(super::now()));
    let body = toml::to_string(&t).map_err(|e| e.to_string())?;
    // 0644: the node user reads it (it isn't in k2nodectl); it is
    // single-use and removed once the node answers.
    let p = config_dir().join("enroll-request.toml");
    write_atomic(&p, &body)?;
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644));
    Ok(id)
}

/// The machine owner's approved system packages (`/etc/k2-node-packages.toml`,
/// root-owned; `K2_NODE_PACKAGES_FILE` overrides for tests).
pub fn approved_packages() -> Vec<String> {
    let p = std::env::var_os("K2_NODE_PACKAGES_FILE")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/etc/k2-node-packages.toml"));
    read_toml(&p)
        .and_then(|t| t.get("approved").and_then(|a| a.as_array()).cloned())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

/// The node's answer to a request (`run/enroll-result.json`).
pub fn enroll_result() -> Option<serde_json::Value> {
    let s = std::fs::read_to_string(node_home().join("run/enroll-result.json")).ok()?;
    serde_json::from_str(&s).ok()
}

/// Remove the request file once answered (best effort).
pub fn clear_enroll_request() {
    let _ = std::fs::remove_file(config_dir().join("enroll-request.toml"));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_dirs(f: impl FnOnce(&Path, &Path)) {
        // The one env lock (re-entrant), held for the whole test.
        let _env = crate::test_env::lock();
        let base = std::env::temp_dir().join(format!("k2-compute-local-{}-{}", std::process::id(), super::super::now()));
        let home = base.join("home");
        let cfg = base.join("etc");
        std::fs::create_dir_all(home.join("run")).unwrap();
        std::fs::create_dir_all(&cfg).unwrap();
        {
            let _home = crate::test_env::EnvVar::set("K2_NODE_HOME", &home);
            let _cfg = crate::test_env::EnvVar::set("K2_NODE_CONFIG_DIR", &cfg);
            f(&home, &cfg);
        }
        let _ =std::fs::remove_dir_all(&base);
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
    fn enroll_requests_round_trip_and_home_check_reports_open_homes() {
        use std::os::unix::fs::PermissionsExt;
        with_dirs(|home, cfg| {
            let id = write_enroll_request("https://rosson.k2.dev/", Some("ABCDE-FGHJK.0123456789abcdef"), None, "mini-1", "rosson").unwrap();
            let p = cfg.join("enroll-request.toml");
            assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o644);
            let t: toml::Table = std::fs::read_to_string(&p).unwrap().parse().unwrap();
            assert_eq!(t["id"].as_str(), Some(id.as_str()));
            assert_eq!(t["controller"].as_str(), Some("https://rosson.k2.dev"));
            assert_eq!(t["enroll"].as_str(), Some("ABCDE-FGHJK.0123456789abcdef"));
            assert!(t.get("ticket").is_none());
            assert_eq!(status()["enrollRequestPending"], true);
            assert!(write_enroll_request("https://x.k2.dev", Some("a"), Some("b"), "n", "x").is_err(), "one credential only");
            assert!(write_enroll_request("https://x.k2.dev", None, None, "n", "x").is_err());
            assert!(write_enroll_request("ftp://x", Some("a"), None, "n", "x").is_err());
            assert!(write_enroll_request("https://x\"\n", Some("a"), None, "n", "x").is_err());
            assert!(write_enroll_request("https://x.k2.dev", Some("a"), None, "Bad Name", "x").is_err());
            std::fs::write(home.join("run/enroll-result.json"), format!(r#"{{"id":"{id}","ok":true,"sas":"123 456"}}"#)).unwrap();
            assert_eq!(status()["enrollResult"]["sas"], "123 456");
            clear_enroll_request();
            assert_eq!(status()["enrollRequestPending"], false);

            let h = home.join("human");
            std::fs::create_dir_all(h.join(".ssh")).unwrap();
            std::fs::set_permissions(&h, std::fs::Permissions::from_mode(0o755)).unwrap();
            let c = home_check(&h).unwrap();
            assert_eq!(c["private"], false);
            assert_eq!(c["mode"], "755");
            assert_eq!(c["readable"][0], h.join(".ssh").display().to_string());
            std::fs::set_permissions(&h, std::fs::Permissions::from_mode(0o700)).unwrap();
            let c = home_check(&h).unwrap();
            assert_eq!(c["private"], true);
            assert_eq!(c["readable"].as_array().unwrap().len(), 0);
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
