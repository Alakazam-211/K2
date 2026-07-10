//! S9 ops layer for EXTERNAL assistant inboxes (PRD §17.5 — the seam
//! V1 kept open, now filled): the user's OWN email accounts (Gmail
//! app-password, Fastmail, company IMAP), each bound to exactly ONE
//! workspace at add time. Agents in that workspace READ the inbox and
//! save reply DRAFTS into the account's real Drafts folder — the user
//! reviews and sends from their own mail client. **Sending from an
//! external account has NO code path in V1** (drafts only, by
//! construction: this module exposes read + APPEND-\Draft, nothing
//! else).
//!
//! §17.5 BINDING placement: markers, masked ownership, and never-log-
//! bodies stay at the route/ops layer exactly as for local Stalwart —
//! external content flows through the SAME [`shape_full_json`]/
//! [`wrap_untrusted`] path via the [`ReadBackend`] trait
//! ([`ExternalImapBackend`] here), so it inherits the §8.1 contract
//! unchanged. Nothing protocol-shaped leaks above this module.
//!
//! AUTH (V1): password / app-password ONLY. OAuth2 (`gmail-api`) and
//! JMAP are explicitly OUT of V1 — the schema `kind` CHECK anticipates
//! them and [`validate_new_inbox`] refuses them with a teaching error.
//! The password lives in the daemon vault under the DETERMINISTIC key
//! [`vault_key`] (`ext-inbox-<row-id>`) — never a DB column, never in
//! any response, never logged.
//!
//! Every IMAP effect goes through the [`ImapOps`] trait so this whole
//! module unit-tests with fakes; the production impl is
//! [`crate::mail::external_imap::RealImapOps`]. ⚠ LIVE-BOX functions
//! (genuinely-uncertain live-server behavior) live in `external_imap`
//! and are listed in ITS module header.

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;

use crate::mail::addresses::{self, AddrError};
use crate::mail::jmap::{AttachmentMeta, EmailFull, EmailSummary, MailAddr};
use crate::mail::messages::{ListFilter, ReadBackend, ReadError};
use crate::mail::secrets::SecretStore;
use k2_core::db::schema::MailExternalInbox;
use mail_parser::{MessageParser, MimeHeaders as _};

pub const TLS_IMPLICIT: &str = "implicit-tls";
pub const TLS_STARTTLS: &str = "starttls";
pub const KIND_IMAP: &str = "imap";

// ── Errors (mapped by routes onto the stable {code, hint} shapes) ───────

#[derive(Debug)]
pub enum ExtError {
    /// Bad input → 400 `usage`.
    Usage(String),
    /// Unknown inbox → 404 `not_found` (masked where a workspace asks).
    NotFound(String),
    /// Address collision → 409 `exists`.
    Exists(String),
    /// The IMAP server said no / transport failed → 502 `engine`.
    Engine(String),
}
// (`not_ready` 503s — vault credential missing — are emitted directly
// by the route layer, which owns the vault lookups; the ops layer
// never produces them.)

// ── Row access ──────────────────────────────────────────────────────────

const EXT_COLS: &str = "id, owner_project_id, email_address, display_name, kind, host, port, \
                        tls, username, drafts_folder, status, last_checked_at, last_error, \
                        created_at";

fn map_row(r: &rusqlite::Row) -> rusqlite::Result<MailExternalInbox> {
    Ok(MailExternalInbox {
        id: r.get(0)?,
        owner_project_id: r.get(1)?,
        email_address: r.get(2)?,
        display_name: r.get(3)?,
        kind: r.get(4)?,
        host: r.get(5)?,
        port: r.get(6)?,
        tls: r.get(7)?,
        username: r.get(8)?,
        drafts_folder: r.get(9)?,
        status: r.get(10)?,
        last_checked_at: r.get(11)?,
        last_error: r.get(12)?,
        created_at: r.get(13)?,
    })
}

/// The §17.5 seam lookup: is this (normalized) address an external
/// inbox? Used by [`crate::mail::messages::backend_for_address`] —
/// and nowhere else above the ops layer.
pub fn inbox_for_address(address: &str) -> Option<MailExternalInbox> {
    let normalized = addresses::normalize_address(address).ok()?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        &format!("SELECT {EXT_COLS} FROM mail_external_inboxes WHERE email_address = ?1"),
        rusqlite::params![normalized],
        map_row,
    )
    .ok()
}

