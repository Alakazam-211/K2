//! On-box PEM store for custom-domain certs (`~/.k2/certs/<hostname>/`).
//! Private key is 0600; chain is 0644. Never silent-rcgen.

use std::fs;
use std::path::{Path, PathBuf};

pub fn certs_root() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".k2")
        .join("certs")
}

pub fn host_dir(hostname: &str) -> PathBuf {
    certs_root().join(hostname)
}

pub fn key_path(hostname: &str) -> PathBuf {
    host_dir(hostname).join("privkey.pem")
}

pub fn chain_path(hostname: &str) -> PathBuf {
    host_dir(hostname).join("fullchain.pem")
}

pub fn acme_account_path() -> PathBuf {
    certs_root().join("acme-account.json")
}

pub fn acme_config_path() -> PathBuf {
    certs_root().join("acme-config.json")
}

#[derive(Debug, Clone)]
pub struct InstalledPem {
    pub hostname: String,
    pub chain_pem: String,
    pub key_pem: String,
}

pub fn load(hostname: &str) -> Option<InstalledPem> {
    let chain = fs::read_to_string(chain_path(hostname)).ok()?;
    let key = fs::read_to_string(key_path(hostname)).ok()?;
    if chain.trim().is_empty() || key.trim().is_empty() {
        return None;
    }
    Some(InstalledPem {
        hostname: hostname.to_string(),
        chain_pem: chain,
        key_pem: key,
    })
}

pub fn install(hostname: &str, chain_pem: &str, key_pem: &str) -> Result<PathBuf, String> {
    if chain_pem.contains("BEGIN CERTIFICATE") && key_pem.contains("BEGIN") {
        // ok
    } else {
        return Err("upload requires PEM certificate and private key".into());
    }
    let dir = host_dir(hostname);
    fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    write_tmp_rename(&chain_path(hostname), chain_pem.as_bytes(), 0o644)?;
    write_tmp_rename(&key_path(hostname), key_pem.as_bytes(), 0o600)?;
    Ok(dir)
}

fn write_tmp_rename(path: &Path, bytes: &[u8], mode: u32) -> Result<(), String> {
    let tmp = path.with_extension(format!(
        "tmp.{}",
        std::process::id()
    ));
    fs::write(&tmp, bytes).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(mode))
            .map_err(|e| format!("chmod {:o} {}: {e}", mode, tmp.display()))?;
    }
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("rename {}: {e}", path.display())
    })?;
    Ok(())
}

pub fn key_mode_is_0600(hostname: &str) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(key_path(hostname))
            .map(|m| m.permissions().mode() & 0o777 == 0o600)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        let _ = hostname;
        true
    }
}

/// Copy PEMs onto Stalwart's certs dir when present (role=mail, 465).
pub fn plant_mail_pem(hostname: &str, chain_pem: &str, key_pem: &str) -> Result<(), String> {
    let mut last_err = None;
    let mut planted = false;
    for dir in ["/var/lib/stalwart/certs", "/etc/stalwart/certs"] {
        let p = Path::new(dir);
        if !p.is_dir() {
            continue;
        }
        let crt = p.join(format!("{hostname}.crt"));
        let key = p.join(format!("{hostname}.key"));
        match write_tmp_rename(&crt, chain_pem.as_bytes(), 0o644)
            .and_then(|_| write_tmp_rename(&key, key_pem.as_bytes(), 0o600))
        {
            Ok(()) => planted = true,
            Err(e) => last_err = Some(e),
        }
    }
    if planted {
        Ok(())
    } else if let Some(e) = last_err {
        Err(e)
    } else {
        Ok(()) // no Stalwart dir — inventory still holds the PEM
    }
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcmeConfig {
    pub email: Option<String>,
    pub directory: Option<String>,
}

pub fn load_acme_config() -> AcmeConfig {
    fs::read_to_string(acme_config_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_acme_config(cfg: &AcmeConfig) -> Result<(), String> {
    let dir = certs_root();
    fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    let body = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    write_tmp_rename(&acme_config_path(), body.as_bytes(), 0o600)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_key_is_0600() {
        let root = std::env::temp_dir().join(format!(
            "k2-certs-mode-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let prev = std::env::var_os("HOME");
        std::env::set_var("HOME", &root);
        let chain = "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n";
        let key = "-----BEGIN PRIVATE KEY-----\nMIIB\n-----END PRIVATE KEY-----\n";
        install("mail.example.com", chain, key).expect("install");
        assert!(key_mode_is_0600("mail.example.com"));
        assert!(load("mail.example.com").is_some());
        match prev {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
        let _ = fs::remove_dir_all(root);
    }
}
