//! S5 ops layer for K2 Mail outbound (PRD §8.3/§8.4) — everything the
//! `routes_send` handlers call, testable with fakes (no network, no
//! real clock, no real sends — pre-mortem #1: the ONLY thing that ever
//! touches a wire is the production [`SubmitBackend`] impl, and that
//! dials the LOCAL Stalwart's loopback JMAP only).
//!
//! FAIL-CLOSED GOVERNANCE (pre-mortem #11, BINDING — tested here):
//! - the effective gating mode is read ONLY through
//!   `k2_core::workspace::settings::mail_agent_send_for_path` (itself
//!   fail-closed to `off`); [`effective_gate`] additionally refuses an
//!   unreadable or unrecognized mode value — an error NEVER becomes a
//!   send;
//! - EVERY outbound gets a `mail_outbound` audit row BEFORE anything is
//!   handed to Stalwart — an insert failure aborts the send
//!   ([`gate_and_dispatch`]: no row, no send);
//! - an unreadable rate-limit count aborts the send (a broken counter
//!   never fails open);
//! - approve is an ATOMIC `pending → approved` transition; a row in any
//!   other state (already decided, expired, unknown) never submits
//!   ([`approve_and_submit`]).
//!
//! SENDER IDENTITY is server-stamped: the routes resolve From through
//! the S3 ownership model (`messages::owned_active_address`, masked
//! `not_found`) — nothing in a request body ever dictates the From
//! header; this module only ever receives an already-owned address.
//!
//! ALWAYS-ON LIMITS (§8.4, every mode): 20 sends/hour + 100/day per
//! FROM address (counted over the audit rows — attempts count,
//! restart-proof), max 10 recipients, max 10 MB of message text.
//! Reply guardrails (§8.4): recipient locked to the original sender,
//! no reply-to-self loops, >100-References refusal, DMARC-fail gate,
//! max 4 replies/thread/hour (in-memory, daemon-lifetime — the
//! DB-backed hourly cap backstops it across restarts).
//!
//! Message bodies/subjects are NEVER logged (pre-mortem #16) and never
//! ride error strings; the audit row stores the composed message under
//! `body_ref` (a 0600 file in the daemon's outbound store), never
//! inline in the DB.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use k2_core::db::schema::MailOutbound;

use crate::mail::addresses::{self, AddrError};
use crate::mail::jmap::{ReplyContext, StalwartClient};
use crate::mail::messages;
use crate::mail::secrets::SecretStore;

// ── §8.4 always-on limits ───────────────────────────────────────────────

pub const RATE_LIMIT_HOURLY: u32 = 20;
pub const RATE_LIMIT_DAILY: u32 = 100;
pub const MAX_RECIPIENTS: usize = 10;
pub const MAX_MESSAGE_BYTES: usize = 10 * 1024 * 1024;
/// §8.4 loop caps.
pub const MAX_REPLY_REFERENCES: usize = 100;
pub const MAX_THREAD_REPLIES_PER_HOUR: usize = 4;
/// §12: pending approvals auto-deny after 7 days.
pub const APPROVAL_EXPIRE_SECS: i64 = 7 * 86_400;
/// `send --wait` poll cadence inside the held request (§11.1.4 —
/// timeout bounds reuse the S4 wait constants).
pub const DECISION_POLL_SECS: u64 = 2;

// ── Errors (mapped onto the route layer's stable {code, hint}) ──────────

/// Ops errors. Codes extend the S2/S3/S4 family: `usage`/`not_found`/
/// `not_ready`/`engine`/`rate_limited` plus the S5-specific `gated`
/// (403 — CLI exit 3), `send_mode` (409 — the domain refuses), and
/// `guardrail`/`conflict` (409).
#[derive(Debug)]
pub enum SendError {
    Usage(String),
    /// Masked (S3 rule) — foreign/unknown address or outbound row.
    NotFound(String),
    /// Send gating is OFF for the workspace → CLI exit 3.
    Gated(String),
    /// The sending domain's mode refuses (receive-only / relay
    /// misconfigured / unsupported relay kind).
    SendMode(String),
    /// A §8.4 reply guardrail refused.
    Guardrail(String),
    /// Approve/deny raced an already-decided row.
    Conflict(String),
    /// Hourly/daily/thread caps. `window` ∈ hour|day|thread.
    RateLimited { window: &'static str, hint: String },
    Engine(String),
}

// ── The gate (D4 — fail-closed, pre-mortem #11) ─────────────────────────

/// The effective send gate for one request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    Off,
    Approval,
    On,
}

/// Decide the gate from the settings read. FAIL-CLOSED (tested): a
/// failed read OR an unrecognized stored value is an ERROR, never a
/// send — and never silently `Off` either (a corrupt setting must
/// surface, not masquerade as the teaching gated-off message).
pub fn effective_gate(mode: Result<String, String>) -> Result<Gate, SendError> {
    match mode {
        Ok(m) if m == "off" => Ok(Gate::Off),
        Ok(m) if m == "approval" => Ok(Gate::Approval),
        Ok(m) if m == "on" => Ok(Gate::On),
        Ok(other) => Err(SendError::Engine(format!(
            "unrecognized send-gating mode '{other}' — refusing to send (fail-closed); \
             fix the setting in Settings → Email → Sending"
        ))),
        Err(e) => Err(SendError::Engine(format!(
            "could not read the send-gating setting — refusing to send (fail-closed): {e}"
        ))),
    }
}

// ── The composed message (stored at body_ref; JSON, never MIME) ─────────

/// One composed outbound message — the §8.4 "full rendered message
/// stored in K2's DB": what the approvals preview shows and exactly
/// what submission sends. Serialized (JSON) into the outbound store's
/// 0600 body file; [`submission_email_json`] turns it into the RFC 8621
/// Email object at submit time (no MIME composing anywhere).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboundMessage {
    /// Display name for the From header (the workspace's agent display
    /// name — server-chosen, like the address itself).
    #[serde(default)]
    pub from_name: Option<String>,
    /// The server-stamped From address (owned, ACTIVE, normalized).
    pub from: String,
    pub to: Vec<String>,
    #[serde(default)]
    pub cc: Vec<String>,
    pub subject: String,
    pub text_body: String,
    /// Reply threading (§8.4): the original's Message-ID.
    #[serde(default)]
    pub in_reply_to: Option<String>,
    /// Reply threading: original References + original Message-ID.
    #[serde(default)]
    pub references: Option<String>,
}

impl OutboundMessage {
    /// All SMTP envelope recipients (to + cc, in order).
    pub fn all_recipients(&self) -> Vec<String> {
        self.to.iter().chain(self.cc.iter()).cloned().collect()
    }
}

/// Pure builder: the RFC 8621 Email object for `Email/set create`
/// (bodyValues text part; threading headers only when set). The JMAP
/// layer adds `mailboxIds` — everything content-shaped lives here so
/// it fixture-tests without a client.
pub fn submission_email_json(msg: &OutboundMessage) -> serde_json::Value {
    let addr_list = |list: &[String]| -> Vec<serde_json::Value> {
        list.iter().map(|e| serde_json::json!({ "email": e })).collect()
    };
    let mut v = serde_json::json!({
        "from": [{ "name": msg.from_name, "email": msg.from }],
        "to": addr_list(&msg.to),
        "subject": msg.subject,
        "keywords": { "$seen": true },
        "bodyValues": { "k2body": { "value": msg.text_body } },
        "textBody": [{ "partId": "k2body", "type": "text/plain" }],
    });
    if !msg.cc.is_empty() {
        v["cc"] = serde_json::Value::Array(addr_list(&msg.cc));
    }
    if let Some(irt) = msg.in_reply_to.as_deref() {
        v["header:In-Reply-To:asText"] = serde_json::json!(irt);
    }
    if let Some(refs) = msg.references.as_deref() {
        v["header:References:asText"] = serde_json::json!(refs);
    }
    v
}

/// Validate + normalize a recipient list: every entry must be a full
/// `local@domain` (punycode-normalized domain, pre-mortem #14);
/// duplicates collapse (order kept). Usage errors name the bad entry —
/// recipients came from the caller, they're not secret to the caller.
pub fn normalize_recipients(raw: &[String]) -> Result<Vec<String>, SendError> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::with_capacity(raw.len());
    for r in raw {
        let norm = addresses::normalize_address(r).map_err(|e| match e {
            AddrError::Usage(hint) => SendError::Usage(format!("invalid recipient: {hint}")),
            _ => SendError::Usage(format!("invalid recipient '{r}'")),
        })?;
        if seen.insert(norm.clone()) {
            out.push(norm);
        }
    }
    Ok(out)
}

// ── The outbound store (audit rows + 0600 body files) ───────────────────

/// A new audit row. `status` is the INITIAL state the gate decided
/// (`pending` under approval, `approved` under on-mode policy).
pub struct NewOutbound<'a> {
    pub owner_project_id: &'a str,
    pub agent_name: &'a str,
    pub message: &'a OutboundMessage,
    pub status: &'a str,
    pub decided_by: Option<&'a str>,
    pub now: i64,
}

