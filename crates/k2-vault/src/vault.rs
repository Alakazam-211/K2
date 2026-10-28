//! `LocalVault`: the built-in `SecretStore` over `~/.k2/vault/vault.db`.
//!
//! Key chain (§7.1 as amended by VS7/VS17):
//! - VDK (32 B, random) encrypts every value with XChaCha20-Poly1305;
//!   AAD binds vault, secret, version, namespace, scope and kind.
//! - The box holder wraps the VDK with XChaCha20-Poly1305 under a
//!   KEK-derived key (authenticated; AAD `k2-vault/v1 box-wrap ‖ vault_id`).
//!   Recovery, backup and clone-target holders are age X25519 files.
//! - `header_mac` is keyed from the KEK; row and audit MACs from the VDK.
//! - The KeyStore keeps an anchor `{vault_id, gen, audit_head}` where it
//!   can: an older vault.db → `vault_rollback`; another vault id →
//!   `vault_substituted`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use chrono::{DateTime, SecondsFormat, Utc};
use parking_lot::{Mutex, RwLock};
use rusqlite::{params, Connection, OptionalExtension, Row, Transaction};
use serde::{Deserialize, Serialize};

use crate::agefmt::{self, Recipient};
use crate::audit::{self, Entry, Rollups};
use crate::confirm::{self, Broker, KdfCost, Throttle};
use crate::crypto::{self, hex, hkdf32, hmac, hmac_verify, unhex, xopen, xseal, Enc};
use crate::db;
use crate::error::{Result, VaultError};
use crate::hosts::{self, OriginPattern};
use crate::inject::{self, Inject};
use crate::keystore::{Anchor, KeyStore};
use crate::model::*;
use crate::policy::{self, Actor, Facts, HumanProof};
use crate::recovery::{RecoverySeed, CHECK_WORDS};
use crate::redact::{Redactor, SURFACE_MIN_LEN};
use crate::secret::{Key32, SecretBytes};
use crate::store::*;

mod agent_store;
mod handoff_store;
mod placement_store;

pub use agent_store::{AgentIdentity, AgentPolicy, AgentPolicyPatch, AgentSocket, DowngradeBlockers};
pub use placement_store::{NewPlacement, Placement, PlacementTarget, Placements};

pub const FORMAT: &str = "k2-vault/1";
/// Distinct tamper notes kept (status and doctor show them).
const TAMPER_NOTES_MAX: usize = 256;
pub const VERSION_RETENTION_DAYS: i64 = 30;
pub const REQUEST_EXPIRY_DAYS: i64 = 7;
pub const GUEST_TTL_MAX_SECS: i64 = 24 * 3600;
pub const GUEST_PENDING_MAX: usize = 3;

fn now() -> DateTime<Utc> {
    Utc::now()
}

fn ts(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Now, as vault rows write it (RFC 3339, seconds, `Z`).
pub(crate) fn now_ts() -> String {
    ts(now())
}

fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&Utc))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Wrap {
    /// `recovery` | `backup` | `clone-target`
    pub holder: String,
    pub recipient: String,
    /// age file, base64.
    pub wrapped: String,
    #[serde(rename = "createdAt")]
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultStatus {
    pub open: bool,
    pub vault_id: String,
    pub format: String,
    pub key_home: String,
    pub key_home_label: String,
    pub rollback_guard: String,
    pub rollback: bool,
    pub tamper: String,
    pub recovery: String,
    pub recovery_confirmed_at: Option<String>,
    pub confirm_password: bool,
    pub gen: u64,
    pub counts: Counts,
    pub memory_lock: String,
    pub created_at: String,
    /// Who can open the vault (VU-B1): this box's key, then every age
    /// holder (recovery key, backup key). Public halves only.
    pub holders: Vec<Holder>,
}

/// One key holder: something that can open the vault's data key. Never
/// key material; the fingerprint is the first 8 bytes of the SHA-256 of
/// the age public key.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Holder {
    /// `box` (this machine's key store), `recovery`, `backup`, …
    pub kind: String,
    pub fingerprint: Option<String>,
    pub created_at: Option<String>,
}

/// A holder added without a recorded person confirm (K2 before 0.46.6
/// let a backup key be added with the owner token alone).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UnconfirmedHolder {
    pub holder: Holder,
    /// The actor stamp on the last `wrap` audit row for this holder kind.
    pub added_by: Option<String>,
}

/// Audit `consumer` on a holder change a person confirmed (VU-B1).
pub const HOLDER_CONFIRMED_PREFIX: &str = "confirmed:";

/// An age holder's fingerprint (`?` when the stored recipient doesn't parse).
fn wrap_fingerprint(recipient: &str) -> String {
    Recipient::parse(recipient).map(|r| r.fingerprint()).unwrap_or_else(|_| "?".into())
}

/// The box key first, then each age holder.
fn holders_of(wraps: &[Wrap]) -> Vec<Holder> {
    let mut out = vec![Holder { kind: "box".into(), fingerprint: None, created_at: None }];
    out.extend(wraps.iter().map(|w| Holder {
        kind: w.holder.clone(),
        fingerprint: Some(wrap_fingerprint(&w.recipient)),
        created_at: Some(w.created_at.clone()),
    }));
    out
}

