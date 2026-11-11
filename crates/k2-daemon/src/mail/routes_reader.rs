//! `/cli/mail/reader/*` — the mail reader: a person's tool for opening the
//! inboxes on this server, hosted and linked (prd-ocmail-admin-in-k2-v1,
//! slices OA1, OA2, OA3 and OA9).
//!
//! **Who.** The Owner and Admins open every inbox on the server: agents'
//! and people's, hosted and linked (Rosson, 2026-11-10: no extra switch).
//! Anyone else opens only their own person mailbox: a hosted address whose
//! `person_principal` is them, or a linked inbox they linked
//! (`linked_by_principal`, 0288). Agents are refused: an agent passport gets
//! 403 `owner_only` before anything else (the dispatcher arm and the cell
//! socket answer it; this module is never in `cli::dispatch`). A Member
//! naming an inbox that isn't theirs gets the same masked `not_found` an
//! unknown address gets.
//!
//! **Logged, always.** Every call runs inside [`mailbox_guard::run`] with the
//! viewer, and every inbox it reaches is decided by
//! [`mailbox_guard::check_with`] under [`Rule::Reader`], which writes the
//! `mail_reader_audit` row (0284) BEFORE the engine is dialled. No row, no
//! access (503 `audit_unavailable`). The viewer's `reason`, when given,
//! rides in the row. Nobody is notified (Rosson, 2026-11-10).
//!
//! **Peek only.** Opening a message never marks it read: every read here is
//! `fetch_full` (JMAP `Email/get`, IMAP `BODY.PEEK[]`, Graph GET), never
//! `mark_seen` or `fetch_full_and_mark_seen`. The only way the reader changes
//! read state is the explicit `flag` action.
//!
//! **Person-shaped answers.** No BEGIN/END agent markers (those are for
//! agents). HTML is cleaned by [`k2_core::mail_html`] (no script, no remote
//! content, links kept only as inert data) before it leaves; the renderer
//! cleans it again and draws it in an inert frame. No credential, username,
//! host, account id or server URL is in any answer. Bodies are never logged.
//!
//! **Writes (OA9).** move, flag, archive, delete, reply, draft and send run
//! the same `k2 mail` handler an agent's call runs, as the inbox's own
//! workspace: the inbox's level, its manage/delete switches, the workspace's
//! send gate (on / approval / off) and the outbox all apply unchanged. The
//! reader adds no send path of its own; it decides who may ask (the reader
//! rule above) and logs the ask like a read.
//!
//! **Drafted replies** (Rosson, 2026-11-10). A thread answers its unsent
//! drafts first (`draft: true`, who drafted it, when it was last edited), the
//! drafts K2 tracks in `mail::drafts` (an agent's `k2 mail draft <id>`, or a
//! person's reply draft saved here). `draft` opens one (logged), `drafts`
//! lists an inbox's, and `draft-save`, `draft-send` and `draft-discard` act
//! on one under the same reader rule and log: Save replaces the draft in the
//! mailbox, Send runs the workspace's own `k2 mail send` (gate, outbox,
//! approvals) with the reply's thread headers, then takes the draft out of
//! Drafts; Discard takes it out. The drafting agent is told.

use std::collections::HashMap;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;

use crate::cli_response::CliResponse;
use crate::mail::access::Source;
use crate::mail::drafts;
use crate::mail::jmap::{EmailFull, EmailSummary, MailAddr};
use crate::mail::mailbox_guard::{self, Rule};
use crate::mail::messages::{self, MailBackend, MailFolder};

/// Read routes (GET).
pub const READ_ROUTES: &[&str] = &[
    "/cli/mail/reader/attachment",
    "/cli/mail/reader/draft-open",
    "/cli/mail/reader/drafts",
    "/cli/mail/reader/folders",
    "/cli/mail/reader/inboxes",
    "/cli/mail/reader/log",
    "/cli/mail/reader/message",
    "/cli/mail/reader/messages",
    "/cli/mail/reader/presence",
    "/cli/mail/reader/thread",
];

/// Write routes (POST, JSON body), each backed by an existing `k2 mail`
/// handler (OA9).
pub const WRITE_ROUTES: &[&str] = &[
    "/cli/mail/reader/archive",
    "/cli/mail/reader/delete",
    "/cli/mail/reader/draft",
    "/cli/mail/reader/draft-discard",
    "/cli/mail/reader/draft-save",
    "/cli/mail/reader/draft-send",
    "/cli/mail/reader/flag",
    "/cli/mail/reader/move",
    "/cli/mail/reader/reply",
    "/cli/mail/reader/send",
];

const DEFAULT_PAGE: usize = 50;
const MAX_PAGE: usize = 100;
/// Attachments stream to the viewer as a download, up to the compose cap.
const MAX_ATTACHMENT_BYTES: u64 = 25 * 1024 * 1024;
/// A thread is gathered from this many of the folder's newest messages…
const THREAD_SCAN: usize = 100;
/// …and shows at most this many of them.
const THREAD_MAX: usize = 25;
const LOG_DEFAULT: usize = 100;
const LOG_MAX: usize = 500;

/// The refusal an agent gets on every reader route.
pub const AGENT_HINT: &str = "the mail reader is a person's tool; agents use 'k2 mail' \
     (messages, read, move, reply, send) on the inboxes their workspace can use";

/// Is `path` a mail reader route (any method)?
pub fn is_reader_path(path: &str) -> bool {
    path.starts_with("/cli/mail/reader/")
}

/// 403 `owner_only` for an agent passport (TCP and the cell socket).
pub fn agent_refusal() -> CliResponse {
    error_response("403 Forbidden", "owner_only", AGENT_HINT)
}

fn error_response(status: &'static str, code: &str, hint: &str) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: serde_json::json!({ "ok": false, "error": { "code": code, "hint": hint } }).to_string(),
    }
}

fn usage(hint: &str) -> CliResponse {
    error_response("400 Bad Request", "usage", hint)
}

fn ok_json(v: serde_json::Value) -> CliResponse {
    CliResponse::ok_json(v.to_string())
}

fn method_not_allowed() -> CliResponse {
    CliResponse::method_not_allowed()
}

/// The masked answer for an inbox: the same bytes whether it doesn't exist
/// or isn't the viewer's to open.
fn inbox_not_found(address: &str) -> CliResponse {
    error_response(
        "404 Not Found",
        "not_found",
        &format!("no inbox '{address}' you can open — 'k2 mail reader inboxes' lists the ones you can"),
    )
}

/// The masked answer for a message id.
fn message_not_found(token: &str) -> CliResponse {
    error_response(
        "404 Not Found",
        "not_found",
        &format!("no message '{token}' you can open — ids come from 'k2 mail reader messages'"),
    )
}

