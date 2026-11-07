//! What the person at the signer trusts, and what the signer has seen.
//!
//! - **`trust.json`** (`/usr/local/etc/k2-signer`, root-owned, pinned):
//!   written only by admin verbs (a person with the admin password):
//!   - `controllers`: client servers paired with a 6-digit SAS
//!     ([`crate::pairing`]) or adopted from k2sign's S3b pin (V28). A
//!     receipt's `controller_fp` must be one of these **and** in the
//!     project policy's `controllers`;
//!   - `owners`: node-roster keys pinned by `trust-owner` (the person types
//!     the fingerprint they read on the owner's own screen), each with the
//!     node keys a person accepted from its rosters (`roster_changes = ask`,
//!     vs-live S1).
//! - **roster marks** (`/var/k2sign/k2-signer/rosters/<owner fp>.json`,
//!   the signing user's): the highest `seq` seen per owner, its digest and
//!   when it was issued (V13), plus keys accepted under
//!   `roster_changes = auto`. Written only when evidence registers.

use std::collections::BTreeMap;
use std::path::PathBuf;

use k2_node_proto::admin::ApprovalKey;
use serde::{Deserialize, Serialize};

use crate::ctx::{self, Ctx};
use crate::{refused, Fail, R};

pub const TRUST_V: u32 = 1;
pub const MAX_TRUST_BYTES: u64 = 1 << 20;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Trust {
    pub v: u32,
    pub controllers: BTreeMap<String, ControllerPin>,
    pub owners: BTreeMap<String, OwnerPin>,
    /// M7: per project, whether a node another owner runs needs the client
    /// server's pairing pin in the evidence. Unset = required for client
    /// projects, optional for `k2` ([`Trust::pairing_pins_required`]).
    #[serde(default)]
    pub pairing_pins: BTreeMap<String, bool>,
}

