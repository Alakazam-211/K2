//! Signed address notices: when this box's public address changes (a K2
//! Connect subdomain switch, or a new advertised LAN/Tailscale URL), every
//! paired peer learns the new address without a human.
//!
//! Peers pin our federation KEY ([`super::identity`]); the address is only a
//! routing hint (`FederationPeer::subdomain` / `base_url`). A notice is
//! signed with the identity key and verified by the receiver against the key
//! it ALREADY pinned for that fingerprint, so it can only ever move the
//! address of the peer whose key signed it. It never changes a key, a trust
//! state, a capability or another peer's row.
//!
//! ## Wire (`POST /cli/federation/address`, JSON)
//!
//! ```json
//! { "notice": { "v": 1, "fingerprint": "<sha256 spki hex>",
//!               "old_address": { "subdomain": "afs", "base_url": "" },
//!               "new_address": { "subdomain": "k2n1-m4", "base_url": "" },
//!               "issued_at": "<RFC 3339>", "seq": 1760000000000 },
//!   "sig": "<base64 ASN.1 ECDSA P-256 over NOTICE_SIG_PREFIX + canonical JSON of notice>" }
//! ```
//!
//! ## Receiver rules ([`apply`])
//!
//! unknown fingerprint → 403 `unknown_peer` (nothing changes); not Trusted →
//! 403; bad signature → 403; `issued_at` older than 7 days → 409
//! `stale_notice`, more than 5 min ahead → 400 `issued_in_future`; `seq` at or
//! below the last accepted seq from that peer → 409 `replay`. Then, per field
//! that changed (`old != new`, `new` non-empty): the stored value is updated
//! when it equals `old`; left alone when empty (that peer does not route by
//! that field) or already `new`; a stored value that is neither is a
//! conflict. Only conflicts and no update → 409 `address_mismatch`.
//!
//! ## Sender ([`observe_and_issue`], [`due`], [`record_outcome`])
//!
//! A small persisted outbox, `~/.k2/federation/address-notices.json` (0600),
//! one row per peer: what the peer is believed to know (`peer_known`), the
//! address to tell it (`target`), the `seq`, status and retry schedule. A
//! notice is signed at send time from the row, so a delivery that lands
//! while a newer change is pending still leaves a consistent `old_address`
//! for the next one. Retry with backoff (30 s doubling, 2 h cap) until a 2xx,
//! up to 7 days, then `failed` (kept, visible in `k2 fed peers`). A 404 means
//! the peer has no such route (a peer older than 0.45.2, or its federation is
//! off): recorded as failed `peer_too_old`, never retried.
//!
//! `seq` is per box, persisted, and never below the current Unix time in
//! milliseconds, so a lost state file cannot make new notices look like
//! replays to peers.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::{
    EcdsaKeyPair, UnparsedPublicKey, ECDSA_P256_SHA256_ASN1, ECDSA_P256_SHA256_ASN1_SIGNING,
};
use base64::Engine;
use chrono::{DateTime, Duration, Utc};
use rcgen::KeyPair;
use serde::{Deserialize, Serialize};

use super::peers::{
    fingerprint_of_spki_der, fingerprint_of_spki_pem, raw_point_from_spki_der, spki_pem_to_der,
    PeerStore, PeerTrust,
};

/// Notice wire version.
pub const NOTICE_VERSION: u32 = 1;
/// Signature domain separation: these bytes precede the canonical JSON.
pub const NOTICE_SIG_PREFIX: &str = "k2-federation-address-notice-v1\n";
/// A notice older than this is refused by the receiver, and the sender
/// stops retrying a row after the same span.
pub const MAX_AGE_DAYS: i64 = 7;
/// How far in the future `issued_at` may be (clock skew).
pub const MAX_FUTURE_SECS: i64 = 300;
/// The receiver route.
pub const ROUTE: &str = "/cli/federation/address";

const BACKOFF_BASE_SECS: i64 = 30;
const BACKOFF_CAP_SECS: i64 = 2 * 60 * 60;
const STATE_VERSION: u32 = 1;

/// A routing address, the same two fields a pinned peer row stores.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerAddress {
    #[serde(default)]
    pub subdomain: String,
    #[serde(default)]
    pub base_url: String,
}

impl PeerAddress {
    pub fn new(subdomain: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self {
            subdomain: subdomain.into().trim().to_string(),
            base_url: super::dial::trim_base(&base_url.into()),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.subdomain.is_empty() && self.base_url.is_empty()
    }

    /// Same address after normalizing spelling (case, `.k2.dev`, scheme,
    /// trailing slash).
    pub fn same_as(&self, other: &PeerAddress) -> bool {
        same_subdomain(&self.subdomain, &other.subdomain) && same_base(&self.base_url, &other.base_url)
    }
}

/// The signed body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddressNotice {
    pub v: u32,
    pub fingerprint: String,
    pub old_address: PeerAddress,
    pub new_address: PeerAddress,
    /// RFC 3339.
    pub issued_at: String,
    pub seq: u64,
}

/// The wire envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedAddressNotice {
    pub notice: AddressNotice,
    pub sig: String,
}

fn signed_bytes(notice: &AddressNotice) -> Result<Vec<u8>, String> {
    let json = serde_json::to_vec(notice).map_err(|e| format!("encode notice: {e}"))?;
    let mut out = NOTICE_SIG_PREFIX.as_bytes().to_vec();
    out.extend_from_slice(&json);
    Ok(out)
}

/// Sign `notice` with `key`. The notice's `fingerprint` must be `key`'s.
pub fn sign(notice: &AddressNotice, key: &KeyPair) -> Result<SignedAddressNotice, String> {
    let fp = fingerprint_of_spki_der(&key.public_key_der());
    if notice.fingerprint != fp {
        return Err(format!(
            "notice names fingerprint {} but the signing key is {fp}",
            notice.fingerprint
        ));
    }
    let bytes = signed_bytes(notice)?;
    let signing = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &key.serialize_der())
        .map_err(|e| format!("load signing key: {e}"))?;
    let sig = signing
        .sign(&SystemRandom::new(), &bytes)
        .map_err(|e| format!("sign notice: {e}"))?;
    Ok(SignedAddressNotice {
        notice: notice.clone(),
        sig: base64::engine::general_purpose::STANDARD.encode(sig.as_ref()),
    })
}

