//! OA0 (0.46.11, prd-ocmail-admin-in-k2-v1 §7.3): who may act on a mailbox
//! through a named workspace, and the access log every such call writes.
//!
//! **The gap this closes.** A login with no agent passport names the
//! workspace it acts on (`project=`), and the mail routes trusted that
//! claim: any Member could read, move, reply to or delete a person's mail
//! by naming the workspace the mailbox sits in. Nothing logged it, and a
//! read marked the mail read.
//!
//! **The rule (Rosson, 2026-11-10).** A *person's mailbox* (a hosted
//! address with `person_mailbox = 1` or a `person_principal`, and every
//! linked inbox, which is always someone's own account) may be used only
//! by that person, an Admin or the Owner. Everyone else gets the same
//! masked `not_found` an unknown address gets. An agent inbox keeps working
//! for Members (the agent tab's Inbox depends on it). Every access by a
//! login or the owner token is logged in `mail_reader_audit` (0284) before
//! the mail is touched; no row, no access (503). The person isn't told.
//!
//! **Where it runs.** The dispatcher's mail arms wrap the handler in
//! [`run`] when the caller has no passport. The access layer
//! (`mail::access`) asks [`check`] for every inbox it resolves, so every
//! route that reaches an inbox through `can_read` / `can_draft` /
//! `can_send` / `can_manage` / `can_delete` or the sweep lists is covered
//! in one place; the few routes that act on an inbox some other way call
//! [`check_address`] themselves. Agent passports and background loops run
//! with no context installed, so their rules are unchanged.
//!
//! **Read state.** [`may_mark_seen`] is true only for the mailbox's own
//! reader (its person, or its agent through a passport). Everyone else
//! reads without marking the mail read.
//!
//! **The mail reader** (`mail::routes_reader`, OA1–OA9) uses the same
//! context and log under a stricter rule, [`Rule::Reader`]: the Owner and
//! Admins open every inbox, anyone else only their own person mailbox. A
//! linked inbox's person is who linked it (`linked_by_principal`, 0288),
//! for both rules. The viewer's `reason` rides in the row ([`set_reason`]).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use crate::cli_response::CliResponse;
use crate::mail::access::Source;

/// Who is calling, when it isn't an agent passport.
#[derive(Debug, Clone)]
pub enum Viewer {
    /// The on-box owner daemon token.
    OwnerToken,
    /// A Connect login (local or through the tunnel).
    Login { username: String, role: k2_core::connect_users::Role },
    /// Authenticated, but neither of the above (never expected). Treated
    /// like a Member with no person: fail closed.
    Unknown,
}

impl Viewer {
    fn actor(&self) -> String {
        match self {
            Viewer::OwnerToken => "owner-token".to_string(),
            Viewer::Login { username, .. } => format!("user:{username}"),
            Viewer::Unknown => "unknown".to_string(),
        }
    }

    fn role_wire(&self) -> &'static str {
        match self {
            Viewer::OwnerToken => "owner",
            Viewer::Login { role, .. } => role.as_wire(),
            Viewer::Unknown => "unknown",
        }
    }

    /// Owner or Admin: may open any mailbox (logged).
    fn opens_any(&self) -> bool {
        match self {
            Viewer::OwnerToken => true,
            Viewer::Login { role, .. } => k2_core::connect_users::can_manage_users(*role),
            Viewer::Unknown => false,
        }
    }

    /// The person key this viewer is (`owner` for the owner token, else the
    /// login's person key), as `person_principal` stores it.
    fn person(&self) -> Option<String> {
        match self {
            Viewer::OwnerToken => Some(k2_core::notify_prefs::OWNER.to_string()),
            Viewer::Login { username, .. } => {
                let people = k2_core::notify_prefs::People::load().ok()?;
                people.login_person_key(username)
            }
            Viewer::Unknown => None,
        }
    }
}