/// Everything S5 needs from the audit/queue storage, behind a trait so
/// the fail-closed invariants test with recording/failing fakes.
/// Production impl is [`DbOutboundStore`].
pub trait OutboundStore {
    /// Write the body file + insert the row. Returns the `out_…` id.
    /// ANY failure here means NO send happened and none may follow.
    fn insert(&self, new: &NewOutbound) -> Result<String, String>;
    fn load(&self, id: &str) -> Result<Option<MailOutbound>, String>;
    /// Audit rows for `from_address` created at/after `since` (the
    /// rate-limit denominator — attempts count, every status).
    fn count_recent(&self, from_address: &str, since: i64) -> Result<u32, String>;
    /// Atomic status transition: only fires when the row currently
    /// holds `from_status`; `Ok(false)` = it didn't (decided already /
    /// unknown id) — the caller must treat that as NOT actionable.
    fn transition(
        &self,
        id: &str,
        from_status: &str,
        to_status: &str,
        decided_by: Option<&str>,
        note: Option<&str>,
        now: i64,
    ) -> Result<bool, String>;
    /// The composed message back from the body file.
    fn load_message(&self, id: &str) -> Result<OutboundMessage, String>;
    /// Append one line to the row's note (submit-failure details).
    fn append_note(&self, id: &str, extra: &str, now: i64) -> Result<(), String>;
}

const OUTBOUND_COLS: &str = "id, owner_project_id, agent_name, from_address, to_json, \
                             cc_json, subject, body_ref, attachments_ref, status, \
                             decided_by, note, created_at, updated_at, decided_at, sent_at";

fn map_outbound_row(r: &rusqlite::Row) -> rusqlite::Result<MailOutbound> {
    Ok(MailOutbound {
        id: r.get(0)?,
        owner_project_id: r.get(1)?,
        agent_name: r.get(2)?,
        from_address: r.get(3)?,
        to_json: r.get(4)?,
        cc_json: r.get(5)?,
        subject: r.get(6)?,
        body_ref: r.get(7)?,
        attachments_ref: r.get(8)?,
        status: r.get(9)?,
        decided_by: r.get(10)?,
        note: r.get(11)?,
        created_at: r.get(12)?,
        updated_at: r.get(13)?,
        decided_at: r.get(14)?,
        sent_at: r.get(15)?,
    })
}

/// The production store: `mail_outbound` rows in the shared DB +
/// composed-message JSON files under `~/.k2/mail-outbound/` (0600,
/// the mail-secrets file-permission convention — bodies never live in
/// the DB and never in logs).
pub struct DbOutboundStore {
    body_dir: PathBuf,
}

impl Default for DbOutboundStore {
    fn default() -> Self {
        // House rule: tests never touch the real `~/.k2` — the route
        // tests drive the REAL handlers (which construct this default),
        // so the test build points the body dir at a per-process temp
        // location instead. Production stays `~/.k2/mail-outbound`.
        if cfg!(test) {
            return Self {
                body_dir: std::env::temp_dir()
                    .join(format!("k2-mail-outbound-testdefault-{}", std::process::id())),
            };
        }
        let dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".k2")
            .join("mail-outbound");
        Self { body_dir: dir }
    }
}

impl DbOutboundStore {
    #[cfg(test)]
    pub fn at(body_dir: PathBuf) -> Self {
        Self { body_dir }
    }

    fn body_path(&self, id: &str) -> PathBuf {
        self.body_dir.join(format!("{id}.json"))
    }
}

#[cfg(unix)]
fn restrict_mode(file: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_mode(_file: &Path) {}

/// `out_…` id (§11.1.9: stable `out_*` ids) — 12 hex of UUID.
fn new_outbound_id() -> String {
    let hex = uuid::Uuid::new_v4().simple().to_string();
    format!("out_{}", &hex[..12])
}

impl OutboundStore for DbOutboundStore {
    fn insert(&self, new: &NewOutbound) -> Result<String, String> {
        let id = new_outbound_id();
        std::fs::create_dir_all(&self.body_dir)
            .map_err(|e| format!("outbound store dir: {e}"))?;
        let path = self.body_path(&id);
        let body = serde_json::to_string(new.message)
            .map_err(|e| format!("outbound message serialize: {e}"))?;
        std::fs::write(&path, body.as_bytes())
            .map_err(|e| format!("outbound body write: {e}"))?;
        restrict_mode(&path);
        let to_json = serde_json::to_string(&new.message.to)
            .map_err(|e| format!("to serialize: {e}"))?;
        let cc_json = if new.message.cc.is_empty() {
            None
        } else {
            Some(
                serde_json::to_string(&new.message.cc)
                    .map_err(|e| format!("cc serialize: {e}"))?,
            )
        };
        let decided_at: Option<i64> = new.decided_by.map(|_| new.now);
        let inserted = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO mail_outbound (id, owner_project_id, agent_name, from_address, \
                 to_json, cc_json, subject, body_ref, status, decided_by, decided_at, \
                 created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12)",
                rusqlite::params![
                    id,
                    new.owner_project_id,
                    new.agent_name,
                    new.message.from,
                    to_json,
                    cc_json,
                    new.message.subject,
                    path.to_string_lossy(),
                    new.status,
                    new.decided_by,
                    decided_at,
                    new.now,
                ],
            )
        };
        if let Err(e) = inserted {
            // No row → no send; don't leave an orphan body file either.
            let _ = std::fs::remove_file(&path);
            return Err(format!("outbound row insert: {e}"));
        }
        Ok(id)
    }

    fn load(&self, id: &str) -> Result<Option<MailOutbound>, String> {
        let db = k2_core::db::shared();
        let conn = db.lock();
        match conn.query_row(
            &format!("SELECT {OUTBOUND_COLS} FROM mail_outbound WHERE id = ?1"),
            rusqlite::params![id],
            map_outbound_row,
        ) {
            Ok(row) => Ok(Some(row)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(format!("outbound load: {e}")),
        }
    }

    fn count_recent(&self, from_address: &str, since: i64) -> Result<u32, String> {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT COUNT(*) FROM mail_outbound WHERE from_address = ?1 AND created_at >= ?2",
            rusqlite::params![from_address, since],
            |r| r.get::<_, i64>(0),
        )
        .map(|n| n.max(0) as u32)
        .map_err(|e| format!("outbound rate count: {e}"))
    }

    fn transition(
        &self,
        id: &str,
        from_status: &str,
        to_status: &str,
        decided_by: Option<&str>,
        note: Option<&str>,
        now: i64,
    ) -> Result<bool, String> {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "UPDATE mail_outbound SET status = ?2, \
             decided_by = COALESCE(?3, decided_by), \
             note = COALESCE(?4, note), \
             decided_at = CASE WHEN ?2 IN ('approved','denied') THEN ?5 ELSE decided_at END, \
             sent_at = CASE WHEN ?2 = 'sent' THEN ?5 ELSE sent_at END, \
             updated_at = ?5 \
             WHERE id = ?1 AND status = ?6",
            rusqlite::params![id, to_status, decided_by, note, now, from_status],
        )
        .map(|n| n == 1)
        .map_err(|e| format!("outbound transition: {e}"))
    }

    fn load_message(&self, id: &str) -> Result<OutboundMessage, String> {
        let body_ref: Option<String> = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.query_row(
                "SELECT body_ref FROM mail_outbound WHERE id = ?1",
                rusqlite::params![id],
                |r| r.get(0),
            )
            .map_err(|e| format!("outbound body_ref: {e}"))?
        };
        let Some(body_ref) = body_ref.filter(|s| !s.trim().is_empty()) else {
            return Err(format!("outbound '{id}' has no stored message body"));
        };
        let raw = std::fs::read_to_string(&body_ref)
            .map_err(|e| format!("outbound body read: {e}"))?;
        serde_json::from_str(&raw).map_err(|e| format!("outbound body parse: {e}"))
    }

    fn append_note(&self, id: &str, extra: &str, now: i64) -> Result<(), String> {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "UPDATE mail_outbound SET note = CASE \
             WHEN note IS NULL OR note = '' THEN ?2 ELSE note || char(10) || ?2 END, \
             updated_at = ?3 WHERE id = ?1",
            rusqlite::params![id, extra, now],
        )
        .map(|_| ())
        .map_err(|e| format!("outbound note append: {e}"))
    }
}

/// Record a SUCCESSFUL linked-SMTP submission in the outbox (issue
/// #31.5). Unlike hosted sends, linked send is ungated and goes out
/// immediately — so the row is born via the same `approved → sent`
/// lifecycle a hosted send ends on, leaving it `sent` (wire:
/// `submitted` — accepted-for-delivery, NEVER "delivered", pre-mortem
/// #9) with `sent_at`/`updated_at` stamped exactly like a hosted row.
/// `decided_by` is `None` (no human decision — it's ungated). Returns
/// the new `out_…` id. The caller treats a failure as non-fatal: the
/// mail already left, this is only the audit trail.
pub fn record_linked_submitted(
    store: &dyn OutboundStore,
    owner_project_id: &str,
    agent_name: &str,
    message: &OutboundMessage,
    now: i64,
) -> Result<String, String> {
    let id = store.insert(&NewOutbound {
        owner_project_id,
        agent_name,
        message,
        status: "approved",
        decided_by: None,
        now,
    })?;
    // Stamp it accepted-for-delivery. A failed transition leaves a
    // truthful `approved` row rather than losing the trail entirely.
    store.transition(&id, "approved", "sent", None, None, now)?;
    Ok(id)
}

