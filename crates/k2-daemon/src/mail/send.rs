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
     agent session) or Settings → Workspaces / Agents → <workspace> → Permissions → Email sending";

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
             Settings → Workspaces / Agents → <workspace> → Permissions → Email sending"
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
        let dir = k2_core::paths::k2_home()
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
    use rusqlite::OptionalExtension as _;
    sweep_store_with(store, now, &|id| {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT status, updated_at FROM mail_outbound WHERE id = ?1",
            rusqlite::params![id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
    })
}

/// A row's status and `updated_at`. `Ok(None)`: no such row. An error says
/// nothing about the row, so what belongs to it stays.
type RowState = rusqlite::Result<Option<(String, i64)>>;

fn sweep_store_with(store: &DbOutboundStore, now: i64, status_of: &dyn Fn(&str) -> RowState) -> usize {
    let Ok(rd) = std::fs::read_dir(&store.body_dir) else { return 0 };
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
                Err(e) => {
                    k2_core::log_debug!("[mail/send] tidy: leaving {name}, its row can't be read: {e}");
                    false
                }
                Ok(None) => old_enough(&entry),
                Ok(Some((status, _))) => !matches!(status.as_str(), "pending" | "scheduled" | "approved"),
            };
            if gone && std::fs::remove_dir_all(entry.path()).is_ok() {
                n += 1;
            }
        } else if let Some(id) = name.strip_suffix(".json") {
            match status_of(id) {
                Err(e) => k2_core::log_debug!("[mail/send] tidy: leaving {name}, its row can't be read: {e}"),
                Ok(None) if old_enough(&entry) => {
                    if std::fs::remove_file(entry.path()).is_ok() {
                        n += 1;
                    }
                }
                Ok(Some((status, updated_at)))
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
        "pending" => "queued for approval — your human decides in Settings → Cores → Email Hosting → Approvals",
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
             human can change its send mode in Settings → Cores → Email Hosting → Domains → Sending"
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
            "domain '{domain}' is in relay mode but {what} — fix it in Settings → Cores → Email Hosting → Domains → Sending"
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
             domain '{domain}' cannot send until its relay is switched in Settings → Cores → Email Hosting → Domains → Sending"
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
#[path = "send_tests.rs"]
mod tests;
