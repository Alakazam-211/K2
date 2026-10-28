//! Agent email notices (prd-email-notifications-part2-v1 §4 N10a; vs-live
//! EN-A4–EN-A7): an agent, or one of its routines, emails **a person on
//! this box** ("the nightly build failed", "3 invoices need you").
//!
//! ```text
//! POST /cli/notify/email                      {to, subject, text}  the workspace's agent (its passport or
//!                                                                  a shell tab in it): send one notice
//! POST /cli/notify/email/off                  {}                   the workspace's agent: turn its own off
//! GET  /cli/notifications/email/agents                             the owner, Admins
//! POST /cli/notifications/email/agent/on      {workspace, sender, confirmToken}  the owner in person + confirm
//! POST /cli/notifications/email/agent/off     {workspace}          the owner, Admins
//! POST /cli/notify/email/mute                 {workspace, muted}   a person: stop (or restart) one
//!                                                                  workspace's notices to them
//! ```
//!
//! - **Who can receive:** the owner and the box's people (Connect logins)
//!   with a confirmed notice address (slice V, `person_notice`). Nobody
//!   else: no App users, no outside addresses.
//! - **Sender:** the workspace's own send-only sender (granted once by the
//!   owner with a confirm). Never the system sender, never an App's, never
//!   the agent's mailbox.
//! - **Switch:** per workspace, off by default. Only the owner turns it on;
//!   the agent may turn it off. An agent asks with a permission card.
//! - **Text only:** subject ≤ 200 characters, body ≤ 16 KB, no HTML, no
//!   Reply-To, no headers. K2 adds a fixed footer (which agent, which
//!   server, the stop link) and `List-Unsubscribe` one-click (RFC 8058).
//! - **Budget:** 10 an hour and 50 a day per workspace, the box's, and the
//!   warm-up of a new domain. Auto-pause and suppressions as App senders.

use k2_core::email_notify::{self as store, Sender};
use k2_core::email_verify as verified;
use serde_json::{json, Value as J};

use super::{db_fail, fail, Fail, R};
use crate::compute_routes::Who;

pub const SEND: &str = "/cli/notify/email";
pub const OFF: &str = "/cli/notify/email/off";
pub const MUTE: &str = "/cli/notify/email/mute";

pub const MAX_SUBJECT_CHARS: usize = 200;
pub const MAX_TEXT_BYTES: usize = 16 * 1024;

/// The agent routes (passports): served on the cell socket and over TCP.
pub fn is_agent_route(p: &str) -> bool {
    p == SEND || p == OFF
}

fn usage(h: impl Into<String>) -> Fail {
    fail(400, "usage", h)
}

/// The workspace an agent call speaks for (its passport, or a plain shell
/// tab in it: L1, the workspace's agent). People and other credentials
/// aren't agents here.
fn agent_workspace(who: &Who) -> R<String> {
    match who {
        Who::Agent { project_id, .. } | Who::Shell { project_id, .. } => Ok(project_id.clone()),
        Who::Owner | Who::Login { .. } => Err(fail(
            403,
            "agent_only",
            "k2 notify email is an agent's way to email a person here; people use their own mail",
        )),
        _ => Err(fail(403, "forbidden", "invalid or missing token")),
    }
}

fn off_hint(project: &str) -> String {
    format!(
        "email notices are off for this workspace. Only the owner turns them on, with a sender for it: \
         k2 notifications email agent on {project} --sender <address> (from their own terminal). Ask with a permission card."
    )
}

fn workspace_path(project: &str) -> R<String> {
    crate::workspace_msg::resolve_workspace(project).ok_or_else(|| fail(404, "not_found", "the workspace is gone"))
}

/// `(person key, label)` for `to`: `owner`, or a login on this box.
fn person_for(to: &str) -> R<(String, String)> {
    let to = to.trim();
    if to.is_empty() {
        return Err(usage("send {to: <a login on this server, or owner>}"));
    }
    if to.eq_ignore_ascii_case(k2_core::notify_prefs::OWNER) {
        return Ok((k2_core::notify_prefs::OWNER.to_string(), "the owner".to_string()));
    }
    if to.contains('@') {
        return Err(fail(
            400,
            "person_only",
            "notices go to a person on this server by their login (or owner), never to an email address",
        ));
    }
    let people = k2_core::notify_prefs::People::load().map_err(db_fail)?;
    if people.user(to).is_none() {
        return Err(fail(404, "no_such_person", format!("{to} isn't a login on this server (or say owner)")));
    }
    let key = people.login_person_key(to).ok_or_else(|| fail(404, "no_such_person", format!("{to} can't get notices")))?;
    Ok((key, to.to_ascii_lowercase()))
}

/// The workspace's sender, on and able to send.
fn workspace_sender(project: &str, now: i64) -> R<Sender> {
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::agent_sender(&conn, project).map_err(db_fail)?
    };
    let row = row.filter(|r| r.enabled).ok_or_else(|| fail(403, "agent_email_off", off_hint(project)))?;
    let s = super::senders::find_live(&row.sender_id).map_err(|_| fail(409, "no_sender", "this workspace's notice sender was removed; the owner grants another"))?;
    super::senders::ready(&s, now)?;
    super::outcomes::refuse_if_paused(&s)?;
    Ok(s)
}