// ── Row loaders for the read routes (shared DB, route-shaped) ───────────

/// The caller's outbound rows, newest first.
pub fn list_for_project(project_id: &str, limit: usize) -> Vec<MailOutbound> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let Ok(mut stmt) = conn.prepare(&format!(
        "SELECT {OUTBOUND_COLS} FROM mail_outbound WHERE owner_project_id = ?1 \
         ORDER BY created_at DESC, id DESC LIMIT ?2"
    )) else {
        return Vec::new();
    };
    stmt.query_map(rusqlite::params![project_id, limit as i64], map_outbound_row)
        .map(|r| r.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

/// The owner's Approvals queue: every pending row, oldest first.
pub fn list_pending() -> Vec<MailOutbound> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let Ok(mut stmt) = conn.prepare(&format!(
        "SELECT {OUTBOUND_COLS} FROM mail_outbound WHERE status = 'pending' \
         ORDER BY created_at, id"
    )) else {
        return Vec::new();
    };
    stmt.query_map([], map_outbound_row)
        .map(|r| r.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

/// §12 lazy expiry: auto-deny pending rows older than 7 days (called
/// on every outbox/approvals read + before approve/deny — no separate
/// background loop). Emits `MailSendDecided` per expired row. Returns
/// how many expired.
pub fn auto_deny_expired(now: i64) -> usize {
    let cutoff = now - APPROVAL_EXPIRE_SECS;
    let ids: Vec<String> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let Ok(mut stmt) = conn.prepare(
            "SELECT id FROM mail_outbound WHERE status = 'pending' AND created_at < ?1",
        ) else {
            return 0;
        };
        stmt.query_map(rusqlite::params![cutoff], |r| r.get::<_, String>(0))
            .map(|r| r.filter_map(Result::ok).collect())
            .unwrap_or_default()
    };
    let store = DbOutboundStore::default();
    let mut n = 0;
    for id in ids {
        let flipped = store
            .transition(
                &id,
                "pending",
                "denied",
                Some("system:expired"),
                Some("expired: not decided within 7 days (auto-denied)"),
                now,
            )
            .unwrap_or(false);
        if flipped {
            n += 1;
            k2_core::agent_hooks::emit(
                k2_core::agent_hooks::HookEvent::MailSendDecided,
                serde_json::json!({ "outboundId": id, "status": "rejected", "expired": true }),
            );
        }
    }
    n
}

// ── Wire statuses (§11.1.9 / the gate's outbox-note shapes) ─────────────

/// DB status → the stable wire status the CLI/UI shows. `sent` is
/// deliberately spelled `submitted` on the wire — K2 reports
/// accepted-for-delivery, never "delivered" (pre-mortem #9).
pub fn wire_status(db_status: &str) -> &'static str {
    match db_status {
        "pending" => "pending_approval",
        "approved" => "approved",
        "denied" => "rejected",
        "sent" => "submitted",
        "failed" => "failed",
        _ => "unknown",
    }
}

/// The per-status outbox note (rendered next to the owner's own note).
pub fn status_note(db_status: &str) -> &'static str {
    match db_status {
        "pending" => "queued for approval — your human decides in Settings → Email → Approvals",
        "approved" => "approved — handing to the mail server",
        "denied" => "denied by your human — see the note",
        "sent" => "accepted-for-delivery (final delivery is the receiving server's business)",
        "failed" => "submission failed — see the note",
        _ => "unknown state",
    }
}

/// One outbox row in wire shape (`k2 mail outbox [--id]`, §11.1.9:
/// denied items carry the owner's note inline).
pub fn outbound_json(row: &MailOutbound) -> serde_json::Value {
    let list = |raw: Option<&str>| -> Vec<String> {
        raw.and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
            .unwrap_or_default()
    };
    serde_json::json!({
        "id": row.id,
        "from": row.from_address,
        "to": list(Some(row.to_json.as_str())),
        "cc": list(row.cc_json.as_deref()),
        "subject": row.subject,
        "status": wire_status(&row.status),
        "statusNote": status_note(&row.status),
        "note": row.note,
        "agentName": row.agent_name,
        "decidedBy": row.decided_by,
        "createdAt": row.created_at,
        "decidedAt": row.decided_at,
        "sentAt": row.sent_at,
    })
}

// ── The submit seam (local Stalwart behind a trait) ─────────────────────

/// What S5 needs from the engine at submit time. Production impl is
/// the loopback JMAP client; tests use recording fakes (NO real sends,
/// ever — pre-mortem #1).
pub trait SubmitBackend {
    fn submit(&self, account_id: &str, msg: &OutboundMessage) -> Result<(), String>;
}

impl SubmitBackend for StalwartClient {
    fn submit(&self, account_id: &str, msg: &OutboundMessage) -> Result<(), String> {
        self.submission_send(
            account_id,
            &msg.from,
            &msg.all_recipients(),
            submission_email_json(msg),
        )
    }
}

// ── Rate limits (always-on, §8.4) ───────────────────────────────────────

/// The hourly + daily per-From-address caps, counted over audit rows.
/// FAIL-CLOSED: an unreadable count refuses the send.
pub fn check_rate_limits(
    store: &dyn OutboundStore,
    from_address: &str,
    now: i64,
) -> Result<(), SendError> {
    let fail = |e: String| {
        SendError::Engine(format!(
            "could not read the send rate counter — refusing to send (fail-closed): {e}"
        ))
    };
    let hour = store.count_recent(from_address, now - 3_600).map_err(fail)?;
    if hour >= RATE_LIMIT_HOURLY {
        return Err(SendError::RateLimited {
            window: "hour",
            hint: format!(
                "rate limit: {hour}/{RATE_LIMIT_HOURLY} sends from {from_address} in the \
                 last hour — wait before sending more"
            ),
        });
    }
    let day = store.count_recent(from_address, now - 86_400).map_err(fail)?;
    if day >= RATE_LIMIT_DAILY {
        return Err(SendError::RateLimited {
            window: "day",
            hint: format!(
                "rate limit: {day}/{RATE_LIMIT_DAILY} sends from {from_address} in the \
                 last 24 h — try again tomorrow"
            ),
        });
    }
    Ok(())
}

// ── Domain send-mode check (D1/§8.3 — at use time, per 0072) ────────────

/// Enforce the FROM domain's send mode. `receive-only` refuses with the
/// teaching error; `relay` validates its config KIND-AGNOSTICALLY
/// (creds resolve through the secret store — never read into errors,
/// never logged); a missing domain row or unrecognized mode refuses
/// (fail-closed). `direct`'s doctor-grade gate is enforced where the
/// mode is SET (config, with S6), not per-send.
pub fn check_domain_send_mode(
    secrets: &dyn SecretStore,
    from_address: &str,
) -> Result<(), SendError> {
    let domain = from_address
        .split_once('@')
        .map(|(_, d)| d.to_string())
        .ok_or_else(|| SendError::Engine(format!("malformed from address '{from_address}'")))?;
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        crate::mail::domains::load_domain(&conn, &domain)
    };
    let Some(row) = row else {
        return Err(SendError::Engine(format!(
            "the sending domain '{domain}' is not hosted here any more — refusing (fail-closed)"
        )));
    };
    match row.send_mode.as_str() {
        "direct" => Ok(()),
        "receive-only" => Err(SendError::SendMode(format!(
            "domain '{domain}' is receive-only — outbound mail is disabled for it. Your \
             human can change its send mode in Settings → Email → Sending"
        ))),
        "relay" => check_relay_config(secrets, &domain, row.relay_config_id.as_deref()),
        other => Err(SendError::Engine(format!(
            "unrecognized send mode '{other}' for domain '{domain}' — refusing (fail-closed)"
        ))),
    }
}

/// Validate a relay config at use time (0072: "the send path validates
/// it at use time"). Kind-agnostic: every kind must have resolvable
/// creds when it references any; only `smtp` is submittable in V1 —
/// other kinds refuse with a teaching error rather than assuming
/// `kind == smtp` anywhere.
fn check_relay_config(
    secrets: &dyn SecretStore,
    domain: &str,
    relay_config_id: Option<&str>,
) -> Result<(), SendError> {
    let misconfigured = |what: &str| {
        SendError::SendMode(format!(
            "domain '{domain}' is in relay mode but {what} — fix it in Settings → Email → Sending"
        ))
    };
    let Some(config_id) = relay_config_id.filter(|s| !s.trim().is_empty()) else {
        return Err(misconfigured("no relay is configured"));
    };
    type Row = (String, Option<String>, Option<i64>, Option<String>);
    let row: Option<Row> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT kind, host, port, secret_ref FROM mail_relay_configs WHERE id = ?1",
            rusqlite::params![config_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .ok()
    };
    let Some((kind, host, port, secret_ref)) = row else {
        return Err(misconfigured("its relay config row is gone"));
    };
    // Creds resolve or the send refuses (fail-closed) — the VALUE is
    // never inspected, logged, or carried into the error.
    if let Some(sref) = secret_ref.as_deref().filter(|s| !s.trim().is_empty()) {
        match secrets.resolve(sref) {
            Ok(Some(_)) => {}
            Ok(None) => {
                return Err(misconfigured(
                    "its relay credentials are missing from the secret store",
                ))
            }
            Err(e) => {
                return Err(SendError::Engine(format!(
                    "could not resolve the relay credentials — refusing to send \
                     (fail-closed): {e}"
                )))
            }
        }
    }
    match kind.as_str() {
        "smtp" => {
            let host_ok = host.as_deref().map(str::trim).filter(|s| !s.is_empty()).is_some();
            let port_ok = port.map(|p| p > 0).unwrap_or(false);
            let creds_ok = secret_ref.as_deref().filter(|s| !s.trim().is_empty()).is_some();
            if !host_ok || !port_ok || !creds_ok {
                return Err(misconfigured("its SMTP relay config is incomplete \
                                          (host, port, and credentials are required)"));
            }
            Ok(())
        }
        other => Err(SendError::SendMode(format!(
            "relay kind '{other}' is not supported yet (V1 relays over generic SMTP) — \
             domain '{domain}' cannot send until its relay is switched in Settings → Email"
        ))),
    }
}