/// VU-B1: changing who can open the vault (an age holder other than the
/// recovery key) is the owner's or an Admin's, with a person's confirm.
fn holder_change_allowed(holder: &str, actor: &Actor, proof: &HumanProof) -> Result<()> {
    if holder == "recovery" {
        return Err(VaultError::forbidden("vault_admin_required", "the recovery key has its own steps (k2 vault recovery)"));
    }
    if holder == crate::boxmove::MOVE_HOLDER {
        return Err(VaultError::forbidden(
            "holder_reserved",
            "the move key is added by k2 migrate export-secrets and removed by its --unfreeze or the import",
        ));
    }
    if !actor.is_admin_human() {
        return Err(VaultError::forbidden("vault_admin_required", "only the owner changes who can open the vault"));
    }
    if !matches!(proof, HumanProof::Confirmed { .. }) {
        return Err(VaultError::forbidden("vault_confirm_required", "changing who can open the vault needs a person's confirm"));
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Counts {
    pub user: u64,
    pub system: u64,
    pub disabled: u64,
    pub grants: u64,
    pub pending_requests: u64,
    pub rotate_due: u64,
    /// R2: secrets written by a newer K2 that this build skips.
    pub newer_secrets: u64,
    /// R2: grants written by a newer K2 that this build skips.
    pub newer_grants: u64,
}

#[derive(Debug, Clone)]
pub struct OpenOptions {
    pub kdf: KdfCost,
}

impl Default for OpenOptions {
    fn default() -> Self {
        OpenOptions { kdf: KdfCost::Strong }
    }
}

struct Keys {
    vdk: Key32,
    row: Key32,
    audit: Key32,
    fp: Key32,
    ph: Key32,
    /// Header MAC key (from the KEK).
    header: Key32,
    /// Box-wrap key (from the KEK).
    boxw: Key32,
}

impl Keys {
    fn derive(vdk: Key32, kek: &Key32, vault_id: &[u8; 16]) -> Keys {
        Keys {
            row: hkdf32(vdk.as_bytes(), vault_id, crypto::INFO_ROW_MAC),
            audit: hkdf32(vdk.as_bytes(), vault_id, crypto::INFO_AUDIT),
            fp: hkdf32(vdk.as_bytes(), vault_id, crypto::INFO_FINGERPRINT),
            ph: hkdf32(vdk.as_bytes(), vault_id, crypto::INFO_PLACEHOLDER),
            header: hkdf32(kek.as_bytes(), vault_id, crypto::INFO_HEADER),
            boxw: hkdf32(kek.as_bytes(), vault_id, crypto::INFO_BOX_WRAP),
            vdk,
        }
    }

    fn with_vdk(&self, vdk: Key32, vault_id: &[u8; 16]) -> Keys {
        Keys {
            row: hkdf32(vdk.as_bytes(), vault_id, crypto::INFO_ROW_MAC),
            audit: hkdf32(vdk.as_bytes(), vault_id, crypto::INFO_AUDIT),
            fp: hkdf32(vdk.as_bytes(), vault_id, crypto::INFO_FINGERPRINT),
            ph: hkdf32(vdk.as_bytes(), vault_id, crypto::INFO_PLACEHOLDER),
            header: self.header.duplicate(),
            boxw: self.boxw.duplicate(),
            vdk,
        }
    }
}

/// How far the audit chain is known good (item 3, agent-policy freshness).
#[derive(Debug, Clone, PartialEq, Eq)]
enum AuditMark {
    /// Not walked yet.
    Unknown,
    /// Rows 1..=`seq` verified; `mac` is row `seq`'s MAC; `dv` the
    /// connection's `PRAGMA data_version` then.
    Good { seq: u64, mac: Vec<u8>, dv: i64 },
    /// A check failed: stays bad until a full walk passes.
    Bad,
}

#[derive(Debug, Clone)]
struct Header {
    format: String,
    vault_id: String,
    created_at: String,
    gen: u64,
    box_wrap: Vec<u8>,
    wraps: Vec<Wrap>,
    recovery_state: String,
    recovery_confirmed_at: Option<String>,
    confirm_hash: Option<String>,
    carry_recipient: Option<String>,
    audit_seq: u64,
    audit_mac: Vec<u8>,
}

impl Header {
    fn mac_input(&self) -> Vec<u8> {
        let mut e = Enc::new("header");
        e.str(&self.format)
            .str(&self.vault_id)
            .str(&self.created_at)
            .u64(self.gen)
            .bytes(&self.box_wrap)
            .str(&serde_json::to_string(&self.wraps).expect("wraps json"))
            .str(&self.recovery_state)
            .opt_str(self.recovery_confirmed_at.as_deref())
            .opt_str(self.confirm_hash.as_deref())
            .opt_str(self.carry_recipient.as_deref())
            .u64(self.audit_seq)
            .bytes(&self.audit_mac);
        e.finish().to_vec()
    }
}

fn box_aad(vault_id: &[u8; 16]) -> Vec<u8> {
    let mut a = crypto::INFO_BOX_WRAP.to_vec();
    a.extend_from_slice(vault_id);
    a
}

fn vid_bytes(vault_id: &str) -> Result<[u8; 16]> {
    uuid::Uuid::parse_str(vault_id)
        .map(|u| *u.as_bytes())
        .map_err(|_| VaultError::tamper("vault id isn't a uuid"))
}

fn sid_bytes(id: &str) -> [u8; 16] {
    uuid::Uuid::parse_str(id).map(|u| *u.as_bytes()).unwrap_or([0u8; 16])
}

#[derive(Debug, Clone)]
struct SecretRow {
    meta: SecretMeta,
    external_ref: String,
    /// The `inject` column as stored (R1: it may carry fields this build
    /// doesn't know).
    inject_raw: String,
}

fn inject_text(m: &SecretMeta) -> String {
    m.inject.as_ref().map(|i| serde_json::to_string(i).expect("inject")).unwrap_or_default()
}

fn secret_mac_input(m: &SecretMeta, external_ref: &str) -> Vec<u8> {
    secret_mac_input_with(m, external_ref, &inject_text(m))
}

/// R1: a row's MAC is over the inject text this build would write; a newer
/// K2's row may hold an inject with an extra optional field this build
/// drops when it parses, so the stored text is also accepted. Both forms
/// need the row key, so this accepts nothing an attacker could forge.
fn row_mac_ok(key: &Key32, row: &SecretRow, mac: &[u8]) -> bool {
    let ours = inject_text(&row.meta);
    hmac_verify(key, &secret_mac_input_with(&row.meta, &row.external_ref, &ours), mac)
        || (row.inject_raw != ours && hmac_verify(key, &secret_mac_input_with(&row.meta, &row.external_ref, &row.inject_raw), mac))
}

fn secret_mac_input_with(m: &SecretMeta, external_ref: &str, inject: &str) -> Vec<u8> {
    let mut e = Enc::new("secret");
    e.str(&m.id)
        .u8(m.namespace.code())
        .u8(m.scope.kind.code())
        .str(&m.scope.id)
        .str(&m.name)
        .u8(m.kind.code())
        .str(&m.provider)
        .str(external_ref)
        .str(&serde_json::to_string(&m.hosts).expect("hosts"))
        .str(inject)
        .u8(m.default_delivery.code())
        .u8(m.delivery_ceiling.code())
        .str(&m.origin)
        .str(&m.created_by)
        .u8(u8::from(m.human_given))
        .u32(m.latest_version)
        .u8(m.status.code())
        .opt_i64(m.rotate_every_days.map(i64::from))
        .str(&m.description)
        .opt_str(m.expires_at.as_deref())
        .str(&m.created_at)
        .str(&m.updated_at);
    e.finish().to_vec()
}

fn value_aad(vault_id: &[u8; 16], m: &SecretMeta, version: u32) -> Vec<u8> {
    let mut e = Enc::new("value");
    e.bytes(vault_id)
        .bytes(&sid_bytes(&m.id))
        .u32(version)
        .u8(m.namespace.code())
        .u8(m.scope.kind.code())
        .str(&m.scope.id)
        .u8(m.kind.code());
    e.finish().to_vec()
}

#[allow(clippy::too_many_arguments)]
fn version_mac_input(secret_id: &str, version: u32, nonce: &[u8], ct: &[u8], fp: &str, created_at: &str, created_by: &str, state: &str, revoked_at: Option<&str>) -> Vec<u8> {
    let mut e = Enc::new("version");
    e.str(secret_id)
        .u32(version)
        .bytes(nonce)
        .bytes(ct)
        .str(fp)
        .str(created_at)
        .str(created_by)
        .str(state)
        .opt_str(revoked_at);
    e.finish().to_vec()
}

fn grant_mac_input(g: &Grant) -> Vec<u8> {
    let mut e = Enc::new("grant");
    e.str(&g.id)
        .str(&g.secret_id)
        .str(&g.version_selector)
        .u8(g.consumer.kind.code())
        .str(&g.consumer.id)
        .u8(g.delivery.code())
        .opt_str(g.env_name.as_deref())
        .opt_str(g.file_name.as_deref())
        .str(&g.hosts_narrowed.as_ref().map(|h| serde_json::to_string(h).expect("hosts")).unwrap_or_default())
        .str(&g.granted_by)
        .str(&g.granted_at)
        .opt_str(g.expires_at.as_deref());
    e.finish().to_vec()
}

/// A secret card's terms for an existing name (SC-A1), checked as the
/// answering person's policy change and written with the new version.
struct CardTerms {
    meta: SecretMeta,
    loosens: bool,
}

/// Sites as people read them: `https://robot-ws.your-server.de:443` →
/// `robot-ws.your-server.de`.
pub fn sites_words(hosts: &[String]) -> String {
    hosts
        .iter()
        .map(|h| {
            let rest = h.split("://").nth(1).unwrap_or(h);
            match rest.rsplit_once(':') {
                Some((host, "443")) if h.starts_with("https://") => host.to_string(),
                _ => rest.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// A Thread secret card request (§10.1, `secret_request`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SecretRequest {
    pub card_id: String,
    pub conversation: String,
    pub session_id: Option<String>,
    pub workspace_id: String,
    pub requested_by: String,
    pub name: String,
    pub kind: Kind,
    /// `workspace` | `server` | `session`
    pub proposed_scope: String,
    pub proposed_hosts: Vec<String>,
    pub proposed_delivery: Delivery,
    pub prompt: String,
    pub ttl_secs: Option<i64>,
    /// `pending` | `set` | `void` | `expired`
    pub status: String,
    pub answered_by: Option<String>,
    pub secret_id: Option<String>,
    pub created_at: String,
    pub answered_at: Option<String>,
    /// How the card says the secret is sent (prd-secret-card-injection-v1
    /// §1). Sealed in `secret_request_inject` with its own MAC (SC-A2), so
    /// it is not part of [`request_mac_input`]. None: the card names none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inject: Option<Inject>,
}

/// The MAC over a card's sealed injection: (card id, inject JSON) (SC-A2).
fn request_inject_mac_input(card_id: &str, inject_json: &str) -> Vec<u8> {
    let mut e = Enc::new("request_inject");
    e.str(card_id).str(inject_json);
    e.finish().to_vec()
}

fn request_mac_input(r: &SecretRequest) -> Vec<u8> {
    let mut e = Enc::new("request");
    e.str(&r.card_id)
        .str(&r.conversation)
        .opt_str(r.session_id.as_deref())
        .str(&r.workspace_id)
        .str(&r.requested_by)
        .str(&r.name)
        .u8(r.kind.code())
        .str(&r.proposed_scope)
        .str(&serde_json::to_string(&r.proposed_hosts).expect("hosts"))
        .u8(r.proposed_delivery.code())
        .str(&r.prompt)
        .opt_i64(r.ttl_secs)
        .str(&r.status)
        .opt_str(r.answered_by.as_deref())
        .opt_str(r.secret_id.as_deref())
        .str(&r.created_at)
        .opt_str(r.answered_at.as_deref());
    e.finish().to_vec()
}

/// What a person chose on a secret card.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CardAnswer {
    /// `workspace` | `server` | `session`
    pub scope: Option<String>,
    pub hosts: Option<Vec<String>>,
    pub delivery: Option<Delivery>,
    /// Second click after a red mismatch / readable-delivery warning.
    #[serde(default)]
    pub confirmed_warnings: bool,
    /// 0.46.3: the card showed, in plain words before Save, that the agent
    /// can read this value ("Sales will get this as a file it can read").
    /// That sentence is the readable-delivery disclosure, so this skips
    /// only that check. A key that looks like another provider's still
    /// needs `confirmed_warnings`. Clients that don't show the sentence
    /// (older cards, the CLI) never send it and keep the second click.
    #[serde(default)]
    pub readable_disclosed: bool,
    /// How the card showed the secret is sent, echoed back on Save
    /// (prd-secret-card-injection-v1 §2): the approval of the injection,
    /// as `readable_disclosed` is of a readable delivery. On a card that
    /// names one, a missing echo is `card_inject_unseen` (an older client
    /// that can't show it, §5); a different one is the person's own edit.
    #[serde(default)]
    pub inject: Option<Inject>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CardResult {
    pub meta: SecretMeta,
    pub reference: String,
    pub trimmed: bool,
    /// The card replaced a secret that already had this name (SC-A1).
    pub replaced: bool,
    /// How the replaced secret was sent before, in words, when the card
    /// changed it ("as Authorization: Bearer ‹secret›").
    pub inject_was: Option<String>,
}

/// Pending recovery-key setup (phrase shown once, not yet confirmed).
struct PendingRecovery {
    seed: RecoverySeed,
    actor: String,
    started: Instant,
}

pub struct RecoveryChallenge {
    pub id: String,
    pub phrase: SecretBytes,
    pub check_words: [usize; 3],
    pub recipient: String,
}

pub struct LocalVault {
    dir: PathBuf,
    conn: Mutex<Connection>,
    ks: Arc<dyn KeyStore>,
    keys: RwLock<Keys>,
    vault_id: String,
    vid: [u8; 16],
    rollback: Mutex<bool>,
    tamper: Mutex<Vec<String>>,
    /// Item 3: how far the audit chain is known good (see
    /// `agent_store`): the full chain is walked at open and on the minute
    /// tick, the sign path walks only the tail since this mark.
    audit_mark: Mutex<AuditMark>,
    /// Audit rows whose MAC the agent-policy checks verified (tests show
    /// the sign path stays incremental).
    audit_rows_verified: std::sync::atomic::AtomicU64,
    /// Things worth telling the owner that aren't tamper (doctor `info`).
    info: Mutex<Vec<String>>,
    rollups: Mutex<Rollups>,
    pub confirm: Broker,
    pub throttle: Throttle,
    kdf: KdfCost,
    redactor: Mutex<Option<Arc<Redactor>>>,
    recovery_pending: Mutex<HashMap<String, PendingRecovery>>,
    /// prd-box-move-carry-v1 §3.2: this vault was exported for a box move
    /// (`moved-out.json` beside vault.db). Reads work; every write is
    /// refused with `vault_moved_out` (423) except audit rows, roll-ups
    /// and session placeholders, which never move `gen` while frozen.
    frozen: std::sync::atomic::AtomicBool,
}

impl LocalVault {
    // ── open / create ───────────────────────────────────────────────

    /// Open (or create) the vault in `dir` (normally `~/.k2/vault`).
    pub fn open(dir: impl Into<PathBuf>, ks: Arc<dyn KeyStore>, opts: OpenOptions) -> Result<LocalVault> {
        let dir = dir.into();
        crate::fsutil::ensure_private_dir(&dir)?;
        let path = dir.join("vault.db");
        crate::fsutil::touch_private(&path)?;
        let conn = Connection::open(&path)?;
        db::configure(&conn)?;
        db::migrate(&conn)?;
        for side in ["vault.db-wal", "vault.db-shm"] {
            let p = dir.join(side);
            if p.exists() {
                crate::fsutil::touch_private(&p)?;
            }
        }
        let existing = load_header(&conn)?;
        let (header, keys, tamper_notes) = match existing {
            None => {
                let kek = ks.load_or_init()?;
                let vault_id = uuid::Uuid::new_v4().to_string();
                let vid = vid_bytes(&vault_id)?;
                let vdk = Key32::random();
                let keys = Keys::derive(vdk, &kek, &vid);
                let (n, ct) = xseal(&keys.boxw, &box_aad(&vid), keys.vdk.as_bytes());
                let mut box_wrap = n.to_vec();
                box_wrap.extend_from_slice(&ct);
                let mut h = Header {
                    format: FORMAT.into(),
                    vault_id: vault_id.clone(),
                    created_at: ts(now()),
                    gen: 1,
                    box_wrap,
                    wraps: vec![],
                    recovery_state: "unconfirmed".into(),
                    recovery_confirmed_at: None,
                    confirm_hash: None,
                    carry_recipient: None,
                    audit_seq: 0,
                    audit_mac: vec![0u8; 32],
                };
                let mut notes = vec![];
                if let Some(a) = ks.anchor()? {
                    if a.vault_id.is_some() {
                        notes.push("vault.db was missing and has been recreated empty".to_string());
                    }
                }
                // Start the chain with a create row.
                let e = Entry::new("create", "system:vault", "ok");
                let t = ts(now());
                let mac = audit::row_mac(&keys.audit, 1, &t, &e, &h.audit_mac);
                conn.execute(
                    "INSERT INTO audit (seq, ts, secret_id, version, action, actor, consumer, target_host, outcome, count, prev_mac, row_mac)
                     VALUES (1, ?1, NULL, NULL, 'create', 'system:vault', NULL, NULL, 'ok', 1, ?2, ?3)",
                    params![t, h.audit_mac, mac.to_vec()],
                )?;
                h.audit_seq = 1;
                h.audit_mac = mac.to_vec();
                insert_header(&conn, &h, &keys.header)?;
                ks.set_anchor(&Anchor {
                    vault_id: Some(vault_id.clone()),
                    gen: h.gen,
                    audit_seq: Some(h.audit_seq),
                    audit_mac: Some(hex(&h.audit_mac)),
                })?;
                (h, keys, notes)
            }
            Some((h, mac)) => {
                let vid = vid_bytes(&h.vault_id)?;
                let kek = ks.load()?.ok_or_else(|| {
                    VaultError::new(
                        "vault_key_missing",
                        500,
                        format!("the vault's box key isn't in {} — restore it, or restore the vault with the recovery key", ks.kind().describe()),
                    )
                })?;
                let header_key = hkdf32(kek.as_bytes(), &vid, crypto::INFO_HEADER);
                if !hmac_verify(&header_key, &h.mac_input(), &mac) {
                    return Err(VaultError::new(
                        "vault_substituted",
                        500,
                        "vault.db wasn't made with this box's key (header check failed)",
                    ));
                }
                let boxw = hkdf32(kek.as_bytes(), &vid, crypto::INFO_BOX_WRAP);
                if h.box_wrap.len() < 24 {
                    return Err(VaultError::tamper("box wrap too short"));
                }
                let mut vdk_bytes = xopen(&boxw, &h.box_wrap[..24], &box_aad(&vid), &h.box_wrap[24..])?;
                if vdk_bytes.len() != 32 {
                    return Err(VaultError::tamper("data key has the wrong length"));
                }
                let mut arr = [0u8; 32];
                arr.copy_from_slice(&vdk_bytes);
                zeroize::Zeroize::zeroize(&mut vdk_bytes);
                let keys = Keys::derive(Key32::from_array(arr), &kek, &vid);
                (h, keys, vec![])
            }
        };
        let vid = vid_bytes(&header.vault_id)?;
        let v = LocalVault {
            dir,
            conn: Mutex::new(conn),
            ks,
            keys: RwLock::new(keys),
            vault_id: header.vault_id.clone(),
            vid,
            rollback: Mutex::new(false),
            tamper: Mutex::new(tamper_notes),
            audit_mark: Mutex::new(AuditMark::Unknown),
            audit_rows_verified: std::sync::atomic::AtomicU64::new(0),
            info: Mutex::new(Vec::new()),
            rollups: Mutex::new(Rollups::default()),
            confirm: Broker::default(),
            throttle: Throttle::default(),
            kdf: opts.kdf,
            redactor: Mutex::new(None),
            recovery_pending: Mutex::new(HashMap::new()),
            frozen: std::sync::atomic::AtomicBool::new(false),
        };
        if v.dir.join(crate::boxmove::FROZEN_MARKER).exists() {
            v.frozen.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        v.check_anchor(&header)?;
        v.verify_all();
        // The full audit chain, once, off the sign path (item 3).
        v.verify_audit_full();
        Ok(v)
    }

    fn check_anchor(&self, h: &Header) -> Result<()> {
        let a = match self.ks.anchor()? {
            Some(a) => a,
            None => {
                if self.ks.supports_anchor() {
                    // First open with an anchor-capable store (e.g. after
                    // an upgrade): write it now.
                    self.write_anchor(h);
                }
                return Ok(());
            }
        };
        if let Some(id) = &a.vault_id {
            if id != &h.vault_id {
                return Err(VaultError::new(
                    "vault_substituted",
                    500,
                    "vault.db belongs to another vault than the one this box's key remembers",
                ));
            }
        }
        if h.gen < a.gen {
            *self.rollback.lock() = true;
            self.tamper_once(format!("vault.db is older than expected (gen {} < {})", h.gen, a.gen));
            return Ok(());
        }
        if let (Some(seq), Some(mac)) = (a.audit_seq, a.audit_mac.as_deref()) {
            if h.gen == a.gen || h.gen == a.gen + 1 {
                let conn = self.conn.lock();
                let row: Option<Vec<u8>> = conn
                    .query_row("SELECT row_mac FROM audit WHERE seq = ?1", params![seq as i64], |r| r.get(0))
                    .optional()?;
                drop(conn);
                if row.as_deref().map(hex).as_deref() != Some(mac) && seq <= h.audit_seq {
                    self.tamper_once(format!("audit row {seq} doesn't match the anchored copy"));
                } else if seq > h.audit_seq {
                    self.tamper_once("audit log is missing its newest rows".to_string());
                }
            }
        }
        if h.gen > a.gen {
            // Our own commit landed but the anchor write didn't: repair.
            self.write_anchor(h);
        }
        Ok(())
    }

    fn write_anchor(&self, h: &Header) {
        let a = Anchor {
            vault_id: Some(h.vault_id.clone()),
            gen: h.gen,
            audit_seq: Some(h.audit_seq),
            audit_mac: Some(hex(&h.audit_mac)),
        };
        if let Err(e) = self.ks.set_anchor(&a) {
            // The next open accepts gen+1 and repairs (VS8).
            self.tamper.lock().retain(|t| !t.starts_with("anchor write failed"));
            self.tamper_once(format!("anchor write failed: {}", e.code));
        } else {
            self.tamper.lock().retain(|t| !t.starts_with("anchor write failed"));
        }
    }

    /// Check every row MAC and the audit chain; record problems.
    pub fn verify_all(&self) {
        let keys = self.keys.read();
        let conn = self.conn.lock();
        let mut problems: Vec<String> = vec![];
        if let Err(e) = verify_chain(&conn, &keys.audit) {
            problems.push(e.hint);
        }
        if let Ok(Some((h, _))) = load_header(&conn) {
            let last: Option<(i64, Vec<u8>)> = conn
                .query_row("SELECT seq, row_mac FROM audit ORDER BY seq DESC LIMIT 1", [], |r| Ok((r.get(0)?, r.get(1)?)))
                .optional()
                .unwrap_or(None);
            match last {
                Some((s, m)) if s as u64 == h.audit_seq && m == h.audit_mac => {}
                _ => problems.push("audit log doesn't end where the header says".into()),
            }
        }
        if let Ok(mut st) = conn.prepare("SELECT * FROM secret") {
            if let Ok(rows) = st.query_map([], |r| Ok(read_secret_raw(r))) {
                for row in rows.flatten() {
                    match row {
                        Ok((sr, mac)) => {
                            if !row_mac_ok(&keys.row, &sr, &mac) {
                                problems.push(format!("secret {} failed its integrity check", sr.meta.id));
                            }
                        }
                        Err(e) if e.is_needs_newer() => {}
                        Err(e) => problems.push(e.hint),
                    }
                }
            }
        }
        let mut t = self.tamper.lock();
        t.retain(|x| x.starts_with("vault.db was missing") || x.starts_with("vault.db is older") || x.starts_with("anchor") || x.starts_with("audit row"));
        t.extend(problems);
    }

    pub fn vault_id(&self) -> &str {
        &self.vault_id
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn key_store(&self) -> &Arc<dyn KeyStore> {
        &self.ks
    }

    pub fn in_rollback(&self) -> bool {
        *self.rollback.lock()
    }

    pub fn tamper_notes(&self) -> Vec<String> {
        self.tamper.lock().clone()
    }

    /// Notes for `k2 vault doctor` that aren't tamper (e.g. agent policies
    /// reset by an upgrade from a pre-release build).
    pub fn info_notes(&self) -> Vec<String> {
        self.info.lock().clone()
    }

    fn info_once(&self, note: String) {
        let mut t = self.info.lock();
        if t.len() < TAMPER_NOTES_MAX && !t.contains(&note) {
            t.push(note);
        }
    }

    /// Record a tamper note once: a repeated read of the same bad row (a
    /// sign loop, every spawn) never grows the list; it is also capped.
    fn tamper_once(&self, note: String) {
        let mut t = self.tamper.lock();
        if t.len() < TAMPER_NOTES_MAX && !t.contains(&note) {
            t.push(note);
        }
    }

    // ── internal: transactions, header, audit ───────────────────────

    /// Was this vault exported for a box move (reads only until
    /// `k2 migrate export-secrets --unfreeze`)?
    pub fn is_frozen(&self) -> bool {
        self.frozen.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub(crate) fn set_frozen(&self, on: bool) {
        // Under the connection lock: a write that already holds it
        // finishes first, and every later one sees the new state.
        let _conn = self.conn.lock();
        self.frozen.store(on, std::sync::atomic::Ordering::SeqCst);
    }

    /// Run a mutation: body + audit rows + gen bump + header MAC in one
    /// transaction, then the anchor (VS8 write order). Refused while the
    /// vault is frozen for a box move.
    fn mutate<T>(&self, bump_gen: bool, f: impl FnOnce(&Transaction<'_>, &Keys) -> Result<(T, Vec<Entry>)>) -> Result<T> {
        self.mutate_inner(bump_gen, false, f)
    }

    /// The writes a frozen vault still takes (audit rows, session
    /// placeholders). They never move `gen` while frozen, so the export
    /// snapshot stays the newest anchored state of this vault.
    fn mutate_frozen_ok<T>(&self, bump_gen: bool, f: impl FnOnce(&Transaction<'_>, &Keys) -> Result<(T, Vec<Entry>)>) -> Result<T> {
        self.mutate_inner(bump_gen, true, f)
    }

    fn mutate_inner<T>(&self, bump_gen: bool, frozen_ok: bool, f: impl FnOnce(&Transaction<'_>, &Keys) -> Result<(T, Vec<Entry>)>) -> Result<T> {
        self.flush_rollups_if_due();
        let keys = self.keys.read();
        let mut conn = self.conn.lock();
        // Checked under the connection lock: the export sets the freeze
        // under this lock and then snapshots, so no write lands between.
        let frozen = self.is_frozen();
        if frozen && !frozen_ok {
            return Err(crate::boxmove::moved_out());
        }
        let bump_gen = bump_gen && !frozen;
        let tx = conn.transaction()?;
        let (out, entries) = f(&tx, &keys)?;
        let mut h = load_header(&tx)?.ok_or_else(|| VaultError::internal("header missing"))?.0;
        for e in &entries {
            append_audit(&tx, &keys.audit, &mut h, e)?;
        }
        let rollback = *self.rollback.lock();
        if bump_gen && !rollback {
            h.gen += 1;
        }
        write_header(&tx, &h, &keys.header)?;
        tx.commit()?;
        drop(conn);
        drop(keys);
        if bump_gen && !rollback {
            self.write_anchor(&h);
        }
        *self.redactor.lock() = None;
        Ok(out)
    }

    /// Append audit rows without other changes (denies, roll-ups).
    fn audit_only(&self, entries: Vec<Entry>) {
        if entries.is_empty() {
            return;
        }
        let r = self.mutate_frozen_ok(false, |_, _| Ok(((), entries)));
        if let Err(e) = r {
            self.tamper_once(format!("audit write failed: {}", e.code));
        }
    }

    /// Record a carry in the vault's own audit log (H21): one row per
    /// secret (`secrets` = id + version; never values) and one summary row
    /// whose `count` is the number of secrets. `peer` names the other end
    /// (e.g. `carry:<recipient fingerprint>@<vault id>`) and is stored as
    /// the consumer. The actor is the caller (agent + session, or the
    /// person), not `system:carry`. Never blocks the carry.
    pub fn audit_carry(&self, action: &str, actor: &Actor, peer: &str, secrets: &[(String, u32)]) {
        let stamp = actor.stamp();
        let mut rows: Vec<Entry> =
            secrets.iter().map(|(id, v)| Entry::new(action, &stamp, "ok").secret(id, Some(*v)).consumer(peer)).collect();
        let mut sum = Entry::new(action, &stamp, "ok").consumer(peer);
        sum.count = secrets.len() as u64;
        rows.push(sum);
        self.audit_only(rows);
    }

    fn deny(&self, mut e: Entry, err: VaultError) -> VaultError {
        e.outcome = format!("deny:{}", err.code);
        self.audit_only(vec![e]);
        err
    }

    pub fn flush_rollups(&self) {
        let rows = self.rollups.lock().drain();
        if !rows.is_empty() {
            let _ = self.mutate_frozen_ok(true, |_, _| Ok(((), rows)));
        }
    }

    fn flush_rollups_if_due(&self) {
        let rows = {
            let mut r = self.rollups.lock();
            if !r.due() {
                return;
            }
            r.drain()
        };
        // Write directly (we're not inside a transaction here).
        let keys = self.keys.read();
        let mut conn = self.conn.lock();
        let bump = !*self.rollback.lock() && !self.is_frozen();
        let res = (|| -> Result<Header> {
            let tx = conn.transaction()?;
            let mut h = load_header(&tx)?.ok_or_else(|| VaultError::internal("header missing"))?.0;
            for e in &rows {
                append_audit(&tx, &keys.audit, &mut h, e)?;
            }
            if bump {
                h.gen += 1;
            }
            write_header(&tx, &h, &keys.header)?;
            tx.commit()?;
            Ok(h)
        })();
        drop(conn);
        drop(keys);
        match res {
            Ok(h) => {
                if bump {
                    self.write_anchor(&h);
                }
            }
            Err(e) => self.tamper_once(format!("audit roll-up write failed: {}", e.code)),
        }
    }

    // ── internal: reads ─────────────────────────────────────────────

    fn find_row(&self, conn: &Connection, keys: &Keys, sel: &Selector) -> Result<SecretRow> {
        let r = match sel {
            Selector::Id(id) => conn
                .query_row("SELECT * FROM secret WHERE id = ?1", params![id], |r| Ok(read_secret_raw(r)))
                .optional()?,
            Selector::Name { namespace, scope, name } => conn
                .query_row(
                    "SELECT * FROM secret WHERE namespace = ?1 AND scope_kind = ?2 AND scope_id = ?3 AND name = ?4",
                    params![namespace.as_str(), scope.kind.as_str(), scope.id, name],
                    |r| Ok(read_secret_raw(r)),
                )
                .optional()?,
        };
        let (row, mac) = r.ok_or_else(|| VaultError::not_found("no such secret"))??;
        if !row_mac_ok(&keys.row, &row, &mac) {
            self.tamper_once(format!("secret {} failed its integrity check", row.meta.id));
            return Err(VaultError::tamper("this secret's record failed its integrity check"));
        }
        Ok(row)
    }

    fn grants_for(&self, conn: &Connection, keys: &Keys, secret_id: &str) -> Result<Vec<Grant>> {
        let mut st = conn.prepare("SELECT * FROM grants WHERE secret_id = ?1 ORDER BY granted_at")?;
        let rows = st.query_map(params![secret_id], |r| Ok(read_grant_raw(r)))?;
        let mut out = vec![];
        for row in rows {
            let Some((g, mac)) = tolerate(row)? else { continue };
            if !hmac_verify(&keys.row, &grant_mac_input(&g), &mac) {
                self.tamper_once(format!("grant {} failed its integrity check", g.id));
                continue;
            }
            out.push(g);
        }
        Ok(out)
    }

    fn facts_for<'a>(&self, conn: &Connection, keys: &Keys, m: &'a SecretMeta, actor: &Actor) -> Result<Facts<'a>> {
        let own_ws_has_grant = match actor.agent() {
            Some(a) => self
                .grants_for(conn, keys, &m.id)?
                .iter()
                .any(|g| g.consumer.kind == ConsumerKind::Workspace && (g.consumer.id == a.workspace_id || g.consumer.id == "*")),
            None => false,
        };
        Ok(Facts {
            namespace: m.namespace,
            scope: &m.scope,
            created_by: &m.created_by,
            human_given: m.human_given,
            own_ws_has_grant,
        })
    }

    fn granted_to_actor(&self, conn: &Connection, keys: &Keys, m: &SecretMeta, actor: &Actor) -> Result<bool> {
        let a = match actor.agent() {
            Some(a) => a,
            None => return Ok(false),
        };
        Ok(self.grants_for(conn, keys, &m.id)?.iter().any(|g| match g.consumer.kind {
            ConsumerKind::Workspace => g.consumer.id == a.workspace_id || g.consumer.id == "*",
            ConsumerKind::Session => a.session_id.as_deref() == Some(g.consumer.id.as_str()),
            _ => false,
        }))
    }

    fn decrypt_version(&self, conn: &Connection, keys: &Keys, m: &SecretMeta, version: u32) -> Result<SecretBytes> {
        let row: Option<(Vec<u8>, Vec<u8>, String, String, String, String, Option<String>, Vec<u8>)> = conn
            .query_row(
                "SELECT nonce, ciphertext, fingerprint, created_at, created_by, state, revoked_at, row_mac FROM secret_version WHERE secret_id = ?1 AND version = ?2",
                params![m.id, version],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?)),
            )
            .optional()?;
        let (nonce, ct, fp, cat, cby, state, rev, mac) = row.ok_or_else(|| VaultError::not_found("that version doesn't exist"))?;
        if !hmac_verify(&keys.row, &version_mac_input(&m.id, version, &nonce, &ct, &fp, &cat, &cby, &state, rev.as_deref()), &mac) {
            self.tamper_once(format!("secret {} v{} failed its integrity check", m.id, version));
            return Err(VaultError::tamper("this value failed its integrity check"));
        }
        if state == "destroyed" {
            return Err(VaultError::not_found("that version was destroyed"));
        }
        let pt = xopen(&keys.vdk, &nonce, &value_aad(&self.vid, m, version), &ct)?;
        Ok(SecretBytes::new(pt))
    }

    fn fingerprint(&self, keys: &Keys, v: &[u8]) -> String {
        hex(&hmac(&keys.fp, v))
    }

    // ── put / rotate ────────────────────────────────────────────────

    fn insert_version(&self, tx: &Transaction<'_>, keys: &Keys, m: &SecretMeta, version: u32, value: &SecretBytes, by: &str) -> Result<()> {
        let (nonce, ct) = xseal(&keys.vdk, &value_aad(&self.vid, m, version), value.expose());
        let fp = self.fingerprint(keys, value.expose());
        let cat = ts(now());
        let mac = hmac(&keys.row, &version_mac_input(&m.id, version, &nonce, &ct, &fp, &cat, by, "active", None));
        tx.execute(
            "INSERT INTO secret_version (secret_id, version, nonce, ciphertext, fingerprint, created_at, created_by, state, revoked_at, row_mac)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'active', NULL, ?8)",
            params![m.id, version, nonce.to_vec(), ct, fp, cat, by, mac.to_vec()],
        )?;
        Ok(())
    }

    /// Mark older active versions `revoked` (kept 30 days for rollback).
    fn revoke_older(&self, tx: &Transaction<'_>, keys: &Keys, secret_id: &str, keep: u32) -> Result<()> {
        let rows: Vec<(u32, Vec<u8>, Vec<u8>, String, String, String)> = {
            let mut st = tx.prepare("SELECT version, nonce, ciphertext, fingerprint, created_at, created_by FROM secret_version WHERE secret_id = ?1 AND state = 'active' AND version != ?2")?;
            let it = st.query_map(params![secret_id, keep], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))?;
            it.collect::<std::result::Result<_, _>>()?
        };
        let rat = ts(now());
        for (v, n, ct, fp, cat, cby) in rows {
            let mac = hmac(&keys.row, &version_mac_input(secret_id, v, &n, &ct, &fp, &cat, &cby, "revoked", Some(&rat)));
            tx.execute(
                "UPDATE secret_version SET state = 'revoked', revoked_at = ?3, row_mac = ?4 WHERE secret_id = ?1 AND version = ?2",
                params![secret_id, v, rat, mac.to_vec()],
            )?;
        }
        Ok(())
    }

    fn write_secret(&self, tx: &Transaction<'_>, keys: &Keys, m: &SecretMeta, external_ref: &str, insert: bool) -> Result<()> {
        let mac = hmac(&keys.row, &secret_mac_input(m, external_ref));
        let hosts = serde_json::to_string(&m.hosts).expect("hosts");
        let inject = m.inject.as_ref().map(|i| serde_json::to_string(i).expect("inject")).unwrap_or_default();
        if insert {
            tx.execute(
                "INSERT INTO secret (id, namespace, scope_kind, scope_id, name, kind, provider, external_ref, allowed_hosts, inject,
                    default_delivery, delivery_ceiling, origin, created_by, human_given, latest_version, status, rotate_every_days,
                    description, expires_at, created_at, updated_at, row_mac)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23)",
                params![
                    m.id, m.namespace.as_str(), m.scope.kind.as_str(), m.scope.id, m.name, m.kind.as_str(), m.provider,
                    external_ref, hosts, inject, m.default_delivery.as_str(), m.delivery_ceiling.as_str(), m.origin,
                    m.created_by, m.human_given as i64, m.latest_version, m.status.as_str(), m.rotate_every_days,
                    m.description, m.expires_at, m.created_at, m.updated_at, mac.to_vec()
                ],
            )?;
        } else {
            tx.execute(
                "UPDATE secret SET allowed_hosts = ?2, inject = ?3, default_delivery = ?4, delivery_ceiling = ?5,
                    human_given = ?6, latest_version = ?7, status = ?8, rotate_every_days = ?9, description = ?10,
                    expires_at = ?11, updated_at = ?12, row_mac = ?13
                 WHERE id = ?1",
                params![
                    m.id, hosts, inject, m.default_delivery.as_str(), m.delivery_ceiling.as_str(), m.human_given as i64,
                    m.latest_version, m.status.as_str(), m.rotate_every_days, m.description, m.expires_at, m.updated_at,
                    mac.to_vec()
                ],
            )?;
        }
        Ok(())
    }

    fn prepare_value(kind: Kind, mut value: SecretBytes) -> Result<(SecretBytes, bool)> {
        if value.is_empty() {
            return Err(VaultError::bad_request("empty_value", "the value is empty"));
        }
        if value.len() > 1024 * 1024 {
            return Err(VaultError::bad_request("value_too_large", "values are at most 1 MiB"));
        }
        let trimmed = if kind.trims_on_save() { value.trim_ascii_whitespace() } else { false };
        if value.is_empty() {
            return Err(VaultError::bad_request("empty_value", "the value is empty"));
        }
        Ok((value, trimmed))
    }

    fn destination_of(v: &[u8]) -> Option<(String, u16)> {
        let s = std::str::from_utf8(v).ok()?;
        let u = url::Url::parse(s.trim()).ok()?;
        let host = u.host_str()?.to_ascii_lowercase();
        Some((host, u.port_or_known_default().unwrap_or(0)))
    }

    fn put_inner(&self, req: PutRequest, value: SecretBytes, actor: &Actor) -> Result<PutOutcome> {
        validate_name(&req.name, req.namespace == Namespace::User)?;
        // `person` (a person's wallet, 0.46.3) opens in the system namespace
        // only; `validate_for` refuses it in `user` like before.
        req.scope.validate_for(req.namespace).or_else(|e| if req.scope.kind == ScopeKind::Engine && req.namespace == Namespace::System { Ok(()) } else { Err(e) })?;
        if req.scope.kind == ScopeKind::Engine && req.namespace != Namespace::System {
            return Err(VaultError::bad_request("bad_scope", "engine scope is for K2's own secrets"));
        }
        if req.namespace == Namespace::User && *self.rollback.lock() {
            return Err(VaultError::new("vault_rollback", 409, "the vault looks rolled back; the owner must confirm in Settings → Vault"));
        }
        let (value, trimmed) = Self::prepare_value(req.kind, value)?;
        let stamp = actor.stamp();
        let entry_base = Entry::new("set", &stamp, "ok");
        let existing = {
            let keys = self.keys.read();
            let conn = self.conn.lock();
            let sel = Selector::Name { namespace: req.namespace, scope: req.scope.clone(), name: req.name.clone() };
            match self.find_row(&conn, &keys, &sel) {
                Ok(r) => Some(r),
                Err(e) if e.status == 404 => None,
                Err(e) => return Err(e),
            }
        };
        match existing {
            Some(row) => {
                if req.mode == PutMode::Create {
                    return Err(VaultError::conflict("secret_exists", format!("{} already exists; use rotate", req.name)));
                }
                // A new ssh_key version must read as a key (item 3), unless
                // another vault already accepted it.
                if row.meta.kind == Kind::SshKey && !req.landed_copy {
                    crate::ssh_agent::check_ssh_key_value(value.expose())?;
                }
                self.rotate_row(row, value, actor, req.idempotent, trimmed, None, None)
            }
            None => {
                if req.mode == PutMode::Rotate {
                    return Err(VaultError::not_found(format!("{} doesn't exist", req.name)));
                }
                if let Err(e) = policy::can_create(actor, req.namespace, &req.scope) {
                    return Err(self.deny(entry_base.clone(), e));
                }
                // Hosts: wildcards only from people (VS4).
                let hosts = hosts::normalise_list(&req.hosts, actor.may_add_wildcards())?;
                let preset = inject::detect(value.expose());
                let inject = match (&req.inject, req.kind) {
                    (Some(i), _) => Some(i.clone()),
                    (None, Kind::ApiKey | Kind::OauthToken | Kind::Generic) => Some(preset.map(|p| p.inject()).unwrap_or_default()),
                    (None, _) => None,
                };
                let signed = inject.as_ref().and_then(|i| i.sign.as_ref());
                if let Some(i) = &inject {
                    i.validate()?;
                    if i.uses_body() && !actor.may_set_body_inject() {
                        return Err(VaultError::forbidden("inject_body_needs_person", "only a person can let K2 put a secret in a request body"));
                    }
                    if let Some(s) = &i.sign {
                        s.check_value(value.expose())?;
                    }
                }
                // A signer's default and ceiling are fetch (item 2); a person
                // may raise the ceiling.
                let default_delivery = req.default_delivery.unwrap_or_else(|| if signed.is_some() { Delivery::Fetch } else { req.kind.default_delivery() });
                let ceiling = req.delivery_ceiling.unwrap_or(if signed.is_some() {
                    if default_delivery.level() > Delivery::Fetch.level() { default_delivery } else { Delivery::Fetch }
                } else if default_delivery.level() >= Delivery::Env.level() {
                    default_delivery
                } else {
                    Delivery::Env
                });
                if default_delivery.level() > ceiling.level() {
                    return Err(VaultError::bad_request("delivery_above_ceiling", "the delivery is looser than the ceiling"));
                }
                agent_store::check_agent_delivery(req.kind, &[default_delivery, ceiling], Some(value.expose()))?;
                if req.kind == Kind::SshKey && !req.landed_copy {
                    crate::ssh_agent::check_ssh_key_value(value.expose())?;
                }
                if req.scope.kind == ScopeKind::Session && req.expires_at.is_none() {
                    return Err(VaultError::bad_request("expiry_required", "chat secrets need an expiry"));
                }
                if let Some(x) = &req.expires_at {
                    parse_ts(x).ok_or_else(|| VaultError::bad_request("bad_expiry", "expiry must be RFC 3339"))?;
                }
                let human_given = req.human_given_override.unwrap_or_else(|| actor.is_human());
                let t = ts(now());
                let meta = SecretMeta {
                    id: uuid::Uuid::new_v4().to_string(),
                    namespace: req.namespace,
                    scope: req.scope.clone(),
                    name: req.name.clone(),
                    kind: req.kind,
                    provider: "local".into(),
                    hosts,
                    inject,
                    default_delivery,
                    delivery_ceiling: ceiling,
                    origin: if req.origin.is_empty() { default_origin(actor) } else { req.origin.clone() },
                    created_by: stamp.clone(),
                    human_given,
                    latest_version: 1,
                    status: SecretStatus::Active,
                    rotate_every_days: req.rotate_every_days.filter(|d| *d > 0),
                    description: req.description.chars().take(500).collect(),
                    expires_at: req.expires_at.clone(),
                    created_at: t.clone(),
                    updated_at: t,
                };
                let m2 = meta.clone();
                self.mutate(true, |tx, keys| {
                    self.write_secret(tx, keys, &m2, "", true)?;
                    self.insert_version(tx, keys, &m2, 1, &value, &stamp)?;
                    Ok(((), vec![Entry::new("create", &stamp, "ok").secret(&m2.id, Some(1))]))
                })?;
                Ok(PutOutcome { meta, created: true, new_version: true, trimmed, owner_notice: false })
            }
        }
    }

    /// `version_by`: the new version's `created_by` when it isn't simply the
    /// actor's stamp (a rollback of a version no person wrote keeps that
    /// author, review A3-P3).
    ///
    /// `terms`: a secret card's terms for an existing name (SC-A1), already
    /// checked by [`Self::plan_policy`] as the answering person's own
    /// policy change. They are written with the new version, in one
    /// transaction, so a refusal below writes neither.
    #[allow(clippy::too_many_arguments)]
    fn rotate_row(&self, row: SecretRow, value: SecretBytes, actor: &Actor, idempotent: bool, trimmed: bool, version_by: Option<String>, terms: Option<CardTerms>) -> Result<PutOutcome> {
        let stamp = actor.stamp();
        let version_stamp = version_by.unwrap_or_else(|| stamp.clone());
        let policy_outcome = terms.as_ref().filter(|t| t.meta != row.meta).map(|t| if t.loosens { "ok:widened:card" } else { "ok:card" });
        let mut m = terms.map(|t| t.meta).unwrap_or_else(|| row.meta.clone());
        if m.status == SecretStatus::Destroyed {
            // K2's own refs are reused by name (a deleted mailbox's id can
            // come back); a person's or agent's name stays retired.
            if !(actor.is_system() && m.namespace == Namespace::System) {
                return Err(VaultError::conflict("secret_destroyed", "that secret was destroyed; pick a new name"));
            }
            m.status = SecretStatus::Active;
        }
        let mut owner_notice = false;
        if let Some(s) = m.inject.as_ref().and_then(|i| i.sign.as_ref()) {
            s.check_value(value.expose())?;
        }
        agent_store::check_agent_delivery(m.kind, &[m.default_delivery, m.delivery_ceiling], Some(value.expose()))?;
        {
            let keys = self.keys.read();
            let conn = self.conn.lock();
            let f = self.facts_for(&conn, &keys, &m, actor)?;
            if let Err(e) = policy::can_manage(actor, &f) {
                drop(conn);
                drop(keys);
                return Err(self.deny(Entry::new("rotate", &stamp, "deny").secret(&m.id, None), e));
            }
            let old = self.decrypt_version(&conn, &keys, &m, m.latest_version).ok();
            if idempotent {
                if let Some(o) = &old {
                    if self.fingerprint(&keys, o.expose()) == self.fingerprint(&keys, value.expose()) {
                        return Ok(PutOutcome { meta: m, created: false, new_version: false, trimmed, owner_notice: false });
                    }
                }
            }
            // VS11: an agent replacing a person's value.
            if m.human_given && actor.agent().is_some() {
                owner_notice = true;
                if m.kind.carries_destination() {
                    let before = old.as_ref().and_then(|o| Self::destination_of(o.expose()));
                    let after = Self::destination_of(value.expose());
                    if before.is_none() || before != after {
                        return Err(VaultError::owner_confirm_required(
                            "an agent can't point this secret at a different host; the owner can",
                        ));
                    }
                }
            }
        }
        m.latest_version += 1;
        m.updated_at = ts(now());
        let new_v = m.latest_version;
        let m2 = m.clone();
        let ext = row.external_ref.clone();
        self.mutate(true, |tx, keys| {
            self.insert_version(tx, keys, &m2, new_v, &value, &version_stamp)?;
            self.revoke_older(tx, keys, &m2.id, new_v)?;
            self.write_secret(tx, keys, &m2, &ext, false)?;
            let outcome = if owner_notice { "ok:owner-notice" } else { "ok" };
            let mut entries = Vec::with_capacity(2);
            if let Some(p) = policy_outcome {
                entries.push(Entry::new("policy", &stamp, p).secret(&m2.id, None));
            }
            entries.push(Entry::new("rotate", &stamp, outcome).secret(&m2.id, Some(new_v)));
            Ok(((), entries))
        })?;
        Ok(PutOutcome { meta: m, created: false, new_version: true, trimmed, owner_notice })
    }

    /// Roll back to an earlier kept version (owner). Writes it as a new
    /// version.
    pub fn rollback_to(&self, sel: &Selector, version: u32, actor: &Actor) -> Result<SecretMeta> {
        if !actor.is_admin_human() {
            return Err(VaultError::forbidden("vault_admin_required", "rolling back needs an Owner or Admin"));
        }
        let (row, value) = {
            let keys = self.keys.read();
            let conn = self.conn.lock();
            let row = self.find_row(&conn, &keys, sel)?;
            let v = self.decrypt_version(&conn, &keys, &row.meta, version)?;
            (row, v)
        };
        // The restored value keeps who really wrote it when that wasn't a
        // person: "<person> (rollback of vN by <stamp>)", which is not a
        // person's stamp, so a later systemd-cred placement still warns and
        // needs --accept-agent-value (review A3-P3).
        let (orig, _) = self.version_author(&row.meta.id, version)?;
        let person = orig == "owner" || (orig.starts_with("connect:") && !orig.contains(' '));
        let by = (!person).then(|| format!("{} (rollback of v{version} by {orig})", actor.stamp()));
        Ok(self.rotate_row(row, value, actor, false, false, by, None)?.meta)
    }

    // ── policy ──────────────────────────────────────────────────────

    fn set_policy_inner(&self, sel: &Selector, p: PolicyPatch, actor: &Actor) -> Result<SecretMeta> {
        let stamp = actor.stamp();
        let (row, mut m, loosens) = self.plan_policy(sel, p, actor)?;
        if m == row.meta {
            return Ok(m);
        }
        // A person's changes to hosts count as human-given (card rule).
        m.updated_at = ts(now());
        let m2 = m.clone();
        let ext = row.external_ref.clone();
        let outcome = if loosens { "ok:widened" } else { "ok" };
        self.mutate(true, |tx, keys| {
            self.write_secret(tx, keys, &m2, &ext, false)?;
            Ok(((), vec![Entry::new("policy", &stamp, outcome).secret(&m2.id, None)]))
        })?;
        Ok(m)
    }

    /// A policy change, checked but not written: the row, the new meta
    /// (`updated_at` untouched) and whether it loosens. A refusal is
    /// audited here. Shared by `set_policy` and a secret card for an
    /// existing name (SC-A1), so a card's terms pass exactly the rules
    /// `k2 vault policy` by that person would.
    fn plan_policy(&self, sel: &Selector, p: PolicyPatch, actor: &Actor) -> Result<(SecretRow, SecretMeta, bool)> {
        let stamp = actor.stamp();
        let new_sign = p.inject.as_ref().and_then(|i| i.sign.clone());
        let (row, f_own_grant) = {
            let keys = self.keys.read();
            let conn = self.conn.lock();
            let row = self.find_row(&conn, &keys, sel)?;
            let f = self.facts_for(&conn, &keys, &row.meta, actor)?;
            // A new signer must fit the value already stored (OVH's JSON).
            if let Some(s) = new_sign.as_ref().filter(|s| row.meta.inject.as_ref().and_then(|i| i.sign.as_ref()) != Some(*s)) {
                if row.meta.status != SecretStatus::Destroyed {
                    let v = self.decrypt_version(&conn, &keys, &row.meta, row.meta.latest_version)?;
                    s.check_value(v.expose())?;
                }
            }
            // Item 3: agent delivery needs an ssh key the agent can serve.
            let to_agent: Vec<Delivery> = [p.default_delivery, p.delivery_ceiling].into_iter().flatten().filter(|d| *d == Delivery::Agent).collect();
            if !to_agent.is_empty() && row.meta.status != SecretStatus::Destroyed {
                let v = self.decrypt_version(&conn, &keys, &row.meta, row.meta.latest_version)?;
                agent_store::check_agent_delivery(row.meta.kind, &to_agent, Some(v.expose()))?;
            }
            (row.clone(), f.own_ws_has_grant)
        };
        let old = row.meta.clone();
        if old.status == SecretStatus::Destroyed {
            return Err(VaultError::conflict("secret_destroyed", "that secret was destroyed"));
        }
        let mut m = old.clone();
        let allow_wc = actor.may_add_wildcards();
        if let Some(h) = &p.hosts {
            m.hosts = hosts::normalise_list(h, allow_wc)?;
        }
        if !p.add_hosts.is_empty() {
            let mut add = hosts::normalise_list(&p.add_hosts, allow_wc)?;
            m.hosts.append(&mut add);
            m.hosts.sort();
            m.hosts.dedup();
        }
        if !p.remove_hosts.is_empty() {
            let rm = hosts::normalise_list(&p.remove_hosts, true)?;
            m.hosts.retain(|h| !rm.contains(h));
        }
        if let Some(d) = p.default_delivery {
            m.default_delivery = d;
        }
        if let Some(c) = p.delivery_ceiling {
            m.delivery_ceiling = c;
        }
        if let Some(i) = &p.inject {
            i.validate()?;
            if i.uses_body() && !actor.may_set_body_inject() {
                return Err(VaultError::forbidden("inject_body_needs_person", "only a person can let K2 put a secret in a request body"));
            }
            m.inject = Some(i.clone());
        }
        if let Some(d) = &p.description {
            m.description = d.chars().take(500).collect();
        }
        if let Some(r) = p.rotate_every_days {
            m.rotate_every_days = if r == 0 { None } else { Some(r) };
        }
        let loosens = hosts::widens(&old.hosts, &m.hosts)
            || m.default_delivery.level() > old.default_delivery.level()
            || m.delivery_ceiling.level() > old.delivery_ceiling.level()
            || m.inject != old.inject;
        let facts = Facts {
            namespace: old.namespace,
            scope: &old.scope,
            created_by: &old.created_by,
            human_given: old.human_given,
            own_ws_has_grant: f_own_grant,
        };
        let check = if loosens { policy::can_loosen(actor, &facts) } else { policy::can_manage(actor, &facts) };
        if let Err(e) = check {
            return Err(self.deny(Entry::new("policy", &stamp, "deny").secret(&old.id, None), e));
        }
        // After the permission check (2026-10-28): an agent that may not
        // widen this secret at all hears that (and can ask with a card),
        // not that it also has to raise the ceiling.
        if m.default_delivery.level() > m.delivery_ceiling.level() {
            return Err(VaultError::bad_request("delivery_above_ceiling", "the delivery is looser than the ceiling"));
        }
        Ok((row, m, loosens))
    }

    fn set_status(&self, sel: &Selector, actor: &Actor, to: SecretStatus) -> Result<()> {
        let stamp = actor.stamp();
        let row = {
            let keys = self.keys.read();
            let conn = self.conn.lock();
            let row = self.find_row(&conn, &keys, sel)?;
            let f = self.facts_for(&conn, &keys, &row.meta, actor)?;
            let check = match to {
                // Re-enabling is a widening.
                SecretStatus::Active => policy::can_loosen(actor, &f),
                _ => policy::can_manage(actor, &f),
            };
            if let Err(e) = check {
                drop(conn);
                drop(keys);
                return Err(self.deny(Entry::new(to_action(to), &stamp, "deny").secret(&row.meta.id, None), e));
            }
            row
        };
        let mut m = row.meta.clone();
        if m.status == SecretStatus::Destroyed {
            return Err(VaultError::conflict("secret_destroyed", "that secret was destroyed"));
        }
        if m.status == to {
            return Ok(());
        }
        m.status = to;
        m.updated_at = ts(now());
        let ext = row.external_ref.clone();
        let destroy = to == SecretStatus::Destroyed;
        self.mutate(true, |tx, keys| {
            if destroy {
                tx.execute("DELETE FROM secret_version WHERE secret_id = ?1", params![m.id])?;
                tx.execute("DELETE FROM grants WHERE secret_id = ?1", params![m.id])?;
                tx.execute("DELETE FROM placeholder WHERE secret_id = ?1", params![m.id])?;
            }
            self.write_secret(tx, keys, &m, &ext, false)?;
            Ok(((), vec![Entry::new(to_action(to), &stamp, "ok").secret(&m.id, None)]))
        })?;
        if destroy {
            self.checkpoint();
        }
        Ok(())
    }

    /// An App handover (prd-app-handoff-v1 §4.5): move a secret to another
    /// scope (and, on a name clash there, another name), keeping its id,
    /// versions and grants. Every version is sealed again: a value's AAD
    /// names its scope. Only the owner or a K2 subsystem acting on the
    /// owner's approved card; never an agent. A secret of that name in the
    /// target scope is `name_taken`. Moving to where it already is changes
    /// nothing.
    pub fn rescope(&self, sel: &Selector, to: Scope, new_name: Option<&str>, actor: &Actor) -> Result<SecretMeta> {
        if !matches!(actor, Actor::Owner | Actor::System(_)) {
            return Err(VaultError::forbidden("rescope_owner_only", "only the owner moves a secret to another scope"));
        }
        let stamp = actor.stamp();
        let row = {
            let keys = self.keys.read();
            let conn = self.conn.lock();
            self.find_row(&conn, &keys, sel)?
        };
        let old = row.meta.clone();
        if old.status == SecretStatus::Destroyed {
            return Err(VaultError::conflict("secret_destroyed", "that secret was destroyed"));
        }
        let mut m = old.clone();
        m.scope = to;
        if let Some(n) = new_name.map(str::trim).filter(|n| !n.is_empty()) {
            m.name = n.to_string();
        }
        if m.scope == old.scope && m.name == old.name {
            return Ok(old);
        }
        m.updated_at = ts(now());
        let ext = row.external_ref.clone();
        let out = m.clone();
        self.mutate(true, |tx, keys| {
            let clash: Option<String> = tx
                .query_row(
                    "SELECT id FROM secret WHERE namespace = ?1 AND scope_kind = ?2 AND scope_id = ?3 AND name = ?4",
                    params![m.namespace.as_str(), m.scope.kind.as_str(), m.scope.id, m.name],
                    |r| r.get(0),
                )
                .optional()?;
            if clash.is_some() {
                return Err(VaultError::conflict("name_taken", format!("{} already exists in that scope", m.name)));
            }
            #[allow(clippy::type_complexity)]
            let versions: Vec<(u32, Vec<u8>, Vec<u8>, String, String, String, Option<String>, String, Vec<u8>)> = {
                let mut st = tx.prepare(
                    "SELECT version, nonce, ciphertext, created_at, created_by, state, revoked_at, fingerprint, row_mac FROM secret_version WHERE secret_id = ?1",
                )?;
                let it = st.query_map(params![m.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?)))?;
                it.collect::<std::result::Result<_, _>>()?
            };
            for (v, nonce, ct, cat, cby, state, rev, fp, vmac) in versions {
                if !hmac_verify(&keys.row, &version_mac_input(&m.id, v, &nonce, &ct, &fp, &cat, &cby, &state, rev.as_deref()), &vmac) {
                    return Err(VaultError::tamper(format!("secret {} v{v} failed its integrity check; nothing moved", m.id)));
                }
                if state == "destroyed" {
                    continue;
                }
                let pt = SecretBytes::new(xopen(&keys.vdk, &nonce, &value_aad(&self.vid, &old, v), &ct)?);
                let (n2, c2) = xseal(&keys.vdk, &value_aad(&self.vid, &m, v), pt.expose());
                let vmac2 = hmac(&keys.row, &version_mac_input(&m.id, v, &n2, &c2, &fp, &cat, &cby, &state, rev.as_deref()));
                tx.execute(
                    "UPDATE secret_version SET nonce = ?3, ciphertext = ?4, row_mac = ?5 WHERE secret_id = ?1 AND version = ?2",
                    params![m.id, v, n2.to_vec(), c2, vmac2.to_vec()],
                )?;
            }
            let smac = hmac(&keys.row, &secret_mac_input(&m, &ext));
            tx.execute(
                "UPDATE secret SET scope_kind = ?2, scope_id = ?3, name = ?4, updated_at = ?5, row_mac = ?6 WHERE id = ?1",
                params![m.id, m.scope.kind.as_str(), m.scope.id, m.name, m.updated_at, smac.to_vec()],
            )?;
            Ok(((), vec![Entry::new("rescope", &stamp, "ok").secret(&m.id, None)]))
        })?;
        Ok(out)
    }

    /// An App handover (§4.5): a copy of a secret's latest value, kind,
    /// hosts and deliveries as a new secret in `to` named `name` (the
    /// original stays where it is). Owner or a K2 subsystem only.
    pub fn copy_to(&self, sel: &Selector, to: Scope, name: &str, actor: &Actor) -> Result<SecretMeta> {
        if !matches!(actor, Actor::Owner | Actor::System(_)) {
            return Err(VaultError::forbidden("copy_owner_only", "only the owner copies a secret to another scope"));
        }
        let (meta, value) = {
            let keys = self.keys.read();
            let conn = self.conn.lock();
            let row = self.find_row(&conn, &keys, sel)?;
            if row.meta.status == SecretStatus::Destroyed {
                return Err(VaultError::conflict("secret_destroyed", "that secret was destroyed"));
            }
            let v = self.decrypt_version(&conn, &keys, &row.meta, row.meta.latest_version)?;
            (row.meta, v)
        };
        let mut put = PutRequest::new(meta.namespace, to, name, meta.kind);
        put.hosts = meta.hosts.clone();
        put.inject = meta.inject.clone();
        put.default_delivery = Some(meta.default_delivery);
        put.delivery_ceiling = Some(meta.delivery_ceiling);
        put.origin = "k2 app handover".into();
        put.description = format!("copy of {} for an App handover", meta.name);
        put.human_given_override = Some(meta.human_given);
        put.landed_copy = true;
        Ok(self.put_inner(put, value, actor)?.meta)
    }

    /// Push WAL pages into the main file and truncate the WAL, so deleted
    /// ciphertext doesn't linger there (secure_delete zeroes the pages).
    pub fn checkpoint(&self) {
        let conn = self.conn.lock();
        let _ = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
    }

    // ── grants ──────────────────────────────────────────────────────

    fn grant_inner(&self, g: GrantRequest, actor: &Actor) -> Result<Grant> {
        let stamp = actor.stamp();
        let (row, facts_own) = {
            let keys = self.keys.read();
            let conn = self.conn.lock();
            let row = self.find_row(&conn, &keys, &g.selector)?;
            let f = self.facts_for(&conn, &keys, &row.meta, actor)?;
            (row.clone(), f.own_ws_has_grant)
        };
        let m = &row.meta;
        if m.status != SecretStatus::Active {
            return Err(VaultError::conflict("secret_not_active", "that secret is disabled or destroyed"));
        }
        if g.delivery == Delivery::None {
            return Err(VaultError::bad_request("bad_delivery", "pick a delivery"));
        }
        if g.delivery.level() > m.delivery_ceiling.level() {
            return Err(VaultError::forbidden(
                "delivery_above_ceiling",
                format!("this secret can be delivered at most as {}", m.delivery_ceiling),
            ));
        }
        if g.delivery == Delivery::Agent {
            let keys = self.keys.read();
            let conn = self.conn.lock();
            let v = self.decrypt_version(&conn, &keys, m, m.latest_version)?;
            agent_store::check_agent_delivery(m.kind, &[Delivery::Agent], Some(v.expose()))?;
        }
        match g.consumer.kind {
            ConsumerKind::App | ConsumerKind::Widget => {
                return Err(VaultError::bad_request("consumer_reserved", "app and widget grants aren't available yet"))
            }
            ConsumerKind::Workspace | ConsumerKind::Session | ConsumerKind::PublishedService => {}
            // `k2 job` runs (item 5): user secrets only, id `<ws>/<job>`.
            ConsumerKind::Job if m.namespace == Namespace::User && g.consumer.job_workspace().is_some() => {}
            // Compute jobs on nodes (secret-env SE13/SE14): user secrets
            // only, `<ws>/<node or *>`, env delivery only in v1.
            ConsumerKind::Compute if m.namespace == Namespace::User && g.consumer.compute_parts().is_some() => {
                if g.delivery != Delivery::Env {
                    return Err(VaultError::bad_request("bad_delivery", "a compute job gets a secret as env only (--as env)"));
                }
            }
            ConsumerKind::Engine | ConsumerKind::Daemon if m.namespace == Namespace::System => {}
            // Garden v3 data sources [GV42]: a daemon consumer `source:<id>`
            // may hold a User secret (fetch only, narrowed to one origin by
            // the caller). Not a new consumer kind, so older binaries still
            // read every grant.
            ConsumerKind::Daemon
                if m.namespace == Namespace::User && g.consumer.id.starts_with("source:") && g.delivery == Delivery::Fetch => {}
            _ => return Err(VaultError::bad_request("bad_consumer", "that consumer can't hold this secret")),
        }
        if g.consumer.id.is_empty() || g.consumer.id.len() > 128 {
            return Err(VaultError::bad_request("bad_consumer", "consumer id"));
        }
        let env_name = match (&g.env_name, g.delivery) {
            (Some(n), _) => {
                crate::delivery::validate_env_name(n)?;
                Some(n.clone())
            }
            (None, Delivery::Env) => {
                let n = crate::delivery::default_env_name(&m.name);
                crate::delivery::validate_env_name(&n)?;
                Some(n)
            }
            _ => None,
        };
        let file_name = match &g.file_name {
            Some(n) => {
                crate::delivery::validate_file_name(n)?;
                Some(n.clone())
            }
            None => None,
        };
        let hosts_narrowed = match &g.hosts_narrowed {
            Some(h) => {
                let n = hosts::normalise_list(h, true)?;
                if n.iter().any(|x| !m.hosts.contains(x)) {
                    return Err(VaultError::bad_request("hosts_not_subset", "a grant can only narrow the secret's sites"));
                }
                Some(n)
            }
            None => None,
        };
        let facts = Facts {
            namespace: m.namespace,
            scope: &m.scope,
            created_by: &m.created_by,
            human_given: m.human_given,
            own_ws_has_grant: facts_own,
        };
        if let Err(e) = policy::can_grant(actor, &facts, &g.consumer, g.delivery, m.default_delivery) {
            return Err(self.deny(Entry::new("grant", &stamp, "deny").secret(&m.id, None).consumer(g.consumer.label()), e));
        }
        if let Some(x) = &g.expires_at {
            parse_ts(x).ok_or_else(|| VaultError::bad_request("bad_expiry", "expiry must be RFC 3339"))?;
        }
        let grant = Grant {
            id: uuid::Uuid::new_v4().to_string(),
            secret_id: m.id.clone(),
            version_selector: g.version.map(|v| v.to_string()).unwrap_or_else(|| "latest".into()),
            consumer: g.consumer.clone(),
            delivery: g.delivery,
            env_name,
            file_name,
            hosts_narrowed,
            granted_by: stamp.clone(),
            granted_at: ts(now()),
            expires_at: g.expires_at.clone(),
        };
        let g2 = grant.clone();
        self.mutate(true, |tx, keys| {
            // One grant per (secret, consumer, delivery): replace.
            tx.execute(
                "DELETE FROM grants WHERE secret_id = ?1 AND consumer_kind = ?2 AND consumer_id = ?3 AND delivery = ?4",
                params![g2.secret_id, g2.consumer.kind.as_str(), g2.consumer.id, g2.delivery.as_str()],
            )?;
            let mac = hmac(&keys.row, &grant_mac_input(&g2));
            tx.execute(
                "INSERT INTO grants (id, secret_id, version_selector, consumer_kind, consumer_id, delivery, env_name, file_name, hosts_narrowed, granted_by, granted_at, expires_at, row_mac)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
                params![
                    g2.id, g2.secret_id, g2.version_selector, g2.consumer.kind.as_str(), g2.consumer.id, g2.delivery.as_str(),
                    g2.env_name, g2.file_name, g2.hosts_narrowed.as_ref().map(|h| serde_json::to_string(h).expect("hosts")),
                    g2.granted_by, g2.granted_at, g2.expires_at, mac.to_vec()
                ],
            )?;
            Ok(((), vec![Entry::new("grant", &stamp, "ok").secret(&g2.secret_id, None).consumer(g2.consumer.label())]))
        })?;
        Ok(grant)
    }

    fn revoke_inner(&self, grant_id: &str, actor: &Actor) -> Result<()> {
        let stamp = actor.stamp();
        let (grant, row) = {
            let keys = self.keys.read();
            let conn = self.conn.lock();
            let r = conn.query_row("SELECT * FROM grants WHERE id = ?1", params![grant_id], |r| Ok(read_grant_raw(r))).optional()?;
            let (g, mac) = r.ok_or_else(|| VaultError::not_found("no such grant"))??;
            if !hmac_verify(&keys.row, &grant_mac_input(&g), &mac) {
                return Err(VaultError::tamper("grant failed its integrity check"));
            }
            let row = self.find_row(&conn, &keys, &Selector::Id(g.secret_id.clone()))?;
            let f = self.facts_for(&conn, &keys, &row.meta, actor)?;
            let ok = g.granted_by == stamp || policy::can_manage(actor, &f).is_ok() || actor.is_admin_human();
            if !ok {
                drop(conn);
                drop(keys);
                return Err(self.deny(
                    Entry::new("revoke", &stamp, "deny").secret(&g.secret_id, None),
                    VaultError::forbidden("vault_not_yours", "only the owner or whoever shared it can remove that share"),
                ));
            }
            (g, row)
        };
        self.mutate(true, |tx, _| {
            tx.execute("DELETE FROM grants WHERE id = ?1", params![grant.id])?;
            Ok(((), vec![Entry::new("revoke", &stamp, "ok").secret(&row.meta.id, None).consumer(grant.consumer.label())]))
        })
    }

    pub fn grants(&self, sel: &Selector, actor: &Actor) -> Result<Vec<Grant>> {
        let keys = self.keys.read();
        let conn = self.conn.lock();
        let row = self.find_row(&conn, &keys, sel)?;
        let f = self.facts_for(&conn, &keys, &row.meta, actor)?;
        let granted = self.granted_to_actor(&conn, &keys, &row.meta, actor)?;
        if !policy::can_see(actor, &f, granted) {
            return Err(VaultError::not_found("no such secret"));
        }
        self.grants_for(&conn, &keys, &row.meta.id)
    }

    /// One grant by id, integrity-checked (subsystems: e.g. what a revoke
    /// is about to remove). `None` when missing or failing its MAC.
    pub fn grant_info(&self, grant_id: &str) -> Option<Grant> {
        let keys = self.keys.read();
        let conn = self.conn.lock();
        let r = conn.query_row("SELECT * FROM grants WHERE id = ?1", params![grant_id], |r| Ok(read_grant_raw(r))).optional().ok()??;
        let (g, mac) = r.ok()?;
        hmac_verify(&keys.row, &grant_mac_input(&g), &mac).then_some(g)
    }

    /// Every grant naming `consumer`, with the secret's name (people and
    /// subsystems; e.g. a published service's bindings).
    pub fn grants_to(&self, consumer: &Consumer) -> Result<Vec<(Grant, String)>> {
        let keys = self.keys.read();
        let conn = self.conn.lock();
        let mut st = conn.prepare("SELECT * FROM grants WHERE consumer_kind = ?1 AND consumer_id = ?2")?;
        let rows = st.query_map(params![consumer.kind.as_str(), consumer.id], |r| Ok(read_grant_raw(r)))?;
        let mut out = vec![];
        for r in rows {
            let Some((g, mac)) = tolerate(r)? else { continue };
            if !hmac_verify(&keys.row, &grant_mac_input(&g), &mac) {
                continue;
            }
            let name = self.find_row(&conn, &keys, &Selector::Id(g.secret_id.clone())).map(|r| r.meta.name).unwrap_or_default();
            out.push((g, name));
        }
        Ok(out)
    }

    // ── resolve ─────────────────────────────────────────────────────

    /// Effective deliveries for a requester: (delivery, env, file, hosts).
    #[allow(clippy::type_complexity)]
    fn effective(&self, conn: &Connection, keys: &Keys, m: &SecretMeta, who: &Requester) -> Result<Vec<(Delivery, Option<String>, Option<String>, Option<Vec<String>>, bool)>> {
        let mut out = vec![];
        // Implicit grants: a workspace's own secrets; a chat's own secrets.
        if let Requester::Session { workspace_id, session_id } = who {
            let implicit = match m.scope.kind {
                ScopeKind::Workspace => &m.scope.id == workspace_id,
                ScopeKind::Session => session_id.as_deref() == Some(m.scope.id.as_str()),
                _ => false,
            };
            if implicit {
                out.push((m.default_delivery, None, None, None, false));
            }
        }
        let t = now();
        for g in self.grants_for(conn, keys, &m.id)? {
            if let Some(x) = g.expires_at.as_deref().and_then(parse_ts) {
                if x < t {
                    continue;
                }
            }
            let matches = match who {
                Requester::Session { workspace_id, session_id } => match g.consumer.kind {
                    ConsumerKind::Workspace => &g.consumer.id == workspace_id || g.consumer.id == "*",
                    ConsumerKind::Session => session_id.as_deref() == Some(g.consumer.id.as_str()),
                    _ => false,
                },
                Requester::PublishedService { id, .. } => g.consumer.kind == ConsumerKind::PublishedService && &g.consumer.id == id,
                Requester::Engine(e) => g.consumer.kind == ConsumerKind::Engine && &g.consumer.id == e,
                Requester::System(_) => false,
                Requester::Source { id } => {
                    g.consumer.kind == ConsumerKind::Daemon && g.consumer.id.strip_prefix("source:") == Some(id.as_str())
                }
                Requester::Job { workspace_id, job_id, .. } => {
                    g.consumer.kind == ConsumerKind::Job && g.consumer.id == format!("{workspace_id}/{job_id}")
                }
                // SE10/Q2: the named node, or `*` only on a node this
                // server owns. Never an implicit workspace grant.
                Requester::Compute { workspace_id, node_id, owned_node, .. } => match g.consumer.compute_parts() {
                    Some((ws, node)) => ws == workspace_id && (node == node_id || (node == "*" && *owned_node)),
                    None => false,
                },
            };
            if matches {
                out.push((g.delivery, g.env_name.clone(), g.file_name.clone(), g.hosts_narrowed.clone(), true));
            }
        }
        Ok(out)
    }

    fn resolve_inner(&self, sel: &Selector, use_: Use<'_>, who: &Requester) -> Result<Resolved> {
        self.flush_rollups_if_due();
        let actor_label = who.label();
        let keys = self.keys.read();
        let conn = self.conn.lock();
        let row = match self.find_row(&conn, &keys, sel) {
            Ok(r) => r,
            Err(e) => {
                drop(conn);
                drop(keys);
                return Err(self.deny(Entry::new(use_.action(), &actor_label, "deny"), e));
            }
        };
        let m = row.meta.clone();
        let deny_entry = Entry::new(use_.action(), &actor_label, "deny").secret(&m.id, None).consumer(actor_label.clone());
        let fail = |e: VaultError| -> Result<Resolved> { Err(e) };
        if m.status != SecretStatus::Active {
            drop(conn);
            drop(keys);
            return Err(self.deny(deny_entry, VaultError::forbidden("secret_not_active", "that secret is disabled")));
        }
        if let Some(x) = m.expires_at.as_deref().and_then(parse_ts) {
            if x < now() {
                drop(conn);
                drop(keys);
                return Err(self.deny(deny_entry, VaultError::forbidden("secret_expired", "that secret has expired")));
            }
        }
        // Item 3: an ssh private key never goes into an outbound request.
        if m.kind == Kind::SshKey && matches!(use_, Use::Fetch(_) | Use::Proxy(_)) {
            drop(conn);
            drop(keys);
            return Err(self.deny(
                deny_entry,
                VaultError::forbidden("ssh_key_not_sent", "an ssh key is never put in a request; use agent (or file) delivery"),
            ));
        }
        // System namespace: daemon subsystems and engines only.
        if m.namespace == Namespace::System {
            let ok = match who {
                Requester::System(_) => true,
                Requester::Engine(e) => m.scope.kind == ScopeKind::Engine && &m.scope.id == e,
                _ => false,
            };
            if !ok || !matches!(use_, Use::System | Use::File | Use::Env) {
                drop(conn);
                drop(keys);
                return Err(self.deny(deny_entry, VaultError::forbidden("vault_system_namespace", "K2's own secrets are used only by K2")));
            }
            let v = self.decrypt_version(&conn, &keys, &m, m.latest_version)?;
            drop(conn);
            drop(keys);
            self.rollups.lock().add(&Entry::new("resolve", &actor_label, "ok").secret(&m.id, Some(m.latest_version)));
            return Ok(Resolved {
                value: v,
                version: m.latest_version,
                delivery: use_.needs(),
                env_name: None,
                file_name: None,
                inject: m.inject.clone().unwrap_or_default(),
                meta: m,
            });
        }
        if matches!(use_, Use::System) || matches!(who, Requester::System(_) | Requester::Engine(_)) {
            if !matches!(who, Requester::System(_)) || !matches!(use_, Use::System) {
                drop(conn);
                drop(keys);
                return Err(self.deny(deny_entry, VaultError::forbidden("bad_use", "that use isn't allowed here")));
            }
            // A daemon subsystem resolving a user secret on the owner's
            // behalf (e.g. the microVM key for a workspace): allowed.
            let v = self.decrypt_version(&conn, &keys, &m, m.latest_version)?;
            drop(conn);
            drop(keys);
            self.rollups.lock().add(&Entry::new("resolve", &actor_label, "ok").secret(&m.id, Some(m.latest_version)));
            return Ok(Resolved {
                value: v,
                version: m.latest_version,
                delivery: Delivery::None,
                env_name: None,
                file_name: None,
                inject: m.inject.clone().unwrap_or_default(),
                meta: m,
            });
        }
        let rollback = *self.rollback.lock();
        let need = use_.needs();
        let candidates = self.effective(&conn, &keys, &m, who)?;
        // A workspace's own secret gets an implicit, unnamed candidate
        // first; an eligible explicit grant that names an env or file must
        // still win, or its name is never used.
        let mut chosen: Option<(Delivery, Option<String>, Option<String>)> = None;
        for (d, env, file, narrowed, explicit) in candidates {
            if explicit && rollback {
                continue; // VS8: explicit grants stop while rolled back.
            }
            if d.level() < need.level() {
                continue;
            }
            if let Use::Fetch(o) | Use::Proxy(o) = use_ {
                let pats: Vec<OriginPattern> = match &narrowed {
                    Some(n) => hosts::parse_stored(n),
                    None => hosts::parse_stored(&m.hosts),
                };
                if crate::fetch::check_host(o, &pats).is_err() {
                    continue;
                }
            }
            let named = explicit && (env.is_some() || file.is_some());
            if chosen.is_none() || named {
                chosen = Some((d, env, file));
            }
            if named {
                break;
            }
        }
        let (d, env, file) = match chosen {
            Some(c) => c,
            None => {
                let e = match use_ {
                    Use::Fetch(o) | Use::Proxy(o) => {
                        let pats = hosts::parse_stored(&m.hosts);
                        match crate::fetch::check_host(o, &pats) {
                            Err(e) => e,
                            Ok(()) => VaultError::forbidden("not_granted", "this workspace can't use that secret"),
                        }
                    }
                    _ => VaultError::forbidden(
                        "delivery_not_granted",
                        format!("this secret isn't delivered to you as {}", need),
                    ),
                };
                drop(conn);
                drop(keys);
                let mut de = deny_entry;
                if let Use::Fetch(o) | Use::Proxy(o) = use_ {
                    de = de.host(o.render());
                }
                return Err(self.deny(de, e));
            }
        };
        let v = match self.decrypt_version(&conn, &keys, &m, m.latest_version) {
            Ok(v) => v,
            Err(e) => return fail(e),
        };
        drop(conn);
        drop(keys);
        // Item 2: a signed fetch says which scheme signed it.
        let outcome = match (&use_, m.inject.as_ref().and_then(|i| i.sign.as_ref())) {
            (Use::Fetch(_), Some(s)) => format!("ok:sign={}", s.scheme()),
            _ => "ok".to_string(),
        };
        // A compute release names the job and attempt (SE10).
        let consumer_label = match who {
            Requester::Compute { job_id, generation, .. } => format!("{actor_label} job {job_id} gen {generation}"),
            _ => actor_label.clone(),
        };
        let mut e = Entry::new(use_.action(), &actor_label, &outcome).secret(&m.id, Some(m.latest_version)).consumer(consumer_label);
        if let Use::Fetch(o) | Use::Proxy(o) = use_ {
            e = e.host(o.render());
        }
        match use_ {
            // A value leaving this machine for a node: one row per release,
            // written now, never rolled up (SE10). No row, no release.
            _ if matches!(who, Requester::Compute { .. }) => self.mutate(false, |_, _| Ok(((), vec![e])))?,
            Use::Fetch(_) | Use::Proxy(_) | Use::File | Use::Env => self.rollups.lock().add(&e),
            // Item 3: `agent_sign` audits once the signature exists.
            Use::Agent(_) => {}
            _ => self.audit_only(vec![e]),
        }
        Ok(Resolved {
            value: v,
            version: m.latest_version,
            delivery: d,
            env_name: env.or_else(|| if d == Delivery::Env { Some(crate::delivery::default_env_name(&m.name)) } else { None }),
            file_name: file.or_else(|| if d.level() >= Delivery::File.level() { Some(m.name.clone()) } else { None }),
            inject: m.inject.clone().unwrap_or_default(),
            meta: m,
        })
    }

    /// May `who` have `m` through a `k2v://` reference in its own config
    /// (prd-env-to-vault-v1 R8)? `Some(version)` when it may (`None` =
    /// latest, `Some(v)` = the version a grant pins), `None` when not.
    ///
    /// - Implicit: its own workspace's secret (a session, a published
    ///   service) or its own chat's secret (a session), and only when the
    ///   secret's **default delivery** is env or above, the same cap the
    ///   implicit grant has in [`Self::effective`]. A fetch- or proxy-only
    ///   key never becomes raw env through a reference (review fix 1).
    /// - Explicit: an unexpired grant at env delivery or above, to the
    ///   session or its workspace (`ws:<id>`, `ws:*`); to the service, or
    ///   to the service's workspace (`ws:<id>`, `ws:*`); to the job.
    ///   Explicit grants stop while the vault is rolled back (VS8). An
    ///   unpinned grant wins over a pinned one.
    ///
    /// A workspace grant counts for a service only here, when the service
    /// names the secret itself: [`Self::spawn_items`] still delivers a
    /// service only the grants made to that service.
    fn ref_entitled(&self, conn: &Connection, keys: &Keys, m: &SecretMeta, who: &Requester) -> Result<Option<Option<u32>>> {
        let own = match who {
            Requester::Session { workspace_id, session_id } => match m.scope.kind {
                ScopeKind::Workspace => &m.scope.id == workspace_id,
                ScopeKind::Session => session_id.as_deref() == Some(m.scope.id.as_str()),
                _ => false,
            },
            Requester::PublishedService { workspace_id, .. } => m.scope.kind == ScopeKind::Workspace && &m.scope.id == workspace_id,
            // A job gets nothing by default (Q13); the rest never name refs.
            _ => false,
        };
        if own && m.default_delivery.level() >= Delivery::Env.level() {
            return Ok(Some(None));
        }
        if *self.rollback.lock() {
            return Ok(None);
        }
        let t = now();
        let mut pinned: Option<u32> = None;
        for g in self.grants_for(conn, keys, &m.id)? {
            if g.expires_at.as_deref().and_then(parse_ts).is_some_and(|x| x < t) {
                continue;
            }
            if g.delivery.level() < Delivery::Env.level() {
                continue;
            }
            let ws_match = |ws: &str| g.consumer.kind == ConsumerKind::Workspace && (g.consumer.id == ws || g.consumer.id == "*");
            let ok = match who {
                Requester::Session { workspace_id, session_id } => {
                    ws_match(workspace_id)
                        || (g.consumer.kind == ConsumerKind::Session && session_id.as_deref() == Some(g.consumer.id.as_str()))
                }
                Requester::PublishedService { id, workspace_id } => {
                    (g.consumer.kind == ConsumerKind::PublishedService && &g.consumer.id == id) || ws_match(workspace_id)
                }
                Requester::Job { workspace_id, job_id, .. } => {
                    g.consumer.kind == ConsumerKind::Job && g.consumer.id == format!("{workspace_id}/{job_id}")
                }
                _ => false,
            };
            if !ok {
                continue;
            }
            match g.version_selector.parse::<u32>() {
                Ok(v) => pinned = Some(pinned.map_or(v, |p| p.max(v))),
                Err(_) => return Ok(Some(None)),
            }
        }
        Ok(pinned.map(Some))
    }

    /// Write one daemon note into the vault's own audit chain (MAC'd,
    /// append-only): `action` by `actor`, about `consumer`. Names only.
    /// The R8 upgrade keeps its state here (done flag, first pass, the
    /// pairs it decided), not in a file an agent could delete.
    pub fn audit_note(&self, action: &str, actor: &Actor, consumer: &str) -> Result<()> {
        let e = Entry::new(action, &actor.stamp(), "ok").consumer(consumer.to_string());
        self.mutate(false, |_, _| Ok(((), vec![e])))
    }

    /// Check every audit row's MAC and the chain. Callers that make
    /// decisions from audit rows (the R8 upgrade) call this first, so a row
    /// written straight into `vault.db` without the audit key is refused.
    pub fn verify_audit_chain(&self) -> Result<()> {
        let keys = self.keys.read();
        let conn = self.conn.lock();
        verify_chain(&conn, &keys.audit)
    }

    /// Is there an `ok` audit row with exactly this `action` and `actor`
    /// stamp? Verifies the chain first.
    pub fn audit_has(&self, action: &str, actor: &Actor) -> Result<bool> {
        Ok(self.audit_first_ts(action, actor)?.is_some())
    }

    /// The earliest `ok` row's time for `action` by `actor` (chain verified).
    pub fn audit_first_ts(&self, action: &str, actor: &Actor) -> Result<Option<String>> {
        self.verify_audit_chain()?;
        let conn = self.conn.lock();
        let ts: Option<String> = conn
            .query_row(
                "SELECT ts FROM audit WHERE action = ?1 AND actor = ?2 AND outcome = 'ok' ORDER BY seq ASC LIMIT 1",
                params![action, actor.stamp()],
                |r| r.get(0),
            )
            .optional()?;
        Ok(ts)
    }

    /// Every `consumer` of `ok` rows for `action` by `actor` (chain
    /// verified). No row cap.
    pub fn audit_consumers(&self, action: &str, actor: &Actor) -> Result<Vec<String>> {
        self.verify_audit_chain()?;
        let conn = self.conn.lock();
        let mut st = conn.prepare("SELECT consumer FROM audit WHERE action = ?1 AND actor = ?2 AND outcome = 'ok' AND consumer IS NOT NULL ORDER BY seq")?;
        let rows = st.query_map(params![action, actor.stamp()], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Is there a row for `secret_id` with this `action`, raw `actor`
    /// label (e.g. `daemon:publish`), an outcome starting `ok`, at or
    /// after `since` and (when given) before `until`, and (when given) this
    /// `consumer`? One query by secret, no row cap. Call
    /// [`Self::verify_audit_chain`] first when the answer grants anything.
    pub fn audit_seen(
        &self,
        secret_id: &str,
        action: &str,
        actor_label: &str,
        consumer: Option<&str>,
        since: Option<&str>,
        until: Option<&str>,
    ) -> Result<bool> {
        let conn = self.conn.lock();
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM audit WHERE secret_id = ?1 AND action = ?2 AND actor = ?3 AND outcome LIKE 'ok%' \
             AND (?4 IS NULL OR consumer = ?4) AND (?5 IS NULL OR ts >= ?5) AND (?6 IS NULL OR ts < ?6)",
            params![secret_id, action, actor_label, consumer, since, until],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    /// Would [`Self::resolve_ref_env`] give `who` this secret? Names only,
    /// no value read, no audit row (the upgrade pass and the owner notice).
    pub fn ref_would_resolve(&self, sel: &Selector, who: &Requester) -> Result<bool> {
        let keys = self.keys.read();
        let conn = self.conn.lock();
        let row = self.find_row(&conn, &keys, sel)?;
        let m = &row.meta;
        if m.namespace != Namespace::User || m.status != SecretStatus::Active {
            return Ok(false);
        }
        if m.expires_at.as_deref().and_then(parse_ts).is_some_and(|x| x < now()) {
            return Ok(false);
        }
        if m.delivery_ceiling.level() < Delivery::Env.level() {
            return Ok(false);
        }
        Ok(self.ref_entitled(&conn, &keys, m, who)?.is_some())
    }

    /// Resolve a `k2v://` reference that a consumer's own config names (a
    /// published service's `.env.k2`, an agent's launch env) into env for
    /// `who`, only when `who` may have it ([`Self::ref_entitled`]) and the
    /// secret's ceiling allows env (prd-env-to-vault-v1 R8). A server
    /// secret needs a grant like any other shared secret. Audited under
    /// `who`'s own label, allowed or refused.
    pub fn resolve_ref_env(&self, sel: &Selector, who: &Requester) -> Result<Resolved> {
        self.flush_rollups_if_due();
        let actor_label = who.label();
        let keys = self.keys.read();
        let conn = self.conn.lock();
        let row = match self.find_row(&conn, &keys, sel) {
            Ok(r) => r,
            Err(e) => {
                drop(conn);
                drop(keys);
                return Err(self.deny(Entry::new("resolve_ref", &actor_label, "deny"), e));
            }
        };
        let m = row.meta.clone();
        let deny_entry = Entry::new("resolve_ref", &actor_label, "deny").secret(&m.id, None).consumer(actor_label.clone());
        let entitled = if m.namespace == Namespace::User { self.ref_entitled(&conn, &keys, &m, who)? } else { None };
        let refusal = if m.namespace != Namespace::User {
            Some(VaultError::forbidden("vault_system_namespace", "K2's own secrets are used only by K2"))
        } else if m.status != SecretStatus::Active {
            Some(VaultError::forbidden("secret_not_active", "that secret is disabled"))
        } else if m.expires_at.as_deref().and_then(parse_ts).is_some_and(|x| x < now()) {
            Some(VaultError::forbidden("secret_expired", "that secret has expired"))
        } else if m.delivery_ceiling.level() < Delivery::Env.level() {
            Some(VaultError::forbidden("delivery_above_ceiling", "this secret can't be delivered as env"))
        } else if entitled.is_none() {
            Some(VaultError::forbidden(
                "not_granted",
                format!(
                    "{} isn't shared with {actor_label}; the owner can share it: k2 vault grant {} --to <service:NAME | ws:NAME> --as env",
                    m.name, m.name
                ),
            ))
        } else {
            None
        };
        if let Some(e) = refusal {
            drop(conn);
            drop(keys);
            return Err(self.deny(deny_entry, e));
        }
        // Review fix 9: a version a grant pins is the version delivered.
        let version = entitled.flatten().unwrap_or(m.latest_version);
        let v = match self.decrypt_version(&conn, &keys, &m, version) {
            Ok(v) => v,
            Err(e) => {
                drop(conn);
                drop(keys);
                return Err(self.deny(deny_entry, e));
            }
        };
        drop(conn);
        drop(keys);
        self.rollups
            .lock()
            .add(&Entry::new("resolve_ref", &actor_label, "ok").secret(&m.id, Some(version)).consumer(actor_label.clone()));
        Ok(Resolved {
            value: v,
            version,
            delivery: Delivery::Env,
            env_name: None,
            file_name: None,
            inject: m.inject.clone().unwrap_or_default(),
            meta: m,
        })
    }

    /// Every env/file/agent delivery a session (or service) gets at spawn.
    /// `agent` items (ssh keys, item 3) are served on the session's
    /// ssh-agent socket; a published service skips them.
    pub fn spawn_items(&self, who: &Requester) -> Result<Vec<SpawnItem>> {
        let keys = self.keys.read();
        let conn = self.conn.lock();
        let rows: Vec<(SecretRow, Vec<u8>)> = {
            let mut st = conn.prepare("SELECT * FROM secret WHERE namespace = 'user' AND status = 'active'")?;
            let it = st.query_map([], |r| Ok(read_secret_raw(r)))?;
            let mut v = vec![];
            for r in it {
                v.extend(tolerate(r)?);
            }
            v
        };
        let rollback = *self.rollback.lock();
        let t = now();
        let mut out = vec![];
        for (row, mac) in rows {
            if !row_mac_ok(&keys.row, &row, &mac) {
                continue;
            }
            let m = &row.meta;
            if m.expires_at.as_deref().and_then(parse_ts).is_some_and(|x| x < t) {
                continue;
            }
            let mut best: Option<(Delivery, Option<String>, Option<String>, bool)> = None;
            for (d, env, file, _, explicit) in self.effective(&conn, &keys, m, who)? {
                if explicit && rollback {
                    continue;
                }
                if !matches!(d, Delivery::Env | Delivery::File | Delivery::Agent) {
                    continue;
                }
                // Highest delivery wins; on a tie, a grant that names the
                // env or file beats the implicit unnamed one.
                let named = explicit && (env.is_some() || file.is_some());
                let better = match &best {
                    None => true,
                    Some(b) => d.level() > b.0.level() || (d.level() == b.0.level() && named && !b.3),
                };
                if better {
                    best = Some((d, env, file, named));
                }
            }
            if let Some((d, env, file, _)) = best {
                let env_name = if d == Delivery::Env {
                    let n = env.unwrap_or_else(|| crate::delivery::default_env_name(&m.name));
                    if crate::delivery::validate_env_name(&n).is_err() {
                        continue;
                    }
                    Some(n)
                } else {
                    None
                };
                let file_name = file.unwrap_or_else(|| m.name.clone());
                if crate::delivery::validate_file_name(&file_name).is_err() {
                    continue;
                }
                out.push(SpawnItem {
                    secret_id: m.id.clone(),
                    name: m.name.clone(),
                    delivery: d,
                    env_name,
                    file_name: Some(file_name),
                });
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    /// Every proxy delivery a session gets at spawn (§9.3): secrets whose
    /// delivery for this requester is exactly `proxy` (implicit or
    /// granted). Fetch-only secrets don't go on the proxy.
    pub fn proxy_items(&self, who: &Requester) -> Result<Vec<ProxyItem>> {
        let keys = self.keys.read();
        let conn = self.conn.lock();
        let rows: Vec<(SecretRow, Vec<u8>)> = {
            let mut st = conn.prepare("SELECT * FROM secret WHERE namespace = 'user' AND status = 'active'")?;
            let it = st.query_map([], |r| Ok(read_secret_raw(r)))?;
            let mut v = vec![];
            for r in it {
                v.extend(tolerate(r)?);
            }
            v
        };
        let rollback = *self.rollback.lock();
        let t = now();
        let mut out = vec![];
        for (row, mac) in rows {
            if !row_mac_ok(&keys.row, &row, &mac) {
                continue;
            }
            let m = &row.meta;
            if m.expires_at.as_deref().and_then(parse_ts).is_some_and(|x| x < t) {
                continue;
            }
            for (d, env, _, narrowed, explicit) in self.effective(&conn, &keys, m, who)? {
                if (explicit && rollback) || d != Delivery::Proxy {
                    continue;
                }
                out.push(ProxyItem {
                    secret_id: m.id.clone(),
                    name: m.name.clone(),
                    hosts: narrowed.unwrap_or_else(|| m.hosts.clone()),
                    env_name: env,
                });
                break;
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    // ── list / meta / audit ─────────────────────────────────────────

    fn list_inner(&self, filter: &ListFilter, actor: &Actor) -> Result<Vec<SecretMeta>> {
        let keys = self.keys.read();
        let conn = self.conn.lock();
        let mut st = conn.prepare("SELECT * FROM secret ORDER BY namespace, scope_kind, scope_id, name")?;
        let it = st.query_map([], |r| Ok(read_secret_raw(r)))?;
        let mut out = vec![];
        for r in it {
            let Some((row, mac)) = tolerate(r)? else { continue };
            if !row_mac_ok(&keys.row, &row, &mac) {
                continue;
            }
            let m = row.meta;
            if m.namespace == Namespace::System && !filter.include_system {
                continue;
            }
            if m.status == SecretStatus::Destroyed && !filter.include_destroyed {
                continue;
            }
            if let Some(s) = &filter.scope {
                if &m.scope != s {
                    continue;
                }
            }
            let f = self.facts_for(&conn, &keys, &m, actor)?;
            let granted = self.granted_to_actor(&conn, &keys, &m, actor)?;
            if policy::can_see(actor, &f, granted) {
                out.push(m);
            }
        }
        Ok(out)
    }

    fn meta_inner(&self, sel: &Selector, actor: &Actor) -> Result<SecretMeta> {
        let keys = self.keys.read();
        let conn = self.conn.lock();
        let row = self.find_row(&conn, &keys, sel)?;
        let f = self.facts_for(&conn, &keys, &row.meta, actor)?;
        let granted = self.granted_to_actor(&conn, &keys, &row.meta, actor)?;
        if !policy::can_see(actor, &f, granted) {
            return Err(VaultError::not_found("no such secret"));
        }
        Ok(row.meta)
    }

    fn audit_inner(&self, q: &AuditQuery, actor: &Actor) -> Result<Vec<AuditRow>> {
        let agent_ws = actor.agent().map(|a| a.workspace_id.clone());
        if agent_ws.is_none() && !actor.is_admin_human() && !actor.is_system() {
            return Err(VaultError::forbidden("vault_admin_required", "the vault audit needs an Owner or Admin"));
        }
        let ws_filter = agent_ws.or_else(|| q.workspace_id.clone());
        let conn = self.conn.lock();
        let mut sql = String::from(
            "SELECT a.seq, a.ts, a.secret_id, a.version, a.action, a.actor, a.consumer, a.target_host, a.outcome, a.count FROM audit a",
        );
        let mut conds: Vec<String> = vec![];
        let mut args: Vec<String> = vec![];
        if ws_filter.is_some() {
            sql.push_str(" JOIN secret s ON s.id = a.secret_id");
            conds.push("s.namespace = 'user' AND s.scope_kind = 'workspace' AND s.scope_id = ?".into());
            args.push(ws_filter.clone().unwrap_or_default());
        }
        if let Some(id) = &q.secret_id {
            conds.push("a.secret_id = ?".into());
            args.push(id.clone());
        }
        if let Some(s) = &q.since {
            conds.push("a.ts >= ?".into());
            args.push(s.clone());
        }
        if !conds.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&conds.join(" AND "));
        }
        sql.push_str(" ORDER BY a.seq DESC LIMIT ?");
        args.push(q.limit.unwrap_or(200).min(5000).to_string());
        let mut st = conn.prepare(&sql)?;
        let it = st.query_map(rusqlite::params_from_iter(args.iter()), |r| {
            Ok(AuditRow {
                seq: r.get::<_, i64>(0)? as u64,
                ts: r.get(1)?,
                secret_id: r.get(2)?,
                version: r.get(3)?,
                action: r.get(4)?,
                actor: r.get(5)?,
                consumer: r.get(6)?,
                target_host: r.get(7)?,
                outcome: r.get(8)?,
                count: r.get::<_, i64>(9)? as u64,
            })
        })?;
        let rows: Vec<AuditRow> = it.collect::<std::result::Result<_, _>>()?;
        Ok(rows)
    }

    // ── reveal / confirm password ───────────────────────────────────

    fn reveal_inner(&self, sel: &Selector, actor: &Actor, proof: &HumanProof) -> Result<SecretBytes> {
        let stamp = actor.stamp();
        let keys = self.keys.read();
        let conn = self.conn.lock();
        let row = self.find_row(&conn, &keys, sel)?;
        if let Err(e) = policy::can_reveal(actor, row.meta.namespace, proof) {
            drop(conn);
            drop(keys);
            return Err(self.deny(Entry::new("reveal", &stamp, "deny").secret(&row.meta.id, None), e));
        }
        if row.meta.status == SecretStatus::Destroyed {
            return Err(VaultError::conflict("secret_destroyed", "that secret was destroyed"));
        }
        let v = self.decrypt_version(&conn, &keys, &row.meta, row.meta.latest_version)?;
        drop(conn);
        drop(keys);
        self.audit_only(vec![Entry::new("reveal", &stamp, "ok").secret(&row.meta.id, Some(row.meta.latest_version))]);
        Ok(v)
    }

    pub fn has_confirm_password(&self) -> bool {
        let conn = self.conn.lock();
        load_header(&conn).ok().flatten().is_some_and(|(h, _)| h.confirm_hash.is_some())
    }

    /// Set or change the vault confirm password. The daemon allows this
    /// only from a person's channel (VS2): Connect login, a TTY outside the
    /// daemon's agents, or the desktop with a confirm.
    pub fn set_confirm_password(&self, new_pw: &str, actor: &Actor) -> Result<()> {
        if !actor.is_admin_human() {
            return Err(VaultError::forbidden("vault_admin_required", "the vault password is set by an Owner or Admin"));
        }
        let phc = confirm::hash_password(new_pw, self.kdf)?;
        let stamp = actor.stamp();
        self.mutate(true, |tx, keys| {
            let mut h = load_header(tx)?.ok_or_else(|| VaultError::internal("header"))?.0;
            h.confirm_hash = Some(phc);
            write_header(tx, &h, &keys.header)?;
            Ok(((), vec![Entry::new("confirm-password", &stamp, "ok")]))
        })
    }

    /// Check the vault confirm password (lockout applies, VS16).
    pub fn check_confirm_password(&self, pw: &str, actor_stamp: &str) -> Result<()> {
        self.throttle.check()?;
        let phc = {
            let conn = self.conn.lock();
            load_header(&conn)?.and_then(|(h, _)| h.confirm_hash)
        };
        let phc = phc.ok_or_else(|| VaultError::conflict("no_confirm_password", "no vault password is set yet"))?;
        let ok = confirm::verify_password(pw, &phc);
        let locked = self.throttle.record(ok);
        if !ok {
            self.audit_only(vec![Entry::new("confirm", actor_stamp, if locked { "deny:locked" } else { "deny:wrong-password" })]);
            return Err(VaultError::forbidden("confirm_failed", "that isn't the vault password"));
        }
        Ok(())
    }

    // ── recovery key (§7.4, VS2) ────────────────────────────────────

    pub fn recovery_state(&self) -> (String, Option<String>, Option<String>) {
        let conn = self.conn.lock();
        match load_header(&conn) {
            Ok(Some((h, _))) => {
                let r = h.wraps.iter().find(|w| w.holder == "recovery").map(|w| w.recipient.clone());
                (h.recovery_state, h.recovery_confirmed_at, r)
            }
            _ => ("unknown".into(), None, None),
        }
    }

    /// Step 1: make a new phrase and hold it (not yet stored). The daemon
    /// calls this only for a person's channel (VS2).
    pub fn recovery_begin(&self, actor: &Actor) -> Result<RecoveryChallenge> {
        if !actor.is_admin_human() {
            return Err(VaultError::forbidden("vault_admin_required", "recovery keys are for the owner"));
        }
        let seed = RecoverySeed::generate();
        let phrase = seed.phrase();
        let recipient = seed.identity().recipient().encode();
        let id = crypto::base62(16);
        let mut g = self.recovery_pending.lock();
        g.retain(|_, p| p.started.elapsed().as_secs() < 900);
        g.insert(id.clone(), PendingRecovery { seed, actor: actor.stamp(), started: Instant::now() });
        Ok(RecoveryChallenge { id, phrase, check_words: CHECK_WORDS, recipient })
    }

    /// Step 2: the person types back words 3, 11 and 20. On success the
    /// VDK is wrapped to the new recovery key and any older one dropped.
    pub fn recovery_confirm(&self, id: &str, words: &[String], actor: &Actor) -> Result<String> {
        let stamp = actor.stamp();
        let pending = self.recovery_pending.lock().remove(id);
        let p = pending.ok_or_else(|| VaultError::conflict("recovery_expired", "start again: run show-once"))?;
        if p.actor != stamp {
            return Err(VaultError::forbidden("recovery_wrong_person", "finish on the same login that started"));
        }
        if words.len() != CHECK_WORDS.len() {
            return Err(VaultError::bad_request("recovery_words", "type back words 3, 11 and 20"));
        }
        for (i, n) in CHECK_WORDS.iter().enumerate() {
            let want = p.seed.word(*n).unwrap_or_default();
            if words[i].trim().to_lowercase() != want {
                self.audit_only(vec![Entry::new("recovery", &stamp, "deny:words")]);
                return Err(VaultError::bad_request("recovery_words", "those words don't match; start again with show-once"));
            }
        }
        let recipient = p.seed.identity().recipient();
        let r_str = recipient.encode();
        self.mutate(true, |tx, keys| {
            let mut h = load_header(tx)?.ok_or_else(|| VaultError::internal("header"))?.0;
            let wrapped = agefmt::encrypt(&[recipient], keys.vdk.as_bytes());
            h.wraps.retain(|w| w.holder != "recovery");
            h.wraps.push(Wrap {
                holder: "recovery".into(),
                recipient: r_str.clone(),
                wrapped: base64_encode(&wrapped),
                created_at: ts(now()),
            });
            h.recovery_state = "confirmed".into();
            h.recovery_confirmed_at = Some(ts(now()));
            write_header(tx, &h, &keys.header)?;
            Ok(((), vec![Entry::new("recovery", &stamp, "ok")]))
        })?;
        Ok(r_str)
    }

    /// Add or replace an age holder (a backup key). It changes who can open
    /// the vault, so it needs a person's confirm (VU-B1, 0.46.6); the
    /// recovery key has its own steps. The audit rows name the actor and
    /// the fingerprints, never key material. Returns the replaced key's
    /// fingerprint.
    pub fn add_wrap(&self, holder: &str, recipient: &Recipient, actor: &Actor, proof: &HumanProof) -> Result<Option<String>> {
        holder_change_allowed(holder, actor, proof)?;
        let stamp = actor.stamp();
        let r = *recipient;
        let fp = r.fingerprint();
        let holder = holder.to_string();
        self.mutate(true, |tx, keys| {
            let mut h = load_header(tx)?.ok_or_else(|| VaultError::internal("header"))?.0;
            let old = h.wraps.iter().find(|w| w.holder == holder).map(|w| wrap_fingerprint(&w.recipient));
            let wrapped = agefmt::encrypt(&[r], keys.vdk.as_bytes());
            h.wraps.retain(|w| w.holder != holder);
            h.wraps.push(Wrap { holder: holder.clone(), recipient: r.encode(), wrapped: base64_encode(&wrapped), created_at: ts(now()) });
            write_header(tx, &h, &keys.header)?;
            let mut rows = vec![];
            if let Some(o) = &old {
                rows.push(Entry::new("unwrap", &stamp, &format!("ok:{holder}")).consumer(format!("{HOLDER_CONFIRMED_PREFIX}{o}")));
            }
            rows.push(Entry::new("wrap", &stamp, &format!("ok:{holder}")).consumer(format!("{HOLDER_CONFIRMED_PREFIX}{fp}")));
            Ok((old, rows))
        })
    }

    /// Remove an age holder (a backup key): a person's confirm, as for
    /// adding one. Returns the removed key's fingerprint.
    pub fn remove_wrap(&self, holder: &str, actor: &Actor, proof: &HumanProof) -> Result<String> {
        holder_change_allowed(holder, actor, proof)?;
        let stamp = actor.stamp();
        let holder = holder.to_string();
        self.mutate(true, |tx, keys| {
            let mut h = load_header(tx)?.ok_or_else(|| VaultError::internal("header"))?.0;
            let old = h
                .wraps
                .iter()
                .find(|w| w.holder == holder)
                .map(|w| wrap_fingerprint(&w.recipient))
                .ok_or_else(|| VaultError::not_found(format!("there's no {holder} key")))?;
            h.wraps.retain(|w| w.holder != holder);
            write_header(tx, &h, &keys.header)?;
            let row = Entry::new("unwrap", &stamp, &format!("ok:{holder}")).consumer(format!("{HOLDER_CONFIRMED_PREFIX}{old}"));
            Ok((old, vec![row]))
        })
    }

    /// Who can open the vault now (the same list status shows).
    pub fn holders(&self) -> Vec<Holder> {
        let conn = self.conn.lock();
        match load_header(&conn) {
            Ok(Some((h, _))) => holders_of(&h.wraps),
            _ => holders_of(&[]),
        }
    }

    /// Age holders other than the recovery key with no person-confirmed
    /// `wrap` row in the audit chain for their fingerprint: added before
    /// 0.46.6 with the owner token alone, or the chain doesn't verify (then
    /// every one is listed: fail closed). Never removes anything.
    pub fn unconfirmed_holders(&self) -> Result<Vec<UnconfirmedHolder>> {
        let wraps = {
            let conn = self.conn.lock();
            load_header(&conn)?.map(|(h, _)| h.wraps).unwrap_or_default()
        };
        let trusted = self.verify_audit_chain().is_ok();
        let conn = self.conn.lock();
        let mut out = vec![];
        for w in wraps.iter().filter(|w| w.holder != "recovery") {
            let fp = wrap_fingerprint(&w.recipient);
            let outcome = format!("ok:{}", w.holder);
            let confirmed = trusted && {
                let n: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM audit WHERE action = 'wrap' AND outcome = ?1 AND consumer = ?2",
                    params![outcome, format!("{HOLDER_CONFIRMED_PREFIX}{fp}")],
                    |r| r.get(0),
                )?;
                n > 0
            };
            if confirmed {
                continue;
            }
            let added_by: Option<String> = conn
                .query_row(
                    "SELECT actor FROM audit WHERE action = 'wrap' AND outcome = ?1 ORDER BY seq DESC LIMIT 1",
                    params![outcome],
                    |r| r.get(0),
                )
                .optional()?;
            out.push(UnconfirmedHolder {
                holder: Holder { kind: w.holder.clone(), fingerprint: Some(fp), created_at: Some(w.created_at.clone()) },
                added_by,
            });
        }
        Ok(out)
    }

    /// Test seam: a holder written the way K2 before 0.46.6 did (no person
    /// confirm on its audit row), for the unconfirmed-holder check.
    #[cfg(any(test, feature = "test-util"))]
    pub fn add_wrap_unconfirmed_for_test(&self, holder: &str, recipient: &Recipient, actor: &Actor) -> Result<()> {
        let stamp = actor.stamp();
        let r = *recipient;
        let holder = holder.to_string();
        self.mutate(true, |tx, keys| {
            let mut h = load_header(tx)?.ok_or_else(|| VaultError::internal("header"))?.0;
            let wrapped = agefmt::encrypt(&[r], keys.vdk.as_bytes());
            h.wraps.retain(|w| w.holder != holder);
            h.wraps.push(Wrap { holder: holder.clone(), recipient: r.encode(), wrapped: base64_encode(&wrapped), created_at: ts(now()) });
            write_header(tx, &h, &keys.header)?;
            Ok(((), vec![Entry::new("wrap", &stamp, &format!("ok:{holder}"))]))
        })
    }

    /// The box-move export's own holder (prd-box-move-carry-v1): the data
    /// key wrapped to the new box's one-time move key, after the person's
    /// confirm (`proof`). `frozen_ok`: a second export replacing it on an
    /// already frozen vault. Audited like any confirmed holder change.
    pub(crate) fn put_move_wrap(&self, recipient: &Recipient, actor: &Actor, proof: &HumanProof, frozen_ok: bool) -> Result<()> {
        if !actor.is_admin_human() || !matches!(proof, HumanProof::Confirmed { .. }) {
            return Err(VaultError::forbidden("vault_confirm_required", "a box move export needs the owner's confirm"));
        }
        let stamp = actor.stamp();
        let r = *recipient;
        let fp = r.fingerprint();
        let holder = crate::boxmove::MOVE_HOLDER.to_string();
        self.mutate_inner(true, frozen_ok, |tx, keys| {
            let mut h = load_header(tx)?.ok_or_else(|| VaultError::internal("header"))?.0;
            let old = h.wraps.iter().find(|w| w.holder == holder).map(|w| wrap_fingerprint(&w.recipient));
            let wrapped = agefmt::encrypt(&[r], keys.vdk.as_bytes());
            h.wraps.retain(|w| w.holder != holder);
            h.wraps.push(Wrap { holder: holder.clone(), recipient: r.encode(), wrapped: base64_encode(&wrapped), created_at: ts(now()) });
            write_header(tx, &h, &keys.header)?;
            let mut rows = vec![];
            if let Some(o) = &old {
                rows.push(Entry::new("unwrap", &stamp, &format!("ok:{holder}")).consumer(format!("{HOLDER_CONFIRMED_PREFIX}{o}")));
            }
            rows.push(Entry::new("wrap", &stamp, &format!("ok:{holder}")).consumer(format!("{HOLDER_CONFIRMED_PREFIX}{fp}")));
            Ok(((), rows))
        })
    }

    /// Drop the box-move holder (`Ok(false)` when it wasn't there): after
    /// an import (the one-time key is gone) or an unfreeze (the move is off).
    pub(crate) fn drop_move_wrap(&self, stamp: &str) -> Result<bool> {
        let holder = crate::boxmove::MOVE_HOLDER;
        if !self.wrap_holders().iter().any(|h| h == holder) {
            return Ok(false);
        }
        let stamp = stamp.to_string();
        self.mutate(true, |tx, keys| {
            let mut h = load_header(tx)?.ok_or_else(|| VaultError::internal("header"))?.0;
            let old = h.wraps.iter().find(|w| w.holder == holder).map(|w| wrap_fingerprint(&w.recipient)).unwrap_or_default();
            h.wraps.retain(|w| w.holder != holder);
            write_header(tx, &h, &keys.header)?;
            Ok((true, vec![Entry::new("unwrap", &stamp, &format!("ok:{holder}")).consumer(format!("{HOLDER_CONFIRMED_PREFIX}{old}"))]))
        })
    }

    /// The age holders this vault has (`recovery`, `backup`, `move`, …).
    pub fn wrap_holders(&self) -> Vec<String> {
        let conn = self.conn.lock();
        load_header(&conn).ok().flatten().map(|(h, _)| h.wraps.into_iter().map(|w| w.holder).collect()).unwrap_or_default()
    }

    /// `(gen, audit_seq, audit_mac hex)` from this vault's header.
    pub(crate) fn head(&self) -> Result<(u64, u64, String)> {
        let conn = self.conn.lock();
        let (h, _) = load_header(&conn)?.ok_or_else(|| VaultError::internal("header missing"))?;
        Ok((h.gen, h.audit_seq, hex(&h.audit_mac)))
    }

    // ── KEK rotation, rekey, rollback acknowledge ───────────────────

    /// Rotate the box key: new KEK, re-wrap, then commit it in the store.
    pub fn kek_rotate(&self, actor: &Actor, proof: &HumanProof) -> Result<()> {
        if !actor.is_admin_human() || !matches!(proof, HumanProof::Confirmed { .. }) {
            return Err(VaultError::forbidden("vault_confirm_required", "rotating the box key needs the owner's confirm"));
        }
        if self.is_frozen() {
            return Err(crate::boxmove::moved_out());
        }
        let next = self.ks.stage_next()?;
        let res = self.rewrap_box(&next, &actor.stamp(), "kek-rotate");
        match res {
            Ok(()) => self.ks.commit_next(),
            Err(e) => {
                let _ = self.ks.abort_next();
                Err(e)
            }
        }
    }

    /// Re-wrap the VDK to a new KEK (KEK rotation or an in-place KeyStore
    /// upgrade).
    pub fn rewrap_box(&self, new_kek: &Key32, stamp: &str, action: &str) -> Result<()> {
        let mut keys = self.keys.write();
        let new_header = hkdf32(new_kek.as_bytes(), &self.vid, crypto::INFO_HEADER);
        let new_boxw = hkdf32(new_kek.as_bytes(), &self.vid, crypto::INFO_BOX_WRAP);
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let mut h = load_header(&tx)?.ok_or_else(|| VaultError::internal("header"))?.0;
        let (n, ct) = xseal(&new_boxw, &box_aad(&self.vid), keys.vdk.as_bytes());
        h.box_wrap = n.to_vec();
        h.box_wrap.extend_from_slice(&ct);
        append_audit(&tx, &keys.audit, &mut h, &Entry::new(action, stamp, "ok"))?;
        h.gen += 1;
        write_header(&tx, &h, &new_header)?;
        tx.commit()?;
        drop(conn);
        keys.header = new_header;
        keys.boxw = new_boxw;
        drop(keys);
        Ok(())
    }

    /// Re-anchor after an in-place KeyStore move (the new store has no
    /// anchor yet).
    pub fn reanchor(&self) {
        let conn = self.conn.lock();
        if let Ok(Some((h, _))) = load_header(&conn) {
            drop(conn);
            self.write_anchor(&h);
        }
    }

    /// New VDK; re-encrypt every version; re-MAC every row and the audit
    /// chain; re-wrap to every holder (§12 `rekey`).
    pub fn rekey(&self, actor: &Actor, proof: &HumanProof) -> Result<()> {
        if !actor.is_admin_human() || !matches!(proof, HumanProof::Confirmed { .. }) {
            return Err(VaultError::forbidden("vault_confirm_required", "a rekey needs the owner's confirm"));
        }
        if self.is_frozen() {
            return Err(crate::boxmove::moved_out());
        }
        self.flush_rollups();
        let stamp = actor.stamp();
        let mut keys = self.keys.write();
        verify_chain(&self.conn.lock(), &keys.audit)?;
        let new_keys = keys.with_vdk(Key32::random(), &self.vid);
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        // Secrets and versions.
        let rows: Vec<(SecretRow, Vec<u8>)> = {
            let mut st = tx.prepare("SELECT * FROM secret")?;
            let it = st.query_map([], |r| Ok(read_secret_raw(r)))?;
            let mut v = vec![];
            for r in it {
                v.push(r??);
            }
            v
        };
        for (row, mac) in rows {
            if !row_mac_ok(&keys.row, &row, &mac) {
                return Err(VaultError::tamper(format!("secret {} failed its integrity check; rekey stopped", row.meta.id)));
            }
            let m = &row.meta;
            #[allow(clippy::type_complexity)]
            let versions: Vec<(u32, Vec<u8>, Vec<u8>, String, String, String, Option<String>, String, Vec<u8>)> = {
                let mut st = tx.prepare(
                    "SELECT version, nonce, ciphertext, created_at, created_by, state, revoked_at, fingerprint, row_mac FROM secret_version WHERE secret_id = ?1",
                )?;
                let it = st.query_map(params![m.id], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?))
                })?;
                it.collect::<std::result::Result<_, _>>()?
            };
            for (v, nonce, ct, cat, cby, state, rev, old_fp, vmac_old) in versions {
                if !hmac_verify(&keys.row, &version_mac_input(&m.id, v, &nonce, &ct, &old_fp, &cat, &cby, &state, rev.as_deref()), &vmac_old) {
                    return Err(VaultError::tamper(format!("secret {} v{v} failed its integrity check; rekey stopped", m.id)));
                }
                let pt = SecretBytes::new(xopen(&keys.vdk, &nonce, &value_aad(&self.vid, m, v), &ct)?);
                let (n2, c2) = xseal(&new_keys.vdk, &value_aad(&self.vid, m, v), pt.expose());
                let fp = hex(&hmac(&new_keys.fp, pt.expose()));
                let vmac = hmac(&new_keys.row, &version_mac_input(&m.id, v, &n2, &c2, &fp, &cat, &cby, &state, rev.as_deref()));
                tx.execute(
                    "UPDATE secret_version SET nonce = ?3, ciphertext = ?4, fingerprint = ?5, row_mac = ?6 WHERE secret_id = ?1 AND version = ?2",
                    params![m.id, v, n2.to_vec(), c2, fp, vmac.to_vec()],
                )?;
            }
            let smac = hmac(&new_keys.row, &secret_mac_input(m, &row.external_ref));
            tx.execute("UPDATE secret SET row_mac = ?2 WHERE id = ?1", params![m.id, smac.to_vec()])?;
        }
        // Grants and requests: verified under the old key first (a tampered
        // row must stop the rekey, never come out valid under the new key).
        let grants: Vec<(Grant, Vec<u8>)> = {
            let mut st = tx.prepare("SELECT * FROM grants")?;
            let it = st.query_map([], |r| Ok(read_grant_raw(r)))?;
            let mut v = vec![];
            for r in it {
                v.push(r??);
            }
            v
        };
        for (g, old_mac) in grants {
            if !hmac_verify(&keys.row, &grant_mac_input(&g), &old_mac) {
                return Err(VaultError::tamper(format!("grant {} failed its integrity check; rekey stopped", g.id)));
            }
            let mac = hmac(&new_keys.row, &grant_mac_input(&g));
            tx.execute("UPDATE grants SET row_mac = ?2 WHERE id = ?1", params![g.id, mac.to_vec()])?;
        }
        let reqs: Vec<(SecretRequest, Vec<u8>)> = {
            let mut st = tx.prepare("SELECT * FROM secret_request")?;
            let it = st.query_map([], |r| Ok(read_request_raw(r)))?;
            let mut v = vec![];
            for r in it {
                v.push(r??);
            }
            v
        };
        for (r, old_mac) in reqs {
            if !hmac_verify(&keys.row, &request_mac_input(&r), &old_mac) {
                return Err(VaultError::tamper(format!("secret card {} failed its integrity check; rekey stopped", r.card_id)));
            }
            let mac = hmac(&new_keys.row, &request_mac_input(&r));
            tx.execute("UPDATE secret_request SET row_mac = ?2 WHERE card_id = ?1", params![r.card_id, mac.to_vec()])?;
        }
        // Every other MAC'd table, the same way (verify old, re-MAC new);
        // pending handoff values are re-sealed under the new data key.
        self.rekey_placements(&tx, &keys, &new_keys)?;
        self.rekey_request_inject(&tx, &keys, &new_keys)?;
        self.rekey_handoff_tables(&tx, &keys, &new_keys)?;
        self.rekey_agent_tables(&tx, &keys, &new_keys)?;
        // Placeholders hash under the old key: drop them (sessions re-mint).
        tx.execute("DELETE FROM placeholder", [])?;
        // Audit chain under the new key.
        let audit_rows: Vec<(i64, String, Entry)> = {
            let mut st = tx.prepare("SELECT seq, ts, secret_id, version, action, actor, consumer, target_host, outcome, count FROM audit ORDER BY seq")?;
            let it = st.query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    Entry {
                        secret_id: r.get(2)?,
                        version: r.get(3)?,
                        action: r.get(4)?,
                        actor: r.get(5)?,
                        consumer: r.get(6)?,
                        target_host: r.get(7)?,
                        outcome: r.get(8)?,
                        count: r.get::<_, i64>(9)? as u64,
                    },
                ))
            })?;
            it.collect::<std::result::Result<_, _>>()?
        };
        let mut prev = vec![0u8; 32];
        for (seq, t, e) in &audit_rows {
            let mac = audit::row_mac(&new_keys.audit, *seq as u64, t, e, &prev);
            tx.execute("UPDATE audit SET prev_mac = ?2, row_mac = ?3 WHERE seq = ?1", params![seq, prev, mac.to_vec()])?;
            prev = mac.to_vec();
        }
        let mut h = load_header(&tx)?.ok_or_else(|| VaultError::internal("header"))?.0;
        h.audit_mac = prev;
        let (n, ct) = xseal(&new_keys.boxw, &box_aad(&self.vid), new_keys.vdk.as_bytes());
        h.box_wrap = n.to_vec();
        h.box_wrap.extend_from_slice(&ct);
        for w in h.wraps.iter_mut() {
            let r = Recipient::parse(&w.recipient)?;
            w.wrapped = base64_encode(&agefmt::encrypt(&[r], new_keys.vdk.as_bytes()));
        }
        append_audit(&tx, &new_keys.audit, &mut h, &Entry::new("rekey", &stamp, "ok"))?;
        h.gen += 1;
        write_header(&tx, &h, &new_keys.header)?;
        tx.commit()?;
        drop(conn);
        *keys = new_keys;
        drop(keys);
        self.write_anchor(&h);
        self.checkpoint();
        *self.redactor.lock() = None;
        // Every row MAC changed: re-mark the audit chain under the new key
        // (the sign path's incremental check starts from the marked MAC).
        self.verify_audit_full();
        Ok(())
    }

    /// The owner confirms a detected rollback is fine (VS8): re-anchor at
    /// the current state and resume explicit grants.
    pub fn acknowledge_rollback(&self, actor: &Actor, proof: &HumanProof) -> Result<()> {
        if !actor.is_admin_human() || !matches!(proof, HumanProof::Confirmed { .. }) {
            return Err(VaultError::forbidden("vault_confirm_required", "this needs the owner's confirm"));
        }
        let a = self.ks.anchor()?.map(|a| a.gen).unwrap_or(0);
        *self.rollback.lock() = false;
        let stamp = actor.stamp();
        self.mutate(true, |tx, keys| {
            let mut h = load_header(tx)?.ok_or_else(|| VaultError::internal("header"))?.0;
            if h.gen <= a {
                h.gen = a;
            }
            write_header(tx, &h, &keys.header)?;
            Ok(((), vec![Entry::new("restore", &stamp, "ok:rollback-acknowledged")]))
        })?;
        self.tamper.lock().retain(|t| !t.starts_with("vault.db is older"));
        Ok(())
    }

    // ── redactor (KV33) ─────────────────────────────────────────────

    /// Exact-value matcher over every active `user` value (≥ 8 bytes).
    pub fn redactor(&self) -> Arc<Redactor> {
        if let Some(r) = self.redactor.lock().as_ref() {
            return r.clone();
        }
        let keys = self.keys.read();
        let conn = self.conn.lock();
        let mut vals: Vec<(String, SecretBytes)> = vec![];
        if let Ok(mut st) = conn.prepare("SELECT * FROM secret WHERE namespace = 'user' AND status != 'destroyed'") {
            if let Ok(it) = st.query_map([], |r| Ok(read_secret_raw(r))) {
                for r in it.flatten().flatten() {
                    let (row, mac) = r;
                    if !row_mac_ok(&keys.row, &row, &mac) {
                        continue;
                    }
                    if let Ok(v) = self.decrypt_version(&conn, &keys, &row.meta, row.meta.latest_version) {
                        if v.len() >= SURFACE_MIN_LEN {
                            vals.push((row.meta.name.clone(), v));
                        }
                    }
                }
            }
        }
        drop(conn);
        drop(keys);
        let r = Arc::new(Redactor::new(vals.iter().map(|(n, v)| (n.as_str(), v.expose()))));
        *self.redactor.lock() = Some(r.clone());
        r
    }

    // ── maintenance ─────────────────────────────────────────────────

    /// Periodic work: flush roll-ups, drop versions revoked > 30 days ago,
    /// expire chat secrets and old requests. Returns the requests it
    /// expired, so the daemon can close their Thread cards.
    pub fn tick(&self) -> Result<Vec<SecretRequest>> {
        self.flush_rollups_if_due();
        // The full audit chain, first (no early return skips it): an old
        // row edited in place is caught here; the sign path checks only
        // the tail.
        self.verify_audit_full();
        if self.is_frozen() {
            // Moved out (prd-box-move-carry-v1 §3.2): the ticker's writes
            // (expiry, retention, card expiry) wait; reads go on.
            return Ok(vec![]);
        }
        let cutoff = ts(now() - chrono::Duration::days(VERSION_RETENTION_DAYS));
        let req_cutoff = ts(now() - chrono::Duration::days(REQUEST_EXPIRY_DAYS));
        let nowt = ts(now());
        let expired_sessions: Vec<String> = {
            let conn = self.conn.lock();
            let mut st = conn.prepare(
                "SELECT id FROM secret WHERE scope_kind = 'session' AND status = 'active' AND expires_at IS NOT NULL AND expires_at < ?1",
            )?;
            let it = st.query_map(params![nowt], |r| r.get::<_, String>(0))?;
            it.collect::<std::result::Result<_, _>>()?
        };
        for id in expired_sessions {
            let _ = self.set_status(&Selector::Id(id), &Actor::System("expiry".into()), SecretStatus::Destroyed);
        }
        let n = {
            let conn = self.conn.lock();
            conn.query_row(
                "SELECT COUNT(*) FROM secret_version WHERE state = 'revoked' AND revoked_at < ?1",
                params![cutoff],
                |r| r.get::<_, i64>(0),
            )?
        };
        if n > 0 {
            self.mutate(true, |tx, _| {
                tx.execute("DELETE FROM secret_version WHERE state = 'revoked' AND revoked_at < ?1", params![cutoff])?;
                Ok(((), vec![Entry::new("destroy", "system:retention", &format!("ok:{n} old versions"))]))
            })?;
            self.checkpoint();
        }
        let stale: Vec<String> = {
            let conn = self.conn.lock();
            let mut st = conn.prepare("SELECT card_id FROM secret_request WHERE status = 'pending' AND created_at < ?1")?;
            let it = st.query_map(params![req_cutoff], |r| r.get::<_, String>(0))?;
            it.collect::<std::result::Result<_, _>>()?
        };
        let mut expired = Vec::with_capacity(stale.len());
        for c in stale {
            if let Ok(r) = self.request_set_status(&c, "expired", None, None, "system:expiry") {
                expired.push(r);
            }
        }
        Ok(expired)
    }

    // ── Thread secret requests (§10.1, VS12, VS13, VS23) ────────────

    #[allow(clippy::too_many_arguments)]
    pub fn request_create(&self, r: SecretRequest) -> Result<SecretRequest> {
        validate_name(&r.name, true)?;
        if r.card_id.len() < 16 {
            return Err(VaultError::bad_request("bad_card", "card ids are random 128-bit ids"));
        }
        if r.prompt.chars().count() > 200 {
            return Err(VaultError::bad_request("prompt_too_long", "card prompts are at most 200 characters"));
        }
        // Agents propose exact hosts only.
        let hosts = hosts::normalise_list(&r.proposed_hosts, false)?;
        if let Some(i) = &r.inject {
            i.card_ok()?;
            // A sealed injection is sent by K2 (SC-A5).
            if r.proposed_delivery != Delivery::Fetch {
                return Err(VaultError::bad_request("inject_needs_fetch", "how a secret is sent applies when K2 makes the call (delivery fetch)"));
            }
        }
        let mut r = r;
        r.proposed_hosts = hosts;
        r.status = "pending".into();
        r.created_at = ts(now());
        let r2 = r.clone();
        self.mutate(false, |tx, keys| {
            let mac = hmac(&keys.row, &request_mac_input(&r2));
            tx.execute(
                "INSERT INTO secret_request (card_id, conversation, session_id, workspace_id, requested_by, name, kind, proposed_scope,
                    proposed_hosts, proposed_delivery, prompt, ttl_secs, status, answered_by, secret_id, created_at, answered_at, row_mac)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,NULL,NULL,?14,NULL,?15)",
                params![
                    r2.card_id, r2.conversation, r2.session_id, r2.workspace_id, r2.requested_by, r2.name, r2.kind.as_str(),
                    r2.proposed_scope, serde_json::to_string(&r2.proposed_hosts).expect("hosts"), r2.proposed_delivery.as_str(),
                    r2.prompt, r2.ttl_secs, r2.status, r2.created_at, mac.to_vec()
                ],
            )?;
            if let Some(i) = &r2.inject {
                let j = serde_json::to_string(i).expect("inject");
                let imac = hmac(&keys.row, &request_inject_mac_input(&r2.card_id, &j));
                tx.execute(
                    "INSERT INTO secret_request_inject (card_id, inject, row_mac) VALUES (?1, ?2, ?3)",
                    params![r2.card_id, j, imac.to_vec()],
                )?;
            }
            Ok(((), vec![Entry::new("request", &r2.requested_by, "ok").consumer(format!("card:{}", r2.card_id))]))
        })?;
        Ok(r)
    }

    pub fn request_get(&self, card_id: &str) -> Result<SecretRequest> {
        let keys = self.keys.read();
        let conn = self.conn.lock();
        let r = conn
            .query_row("SELECT * FROM secret_request WHERE card_id = ?1", params![card_id], |r| Ok(read_request_raw(r)))
            .optional()?;
        let (mut req, mac) = r.ok_or_else(|| VaultError::not_found("no such card"))??;
        if !hmac_verify(&keys.row, &request_mac_input(&req), &mac) {
            return Err(VaultError::tamper("card record failed its integrity check"));
        }
        req.inject = Self::request_inject(&conn, &keys, card_id)?;
        Ok(req)
    }

    /// Is there a live user secret by this name in this scope? A secret
    /// card's `replaces` (SC-A16). Names only; a record that fails its
    /// integrity check reads as present (the answer refuses it anyway).
    pub fn user_secret_exists(&self, scope: &Scope, name: &str) -> bool {
        let keys = self.keys.read();
        let conn = self.conn.lock();
        match self.find_row(&conn, &keys, &Selector::Name { namespace: Namespace::User, scope: scope.clone(), name: name.to_string() }) {
            Ok(r) => r.meta.status != SecretStatus::Destroyed,
            Err(e) => e.status != 404,
        }
    }

    /// A card's sealed injection (SC-A2): None when the card names none. A
    /// row that fails its MAC (tamper, or a row an older daemon's rekey
    /// left under the old key) is a tamper error, so that card can't be
    /// answered (fail closed, §5).
    fn request_inject(conn: &Connection, keys: &Keys, card_id: &str) -> Result<Option<Inject>> {
        let row = conn
            .query_row("SELECT inject, row_mac FROM secret_request_inject WHERE card_id = ?1", params![card_id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
            })
            .optional()?;
        let Some((j, mac)) = row else { return Ok(None) };
        if !hmac_verify(&keys.row, &request_inject_mac_input(card_id, &j), &mac) {
            return Err(VaultError::tamper("the card's 'how it is sent' record failed its integrity check"));
        }
        let i: Inject = serde_json::from_str(&j).map_err(|_| VaultError::tamper("the card's 'how it is sent' record doesn't read"))?;
        Ok(Some(i))
    }

    /// Rekey (SC-A18): verify every sealed card injection under the old row
    /// key, then re-MAC it under the new one, in the rekey's transaction.
    fn rekey_request_inject(&self, tx: &Transaction<'_>, old: &Keys, new: &Keys) -> Result<()> {
        let rows: Vec<(String, String, Vec<u8>)> = {
            let mut st = tx.prepare("SELECT card_id, inject, row_mac FROM secret_request_inject")?;
            let it = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            it.collect::<std::result::Result<_, _>>()?
        };
        for (card, j, mac) in rows {
            if !hmac_verify(&old.row, &request_inject_mac_input(&card, &j), &mac) {
                return Err(VaultError::tamper(format!("secret card {card}'s injection failed its integrity check; rekey stopped")));
            }
            let m2 = hmac(&new.row, &request_inject_mac_input(&card, &j));
            tx.execute("UPDATE secret_request_inject SET row_mac = ?2 WHERE card_id = ?1", params![card, m2.to_vec()])?;
        }
        Ok(())
    }

    pub fn requests(&self, workspace_id: Option<&str>) -> Result<Vec<SecretRequest>> {
        let keys = self.keys.read();
        let conn = self.conn.lock();
        let mut st = conn.prepare("SELECT * FROM secret_request ORDER BY created_at DESC LIMIT 500")?;
        let it = st.query_map([], |r| Ok(read_request_raw(r)))?;
        let mut out = vec![];
        for r in it {
            let Some((mut req, mac)) = tolerate(r)? else { continue };
            if !hmac_verify(&keys.row, &request_mac_input(&req), &mac) {
                continue;
            }
            if workspace_id.is_some_and(|w| w != req.workspace_id) {
                continue;
            }
            // A card whose injection fails its MAC is left out, like a bad row.
            match Self::request_inject(&conn, &keys, &req.card_id) {
                Ok(i) => req.inject = i,
                Err(_) => continue,
            }
            out.push(req);
        }
        Ok(out)
    }

    /// Pending guest cards for one chat (VS13 cap).
    pub fn pending_requests_in(&self, conversation: &str) -> Result<usize> {
        Ok(self.requests(None)?.iter().filter(|r| r.status == "pending" && r.conversation == conversation).count())
    }

    fn request_set_status(&self, card_id: &str, status: &str, answered_by: Option<&str>, secret_id: Option<&str>, stamp: &str) -> Result<SecretRequest> {
        let mut r = self.request_get(card_id)?;
        r.status = status.into();
        r.answered_by = answered_by.map(str::to_string);
        r.secret_id = secret_id.map(str::to_string);
        r.answered_at = Some(ts(now()));
        let r2 = r.clone();
        let st = stamp.to_string();
        self.mutate(false, |tx, keys| {
            let mac = hmac(&keys.row, &request_mac_input(&r2));
            tx.execute(
                "UPDATE secret_request SET status = ?2, answered_by = ?3, secret_id = ?4, answered_at = ?5, row_mac = ?6 WHERE card_id = ?1",
                params![r2.card_id, r2.status, r2.answered_by, r2.secret_id, r2.answered_at, mac.to_vec()],
            )?;
            Ok(((), vec![Entry::new("request", &st, &format!("ok:{}", r2.status)).consumer(format!("card:{}", r2.card_id))]))
        })?;
        Ok(r)
    }

    /// Tests: move a request's `created_at` back `days` (re-MACed), so the
    /// expiry paths run without waiting a week.
    #[cfg(any(test, feature = "test-util"))]
    pub fn backdate_request_for_tests(&self, card_id: &str, days: i64) -> Result<()> {
        let mut r = self.request_get(card_id)?;
        let created = parse_ts(&r.created_at).ok_or_else(|| VaultError::bad_request("bad_ts", "request time"))?;
        r.created_at = ts(created - chrono::Duration::days(days));
        self.mutate(false, |tx, keys| {
            let mac = hmac(&keys.row, &request_mac_input(&r));
            tx.execute(
                "UPDATE secret_request SET created_at = ?2, row_mac = ?3 WHERE card_id = ?1",
                params![r.card_id, r.created_at, mac.to_vec()],
            )?;
            Ok(((), Vec::new()))
        })
    }

    pub fn request_void(&self, card_id: &str, actor: &Actor) -> Result<SecretRequest> {
        let r = self.request_get(card_id)?;
        if r.status != "pending" {
            return Err(VaultError::conflict("card_not_pending", "that card was already answered"));
        }
        self.request_set_status(card_id, "void", Some(&actor.stamp()), None, &actor.stamp())
    }

    /// A person answers a secret card: the value goes straight into the
    /// vault (never the Thread store). Agents and K2 shell tabs can't
    /// answer (KV8). Guests get chat scope, ≤ 24 h, fetch only, ≥ 1 exact
    /// host (VS13).
    pub fn request_answer(&self, card_id: &str, value: SecretBytes, ans: &CardAnswer, actor: &Actor) -> Result<CardResult> {
        if matches!(actor, Actor::Agent(_) | Actor::System(_)) {
            return Err(VaultError::forbidden("secret_card_human_only", "secret cards are answered by a person"));
        }
        let req = self.request_get(card_id)?;
        if req.status != "pending" {
            return Err(VaultError::conflict("card_not_pending", "that card was already answered"));
        }
        if parse_ts(&req.created_at).is_some_and(|c| now() - c > chrono::Duration::days(REQUEST_EXPIRY_DAYS)) {
            let _ = self.request_set_status(card_id, "expired", None, None, "system:expiry");
            return Err(VaultError::conflict("card_not_pending", "that card expired"));
        }
        let guest = matches!(actor, Actor::Guest { .. });
        let member = actor.is_member();
        let scope_word = ans.scope.clone().unwrap_or_else(|| if guest { "session".into() } else { req.proposed_scope.clone() });
        let scope = match scope_word.as_str() {
            "workspace" if !guest => Scope::workspace(req.workspace_id.clone()),
            "server" if actor.is_admin_human() => Scope::server(),
            "session" => Scope::session(req.conversation.clone()),
            "server" => return Err(VaultError::forbidden("scope_not_allowed", "only an Owner or Admin can share with all agents")),
            _ => return Err(VaultError::forbidden("scope_not_allowed", "guests can only give a secret for this chat")),
        };
        let _ = member;
        // People may type wildcards (VS4); agent proposals were exact.
        let hosts = hosts::normalise_list(&ans.hosts.clone().unwrap_or_else(|| req.proposed_hosts.clone()), !guest)?;
        let mut delivery = ans.delivery.unwrap_or(req.proposed_delivery);
        if guest {
            if delivery != Delivery::Fetch {
                return Err(VaultError::forbidden("guest_fetch_only", "a guest's secret is only added to calls by K2"));
            }
            if hosts.is_empty() || hosts.iter().any(|h| h.contains("://*.")) {
                return Err(VaultError::bad_request("guest_host_required", "add the exact site this key is for"));
            }
        }
        if delivery == Delivery::Proxy {
            delivery = Delivery::Fetch;
        }
        if delivery == Delivery::Fetch && hosts.is_empty() {
            return Err(VaultError::bad_request("hosts_required", "add a site, or let the agent read it"));
        }
        // How it is sent (prd-secret-card-injection-v1 §2, §3.1). The
        // proposal is the sealed row, never the Thread doc. Save echoes
        // what the card showed: no echo is an older client that can't
        // show it (fail closed, §5); a different one is the person's own
        // edit, checked like the agent's and never a guest's (SC-A14).
        let inject = match (&req.inject, &ans.inject) {
            (Some(sealed), None) => {
                let to = if req.proposed_hosts.is_empty() { String::new() } else { format!(" to {}", sites_words(&req.proposed_hosts)) };
                return Err(VaultError::conflict(
                    "card_inject_unseen",
                    format!("Update K2 to answer this card. It sends {} {}{to}, and this app can't show that. You can also answer it in the server's own K2 app.", req.name, sealed.words()),
                ));
            }
            (Some(sealed), Some(shown)) if shown == sealed => Some(sealed.clone()),
            (_, Some(shown)) => {
                if guest {
                    return Err(VaultError::forbidden("inject_not_editable", "a guest can't change how this secret is sent"));
                }
                shown.card_ok()?;
                Some(shown.clone())
            }
            (None, None) => None,
        };
        if let Some(i) = &inject {
            i.check_card_value(value.expose())?;
        }
        // VS12: a key that looks like another provider's, or a readable
        // delivery, needs the second click. A card that already showed the
        // readable delivery in plain words (`readable_disclosed`) skips
        // only the second. The mismatch check stays first: an older card
        // that retries a `card_readable_delivery` with `confirm` relies on
        // it having passed.
        if !ans.confirmed_warnings {
            if let Some(p) = inject::host_mismatch(value.expose(), &hosts) {
                return Err(VaultError::conflict(
                    "card_host_mismatch",
                    format!("this looks like a {} key, but the card sends it somewhere else; confirm to save anyway", p.label),
                ));
            }
            if delivery.readable() && !ans.readable_disclosed {
                return Err(VaultError::conflict("card_readable_delivery", "the agent will be able to see this value; confirm to save"));
            }
        }
        let expires_at = if scope.kind == ScopeKind::Session {
            let ttl = req.ttl_secs.unwrap_or(GUEST_TTL_MAX_SECS).clamp(60, if guest { GUEST_TTL_MAX_SECS } else { 30 * 24 * 3600 });
            Some(ts(now() + chrono::Duration::seconds(ttl)))
        } else {
            None
        };
        // SC-A1: a card for a name that already exists. Its terms (sites,
        // delivery, how it is sent) are applied as the answering person's
        // own policy change, then the value is replaced. Before 0.46.6 only
        // the value was replaced and the card's terms were silently dropped.
        let existing = {
            let keys = self.keys.read();
            let conn = self.conn.lock();
            match self.find_row(&conn, &keys, &Selector::Name { namespace: Namespace::User, scope: scope.clone(), name: req.name.clone() }) {
                Ok(r) => Some(r),
                Err(e) if e.status == 404 => None,
                Err(e) => return Err(e),
            }
        };
        let (out, replaced, inject_was) = match existing {
            Some(row) => {
                let (out, was) = self.card_replace(row, value, hosts, delivery, inject, guest, actor)?;
                (out, true, was)
            }
            None => {
                let mut put = PutRequest::new(Namespace::User, scope.clone(), req.name.clone(), req.kind);
                put.hosts = hosts;
                put.inject = inject;
                put.default_delivery = Some(delivery);
                put.delivery_ceiling = Some(if guest { Delivery::Fetch } else if delivery.level() > Delivery::Env.level() { delivery } else { Delivery::Env });
                put.origin = format!("thread:{card_id}");
                put.expires_at = expires_at;
                // Create, not upsert: a secret that appeared since the look
                // above is refused, never rotated without the card's terms.
                put.mode = PutMode::Create;
                put.human_given_override = Some(true);
                (self.put_inner(put, value, actor)?, false, None)
            }
        };
        self.request_set_status(card_id, "set", Some(&actor.stamp()), Some(&out.meta.id), &actor.stamp())?;
        let reference = crate::refs::VaultRef {
            scope_kind: scope.kind,
            scope_ref: scope.id.clone(),
            name: req.name.clone(),
            version: None,
        }
        .render();
        Ok(CardResult { meta: out.meta, reference, trimmed: out.trimmed, replaced, inject_was })
    }

    /// A secret card answered for a name that already exists (SC-A1). The
    /// card's terms are the sites, the delivery (the ceiling rises only as
    /// far as that delivery needs) and how it is sent (kept as stored when
    /// the card names none). When they differ from what is stored they go
    /// through [`Self::plan_policy`] for this person: Owner or Admin may
    /// widen; a Member who would widen gets `vault_admin_required`; a guest
    /// only ever replaces the value of matching terms. Nothing is written
    /// on a refusal. The value is then a person's (`human_given`), so an
    /// agent can't change how it is sent afterwards without the owner.
    /// Returns how it was sent before, in words, when that changed.
    #[allow(clippy::too_many_arguments)]
    fn card_replace(
        &self,
        row: SecretRow,
        value: SecretBytes,
        hosts: Vec<String>,
        delivery: Delivery,
        inject: Option<Inject>,
        guest: bool,
        actor: &Actor,
    ) -> Result<(PutOutcome, Option<String>)> {
        if *self.rollback.lock() {
            return Err(VaultError::new("vault_rollback", 409, "the vault looks rolled back; the owner must confirm in Settings → Vault"));
        }
        let (value, trimmed) = Self::prepare_value(row.meta.kind, value)?;
        if row.meta.kind == Kind::SshKey {
            crate::ssh_agent::check_ssh_key_value(value.expose())?;
        }
        let old = row.meta.clone();
        let ceiling = if delivery.level() > old.delivery_ceiling.level() { delivery } else { old.delivery_ceiling };
        let new_inject = inject.clone().or_else(|| old.inject.clone());
        let differs = old.hosts != hosts || old.default_delivery != delivery || old.delivery_ceiling != ceiling || old.inject != new_inject;
        let mut meta = if differs {
            if guest {
                return Err(VaultError::forbidden(
                    "vault_admin_required",
                    format!("{} already exists and this card changes where it goes or how it is sent; an Owner or Admin must save this card", old.name),
                ));
            }
            let patch = PolicyPatch {
                hosts: Some(hosts),
                default_delivery: Some(delivery),
                delivery_ceiling: Some(ceiling),
                inject: inject.clone(),
                ..Default::default()
            };
            match self.plan_policy(&Selector::Id(old.id.clone()), patch, actor) {
                Ok((_, m, loosens)) => Some((m, loosens)),
                Err(e) if e.code == "vault_admin_required" => {
                    return Err(VaultError::forbidden(
                        "vault_admin_required",
                        format!("this card widens where {} goes or how it is sent; an Owner or Admin must save this card", old.name),
                    ))
                }
                Err(e) => return Err(e),
            }
        } else {
            None
        };
        // The value is now a person's (§3.3): the stricter rule for agents.
        if !old.human_given {
            let (m, l) = meta.take().unwrap_or_else(|| (old.clone(), false));
            meta = Some((SecretMeta { human_given: true, ..m }, l));
        }
        let inject_was = meta
            .as_ref()
            .filter(|(m, _)| m.inject != old.inject)
            .map(|_| old.inject.as_ref().map(Inject::words).unwrap_or_else(|| "not added to calls".into()));
        let terms = meta.map(|(meta, loosens)| CardTerms { meta, loosens });
        let out = self.rotate_row(row, value, actor, false, trimmed, None, terms)?;
        Ok((out, inject_was))
    }

    // ── proxy placeholders (§6.4, §9.3) ────────────────────────────

    fn placeholder_hash(&self, token: &str) -> String {
        hex(&hmac(&self.keys.read().ph, token.as_bytes()))
    }

    /// Mint a placeholder for (secret, workspace, session, provider). Only
    /// its hash is stored. The proxy resolves the secret for that session
    /// at use time, so a revoked grant stops a live placeholder.
    pub fn placeholder_mint(
        &self,
        secret_id: &str,
        workspace_id: &str,
        session_id: &str,
        provider: &str,
        expires_at: Option<String>,
    ) -> Result<String> {
        if secret_id.is_empty() || workspace_id.trim().is_empty() || session_id.trim().is_empty() || provider.is_empty() {
            return Err(VaultError::bad_request(
                "bad_placeholder",
                "a placeholder needs a secret, a workspace, a session and a provider",
            ));
        }
        let token = crate::refs::new_placeholder();
        let h = self.placeholder_hash(&token);
        let (sid, ws, sess, prov) =
            (secret_id.to_string(), workspace_id.to_string(), session_id.to_string(), provider.to_string());
        self.mutate_frozen_ok(false, |tx, _| {
            tx.execute(
                "INSERT INTO placeholder (token_hash, secret_id, workspace_id, session_id, provider, created_at, expires_at) VALUES (?1,?2,?3,?4,?5,?6,?7)",
                params![h, sid, ws, sess, prov, ts(now()), expires_at],
            )?;
            Ok(((), vec![]))
        })?;
        Ok(token)
    }

    /// Look a placeholder up by its hash.
    pub fn placeholder_lookup(&self, token: &str) -> Result<Placeholder> {
        if !crate::refs::looks_like_placeholder(token) {
            return Err(VaultError::new("placeholder_unknown", 401, "unknown placeholder"));
        }
        let h = self.placeholder_hash(token);
        let conn = self.conn.lock();
        let r: Option<(Placeholder, Option<String>)> = conn
            .query_row(
                "SELECT secret_id, workspace_id, session_id, provider, expires_at FROM placeholder WHERE token_hash = ?1",
                params![h],
                |r| {
                    Ok((
                        Placeholder {
                            secret_id: r.get(0)?,
                            workspace_id: r.get(1)?,
                            session_id: r.get(2)?,
                            provider: r.get(3)?,
                        },
                        r.get(4)?,
                    ))
                },
            )
            .optional()?;
        let (p, exp) = r.ok_or_else(|| VaultError::new("placeholder_unknown", 401, "unknown placeholder"))?;
        if exp.as_deref().and_then(parse_ts).is_some_and(|x| x < now()) {
            return Err(VaultError::new("placeholder_unknown", 401, "placeholder expired"));
        }
        Ok(p)
    }

    /// Session ended: its placeholders die, and so do the session grants
    /// K2 itself made for it (an LLM wallet key on the proxy).
    pub fn placeholders_drop_session(&self, session_id: &str) -> Result<()> {
        self.drop_session_material(Some(session_id))
    }

    /// Drop every placeholder and every session grant K2 itself made.
    ///
    /// Never call this at daemon boot: with k2-ptyd, agent sessions survive
    /// a daemon restart and keep using theirs (PRD prd-k2-ptyd-v1 P90). A
    /// session's material goes with the session
    /// ([`Self::placeholders_drop_session`]).
    pub fn placeholders_drop_all(&self) -> Result<()> {
        self.drop_session_material(None)
    }

    fn drop_session_material(&self, session_id: Option<&str>) -> Result<()> {
        let sess = session_id.map(str::to_string);
        self.mutate_frozen_ok(false, |tx, _| {
            match &sess {
                Some(s) => tx.execute("DELETE FROM placeholder WHERE session_id = ?1", params![s])?,
                None => tx.execute("DELETE FROM placeholder", [])?,
            };
            Ok(((), vec![]))
        })?;
        // Grants are anchored state: only move the generation when there
        // is one to remove.
        let gone: Vec<(String, String, String)> = {
            let conn = self.conn.lock();
            let mut st = conn.prepare(
                "SELECT id, secret_id, consumer_id FROM grants \
                 WHERE consumer_kind = 'session' AND granted_by LIKE 'system:%' AND (?1 IS NULL OR consumer_id = ?1)",
            )?;
            let it = st.query_map(params![sess], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            it.collect::<std::result::Result<_, _>>()?
        };
        if gone.is_empty() {
            return Ok(());
        }
        self.mutate(true, |tx, _| {
            let mut entries = vec![];
            for (id, secret_id, consumer) in &gone {
                tx.execute("DELETE FROM grants WHERE id = ?1", params![id])?;
                entries.push(
                    Entry::new("revoke", "system:session-end", "ok")
                        .secret(secret_id, None)
                        .consumer(Consumer::session(consumer.clone()).label()),
                );
            }
            Ok(((), entries))
        })
    }

    // ── status ──────────────────────────────────────────────────────

    pub fn status_inner(&self) -> VaultStatus {
        let conn = self.conn.lock();
        let header = load_header(&conn).ok().flatten().map(|(h, _)| h);
        let count = |sql: &str| conn.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap_or(0) as u64;
        let counts = Counts {
            user: count("SELECT COUNT(*) FROM secret WHERE namespace = 'user' AND status = 'active'"),
            system: count("SELECT COUNT(*) FROM secret WHERE namespace = 'system' AND status = 'active'"),
            disabled: count("SELECT COUNT(*) FROM secret WHERE status = 'disabled'"),
            grants: count("SELECT COUNT(*) FROM grants"),
            pending_requests: count("SELECT COUNT(*) FROM secret_request WHERE status = 'pending'"),
            rotate_due: 0,
            newer_secrets: count_newer(&conn, "SELECT * FROM secret", read_secret_raw),
            newer_grants: count_newer(&conn, "SELECT * FROM grants", read_grant_raw),
        };
        drop(conn);
        let due = self
            .list_inner(&ListFilter::default(), &Actor::System("status".into()))
            .map(|l| l.iter().filter(|m| rotate_due(m)).count() as u64)
            .unwrap_or(0);
        let tamper = self.tamper.lock().clone();
        let h = header.unwrap_or(Header {
            format: FORMAT.into(),
            vault_id: self.vault_id.clone(),
            created_at: String::new(),
            gen: 0,
            box_wrap: vec![],
            wraps: vec![],
            recovery_state: "unknown".into(),
            recovery_confirmed_at: None,
            confirm_hash: None,
            carry_recipient: None,
            audit_seq: 0,
            audit_mac: vec![],
        });
        let home = self.ks.kind();
        VaultStatus {
            open: true,
            vault_id: self.vault_id.clone(),
            format: h.format,
            key_home: home.as_str().into(),
            key_home_label: home.describe().into(),
            rollback_guard: if self.ks.supports_anchor() { "on".into() } else { "off".into() },
            rollback: *self.rollback.lock(),
            tamper: if tamper.is_empty() { "ok".into() } else { tamper.join("; ") },
            recovery: h.recovery_state,
            recovery_confirmed_at: h.recovery_confirmed_at,
            confirm_password: h.confirm_hash.is_some(),
            gen: h.gen,
            counts: Counts { rotate_due: due, ..counts },
            memory_lock: if crate::secret::mlock_failed() { "failed".into() } else { "ok".into() },
            created_at: h.created_at,
            holders: holders_of(&h.wraps),
        }
    }

    /// Restore support: open a copied vault.db with an age identity
    /// (recovery or backup), then re-wrap it to this box's key.
    /// Restore dry run: does `identity` open the `holder` wrap of the vault
    /// in `dir`, and does its audit chain verify? Changes nothing. Returns
    /// the vault id and how many `user` secrets it holds.
    pub fn check_identity(dir: &Path, identity: &agefmt::Identity, holder: &str) -> Result<(String, u64)> {
        let path = dir.join("vault.db");
        if !path.exists() {
            return Err(VaultError::not_found("no vault.db there"));
        }
        let conn = Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let (h, _mac) = load_header(&conn)?.ok_or_else(|| VaultError::tamper("vault.db has no header"))?;
        let w = h
            .wraps
            .iter()
            .find(|w| w.holder == holder)
            .ok_or_else(|| VaultError::forbidden("no_holder", format!("this vault has no {holder} key")))?;
        let vdk = agefmt::decrypt(identity, &base64_decode(&w.wrapped)?)?;
        if vdk.len() != 32 {
            return Err(VaultError::tamper("data key has the wrong length"));
        }
        let vid = vid_bytes(&h.vault_id)?;
        let audit_key = hkdf32(vdk.expose(), &vid, crypto::INFO_AUDIT);
        verify_chain(&conn, &audit_key)?;
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM secret WHERE namespace = 'user' AND status = 'active'", [], |r| r.get(0))?;
        Ok((h.vault_id, n as u64))
    }

    /// Read a vault.db header without any key (its columns are plain):
    /// `(vault id, gen, audit_seq, audit_mac hex)`, or `None` when there is
    /// no vault.db or no header. The header MAC isn't checked here.
    pub fn peek_header(path: &Path) -> Result<Option<(String, u64, u64, String)>> {
        if !path.exists() {
            return Ok(None);
        }
        let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        Ok(load_header(&conn)?.map(|(h, _)| (h.vault_id, h.gen, h.audit_seq, hex(&h.audit_mac))))
    }

    /// How many active `user` secrets this vault holds.
    pub fn user_secret_count(&self) -> u64 {
        let conn = self.conn.lock();
        conn.query_row("SELECT COUNT(*) FROM secret WHERE namespace = 'user' AND status = 'active'", [], |r| r.get::<_, i64>(0))
            .unwrap_or(0) as u64
    }

    pub fn adopt_with_identity(dir: impl Into<PathBuf>, ks: Arc<dyn KeyStore>, identity: &agefmt::Identity, holder: &str, opts: OpenOptions) -> Result<LocalVault> {
        let dir = dir.into();
        let path = dir.join("vault.db");
        if !path.exists() {
            return Err(VaultError::not_found("no vault.db to restore"));
        }
        let (h, vdk) = {
            let conn = Connection::open(&path)?;
            db::configure(&conn)?;
            db::migrate(&conn)?;
            let (h, _mac) = load_header(&conn)?.ok_or_else(|| VaultError::tamper("vault.db has no header"))?;
            let w = h
                .wraps
                .iter()
                .find(|w| w.holder == holder)
                .ok_or_else(|| VaultError::forbidden("no_holder", format!("this vault has no {holder} key")))?;
            let file = base64_decode(&w.wrapped)?;
            let vdk_bytes = agefmt::decrypt(identity, &file)?;
            if vdk_bytes.len() != 32 {
                return Err(VaultError::tamper("data key has the wrong length"));
            }
            let mut arr = [0u8; 32];
            arr.copy_from_slice(vdk_bytes.expose());
            (h, Key32::from_array(arr))
        };
        let vid = vid_bytes(&h.vault_id)?;
        // Check the data key against the audit chain before trusting it.
        let audit_key = hkdf32(vdk.as_bytes(), &vid, crypto::INFO_AUDIT);
        {
            let conn = Connection::open(&path)?;
            verify_chain(&conn, &audit_key)?;
        }
        let kek = ks.load_or_init()?;
        let keys = Keys::derive(vdk, &kek, &vid);
        // BM-A6: a reused box may keep a higher anchor `gen` from an older
        // install (the keeper only counts up), so the adopted vault starts
        // above both: gen = max(this vault.db, the anchor) + 1.
        let anchor_gen = ks.anchor()?.map(|a| a.gen).unwrap_or(0);
        {
            let mut conn = Connection::open(&path)?;
            let tx = conn.transaction()?;
            let mut h2 = h.clone();
            h2.gen = h2.gen.max(anchor_gen);
            let (n, ct) = xseal(&keys.boxw, &box_aad(&vid), keys.vdk.as_bytes());
            h2.box_wrap = n.to_vec();
            h2.box_wrap.extend_from_slice(&ct);
            append_audit(&tx, &keys.audit, &mut h2, &Entry::new("restore", "system:restore", &format!("ok:{holder}")))?;
            h2.gen += 1;
            write_header(&tx, &h2, &keys.header)?;
            tx.commit()?;
            // The restored vault is this box's vault now: anchor it.
            ks.set_anchor(&Anchor {
                vault_id: Some(h2.vault_id.clone()),
                gen: h2.gen,
                audit_seq: Some(h2.audit_seq),
                audit_mac: Some(hex(&h2.audit_mac)),
            })?;
        }
        drop(keys);
        LocalVault::open(dir, ks, opts)
    }

    /// `VACUUM INTO` a new file (0600).
    pub(crate) fn vacuum_into(&self, dest: &Path) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute("VACUUM INTO ?1", params![dest.to_string_lossy().to_string()])?;
        drop(conn);
        crate::fsutil::set_private_file(dest)?;
        Ok(())
    }
}