/// The fixed footer every notice carries.
pub fn footer(agent: &str, box_label: &str, stop_url: &str) -> String {
    format!(
        "\n\n--\nSent by {agent}, an AI agent on {box_label}, through K2. Replies aren't read.\n\
         Stop {agent}'s emails to this address: {stop_url}\n"
    )
}

/// POST `/cli/notify/email`.
pub fn send(who: &Who, v: &J, now: i64) -> R<J> {
    let project = agent_workspace(who)?;
    if !super::box_enabled() {
        return Err(fail(403, "email_off", "email notifications are off on this server"));
    }
    if super::airgap_blocks() {
        return Err(fail(503, "airgap", "air-gap is on: nothing leaves this server"));
    }
    if let Some(o) = v.as_object() {
        if let Some(k) = o.keys().find(|k| !matches!(k.as_str(), "to" | "subject" | "text")) {
            return Err(usage(format!("{k} isn't allowed: a notice is {{to, subject, text}} (text only, no HTML, no Reply-To, no headers)")));
        }
    }
    let to = v.get("to").and_then(J::as_str).unwrap_or("");
    let subject = v.get("subject").and_then(J::as_str).map(str::trim).unwrap_or("");
    let text = v.get("text").and_then(J::as_str).unwrap_or("");
    if subject.is_empty() || subject.chars().count() > MAX_SUBJECT_CHARS || subject.chars().any(char::is_control) {
        return Err(usage(format!("subject: 1–{MAX_SUBJECT_CHARS} characters on one line")));
    }
    if text.trim().is_empty() || text.len() > MAX_TEXT_BYTES {
        return Err(usage(format!("text: the notice itself, at most {} KB", MAX_TEXT_BYTES / 1024)));
    }
    let (person, label) = person_for(to)?;
    let s = workspace_sender(&project, now)?;
    let (address, address_id) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if store::notice_muted(&conn, &person, &project) {
            return Err(fail(403, "person_muted", format!("{label} turned this workspace's email notices off")));
        }
        match verified::verified_for(&conn, verified::PURPOSE_PERSON_NOTICE, &person).map_err(db_fail)? {
            Some(v) => (v.address, v.id),
            None => {
                return Err(fail(
                    409,
                    "no_notice_address",
                    format!("{label} hasn't confirmed a notice address yet (they run k2 notifications email address set <address> themselves)"),
                ))
            }
        }
    };
    let to_list = vec![address.clone()];
    let (keep, dropped) = super::outcomes::split_suppressed(&s, None, &to_list);
    if keep.is_empty() {
        return Err(super::outcomes::all_suppressed());
    }
    let path = workspace_path(&project)?;
    let agent = k2_core::workspace::display::agent_display_name(&path);
    let box_label = crate::app_user_invites::box_label();
    let warm = super::budget::warming_up(&s, now).then_some(s.domain.as_str());
    let id = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        super::budget::check(&super::budget::Kind::Agent { project: &project, warmup_domain: warm }, keep.len(), now)?;
        let tx = conn.unchecked_transaction().map_err(db_fail)?;
        let id = store::insert_log(
            &tx,
            &store::NewLog {
                sender_id: &s.id,
                origin: store::ORIGIN_AGENT,
                app_service_id: None,
                purpose: Some(store::PURPOSE_AGENT_NOTICE),
                to: &to_list,
                subject: &crate::mail::send::hide_links(subject),
                tags: &[],
                status: store::ST_QUEUED,
                error_code: None,
                message_id: None,
                send_after: None,
                scope_any: false,
                now,
            },
        )
        .map_err(db_fail)?;
        store::set_log_project(&tx, &id, &project).map_err(db_fail)?;
        tx.commit().map_err(db_fail)?;
        id
    };
    super::outcomes::record_suppressed(&id, &s, &dropped, now);
    let stop = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        verified::mint_stop(&conn, &address_id, now).map_err(db_fail)?
    };
    let stop_url = super::verify::link(super::verify::STOP_PATH, &stop)?;
    let body = format!("{}{}", text.trim_end(), footer(&agent, &box_label, &stop_url));
    let mut headers = std::collections::BTreeMap::new();
    headers.insert("List-Unsubscribe".to_string(), format!("<{stop_url}>"));
    headers.insert("List-Unsubscribe-Post".to_string(), "List-Unsubscribe=One-Click".to_string());
    let out = super::send::deliver(&s, &id, &agent, &to_list, &[], &keep, subject, Some(body.as_str()), None, &[], &headers);
    let row = super::send::finish(&id, store::ST_QUEUED, &out, now)?;
    out?;
    super::audit_line("email-agent-notice", &format!("agent:{project}"), &format!("log={id} person={person}"));
    Ok(json!({ "ok": true, "accepted": true, "id": row.id, "status": row.status, "to": label }))
}