/// Run one reader call. The caller has installed the viewer
/// ([`mailbox_guard::run`] with `Some(viewer)`); with no viewer (never
/// expected) the call is refused like an agent's.
pub fn dispatch(
    path: &str,
    is_post: bool,
    params: &HashMap<String, String>,
    body: &[u8],
) -> CliResponse {
    if mailbox_guard::viewer_info().is_none() {
        return agent_refusal();
    }
    if READ_ROUTES.contains(&path) {
        if is_post {
            return method_not_allowed();
        }
        mailbox_guard::set_reason(params.get("reason").map(String::as_str));
        return match path {
            "/cli/mail/reader/inboxes" => handle_inboxes(),
            "/cli/mail/reader/folders" => handle_folders(params),
            "/cli/mail/reader/messages" => handle_messages(params),
            "/cli/mail/reader/message" => handle_message(params),
            "/cli/mail/reader/thread" => handle_thread(params),
            "/cli/mail/reader/attachment" => handle_attachment(params),
            "/cli/mail/reader/draft-open" => handle_draft_open(params),
            "/cli/mail/reader/drafts" => handle_drafts(params),
            "/cli/mail/reader/log" => handle_log(params),
            crate::mail::reader_presence::ROUTE => crate::mail::reader_presence::handle(),
            _ => CliResponse::not_found(),
        };
    }
    if WRITE_ROUTES.contains(&path) {
        // Mutating: POST only (the 0.44.4 rule; the dispatcher's top guard
        // answers a GET first, this keeps the handler safe on its own).
        if !is_post {
            return method_not_allowed();
        }
        let verb = path.trim_start_matches("/cli/mail/reader/");
        if matches!(verb, "draft-save" | "draft-send" | "draft-discard") {
            return handle_draft_action(verb, body);
        }
        return handle_write(verb, body);
    }
    CliResponse::not_found()
}

// ── The inbox catalog the reader sees ───────────────────────────────────

/// One inbox on this server, as the reader knows it. Never carries a
/// credential, username, host or server URL out of this module.
#[derive(Debug, Clone)]
pub(super) struct ReaderInbox {
    source: Source,
    inbox_id: String,
    address: String,
    display_name: Option<String>,
    owner_project_id: String,
    workspace: Option<String>,
    workspace_path: Option<String>,
    /// hosted | gmail | microsoft | imap
    provider: &'static str,
    person_kind: bool,
    person: Option<String>,
    status: String,
    /// The backend's handle: the Stalwart account (hosted) or the row id
    /// (linked).
    account_id: Option<String>,
    primary_level: String,
    can_manage: bool,
    can_delete: bool,
}

impl ReaderInbox {
    pub(super) fn own(&self, viewer_person: Option<&str>) -> bool {
        self.person_kind
            && self.person.is_some()
            && viewer_person.is_some_and(|me| Some(me) == self.person.as_deref())
    }

    fn to_json(&self, viewer_person: Option<&str>) -> serde_json::Value {
        serde_json::json!({
            "address": self.address,
            "displayName": self.display_name,
            "source": self.source.as_str(),
            "provider": self.provider,
            "kind": if self.person_kind { "person" } else { "agent" },
            "own": self.own(viewer_person),
            "workspace": self.workspace,
            "person": self.person,
            "status": self.status,
        })
    }
}

fn workspace_label(name: Option<String>, path: Option<&str>) -> Option<String> {
    name.filter(|n| !n.trim().is_empty())
        .or_else(|| path.and_then(|p| p.rsplit('/').find(|s| !s.is_empty())).map(String::from))
}

fn linked_provider(kind: &str, provider: Option<&str>, host: &str) -> &'static str {
    let host = host.to_ascii_lowercase();
    match (kind, provider) {
        ("graph", _) | (_, Some("microsoft")) => "microsoft",
        (_, Some("gmail")) => "gmail",
        _ if host.ends_with("gmail.com") || host.ends_with("googlemail.com") => "gmail",
        _ if host.contains("outlook.") || host.contains("office365.") => "microsoft",
        _ => "imap",
    }
}

/// Every inbox on the server: active hosted addresses and every linked
/// inbox, ordered by workspace then address.
pub(super) fn catalog() -> Vec<ReaderInbox> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut out: Vec<ReaderInbox> = Vec::new();
    if let Ok(mut stmt) = conn.prepare(
        "SELECT a.id, a.address, a.owner_project_id, a.status, a.stalwart_account_id, \
                a.person_mailbox, a.person_principal, a.primary_level, a.primary_can_manage, \
                a.primary_can_delete, p.name, p.path \
         FROM mail_addresses a LEFT JOIN projects p ON p.id = a.owner_project_id \
         WHERE a.status = 'active'",
    ) {
        let rows = stmt.query_map([], |r| {
            let person: Option<String> = r.get(6)?;
            let path: Option<String> = r.get(11)?;
            Ok(ReaderInbox {
                source: Source::Hosted,
                inbox_id: r.get(0)?,
                address: r.get(1)?,
                display_name: None,
                owner_project_id: r.get(2)?,
                status: r.get(3)?,
                account_id: r.get(4)?,
                person_kind: r.get::<_, i64>(5)? != 0 || person.is_some(),
                person,
                primary_level: r.get(7)?,
                can_manage: r.get::<_, i64>(8)? != 0,
                can_delete: r.get::<_, i64>(9)? != 0,
                workspace: workspace_label(r.get(10)?, path.as_deref()),
                workspace_path: path,
                provider: "hosted",
            })
        });
        if let Ok(rows) = rows {
            out.extend(rows.filter_map(Result::ok));
        }
    }
    if let Ok(mut stmt) = conn.prepare(
        "SELECT e.id, e.email_address, e.owner_project_id, e.status, e.display_name, e.kind, \
                e.provider, e.host, e.linked_by_principal, e.primary_level, e.primary_can_manage, \
                e.primary_can_delete, p.name, p.path \
         FROM mail_external_inboxes e LEFT JOIN projects p ON p.id = e.owner_project_id",
    ) {
        let rows = stmt.query_map([], |r| {
            let id: String = r.get(0)?;
            let kind: String = r.get(5)?;
            let provider: Option<String> = r.get(6)?;
            let host: String = r.get(7)?;
            let path: Option<String> = r.get(13)?;
            Ok(ReaderInbox {
                source: Source::Linked,
                account_id: Some(id.clone()),
                inbox_id: id,
                address: r.get(1)?,
                owner_project_id: r.get(2)?,
                status: r.get(3)?,
                display_name: r.get(4)?,
                provider: linked_provider(&kind, provider.as_deref(), &host),
                // A linked inbox is always someone's own account.
                person_kind: true,
                person: r.get(8)?,
                primary_level: r.get(9)?,
                can_manage: r.get::<_, i64>(10)? != 0,
                can_delete: r.get::<_, i64>(11)? != 0,
                workspace: workspace_label(r.get(12)?, path.as_deref()),
                workspace_path: path,
            })
        });
        if let Ok(rows) = rows {
            out.extend(rows.filter_map(Result::ok));
        }
    }
    out.sort_by(|a, b| {
        (a.workspace.as_deref().unwrap_or(""), a.address.as_str())
            .cmp(&(b.workspace.as_deref().unwrap_or(""), b.address.as_str()))
    });
    out
}

/// The inbox an address names: a linked inbox first (the read seam's own
/// precedence, [`messages::backend_for_address`]), else a hosted one.
fn find_inbox(raw: &str) -> Option<ReaderInbox> {
    let address = crate::mail::addresses::normalize_address(raw).ok()?;
    let all = catalog();
    all.iter()
        .find(|i| i.source == Source::Linked && i.address.eq_ignore_ascii_case(&address))
        .or_else(|| all.iter().find(|i| i.source == Source::Hosted && i.address.eq_ignore_ascii_case(&address)))
        .cloned()
}

