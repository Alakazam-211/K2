//! `ticket:<id>` on the Thread routes (ticket-thread-v1, 0.46.17).
//!
//! A ticket's discussion is the Thread conversation `ticket:<full id>`
//! (`k2_core::overlay::ticket`), and its address is the same string (a
//! unique id prefix also resolves, like `k2 tickets show`). Every Thread
//! route that resolves an address takes it: reads and deltas, the overlay
//! socket, typing, read marks, posts, attachments, deleting your own post.
//!
//! **Access is the TICKET's, never `thread:read`:**
//! - a person (the owner token, a Connect login) reads and posts every
//!   ticket, except that a login that may not see K2's own notices (a
//!   Member) gets "not found" for one (`feedback_routes::is_owner_notice`),
//!   the same answer as a wrong id;
//! - an App pass needs `tickets:read` in the ticket's room to read, and
//!   `tickets:post` there to post; another room's ticket is `skin_room`
//!   (no oracle);
//! - an agent passport reads and posts its own workspace's tickets only
//!   (as `k2 tickets show` / `comment`): the overlay's own-workspace rule,
//!   with the ticket's workspace as the Thread's.
//!
//! **A post is a ticket message** (`crate::ticket_comment`): every ticket
//! rule applies (redaction, option pick → answered, free text →
//! needs_discussion and reopen, `[feedback:<id>]` delivery for a person's
//! words, an agent's stored only, the ticket events and push). Files are
//! checked against the ticket's workspace by the ticket door. A `command`,
//! a widget `origin` or a Board `context` is refused, and so are the card
//! routes (`ask`, `secret`, `answer`, `resend`): a ticket takes messages.

use std::cell::RefCell;

use k2_core::feedback::{self, CommentBy, PrefixError};
use k2_core::overlay::ticket as kt;

use super::*;

/// Is `addr` a ticket's address?
pub(crate) fn is_ticket_addr(addr: &str) -> bool {
    addr.trim().starts_with(kt::TICKET_PREFIX)
}

/// Is this resolved Thread a ticket's discussion?
pub(super) fn is_ticket(resolved: &ResolvedOverlay) -> bool {
    kt::is_ticket(&resolved.conversation_id)
}

thread_local! {
    /// The login behind this POST (the dispatcher binds GET logins with
    /// `read_routes::with_read_login`; a POST names it as its session
    /// author), for the Member rule.
    static POST_LOGIN: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Run `f` with the POST's login bound (`None`: an agent, an App pass).
pub(super) fn with_post_login<T>(login: Option<String>, f: impl FnOnce() -> T) -> T {
    POST_LOGIN.with(|slot| {
        let prev = slot.replace(login);
        let out = f();
        slot.replace(prev);
        out
    })
}

/// The person login of this request, if a person made it.
fn viewer_login() -> Option<String> {
    crate::read_routes::request_login().or_else(|| POST_LOGIN.with(|slot| slot.borrow().clone()))
}

/// May `login` see ticket `item`? A Member never sees K2's own notices.
pub(crate) fn login_sees(login: Option<&str>, item: &feedback::FeedbackItem) -> bool {
    match login.map(str::trim).filter(|l| !l.is_empty()) {
        Some(l) if crate::feedback_routes::is_owner_notice(&item.project_id) => {
            crate::feedback_routes::sees_owner_notices(crate::session_author::is_owner_token(l), Some(l))
        }
        _ => true,
    }
}

fn resolved_for(item: &feedback::FeedbackItem, requested: &str) -> ResolvedOverlay {
    let conv = kt::conversation_id(&item.id);
    ResolvedOverlay {
        conversation_id: conv.clone(),
        project_id: item.project_id.clone(),
        addr: conv,
        requested_addr: requested.to_string(),
        canonical_alias: false,
    }
}

/// `resolve_addr`'s arm for a person or an agent: `Some` for every
/// `ticket:` address (found, or the ticket route's own not-found /
/// ambiguous answer), `None` for any other address.
pub(super) fn resolve(addr: &str) -> Option<Result<ResolvedOverlay, CliResponse>> {
    if !is_ticket_addr(addr) {
        return None;
    }
    let given = kt::ticket_of(addr).unwrap_or("");
    let not_found = || crate::feedback_routes::prefix_error_response(given, PrefixError::NotFound);
    let full = match feedback::resolve_id_prefix(given) {
        Ok(full) => full,
        Err(e) => return Some(Err(crate::feedback_routes::prefix_error_response(given, e))),
    };
    let Some(item) = feedback::get_item(&full) else { return Some(Err(not_found())) };
    if !login_sees(viewer_login().as_deref(), &item) {
        return Some(Err(not_found()));
    }
    Some(Ok(resolved_for(&item, addr)))
}

/// `resolve_skin_thread_addr`'s arm: an App pass reaches a ticket of its
/// own rooms with `tickets:read` there (anything else is `skin_room` or the
/// missing cap, as on `GET /cli/feedback/show`).
pub(super) fn resolve_for_pass(addr: &str, pass: &SkinPass) -> Option<Result<ResolvedOverlay, CliResponse>> {
    if !is_ticket_addr(addr) {
        return None;
    }
    let given = kt::ticket_of(addr).unwrap_or("");
    Some(crate::feedback_routes::skin_load_item(pass, given, crate::skin_routes::TICKETS_READ).map(|item| resolved_for(&item, addr)))
}

/// The App cap a Thread route asks for, said for a ticket: `thread:read`
/// is `tickets:read` there and `thread:post` is `tickets:post`.
pub(super) fn cap_for<'a>(resolved: &ResolvedOverlay, cap: &'a str) -> &'a str {
    if !is_ticket(resolved) {
        return cap;
    }
    if cap == k2_core::skin::CAP_THREAD_READ {
        crate::skin_routes::TICKETS_READ
    } else if cap == k2_core::skin::CAP_THREAD_POST {
        crate::skin_routes::TICKETS_POST
    } else {
        cap
    }
}

