//! The `approval` card on the Thread between the agent and the person, its
//! frames, the push to the person's phones and the agent's outcome line.
//!
//! The card and the push carry ids, state and K2's generic words only
//! (SA-15). The payload's words are worked out on the person's device from
//! the bytes (`GET /cli/approve/get`), never by the daemon.

use k2_core::approve_requests::{ApprovalRequest, State};
use k2_core::overlay::{self, ApprovalCardBody};
use k2_core::push::{PushEvent, ThreadBudget, ThreadMsgKind, ThreadPush};

use super::people;
use k2_core::brand::CLI_NAME;

/// The asking workspace's Thread address (its handle), or its id.
pub(crate) fn agent_addr(workspace_id: &str) -> String {
    k2_core::workspace_session_handles::workspace_address_name_shared(workspace_id).unwrap_or_else(|_| workspace_id.to_string())
}

fn body_of(req: &ApprovalRequest, addr: &str) -> ApprovalCardBody {
    ApprovalCardBody {
        request_id: req.id.clone(),
        status: req.state.as_str().to_string(),
        requested_by: addr.to_string(),
        person: req.person_login.clone(),
        answer_by: req.answer_by,
        device: req.device.clone(),
        answered_via: req.answered_via.clone(),
        answered_at: req.answered_at,
    }
}

/// Post the card on the agent's pinned Thread and push the person's
/// phones. Returns `(conversation_id, card_id)` when a card went up (a
/// workspace with no pinned Thread yet gets no card; the push and the
/// person's pending list still reach them).
pub(crate) fn post(req: &ApprovalRequest) -> Option<(String, String)> {
    let addr = agent_addr(&req.workspace_id);
    let posted = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let conv = match k2_core::db::schema::WorkspaceSession::get(&conn, &req.workspace_id) {
            Ok(s) => s.and_then(|s| s.session_id).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
            Err(e) => {
                k2_core::log_debug!("[approve] pinned Thread for {}: {e}", req.workspace_id);
                None
            }
        };
        match conv {
            None => None,
            Some(conv) => {
                match overlay::post_approval(&conn, &conv, &req.workspace_id, &addr, &req.person_login, body_of(req, &addr)) {
                    Ok((item, links)) => {
                        // Publish under the write's lock (bus order = commit order).
                        crate::overlay_ws::emit_links(&links, &item.doc);
                        if let Err(e) = k2_core::approve_requests::set_card(&conn, &req.id, &conv, &item.id) {
                            k2_core::log_debug!("[approve] card ref for {}: {e}", req.id);
                        }
                        Some((conv, item.id, item.seq))
                    }
                    Err(e) => {
                        k2_core::log_debug!("[approve] card for {}: {e}", req.id);
                        None
                    }
                }
            }
        }
    };
    let (conv, seq) = posted.as_ref().map(|(c, _, s)| (c.clone(), *s)).unwrap_or_default();
    push(req, &addr, &conv, seq);
    crate::badge_push::trigger_everyone();
    posted.map(|(c, id, _)| (c, id))
}

/// One push to the asked person's own phones (their login's devices), from
/// the per-person card bucket. Generic words only.
fn push(req: &ApprovalRequest, addr: &str, conversation_id: &str, seq: i64) {
    let devices: Vec<String> = match k2_core::push_devices::list_devices_for_usernames(std::slice::from_ref(&req.person_login)) {
        Ok(d) => d.into_iter().map(|d| d.username).collect(),
        Err(e) => {
            k2_core::log_debug!("[approve] push devices for {}: {e}", req.person_login);
            return;
        }
    };
    if devices.is_empty() {
        return;
    }
    let mut device_usernames = devices;
    device_usernames.sort();
    device_usernames.dedup();
    let (agent_color, avatar_rev) = k2_core::projects_ops::project_push_look(&req.workspace_id);
    crate::push_routes::deliver_thread(PushEvent::ThreadPost(ThreadPush {
        agent_name: addr.to_string(),
        conversation_id: conversation_id.to_string(),
        addr: addr.to_string(),
        seq,
        project_id: req.workspace_id.clone(),
        msg_kind: ThreadMsgKind::Choice,
        count: 1,
        principal: people::person_key(&req.person_login),
        device_usernames,
        unread_total: String::new(),
        waiting_total: String::new(),
        avatar_rev,
        agent_color,
        preview: String::new(),
        msg: None,
        budget: ThreadBudget::Approval { request_id: req.id.clone() },
    }));
}

/// Draw the request's final state on its card (every open view updates).
/// A card already closed, or none, is a no-op.
pub(crate) fn close(req: &ApprovalRequest) {
    let (Some(conv), Some(card)) = (req.conversation_id.as_deref(), req.card_id.as_deref()) else { return };
    match overlay::mark_approval(conv, card, req.state.as_str(), req.device.clone(), req.answered_via.clone()) {
        Ok(cb) => crate::overlay_routes::publish_card_frame(&cb),
        Err(e) if e == overlay::CARD_NOT_PENDING => {}
        Err(e) => k2_core::log_debug!("[approve] close card {card}: {e}"),
    }
}

/// The agent's one outcome line, unless a `--wait` is already holding the
/// answer for it.
pub(crate) fn outcome_line(req: &ApprovalRequest) -> Option<String> {
    let by = &req.person_login;
    let what = match req.state {
        State::Approved => format!(
            "{by} approved it{}. Get the approval record with: {CLI_NAME} approve status {} --out approval.json",
            req.device.as_deref().map(|d| format!(" on {d}")).unwrap_or_default(),
            req.id
        ),
        State::Declined => format!("{by} declined it. Don't ask again for the same thing unless they tell you to."),
        State::Expired => format!("{by} didn't answer before it expired."),
        State::Waiting | State::Cancelled => return None,
    };
    Some(format!("[approval:{}] {what}", req.id))
}

/// Tell the agent how its request ended (its own address), off the
/// caller's thread. Skipped while a `--wait` holds the request.
pub(crate) fn tell_agent(req: &ApprovalRequest) {
    if super::wait::is_waited(&req.id) {
        return;
    }
    let Some(line) = outcome_line(req) else { return };
    deliver(&agent_addr(&req.workspace_id), &line);
}

#[cfg(test)]
static SINK: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());

/// Lines sent to `addr` (tests only).
#[cfg(test)]
pub(crate) fn sent_to(addr: &str) -> Vec<String> {
    SINK.lock().expect("approve sink").iter().filter(|(a, _)| a == addr).map(|(_, l)| l.clone()).collect()
}

#[cfg(test)]
fn deliver(addr: &str, line: &str) {
    SINK.lock().expect("approve sink").push((addr.to_string(), line.to_string()));
}

#[cfg(not(test))]
fn deliver(addr: &str, line: &str) {
    let addr = addr.to_string();
    let line = line.to_string();
    let spawned = crate::rt::spawn("approval-turn", move || {
        let r = crate::workspace_msg::deliver_live_with_via(&addr, &line, "k2", "", true, crate::workspace_msg::DEFAULT_WAKE_TIMEOUT, "thread");
        if !r.success {
            k2_core::log_debug!("[approve] outcome line to {addr} not delivered: {:?}", r.reason);
        }
    });
    if let Err(e) = spawned {
        k2_core::log_debug!("[approve] could not start the outcome line: {e}");
    }
}
