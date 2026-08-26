//! Thread secret vault. Bytes never enter redb / thread JSON / CLI stdout.
//!
//! Prod: `~/.k2/thread-secrets/<workspace-id>/<name>` (0600).
//! Tests: tempfile via `K2_THREAD_SECRETS_DIR` or `test-util`.

use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;

use crate::fs_atomic;

pub fn vault_root() -> PathBuf {
    if let Ok(p) = std::env::var("K2_THREAD_SECRETS_DIR") {
        let trimmed = p.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    #[cfg(any(test, feature = "test-util"))]
    {
        return test_vault_root();
    }
    #[cfg(not(any(test, feature = "test-util")))]
    {
        crate::paths::k2_home().join("thread-secrets")
    }
}

#[cfg(any(test, feature = "test-util"))]
fn test_vault_root() -> PathBuf {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!(
            "k2-thread-secrets-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let _ = fs::create_dir_all(&dir);
        dir
    })
    .clone()
}

pub fn validate_name(name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("secret --name is required".to_string());
    }
    if name.contains('/')
        || name.contains('\\')
        || name.contains('\0')
        || name.contains("..")
    {
        return Err("invalid secret name".to_string());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(
            "secret name must be letters, digits, underscore, or hyphen".to_string(),
        );
    }
    Ok(())
}

fn secret_path(workspace_id: &str, name: &str) -> Result<PathBuf, String> {
    validate_name(name)?;
    let ws = workspace_id.trim();
    if ws.is_empty() || ws.contains('/') || ws.contains("..") {
        return Err("invalid workspace id for vault".to_string());
    }
    Ok(vault_root().join(ws).join(name.trim()))
}

pub fn put(workspace_id: &str, name: &str, bytes: &[u8]) -> Result<(), String> {
    if bytes.is_empty() {
        return Err("secret value must not be empty".to_string());
    }
    let path = secret_path(workspace_id, name)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("vault mkdir: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
        }
    }
    fs_atomic::atomic_write(&path, bytes).map_err(|e| format!("vault write: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("vault chmod 0600: {e}"))?;
    }
    Ok(())
}

pub fn exists(workspace_id: &str, name: &str) -> bool {
    secret_path(workspace_id, name)
        .ok()
        .is_some_and(|p| p.is_file())
}

pub fn delete(workspace_id: &str, name: &str) -> Result<(), String> {
    let path = match secret_path(workspace_id, name) {
        Ok(p) => p,
        Err(_) => return Ok(()),
    };
    if path.exists() {
        fs::remove_file(&path).map_err(|e| format!("vault delete: {e}"))?;
    }
    Ok(())
}

/// Test helper: read vault bytes. Production overlay never returns this.
#[cfg(any(test, feature = "test-util"))]
pub fn debug_read(workspace_id: &str, name: &str) -> Result<Vec<u8>, String> {
    let path = secret_path(workspace_id, name)?;
    fs::read(&path).map_err(|e| format!("vault read: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_then_delete_roundtrip() {
        let ws = uuid::Uuid::new_v4().to_string();
        put(&ws, "API_TOKEN", b"s3cret-bytes").expect("put");
        assert!(exists(&ws, "API_TOKEN"));
        let got = debug_read(&ws, "API_TOKEN").expect("read");
        assert_eq!(got, b"s3cret-bytes");
        delete(&ws, "API_TOKEN").expect("delete");
        assert!(!exists(&ws, "API_TOKEN"));
    }

    #[test]
    fn rejects_path_name() {
        validate_name("../etc/passwd").expect_err("dotdot");
        validate_name("a/b").expect_err("slash");
    }
}
