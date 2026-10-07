//! Who may hold mail credentials, approve agent mail and change agent mail
//! policy.
//!
//! K2's send path (`k2 mail send` / `reply`) is where the owner's rules
//! live: the agentSend mode, the always-BCC list, the 10-recipient cap and
//! the outbox record. An IMAP/SMTP secret skips every one of them
//! (Stalwart submission on :465 knows nothing about K2). The rule
//! (Rosson, 0.45.0):
//!
//! - **Owner** (owner daemon token, or an owner/admin login — no scoped
//!   passport): everything.
//! - **IT agent** (an agent session whose workspace has mail-manage): as
//!   capable as the owner for everything mail, EXCEPT loosening the rules
//!   on its own sends. It may not mint, rotate or see an SMTP-capable
//!   secret for its OWN workspace's mailboxes (a person's mailbox bound to
//!   its workspace is fine — see `mail_addresses.person_mailbox`), approve
//!   or reject its own workspace's queued sends, change agentSend or
//!   always-BCC for its own workspace, or mark its own mailbox a person's.
//!   Those get 403 `owner_only` ("an agent can't loosen mail rules on its
//!   own sends").
//! - **Other agents**: no mail credentials at all (their mint withholds the
//!   password), no approvals, no policy. The mail-manage routes are already
//!   gated off for them by the dispatcher (`owner_only`); the handlers
//!   repeat it as 403 `agent_send_path_only`.
//!
//! Every secret shown from 0.45.0 is recorded in `mail_credential_marks`
//! (migration 0133; ids and the creator's workspace id, never a secret).
//! [`credential_check_for`] is the doctor check: it flags a live secret
//! with no record (from before 0.45.0 — K2 can't tell who holds it) or one
//! an agent made for its own mailbox. It never revokes anything.
//!
//! Known platform limit, not fixed here: agents run as the same Unix user
//! as the daemon, so a determined agent could read the daemon's 0600
//! vault (`~/.k2/mail-secrets.json`). Nothing in this module writes a
//! secret anywhere.

use crate::cli_response::CliResponse;
use crate::mail::app_password::AppPasswordEngine;
use crate::mail::doctor::{DoctorCheck, ST_INFO, ST_PASS, ST_UNKNOWN, ST_WARN};
use k2_core::db::schema::MailAddress;

/// Refusal code for an agent without mail-manage.
pub const AGENT_SEND_PATH_ONLY: &str = "agent_send_path_only";

pub const KIND_MAILBOX: &str = "mailbox";
pub const KIND_APP_PASSWORD: &str = "app_password";

pub const ORIGIN_MINTED: &str = "minted";
pub const ORIGIN_ROTATED: &str = "rotated";
pub const ORIGIN_KEPT: &str = "kept";
pub const ORIGIN_WITHHELD: &str = "withheld";

/// Doctor check id.
pub const CHECK_ID: &str = "agent-credentials";
const CHECK_LABEL: &str = "Mail passwords an agent may hold";
/// Items named in the doctor detail before "and N more".
const DETAIL_ITEM_CAP: usize = 20;

// ── Who is calling ──────────────────────────────────────────────────────

/// The caller of a mail route, by what it may do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MailCaller {
    /// Owner daemon token or an owner/admin login (no scoped passport).
    Owner,
    /// An agent session whose workspace has mail-manage.
    ItAgent { workspace_uuid: String },
    /// Any other agent session.
    Agent { workspace_uuid: String },
}

impl MailCaller {
    /// The workspace recorded as a credential's creator (`None` = owner).
    pub fn creator(&self) -> Option<&str> {
        match self {
            MailCaller::Owner => None,
            MailCaller::ItAgent { workspace_uuid } | MailCaller::Agent { workspace_uuid } => {
                Some(workspace_uuid)
            }
        }
    }

    pub fn is_agent(&self) -> bool {
        !matches!(self, MailCaller::Owner)
    }
}

/// Whether `workspace_uuid`'s workspace has mail-manage on.
pub fn mail_manage_for_workspace(workspace_uuid: &str) -> bool {
    crate::workspace_msg::resolve_workspace(workspace_uuid)
        .as_deref()
        .is_some_and(k2_core::workspace::settings::mail_manage_allowed_for_path)
}

/// Classify the current request ([`crate::caller_workspace::request_principal`]).
pub fn mail_caller() -> MailCaller {
    match crate::caller_workspace::request_principal() {
        None => MailCaller::Owner,
        Some(p) => {
            if mail_manage_for_workspace(&p.workspace_uuid) {
                MailCaller::ItAgent {
                    workspace_uuid: p.workspace_uuid,
                }
            } else {
                MailCaller::Agent {
                    workspace_uuid: p.workspace_uuid,
                }
            }
        }
    }
}