/// Decide (and log) the viewer's access to an inbox under the reader rule.
/// `Err` carries the answer: the masked `masked` for a refusal or an
/// unknown inbox, 503 when the log can't be written.
fn open(inbox: Option<ReaderInbox>, masked: CliResponse) -> Result<ReaderInbox, CliResponse> {
    let Some(inbox) = inbox else {
        return Err(masked);
    };
    let allowed = mailbox_guard::check_with(
        Some(inbox.owner_project_id.as_str()),
        inbox.source,
        &inbox.inbox_id,
        &inbox.address,
        Rule::Reader,
        true,
    );
    if mailbox_guard::audit_failed() {
        return Err(mailbox_guard::audit_unavailable());
    }
    if !allowed {
        return Err(masked);
    }
    Ok(inbox)
}

fn open_address(raw: &str) -> Result<ReaderInbox, CliResponse> {
    open(find_inbox(raw), inbox_not_found(raw.trim()))
}

/// Decode a message id and open its inbox. `(inbox, backend email id)`.
fn open_message(token: &str) -> Result<(ReaderInbox, String), CliResponse> {
    let Some((address, email_id)) = messages::decode_message_id(token) else {
        return Err(usage("invalid message id — use an id from 'k2 mail reader messages'"));
    };
    let inbox = open(find_inbox(&address), message_not_found(token))?;
    Ok((inbox, email_id))
}

fn account_of(inbox: &ReaderInbox) -> Result<String, CliResponse> {
    inbox.account_id.clone().filter(|a| !a.is_empty()).ok_or_else(|| {
        error_response(
            "502 Bad Gateway",
            "engine",
            &format!("'{}' has no mailbox on the mail server yet", inbox.address),
        )
    })
}

fn required<'a>(params: &'a HashMap<String, String>, key: &str, hint: &str) -> Result<&'a str, CliResponse> {
    params
        .get(key)
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| usage(hint))
}

// ── GET inboxes ─────────────────────────────────────────────────────────

fn handle_inboxes() -> CliResponse {
    let Some(viewer) = mailbox_guard::viewer_info() else {
        return agent_refusal();
    };
    let mut list = Vec::new();
    for inbox in catalog() {
        // What the viewer can't open is left out, without a row per inbox
        // it can't see; what it can open is logged (once per 10 minutes).
        let allowed = mailbox_guard::check_with(
            Some(inbox.owner_project_id.as_str()),
            inbox.source,
            &inbox.inbox_id,
            &inbox.address,
            Rule::Reader,
            false,
        );
        if mailbox_guard::audit_failed() {
            return mailbox_guard::audit_unavailable();
        }
        if allowed {
            list.push(inbox.to_json(viewer.person.as_deref()));
        }
    }
    ok_json(serde_json::json!({
        "ok": true,
        "viewer": { "actor": viewer.actor, "role": viewer.role, "opensAny": viewer.opens_any },
        "count": list.len(),
        "inboxes": list,
    }))
}

// ── GET folders ─────────────────────────────────────────────────────────

/// A folder's role, from its name (the same names the manage verbs know).
fn folder_role(name: &str) -> Option<&'static str> {
    let n = name.trim().to_ascii_lowercase();
    let n = n.strip_prefix("[gmail]/").or_else(|| n.strip_prefix("[google mail]/")).unwrap_or(&n);
    match n {
        "inbox" => Some("inbox"),
        "sent" | "sent items" | "sent mail" | "sent messages" => Some("sent"),
        "drafts" | "draft" => Some("drafts"),
        "junk" | "spam" | "junk email" | "junk e-mail" | "bulk mail" => Some("junk"),
        "trash" | "deleted" | "deleted items" | "deleted messages" | "bin" => Some("trash"),
        "archive" | "archives" | "all mail" => Some("archive"),
        _ => None,
    }
}

fn role_rank(role: Option<&str>) -> usize {
    match role {
        Some("inbox") => 0,
        Some("drafts") => 1,
        Some("sent") => 2,
        Some("archive") => 3,
        Some("junk") => 4,
        Some("trash") => 5,
        _ => 6,
    }
}

