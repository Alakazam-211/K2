//! Agent-to-agent secret handoff (`prd-agent-secret-handoff-v1`, as
//! amended by the vs-live H1–H23).
//!
//! An agent hands a vault secret to another agent **by name**. The value
//! moves vault → daemon memory → vault (same server), or sealed vault to
//! vault (cross server). It never enters a PTY, argv, env, a message body
//! or a transcript as part of the handoff.
//!
//! - This module: the vocabulary (items, the sealed payload), the sendable
//!   checks (§7.5, H20), the note check (H14), the landing shape (H11,
//!   H12), and the `k2-vault-handoff/1` seal/open (H5, H6).
//! - `vault/handoff_store.rs`: the four vault.db tables, the land-or-pend
//!   decision (§6.1), person decisions, the accept-from list, the peer
//!   vault cache and the expiry sweep.
//!
//! Sealing reuses the carry identity (age X25519) and the carry signing
//! key (Ed25519), with its own format string: a handoff never opens as a
//! carry and a carry never opens as a handoff (H6). The inner signature
//! is self-asserted; who sent it comes from the outer federation
//! signature, which the daemon checks, and `from.fingerprint` must equal
//! the peer that signature authenticated (H5).

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::agefmt::{self, Recipient};
use crate::carry::{ENGINE, SIGNING};
use crate::error::{Result, VaultError};
use crate::inject::Inject;
use crate::model::{Delivery, Kind, Namespace, ScopeKind, SecretStatus};
use crate::redact::Redactor;
use crate::secret::SecretBytes;
use crate::store::SecretMeta;
use crate::vault::LocalVault;

/// The sealed payload's format string (H6).
pub const FORMAT: &str = "k2-vault-handoff/1";
/// The feature a receiving server advertises in its signed vault-recipient
/// statement (H3).
pub const FEATURE: &str = "vault-handoff/1";
/// Advertised beside [`FEATURE`] by a server that understands signed
/// secrets (`inject.sign`, PRD vault-gaps item 2). An older server would
/// drop the signer and paste the value, so a signed secret isn't sent to it.
pub const FEATURE_SIGN: &str = "sign-v1";
/// A sealed handoff opens for ten minutes after it was made (§5.3).
pub const TRANSPORT_TTL_SECS: i64 = 600;
/// Values a handoff carries: SSH and TLS keys, kubeconfigs, service-account
/// JSON fit. The vault itself takes 1 MiB.
pub const MAX_VALUE_BYTES: usize = 64 * 1024;
/// A pending copy waits this long for a person, then expires (like secret
/// cards, `REQUEST_EXPIRY_DAYS`).
pub const PENDING_DAYS: i64 = 7;
/// `k2 vault send --note` limit.
pub const NOTE_MAX: usize = 200;
/// `k2 msg … --secret` carries the message text as the note (§4.2).
pub const MSG_NOTE_MAX: usize = 2000;
/// Pending caps (H17): per sender, and per receiving workspace.
pub const PENDING_PER_SENDER: usize = 5;
pub const PENDING_PER_WORKSPACE: usize = 25;
/// `--ttl` bounds on the landed copy.
pub const TTL_MIN_SECS: i64 = 3600;
pub const TTL_MAX_SECS: i64 = 90 * 24 * 3600;
/// `origin` of a landed copy: `handoff:<delivery id>`.
pub const ORIGIN_PREFIX: &str = "handoff:";
/// The system actor that lands copies (`created_by = system:handoff`, so
/// the receiving agent can't widen it, H12).
pub const SYSTEM_ACTOR: &str = "handoff";

/// Values shaped like a K2 credential never travel (H20): API keys, App
/// passes, remote-session grants and vault placeholders.
pub const CREDENTIAL_PREFIXES: &[&str] = &["k2sk_", "k2skn_", "k2rs_", "k2v_ph_"];

/// A new delivery id (`hd_<uuid>`).
pub fn new_delivery_id() -> String {
    format!("hd_{}", uuid::Uuid::new_v4().simple())
}

