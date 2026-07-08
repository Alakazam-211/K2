//! Mail secret storage — the store behind `mail_server.admin_secret_ref`
//! / `api_key_ref` (+ future `mail_relay_configs.secret_ref`).
//!
//! The 0072 schema promised "references into the daemon's secret
//! storage"; the repo convention for daemon-held secrets is an
//! owner-only 0600 JSON file written tmp+rename (the
//! `k2_core::tunnel::config` pattern — `~/.k2/tunnel.json` holds the
//! tunnel bearer token exactly this way). This module is that store
//! for the mail family: `~/.k2/mail-secrets.json`, a flat
//! `ref → secret` map. Refs are opaque (`mailsec_<kind>_<hex>`) and
//! safe to persist in the DB / print in logs; secrets never are.
//!
//! Behind the [`SecretStore`] trait so the S1 enable state machine is
//! unit-tested with a recording fake — tests never touch the real
//! `~/.k2` (house rule: no real fs side effects in tests).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// Store/resolve/delete for mail secrets. `store` returns the opaque
/// REF that goes into the `mail_server` row; `resolve` turns a ref
/// back into the secret (None = unknown ref — treat as missing, not
/// an error, so a purged store degrades to "re-bootstrap needed").
pub trait SecretStore: Send + Sync {
    fn store(&self, kind: &str, secret: &str) -> Result<String, String>;
    fn resolve(&self, sref: &str) -> Result<Option<String>, String>;
    fn delete(&self, sref: &str) -> Result<(), String>;
}

/// The real 0600-file store. `Default` points at
/// `~/.k2/mail-secrets.json`; tests construct with a temp path.
pub struct FileSecretStore {
    path: PathBuf,
}

impl Default for FileSecretStore {
    fn default() -> Self {
        let dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".k2");
        Self { path: dir.join("mail-secrets.json") }
    }
}

impl FileSecretStore {
    #[cfg(test)]
    pub fn at(path: PathBuf) -> Self {
        Self { path }
    }

    /// Whole-file read. Missing file = empty map; malformed file is an
    /// ERROR (fail loud — never silently fall back over a corrupt
    /// secret store; tunnel/config.rs sets the precedent).
    fn load(&self) -> Result<serde_json::Map<String, serde_json::Value>, String> {
        if !self.path.exists() {
            return Ok(serde_json::Map::new());
        }
        let raw = fs::read_to_string(&self.path)
            .map_err(|e| format!("read {}: {e}", self.path.display()))?;
        if raw.trim().is_empty() {
            return Ok(serde_json::Map::new());
        }
        let v: serde_json::Value = serde_json::from_str(&raw)
            .map_err(|e| format!("parse {}: {e}", self.path.display()))?;
        match v {
            serde_json::Value::Object(map) => Ok(map),
            _ => Err(format!("{}: not a JSON object", self.path.display())),
        }
    }

    /// tmp+rename persist, chmod 0600 on both tmp and final (tunnel
    /// pattern) so the secrets are owner-only from first byte.
    fn save(&self, map: &serde_json::Map<String, serde_json::Value>) -> Result<(), String> {
        let dir = self
            .path
            .parent()
            .ok_or_else(|| format!("{}: no parent dir", self.path.display()))?;
        fs::create_dir_all(dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
        let tmp = dir.join(format!(
            "mail-secrets.json.tmp.{}",
            std::process::id()
        ));
        let body = serde_json::to_string_pretty(&serde_json::Value::Object(map.clone()))
            .map_err(|e| format!("serialize mail secrets: {e}"))?;
        fs::write(&tmp, body.as_bytes()).map_err(|e| format!("write {}: {e}", tmp.display()))?;
        restrict_mode(&tmp);
        fs::rename(&tmp, &self.path).map_err(|e| {
            let _ = fs::remove_file(&tmp);
            format!("rename into place {}: {e}", self.path.display())
        })?;
        restrict_mode(&self.path);
        Ok(())
    }
}

/// Process-wide lock serializing read-modify-write cycles (same shape
/// as `tunnel::config::update`).
fn store_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

impl SecretStore for FileSecretStore {
    fn store(&self, kind: &str, secret: &str) -> Result<String, String> {
        let _g = store_lock().lock().unwrap_or_else(|p| p.into_inner());
        let sref = format!("mailsec_{kind}_{}", random_hex_12());
        let mut map = self.load()?;
        map.insert(sref.clone(), serde_json::Value::String(secret.to_string()));
        self.save(&map)?;
        Ok(sref)
    }

