//! Drafted replies K2 keeps track of (Rosson, 2026-11-10): an agent's draft
//! reply shows at the top of its thread in the mail reader, plainly unsent,
//! and a person edits it, sends it or discards it.
//!
//! **The draft is a real draft.** It lives in the mailbox's Drafts folder
//! (hosted: a JMAP Email with `$draft`; linked IMAP: an APPEND with `\Draft`),
//! threaded onto the message it answers (`In-Reply-To`, `References`, the
//! `Re:` subject, the original sender as the recipient). The person's own mail
//! client sees it too.
//!
//! **K2's record holds no mail.** `mail_drafts` (0292) says where the draft is,
//! which message it answers, who wrote it, who last edited it and what became
//! of it. No text, no recipient lists. Bodies are never logged.
//!
//! **Who.** Writing a draft is the `k2 mail draft` verb (the inbox's draft
//! level). Opening, saving, sending and discarding one in the reader follow
//! the reader's rule (`routes_reader`), are logged like every reader action,
//! and Send runs the workspace's own `k2 mail send` pipeline (send gate,
//! outbox, approvals).
//!
//! **The agent hears.** When a person sends, edits or discards an agent's
//! draft, the agent gets one line in its live session (never a wake: nothing
//! wakes an agent about mail unless someone chose that), and `k2 mail drafts`
//! always shows the state.

use crate::cli_response::CliResponse;
use crate::mail::access::{AccessInbox, Source};

/// A draft K2 tracks (one `mail_drafts` row).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftRow {
    pub id: String,
    pub source: Source,
    pub inbox_id: String,
    pub address: String,
    pub project_id: String,
    /// The `m_…` id of the message the draft answers.
    pub reply_of: String,
    pub thread_ref: Option<String>,
    pub in_reply_to: Option<String>,
    pub refs: Option<String>,
    /// The draft's backend email id; `None` = the backend didn't say where
    /// it saved it (shown, not editable).
    pub backend_ref: Option<String>,
    /// `agent` | `person`.
    pub author_kind: String,
    /// The workspace id (agent) or the actor (`user:<login>`, `owner-token`).
    pub author: String,
    pub author_label: String,
    pub created_at: i64,
    pub edited_at: i64,
    pub edited_by: Option<String>,
    /// `open` | `sent` | `held` | `discarded`.
    pub state: String,
    pub state_at: Option<i64>,
    pub state_by: Option<String>,
    pub outbound_id: Option<String>,
}

pub const STATE_OPEN: &str = "open";
pub const STATE_SENT: &str = "sent";
pub const STATE_HELD: &str = "held";
pub const STATE_DISCARDED: &str = "discarded";

pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// A fresh draft id: `d_` + 32 hex.
pub fn new_id() -> String {
    format!("d_{}", uuid::Uuid::new_v4().simple())
}

/// Is `s` shaped like a draft id?
pub fn is_draft_id(s: &str) -> bool {
    s.len() == 34 && s.starts_with("d_") && s[2..].bytes().all(|b| b.is_ascii_hexdigit())
}

/// Who is writing a draft right now: the person on this request (a login or
/// the owner token, while a mailbox guard is active), else the workspace's
/// agent.
pub struct Author {
    pub kind: &'static str,
    pub key: String,
    pub label: String,
}

/// The name a person shows as: their login, or "Owner" for the owner token.
pub fn actor_label(actor: &str) -> String {
    if actor == "owner-token" {
        return "Owner".to_string();
    }
    actor.strip_prefix("user:").unwrap_or(actor).to_string()
}

pub fn author_now(project_id: &str, project_path: &str) -> Author {
    match crate::mail::mailbox_guard::viewer_info() {
        Some(v) => Author { kind: "person", label: actor_label(&v.actor), key: v.actor },
        None => Author {
            kind: "agent",
            key: project_id.to_string(),
            label: k2_core::workspace::display::agent_display_name(project_path),
        },
    }
}

/// The person on this request, as a label (for `edited_by` / `state_by`).
pub fn viewer_label() -> Option<String> {
    crate::mail::mailbox_guard::viewer_info().map(|v| actor_label(&v.actor))
}