/// A new ask id (`ha_<uuid>`, §4.3).
pub fn new_ask_id() -> String {
    format!("ha_{}", uuid::Uuid::new_v4().simple())
}

/// `ha_` + 32 hex characters.
pub fn valid_ask_id(id: &str) -> bool {
    id.strip_prefix("ha_").is_some_and(|h| h.len() == 32 && h.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// `hd_` + 32 hex characters.
pub fn valid_delivery_id(id: &str) -> bool {
    id.strip_prefix("hd_").is_some_and(|h| h.len() == 32 && h.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Kinds that never travel (CL13 mirror): K2 login slots, App passes and
/// engine credentials.
pub fn never_sendable_kind(kind: &str) -> bool {
    crate::carry::never_carried_kind(kind)
}

/// Does the value look like a K2 credential by its prefix (H20)? The
/// daemon also refuses the owner token, live passports, Connect sessions
/// and the tunnel token by exact match.
pub fn credential_shaped(v: &[u8]) -> bool {
    let t = v.trim_ascii();
    CREDENTIAL_PREFIXES.iter().any(|p| t.starts_with(p.as_bytes()))
}

fn never(hint: impl Into<String>) -> VaultError {
    VaultError::new("never_sendable", 422, hint)
}

/// Can this secret be sent at all (§7.4, §7.5)? Workspace scope only in
/// v1: `server` scope answers `send_not_allowed` with the grant hint,
/// `session` scope and the `system` namespace are never sendable.
pub fn check_sendable(m: &SecretMeta) -> Result<()> {
    if m.namespace != Namespace::User {
        return Err(never("K2's own secrets never leave this server"));
    }
    if never_sendable_kind(m.kind.as_str()) {
        return Err(never(format!(
            "{} is a {}; K2 logins, App passes and engine credentials are never sent (they are made again where needed)",
            m.name, m.kind
        )));
    }
    match m.scope.kind {
        ScopeKind::Workspace => {}
        ScopeKind::Server => {
            return Err(VaultError::forbidden(
                "send_not_allowed",
                format!(
                    "{} is a server-wide secret; those aren't sent in this version. On this server, the owner shares it instead: k2 vault grant {} --to ws:<agent>",
                    m.name, m.name
                ),
            ))
        }
        _ => return Err(never(format!("{} belongs to one chat; chat secrets are never sent", m.name))),
    }
    if m.status != SecretStatus::Active {
        return Err(VaultError::conflict("secret_not_active", format!("{} is {}; enable it before sending", m.name, m.status)));
    }
    if let Some(x) = m.expires_at.as_deref().and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok()) {
        if x < chrono::Utc::now() {
            return Err(VaultError::conflict("secret_expired", format!("{} has expired", m.name)));
        }
    }
    Ok(())
}

/// The value itself (§5.3, H20): size, then K2-credential prefixes.
pub fn check_value(v: &[u8]) -> Result<()> {
    if v.is_empty() {
        return Err(VaultError::bad_request("empty_value", "the value is empty"));
    }
    if v.len() > MAX_VALUE_BYTES {
        return Err(VaultError::new("value_too_large", 422, "a handoff carries values up to 64 KiB"));
    }
    if credential_shaped(v) {
        return Err(never("that value is a K2 credential (an API key, App pass, session grant or placeholder); K2 credentials are never sent"));
    }
    Ok(())
}

/// The note (H14): at most `max` characters, single-spaced, and it must
/// not hold the value in any known encoding (raw, base64, hex, percent,
/// JSON-escaped). A note that does is refused, not redacted, so the sender
/// learns it tried to paste the key.
pub fn check_note(note: &str, max: usize, value: &[u8]) -> Result<String> {
    let n: String = note.trim().chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    if n.chars().count() > max {
        return Err(VaultError::bad_request("note_too_long", format!("notes are at most {max} characters")));
    }
    if !n.is_empty() && value.len() >= 4 {
        let r = Redactor::new([("value", value)]);
        if r.hits(n.as_bytes()) {
            return Err(VaultError::new(
                "note_has_value",
                422,
                "the note contains the secret's value; send the name only (K2 moves the value)",
            ));
        }
    }
    Ok(n)
}

/// What travels about a secret: everything but the value. Stored on the
/// `handoff` row as `item_json` and inside the sealed payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HandoffItem {
    /// The sender's name for it.
    pub name: String,
    /// `--as`: the name it lands under.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub as_name: Option<String>,
    pub kind: String,
    #[serde(default)]
    pub hosts: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inject: Option<Inject>,
    #[serde(default)]
    pub description: String,
    /// The sender's default delivery and ceiling.
    pub delivery: String,
    pub ceiling: String,
    /// A person gave it on the sending side (R3).
    pub human_given: bool,
    /// `--ttl` on the landed copy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_secs: Option<i64>,
}

impl HandoffItem {
    /// The item for a sender's secret.
    pub fn from_meta(m: &SecretMeta, as_name: Option<String>, ttl_secs: Option<i64>) -> HandoffItem {
        HandoffItem {
            name: m.name.clone(),
            as_name,
            kind: m.kind.as_str().to_string(),
            hosts: m.hosts.clone(),
            inject: m.inject.clone(),
            description: m.description.clone(),
            delivery: m.default_delivery.as_str().to_string(),
            ceiling: m.delivery_ceiling.as_str().to_string(),
            human_given: m.human_given,
            ttl_secs,
        }
    }

    /// Check what arrived (a peer's payload or a route body).
    pub fn validate(&self) -> Result<()> {
        crate::model::validate_name(&self.name, true)?;
        if let Some(a) = &self.as_name {
            crate::model::validate_name(a, true)?;
        }
        Kind::parse(&self.kind)?;
        if never_sendable_kind(&self.kind) {
            return Err(never(format!("a {} is never sent", self.kind)));
        }
        Delivery::parse(&self.delivery)?;
        Delivery::parse(&self.ceiling)?;
        if self.hosts.len() > 32 {
            return Err(VaultError::bad_request("too_many_hosts", "at most 32 sites"));
        }
        if let Some(i) = &self.inject {
            i.validate()?;
        }
        if let Some(t) = self.ttl_secs {
            if !(TTL_MIN_SECS..=TTL_MAX_SECS).contains(&t) {
                return Err(VaultError::bad_request("bad_ttl", "--ttl is 1 hour to 90 days"));
            }
        }
        if self.description.chars().count() > 500 {
            return Err(VaultError::bad_request("bad_description", "descriptions are at most 500 characters"));
        }
        Ok(())
    }

    pub fn kind(&self) -> Result<Kind> {
        Kind::parse(&self.kind)
    }

    /// The name it lands under.
    pub fn landed_name(&self) -> &str {
        self.as_name.as_deref().unwrap_or(&self.name)
    }

    /// Wildcard sites or body injection: only a person can land it, since
    /// a system `put` refuses both (H11).
    pub fn needs_person(&self) -> bool {
        self.hosts.iter().any(|h| h.contains("*.") || h.contains('*'))
            || self.inject.as_ref().is_some_and(|i| i.uses_body())
    }

    /// The landed copy's delivery and ceiling (H12): `fetch` with sites,
    /// else `file`; the ceiling is the lower of the sender's and `env`,
    /// never `api`; and the delivery never exceeds the ceiling. An ssh key
    /// the sender served only through the agent (item 3) lands as `agent`,
    /// so a handoff never makes it readable.
    pub fn landing_delivery(&self) -> (Delivery, Delivery) {
        let sender_ceiling = Delivery::parse(&self.ceiling).unwrap_or(Delivery::Fetch);
        let mut ceiling = if sender_ceiling.level() > Delivery::Env.level() { Delivery::Env } else { sender_ceiling };
        if ceiling == Delivery::None || ceiling == Delivery::Proxy {
            ceiling = Delivery::Fetch;
        }
        let mut delivery = if self.hosts.is_empty() { Delivery::File } else { Delivery::Fetch };
        if self.kind == Kind::SshKey.as_str() && Delivery::parse(&self.delivery).ok() == Some(Delivery::Agent) {
            delivery = Delivery::Agent;
        }
        if delivery.level() > ceiling.level() {
            delivery = ceiling;
        }
        (delivery, ceiling)
    }
}

/// One end of a sealed handoff.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Party {
    /// Federation fingerprint (SHA-256 of the SPKI).
    pub fingerprint: String,
    #[serde(default)]
    pub host: String,
    pub workspace_id: String,
    #[serde(default)]
    pub handle: String,
    /// `agent:<ws>/<sid>` | `owner` | `connect:<user>` (sender only).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub actor: String,
    pub vault_id: String,
}

/// `k2-vault-handoff/1`: what the receiving vault opens.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HandoffPayload {
    pub format: String,
    pub delivery_id: String,
    pub created_at: String,
    pub transport_expires_at: String,
    pub from: Party,
    pub to: Party,
    pub item: HandoffItem,
    pub value_b64: String,
    #[serde(default)]
    pub note: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    /// Hex Ed25519 key the inner signature is checked against (self-asserted).
    #[serde(default)]
    pub source_public_key: String,
}

