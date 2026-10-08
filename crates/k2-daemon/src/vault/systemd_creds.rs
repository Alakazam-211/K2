// Linux-only at runtime; compiled everywhere so the code stays checked.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

//! Optional Linux KeyStore (Q24 / VS22): the box key as a systemd
//! encrypted credential. When the daemon's system unit carries
//! `LoadCredentialEncrypted=k2-vault-kek:/etc/k2/k2-vault-kek.cred`, PID 1
//! decrypts it (root-only host key, plus TPM2 when present) into
//! `$CREDENTIALS_DIRECTORY/k2-vault-kek` (ramfs, non-swappable) for the
//! daemon only. The key is then no longer a file in the home directory, so
//! home backups and provider snapshots of `/home` don't carry it. It does
//! not stop same-user agents (they can read the credentials dir), and the
//! copy says so.
//!
//! Set up with `scripts/k2-vault-systemd-creds.sh` (root, once), which
//! seals the existing file key so nothing re-wraps. Read-only here: the
//! daemon never writes the credential; rotation is the script's job.

use std::path::PathBuf;

use k2_vault::keystore::{Anchor, KeyHome, KeyStore};
use k2_vault::{Key32, VaultError};
use zeroize::Zeroize;

pub const CREDENTIAL_NAME: &str = "k2-vault-kek";

pub struct SystemdCredsKeyStore {
    path: PathBuf,
}

impl SystemdCredsKeyStore {
    /// Present only when systemd handed the daemon the credential.
    pub fn detect() -> Option<SystemdCredsKeyStore> {
        if !cfg!(target_os = "linux") {
            return None;
        }
        let dir = std::env::var_os("CREDENTIALS_DIRECTORY")?;
        let path = PathBuf::from(dir).join(CREDENTIAL_NAME);
        path.is_file().then_some(SystemdCredsKeyStore { path })
    }

    #[cfg(test)]
    pub fn at(path: PathBuf) -> SystemdCredsKeyStore {
        SystemdCredsKeyStore { path }
    }
}

fn read_only() -> VaultError {
    VaultError::new(
        "keystore_read_only",
        409,
        "the box key is a systemd credential; change it with scripts/k2-vault-systemd-creds.sh as root",
    )
}

impl KeyStore for SystemdCredsKeyStore {
    fn kind(&self) -> KeyHome {
        KeyHome::SystemdCreds
    }

    fn load(&self) -> Result<Option<Key32>, VaultError> {
        let mut b = std::fs::read(&self.path).map_err(|e| VaultError::internal(format!("credential: {}", e.kind())))?;
        if b.len() != 32 {
            b.zeroize();
            return Err(VaultError::tamper("the systemd credential has the wrong length"));
        }
        let mut a = [0u8; 32];
        a.copy_from_slice(&b);
        b.zeroize();
        Ok(Some(Key32::from_array(a)))
    }

    fn init(&self) -> Result<Key32, VaultError> {
        Err(read_only())
    }

    fn supports_anchor(&self) -> bool {
        false
    }

    fn anchor(&self) -> Result<Option<Anchor>, VaultError> {
        Ok(None)
    }

    fn set_anchor(&self, _a: &Anchor) -> Result<(), VaultError> {
        Ok(())
    }

    fn stage_next(&self) -> Result<Key32, VaultError> {
        Err(read_only())
    }

    fn commit_next(&self) -> Result<(), VaultError> {
        Err(read_only())
    }

    fn abort_next(&self) -> Result<(), VaultError> {
        Ok(())
    }

    fn remove(&self) -> Result<(), VaultError> {
        Err(read_only())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_32_byte_credential_and_refuses_writes() {
        let dir = std::env::temp_dir().join(format!("k2-creds-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("dir");
        let p = dir.join(CREDENTIAL_NAME);
        std::fs::write(&p, [7u8; 32]).expect("write");
        let ks = SystemdCredsKeyStore::at(p.clone());
        assert_eq!(ks.load().expect("load").expect("some").as_bytes(), &[7u8; 32]);
        assert_eq!(ks.init().err().expect("ro").code, "keystore_read_only");
        assert_eq!(ks.kind(), KeyHome::SystemdCreds);
        std::fs::write(&p, [1u8; 5]).expect("write");
        assert_eq!(ks.load().err().expect("bad").code, "vault_tamper");
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
}