/// Verify `signed` against a pinned SPKI PEM. Checks that the claimed
/// fingerprint is the pinned key's, then the signature.
pub fn verify(signed: &SignedAddressNotice, pinned_pem: &str) -> Result<(), NoticeError> {
    let pinned_fp = fingerprint_of_spki_pem(pinned_pem).map_err(NoticeError::Key)?;
    if signed.notice.fingerprint != pinned_fp {
        return Err(NoticeError::BadSignature);
    }
    let der = spki_pem_to_der(pinned_pem).map_err(NoticeError::Key)?;
    let point = raw_point_from_spki_der(&der).map_err(NoticeError::Key)?;
    let sig = base64::engine::general_purpose::STANDARD
        .decode(signed.sig.as_bytes())
        .map_err(|_| NoticeError::BadSignature)?;
    let bytes = signed_bytes(&signed.notice).map_err(NoticeError::Decode)?;
    UnparsedPublicKey::new(&ECDSA_P256_SHA256_ASN1, point)
        .verify(&bytes, &sig)
        .map_err(|_| NoticeError::BadSignature)
}

/// Why a notice was refused. Every variant changes nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoticeError {
    Decode(String),
    UnsupportedVersion(u32),
    UnknownPeer,
    NotTrusted,
    Key(String),
    BadSignature,
    BadTime(String),
    Stale,
    InFuture,
    Replay { seq: u64, last: u64 },
    AddressMismatch,
    Store(String),
}

impl NoticeError {
    /// HTTP status line for the receiver route.
    pub fn status(&self) -> &'static str {
        match self {
            Self::Decode(_) | Self::UnsupportedVersion(_) | Self::BadTime(_) | Self::InFuture => {
                "400 Bad Request"
            }
            Self::UnknownPeer | Self::NotTrusted | Self::Key(_) | Self::BadSignature => "403 Forbidden",
            Self::Stale | Self::Replay { .. } | Self::AddressMismatch => "409 Conflict",
            Self::Store(_) => "500 Internal Server Error",
        }
    }

    /// Stable machine code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Decode(_) => "bad_notice",
            Self::UnsupportedVersion(_) => "unsupported_version",
            Self::UnknownPeer => "unknown_peer",
            Self::NotTrusted => "not_trusted",
            Self::Key(_) | Self::BadSignature => "bad_signature",
            Self::BadTime(_) => "bad_issued_at",
            Self::Stale => "stale_notice",
            Self::InFuture => "issued_in_future",
            Self::Replay { .. } => "replay",
            Self::AddressMismatch => "address_mismatch",
            Self::Store(_) => "store_error",
        }
    }
}

impl std::fmt::Display for NoticeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Decode(e) => write!(f, "malformed address notice: {e}"),
            Self::UnsupportedVersion(v) => write!(f, "unsupported address notice version {v}"),
            Self::UnknownPeer => write!(f, "no peer is pinned with that fingerprint"),
            Self::NotTrusted => write!(f, "that peer is not trusted"),
            Self::Key(e) => write!(f, "pinned key: {e}"),
            Self::BadSignature => write!(f, "signature does not verify against the pinned key"),
            Self::BadTime(e) => write!(f, "issued_at is not RFC 3339: {e}"),
            Self::Stale => write!(f, "notice is older than {MAX_AGE_DAYS} days"),
            Self::InFuture => write!(f, "notice is issued more than {MAX_FUTURE_SECS}s in the future"),
            Self::Replay { seq, last } => {
                write!(f, "seq {seq} is not newer than the last accepted seq {last}")
            }
            Self::AddressMismatch => write!(
                f,
                "this server stores a different address for that peer than the notice's old address"
            ),
            Self::Store(e) => write!(f, "peer store: {e}"),
        }
    }
}

impl std::error::Error for NoticeError {}

/// What an accepted notice did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    pub fingerprint: String,
    pub seq: u64,
    /// Fields that were rewritten (`"subdomain"`, `"base_url"`).
    pub changed: Vec<&'static str>,
    /// Fields that were left alone because the stored value differs from
    /// both the old and the new address.
    pub conflicts: Vec<&'static str>,
    pub previous: PeerAddress,
    pub current: PeerAddress,
    /// The row changed (address and/or accepted seq) and must be saved.
    pub wrote: bool,
}

fn label_of(s: &str) -> String {
    let h = crate::connections::normalize_remote_host(&super::dial::host_port(s));
    h.strip_suffix(".k2.dev").unwrap_or(&h).to_string()
}

fn same_subdomain(a: &str, b: &str) -> bool {
    label_of(a) == label_of(b)
}

fn same_base(a: &str, b: &str) -> bool {
    super::dial::trim_base(a).to_ascii_lowercase() == super::dial::trim_base(b).to_ascii_lowercase()
}

enum FieldStep {
    Update,
    Keep,
    Conflict,
}

fn field_step(stored: &str, old: &str, new: &str, same: fn(&str, &str) -> bool) -> FieldStep {
    if new.trim().is_empty() || same(old, new) || stored.trim().is_empty() || same(stored, new) {
        return FieldStep::Keep;
    }
    if same(stored, old) {
        FieldStep::Update
    } else {
        FieldStep::Conflict
    }
}

