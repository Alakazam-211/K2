//! SQL sidecar secret storage — `~/.k2/db-secrets.json`, refs `dbsec_*`.
//!
//! Copied from `mail/secrets.rs` (0600 tmp+rename). Not mail-secrets.json.

use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

pub trait SecretStore: Send + Sync {
    fn store(&self, kind: &str, secret: &str) -> Result<String, String>;
    fn resolve(&self, sref: &str) -> Result<Option<String>, String>;
    #[allow(dead_code)]
    fn delete(&self, sref: &str) -> Result<(), String>;
}

/// The DB secret store. `Default` is the K2 vault (`system` secrets in
/// scope `engine:postgres`, KV11) whenever it's open; a box whose vault
/// couldn't open falls back to `~/.k2/db-secrets.json`. Tests get the
/// test vault (never the real home, KV12).
pub struct FileSecretStore {
    path: PathBuf,
    vault: Option<crate::vault::refstore::VaultRefStore>,
}

/// The old file location (adoption reads it once).
pub fn legacy_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".k2")
        .join("db-secrets.json")
}

impl Default for FileSecretStore {
    fn default() -> Self {
        #[cfg(test)]
        let vault = Some(crate::vault::ensure_test_vault());
        #[cfg(not(test))]
        let vault = crate::vault::get().ok();
        Self {
            path: legacy_path(),
            vault: vault.map(|v| crate::vault::refstore::VaultRefStore::new(v, "postgres", "db")),
        }
    }
}

impl FileSecretStore {
    #[cfg(test)]
    pub fn at(path: PathBuf) -> Self {
        Self { path, vault: None }
    }

    /// The old JSON file as a map (adoption).
    pub fn legacy_map(path: &std::path::Path) -> Result<serde_json::Map<String, serde_json::Value>, String> {
        Self { path: path.to_path_buf(), vault: None }.load()
    }

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

    /// Created 0600 (missing parent dir 0700) — never write-then-chmod.
    fn save(&self, map: &serde_json::Map<String, serde_json::Value>) -> Result<(), String> {
        let body = serde_json::to_string_pretty(&serde_json::Value::Object(map.clone()))
            .map_err(|e| format!("serialize db secrets: {e}"))?;
        k2_core::fs_atomic::atomic_write_private(&self.path, body.as_bytes())
            .map_err(|e| format!("write {}: {e}", self.path.display()))
    }
}

fn store_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

impl SecretStore for FileSecretStore {
    fn store(&self, kind: &str, secret: &str) -> Result<String, String> {
        if let Some(v) = &self.vault {
            return v.store_new("dbsec", kind, secret);
        }
        let _g = store_lock().lock().unwrap_or_else(|p| p.into_inner());
        let sref = format!("dbsec_{kind}_{}", random_hex_12());
        let mut map = self.load()?;
        map.insert(sref.clone(), serde_json::Value::String(secret.to_string()));
        self.save(&map)?;
        Ok(sref)
    }

    fn resolve(&self, sref: &str) -> Result<Option<String>, String> {
        if let Some(v) = &self.vault {
            return v.resolve(sref);
        }
        let _g = store_lock().lock().unwrap_or_else(|p| p.into_inner());
        Ok(self
            .load()?
            .get(sref)
            .and_then(|v| v.as_str())
            .map(str::to_string))
    }

    fn delete(&self, sref: &str) -> Result<(), String> {
        if let Some(v) = &self.vault {
            return v.delete(sref);
        }
        let _g = store_lock().lock().unwrap_or_else(|p| p.into_inner());
        let mut map = self.load()?;
        if map.remove(sref).is_some() {
            self.save(&map)?;
        }
        Ok(())
    }
}

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

pub fn generate_secret() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).map_err(|e| format!("os rng unavailable: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
pub(crate) struct MemSecretStore {
    pub map: Mutex<std::collections::HashMap<String, String>>,
}

#[cfg(test)]
impl Default for MemSecretStore {
    fn default() -> Self {
        Self {
            map: Mutex::new(std::collections::HashMap::new()),
        }
    }
}

#[cfg(test)]
impl SecretStore for MemSecretStore {
    fn store(&self, kind: &str, secret: &str) -> Result<String, String> {
        let sref = format!("dbsec_{kind}_{}", random_hex_12());
        self.map
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(sref.clone(), secret.to_string());
        Ok(sref)
    }
    fn resolve(&self, sref: &str) -> Result<Option<String>, String> {
        Ok(self
            .map
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(sref)
            .cloned())
    }
    fn delete(&self, sref: &str) -> Result<(), String> {
        self.map
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(sref);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> (FileSecretStore, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "k2-db-secrets-test-{}-{}",
            std::process::id(),
            random_hex_12()
        ));
        (FileSecretStore::at(dir.join("db-secrets.json")), dir)
    }

    /// The store's directory and file are owner-only as soon as they
    /// exist, and a pre-existing loose file is replaced by a 0600 one.
    #[cfg(unix)]
    #[test]
    fn save_creates_owner_only_dir_and_file() {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &std::path::Path| {
            fs::metadata(p).expect("metadata").permissions().mode() & 0o777
        };
        let (store, dir) = temp_store();
        assert!(!dir.exists(), "temp store dir must start absent");
        let r = store.store("agent", "v1").expect("store");
        assert!(r.starts_with("dbsec_"), "{r}");
        assert_eq!(mode(&dir), 0o700, "store dir created by the write must be 0700");
        assert_eq!(mode(&dir.join("db-secrets.json")), 0o600, "secrets file must be 0600");

        fs::set_permissions(dir.join("db-secrets.json"), fs::Permissions::from_mode(0o644))
            .expect("loosen");
        store.store("agent", "v2").expect("store again");
        assert_eq!(mode(&dir.join("db-secrets.json")), 0o600, "rewrite must land 0600");
        let leftovers: Vec<_> = fs::read_dir(&dir)
            .expect("read_dir")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n != "db-secrets.json")
            .collect();
        assert!(leftovers.is_empty(), "no temp files left behind: {leftovers:?}");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn store_resolve_delete_roundtrip_and_0600() {
        let (store, dir) = temp_store();
        let r1 = store.store("agent", "s3cret-one").expect("store");
        assert!(r1.starts_with("dbsec_agent_"), "{r1}");
        assert_eq!(
            store.resolve(&r1).expect("resolve"),
            Some("s3cret-one".into())
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(dir.join("db-secrets.json"))
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "secrets file must be owner-only");
        }
        store.delete(&r1).expect("delete");
        assert_eq!(store.resolve(&r1).expect("resolve"), None);
        let _ = fs::remove_dir_all(dir);
    }
}
