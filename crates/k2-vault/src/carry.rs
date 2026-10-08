//! Clone / migrate carries and backups (§13, KV31, VS2, VS20).
//!
//! - Every vault has a carry identity (an age X25519 key kept as a
//!   `system` secret); its public half is the vault's **recipient**
//!   (`GET /cli/vault/recipient`).
//! - A carry is a set of chosen `user` secrets, signed by the source
//!   vault's Ed25519 key, bound to the target vault id, and age-encrypted
//!   to the target's recipient. Migrations never use the recovery key
//!   (VS2): values are re-encrypted box to box, never shown to anyone.
//! - [`LocalVault::snapshot_to`] copies vault.db (ciphertext) for a
//!   backup; restore on a new box uses the recovery key or the backup
//!   identity ([`LocalVault::adopt_with_identity`]).

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::agefmt::{self, Identity, Recipient};
use crate::engine::Gen;
use crate::error::{Result, VaultError};
use crate::secret::{Key32, SecretBytes};
use crate::vault::LocalVault;

pub const ENGINE: &str = "vault";
pub const IDENTITY: &str = "carry-identity";
pub const SIGNING: &str = "carry-signing";
pub const FORMAT: &str = "k2-vault-carry/1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CarryItem {
    /// `workspace` | `server`
    pub scope_kind: String,
    /// Workspace handle on the source box (the target maps it to its own).
    pub scope_label: String,
    pub name: String,
    pub kind: String,
    pub hosts: Vec<String>,
    pub inject: Option<crate::inject::Inject>,
    pub default_delivery: String,
    pub delivery_ceiling: String,
    pub description: String,
    pub human_given: bool,
    pub value_b64: String,
}

impl Drop for CarryItem {
    fn drop(&mut self) {
        self.value_b64.zeroize();
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CarryPayload {
    pub format: String,
    pub source_vault_id: String,
    pub target_vault_id: String,
    pub created_at: String,
    /// Hex Ed25519 public key of the source vault (compare out of band for
    /// a clone to another owner).
    pub source_public_key: String,
    /// Same box owner moving their own workspace (keeps `human_given`).
    pub own_box_move: bool,
    pub items: Vec<CarryItem>,
}

#[derive(Serialize, Deserialize)]
struct Envelope {
    payload: String,
    sig: String,
}

impl LocalVault {
    fn carry_identity(&self) -> Result<Identity> {
        self.engine_ensure(ENGINE, IDENTITY, Gen::Ed25519)?;
        let v = self.engine_value(ENGINE, IDENTITY)?;
        let mut arr = [0u8; 32];
        if v.len() != 32 {
            return Err(VaultError::tamper("carry identity has the wrong length"));
        }
        arr.copy_from_slice(v.expose());
        Ok(Identity::from_scalar(Key32::from_array(arr)))
    }

    /// This vault's public carry recipient (`age1…`).
    pub fn carry_recipient(&self) -> Result<Recipient> {
        Ok(self.carry_identity()?.recipient())
    }

    /// Sign and seal a carry to a target vault.
    pub fn carry_seal(&self, target: &Recipient, target_vault_id: &str, own_box_move: bool, items: Vec<CarryItem>) -> Result<Vec<u8>> {
        let payload = CarryPayload {
            format: FORMAT.into(),
            source_vault_id: self.vault_id().to_string(),
            target_vault_id: target_vault_id.to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            source_public_key: crate::crypto::hex(&self.engine_public_key(ENGINE, SIGNING)?),
            own_box_move,
            items,
        };
        let mut p = serde_json::to_string(&payload).map_err(|_| VaultError::internal("carry json"))?;
        let sig = self.engine_sign(ENGINE, SIGNING, p.as_bytes())?;
        let env = Envelope { payload: p.clone(), sig: crate::crypto::hex(&sig) };
        let mut body = serde_json::to_vec(&env).map_err(|_| VaultError::internal("carry json"))?;
        p.zeroize();
        let out = agefmt::encrypt(&[*target], &body);
        body.zeroize();
        Ok(out)
    }

    /// Open a carry made for this vault: decrypt, check the signature
    /// against the key inside it, and check it was made for this vault.
    pub fn carry_open(&self, file: &[u8]) -> Result<CarryPayload> {
        let id = self.carry_identity()?;
        let raw = agefmt::decrypt(&id, file)?;
        let env: Envelope = serde_json::from_slice(raw.expose()).map_err(|_| VaultError::bad_request("bad_carry", "not a vault carry"))?;
        let payload: CarryPayload =
            serde_json::from_str(&env.payload).map_err(|_| VaultError::bad_request("bad_carry", "not a vault carry"))?;
        if payload.format != FORMAT {
            return Err(VaultError::bad_request("bad_carry", "unknown carry format"));
        }
        let pk: [u8; 32] = crate::crypto::unhex(&payload.source_public_key)
            .and_then(|v| v.try_into().ok())
            .ok_or_else(|| VaultError::bad_request("bad_carry", "bad source key"))?;
        let sig: [u8; 64] = crate::crypto::unhex(&env.sig)
            .and_then(|v| v.try_into().ok())
            .ok_or_else(|| VaultError::bad_request("bad_carry", "bad signature"))?;
        if !crate::engine::verify(&pk, env.payload.as_bytes(), &sig) {
            return Err(VaultError::tamper("the carry's signature doesn't match"));
        }
        if payload.target_vault_id != self.vault_id() {
            return Err(VaultError::forbidden("carry_not_for_this_vault", "this carry was made for another vault"));
        }
        Ok(payload)
    }

    /// Copy vault.db (ciphertext) to `dest` for a backup (online, `VACUUM
    /// INTO`). The box key is never part of it.
    pub fn snapshot_to(&self, dest: &std::path::Path) -> Result<()> {
        self.flush_rollups();
        if dest.exists() {
            return Err(VaultError::conflict("exists", "the snapshot target already exists"));
        }
        self.vacuum_into(dest)
    }
}

impl CarryItem {
    pub fn value(&self) -> Result<SecretBytes> {
        STANDARD
            .decode(&self.value_b64)
            .map(SecretBytes::new)
            .map_err(|_| VaultError::bad_request("bad_carry", "bad value encoding"))
    }

    pub fn encode_value(v: &[u8]) -> String {
        STANDARD.encode(v)
    }
}