/// Receiver: verify `body` and apply it to `store` (the caller saves the
/// store on `Ok`). Pure apart from the store mutation.
pub fn apply(store: &mut PeerStore, body: &[u8], now: DateTime<Utc>) -> Result<Applied, NoticeError> {
    let signed: SignedAddressNotice =
        serde_json::from_slice(body).map_err(|e| NoticeError::Decode(e.to_string()))?;
    let n = &signed.notice;
    if n.v != NOTICE_VERSION {
        return Err(NoticeError::UnsupportedVersion(n.v));
    }
    // The key comes ONLY from the row already pinned for this fingerprint.
    let peer = store.get(&n.fingerprint).ok_or(NoticeError::UnknownPeer)?.clone();
    verify(&signed, &peer.public_key_pem)?;
    if peer.trust != PeerTrust::Trusted {
        return Err(NoticeError::NotTrusted);
    }
    let issued = DateTime::parse_from_rfc3339(n.issued_at.trim())
        .map_err(|e| NoticeError::BadTime(e.to_string()))?
        .with_timezone(&Utc);
    if issued > now + Duration::seconds(MAX_FUTURE_SECS) {
        return Err(NoticeError::InFuture);
    }
    if issued < now - Duration::days(MAX_AGE_DAYS) {
        return Err(NoticeError::Stale);
    }
    if n.seq <= peer.address_seq {
        return Err(NoticeError::Replay { seq: n.seq, last: peer.address_seq });
    }

    let previous = PeerAddress { subdomain: peer.subdomain.clone(), base_url: peer.base_url.clone() };
    let mut current = previous.clone();
    let mut changed = Vec::new();
    let mut conflicts = Vec::new();
    match field_step(&peer.subdomain, &n.old_address.subdomain, &n.new_address.subdomain, same_subdomain) {
        FieldStep::Update => {
            current.subdomain = n.new_address.subdomain.trim().to_string();
            changed.push("subdomain");
        }
        FieldStep::Conflict => conflicts.push("subdomain"),
        FieldStep::Keep => {}
    }
    match field_step(&peer.base_url, &n.old_address.base_url, &n.new_address.base_url, same_base) {
        FieldStep::Update => {
            current.base_url = super::dial::trim_base(&n.new_address.base_url);
            changed.push("base_url");
        }
        FieldStep::Conflict => conflicts.push("base_url"),
        FieldStep::Keep => {}
    }
    if changed.is_empty() && !conflicts.is_empty() {
        return Err(NoticeError::AddressMismatch);
    }

    // Nothing to change (the peer already has this address): a no-op that
    // writes nothing, not even the seq.
    let wrote = !changed.is_empty();
    if wrote {
        let row = store.get_mut(&n.fingerprint).ok_or(NoticeError::UnknownPeer)?;
        row.subdomain = current.subdomain.clone();
        row.base_url = current.base_url.clone();
        row.address_seq = n.seq;
        row.address_updated_at = Some(now);
    }
    Ok(Applied {
        fingerprint: n.fingerprint.clone(),
        seq: n.seq,
        changed,
        conflicts,
        previous,
        current,
        wrote,
    })
}

// ─────────────────────────────────────────────────────────────────────
// Sender: the persisted per-peer outbox
// ─────────────────────────────────────────────────────────────────────

/// `~/.k2/federation/address-notices.json`.
pub fn state_path() -> PathBuf {
    super::identity::identity_dir().join("address-notices.json")
}

/// Delivery status of one peer's row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeStatus {
    Pending,
    Delivered,
    Failed,
}

/// One peer's row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoticeRow {
    /// The address this peer is believed to store for us.
    pub peer_known: PeerAddress,
    /// The address to tell it.
    pub target: PeerAddress,
    pub seq: u64,
    /// When this change was issued (the notice's `issued_at`).
    pub issued_at: DateTime<Utc>,
    pub status: NoticeStatus,
    pub attempts: u32,
    pub next_attempt_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_attempt_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// Machine reason for a `failed` row (`peer_too_old`, `gave_up`, the
    /// receiver's error code, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivered_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct NoticeState {
    version: u32,
    last_seq: u64,
    /// The address this box last announced (or first observed).
    #[serde(default)]
    announced: Option<PeerAddress>,
    #[serde(default)]
    peers: BTreeMap<String, NoticeRow>,
}

static STATE_LOCK: Mutex<()> = Mutex::new(());

fn load_state() -> Result<NoticeState, String> {
    let path = state_path();
    if !path.exists() {
        return Ok(NoticeState { version: STATE_VERSION, ..Default::default() });
    }
    let raw = fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let st: NoticeState =
        serde_json::from_str(&raw).map_err(|e| format!("parse {}: {e}", path.display()))?;
    if st.version != STATE_VERSION {
        return Err(format!("{} has unknown version {}", path.display(), st.version));
    }
    Ok(st)
}

fn save_state(st: &NoticeState) -> Result<(), String> {
    let dir = super::identity::identity_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
    }
    let path = state_path();
    let tmp = dir.join(format!("address-notices.json.tmp.{}", uuid::Uuid::new_v4()));
    let json = serde_json::to_vec_pretty(st).map_err(|e| format!("encode notice state: {e}"))?;
    fs::write(&tmp, json).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("chmod {}: {e}", tmp.display()))?;
    }
    fs::rename(&tmp, &path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("rename into place {}: {e}", path.display())
    })
}

/// This box's address as it stands now: the K2 Connect subdomain from
/// `tunnel.json` and the advertised LAN/Tailscale base URL. Either may be
/// empty.
pub fn local_address() -> PeerAddress {
    let subdomain = crate::tunnel::config::load()
        .ok()
        .filter(|c| !c.token.trim().is_empty())
        .map(|c| c.subdomain)
        .unwrap_or_default();
    PeerAddress::new(subdomain, super::dial::advertised_federation_base())
}

/// What [`observe_and_issue`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IssueOutcome {
    /// First observation: recorded as the baseline, nothing sent.
    Baseline(PeerAddress),
    /// No change since the last announcement.
    Unchanged,
    /// A new notice for these peers.
    Issued { seq: u64, address: PeerAddress, peers: Vec<String> },
}

