//! "Reach a department" (prd-org-chart-v1 §6.4, vs-live A4): an agent
//! needs an org's head and asks with `k2 organization reach <org|head>
//! --why "…"`.
//!
//! 1. **Already connected** (`connections::are_local_peers`): it gets the
//!    `k2 msg` line; nothing is filed.
//! 2. **Not connected, permission cards on for it**: the existing
//!    `connections.add to=<head>` card is filed for it (an Admin approves,
//!    the card's apply adds the connection).
//! 3. **Otherwise**: the daemon files a Ticket assigned to the server's
//!    Owner and Admins (`file_owner_notice`), with the exact fix.
//!
//! **Closing the ask:** a connection between the two added by any path
//! (card, Settings, `k2 connections add`, team links, a raw insert) marks
//! the ask `connected` (a trigger on `workspace_relations`, migration
//! 0283). [`tell_pass`] (on the heartbeat tick) then tells the agent once,
//! resolves the ticket, and turns a denied card or a dismissed ticket into
//! one "declined" line. `reach` never adds a connection itself.

use std::collections::HashMap;

use k2_core::org_chart::store::{self, Org};
use k2_core::org_chart::{self as oc, text, OrgError};
use serde_json::{json, Map, Value};

use super::caller::refusal;
use super::routes::fail;
use crate::cli_response::CliResponse;

fn s<'a>(m: &'a Map<String, Value>, k: &str) -> Option<&'a str> {
    m.get(k).and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty())
}

/// The org and head a token names: an org (its head), or an agent that
/// heads an org.
fn target(token: &str) -> Result<(Org, String), CliResponse> {
    let org = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::lookup_org(&conn, token.strip_prefix("org:").unwrap_or(token)).map_err(|e| fail(&e))?
    };
    let org = match org {
        Some(o) => o,
        None => {
            let agent = super::routes::resolve_agent(token)?;
            let db = k2_core::db::shared();
            let conn = db.lock();
            store::headed_by(&conn, &agent).map_err(|e| fail(&e))?.into_iter().next().ok_or_else(|| {
                refusal(
                    "400 Bad Request",
                    "not_a_head",
                    &format!(
                        "{} doesn't head an org. Reach an org by name (k2 organization bulletin lists them and their heads).",
                        store::agent_name(&conn, &agent)
                    ),
                )
            })?
        }
    };
    let Some(head) = org.head.clone() else {
        return Err(refusal(
            "409 Conflict",
            "no_head",
            &format!("{} has no head yet. Ask your person, or an Owner or Admin sets one (k2 organization head {} <agent>).", org.name, org.name),
        ));
    };
    Ok((org, head))
}

fn msg_line(head: &str) -> String {
    format!("k2 msg {head} \"<your message>\"")
}

/// `POST /cli/organization/reach` for the agent `from`.
pub fn reach(from: &str, session_id: &str, m: &Map<String, Value>, now: i64) -> Result<CliResponse, CliResponse> {
    let token = s(m, "org").or_else(|| s(m, "head")).or_else(|| s(m, "to")).ok_or_else(|| {
        refusal("400 Bad Request", "usage", "name the org (or its head): k2 organization reach <org> --why \"<one line>\"")
    })?;
    let why = text::capped(s(m, "why").unwrap_or(""), text::WHY_MAX_CHARS, "--why", "why_required").map_err(|e| {
        fail(&OrgError::new(e.status, e.code, format!("{} Say in one line why you need them: --why \"…\"", e.hint)))
    })?;
    let (org, head) = target(token)?;
    let head_name = super::caller_handle(&head);
    let base = json!({ "org": { "id": org.id, "name": org.name }, "head": { "id": head, "handle": head_name } });
    let answer = |status: &str, extra: Value| {
        let mut v = base.clone();
        v["ok"] = json!(true);
        v["status"] = json!(status);
        if let (Some(o), Value::Object(e)) = (v.as_object_mut(), extra) {
            o.extend(e);
        }
        CliResponse::ok_json(v.to_string())
    };
    if head == from {
        return Ok(answer("self", json!({ "text": format!("You are {}'s head.", org.name) })));
    }
    if k2_core::connections::are_local_peers(from, &head) {
        return Ok(answer(
            "connected",
            json!({ "msg": msg_line(&head_name), "text": format!("You can message {}'s head: {}", org.name, msg_line(&head_name)) }),
        ));
    }
    let open = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::open_reach(&conn, from, &head).map_err(|e| fail(&e))?
    };
    if let Some(r) = open {
        return Ok(answer(
            "open",
            json!({
                "via": r.via, "reachId": r.id, "since": r.created_at,
                "text": format!("You already asked to be connected to {head_name} ({}'s head). K2 tells you when it's done; don't ask again.", org.name),
            }),
        ));
    }
    let me = super::caller_handle(from);
    // 2. The connections.add card, when permission cards are on for it.
    if k2_core::permission::gate::server_on() && k2_core::permission::gate::agent_switch_on(from) {
        let body = json!({ "action": "connections.add", "params": { "to": head_name }, "why": why }).to_string();
        let who = crate::compute_routes::Who::Agent { project_id: from.to_string(), session_id: session_id.to_string() };
        let r = crate::permission::routes::handle_gated(crate::permission::routes::REQUEST, &who, true, body.as_bytes(), &HashMap::new());
        let v: Value = serde_json::from_str(&r.body).unwrap_or(Value::Null);
        if r.status.starts_with('2') {
            let request_id = v.get("id").and_then(Value::as_str).map(str::to_string);
            let row = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                store::insert_reach(&conn, from, &head, &org.id, &why, "card", request_id.as_deref(), None, now).map_err(|e| fail(&e))?
            };
            audit(from, &format!("card {} -> {head_name}", org.name));
            return Ok(answer(
                "card",
                json!({
                    "via": "card", "reachId": row.id, "request": v,
                    "text": format!("Asked an Admin to connect you to {head_name} ({}'s head) on a permission card. K2 tells you when it's done; you don't need to ask again.", org.name),
                }),
            ));
        }
        k2_core::log_debug!("[org-chart] reach card refused ({}): {}; filing a ticket", r.status, r.body);
    }
    // 3. A ticket for the server's Owner and Admins.
    let title = format!("Connect {me} to {head_name} (head of {})", org.name);
    let body = format!(
        "{me} needs to reach {head_name}, the head of {} on the Org Chart, and the two aren't connected.\n\nWhy: {why}\n\nTo allow it, add the connection in Settings → Connections, or run `k2 connections add {head_name} --workspace {me}`. K2 tells {me} once it's done. Org membership never connects agents by itself.",
        org.name
    );
    let item = crate::feedback_routes::file_owner_notice(from, &me, &title, &body)
        .map_err(|e| fail(&OrgError::new(500, "internal", format!("couldn't file the ticket: {e}"))))?;
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::insert_reach(&conn, from, &head, &org.id, &why, "ticket", None, Some(&item.id), now).map_err(|e| fail(&e))?
    };
    audit(from, &format!("ticket {} -> {head_name}", org.name));
    Ok(answer(
        "ticket",
        json!({
            "via": "ticket", "reachId": row.id, "ticket": item.id,
            "text": format!("Asked an Admin (a ticket) to connect you to {head_name} ({}'s head). K2 tells you when it's done; you don't need to ask again.", org.name),
        }),
    ))
}