impl Default for Trust {
    fn default() -> Self {
        Self { v: TRUST_V, controllers: BTreeMap::new(), owners: BTreeMap::new(), pairing_pins: BTreeMap::new() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PinHow {
    /// `k2-signer-admin pair-accept`: code + SAS a person confirmed here.
    Sas,
    /// `k2-signer-admin adopt-k2sign`: k2sign's S3b controller pin, which
    /// root derived from the controller's key on this Mac (V28).
    K2signAdopted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerPin {
    pub label: String,
    /// The server's tunnel key (base64 SPKI DER); `null` when adopted from
    /// k2sign (which pinned the fingerprint only).
    pub spki: Option<String>,
    pub how: PinHow,
    pub at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnerPin {
    /// `ed25519:<b64>`; its SHA-256 is the map key.
    pub key: String,
    pub label: String,
    pub at: i64,
    /// Node keys a person accepted from this owner's rosters, by node fp.
    pub accepted: BTreeMap<String, AcceptedKey>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptedKey {
    /// The roster entry's node name when accepted.
    pub name: String,
    /// The roster `seq` it was accepted from.
    pub seq: u64,
    pub at: i64,
    /// M4: [`entry_attrs`] of the entry as accepted; another value (a new
    /// host, provider, site, admin reach or taint basis) asks again.
    pub attrs: String,
}

/// The digest of what a person accepts about a roster entry (M4): the key
/// and the owner's word on it, not its status (retire and revoke only
/// narrow) nor its statement (checked on every receipt).
pub fn entry_attrs(e: &k2_node_proto::node_roster::RosterEntry) -> String {
    let v = serde_json::json!({
        "node_fp": e.node_fp, "node_spki": e.node_spki, "name": e.name, "key_epoch": e.key_epoch,
        "taint_basis": e.taint_basis, "attestation_digest": e.attestation_digest, "host": e.host,
        "provider": e.provider, "site": e.site, "admin_reach": e.admin_reach, "pod": e.pod, "point": e.point,
    });
    let text = k2_node_proto::canonical::to_string(&v).unwrap_or_default();
    format!("sha256:{}", k2_node_proto::crypto::sha256_hex(text.as_bytes()))
}

impl Trust {
    pub fn path(ctx: &Ctx) -> PathBuf {
        ctx.etc().join("trust.json")
    }

    /// The installed trust (empty when none was written yet).
    pub fn load(ctx: &Ctx) -> R<Self> {
        let p = Self::path(ctx);
        if !p.exists() {
            return Ok(Self::default());
        }
        let raw = ctx::read_trusted(ctx, &p, MAX_TRUST_BYTES)?;
        let t: Trust = serde_json::from_slice(&raw).map_err(|e| Fail::Refused(format!("trust.json: {e}")))?;
        t.check()?;
        Ok(t)
    }

    pub fn check(&self) -> R<()> {
        if self.v != TRUST_V {
            return refused(format!("trust.json v{} (this build knows v{TRUST_V})", self.v));
        }
        for fp in self.controllers.keys() {
            if !k2_node_proto::ci::is_fp(fp) {
                return refused("trust.json: a controller fingerprint is not 64 hex");
            }
        }
        for (fp, o) in &self.controllers {
            if let Some(spki) = &o.spki {
                let der = k2_node_proto::crypto::unb64(spki).map_err(|e| Fail::Refused(format!("trust.json: controller {fp}: {e}")))?;
                if k2_node_proto::crypto::fingerprint_of_spki_der(&der) != *fp {
                    return refused(format!("trust.json: controller {fp}'s key is not its fingerprint"));
                }
            }
        }
        for p in self.pairing_pins.keys() {
            if !k2_node_proto::ci::is_name(p) {
                return refused("trust.json: pairing_pins names a bad project");
            }
        }
        for (fp, o) in &self.owners {
            let k = ApprovalKey::parse(&o.key).ok_or_else(|| Fail::Refused(format!("trust.json: owner {fp}: not ed25519:<b64>")))?;
            if k.fingerprint() != *fp {
                return refused(format!("trust.json: owner {fp}'s key is not its fingerprint"));
            }
            for n in o.accepted.keys() {
                if !k2_node_proto::ci::is_fp(n) {
                    return refused("trust.json: an accepted node fingerprint is not 64 hex");
                }
            }
        }
        Ok(())
    }

    /// M7: does `project` require pairing pins for nodes it doesn't own?
    pub fn pairing_pins_required(&self, project: &str) -> bool {
        self.pairing_pins.get(project).copied().unwrap_or(project != "k2")
    }

    pub fn owner_key(&self, fp: &str) -> Option<ApprovalKey> {
        self.owners.get(fp).and_then(|o| ApprovalKey::parse(&o.key))
    }

    /// Admin only: write it root-owned 0644 and re-pin.
    pub fn save(&self, ctx: &Ctx) -> R<()> {
        self.check()?;
        let mut text = serde_json::to_string_pretty(self).map_err(|e| Fail::Internal(format!("trust.json: {e}")))?;
        text.push('\n');
        std::fs::create_dir_all(ctx.etc())?;
        let p = Self::path(ctx);
        let tmp = ctx.etc().join("trust.json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::set_permissions(&tmp, std::os::unix::fs::PermissionsExt::from_mode(0o644))?;
        std::fs::rename(&tmp, &p)?;
        ctx::repin(ctx)?;
        Ok(())
    }
}

/// The signer's own record of an owner's rosters (V13).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RosterMark {
    /// The highest `seq` registered evidence carried.
    pub seq: u64,
    /// That roster's digest (`sha256:` of its body).
    pub digest: String,
    pub issued_at: i64,
    pub seen_at: i64,
    /// Keys accepted under `roster_changes = auto`, **by project** (H1):
    /// project → node fp → acceptance. Only that project's `auto` reads them.
    pub auto_accepted: BTreeMap<String, BTreeMap<String, AcceptedKey>>,
}

pub fn marks_dir(ctx: &Ctx) -> PathBuf {
    ctx.own_dir().join("rosters")
}

/// Every owner's mark, by owner fp.
pub fn load_marks(ctx: &Ctx) -> R<BTreeMap<String, RosterMark>> {
    let d = marks_dir(ctx);
    let mut out = BTreeMap::new();
    if !d.is_dir() {
        return Ok(out);
    }
    for e in std::fs::read_dir(&d)? {
        let e = e?;
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(fp) = name.strip_suffix(".json") else { continue };
        if !k2_node_proto::ci::is_fp(fp) {
            continue;
        }
        let m: RosterMark = serde_json::from_slice(&std::fs::read(e.path())?)
            .map_err(|err| Fail::Refused(format!("roster mark {fp}: {err}")))?;
        out.insert(fp.to_string(), m);
    }
    Ok(out)
}

pub fn save_mark(ctx: &Ctx, owner_fp: &str, m: &RosterMark) -> R<()> {
    ctx::mkdirs(ctx, &ctx.own_dir())?;
    ctx::mkdirs(ctx, &marks_dir(ctx))?;
    let text = serde_json::to_vec_pretty(m).map_err(|e| Fail::Internal(e.to_string()))?;
    ctx::write_file(ctx, &marks_dir(ctx).join(format!("{owner_fp}.json")), &text, 0o600)
}
