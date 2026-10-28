//! macOS KeyStore: the vault's box key in the login Keychain, created and
//! read by the daemon itself through Security.framework (KV5), never
//! through `/usr/bin/security`. An item created by a process trusts that
//! process's code identity (`dev.k2.app.daemon`, team `36B8R93HXV` on the
//! signed build), so another process asking for it makes macOS show a
//! prompt. The anchor (`{vault_id, gen, audit_head}`) is a second item.
//!
//! Items (service `dev.k2.vault`): account `kek`, `kek.next` (rotation),
//! `anchor` (JSON). Verified on the signed build per the smoke list
//! (§22.6 #1); tests never touch a real keychain.

use k2_vault::keystore::{Anchor, KeyHome, KeyStore};
use k2_vault::{Key32, VaultError};
use security_framework::passwords::{delete_generic_password, get_generic_password, set_generic_password};
use zeroize::Zeroize;

pub const SERVICE: &str = "dev.k2.vault";
const KEK: &str = "kek";
const NEXT: &str = "kek.next";
const ANCHOR: &str = "anchor";

/// errSecItemNotFound
const NOT_FOUND: i32 = -25300;
/// errSecUserCanceled (Cancel on the unlock or access dialog) and
/// errSecAuthFailed (Deny, or a wrong password at the prompt).
const USER_SAID_NO: [i32; 2] = [-128, -25293];

pub struct KeychainKeyStore;

impl KeychainKeyStore {
    pub fn new() -> KeychainKeyStore {
        KeychainKeyStore
    }

    fn read(account: &str) -> Result<Option<Vec<u8>>, VaultError> {
        match get_generic_password(SERVICE, account) {
            Ok(v) => Ok(Some(v)),
            Err(e) if e.code() == NOT_FOUND => Ok(None),
            // The person cancelled or denied the prompt: a code of its own,
            // so the boot retry stops asking (it would prompt again).
            Err(e) if USER_SAID_NO.contains(&e.code()) => Err(VaultError::new(
                "user_cancelled",
                500,
                format!("the Keychain prompt for the vault key was cancelled ({}); the vault stays closed until `k2 vault open`", e.code()),
            )),
            Err(e) => Err(VaultError::new(
                "keychain_error",
                500,
                format!("the Keychain refused the vault key ({}); the vault stays closed", e.code()),
            )),
        }
    }

    fn write(account: &str, bytes: &[u8]) -> Result<(), VaultError> {
        set_generic_password(SERVICE, account, bytes)
            .map_err(|e| VaultError::new("keychain_error", 500, format!("couldn't save to the Keychain ({})", e.code())))
    }

    fn delete(account: &str) -> Result<(), VaultError> {
        match delete_generic_password(SERVICE, account) {
            Ok(()) => Ok(()),
            Err(e) if e.code() == NOT_FOUND => Ok(()),
            Err(e) => Err(VaultError::new("keychain_error", 500, format!("couldn't delete from the Keychain ({})", e.code()))),
        }
    }

    fn key_from(mut v: Vec<u8>) -> Result<Key32, VaultError> {
        if v.len() != 32 {
            v.zeroize();
            return Err(VaultError::tamper("the Keychain's vault key has the wrong length"));
        }
        let mut a = [0u8; 32];
        a.copy_from_slice(&v);
        v.zeroize();
        Ok(Key32::from_array(a))
    }
}

impl Default for KeychainKeyStore {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyStore for KeychainKeyStore {
    fn kind(&self) -> KeyHome {
        KeyHome::Keychain
    }

    fn load(&self) -> Result<Option<Key32>, VaultError> {
        match Self::read(KEK)? {
            Some(v) => Ok(Some(Self::key_from(v)?)),
            None => Ok(None),
        }
    }

    fn init(&self) -> Result<Key32, VaultError> {
        if Self::read(KEK)?.is_some() {
            return Err(VaultError::conflict("kek_exists", "a box key already exists"));
        }
        let k = Key32::random();
        Self::write(KEK, k.as_bytes())?;
        Ok(k)
    }

    fn supports_anchor(&self) -> bool {
        true
    }

    fn anchor(&self) -> Result<Option<Anchor>, VaultError> {
        match Self::read(ANCHOR)? {
            Some(v) => serde_json::from_slice(&v)
                .map(Some)
                .map_err(|_| VaultError::tamper("the Keychain's vault anchor is unreadable")),
            None => Ok(None),
        }
    }

    fn set_anchor(&self, a: &Anchor) -> Result<(), VaultError> {
        let j = serde_json::to_vec(a).map_err(|_| VaultError::internal("anchor json"))?;
        Self::write(ANCHOR, &j)
    }

    fn stage_next(&self) -> Result<Key32, VaultError> {
        let k = Key32::random();
        Self::write(NEXT, k.as_bytes())?;
        Ok(k)
    }

    fn commit_next(&self) -> Result<(), VaultError> {
        let n = Self::read(NEXT)?.ok_or_else(|| VaultError::conflict("no_staged_kek", "nothing staged"))?;
        let k = Self::key_from(n)?;
        Self::write(KEK, k.as_bytes())?;
        Self::delete(NEXT)
    }

    fn abort_next(&self) -> Result<(), VaultError> {
        Self::delete(NEXT)
    }

    fn remove(&self) -> Result<(), VaultError> {
        Self::delete(KEK)?;
        Self::delete(NEXT)
    }
}
