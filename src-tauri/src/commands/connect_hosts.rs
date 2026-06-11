//! K2 Connect client address book persistence — `~/.k2so/connect-hosts.json`
//! (PRD §1, §3 of `.k2so/prds/k2-connect-client-ux.md`, build order step #3).
//!
//! The NON-SECRET half of a saved [`ConnectHost`]: label, hostname, port,
//! secure flag, remember flag, lastConnectedAt. The auth TOKEN never lives
//! here — it goes to the OS keychain via `commands::secrets`
//! (`k2_secret_{set,get,delete}`), keyed by host id. This split is the
//! security invariant: a leaked/synced connect-hosts.json reveals which
//! servers a user connects to, but no credentials.
//!
//! ## Why a Tauri command, not a daemon route?
//!
//! `feedback_daemon_first.md` says logic belongs in the daemon, but this
//! is the same explicit exception as `worktree.rs`: connect-hosts.json is
//! a CLIENT-side config the renderer reads at boot to populate the server
//! switcher — BEFORE any daemon (local or remote) has even been chosen.
//! It describes which daemons to talk to; it can't itself depend on one.
//! It's a pure local-filesystem read/write of renderer config, so a thin
//! Tauri shim is the right home.
//!
//! The renderer owns the JSON SHAPE (camelCase `ConnectHost` minus token);
//! this command treats the body as an opaque JSON array and only validates
//! that it parses as a JSON array, so the schema can evolve renderer-side
//! without a Rust change. It is NOT secret, so it is fine to log the path
//! (never any token — tokens are never in this file).

use std::fs;
use std::path::{Path, PathBuf};

fn k2so_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".k2")
}

/// Path to `~/.k2so/connect-hosts.json`.
fn hosts_path() -> PathBuf {
    k2so_dir().join("connect-hosts.json")
}

#[cfg(unix)]
fn restrict_mode(file: &Path) {
    use std::os::unix::fs::PermissionsExt;
    // 0600 even though there are no secrets here — the address book is
    // still user-private; defense in depth + consistency with tunnel.json.
    let _ = fs::set_permissions(file, fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_mode(_file: &Path) {}

/// Read `~/.k2so/connect-hosts.json` and return its raw JSON text.
///
/// Missing file → `"[]"` so the renderer always gets a valid JSON array
/// to parse (empty address book). Malformed/unreadable file is surfaced
/// as `Err` so the renderer can decide whether to warn — we do NOT
/// silently overwrite a corrupt file on read.
#[tauri::command]
pub fn connect_hosts_read() -> Result<String, String> {
    let file = hosts_path();
    if !file.exists() {
        return Ok("[]".to_string());
    }
    fs::read_to_string(&file).map_err(|e| format!("read connect-hosts.json: {e}"))
}

/// Write the (token-less) host list to `~/.k2so/connect-hosts.json` via
/// tmp+rename, chmod 0600.
///
/// `json` is the renderer-owned serialized `ConnectHost[]` MINUS the
/// token field. We validate it parses as a JSON ARRAY (rejecting a stray
/// object/string) so a renderer bug can't write garbage that breaks the
/// next read, but we don't otherwise inspect the shape — the renderer
/// owns the schema.
#[tauri::command]
pub fn connect_hosts_write(json: String) -> Result<(), String> {
    // Validate shape: must be a JSON array.
    let parsed: serde_json::Value =
        serde_json::from_str(&json).map_err(|e| format!("connect-hosts must be valid JSON: {e}"))?;
    if !parsed.is_array() {
        return Err("connect-hosts payload must be a JSON array".to_string());
    }
    // Belt-and-suspenders: a correctly-built payload never carries a
    // `token`, but reject one outright so a renderer regression can't
    // leak a secret into this plaintext file.
    if let Some(arr) = parsed.as_array() {
        for entry in arr {
            if entry.get("token").is_some() {
                return Err(
                    "connect-hosts entries must not contain a token (tokens go to the keychain)"
                        .to_string(),
                );
            }
        }
    }

    let dir = k2so_dir();
    if !dir.exists() {
        fs::create_dir_all(&dir).map_err(|e| format!("create ~/.k2so: {e}"))?;
    }
    let file = hosts_path();
    let tmp = file.with_extension("json.tmp");
    fs::write(&tmp, json.as_bytes()).map_err(|e| format!("write {tmp:?}: {e}"))?;
    restrict_mode(&tmp);
    fs::rename(&tmp, &file).map_err(|e| format!("rename {tmp:?} -> {file:?}: {e}"))?;
    restrict_mode(&file);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // connect-hosts.json is HOME-relative. Serialize these tests against
    // each other with a module-local lock so cargo's parallel runner
    // can't race two of them swapping `$HOME`. (k2so-core's crate-wide
    // HOME_LOCK is `pub(crate)` and not reachable from this crate; no
    // OTHER test in the k2so crate touches ~/.k2so/connect-hosts.json,
    // so a module-local lock is sufficient here.)
    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct HomeGuard {
        prev: Option<std::ffi::OsString>,
    }
    impl HomeGuard {
        fn set(dir: &Path) -> Self {
            let prev = std::env::var_os("HOME");
            std::env::set_var("HOME", dir);
            Self { prev }
        }
    }
    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match self.prev.take() {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
    }

    fn tmp_home(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "k2so-connhosts-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn read_returns_empty_array_when_missing() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let dir = tmp_home("read-missing");
        let _home = HomeGuard::set(&dir);
        assert_eq!(connect_hosts_read().unwrap(), "[]");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_then_read_round_trips() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let dir = tmp_home("rt");
        let _home = HomeGuard::set(&dir);
        // connect-users (#617): `username` is a non-secret field that
        // persists here alongside hostname/port (the password + session
        // token stay in the keychain).
        let payload = r#"[{"id":"host-1","label":"Hetzner","hostname":"rosson.k2.dev","port":443,"username":"rosson","secure":true,"remember":true,"lastConnectedAt":null}]"#;
        connect_hosts_write(payload.to_string()).unwrap();
        let back = connect_hosts_read().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&back).unwrap();
        assert_eq!(parsed[0]["id"], serde_json::json!("host-1"));
        assert_eq!(parsed[0]["hostname"], serde_json::json!("rosson.k2.dev"));
        assert_eq!(parsed[0]["username"], serde_json::json!("rosson"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_rejects_non_array() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let dir = tmp_home("non-array");
        let _home = HomeGuard::set(&dir);
        assert!(connect_hosts_write("{}".to_string()).is_err());
        assert!(connect_hosts_write("\"nope\"".to_string()).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_rejects_entry_with_token() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let dir = tmp_home("token-leak");
        let _home = HomeGuard::set(&dir);
        // The security invariant: a token must NEVER reach this file.
        let leaky = r#"[{"id":"h","label":"x","hostname":"h","port":443,"token":"secret","remember":true,"lastConnectedAt":null}]"#;
        let err = connect_hosts_write(leaky.to_string()).unwrap_err();
        assert!(err.contains("token"), "error must call out the token: {err}");
        // And nothing was written.
        assert_eq!(connect_hosts_read().unwrap(), "[]");
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn write_chmods_0600() {
        use std::os::unix::fs::PermissionsExt;
        let _g = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let dir = tmp_home("mode");
        let _home = HomeGuard::set(&dir);
        connect_hosts_write("[]".to_string()).unwrap();
        let mode = fs::metadata(hosts_path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let _ = fs::remove_dir_all(&dir);
    }
}