struct Ctx {
    viewer: Viewer,
    person: Option<String>,
    action: String,
    message_ref: Option<String>,
    ip_hint: Option<String>,
    /// The viewer's own words for why (the mail reader's `reason`).
    reason: Option<String>,
    /// `(source, inbox_id)` → the decision already made in this request.
    decided: HashMap<(&'static str, String), bool>,
    /// Rows this request wrote (their `status` is filled in at the end).
    rows: Vec<i64>,
    /// Normalized addresses this viewer reads as the mailbox's own person.
    own: HashSet<String>,
    /// The log couldn't be written: the call answers 503.
    audit_failed: bool,
}

thread_local! {
    static CTX: RefCell<Option<Ctx>> = const { RefCell::new(None) };
}

/// List-shaped calls log one row per inbox per viewer per 10 minutes, so
/// the agent tab refreshing doesn't flood the table (PRD §7.2). Every
/// other call writes its own row.
const LIST_WINDOW_SECS: i64 = 600;

fn is_list_action(action: &str) -> bool {
    matches!(
        action,
        "messages"
            | "folder/list"
            | "inboxes"
            | "capabilities"
            | "address/list"
            | "outbox"
            | "watch"
            | "watch/log"
            | "watch/inbox"
            | "calendar/list"
            | "calendar/events"
            | "calendar/freebusy"
            | "reader/inboxes"
            | "reader/folders"
            | "reader/messages"
    )
}

/// The answer when the log can't be written. Nothing was fetched.
pub fn audit_unavailable() -> CliResponse {
    CliResponse {
        status: "503 Service Unavailable",
        content_type: "application/json",
        body: serde_json::json!({
            "ok": false,
            "error": {
                "code": "audit_unavailable",
                "hint": "K2 couldn't write the mailbox access log, so it didn't open the mailbox. Try again; if it keeps happening, the daemon's database needs a look (k2 doctor).",
            },
        })
        .to_string(),
    }
}

fn status_code(status: &str) -> Option<i64> {
    status.split_whitespace().next().and_then(|s| s.parse().ok())
}

/// Run one mail handler for `viewer` (`None` = an agent passport: no
/// context, today's rules). `route` is the request path; `message_ref` the
/// opaque message id the call names, if any; `ip_hint` where the call came
/// in (loopback, LAN, tunnel). After the handler, each row it wrote gets
/// the HTTP status; a failed log write turns the answer into a 503.
pub fn run(
    viewer: Option<Viewer>,
    route: &str,
    message_ref: Option<String>,
    ip_hint: Option<&str>,
    f: impl FnOnce() -> CliResponse,
) -> CliResponse {
    let Some(viewer) = viewer else {
        return f();
    };
    struct ClearOnDrop;
    impl Drop for ClearOnDrop {
        fn drop(&mut self) {
            CTX.with(|c| *c.borrow_mut() = None);
        }
    }
    let _guard = ClearOnDrop;
    let person = viewer.person();
    let action = route.strip_prefix("/cli/mail/").unwrap_or(route).to_string();
    CTX.with(|c| {
        *c.borrow_mut() = Some(Ctx {
            viewer,
            person,
            action,
            message_ref: message_ref.filter(|m| !m.trim().is_empty()),
            ip_hint: ip_hint.map(str::to_string),
            reason: None,
            decided: HashMap::new(),
            rows: Vec::new(),
            own: HashSet::new(),
            audit_failed: false,
        })
    });
    let resp = f();
    let Some(ctx) = CTX.with(|c| c.borrow_mut().take()) else {
        return resp;
    };
    if ctx.audit_failed {
        return audit_unavailable();
    }
    if let (Some(code), false) = (status_code(resp.status), ctx.rows.is_empty()) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        for id in &ctx.rows {
            if let Err(e) = conn.execute(
                "UPDATE mail_reader_audit SET status = ?1 WHERE id = ?2",
                rusqlite::params![code, id],
            ) {
                k2_core::log_debug!("[mail/guard] status for audit row {id}: {e}");
            }
        }
    }
    resp
}

/// True while a login or the owner token is calling (a [`run`] is active).
pub fn active() -> bool {
    CTX.with(|c| c.borrow().is_some())
}

/// Whether this request may mark mail in `address` read: always without a
/// context (agent passports, background loops), else only when the viewer
/// is the mailbox's own person.
pub fn may_mark_seen(address: &str) -> bool {
    let norm = address.trim().to_ascii_lowercase();
    CTX.with(|c| match c.borrow().as_ref() {
        None => true,
        Some(ctx) => ctx.own.contains(&norm),
    })
}

