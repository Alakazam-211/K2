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
/// Longest agent-chosen From display name (`k2 mail send --from-name`),
/// in characters (Unicode scalar values, after trimming).
pub const MAX_FROM_NAME_CHARS: usize = 128;
/// The most text a HOSTED send can hand the mail server. The text rides the
/// JSON of one JMAP request and Stalwart refuses a request over its
/// `maxSizeRequest` (10,000,000 bytes, tests/mail/stalwart-attachments-probe.py
/// A9), so this counts the JSON-escaped text and leaves room for the rest of
/// the request. Attachments ride uploads and don't count.
pub const MAX_HOSTED_TEXT_JSON_BYTES: usize = 9_500_000;
/// Most `--reply-to` addresses on one message.
pub const MAX_REPLY_TO: usize = 3;
/// Most bytes of attachments this box holds at once, for messages waiting for
/// approval or a send time, across every workspace.
pub const MAX_HELD_BYTES: u64 = 5 * 1024 * 1024 * 1024;
/// One workspace's share of [`MAX_HELD_BYTES`], so one agent can't block the
/// others' files for days.
pub const MAX_HELD_BYTES_PER_WORKSPACE: u64 = 1024 * 1024 * 1024;
/// Free disk that must remain after holding another message's files.
pub const MIN_FREE_AFTER_HOLD: u64 = 2 * 1024 * 1024 * 1024;
/// Waiting text at or under this needs no disk check.
pub const TEXT_FLOOR_FROM_BYTES: u64 = 1024 * 1024;
/// A decided message's stored body goes after this long; its row stays.
pub const DECIDED_BODY_KEEP_SECS: i64 = 30 * 86_400;
/// A file with no row is older than any send in progress after this long.
const ORPHAN_AFTER_SECS: i64 = 3600;
/// Largest body file read back (10 MB of text, plus a legacy linked message's
/// 25 MiB of files in base64).
const MAX_BODY_FILE_BYTES: u64 = 64 * 1024 * 1024;
/// A message with files can be scheduled at most this far ahead (the box holds
/// the bytes until it leaves).
pub const MAX_HELD_SCHEDULE_SECS: i64 = 7 * 86_400;
/// §8.4 loop caps.
pub const MAX_REPLY_REFERENCES: usize = 100;
pub const MAX_THREAD_REPLIES_PER_HOUR: usize = 4;
/// §12: pending approvals auto-deny after 7 days.
pub const APPROVAL_EXPIRE_SECS: i64 = 7 * 86_400;
/// `send --wait` poll cadence inside the held request (§11.1.4 —
/// timeout bounds reuse the S4 wait constants).
pub const DECISION_POLL_SECS: u64 = 2;
/// Max horizon for `--at` / `--in` scheduling (30 days).
pub const MAX_SCHEDULE_SECS: i64 = 30 * 86_400;
/// Scheduled-send flusher tick (overridable via K2_MAIL_SCHEDULED_TICK_SECS).
pub const DEFAULT_SCHEDULED_TICK_SECS: u64 = 15;

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

/// Where the owner changes a workspace's agent-send mode. Both controls
/// exist on every OS (agent-send gates linked/BYO sends too, which run on
/// macOS): the `k2 mail sending` owner command and the per-workspace
/// Settings control. Every `gated` hint names them, never a hosted-only
/// page.
pub const AGENT_SEND_CONTROLS: &str =
    "`k2 mail sending <workspace> on` (the owner runs it from a terminal outside a K2 \
     agent session) or Settings → Workspaces / Agents → <workspace> → Email sending";

/// The `gated` hint for a workspace whose agent-send mode is `off`.
pub fn agent_send_off_hint() -> String {
    format!(
        "outbound email is disabled for this workspace. Your human can turn it on with \
         {AGENT_SEND_CONTROLS}"
    )
}

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
             your human can reset it with `k2 mail sending <workspace> off|approval|on` or \
             Settings → Workspaces / Agents → <workspace> → Email sending"
        ))),
        Err(e) => Err(SendError::Engine(format!(
            "could not read the send-gating setting — refusing to send (fail-closed): {e}"
        ))),
    }
}

/// Resolve the effective send gate for a workspace path via the
/// sanctioned settings reader (fail-closed to `off` there; unrecognized
/// stored values still surface as Engine via [`effective_gate`]).
pub fn gate_for_path(project_path: &str) -> Result<Gate, SendError> {
    effective_gate(Ok(
        k2_core::workspace::settings::mail_agent_send_for_path(project_path),
    ))
}

/// Linked/BYO send gate (hole #3 / E4). Same `mail_agent_send` setting
/// as hosted: **off** refuses, **on** allows, and — since MCS2's linked
/// approval hand-off (prd-app-mail-calendar-surfaces-v1 §3.3, MCS-A5) —
/// **approval** queues: an approved linked message leaves through its own
/// provider ([`approve_and_submit_with`] resolves a
/// [`crate::mail::submit::MailSubmit`] from the From address).
pub fn allow_linked_send(gate: Gate) -> Result<(), SendError> {
    match gate {
        Gate::On | Gate::Approval => Ok(()),
        Gate::Off => Err(SendError::Gated(agent_send_off_hint())),
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
    /// Blind copies the AGENT asked for (`--bcc`). ENVELOPE ONLY — never
    /// rendered into a header ([`submission_email_json`] has no bcc), so
    /// To/Cc recipients can't see them. Stored in the body file so they
    /// survive the approval queue and scheduled sends.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bcc: Vec<String>,
    /// Blind copies the OWNER's "always BCC" policy added at send time
    /// (server-stamped, never from the request body). Envelope only,
    /// audited apart from [`Self::bcc`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub policy_bcc: Vec<String>,
    /// The Reply-To header (`--reply-to`), HOSTED only. A header, not a
    /// recipient: nothing is sent there, so it is not in the envelope and
    /// doesn't count toward the recipient cap. It steers where answers go, so
    /// it shows in the approval view and the outbox (`reply_to_json`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reply_to: Vec<String>,
    pub subject: String,
    pub text_body: String,
    /// Reply threading (§8.4): the original's Message-ID.
    #[serde(default)]
    pub in_reply_to: Option<String>,
    /// Reply threading: original References + original Message-ID.
    #[serde(default)]
    pub references: Option<String>,
    /// MCS2, LINKED replies: the backend id of the message being answered
    /// (`uid:…` IMAP, `graph:…` Graph). The provider re-reads it at send
    /// time and locks the recipient to its sender (SMTP threading /
    /// Graph `createReply`). `None` for a fresh message and for hosted
    /// replies (which carry their own threading headers above).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_source: Option<String>,
    /// The attachment files, captured when the message is composed: what an
    /// approver sees is what leaves, whatever happens to the workspace file
    /// afterwards. A HOSTED message always carries them here; a LINKED one
    /// only while it waits for approval (MCS2). Stored base64 inside the 0600
    /// body file, never in the DB, never logged, and dropped from the file
    /// once the message is decided ([`at_rest`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<StoredAttachment>,
    /// A one-time-link mail (an App user's invite or reset,
    /// agent-app-users): the server destroys its stored copy as soon as
    /// it takes the message (JMAP `onSuccessDestroyEmail`), so no Drafts or
    /// Sent copy keeps the link in the sending mailbox, and previews hide
    /// the link.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ephemeral: bool,
    /// Email Notifications (E15): a notification held for the owner's
    /// approval (`k2 app email send` under `approval`) carries what the
    /// plain message has no field for. Its From is a notification sender;
    /// the approvals resolver hands it to `email_notify`, never to the
    /// mailbox path. `None` for every other message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify: Option<NotifyHeld>,
    /// `k2 mail forward-message`: what this message forwards, for whoever
    /// decides it. Never a header. `None` for every other message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forward_of: Option<ForwardOf>,
    /// A reply or forward: the DMARC verdict this box stamped on the original,
    /// for whoever decides. `None` = no verdict, or no original.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_dmarc: Option<String>,
    /// A reply or forward whose original the sender check didn't clear: why, in
    /// words that follow "the original message". The approver sees it, and the
    /// one-click card refuses (`source_check` on the row).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_check: Option<String>,
}

/// What a forward forwards ([`OutboundMessage::forward_of`]): shown to the
/// approver, because an `.eml` forward's text may say nothing about it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForwardOf {
    /// The mailbox the original is in.
    pub mailbox: String,
    /// Who the original says sent it (cleaned, unverified).
    pub sender: String,
    /// True when the original rides whole as a message/rfc822 file.
    #[serde(default)]
    pub as_eml: bool,
}

/// The parts of a held notification beyond [`OutboundMessage`] (E15).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotifyHeld {
    /// The `email_notify_log` row it belongs to.
    pub log_id: String,
    /// The App it is sent for.
    pub app: String,
    #[serde(default)]
    pub html: Option<String>,
    #[serde(default)]
    pub reply_to: Vec<String>,
    #[serde(default)]
    pub headers: std::collections::BTreeMap<String, String>,
}

/// The copy of `msg` kept at rest once nothing needs to send it again
/// (`~/.k2/mail-outbound/out_*.json`): never the attachment bytes (the outbox
/// keeps their names), and a one-time-link mail (`ephemeral`) keeps its text
/// with every link hidden (verification review 2026-10-26, item 4). Any other
/// mail as is.
pub fn at_rest(msg: &OutboundMessage) -> std::borrow::Cow<'_, OutboundMessage> {
    if !msg.ephemeral && msg.attachments.is_empty() {
        return std::borrow::Cow::Borrowed(msg);
    }
    let mut m = msg.clone();
    m.attachments = Vec::new();
    if m.ephemeral {
        m.subject = hide_links(&m.subject);
        m.text_body = hide_links(&m.text_body);
    }
    std::borrow::Cow::Owned(m)
}

/// A message's text as the person deciding it sees it: exactly what leaves,
/// except that a one-time-link mail hides its links. Only that kind: anything
/// else could hide data from the approver by dressing it as a link.
pub fn text_for_approver(msg: &OutboundMessage) -> String {
    if msg.ephemeral {
        hide_links(&msg.text_body)
    } else {
        msg.text_body.clone()
    }
}

/// `k2inv_…` tokens in `text` replaced by `k2inv_[hidden]` (previews of
/// one-time-link mails).
pub fn hide_links(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("k2inv_") {
        out.push_str(&rest[..i]);
        out.push_str("k2inv_[hidden]");
        let after = &rest[i + "k2inv_".len()..];
        let n = after.find(|c: char| !c.is_ascii_alphanumeric()).unwrap_or(after.len());
        rest = &after[n..];
    }
    out.push_str(rest);
    out
}

/// One attachment of a composed message. The body file keeps this entry; the
/// bytes live in their own 0600 file beside it (`<id>.files/<n>`), so listing
/// the approvals queue never reads them. `bytes` is in memory only while a
/// message is being sent. `Debug` prints the size, never the bytes.
#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredAttachment {
    pub filename: String,
    pub content_type: String,
    /// Bytes; recorded so a queue can be listed without reading them.
    #[serde(default)]
    pub size: u64,
    /// Hex SHA-256: checked when the bytes are read back, and kept in the
    /// outbox as the record of what left.
    #[serde(default)]
    pub sha256: String,
    /// The content scan read all of it (`secret_paths::scan_attachment`).
    /// False for a binary file and for what a forward copies from outside.
    #[serde(default)]
    pub scanned: bool,
    /// A message queued by an older K2 held its bytes here, base64.
    #[serde(default, rename = "dataB64", skip_serializing_if = "Option::is_none")]
    pub legacy_b64: Option<String>,
    #[serde(skip)]
    pub bytes: Option<std::sync::Arc<Vec<u8>>>,
}

impl std::fmt::Debug for StoredAttachment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredAttachment")
            .field("filename", &self.filename)
            .field("content_type", &self.content_type)
            .field("size", &self.size())
            .finish()
    }
}

/// Hex SHA-256 of some bytes.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

impl StoredAttachment {
    /// The file's size in bytes.
    pub fn size(&self) -> u64 {
        match &self.legacy_b64 {
            Some(b64) => {
                let b = b64.as_bytes();
                let pad = b.iter().rev().take(2).filter(|c| **c == b'=').count() as u64;
                (b.len() as u64 / 4 * 3).saturating_sub(pad)
            }
            None => self.size,
        }
    }

    /// What the approver is shown for this file.
    pub fn view_json(&self, one_based: usize) -> serde_json::Value {
        serde_json::json!({
            "n": one_based,
            "name": self.filename,
            "size": self.size(),
            "contentType": self.content_type,
            "scanned": self.scanned,
        })
    }

    pub fn from_out(a: &crate::mail::external_smtp::OutAttachment) -> StoredAttachment {
        StoredAttachment {
            filename: a.filename.clone(),
            content_type: a.content_type.clone(),
            size: a.bytes.len() as u64,
            sha256: sha256_hex(&a.bytes),
            scanned: a.scanned,
            legacy_b64: None,
            bytes: Some(std::sync::Arc::new(a.bytes.clone())),
        }
    }

    /// The file's bytes, from memory or a legacy body. A stored file has to be
    /// read back first ([`OutboundStore::load_message_full`]).
    pub fn bytes_now(&self) -> Result<Vec<u8>, String> {
        use base64::Engine as _;
        if let Some(b) = &self.bytes {
            return Ok(b.as_ref().clone());
        }
        match &self.legacy_b64 {
            Some(b64) => base64::engine::general_purpose::STANDARD
                .decode(b64.as_bytes())
                .map_err(|e| format!("stored attachment '{}' is corrupt: {e}", self.filename)),
            None => Err(format!("attachment '{}' wasn't read back", self.filename)),
        }
    }

    pub fn to_out(&self) -> Result<crate::mail::external_smtp::OutAttachment, String> {
        Ok(crate::mail::external_smtp::OutAttachment {
            filename: self.filename.clone(),
            content_type: self.content_type.clone(),
            bytes: self.bytes_now()?,
            scanned: self.scanned,
        })
    }
}

impl OutboundMessage {
    /// All SMTP envelope recipients (to + cc + agent bcc + policy bcc,
    /// in order, duplicates collapsed — one RCPT TO per address).
    pub fn all_recipients(&self) -> Vec<String> {
        let mut seen: HashSet<String> = HashSet::new();
        self.to
            .iter()
            .chain(self.cc.iter())
            .chain(self.bcc.iter())
            .chain(self.policy_bcc.iter())
            .filter(|a| seen.insert(a.to_ascii_lowercase()))
            .cloned()
            .collect()
    }

    /// Every blind (envelope-only) recipient: agent bcc + policy bcc.
    pub fn all_bcc(&self) -> Vec<String> {
        self.bcc.iter().chain(self.policy_bcc.iter()).cloned().collect()
    }
}

// ── BCC: the owner's "always BCC" policy + the recipient cap ────────────

/// Read + normalize the owner's "always BCC" policy for a workspace.
/// FAIL-CLOSED: an unreadable or corrupt policy refuses the send (the
/// owner's oversight copy is never silently dropped).
pub fn policy_bcc_for_path(project_path: &str) -> Result<Vec<String>, SendError> {
    let raw = k2_core::workspace::settings::mail_always_bcc_for_path(project_path).map_err(
        |e| {
            SendError::Engine(format!(
                "could not read this workspace's always-BCC policy — refusing to send \
                 (fail-closed): {e}"
            ))
        },
    )?;
    normalize_recipients(&raw).map_err(|e| {
        SendError::Engine(format!(
            "this workspace's always-BCC policy holds an invalid address — refusing to send \
             (fail-closed); the owner can fix it with 'k2 hostmail config --workspace <ws> \
             --always-bcc …': {e:?}"
        ))
    })
}

/// Stamp the policy BCCs onto a composed message. A policy address the
/// agent ALSO passed as `--bcc` is recorded as policy (not twice); the
/// agent's other BCCs stay as theirs. The caller has no way to drop a
/// policy entry — this runs server-side after the body is parsed.
pub fn apply_policy_bcc(msg: &mut OutboundMessage, policy: Vec<String>) {
    let policy_lc: HashSet<String> = policy.iter().map(|a| a.to_ascii_lowercase()).collect();
    msg.bcc.retain(|a| !policy_lc.contains(&a.to_ascii_lowercase()));
    msg.policy_bcc = policy;
}