impl Drop for HandoffPayload {
    fn drop(&mut self) {
        self.value_b64.zeroize();
    }
}

impl HandoffPayload {
    /// A payload to seal (the vault fills `format` and the signing key).
    pub fn new(delivery_id: &str, from: Party, to: Party, item: HandoffItem, value: &SecretBytes, note: String, reply_to: Option<String>) -> HandoffPayload {
        let now = chrono::Utc::now();
        HandoffPayload {
            format: FORMAT.into(),
            delivery_id: delivery_id.to_string(),
            created_at: now.to_rfc3339(),
            transport_expires_at: (now + chrono::Duration::seconds(TRANSPORT_TTL_SECS)).to_rfc3339(),
            from,
            to,
            item,
            value_b64: STANDARD.encode(value.expose()),
            note,
            reply_to,
            source_public_key: String::new(),
        }
    }

    pub fn value(&self) -> Result<SecretBytes> {
        STANDARD
            .decode(&self.value_b64)
            .map(SecretBytes::new)
            .map_err(|_| VaultError::bad_request("bad_handoff", "bad value encoding"))
    }
}

/// One row of the `handoff` table (§6.3): names, never a value.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HandoffRecord {
    pub delivery_id: String,
    /// `in` | `out` (`ask_in` | `ask_out` reserved for asks).
    pub direction: String,
    /// `pending` | `held` | `landed` | `declined` | `expired` | `unsent` |
    /// `unknown` | `refused`
    pub status: String,
    /// The receiving workspace (`in`) or the sending one (`out`).
    pub local_workspace_id: String,
    /// The other end: `sales` (same server) or `sales::rosson.k2.dev`.
    pub peer_addr: String,
    pub peer_fp: Option<String>,
    pub sender_actor: String,
    pub source_vault_id: Option<String>,
    pub name: String,
    pub landed_name: Option<String>,
    pub kind: String,
    pub note: String,
    pub reply_to: Option<String>,
    pub item: HandoffItem,
    pub secret_id: Option<String>,
    pub version: Option<u32>,
    /// Why it pends (`not_allowed`, `clash`, `person_given`, `needs_person`,
    /// `resend`, `remote_instruction_off`) or was refused (an error code).
    pub reason: Option<String>,
    /// The Thread card a pending copy posted: `<conversation>/<card id>`.
    pub card: Option<String>,
    pub created_at: String,
    pub decided_at: Option<String>,
    pub decided_by: Option<String>,
    /// Pending: when it expires. Landed: the copy's own expiry (`--ttl`).
    pub expires_at: Option<String>,
}