/// `(is a person's mailbox, its person key)`. A lookup failure counts as a
/// person's mailbox (fail closed).
fn inbox_kind(conn: &rusqlite::Connection, source: Source, inbox_id: &str) -> (bool, Option<String>) {
    match source {
        // A linked inbox is always someone's own account; whose, when the
        // login that linked it is on record (0288).
        Source::Linked => conn
            .query_row(
                "SELECT linked_by_principal FROM mail_external_inboxes WHERE id = ?1",
                rusqlite::params![inbox_id],
                |r| r.get::<_, Option<String>>(0),
            )
            .map(|person| (true, person))
            .unwrap_or((true, None)),
        Source::Hosted => conn
            .query_row(
                "SELECT person_mailbox, person_principal FROM mail_addresses WHERE id = ?1",
                rusqlite::params![inbox_id],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?)),
            )
            .map(|(flag, person)| (flag != 0 || person.is_some(), person))
            .unwrap_or((true, None)),
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Write one row (or find the list-window row it repeats). `Ok(None)` =
/// the window already has it.
#[allow(clippy::too_many_arguments)]
fn write_row(
    conn: &rusqlite::Connection,
    ctx: &Ctx,
    source: Source,
    inbox_id: &str,
    address: &str,
    person_kind: bool,
    own: bool,
    workspace: Option<&str>,
    allowed: bool,
) -> rusqlite::Result<Option<i64>> {
    let at = now_secs();
    let actor = ctx.viewer.actor();
    let outcome = if allowed { "ok" } else { "refused" };
    if is_list_action(&ctx.action) {
        let seen: i64 = conn.query_row(
            "SELECT COUNT(*) FROM mail_reader_audit WHERE actor = ?1 AND inbox_source = ?2 \
             AND inbox_id = ?3 AND action = ?4 AND outcome = ?5 AND at > ?6",
            rusqlite::params![actor, source.as_str(), inbox_id, ctx.action.as_str(), outcome, at - LIST_WINDOW_SECS],
            |r| r.get(0),
        )?;
        if seen > 0 {
            return Ok(None);
        }
    }
    conn.execute(
        "INSERT INTO mail_reader_audit (at, actor, actor_role, inbox_source, inbox_id, address, \
         inbox_kind, own, workspace_id, action, message_ref, ip_hint, outcome, reason) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        rusqlite::params![
            at,
            actor,
            ctx.viewer.role_wire(),
            source.as_str(),
            inbox_id,
            address,
            if person_kind { "person" } else { "agent" },
            own as i64,
            workspace,
            ctx.action.as_str(),
            ctx.message_ref.as_deref(),
            ctx.ip_hint.as_deref(),
            outcome,
            ctx.reason.as_deref(),
        ],
    )?;
    Ok(Some(conn.last_insert_rowid()))
}

/// Which rule a decision follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    /// OA0, a call naming a workspace: a person's mailbox needs its own
    /// person, an Admin or the Owner; an agent inbox is open to every login
    /// (the agent tab's Inbox).
    Workspace,
    /// The mail reader (OA1/OA3): the Owner and Admins open every inbox;
    /// anyone else only their own person mailbox.
    Reader,
}

/// May this request act on the inbox `(source, inbox_id)` (`address`),
/// reached through `workspace`? Always true with no context. With one, the
/// [`Rule::Workspace`] rule. Logs the decision first; a failed write
/// refuses (and the call answers 503). The caller must not hold the DB lock.
pub fn check(workspace: Option<&str>, source: Source, inbox_id: &str, address: &str) -> bool {
    check_with(workspace, source, inbox_id, address, Rule::Workspace, true)
}