/// Whether the mailbox row is a person's mailbox (`person_mailbox = 1`).
pub fn is_person_mailbox(address_id: &str) -> bool {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT person_mailbox FROM mail_addresses WHERE id = ?1",
        rusqlite::params![address_id],
        |r| r.get::<_, i64>(0),
    )
    .map(|v| v == 1)
    .unwrap_or(false)
}

/// An agent's OWN mailbox: bound to its workspace and not a person's.
pub fn is_own_mailbox(workspace_uuid: &str, row: &MailAddress) -> bool {
    row.owner_project_id == workspace_uuid && !is_person_mailbox(&row.id)
}

// ── Refusals ────────────────────────────────────────────────────────────

fn err_json(status: &'static str, code: &str, hint: String) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: serde_json::json!({
            "ok": false,
            "error": { "code": code, "hint": hint },
        })
        .to_string(),
    }
}

/// 403 `agent_send_path_only`: an agent without mail-manage.
pub fn refuse_needs_mail_manage(what: &str) -> CliResponse {
    err_json(
        "403 Forbidden",
        AGENT_SEND_PATH_ONLY,
        format!(
            "Agents send mail with `k2 mail send` (and `k2 mail reply`), where the owner's \
             rules apply. Only the owner, or an agent whose workspace may manage hosted mail, \
             can {what}. Ask your human."
        ),
    )
}

/// 403 `owner_only`: an IT agent loosening the rules on its own sends.
pub fn refuse_self_elevation(what: &str) -> CliResponse {
    err_json(
        "403 Forbidden",
        "owner_only",
        format!(
            "An agent can't loosen mail rules on its own sends: {what} for this agent's own \
             workspace needs the owner. Ask your human (the owner runs it from a terminal \
             outside a K2 agent session)."
        ),
    )
}

/// Credential gate for minting/rotating/showing an SMTP-capable secret
/// for `row`. `Ok(creator)` = allowed (`creator` is recorded on the mark).
pub fn credential_gate(
    caller: &MailCaller,
    row: &MailAddress,
    what: &str,
) -> Result<Option<String>, CliResponse> {
    match caller {
        MailCaller::Owner => Ok(None),
        MailCaller::Agent { .. } => Err(refuse_needs_mail_manage(what)),
        MailCaller::ItAgent { workspace_uuid } => {
            if is_own_mailbox(workspace_uuid, row) {
                Err(refuse_self_elevation(&format!(
                    "{what} for {}",
                    row.address
                )))
            } else {
                Ok(Some(workspace_uuid.clone()))
            }
        }
    }
}

/// Gate for deciding (approve/deny) a queued send from `sender_project_id`.
pub fn approval_gate(
    caller: &MailCaller,
    sender_project_id: Option<&str>,
) -> Result<(), CliResponse> {
    match caller {
        MailCaller::Owner => Ok(()),
        MailCaller::Agent { .. } => Err(refuse_needs_mail_manage("approve or reject agent mail")),
        MailCaller::ItAgent { workspace_uuid } => {
            if sender_project_id == Some(workspace_uuid.as_str()) {
                Err(refuse_self_elevation(
                    "approving or rejecting a queued send",
                ))
            } else {
                Ok(())
            }
        }
    }
}

/// Gate for changing agentSend / always-BCC on `target_path`'s workspace.
pub fn policy_gate(caller: &MailCaller, target_path: &str) -> Result<(), CliResponse> {
    match caller {
        MailCaller::Owner => Ok(()),
        MailCaller::Agent { .. } => Err(refuse_needs_mail_manage(
            "change agent mail policy (agentSend, always-BCC)",
        )),
        MailCaller::ItAgent { workspace_uuid } => {
            let own = crate::workspace_msg::resolve_workspace(workspace_uuid);
            if own.as_deref() == Some(target_path) {
                Err(refuse_self_elevation("changing agentSend or always-BCC"))
            } else {
                Ok(())
            }
        }
    }
}

/// The note a mint gets in place of the password when it is withheld.
pub const MINT_WITHHELD_HINT: &str =
    "password withheld: agents send mail with `k2 mail send` and read it with `k2 mail \
     messages`, so they never need it. The owner can set one for Mail.app or IMAP with `k2 \
     hostmail password rotate <addr>` (from a terminal outside a K2 agent session).";

/// Strip the once password out of a mint response. Returns whether a
/// password was removed (a fresh mint, not an idempotent hit).
pub fn withhold_mint_password(v: &mut serde_json::Value) -> bool {
    let removed = v
        .as_object_mut()
        .and_then(|o| o.remove("password"))
        .is_some();
    if removed {
        v["passwordWithheld"] = serde_json::Value::Bool(true);
        v["passwordHint"] = serde_json::Value::String(MINT_WITHHELD_HINT.to_string());
    }
    removed
}