    fn resolve(&self, sref: &str) -> Result<Option<String>, String> {
        let _g = store_lock().lock().unwrap_or_else(|p| p.into_inner());
        Ok(self
            .load()?
            .get(sref)
            .and_then(|v| v.as_str())
            .map(str::to_string))
    }

    fn delete(&self, sref: &str) -> Result<(), String> {
        let _g = store_lock().lock().unwrap_or_else(|p| p.into_inner());
        let mut map = self.load()?;
        if map.remove(sref).is_some() {
            self.save(&map)?;
        }
        Ok(())
    }
}

#[cfg(unix)]
fn restrict_mode(file: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(file, fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_mode(_file: &Path) {}

/// 12 hex chars of cryptographic randomness for the ref suffix
/// (clone_routes' `new_pack_job_id` idiom, with the same nanos
/// fallback: refs must be unique, not unguessable — the SECRET is the
/// secret).
fn random_hex_12() -> String {
    let mut bytes = [0u8; 6];
    if getrandom::getrandom(&mut bytes).is_err() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        return format!("{nanos:012x}");
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A cryptographically random secret value (rotated admin password,
/// service passwords): 32 bytes → 64 hex chars. Errors loudly if the
/// OS RNG is unavailable — we never mint a weak secret.
pub fn generate_secret() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).map_err(|e| format!("os rng unavailable: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> (FileSecretStore, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "k2-mail-secrets-test-{}-{}",
            std::process::id(),
            random_hex_12()
        ));
        (FileSecretStore::at(dir.join("mail-secrets.json")), dir)
    }

    #[test]
    fn store_resolve_delete_roundtrip_and_0600() {
        let (store, dir) = temp_store();
        let r1 = store.store("api-key", "s3cret-one").expect("store");
        let r2 = store.store("admin", "s3cret-two").expect("store");
        assert!(r1.starts_with("mailsec_api-key_"), "{r1}");
        assert!(r2.starts_with("mailsec_admin_"), "{r2}");
        assert_ne!(r1, r2);
        assert_eq!(store.resolve(&r1).expect("resolve"), Some("s3cret-one".into()));
        assert_eq!(store.resolve(&r2).expect("resolve"), Some("s3cret-two".into()));
        assert_eq!(store.resolve("mailsec_unknown_0").expect("resolve"), None);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(dir.join("mail-secrets.json"))
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "secrets file must be owner-only");
        }

        store.delete(&r1).expect("delete");
        assert_eq!(store.resolve(&r1).expect("resolve"), None);
        assert_eq!(
            store.resolve(&r2).expect("resolve"),
            Some("s3cret-two".into()),
            "deleting one ref must not disturb others"
        );
        // Deleting an unknown ref is a no-op, not an error.
        store.delete("mailsec_unknown_0").expect("delete unknown");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn corrupt_store_fails_loudly() {
        let (store, dir) = temp_store();
        fs::create_dir_all(&dir).expect("mkdir");
        fs::write(dir.join("mail-secrets.json"), b"{ not json").expect("write");
        let err = store.resolve("mailsec_x_0").expect_err("must fail loud");
        assert!(err.contains("parse"), "{err}");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn generated_secrets_are_long_and_unique() {
        let a = generate_secret().expect("rng");
        let b = generate_secret().expect("rng");
        assert_eq!(a.len(), 64);
        assert_ne!(a, b);
    }
}