// ── The gate dispatch (audit-then-maybe-submit — pre-mortem #11) ────────

/// Everything about one outbound the pipeline needs beyond the message.
pub struct OutboundRequest<'a> {
    pub project_id: &'a str,
    pub agent_name: &'a str,
    /// The From address's backend mailbox handle (submission scope).
    pub account_id: &'a str,
    pub message: &'a OutboundMessage,
}

/// What the gate decided + did.
#[derive(Debug)]
pub enum SendOutcome {
    /// Approval mode: row is pending; NOTHING left the box.
    Queued { id: String },
    /// On mode: audited, handed to Stalwart, accepted-for-delivery.
    Submitted { id: String },
    /// On mode: audited, submission failed — row is `failed`.
    SubmitFailed { id: String, error: String },
}

/// The §8.4 pipeline tail: rate limits → audit row → (queue | submit).
/// INVARIANTS (tested): `Off` never writes and never submits; a failed
/// insert never submits (no row, no send); `Approval` never touches
/// `backend`; `On` without a backend refuses (fail-closed).
pub fn gate_and_dispatch(
    store: &dyn OutboundStore,
    backend: Option<&dyn SubmitBackend>,
    gate: Gate,
    req: &OutboundRequest,
    now: i64,
) -> Result<SendOutcome, SendError> {
    check_rate_limits(store, &req.message.from, now)?;
    match gate {
        Gate::Off => Err(SendError::Gated(
            "outbound email is disabled for this workspace. Your human can enable it in \
             Settings → Email → Sending"
                .to_string(),
        )),
        Gate::Approval => {
            let id = store
                .insert(&NewOutbound {
                    owner_project_id: req.project_id,
                    agent_name: req.agent_name,
                    message: req.message,
                    status: "pending",
                    decided_by: None,
                    now,
                })
                .map_err(SendError::Engine)?;
            Ok(SendOutcome::Queued { id })
        }
        Gate::On => {
            let backend = backend.ok_or_else(|| {
                SendError::Engine(
                    "no submit backend available — refusing to send (fail-closed)".to_string(),
                )
            })?;
            // Audit row FIRST (no row, no send), decided by policy.
            let id = store
                .insert(&NewOutbound {
                    owner_project_id: req.project_id,
                    agent_name: req.agent_name,
                    message: req.message,
                    status: "approved",
                    decided_by: Some("policy:agent-send-on"),
                    now,
                })
                .map_err(SendError::Engine)?;
            match backend.submit(req.account_id, req.message) {
                Ok(()) => {
                    if let Err(e) = store.transition(&id, "approved", "sent", None, None, now) {
                        // The message IS accepted — a bookkeeping miss
                        // must not double-send; log and report success.
                        k2_core::log_debug!("[mail/send] sent-mark failed for {id}: {e}");
                    }
                    Ok(SendOutcome::Submitted { id })
                }
                Err(e) => {
                    let _ = store.transition(&id, "approved", "failed", None, None, now);
                    let _ = store.append_note(&id, &format!("submit failed: {e}"), now);
                    Ok(SendOutcome::SubmitFailed { id, error: e })
                }
            }
        }
    }
}

// ── Approve / deny (owner verbs — atomic, fail-closed) ──────────────────

#[derive(Debug, PartialEq, Eq)]
pub enum ApproveOutcome {
    /// approved → handed to Stalwart → `sent`.
    Submitted,
    /// approved, but the hand-off failed → row is `failed` with the
    /// reason appended to its note. NOTHING left the box.
    FailedToSubmit(String),
}

/// Approve one pending outbound and submit it. The transition is
/// ATOMIC (`pending → approved`); any other current state refuses
/// without submitting (pre-mortem #11: an unknown/ambiguous approval
/// state never sends). `backend` arrives as a Result so an engine that
/// can't even be constructed marks the row failed rather than leaving
/// it silently approved-forever (no background retrier exists —
/// pre-mortem #9).
pub fn approve_and_submit(
    store: &dyn OutboundStore,
    backend: Result<&dyn SubmitBackend, String>,
    account_id_for_from: &mut dyn FnMut(&str) -> Result<String, String>,
    id: &str,
    decided_by: &str,
    note: Option<&str>,
    now: i64,
) -> Result<ApproveOutcome, SendError> {
    let row = store
        .load(id)
        .map_err(|e| SendError::Engine(format!("approval state unreadable — refusing: {e}")))?;
    let Some(row) = row else {
        return Err(SendError::NotFound(format!("no outbound message '{id}'")));
    };
    let flipped = store
        .transition(id, "pending", "approved", Some(decided_by), note, now)
        .map_err(|e| {
            SendError::Engine(format!("approval transition failed — nothing sent: {e}"))
        })?;
    if !flipped {
        return Err(SendError::Conflict(format!(
            "outbound '{id}' is already {} — approve applies to pending messages only",
            wire_status(&row.status)
        )));
    }
    let fail = |store: &dyn OutboundStore, reason: &str| {
        let _ = store.transition(id, "approved", "failed", None, None, now);
        let _ = store.append_note(id, &format!("submit failed: {reason}"), now);
        Ok(ApproveOutcome::FailedToSubmit(reason.to_string()))
    };
    let backend = match backend {
        Ok(b) => b,
        Err(e) => return fail(store, &format!("mail server unavailable: {e}")),
    };
    let msg = match store.load_message(id) {
        Ok(m) => m,
        Err(e) => return fail(store, &format!("stored message unreadable: {e}")),
    };
    let account_id = match account_id_for_from(&msg.from) {
        Ok(a) => a,
        Err(e) => return fail(store, &format!("sender address unavailable: {e}")),
    };
    match backend.submit(&account_id, &msg) {
        Ok(()) => {
            if let Err(e) = store.transition(id, "approved", "sent", None, None, now) {
                k2_core::log_debug!("[mail/send] sent-mark failed for {id}: {e}");
            }
            Ok(ApproveOutcome::Submitted)
        }
        Err(e) => fail(store, &e),
    }
}

/// Deny one pending outbound (atomic; the note flows back to the
/// agent's outbox, §8.4).
pub fn deny(
    store: &dyn OutboundStore,
    id: &str,
    decided_by: &str,
    note: &str,
    now: i64,
) -> Result<(), SendError> {
    let row = store
        .load(id)
        .map_err(|e| SendError::Engine(format!("approval state unreadable — refusing: {e}")))?;
    let Some(row) = row else {
        return Err(SendError::NotFound(format!("no outbound message '{id}'")));
    };
    let flipped = store
        .transition(id, "pending", "denied", Some(decided_by), Some(note), now)
        .map_err(|e| SendError::Engine(format!("deny transition failed: {e}")))?;
    if !flipped {
        return Err(SendError::Conflict(format!(
            "outbound '{id}' is already {} — deny applies to pending messages only",
            wire_status(&row.status)
        )));
    }
    Ok(())
}

// ── §8.4 reply guardrails (pure over the fetched context) ───────────────

/// `Re: ` prefixing without stacking (`Re: Re: …`).
pub fn reply_subject(original: &str) -> String {
    let t = original.trim();
    if t.to_lowercase().starts_with("re:") {
        t.to_string()
    } else {
        format!("Re: {t}")
    }
}

/// Count message-ids in a raw References header ('<' occurrences).
pub fn count_references(references: Option<&str>) -> usize {
    references.map(|r| r.matches('<').count()).unwrap_or(0)
}

/// The outgoing References chain: original References + original
/// Message-ID (RFC 5322 §3.6.4).
pub fn build_out_references(
    orig_references: Option<&str>,
    orig_message_id: Option<&str>,
) -> Option<String> {
    match (orig_references, orig_message_id) {
        (Some(r), Some(m)) => Some(format!("{r} {m}")),
        (None, Some(m)) => Some(m.to_string()),
        (Some(r), None) => Some(r.to_string()),
        (None, None) => None,
    }
}