// ── Marks (migration 0133) ──────────────────────────────────────────────

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// One mark row: `(origin, creator_project_id)`.
pub type Mark = (String, Option<String>);

/// Record (or replace) the mark for one credential. Failure is returned,
/// never swallowed — callers log it; the doctor then flags the secret,
/// which is the safe direction.
pub fn mark(
    address_id: &str,
    kind: &str,
    credential_id: &str,
    origin: &str,
    creator: Option<&str>,
) -> Result<(), String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT OR REPLACE INTO mail_credential_marks \
         (address_id, kind, credential_id, origin, creator_project_id, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![address_id, kind, credential_id, origin, creator, now_secs()],
    )
    .map(|_| ())
    .map_err(|e| format!("mail_credential_marks write: {e}"))
}

/// Mark, logging (not failing the request) on a DB error.
pub fn mark_or_log(
    address_id: &str,
    kind: &str,
    credential_id: &str,
    origin: &str,
    creator: Option<&str>,
) {
    if let Err(e) = mark(address_id, kind, credential_id, origin, creator) {
        k2_core::log_debug!("[mail] {e} ({kind} {credential_id} on {address_id})");
    }
}

/// Drop the mark for a revoked app password.
pub fn unmark(address_id: &str, kind: &str, credential_id: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    if let Err(e) = conn.execute(
        "DELETE FROM mail_credential_marks \
         WHERE address_id = ?1 AND kind = ?2 AND credential_id = ?3",
        rusqlite::params![address_id, kind, credential_id],
    ) {
        k2_core::log_debug!("[mail] mail_credential_marks delete: {e}");
    }
}

/// The mark on file for one credential, if any.
pub fn mark_of(address_id: &str, kind: &str, credential_id: &str) -> Option<Mark> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT origin, creator_project_id FROM mail_credential_marks \
         WHERE address_id = ?1 AND kind = ?2 AND credential_id = ?3",
        rusqlite::params![address_id, kind, credential_id],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
    )
    .ok()
}

/// Why the doctor flags a credential, or `None` when it is vouched for:
/// no mark = unknown creator (from before 0.45.0); a mark an agent made
/// for its own mailbox (shown to it — not `withheld`) = self-made.
fn review_reason(row: &MailAddress, mark: Option<&Mark>) -> Option<&'static str> {
    match mark {
        None => Some("unknown creator"),
        Some((origin, Some(creator)))
            if origin != ORIGIN_WITHHELD
                && creator == &row.owner_project_id
                && !is_person_mailbox(&row.id) =>
        {
            Some("an agent made it for its own mailbox")
        }
        Some(_) => None,
    }
}

// ── Doctor ──────────────────────────────────────────────────────────────

/// Every ACTIVE hosted mailbox with a mail-server account.
fn active_hosted_rows() -> Vec<MailAddress> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut stmt = match conn.prepare(
        "SELECT id, address, domain_id, stalwart_account_id, owner_project_id, client_id, \
         status, created_at, retired_at FROM mail_addresses \
         WHERE status = 'active' AND stalwart_account_id IS NOT NULL \
         AND TRIM(stalwart_account_id) != '' ORDER BY address",
    ) {
        Ok(s) => s,
        Err(e) => {
            k2_core::log_debug!("[mail] agent-credentials doctor query: {e}");
            return Vec::new();
        }
    };
    let rows = stmt.query_map([], |r| {
        Ok(MailAddress {
            id: r.get(0)?,
            address: r.get(1)?,
            domain_id: r.get(2)?,
            stalwart_account_id: r.get(3)?,
            owner_project_id: r.get(4)?,
            client_id: r.get(5)?,
            status: r.get(6)?,
            created_at: r.get(7)?,
            retired_at: r.get(8)?,
        })
    });
    match rows {
        Ok(it) => it.filter_map(Result::ok).collect(),
        Err(e) => {
            k2_core::log_debug!("[mail] agent-credentials doctor rows: {e}");
            Vec::new()
        }
    }
}

/// Production doctor entry (server-level run): every active hosted
/// mailbox, app passwords listed through the live engine.
pub fn doctor_credential_checks() -> Vec<DoctorCheck> {
    let rows = active_hosted_rows();
    match crate::mail::domains::engine_from_db() {
        Ok((engine, _)) => vec![credential_check_for(&rows, Some(&engine))],
        Err(_) => vec![credential_check_for(&rows, None)],
    }
}