fn rotate_due(m: &SecretMeta) -> bool {
    match (m.rotate_every_days, parse_ts(&m.updated_at)) {
        (Some(d), Some(u)) => now() - u > chrono::Duration::days(i64::from(d)),
        _ => false,
    }
}

fn to_action(s: SecretStatus) -> &'static str {
    match s {
        SecretStatus::Active => "enable",
        SecretStatus::Disabled => "disable",
        SecretStatus::Destroyed => "destroy",
    }
}

fn default_origin(actor: &Actor) -> String {
    match actor {
        Actor::Owner => "cli-owner".into(),
        Actor::Login { .. } => "ui".into(),
        Actor::Agent(_) => "cli-agent".into(),
        Actor::Guest { .. } => "guest".into(),
        Actor::System(s) => format!("system:{s}"),
    }
}

fn base64_encode(b: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(b)
}

fn base64_decode(s: &str) -> Result<Vec<u8>> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.decode(s).map_err(|_| VaultError::tamper("bad base64 in header"))
}

// ── row (de)serialisation ───────────────────────────────────────────

/// R2 (PRD vault-gaps): an enum value this build doesn't know means the row
/// was written by a newer K2, not that it was tampered with.
fn newer<T>(r: Result<T>) -> Result<T> {
    r.map_err(|e| VaultError::needs_newer(format!("a vault row was written by a newer K2 ({}); update K2 to use it", e.hint)))
}