/// POST `/cli/notify/email/off`: the agent turns its own workspace's
/// notices off (only the owner turns them back on).
pub fn agent_off(who: &Who, now: i64) -> R<J> {
    let project = agent_workspace(who)?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    if !store::set_agent_off(&conn, &project, "off by its agent", now).map_err(db_fail)? {
        return Err(fail(404, "not_found", "this workspace has no email notices to turn off"));
    }
    drop(conn);
    super::audit_line("email-agent-off", &format!("agent:{project}"), "by its agent");
    Ok(json!({ "ok": true, "on": false }))
}

/// The agent routes ([`SEND`], [`OFF`]): POST only, JSON body.
pub fn handle(path: &str, who: Who, is_post: bool, body: &[u8]) -> crate::cli_response::CliResponse {
    if !is_post {
        return crate::cli_response::CliResponse::method_not_allowed();
    }
    let v = if body.iter().all(|c| c.is_ascii_whitespace()) {
        json!({})
    } else {
        match serde_json::from_slice::<J>(body) {
            Ok(v @ J::Object(_)) => v,
            _ => return super::fail_response(&usage("the body is a JSON object")),
        }
    };
    let now = super::now();
    super::respond(match path {
        SEND => send(&who, &v, now),
        OFF => agent_off(&who, now),
        _ => Err(fail(404, "not_found", "no such route")),
    })
}

/// `{workspace}` → (project id, path).
pub fn resolve_workspace(raw: &str) -> R<(String, String)> {
    let path = crate::workspace_msg::resolve_workspace(raw.trim()).ok_or_else(|| fail(404, "not_found", format!("no workspace {}", raw.trim())))?;
    let id = crate::clone_vault::project_id_for_path(&path).ok_or_else(|| fail(404, "not_found", format!("no workspace {}", raw.trim())))?;
    Ok((id, path))
}

/// The `secretId` the `agent-email-on` confirm binds: the workspace and
/// the sender.
pub fn on_binding(project_id: &str, sender_address: &str) -> String {
    format!("{project_id}|{}", sender_address.trim().to_ascii_lowercase())
}

/// Owner: give `sender` to the workspace and turn its notices on. The
/// route checked the person and the confirm.
pub fn agent_on(project_id: &str, sender: &Sender, by: &str, now: i64) -> R<J> {
    if sender.is_system() {
        return Err(fail(409, "system_sender", "the system sender is K2's own; a workspace gets its own sender"));
    }
    if let Some(app) = sender.app() {
        return Err(fail(409, "sender_in_use", format!("{} is App {app}'s sender (one owner per address)", sender.address)));
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    if let Some(other) = store::agent_for_sender(&conn, &sender.id).map_err(db_fail)?.filter(|p| p != project_id) {
        return Err(fail(409, "sender_in_use", format!("{} is workspace {other}'s notice sender (one owner per address)", sender.address)));
    }
    store::set_agent_on(&conn, project_id, &sender.id, by, now).map_err(db_fail)?;
    drop(conn);
    super::audit_line("email-agent-on", by, &format!("workspace={project_id} sender={}", sender.id));
    if workspace_path(project_id).is_ok() {
        crate::apps::alerts::note(
            project_id,
            "Email notices on",
            &format!(
                "The owner turned email notices on for this workspace from {}. Email a person here: \
                 k2 notify email <their login, or owner> --subject \"…\" --text \"…\" (text only; they must have confirmed a notice address).",
                sender.address
            ),
        );
    }
    Ok(json!({ "ok": true, "workspace": project_id, "sender": sender.address, "on": true }))
}

/// Owner/Admin: turn a workspace's notices off.
pub fn owner_off(project_id: &str, by: &str, now: i64) -> R<J> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    if !store::set_agent_off(&conn, project_id, &format!("off by {by}"), now).map_err(db_fail)? {
        return Err(fail(404, "not_found", "that workspace has no email notices"));
    }
    drop(conn);
    super::audit_line("email-agent-off", by, &format!("workspace={project_id}"));
    Ok(json!({ "ok": true, "workspace": project_id, "on": false }))
}

/// Owner/Admin: every workspace's notice switch.
pub fn list() -> R<J> {
    let rows = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::list_agent_senders(&conn).map_err(db_fail)?
    };
    let items: Vec<J> = rows
        .iter()
        .map(|r| {
            let sender = super::senders::find_live(&r.sender_id).map(|s| s.address).ok();
            json!({ "workspace": r.project_id, "sender": sender, "on": r.enabled, "offReason": r.off_reason, "grantedAt": r.granted_at })
        })
        .collect();
    Ok(json!({ "ok": true, "agents": items }))
}

/// A person mutes (or unmutes) one workspace's notices to them.
pub fn mute(person: &str, v: &J, now: i64) -> R<J> {
    let raw = v.get("workspace").and_then(J::as_str).ok_or_else(|| usage("send {workspace, muted}"))?;
    let muted = v.get("muted").and_then(J::as_bool).ok_or_else(|| usage("send {workspace, muted: true|false}"))?;
    let (project, _) = resolve_workspace(raw)?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::set_notice_mute(&conn, person, &project, muted, now).map_err(db_fail)?;
    Ok(json!({ "ok": true, "workspace": project, "muted": muted }))
}