/// The `agent-credentials` check over `rows` (see [`review_reason`]).
/// `engine = None` (server down) checks mailbox passwords only and reports
/// the app passwords as unknown. Never gates direct mode; never revokes.
pub fn credential_check_for(
    rows: &[MailAddress],
    engine: Option<&dyn AppPasswordEngine>,
) -> DoctorCheck {
    let mut items: Vec<String> = Vec::new();
    let mut list_errors: Vec<String> = Vec::new();
    for row in rows {
        let Some(account_id) = row
            .stalwart_account_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        if let Some(why) = review_reason(row, mark_of(&row.id, KIND_MAILBOX, "").as_ref()) {
            items.push(format!(
                "mailbox password of {} — {why} (rotate: k2 hostmail password rotate {}; keep: \
                 k2 hostmail password keep {})",
                row.address, row.address, row.address
            ));
        }
        let Some(engine) = engine else { continue };
        match engine.list(account_id) {
            Ok(aps) => {
                for ap in aps {
                    let m = mark_of(&row.id, KIND_APP_PASSWORD, &ap.id);
                    if let Some(why) = review_reason(row, m.as_ref()) {
                        items.push(format!(
                            "app password {} '{}' on {} — {why} (revoke: k2 hostmail \
                             app-password revoke {} {}; keep: k2 hostmail app-password keep {} \
                             {})",
                            ap.id,
                            ap.description,
                            row.address,
                            row.address,
                            ap.id,
                            row.address,
                            ap.id
                        ));
                    }
                }
            }
            Err(e) => list_errors.push(format!("{}: {e}", row.address)),
        }
    }

    let preamble = "If you know who has every password that existed before this update, \
                    mark them all reviewed in one go: `k2 hostmail password keep \
                    --all-existing` and `k2 hostmail app-password keep --all-existing` (newer \
                    ones are never covered). Why: agents without mail-manage can no longer get \
                    mail passwords (they send only with `k2 mail send`), and an IT agent can't \
                    get one for its own mailbox; but before 0.45.0 minting a mailbox showed \
                    an agent its password and a mail-manage agent could add app passwords \
                    anywhere, and K2 can't tell who holds these. Nothing was revoked. \
                    Otherwise, one by one, revoke or rotate any you didn't hand out and keep \
                    the ones you did";
    let status;
    let mut detail;
    if items.is_empty() {
        if engine.is_none() {
            status = ST_UNKNOWN;
            detail = "every mailbox password is accounted for; app passwords not checked — \
                      the mail engine is not reachable"
                .to_string();
        } else if !list_errors.is_empty() {
            status = ST_UNKNOWN;
            detail = format!(
                "nothing flagged, but app passwords could not be listed for: {}",
                list_errors.join("; ")
            );
        } else {
            status = ST_PASS;
            detail = "every mailbox password and app password was made, rotated or kept by \
                      the owner or an IT agent for someone else's mailbox (or shown to nobody)"
                .to_string();
        }
    } else {
        status = ST_WARN;
        let shown: Vec<&str> = items
            .iter()
            .take(DETAIL_ITEM_CAP)
            .map(String::as_str)
            .collect();
        detail = format!(
            "{} credential(s) to review. {preamble}: {}",
            items.len(),
            shown.join(" · ")
        );
        if items.len() > DETAIL_ITEM_CAP {
            detail.push_str(&format!(" · and {} more", items.len() - DETAIL_ITEM_CAP));
        }
        if engine.is_none() {
            detail.push_str(" · app passwords not checked — the mail engine is not reachable");
        } else if !list_errors.is_empty() {
            detail.push_str(&format!(
                " · app passwords could not be listed for: {}",
                list_errors.join("; ")
            ));
        }
    }
    DoctorCheck {
        id: CHECK_ID.to_string(),
        label: CHECK_LABEL.to_string(),
        status,
        detail,
        gates_direct: false,
    }
}

// ── POST /cli/mail/credentials/keep ─────────────────────────────────────

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct KeepBody {
    address: String,
    /// Omitted = the mailbox password; set = that app password id.
    app_password_id: Option<String>,
    /// Bulk review: every credential created BEFORE migration 0133 ran.
    all_existing: bool,
    /// With `allExisting`: `mailbox` | `app_password` | `all` (default).
    kind: Option<String>,
}

/// The unix time migration 0133 ran (`mail_credential_baseline`).
pub fn baseline_cutoff() -> Option<i64> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT migrated_at FROM mail_credential_baseline WHERE id = 1",
        [],
        |r| r.get::<_, i64>(0),
    )
    .ok()
}

/// Stalwart's AppPassword `createdAt` as unix seconds: an RFC 3339 string
/// or a number (seconds; milliseconds when it is clearly too large).
/// Anything else is `None` — such a secret is never bulk-kept.
fn created_at_secs(v: &serde_json::Value) -> Option<i64> {
    match v {
        serde_json::Value::String(s) => chrono::DateTime::parse_from_rfc3339(s.trim())
            .ok()
            .map(|d| d.timestamp()),
        serde_json::Value::Number(n) => {
            n.as_i64()
                .map(|x| if x > 100_000_000_000 { x / 1000 } else { x })
        }
        _ => None,
    }
}