/// A secret's `inject` JSON. Not JSON at all is tamper; a JSON object this
/// build can't read (a newer `dst`, say) is a newer K2's row (R2). Unknown
/// fields are ignored by serde (R1), so a newer optional field never lands
/// here.
fn parse_inject_row(s: &str) -> Result<Inject> {
    match serde_json::from_str::<Inject>(s) {
        Ok(i) => Ok(i),
        Err(_) => match serde_json::from_str::<serde_json::Value>(s) {
            Ok(serde_json::Value::Object(_)) => Err(VaultError::needs_newer(
                "a vault row was written by a newer K2 (its request inject isn't known here); update K2 to use it",
            )),
            _ => Err(VaultError::tamper("bad inject json")),
        },
    }
}

/// R2: how many rows of one table a newer K2 wrote (this build skips them).
fn count_newer<T>(conn: &Connection, sql: &str, read: fn(&Row<'_>) -> Result<T>) -> u64 {
    let Ok(mut st) = conn.prepare(sql) else { return 0 };
    let Ok(it) = st.query_map([], |r| Ok(read(r).err().is_some_and(|e| e.is_needs_newer()))) else { return 0 };
    it.flatten().filter(|x| *x).count() as u64
}

/// R2: one row from a multi-row scan. `Ok(None)` = written by a newer K2,
/// skip it (counted by [`LocalVault::newer_rows`]); anything else
/// propagates as before (tamper stays fatal).
fn tolerate<T>(r: rusqlite::Result<Result<T>>) -> Result<Option<T>> {
    match r? {
        Ok(v) => Ok(Some(v)),
        Err(e) if e.is_needs_newer() => Ok(None),
        Err(e) => Err(e),
    }
}

fn read_secret_raw(r: &Row<'_>) -> Result<(SecretRow, Vec<u8>)> {
    let get = |n: &str| -> Result<String> { r.get::<_, String>(n).map_err(VaultError::from) };
    let hosts: Vec<String> = serde_json::from_str(&get("allowed_hosts")?).map_err(|_| VaultError::tamper("bad hosts json"))?;
    let inject_s = get("inject")?;
    let inject: Option<Inject> = if inject_s.is_empty() {
        None
    } else {
        Some(parse_inject_row(&inject_s)?)
    };
    let meta = SecretMeta {
        id: get("id")?,
        namespace: newer(Namespace::parse(&get("namespace")?))?,
        scope: Scope { kind: newer(ScopeKind::parse(&get("scope_kind")?))?, id: get("scope_id")? },
        name: get("name")?,
        kind: newer(Kind::parse(&get("kind")?))?,
        provider: get("provider")?,
        hosts,
        inject,
        default_delivery: newer(Delivery::parse(&get("default_delivery")?))?,
        delivery_ceiling: newer(Delivery::parse(&get("delivery_ceiling")?))?,
        origin: get("origin")?,
        created_by: get("created_by")?,
        human_given: r.get::<_, i64>("human_given")? != 0,
        latest_version: r.get::<_, i64>("latest_version")? as u32,
        status: newer(SecretStatus::parse(&get("status")?))?,
        rotate_every_days: r.get::<_, Option<i64>>("rotate_every_days")?.map(|d| d as u32),
        description: get("description")?,
        expires_at: r.get("expires_at")?,
        created_at: get("created_at")?,
        updated_at: get("updated_at")?,
    };
    let mac: Vec<u8> = r.get("row_mac")?;
    Ok((SecretRow { meta, external_ref: get("external_ref")?, inject_raw: inject_s }, mac))
}

fn read_grant_raw(r: &Row<'_>) -> Result<(Grant, Vec<u8>)> {
    let hosts: Option<String> = r.get("hosts_narrowed")?;
    let g = Grant {
        id: r.get("id")?,
        secret_id: r.get("secret_id")?,
        version_selector: r.get("version_selector")?,
        consumer: Consumer {
            kind: newer(ConsumerKind::parse(&r.get::<_, String>("consumer_kind")?))?,
            id: r.get("consumer_id")?,
        },
        delivery: newer(Delivery::parse(&r.get::<_, String>("delivery")?))?,
        env_name: r.get("env_name")?,
        file_name: r.get("file_name")?,
        hosts_narrowed: match hosts {
            Some(h) => Some(serde_json::from_str(&h).map_err(|_| VaultError::tamper("bad hosts json"))?),
            None => None,
        },
        granted_by: r.get("granted_by")?,
        granted_at: r.get("granted_at")?,
        expires_at: r.get("expires_at")?,
    };
    Ok((g, r.get("row_mac")?))
}

fn read_request_raw(r: &Row<'_>) -> Result<(SecretRequest, Vec<u8>)> {
    let hosts: String = r.get("proposed_hosts")?;
    let req = SecretRequest {
        card_id: r.get("card_id")?,
        conversation: r.get("conversation")?,
        session_id: r.get("session_id")?,
        workspace_id: r.get("workspace_id")?,
        requested_by: r.get("requested_by")?,
        name: r.get("name")?,
        kind: newer(Kind::parse(&r.get::<_, String>("kind")?))?,
        proposed_scope: r.get("proposed_scope")?,
        proposed_hosts: serde_json::from_str(&hosts).map_err(|_| VaultError::tamper("bad hosts json"))?,
        proposed_delivery: newer(Delivery::parse(&r.get::<_, String>("proposed_delivery")?))?,
        prompt: r.get("prompt")?,
        ttl_secs: r.get("ttl_secs")?,
        status: r.get("status")?,
        answered_by: r.get("answered_by")?,
        secret_id: r.get("secret_id")?,
        created_at: r.get("created_at")?,
        answered_at: r.get("answered_at")?,
        inject: None,
    };
    Ok((req, r.get("row_mac")?))
}

fn load_header(conn: &Connection) -> Result<Option<(Header, Vec<u8>)>> {
    let r = conn
        .query_row("SELECT * FROM header WHERE id = 1", [], |r| {
            Ok((
                r.get::<_, String>("format")?,
                r.get::<_, String>("vault_id")?,
                r.get::<_, String>("created_at")?,
                r.get::<_, i64>("gen")?,
                r.get::<_, Vec<u8>>("box_wrap")?,
                r.get::<_, String>("wraps")?,
                r.get::<_, String>("recovery_state")?,
                r.get::<_, Option<String>>("recovery_confirmed_at")?,
                r.get::<_, Option<String>>("confirm_hash")?,
                r.get::<_, Option<String>>("carry_recipient")?,
                r.get::<_, i64>("audit_seq")?,
                r.get::<_, Vec<u8>>("audit_mac")?,
                r.get::<_, Vec<u8>>("header_mac")?,
            ))
        })
        .optional()?;
    match r {
        None => Ok(None),
        Some((format, vault_id, created_at, gen, box_wrap, wraps, rs, rca, ch, cr, aseq, amac, hmac_)) => {
            let wraps: Vec<Wrap> = serde_json::from_str(&wraps).map_err(|_| VaultError::tamper("bad wraps json"))?;
            Ok(Some((
                Header {
                    format,
                    vault_id,
                    created_at,
                    gen: gen as u64,
                    box_wrap,
                    wraps,
                    recovery_state: rs,
                    recovery_confirmed_at: rca,
                    confirm_hash: ch,
                    carry_recipient: cr,
                    audit_seq: aseq as u64,
                    audit_mac: amac,
                },
                hmac_,
            )))
        }
    }
}

fn insert_header(conn: &Connection, h: &Header, key: &Key32) -> Result<()> {
    let mac = hmac(key, &h.mac_input());
    conn.execute(
        "INSERT INTO header (id, format, vault_id, created_at, gen, box_wrap, wraps, recovery_state, recovery_confirmed_at, confirm_hash, carry_recipient, audit_seq, audit_mac, header_mac)
         VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            h.format, h.vault_id, h.created_at, h.gen as i64, h.box_wrap, serde_json::to_string(&h.wraps).expect("wraps"),
            h.recovery_state, h.recovery_confirmed_at, h.confirm_hash, h.carry_recipient, h.audit_seq as i64, h.audit_mac,
            mac.to_vec()
        ],
    )?;
    Ok(())
}