/// Every external inbox bound to this workspace (the default read
/// scope joins these with the workspace's minted addresses).
pub fn inboxes_for_project(project_id: &str) -> Vec<MailExternalInbox> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let Ok(mut stmt) = conn.prepare(&format!(
        "SELECT {EXT_COLS} FROM mail_external_inboxes \
         WHERE owner_project_id = ?1 ORDER BY created_at, email_address"
    )) else {
        return Vec::new();
    };
    stmt.query_map(rusqlite::params![project_id], map_row)
        .map(|r| r.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

/// The MASKED workspace-ownership gate for an explicitly named
/// external address (mirrors [`crate::mail::messages::
/// owned_active_address`]): exists AND bound to the calling workspace,
/// or the same masked `not_found` a foreign minted address gets — a
/// workspace never learns which inboxes exist outside it.
pub fn owned_external_inbox(
    project_id: &str,
    raw_address: &str,
) -> Result<MailExternalInbox, ReadError> {
    let address = addresses::normalize_address(raw_address).map_err(|e| match e {
        AddrError::Usage(hint) => ReadError::Usage(hint),
        _ => ReadError::Usage(format!("'{raw_address}' is not a valid address")),
    })?;
    let masked = || ReadError::NotFound(format!("no address '{address}' in this workspace"));
    let row = inbox_for_address(&address).ok_or_else(masked)?;
    if row.owner_project_id != project_id {
        return Err(masked());
    }
    Ok(row)
}

// ── Opaque backend ids (UID space — collision-proof vs JMAP ids) ────────

/// Backend email id for the IMAP world: `uid:<uidvalidity>:<uid>`.
/// UIDs are only stable per UIDVALIDITY generation (RFC 3501 §2.3.1.1)
/// — the generation rides in the token so a rebuilt mailbox makes old
/// ids answer the masked not_found instead of the WRONG message. The
/// `uid:` prefix keeps the namespace disjoint from Stalwart JMAP ids
/// (which are opaque base32-ish strings, never colon-shaped).
pub fn encode_uid_token(uidvalidity: u32, uid: u32) -> String {
    format!("uid:{uidvalidity}:{uid}")
}

pub fn parse_uid_token(s: &str) -> Option<(u32, u32)> {
    let rest = s.strip_prefix("uid:")?;
    let (v, u) = rest.split_once(':')?;
    Some((v.parse().ok()?, u.parse().ok()?))
}

/// Attachment blob ids: `<uid-token>#<1-based part>`; the bare token
/// is the whole raw RFC 822 message (`read --raw`).
pub fn encode_blob_token(uid_token: &str, part_1based: usize) -> String {
    format!("{uid_token}#{part_1based}")
}

/// → `(uid_token, Some(part))` or `(uid_token, None)` for the raw
/// message. Malformed → None.
pub fn parse_blob_token(s: &str) -> Option<(String, Option<usize>)> {
    match s.split_once('#') {
        None => {
            parse_uid_token(s)?;
            Some((s.to_string(), None))
        }
        Some((tok, part)) => {
            parse_uid_token(tok)?;
            let n: usize = part.parse().ok()?;
            if n == 0 {
                return None; // parts are 1-based like everything §11.1.11
            }
            Some((tok.to_string(), Some(n)))
        }
    }
}

/// The deterministic vault key for a row's password: no ref column
/// exists in the DB — the row id IS the ref.
pub fn vault_key(row_id: &str) -> String {
    format!("ext-inbox-{row_id}")
}

// ── The IMAP effect seam (production: external_imap::RealImapOps) ───────

/// What the add-time connect check learned.
#[derive(Debug, Clone)]
pub struct ConnectCheck {
    /// The drafts folder that will be used: the configured override
    /// (verified to exist) or the autodetected one. `None` = nothing
    /// detected — add still succeeds (read works), but `draft` will
    /// fail with guidance until `drafts_folder` is set.
    pub drafts_folder: Option<String>,
}

/// One fetched message, raw.
#[derive(Debug, Clone)]
pub struct RawEmail {
    /// INTERNALDATE as RFC 3339 (the summary/`read` `date` field).
    pub received_at_iso: String,
    pub unread: bool,
    pub raw: Vec<u8>,
}

/// Every IMAP effect S9 performs, behind one trait (the house
/// engine-trait pattern) so ops + routes unit-test with fakes. All
/// `uid_token` params are [`encode_uid_token`] strings; `Ok(None)`
/// from fetch means unknown-UID OR changed UIDVALIDITY — the caller
/// masks it exactly like a foreign id.
pub trait ImapOps: Send + Sync {
    /// Login + folder survey (add-time validation and drafts-folder
    /// resolution). Never returns credentials in any form.
    fn check_connect(
        &self,
        inbox: &MailExternalInbox,
        password: &str,
    ) -> Result<ConnectCheck, String>;
    /// INBOX summaries, newest first, server-side filtered where the
    /// protocol allows (client-side re-filtering happens above).
    fn list_inbox(
        &self,
        inbox: &MailExternalInbox,
        password: &str,
        filter: &ListFilter,
        limit: usize,
    ) -> Result<Vec<EmailSummary>, String>;
    fn fetch_raw(
        &self,
        inbox: &MailExternalInbox,
        password: &str,
        uid_token: &str,
    ) -> Result<Option<RawEmail>, String>;
    fn mark_seen(
        &self,
        inbox: &MailExternalInbox,
        password: &str,
        uid_token: &str,
    ) -> Result<(), String>;
    /// APPEND `rfc822` to `folder` with `\Draft` set. The ONLY write
    /// S9 ever performs against an external account (besides \Seen).
    fn append_draft(
        &self,
        inbox: &MailExternalInbox,
        password: &str,
        folder: &str,
        rfc822: &[u8],
    ) -> Result<(), String>;
}

// ── ReadBackend adapter (the §17.5 seam's external variant) ─────────────

/// [`ReadBackend`] over an external IMAP inbox: the S4 routes drive
/// external content through the exact same shaping/marker/ownership
/// pipeline as local Stalwart. `account_id` (the ReadBackend "backend
/// mailbox handle") is the `mail_external_inboxes.id`.
pub struct ExternalImapBackend {
    inbox: MailExternalInbox,
    password: String,
    ops: std::sync::Arc<dyn ImapOps>,
}

impl ExternalImapBackend {
    pub fn new(
        inbox: MailExternalInbox,
        password: String,
        ops: std::sync::Arc<dyn ImapOps>,
    ) -> Self {
        Self { inbox, password, ops }
    }

    fn check_handle(&self, account_id: &str) -> Result<(), String> {
        if account_id == self.inbox.id {
            Ok(())
        } else {
            Err("external backend called with a foreign mailbox handle".to_string())
        }
    }
}

impl ReadBackend for ExternalImapBackend {
    fn list_inbox(
        &self,
        account_id: &str,
        filter: &ListFilter,
        limit: usize,
    ) -> Result<Vec<EmailSummary>, String> {
        self.check_handle(account_id)?;
        self.ops.list_inbox(&self.inbox, &self.password, filter, limit)
    }

    fn fetch_full(&self, account_id: &str, email_id: &str) -> Result<Option<EmailFull>, String> {
        self.check_handle(account_id)?;
        let Some(raw) = self.ops.fetch_raw(&self.inbox, &self.password, email_id)? else {
            return Ok(None);
        };
        full_from_raw(email_id, &raw).map(Some)
    }

    fn mark_seen(&self, account_id: &str, email_id: &str) -> Result<(), String> {
        self.check_handle(account_id)?;
        self.ops.mark_seen(&self.inbox, &self.password, email_id)
    }

    fn fetch_blob(
        &self,
        account_id: &str,
        blob_id: &str,
        _name: &str,
        _mime: &str,
    ) -> Result<Vec<u8>, String> {
        self.check_handle(account_id)?;
        let Some((uid_token, part)) = parse_blob_token(blob_id) else {
            return Err(format!("malformed external blob id '{blob_id}'"));
        };
        let Some(raw) = self.ops.fetch_raw(&self.inbox, &self.password, &uid_token)? else {
            return Err("the message is no longer on the server".to_string());
        };
        match part {
            None => Ok(raw.raw),
            Some(n) => attachment_bytes(&raw.raw, n),
        }
    }
}

// ── RFC 822 → EmailFull shaping (pure — fixture-tested) ─────────────────

fn addr_list(a: Option<&mail_parser::Address<'_>>) -> Vec<MailAddr> {
    a.map(|a| {
        a.iter()
            .filter_map(|x| {
                let email = x.address.as_deref()?.trim();
                if email.is_empty() {
                    return None;
                }
                Some(MailAddr {
                    name: x
                        .name
                        .as_deref()
                        .map(str::trim)
                        .filter(|n| !n.is_empty())
                        .map(str::to_string),
                    email: email.to_string(),
                })
            })
            .collect()
    })
    .unwrap_or_default()
}

/// Parse one fetched raw message into the S4 [`EmailFull`] shape —
/// the SAME struct the local backend produces, so
/// [`crate::mail::messages::shape_full_json`] (markers, HTML-strip
/// fallback, 1-based attachments, auth verdicts) applies untouched.
pub fn full_from_raw(uid_token: &str, raw: &RawEmail) -> Result<EmailFull, String> {
    let msg = MessageParser::default()
        .parse(raw.raw.as_slice())
        .ok_or_else(|| "unparseable message (not RFC 822)".to_string())?;
    let attachments: Vec<AttachmentMeta> = msg
        .attachments()
        .enumerate()
        .map(|(i, p)| AttachmentMeta {
            blob_id: encode_blob_token(uid_token, i + 1),
            filename: p.attachment_name().map(str::to_string),
            mime: p
                .content_type()
                .map(|ct| match ct.subtype() {
                    Some(sub) => format!("{}/{sub}", ct.ctype()),
                    None => ct.ctype().to_string(),
                })
                .unwrap_or_else(|| "application/octet-stream".to_string()),
            size: p.len() as u64,
        })
        .collect();
    let auth_results: Vec<String> = msg
        .header_values(mail_parser::HeaderName::Other("Authentication-Results".into()))
        .filter_map(|v| v.as_text())
        .map(str::to_string)
        .collect();
    let summary = EmailSummary {
        id: uid_token.to_string(),
        thread_id: None,
        from: addr_list(msg.from()),
        to: addr_list(msg.to()),
        subject: msg.subject().unwrap_or_default().to_string(),
        received_at: raw.received_at_iso.clone(),
        unread: raw.unread,
        has_attachment: !attachments.is_empty(),
    };
    Ok(EmailFull {
        summary,
        cc: addr_list(msg.cc()),
        // The raw message IS the blob (`read --raw`).
        blob_id: Some(uid_token.to_string()),
        text: msg.body_text(0).map(|t| t.into_owned()),
        html: msg.body_html(0).map(|t| t.into_owned()),
        attachments,
        auth_results,
    })
}

/// Bytes of one attachment (1-based, §11.1.11) out of a raw message.
pub fn attachment_bytes(raw: &[u8], part_1based: usize) -> Result<Vec<u8>, String> {
    let msg = MessageParser::default()
        .parse(raw)
        .ok_or_else(|| "unparseable message (not RFC 822)".to_string())?;
    let count = msg.attachment_count();
    let bytes = msg
        .attachments()
        .nth(part_1based - 1)
        .map(|p| p.contents().to_vec());
    bytes.ok_or_else(|| {
        format!("no attachment #{part_1based} — this message has {count} (1-based)")
    })
}

// ── Draft composition (pure — the ONLY thing K2 writes into Drafts) ─────

/// The reply context extracted from the source message.
#[derive(Debug, Clone, Default)]
pub struct DraftSource {
    /// Angle-bracketed, ready for In-Reply-To.
    pub message_id: Option<String>,
    /// Angle-bracketed chain, ready for References.
    pub references: Option<String>,
    pub subject: String,
    /// Reply-To first, else the first From — where the reply goes.
    pub reply_to: Option<MailAddr>,
}

fn bracketed(id: &str) -> String {
    let t = id.trim();
    if t.starts_with('<') {
        t.to_string()
    } else {
        format!("<{t}>")
    }
}

/// Extract the reply context from a raw source message (mail-parser
/// strips the angle brackets from ids; header composition re-adds
/// them, RFC 5322 §3.6.4).
pub fn draft_source_from_raw(raw: &[u8]) -> Result<DraftSource, String> {
    let msg = MessageParser::default()
        .parse(raw)
        .ok_or_else(|| "unparseable source message".to_string())?;
    let references = msg
        .references()
        .as_text_list()
        .map(|ids| ids.iter().map(|i| bracketed(i)).collect::<Vec<_>>().join(" "))
        .filter(|s| !s.is_empty());
    let reply_to = addr_list(msg.reply_to())
        .into_iter()
        .next()
        .or_else(|| addr_list(msg.from()).into_iter().next());
    Ok(DraftSource {
        message_id: msg.message_id().map(bracketed),
        references,
        subject: msg.subject().unwrap_or_default().to_string(),
        reply_to,
    })
}

/// RFC 2047 B-encoding for header text that isn't plain ASCII (one
/// encoded word — fine for draft-sized subjects/names).
fn encode_header_text(s: &str) -> String {
    let s = s.replace(['\r', '\n'], " ");
    if s.is_ascii() {
        return s;
    }
    format!("=?UTF-8?B?{}?=", B64.encode(s.as_bytes()))
}

/// `"Display Name" <addr>` with RFC 2047 for non-ASCII names; header-
/// injection-proof (CR/LF stripped by [`encode_header_text`]).
fn format_mailbox(name: Option<&str>, email: &str) -> String {
    match name.map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) if n.is_ascii() => {
            format!("\"{}\" <{email}>", n.replace(['\r', '\n'], " ").replace('"', "'"))
        }
        Some(n) => format!("{} <{email}>", encode_header_text(n)),
        None => format!("<{email}>"),
    }
}

