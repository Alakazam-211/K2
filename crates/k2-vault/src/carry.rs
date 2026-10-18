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
/// Clone carries (0.46.0, prd-clone-carries-threads-and-vault-v1 §6.3):
/// bound to one clone id and expiring; grants and blank rows ride along.
pub const FORMAT_V2: &str = "k2-vault-carry/2";
/// A clone carry opens for one hour after it was sealed (CL26).
pub const CLONE_CARRY_TTL_SECS: i64 = 3600;

/// Kinds that never travel in any carry (CL13): K2 login slots, App
/// passes and engine credentials are made again on the target.
pub fn never_carried_kind(kind: &str) -> bool {
    matches!(kind, "login_slot" | "app_pass" | "engine_cred")
}

/// A grant the source workspace had on a carried secret, re-created for
/// the clone's workspace (env / file names kept).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CarryGrant {
    pub delivery: String,
    #[serde(default)]
    pub env_name: Option<String>,
    #[serde(default)]
    pub file_name: Option<String>,
    #[serde(default)]
    pub hosts_narrowed: Option<Vec<String>>,
}

/// A secret the owner chose to leave blank: names only, never a value.
/// The target posts a secret card for it on the clone's Thread.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CarryBlank {
    pub name: String,
    pub kind: String,
    #[serde(default)]
    pub hosts: Vec<String>,
    pub delivery: String,
    #[serde(default)]
    pub env_names: Vec<String>,
    /// `workspace` | `server` | … on the source.
    pub source_scope: String,
}