/// Compare `current` with the last announced address; on a change, issue a
/// notice (new seq) to every Trusted peer in `store`. An empty field in
/// `current` never replaces a known one (losing the tunnel or the LAN
/// address is not announced).
pub fn observe_and_issue(
    current: &PeerAddress,
    store: &PeerStore,
    now: DateTime<Utc>,
) -> Result<IssueOutcome, String> {
    let _g = STATE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut st = load_state()?;
    let announced = match &st.announced {
        None => {
            if current.is_empty() {
                return Ok(IssueOutcome::Unchanged);
            }
            st.announced = Some(current.clone());
            save_state(&st)?;
            return Ok(IssueOutcome::Baseline(current.clone()));
        }
        Some(a) => a.clone(),
    };
    let merged = PeerAddress {
        subdomain: if current.subdomain.is_empty() { announced.subdomain.clone() } else { current.subdomain.clone() },
        base_url: if current.base_url.is_empty() { announced.base_url.clone() } else { current.base_url.clone() },
    };
    if merged.same_as(&announced) {
        return Ok(IssueOutcome::Unchanged);
    }
    let seq = st
        .last_seq
        .saturating_add(1)
        .max(u64::try_from(now.timestamp_millis()).unwrap_or(0));
    st.last_seq = seq;
    st.announced = Some(merged.clone());
    let mut peers = Vec::new();
    for p in store.list().iter().filter(|p| p.trust == PeerTrust::Trusted) {
        let peer_known = match st.peers.get(&p.fingerprint) {
            Some(row) => row.peer_known.clone(),
            None => announced.clone(),
        };
        if peer_known.same_as(&merged) {
            // Switched back before the peer heard about the move.
            st.peers.remove(&p.fingerprint);
            continue;
        }
        st.peers.insert(
            p.fingerprint.clone(),
            NoticeRow {
                peer_known,
                target: merged.clone(),
                seq,
                issued_at: now,
                status: NoticeStatus::Pending,
                attempts: 0,
                next_attempt_at: now,
                last_attempt_at: None,
                last_error: None,
                failed_reason: None,
                delivered_at: None,
            },
        );
        peers.push(p.fingerprint.clone());
    }
    save_state(&st)?;
    Ok(IssueOutcome::Issued { seq, address: merged, peers })
}

/// A pending row that is due, with its notice signed now.
#[derive(Debug, Clone)]
pub struct DueNotice {
    pub fingerprint: String,
    pub seq: u64,
    pub target: PeerAddress,
    pub body: Vec<u8>,
}

/// Rows due for a send at `now`, each signed with `key`. Rows past
/// [`MAX_AGE_DAYS`] are marked failed (`gave_up`) instead.
pub fn due(key: &KeyPair, now: DateTime<Utc>) -> Result<Vec<DueNotice>, String> {
    let _g = STATE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut st = load_state()?;
    let my_fp = fingerprint_of_spki_der(&key.public_key_der());
    let mut out = Vec::new();
    let mut dirty = false;
    for (fp, row) in st.peers.iter_mut() {
        if row.status != NoticeStatus::Pending {
            continue;
        }
        if now - row.issued_at >= Duration::days(MAX_AGE_DAYS) {
            row.status = NoticeStatus::Failed;
            row.failed_reason = Some("gave_up".to_string());
            row.last_error = Some(format!(
                "no answer from the peer within {MAX_AGE_DAYS} days{}",
                row.last_error.as_deref().map(|e| format!(" (last error: {e})")).unwrap_or_default()
            ));
            dirty = true;
            continue;
        }
        if row.next_attempt_at > now {
            continue;
        }
        let notice = AddressNotice {
            v: NOTICE_VERSION,
            fingerprint: my_fp.clone(),
            old_address: row.peer_known.clone(),
            new_address: row.target.clone(),
            issued_at: row.issued_at.to_rfc3339(),
            seq: row.seq,
        };
        let signed = sign(&notice, key)?;
        let body = serde_json::to_vec(&signed).map_err(|e| format!("encode notice: {e}"))?;
        out.push(DueNotice { fingerprint: fp.clone(), seq: row.seq, target: row.target.clone(), body });
    }
    if dirty {
        save_state(&st)?;
    }
    Ok(out)
}

/// The result of one send attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendResult {
    /// The peer answered with this HTTP status and body.
    Http(u16, String),
    /// No answer (connect error, timeout, a refused dial).
    Transport(String),
}

/// How a send attempt is classified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryOutcome {
    Delivered,
    Retry(String),
    Failed { reason: String, detail: String },
}

/// Classify a [`SendResult`].
pub fn classify(res: &SendResult) -> DeliveryOutcome {
    match res {
        SendResult::Transport(e) => DeliveryOutcome::Retry(e.clone()),
        SendResult::Http(s, _) if (200..300).contains(s) => DeliveryOutcome::Delivered,
        SendResult::Http(404, body) => DeliveryOutcome::Failed {
            reason: "peer_too_old".to_string(),
            detail: format!(
                "the peer answered 404: it has no address-notice route (older than 0.45.2) \
                 or its federation is off ({})",
                body.trim()
            ),
        },
        SendResult::Http(s, body) if (400..500).contains(s) && *s != 408 && *s != 429 => {
            let code = serde_json::from_str::<serde_json::Value>(body)
                .ok()
                .and_then(|v| v.get("code").and_then(|c| c.as_str()).map(str::to_string))
                .unwrap_or_else(|| format!("http_{s}"));
            if code == "replay" {
                // The peer already holds this seq or a newer one (a lost
                // response): it knows.
                DeliveryOutcome::Delivered
            } else {
                DeliveryOutcome::Failed { reason: code, detail: format!("HTTP {s}: {}", body.trim()) }
            }
        }
        SendResult::Http(s, body) => DeliveryOutcome::Retry(format!("HTTP {s}: {}", body.trim())),
    }
}

fn backoff(attempts: u32) -> Duration {
    let n = attempts.max(1) - 1;
    let secs = if n >= 20 { BACKOFF_CAP_SECS } else { (BACKOFF_BASE_SECS << n).min(BACKOFF_CAP_SECS) };
    Duration::seconds(secs)
}