// ── The store ───────────────────────────────────────────────────────────

const COLUMNS: &str = "id, inbox_source, inbox_id, address, project_id, reply_of, thread_ref, in_reply_to, refs, \
     backend_ref, author_kind, author, author_label, created_at, edited_at, edited_by, state, state_at, state_by, \
     outbound_id";

fn row_from(r: &rusqlite::Row<'_>) -> rusqlite::Result<DraftRow> {
    let source: String = r.get(1)?;
    Ok(DraftRow {
        id: r.get(0)?,
        source: if source == "linked" { Source::Linked } else { Source::Hosted },
        inbox_id: r.get(2)?,
        address: r.get(3)?,
        project_id: r.get(4)?,
        reply_of: r.get(5)?,
        thread_ref: r.get(6)?,
        in_reply_to: r.get(7)?,
        refs: r.get(8)?,
        backend_ref: r.get(9)?,
        author_kind: r.get(10)?,
        author: r.get(11)?,
        author_label: r.get(12)?,
        created_at: r.get(13)?,
        edited_at: r.get(14)?,
        edited_by: r.get(15)?,
        state: r.get(16)?,
        state_at: r.get(17)?,
        state_by: r.get(18)?,
        outbound_id: r.get(19)?,
    })
}

pub fn insert(row: &DraftRow) -> rusqlite::Result<()> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        &format!("INSERT INTO mail_drafts ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)"),
        rusqlite::params![
            row.id,
            row.source.as_str(),
            row.inbox_id,
            row.address,
            row.project_id,
            row.reply_of,
            row.thread_ref,
            row.in_reply_to,
            row.refs,
            row.backend_ref,
            row.author_kind,
            row.author,
            row.author_label,
            row.created_at,
            row.edited_at,
            row.edited_by,
            row.state,
            row.state_at,
            row.state_by,
            row.outbound_id,
        ],
    )?;
    Ok(())
}

fn query(sql_tail: &str, args: &[rusqlite::types::Value]) -> Vec<DraftRow> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let Ok(mut stmt) = conn.prepare(&format!("SELECT {COLUMNS} FROM mail_drafts {sql_tail}")) else {
        return Vec::new();
    };
    let rows = stmt.query_map(rusqlite::params_from_iter(args.iter()), row_from);
    match rows {
        Ok(rows) => rows.filter_map(Result::ok).collect(),
        Err(_) => Vec::new(),
    }
}

pub fn load(id: &str) -> Option<DraftRow> {
    query("WHERE id = ?1", &[id.to_string().into()]).into_iter().next()
}

/// The open drafts in `address` that answer one of `reply_tokens`, or sit in
/// the backend thread `thread_ref`. Newest edit first.
pub fn open_in_thread(address: &str, reply_tokens: &[String], thread_ref: Option<&str>) -> Vec<DraftRow> {
    let mut rows = query(
        "WHERE lower(address) = ?1 AND state = 'open' ORDER BY edited_at DESC, created_at DESC",
        &[address.trim().to_ascii_lowercase().into()],
    );
    rows.retain(|r| {
        reply_tokens.iter().any(|t| t == &r.reply_of)
            || thread_ref.is_some_and(|t| r.thread_ref.as_deref() == Some(t))
    });
    rows
}

/// Every draft K2 tracks in `address`, open ones first, then the newest.
pub fn list_for_address(address: &str, limit: usize) -> Vec<DraftRow> {
    query(
        "WHERE lower(address) = ?1 ORDER BY (state = 'open') DESC, edited_at DESC LIMIT ?2",
        &[address.trim().to_ascii_lowercase().into(), (limit as i64).into()],
    )
}

/// The drafts this workspace's agent wrote, newest first.
pub fn list_for_agent(project_id: &str, limit: usize) -> Vec<DraftRow> {
    query(
        "WHERE author_kind = 'agent' AND author = ?1 ORDER BY created_at DESC LIMIT ?2",
        &[project_id.to_string().into(), (limit as i64).into()],
    )
}

