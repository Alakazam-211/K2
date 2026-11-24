//! `project:<id>` on the Thread routes (project-chat-thread-v1).
//!
//! A project's chat is the Thread conversation `project:<group id>`
//! (`k2_core::overlay::project`). Its address takes the group's id, its
//! exact name or a unique prefix (as `k2 project`; a name may hold `:` or
//! spaces), and the conversation id is always `project:<group id>`, so a
//! rename never moves it. Every Thread route that resolves an address takes
//! it: reads and deltas, the overlay socket, typing, viewing, read marks,
//! posts, attachments and deleting your own line.
//!
//! **Access is the project's, never the overlay's own-workspace rule:**
//! - a person (the owner token, a Connect login at Member or above, a
//!   person in a K2 shell tab) reads and posts every project;
//! - an agent passport reads and posts only in projects its workspace is a
//!   member of (`project_group_routes::agent_chat_refusal`, `not_a_member`),
//!   always as its own workspace;
//! - an App pass reads under the all-members rule (`thread:read` in
//!   **every** member room, else the uniform `skin_room`) and posts with
//!   `project:write` in every member room (`apps::project_chat`). Its live
//!   frames come from `project.events`; the overlay socket refuses it.
//!
//! The catalog's `project_id` for a project chat is the GROUP id, never a
//! workspace: [`authorize_read`] / [`authorize_write`] and the App cap
//! check ask this module instead of comparing it with a workspace.
//!
//! **A post is a project line** (`project_group_routes::store_and_deliver`,
//! the one write path): stored, its rollback copy, the project bus, the
//! hook, the push, and `[project:<name>]` to the point of contact (never
//! its own lines). Files are kept in the point of contact's workspace
//! (`crate::project_chat_files`). A `command`, a widget `origin` or a Board
//! `context` is refused, and so are the card routes (`ask`, `secret`,
//! `answer`, `resend`): a project chat takes messages and files.

use k2_core::project_groups::{self, ProjectGroup};
use k2_core::overlay::project as kp;

use super::*;

/// Is `addr` a project chat's address?
pub(crate) fn is_project_addr(addr: &str) -> bool {
    addr.trim().starts_with(kp::PROJECT_PREFIX)
}

/// Is this resolved Thread a project's chat?
pub(super) fn is_project(resolved: &ResolvedOverlay) -> bool {
    kp::is_project(&resolved.conversation_id)
}

fn group_of(resolved: &ResolvedOverlay) -> Option<&str> {
    kp::group_of(&resolved.conversation_id)
}

fn resolved_for(group: &ProjectGroup, requested: &str) -> ResolvedOverlay {
    let conv = kp::conversation_id(&group.id);
    ResolvedOverlay {
        conversation_id: conv.clone(),
        project_id: group.id.clone(),
        addr: conv,
        requested_addr: requested.to_string(),
        canonical_alias: false,
    }
}

/// The group a `project:<selector>` address names (the project routes' own
/// 400 / 404 / 409 when it doesn't).
fn load(addr: &str) -> Result<ProjectGroup, CliResponse> {
    let selector = addr.trim().strip_prefix(kp::PROJECT_PREFIX).unwrap_or("").trim();
    let id = crate::project_group_routes::resolve_group_selector(selector)?;
    project_groups::get_group_by_id(&id)
        .ok_or_else(|| crate::project_group_routes::resolve_error_response(selector, project_groups::ResolveError::NotFound))
}

/// `resolve_addr`'s arm (a person or an agent; an agent's membership is
/// checked by [`authorize_read`] / [`authorize_write`]): `Some` for every
/// `project:` address, `None` for any other.
pub(super) fn resolve(addr: &str) -> Option<Result<ResolvedOverlay, CliResponse>> {
    if !is_project_addr(addr) {
        return None;
    }
    Some(load(addr).map(|g| resolved_for(&g, addr)))
}

/// `resolve_skin_thread_addr`'s arm: an App pass sees a project's chat
/// under the all-members rule; anything else is the uniform `skin_room`
/// (no oracle: an unknown project answers the same).
pub(super) fn resolve_for_pass(addr: &str, pass: &SkinPass) -> Option<Result<ResolvedOverlay, CliResponse>> {
    if !is_project_addr(addr) {
        return None;
    }
    // Exact id or exact name among the projects it may read: never a
    // prefix, never an oracle (`project.read`'s rule).
    let selector = addr.trim().strip_prefix(kp::PROJECT_PREFIX).unwrap_or("").trim();
    Some(match crate::apps::project_chat::visible_group(pass, selector) {
        Ok(Some((g, _))) => Ok(resolved_for(&g, addr)),
        Ok(None) => Err(crate::apps::project_chat::hidden()),
        Err(e) => Err(error_json("500 Internal Server Error", "store", e)),
    })
}