/// Record a send attempt for `fp` that carried `seq` / `target`.
pub fn record_outcome(
    fp: &str,
    seq: u64,
    target: &PeerAddress,
    outcome: &DeliveryOutcome,
    now: DateTime<Utc>,
) -> Result<(), String> {
    let _g = STATE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut st = load_state()?;
    let Some(row) = st.peers.get_mut(fp) else {
        return Ok(()); // the peer row was dropped meanwhile
    };
    let current_seq = row.seq == seq;
    match outcome {
        DeliveryOutcome::Delivered => {
            row.peer_known = target.clone();
            row.last_attempt_at = Some(now);
            if current_seq {
                row.status = NoticeStatus::Delivered;
                row.delivered_at = Some(now);
                row.last_error = None;
                row.failed_reason = None;
            }
        }
        _ if !current_seq => {} // a newer notice replaced this one
        DeliveryOutcome::Retry(e) => {
            row.attempts = row.attempts.saturating_add(1);
            row.last_attempt_at = Some(now);
            row.last_error = Some(e.clone());
            row.next_attempt_at = now + backoff(row.attempts);
        }
        DeliveryOutcome::Failed { reason, detail } => {
            row.attempts = row.attempts.saturating_add(1);
            row.last_attempt_at = Some(now);
            row.status = NoticeStatus::Failed;
            row.failed_reason = Some(reason.clone());
            row.last_error = Some(detail.clone());
        }
    }
    save_state(&st)
}

/// Every peer's row (for `k2 fed peers`).
pub fn rows() -> Result<BTreeMap<String, NoticeRow>, String> {
    let _g = STATE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    Ok(load_state()?.peers)
}

/// The address this box last announced.
pub fn announced() -> Result<Option<PeerAddress>, String> {
    let _g = STATE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    Ok(load_state()?.announced)
}

/// Make every pending row due now (a fresh trigger such as an identity
/// change or the peer proving it is online).
pub fn make_due_now(now: DateTime<Utc>) -> Result<(), String> {
    let _g = STATE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut st = load_state()?;
    let mut dirty = false;
    for row in st.peers.values_mut() {
        if row.status == NoticeStatus::Pending && row.next_attempt_at > now {
            row.next_attempt_at = now;
            dirty = true;
        }
    }
    if dirty {
        save_state(&st)?;
    }
    Ok(())
}