fn exec(sql: &str, args: &[rusqlite::types::Value]) -> rusqlite::Result<usize> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(sql, rusqlite::params_from_iter(args.iter()))
}

/// A save: the draft's new place in the mailbox and who saved it.
pub fn set_edited(id: &str, backend_ref: Option<&str>, by: Option<&str>, now: i64) -> rusqlite::Result<usize> {
    exec(
        "UPDATE mail_drafts SET backend_ref = ?2, edited_at = ?3, edited_by = COALESCE(?4, edited_by) WHERE id = ?1",
        &[
            id.to_string().into(),
            backend_ref.map(String::from).into(),
            now.into(),
            by.map(String::from).into(),
        ],
    )
}

/// The end of a draft: sent, held for approval, or discarded.
pub fn set_state(id: &str, state: &str, by: Option<&str>, outbound_id: Option<&str>, now: i64) -> rusqlite::Result<usize> {
    exec(
        "UPDATE mail_drafts SET state = ?2, state_at = ?3, state_by = ?4, outbound_id = ?5, \
         backend_ref = CASE WHEN ?2 = 'open' THEN backend_ref ELSE NULL END WHERE id = ?1",
        &[
            id.to_string().into(),
            state.to_string().into(),
            now.into(),
            by.map(String::from).into(),
            outbound_id.map(String::from).into(),
        ],
    )
}

fn set_agent_told(id: &str, now: i64) {
    if let Err(e) = exec("UPDATE mail_drafts SET agent_told_at = ?2 WHERE id = ?1", &[id.to_string().into(), now.into()]) {
        k2_core::log_debug!("[mail/drafts] agent_told_at for {id}: {e}");
    }
}

/// What a draft is, in one word: `waiting` (nobody has touched it), `edited`
/// (a person saved it), `sent`, `held` (waiting for approval), `discarded`.
pub fn status_wire(row: &DraftRow) -> &'static str {
    match row.state.as_str() {
        STATE_SENT => "sent",
        STATE_HELD => "held",
        STATE_DISCARDED => "discarded",
        _ if row.edited_by.is_some() => "edited",
        _ => "waiting",
    }
}

/// What the reader says about a draft only Outlook can change.
pub const OUTLOOK_ONLY_HINT: &str = "This draft can only be edited in Outlook.";

/// Is this a draft in a Microsoft 365 inbox linked over Microsoft Graph?
/// K2 shows those but never changes them (Graph drafts are left to their
/// mailbox). Keyed off the backend that serves the address, not the
/// provider label: Outlook over IMAP is labelled microsoft too, and K2 can
/// edit that one.
pub fn outlook_only(row: &DraftRow) -> bool {
    row.source == Source::Linked
        && crate::mail::messages::backend_for_address(&row.address) == crate::mail::messages::MailBackend::Graph
}

/// May the reader change this draft: open, located in its mailbox, and
/// not one only Outlook can change.
pub fn editable(row: &DraftRow) -> bool {
    row.state == STATE_OPEN && row.backend_ref.is_some() && !outlook_only(row)
}

/// A draft's record as JSON (no mail content).
pub fn meta_json(row: &DraftRow) -> serde_json::Value {
    let mut v = meta_json_fields(row);
    if outlook_only(row) {
        v["hint"] = serde_json::json!(OUTLOOK_ONLY_HINT);
    }
    v
}

fn meta_json_fields(row: &DraftRow) -> serde_json::Value {
    serde_json::json!({
        "id": row.id,
        "inbox": row.address,
        "replyOf": row.reply_of,
        "status": status_wire(row),
        "author": { "kind": row.author_kind, "name": row.author_label },
        "createdAt": row.created_at,
        "editedAt": row.edited_at,
        "editedBy": row.edited_by,
        "stateAt": row.state_at,
        "stateBy": row.state_by,
        "outboundId": row.outbound_id,
        "editable": editable(row),
    })
}

// ── Composing ───────────────────────────────────────────────────────────

