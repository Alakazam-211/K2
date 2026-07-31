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

fn k2_home_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".k2")
}

/// Path to `~/.k2so/connect-hosts.json`.
fn hosts_path() -> PathBuf {
    k2_home_dir().join("connect-hosts.json")
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

    let dir = k2_home_dir();
    if !dir.exists() {
        fs::create_dir_all(&dir).map_err(|e| format!("create ~/.k2: {e}"))?;
    }
    let file = hosts_path();
    let tmp = file.with_extension("json.tmp");
    fs::write(&tmp, json.as_bytes()).map_err(|e| format!("write {tmp:?}: {e}"))?;
    restrict_mode(&tmp);
    fs::rename(&tmp, &file).map_err(|e| format!("rename {tmp:?} -> {file:?}: {e}"))?;
    restrict_mode(&file);
    Ok(())
}

// ── CLI token mirror (`~/.k2/connect-tokens.json`) ─────────────────────
//
// The bash CLI (`k2 msg agent::host --inbox-silent <file>`) cannot read
// the OS keychain the same way Tauri does. It resolves destination-host
// tokens from `connect-tokens.json` (0600). When the desktop remembers a
// Connect session token, mirror it here keyed by hostname so agents and
// CLI tray file send work without a hand-written token file (GH #60).

fn tokens_path() -> PathBuf {
    k2_home_dir().join("connect-tokens.json")
}

fn read_tokens_map() -> Result<serde_json::Map<String, serde_json::Value>, String> {
    let file = tokens_path();
    if !file.exists() {
        return Ok(serde_json::Map::new());
    }
    let raw = fs::read_to_string(&file).map_err(|e| format!("read connect-tokens.json: {e}"))?;
    let parsed: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("parse connect-tokens.json: {e}"))?;
    match parsed {
        serde_json::Value::Object(m) => Ok(m),
        _ => Err("connect-tokens.json must be a JSON object".to_string()),
    }
}

fn write_tokens_map(map: &serde_json::Map<String, serde_json::Value>) -> Result<(), String> {
    let dir = k2_home_dir();
    if !dir.exists() {
        fs::create_dir_all(&dir).map_err(|e| format!("create ~/.k2: {e}"))?;
    }
    let file = tokens_path();
    let tmp = file.with_extension("json.tmp");
    let body = serde_json::to_string_pretty(&serde_json::Value::Object(map.clone()))
        .map_err(|e| format!("serialize connect-tokens: {e}"))?;
    fs::write(&tmp, body.as_bytes()).map_err(|e| format!("write {tmp:?}: {e}"))?;
    restrict_mode(&tmp);
    fs::rename(&tmp, &file).map_err(|e| format!("rename {tmp:?} -> {file:?}: {e}"))?;
    restrict_mode(&file);
    Ok(())
}

/// Upsert a hostname → session token for CLI tray / remote verbs.
///
/// `hostname` is the Connect host name (e.g. `claimchaser.k2.dev`). Empty
/// hostname or token is rejected. Never logs the token value.
#[tauri::command]
pub fn connect_cli_token_upsert(hostname: String, token: String) -> Result<(), String> {
    let host = hostname.trim().to_string();
    if host.is_empty() {
        return Err("hostname is empty".to_string());
    }
    if token.is_empty() {
        return Err("token is empty".to_string());
    }
    let mut map = read_tokens_map()?;
    map.insert(host, serde_json::Value::String(token));
    write_tokens_map(&map)?;
    log_debug!("[connect-tokens] upserted CLI token for a host (value not logged)");
    Ok(())
}

/// Remove a hostname's CLI token mirror (sign-out / forget).
///
/// Idempotent: missing file or key is success.
#[tauri::command]
pub fn connect_cli_token_delete(hostname: String) -> Result<(), String> {
    let host = hostname.trim().to_string();
    if host.is_empty() {
        return Ok(());
    }
    let file = tokens_path();
    if !file.exists() {
        return Ok(());
    }
    let mut map = read_tokens_map()?;
    if map.remove(&host).is_none() {
        // Also try bare / .k2.dev variants so forget matches upsert keying.
        let bare = host
            .trim_end_matches(".k2.dev")
            .trim_end_matches(".K2.DEV")
            .to_string();
        map.remove(&bare);
        map.remove(&format!("{bare}.k2.dev"));
    }
    write_tokens_map(&map)?;
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
    fn cli_token_upsert_round_trips_and_delete() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let dir = tmp_home("cli-tok");
        let _home = HomeGuard::set(&dir);
        connect_cli_token_upsert(
            "claimchaser.k2.dev".to_string(),
            "sess-abc".to_string(),
        )
        .unwrap();
        let map = read_tokens_map().unwrap();
        assert_eq!(
            map.get("claimchaser.k2.dev").and_then(|v| v.as_str()),
            Some("sess-abc")
        );
        connect_cli_token_delete("claimchaser.k2.dev".to_string()).unwrap();
        let map2 = read_tokens_map().unwrap();
        assert!(map2.get("claimchaser.k2.dev").is_none());
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