/// What a receiving daemon hands the vault (§5.1 step 4–6, §5.2 step 6).
/// The daemon decides `auto_land` from its gates (accept-from, H16
/// connection, a reply to an ask).
#[derive(Debug, Clone)]
pub struct Incoming {
    pub delivery_id: String,
    pub workspace_id: String,
    pub peer_addr: String,
    pub peer_fp: Option<String>,
    pub sender_actor: String,
    pub source_vault_id: Option<String>,
    pub item: HandoffItem,
    pub note: String,
    pub reply_to: Option<String>,
    /// The sender may land straight in (allow-listed, or a reply to this
    /// workspace's own ask).
    pub auto_land: bool,
    /// Cross server with the receiver's remote-instruct opt-in off (R4):
    /// always pending.
    pub remote_off: bool,
    /// Shown in the landed copy's description: `sales on rosson.k2.dev`.
    pub sender_label: String,
}

/// The receiving vault's answer.
#[derive(Debug, Clone)]
pub struct Received {
    pub record: HandoffRecord,
    /// The landed secret (status `landed`).
    pub meta: Option<SecretMeta>,
}

/// A person's decision on a pending copy (§4.5).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Decision {
    /// `accept` | `decline`
    pub action: String,
    /// Accept as another name.
    #[serde(default, rename = "as")]
    pub as_name: Option<String>,
    /// Accept over an existing secret of that name (a new version).
    #[serde(default)]
    pub replace: bool,
}