/// Pure: the RFC 8621 Email create object for a draft (the JMAP layer adds
/// `mailboxIds` and the `$draft` keyword). Threading headers only when set.
#[allow(clippy::too_many_arguments)]
pub fn draft_email_json(
    from_name: Option<&str>,
    from: &str,
    to: &[String],
    cc: &[String],
    subject: &str,
    body: &str,
    in_reply_to: Option<&str>,
    references: Option<&str>,
) -> serde_json::Value {
    let list = |l: &[String]| -> Vec<serde_json::Value> { l.iter().map(|e| serde_json::json!({ "email": e })).collect() };
    let mut v = serde_json::json!({
        "from": [{ "name": from_name, "email": from }],
        "to": list(to),
        "subject": subject,
        "bodyValues": { "k2body": { "value": body } },
        "textBody": [{ "partId": "k2body", "type": "text/plain" }],
    });
    if !cc.is_empty() {
        v["cc"] = serde_json::Value::Array(list(cc));
    }
    if let Some(irt) = in_reply_to.filter(|s| !s.trim().is_empty()) {
        v["header:In-Reply-To:asText"] = serde_json::json!(irt);
    }
    if let Some(refs) = references.filter(|s| !s.trim().is_empty()) {
        v["header:References:asText"] = serde_json::json!(refs);
    }
    v
}

fn error_response(status: &'static str, code: &str, hint: &str) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: serde_json::json!({ "ok": false, "error": { "code": code, "hint": hint } }).to_string(),
    }
}

/// The answer a new reply draft gives (hosted and linked alike).
pub fn saved_answer(address: &str, folder: &str, source_token: &str, draft_id: Option<&str>) -> serde_json::Value {
    let mut v = serde_json::json!({
        "ok": true,
        "address": address,
        "folder": folder,
        "id": source_token,
        "hint": format!(
            "draft saved to '{folder}' in {address} — it shows at the top of its thread in K2's Mail page, \
             where a person can edit and send it; 'k2 mail drafts' shows what became of it"
        ),
    });
    if let Some(d) = draft_id {
        v["draftId"] = serde_json::json!(d);
    }
    v
}

/// Record a new reply draft. A failed write after the draft is already in
/// the mailbox says so plainly (the draft exists; K2 just can't show it).
#[allow(clippy::too_many_arguments)]
pub fn record_new(
    source: Source,
    inbox_id: &str,
    address: &str,
    project_id: &str,
    project_path: &str,
    reply_of: &str,
    thread_ref: Option<String>,
    in_reply_to: Option<String>,
    refs: Option<String>,
    backend_ref: Option<String>,
) -> Result<String, CliResponse> {
    let author = author_now(project_id, project_path);
    let now = now_secs();
    let row = DraftRow {
        id: new_id(),
        source,
        inbox_id: inbox_id.to_string(),
        address: address.to_string(),
        project_id: project_id.to_string(),
        reply_of: reply_of.to_string(),
        thread_ref,
        in_reply_to,
        refs,
        backend_ref,
        author_kind: author.kind.to_string(),
        author: author.key,
        author_label: author.label,
        created_at: now,
        edited_at: now,
        edited_by: None,
        state: STATE_OPEN.to_string(),
        state_at: None,
        state_by: None,
        outbound_id: None,
    };
    insert(&row).map_err(|e| {
        error_response(
            "503 Service Unavailable",
            "not_ready",
            &format!(
                "the draft was saved to the Drafts folder of {address}, but K2 couldn't record it ({e}), so it \
                 won't show at the top of its thread; 'k2 doctor' checks the daemon's database"
            ),
        )
    })?;
    Ok(row.id)
}