/// POST `/cli/mail/credentials/keep` — mark a secret as reviewed so the
/// doctor stops flagging it. `{address, appPasswordId?}` for one;
/// `{allExisting: true, kind?}` for every credential that existed BEFORE
/// migration 0133 ran (never a newer one). Owner or IT agent (mail-manage
/// surface); an IT agent can't clear the flag on its own mailbox's
/// secrets, and the bulk form skips those.
pub fn handle_credentials_keep(body: &[u8]) -> CliResponse {
    let caller = mail_caller();
    if let MailCaller::Agent { .. } = caller {
        return refuse_needs_mail_manage("mark mail passwords as reviewed");
    }
    let b: KeepBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    if b.all_existing {
        if !b.address.trim().is_empty() || b.app_password_id.is_some() {
            return err_json(
                "400 Bad Request",
                "usage",
                "'allExisting' covers every mailbox — don't pass 'address' or 'appPasswordId' \
                 with it"
                    .to_string(),
            );
        }
        let kind = b.kind.as_deref().map(str::trim).unwrap_or("all");
        let (mailboxes, app_passwords) = match kind {
            "all" | "" => (true, true),
            KIND_MAILBOX => (true, false),
            KIND_APP_PASSWORD => (false, true),
            other => {
                return err_json(
                    "400 Bad Request",
                    "usage",
                    format!("unknown kind '{other}' — mailbox, app_password or all"),
                )
            }
        };
        let Some(cutoff) = baseline_cutoff() else {
            return err_json(
                "409 Conflict",
                "not_ready",
                "no credential baseline on file (migration 0133) — restart the daemon".to_string(),
            );
        };
        let rows = active_hosted_rows();
        if !app_passwords {
            return keep_all_existing_on(&caller, None, &rows, cutoff, mailboxes, false);
        }
        return match crate::mail::domains::engine_from_db() {
            Ok((engine, _)) => {
                keep_all_existing_on(&caller, Some(&engine), &rows, cutoff, mailboxes, true)
            }
            Err(hint) => err_json("503 Service Unavailable", "not_ready", hint),
        };
    }
    if b.address.trim().is_empty() {
        return err_json(
            "400 Bad Request",
            "usage",
            "missing 'address' — which hosted mailbox?".to_string(),
        );
    }
    let ap_id = b
        .app_password_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let (row, account_id) = match crate::mail::app_password::lookup_active_hosted(&b.address) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    match ap_id {
        None => keep_on(&caller, None, &row, &account_id, None),
        Some(id) => {
            let (engine, _) = match crate::mail::domains::engine_from_db() {
                Ok(e) => e,
                Err(hint) => return err_json("503 Service Unavailable", "not_ready", hint),
            };
            keep_on(&caller, Some(&engine), &row, &account_id, Some(&id))
        }
    }
}

/// Testable core of the single-credential keep.
pub(crate) fn keep_on(
    caller: &MailCaller,
    engine: Option<&dyn AppPasswordEngine>,
    row: &MailAddress,
    account_id: &str,
    app_password_id: Option<&str>,
) -> CliResponse {
    let creator = match caller {
        MailCaller::Owner => None,
        MailCaller::Agent { .. } => {
            return refuse_needs_mail_manage("mark mail passwords as reviewed")
        }
        MailCaller::ItAgent { workspace_uuid } => {
            if is_own_mailbox(workspace_uuid, row) {
                return refuse_self_elevation(&format!(
                    "clearing the doctor's flag on {}'s passwords",
                    row.address
                ));
            }
            Some(workspace_uuid.as_str())
        }
    };
    let (kind, cred) = match app_password_id {
        None => (KIND_MAILBOX, String::new()),
        Some(id) => {
            let Some(engine) = engine else {
                return err_json(
                    "503 Service Unavailable",
                    "not_ready",
                    "the mail server is not reachable".to_string(),
                );
            };
            let ids = match engine.query_ids(account_id) {
                Ok(ids) => ids,
                Err(e) => return err_json("502 Bad Gateway", "engine", e),
            };
            if !ids.iter().any(|owned| owned == id) {
                return err_json(
                    "404 Not Found",
                    "not_found",
                    format!("no app password '{id}' on '{}'", row.address),
                );
            }
            (KIND_APP_PASSWORD, id.to_string())
        }
    };
    if let Err(e) = mark(&row.id, kind, &cred, ORIGIN_KEPT, creator) {
        return err_json("500 Internal Server Error", "engine", e);
    }
    let ap = if cred.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::Value::String(cred)
    };
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "address": row.address,
            "kind": kind,
            "appPasswordId": ap,
            "kept": true,
        })
        .to_string(),
    )
}