fn handle_folders(params: &HashMap<String, String>) -> CliResponse {
    let raw = match required(params, "inbox", "missing 'inbox' — an address from 'k2 mail reader inboxes'") {
        Ok(v) => v,
        Err(r) => return r,
    };
    let inbox = match open_address(raw) {
        Ok(i) => i,
        Err(r) => return r,
    };
    let account = match account_of(&inbox) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let backend = match crate::mail::routes_messages::manage_backend_for(&inbox.address) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let names = match backend.folder_list(&account) {
        Ok(n) => n,
        Err(hint) => return error_response("502 Bad Gateway", "engine", &hint),
    };
    let mut folders: Vec<(usize, String, Option<&'static str>)> = names
        .into_iter()
        .map(|n| {
            let role = folder_role(&n);
            (role_rank(role), n, role)
        })
        .collect();
    folders.sort_by(|a, b| (a.0, a.1.to_ascii_lowercase()).cmp(&(b.0, b.1.to_ascii_lowercase())));
    let list: Vec<serde_json::Value> = folders
        .into_iter()
        .map(|(_, name, role)| serde_json::json!({ "name": name, "role": role }))
        .collect();
    ok_json(serde_json::json!({ "ok": true, "inbox": inbox.address, "folders": list }))
}

// ── GET messages ────────────────────────────────────────────────────────

fn addr_json(a: &MailAddr) -> serde_json::Value {
    serde_json::json!({ "name": a.name, "address": a.email })
}

fn summary_json(address: &str, s: &EmailSummary) -> serde_json::Value {
    serde_json::json!({
        "id": messages::encode_message_id(address, &s.id),
        "inbox": address,
        "from": s.from.first().map(addr_json),
        "to": s.to.iter().map(addr_json).collect::<Vec<_>>(),
        "subject": s.subject,
        "date": s.received_at,
        "unread": s.unread,
        "hasAttachment": s.has_attachment,
        "threadId": s.thread_id,
    })
}

fn folder_param(raw: Option<&str>) -> MailFolder {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        None => MailFolder::Inbox,
        Some(f) if f.eq_ignore_ascii_case("inbox") => MailFolder::Inbox,
        Some(f) if f.eq_ignore_ascii_case("junk") => MailFolder::Junk,
        Some(f) => MailFolder::Named(f.to_string()),
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn handle_messages(params: &HashMap<String, String>) -> CliResponse {
    let raw = match required(params, "inbox", "missing 'inbox' — an address from 'k2 mail reader inboxes'") {
        Ok(v) => v,
        Err(r) => return r,
    };
    let limit = match params.get("limit").map(|v| v.trim().parse::<usize>()) {
        None => DEFAULT_PAGE,
        Some(Ok(n)) if n >= 1 => n.min(MAX_PAGE),
        _ => return usage(&format!("invalid 'limit' — a number from 1 to {MAX_PAGE} (default {DEFAULT_PAGE})")),
    };
    let offset = match params.get("offset").map(|v| v.trim().parse::<usize>()) {
        None => 0,
        Some(Ok(n)) => n,
        Some(Err(_)) => return usage("invalid 'offset' — a whole number (use the nextOffset we return)"),
    };
    let now = now_secs();
    let bound = |key: &str| match params.get(key).map(|s| messages::parse_date_bound(s, now)) {
        None => Ok(None),
        Some(Ok(t)) => Ok(Some(t)),
        Some(Err(hint)) => Err(usage(&hint)),
    };
    let after_unix = match bound("since") {
        Ok(v) => v,
        Err(r) => return r,
    };
    let before_unix = match bound("before") {
        Ok(v) => v,
        Err(r) => return r,
    };
    let folder = folder_param(params.get("folder").map(String::as_str));
    let filter = messages::ListFilter {
        unread_only: crate::cli::bool_param(params, "unread"),
        query: crate::cli::opt_param(params, "query"),
        from: crate::cli::opt_param(params, "from"),
        since_unix: None,
        after_unix,
        before_unix,
        folder: folder.clone(),
        offset,
    };
    let inbox = match open_address(raw) {
        Ok(i) => i,
        Err(r) => return r,
    };
    let account = match account_of(&inbox) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let mut search_window = None;
    if messages::backend_for_address(&inbox.address) == MailBackend::ExternalImap {
        match crate::mail::external_imap::linked_list_search_window(&filter, now) {
            Ok(w) => search_window = w,
            Err(e) => return crate::mail::routes_messages::list_error_response(e),
        }
    }
    let backend = match crate::mail::routes_messages::backend_for(&inbox.address) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let summaries = match backend.list_inbox(&account, &filter, limit) {
        Ok(s) => s,
        Err(e) => return crate::mail::routes_messages::list_error_response(e),
    };
    let next_offset = (summaries.len() >= limit).then_some(offset + limit);
    let items: Vec<serde_json::Value> = summaries.iter().map(|s| summary_json(&inbox.address, s)).collect();
    let folder_label = match &folder {
        MailFolder::Inbox => "inbox".to_string(),
        MailFolder::Junk => "junk".to_string(),
        MailFolder::Named(n) => n.clone(),
    };
    let mut body = serde_json::json!({
        "ok": true,
        "inbox": inbox.address,
        "folder": folder_label,
        "offset": offset,
        "limit": limit,
        "count": items.len(),
        "items": items,
        "nextOffset": next_offset,
    });
    if let Some(w) = search_window {
        body["searchWindow"] = w.to_json();
    }
    ok_json(body)
}

// ── GET message / thread ────────────────────────────────────────────────

/// One message in the person shape: no agent markers, HTML cleaned, never
/// marked read.
fn message_json(address: &str, full: &EmailFull, authority: &messages::AuthAuthority) -> serde_json::Value {
    let text = match (&full.text, &full.html) {
        (Some(t), _) => t.clone(),
        (None, Some(h)) => messages::html_to_text(h),
        (None, None) => String::new(),
    };
    let cleaned = full.html.as_deref().map(k2_core::mail_html::clean_mail_html);
    serde_json::json!({
        "id": messages::encode_message_id(address, &full.summary.id),
        "inbox": address,
        "from": full.summary.from.first().map(addr_json),
        "to": full.summary.to.iter().map(addr_json).collect::<Vec<_>>(),
        "cc": full.cc.iter().map(addr_json).collect::<Vec<_>>(),
        "subject": full.summary.subject,
        "date": full.summary.received_at,
        "unread": full.summary.unread,
        "text": text,
        "html": cleaned.as_ref().map(|c| c.html.clone()),
        "remoteContentBlocked": cleaned.as_ref().is_some_and(|c| c.remote_blocked),
        "attachments": full
            .attachments
            .iter()
            .enumerate()
            .map(|(i, a)| messages::attachment_json(i, a))
            .collect::<Vec<_>>(),
        "threadId": full.summary.thread_id,
        "authResults": messages::parse_auth_results(&full.auth_results, authority),
    })
}

fn engine(hint: String) -> CliResponse {
    error_response("502 Bad Gateway", "engine", &hint)
}

fn handle_message(params: &HashMap<String, String>) -> CliResponse {
    let token = match required(params, "id", "missing 'id' — a message id from 'k2 mail reader messages'") {
        Ok(v) => v,
        Err(r) => return r,
    };
    let (inbox, email_id) = match open_message(token) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let account = match account_of(&inbox) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let backend = match crate::mail::routes_messages::backend_for(&inbox.address) {
        Ok(b) => b,
        Err(r) => return r,
    };
    // Peek: `fetch_full` never marks read on any backend.
    let full = match backend.fetch_full(&account, &email_id) {
        Ok(Some(f)) => f,
        Ok(None) => return message_not_found(token),
        Err(hint) => return engine(hint),
    };
    ok_json(serde_json::json!({
        "ok": true,
        "markedRead": false,
        "message": message_json(&inbox.address, &full, &backend.auth_authority()),
    }))
}

fn handle_thread(params: &HashMap<String, String>) -> CliResponse {
    let token = match required(params, "id", "missing 'id' — a message id from 'k2 mail reader messages'") {
        Ok(v) => v,
        Err(r) => return r,
    };
    let (inbox, email_id) = match open_message(token) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let account = match account_of(&inbox) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let backend = match crate::mail::routes_messages::backend_for(&inbox.address) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let authority = backend.auth_authority();
    let first = match backend.fetch_full(&account, &email_id) {
        Ok(Some(f)) => f,
        Ok(None) => return message_not_found(token),
        Err(hint) => return engine(hint),
    };
    // The open drafts K2 tracks in this inbox: their copies in Drafts are
    // answered as drafts, never as messages.
    let draft_refs: std::collections::HashSet<String> = drafts::list_for_address(&inbox.address, 200)
        .into_iter()
        .filter(|d| d.state == drafts::STATE_OPEN)
        .filter_map(|d| d.backend_ref)
        .collect();
    let thread_id = first.summary.thread_id.clone().filter(|t| !t.is_empty());
    let mut fulls: Vec<EmailFull> = vec![first];
    if let Some(thread_id) = thread_id.as_deref() {
        let asked = folder_param(params.get("folder").map(String::as_str));
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        seen.insert(fulls[0].summary.id.clone());
        let mut ids: Vec<(i64, String)> = Vec::new();
        let mut take = |summaries: Vec<EmailSummary>, ids: &mut Vec<(i64, String)>| {
            for s in summaries {
                if s.thread_id.as_deref() == Some(thread_id) && !draft_refs.contains(&s.id) && seen.insert(s.id.clone()) {
                    ids.push((messages::iso_to_unix(&s.received_at), s.id));
                }
            }
        };
        let filter = messages::ListFilter { folder: asked.clone(), ..Default::default() };
        match backend.list_inbox(&account, &filter, THREAD_SCAN) {
            Ok(s) => take(s, &mut ids),
            Err(e) => return crate::mail::routes_messages::list_error_response(e),
        }
        // A reply sent from this inbox sits in Sent (or, for K2's own
        // hosted sends, the Drafts mailbox without `$draft`): it belongs in
        // the conversation. A folder this backend doesn't have is skipped.
        for role in ["sent", "drafts"] {
            if matches!(&asked, MailFolder::Named(n) if n.eq_ignore_ascii_case(role)) {
                continue;
            }
            let filter = messages::ListFilter { folder: MailFolder::Named(role.to_string()), ..Default::default() };
            if let Ok(s) = backend.list_inbox(&account, &filter, THREAD_SCAN) {
                take(s, &mut ids);
            }
        }
        ids.sort();
        // The newest of the thread, keeping room for the message asked for.
        let keep_from = ids.len().saturating_sub(THREAD_MAX - 1);
        for (_, id) in &ids[keep_from..] {
            match backend.fetch_full(&account, id) {
                Ok(Some(f)) => fulls.push(f),
                Ok(None) => {}
                Err(hint) => return engine(hint),
            }
        }
    }
    fulls.retain(|f| !draft_refs.contains(&f.summary.id));
    fulls.sort_by_key(|f| messages::iso_to_unix(&f.summary.received_at));
    // Drafts first (newest edit first), then the conversation, oldest first.
    let tokens: Vec<String> = std::iter::once(token.to_string())
        .chain(fulls.iter().map(|f| messages::encode_message_id(&inbox.address, &f.summary.id)))
        .collect();
    let mut list: Vec<serde_json::Value> = Vec::new();
    for row in drafts::open_in_thread(&inbox.address, &tokens, thread_id.as_deref()) {
        if let Some(d) = draft_json_fetched(&row, &account, backend.as_ref()) {
            list.push(d);
        }
    }
    list.extend(fulls.iter().map(|f| {
        let mut m = message_json(&inbox.address, f, &authority);
        m["draft"] = serde_json::json!(false);
        m
    }));
    ok_json(serde_json::json!({ "ok": true, "messages": list }))
}

// ── Drafted replies ─────────────────────────────────────────────────────

/// One tracked draft in the message shape the thread answers, marked
/// `draft: true`. `full` is its copy in the mailbox (`None` = K2 can't
/// read it: shown without content, not editable).
fn draft_json(row: &drafts::DraftRow, full: Option<&EmailFull>, hint: Option<&str>) -> serde_json::Value {
    let iso = |t: i64| {
        chrono::DateTime::<chrono::Utc>::from_timestamp(t, 0)
            .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
            .unwrap_or_default()
    };
    let editable = full.is_some() && row.backend_ref.is_some();
    let mut v = serde_json::json!({
        "id": row.id,
        "draft": true,
        "draftId": row.id,
        "inbox": row.address,
        "replyOf": row.reply_of,
        "status": drafts::status_wire(row),
        "author": { "kind": row.author_kind, "name": row.author_label },
        "createdAt": row.created_at,
        "editedAt": row.edited_at,
        "editedBy": row.edited_by,
        "editable": editable,
        "date": iso(row.edited_at),
        "unread": false,
        "hasAttachment": false,
        "attachments": [],
        "threadId": row.thread_ref,
        "from": null,
        "to": [],
        "cc": [],
        "subject": null,
        "text": null,
        "html": null,
    });
    if let Some(f) = full {
        v["from"] = f.summary.from.first().map(addr_json).unwrap_or(serde_json::Value::Null);
        v["to"] = serde_json::json!(f.summary.to.iter().map(addr_json).collect::<Vec<_>>());
        v["cc"] = serde_json::json!(f.cc.iter().map(addr_json).collect::<Vec<_>>());
        v["subject"] = serde_json::json!(f.summary.subject);
        v["text"] = serde_json::json!(match (&f.text, &f.html) {
            (Some(t), _) => t.clone(),
            (None, Some(h)) => messages::html_to_text(h),
            (None, None) => String::new(),
        });
    }
    let hint = hint.map(str::to_string).or_else(|| {
        (!editable).then(|| {
            "K2 can show this draft but not change it; edit or send it from the inbox's own mail app".to_string()
        })
    });
    if let Some(h) = hint {
        v["hint"] = serde_json::json!(h);
    }
    v
}

/// [`draft_json`] after reading the draft's copy. A copy the mailbox no
/// longer has (sent or deleted in the person's own mail app) ends the
/// draft: `None`, and the record says so.
fn draft_json_fetched(row: &drafts::DraftRow, account: &str, backend: &dyn messages::ReadBackend) -> Option<serde_json::Value> {
    let Some(r) = row.backend_ref.as_deref() else {
        return Some(draft_json(row, None, None));
    };
    match backend.fetch_full(account, r) {
        Ok(Some(f)) => Some(draft_json(row, Some(&f), None)),
        Ok(None) => {
            if let Err(e) = drafts::set_state(&row.id, drafts::STATE_DISCARDED, Some("its mailbox"), None, drafts::now_secs()) {
                k2_core::log_debug!("[mail/reader] draft {} left its mailbox; record: {e}", row.id);
            }
            None
        }
        Err(hint) => Some(draft_json(row, None, Some(&format!("K2 couldn't read this draft just now: {hint}")))),
    }
}

/// The masked answer for a draft id: the same bytes whether it doesn't
/// exist, is no longer a draft, or isn't the viewer's to open.
fn draft_not_found(token: &str) -> CliResponse {
    error_response(
        "404 Not Found",
        "not_found",
        &format!("no draft '{token}' you can open — drafts show at the top of their thread, or in 'k2 mail reader drafts <inbox>'"),
    )
}

/// The open draft `token` names, with its inbox opened under the reader
/// rule (logged). Every refusal is [`draft_not_found`].
fn open_draft(token: &str) -> Result<(drafts::DraftRow, ReaderInbox), CliResponse> {
    let Some(row) = drafts::load(token).filter(|r| r.state == drafts::STATE_OPEN) else {
        return Err(draft_not_found(token));
    };
    let inbox = open(find_inbox(&row.address), draft_not_found(token))?;
    if inbox.source != row.source || inbox.inbox_id != row.inbox_id {
        return Err(draft_not_found(token));
    }
    Ok((row, inbox))
}

fn handle_draft_open(params: &HashMap<String, String>) -> CliResponse {
    let token = match required(params, "id", "missing 'id' — a draft id (d_…) from a thread or 'k2 mail reader drafts'") {
        Ok(v) => v,
        Err(r) => return r,
    };
    let (row, inbox) = match open_draft(token) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let account = match account_of(&inbox) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let backend = match crate::mail::routes_messages::backend_for(&inbox.address) {
        Ok(b) => b,
        Err(r) => return r,
    };
    match draft_json_fetched(&row, &account, backend.as_ref()) {
        Some(d) => ok_json(serde_json::json!({ "ok": true, "markedRead": false, "draft": d })),
        None => draft_not_found(token),
    }
}

fn handle_drafts(params: &HashMap<String, String>) -> CliResponse {
    let raw = match required(params, "inbox", "missing 'inbox' — an address from 'k2 mail reader inboxes'") {
        Ok(v) => v,
        Err(r) => return r,
    };
    let inbox = match open_address(raw) {
        Ok(i) => i,
        Err(r) => return r,
    };
    let rows: Vec<serde_json::Value> = drafts::list_for_address(&inbox.address, 50)
        .iter()
        .filter(|d| d.source == inbox.source && d.inbox_id == inbox.inbox_id)
        .map(drafts::meta_json)
        .collect();
    ok_json(serde_json::json!({ "ok": true, "inbox": inbox.address, "count": rows.len(), "drafts": rows }))
}

fn str_list(v: &serde_json::Value, key: &str) -> Result<Option<Vec<String>>, CliResponse> {
    match &v[key] {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::String(s) => {
            Ok(Some(s.split([',', ';']).map(str::trim).filter(|p| !p.is_empty()).map(String::from).collect()))
        }
        serde_json::Value::Array(a) => a
            .iter()
            .map(|x| x.as_str().map(|s| s.trim().to_string()).ok_or(()))
            .collect::<Result<Vec<_>, _>>()
            .map(|l| Some(l.into_iter().filter(|s| !s.is_empty()).collect()))
            .map_err(|_| usage(&format!("'{key}' must be a list of addresses"))),
        _ => Err(usage(&format!("'{key}' must be a list of addresses"))),
    }
}

/// The draft as the person wants it: what the body names, the draft's
/// current copy for anything it leaves out. Checked like a send.
fn draft_values(
    v: &serde_json::Value,
    row: &drafts::DraftRow,
    account: &str,
    address: &str,
) -> Result<drafts::DraftValues, CliResponse> {
    let to = str_list(v, "to")?;
    let cc = str_list(v, "cc")?;
    let subject = v["subject"].as_str().map(|s| s.trim().to_string());
    let body = v["body"].as_str().map(str::to_string);
    let current = if to.is_none() || cc.is_none() || subject.is_none() || body.is_none() {
        let Some(r) = row.backend_ref.as_deref() else {
            return Err(drafts::not_editable(row));
        };
        let backend = crate::mail::routes_messages::backend_for(address)?;
        match backend.fetch_full(account, r) {
            Ok(Some(f)) => Some(f),
            Ok(None) => return Err(draft_not_found(&row.id)),
            Err(hint) => return Err(engine(hint)),
        }
    };
    let cur_list = |l: &[MailAddr]| l.iter().map(|a| a.email.clone()).collect::<Vec<_>>();
    let to = to.unwrap_or_else(|| current.as_ref().map(|f| cur_list(&f.summary.to)).unwrap_or_default());
    let cc = cc.unwrap_or_else(|| current.as_ref().map(|f| cur_list(&f.cc)).unwrap_or_default());
    let subject = subject.unwrap_or_else(|| current.as_ref().map(|f| f.summary.subject.clone()).unwrap_or_default());
    let body = body.unwrap_or_else(|| current.as_ref().and_then(|f| f.text.clone()).unwrap_or_default());
    if to.is_empty() {
        return Err(usage("a draft needs at least one 'to' recipient"));
    }
    if subject.is_empty() {
        return Err(usage("a draft needs a 'subject'"));
    }
    if subject.contains('\r') || subject.contains('\n') {
        return Err(usage("the subject can't hold a line break"));
    }
    if body.trim().is_empty() {
        return Err(usage("a draft needs a 'body' — the text"));
    }
    if subject.len() + body.len() > crate::mail::send::MAX_MESSAGE_BYTES {
        return Err(usage(&format!("draft too large — max {} bytes of text", crate::mail::send::MAX_MESSAGE_BYTES)));
    }
    if to.len() + cc.len() > crate::mail::send::MAX_RECIPIENTS {
        return Err(usage(&format!(
            "too many recipients ({}) — max {} per message, counting to + cc",
            to.len() + cc.len(),
            crate::mail::send::MAX_RECIPIENTS
        )));
    }
    let norm = |l: &[String]| {
        crate::mail::send::normalize_recipients(l).map_err(|e| match e {
            crate::mail::send::SendError::Usage(h) | crate::mail::send::SendError::NotFound(h) => usage(&h),
            other => usage(&format!("invalid recipient: {other:?}")),
        })
    };
    Ok(drafts::DraftValues { to: norm(&to)?, cc: norm(&cc)?, subject, body })
}

/// POST draft-save | draft-send | draft-discard `{id, to?, cc?, subject?,
/// body?, reason?}`.
fn handle_draft_action(verb: &str, body: &[u8]) -> CliResponse {
    let v: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v @ serde_json::Value::Object(_)) => v,
        Ok(_) => return usage("the body must be a JSON object"),
        Err(e) => return usage(&format!("invalid JSON body: {e}")),
    };
    mailbox_guard::set_reason(v.get("reason").and_then(|r| r.as_str()));
    let Some(token) = v.get("id").and_then(|i| i.as_str()).map(str::trim).filter(|s| !s.is_empty()) else {
        return usage("missing 'id' — a draft id (d_…) from a thread or 'k2 mail reader drafts'");
    };
    let (row, inbox) = match open_draft(token) {
        Ok(x) => x,
        Err(r) => return r,
    };
    if let Some(r) = level_refusal(if verb == "draft-send" { "send" } else { "draft" }, &inbox) {
        return r;
    }
    let account = match account_of(&inbox) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let by = drafts::viewer_label().unwrap_or_else(|| "a person".to_string());
    let now = drafts::now_secs();
    match verb {
        "draft-discard" => {
            if let Err(r) = drafts::remove_from_mailbox(&row, &account) {
                return r;
            }
            if let Err(e) = drafts::set_state(&row.id, drafts::STATE_DISCARDED, Some(&by), None, now) {
                return error_response("503 Service Unavailable", "not_ready", &format!("the draft left Drafts, but K2 couldn't record that ({e})"));
            }
            drafts::tell_agent(&row, "discarded", &by);
            ok_json(serde_json::json!({
                "ok": true,
                "draftId": row.id,
                "draftStatus": "discarded",
                "hint": format!("draft {} discarded from {}", row.id, row.address),
            }))
        }
        "draft-save" => {
            let values = match draft_values(&v, &row, &account, &inbox.address) {
                Ok(x) => x,
                Err(r) => return r,
            };
            let from_name = k2_core::workspace::display::agent_display_name(
                inbox.workspace_path.as_deref().unwrap_or(&inbox.owner_project_id),
            );
            let new_ref = match drafts::replace_in_mailbox(&row, &account, &from_name, &values) {
                Ok(r) => r,
                Err(resp) => return resp,
            };
            if let Err(e) = drafts::set_edited(&row.id, new_ref.as_deref(), Some(&by), now) {
                return error_response("503 Service Unavailable", "not_ready", &format!("the draft was saved, but K2 couldn't record it ({e})"));
            }
            if row.edited_by.is_none() {
                drafts::tell_agent(&row, "edited", &by);
            }
            let Some(saved) = drafts::load(&row.id) else {
                return draft_not_found(token);
            };
            let backend = match crate::mail::routes_messages::backend_for(&inbox.address) {
                Ok(b) => b,
                Err(r) => return r,
            };
            let mut out = serde_json::json!({ "ok": true, "draftId": saved.id, "draftStatus": drafts::status_wire(&saved) });
            if let Some(d) = draft_json_fetched(&saved, &account, backend.as_ref()) {
                out["draft"] = d;
            }
            ok_json(out)
        }
        "draft-send" => {
            let values = match draft_values(&v, &row, &account, &inbox.address) {
                Ok(x) => x,
                Err(r) => return r,
            };
            let project = inbox.workspace_path.clone().unwrap_or_else(|| inbox.owner_project_id.clone());
            let send_body = serde_json::json!({
                "project": project,
                "from": inbox.address,
                "to": values.to,
                "cc": values.cc,
                "subject": values.subject,
                "body": values.body,
            });
            let thread = crate::mail::routes_send::ReplyThread {
                in_reply_to: row.in_reply_to.clone(),
                references: row.refs.clone(),
            };
            let resp = crate::mail::routes_send::handle_send_in_thread(send_body.to_string().as_bytes(), &thread);
            let mut out: serde_json::Value = match serde_json::from_str(&resp.body) {
                Ok(j) if resp.status.starts_with('2') && j["ok"] == true => j,
                // Refused or failed: the draft stays as it was.
                _ => return resp,
            };
            let state = match out["status"].as_str() {
                Some("pending_approval") => drafts::STATE_HELD,
                _ => drafts::STATE_SENT,
            };
            if let Err(r) = drafts::remove_from_mailbox(&row, &account) {
                k2_core::log_debug!("[mail/reader] draft {} was sent; its copy stayed in Drafts: {}", row.id, r.body);
            }
            let outbound = out["id"].as_str().map(str::to_string);
            if let Err(e) = drafts::set_state(&row.id, state, Some(&by), outbound.as_deref(), now) {
                k2_core::log_debug!("[mail/reader] draft {} was sent; record: {e}", row.id);
            }
            drafts::tell_agent(&row, state, &by);
            out["draftId"] = serde_json::json!(row.id);
            out["draftStatus"] = serde_json::json!(if state == drafts::STATE_HELD { "held" } else { "sent" });
            ok_json(out)
        }
        _ => CliResponse::not_found(),
    }
}