/// Base64 body wrapped at 76 columns with CRLF (fully-ASCII output —
/// safe in any APPEND literal, no 8BITMIME dependence).
fn b64_body(text: &str) -> String {
    let encoded = B64.encode(text.as_bytes());
    encoded
        .as_bytes()
        .chunks(76)
        .map(|c| std::str::from_utf8(c).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\r\n")
}

/// Compose the reply draft: From = the external account (the USER —
/// drafts are theirs to send), To = the source's Reply-To/From,
/// threading headers per RFC 5322. No Message-ID — the user's mail
/// client assigns one at send time. `date_rfc2822` is injected (no
/// clock in a pure fn).
pub fn compose_draft_rfc822(
    inbox: &MailExternalInbox,
    src: &DraftSource,
    body: &str,
    date_rfc2822: &str,
) -> Result<Vec<u8>, String> {
    let Some(to) = src.reply_to.as_ref() else {
        return Err("the source message has no sender address to reply to".to_string());
    };
    let mut h = String::new();
    h.push_str(&format!("Date: {date_rfc2822}\r\n"));
    h.push_str(&format!(
        "From: {}\r\n",
        format_mailbox(inbox.display_name.as_deref(), &inbox.email_address)
    ));
    h.push_str(&format!("To: {}\r\n", format_mailbox(to.name.as_deref(), &to.email)));
    h.push_str(&format!(
        "Subject: {}\r\n",
        encode_header_text(&crate::mail::send::reply_subject(&src.subject))
    ));
    if let Some(irt) = src.message_id.as_deref() {
        h.push_str(&format!("In-Reply-To: {irt}\r\n"));
    }
    if let Some(refs) =
        crate::mail::send::build_out_references(src.references.as_deref(), src.message_id.as_deref())
    {
        h.push_str(&format!("References: {refs}\r\n"));
    }
    h.push_str("MIME-Version: 1.0\r\n");
    h.push_str("Content-Type: text/plain; charset=utf-8\r\n");
    h.push_str("Content-Transfer-Encoding: base64\r\n");
    h.push_str("\r\n");
    h.push_str(&b64_body(body));
    h.push_str("\r\n");
    Ok(h.into_bytes())
}

// ── Add / remove / list ops ─────────────────────────────────────────────

/// Validated add-time spec (everything but the password).
#[derive(Debug, Clone)]
pub struct NewExternalInbox {
    pub email_address: String,
    pub display_name: Option<String>,
    pub host: String,
    pub port: u16,
    pub tls: String,
    pub username: String,
    pub drafts_folder: Option<String>,
}

/// Validate + normalize the add-time inputs. `kind` other than `imap`
/// is refused with the V1 teaching line (OAuth2/JMAP are V2 — the
/// schema anticipates them, this function is the single V1 gate).
#[allow(clippy::too_many_arguments)]
pub fn validate_new_inbox(
    raw_address: &str,
    display_name: Option<&str>,
    kind: Option<&str>,
    host: &str,
    port: Option<i64>,
    tls: Option<&str>,
    username: Option<&str>,
    drafts_folder: Option<&str>,
) -> Result<NewExternalInbox, ExtError> {
    let email_address = addresses::normalize_address(raw_address).map_err(|e| match e {
        AddrError::Usage(hint) => ExtError::Usage(hint),
        _ => ExtError::Usage(format!("'{raw_address}' is not a valid address")),
    })?;
    match kind.map(str::trim).filter(|k| !k.is_empty()) {
        None => {}
        Some(k) if k == KIND_IMAP => {}
        Some(k @ ("jmap" | "gmail-api")) => {
            return Err(ExtError::Usage(format!(
                "kind '{k}' is not supported yet — V1 external inboxes are IMAP with a \
                 password/app-password (OAuth2 comes later)"
            )))
        }
        Some(k) => {
            return Err(ExtError::Usage(format!(
                "unknown kind '{k}' — V1 supports 'imap'"
            )))
        }
    }
    let tls = match tls.map(str::trim).filter(|t| !t.is_empty()) {
        None => TLS_IMPLICIT.to_string(),
        Some(t) if t == TLS_IMPLICIT || t == TLS_STARTTLS => t.to_string(),
        Some(t) => {
            return Err(ExtError::Usage(format!(
                "invalid tls '{t}' — 'implicit-tls' (port 993) or 'starttls' (port 143); \
                 plaintext IMAP is not a thing K2 will speak"
            )))
        }
    };
    let host = host.trim().to_ascii_lowercase();
    if host.is_empty()
        || host.contains(['/', ' ', '@'])
        || host.contains("://")
    {
        return Err(ExtError::Usage(format!(
            "invalid host '{host}' — a bare hostname like imap.gmail.com (no scheme, no path)"
        )));
    }
    let port = match port {
        None => {
            if tls == TLS_IMPLICIT {
                993
            } else {
                143
            }
        }
        Some(p) if (1..=65535).contains(&p) => p as u16,
        Some(p) => {
            return Err(ExtError::Usage(format!("invalid port {p} — 1-65535")));
        }
    };
    let username = username
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| email_address.clone());
    Ok(NewExternalInbox {
        email_address,
        display_name: display_name
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .map(str::to_string),
        host,
        port,
        tls,
        username,
        drafts_folder: drafts_folder
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .map(str::to_string),
    })
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// The owner add flow: uniqueness (vs BOTH tables), live connect-check
/// through the injected ops (login + drafts-folder survey), vault the
/// password under the deterministic key, insert the row bound to
/// `owner_project_id`. Compensation: an insert failure deletes the
/// just-vaulted secret (no orphaned credentials). The response JSON
/// carries NO credentials and NO secret refs — ever.
pub fn add_inbox(
    ops: &dyn ImapOps,
    secrets: &dyn SecretStore,
    owner_project_id: &str,
    spec: &NewExternalInbox,
    password: &str,
) -> Result<serde_json::Value, ExtError> {
    if password.is_empty() {
        return Err(ExtError::Usage(
            "missing password — pipe it via --pass-stdin or point --pass-ref at env:<VAR> / \
             an absolute file path"
                .to_string(),
        ));
    }
    // Collisions: one row per account; and a K2-MINTED address can
    // never double as an external inbox (the seam key must be
    // unambiguous).
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let minted: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM mail_addresses WHERE address = ?1",
                rusqlite::params![spec.email_address],
                |r| r.get(0),
            )
            .unwrap_or(false);
        if minted {
            return Err(ExtError::Exists(format!(
                "'{}' is an address on THIS K2 mail server — external inboxes are for accounts \
                 hosted elsewhere (agents already read it via 'k2 mail messages')",
                spec.email_address
            )));
        }
        let taken: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM mail_external_inboxes WHERE email_address = ?1",
                rusqlite::params![spec.email_address],
                |r| r.get(0),
            )
            .unwrap_or(false);
        if taken {
            return Err(ExtError::Exists(format!(
                "external inbox '{}' is already connected — remove it first with \
                 'k2 mail external remove {}'",
                spec.email_address, spec.email_address
            )));
        }
    }
    let id = uuid::Uuid::new_v4().to_string();
    let now = now_secs();
    let candidate = MailExternalInbox {
        id: id.clone(),
        owner_project_id: owner_project_id.to_string(),
        email_address: spec.email_address.clone(),
        display_name: spec.display_name.clone(),
        kind: KIND_IMAP.to_string(),
        host: spec.host.clone(),
        port: spec.port as i64,
        tls: spec.tls.clone(),
        username: spec.username.clone(),
        drafts_folder: spec.drafts_folder.clone(),
        status: "connected".to_string(),
        last_checked_at: Some(now),
        last_error: None,
        created_at: now,
    };
    // Live connect check BEFORE anything persists: bad credentials /
    // unreachable host fail the add outright (never a half-added row
    // the agent then trips over).
    let check = ops
        .check_connect(&candidate, password)
        .map_err(|e| ExtError::Engine(format!("could not connect to {}: {e}", spec.host)))?;
    secrets
        .store_exact(&vault_key(&id), password)
        .map_err(|e| ExtError::Engine(format!("vault write failed: {e}")))?;
    let inserted = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO mail_external_inboxes (id, owner_project_id, email_address, \
             display_name, kind, host, port, tls, username, drafts_folder, status, \
             last_checked_at, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'connected', ?11, ?11)",
            rusqlite::params![
                id,
                owner_project_id,
                candidate.email_address,
                candidate.display_name,
                candidate.kind,
                candidate.host,
                candidate.port,
                candidate.tls,
                candidate.username,
                candidate.drafts_folder,
                now,
            ],
        )
    };
    if let Err(e) = inserted {
        // Compensating delete — no orphaned credential outlives a
        // failed add.
        let _ = secrets.delete(&vault_key(&id));
        return Err(ExtError::Engine(format!("insert external inbox: {e}")));
    }
    k2_core::log_debug!(
        "[mail/external] connected inbox {} (host {}) bound to workspace {}",
        candidate.email_address,
        candidate.host,
        owner_project_id
    );
    Ok(serde_json::json!({
        "ok": true,
        "id": id,
        "address": candidate.email_address,
        "workspace": owner_project_id,
        "draftsFolder": candidate.drafts_folder.as_deref().or(check.drafts_folder.as_deref()),
        "hint": format!(
            "connected — agents in the bound workspace can read '{}' and save reply drafts; \
             sending from it is impossible (drafts only)",
            candidate.email_address
        ),
    }))
}