/// Insert a `kept` mark unless one is already on file (never overwrites
/// another mark). Returns whether a row was added.
fn keep_if_unmarked(
    address_id: &str,
    kind: &str,
    credential_id: &str,
    creator: Option<&str>,
) -> Result<bool, String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT OR IGNORE INTO mail_credential_marks \
         (address_id, kind, credential_id, origin, creator_project_id, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            address_id,
            kind,
            credential_id,
            ORIGIN_KEPT,
            creator,
            now_secs()
        ],
    )
    .map(|n| n > 0)
    .map_err(|e| format!("mail_credential_marks write: {e}"))
}

/// Testable core of the bulk review. Marks as `kept` every unmarked
/// mailbox password whose mailbox was created before `cutoff` and every
/// unmarked app password whose Stalwart `createdAt` is before `cutoff`.
/// Newer secrets and app passwords with no readable creation time are left
/// flagged and counted. An IT agent's run skips its own mailboxes (left
/// for the owner). Never revokes; never shows a secret.
pub(crate) fn keep_all_existing_on(
    caller: &MailCaller,
    engine: Option<&dyn AppPasswordEngine>,
    rows: &[MailAddress],
    cutoff: i64,
    mailboxes: bool,
    app_passwords: bool,
) -> CliResponse {
    if let MailCaller::Agent { .. } = caller {
        return refuse_needs_mail_manage("mark mail passwords as reviewed");
    }
    let creator = caller.creator();
    let mut kept_mailboxes = 0u64;
    let mut kept_app_passwords = 0u64;
    let mut newer = 0u64;
    let mut no_created_at = 0u64;
    let mut own_skipped = 0u64;
    let mut list_errors: Vec<String> = Vec::new();
    for row in rows {
        let Some(account_id) = row
            .stalwart_account_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        if let MailCaller::ItAgent { workspace_uuid } = caller {
            if is_own_mailbox(workspace_uuid, row) {
                own_skipped += 1;
                continue;
            }
        }
        if mailboxes {
            if row.created_at < cutoff {
                match keep_if_unmarked(&row.id, KIND_MAILBOX, "", creator) {
                    Ok(true) => kept_mailboxes += 1,
                    Ok(false) => {}
                    Err(e) => return err_json("500 Internal Server Error", "engine", e),
                }
            } else if mark_of(&row.id, KIND_MAILBOX, "").is_none() {
                newer += 1;
            }
        }
        if !app_passwords {
            continue;
        }
        let Some(engine) = engine else {
            return err_json(
                "503 Service Unavailable",
                "not_ready",
                "the mail server is not reachable — app passwords can't be listed".to_string(),
            );
        };
        match engine.list(account_id) {
            Ok(aps) => {
                for ap in aps {
                    if mark_of(&row.id, KIND_APP_PASSWORD, &ap.id).is_some() {
                        continue;
                    }
                    match created_at_secs(&ap.created_at) {
                        Some(t) if t < cutoff => {
                            match keep_if_unmarked(&row.id, KIND_APP_PASSWORD, &ap.id, creator) {
                                Ok(true) => kept_app_passwords += 1,
                                Ok(false) => {}
                                Err(e) => {
                                    return err_json("500 Internal Server Error", "engine", e)
                                }
                            }
                        }
                        Some(_) => newer += 1,
                        None => no_created_at += 1,
                    }
                }
            }
            Err(e) => list_errors.push(format!("{}: {e}", row.address)),
        }
    }
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "allExisting": true,
            "cutoff": cutoff,
            "kept": { "mailboxPasswords": kept_mailboxes, "appPasswords": kept_app_passwords },
            "leftFlagged": {
                "createdAfterCutoff": newer,
                "noCreationTime": no_created_at,
                "ownMailboxesForTheOwner": own_skipped,
            },
            "listErrors": list_errors,
        })
        .to_string(),
    )
}

// ── POST /cli/mail/address/person ───────────────────────────────────────

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default)]
struct PersonBody {
    address: String,
    person: Option<bool>,
    /// Bulk: every eligible mailbox bound to `workspace` (see
    /// [`person_candidates`]) — never its send identities.
    all: bool,
    workspace: Option<String>,
}