// ── GET attachment ──────────────────────────────────────────────────────

fn handle_attachment(params: &HashMap<String, String>) -> CliResponse {
    let token = match required(params, "id", "missing 'id' — a message id from 'k2 mail reader messages'") {
        Ok(v) => v,
        Err(r) => return r,
    };
    let part = match params.get("part").map(|v| v.trim().parse::<usize>()) {
        Some(Ok(n)) if n >= 1 => n,
        _ => return usage("missing or invalid 'part' — the 1-based attachment number from the message"),
    };
    let (inbox, email_id) = match open_message(token) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let account = match account_of(&inbox) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let backend = match crate::mail::routes_messages::backend_for(&inbox.address) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let full = match backend.fetch_full(&account, &email_id) {
        Ok(Some(f)) => f,
        Ok(None) => return message_not_found(token),
        Err(hint) => return engine(hint),
    };
    let Some(meta) = full.attachments.get(part - 1) else {
        return error_response(
            "404 Not Found",
            "not_found",
            &format!("no attachment #{part} — this message has {} (1-based)", full.attachments.len()),
        );
    };
    if meta.size > MAX_ATTACHMENT_BYTES {
        return error_response(
            "413 Payload Too Large",
            "too_large",
            &format!("attachment #{part} is larger than the reader's 25 MB download cap"),
        );
    }
    let name = meta.filename.clone().unwrap_or_else(|| format!("attachment-{part}"));
    let bytes = match backend.fetch_blob(&account, &meta.blob_id, &name, &meta.mime) {
        Ok(b) => b,
        Err(hint) => return engine(hint),
    };
    if bytes.len() as u64 > MAX_ATTACHMENT_BYTES {
        return error_response(
            "413 Payload Too Large",
            "too_large",
            &format!("attachment #{part} is larger than the reader's 25 MB download cap"),
        );
    }
    ok_json(serde_json::json!({
        "ok": true,
        "filename": name,
        "mime": meta.mime,
        "size": bytes.len(),
        "dataBase64": B64.encode(&bytes),
    }))
}