/// [`check`] under `rule`. `log_refused = false` decides a refusal without
/// writing a row (the reader's inbox list leaves out what a Member can't
/// open instead of logging every other inbox on the box); an allowed
/// access is always logged. A decision is made once per inbox per request.
pub fn check_with(
    workspace: Option<&str>,
    source: Source,
    inbox_id: &str,
    address: &str,
    rule: Rule,
    log_refused: bool,
) -> bool {
    let key = (source.as_str(), inbox_id.to_string());
    let prior = CTX.with(|c| c.borrow().as_ref().map(|ctx| ctx.decided.get(&key).copied()));
    let prior = match prior {
        None => return true,
        Some(p) => p,
    };
    if let Some(decided) = prior {
        return decided;
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    let (person_kind, person) = inbox_kind(&conn, source, inbox_id);
    CTX.with(|c| {
        let mut slot = c.borrow_mut();
        let Some(ctx) = slot.as_mut() else { return true };
        let own = person_kind
            && person.is_some()
            && ctx.person.as_deref().is_some_and(|me| Some(me) == person.as_deref());
        let allowed = match rule {
            Rule::Workspace => !person_kind || own || ctx.viewer.opens_any(),
            Rule::Reader => own || ctx.viewer.opens_any(),
        };
        if !allowed && !log_refused {
            ctx.decided.insert(key, false);
            return false;
        }
        let written = write_row(&conn, ctx, source, inbox_id, address, person_kind, own, workspace, allowed);
        let allowed = match written {
            Ok(id) => {
                if let Some(id) = id {
                    ctx.rows.push(id);
                }
                allowed
            }
            Err(e) => {
                k2_core::log_debug!("[mail/guard] audit write failed: {e}");
                ctx.audit_failed = true;
                false
            }
        };
        if allowed && own {
            ctx.own.insert(address.trim().to_ascii_lowercase());
        }
        ctx.decided.insert(key, allowed);
        allowed
    })
}

/// What the mail reader knows about the viewer of this request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewerInfo {
    /// `owner-token` | `user:<login>` (the log's `actor`).
    pub actor: String,
    /// `owner` | `admin` | `member` | `voice_chat` | `unknown`.
    pub role: &'static str,
    /// The Owner or an Admin: may open every inbox.
    pub opens_any: bool,
    /// The person key mailboxes record for this viewer (`person_principal`,
    /// `linked_by_principal`), when it has one.
    pub person: Option<String>,
}

/// The viewer of this request, while a [`run`] is active.
pub fn viewer_info() -> Option<ViewerInfo> {
    CTX.with(|c| {
        c.borrow().as_ref().map(|ctx| ViewerInfo {
            actor: ctx.viewer.actor(),
            role: ctx.viewer.role_wire(),
            opens_any: ctx.viewer.opens_any(),
            person: ctx.person.clone(),
        })
    })
}

/// The person key of the login (or the owner token) making this request,
/// if a [`run`] is active and the viewer has one. `None` for an agent
/// passport and background work. Recorded as who linked an inbox (0288).
pub fn viewer_person() -> Option<String> {
    CTX.with(|c| c.borrow().as_ref().and_then(|ctx| ctx.person.clone()))
}

/// Record the viewer's reason for the rest of this request's log rows
/// (trimmed, at most 500 characters; blank = none).
pub fn set_reason(reason: Option<&str>) {
    let reason = reason
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .map(|r| r.chars().take(500).collect::<String>());
    CTX.with(|c| {
        if let Some(ctx) = c.borrow_mut().as_mut() {
            ctx.reason = reason;
        }
    });
}

/// Whether this request's log couldn't be written (the call will answer
/// 503 when it ends). Long reads check it before dialling the engine.
pub fn audit_failed() -> bool {
    CTX.with(|c| c.borrow().as_ref().is_some_and(|ctx| ctx.audit_failed))
}

/// [`check`] for an address (hosted or linked). An address K2 has no row
/// for is not an inbox at all: true (the route answers its own not_found).
pub fn check_address(workspace: Option<&str>, raw_address: &str) -> bool {
    if !active() {
        return true;
    }
    let Ok(address) = crate::mail::addresses::normalize_address(raw_address) else {
        return true;
    };
    if let Some(row) = crate::mail::external::inbox_for_address(&address) {
        return check(workspace.or(Some(row.owner_project_id.as_str())), Source::Linked, &row.id, &address);
    }
    let hosted = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT id, owner_project_id FROM mail_addresses WHERE lower(address) = lower(?1) \
             ORDER BY (status = 'active') DESC, created_at DESC LIMIT 1",
            rusqlite::params![address],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )
        .ok()
    };
    match hosted {
        Some((id, owner)) => check(workspace.or(Some(owner.as_str())), Source::Hosted, &id, &address),
        None => true,
    }
}