fn write_header(conn: &Connection, h: &Header, key: &Key32) -> Result<()> {
    let mac = hmac(key, &h.mac_input());
    conn.execute(
        "UPDATE header SET format = ?1, gen = ?2, box_wrap = ?3, wraps = ?4, recovery_state = ?5, recovery_confirmed_at = ?6,
            confirm_hash = ?7, carry_recipient = ?8, audit_seq = ?9, audit_mac = ?10, header_mac = ?11 WHERE id = 1",
        params![
            h.format, h.gen as i64, h.box_wrap, serde_json::to_string(&h.wraps).expect("wraps"), h.recovery_state,
            h.recovery_confirmed_at, h.confirm_hash, h.carry_recipient, h.audit_seq as i64, h.audit_mac, mac.to_vec()
        ],
    )?;
    Ok(())
}

fn append_audit(conn: &Connection, key: &Key32, h: &mut Header, e: &Entry) -> Result<()> {
    let seq = h.audit_seq + 1;
    let t = ts(now());
    let mac = audit::row_mac(key, seq, &t, e, &h.audit_mac);
    conn.execute(
        "INSERT INTO audit (seq, ts, secret_id, version, action, actor, consumer, target_host, outcome, count, prev_mac, row_mac)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
        params![
            seq as i64, t, e.secret_id, e.version, e.action, e.actor, e.consumer, e.target_host, e.outcome, e.count as i64,
            h.audit_mac, mac.to_vec()
        ],
    )?;
    h.audit_seq = seq;
    h.audit_mac = mac.to_vec();
    Ok(())
}