/// What binds a clone carry to one clone (CL26).
#[derive(Debug, Clone)]
pub struct CloneBinding {
    /// The clone bundle's manifest `clone_id`.
    pub clone_id: String,
    pub source_workspace_id: String,
    /// The source workspace's handle (for `.env.k2` rewrites and cards).
    pub source_label: String,
    /// Seconds until the carry stops opening ([`CLONE_CARRY_TTL_SECS`]).
    pub ttl_secs: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CarryItem {
    /// `workspace` | `server`
    pub scope_kind: String,
    /// Workspace handle on the source box (shown to people only; never
    /// used to pick the target workspace, CL12).
    pub scope_label: String,
    /// The source workspace's id (`projects.id`). Carries made before
    /// 0.46.0 have none; their workspace secrets need an explicit target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope_id: Option<String>,
    pub name: String,
    pub kind: String,
    pub hosts: Vec<String>,
    pub inject: Option<crate::inject::Inject>,
    pub default_delivery: String,
    pub delivery_ceiling: String,
    pub description: String,
    pub human_given: bool,
    pub value_b64: String,
    /// Where the secret lived on the source (`workspace` | `server`). A
    /// clone lands every item in the clone's workspace scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_scope: Option<String>,
    /// The source workspace's grants on it (env/file names), for a clone.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub grants: Vec<CarryGrant>,
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
    /// The workspace id on the TARGET box this carry was made for, when the
    /// sender named one (`--target-workspace`). Import matches workspaces
    /// by id only (CL12).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_workspace_id: Option<String>,
    pub items: Vec<CarryItem>,
    /// `/2`: the clone this carry belongs to (the manifest's `clone_id`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clone_id: Option<String>,
    /// `/2`: RFC 3339; the carry doesn't open after this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    /// `/2`: the source workspace id and handle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_label: Option<String>,
    /// `/2`: rows left blank (names only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blank: Vec<CarryBlank>,
}

impl CarryPayload {
    /// A clone payload that never leaves this process (same-box clone,
    /// §6.5): the same shape the target applies, without a seal.
    pub fn in_process(
        vault_id: &str,
        own_box_move: bool,
        clone_id: &str,
        source_workspace_id: &str,
        source_label: &str,
        items: Vec<CarryItem>,
        blank: Vec<CarryBlank>,
    ) -> CarryPayload {
        CarryPayload {
            format: FORMAT_V2.into(),
            source_vault_id: vault_id.to_string(),
            target_vault_id: vault_id.to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            source_public_key: String::new(),
            own_box_move,
            target_workspace_id: None,
            items: items.into_iter().filter(|i| !never_carried_kind(&i.kind) && i.scope_kind != "session").collect(),
            clone_id: (!clone_id.is_empty()).then(|| clone_id.to_string()),
            expires_at: None,
            source_workspace_id: Some(source_workspace_id.to_string()),
            source_label: Some(source_label.to_string()),
            blank,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Envelope {
    payload: String,
    sig: String,
}

impl LocalVault {
    pub(crate) fn carry_identity(&self) -> Result<Identity> {
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
        self.carry_seal_for(target, target_vault_id, own_box_move, None, items)
    }

    /// [`Self::carry_seal`], naming the workspace id on the target box the
    /// carry is for.
    pub fn carry_seal_for(
        &self,
        target: &Recipient,
        target_vault_id: &str,
        own_box_move: bool,
        target_workspace_id: Option<String>,
        items: Vec<CarryItem>,
    ) -> Result<Vec<u8>> {
        refuse_never_carried(&items)?;
        let payload = CarryPayload {
            format: FORMAT.into(),
            source_vault_id: self.vault_id().to_string(),
            target_vault_id: target_vault_id.to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            source_public_key: crate::crypto::hex(&self.engine_public_key(ENGINE, SIGNING)?),
            own_box_move,
            target_workspace_id,
            items,
            clone_id: None,
            expires_at: None,
            source_workspace_id: None,
            source_label: None,
            blank: vec![],
        };
        self.seal_payload(target, &payload)
    }

    /// Seal a clone carry (`k2-vault-carry/2`): bound to the target vault,
    /// the clone id, and an expiry; never-carried kinds refused (CL13).
    pub fn carry_seal_clone(
        &self,
        target: &Recipient,
        target_vault_id: &str,
        own_box_move: bool,
        binding: CloneBinding,
        items: Vec<CarryItem>,
        blank: Vec<CarryBlank>,
    ) -> Result<Vec<u8>> {
        refuse_never_carried(&items)?;
        if binding.clone_id.trim().is_empty() {
            return Err(VaultError::bad_request("clone_id_required", "a clone carry needs the clone id"));
        }
        let now = chrono::Utc::now();
        let payload = CarryPayload {
            format: FORMAT_V2.into(),
            source_vault_id: self.vault_id().to_string(),
            target_vault_id: target_vault_id.to_string(),
            created_at: now.to_rfc3339(),
            source_public_key: crate::crypto::hex(&self.engine_public_key(ENGINE, SIGNING)?),
            own_box_move,
            target_workspace_id: None,
            items,
            clone_id: Some(binding.clone_id),
            expires_at: Some((now + chrono::Duration::seconds(binding.ttl_secs)).to_rfc3339()),
            source_workspace_id: Some(binding.source_workspace_id),
            source_label: Some(binding.source_label),
            blank,
        };
        self.seal_payload(target, &payload)
    }

    /// Test seam: seal any payload as given (no kind check), to prove the
    /// import side refuses what the seal side never makes.
    #[cfg(any(test, feature = "test-util"))]
    pub fn carry_seal_unchecked(&self, target: &Recipient, payload: &CarryPayload) -> Result<Vec<u8>> {
        self.seal_payload(target, payload)
    }

    fn seal_payload(&self, target: &Recipient, payload: &CarryPayload) -> Result<Vec<u8>> {
        let mut p = serde_json::to_string(payload).map_err(|_| VaultError::internal("carry json"))?;
        let sig = self.engine_sign(ENGINE, SIGNING, p.as_bytes())?;
        let env = Envelope { payload: p.clone(), sig: crate::crypto::hex(&sig) };
        let mut body = serde_json::to_vec(&env).map_err(|_| VaultError::internal("carry json"))?;
        p.zeroize();
        let out = agefmt::encrypt(&[*target], &body);
        body.zeroize();
        Ok(out)
    }

    /// The public key carries from this vault are signed with (hex).
    pub fn carry_public_key(&self) -> Result<String> {
        Ok(crate::crypto::hex(&self.engine_public_key(ENGINE, SIGNING)?))
    }

    /// Open a carry made for this vault: decrypt, check the signature
    /// against the key inside it, and check it was made for this vault.
    /// `/2` carries also must not have expired.
    pub fn carry_open(&self, file: &[u8]) -> Result<CarryPayload> {
        let id = self.carry_identity()?;
        let raw = agefmt::decrypt(&id, file)?;
        let env: Envelope = serde_json::from_slice(raw.expose()).map_err(|_| VaultError::bad_request("bad_carry", "not a vault carry"))?;
        let payload: CarryPayload =
            serde_json::from_str(&env.payload).map_err(|_| VaultError::bad_request("bad_carry", "not a vault carry"))?;
        if payload.format != FORMAT && payload.format != FORMAT_V2 {
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
        if payload.format == FORMAT_V2 {
            let exp = payload
                .expires_at
                .as_deref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .ok_or_else(|| VaultError::bad_request("bad_carry", "a clone carry without an expiry"))?;
            if chrono::Utc::now() > exp {
                return Err(VaultError::forbidden("carry_expired", "this clone carry expired (they open for one hour); clone again"));
            }
        }
        Ok(payload)
    }

    /// Open a clone carry for the clone `clone_id` only (CL26): `/2`, not
    /// expired, made for this vault and this clone.
    pub fn carry_open_clone(&self, file: &[u8], clone_id: &str) -> Result<CarryPayload> {
        let p = self.carry_open(file)?;
        if p.format != FORMAT_V2 {
            return Err(VaultError::forbidden("carry_not_for_clone", "this carry wasn't made for a clone"));
        }
        if clone_id.trim().is_empty() || p.clone_id.as_deref() != Some(clone_id) {
            return Err(VaultError::forbidden("carry_clone_mismatch", "this carry was made for another clone"));
        }
        Ok(p)
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

/// CL13: a seal never holds a never-carried kind or a chat's own secret.
fn refuse_never_carried(items: &[CarryItem]) -> Result<()> {
    if let Some(i) = items.iter().find(|i| never_carried_kind(&i.kind) || i.scope_kind == "session") {
        return Err(VaultError::forbidden(
            "never_carried",
            format!("{} ({}) never travels in a carry; it is made again on the target", i.name, i.kind),
        ));
    }
    Ok(())
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