/// POST `/cli/mail/address/person` — mark a hosted mailbox as a person's
/// (or back to an agent's): `{address, person}` for one, `{all: true,
/// workspace, person}` for every eligible mailbox bound to a workspace
/// (its send identities are never included). Owner or IT agent; an IT
/// agent can't turn it ON in its own workspace (that would let it get
/// those mailboxes' secrets). Turning it OFF is always allowed.
pub fn handle_address_person(body: &[u8]) -> CliResponse {
    let caller = mail_caller();
    if let MailCaller::Agent { .. } = caller {
        return refuse_needs_mail_manage("mark a mailbox as a person's");
    }
    let b: PersonBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let Some(person) = b.person else {
        return err_json(
            "400 Bad Request",
            "usage",
            "missing 'person' (true = a person's mailbox, false = an agent's)".to_string(),
        );
    };
    if b.all {
        if !b.address.trim().is_empty() {
            return err_json(
                "400 Bad Request",
                "usage",
                "'all' covers every mailbox in 'workspace' — don't pass 'address' with it"
                    .to_string(),
            );
        }
        let Some(ws) = b
            .workspace
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            return err_json(
                "400 Bad Request",
                "usage",
                "'all' needs 'workspace' (name | path | UUID)".to_string(),
            );
        };
        let Some(path) = crate::workspace_msg::resolve_workspace(ws) else {
            return crate::workspace_routes::workspace_not_found_response(ws);
        };
        let Some(workspace_uuid) = project_id_for_path(&path) else {
            return crate::workspace_routes::workspace_not_found_response(ws);
        };
        return set_person_all_on(&caller, &workspace_uuid, person);
    }
    if b.address.trim().is_empty() {
        return err_json(
            "400 Bad Request",
            "usage",
            "missing 'address' — which hosted mailbox?".to_string(),
        );
    }
    let (row, _) = match crate::mail::app_password::lookup_active_hosted(&b.address) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    set_person_on(&caller, &row, person)
}

/// Testable core of [`handle_address_person`].
pub(crate) fn set_person_on(caller: &MailCaller, row: &MailAddress, person: bool) -> CliResponse {
    match caller {
        MailCaller::Agent { .. } => {
            return refuse_needs_mail_manage("mark a mailbox as a person's")
        }
        MailCaller::ItAgent { workspace_uuid }
            if person && row.owner_project_id == *workspace_uuid =>
        {
            return refuse_self_elevation(&format!(
                "marking {} (bound to this agent's workspace) as a person's mailbox",
                row.address
            ))
        }
        _ => {}
    }
    if let Err(e) = set_person_flag(&row.id, person) {
        return err_json("500 Internal Server Error", "engine", e);
    }
    CliResponse::ok_json(
        serde_json::json!({ "ok": true, "address": row.address, "person": person }).to_string(),
    )
}

fn project_id_for_path(path: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT id FROM projects WHERE path = ?1",
        rusqlite::params![path],
        |r| r.get::<_, String>(0),
    )
    .ok()
}

/// The workspace's SEND IDENTITIES: every address its agent has sent from
/// through K2 (`mail_outbound.from_address`, any status). K2 stores no
/// configured "default From" — `k2 mail send` picks the workspace's single
/// sendable address or an explicit `--from` — so the outbox is the only
/// record of which mailboxes are the agent's own sending identity.
pub fn send_identities(workspace_uuid: &str) -> std::collections::BTreeSet<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut stmt = match conn.prepare(
        "SELECT DISTINCT lower(from_address) FROM mail_outbound WHERE owner_project_id = ?1",
    ) {
        Ok(s) => s,
        Err(e) => {
            k2_core::log_debug!("[mail] send identities query: {e}");
            return Default::default();
        }
    };
    let rows = stmt.query_map(rusqlite::params![workspace_uuid], |r| r.get::<_, String>(0));
    match rows {
        Ok(it) => it.filter_map(Result::ok).collect(),
        Err(_) => Default::default(),
    }
}

/// Active hosted mailboxes bound to `workspace_uuid` that a bulk
/// `person … on` would flag: not already a person's and not one of the
/// workspace's send identities — an address its agent has sent from, or a
/// mailbox its agent minted for itself from 0.45.0 (mark `withheld`).
/// Returns `(eligible (id, address), excluded send identities)`.
pub fn person_candidates(workspace_uuid: &str) -> (Vec<(String, String)>, Vec<String>) {
    let identities = send_identities(workspace_uuid);
    let rows: Vec<(String, String)> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let mut stmt = match conn.prepare(
            "SELECT id, address FROM mail_addresses WHERE owner_project_id = ?1 \
             AND status = 'active' AND person_mailbox = 0 ORDER BY address",
        ) {
            Ok(s) => s,
            Err(e) => {
                k2_core::log_debug!("[mail] person candidates query: {e}");
                return (Vec::new(), Vec::new());
            }
        };
        let it = stmt.query_map(rusqlite::params![workspace_uuid], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        });
        match it {
            Ok(it) => it.filter_map(Result::ok).collect(),
            Err(_) => Vec::new(),
        }
    };
    let mut eligible = Vec::new();
    let mut excluded = Vec::new();
    for (id, address) in rows {
        let self_minted = matches!(
            mark_of(&id, KIND_MAILBOX, ""),
            Some((origin, Some(creator))) if origin == ORIGIN_WITHHELD && creator == workspace_uuid
        );
        if self_minted || identities.contains(&address.to_ascii_lowercase()) {
            excluded.push(address);
        } else {
            eligible.push((id, address));
        }
    }
    (eligible, excluded)
}