fn verify_chain(conn: &Connection, key: &Key32) -> Result<()> {
    let mut st = conn.prepare(
        "SELECT seq, ts, secret_id, version, action, actor, consumer, target_host, outcome, count, prev_mac, row_mac FROM audit ORDER BY seq",
    )?;
    let mut rows = st.query([])?;
    let mut prev = vec![0u8; 32];
    let mut expect_seq = 1i64;
    while let Some(r) = rows.next()? {
        let seq: i64 = r.get(0)?;
        if seq != expect_seq {
            return Err(VaultError::tamper(format!("audit log has a gap at {expect_seq}")));
        }
        let e = Entry {
            secret_id: r.get(2)?,
            version: r.get(3)?,
            action: r.get(4)?,
            actor: r.get(5)?,
            consumer: r.get(6)?,
            target_host: r.get(7)?,
            outcome: r.get(8)?,
            count: r.get::<_, i64>(9)? as u64,
        };
        let t: String = r.get(1)?;
        let p: Vec<u8> = r.get(10)?;
        let m: Vec<u8> = r.get(11)?;
        if p != prev || !hmac_verify(key, &{
            let mut enc = Enc::new("audit");
            enc.u64(seq as u64)
                .str(&t)
                .opt_str(e.secret_id.as_deref())
                .opt_i64(e.version)
                .str(&e.action)
                .str(&e.actor)
                .opt_str(e.consumer.as_deref())
                .opt_str(e.target_host.as_deref())
                .str(&e.outcome)
                .u64(e.count)
                .bytes(&p);
            enc.finish().to_vec()
        }, &m)
        {
            return Err(VaultError::tamper(format!("audit log was changed at row {seq}")));
        }
        prev = m;
        expect_seq += 1;
    }
    Ok(())
}