// ── GET log (OA2) ───────────────────────────────────────────────────────

/// The access log. The Owner and Admins see every row; anyone else the
/// rows about their own mailboxes. Reading the log reads no mail, so it
/// writes no row.
fn handle_log(params: &HashMap<String, String>) -> CliResponse {
    let Some(viewer) = mailbox_guard::viewer_info() else {
        return agent_refusal();
    };
    let limit = match params.get("limit").map(|v| v.trim().parse::<usize>()) {
        None => LOG_DEFAULT,
        Some(Ok(n)) if n >= 1 => n.min(LOG_MAX),
        _ => return usage(&format!("invalid 'limit' — a number from 1 to {LOG_MAX} (default {LOG_DEFAULT})")),
    };
    let since = match params.get("since").map(|s| messages::parse_date_bound(s, now_secs())) {
        None => None,
        Some(Ok(t)) => Some(t),
        Some(Err(hint)) => return usage(&hint),
    };
    let before_id = match params.get("before").map(|v| v.trim().parse::<i64>()) {
        None => None,
        Some(Ok(n)) => Some(n),
        Some(Err(_)) => return usage("invalid 'before' — a row id from an earlier page"),
    };
    let inbox = crate::cli::opt_param(params, "inbox").map(|a| a.trim().to_ascii_lowercase());
    let actor = crate::cli::opt_param(params, "actor").map(|a| {
        let a = a.trim().to_string();
        if a == "owner-token" || a.starts_with("user:") { a } else { format!("user:{a}") }
    });
    // Whose rows this viewer may see: all, or their own mailboxes'.
    let own: Option<Vec<String>> = if viewer.opens_any {
        None
    } else {
        Some(
            catalog()
                .into_iter()
                .filter(|i| i.own(viewer.person.as_deref()))
                .map(|i| i.address.to_ascii_lowercase())
                .collect(),
        )
    };
    if own.as_ref().is_some_and(|o| o.is_empty()) {
        return ok_json(serde_json::json!({ "ok": true, "count": 0, "rows": [] }));
    }
    let mut sql = String::from(
        "SELECT id, at, actor, actor_role, inbox_source, address, inbox_kind, own, action, \
         message_ref, reason, ip_hint, outcome, status FROM mail_reader_audit WHERE 1 = 1",
    );
    let mut args: Vec<rusqlite::types::Value> = Vec::new();
    if let Some(a) = &inbox {
        sql.push_str(" AND lower(address) = ?");
        args.push(a.clone().into());
    }
    if let Some(a) = &actor {
        sql.push_str(" AND actor = ?");
        args.push(a.clone().into());
    }
    if let Some(t) = since {
        sql.push_str(" AND at >= ?");
        args.push(t.into());
    }
    if let Some(b) = before_id {
        sql.push_str(" AND id < ?");
        args.push(b.into());
    }
    if let Some(own) = &own {
        sql.push_str(" AND lower(address) IN (");
        sql.push_str(&vec!["?"; own.len()].join(", "));
        sql.push(')');
        args.extend(own.iter().map(|a| rusqlite::types::Value::from(a.clone())));
    }
    sql.push_str(" ORDER BY id DESC LIMIT ?");
    args.push((limit as i64).into());
    match read_log_rows(&sql, &args) {
        Ok(rows) => ok_json(serde_json::json!({ "ok": true, "count": rows.len(), "rows": rows })),
        Err(e) => error_response(
            "503 Service Unavailable",
            "audit_unavailable",
            &format!("K2 couldn't read the mailbox access log ({e}); 'k2 doctor' checks the daemon's database"),
        ),
    }
}