/// `k2 mail draft <id>` on a message in a HOSTED inbox: a real draft in the
/// account's Drafts mailbox, threaded onto the message, addressed back to its
/// sender. `inbox` already passed the draft gate.
pub fn save_hosted_reply(
    project_id: &str,
    project_path: &str,
    inbox: &AccessInbox,
    email_id: &str,
    source_token: &str,
    body: &str,
) -> CliResponse {
    let Some(account) = inbox.account_id.clone().filter(|a| !a.is_empty()) else {
        return error_response(
            "502 Bad Gateway",
            "engine",
            &format!("'{}' has no mailbox on the mail server yet", inbox.address),
        );
    };
    let (client, _host) = match crate::mail::domains::engine_from_db() {
        Ok(c) => c,
        Err(hint) => return error_response("503 Service Unavailable", "not_ready", &hint),
    };
    let ctx = match client.email_get_reply_context(&account, email_id) {
        Ok(Some(c)) => c,
        Ok(None) => {
            return error_response("404 Not Found", "not_found", &format!("no message '{source_token}' in this workspace"))
        }
        Err(hint) => return error_response("502 Bad Gateway", "engine", &hint),
    };
    let Some(to) = ctx.from.first().map(|a| a.email.clone()).filter(|e| !e.trim().is_empty()) else {
        return error_response("400 Bad Request", "usage", "the message has no sender address to reply to");
    };
    let refs = crate::mail::send::build_out_references(ctx.references.as_deref(), ctx.message_id.as_deref());
    let subject = crate::mail::send::reply_subject(&ctx.subject);
    let from_name = k2_core::workspace::display::agent_display_name(project_path);
    let email = draft_email_json(
        Some(&from_name),
        &inbox.address,
        &[to],
        &[],
        &subject,
        body,
        ctx.message_id.as_deref(),
        refs.as_deref(),
    );
    let backend_ref = match client.email_create_draft(&account, email) {
        Ok(id) => id,
        Err(hint) => return error_response("502 Bad Gateway", "engine", &hint),
    };
    let inbox_id = hosted_inbox_id(&inbox.address).unwrap_or_default();
    match record_new(
        Source::Hosted,
        &inbox_id,
        &inbox.address,
        project_id,
        project_path,
        source_token,
        ctx.thread_id.clone(),
        ctx.message_id.clone(),
        refs,
        Some(backend_ref),
    ) {
        Ok(id) => CliResponse::ok_json(saved_answer(&inbox.address, "Drafts", source_token, Some(&id)).to_string()),
        Err(resp) => resp,
    }
}

/// The `mail_addresses.id` of an active hosted address.
fn hosted_inbox_id(address: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT id FROM mail_addresses WHERE lower(address) = lower(?1) AND status = 'active'",
        rusqlite::params![address],
        |r| r.get(0),
    )
    .ok()
}

// ── Replacing and removing a draft in its mailbox ───────────────────────

/// What a draft says, as the person edits it.
#[derive(Debug, Clone)]
pub struct DraftValues {
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub subject: String,
    pub body: String,
}

/// Save `values` as the draft's new copy, then remove the old one. Answers
/// the new copy's backend id (`None` = saved where the server wouldn't say).
pub fn replace_in_mailbox(
    row: &DraftRow,
    account: &str,
    from_name: &str,
    values: &DraftValues,
) -> Result<Option<String>, CliResponse> {
    let Some(old) = row.backend_ref.as_deref() else {
        return Err(not_editable(row));
    };
    match row.source {
        Source::Hosted => {
            let (client, _host) = crate::mail::domains::engine_from_db()
                .map_err(|h| error_response("503 Service Unavailable", "not_ready", &h))?;
            let email = draft_email_json(
                Some(from_name),
                &row.address,
                &values.to,
                &values.cc,
                &values.subject,
                &values.body,
                row.in_reply_to.as_deref(),
                row.refs.as_deref(),
            );
            let new_ref = client.email_create_draft(account, email).map_err(|h| error_response("502 Bad Gateway", "engine", &h))?;
            if let Err(e) = client.email_destroy(account, &[old.to_string()]) {
                k2_core::log_debug!("[mail/drafts] the old copy of {} stayed in Drafts: {e}", row.id);
            }
            Ok(Some(new_ref))
        }
        Source::Linked => {
            let inbox = crate::mail::external::inbox_for_address(&row.address)
                .ok_or_else(|| error_response("404 Not Found", "not_found", "the linked inbox is no longer connected"))?;
            if crate::mail::messages::backend_for_address(&row.address) == crate::mail::messages::MailBackend::Graph {
                return Err(not_editable(row));
            }
            let password = crate::mail::routes_send::linked_password(&inbox)?;
            let new_ref = crate::mail::external::save_edited_draft(
                &crate::mail::external_imap::RealImapOps,
                &inbox,
                &password,
                &values.to,
                &values.cc,
                &values.subject,
                &values.body,
                row.in_reply_to.as_deref(),
                row.refs.as_deref(),
            )
            .map_err(|e| error_response("502 Bad Gateway", "engine", &e))?;
            trash_linked(row, old);
            Ok(new_ref)
        }
    }
}