/// The App cap check on a project chat: `thread:read` is the all-members
/// rule, `thread:post` is `project:write` in every member room; any other
/// cap is refused (a project chat has no cards, activity or settings of its
/// own). `None` for any other Thread.
pub(super) fn pass_cap(resolved: &ResolvedOverlay, pass: &SkinPass, cap: &str) -> Option<Result<(), CliResponse>> {
    let gid = group_of(resolved)?;
    Some(if cap == k2_core::skin::CAP_THREAD_READ {
        if crate::apps::project_chat::can_read(pass, gid) {
            Ok(())
        } else {
            Err(crate::apps::project_chat::hidden())
        }
    } else if cap == k2_core::skin::CAP_THREAD_POST {
        if crate::apps::project_chat::can_post(pass, gid) {
            Ok(())
        } else {
            Err(crate::skin_routes::missing_cap_response(k2_core::skin::CAP_PROJECT_WRITE))
        }
    } else {
        Err(crate::skin_routes::missing_cap_response(cap))
    })
}

/// An agent passport on a project chat: its workspace must be a member.
/// `None` for any other Thread (the own-workspace rule applies there).
pub(super) fn agent_gate(principal: &HookPrincipal, resolved: &ResolvedOverlay) -> Option<Result<(), CliResponse>> {
    let gid = group_of(resolved)?;
    Some(match crate::project_group_routes::agent_chat_refusal(principal.workspace_uuid.trim(), gid) {
        None => Ok(()),
        Some(r) => Err(r),
    })
}

/// A Thread write route that a project chat doesn't take.
pub(super) fn refuse_card_route(path: &str, params: &HashMap<String, String>) -> Option<CliResponse> {
    let card_route = matches!(path, "/cli/thread/ask" | "/cli/thread/secret" | "/cli/thread/answer" | "/cli/thread/void")
        || path == notice_resend::ROUTE;
    let addr = opt_param(params, "addr").or_else(|| opt_param(params, "conversation")).unwrap_or_default();
    (card_route && is_project_addr(&addr)).then(|| {
        error_json(
            "400 Bad Request",
            "project_messages_only",
            "a project's chat takes messages and files only (POST /cli/thread/post, or k2 project msg)",
        )
    })
}

/// `POST /cli/thread/post` to `project:<id>`: a project line from this
/// caller (see the module docs). Answers the Thread post's shape (`id`,
/// `seq`, `from`, `author`, `body`, `attachments`, `clientMsgId`, `mine`,
/// …) plus `project: {groupId, groupName, delivered, deliveryReason}`; a
/// send-once repeat answers `duplicate: true` and does nothing.
pub(super) fn post(params: &HashMap<String, String>, session_author: &str, resolved: &ResolvedOverlay, client_msg_id: Option<String>) -> CliResponse {
    let text = str_param(params, "text");
    if !opt_param(params, "command").unwrap_or_default().trim().is_empty() {
        return usage("a project line takes no command");
    }
    if ["origin", "context"].iter().any(|k| params.get(*k).is_some_and(|v| !v.trim().is_empty())) {
        return error_json("400 Bad Request", "context_not_allowed", "a project line takes no widget origin or Board context");
    }
    let attach_raw = params.get("attachments").map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let attach_value = match &attach_raw {
        None => None,
        Some(raw) => match serde_json::from_str::<serde_json::Value>(raw) {
            Ok(v) => Some(v),
            Err(_) => return usage(crate::ticket_attachments::FIELD_HINT),
        },
    };
    if text.trim().is_empty() && attach_value.is_none() {
        return usage("missing text");
    }
    let Some(gid) = group_of(resolved) else { return not_found("project not found") };
    let Some(group) = project_groups::get_group_by_id(gid) else {
        return not_found("project not found");
    };
    let skin = request_skin();
    if let Some(pass) = &skin {
        if !crate::apps::project_chat::can_post(pass, gid) {
            return crate::skin_routes::missing_cap_response(k2_core::skin::CAP_PROJECT_WRITE);
        }
        if text.len() > crate::apps::project_chat::MAX_BODY_BYTES {
            return usage(format!("text is over {} bytes", crate::apps::project_chat::MAX_BODY_BYTES));
        }
    }
    let principal = crate::caller_workspace::principal_from_params(params);
    let bound = principal_is_bound(params);
    // Who: an App guest (`guest:<username>`), a member agent (stored as
    // its workspace's address name), else the person behind the token.
    let (author, kind, agent_ws, person, ns) = match (&skin, principal.as_ref().filter(|_| bound)) {
        (Some(pass), _) => (
            project_groups::guest_author(&pass.username),
            project_groups::AUTHOR_KIND_GUEST,
            None,
            None,
            crate::client_msg_id::guest_ns(pass),
        ),
        (None, Some(p)) => {
            let ws = p.workspace_uuid.trim().to_string();
            if let Some(r) = crate::project_group_routes::agent_chat_refusal(&ws, gid) {
                return r;
            }
            let name = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                k2_core::workspace_session_handles::workspace_address_name(&conn, &ws).unwrap_or_else(|_| ws.clone())
            };
            let ns = crate::client_msg_id::agent_ns(&ws);
            (name, project_groups::AUTHOR_KIND_AGENT, Some(ws), None, ns)
        }
        (None, None) => {
            let a = session_author.trim();
            let a = if a.is_empty() { "owner" } else { a };
            let person = match k2_core::notify_prefs::People::load() {
                Ok(people) => people.person_key(a),
                Err(_) => a.to_ascii_lowercase(),
            };
            (a.to_string(), project_groups::AUTHOR_KIND_PERSON, None, Some(person), crate::client_msg_id::person_ns(a))
        }
    };
    let body_hash = crate::client_msg_id::body_hash(&text, attach_raw.as_deref().unwrap_or(""));
    let at_ms = chrono::Utc::now().timestamp_millis();
    let client = client_msg_id.as_deref().map(|id| project_groups::ClientKey { ns: &ns, client_msg_id: id, at_ms, body_hash: &body_hash });
    let files = match crate::project_chat_files::check_and_snapshot_as(&group, attach_value.as_ref(), agent_ws.as_deref(), skin.as_ref(), &author) {
        Ok(f) => f,
        Err(r) => return r,
    };
    let files_root = files.root.clone();
    let delivery = if agent_ws.is_some() {
        crate::project_group_routes::PocDelivery::Wait
    } else {
        crate::project_group_routes::PocDelivery::Later
    };
    let stored = match crate::project_group_routes::store_and_deliver(
        &group,
        gid,
        &author,
        kind,
        agent_ws.as_deref(),
        person.as_deref(),
        &text,
        files,
        client,
        skin.as_ref().map(crate::overlay_routes::guest_stamp),
        delivery,
    ) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let (msg, delivered, reason, mismatch) = match stored {
        crate::project_group_routes::Stored::Posted(p) => {
            let reason = p.delivery_reason.clone();
            (p.msg, Some(p.delivered), reason, None)
        }
        crate::project_group_routes::Stored::Duplicate { msg, mismatch } => (msg, None, None, Some(mismatch)),
    };
    if mismatch.is_none() {
        if let Some(pass) = &skin {
            crate::apps::project_chat::note_guest_own_post(pass, gid, msg.seq);
        }
    }
    let item = match overlay::find_thread_item(&resolved.conversation_id, &msg.id) {
        Ok(Some(i)) => i,
        Ok(None) => return error_json("500 Internal Server Error", "store", "the project line vanished after its write"),
        Err(e) => return error_json("500 Internal Server Error", "store", e),
    };
    let mut body = post_reply_json(&item, resolved, files_root.as_deref());
    if let Some(m) = mismatch {
        body["duplicate"] = serde_json::json!(true);
        if m {
            body["mismatch"] = serde_json::json!(true);
        }
    }
    body["project"] = serde_json::json!({
        "groupId": group.id,
        "groupName": group.name,
        "delivered": delivered,
        "deliveryReason": reason,
    });
    CliResponse::ok_json(body.to_string())
}