/// The always-on §8.4 recipient cap over the FINAL envelope (to + cc +
/// bcc + policy bcc, duplicates collapsed). When the owner's policy is
/// what pushes it over, the error says so — a recipient is never
/// silently dropped to make room.
pub fn check_recipient_limit(msg: &OutboundMessage) -> Result<(), SendError> {
    let total = msg.all_recipients().len();
    if total <= MAX_RECIPIENTS {
        return Ok(());
    }
    if msg.policy_bcc.is_empty() {
        return Err(SendError::Usage(format!(
            "too many recipients ({total}) — max {MAX_RECIPIENTS} per message, counting to + \
             cc + bcc"
        )));
    }
    Err(SendError::Usage(format!(
        "too many recipients ({total}) — max {MAX_RECIPIENTS} per message, counting to + cc + \
         bcc. This workspace's owner policy always BCCs {} address(es) ({}), which count toward \
         the limit and can't be dropped — send to fewer recipients",
        msg.policy_bcc.len(),
        msg.policy_bcc.join(", ")
    )))
}

/// A fresh Message-ID for a message sent FROM `from_address`, without
/// the angle brackets (the JMAP `messageId` form): `<uuid>@<sender
/// domain>`. Without it Stalwart's composer stamps `@<machine
/// hostname>`, which leaks the box name and doesn't match the sender.
pub fn new_message_id_for(from_address: &str) -> String {
    let domain = from_address
        .rsplit_once('@')
        .map(|(_, d)| d)
        .filter(|d| !d.is_empty())
        .unwrap_or("localhost");
    format!("{}@{}", uuid::Uuid::new_v4().simple(), domain)
}

/// Refuse text a HOSTED send could never hand the mail server, up front with
/// a clear line ([`MAX_HOSTED_TEXT_JSON_BYTES`]; before this the send failed
/// at the engine with `maxSizeRequest`).
pub fn check_hosted_text_size(subject: &str, text: &str) -> Result<(), SendError> {
    let json_len = |s: &str| serde_json::to_string(s).map(|j| j.len()).unwrap_or(usize::MAX);
    let n = json_len(subject).saturating_add(json_len(text));
    if n > MAX_HOSTED_TEXT_JSON_BYTES {
        return Err(SendError::Usage(format!(
            "message text too large for a hosted mailbox ({n} bytes as sent to the mail server; \
             the limit is {MAX_HOSTED_TEXT_JSON_BYTES}) — shorten it, or attach the long part as a \
             file (on a forward, use --as-eml)"
        )));
    }
    Ok(())
}

/// One attachment uploaded to the mail server, ready to go in an Email object.
pub struct UploadedAttachment<'a> {
    pub blob_id: String,
    pub file: &'a StoredAttachment,
}

/// A stored `type/subtype[; charset=x]` split into what an RFC 8621 body part
/// takes: a bare `type`, and `charset` only on a text type.
fn jmap_type_and_charset(content_type: &str) -> (String, Option<String>) {
    let mut bits = content_type.split(';');
    let media = bits.next().unwrap_or("").trim().to_ascii_lowercase();
    if !media.starts_with("text/") {
        return (media, None);
    }
    let charset = bits.find_map(|p| {
        let (k, v) = p.split_once('=')?;
        k.trim().eq_ignore_ascii_case("charset").then(|| v.trim().trim_matches('"').to_ascii_lowercase())
    });
    (media, charset)
}

/// Pure builder: [`submission_email_json`] plus an `attachments` list naming
/// blobs already uploaded to the account. Stalwart's composer picks each
/// part's transfer encoding (probe A2, A10: the bytes arrive unchanged).
pub fn submission_email_json_with(
    msg: &OutboundMessage,
    uploaded: &[UploadedAttachment<'_>],
) -> serde_json::Value {
    let mut v = submission_email_json(msg);
    if !uploaded.is_empty() {
        let parts: Vec<serde_json::Value> = uploaded
            .iter()
            .map(|u| {
                let (media, charset) = jmap_type_and_charset(&u.file.content_type);
                let mut p = serde_json::json!({
                    "blobId": u.blob_id,
                    "type": media,
                    "name": u.file.filename,
                    "disposition": "attachment",
                });
                if let Some(c) = charset {
                    p["charset"] = serde_json::json!(c);
                }
                p
            })
            .collect();
        v["attachments"] = serde_json::Value::Array(parts);
    }
    v
}

/// Pure builder: the RFC 8621 Email object for `Email/set create`
/// (bodyValues text part; threading headers only when set). The JMAP
/// layer adds `mailboxIds` — everything content-shaped lives here so
/// it fixture-tests without a client. BCCs are deliberately ABSENT:
/// they ride only the submission envelope ([`OutboundMessage::all_recipients`]).
pub fn submission_email_json(msg: &OutboundMessage) -> serde_json::Value {
    let addr_list = |list: &[String]| -> Vec<serde_json::Value> {
        list.iter().map(|e| serde_json::json!({ "email": e })).collect()
    };
    let mut v = serde_json::json!({
        "from": [{ "name": msg.from_name, "email": msg.from }],
        "messageId": [new_message_id_for(&msg.from)],
        "to": addr_list(&msg.to),
        "subject": msg.subject,
        "keywords": { "$seen": true },
        "bodyValues": { "k2body": { "value": msg.text_body } },
        "textBody": [{ "partId": "k2body", "type": "text/plain" }],
    });
    if !msg.cc.is_empty() {
        v["cc"] = serde_json::Value::Array(addr_list(&msg.cc));
    }
    if !msg.reply_to.is_empty() {
        v["replyTo"] = serde_json::Value::Array(addr_list(&msg.reply_to));
    }
    if let Some(irt) = msg.in_reply_to.as_deref() {
        v["header:In-Reply-To:asText"] = serde_json::json!(irt);
    }
    if let Some(refs) = msg.references.as_deref() {
        v["header:References:asText"] = serde_json::json!(refs);
    }
    v
}

/// Invisible formatting characters refused in a From display name: they
/// can reorder or hide what a mail client shows (bidi overrides, zero-
/// width space, BOM, line/paragraph separators). ZWJ/ZWNJ (U+200C/D) stay
/// allowed — emoji sequences and several scripts need them.
pub(crate) fn is_spoofy_format_char(c: char) -> bool {
    matches!(
        c,
        '\u{200B}'
            | '\u{200E}'
            | '\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{2069}'
            | '\u{FEFF}'
    )
}

/// Validate an agent-chosen From display name (`fromName`, `k2 mail send
/// --from-name`). COSMETIC ONLY: the From ADDRESS stays the server-stamped
/// one the workspace may send from — this never changes who the mail is
/// from. Returns the trimmed name. Refuses (usage, nothing sent):
/// - empty / whitespace only;
/// - over [`MAX_FROM_NAME_CHARS`] characters;
/// - CR, LF, TAB or any other control character (no header injection), and
///   invisible bidi/zero-width formatting characters (no display spoofing);
/// - `@` (and its fullwidth/small forms): a name that looks like an
///   address ("ceo@bank.com" <bot@acme.dev>) would hide the real sender in
///   clients that show only the name.
///
/// Quoting is NOT done here — the name rides the JMAP `from[].name` field
/// as a structured value and the mail server's composer quotes it
/// (`"…"` with `\"`/`\\` escapes) or RFC 2047-encodes it (non-ASCII), so
/// `<`, `>`, `"` and `,` stay inside the display name.
pub fn validate_from_name(raw: &str) -> Result<String, SendError> {
    // Checked on the RAW value: a trailing CR/LF is refused, not trimmed.
    if let Some(c) = raw.chars().find(|c| c.is_control() || is_spoofy_format_char(*c)) {
        return Err(SendError::Usage(format!(
            "fromName holds a control or invisible formatting character (U+{:04X}) — line \
             breaks, tabs and other control characters can't go in a display name",
            c as u32
        )));
    }
    let name = raw.trim();
    if name.is_empty() {
        return Err(SendError::Usage(
            "fromName is empty — pass a display name, or omit --from-name to use the \
             workspace's agent name"
                .to_string(),
        ));
    }
    let n = name.chars().count();
    if n > MAX_FROM_NAME_CHARS {
        return Err(SendError::Usage(format!(
            "fromName is too long ({n} characters) — max {MAX_FROM_NAME_CHARS}"
        )));
    }
    if name.chars().any(|c| matches!(c, '@' | '\u{FF20}' | '\u{FE6B}')) {
        return Err(SendError::Usage(
            "fromName can't contain '@' — a display name that looks like an email address \
             would hide the real sender. The name is cosmetic only; pick the sending address \
             with --from"
                .to_string(),
        ));
    }
    Ok(name.to_string())
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

/// Validate `--reply-to`: at most [`MAX_REPLY_TO`], each a full address by the
/// same strict rule as a recipient ([`normalize_recipients`]). This check is
/// what keeps a line break out of the header: the mail server writes
/// `replyTo[].email` as it gets it (tests/mail/stalwart-attachments-probe.py
/// A8), so nothing unchecked may reach it.
pub fn normalize_reply_to(raw: &[String]) -> Result<Vec<String>, SendError> {
    if raw.len() > MAX_REPLY_TO {
        return Err(SendError::Usage(format!(
            "too many --reply-to addresses ({}) — max {MAX_REPLY_TO}",
            raw.len()
        )));
    }
    normalize_recipients(raw).map_err(|e| match e {
        SendError::Usage(h) => SendError::Usage(h.replacen("invalid recipient", "invalid --reply-to", 1)),
        other => other,
    })
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
    /// Attachment filenames (basenames only — never paths, never bytes)
    /// that rode this send. Stored as a compact JSON array of strings in
    /// `attachments_ref`; empty leaves the column NULL.
    pub attachment_names: &'a [String],
    /// The same files with size and hash, when known: `attachments_ref` then
    /// holds these instead of bare names.
    pub attachment_files: &'a [FileAudit],
    /// Unix seconds when the message may leave. None = immediate.
    /// Rate limits apply at enqueue time.
    pub send_after: Option<i64>,
    pub now: i64,
}

/// What the outbox keeps of a file that rode a message: enough to show later
/// which bytes left, and never the bytes.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FileAudit {
    pub name: String,
    pub size: u64,
    pub sha256: String,
}

impl From<&StoredAttachment> for FileAudit {
    fn from(a: &StoredAttachment) -> Self {
        FileAudit { name: a.filename.clone(), size: a.size(), sha256: a.sha256.clone() }
    }
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
    /// The composed message back from the body file: its files are named,
    /// not read.
    fn load_message(&self, id: &str) -> Result<OutboundMessage, String>;
    /// [`Self::load_message`] with every file's bytes read back and checked
    /// against the hash queued with them: what a send uses.
    fn load_message_full(&self, id: &str) -> Result<OutboundMessage, String> {
        self.load_message(id)
    }
    /// May the box hold this much more for this workspace, until a person or a
    /// send time gets to it? `Err` says why not.
    fn check_can_hold(&self, _project_id: &str, _file_bytes: u64, _text_bytes: u64) -> Result<(), String> {
        Ok(())
    }
    /// Append one line to the row's note (submit-failure details).
    fn append_note(&self, id: &str, extra: &str, now: i64) -> Result<(), String>;
    /// Rewrite the stored body of a one-time-link mail with its links
    /// hidden ([`at_rest`]) once it has been decided (sent, failed or
    /// denied). No-op for a store that keeps no body.
    fn scrub_body(&self, _id: &str) -> Result<(), String> {
        Ok(())
    }
    /// Store a message's blind copies after the owner's always-BCC policy
    /// changed while it waited: its body and its audit row. No-op for a store
    /// that keeps no body.
    fn update_blind_copies(&self, _id: &str, _msg: &OutboundMessage) -> Result<(), String> {
        Ok(())
    }
}

const OUTBOUND_COLS: &str = "id, owner_project_id, agent_name, from_address, to_json, \
                             cc_json, subject, body_ref, attachments_ref, status, \
                             decided_by, note, created_at, updated_at, decided_at, sent_at, \
                             send_after, bcc_json, policy_bcc_json, reply_to_json, body_sha256, \
                             forward_of_json, source_check";

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
        send_after: r.get(16)?,
        bcc_json: r.get(17)?,
        policy_bcc_json: r.get(18)?,
        reply_to_json: r.get(19)?,
        body_sha256: r.get(20)?,
        forward_of_json: r.get(21)?,
        source_check: r.get(22)?,
    })
}

/// The production store: `mail_outbound` rows in the shared DB +
/// composed-message JSON files under `~/.k2/mail-outbound/` (0600,
/// the mail-secrets file-permission convention — bodies never live in
/// the DB and never in logs).
pub struct DbOutboundStore {
    body_dir: PathBuf,
    max_held_bytes: u64,
    max_held_per_workspace: u64,
    min_free_after_hold: u64,
}

impl Default for DbOutboundStore {
    fn default() -> Self {
        // House rule: tests never touch the real `~/.k2` — the route
        // tests drive the REAL handlers (which construct this default),
        // so the test build points the body dir at a per-process temp
        // location instead. Production stays `~/.k2/mail-outbound`.
        if cfg!(test) {
            return Self::in_dir(
                std::env::temp_dir().join(format!("k2-mail-outbound-testdefault-{}", std::process::id())),
            );
        }
        let dir = k2_core::paths::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".k2")
            .join("mail-outbound");
        Self::in_dir(dir)
    }
}

impl DbOutboundStore {
    fn in_dir(body_dir: PathBuf) -> Self {
        Self {
            body_dir,
            max_held_bytes: MAX_HELD_BYTES,
            max_held_per_workspace: MAX_HELD_BYTES_PER_WORKSPACE,
            min_free_after_hold: MIN_FREE_AFTER_HOLD,
        }
    }

    #[cfg(test)]
    pub fn at(body_dir: PathBuf) -> Self {
        Self::in_dir(body_dir)
    }

    fn body_path(&self, id: &str) -> PathBuf {
        self.body_dir.join(format!("{id}.json"))
    }

    fn files_dir(&self, id: &str) -> PathBuf {
        self.body_dir.join(format!("{id}.files"))
    }

    /// Every byte held in `<id>.files/` folders now.
    fn held_bytes(&self) -> u64 {
        let Ok(rd) = std::fs::read_dir(&self.body_dir) else { return 0 };
        rd.flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".files"))
            .flat_map(|e| std::fs::read_dir(e.path()).into_iter().flatten().flatten())
            .filter_map(|f| f.metadata().ok())
            .map(|m| m.len())
            .sum()
    }

    fn dir_bytes(dir: &Path) -> u64 {
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|f| f.metadata().ok())
            .map(|m| m.len())
            .sum()
    }

    /// The bytes held for this workspace's waiting messages.
    fn held_bytes_for(&self, project_id: &str) -> u64 {
        let ids: Vec<String> = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let Ok(mut stmt) = conn.prepare(
                "SELECT id FROM mail_outbound WHERE owner_project_id = ?1 AND status IN ('pending', 'scheduled')",
            ) else {
                return 0;
            };
            stmt.query_map(rusqlite::params![project_id], |r| r.get(0))
                .map(|r| r.filter_map(Result::ok).collect())
                .unwrap_or_default()
        };
        ids.iter().map(|id| Self::dir_bytes(&self.files_dir(id))).sum()
    }

    /// Write the attachments that are in memory, each to its own 0600 file.
    fn write_files(&self, id: &str, msg: &OutboundMessage) -> Result<(), String> {
        if !msg.attachments.iter().any(|a| a.bytes.is_some()) {
            return Ok(());
        }
        let dir = self.files_dir(id);
        std::fs::create_dir_all(&dir).map_err(|e| format!("outbound files dir: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        }
        for (n, a) in msg.attachments.iter().enumerate() {
            if let Some(b) = &a.bytes {
                write_private_new(&dir.join(n.to_string()), b)
                    .map_err(|e| format!("outbound file write: {e}"))?;
            }
        }
        Ok(())
    }

    /// One held file's bytes, refused if they aren't the bytes queued.
    fn read_held(&self, id: &str, n: usize, a: &StoredAttachment) -> Result<Vec<u8>, String> {
        if a.legacy_b64.is_some() {
            return a.bytes_now();
        }
        let bytes = read_regular(&self.files_dir(id).join(n.to_string()), a.size)
            .map_err(|e| format!("held file '{}' unreadable: {e}", a.filename))?;
        if sha256_hex(&bytes) != a.sha256 {
            return Err(format!("held file '{}' changed since it was queued", a.filename));
        }
        Ok(bytes)
    }

    /// One held file, for the approver.
    pub fn load_file(&self, id: &str, n: usize) -> Result<Option<(StoredAttachment, Vec<u8>)>, String> {
        let msg = self.load_message(id)?;
        let Some(a) = msg.attachments.get(n).cloned() else { return Ok(None) };
        let bytes = self.read_held(id, n, &a)?;
        Ok(Some((a, bytes)))
    }

    /// Rewrite a body, keeping the row's hash in step. Its id already exists.
    fn rewrite_body(&self, id: &str, msg: &OutboundMessage) -> Result<(), String> {
        let path = self.body_path(id);
        let body = serde_json::to_string(msg).map_err(|e| format!("outbound message serialize: {e}"))?;
        std::fs::write(&path, body.as_bytes()).map_err(|e| format!("outbound body write: {e}"))?;
        restrict_mode(&path);
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "UPDATE mail_outbound SET body_sha256 = ?2 WHERE id = ?1",
            rusqlite::params![id, sha256_hex(body.as_bytes())],
        )
        .map(|_| ())
        .map_err(|e| format!("outbound body hash: {e}"))
    }
}

