//! SQL sidecar secret storage — `~/.k2/db-secrets.json`, refs `dbsec_*`.
//!
//! Copied from `mail/secrets.rs` (0600 tmp+rename). Not mail-secrets.json.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

pub trait SecretStore: Send + Sync {
    fn store(&self, kind: &str, secret: &str) -> Result<String, String>;
    fn resolve(&self, sref: &str) -> Result<Option<String>, String>;
    #[allow(dead_code)]
    fn delete(&self, sref: &str) -> Result<(), String>;
}

pub struct FileSecretStore {
    path: PathBuf,
}

impl Default for FileSecretStore {
    fn default() -> Self {
        let dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".k2");
        Self {
            path: dir.join("db-secrets.json"),
        }
    }
}

impl FileSecretStore {
    #[cfg(test)]
    pub fn at(path: PathBuf) -> Self {
        Self { path }
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

    fn save(&self, map: &serde_json::Map<String, serde_json::Value>) -> Result<(), String> {
        let dir = self
            .path
            .parent()
            .ok_or_else(|| format!("{}: no parent dir", self.path.display()))?;
        fs::create_dir_all(dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
        let tmp = dir.join(format!("db-secrets.json.tmp.{}", std::process::id()));
        let body = serde_json::to_string_pretty(&serde_json::Value::Object(map.clone()))
            .map_err(|e| format!("serialize db secrets: {e}"))?;
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

fn store_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

impl SecretStore for FileSecretStore {
    fn store(&self, kind: &str, secret: &str) -> Result<String, String> {
        let _g = store_lock().lock().unwrap_or_else(|p| p.into_inner());
        let sref = format!("dbsec_{kind}_{}", random_hex_12());
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