/// Take the draft out of its mailbox: destroyed (hosted) or moved to Trash
/// (linked IMAP: K2 never expunges). Graph drafts are left to their mailbox.
pub fn remove_from_mailbox(row: &DraftRow, account: &str) -> Result<(), CliResponse> {
    let Some(old) = row.backend_ref.as_deref() else {
        return Ok(());
    };
    match row.source {
        Source::Hosted => {
            let (client, _host) = crate::mail::domains::engine_from_db()
                .map_err(|h| error_response("503 Service Unavailable", "not_ready", &h))?;
            client
                .email_destroy(account, &[old.to_string()])
                .map(|_| ())
                .map_err(|h| error_response("502 Bad Gateway", "engine", &h))
        }
        Source::Linked => {
            let backend = crate::mail::routes_messages::manage_backend_for(&row.address)?;
            backend
                .trash_message(account, old)
                .map(|_| ())
                .map_err(|e| error_response("502 Bad Gateway", "engine", &format!("{e:?}")))
        }
    }
}

fn trash_linked(row: &DraftRow, uid_token: &str) {
    match crate::mail::routes_messages::manage_backend_for(&row.address) {
        Ok(b) => {
            let account = row.inbox_id.clone();
            if let Err(e) = b.trash_message(&account, uid_token) {
                k2_core::log_debug!("[mail/drafts] the old copy of {} stayed in Drafts: {e:?}", row.id);
            }
        }
        Err(_) => k2_core::log_debug!("[mail/drafts] no manage backend for the old copy of {}", row.id),
    }
}

pub fn not_editable(row: &DraftRow) -> CliResponse {
    if outlook_only(row) {
        return error_response("409 Conflict", "not_editable", OUTLOOK_ONLY_HINT);
    }
    error_response(
        "409 Conflict",
        "not_editable",
        &format!(
            "K2 can show this draft but can't change it: {}'s mail server didn't say where it saved it. \
             Edit or send it from that mailbox's own mail app",
            row.address
        ),
    )
}

// ── GET /cli/mail/drafts (`k2 mail drafts`, the agent verb) ─────────────

/// The calling workspace's own drafted replies, newest first, with what
/// became of each (`waiting`, `edited`, `sent`, `held`, `discarded`).
/// Records only: no mail text, no recipients. A login asking for a
/// workspace sees only the drafts in inboxes it may open there (OA0).
pub fn handle_agent_drafts(params: &std::collections::HashMap<String, String>) -> CliResponse {
    let (_path, project_id) = match crate::mail::identity::resolve_caller_params(params) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let limit = match params.get("limit").map(|v| v.trim().parse::<usize>()) {
        None => 50,
        Some(Ok(n)) if n >= 1 => n.min(200),
        _ => return error_response("400 Bad Request", "usage", "invalid 'limit' — a number from 1 to 200 (default 50)"),
    };
    let mut rows = Vec::new();
    for row in list_for_agent(&project_id, limit) {
        let allowed = crate::mail::mailbox_guard::check_with(
            Some(project_id.as_str()),
            row.source,
            &row.inbox_id,
            &row.address,
            crate::mail::mailbox_guard::Rule::Workspace,
            false,
        );
        if crate::mail::mailbox_guard::audit_failed() {
            return crate::mail::mailbox_guard::audit_unavailable();
        }
        if allowed {
            rows.push(meta_json(&row));
        }
    }
    CliResponse::ok_json(serde_json::json!({ "ok": true, "count": rows.len(), "drafts": rows }).to_string())
}