/// A stored file's bytes, opened so it can't hang or be redirected: never a FIFO,
/// a device or a link, and never more than `limit`.
fn read_regular(file: &Path, limit: u64) -> std::io::Result<Vec<u8>> {
    use std::io::Read as _;
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    let f = opts.open(file)?;
    if !f.metadata()?.is_file() {
        return Err(std::io::Error::other("not a regular file"));
    }
    let mut bytes = Vec::new();
    f.take(limit.saturating_add(1)).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(std::io::Error::other("larger than it should be"));
    }
    Ok(bytes)
}

/// Create a file readable by this user only from its first byte: it can hold
/// a person's mail or a file attached to it. A failed write leaves nothing.
fn write_private_new(file: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(file)?;
    let wrote = f.write_all(bytes).and_then(|()| f.flush());
    if wrote.is_err() {
        drop(f);
        let _ = std::fs::remove_file(file);
    }
    wrote
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
        let cleanup = |store: &Self| {
            let _ = std::fs::remove_file(&path);
            let _ = std::fs::remove_dir_all(store.files_dir(&id));
        };
        if let Err(e) = self.write_files(&id, new.message) {
            cleanup(self);
            return Err(e);
        }
        if let Err(e) = write_private_new(&path, body.as_bytes()) {
            cleanup(self);
            return Err(format!("outbound body write: {e}"));
        }
        let body_sha256 = sha256_hex(body.as_bytes());
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
        // BCC audit (0131): agent-added and policy-added kept apart so
        // the outbox shows who put each blind copy there. Empty → NULL.
        let json_or_null = |list: &[String], what: &str| -> Result<Option<String>, String> {
            if list.is_empty() {
                Ok(None)
            } else {
                serde_json::to_string(list)
                    .map(Some)
                    .map_err(|e| format!("{what} serialize: {e}"))
            }
        };
        let bcc_json = json_or_null(&new.message.bcc, "bcc")?;
        let policy_bcc_json = json_or_null(&new.message.policy_bcc, "policy bcc")?;
        let reply_to_json = json_or_null(&new.message.reply_to, "reply-to")?;
        let forward_of_json = new
            .message
            .forward_of
            .as_ref()
            .map(|f| serde_json::to_string(f).map_err(|e| format!("forward serialize: {e}")))
            .transpose()?;
        // Filenames only — basenames, never paths, never bytes (they're
        // non-secret display names). Empty leaves the column NULL.
        let attachments_ref = if !new.attachment_files.is_empty() {
            Some(
                serde_json::to_string(new.attachment_files)
                    .map_err(|e| format!("attachments serialize: {e}"))?,
            )
        } else if new.attachment_names.is_empty() {
            None
        } else {
            Some(
                serde_json::to_string(new.attachment_names)
                    .map_err(|e| format!("attachments serialize: {e}"))?,
            )
        };
        let decided_at: Option<i64> = new.decided_by.map(|_| new.now);
        let inserted = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO mail_outbound (id, owner_project_id, agent_name, from_address, \
                 to_json, cc_json, subject, body_ref, attachments_ref, status, decided_by, \
                 decided_at, created_at, updated_at, send_after, bcc_json, policy_bcc_json, \
                 reply_to_json, body_sha256, forward_of_json, source_check) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13, ?14, \
                 ?15, ?16, ?17, ?18, ?19, ?20)",
                rusqlite::params![
                    id,
                    new.owner_project_id,
                    new.agent_name,
                    new.message.from,
                    to_json,
                    cc_json,
                    new.message.subject,
                    path.to_string_lossy(),
                    attachments_ref,
                    new.status,
                    new.decided_by,
                    decided_at,
                    new.now,
                    new.send_after,
                    bcc_json,
                    policy_bcc_json,
                    reply_to_json,
                    body_sha256,
                    forward_of_json,
                    new.message.source_check,
                ],
            )
        };
        if let Err(e) = inserted {
            // No row → no send; don't leave an orphan body file either.
            cleanup(self);
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
             decided_at = CASE WHEN ?2 IN ('approved','denied','scheduled') \
                               AND ?3 IS NOT NULL THEN ?5 ELSE decided_at END, \
             sent_at = CASE WHEN ?2 = 'sent' THEN ?5 ELSE sent_at END, \
             updated_at = ?5 \
             WHERE id = ?1 AND status = ?6",
            rusqlite::params![id, to_status, decided_by, note, now, from_status],
        )
        .map(|n| n == 1)
        .map_err(|e| format!("outbound transition: {e}"))
    }

    fn load_message(&self, id: &str) -> Result<OutboundMessage, String> {
        let (body_ref, want_hash): (Option<String>, Option<String>) = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.query_row(
                "SELECT body_ref, body_sha256 FROM mail_outbound WHERE id = ?1",
                rusqlite::params![id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(|e| format!("outbound body_ref: {e}"))?
        };
        let Some(body_ref) = body_ref.filter(|s| !s.trim().is_empty()) else {
            return Err(format!("outbound '{id}' has no stored message body"));
        };
        let raw = read_regular(Path::new(&body_ref), MAX_BODY_FILE_BYTES)
            .map_err(|e| format!("outbound body read: {e}"))?;
        let raw = String::from_utf8(raw).map_err(|e| format!("outbound body read: {e}"))?;
        // What the row was told at queue time. A row from an older K2 has none.
        if want_hash.is_some_and(|h| h != sha256_hex(raw.as_bytes())) {
            return Err(format!(
                "the stored message for '{id}' changed after it was queued, so it can't be shown or sent. \
                 Tell your human"
            ));
        }
        serde_json::from_str(&raw).map_err(|e| format!("outbound body parse: {e}"))
    }

    fn load_message_full(&self, id: &str) -> Result<OutboundMessage, String> {
        let mut msg = self.load_message(id)?;
        for n in 0..msg.attachments.len() {
            if msg.attachments[n].legacy_b64.is_none() {
                let bytes = self.read_held(id, n, &msg.attachments[n])?;
                msg.attachments[n].bytes = Some(std::sync::Arc::new(bytes));
            }
        }
        Ok(msg)
    }

    fn check_can_hold(&self, project_id: &str, file_bytes: u64, text_bytes: u64) -> Result<(), String> {
        if file_bytes > 0 {
            if self.held_bytes().saturating_add(file_bytes) > self.max_held_bytes {
                return Err(
                    "this server is already holding as many files as it can for messages waiting \
                     for approval or a send time. Nothing was queued. Send fewer or smaller files, \
                     or ask your human to clear the queue (k2 hostmail approvals)"
                        .to_string(),
                );
            }
            if self.held_bytes_for(project_id).saturating_add(file_bytes) > self.max_held_per_workspace {
                return Err(
                    "this workspace already has as many files waiting for approval or a send time \
                     as it may hold. Nothing was queued. Send fewer or smaller files, or ask your \
                     human to decide what's waiting (k2 hostmail approvals)"
                        .to_string(),
                );
            }
        }
        if file_bytes == 0 && text_bytes <= TEXT_FLOOR_FROM_BYTES {
            return Ok(());
        }
        let _ = std::fs::create_dir_all(&self.body_dir);
        let needed = file_bytes.saturating_add(text_bytes).saturating_add(self.min_free_after_hold);
        match k2_core::disk_guard::probe(&self.body_dir).map(|fs| fs.free_bytes) {
            Some(free) if free >= needed => Ok(()),
            Some(_) => Err(
                "this server's disk is too full to hold this message until it's sent. Nothing was \
                 queued. Tell your human, or retry with a smaller message"
                    .to_string(),
            ),
            None => Err(
                "can't read this server's free disk space, so nothing was queued. Try again shortly; \
                 if it repeats, tell your human"
                    .to_string(),
            ),
        }
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

    fn scrub_body(&self, id: &str) -> Result<(), String> {
        // The bytes go first, whatever the body says: a body that can't be
        // read must not keep its files on disk.
        let _ = std::fs::remove_dir_all(self.files_dir(id));
        let msg = self.load_message(id)?;
        if !msg.ephemeral && msg.attachments.is_empty() {
            return Ok(());
        }
        let mut kept = at_rest(&msg).into_owned();
        // Email Notifications (security review M3): a decided notification
        // keeps no body at all (its text, html and headers), only what the
        // outbox lists.
        if let Some(n) = kept.notify.as_mut() {
            kept.text_body = String::new();
            n.html = None;
            n.headers.clear();
        }
        self.rewrite_body(id, &kept)
    }

    fn update_blind_copies(&self, id: &str, msg: &OutboundMessage) -> Result<(), String> {
        self.rewrite_body(id, msg)?;
        let json_or_null = |list: &[String]| -> Result<Option<String>, String> {
            if list.is_empty() {
                Ok(None)
            } else {
                serde_json::to_string(list).map(Some).map_err(|e| format!("bcc serialize: {e}"))
            }
        };
        let (bcc, policy) = (json_or_null(&msg.bcc)?, json_or_null(&msg.policy_bcc)?);
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "UPDATE mail_outbound SET bcc_json = ?2, policy_bcc_json = ?3 WHERE id = ?1",
            rusqlite::params![id, bcc, policy],
        )
        .map(|_| ())
        .map_err(|e| format!("outbound bcc update: {e}"))
    }
}