/// The §8.4 reply guardrails over the original message's context.
/// Returns the LOCKED recipient (the original sender — the caller may
/// not choose it). Refusals: no sender, reply-to-self, References
/// loop cap, DMARC-fail outside approval mode.
pub fn check_reply_guardrails(
    ctx: &ReplyContext,
    from_address: &str,
    gate: Gate,
) -> Result<String, SendError> {
    let sender = ctx
        .from
        .first()
        .map(|a| a.email.trim().to_string())
        .filter(|e| !e.is_empty())
        .ok_or_else(|| {
            SendError::Guardrail(
                "the original message has no sender address to reply to".to_string(),
            )
        })?;
    let recipient = addresses::normalize_address(&sender).map_err(|_| {
        SendError::Guardrail(format!(
            "the original sender's address ('{sender}') is not a replyable address"
        ))
    })?;
    if recipient == from_address {
        return Err(SendError::Guardrail(
            "refusing a reply-to-self — that's a mail loop (§8.4)".to_string(),
        ));
    }
    if count_references(ctx.references.as_deref()) > MAX_REPLY_REFERENCES {
        return Err(SendError::Guardrail(format!(
            "this thread is too deep (more than {MAX_REPLY_REFERENCES} References) — \
             refusing per the §8.4 loop cap"
        )));
    }
    let verdicts = messages::parse_auth_results(&ctx.auth_results);
    if verdicts["dmarc"].as_str() == Some("fail") && gate != Gate::Approval {
        return Err(SendError::Guardrail(
            "the original message FAILED DMARC — replying is blocked unless send gating \
             is 'approval' (your human reviews it). Ask your human via 'k2 feedback ask'"
                .to_string(),
        ));
    }
    Ok(recipient)
}

// ── Per-thread reply cap (in-memory sliding window) ─────────────────────

/// (project, thread) → reply timestamps within the last hour.
/// Daemon-lifetime memory (a restart resets it); the DB-backed 20/hr
/// per-address cap backstops it across restarts.
fn thread_reply_log() -> &'static Mutex<HashMap<String, Vec<i64>>> {
    static LOG: OnceLock<Mutex<HashMap<String, Vec<i64>>>> = OnceLock::new();
    LOG.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Enforce + record the §8.4 max-4-replies-per-thread-per-hour cap.
/// Call ONLY when the reply is otherwise proceeding (a refused reply
/// must not consume the budget).
pub fn check_and_record_thread_reply(key: &str, now: i64) -> Result<(), SendError> {
    let mut map = thread_reply_log().lock().unwrap_or_else(|p| p.into_inner());
    let entries = map.entry(key.to_string()).or_default();
    entries.retain(|t| now - *t < 3_600);
    if entries.len() >= MAX_THREAD_REPLIES_PER_HOUR {
        return Err(SendError::RateLimited {
            window: "thread",
            hint: format!(
                "this thread already has {MAX_THREAD_REPLIES_PER_HOUR} replies in the \
                 last hour — waiting is the §8.4 loop cap, not an error to retry"
            ),
        });
    }
    entries.push(now);
    Ok(())
}

// ── `--wait` (§11.1.4 — block until decided; injected clock) ────────────

/// Poll `poll` (the row's `(db_status, note)`) every `poll_secs` until
/// the status leaves `pending` or `timeout_secs` elapse. `Ok(None)` =
/// timeout — the message is STILL PENDING (the route answers
/// `{ok:true, timedOut:true}`; CLI exit 2). `now_unix`/`sleep` are
/// injected — tests never sleep for real.
pub fn wait_for_decision(
    poll: &mut dyn FnMut() -> Result<(String, Option<String>), String>,
    timeout_secs: u64,
    poll_secs: u64,
    now_unix: &mut dyn FnMut() -> i64,
    sleep: &mut dyn FnMut(std::time::Duration),
) -> Result<Option<(String, Option<String>)>, String> {
    let deadline = now_unix().saturating_add(timeout_secs as i64);
    loop {
        let (status, note) = poll()?;
        if status != "pending" {
            return Ok(Some((status, note)));
        }
        if now_unix() >= deadline {
            return Ok(None);
        }
        sleep(std::time::Duration::from_secs(poll_secs));
    }
}