/// One drain pass: send every due row through `send` and record the result.
/// No sends while air-gap is on. Returns `(fingerprint, outcome)` per send.
pub fn drain_with<F>(key: &KeyPair, store: &PeerStore, now: DateTime<Utc>, mut send: F) -> Result<Vec<(String, DeliveryOutcome)>, String>
where
    F: FnMut(&super::FederationPeer, &[u8]) -> SendResult,
{
    if crate::airgap::enabled() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for d in due(key, now)? {
        let outcome = match store.get(&d.fingerprint) {
            Some(peer) if peer.trust == PeerTrust::Trusted => classify(&send(peer, &d.body)),
            Some(_) => DeliveryOutcome::Failed {
                reason: "not_trusted".to_string(),
                detail: "this server no longer trusts that peer".to_string(),
            },
            None => DeliveryOutcome::Failed {
                reason: "unpaired".to_string(),
                detail: "the peer is no longer pinned on this server".to_string(),
            },
        };
        record_outcome(&d.fingerprint, d.seq, &d.target, &outcome, now)?;
        out.push((d.fingerprint, outcome));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::federation::FederationPeer;
    use crate::tunnel::test_support::with_temp_home;

    fn keypair() -> KeyPair {
        KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("gen key")
    }

    fn fp(k: &KeyPair) -> String {
        fingerprint_of_spki_der(&k.public_key_der())
    }

    fn trusted(store: &mut PeerStore, k: &KeyPair, sub: &str, base: &str) -> String {
        let peer = FederationPeer::pin("peer", sub, k.public_key_pem()).expect("pin").with_base_url(base);
        let f = store.upsert(peer);
        store.set_trust(&f, PeerTrust::Trusted);
        store.grant(&f, "inbound");
        f
    }

    fn notice(k: &KeyPair, old: (&str, &str), new: (&str, &str), seq: u64, at: DateTime<Utc>) -> AddressNotice {
        AddressNotice {
            v: NOTICE_VERSION,
            fingerprint: fp(k),
            old_address: PeerAddress::new(old.0, old.1),
            new_address: PeerAddress::new(new.0, new.1),
            issued_at: at.to_rfc3339(),
            seq,
        }
    }

    fn body(s: &SignedAddressNotice) -> Vec<u8> {
        serde_json::to_vec(s).expect("encode")
    }

    #[test]
    fn sign_then_verify_round_trips() {
        let k = keypair();
        let n = notice(&k, ("afs", ""), ("k2n1-m4", ""), 7, Utc::now());
        let s = sign(&n, &k).expect("sign");
        verify(&s, &k.public_key_pem()).expect("verify");
        // Through the wire encoding too.
        let back: SignedAddressNotice = serde_json::from_slice(&body(&s)).expect("decode");
        verify(&back, &k.public_key_pem()).expect("verify after round trip");
        // Tampering breaks it.
        let mut t = back.clone();
        t.notice.new_address.subdomain = "evil".to_string();
        assert_eq!(verify(&t, &k.public_key_pem()), Err(NoticeError::BadSignature));
    }

    #[test]
    fn sign_refuses_a_fingerprint_that_is_not_the_key() {
        let (a, b) = (keypair(), keypair());
        let mut n = notice(&a, ("afs", ""), ("x", ""), 1, Utc::now());
        n.fingerprint = fp(&b);
        assert!(sign(&n, &a).is_err());
    }

    #[test]
    fn apply_updates_only_the_signing_peer_address() {
        let k = keypair();
        let other = keypair();
        let mut store = PeerStore::default();
        let f = trusted(&mut store, &k, "afs", "");
        let fo = trusted(&mut store, &other, "zed", "http://192.168.1.9:38471");
        let before = store.get(&f).expect("peer").clone();
        let other_before = store.get(&fo).expect("other").clone();
        let now = Utc::now();
        let s = sign(&notice(&k, ("afs", ""), ("k2n1-m4", ""), 10, now), &k).expect("sign");

        let applied = apply(&mut store, &body(&s), now).expect("apply");
        assert_eq!(applied.changed, vec!["subdomain"]);
        let after = store.get(&f).expect("peer");
        assert_eq!(after.subdomain, "k2n1-m4");
        assert_eq!(after.address_seq, 10);
        assert_eq!(after.address_updated_at, Some(now));
        // Never a key, trust, capability or epoch.
        assert_eq!(after.public_key_pem, before.public_key_pem);
        assert_eq!(after.trust, before.trust);
        assert_eq!(after.capabilities, before.capabilities);
        assert_eq!(after.epoch, before.epoch);
        assert_eq!(after.base_url, before.base_url);
        // Another peer is untouched.
        assert_eq!(store.get(&fo).expect("other"), &other_before);
    }

    #[test]
    fn a_notice_that_changes_nothing_writes_nothing() {
        let k = keypair();
        let mut store = PeerStore::default();
        let f = trusted(&mut store, &k, "k2n1-m4", "");
        let before = store.get(&f).expect("peer").clone();
        // The peer already stores the new address.
        let s = sign(&notice(&k, ("afs", ""), ("k2n1-m4", ""), 7, Utc::now()), &k).expect("sign");
        let a = apply(&mut store, &body(&s), Utc::now()).expect("no-op is accepted");
        assert!(!a.wrote, "{a:?}");
        assert!(a.changed.is_empty());
        assert_eq!(store.get(&f).expect("peer"), &before, "not even the seq moves");
    }

    #[test]
    fn a_wrong_key_is_rejected_and_changes_nothing() {
        let k = keypair();
        let attacker = keypair();
        let mut store = PeerStore::default();
        let f = trusted(&mut store, &k, "afs", "");
        let snapshot = store.get(&f).expect("peer").clone();
        // The attacker claims k's fingerprint but signs with its own key.
        let n = notice(&k, ("afs", ""), ("evil", ""), 5, Utc::now());
        let bytes = signed_bytes(&n).expect("bytes");
        let signer = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &attacker.serialize_der())
            .expect("load");
        let sig = signer.sign(&SystemRandom::new(), &bytes).expect("sign");
        let forged = SignedAddressNotice {
            notice: n,
            sig: base64::engine::general_purpose::STANDARD.encode(sig.as_ref()),
        };
        assert_eq!(apply(&mut store, &body(&forged), Utc::now()), Err(NoticeError::BadSignature));
        assert_eq!(store.get(&f).expect("peer"), &snapshot);
    }

    #[test]
    fn an_unknown_fingerprint_changes_nothing() {
        let k = keypair();
        let stranger = keypair();
        let mut store = PeerStore::default();
        let f = trusted(&mut store, &k, "afs", "");
        let snapshot = store.list().to_vec();
        let s = sign(&notice(&stranger, ("afs", ""), ("evil", ""), 5, Utc::now()), &stranger).expect("sign");
        let err = apply(&mut store, &body(&s), Utc::now()).expect_err("unknown");
        assert_eq!(err, NoticeError::UnknownPeer);
        assert_eq!(err.status(), "403 Forbidden");
        assert_eq!(store.list(), snapshot.as_slice());
        assert_eq!(store.get(&f).expect("peer").subdomain, "afs");
    }

    #[test]
    fn a_notice_signed_by_peer_a_cannot_change_peer_b() {
        let (a, b) = (keypair(), keypair());
        let mut store = PeerStore::default();
        let fa = trusted(&mut store, &a, "alpha", "");
        let fb = trusted(&mut store, &b, "bravo", "");
        let snapshot = store.list().to_vec();
        // A signs a notice that names B's fingerprint.
        let mut n = notice(&a, ("bravo", ""), ("evil", ""), 5, Utc::now());
        n.fingerprint = fb.clone();
        let bytes = signed_bytes(&n).expect("bytes");
        let signer = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &a.serialize_der()).expect("load");
        let sig = signer.sign(&SystemRandom::new(), &bytes).expect("sign");
        let forged = SignedAddressNotice {
            notice: n,
            sig: base64::engine::general_purpose::STANDARD.encode(sig.as_ref()),
        };
        assert_eq!(apply(&mut store, &body(&forged), Utc::now()), Err(NoticeError::BadSignature));
        assert_eq!(store.list(), snapshot.as_slice());
        // A's own notice only ever moves A.
        let s = sign(&notice(&a, ("alpha", ""), ("alpha2", ""), 6, Utc::now()), &a).expect("sign");
        apply(&mut store, &body(&s), Utc::now()).expect("apply");
        assert_eq!(store.get(&fa).expect("a").subdomain, "alpha2");
        assert_eq!(store.get(&fb).expect("b").subdomain, "bravo");
    }

    #[test]
    fn a_replayed_or_older_seq_is_rejected() {
        let k = keypair();
        let mut store = PeerStore::default();
        let f = trusted(&mut store, &k, "afs", "");
        let now = Utc::now();
        let first = sign(&notice(&k, ("afs", ""), ("b", ""), 100, now), &k).expect("sign");
        apply(&mut store, &body(&first), now).expect("first");
        // The exact same bytes again.
        assert_eq!(
            apply(&mut store, &body(&first), now),
            Err(NoticeError::Replay { seq: 100, last: 100 })
        );
        // An older seq moving it back.
        let older = sign(&notice(&k, ("b", ""), ("afs", ""), 99, now), &k).expect("sign");
        let err = apply(&mut store, &body(&older), now).expect_err("older");
        assert_eq!(err.code(), "replay");
        assert_eq!(store.get(&f).expect("peer").subdomain, "b");
    }

    #[test]
    fn issued_at_window_is_enforced() {
        let k = keypair();
        let mut store = PeerStore::default();
        trusted(&mut store, &k, "afs", "");
        let now = Utc::now();
        let stale = sign(&notice(&k, ("afs", ""), ("b", ""), 1, now - Duration::days(8)), &k).expect("sign");
        assert_eq!(apply(&mut store, &body(&stale), now), Err(NoticeError::Stale));
        let future = sign(&notice(&k, ("afs", ""), ("b", ""), 2, now + Duration::minutes(10)), &k).expect("sign");
        assert_eq!(apply(&mut store, &body(&future), now), Err(NoticeError::InFuture));
        let ok = sign(&notice(&k, ("afs", ""), ("b", ""), 3, now - Duration::days(6)), &k).expect("sign");
        apply(&mut store, &body(&ok), now).expect("6 days old is inside the window");
    }

    #[test]
    fn untrusted_peer_and_mismatched_address_change_nothing() {
        let k = keypair();
        let mut store = PeerStore::default();
        let f = store.upsert(FederationPeer::pin("p", "afs", k.public_key_pem()).expect("pin"));
        let s = sign(&notice(&k, ("afs", ""), ("b", ""), 1, Utc::now()), &k).expect("sign");
        assert_eq!(apply(&mut store, &body(&s), Utc::now()), Err(NoticeError::NotTrusted));
        store.set_trust(&f, PeerTrust::Trusted);
        // The owner here stores another name than the notice's old address.
        let s = sign(&notice(&k, ("other", ""), ("b", ""), 2, Utc::now()), &k).expect("sign");
        assert_eq!(apply(&mut store, &body(&s), Utc::now()), Err(NoticeError::AddressMismatch));
        assert_eq!(store.get(&f).expect("peer").subdomain, "afs");
        assert_eq!(store.get(&f).expect("peer").address_seq, 0);
    }

    #[test]
    fn spelling_of_the_stored_subdomain_does_not_matter_and_lan_url_moves() {
        let k = keypair();
        let mut store = PeerStore::default();
        let f = trusted(&mut store, &k, "AFS.k2.dev", "http://192.168.1.5:38471/");
        let s = sign(
            &notice(&k, ("afs", "http://192.168.1.5:38471"), ("k2n1-m4", "http://192.168.1.77:38471"), 1, Utc::now()),
            &k,
        )
        .expect("sign");
        let a = apply(&mut store, &body(&s), Utc::now()).expect("apply");
        assert_eq!(a.changed, vec!["subdomain", "base_url"]);
        let p = store.get(&f).expect("peer");
        assert_eq!(p.subdomain, "k2n1-m4");
        assert_eq!(p.base_url, "http://192.168.1.77:38471");
    }

    // ── Sender ──

    fn peers_with(keys: &[(&KeyPair, &str)]) -> PeerStore {
        let mut store = PeerStore::default();
        for (k, sub) in keys {
            trusted(&mut store, k, sub, "");
        }
        store
    }

    #[test]
    fn first_observation_is_a_baseline_then_a_change_issues_to_trusted_peers() {
        with_temp_home(|| {
            let (a, b) = (keypair(), keypair());
            let mut store = peers_with(&[(&a, "pa"), (&b, "pb")]);
            let pending = store.upsert(FederationPeer::pin("p", "pc", keypair().public_key_pem()).expect("pin"));
            let now = Utc::now();
            assert_eq!(
                observe_and_issue(&PeerAddress::new("afs", ""), &store, now).expect("obs"),
                IssueOutcome::Baseline(PeerAddress::new("afs", ""))
            );
            assert_eq!(observe_and_issue(&PeerAddress::new("afs", ""), &store, now).expect("obs"), IssueOutcome::Unchanged);
            // Losing the tunnel is not announced.
            assert_eq!(observe_and_issue(&PeerAddress::default(), &store, now).expect("obs"), IssueOutcome::Unchanged);
            match observe_and_issue(&PeerAddress::new("k2n1-m4", ""), &store, now).expect("obs") {
                IssueOutcome::Issued { seq, address, peers } => {
                    assert!(seq >= u64::try_from(now.timestamp_millis()).expect("ms"));
                    assert_eq!(address, PeerAddress::new("k2n1-m4", ""));
                    assert_eq!(peers.len(), 2);
                    assert!(!peers.contains(&pending), "a pending peer is not told");
                }
                other => panic!("expected Issued, got {other:?}"),
            }
            let rows = rows().expect("rows");
            assert_eq!(rows.len(), 2);
            for row in rows.values() {
                assert_eq!(row.peer_known, PeerAddress::new("afs", ""));
                assert_eq!(row.target, PeerAddress::new("k2n1-m4", ""));
                assert_eq!(row.status, NoticeStatus::Pending);
            }
            store.remove(&pending);
        });
    }

    #[cfg(unix)]
    #[test]
    fn outbox_survives_a_restart_retries_a_dead_port_then_delivers() {
        with_temp_home(|| {
            let me = keypair();
            let peer = keypair();
            let store = peers_with(&[(&peer, "pp")]);
            let t0 = Utc::now();
            observe_and_issue(&PeerAddress::new("afs", ""), &store, t0).expect("baseline");
            observe_and_issue(&PeerAddress::new("k2n1-m4", ""), &store, t0).expect("issue");

            // A ClosedPort fake: a real dial that is refused.
            let closed = crate::test_env::ClosedPort::new();
            let url = format!("http://127.0.0.1:{}{ROUTE}", closed.port());
            let send_closed = |_: &crate::federation::FederationPeer, b: &[u8]| {
                match reqwest::blocking::Client::new().post(&url).body(b.to_vec()).send() {
                    Ok(r) => SendResult::Http(r.status().as_u16(), r.text().unwrap_or_default()),
                    Err(e) => SendResult::Transport(e.to_string()),
                }
            };
            let res = drain_with(&me, &store, t0, send_closed).expect("drain");
            assert_eq!(res.len(), 1);
            assert!(matches!(res[0].1, DeliveryOutcome::Retry(_)), "{res:?}");
            let row = rows().expect("rows").into_values().next().expect("row");
            assert_eq!(row.status, NoticeStatus::Pending);
            assert_eq!(row.attempts, 1);
            assert_eq!(row.next_attempt_at, t0 + Duration::seconds(30));
            // Not due again before the backoff.
            assert!(drain_with(&me, &store, t0 + Duration::seconds(10), |_, _| panic!("not due")).expect("drain").is_empty());

            // "Restart": everything lives on disk; reload from the file.
            let reloaded: NoticeState =
                serde_json::from_str(&fs::read_to_string(state_path()).expect("state file")).expect("parse");
            assert_eq!(reloaded.peers.len(), 1);

            // A fake peer that verifies and accepts.
            let mut receiver = PeerStore::default();
            let my_fp_at_peer = trusted(&mut receiver, &me, "afs", "");
            let t1 = t0 + Duration::seconds(31);
            let res = drain_with(&me, &store, t1, |_, b| match apply(&mut receiver, b, t1) {
                Ok(_) => SendResult::Http(200, "{}".to_string()),
                Err(e) => SendResult::Http(409, e.to_string()),
            })
            .expect("drain");
            assert_eq!(res[0].1, DeliveryOutcome::Delivered);
            assert_eq!(receiver.get(&my_fp_at_peer).expect("me").subdomain, "k2n1-m4");
            let row = rows().expect("rows").into_values().next().expect("row");
            assert_eq!(row.status, NoticeStatus::Delivered);
            assert_eq!(row.peer_known, PeerAddress::new("k2n1-m4", ""));
            // Nothing more to send.
            assert!(drain_with(&me, &store, t1 + Duration::hours(3), |_, _| panic!("delivered")).expect("drain").is_empty());
        });
    }

    #[test]
    fn a_404_peer_is_recorded_as_too_old_and_never_retried() {
        with_temp_home(|| {
            let me = keypair();
            let store = peers_with(&[(&keypair(), "old")]);
            let t0 = Utc::now();
            observe_and_issue(&PeerAddress::new("afs", ""), &store, t0).expect("baseline");
            observe_and_issue(&PeerAddress::new("k2n1-m4", ""), &store, t0).expect("issue");
            let res = drain_with(&me, &store, t0, |_, _| SendResult::Http(404, r#"{"error":"not found"}"#.into()))
                .expect("drain");
            assert!(matches!(&res[0].1, DeliveryOutcome::Failed { reason, .. } if reason == "peer_too_old"), "{res:?}");
            let row = rows().expect("rows").into_values().next().expect("row");
            assert_eq!(row.status, NoticeStatus::Failed);
            assert_eq!(row.failed_reason.as_deref(), Some("peer_too_old"));
            assert!(drain_with(&me, &store, t0 + Duration::days(1), |_, _| panic!("no retry")).expect("drain").is_empty());
        });
    }

    #[test]
    fn a_row_gives_up_after_seven_days() {
        with_temp_home(|| {
            let me = keypair();
            let store = peers_with(&[(&keypair(), "p")]);
            let t0 = Utc::now();
            observe_and_issue(&PeerAddress::new("afs", ""), &store, t0).expect("baseline");
            observe_and_issue(&PeerAddress::new("b", ""), &store, t0).expect("issue");
            drain_with(&me, &store, t0, |_, _| SendResult::Transport("refused".into())).expect("drain");
            let res = drain_with(&me, &store, t0 + Duration::days(7), |_, _| panic!("gave up")).expect("drain");
            assert!(res.is_empty());
            let row = rows().expect("rows").into_values().next().expect("row");
            assert_eq!(row.status, NoticeStatus::Failed);
            assert_eq!(row.failed_reason.as_deref(), Some("gave_up"));
            assert!(row.last_error.as_deref().is_some_and(|e| e.contains("refused")), "{row:?}");
        });
    }

    #[test]
    fn air_gap_sends_nothing() {
        with_temp_home(|| {
            let _air = crate::test_env::EnvVar::set("K2_AIRGAP", "1");
            let me = keypair();
            let store = peers_with(&[(&keypair(), "p")]);
            let t0 = Utc::now();
            observe_and_issue(&PeerAddress::new("afs", ""), &store, t0).expect("baseline");
            observe_and_issue(&PeerAddress::new("b", ""), &store, t0).expect("issue");
            let res = drain_with(&me, &store, t0, |_, _| panic!("air-gap must not send")).expect("drain");
            assert!(res.is_empty());
            let row = rows().expect("rows").into_values().next().expect("row");
            assert_eq!(row.status, NoticeStatus::Pending, "kept for when air-gap is off");
            assert_eq!(row.attempts, 0);
        });
    }

    #[test]
    fn a_second_switch_before_delivery_keeps_what_the_peer_knows() {
        with_temp_home(|| {
            let me = keypair();
            let peer = keypair();
            let store = peers_with(&[(&peer, "p")]);
            let t0 = Utc::now();
            observe_and_issue(&PeerAddress::new("a", ""), &store, t0).expect("baseline");
            observe_and_issue(&PeerAddress::new("b", ""), &store, t0).expect("a->b");
            observe_and_issue(&PeerAddress::new("c", ""), &store, t0 + Duration::seconds(1)).expect("b->c");
            let row = rows().expect("rows").into_values().next().expect("row");
            assert_eq!(row.peer_known, PeerAddress::new("a", ""), "the peer never heard about b");
            assert_eq!(row.target, PeerAddress::new("c", ""));

            let mut receiver = PeerStore::default();
            let f = trusted(&mut receiver, &me, "a", "");
            let t1 = t0 + Duration::seconds(2);
            drain_with(&me, &store, t1, |_, b| match apply(&mut receiver, b, t1) {
                Ok(_) => SendResult::Http(200, "{}".into()),
                Err(e) => SendResult::Http(409, e.to_string()),
            })
            .expect("drain");
            assert_eq!(receiver.get(&f).expect("me").subdomain, "c");

            // Switching back before delivery drops the row.
            observe_and_issue(&PeerAddress::new("d", ""), &store, t1).expect("c->d");
            observe_and_issue(&PeerAddress::new("c", ""), &store, t1).expect("d->c");
            assert!(rows().expect("rows").is_empty(), "nothing to tell: the peer has c");
        });
    }

    #[test]
    fn classify_maps_statuses() {
        assert_eq!(classify(&SendResult::Http(200, String::new())), DeliveryOutcome::Delivered);
        assert_eq!(
            classify(&SendResult::Http(409, r#"{"error":"x","code":"replay"}"#.into())),
            DeliveryOutcome::Delivered
        );
        assert!(matches!(
            classify(&SendResult::Http(409, r#"{"error":"x","code":"address_mismatch"}"#.into())),
            DeliveryOutcome::Failed { reason, .. } if reason == "address_mismatch"
        ));
        assert!(matches!(classify(&SendResult::Http(403, String::new())), DeliveryOutcome::Failed { .. }));
        assert!(matches!(classify(&SendResult::Http(503, String::new())), DeliveryOutcome::Retry(_)));
        assert!(matches!(classify(&SendResult::Http(429, String::new())), DeliveryOutcome::Retry(_)));
        assert!(matches!(classify(&SendResult::Transport("x".into())), DeliveryOutcome::Retry(_)));
    }
}