/// Record a SUCCESSFUL linked-SMTP submission in the outbox (issue
/// #31.5). Linked send is gated by [`allow_linked_send`] (`on` only)
/// before SMTP; once the provider accepts, the row is born via the
/// same `approved → sent` lifecycle a hosted send ends on, leaving it
/// `sent` (wire: `submitted` — accepted-for-delivery, NEVER
/// "delivered", pre-mortem #9) with `sent_at`/`updated_at` stamped
/// exactly like a hosted row. `decided_by` is `None` (mode was `on` —
/// policy auto-approved at the gate, no human decision). Returns the
/// new `out_…` id. The caller treats a failure as non-fatal: the mail
/// already left, this is only the audit trail.
pub fn record_linked_submitted(
    store: &dyn OutboundStore,
    owner_project_id: &str,
    agent_name: &str,
    message: &OutboundMessage,
    attachment_names: &[String],
    now: i64,
) -> Result<String, String> {
    let id = store.insert(&NewOutbound {
        owner_project_id,
        agent_name,
        message,
        status: "approved",
        decided_by: None,
        attachment_names,
        attachment_files: &[],
        send_after: None,
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

/// §12 lazy expiry: auto-deny rows that have been pending for 7 days
/// (called on every outbox/approvals read + before approve/deny — no
/// separate background loop). The clock is the row's last change, so a
/// scheduled message that goes back to approval at its send time gets its
/// full 7 days. Emits `MailSendDecided` per expired row. Returns how many
/// expired.
pub fn auto_deny_expired(now: i64) -> usize {
    let cutoff = now - APPROVAL_EXPIRE_SECS;
    let ids: Vec<String> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let Ok(mut stmt) = conn.prepare(
            "SELECT id FROM mail_outbound WHERE status = 'pending' AND updated_at < ?1",
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
            // An expired one-time-link mail or notification keeps no live
            // link or body either (Email Notifications review M3).
            scrub(&store, &id);
            k2_core::agent_hooks::emit(
                k2_core::agent_hooks::HookEvent::MailSendDecided,
                serde_json::json!({ "outboundId": id, "status": "rejected", "expired": true }),
            );
        }
    }
    n
}

/// A mailbox is going away: its waiting messages can't leave, so they end now
/// and their held files go with them. Returns how many.
pub fn purge_held_for_address(address: &str, now: i64) -> usize {
    let rows: Vec<(String, String)> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let Ok(mut stmt) = conn.prepare(
            "SELECT id, status FROM mail_outbound \
             WHERE from_address = ?1 AND status IN ('pending', 'scheduled')",
        ) else {
            return 0;
        };
        stmt.query_map(rusqlite::params![address], |r| Ok((r.get(0)?, r.get(1)?)))
            .map(|r| r.filter_map(Result::ok).collect())
            .unwrap_or_default()
    };
    let store = DbOutboundStore::default();
    let mut n = 0;
    for (id, status) in rows {
        let flipped = store
            .transition(
                &id,
                &status,
                "denied",
                Some("system:mailbox-removed"),
                Some("not sent: its mailbox was removed"),
                now,
            )
            .unwrap_or(false);
        if flipped {
            n += 1;
            scrub(&store, &id);
            k2_core::agent_hooks::emit(
                k2_core::agent_hooks::HookEvent::MailSendDecided,
                serde_json::json!({ "outboundId": id, "status": "rejected" }),
            );
        }
    }
    n
}

/// A boot's tidy of the outbound store: held files with no waiting message, and
/// bodies with no row or decided long ago. A crash can leave the first kind and
/// nothing else removes the last. Rows stay: they're the audit trail. Returns how
/// many things went.
pub fn sweep_store(store: &DbOutboundStore, now: i64) -> usize {
    let Ok(rd) = std::fs::read_dir(&store.body_dir) else { return 0 };
    let status_of = |id: &str| -> Option<(String, i64)> {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT status, updated_at FROM mail_outbound WHERE id = ?1",
            rusqlite::params![id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok()
    };
    let old_enough = |e: &std::fs::DirEntry| {
        e.metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .is_some_and(|d| d.as_secs() as i64 + ORPHAN_AFTER_SECS < now)
    };
    let mut n = 0;
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(id) = name.strip_suffix(".files") {
            let gone = match status_of(id) {
                None => old_enough(&entry),
                Some((status, _)) => !matches!(status.as_str(), "pending" | "scheduled" | "approved"),
            };
            if gone && std::fs::remove_dir_all(entry.path()).is_ok() {
                n += 1;
            }
        } else if let Some(id) = name.strip_suffix(".json") {
            match status_of(id) {
                None if old_enough(&entry) => {
                    if std::fs::remove_file(entry.path()).is_ok() {
                        n += 1;
                    }
                }
                Some((status, updated_at))
                    if matches!(status.as_str(), "sent" | "denied" | "failed")
                        && updated_at + DECIDED_BODY_KEEP_SECS < now =>
                {
                    if std::fs::remove_file(entry.path()).is_ok() {
                        let db = k2_core::db::shared();
                        let conn = db.lock();
                        let _ = conn.execute(
                            "UPDATE mail_outbound SET body_ref = NULL, body_sha256 = NULL WHERE id = ?1",
                            rusqlite::params![id],
                        );
                        n += 1;
                    }
                }
                _ => {}
            }
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
        "scheduled" => "scheduled",
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
        "scheduled" => "scheduled — waiting for send_after before leaving the box",
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
    let mut v = serde_json::json!({
        "id": row.id,
        "from": row.from_address,
        "to": list(Some(row.to_json.as_str())),
        "cc": list(row.cc_json.as_deref()),
        // Blind copies (envelope only — never in the sent headers):
        // `bcc` = the agent's own `--bcc`; `policyBcc` = what the
        // owner's "always BCC" policy added. Kept apart for the audit.
        "bcc": list(row.bcc_json.as_deref()),
        "policyBcc": list(row.policy_bcc_json.as_deref()),
        // Where answers go instead of From (`--reply-to`). No mail goes there.
        "replyTo": list(row.reply_to_json.as_deref()),
        "subject": row.subject,
        "status": wire_status(&row.status),
        "statusNote": status_note(&row.status),
        "note": row.note,
        "agentName": row.agent_name,
        "decidedBy": row.decided_by,
        "createdAt": row.created_at,
        "decidedAt": row.decided_at,
        "sentAt": row.sent_at,
        "sendAfter": row.send_after,
    });
    // Attachment trail (issue #31.5 follow-up): the outbox row records the
    // filenames that rode a send so an agent can verify its own attachment
    // send without opening the recipient inbox. Filenames only — the row
    // never stores bytes or paths. Omitted entirely when there were none.
    // A row from before 0.46.8 holds bare names; a newer one size and hash too.
    let refs: Vec<serde_json::Value> =
        row.attachments_ref.as_deref().and_then(|s| serde_json::from_str(s).ok()).unwrap_or_default();
    let names: Vec<&str> = refs
        .iter()
        .filter_map(|r| r.as_str().or_else(|| r["name"].as_str()))
        .collect();
    if !names.is_empty() {
        v["attachments"] = serde_json::json!({ "count": names.len(), "names": names });
        if refs.iter().all(serde_json::Value::is_object) {
            v["attachments"]["files"] = serde_json::Value::Array(refs.clone());
        }
    }
    if let Some(why) = row.source_check.as_deref() {
        v["sourceCheck"] = serde_json::json!(why);
    }
    // What a forward forwarded (`k2 mail forward-message`).
    if let Some(f) = row.forward_of_json.as_deref().and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok()) {
        v["forwardOf"] = f;
    }
    v
}

// ── The submit seam (local Stalwart behind a trait) ─────────────────────

/// What S5 needs from the engine at submit time. Production impl is
/// the loopback JMAP client; tests use recording fakes (NO real sends,
/// ever — pre-mortem #1).
pub trait SubmitBackend {
    fn submit(&self, account_id: &str, msg: &OutboundMessage) -> Result<(), String>;
}

/// Every address in a message about to be handed to the mail server is still
/// exactly a plain address. The routes checked them when it was composed; this
/// is the last check before Stalwart, which writes `replyTo` as it gets it, and
/// it holds whatever happened to a stored body in between.
pub fn check_addresses_at_submit(msg: &OutboundMessage) -> Result<(), String> {
    for a in std::iter::once(&msg.from)
        .chain(&msg.to)
        .chain(&msg.cc)
        .chain(&msg.bcc)
        .chain(&msg.policy_bcc)
        .chain(&msg.reply_to)
    {
        if addresses::normalize_address(a).ok().as_deref() != Some(a.as_str()) {
            return Err(
                "an address in this message isn't a plain local@domain address, so nothing was sent. \
                 Tell your human"
                    .to_string(),
            );
        }
    }
    Ok(())
}

impl SubmitBackend for StalwartClient {
    fn submit(&self, account_id: &str, msg: &OutboundMessage) -> Result<(), String> {
        check_addresses_at_submit(msg)?;
        // The files go up first, then one request composes and submits. A
        // failed upload sends nothing; an upload the send never uses is
        // Stalwart's to expire.
        let mut uploaded = Vec::with_capacity(msg.attachments.len());
        for file in &msg.attachments {
            let bytes = file.bytes_now()?;
            let blob_id = self
                .blob_upload(account_id, &bytes)
                .map_err(|e| format!("attachment '{}' didn't upload: {e}", file.filename))?;
            uploaded.push(UploadedAttachment { blob_id, file });
        }
        self.submission_send_opts(
            account_id,
            &msg.from,
            &msg.all_recipients(),
            submission_email_json_with(msg, &uploaded),
            msg.ephemeral,
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
    /// Optional future unix seconds; None = send as soon as policy allows.
    pub send_after: Option<i64>,
}

/// What the gate decided + did.
#[derive(Debug)]
pub enum SendOutcome {
    /// Approval mode: row is pending; NOTHING left the box.
    Queued { id: String, send_after: Option<i64> },
    /// On mode + future send_after: row is `scheduled`; NOTHING left yet.
    Scheduled { id: String, send_after: i64 },
    /// On mode: audited, handed to Stalwart, accepted-for-delivery.
    Submitted { id: String },
    /// On mode: audited, submission failed — row is `failed`.
    SubmitFailed { id: String, error: String },
}

/// Before holding a message on this box: its files not too far ahead, and only
/// if there is room. Nothing is recorded on a refusal.
fn check_can_hold_message(
    store: &dyn OutboundStore,
    project_id: &str,
    msg: &OutboundMessage,
    send_after: Option<i64>,
    now: i64,
) -> Result<(), SendError> {
    if !msg.attachments.is_empty() && send_after.is_some_and(|t| t - now > MAX_HELD_SCHEDULE_SECS) {
        return Err(SendError::Usage(
            "a message with files can be scheduled at most 7 days ahead".to_string(),
        ));
    }
    let file_bytes = msg.attachments.iter().map(StoredAttachment::size).sum();
    let text_bytes = (msg.text_body.len() + msg.subject.len()) as u64;
    store.check_can_hold(project_id, file_bytes, text_bytes).map_err(SendError::Guardrail)
}

/// Check-and-write for held messages takes turns, so parallel sends can't all
/// pass a limit that only has room for one.
fn hold_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(())).lock().unwrap_or_else(|p| p.into_inner())
}

/// Rate limits → audit row → (queue | schedule | submit).
/// Rate limits apply at *enqueue*; spacing is agent-driven via send_after.
pub fn gate_and_dispatch(
    store: &dyn OutboundStore,
    backend: Option<&dyn SubmitBackend>,
    gate: Gate,
    req: &OutboundRequest,
    now: i64,
) -> Result<SendOutcome, SendError> {
    check_rate_limits(store, &req.message.from, now)?;
    let future_after = req.send_after.filter(|t| *t > now);
    // The outbox keeps the filenames. The bytes stay on disk only while
    // something still has to send them (approval, a schedule).
    let names: Vec<String> = req.message.attachments.iter().map(|a| a.filename.clone()).collect();
    let files: Vec<FileAudit> = req.message.attachments.iter().map(FileAudit::from).collect();
    let holds = gate == Gate::Approval || (gate == Gate::On && future_after.is_some());
    let _hold = holds.then(hold_lock);
    if holds {
        check_can_hold_message(store, req.project_id, req.message, future_after, now)?;
    }
    match gate {
        Gate::Off => Err(SendError::Gated(agent_send_off_hint())),
        Gate::Approval => {
            let id = store
                .insert(&NewOutbound {
                    owner_project_id: req.project_id,
                    agent_name: req.agent_name,
                    message: req.message,
                    status: "pending",
                    decided_by: None,
                    attachment_names: &names,
                    attachment_files: &files,
                    send_after: future_after.or(req.send_after),
                    now,
                })
                .map_err(SendError::Engine)?;
            Ok(SendOutcome::Queued {
                id,
                send_after: future_after.or(req.send_after),
            })
        }
        Gate::On => {
            if let Some(after) = future_after {
                let id = store
                    .insert(&NewOutbound {
                        owner_project_id: req.project_id,
                        agent_name: req.agent_name,
                        message: req.message,
                        status: "scheduled",
                        decided_by: Some("policy:agent-send-on"),
                        attachment_names: &names,
                        attachment_files: &files,
                        send_after: Some(after),
                        now,
                    })
                    .map_err(SendError::Engine)?;
                return Ok(SendOutcome::Scheduled {
                    id,
                    send_after: after,
                });
            }
            let backend = backend.ok_or_else(|| {
                SendError::Engine(
                    "no submit backend available — refusing to send (fail-closed)".to_string(),
                )
            })?;
            // Nothing reads this copy back to send it: no attachment bytes,
            // and a one-time-link mail's links hidden (review item 4).
            let kept = at_rest(req.message);
            let id = store
                .insert(&NewOutbound {
                    owner_project_id: req.project_id,
                    agent_name: req.agent_name,
                    message: &kept,
                    status: "approved",
                    decided_by: Some("policy:agent-send-on"),
                    attachment_names: &names,
                    attachment_files: &files,
                    send_after: None,
                    now,
                })
                .map_err(SendError::Engine)?;
            match backend.submit(req.account_id, req.message) {
                Ok(()) => {
                    if let Err(e) = store.transition(&id, "approved", "sent", None, None, now) {
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
    /// approved, but the hand-off failed → row is `failed`.
    FailedToSubmit(String),
    /// Human approved; send_after still future → now `scheduled`.
    Scheduled { send_after: i64 },
}

/// Approve one pending HOSTED outbound and submit it (or hold as
/// scheduled) — the pre-MCS2 entry point, kept for the hosted-only
/// callers; it resolves every message to `backend` + the From's account.
pub fn approve_and_submit(
    store: &dyn OutboundStore,
    backend: Result<&dyn SubmitBackend, String>,
    account_id_for_from: &mut dyn FnMut(&str) -> Result<String, String>,
    id: &str,
    decided_by: &str,
    note: Option<&str>,
    now: i64,
) -> Result<ApproveOutcome, SendError> {
    let mut resolve = |msg: &OutboundMessage| -> Result<Box<dyn crate::mail::submit::MailSubmit + '_>, String> {
        let b = match &backend {
            Ok(b) => *b,
            Err(e) => return Err(format!("mail server unavailable: {e}")),
        };
        let account = account_id_for_from(&msg.from)
            .map_err(|e| format!("sender address unavailable: {e}"))?;
        Ok(Box::new(crate::mail::submit::HostedSubmit { backend: b, account_id: account }))
    };
    approve_and_submit_with(store, &mut resolve, id, decided_by, note, now)
}

/// MCS2 (PRD §3.3, MCS-A5): approve one pending outbound and hand it to
/// the backend `resolve` picks for its From address — hosted JMAP, linked
/// SMTP, or Microsoft Graph — so the approval queue covers linked inboxes
/// too. Same atomic `pending → approved` CAS first; a resolve/submit
/// failure marks the row `failed` with the reason (never a body, never a
/// credential). A LINKED reply re-reads its source and re-locks the
/// recipient at send time.
pub fn approve_and_submit_with<'r>(
    store: &dyn OutboundStore,
    resolve: &mut dyn FnMut(&OutboundMessage) -> Result<Box<dyn crate::mail::submit::MailSubmit + 'r>, String>,
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
    if let Some(after) = row.send_after.filter(|t| *t > now) {
        let flipped = store
            .transition(id, "pending", "scheduled", Some(decided_by), note, now)
            .map_err(|e| {
                SendError::Engine(format!("approval transition failed — nothing sent: {e}"))
            })?;
        if !flipped {
            return Err(SendError::Conflict(format!(
                "outbound '{id}' is already {} — approve applies to pending messages only",
                wire_status(&row.status)
            )));
        }
        return Ok(ApproveOutcome::Scheduled { send_after: after });
    }
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
        scrub(store, id);
        Ok(ApproveOutcome::FailedToSubmit(reason.to_string()))
    };
    let msg = match store.load_message_full(id) {
        Ok(m) => m,
        Err(e) => return fail(store, &format!("stored message unreadable: {e}")),
    };
    if !row_matches_message(&row, &msg) {
        return fail(
            store,
            "not sent: the stored message changed after it was queued, so this approval no longer \
             applies. Tell your human before queuing it again",
        );
    }
    let submit = match resolve(&msg) {
        Ok(s) => s,
        Err(e) => return fail(store, &e),
    };
    let mut attachments = Vec::with_capacity(msg.attachments.len());
    for a in &msg.attachments {
        match a.to_out() {
            Ok(o) => attachments.push(o),
            Err(e) => return fail(store, &e),
        }
    }
    let sent = match msg.reply_source.as_deref() {
        Some(src) => submit.reply(src, &msg, &attachments),
        None => submit.submit(&msg, &attachments),
    };
    match sent {
        Ok(_) => {
            if let Err(e) = store.transition(id, "approved", "sent", None, None, now) {
                k2_core::log_debug!("[mail/send] sent-mark failed for {id}: {e}");
            }
            scrub(store, id);
            Ok(ApproveOutcome::Submitted)
        }
        Err(e) => fail(store, &e.to_string()),
    }
}

/// Does the stored message say what the approver was shown? The queue shows the
/// row; what leaves comes from the body. A body that differs from its row, however
/// it came to, doesn't leave.
pub fn row_matches_message(row: &MailOutbound, msg: &OutboundMessage) -> bool {
    let same = |raw: Option<&str>, list: &[String]| -> bool {
        let stored: Vec<String> = match raw {
            Some(s) => match serde_json::from_str(s) {
                Ok(v) => v,
                Err(_) => return false,
            },
            None => Vec::new(),
        };
        stored == list
    };
    row.from_address == msg.from
        && row.subject == msg.subject
        && same(Some(&row.to_json), &msg.to)
        && same(row.cc_json.as_deref(), &msg.cc)
        && same(row.bcc_json.as_deref(), &msg.bcc)
        && same(row.policy_bcc_json.as_deref(), &msg.policy_bcc)
        && same(row.reply_to_json.as_deref(), &msg.reply_to)
}

/// [`OutboundStore::scrub_body`], logged on failure (the send itself is done).
pub(crate) fn scrub(store: &dyn OutboundStore, id: &str) {
    if let Err(e) = store.scrub_body(id) {
        k2_core::log_debug!("[mail/send] scrub {id}: {e}");
    }
}

/// MCS2 (PRD §3.3, MCS-A5): the governed pipeline for a LINKED inbox —
/// the same rate limits and audit-row-first rule as hosted, then:
/// approval → a `pending` row (attachments captured into the body file);
/// on → an `approved` row, then the provider (`submit`), then `sent` or
/// `failed`. No row, no send. Linked scheduling stays hosted-only (the
/// route refuses `--at/--in` before this).
pub fn gate_and_dispatch_linked(
    store: &dyn OutboundStore,
    submit: &dyn crate::mail::submit::MailSubmit,
    gate: Gate,
    req: &OutboundRequest,
    attachments: &[crate::mail::external_smtp::OutAttachment],
    now: i64,
) -> Result<(SendOutcome, Option<crate::mail::submit::Receipt>), SendError> {
    check_rate_limits(store, &req.message.from, now)?;
    let names: Vec<String> = attachments.iter().map(|a| a.filename.clone()).collect();
    let files: Vec<FileAudit> = attachments
        .iter()
        .map(|a| FileAudit { name: a.filename.clone(), size: a.bytes.len() as u64, sha256: sha256_hex(&a.bytes) })
        .collect();
    match gate {
        Gate::Off => Err(SendError::Gated(agent_send_off_hint())),
        Gate::Approval => {
            let mut held = req.message.clone();
            held.attachments = attachments.iter().map(StoredAttachment::from_out).collect();
            let _hold = hold_lock();
            check_can_hold_message(store, req.project_id, &held, None, now)?;
            let id = store
                .insert(&NewOutbound {
                    owner_project_id: req.project_id,
                    agent_name: req.agent_name,
                    message: &held,
                    status: "pending",
                    decided_by: None,
                    attachment_names: &names,
                    attachment_files: &files,
                    send_after: None,
                    now,
                })
                .map_err(SendError::Engine)?;
            Ok((SendOutcome::Queued { id, send_after: None }, None))
        }
        Gate::On => {
            let id = store
                .insert(&NewOutbound {
                    owner_project_id: req.project_id,
                    agent_name: req.agent_name,
                    message: req.message,
                    status: "approved",
                    decided_by: Some("policy:agent-send-on"),
                    attachment_names: &names,
                    attachment_files: &files,
                    send_after: None,
                    now,
                })
                .map_err(SendError::Engine)?;
            let sent = match req.message.reply_source.as_deref() {
                Some(src) => submit.reply(src, req.message, attachments),
                None => submit.submit(req.message, attachments),
            };
            match sent {
                Ok(receipt) => {
                    if let Err(e) = store.transition(&id, "approved", "sent", None, None, now) {
                        k2_core::log_debug!("[mail/send] sent-mark failed for {id}: {e}");
                    }
                    Ok((SendOutcome::Submitted { id }, Some(receipt)))
                }
                Err(e) => {
                    let _ = store.transition(&id, "approved", "failed", None, None, now);
                    let _ = store.append_note(&id, &format!("submit failed: {e}"), now);
                    Ok((SendOutcome::SubmitFailed { id, error: e.to_string() }, None))
                }
            }
        }
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
    scrub(store, id);
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
    authority: &messages::AuthAuthority,
    sender_on_this_box: bool,
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
    if gate != Gate::Approval {
        if let messages::SenderCheck::NeedsPerson(why) =
            messages::sender_check(&ctx.auth_results, authority, &ctx.from, sender_on_this_box)
        {
            return Err(SendError::Guardrail(format!(
                "the original message {why}, so its sender may be forged — replying is blocked \
                 unless send gating is 'approval' (your human reviews it). Ask your human via \
                 'k2 tickets ask' with an HTML brief (k2 study ticket-brief)"
            )));
        }
    }
    Ok(recipient)
}

/// Is this one sender a mailbox on this box? Mail between them gets no stamp.
/// An address that's merely on a hosted domain doesn't count.
pub fn sender_on_this_box(from: &[crate::mail::jmap::MailAddr]) -> bool {
    let [sender] = from else { return false };
    let Ok(address) = addresses::normalize_address(&sender.email) else { return false };
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT 1 FROM mail_addresses WHERE address = ?1 AND status = 'active'",
        rusqlite::params![address],
        |_| Ok(()),
    )
    .is_ok()
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

    #[test]
    fn hide_links_masks_one_time_tokens_only() {
        assert_eq!(
            hide_links("Set your password: https://d.example/set#k2inv_ab12CD34 now. k2inv_x9"),
            "Set your password: https://d.example/set#k2inv_[hidden] now. k2inv_[hidden]"
        );
        assert_eq!(hide_links("no link here"), "no link here");
    }

    #[test]
    fn an_ephemeral_message_round_trips_and_a_plain_one_stays_lean() {
        let mut m = msg();
        m.ephemeral = true;
        let s = serde_json::to_string(&m).unwrap();
        assert!(s.contains("\"ephemeral\":true"), "{s}");
        assert!(serde_json::from_str::<OutboundMessage>(&s).unwrap().ephemeral);
        let plain = serde_json::to_string(&msg()).unwrap();
        assert!(!plain.contains("ephemeral"), "old body files stay byte-identical: {plain}");
        assert!(!serde_json::from_str::<OutboundMessage>(&plain).unwrap().ephemeral);
    }

    fn msg() -> OutboundMessage {
        OutboundMessage {
            from_name: Some("Research Bot".to_string()),
            from: "bot@acme.dev".to_string(),
            to: vec!["human@example.com".to_string()],
            cc: vec![],
            bcc: vec![],
            policy_bcc: vec![],
            reply_to: Vec::new(),
            subject: "Weekly digest".to_string(),
            text_body: "All green.".to_string(),
            in_reply_to: None,
            references: None,
            reply_source: None,
            attachments: Vec::new(),
            ephemeral: false,
            notify: None,
            forward_of: None,
            source_dmarc: None,
            source_check: None,
        }
    }

    /// Hosted send/reply: K2 hands Stalwart the body VERBATIM in
    /// `bodyValues`, and Stalwart's own composer (mail-builder 1.0, the
    /// version Stalwart 0.16.x pins) encodes it without a hard wrap —
    /// the text part decodes to the input line for line, wire lines ≤ 998.
    #[test]
    fn hosted_send_and_reply_keep_paragraphs_exactly() {
        let body = crate::mail::external::tests::fidelity_body();
        for reply in [false, true] {
            let mut m = msg();
            m.text_body = body.clone();
            if reply {
                m.subject = "Re: Weekly digest".to_string();
                m.in_reply_to = Some("<src@example.com>".to_string());
                m.references = Some("<src@example.com>".to_string());
            }
            let v = submission_email_json(&m);
            let value = v["bodyValues"]["k2body"]["value"].as_str().expect("body value");
            assert_eq!(value, body, "the hosted body reaches Stalwart verbatim");
            let wire = mail_builder::MessageBuilder::new()
                .from(("Research Bot", "bot@acme.dev"))
                .to("human@example.com")
                .subject(m.subject.as_str())
                .text_body(value)
                .write_to_vec()
                .expect("compose");
            crate::mail::external::tests::assert_body_fidelity(&wire, &body);
        }
    }

    /// Recording fake store: counts inserts, canned failure switches.
    #[derive(Default)]
    struct FakeStore {
        fail_insert: bool,
        fail_count: bool,
        counts: (u32, u32), // (hour, day)
        inserts: Mutex<Vec<(String, Option<String>, Option<i64>)>>, // (status, decided_by, send_after)
        insert_attachments: Mutex<Vec<Vec<String>>>,    // attachment_names per insert
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
            self.inserts.lock().unwrap().push((
                new.status.to_string(),
                new.decided_by.map(String::from),
                new.send_after,
            ));
            self.insert_attachments
                .lock()
                .unwrap()
                .push(new.attachment_names.to_vec());
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
            send_after: None,
        }
    }

    fn req_at(m: &OutboundMessage, send_after: i64) -> OutboundRequest<'_> {
        OutboundRequest {
            project_id: "P1",
            agent_name: "bot",
            account_id: "acc-1",
            message: m,
            send_after: Some(send_after),
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

    /// E4: linked/BYO send — off blocks, on allows, and since MCS2's
    /// linked approval hand-off approval queues too (approve resolves the
    /// linked provider).
    #[test]
    fn linked_send_gate_off_blocks_on_and_approval_allow() {
        assert!(allow_linked_send(Gate::On).is_ok());
        assert!(allow_linked_send(Gate::Approval).is_ok(), "MCS2: linked approval queues");

        let err = allow_linked_send(Gate::Off).expect_err("off must block");
        match err {
            SendError::Gated(h) => {
                assert!(h.contains("disabled"), "{h}");
                // Names controls that exist on every OS — never the
                // hosted-only Email page (Appa/Baden, macOS 0.45.4).
                assert!(h.contains("`k2 mail sending <workspace> on`"), "{h}");
                assert!(h.contains("Workspaces / Agents → <workspace> → Email sending"), "{h}");
                assert!(!h.contains("Settings → Email → Sending"), "{h}");
            }
            other => panic!("expected Gated, got {other:?}"),
        }
    }

    #[test]
    fn off_mode_never_writes_and_never_submits() {
        let store = FakeStore { transition_result: Some(true), ..Default::default() };
        let submit = FakeSubmit::default();
        let m = msg();
        let err = gate_and_dispatch(&store, Some(&submit), Gate::Off, &req(&m), 1_000_000)
            .expect_err("gated");
        assert!(
            matches!(&err, SendError::Gated(h) if h.contains("`k2 mail sending <workspace> on`")),
            "{err:?}"
        );
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
        assert_eq!(inserts.as_slice(), &[("pending".to_string(), None, None)]);
    }

    /// #31.5 follow-up: a linked send threads its attachment filenames
    /// (basenames only) into the audit row; a send with none records an
    /// empty list (→ NULL `attachments_ref`).
    #[test]
    fn record_linked_submitted_threads_attachment_filenames() {
        let store = FakeStore { transition_result: Some(true), ..Default::default() };
        let m = msg();
        let names = vec!["invoice.pdf".to_string(), "photo.jpg".to_string()];
        record_linked_submitted(&store, "P1", "bot", &m, &names, 1_000_000)
            .expect("recorded");
        assert_eq!(
            store.insert_attachments.lock().unwrap().as_slice(),
            &[names.clone()],
            "the two basenames rode the insert"
        );
        // No bytes / no paths — the recorded names are exactly the basenames.
        for n in store.insert_attachments.lock().unwrap().iter().flatten() {
            assert!(!n.contains('/'), "no path separators in a recorded name: {n}");
        }

        // Zero attachments → an empty list (the store turns that into a
        // NULL column, verified in the DB-backed route test).
        let store2 = FakeStore { transition_result: Some(true), ..Default::default() };
        record_linked_submitted(&store2, "P1", "bot", &m, &[], 1_000_000).expect("recorded");
        assert_eq!(
            store2.insert_attachments.lock().unwrap().as_slice(),
            &[Vec::<String>::new()],
            "no attachments → empty list"
        );
    }

    /// The outbox read surfaces a `{count, names}` summary when the row
    /// carries attachment filenames, and omits it entirely otherwise.
    #[test]
    fn outbound_json_surfaces_attachments_summary() {
        let base = MailOutbound {
            id: "out_x".into(),
            owner_project_id: "P1".into(),
            agent_name: "bot".into(),
            from_address: "bot@acme.dev".into(),
            to_json: r#"["human@example.com"]"#.into(),
            cc_json: None,
            subject: "Files".into(),
            body_ref: Some("/tmp/out_x.json".into()),
            attachments_ref: Some(r#"["invoice.pdf","photo.jpg"]"#.into()),
            status: "sent".into(),
            decided_by: None,
            note: None,
            created_at: 1,
            updated_at: 1,
            decided_at: None,
            sent_at: Some(1),
            send_after: None,
            bcc_json: None,
            policy_bcc_json: None,
            reply_to_json: None,
            body_sha256: None,
            forward_of_json: None,
            source_check: None,
        };
        let v = outbound_json(&base);
        assert_eq!(v["attachments"]["count"], 2);
        assert_eq!(v["attachments"]["names"][0], "invoice.pdf");
        assert_eq!(v["attachments"]["names"][1], "photo.jpg");

        // No attachments → no key at all.
        let plain = MailOutbound { attachments_ref: None, ..base };
        let v = outbound_json(&plain);
        assert!(v.get("attachments").is_none(), "omitted when none: {v}");
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
            &[("approved".to_string(), Some("policy:agent-send-on".to_string()), None)]
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
            send_after: None,
            bcc_json: None,
            policy_bcc_json: None,
            reply_to_json: None,
            body_sha256: None,
            forward_of_json: None,
            source_check: None,
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

    // ── From display name (`k2 mail send --from-name`) + Message-ID ──

    /// Names an agent may pick: kept verbatim (trimmed). Specials that
    /// matter in an address header (`"`, `,`, `<`, `>`, `\`, `;`, `:`)
    /// are allowed — the mail server's composer quotes them (see the
    /// Stalwart contract test below).
    #[test]
    fn from_name_accepts_real_names_verbatim() {
        for (raw, want) in [
            ("Laura", "Laura"),
            ("  Laura  ", "Laura"),
            ("Smith, John", "Smith, John"),
            (r#"Laura "LJ" Jones"#, r#"Laura "LJ" Jones"#),
            ("Laura <Sales>", "Laura <Sales>"),
            (r"Back\slash; colon:", r"Back\slash; colon:"),
            ("Zoë Ünal", "Zoë Ünal"),
            ("李雷", "李雷"),
            // ZWJ emoji sequence (woman technologist) — ZWJ stays allowed.
            ("Cassi \u{1F469}\u{200D}\u{1F4BB}", "Cassi \u{1F469}\u{200D}\u{1F4BB}"),
        ] {
            assert_eq!(validate_from_name(raw).expect(raw), want, "{raw:?}");
        }
    }

    fn usage_hint(r: Result<String, SendError>) -> String {
        match r {
            Err(SendError::Usage(h)) => h,
            other => panic!("expected a usage refusal, got {other:?}"),
        }
    }

    /// CR/LF (header injection) and every other control or invisible
    /// formatting character are refused — on the RAW value, so a trailing
    /// newline is refused, not trimmed away.
    #[test]
    fn from_name_refuses_cr_lf_and_control_characters() {
        for bad in [
            "Laura\r\nBcc: evil@example.com",
            "Laura\nX-Injected: 1",
            "Laura\r",
            "Laura\n",
            "\r\nLaura",
            "La\tura",
            "La\u{0}ura",
            "La\u{7F}ura",
            "La\u{85}ura",
            "La\u{2028}ura",
            "\u{202E}arual",
            "La\u{200B}ura",
            "\u{FEFF}Laura",
        ] {
            let hint = usage_hint(validate_from_name(bad));
            assert!(hint.contains("control or invisible"), "{bad:?} → {hint}");
        }
    }

    #[test]
    fn from_name_refuses_over_long_names_at_the_character_cap() {
        let at_cap = "a".repeat(MAX_FROM_NAME_CHARS);
        assert_eq!(validate_from_name(&at_cap).expect("128 chars is allowed"), at_cap);
        // The cap counts characters, not bytes: 128 multi-byte chars pass.
        let wide = "é".repeat(MAX_FROM_NAME_CHARS);
        assert_eq!(validate_from_name(&wide).expect("128 wide chars"), wide);

        let over = "a".repeat(MAX_FROM_NAME_CHARS + 1);
        let hint = usage_hint(validate_from_name(&over));
        assert!(hint.contains("too long (129 characters)"), "{hint}");
        assert!(hint.contains("max 128"), "{hint}");
    }

    /// Decision: a display name that looks like an address is refused —
    /// `"ceo@bank.com" <bot@acme.dev>` shows as ceo@bank.com in clients
    /// that hide the address. Fullwidth/small `@` lookalikes too.
    #[test]
    fn from_name_refuses_at_signs_and_empty_names() {
        for bad in ["ceo@bank.com", "Laura @ Acme", "ceo\u{FF20}bank.com", "ceo\u{FE6B}bank.com"] {
            let hint = usage_hint(validate_from_name(bad));
            assert!(hint.contains("can't contain '@'"), "{bad:?} → {hint}");
        }
        for empty in ["", "   "] {
            let hint = usage_hint(validate_from_name(empty));
            assert!(hint.contains("fromName is empty"), "{empty:?} → {hint}");
        }
    }

    /// The name reaches the JMAP Email object as a STRUCTURED value (never
    /// pre-quoted or pre-encoded by us), next to the unchanged address.
    #[test]
    fn submission_email_json_carries_the_chosen_from_name_unquoted() {
        let mut m = msg();
        m.from_name = Some(r#"Smith, "LJ" <Laura>"#.to_string());
        let v = submission_email_json(&m);
        let from = v["from"].as_array().expect("from is an array");
        assert_eq!(from.len(), 1, "exactly one From mailbox: {v}");
        assert_eq!(from[0]["name"], r#"Smith, "LJ" <Laura>"#);
        assert_eq!(from[0]["email"], "bot@acme.dev");
    }

    /// Message-ID uses the SENDING address's domain (not the box's
    /// hostname, which Stalwart's composer would otherwise stamp), in the
    /// JMAP bracket-less form, fresh per message.
    #[test]
    fn submission_email_json_stamps_a_message_id_on_the_sender_domain() {
        let m = msg();
        let a = submission_email_json(&m);
        let b = submission_email_json(&m);
        let ids = a["messageId"].as_array().expect("messageId is an array");
        assert_eq!(ids.len(), 1, "{a}");
        let id = ids[0].as_str().expect("string id");
        let (local, domain) = id.split_once('@').expect("id has an @");
        assert_eq!(domain, "acme.dev", "{id}");
        assert_eq!(local.len(), 32, "uuid simple form: {id}");
        assert!(local.chars().all(|c| c.is_ascii_hexdigit()), "{id}");
        assert!(!id.contains('<') && !id.contains('>'), "JMAP form has no brackets: {id}");
        assert_ne!(a["messageId"], b["messageId"], "fresh per message");
        let other = new_message_id_for("x@Sub.Example.org");
        assert_eq!(other.split_once('@').expect("@").1, "Sub.Example.org");
    }

    /// CONTRACT with the hosted path: Stalwart composes the From header
    /// from our JMAP `from[0]` with its own builder (mail-builder 1.0, the
    /// version Stalwart 0.16 pins). Render every allowed kind of name the
    /// same way and parse it back: exactly ONE mailbox, the exact name, the
    /// unchanged address — a `,`, `"` or `<…>` in the name never creates a
    /// second address, and non-ASCII is RFC 2047-encoded on the wire.
    #[test]
    fn hosted_from_header_contract_one_mailbox_exact_name() {
        let from_addr = "laura@alakazam-labs.com";
        for raw in [
            "Laura",
            "Smith, John",
            r#"Laura "LJ" Jones"#,
            "Laura <ceo-office>, Sales",
            r"Back\slash",
            "Zoë Ünal",
            "李雷",
        ] {
            let name = validate_from_name(raw).expect(raw);
            let mut m = msg();
            m.from = from_addr.to_string();
            m.from_name = Some(name.clone());
            let v = submission_email_json(&m);
            let jname = v["from"][0]["name"].as_str().expect("name").to_string();
            let jemail = v["from"][0]["email"].as_str().expect("email").to_string();
            let wire = mail_builder::MessageBuilder::new()
                .from((jname, jemail))
                .to("human@example.com")
                .subject("Digest")
                .text_body("All green.")
                .write_to_string()
                .expect("compose");
            let header_block = wire.split("\r\n\r\n").next().expect("headers").to_string();
            let from_headers = header_block
                .split("\r\n")
                .filter(|l| l.to_ascii_lowercase().starts_with("from:"))
                .count();
            assert_eq!(from_headers, 1, "exactly one From header for {raw:?}:\n{header_block}");
            if !raw.is_ascii() {
                assert!(
                    header_block.to_ascii_lowercase().contains("=?utf-8?"),
                    "non-ASCII name is RFC 2047-encoded for {raw:?}:\n{header_block}"
                );
                assert!(header_block.is_ascii(), "wire headers stay ASCII:\n{header_block}");
            }
            let parsed = mail_parser::MessageParser::default()
                .parse(wire.as_bytes())
                .expect("parses");
            let list = parsed
                .from()
                .and_then(|a| a.as_list())
                .expect("From is a mailbox list");
            assert_eq!(list.len(), 1, "one mailbox for {raw:?}: {list:?}\n{header_block}");
            assert_eq!(list[0].name(), Some(name.as_str()), "{header_block}");
            assert_eq!(list[0].address(), Some(from_addr), "{header_block}");
        }
    }

    // ── BCC (agent --bcc + owner always-BCC policy) ──

    /// A real BCC: every blind copy is in the SMTP envelope the hosted
    /// submit hands Stalwart (`all_recipients` → rcptTo), and NOTHING
    /// about it is in the Email object (the headers To/Cc recipients
    /// see). Duplicates across lists collapse to one RCPT.
    #[test]
    fn bcc_rides_the_envelope_and_never_the_email_headers() {
        let mut m = msg();
        m.cc = vec!["cc@example.com".to_string()];
        m.bcc = vec!["boss@shop.example".to_string(), "human@example.com".to_string()];
        m.policy_bcc = vec!["owner@shop.example".to_string()];

        let rcpt = m.all_recipients();
        assert_eq!(
            rcpt,
            vec![
                "human@example.com".to_string(),
                "cc@example.com".to_string(),
                "boss@shop.example".to_string(),
                "owner@shop.example".to_string(),
            ],
            "to + cc + bcc + policy bcc, deduped"
        );

        let v = submission_email_json(&m);
        assert!(v.get("bcc").is_none(), "no bcc field on the Email object: {v}");
        let wire = v.to_string();
        assert!(!wire.to_ascii_lowercase().contains("bcc"), "no Bcc header anywhere: {wire}");
        assert!(!wire.contains("boss@shop.example"), "agent bcc leaked into headers: {wire}");
        assert!(!wire.contains("owner@shop.example"), "policy bcc leaked into headers: {wire}");
        assert_eq!(v["cc"][0]["email"], "cc@example.com");
    }

    /// The policy stamps over an agent BCC of the same address (recorded
    /// once, as policy) and the cap counts the final envelope; when the
    /// policy pushes it over, the error names the policy — no recipient
    /// is ever dropped to make room.
    #[test]
    fn policy_bcc_merges_and_the_limit_names_the_policy() {
        let mut m = msg();
        m.bcc = vec!["owner@shop.example".to_string(), "qa@shop.example".to_string()];
        apply_policy_bcc(&mut m, vec!["owner@shop.example".to_string()]);
        assert_eq!(m.bcc, vec!["qa@shop.example".to_string()]);
        assert_eq!(m.policy_bcc, vec!["owner@shop.example".to_string()]);
        check_recipient_limit(&m).expect("3 recipients is under the cap");

        // 9 To + 2 policy = 11 > 10 → refused, naming the policy.
        let mut m = msg();
        m.to = (0..9).map(|i| format!("c{i}@cust.example")).collect();
        apply_policy_bcc(
            &mut m,
            vec!["owner@shop.example".to_string(), "audit@shop.example".to_string()],
        );
        let err = check_recipient_limit(&m).expect_err("policy pushes over the cap");
        let SendError::Usage(hint) = err else { panic!("usage expected: {err:?}") };
        assert!(hint.contains("too many recipients (11)"), "{hint}");
        assert!(hint.contains("always BCCs 2"), "{hint}");
        assert!(hint.contains("owner@shop.example"), "{hint}");
        assert_eq!(m.policy_bcc.len(), 2, "nothing was silently dropped");

        // Without a policy the plain to + cc + bcc wording.
        let mut m = msg();
        m.to = (0..8).map(|i| format!("c{i}@cust.example")).collect();
        m.bcc = (0..3).map(|i| format!("b{i}@cust.example")).collect();
        let err = check_recipient_limit(&m).expect_err("11 > 10");
        let SendError::Usage(hint) = err else { panic!("usage expected: {err:?}") };
        assert!(hint.contains("to + cc + bcc"), "{hint}");
    }

    /// Body-file round trip: the stored composed message keeps both BCC
    /// lists (the approval queue and the scheduler submit exactly this),
    /// and a pre-0131 body file without them still parses.
    #[test]
    fn stored_message_keeps_bcc_lists_and_old_bodies_still_parse() {
        let mut m = msg();
        m.bcc = vec!["boss@shop.example".to_string()];
        m.policy_bcc = vec!["owner@shop.example".to_string()];
        let raw = serde_json::to_string(&m).expect("serialize");
        let back: OutboundMessage = serde_json::from_str(&raw).expect("parse");
        assert_eq!(back.bcc, m.bcc);
        assert_eq!(back.policy_bcc, m.policy_bcc);

        let old = r#"{"from":"bot@acme.dev","to":["a@b.example"],"subject":"s","textBody":"t"}"#;
        let back: OutboundMessage = serde_json::from_str(old).expect("old body parses");
        assert!(back.bcc.is_empty() && back.policy_bcc.is_empty());
    }

    /// The outbox wire shape reports agent and policy BCCs apart.
    #[test]
    fn outbound_json_reports_agent_and_policy_bcc_apart() {
        let mut row = pending_row("out_bcc");
        row.bcc_json = Some(r#"["boss@shop.example"]"#.to_string());
        row.policy_bcc_json = Some(r#"["owner@shop.example"]"#.to_string());
        let v = outbound_json(&row);
        assert_eq!(v["bcc"], serde_json::json!(["boss@shop.example"]));
        assert_eq!(v["policyBcc"], serde_json::json!(["owner@shop.example"]));
        let v = outbound_json(&pending_row("out_plain"));
        assert_eq!(v["bcc"], serde_json::json!([]));
        assert_eq!(v["policyBcc"], serde_json::json!([]));
    }

    // ── reply guardrails ──

    fn mx() -> messages::AuthAuthority {
        messages::AuthAuthority::ThisBox(Some("mx".to_string()))
    }

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
        let rcpt = check_reply_guardrails(&c, "bot@acme.dev", Gate::On, &mx(), false).expect("allowed");
        assert_eq!(rcpt, "sender@example.com");

        // Reply-to-self refused.
        let c = ctx("bot@acme.dev", "pass", None);
        let err = check_reply_guardrails(&c, "bot@acme.dev", Gate::On, &mx(), false).expect_err("loop");
        assert!(matches!(&err, SendError::Guardrail(h) if h.contains("loop")), "{err:?}");

        // No sender refused.
        let mut c = ctx("x@y.z", "pass", None);
        c.from.clear();
        assert!(matches!(
            check_reply_guardrails(&c, "bot@acme.dev", Gate::On, &mx(), false),
            Err(SendError::Guardrail(_))
        ));

        // >100 references refused (thread-depth loop cap).
        let deep = "<r@x> ".repeat(101);
        let c = ctx("sender@example.com", "pass", Some(deep.trim()));
        let err = check_reply_guardrails(&c, "bot@acme.dev", Gate::On, &mx(), false).expect_err("deep");
        assert!(matches!(&err, SendError::Guardrail(h) if h.contains("References")), "{err:?}");
        // 100 exactly is allowed.
        let ok = "<r@x> ".repeat(100);
        let c = ctx("sender@example.com", "pass", Some(ok.trim()));
        check_reply_guardrails(&c, "bot@acme.dev", Gate::On, &mx(), false).expect("at the cap");

        // DMARC fail: blocked in on-mode, allowed under approval (the
        // human overrides, §8.4).
        let c = ctx("sender@example.com", "fail", None);
        let err = check_reply_guardrails(&c, "bot@acme.dev", Gate::On, &mx(), false).expect_err("dmarc");
        assert!(matches!(&err, SendError::Guardrail(h) if h.contains("DMARC")), "{err:?}");
        check_reply_guardrails(&c, "bot@acme.dev", Gate::Approval, &mx(), false)
            .expect("approval mode lets the human decide");
    }

    /// The gate reads this box's own verdict and nothing else.
    #[test]
    fn the_dmarc_gate_only_believes_this_boxes_stamp() {
        let with = |lines: Vec<String>| {
            let mut c = ctx("sender@example.com", "pass", None);
            c.auth_results = lines;
            c
        };
        let host = messages::AuthAuthority::ThisBox(Some("mail.example.test".to_string()));
        // The sender chose smtp.mailfrom; the box's real verdict is `fail`.
        let crafted = with(vec![
            "mail.example.test; spf=none smtp.mailfrom=\"dmarc=pass@evil.example\"; iprev=pass; \
             dmarc=fail (policy not aligned) header.from=gmail.com"
                .to_string(),
        ]);
        let err = check_reply_guardrails(&crafted, "bot@acme.dev", Gate::On, &host, false).expect_err("blocked");
        assert!(matches!(&err, SendError::Guardrail(h) if h.contains("DMARC")), "{err:?}");
        check_reply_guardrails(&crafted, "bot@acme.dev", Gate::Approval, &host, false).expect("a person decides");
        // More than one answer is a failure.
        let two = with(vec!["mail.example.test; dmarc=pass; dmarc=fail".to_string()]);
        assert!(check_reply_guardrails(&two, "bot@acme.dev", Gate::On, &host, false).is_err());
        // A failure below this box's stamp doesn't count; a header that came
        // in with the message isn't this box's, whatever it says.
        let forged_pass_below = with(vec![
            "mail.example.test; dmarc=fail".to_string(),
            "mx.evil.example; dmarc=pass".to_string(),
        ]);
        assert!(check_reply_guardrails(&forged_pass_below, "bot@acme.dev", Gate::On, &host, false).is_err());
        let v = messages::parse_auth_results(
            &["mx.evil.example; dmarc=pass".to_string()],
            &host,
        );
        assert!(v["dmarc"].is_null(), "not reported as passing: {v}");
        // Real stamps: two From headers read `none` with an empty header.from,
        // and a lookup that broke reads neither pass nor fail.
        let mut two = with(vec!["mail.example.test; dmarc=none header.from= policy.dmarc=none".to_string()]);
        two.from = vec![MailAddr { name: None, email: "ceo@paypal.com".to_string() }];
        let err = check_reply_guardrails(&two, "bot@acme.dev", Gate::On, &host, false).expect_err("blocked");
        assert!(matches!(&err, SendError::Guardrail(h) if h.contains("DMARC")), "{err:?}");
        check_reply_guardrails(&two, "bot@acme.dev", Gate::Approval, &host, false).expect("a person decides");
        let broken = with(vec!["mail.example.test; dmarc=permerror (x) header.from=example.com".to_string()]);
        assert!(check_reply_guardrails(&broken, "bot@acme.dev", Gate::On, &host, false).is_err());
        // No stamp: a mailbox on this box is fine, anyone else isn't.
        let bare = with(Vec::new());
        assert!(check_reply_guardrails(&bare, "bot@acme.dev", Gate::On, &host, false).is_err());
        check_reply_guardrails(&bare, "bot@acme.dev", Gate::On, &host, true).expect("between mailboxes");
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

    /// Verification review 2026-10-26, item 4: a one-time-link mail never
    /// keeps its link at rest. Sent at once: only the link-hidden copy is
    /// written. Held for approval: the full body waits (the approver sends
    /// it), then is rewritten link-hidden once sent or denied.
    #[test]
    fn a_one_time_link_mail_keeps_no_link_at_rest() {
        struct Keep(Mutex<Vec<OutboundMessage>>);
        impl SubmitBackend for Keep {
            fn submit(&self, _account_id: &str, msg: &OutboundMessage) -> Result<(), String> {
                self.0.lock().unwrap().push(msg.clone());
                Ok(())
            }
        }
        let (store, dir) = temp_store();
        let project = format!("P-{}", uuid::Uuid::new_v4());
        let link = "https://box.example/cli/skin/password/set#k2inv_abcDEF123";
        let mut m = msg();
        m.from = format!("bot-{}@acme.dev", uuid::Uuid::new_v4().simple());
        m.subject = "Your invite".to_string();
        m.text_body = format!("Set your password: {link}\n");
        m.ephemeral = true;
        let body_of = |id: &str| std::fs::read_to_string(dir.join(format!("{id}.json"))).expect("body file");
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;

        // On: the person gets the link; the file never had it.
        let keep = Keep(Mutex::new(Vec::new()));
        let req = OutboundRequest { project_id: &project, agent_name: "bot", account_id: "acct", message: &m, send_after: None };
        let SendOutcome::Submitted { id } = gate_and_dispatch(&store, Some(&keep), Gate::On, &req, now).expect("send") else {
            panic!("expected a submit")
        };
        assert!(keep.0.lock().unwrap()[0].text_body.contains("k2inv_abcDEF123"), "the mail itself carries the link");
        let at_rest = body_of(&id);
        assert!(!at_rest.contains("k2inv_abcDEF123"), "{at_rest}");
        assert!(at_rest.contains("k2inv_[hidden]"), "{at_rest}");

        // Approval: the waiting body keeps it until the decision, then not.
        for approve in [true, false] {
            let SendOutcome::Queued { id, .. } = gate_and_dispatch(&store, None, Gate::Approval, &req, now).expect("queue") else {
                panic!("expected a queued mail")
            };
            assert!(body_of(&id).contains("k2inv_abcDEF123"), "the approver sends the real link");
            if approve {
                let keep = Keep(Mutex::new(Vec::new()));
                let out = approve_and_submit(&store, Ok(&keep), &mut |_: &str| Ok::<String, String>("acct".to_string()), &id, "owner", None, now).expect("approve");
                assert_eq!(out, ApproveOutcome::Submitted);
                assert!(keep.0.lock().unwrap()[0].text_body.contains("k2inv_abcDEF123"));
            } else {
                deny(&store, &id, "owner", "no", now).expect("deny");
            }
            let at_rest = body_of(&id);
            assert!(!at_rest.contains("k2inv_abcDEF123"), "approve={approve}: {at_rest}");
        }

        // A plain mail is kept as is.
        let mut plain = m.clone();
        plain.ephemeral = false;
        let req = OutboundRequest { project_id: &project, agent_name: "bot", account_id: "acct", message: &plain, send_after: None };
        let keep = Keep(Mutex::new(Vec::new()));
        let SendOutcome::Submitted { id } = gate_and_dispatch(&store, Some(&keep), Gate::On, &req, now).expect("send") else {
            panic!("expected a submit")
        };
        assert!(body_of(&id).contains("k2inv_abcDEF123"), "only one-time-link mails are rewritten");
        cleanup_rows(&project);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn db_store_roundtrip_transitions_counts_and_0600_bodies() {
        let (store, dir) = temp_store();
        let project = format!("P-{}", uuid::Uuid::new_v4());
        // Unique From: rate counting is per address and the DB is the
        // shared test DB — a fixed address would race sibling tests.
        let mut m = msg();
        m.from = format!("bot-{}@acme.dev", uuid::Uuid::new_v4().simple());
        // Real-clock timestamps: auto_deny_expired sweeps the WHOLE table
        // (expired_pending_rows_auto_deny with now=10_000_000, and every
        // send route with now_secs()). A 1970 created_at made this pending
        // row look 7+ days old, so a concurrent sweep flipped it to denied.
        let t0 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_secs() as i64;
        let id = store
            .insert(&NewOutbound {
                owner_project_id: &project,
                agent_name: "bot",
                message: &m,
                status: "pending",
                decided_by: None,
                attachment_names: &[],
                attachment_files: &[],
                send_after: None,
                now: t0,
            })
            .expect("insert");
        assert!(id.starts_with("out_"), "{id}");
        // A no-attachment insert leaves attachments_ref NULL.
        assert!(store.load(&id).unwrap().unwrap().attachments_ref.is_none());

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
        assert_eq!(store.count_recent(&m.from, t0).unwrap(), 1);
        assert_eq!(store.count_recent(&m.from, t0 + 1).unwrap(), 0);
        assert_eq!(store.count_recent("other@acme.dev", 0).unwrap(), 0);

        // Transitions are ATOMIC on the current status.
        assert!(store
            .transition(&id, "pending", "denied", Some("owner"), Some("no"), t0 + 500)
            .unwrap());
        assert!(
            !store
                .transition(&id, "pending", "approved", Some("owner"), None, t0 + 600)
                .unwrap(),
            "a decided row never re-transitions from pending"
        );
        let row = store.load(&id).expect("load").expect("row");
        assert_eq!(row.status, "denied");
        assert_eq!(row.decided_by.as_deref(), Some("owner"));
        assert_eq!(row.note.as_deref(), Some("no"));
        assert_eq!(row.decided_at, Some(t0 + 500));

        store.append_note(&id, "extra line", t0 + 700).expect("note");
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
                attachment_names: &[],
                attachment_files: &[],
                send_after: None,
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
                attachment_names: &[],
                attachment_files: &[],
                send_after: None,
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


    #[test]
    fn on_mode_with_future_send_after_inserts_scheduled_not_submitted() {
        let store = FakeStore {
            transition_result: Some(true),
            ..Default::default()
        };
        let submit = FakeSubmit::default();
        let m = msg();
        let now = 1_000_000i64;
        let after = now + 3_600;
        let out = gate_and_dispatch(&store, Some(&submit), Gate::On, &req_at(&m, after), now)
            .expect("scheduled");
        match out {
            SendOutcome::Scheduled { send_after, .. } => assert_eq!(send_after, after),
            other => panic!("expected Scheduled, got {other:?}"),
        }
        assert_eq!(submit.calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            store.inserts.lock().unwrap().as_slice(),
            &[(
                "scheduled".to_string(),
                Some("policy:agent-send-on".to_string()),
                Some(after)
            )]
        );
    }

    #[test]
    fn approval_with_send_after_then_approve_schedules() {
        let m = msg();
        let now = 1_000_000i64;
        let after = now + 7_200;
        let store = FakeStore {
            transition_result: Some(true),
            ..Default::default()
        };
        let out = gate_and_dispatch(&store, None, Gate::Approval, &req_at(&m, after), now)
            .expect("queued");
        assert!(matches!(out, SendOutcome::Queued { send_after: Some(t), .. } if t == after));

        let mut pending = pending_row("out_sched");
        pending.send_after = Some(after);
        let store = FakeStore {
            row: Some(pending),
            transition_result: Some(true),
            ..Default::default()
        };
        let submit = FakeSubmit::default();
        let mut acct = |_from: &str| Ok("acc-1".to_string());
        let out = approve_and_submit(&store, Ok(&submit), &mut acct, "out_sched", "owner", None, now + 10)
            .expect("ok");
        assert_eq!(out, ApproveOutcome::Scheduled { send_after: after });
        assert_eq!(submit.calls.load(Ordering::SeqCst), 0);
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

    // ── hosted attachments ──

    fn with_file(mut m: OutboundMessage, name: &str, ct: &str, bytes: &[u8]) -> OutboundMessage {
        use crate::mail::external_smtp::OutAttachment;
        m.attachments.push(StoredAttachment::from_out(&OutAttachment {
            scanned: false, filename: name.to_string(),
            content_type: ct.to_string(),
            bytes: bytes.to_vec(),
        }));
        m
    }

    fn now_unix() -> i64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
    }

    fn unique_from(mut m: OutboundMessage) -> OutboundMessage {
        m.from = format!("bot-{}@acme.dev", uuid::Uuid::new_v4().simple());
        m
    }

    #[test]
    fn a_stored_file_knows_its_size_without_decoding() {
        for n in [0usize, 1, 2, 3, 4, 5, 1000, 1001, 1002] {
            let m = with_file(msg(), "f.bin", "application/octet-stream", &vec![7u8; n]);
            assert_eq!(m.attachments[0].size(), n as u64, "{n} bytes");
        }
    }

    /// On: the files go up, the composed message names them, the outbox keeps
    /// only their names, and the body file never held the bytes.
    #[test]
    fn on_mode_uploads_the_files_and_keeps_only_their_names() {
        let fake = crate::mail::fake_jmap::FakeJmap::start();
        let client = fake.client();
        let (store, dir) = temp_store();
        let project = format!("P-{}", uuid::Uuid::new_v4());
        let secret_bytes: &[u8] = b"%PDF the real report bytes";
        let m = with_file(
            with_file(unique_from(msg()), "report.pdf", "application/pdf", secret_bytes),
            "notes.txt",
            "text/plain; charset=utf-8",
            b"hello\n",
        );
        let req = OutboundRequest { project_id: &project, agent_name: "bot", account_id: "acct", message: &m, send_after: None };
        let out = gate_and_dispatch(&store, Some(&client), Gate::On, &req, now_unix()).expect("dispatch");
        let SendOutcome::Submitted { id } = out else { panic!("expected a submit, got {out:?}") };

        let st = fake.state.lock().unwrap();
        assert_eq!(st.uploads, vec![secret_bytes.to_vec(), b"hello\n".to_vec()], "the bytes, in order");
        let e = &st.created[0];
        assert_eq!(e["attachments"][0]["blobId"], "up-1", "{e}");
        assert_eq!(e["attachments"][0]["name"], "report.pdf", "{e}");
        assert_eq!(e["attachments"][0]["type"], "application/pdf", "{e}");
        assert!(e["attachments"][0].get("charset").is_none(), "{e}");
        assert_eq!(e["attachments"][1]["type"], "text/plain", "a bare type: {e}");
        assert_eq!(e["attachments"][1]["charset"], "utf-8", "{e}");
        assert_eq!(e["attachments"][1]["disposition"], "attachment", "{e}");
        assert_eq!(st.envelopes.len(), 1, "one submission");
        drop(st);

        let row = store.load(&id).unwrap().expect("row");
        assert_eq!(row.status, "sent");
        let v = outbound_json(&row);
        assert_eq!(v["attachments"]["names"], serde_json::json!(["report.pdf", "notes.txt"]), "{v}");
        let at_rest = std::fs::read_to_string(dir.join(format!("{id}.json"))).expect("body file");
        assert!(!at_rest.contains("dataB64") && !at_rest.contains("JVBERi"), "no bytes at rest: {at_rest}");
        assert!(!dir.join(format!("{id}.files")).exists(), "on mode never holds the files");
        cleanup_rows(&project);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A refused upload sends nothing and says which file.
    #[test]
    fn a_refused_upload_fails_the_row_and_composes_nothing() {
        let fake = crate::mail::fake_jmap::FakeJmap::start();
        fake.state.lock().unwrap().upload_status = Some(413);
        let client = fake.client();
        let (store, dir) = temp_store();
        let project = format!("P-{}", uuid::Uuid::new_v4());
        let m = with_file(unique_from(msg()), "big.zip", "application/zip", b"zip");
        let req = OutboundRequest { project_id: &project, agent_name: "bot", account_id: "acct", message: &m, send_after: None };
        let out = gate_and_dispatch(&store, Some(&client), Gate::On, &req, now_unix()).expect("dispatch");
        let SendOutcome::SubmitFailed { id, error } = out else { panic!("expected a failure, got {out:?}") };
        assert!(error.contains("big.zip"), "{error}");
        assert_eq!(store.load(&id).unwrap().unwrap().status, "failed");
        let st = fake.state.lock().unwrap();
        assert!(st.created.is_empty() && st.envelopes.is_empty(), "nothing composed or submitted");
        drop(st);
        cleanup_rows(&project);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Scheduled: the bytes stay until the flush sends them, then go.
    #[test]
    fn a_scheduled_message_holds_its_files_until_it_leaves() {
        let fake = crate::mail::fake_jmap::FakeJmap::start();
        let client = fake.client();
        let (store, dir) = temp_store();
        let project = format!("P-{}", uuid::Uuid::new_v4());
        let now = now_unix();
        let m = with_file(unique_from(msg()), "a.bin", "application/octet-stream", b"scheduled bytes");
        let req = OutboundRequest { project_id: &project, agent_name: "bot", account_id: "acct", message: &m, send_after: Some(now + 3600) };
        let SendOutcome::Scheduled { id, .. } = gate_and_dispatch(&store, Some(&client), Gate::On, &req, now).expect("dispatch") else {
            panic!("expected a schedule")
        };
        assert_eq!(store.load_message(&id).unwrap().attachments.len(), 1, "held for the flush");
        assert!(fake.state.lock().unwrap().uploads.is_empty(), "nothing left yet");
        let mut acct = |_: &str| Ok("acct".to_string());
        let mut rules = |_: &str, _: &str| Ok(crate::mail::schedule::FlushRules { gate: Gate::On, policy_bcc: Vec::new(), refused: None });
        let out = crate::mail::schedule::flush_one_scheduled(&store, Ok(&client), &mut acct, &mut rules, &id, now + 3601).expect("flush");
        assert_eq!(out, Some(crate::mail::schedule::FlushOutcome::Sent));
        assert_eq!(fake.state.lock().unwrap().uploads, vec![b"scheduled bytes".to_vec()]);
        assert!(store.load_message(&id).unwrap().attachments.is_empty(), "gone once sent");
        cleanup_rows(&project);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Cancelling a schedule drops the bytes too.
    #[test]
    fn a_cancelled_schedule_drops_its_files() {
        let (store, dir) = temp_store();
        let project = format!("P-{}", uuid::Uuid::new_v4());
        let now = now_unix();
        let m = with_file(unique_from(msg()), "a.bin", "application/octet-stream", b"x");
        let req = OutboundRequest { project_id: &project, agent_name: "bot", account_id: "acct", message: &m, send_after: Some(now + 3600) };
        let keep = crate::mail::fake_jmap::FakeJmap::start().client();
        let SendOutcome::Scheduled { id, .. } = gate_and_dispatch(&store, Some(&keep), Gate::On, &req, now).expect("dispatch") else {
            panic!("expected a schedule")
        };
        crate::mail::schedule::cancel_scheduled(&store, &id, &project, "agent:bot", now).expect("cancel");
        assert!(store.load_message(&id).unwrap().attachments.is_empty());
        cleanup_rows(&project);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The hosted backend never drops a file it was handed apart from the
    /// message: it refuses.
    #[test]
    fn hosted_submit_refuses_files_that_did_not_ride_the_message() {
        use crate::mail::submit::{HostedSubmit, MailSubmit, SubmitError};
        let client = crate::mail::fake_jmap::FakeJmap::start().client();
        let hosted = HostedSubmit { backend: &client, account_id: "acct".to_string() };
        let stray = crate::mail::external_smtp::OutAttachment {
            scanned: false, filename: "a".to_string(),
            content_type: "text/plain".to_string(),
            bytes: vec![1],
        };
        assert!(matches!(hosted.submit(&msg(), &[stray]), Err(SubmitError::Engine(_))));
    }

    #[test]
    fn hosted_text_limit_counts_the_json_form() {
        assert!(check_hosted_text_size("s", &"y".repeat(MAX_HOSTED_TEXT_JSON_BYTES - 100)).is_ok());
        let err = check_hosted_text_size("s", &"y".repeat(MAX_HOSTED_TEXT_JSON_BYTES + 1)).unwrap_err();
        assert!(matches!(err, SendError::Usage(ref h) if h.contains("too large")), "{err:?}");
        // Control characters take six bytes each once escaped.
        let err = check_hosted_text_size("s", &"\u{1}".repeat(MAX_HOSTED_TEXT_JSON_BYTES / 4)).unwrap_err();
        assert!(matches!(err, SendError::Usage(_)), "{err:?}");
    }

    #[cfg(unix)]
    #[test]
    fn a_waiting_body_file_is_private_and_never_overwrites() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("k2-body-mode-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("out_x.json");
        write_private_new(&f, b"{}").expect("first write");
        let mode = std::fs::metadata(&f).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "{mode:o}");
        assert!(write_private_new(&f, b"again").is_err(), "an id is never reused");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn normalize_reply_to_caps_and_names_the_flag() {
        assert_eq!(normalize_reply_to(&["A@B.example".to_string()]).unwrap(), vec!["a@b.example"]);
        let four: Vec<String> = (0..4).map(|i| format!("r{i}@x.example")).collect();
        assert!(matches!(normalize_reply_to(&four), Err(SendError::Usage(h)) if h.contains("max 3")));
        match normalize_reply_to(&["a@b.example\r\nBcc: c@d.example".to_string()]) {
            Err(SendError::Usage(h)) => assert!(h.contains("--reply-to"), "{h}"),
            other => panic!("expected a refusal: {other:?}"),
        }
    }

    /// The header goes to the engine as a structured list, next to the rest.
    #[test]
    fn on_mode_reply_to_reaches_the_engine_as_a_header_not_a_recipient() {
        let fake = crate::mail::fake_jmap::FakeJmap::start();
        let client = fake.client();
        let (store, dir) = temp_store();
        let project = format!("P-{}", uuid::Uuid::new_v4());
        let mut m = unique_from(msg());
        m.reply_to = vec!["answers@acme.dev".to_string()];
        let req = OutboundRequest { project_id: &project, agent_name: "bot", account_id: "acct", message: &m, send_after: None };
        let SendOutcome::Submitted { id } = gate_and_dispatch(&store, Some(&client), Gate::On, &req, now_unix()).expect("dispatch") else {
            panic!("expected a submit")
        };
        let st = fake.state.lock().unwrap();
        assert_eq!(st.created[0]["replyTo"], serde_json::json!([{ "email": "answers@acme.dev" }]), "{}", st.created[0]);
        let rcpt = st.envelopes[0]["rcptTo"].to_string();
        assert!(!rcpt.contains("answers@acme.dev"), "{rcpt}");
        drop(st);
        assert_eq!(outbound_json(&store.load(&id).unwrap().unwrap())["replyTo"], serde_json::json!(["answers@acme.dev"]));
        cleanup_rows(&project);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── holding files: disk, tampering, purges ──

    fn queued_with_file(store: &DbOutboundStore, project: &str, bytes: &[u8]) -> String {
        let m = with_file(unique_from(msg()), "a.bin", "application/octet-stream", bytes);
        let req = OutboundRequest { project_id: project, agent_name: "bot", account_id: "acct", message: &m, send_after: None };
        match gate_and_dispatch(store, None, Gate::Approval, &req, now_unix()) {
            Ok(SendOutcome::Queued { id, .. }) => id,
            other => panic!("expected a queued message: {other:?}"),
        }
    }

    /// A held file is its own 0600 file, the body names it, and the queue can
    /// be listed without reading it.
    #[cfg(unix)]
    #[test]
    fn held_bytes_live_beside_the_body() {
        use std::os::unix::fs::PermissionsExt;
        let (store, dir) = temp_store();
        let project = format!("P-{}", uuid::Uuid::new_v4());
        let id = queued_with_file(&store, &project, b"held bytes");
        let file = dir.join(format!("{id}.files/0"));
        assert_eq!(std::fs::read(&file).unwrap(), b"held bytes");
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(dir.join(format!("{id}.files"))).unwrap().permissions().mode() & 0o777, 0o700);
        let body = std::fs::read_to_string(dir.join(format!("{id}.json"))).unwrap();
        assert!(!body.contains("dataB64"), "{body}");
        // The list's read doesn't touch the bytes: gone, it still answers.
        std::fs::remove_file(&file).unwrap();
        let m = store.load_message(&id).expect("named, not read");
        assert_eq!(m.attachments[0].size(), 10);
        assert!(store.load_message_full(&id).is_err(), "a send needs them");
        cleanup_rows(&project);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An edited body or held file fails closed: the row ends failed, nothing leaves.
    #[test]
    fn a_body_or_file_changed_after_queueing_never_leaves() {
        for edit_file in [false, true] {
            let (store, dir) = temp_store();
            let project = format!("P-{}", uuid::Uuid::new_v4());
            let id = queued_with_file(&store, &project, b"what was approved");
            if edit_file {
                std::fs::write(dir.join(format!("{id}.files/0")), b"swapped").unwrap();
            } else {
                let path = dir.join(format!("{id}.json"));
                let raw = std::fs::read_to_string(&path).unwrap();
                let edited = raw.replacen("\"to\":[\"", "\"replyTo\":[\"x@y.example\\r\\nBcc: z@y.example\"],\"to\":[\"", 1);
                assert_ne!(raw, edited);
                std::fs::write(&path, edited).unwrap();
                assert!(store.load_message(&id).is_err(), "not shown either");
            }
            let fake = crate::mail::fake_jmap::FakeJmap::start();
            let client = fake.client();
            let mut acct = |_: &str| Ok("acct".to_string());
            let out = approve_and_submit(&store, Ok(&client), &mut acct, &id, "owner", None, now_unix()).expect("decided");
            assert!(matches!(&out, ApproveOutcome::FailedToSubmit(r) if r.contains("changed")), "{out:?}");
            assert_eq!(store.load(&id).unwrap().unwrap().status, "failed");
            let st = fake.state.lock().unwrap();
            assert!(st.created.is_empty() && st.uploads.is_empty(), "nothing left");
            drop(st);
            cleanup_rows(&project);
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// The last check before the mail server, for a body from anywhere.
    #[test]
    fn an_address_that_isnt_plain_never_reaches_the_engine() {
        let fake = crate::mail::fake_jmap::FakeJmap::start();
        let client = fake.client();
        let mut m = msg();
        m.reply_to = vec!["x@y.example\r\nBcc: z@y.example".to_string()];
        let err = client.submit("acct", &m).expect_err("refused");
        assert!(err.contains("plain local@domain"), "{err}");
        let mut m = msg();
        m.to = vec!["Not@Normal.Example".to_string()];
        assert!(client.submit("acct", &m).is_err());
        assert!(fake.state.lock().unwrap().created.is_empty());
    }

    #[test]
    fn the_box_refuses_to_hold_past_its_cap_or_its_disk() {
        let (mut store, dir) = temp_store();
        let project = format!("P-{}", uuid::Uuid::new_v4());
        store.max_held_bytes = 15;
        let first = queued_with_file(&store, &project, &[1u8; 10]);
        assert!(!first.is_empty());
        let m = with_file(unique_from(msg()), "b.bin", "application/octet-stream", &[2u8; 10]);
        let req = OutboundRequest { project_id: &project, agent_name: "bot", account_id: "acct", message: &m, send_after: None };
        let err = gate_and_dispatch(&store, None, Gate::Approval, &req, now_unix()).expect_err("over the cap");
        assert!(matches!(&err, SendError::Guardrail(h) if h.contains("already holding")), "{err:?}");
        // Deciding what's waiting makes room.
        deny(&store, &first, "owner", "no", now_unix()).expect("deny");
        assert!(gate_and_dispatch(&store, None, Gate::Approval, &req, now_unix()).is_ok());

        store.max_held_bytes = MAX_HELD_BYTES;
        store.min_free_after_hold = u64::MAX / 2;
        let err = store.check_can_hold(&project, 1, 0).expect_err("disk");
        assert!(err.contains("too full"), "{err}");
        // On mode holds nothing, so it needs neither.
        let fake = crate::mail::fake_jmap::FakeJmap::start();
        let client = fake.client();
        assert!(matches!(
            gate_and_dispatch(&store, Some(&client), Gate::On, &req, now_unix()),
            Ok(SendOutcome::Submitted { .. })
        ));
        cleanup_rows(&project);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_message_with_files_is_scheduled_at_most_7_days_ahead() {
        let (store, dir) = temp_store();
        let project = format!("P-{}", uuid::Uuid::new_v4());
        let now = now_unix();
        let m = with_file(unique_from(msg()), "a.bin", "application/octet-stream", b"x");
        for gate in [Gate::On, Gate::Approval] {
            let req = OutboundRequest { project_id: &project, agent_name: "bot", account_id: "acct", message: &m, send_after: Some(now + 8 * 86_400) };
            let err = gate_and_dispatch(&store, None, gate, &req, now).expect_err("too far");
            assert!(matches!(&err, SendError::Usage(h) if h.contains("7 days")), "{err:?}");
        }
        let no_file = unique_from(msg());
        let req = OutboundRequest { project_id: &project, agent_name: "bot", account_id: "acct", message: &no_file, send_after: Some(now + 8 * 86_400) };
        assert!(matches!(gate_and_dispatch(&store, None, Gate::On, &req, now), Ok(SendOutcome::Scheduled { .. })), "text alone keeps 30 days");
        cleanup_rows(&project);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_removed_mailbox_ends_its_waiting_mail_and_keeps_no_files() {
        let store = DbOutboundStore::default();
        let project = format!("P-{}", uuid::Uuid::new_v4());
        let m = with_file(unique_from(msg()), "a.bin", "application/octet-stream", b"x");
        let req = OutboundRequest { project_id: &project, agent_name: "bot", account_id: "acct", message: &m, send_after: None };
        let SendOutcome::Queued { id, .. } = gate_and_dispatch(&store, None, Gate::Approval, &req, now_unix()).expect("queue") else {
            panic!("expected a queued message")
        };
        assert_eq!(purge_held_for_address(&m.from, now_unix()), 1);
        let row = store.load(&id).unwrap().unwrap();
        assert_eq!(row.status, "denied");
        assert!(row.note.as_deref().unwrap_or("").contains("mailbox was removed"), "{:?}", row.note);
        assert!(store.load_message(&id).unwrap().attachments.is_empty());
        assert!(!store.files_dir(&id).exists());
        cleanup_rows(&project);
    }

    /// A linked message queued by an older K2 keeps working.
    #[test]
    fn a_body_from_an_older_k2_still_reads() {
        let raw = r#"{"from":"a@b.example","to":["c@d.example"],"subject":"s","textBody":"t",
            "attachments":[{"filename":"f.txt","contentType":"text/plain","dataB64":"aGk="}]}"#;
        let m: OutboundMessage = serde_json::from_str(raw).expect("parse");
        assert_eq!(m.attachments[0].size(), 2);
        assert_eq!(m.attachments[0].bytes_now().unwrap(), b"hi");
    }

    /// The outbox can show later which bytes left: names, sizes and hashes. A
    /// row from before this keeps reading as names.
    #[test]
    fn the_outbox_keeps_each_files_size_and_hash_never_its_bytes() {
        let fake = crate::mail::fake_jmap::FakeJmap::start();
        let client = fake.client();
        let (store, dir) = temp_store();
        let project = format!("P-{}", uuid::Uuid::new_v4());
        let m = with_file(unique_from(msg()), "a.bin", "application/octet-stream", b"the bytes");
        let req = OutboundRequest { project_id: &project, agent_name: "bot", account_id: "acct", message: &m, send_after: None };
        let SendOutcome::Submitted { id } = gate_and_dispatch(&store, Some(&client), Gate::On, &req, now_unix()).expect("send") else {
            panic!("expected a submit")
        };
        let row = store.load(&id).unwrap().unwrap();
        assert!(!row.attachments_ref.as_deref().unwrap_or("").contains("dGhl"), "no bytes");
        let v = outbound_json(&row);
        assert_eq!(v["attachments"]["names"], serde_json::json!(["a.bin"]), "{v}");
        assert_eq!(v["attachments"]["files"][0]["size"], 9, "{v}");
        assert_eq!(v["attachments"]["files"][0]["sha256"], sha256_hex(b"the bytes"), "{v}");
        let old = MailOutbound { attachments_ref: Some(r#"["old.pdf"]"#.to_string()), ..row };
        let v = outbound_json(&old);
        assert_eq!(v["attachments"]["names"], serde_json::json!(["old.pdf"]), "{v}");
        assert!(v["attachments"].get("files").is_none(), "{v}");
        cleanup_rows(&project);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── the second review ──

    /// A stuck reader mustn't hold the flusher: a FIFO in a held file's place
    /// fails at once, and so does a link.
    #[cfg(unix)]
    #[test]
    fn a_fifo_or_link_in_a_held_files_place_fails_at_once() {
        let (store, dir) = temp_store();
        let project = format!("P-{}", uuid::Uuid::new_v4());
        let id = queued_with_file(&store, &project, b"bytes");
        let file = dir.join(format!("{id}.files/0"));
        std::fs::remove_file(&file).unwrap();
        let c = std::ffi::CString::new(file.to_string_lossy().as_bytes()).unwrap();
        // SAFETY: a NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        let (tx, rx) = std::sync::mpsc::channel();
        let (store2, id2) = (DbOutboundStore::at(dir.clone()), id.clone());
        std::thread::spawn(move || {
            let _ = tx.send(store2.load_message_full(&id2).map(|_| ()));
        });
        let out = rx.recv_timeout(std::time::Duration::from_secs(20)).expect("hung on a FIFO");
        assert!(out.is_err(), "{out:?}");
        std::fs::remove_file(&file).unwrap();
        let elsewhere = dir.join("elsewhere");
        std::fs::write(&elsewhere, b"bytes").unwrap();
        std::os::unix::fs::symlink(&elsewhere, &file).unwrap();
        assert!(store.load_message_full(&id).is_err(), "a link isn't the held file");
        cleanup_rows(&project);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Finding 10: a body changed and its hash cleared still doesn't leave, as
    /// what the approver was shown came from the row.
    #[test]
    fn a_body_that_disagrees_with_its_row_never_leaves() {
        let (store, dir) = temp_store();
        let project = format!("P-{}", uuid::Uuid::new_v4());
        let id = queued_with_file(&store, &project, b"x");
        let path = dir.join(format!("{id}.json"));
        let raw = std::fs::read_to_string(&path).unwrap();
        let edited = raw.replace("human@example.com", "evil@x.example");
        assert_ne!(raw, edited);
        std::fs::write(&path, edited).unwrap();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute("UPDATE mail_outbound SET body_sha256 = NULL WHERE id = ?1", rusqlite::params![id]).unwrap();
        }
        let fake = crate::mail::fake_jmap::FakeJmap::start();
        let client = fake.client();
        let mut acct = |_: &str| Ok("acct".to_string());
        let out = approve_and_submit(&store, Ok(&client), &mut acct, &id, "owner", None, now_unix()).expect("decided");
        assert!(matches!(&out, ApproveOutcome::FailedToSubmit(r) if r.contains("changed")), "{out:?}");
        assert!(fake.state.lock().unwrap().created.is_empty(), "nothing left");
        cleanup_rows(&project);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn row_and_message_agree_on_everything_the_approver_sees() {
        let (store, dir) = temp_store();
        let project = format!("P-{}", uuid::Uuid::new_v4());
        let mut m = unique_from(msg());
        m.cc = vec!["cc@x.example".to_string()];
        m.bcc = vec!["b@x.example".to_string()];
        m.reply_to = vec!["r@x.example".to_string()];
        let req = OutboundRequest { project_id: &project, agent_name: "bot", account_id: "acct", message: &m, send_after: None };
        let SendOutcome::Queued { id, .. } = gate_and_dispatch(&store, None, Gate::Approval, &req, now_unix()).expect("queue") else {
            panic!("expected a queued message")
        };
        let row = store.load(&id).unwrap().unwrap();
        assert!(row_matches_message(&row, &m));
        for change in [
            (|m: &mut OutboundMessage| m.to.push("x@y.example".to_string())) as fn(&mut OutboundMessage),
            |m| m.cc.clear(),
            |m| m.bcc.push("x@y.example".to_string()),
            |m| m.reply_to = vec!["x@y.example".to_string()],
            |m| m.from = "x@y.example".to_string(),
            |m| m.subject = "other".to_string(),
        ] {
            let mut changed = m.clone();
            change(&mut changed);
            assert!(!row_matches_message(&row, &changed), "{changed:?}");
        }
        cleanup_rows(&project);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// N4: parallel sends can't all pass a limit with room for one.
    #[test]
    fn parallel_holds_cannot_overrun_the_cap() {
        let (mut store, dir) = temp_store();
        store.max_held_bytes = 1000;
        let store = std::sync::Arc::new(store);
        let project = format!("P-{}", uuid::Uuid::new_v4());
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let (store, project) = (store.clone(), project.clone());
                std::thread::spawn(move || {
                    let m = with_file(unique_from(msg()), "a.bin", "application/octet-stream", &[1u8; 400]);
                    let req = OutboundRequest { project_id: &project, agent_name: "bot", account_id: "acct", message: &m, send_after: None };
                    gate_and_dispatch(&*store, None, Gate::Approval, &req, now_unix()).is_ok()
                })
            })
            .collect();
        let ok = handles.into_iter().map(|h| h.join().unwrap()).filter(|b| *b).count();
        assert_eq!(ok, 2, "two fit under 1000 bytes");
        assert_eq!(store.held_bytes(), 800);
        cleanup_rows(&project);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn one_workspace_cannot_take_the_whole_cap() {
        let (mut store, dir) = temp_store();
        store.max_held_per_workspace = 15;
        let (a, b) = (format!("P-{}", uuid::Uuid::new_v4()), format!("P-{}", uuid::Uuid::new_v4()));
        queued_with_file(&store, &a, &[1u8; 10]);
        let m = with_file(unique_from(msg()), "b.bin", "application/octet-stream", &[2u8; 10]);
        let req = OutboundRequest { project_id: &a, agent_name: "bot", account_id: "acct", message: &m, send_after: None };
        let err = gate_and_dispatch(&store, None, Gate::Approval, &req, now_unix()).expect_err("over its share");
        assert!(matches!(&err, SendError::Guardrail(h) if h.contains("this workspace")), "{err:?}");
        let other = OutboundRequest { project_id: &b, agent_name: "bot", account_id: "acct", message: &m, send_after: None };
        assert!(gate_and_dispatch(&store, None, Gate::Approval, &other, now_unix()).is_ok(), "another workspace has room");
        cleanup_rows(&a);
        cleanup_rows(&b);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn long_text_needs_disk_too_and_short_text_doesnt() {
        let (mut store, dir) = temp_store();
        store.min_free_after_hold = u64::MAX / 2;
        let project = format!("P-{}", uuid::Uuid::new_v4());
        let mut long = unique_from(msg());
        long.text_body = "y".repeat(TEXT_FLOOR_FROM_BYTES as usize + 1);
        let req = OutboundRequest { project_id: &project, agent_name: "bot", account_id: "acct", message: &long, send_after: None };
        let err = gate_and_dispatch(&store, None, Gate::Approval, &req, now_unix()).expect_err("no room");
        assert!(matches!(&err, SendError::Guardrail(h) if h.contains("too full")), "{err:?}");
        let short = unique_from(msg());
        let req = OutboundRequest { project_id: &project, agent_name: "bot", account_id: "acct", message: &short, send_after: None };
        assert!(gate_and_dispatch(&store, None, Gate::Approval, &req, now_unix()).is_ok());
        cleanup_rows(&project);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The boot sweep: what's waiting stays, an in-flight orphan stays until it's
    /// old, and a decided body goes after 30 days with its row kept.
    #[test]
    fn the_boot_sweep_keeps_what_waits_and_removes_the_rest() {
        let (store, dir) = temp_store();
        let project = format!("P-{}", uuid::Uuid::new_v4());
        let waiting = queued_with_file(&store, &project, b"waiting");
        let decided = queued_with_file(&store, &project, b"decided");
        deny(&store, &decided, "owner", "no", now_unix()).expect("deny");
        // A crash: files and a body with no row, and files left on a decided row.
        let orphan_dir = dir.join("out_orphan000000.files");
        std::fs::create_dir_all(&orphan_dir).unwrap();
        std::fs::write(orphan_dir.join("0"), b"x").unwrap();
        std::fs::write(dir.join("out_orphan000000.json"), b"{}").unwrap();
        let stale_dir = store.files_dir(&decided);
        std::fs::create_dir_all(&stale_dir).unwrap();
        std::fs::write(stale_dir.join("0"), b"x").unwrap();

        let now = now_unix();
        assert_eq!(sweep_store(&store, now), 1, "only the decided row's stale files, for now");
        assert!(orphan_dir.exists(), "a send in progress writes files first");
        assert!(store.files_dir(&waiting).exists());
        assert!(!stale_dir.exists());

        let later = now + DECIDED_BODY_KEEP_SECS + 60;
        // Nothing here touches `waiting`, however late.
        assert_eq!(sweep_store(&store, later), 3, "orphan files, orphan body, old decided body");
        assert!(!orphan_dir.exists() && !dir.join("out_orphan000000.json").exists());
        assert!(!store.body_path(&decided).exists());
        assert!(store.load(&decided).unwrap().is_some(), "the audit row stays");
        assert!(store.load_message_full(&waiting).is_ok(), "what's waiting is untouched");
        cleanup_rows(&project);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