// ──────────────────────────────────────────────────────────────────────
// Inline unit tests — fakes + fixtures; no network, no real sleeps,
// no real sends (pre-mortem #1).
// ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mail::jmap::MailAddr;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn msg() -> OutboundMessage {
        OutboundMessage {
            from_name: Some("Research Bot".to_string()),
            from: "bot@acme.dev".to_string(),
            to: vec!["human@example.com".to_string()],
            cc: vec![],
            subject: "Weekly digest".to_string(),
            text_body: "All green.".to_string(),
            in_reply_to: None,
            references: None,
        }
    }

    /// Recording fake store: counts inserts, canned failure switches.
    #[derive(Default)]
    struct FakeStore {
        fail_insert: bool,
        fail_count: bool,
        counts: (u32, u32), // (hour, day)
        inserts: Mutex<Vec<(String, Option<String>)>>, // (status, decided_by)
        transitions: Mutex<Vec<(String, String, String)>>,
        transition_result: Option<bool>, // None = Err
        row: Option<MailOutbound>,
        notes: Mutex<Vec<String>>,
    }

    impl OutboundStore for FakeStore {
        fn insert(&self, new: &NewOutbound) -> Result<String, String> {
            if self.fail_insert {
                return Err("db unreachable".to_string());
            }
            self.inserts
                .lock()
                .unwrap()
                .push((new.status.to_string(), new.decided_by.map(String::from)));
            Ok("out_fake00000001".to_string())
        }
        fn load(&self, _id: &str) -> Result<Option<MailOutbound>, String> {
            Ok(self.row.clone())
        }
        fn count_recent(&self, _from: &str, since: i64) -> Result<u32, String> {
            if self.fail_count {
                return Err("db unreachable".to_string());
            }
            // The hourly window has the larger `since`.
            Ok(if since >= 1_000_000 - 3_600 { self.counts.0 } else { self.counts.1 })
        }
        fn transition(
            &self,
            id: &str,
            from: &str,
            to: &str,
            _decided_by: Option<&str>,
            _note: Option<&str>,
            _now: i64,
        ) -> Result<bool, String> {
            match self.transition_result {
                None => Err("db unreachable".to_string()),
                Some(ok) => {
                    self.transitions
                        .lock()
                        .unwrap()
                        .push((id.to_string(), from.to_string(), to.to_string()));
                    Ok(ok)
                }
            }
        }
        fn load_message(&self, _id: &str) -> Result<OutboundMessage, String> {
            Ok(msg())
        }
        fn append_note(&self, _id: &str, extra: &str, _now: i64) -> Result<(), String> {
            self.notes.lock().unwrap().push(extra.to_string());
            Ok(())
        }
    }

    /// Recording fake backend — the "did anything try to leave the
    /// box?" probe every fail-closed test asserts against.
    #[derive(Default)]
    struct FakeSubmit {
        calls: AtomicUsize,
        fail: bool,
    }

    impl SubmitBackend for FakeSubmit {
        fn submit(&self, _account_id: &str, _msg: &OutboundMessage) -> Result<(), String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                Err("connection refused".to_string())
            } else {
                Ok(())
            }
        }
    }

    fn req(m: &OutboundMessage) -> OutboundRequest<'_> {
        OutboundRequest {
            project_id: "P1",
            agent_name: "bot",
            account_id: "acc-1",
            message: m,
        }
    }

    // ── the gate (fail-closed, pre-mortem #11) ──

    #[test]
    fn gate_reads_fail_closed_on_error_and_ambiguity() {
        assert_eq!(effective_gate(Ok("off".into())).unwrap(), Gate::Off);
        assert_eq!(effective_gate(Ok("approval".into())).unwrap(), Gate::Approval);
        assert_eq!(effective_gate(Ok("on".into())).unwrap(), Gate::On);
        // A FAILED settings read is an error, never a send.
        let err = effective_gate(Err("db locked".into())).expect_err("must refuse");
        assert!(matches!(&err, SendError::Engine(h) if h.contains("fail-closed")), "{err:?}");
        // An AMBIGUOUS stored value is an error too.
        let err = effective_gate(Ok("banana".into())).expect_err("must refuse");
        assert!(matches!(&err, SendError::Engine(h) if h.contains("banana")), "{err:?}");
    }

    #[test]
    fn off_mode_never_writes_and_never_submits() {
        let store = FakeStore { transition_result: Some(true), ..Default::default() };
        let submit = FakeSubmit::default();
        let m = msg();
        let err = gate_and_dispatch(&store, Some(&submit), Gate::Off, &req(&m), 1_000_000)
            .expect_err("gated");
        assert!(matches!(&err, SendError::Gated(h) if h.contains("Settings → Email")), "{err:?}");
        assert!(store.inserts.lock().unwrap().is_empty(), "no audit row in off mode");
        assert_eq!(submit.calls.load(Ordering::SeqCst), 0, "NOTHING submits");
    }

    #[test]
    fn approval_mode_queues_pending_and_never_touches_the_backend() {
        let store = FakeStore { transition_result: Some(true), ..Default::default() };
        let m = msg();
        // backend deliberately None: approval mode must not need one.
        let out = gate_and_dispatch(&store, None, Gate::Approval, &req(&m), 1_000_000)
            .expect("queued");
        assert!(matches!(out, SendOutcome::Queued { .. }), "{out:?}");
        let inserts = store.inserts.lock().unwrap();
        assert_eq!(inserts.as_slice(), &[("pending".to_string(), None)]);
    }

    /// PRE-MORTEM #11 (BINDING): the audit row write FAILS → nothing is
    /// handed to Stalwart. No row, no send.
    #[test]
    fn failed_audit_row_insert_never_submits() {
        let store = FakeStore {
            fail_insert: true,
            transition_result: Some(true),
            ..Default::default()
        };
        let submit = FakeSubmit::default();
        let m = msg();
        let err = gate_and_dispatch(&store, Some(&submit), Gate::On, &req(&m), 1_000_000)
            .expect_err("must refuse");
        assert!(matches!(err, SendError::Engine(_)));
        assert_eq!(
            submit.calls.load(Ordering::SeqCst),
            0,
            "no row, no send — the fail-closed invariant"
        );
        // Approval mode refuses on the same broken store (queue table
        // unreachable NEVER bypasses approval — pre-mortem #11).
        let err = gate_and_dispatch(&store, None, Gate::Approval, &req(&m), 1_000_000)
            .expect_err("must refuse");
        assert!(matches!(err, SendError::Engine(_)));
    }

    /// FAIL-CLOSED: an unreadable rate counter refuses the send before
    /// any row exists and before any submit.
    #[test]
    fn unreadable_rate_counter_refuses_send() {
        let store = FakeStore {
            fail_count: true,
            transition_result: Some(true),
            ..Default::default()
        };
        let submit = FakeSubmit::default();
        let m = msg();
        let err = gate_and_dispatch(&store, Some(&submit), Gate::On, &req(&m), 1_000_000)
            .expect_err("must refuse");
        assert!(matches!(&err, SendError::Engine(h) if h.contains("fail-closed")), "{err:?}");
        assert!(store.inserts.lock().unwrap().is_empty());
        assert_eq!(submit.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn on_mode_audits_then_submits_then_marks_sent() {
        let store = FakeStore { transition_result: Some(true), ..Default::default() };
        let submit = FakeSubmit::default();
        let m = msg();
        let out = gate_and_dispatch(&store, Some(&submit), Gate::On, &req(&m), 1_000_000)
            .expect("submitted");
        assert!(matches!(out, SendOutcome::Submitted { .. }), "{out:?}");
        assert_eq!(submit.calls.load(Ordering::SeqCst), 1);
        // Row inserted as policy-approved BEFORE the submit; then
        // approved → sent.
        assert_eq!(
            store.inserts.lock().unwrap().as_slice(),
            &[("approved".to_string(), Some("policy:agent-send-on".to_string()))]
        );
        assert_eq!(
            store.transitions.lock().unwrap().as_slice(),
            &[("out_fake00000001".into(), "approved".into(), "sent".into())]
        );
        // On mode WITHOUT a backend refuses (fail-closed), no row.
        let store2 = FakeStore { transition_result: Some(true), ..Default::default() };
        let err = gate_and_dispatch(&store2, None, Gate::On, &req(&m), 1_000_000)
            .expect_err("must refuse");
        assert!(matches!(err, SendError::Engine(_)));
        assert!(store2.inserts.lock().unwrap().is_empty());
    }

    #[test]
    fn on_mode_submit_failure_marks_failed_with_note() {
        let store = FakeStore { transition_result: Some(true), ..Default::default() };
        let submit = FakeSubmit { fail: true, ..Default::default() };
        let m = msg();
        let out = gate_and_dispatch(&store, Some(&submit), Gate::On, &req(&m), 1_000_000)
            .expect("audited outcome");
        match out {
            SendOutcome::SubmitFailed { error, .. } => {
                assert!(error.contains("connection refused"))
            }
            other => panic!("expected SubmitFailed, got {other:?}"),
        }
        assert_eq!(
            store.transitions.lock().unwrap().as_slice(),
            &[("out_fake00000001".into(), "approved".into(), "failed".into())]
        );
        assert!(store.notes.lock().unwrap()[0].contains("submit failed"));
    }

    // ── rate limits ──

    #[test]
    fn hourly_and_daily_caps_produce_distinct_errors() {
        let m = msg();
        let submit = FakeSubmit::default();
        // Hourly cap.
        let store = FakeStore {
            counts: (20, 20),
            transition_result: Some(true),
            ..Default::default()
        };
        let err = gate_and_dispatch(&store, Some(&submit), Gate::On, &req(&m), 1_000_000)
            .expect_err("capped");
        assert!(
            matches!(&err, SendError::RateLimited { window: "hour", .. }),
            "{err:?}"
        );
        // Daily cap (hour under, day over).
        let store = FakeStore {
            counts: (5, 100),
            transition_result: Some(true),
            ..Default::default()
        };
        let err = gate_and_dispatch(&store, Some(&submit), Gate::On, &req(&m), 1_000_000)
            .expect_err("capped");
        assert!(
            matches!(&err, SendError::RateLimited { window: "day", .. }),
            "{err:?}"
        );
        assert_eq!(submit.calls.load(Ordering::SeqCst), 0, "caps precede any submit");
        assert!(store.inserts.lock().unwrap().is_empty(), "caps precede the audit row");
    }

    // ── approve / deny (atomic + fail-closed) ──

    fn pending_row(id: &str) -> MailOutbound {
        MailOutbound {
            id: id.to_string(),
            owner_project_id: "P1".to_string(),
            agent_name: "bot".to_string(),
            from_address: "bot@acme.dev".to_string(),
            to_json: r#"["human@example.com"]"#.to_string(),
            cc_json: None,
            subject: "Weekly digest".to_string(),
            body_ref: Some("/tmp/x.json".to_string()),
            attachments_ref: None,
            status: "pending".to_string(),
            decided_by: None,
            note: None,
            created_at: 1_000_000,
            updated_at: 1_000_000,
            decided_at: None,
            sent_at: None,
        }
    }

    #[test]
    fn approve_submits_only_on_the_atomic_pending_transition() {
        let submit = FakeSubmit::default();
        let mut acct = |_from: &str| Ok("acc-1".to_string());

        // Happy path.
        let store = FakeStore {
            row: Some(pending_row("out_a")),
            transition_result: Some(true),
            ..Default::default()
        };
        let out = approve_and_submit(&store, Ok(&submit), &mut acct, "out_a", "owner", None, 2_000)
            .expect("approved");
        assert_eq!(out, ApproveOutcome::Submitted);
        assert_eq!(submit.calls.load(Ordering::SeqCst), 1);

        // AMBIGUOUS/decided row: transition doesn't fire → NO submit
        // (pre-mortem #11 — an unknown approval state never sends).
        let submit = FakeSubmit::default();
        let mut decided = pending_row("out_b");
        decided.status = "denied".to_string();
        let store = FakeStore {
            row: Some(decided),
            transition_result: Some(false),
            ..Default::default()
        };
        let err = approve_and_submit(&store, Ok(&submit), &mut acct, "out_b", "owner", None, 2_000)
            .expect_err("conflict");
        assert!(matches!(&err, SendError::Conflict(h) if h.contains("rejected")), "{err:?}");
        assert_eq!(submit.calls.load(Ordering::SeqCst), 0, "no submit on a decided row");

        // TRANSITION ERROR (db died mid-flow): refuse, NO submit.
        let submit = FakeSubmit::default();
        let store = FakeStore {
            row: Some(pending_row("out_c")),
            transition_result: None,
            ..Default::default()
        };
        let err = approve_and_submit(&store, Ok(&submit), &mut acct, "out_c", "owner", None, 2_000)
            .expect_err("engine");
        assert!(matches!(&err, SendError::Engine(h) if h.contains("nothing sent")), "{err:?}");
        assert_eq!(submit.calls.load(Ordering::SeqCst), 0, "db failure never fails open");

        // Unknown id: masked not_found, no submit.
        let submit = FakeSubmit::default();
        let store = FakeStore { row: None, transition_result: Some(true), ..Default::default() };
        let err = approve_and_submit(&store, Ok(&submit), &mut acct, "out_d", "owner", None, 2_000)
            .expect_err("not found");
        assert!(matches!(err, SendError::NotFound(_)));
        assert_eq!(submit.calls.load(Ordering::SeqCst), 0);

        // Engine unavailable: row flips to failed, nothing dials out.
        let store = FakeStore {
            row: Some(pending_row("out_e")),
            transition_result: Some(true),
            ..Default::default()
        };
        let out = approve_and_submit(
            &store,
            Err("not installed".to_string()),
            &mut acct,
            "out_e",
            "owner",
            None,
            2_000,
        )
        .expect("failed outcome");
        assert!(matches!(out, ApproveOutcome::FailedToSubmit(_)));
        assert!(store
            .transitions
            .lock()
            .unwrap()
            .iter()
            .any(|(_, f, t)| f == "approved" && t == "failed"));
    }

    #[test]
    fn deny_is_atomic_and_conflicts_loudly() {
        let store = FakeStore {
            row: Some(pending_row("out_a")),
            transition_result: Some(true),
            ..Default::default()
        };
        deny(&store, "out_a", "owner", "not now", 2_000).expect("denied");
        let store = FakeStore { row: None, transition_result: Some(true), ..Default::default() };
        assert!(matches!(
            deny(&store, "out_x", "owner", "n", 2_000),
            Err(SendError::NotFound(_))
        ));
        let mut sent = pending_row("out_s");
        sent.status = "sent".to_string();
        let store = FakeStore {
            row: Some(sent),
            transition_result: Some(false),
            ..Default::default()
        };
        assert!(matches!(
            deny(&store, "out_s", "owner", "n", 2_000),
            Err(SendError::Conflict(_))
        ));
    }

    // ── recipients / sizes / message shape ──

    #[test]
    fn recipients_normalize_punycode_and_dedupe() {
        let got = normalize_recipients(&[
            "Human@Example.COM ".to_string(),
            "human@example.com".to_string(),
            "kollege@münchen.example".to_string(),
        ])
        .expect("valid");
        assert_eq!(
            got,
            vec![
                "human@example.com".to_string(),
                "kollege@xn--mnchen-3ya.example".to_string(),
            ]
        );
        for bad in ["not-an-address", "@nolocal.example", "x@"] {
            assert!(
                matches!(
                    normalize_recipients(&[bad.to_string()]),
                    Err(SendError::Usage(_))
                ),
                "{bad}"
            );
        }
    }

    #[test]
    fn submission_email_json_carries_threading_only_when_set() {
        let mut m = msg();
        let v = submission_email_json(&m);
        assert_eq!(v["from"][0]["email"], "bot@acme.dev");
        assert_eq!(v["from"][0]["name"], "Research Bot");
        assert_eq!(v["to"][0]["email"], "human@example.com");
        assert_eq!(v["bodyValues"]["k2body"]["value"], "All green.");
        assert_eq!(v["textBody"][0]["partId"], "k2body");
        assert!(v.get("cc").is_none());
        assert!(v.get("header:In-Reply-To:asText").is_none());

        m.cc = vec!["boss@example.com".to_string()];
        m.in_reply_to = Some("<orig@github.com>".to_string());
        m.references = Some("<r1@x> <orig@github.com>".to_string());
        let v = submission_email_json(&m);
        assert_eq!(v["cc"][0]["email"], "boss@example.com");
        assert_eq!(v["header:In-Reply-To:asText"], "<orig@github.com>");
        assert_eq!(v["header:References:asText"], "<r1@x> <orig@github.com>");
        assert_eq!(m.all_recipients().len(), 2);
    }

    // ── reply guardrails ──

    fn ctx(from_email: &str, dmarc: &str, refs: Option<&str>) -> ReplyContext {
        ReplyContext {
            from: vec![MailAddr { name: None, email: from_email.to_string() }],
            subject: "Verify".to_string(),
            thread_id: Some("T1".to_string()),
            message_id: Some("<m1@x>".to_string()),
            references: refs.map(String::from),
            auth_results: vec![format!("mx; spf=pass; dkim=pass; dmarc={dmarc}")],
        }
    }

    #[test]
    fn reply_guardrails_lock_recipient_and_refuse_loops_and_dmarc_fail() {
        // Recipient is LOCKED to the original sender, normalized.
        let c = ctx("Sender@Example.COM", "pass", None);
        let rcpt = check_reply_guardrails(&c, "bot@acme.dev", Gate::On).expect("allowed");
        assert_eq!(rcpt, "sender@example.com");

        // Reply-to-self refused.
        let c = ctx("bot@acme.dev", "pass", None);
        let err = check_reply_guardrails(&c, "bot@acme.dev", Gate::On).expect_err("loop");
        assert!(matches!(&err, SendError::Guardrail(h) if h.contains("loop")), "{err:?}");

        // No sender refused.
        let mut c = ctx("x@y.z", "pass", None);
        c.from.clear();
        assert!(matches!(
            check_reply_guardrails(&c, "bot@acme.dev", Gate::On),
            Err(SendError::Guardrail(_))
        ));

        // >100 references refused (thread-depth loop cap).
        let deep = "<r@x> ".repeat(101);
        let c = ctx("sender@example.com", "pass", Some(deep.trim()));
        let err = check_reply_guardrails(&c, "bot@acme.dev", Gate::On).expect_err("deep");
        assert!(matches!(&err, SendError::Guardrail(h) if h.contains("References")), "{err:?}");
        // 100 exactly is allowed.
        let ok = "<r@x> ".repeat(100);
        let c = ctx("sender@example.com", "pass", Some(ok.trim()));
        check_reply_guardrails(&c, "bot@acme.dev", Gate::On).expect("at the cap");

        // DMARC fail: blocked in on-mode, allowed under approval (the
        // human overrides, §8.4).
        let c = ctx("sender@example.com", "fail", None);
        let err = check_reply_guardrails(&c, "bot@acme.dev", Gate::On).expect_err("dmarc");
        assert!(matches!(&err, SendError::Guardrail(h) if h.contains("DMARC")), "{err:?}");
        check_reply_guardrails(&c, "bot@acme.dev", Gate::Approval)
            .expect("approval mode lets the human decide");
    }

    #[test]
    fn reply_subject_and_references_chain() {
        assert_eq!(reply_subject("Hello"), "Re: Hello");
        assert_eq!(reply_subject("  RE: Hello"), "RE: Hello");
        assert_eq!(reply_subject("re: hello"), "re: hello");
        assert_eq!(count_references(None), 0);
        assert_eq!(count_references(Some("<a@x> <b@x>")), 2);
        assert_eq!(
            build_out_references(Some("<a@x>"), Some("<b@x>")).as_deref(),
            Some("<a@x> <b@x>")
        );
        assert_eq!(build_out_references(None, Some("<b@x>")).as_deref(), Some("<b@x>"));
        assert_eq!(build_out_references(Some("<a@x>"), None).as_deref(), Some("<a@x>"));
        assert_eq!(build_out_references(None, None), None);
    }

    #[test]
    fn thread_reply_cap_is_a_sliding_hour_window() {
        let key = format!("test-thread-{}", uuid::Uuid::new_v4());
        for i in 0..MAX_THREAD_REPLIES_PER_HOUR {
            check_and_record_thread_reply(&key, 1_000_000 + i as i64).expect("under cap");
        }
        let err = check_and_record_thread_reply(&key, 1_000_100).expect_err("capped");
        assert!(matches!(err, SendError::RateLimited { window: "thread", .. }));
        // An hour later the window slid open again.
        check_and_record_thread_reply(&key, 1_000_000 + 3_601).expect("window slid");
    }

    // ── wire statuses ──

    #[test]
    fn wire_statuses_match_the_gate_findings() {
        assert_eq!(wire_status("pending"), "pending_approval");
        assert_eq!(wire_status("approved"), "approved");
        assert_eq!(wire_status("denied"), "rejected");
        assert_eq!(wire_status("sent"), "submitted", "never 'delivered' (pre-mortem #9)");
        assert_eq!(wire_status("failed"), "failed");
        assert_eq!(wire_status("garbage"), "unknown");
        assert!(status_note("sent").contains("accepted-for-delivery"));
    }

    // ── wait_for_decision (fake clock — no real sleeps) ──

    #[test]
    fn wait_for_decision_returns_decision_or_timeout_on_fake_clock() {
        // Decided on the 3rd poll.
        let mut polls = 0;
        let mut poll = || {
            polls += 1;
            Ok(if polls >= 3 {
                ("denied".to_string(), Some("not now".to_string()))
            } else {
                ("pending".to_string(), None)
            })
        };
        let t = std::cell::Cell::new(0i64);
        let mut now = || t.get();
        let mut sleep = |d: std::time::Duration| t.set(t.get() + d.as_secs() as i64);
        let got = wait_for_decision(&mut poll, 300, 2, &mut now, &mut sleep)
            .expect("ok")
            .expect("decided");
        assert_eq!(got, ("denied".to_string(), Some("not now".to_string())));

        // Never decided → timeout (None), bounded polls.
        let mut polls = 0usize;
        let mut poll = || {
            polls += 1;
            Ok(("pending".to_string(), None))
        };
        let t = std::cell::Cell::new(0i64);
        let mut now = || t.get();
        let mut sleep = |d: std::time::Duration| t.set(t.get() + d.as_secs() as i64);
        let got =
            wait_for_decision(&mut poll, 10, 2, &mut now, &mut sleep).expect("ok");
        assert!(got.is_none(), "timeout → None (message still pending)");
        assert_eq!(polls, 6, "bounded poll count on the fake clock");

        // Poll errors surface loudly.
        let mut poll = || Err("row vanished".to_string());
        let t = std::cell::Cell::new(0i64);
        let mut now = || t.get();
        let mut sleep = |_: std::time::Duration| {};
        let err = wait_for_decision(&mut poll, 10, 2, &mut now, &mut sleep)
            .expect_err("loud");
        assert!(err.contains("vanished"));
    }

    // ── the production store (shared test DB + temp body dir) ──

    fn temp_store() -> (DbOutboundStore, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "k2-mail-outbound-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        (DbOutboundStore::at(dir.clone()), dir)
    }

    fn cleanup_rows(project_id: &str) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = conn.execute(
            "DELETE FROM mail_outbound WHERE owner_project_id = ?1",
            rusqlite::params![project_id],
        );
    }

    #[test]
    fn db_store_roundtrip_transitions_counts_and_0600_bodies() {
        let (store, dir) = temp_store();
        let project = format!("P-{}", uuid::Uuid::new_v4());
        // Unique From: rate counting is per address and the DB is the
        // shared test DB — a fixed address would race sibling tests.
        let mut m = msg();
        m.from = format!("bot-{}@acme.dev", uuid::Uuid::new_v4().simple());
        let id = store
            .insert(&NewOutbound {
                owner_project_id: &project,
                agent_name: "bot",
                message: &m,
                status: "pending",
                decided_by: None,
                now: 1_000_000,
            })
            .expect("insert");
        assert!(id.starts_with("out_"), "{id}");

        // The body file is 0600 and rounds the message back.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join(format!("{id}.json")))
                .expect("body file")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "body file must be owner-only");
        }
        assert_eq!(store.load_message(&id).expect("message"), m);

        // The row loads with the audit fields.
        let row = store.load(&id).expect("load").expect("row");
        assert_eq!(row.status, "pending");
        assert_eq!(row.from_address, m.from);
        assert_eq!(row.subject, "Weekly digest");
        assert!(row.body_ref.as_deref().unwrap().ends_with(".json"));
        assert!(store.load("out_nope").expect("load").is_none());

        // Rate counting respects the window boundary.
        assert_eq!(store.count_recent(&m.from, 1_000_000).unwrap(), 1);
        assert_eq!(store.count_recent(&m.from, 1_000_001).unwrap(), 0);
        assert_eq!(store.count_recent("other@acme.dev", 0).unwrap(), 0);

        // Transitions are ATOMIC on the current status.
        assert!(store
            .transition(&id, "pending", "denied", Some("owner"), Some("no"), 1_000_500)
            .unwrap());
        assert!(
            !store
                .transition(&id, "pending", "approved", Some("owner"), None, 1_000_600)
                .unwrap(),
            "a decided row never re-transitions from pending"
        );
        let row = store.load(&id).expect("load").expect("row");
        assert_eq!(row.status, "denied");
        assert_eq!(row.decided_by.as_deref(), Some("owner"));
        assert_eq!(row.note.as_deref(), Some("no"));
        assert_eq!(row.decided_at, Some(1_000_500));

        store.append_note(&id, "extra line", 1_000_700).expect("note");
        let row = store.load(&id).expect("load").expect("row");
        assert_eq!(row.note.as_deref(), Some("no\nextra line"));

        cleanup_rows(&project);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn expired_pending_rows_auto_deny() {
        let (store, dir) = temp_store();
        let project = format!("P-{}", uuid::Uuid::new_v4());
        let m = msg();
        let now = 10_000_000;
        let old = store
            .insert(&NewOutbound {
                owner_project_id: &project,
                agent_name: "bot",
                message: &m,
                status: "pending",
                decided_by: None,
                now: now - APPROVAL_EXPIRE_SECS - 10,
            })
            .expect("insert old");
        let fresh = store
            .insert(&NewOutbound {
                owner_project_id: &project,
                agent_name: "bot",
                message: &m,
                status: "pending",
                decided_by: None,
                now: now - 100,
            })
            .expect("insert fresh");
        // NOTE: auto_deny_expired sweeps the WHOLE table; other tests'
        // rows are younger than 7 days by construction (now_secs), so
        // only ours can flip here.
        assert!(auto_deny_expired(now) >= 1);
        let old_row = store.load(&old).expect("load").expect("row");
        assert_eq!(old_row.status, "denied");
        assert_eq!(old_row.decided_by.as_deref(), Some("system:expired"));
        assert!(old_row.note.as_deref().unwrap().contains("expired"));
        let fresh_row = store.load(&fresh).expect("load").expect("row");
        assert_eq!(fresh_row.status, "pending", "fresh items stay queued");
        cleanup_rows(&project);
        let _ = std::fs::remove_dir_all(dir);
    }

    // ── domain send-mode / relay validation (shared DB seeds) ──

    struct MapSecrets(HashMap<String, String>, bool /* fail */);
    impl SecretStore for MapSecrets {
        fn store(&self, _k: &str, _s: &str) -> Result<String, String> {
            unreachable!()
        }
        fn resolve(&self, sref: &str) -> Result<Option<String>, String> {
            if self.1 {
                return Err("store corrupt".to_string());
            }
            Ok(self.0.get(sref).cloned())
        }
        fn delete(&self, _sref: &str) -> Result<(), String> {
            unreachable!()
        }
    }

    fn seed_domain(domain: &str, send_mode: &str, relay_config_id: Option<&str>) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO mail_domains (id, domain, send_mode, relay_config_id, status, \
             created_at) VALUES (?1, ?2, ?3, ?4, 'verified', 100)",
            rusqlite::params![id, domain, send_mode, relay_config_id],
        )
        .expect("seed domain");
        id
    }

    fn seed_relay(
        kind: &str,
        host: Option<&str>,
        port: Option<i64>,
        secret_ref: Option<&str>,
    ) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO mail_relay_configs (id, kind, host, port, username, secret_ref, \
             tls_kind, created_at) VALUES (?1, ?2, ?3, ?4, 'u', ?5, 'starttls', 100)",
            rusqlite::params![id, kind, host, port, secret_ref],
        )
        .expect("seed relay");
        id
    }

    fn cleanup_domain(domain: &str, relay_id: Option<&str>) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = conn.execute(
            "DELETE FROM mail_domains WHERE domain = ?1",
            rusqlite::params![domain],
        );
        if let Some(r) = relay_id {
            let _ = conn.execute(
                "DELETE FROM mail_relay_configs WHERE id = ?1",
                rusqlite::params![r],
            );
        }
    }

    #[test]
    fn domain_send_modes_enforce_per_prd() {
        let secrets = MapSecrets(HashMap::new(), false);
        let uniq = uuid::Uuid::new_v4().simple().to_string();

        // Unknown domain → fail-closed engine error.
        let err = check_domain_send_mode(&secrets, &format!("x@ghost-{uniq}.example"))
            .expect_err("refuse");
        assert!(matches!(&err, SendError::Engine(h) if h.contains("fail-closed")), "{err:?}");

        // receive-only → the teaching error.
        let d = format!("ro-{uniq}.example");
        seed_domain(&d, "receive-only", None);
        let err = check_domain_send_mode(&secrets, &format!("bot@{d}")).expect_err("refuse");
        assert!(
            matches!(&err, SendError::SendMode(h) if h.contains("receive-only")
                && h.contains("Settings → Email")),
            "{err:?}"
        );
        cleanup_domain(&d, None);

        // direct → allowed.
        let d = format!("dir-{uniq}.example");
        seed_domain(&d, "direct", None);
        check_domain_send_mode(&secrets, &format!("bot@{d}")).expect("direct sends");
        cleanup_domain(&d, None);
    }

    #[test]
    fn relay_mode_validates_config_kind_agnostically_and_fail_closed() {
        let uniq = uuid::Uuid::new_v4().simple().to_string();

        // Relay mode without a config → teaching send_mode error.
        let d = format!("r0-{uniq}.example");
        seed_domain(&d, "relay", None);
        let secrets = MapSecrets(HashMap::new(), false);
        let err = check_domain_send_mode(&secrets, &format!("bot@{d}")).expect_err("refuse");
        assert!(matches!(&err, SendError::SendMode(h) if h.contains("no relay")), "{err:?}");
        cleanup_domain(&d, None);

        // Complete smtp relay with resolvable creds → allowed.
        let d = format!("r1-{uniq}.example");
        let relay = seed_relay("smtp", Some("smtp.relay.example"), Some(587), Some("ref-ok"));
        seed_domain(&d, "relay", Some(&relay));
        let secrets = MapSecrets(
            HashMap::from([("ref-ok".to_string(), "s3cret".to_string())]),
            false,
        );
        check_domain_send_mode(&secrets, &format!("bot@{d}")).expect("relay sends");

        // Missing creds in the store → refuse; a FAILING store →
        // fail-closed engine error.
        let empty = MapSecrets(HashMap::new(), false);
        let err = check_domain_send_mode(&empty, &format!("bot@{d}")).expect_err("refuse");
        assert!(matches!(&err, SendError::SendMode(h) if h.contains("credentials")), "{err:?}");
        let broken = MapSecrets(HashMap::new(), true);
        let err = check_domain_send_mode(&broken, &format!("bot@{d}")).expect_err("refuse");
        assert!(matches!(&err, SendError::Engine(h) if h.contains("fail-closed")), "{err:?}");
        cleanup_domain(&d, Some(&relay));

        // Incomplete smtp config (no host) → refuse.
        let d = format!("r2-{uniq}.example");
        let relay = seed_relay("smtp", None, Some(587), Some("ref-ok"));
        seed_domain(&d, "relay", Some(&relay));
        let secrets = MapSecrets(
            HashMap::from([("ref-ok".to_string(), "s3cret".to_string())]),
            false,
        );
        let err = check_domain_send_mode(&secrets, &format!("bot@{d}")).expect_err("refuse");
        assert!(matches!(&err, SendError::SendMode(h) if h.contains("incomplete")), "{err:?}");
        cleanup_domain(&d, Some(&relay));

        // A non-smtp kind refuses with the teaching error (V1) —
        // nothing ASSUMED kind == smtp on the way here.
        let d = format!("r3-{uniq}.example");
        let relay = seed_relay("mailgun", None, None, None);
        seed_domain(&d, "relay", Some(&relay));
        let err = check_domain_send_mode(&secrets, &format!("bot@{d}")).expect_err("refuse");
        assert!(matches!(&err, SendError::SendMode(h) if h.contains("mailgun")), "{err:?}");
        cleanup_domain(&d, Some(&relay));
    }
}