/// The owner remove flow: delete the row AND its vault entry. Owner
/// surface — no masking (the owner sees everything).
pub fn remove_inbox(
    secrets: &dyn SecretStore,
    raw_address: &str,
) -> Result<serde_json::Value, ExtError> {
    let address = addresses::normalize_address(raw_address).map_err(|e| match e {
        AddrError::Usage(hint) => ExtError::Usage(hint),
        _ => ExtError::Usage(format!("'{raw_address}' is not a valid address")),
    })?;
    let row = inbox_for_address(&address).ok_or_else(|| {
        ExtError::NotFound(format!(
            "no external inbox '{address}' — 'k2 mail external list' shows what's connected"
        ))
    })?;
    // Vault first: if the secret can't be purged the row must stay
    // visible (an invisible credential is the worse failure).
    secrets
        .delete(&vault_key(&row.id))
        .map_err(|e| ExtError::Engine(format!("vault delete failed: {e}")))?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "DELETE FROM mail_external_inboxes WHERE id = ?1",
        rusqlite::params![row.id],
    )
    .map_err(|e| ExtError::Engine(format!("delete external inbox: {e}")))?;
    Ok(serde_json::json!({
        "ok": true,
        "address": row.email_address,
        "removed": true,
    }))
}

/// The owner table: every external inbox with its holder workspace.
/// NEVER credentials, NEVER secret refs, NOT EVEN the username (the
/// email address identifies the account; login details stay between
/// the row and the vault).
pub fn list_all_json() -> serde_json::Value {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let rows: Vec<MailExternalInbox> = conn
        .prepare(&format!(
            "SELECT {EXT_COLS} FROM mail_external_inboxes ORDER BY created_at, email_address"
        ))
        .ok()
        .and_then(|mut stmt| {
            stmt.query_map([], map_row)
                .map(|r| r.filter_map(Result::ok).collect())
                .ok()
        })
        .unwrap_or_default();
    let list: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            let holder: Option<String> = conn
                .query_row(
                    "SELECT name FROM projects WHERE id = ?1",
                    rusqlite::params![r.owner_project_id],
                    |row| row.get(0),
                )
                .ok();
            serde_json::json!({
                "id": r.id,
                "address": r.email_address,
                "displayName": r.display_name,
                "kind": r.kind,
                "host": r.host,
                "port": r.port,
                "tls": r.tls,
                "draftsFolder": r.drafts_folder,
                "status": r.status,
                "lastCheckedAt": r.last_checked_at,
                "lastError": r.last_error,
                "holderProjectId": r.owner_project_id,
                "holderWorkspace": holder,
                "createdAt": r.created_at,
            })
        })
        .collect();
    serde_json::json!({ "ok": true, "count": list.len(), "inboxes": list })
}