/// A Thread write route that a ticket's discussion doesn't take.
pub(super) fn refuse_card_route(path: &str, params: &HashMap<String, String>) -> Option<CliResponse> {
    let card_route = matches!(path, "/cli/thread/ask" | "/cli/thread/secret" | "/cli/thread/answer")
        || path == notice_resend::ROUTE;
    let addr = opt_param(params, "addr").or_else(|| opt_param(params, "conversation")).unwrap_or_default();
    (card_route && is_ticket_addr(&addr)).then(|| {
        error_json(
            "400 Bad Request",
            "ticket_messages_only",
            "a ticket's Thread takes messages only: answer a ticket with its options (POST /cli/thread/post with optionPick) or in words",
        )
    })
}

fn truthy(params: &HashMap<String, String>, key: &str) -> bool {
    matches!(params.get(key).map(|v| v.trim().to_ascii_lowercase()).as_deref(), Some("1" | "true" | "yes"))
}

/// `POST /cli/thread/post` to `ticket:<id>` (see the module docs). Answers
/// the Thread post's shape (`id`, `seq`, `from`, `author`, `body`,
/// `attachments`, `clientMsgId`, `mine`, …) plus `ticket: {id, status,
/// answered, delivered, deliveryReason}`; a send-once repeat answers
/// `duplicate: true` and does nothing.
pub(super) fn post(
    params: &HashMap<String, String>,
    session_author: &str,
    resolved: &ResolvedOverlay,
    client_msg_id: Option<String>,
    received_at: i64,
) -> CliResponse {
    let text = str_param(params, "text");
    if !opt_param(params, "command").unwrap_or_default().trim().is_empty() {
        return usage("a ticket post takes no command");
    }
    if ["origin", "context"].iter().any(|k| params.get(*k).is_some_and(|v| !v.trim().is_empty())) {
        return error_json("400 Bad Request", "context_not_allowed", "a ticket post takes no widget origin or Board context");
    }
    let option_pick = truthy(params, "optionPick") || truthy(params, "option_pick");
    let attach_reqs = match params.get("attachments").map(|s| s.trim()).filter(|s| !s.is_empty()) {
        None => Vec::new(),
        Some(raw) => match serde_json::from_str::<serde_json::Value>(raw) {
            Ok(v) => match crate::ticket_attachments::parse_field(Some(&v)) {
                Ok(r) => r,
                Err(e) => return e,
            },
            Err(_) => return usage(crate::ticket_attachments::FIELD_HINT),
        },
    };
    // TK5: files may stand in for the text, except on an option pick.
    if text.trim().is_empty() && (attach_reqs.is_empty() || option_pick) {
        return usage("missing text");
    }
    let skin = request_skin();
    if let Err(e) = require_skin_cap_in_resolved(resolved, k2_core::skin::CAP_THREAD_POST) {
        return e;
    }
    let principal = crate::caller_workspace::principal_from_params(params);
    let bound = principal_is_bound(params);
    let (from, stamp) = thread_sender(params, session_author);
    if let Err(e) = authorize_write(principal.as_ref(), resolved, &from) {
        return e;
    }
    let Some(ticket) = kt::ticket_of(&resolved.conversation_id).and_then(feedback::get_item) else {
        return not_found("ticket not found");
    };
    // Who: an App guest (a person, stored as its pass's username), an agent
    // (its stamped address, stored only), else the person behind the token
    // (stored as their username: the 0.46.0 rule the ticket route keeps).
    let (name, is_human) = match (&skin, bound) {
        (Some(pass), _) => (pass.username.clone(), true),
        (None, true) => (from.clone(), false),
        (None, false) => (
            k2_core::person_name::sender_identity_now(k2_core::person_name::Sender::from_session_author(session_author)),
            true,
        ),
    };
    let who = match &skin {
        Some(p) => format!("guest:{}", p.username.trim()),
        None => name.clone(),
    };
    let via = opt_param(params, "via").unwrap_or_else(|| "thread".to_string());
    let client_key = client_msg_id.map(|id| overlay::ClientMsgKey {
        key: format!("{}/{id}", thread_client_ns(principal.as_ref(), bound, &via, session_author)),
        body_hash: crate::client_msg_id::body_hash(&text, params.get("attachments").map(String::as_str).unwrap_or("")),
        client_msg_id: id,
        at_ms: received_at,
    });
    if let Some(k) = &client_key {
        match overlay::client_msg_lookup(&k.key) {
            Ok(Some(hit)) => return thread_duplicate_reply(resolved, &hit, &k.body_hash),
            Ok(None) => {}
            Err(e) => return error_json("500 Internal Server Error", "store", e),
        }
    }
    let (files, files_root) =
        match crate::ticket_attachments::check_and_snapshot(&ticket.id, &ticket.project_id, skin.as_ref(), &attach_reqs, &who) {
            Ok(pair) => pair,
            Err(e) => return e,
        };
    let body_hash = client_key.as_ref().map(|k| k.body_hash.clone()).unwrap_or_default();
    let input = crate::ticket_comment::CommentInput {
        ticket,
        by: CommentBy { name, author: stamp.clone() },
        is_human,
        skin: skin.as_ref(),
        body: text,
        files,
        files_root: files_root.clone(),
        who,
        option_pick: option_pick && is_human,
        client: client_key,
    };
    let (item, comment, seq, answered, delivery) = match crate::ticket_comment::comment_core(input) {
        Err(e) => return e,
        Ok(crate::ticket_comment::CommentDone::Duplicate(hit)) => return thread_duplicate_reply(resolved, &hit, &body_hash),
        Ok(crate::ticket_comment::CommentDone::Stored { item, comment, seq, answered, delivery }) => {
            (item, comment, seq, answered, delivery)
        }
    };
    // The comment id IS the Thread doc id.
    let stored = match overlay::find_thread_item(&resolved.conversation_id, &comment.id) {
        Ok(Some(i)) => i,
        Ok(None) => return error_json("500 Internal Server Error", "store", "the ticket message vanished after its write"),
        Err(e) => return error_json("500 Internal Server Error", "store", e),
    };
    {
        // thread-read-v1: a person's own post advances their own cursor.
        let db = k2_core::db::shared();
        let conn = db.lock();
        crate::read_routes::note_own_thread_read(&conn, stored.doc.author.as_ref(), &resolved.conversation_id, seq);
        if let Some(pass) = &skin {
            crate::read_routes::note_guest_thread_read(&conn, pass, &resolved.conversation_id, seq);
        }
    }
    let mut body = post_reply_json(&stored, resolved, files_root.as_deref());
    let (delivered, reason) = match &delivery {
        Some((d, r, _)) => (serde_json::json!(d), serde_json::json!(r)),
        None => (serde_json::Value::Null, serde_json::Value::Null),
    };
    body["ticket"] = serde_json::json!({
        "id": item.id,
        "status": item.status,
        "answered": answered,
        "delivered": delivered,
        "deliveryReason": reason,
    });
    CliResponse::ok_json(body.to_string())
}

