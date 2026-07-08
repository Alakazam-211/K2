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

/// Resolve a SCHEME-form secret ref: `env:<VAR>` or an absolute file
/// path (optionally `file:`-prefixed, 0600 — the root of trust is the
/// box itself). These are the operator-supplied spellings (relay
/// credentials via `k2 mail config --relay-secret-ref`, the
/// `mail_server.*_secret_ref` handshake seam); the [`FileSecretStore`]
/// `mailsec_*` refs are the daemon-minted third form. Anything else
/// fails LOUDLY rather than guessing — and the error carries the ref,
/// never a secret value.
pub fn resolve_secret_ref(secret_ref: &str) -> Result<String, String> {
    if let Some(var) = secret_ref.strip_prefix("env:") {
        return std::env::var(var).map_err(|_| format!("secret env var '{var}' is not set"));
    }
    let path = secret_ref.strip_prefix("file:").unwrap_or(secret_ref);
    if path.starts_with('/') {
        return fs::read_to_string(path)
            .map(|s| s.trim().to_string())
            .map_err(|e| format!("secret file '{path}': {e}"))
            .and_then(|s| {
                if s.is_empty() {
                    Err(format!("secret file '{path}' is empty"))
                } else {
                    Ok(s)
                }
            });
    }
    Err(format!(
        "unrecognized secret ref '{secret_ref}' (expected env:<VAR> or an absolute file path)"
    ))
}

/// Is `sref` one of the scheme forms [`resolve_secret_ref`] handles
/// (vs a store-minted `mailsec_*` ref)?
pub fn is_scheme_ref(sref: &str) -> bool {
    sref.starts_with("env:") || sref.starts_with("file:") || sref.starts_with('/')
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
        // Scheme refs (env:<VAR> / absolute path) resolve WITHOUT the
        // file map — one resolution point for all three ref forms, so
        // relay configs whose credentials the owner keeps in an env
        // var or a root-owned file work through the same SecretStore
        // the send path already consults. A set-but-unresolvable
        // scheme ref is a loud Err (the operator pointed somewhere
        // broken), not a silent None.
        if is_scheme_ref(sref) {
            return resolve_secret_ref(sref).map(Some);
        }
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
    fn scheme_refs_resolve_through_the_store_without_the_file_map() {
        let (store, dir) = temp_store();
        // env: form — resolves, and a MISSING var is a loud Err (never
        // a silent None: the operator pointed at something broken).
        std::env::set_var("K2_TEST_MAIL_RELAY_PW", "relay-pw");
        assert_eq!(
            store.resolve("env:K2_TEST_MAIL_RELAY_PW").expect("resolve"),
            Some("relay-pw".into())
        );
        assert!(store.resolve("env:K2_TEST_MAIL_RELAY_PW_MISSING").is_err());
        // Absolute-path form — trims, empty file fails loudly.
        std::fs::create_dir_all(&dir).expect("mkdir");
        let f = dir.join("relay-secret");
        fs::write(&f, "  file-pw\n").expect("write");
        assert_eq!(
            store.resolve(f.to_str().unwrap()).expect("resolve"),
            Some("file-pw".into())
        );
        fs::write(&f, "").expect("write");
        assert!(store.resolve(f.to_str().unwrap()).is_err(), "empty file fails loudly");
        // Unknown scheme fails loudly through the bare helper.
        assert!(resolve_secret_ref("keychain:whatever").is_err());
        assert!(is_scheme_ref("env:X") && is_scheme_ref("/abs/path") && is_scheme_ref("file:/x"));
        assert!(!is_scheme_ref("mailsec_relay_abc123"));
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