fn read_log_rows(sql: &str, args: &[rusqlite::types::Value]) -> rusqlite::Result<Vec<serde_json::Value>> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(args.iter()), |r| {
            Ok(serde_json::json!({
                "id": r.get::<_, i64>(0)?,
                "at": r.get::<_, i64>(1)?,
                "actor": r.get::<_, String>(2)?,
                "actorRole": r.get::<_, String>(3)?,
                "source": r.get::<_, String>(4)?,
                "address": r.get::<_, String>(5)?,
                "kind": r.get::<_, String>(6)?,
                "own": r.get::<_, i64>(7)? != 0,
                "action": r.get::<_, String>(8)?,
                "messageRef": r.get::<_, Option<String>>(9)?,
                "reason": r.get::<_, Option<String>>(10)?,
                "ipHint": r.get::<_, Option<String>>(11)?,
                "outcome": r.get::<_, String>(12)?,
                "status": r.get::<_, Option<i64>>(13)?,
            }))
        })?
        .collect::<rusqlite::Result<Vec<_>>>();
    rows
}

// ── POST writes (OA9) ───────────────────────────────────────────────────

/// What the inbox's own workspace must be allowed to do for `verb`.
fn level_refusal(verb: &str, inbox: &ReaderInbox) -> Option<CliResponse> {
    let level = inbox.primary_level.as_str();
    let (ok, what) = match verb {
        "send" | "reply" => (level == "send", "send from it (its level is not 'send')"),
        "draft" => (level == "send" || level == "draft", "save drafts in it (its level is 'read')"),
        "move" | "flag" | "archive" => (inbox.can_manage, "organize it (move, flag, archive)"),
        "delete" => (inbox.can_delete, "delete from it"),
        _ => (true, ""),
    };
    if ok {
        return None;
    }
    let ws = inbox.workspace.as_deref().unwrap_or("its workspace");
    Some(error_response(
        "403 Forbidden",
        "not_allowed",
        &format!(
            "the reader acts on '{}' as its workspace '{ws}', and '{ws}' may not {what}. An Owner or \
             Admin can change that in Settings → Email, or with 'k2 mail access set-level' / \
             'k2 mail access set-manage'",
            inbox.address
        ),
    ))
}