/// After a message of a ticket's Thread is deleted: the row's counts drop
/// (and its rollback copy goes), the ticket's views refresh
/// (`FeedbackCommented`, `TicketChanged`), and the ticket's agent hears it
/// as every ticket event, `[feedback:<id>] <who> deleted a message …`
/// (names and a time only, never the message). `false` for any other
/// Thread (the caller tells its agent the Thread way).
pub(super) fn after_delete(resolved: &ResolvedOverlay, doc_id: &str, before: &overlay::OverlayDoc, by: &str) -> bool {
    if !is_ticket(resolved) {
        return false;
    }
    let noted = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::feedback_thread::note_deleted(&conn, &resolved.conversation_id, doc_id, before.attachments.len())
    };
    let id = match noted {
        Ok(Some(id)) => id,
        Ok(None) => return true,
        Err(e) => {
            k2_core::log_debug!("[ticket-thread] delete count on {}: {e}", resolved.conversation_id);
            return true;
        }
    };
    crate::feedback_routes::emit_commented(&id, by);
    let Some(item) = feedback::get_item(&id) else { return true };
    crate::feedback_routes::emit_ticket_changed(&item, crate::feedback_routes::CHANGE_COMMENTED, None);
    let skin = request_skin();
    let deleter = skin.as_ref().map(skin_from_stamp).unwrap_or_else(|| by.to_string());
    let note = message_delete::agent_note(&deleter, Some(&before.from), before.created_at);
    let _ = crate::feedback_routes::deliver_ticket_words(&item, skin.as_ref(), by, &note, None, &[]);
    true
}

