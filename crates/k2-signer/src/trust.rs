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
    /// M7: the paired controllers this owner **is** (the client's own server
    /// runs these nodes), set by a person at `trust-owner --own-controller`.
    /// Never read from the owner's roster (`owner_server_fp` is the owner's
    /// own word).
    #[serde(default)]
    pub own_controllers: Vec<String>,
    /// L2: node keys this owner's rosters ever showed revoked; never again
    /// accepted.
    #[serde(default)]
    pub revoked: std::collections::BTreeSet<String>,
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
    /// host, provider, site, admin reach or taint basis) asks again. Empty in
    /// an older file: it never matches, so the person is asked again (L1).
    #[serde(default)]
    pub attrs: String,
    /// L2: the status and validity accepted; a wider one asks again.
    #[serde(default)]
    pub status: Option<k2_node_proto::ci::KeyStatus>,
    #[serde(default)]
    pub valid_from: Option<i64>,
    #[serde(default)]
    pub valid_to: Option<i64>,
}

impl AcceptedKey {
    pub fn of(e: &k2_node_proto::node_roster::RosterEntry, seq: u64, at: i64) -> Self {
        Self {
            name: e.name.clone(),
            seq,
            at,
            attrs: entry_attrs(e),
            status: Some(e.status),
            valid_from: Some(e.valid_from),
            valid_to: e.valid_to,
        }
    }

    /// Whether this acceptance still covers `e`: the same attributes, and a
    /// status and validity no wider than accepted (L2: an un-revoke, an
    /// un-retire, an earlier `valid_from` or a later `valid_to` asks again;
    /// narrowing never does).
    pub fn covers(&self, e: &k2_node_proto::node_roster::RosterEntry) -> bool {
        use k2_node_proto::ci::KeyStatus;
        let rank = |s: KeyStatus| match s {
            KeyStatus::Revoked => 0,
            KeyStatus::Retired => 1,
            KeyStatus::Active => 2,
        };
        let (Some(st), Some(from)) = (self.status, self.valid_from) else { return false };
        if self.attrs.is_empty() || self.attrs != entry_attrs(e) || rank(e.status) > rank(st) || e.valid_from < from {
            return false;
        }
        match (self.valid_to, e.valid_to) {
            (None, _) => true,
            (Some(_), None) => false,
            (Some(a), Some(b)) => b <= a,
        }
    }
}

/// The digest of what a person accepts about a roster entry (M4): the key
/// and the owner's word on it. Status and validity are compared separately
/// ([`AcceptedKey::covers`]: widening asks again, narrowing doesn't), and the
/// statement is checked on every receipt.
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
    /// An older, flat file (node fp → acceptance) loads with them dropped,
    /// keeping `seq` and `digest` (L1).
    #[serde(default, deserialize_with = "auto_compat")]
    pub auto_accepted: BTreeMap<String, BTreeMap<String, AcceptedKey>>,
    /// L2: keys any registered roster of this owner showed revoked.
    #[serde(default)]
    pub revoked: std::collections::BTreeSet<String>,
}

fn auto_compat<'de, D: serde::Deserializer<'de>>(d: D) -> Result<BTreeMap<String, BTreeMap<String, AcceptedKey>>, D::Error> {
    let v: serde_json::Value = Deserialize::deserialize(d)?;
    let Some(m) = v.as_object() else { return Ok(BTreeMap::new()) };
    let mut out = BTreeMap::new();
    for (project, inner) in m {
        // The old flat shape had acceptances (objects with a `name`) here.
        if inner.get("name").is_some() || k2_node_proto::ci::is_fp(project) {
            continue;
        }
        if let Ok(x) = serde_json::from_value::<BTreeMap<String, AcceptedKey>>(inner.clone()) {
            out.insert(project.clone(), x);
        }
    }
    Ok(out)
}

pub fn marks_dir(ctx: &Ctx) -> PathBuf {
    ctx.own_dir().join("rosters")
}

/// Every owner's mark, by owner fp.
pub fn load_marks(ctx: &Ctx) -> R<BTreeMap<String, RosterMark>> {
    let d = marks_dir(ctx);
    let mut out = BTreeMap::new();
    // A real folder (never a symlink), then each mark read without following one (N1).
    if ctx::var_dir(ctx, d.strip_prefix(ctx.var()).unwrap_or(&d), false)?.is_none() {
        return Ok(out);
    }
    for e in std::fs::read_dir(&d)? {
        let e = e?;
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(fp) = name.strip_suffix(".json") else { continue };
        if !k2_node_proto::ci::is_fp(fp) {
            continue;
        }
        let Some(raw) = ctx::read_file(ctx, &e.path(), 1 << 20)? else { continue };
        let m: RosterMark = serde_json::from_slice(&raw).map_err(|err| Fail::Refused(format!("roster mark {fp}: {err}")))?;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// L1: files written before `attrs` and per-project auto acceptances
    /// load; an old acceptance never covers (the person is asked again), and
    /// an old flat `auto_accepted` is dropped, keeping seq and digest.
    #[test]
    fn old_formats_load_and_ask_again() {
        let fp = "a".repeat(64);
        let old_key = r#"{"name":"node-a","seq":1,"at":5}"#;
        let k: AcceptedKey = serde_json::from_str(old_key).unwrap();
        assert!(k.attrs.is_empty() && k.status.is_none());
        let old_mark = format!(
            r#"{{"seq":7,"digest":"sha256:{d}","issued_at":1,"seen_at":2,"auto_accepted":{{"{fp}":{old_key}}}}}"#,
            d = "b".repeat(64)
        );
        let m: RosterMark = serde_json::from_str(&old_mark).unwrap();
        assert_eq!((m.seq, m.digest.len()), (7, 71));
        assert!(m.auto_accepted.is_empty() && m.revoked.is_empty());
        let new_mark = format!(r#"{{"seq":7,"digest":"x","issued_at":1,"seen_at":2,"auto_accepted":{{"acme":{{"{fp}":{old_key}}}}}}}"#);
        let m: RosterMark = serde_json::from_str(&new_mark).unwrap();
        assert!(m.auto_accepted["acme"].contains_key(&fp));
        let old_owner = r#"{"key":"ed25519:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=","label":"o","at":1,"accepted":{}}"#;
        let o: OwnerPin = serde_json::from_str(old_owner).unwrap();
        assert!(o.own_controllers.is_empty() && o.revoked.is_empty());
    }
}