/// Recompute nothing: used by tests to read a fingerprint hex helper.
#[doc(hidden)]
pub fn _unhex(s: &str) -> Option<Vec<u8>> {
    unhex(s)
}

impl SecretStore for LocalVault {
    fn status(&self) -> VaultStatus {
        self.status_inner()
    }
    fn put(&self, req: PutRequest, value: SecretBytes, actor: &Actor) -> Result<PutOutcome> {
        self.put_inner(req, value, actor)
    }
    fn rotate(&self, sel: &Selector, value: SecretBytes, actor: &Actor) -> Result<PutOutcome> {
        let row = {
            let keys = self.keys.read();
            let conn = self.conn.lock();
            self.find_row(&conn, &keys, sel)?
        };
        let (value, trimmed) = Self::prepare_value(row.meta.kind, value)?;
        if row.meta.namespace == Namespace::User && *self.rollback.lock() {
            return Err(VaultError::new("vault_rollback", 409, "the vault looks rolled back; the owner must confirm in Settings → Vault"));
        }
        if row.meta.kind == Kind::SshKey {
            crate::ssh_agent::check_ssh_key_value(value.expose())?;
        }
        self.rotate_row(row, value, actor, false, trimmed, None, None)
    }
    fn meta(&self, sel: &Selector, actor: &Actor) -> Result<SecretMeta> {
        self.meta_inner(sel, actor)
    }
    fn list(&self, filter: &ListFilter, actor: &Actor) -> Result<Vec<SecretMeta>> {
        self.list_inner(filter, actor)
    }
    fn set_policy(&self, sel: &Selector, p: PolicyPatch, actor: &Actor) -> Result<SecretMeta> {
        self.set_policy_inner(sel, p, actor)
    }
    fn grant(&self, g: GrantRequest, actor: &Actor) -> Result<Grant> {
        self.grant_inner(g, actor)
    }
    fn revoke(&self, grant_id: &str, actor: &Actor) -> Result<()> {
        self.revoke_inner(grant_id, actor)
    }
    fn disable(&self, sel: &Selector, actor: &Actor) -> Result<()> {
        self.set_status(sel, actor, SecretStatus::Disabled)
    }
    fn enable(&self, sel: &Selector, actor: &Actor) -> Result<()> {
        self.set_status(sel, actor, SecretStatus::Active)
    }
    fn destroy(&self, sel: &Selector, actor: &Actor) -> Result<()> {
        self.set_status(sel, actor, SecretStatus::Destroyed)
    }
    fn resolve(&self, sel: &Selector, use_: Use<'_>, who: &Requester) -> Result<Resolved> {
        self.resolve_inner(sel, use_, who)
    }
    fn reveal(&self, sel: &Selector, actor: &Actor, proof: &HumanProof) -> Result<SecretBytes> {
        self.reveal_inner(sel, actor, proof)
    }
    fn audit(&self, q: &AuditQuery, actor: &Actor) -> Result<Vec<AuditRow>> {
        self.audit_inner(q, actor)
    }
}

impl Drop for LocalVault {
    fn drop(&mut self) {
        let rows = self.rollups.lock().drain();
        if !rows.is_empty() {
            let _ = self.mutate(false, |_, _| Ok(((), rows)));
        }
    }
}