// ── Telling the agent ───────────────────────────────────────────────────

/// Tell the drafting agent what a person did with its draft: one line in
/// its live session (no wake). `what` is `sent`, `edited`, `discarded` or
/// `held`. The line carries no mail-derived text.
pub fn tell_agent(row: &DraftRow, what: &str, by: &str) {
    if row.author_kind != "agent" {
        return;
    }
    let did = match what {
        "held" => "queued for approval".to_string(),
        other => other.to_string(),
    };
    let text = format!(
        "Your draft reply {} in {} was {did} by {by}. 'k2 mail drafts' shows your drafts and what became of them.",
        row.id, row.address
    );
    if deliver(&row.author, &text) {
        set_agent_told(&row.id, now_secs());
    }
}

#[cfg(not(test))]
fn deliver(project_id: &str, text: &str) -> bool {
    let path: Option<String> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row("SELECT path FROM projects WHERE id = ?1", rusqlite::params![project_id], |r| r.get(0)).ok()
    };
    let Some(path) = path else { return false };
    let r = crate::workspace_msg::deliver_live_with_via(
        &path,
        text,
        "mail",
        "",
        false,
        crate::workspace_msg::DEFAULT_WAKE_TIMEOUT,
        "mail",
    );
    r.success
}

/// Tests: the lines agents were told, `(project id, text)`.
#[cfg(test)]
pub(crate) static TOLD: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());

#[cfg(test)]
fn deliver(project_id: &str, text: &str) -> bool {
    TOLD.lock().unwrap_or_else(|p| p.into_inner()).push((project_id.to_string(), text.to_string()));
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draft_ids_are_d_and_32_hex() {
        let id = new_id();
        assert!(is_draft_id(&id), "{id}");
        assert!(!is_draft_id("d_xyz"));
        assert!(!is_draft_id("m_00000000000000000000000000000000"));
    }

    #[test]
    fn a_draft_email_threads_only_when_told() {
        let v = draft_email_json(Some("Bot"), "bot@x.example", &["ada@example.com".into()], &[], "Re: hi", "b", Some("<a@x>"), Some("<r@x> <a@x>"));
        assert_eq!(v["header:In-Reply-To:asText"], "<a@x>");
        assert_eq!(v["header:References:asText"], "<r@x> <a@x>");
        assert!(v.get("cc").is_none());
        let bare = draft_email_json(None, "bot@x.example", &["ada@example.com".into()], &["bo@example.com".into()], "s", "b", None, Some(" "));
        assert!(bare.get("header:In-Reply-To:asText").is_none());
        assert!(bare.get("header:References:asText").is_none());
        assert_eq!(bare["cc"], serde_json::json!([{ "email": "bo@example.com" }]));
    }

    #[test]
    fn status_words() {
        let mut r = DraftRow {
            id: new_id(),
            source: Source::Hosted,
            inbox_id: "i".into(),
            address: "a@x".into(),
            project_id: "p".into(),
            reply_of: "m_1".into(),
            thread_ref: None,
            in_reply_to: None,
            refs: None,
            backend_ref: Some("e1".into()),
            author_kind: "agent".into(),
            author: "p".into(),
            author_label: "bot".into(),
            created_at: 1,
            edited_at: 1,
            edited_by: None,
            state: STATE_OPEN.into(),
            state_at: None,
            state_by: None,
            outbound_id: None,
        };
        assert_eq!(status_wire(&r), "waiting");
        r.edited_by = Some("ada".into());
        assert_eq!(status_wire(&r), "edited");
        for (s, w) in [(STATE_SENT, "sent"), (STATE_HELD, "held"), (STATE_DISCARDED, "discarded")] {
            r.state = s.into();
            assert_eq!(status_wire(&r), w);
        }
        assert_eq!(actor_label("owner-token"), "Owner");
        assert_eq!(actor_label("user:ada"), "ada");
    }
}