/// Stamp the row's health after an IMAP interaction (add/draft). The
/// error text is transport/server-shaped — never a body, never a
/// credential.
pub fn record_check(row_id: &str, result: Result<(), &str>) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let now = now_secs();
    let _ = match result {
        Ok(()) => conn.execute(
            "UPDATE mail_external_inboxes SET status = 'connected', last_error = NULL, \
             last_checked_at = ?2 WHERE id = ?1",
            rusqlite::params![row_id, now],
        ),
        Err(e) => conn.execute(
            "UPDATE mail_external_inboxes SET status = 'error', last_error = ?2, \
             last_checked_at = ?3 WHERE id = ?1",
            rusqlite::params![row_id, e, now],
        ),
    };
}

/// The agent draft flow: fetch the source (must live in THIS inbox),
/// extract the reply context, compose, APPEND with `\Draft` into the
/// resolved Drafts folder. Returns the folder used. Health is stamped
/// on the row either way.
pub fn save_reply_draft(
    ops: &dyn ImapOps,
    inbox: &MailExternalInbox,
    password: &str,
    source_uid_token: &str,
    body: &str,
) -> Result<String, ExtError> {
    let src_raw = ops
        .fetch_raw(inbox, password, source_uid_token)
        .map_err(|e| {
            record_check(&inbox.id, Err(&e));
            ExtError::Engine(e)
        })?
        .ok_or_else(|| {
            // Unknown UID / stale UIDVALIDITY — same masked shape the
            // route uses for every not-yours/not-there case.
            ExtError::NotFound("the source message is no longer on the server".to_string())
        })?;
    let src = draft_source_from_raw(&src_raw.raw).map_err(ExtError::Engine)?;
    let folder = match inbox.drafts_folder.clone() {
        Some(f) => f,
        None => {
            let check = ops.check_connect(inbox, password).map_err(|e| {
                record_check(&inbox.id, Err(&e));
                ExtError::Engine(e)
            })?;
            check.drafts_folder.ok_or_else(|| {
                ExtError::Engine(format!(
                    "no Drafts folder found on {} — your human can pin one by re-adding with \
                     --drafts-folder",
                    inbox.host
                ))
            })?
        }
    };
    let date = chrono::Utc::now().to_rfc2822();
    let rfc822 = compose_draft_rfc822(inbox, &src, body, &date).map_err(ExtError::Engine)?;
    match ops.append_draft(inbox, password, &folder, &rfc822) {
        Ok(()) => {
            record_check(&inbox.id, Ok(()));
            k2_core::log_debug!(
                "[mail/external] draft appended to '{}' in {}",
                folder,
                inbox.email_address
            );
            Ok(folder)
        }
        Err(e) => {
            record_check(&inbox.id, Err(&e));
            Err(ExtError::Engine(format!("draft APPEND failed: {e}")))
        }
    }
}

/// Pure drafts-folder pick over a LIST survey: a `\Drafts` SPECIAL-USE
/// attribute wins; otherwise the common names, case-insensitively, in
/// specificity order. `folders` = `(name, has_drafts_special_use)`.
pub fn pick_drafts_folder(folders: &[(String, bool)]) -> Option<String> {
    if let Some((name, _)) = folders.iter().find(|(_, special)| *special) {
        return Some(name.clone());
    }
    // ⚠ The classic live-server variance is WHICH names exist (see
    // external_imap's LIVE-BOX list); the PICK order here is fixed +
    // unit-tested.
    const COMMON: [&str; 4] = ["Drafts", "[Gmail]/Drafts", "INBOX.Drafts", "Draft"];
    for want in COMMON {
        if let Some((name, _)) = folders
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(want))
        {
            return Some(name.clone());
        }
    }
    None
}