/// One accept-from entry (§7.3): `ws:<id>`, `agent::host` or `team:<id>`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AcceptEntry {
    pub workspace_id: String,
    pub sender: String,
    pub added_by: String,
    pub added_at: String,
}

/// A peer's vault, learned from its signed vault-recipient statement (H3).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PeerVault {
    pub peer_fp: String,
    pub vault_id: String,
    pub recipient: String,
    pub features: Vec<String>,
    pub fetched_at: String,
}

/// Normalise an accept-from sender: `ws:<id>`, `team:<id>`, or a federated
/// `agent::host` (lowercased; `agent@host` accepted).
pub fn normalise_sender(s: &str) -> Result<String> {
    let t = s.trim();
    if t == "*" || t.ends_with(":*") || t.starts_with("*::") {
        return Err(VaultError::bad_request("no_wildcard", "accept-from takes one sender at a time (no * in this version)"));
    }
    let ok_part = |p: &str| !p.is_empty() && p.len() <= 253 && p.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'));
    if let Some(id) = t.strip_prefix("ws:") {
        if ok_part(id) && !id.contains(':') {
            return Ok(format!("ws:{id}"));
        }
    } else if let Some(id) = t.strip_prefix("team:") {
        if ok_part(id) && !id.contains(':') {
            return Ok(format!("team:{id}"));
        }
    } else {
        let low = t.to_ascii_lowercase();
        let (a, h) = match low.split_once("::") {
            Some(x) => x,
            None => low.split_once('@').unwrap_or(("", "")),
        };
        if ok_part(a) && ok_part(h) && !a.contains(':') && !h.contains("::") {
            return Ok(format!("{a}::{h}"));
        }
    }
    Err(VaultError::bad_request("bad_sender", "a sender is ws:<workspace id>, team:<group id> or agent::host"))
}

#[derive(Serialize, Deserialize)]
struct Sealed {
    payload: String,
    sig: String,
}

/// What the opener knows from the outer, authenticated envelope.
#[derive(Debug, Clone, Copy)]
pub struct OpenExpect<'a> {
    /// This server's federation fingerprint.
    pub local_fingerprint: &'a str,
    /// The peer the federation signature authenticated (H5).
    pub peer_fingerprint: &'a str,
    /// The delivery id the outer envelope named.
    pub delivery_id: &'a str,
}

fn bad(hint: &str) -> VaultError {
    VaultError::bad_request("bad_handoff", hint.to_string())
}