fn audit(from: &str, detail: &str) {
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
        "org_chart.reach",
        &format!("agent:{}", super::caller_handle(from)),
        detail.to_string(),
        "-",
        "-",
        "cli",
    ));
}

/// What [`tell_pass`] did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TellReport {
    pub connected: usize,
    pub declined: usize,
    pub capped: usize,
}

fn card_closed_without_yes(request_id: &str) -> bool {
    use k2_core::permission::request::RequestState as S;
    let req = crate::permission::store::get_by_display_id(request_id)
        .ok()
        .flatten()
        .or_else(|| crate::permission::store::get(request_id).ok().flatten());
    req.is_some_and(|r| matches!(r.state, S::Denied | S::Expired | S::Stale | S::Failed | S::Cancelled | S::Closed))
}

fn ticket_closed(feedback_id: &str) -> bool {
    k2_core::feedback::get_item(feedback_id).is_some_and(|f| matches!(f.status.as_str(), "resolved" | "dismissed"))
}

/// Close what was decided and tell each asking agent once.
pub fn tell_pass(w: &dyn super::Waker, now: i64) -> TellReport {
    let mut rep = TellReport::default();
    if !oc::enabled() {
        return rep;
    }
    // Open asks an Admin said no to (a denied card, a closed ticket).
    let open = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::open_reaches(&conn).unwrap_or_default()
    };
    for r in open {
        let closed = match (r.via.as_str(), &r.permission_request_id, &r.feedback_id) {
            ("card", Some(id), _) => card_closed_without_yes(id),
            ("ticket", _, Some(id)) => ticket_closed(id),
            _ => false,
        };
        if closed {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = store::set_reach_state(&conn, &r.id, "declined", now);
        }
    }
    let due = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::reach_to_tell(&conn).unwrap_or_default()
    };
    for r in due {
        let head = super::caller_handle(&r.to);
        let org = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            store::get_org(&conn, &r.org_id).ok().flatten().map(|o| o.name).unwrap_or_else(|| "its org".into())
        };
        let line = if r.state == "connected" {
            format!("You're now connected to {head} ({org}'s head): {}", msg_line(&head))
        } else {
            format!("Your ask to be connected to {head} ({org}'s head) was declined. Ask your person, or work without them.")
        };
        match super::wake(w, &r.from, store::WAKE_REACH, "org", &line, now) {
            super::WakeOutcome::Capped => {
                rep.capped += 1;
                continue;
            }
            super::WakeOutcome::Delivered | super::WakeOutcome::Failed(_) => {}
        }
        if r.state == "connected" {
            rep.connected += 1;
            if let Some(fb) = &r.feedback_id {
                if let Err(e) = k2_core::feedback::set_status(fb, "resolved") {
                    k2_core::log_debug!("[org-chart] resolve reach ticket {fb}: {e}");
                }
            }
        } else {
            rep.declined += 1;
        }
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = store::mark_reach_told(&conn, &r.id, now);
    }
    rep
}