fn handle_write(verb: &str, body: &[u8]) -> CliResponse {
    let v: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v @ serde_json::Value::Object(_)) => v,
        Ok(_) => return usage("the body must be a JSON object"),
        Err(e) => return usage(&format!("invalid JSON body: {e}")),
    };
    mailbox_guard::set_reason(v.get("reason").and_then(|r| r.as_str()));
    if v.get("attachments").is_some_and(|a| !a.is_null()) {
        return usage("the mail reader can't attach files yet — send attachments with 'k2 mail send --attach' from the inbox's workspace");
    }
    let id = v.get("id").and_then(|i| i.as_str()).map(str::trim).filter(|s| !s.is_empty());
    let inbox_arg = v.get("inbox").and_then(|i| i.as_str()).map(str::trim).filter(|s| !s.is_empty());
    // Which inbox the action names: a message id, or `inbox` for a new
    // message (send, and a draft that isn't a reply).
    let by_inbox = verb == "send" || (verb == "draft" && id.is_none());
    let inbox = if by_inbox {
        if verb == "send" && id.is_some() {
            return usage("send takes 'inbox', not 'id' — to answer a message use reply");
        }
        let Some(addr) = inbox_arg else {
            return usage("missing 'inbox' — the address to send from (from 'k2 mail reader inboxes')");
        };
        match open_address(addr) {
            Ok(i) => i,
            Err(r) => return r,
        }
    } else {
        let Some(token) = id else {
            return usage("missing 'id' — a message id from 'k2 mail reader messages'");
        };
        match open_message(token) {
            Ok((i, _)) => i,
            Err(r) => return r,
        }
    };
    if let Some(r) = level_refusal(verb, &inbox) {
        return r;
    }
    // The same call the inbox's own workspace would make.
    let mut inner = v.clone();
    if let Some(obj) = inner.as_object_mut() {
        for k in ["inbox", "reason", "project", "attachments", "from"] {
            obj.remove(k);
        }
        let project = inbox.workspace_path.clone().unwrap_or_else(|| inbox.owner_project_id.clone());
        obj.insert("project".to_string(), serde_json::json!(project));
        if by_inbox {
            obj.insert("from".to_string(), serde_json::json!(inbox.address));
        }
    }
    let bytes = inner.to_string().into_bytes();
    match verb {
        "move" => crate::mail::routes_messages::handle_move(&bytes),
        "flag" => crate::mail::routes_messages::handle_flag(&bytes),
        "archive" => crate::mail::routes_messages::handle_archive(&bytes),
        "delete" => crate::mail::routes_messages::handle_delete(&bytes),
        "reply" => crate::mail::routes_send::handle_reply(&bytes),
        "draft" => crate::mail::routes_external::handle_draft(&bytes),
        "send" => crate::mail::routes_send::handle_send(&bytes),
        _ => CliResponse::not_found(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_roles_cover_the_common_names() {
        for (name, role) in [
            ("INBOX", Some("inbox")),
            ("Sent Items", Some("sent")),
            ("[Gmail]/Sent Mail", Some("sent")),
            ("Junk Email", Some("junk")),
            ("[Gmail]/Spam", Some("junk")),
            ("Deleted Items", Some("trash")),
            ("Archive", Some("archive")),
            ("Receipts", None),
        ] {
            assert_eq!(folder_role(name), role, "{name}");
        }
    }

    #[test]
    fn linked_providers_are_named_from_kind_provider_and_host() {
        assert_eq!(linked_provider("graph", None, "graph.microsoft.com"), "microsoft");
        assert_eq!(linked_provider("imap", Some("gmail"), "imap.gmail.com"), "gmail");
        assert_eq!(linked_provider("imap", None, "imap.gmail.com"), "gmail");
        assert_eq!(linked_provider("imap", None, "outlook.office365.com"), "microsoft");
        assert_eq!(linked_provider("imap", None, "mail.example.org"), "imap");
    }

    #[test]
    fn every_reader_route_is_classified_and_writes_are_post_only() {
        use crate::routes::route_policy::{lookup, Floor};
        for r in READ_ROUTES {
            let row = lookup(r).unwrap_or_else(|| panic!("{r} has no route_policy row"));
            assert_eq!(row.get, Some(Floor::Member), "{r}");
            assert_eq!(row.post, None, "{r} is a read");
        }
        for r in WRITE_ROUTES {
            let row = lookup(r).unwrap_or_else(|| panic!("{r} has no route_policy row"));
            assert_eq!(row.post, Some(Floor::Member), "{r}");
            assert_eq!(row.get, None, "{r} writes, so GET is refused");
        }
    }
}