/// After a line of a project's chat is deleted: its rollback copy goes,
/// the project's views refresh (`project-group:message-created` is not
/// re-sent; the tombstone frame is the Thread's), and the point of contact
/// hears it as every project line, `[project:<name>] <who> deleted a
/// message …` (names and a time only, never the line). `false` for any
/// other Thread (the caller tells its agent the Thread way).
pub(super) fn after_delete(resolved: &ResolvedOverlay, doc_id: &str, before: &overlay::OverlayDoc, by: &str) -> bool {
    let Some(gid) = group_of(resolved) else { return false };
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if let Err(e) = k2_core::project_chat_thread::note_deleted(&conn, &resolved.conversation_id, doc_id) {
            k2_core::log_debug!("[project-thread] delete copy on {}: {e}", resolved.conversation_id);
        }
    }
    crate::read_live::conversation_changed(k2_core::read_state::KIND_PROJECT, gid);
    let Some(group) = project_groups::get_group_by_id(gid) else { return true };
    let skin = request_skin();
    let deleter = skin.as_ref().map(skin_from_stamp).unwrap_or_else(|| by.to_string());
    let note = message_delete::agent_note(&deleter, Some(&before.from), before.created_at);
    crate::project_group_routes::tell_poc(&group, by, &note, skin.as_ref().map(crate::overlay_routes::guest_stamp));
    true
}

/// The overlay socket's gate for `conversation=project:<id>` (a login):
/// `Some(Ok(group id))` when the project exists, `Some(Err)` when not,
/// `None` for any other conversation. App passes are refused before this
/// (`project.events` is theirs).
pub(crate) fn socket_gate(conversation: &str) -> Option<Result<String, CliResponse>> {
    let gid = kp::group_of(conversation)?;
    Some(match project_groups::get_group_by_id(gid) {
        Some(g) => Ok(g.id),
        None => Err(not_found("project not found")),
    })
}

/// The workspace a file-link batch names as `project:<id>`: its point of
/// contact's, as `(project_id, folder)`. `None` for any other name.
pub(crate) fn workspace_of(token: &str) -> Option<Result<(String, String), CliResponse>> {
    if !is_project_addr(token) {
        return None;
    }
    Some(load(token).and_then(|g| {
        let poc = g.poc_workspace_id.clone().ok_or_else(|| not_found("this project has no point of contact"))?;
        crate::thread_attachments::project_root(&poc)
            .map(|root| (poc, root.to_string_lossy().to_string()))
            .ok_or_else(|| not_found("this project's point of contact's workspace is not registered"))
    }))
}