/// Testable core of the bulk person flag (see [`handle_address_person`]).
/// `on`: flags every [`person_candidates`] mailbox (send identities stay
/// the agent's own). `off`: clears the flag on every person mailbox in
/// the workspace.
pub(crate) fn set_person_all_on(
    caller: &MailCaller,
    workspace_uuid: &str,
    person: bool,
) -> CliResponse {
    match caller {
        MailCaller::Agent { .. } => {
            return refuse_needs_mail_manage("mark a workspace's mailboxes as people's")
        }
        MailCaller::ItAgent {
            workspace_uuid: own,
        } if person && own == workspace_uuid => {
            return refuse_self_elevation(
                "marking this agent's own workspace's mailboxes as people's",
            )
        }
        _ => {}
    }
    let (changed, excluded): (Vec<String>, Vec<String>) = if person {
        let (eligible, excluded) = person_candidates(workspace_uuid);
        for (id, _) in &eligible {
            if let Err(e) = set_person_flag(id, true) {
                return err_json("500 Internal Server Error", "engine", e);
            }
        }
        (eligible.into_iter().map(|(_, a)| a).collect(), excluded)
    } else {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let addrs: Vec<String> = conn
            .prepare(
                "SELECT address FROM mail_addresses WHERE owner_project_id = ?1 \
                 AND person_mailbox = 1 ORDER BY address",
            )
            .and_then(|mut s| {
                s.query_map(rusqlite::params![workspace_uuid], |r| r.get::<_, String>(0))
                    .map(|it| it.filter_map(Result::ok).collect())
            })
            .unwrap_or_default();
        if let Err(e) = conn.execute(
            "UPDATE mail_addresses SET person_mailbox = 0 WHERE owner_project_id = ?1",
            rusqlite::params![workspace_uuid],
        ) {
            return err_json("500 Internal Server Error", "engine", format!("{e}"));
        }
        (addrs, Vec::new())
    };
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "all": true,
            "workspace": workspace_uuid,
            "person": person,
            "changed": changed,
            "excludedSendIdentities": excluded,
        })
        .to_string(),
    )
}

/// Doctor check id for mailboxes an IT agent can't manage yet.
pub const PERSON_CHECK_ID: &str = "person-mailboxes";

/// Production entry: every workspace with mail-manage on (an IT agent's).
pub fn doctor_person_checks() -> Vec<DoctorCheck> {
    let workspaces: Vec<(String, String)> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.prepare("SELECT id, name FROM projects WHERE mail_manage_enabled = 1 ORDER BY name")
            .and_then(|mut s| {
                s.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                    .map(|it| it.filter_map(Result::ok).collect())
            })
            .unwrap_or_default()
    };
    vec![person_check_for(&workspaces)]
}

/// The `person-mailboxes` check over IT workspaces `(id, name)`: mailboxes
/// bound to an IT agent's workspace that aren't marked as people's (and
/// aren't its send identities) are its "own", so the IT agent can't set
/// their passwords. Names the one owner command that flags them all.
pub fn person_check_for(workspaces: &[(String, String)]) -> DoctorCheck {
    let mut parts = Vec::new();
    for (id, name) in workspaces {
        let (eligible, _) = person_candidates(id);
        if !eligible.is_empty() {
            parts.push(format!(
                "{} mailbox(es) in '{name}' ({}) — if they are people's mailboxes (e.g. \
                 imported staff), run once as the owner: k2 hostmail person --all \
                 --workspace {name} on (its agent's send identities are left out)",
                eligible.len(),
                eligible
                    .iter()
                    .take(5)
                    .map(|(_, a)| a.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    let (status, detail) = if parts.is_empty() {
        (
            ST_PASS,
            "every mailbox in an IT agent's workspace is a person's or the agent's own send \
             identity"
                .to_string(),
        )
    } else {
        (
            ST_INFO,
            format!(
                "An IT agent can't set passwords for its own workspace's mailboxes unless they \
                 are marked as people's: {}",
                parts.join(" · ")
            ),
        )
    };
    DoctorCheck {
        id: PERSON_CHECK_ID.to_string(),
        label: "People's mailboxes in IT workspaces".to_string(),
        status,
        detail,
        gates_direct: false,
    }
}

/// Write `mail_addresses.person_mailbox`.
pub fn set_person_flag(address_id: &str, person: bool) -> Result<(), String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "UPDATE mail_addresses SET person_mailbox = ?1 WHERE id = ?2",
        rusqlite::params![i64::from(person), address_id],
    )
    .map(|_| ())
    .map_err(|e| format!("person_mailbox write: {e}"))
}

#[cfg(test)]
mod tests;
