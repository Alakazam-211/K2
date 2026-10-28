//! Where the box key (KEK) lives (§7.2, KV23, KV26, VS7, VS8).
//!
//! | Mode | KeyStore | Anchor (rollback guard) |
//! |---|---|---|
//! | macOS desktop | Keychain via Security.framework (daemon side) | yes |
//! | Windows desktop | DPAPI-sealed file (`dpapi`) | no |
//! | Linux box with the root helper (protocol ≥ 4) | keeper | gen only |
//! | Linux, no helper (0.45.2 default) | file `~/.k2/vault/key/kek` | no |
//! | tests | fake | yes |
//!
//! The anchor is `{vault_id, gen, audit_head}` (VS7/VS8): a vault.db older
//! than the anchor, or with another vault id, fails closed.

#[cfg(windows)]
pub mod dpapi;
#[cfg(any(test, feature = "test-util"))]
pub mod fake;
pub mod file;
pub mod keeper;

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::secret::Key32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum KeyHome {
    Keychain,
    SecretService,
    Keeper,
    KeeperTpm,
    SystemdCreds,
    File,
    Fake,
    /// Windows: the key sealed by the user's DPAPI key.
    Dpapi,
}

impl KeyHome {
    pub fn as_str(&self) -> &'static str {
        match self {
            KeyHome::Keychain => "keychain",
            KeyHome::SecretService => "secret-service",
            KeyHome::Keeper => "keeper",
            KeyHome::KeeperTpm => "keeper+tpm",
            KeyHome::SystemdCreds => "systemd-creds",
            KeyHome::File => "file",
            KeyHome::Fake => "fake",
            KeyHome::Dpapi => "dpapi",
        }
    }

    /// Plain words for Settings / status.
    pub fn describe(&self) -> &'static str {
        match self {
            KeyHome::Keychain => "Your Mac's Keychain",
            KeyHome::SecretService => "Your desktop's keyring",
            KeyHome::Keeper | KeyHome::KeeperTpm => "This server's root helper",
            KeyHome::SystemdCreds => "This server's system credential store",
            KeyHome::File => "A file on this server",
            KeyHome::Fake => "Test key store",
            KeyHome::Dpapi => "Your Windows account's protected storage",
        }
    }
}

/// Rollback/substitution anchor kept outside vault.db.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Anchor {
    #[serde(rename = "vaultId", default, skip_serializing_if = "Option::is_none")]
    pub vault_id: Option<String>,
    pub gen: u64,
    #[serde(rename = "auditSeq", default, skip_serializing_if = "Option::is_none")]
    pub audit_seq: Option<u64>,
    #[serde(rename = "auditMac", default, skip_serializing_if = "Option::is_none")]
    pub audit_mac: Option<String>,
}

pub trait KeyStore: Send + Sync {
    fn kind(&self) -> KeyHome;

    /// The current KEK, or `None` when none was created yet.
    fn load(&self) -> Result<Option<Key32>>;

    /// Create a new KEK. Refuses when one exists.
    fn init(&self) -> Result<Key32>;

    fn load_or_init(&self) -> Result<Key32> {
        match self.load()? {
            Some(k) => Ok(k),
            None => self.init(),
        }
    }

    /// Does this store keep an anchor (rollback guard)?
    fn supports_anchor(&self) -> bool;

    /// The stored anchor (`None` = unsupported or never written).
    fn anchor(&self) -> Result<Option<Anchor>>;

    /// Write the anchor. Stores that keep only a counter keep `gen`.
    fn set_anchor(&self, a: &Anchor) -> Result<()>;

    /// KEK rotation, phase 1: make the next KEK (not yet current).
    fn stage_next(&self) -> Result<Key32>;

    /// Phase 2: the staged KEK becomes current (after the caller re-wrapped
    /// and verified the vault).
    fn commit_next(&self) -> Result<()>;

    /// Drop a staged KEK.
    fn abort_next(&self) -> Result<()>;

    /// Remove the KEK entirely (after an in-place move to a better store).
    fn remove(&self) -> Result<()>;

    /// A KEK rotation is half done (a staged next key is present). The box
    /// move export refuses then (BM-A8): the vault may already be wrapped
    /// to the staged key.
    fn rotation_pending(&self) -> Result<bool> {
        Ok(false)
    }
}