// ──────────────────────────────────────────────────────────────────────
// Inline unit tests — fakes + fixtures, no network (house rules).
// ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Mutex;

    // ── Fakes ──

    /// Recording IMAP fake: canned raw messages per uid token, canned
    /// folder survey, records appends. Never any network.
    #[derive(Default)]
    pub(crate) struct FakeOps {
        pub connect_err: Option<String>,
        pub folders: Vec<(String, bool)>,
        pub raw_by_token: std::collections::HashMap<String, RawEmail>,
        pub appended: Mutex<Vec<(String, Vec<u8>)>>,
        pub marked: Mutex<Vec<String>>,
    }

    impl ImapOps for FakeOps {
        fn check_connect(
            &self,
            _inbox: &MailExternalInbox,
            _password: &str,
        ) -> Result<ConnectCheck, String> {
            if let Some(e) = &self.connect_err {
                return Err(e.clone());
            }
            Ok(ConnectCheck { drafts_folder: pick_drafts_folder(&self.folders) })
        }
        fn list_inbox(
            &self,
            _inbox: &MailExternalInbox,
            _password: &str,
            _filter: &ListFilter,
            _limit: usize,
        ) -> Result<Vec<EmailSummary>, String> {
            let mut out = Vec::new();
            for (tok, raw) in &self.raw_by_token {
                out.push(full_from_raw(tok, raw)?.summary);
            }
            Ok(out)
        }
        fn fetch_raw(
            &self,
            _inbox: &MailExternalInbox,
            _password: &str,
            uid_token: &str,
        ) -> Result<Option<RawEmail>, String> {
            Ok(self.raw_by_token.get(uid_token).cloned())
        }
        fn mark_seen(
            &self,
            _inbox: &MailExternalInbox,
            _password: &str,
            uid_token: &str,
        ) -> Result<(), String> {
            self.marked.lock().unwrap().push(uid_token.to_string());
            Ok(())
        }
        fn append_draft(
            &self,
            _inbox: &MailExternalInbox,
            _password: &str,
            folder: &str,
            rfc822: &[u8],
        ) -> Result<(), String> {
            self.appended
                .lock()
                .unwrap()
                .push((folder.to_string(), rfc822.to_vec()));
            Ok(())
        }
    }

    /// In-memory exact-key vault.
    #[derive(Default)]
    pub(crate) struct FakeVault {
        pub map: Mutex<std::collections::HashMap<String, String>>,
        pub fail_store: bool,
    }

    impl SecretStore for FakeVault {
        fn store(&self, _kind: &str, _secret: &str) -> Result<String, String> {
            unreachable!("external inboxes use store_exact")
        }
        fn resolve(&self, sref: &str) -> Result<Option<String>, String> {
            Ok(self.map.lock().unwrap().get(sref).cloned())
        }
        fn delete(&self, sref: &str) -> Result<(), String> {
            self.map.lock().unwrap().remove(sref);
            Ok(())
        }
        fn store_exact(&self, key: &str, secret: &str) -> Result<(), String> {
            if self.fail_store {
                return Err("vault down".to_string());
            }
            self.map
                .lock()
                .unwrap()
                .insert(key.to_string(), secret.to_string());
            Ok(())
        }
    }

    pub(crate) fn test_inbox(id: &str, project: &str, address: &str) -> MailExternalInbox {
        MailExternalInbox {
            id: id.to_string(),
            owner_project_id: project.to_string(),
            email_address: address.to_string(),
            display_name: Some("Rosson".to_string()),
            kind: KIND_IMAP.to_string(),
            host: "imap.example.com".to_string(),
            port: 993,
            tls: TLS_IMPLICIT.to_string(),
            username: address.to_string(),
            drafts_folder: None,
            status: "connected".to_string(),
            last_checked_at: None,
            last_error: None,
            created_at: 100,
        }
    }

    fn seed_row(row: &MailExternalInbox) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO mail_external_inboxes (id, owner_project_id, email_address, \
             display_name, kind, host, port, tls, username, drafts_folder, status, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            rusqlite::params![
                row.id,
                row.owner_project_id,
                row.email_address,
                row.display_name,
                row.kind,
                row.host,
                row.port,
                row.tls,
                row.username,
                row.drafts_folder,
                row.status,
                row.created_at,
            ],
        )
        .expect("seed external inbox");
    }

    fn cleanup_row(id: &str) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = conn.execute(
            "DELETE FROM mail_external_inboxes WHERE id = ?1",
            rusqlite::params![id],
        );
    }

    pub(crate) const RAW_FIXTURE: &[u8] = b"Date: Wed, 08 Jul 2026 10:15:00 +0000\r\n\
Message-ID: <src-123@mailer.example>\r\n\
References: <thread-1@mailer.example>\r\n\
From: \"Pat Sender\" <pat@sender.example>\r\n\
Reply-To: replies@sender.example\r\n\
To: rosson@example.com\r\n\
Cc: cc@sender.example\r\n\
Subject: Quarterly numbers\r\n\
Authentication-Results: mx.example.com; spf=pass; dkim=pass; dmarc=pass\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"BB\"\r\n\
\r\n\
--BB\r\n\
Content-Type: text/plain; charset=utf-8\r\n\
\r\n\
Here are the numbers you asked for.\r\n\
--BB\r\n\
Content-Type: text/csv; name=\"q2.csv\"\r\n\
Content-Disposition: attachment; filename=\"q2.csv\"\r\n\
\r\n\
a,b\r\n1,2\r\n\
--BB--\r\n";

    fn raw_email() -> RawEmail {
        RawEmail {
            received_at_iso: "2026-07-08T10:15:00Z".to_string(),
            unread: true,
            raw: RAW_FIXTURE.to_vec(),
        }
    }

    // ── tokens ──

    #[test]
    fn uid_and_blob_tokens_round_trip_and_reject_garbage() {
        let t = encode_uid_token(77, 1234);
        assert_eq!(t, "uid:77:1234");
        assert_eq!(parse_uid_token(&t), Some((77, 1234)));
        for bad in ["", "uid:", "uid:1", "uid:x:1", "77:1234", "Mabc"] {
            assert_eq!(parse_uid_token(bad), None, "{bad}");
        }
        assert_eq!(
            parse_blob_token("uid:77:1234"),
            Some(("uid:77:1234".to_string(), None))
        );
        assert_eq!(
            parse_blob_token("uid:77:1234#2"),
            Some(("uid:77:1234".to_string(), Some(2)))
        );
        for bad in ["uid:77:1234#0", "uid:77:1234#x", "junk#1", "#1"] {
            assert_eq!(parse_blob_token(bad), None, "{bad}");
        }
        // The uid namespace can never collide with an m_-token's JMAP
        // id (those are never colon-triples) — belt: the prefix.
        assert!(encode_uid_token(1, 1).starts_with("uid:"));
    }

    // ── MIME shaping ──

    #[test]
    fn full_from_raw_parses_bodies_attachments_and_auth_results() {
        let full = full_from_raw("uid:77:9", &raw_email()).expect("parses");
        assert_eq!(full.summary.id, "uid:77:9");
        assert_eq!(full.summary.subject, "Quarterly numbers");
        assert_eq!(full.summary.from[0].email, "pat@sender.example");
        assert_eq!(full.summary.from[0].name.as_deref(), Some("Pat Sender"));
        assert_eq!(full.summary.received_at, "2026-07-08T10:15:00Z");
        assert!(full.summary.unread);
        assert!(full.summary.has_attachment);
        assert_eq!(full.cc[0].email, "cc@sender.example");
        assert!(full.text.as_deref().unwrap().contains("Here are the numbers"));
        assert_eq!(full.attachments.len(), 1);
        assert_eq!(full.attachments[0].filename.as_deref(), Some("q2.csv"));
        assert_eq!(full.attachments[0].mime, "text/csv");
        assert_eq!(full.attachments[0].blob_id, "uid:77:9#1");
        assert_eq!(full.blob_id.as_deref(), Some("uid:77:9"));
        assert_eq!(full.auth_results.len(), 1);
        assert!(full.auth_results[0].contains("spf=pass"), "{:?}", full.auth_results);
        // And the §8.1 shaping applies to it EXACTLY like local mail.
        let v = crate::mail::messages::shape_full_json("rosson@example.com", &full, false);
        let text = v["text"].as_str().unwrap();
        assert!(text.starts_with(crate::mail::messages::MARKER_BEGIN), "{text}");
        assert!(text.ends_with(crate::mail::messages::MARKER_END), "{text}");
        assert_eq!(v["attachments"][0]["index"], 1);

        // Attachment bytes extract 1-based; out of range is loud.
        let bytes = attachment_bytes(RAW_FIXTURE, 1).expect("attachment");
        assert_eq!(bytes, b"a,b\r\n1,2");
        let err = attachment_bytes(RAW_FIXTURE, 2).expect_err("no #2");
        assert!(err.contains("has 1"), "{err}");
        // Garbage bytes never panic: mail-parser is deliberately
        // LENIENT (real-world mail is malformed constantly), so the
        // worst case is an empty-enveloped message — still safely
        // marker-wrapped upstream, never a crash.
        let junk = full_from_raw("uid:1:1", &RawEmail {
            received_at_iso: "2026-01-01T00:00:00Z".into(),
            unread: false,
            raw: vec![0xFF, 0xFE, 0x00],
        });
        if let Ok(full) = junk {
            assert!(full.summary.from.is_empty(), "junk has no envelope");
        }
    }

    // ── draft composition ──

    #[test]
    fn draft_source_extracts_reply_context_with_brackets_restored() {
        let src = draft_source_from_raw(RAW_FIXTURE).expect("source");
        assert_eq!(src.message_id.as_deref(), Some("<src-123@mailer.example>"));
        assert_eq!(src.references.as_deref(), Some("<thread-1@mailer.example>"));
        assert_eq!(src.subject, "Quarterly numbers");
        // Reply-To wins over From.
        assert_eq!(src.reply_to.as_ref().unwrap().email, "replies@sender.example");
    }

    #[test]
    fn compose_draft_threads_correctly_and_never_contains_credentials() {
        let inbox = test_inbox("X1", "p1", "rosson@example.com");
        let src = draft_source_from_raw(RAW_FIXTURE).expect("source");
        let body = "Thanks — the café numbers look right. — R";
        let rfc822 =
            compose_draft_rfc822(&inbox, &src, body, "Thu, 09 Jul 2026 08:00:00 +0000")
                .expect("composes");
        let text = String::from_utf8(rfc822.clone()).expect("draft is pure ASCII on the wire");
        assert!(text.contains("From: \"Rosson\" <rosson@example.com>\r\n"), "{text}");
        assert!(text.contains("To: <replies@sender.example>\r\n"), "{text}");
        assert!(text.contains("Subject: Re: Quarterly numbers\r\n"), "{text}");
        assert!(text.contains("In-Reply-To: <src-123@mailer.example>\r\n"), "{text}");
        assert!(
            text.contains("References: <thread-1@mailer.example> <src-123@mailer.example>\r\n"),
            "{text}"
        );
        assert!(text.contains("Content-Transfer-Encoding: base64\r\n"), "{text}");
        // No Message-ID — the user's client assigns one at send time.
        assert!(!text.contains("\r\nMessage-ID:"), "{text}");
        // The body round-trips through the encoding (non-ASCII safe).
        let parsed = MessageParser::default().parse(rfc822.as_slice()).expect("parses back");
        assert_eq!(parsed.body_text(0).as_deref(), Some(body));

        // Header injection in a subject/display name cannot smuggle
        // extra HEADER LINES (the text survives, flattened onto the
        // Subject line — harmless data, not a header).
        let mut evil = src.clone();
        evil.subject = "hi\r\nBcc: attacker@evil.example".to_string();
        let rfc822 = compose_draft_rfc822(&inbox, &evil, body, "Thu, 09 Jul 2026 08:00:00 +0000")
            .expect("composes");
        let text = String::from_utf8(rfc822).unwrap();
        assert!(!text.contains("\r\nBcc:"), "no injected header line: {text}");
        assert!(text.contains("Subject: Re: hi  Bcc:"), "flattened onto one line: {text}");

        // No sender to reply to → loud error.
        let no_sender = DraftSource { reply_to: None, ..src };
        assert!(compose_draft_rfc822(&inbox, &no_sender, body, "d").is_err());
    }

    #[test]
    fn non_ascii_headers_are_rfc2047_encoded() {
        let mut inbox = test_inbox("X1", "p1", "rosson@example.com");
        inbox.display_name = Some("Røsson Ålind".to_string());
        let src = DraftSource {
            message_id: Some("<m@x>".to_string()),
            references: None,
            subject: "Überweisung".to_string(),
            reply_to: Some(MailAddr { name: None, email: "a@b.example".to_string() }),
        };
        let rfc822 = compose_draft_rfc822(&inbox, &src, "ok", "d").expect("composes");
        let text = String::from_utf8(rfc822).expect("ASCII wire form");
        assert!(text.contains("Subject: =?UTF-8?B?"), "{text}");
        assert!(text.contains("From: =?UTF-8?B?"), "{text}");
    }

    // ── drafts-folder pick ──

    #[test]
    fn drafts_folder_pick_prefers_special_use_then_common_names() {
        // SPECIAL-USE wins regardless of name.
        let f = vec![
            ("INBOX".to_string(), false),
            ("Entwürfe".to_string(), true),
            ("Drafts".to_string(), false),
        ];
        assert_eq!(pick_drafts_folder(&f), Some("Entwürfe".to_string()));
        // Fallback order: Drafts > [Gmail]/Drafts > INBOX.Drafts > Draft.
        let f = vec![
            ("[Gmail]/Drafts".to_string(), false),
            ("drafts".to_string(), false),
        ];
        assert_eq!(pick_drafts_folder(&f), Some("drafts".to_string()), "case-insensitive");
        let f = vec![("[Gmail]/Drafts".to_string(), false), ("INBOX.Drafts".to_string(), false)];
        assert_eq!(pick_drafts_folder(&f), Some("[Gmail]/Drafts".to_string()));
        assert_eq!(pick_drafts_folder(&[("INBOX".to_string(), false)]), None);
        assert_eq!(pick_drafts_folder(&[]), None);
    }

    // ── validation ──

    #[test]
    fn validate_new_inbox_normalizes_and_teaches() {
        let spec = validate_new_inbox(
            "  Rosson.AFS@Bücher.Example  ",
            Some(" Rosson "),
            None,
            " IMAP.Example.COM ",
            None,
            None,
            None,
            None,
        )
        .expect("valid");
        assert_eq!(spec.email_address, "rosson.afs@xn--bcher-kva.example", "punycode domain");
        assert_eq!(spec.host, "imap.example.com");
        assert_eq!(spec.port, 993, "implicit-tls default port");
        assert_eq!(spec.tls, TLS_IMPLICIT);
        assert_eq!(spec.username, spec.email_address, "username defaults to the address");
        assert_eq!(spec.display_name.as_deref(), Some("Rosson"));

        // starttls default port.
        let spec = validate_new_inbox(
            "a@b.example", None, Some("imap"), "h.example", None, Some("starttls"), None, None,
        )
        .expect("valid");
        assert_eq!((spec.port, spec.tls.as_str()), (143, TLS_STARTTLS));

        // V2 kinds teach, junk errors.
        for (kind, needle) in [("gmail-api", "OAuth2"), ("jmap", "OAuth2"), ("pop3", "unknown")] {
            let err = validate_new_inbox(
                "a@b.example", None, Some(kind), "h", None, None, None, None,
            )
            .expect_err(kind);
            let ExtError::Usage(hint) = err else { panic!("usage, got {err:?}") };
            assert!(hint.contains(needle), "kind={kind}: {hint}");
        }
        // Plaintext is not a spelling that exists.
        assert!(matches!(
            validate_new_inbox("a@b.example", None, None, "h", None, Some("none"), None, None),
            Err(ExtError::Usage(_))
        ));
        // Host shapes.
        for bad_host in ["", "imap://x.example", "x.example/inbox", "user@host"] {
            assert!(
                matches!(
                    validate_new_inbox("a@b.example", None, None, bad_host, None, None, None, None),
                    Err(ExtError::Usage(_))
                ),
                "host '{bad_host}' must be refused"
            );
        }
        // Port bounds.
        for bad_port in [0i64, 65536, -1] {
            assert!(matches!(
                validate_new_inbox("a@b.example", None, None, "h", Some(bad_port), None, None, None),
                Err(ExtError::Usage(_))
            ));
        }
        // Not-an-address.
        assert!(matches!(
            validate_new_inbox("nope", None, None, "h", None, None, None, None),
            Err(ExtError::Usage(_))
        ));
    }

    // ── add / remove / ownership (shared test DB) ──

    fn unique_project() -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, format!("ext-{id}"), format!("/tmp/ext-{id}")],
        )
        .expect("project row");
        id
    }

    fn cleanup_project(project_id: &str) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = conn.execute(
            "DELETE FROM mail_external_inboxes WHERE owner_project_id = ?1",
            rusqlite::params![project_id],
        );
        let _ = conn.execute("DELETE FROM projects WHERE id = ?1", rusqlite::params![project_id]);
    }

    fn unique_addr(label: &str) -> String {
        format!(
            "{label}-{}@ext-test.example",
            &uuid::Uuid::new_v4().simple().to_string()[..12]
        )
    }

    #[test]
    fn add_inbox_connect_checks_vaults_and_binds_the_workspace() {
        let project = unique_project();
        let addr = unique_addr("add");
        let ops = FakeOps { folders: vec![("Drafts".to_string(), true)], ..Default::default() };
        let vault = FakeVault::default();
        let spec = validate_new_inbox(&addr, Some("Rosson"), None, "imap.example.com", None, None, None, None)
            .expect("spec");
        let v = add_inbox(&ops, &vault, &project, &spec, "app-password").expect("adds");
        assert_eq!(v["ok"], true);
        assert_eq!(v["address"], addr);
        assert_eq!(v["draftsFolder"], "Drafts");
        let id = v["id"].as_str().unwrap().to_string();
        // The secret landed under the deterministic key…
        assert_eq!(
            vault.map.lock().unwrap().get(&vault_key(&id)).map(String::as_str),
            Some("app-password")
        );
        // …and NOTHING credential-shaped rides the response.
        let s = v.to_string();
        assert!(!s.contains("app-password") && !s.contains("username"), "{s}");

        // The row is the seam key now.
        let row = inbox_for_address(&addr).expect("row");
        assert_eq!(row.owner_project_id, project);
        assert_eq!(row.status, "connected");

        // Duplicate add → exists.
        let err = add_inbox(&ops, &vault, &project, &spec, "pw").expect_err("dup");
        assert!(matches!(err, ExtError::Exists(_)), "{err:?}");

        // Ownership: bound workspace sees it, any other gets the
        // MASKED not_found.
        assert!(owned_external_inbox(&project, &addr).is_ok());
        let err = owned_external_inbox("some-other-project", &addr).expect_err("masked");
        let ReadError::NotFound(hint) = err else { panic!("masked, got {err:?}") };
        assert!(hint.contains("no address"), "{hint}");

        // The owner list carries the binding but never login details.
        let all = list_all_json();
        let mine = all["inboxes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["address"] == addr.as_str())
            .expect("listed");
        assert_eq!(mine["holderProjectId"], project);
        let s = all.to_string();
        assert!(!s.contains("app-password") && !s.contains("secretRef") && !s.contains("username"), "{s}");

        // Remove deletes row + vault entry.
        let v = remove_inbox(&vault, &addr).expect("removes");
        assert_eq!(v["removed"], true);
        assert!(inbox_for_address(&addr).is_none());
        assert!(vault.map.lock().unwrap().is_empty(), "vault entry purged");
        // Removing again → not_found with the list pointer.
        assert!(matches!(remove_inbox(&vault, &addr), Err(ExtError::NotFound(_))));

        cleanup_project(&project);
    }

    #[test]
    fn add_inbox_fails_closed_on_connect_error_empty_password_and_minted_collision() {
        let project = unique_project();
        let addr = unique_addr("fail");
        let vault = FakeVault::default();
        let spec =
            validate_new_inbox(&addr, None, None, "imap.example.com", None, None, None, None)
                .expect("spec");

        // Empty password is a usage error before anything dials.
        let ops = FakeOps::default();
        assert!(matches!(
            add_inbox(&ops, &vault, &project, &spec, ""),
            Err(ExtError::Usage(_))
        ));

        // Connect failure → engine error, NOTHING persisted.
        let ops = FakeOps { connect_err: Some("LOGIN failed".to_string()), ..Default::default() };
        let err = add_inbox(&ops, &vault, &project, &spec, "pw").expect_err("engine");
        let ExtError::Engine(hint) = err else { panic!("engine") };
        assert!(hint.contains("could not connect"), "{hint}");
        assert!(inbox_for_address(&addr).is_none(), "no row on failed add");
        assert!(vault.map.lock().unwrap().is_empty(), "no secret on failed add");

        // Vault failure after a good connect also leaves nothing.
        let ops = FakeOps::default();
        let vault = FakeVault { fail_store: true, ..Default::default() };
        assert!(matches!(
            add_inbox(&ops, &vault, &project, &spec, "pw"),
            Err(ExtError::Engine(_))
        ));
        assert!(inbox_for_address(&addr).is_none());

        // A K2-minted address can never double as an external inbox.
        let minted = unique_addr("minted");
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO mail_addresses (id, address, domain_id, owner_project_id, created_at) \
                 VALUES (?1, ?2, 'dom-x', ?3, 100)",
                rusqlite::params![uuid::Uuid::new_v4().to_string(), minted, project],
            )
            .expect("seed minted");
        }
        let spec = validate_new_inbox(&minted, None, None, "h.example", None, None, None, None)
            .expect("spec");
        let vault = FakeVault::default();
        let err = add_inbox(&FakeOps::default(), &vault, &project, &spec, "pw").expect_err("exists");
        let ExtError::Exists(hint) = err else { panic!("exists") };
        assert!(hint.contains("THIS K2 mail server"), "{hint}");
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = conn.execute(
                "DELETE FROM mail_addresses WHERE owner_project_id = ?1",
                rusqlite::params![project],
            );
        }
        cleanup_project(&project);
    }

    // ── the draft flow ──

    #[test]
    fn save_reply_draft_appends_to_the_resolved_folder_and_stamps_health() {
        let project = unique_project();
        let addr = unique_addr("draft");
        let mut row = test_inbox(&uuid::Uuid::new_v4().to_string(), &project, &addr);
        row.drafts_folder = None;
        seed_row(&row);

        let mut ops = FakeOps { folders: vec![("[Gmail]/Drafts".to_string(), true)], ..Default::default() };
        ops.raw_by_token.insert("uid:7:42".to_string(), raw_email());

        let folder = save_reply_draft(&ops, &row, "pw", "uid:7:42", "On it — draft reply.")
            .expect("draft saved");
        assert_eq!(folder, "[Gmail]/Drafts", "autodetected via SPECIAL-USE");
        let appended = ops.appended.lock().unwrap();
        assert_eq!(appended.len(), 1);
        assert_eq!(appended[0].0, "[Gmail]/Drafts");
        let text = String::from_utf8(appended[0].1.clone()).unwrap();
        assert!(text.contains("In-Reply-To: <src-123@mailer.example>"), "{text}");
        assert!(text.contains(&format!("From: \"Rosson\" <{addr}>")), "{text}");
        drop(appended);

        // Health stamped connected.
        let fresh = inbox_for_address(&addr).expect("row");
        assert_eq!(fresh.status, "connected");
        assert!(fresh.last_checked_at.is_some());

        // The configured override skips detection entirely.
        let mut row2 = row.clone();
        row2.drafts_folder = Some("Custom/Drafts".to_string());
        let folder = save_reply_draft(&ops, &row2, "pw", "uid:7:42", "x").expect("draft");
        assert_eq!(folder, "Custom/Drafts");

        // A vanished source message answers NotFound (masked upstream).
        assert!(matches!(
            save_reply_draft(&ops, &row, "pw", "uid:7:9999", "x"),
            Err(ExtError::NotFound(_))
        ));

        // No drafts folder anywhere → engine error with guidance, and
        // the row goes unhealthy on real transport errors.
        let ops_nofolder = FakeOps {
            folders: vec![("INBOX".to_string(), false)],
            raw_by_token: ops.raw_by_token.clone(),
            ..Default::default()
        };
        let err = save_reply_draft(&ops_nofolder, &row, "pw", "uid:7:42", "x").expect_err("no folder");
        let ExtError::Engine(hint) = err else { panic!("engine") };
        assert!(hint.contains("--drafts-folder"), "{hint}");

        cleanup_row(&row.id);
        cleanup_project(&project);
    }

    // ── the ReadBackend adapter ──

    #[test]
    fn external_backend_serves_reads_through_the_s4_contract() {
        let row = test_inbox("XB1", "pX", "rosson@example.com");
        let mut ops = FakeOps::default();
        ops.raw_by_token.insert("uid:7:42".to_string(), raw_email());
        let backend = ExternalImapBackend::new(row, "pw".to_string(), std::sync::Arc::new(ops));

        // Wrong handle is refused loudly (defense in depth — the
        // routes always pass the row id).
        assert!(backend.fetch_full("someone-else", "uid:7:42").is_err());

        let full = backend.fetch_full("XB1", "uid:7:42").expect("ok").expect("found");
        assert_eq!(full.summary.subject, "Quarterly numbers");
        // Unknown uid → Ok(None) → the route's masked not_found.
        assert!(backend.fetch_full("XB1", "uid:7:9999").expect("ok").is_none());
        backend.mark_seen("XB1", "uid:7:42").expect("marks");
        // Blob paths: whole raw + 1-based part.
        assert_eq!(backend.fetch_blob("XB1", "uid:7:42", "m.eml", "message/rfc822").unwrap(), RAW_FIXTURE.to_vec());
        assert_eq!(backend.fetch_blob("XB1", "uid:7:42#1", "q2.csv", "text/csv").unwrap(), b"a,b\r\n1,2".to_vec());
        assert!(backend.fetch_blob("XB1", "garbage", "x", "y").is_err());
    }
}