/// The hint when a message moved from a ticket's old comments (marked
/// imported) can't be deleted.
pub(crate) const MOVED_READ_ONLY_HINT: &str = "This message came over from the ticket's old comments and can't be deleted.";

/// The `409 imported_read_only` a ticket's Thread answers for a moved
/// message, in the ticket's words; `None` for any other Thread (the clone
/// wording stays).
pub(super) fn moved_read_only(resolved: &ResolvedOverlay) -> Option<CliResponse> {
    is_ticket(resolved).then(|| error_json("409 Conflict", "imported_read_only", MOVED_READ_ONLY_HINT))
}

/// `GET /cli/thread/activity?addr=ticket:<id>` for an App pass: the ticket's
/// read right (`tickets:read`, checked when the address resolved) is the
/// gate, not `activity:read`. Counts only, as every guest turn.
pub(super) fn guest_activity(resolved: &ResolvedOverlay) -> CliResponse {
    let turn = crate::thread_activity::current_turn_guest(&resolved.conversation_id);
    CliResponse::ok_json(
        serde_json::json!({ "ok": true, "addr": resolved.addr, "conversation_id": resolved.conversation_id, "turn": turn }).to_string(),
    )
}

/// The overlay socket's gate for `conversation=ticket:<id>`: `Some(Ok(project))`
/// when this caller may watch it (an App pass: `tickets:read` in its room;
/// a login: the Member rule), `Some(Err)` when not, `None` for any other
/// conversation.
pub(crate) fn socket_gate(conversation: &str, pass: Option<&SkinPass>, login: Option<&str>) -> Option<Result<String, CliResponse>> {
    let given = kt::ticket_of(conversation)?;
    let r = match pass {
        Some(p) => crate::feedback_routes::skin_load_item(p, given, crate::skin_routes::TICKETS_READ),
        None => feedback::resolve_id_prefix(given)
            .ok()
            .filter(|full| full == given)
            .and_then(|full| feedback::get_item(&full))
            .filter(|item| login_sees(login, item))
            .ok_or_else(|| crate::feedback_routes::prefix_error_response(given, PrefixError::NotFound)),
    };
    Some(r.map(|item| item.project_id))
}

/// The workspace a file-link batch names as `ticket:<id>`: the ticket's
/// (for this login), as `(project_id, folder)`. `None` for any other name.
pub(crate) fn workspace_of(token: &str, pass: Option<&SkinPass>) -> Option<Result<(String, String), CliResponse>> {
    if !is_ticket_addr(token) {
        return None;
    }
    let resolved = match pass {
        Some(p) => resolve_for_pass(token, p)?,
        None => resolve(token)?,
    };
    Some(resolved.and_then(|r| {
        crate::thread_attachments::project_root(&r.project_id)
            .map(|root| (r.project_id.clone(), root.to_string_lossy().to_string()))
            .ok_or_else(|| not_found("this ticket's workspace is not registered"))
    }))
}