impl LocalVault {
    /// Sign and seal a handoff to the receiving vault's recipient (H1: the
    /// value is sealed to the vault's age key, never to a federation key).
    pub fn handoff_seal(&self, target: &Recipient, payload: &mut HandoffPayload) -> Result<Vec<u8>> {
        payload.format = FORMAT.into();
        payload.from.vault_id = self.vault_id().to_string();
        payload.source_public_key = crate::crypto::hex(&self.engine_public_key(ENGINE, SIGNING)?);
        if !valid_delivery_id(&payload.delivery_id) {
            return Err(bad("delivery ids are hd_<uuid>"));
        }
        payload.item.validate()?;
        check_value(payload.value()?.expose())?;
        let mut p = serde_json::to_string(&*payload).map_err(|_| VaultError::internal("handoff json"))?;
        let sig = self.engine_sign(ENGINE, SIGNING, p.as_bytes())?;
        let sealed = Sealed { payload: p.clone(), sig: crate::crypto::hex(&sig) };
        let mut body = serde_json::to_vec(&sealed).map_err(|_| VaultError::internal("handoff json"))?;
        p.zeroize();
        let out = agefmt::encrypt(&[*target], &body);
        body.zeroize();
        Ok(out)
    }

    /// Open a sealed handoff made for this vault (§5.2 step 3): decrypt
    /// with the carry identity; the format, this vault id, this server's
    /// fingerprint, the authenticated peer, the outer delivery id, the
    /// transport expiry and the inner signature must all match.
    pub fn handoff_open(&self, file: &[u8], expect: OpenExpect<'_>) -> Result<HandoffPayload> {
        let id = self.carry_identity()?;
        let raw = agefmt::decrypt(&id, file)?;
        let sealed: Sealed = serde_json::from_slice(raw.expose()).map_err(|_| bad("not a sealed handoff"))?;
        let payload: HandoffPayload = serde_json::from_str(&sealed.payload).map_err(|_| bad("not a sealed handoff"))?;
        if payload.format != FORMAT {
            return Err(bad("unknown handoff format"));
        }
        let pk: [u8; 32] = crate::crypto::unhex(&payload.source_public_key)
            .and_then(|v| v.try_into().ok())
            .ok_or_else(|| bad("bad source key"))?;
        let sig: [u8; 64] = crate::crypto::unhex(&sealed.sig).and_then(|v| v.try_into().ok()).ok_or_else(|| bad("bad signature"))?;
        if !crate::engine::verify(&pk, sealed.payload.as_bytes(), &sig) {
            return Err(VaultError::tamper("the handoff's signature doesn't match"));
        }
        if payload.to.vault_id != self.vault_id() {
            return Err(VaultError::forbidden("carry_not_for_this_vault", "this handoff was sealed for another vault"));
        }
        if expect.local_fingerprint.is_empty() || payload.to.fingerprint != expect.local_fingerprint {
            return Err(VaultError::forbidden("handoff_not_for_this_server", "this handoff was made for another server"));
        }
        if expect.peer_fingerprint.is_empty() || payload.from.fingerprint != expect.peer_fingerprint {
            return Err(VaultError::forbidden("handoff_sender_mismatch", "the sealed sender isn't the server that signed the envelope"));
        }
        if payload.delivery_id != expect.delivery_id || !valid_delivery_id(&payload.delivery_id) {
            return Err(bad("the delivery id doesn't match the envelope"));
        }
        let exp = chrono::DateTime::parse_from_rfc3339(&payload.transport_expires_at).map_err(|_| bad("bad transport expiry"))?;
        if chrono::Utc::now() > exp {
            return Err(VaultError::forbidden("handoff_expired", "this sealed handoff expired in transit (they open for 10 minutes)"));
        }
        payload.item.validate()?;
        check_value(payload.value()?.expose())?;
        Ok(payload)
    }
}

#[cfg(test)]
#[path = "handoff_tests.rs"]
mod tests;
