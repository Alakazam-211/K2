//! `POST /cli/thread/message/delete` (thread-message-delete-v1, 0.46.11).
//!
//! Rosson: "If a user enters something sensitive into the threads, we need
//! to give them a way to delete the sensitive message. If they delete a
//! sensitive message, we can tell the agent about it as well and send a
//! message into the terminal that the user decided to delete that message."
//!
//! Body `{addr, id | seq}` (`conversation` is accepted for `addr`).
//!
//! - **Who:** the person who sent it (the owner token and the owner's
//!   linked logins are one person, `thread_mine`'s rule), or the server
//!   owner for anyone's post. An App guest deletes only its own posts in its
//!   own room (`thread:post` there). An agent passport never deletes: 403
//!   `person_only` (its own cards have `k2 thread withdraw`).
//! - **What:** the doc becomes a tombstone (`deleted`, `deletedAt`,
//!   `deletedBy`; no body, attachments, context or summary) in the one store
//!   write that also notes the change for delta readers. The seq stays.
//!   Outside the store: the copies K2 uploaded for this message (under the
//!   workspace's `.k2/downloads/`, when no other message here points at
//!   them), their thumbnails, and the text in the workspace's compose
//!   history. A file the message only pointed at (a workspace file) stays.
//!   A delivery of the post still waiting on the agent's lane is dropped
//!   (`ThreadDeliveryJob::source_deleted`).
//! - **Live:** one Thread frame with the tombstone, so every open view
//!   replaces the bubble.
//! - **The agent** gets one line on the same lane as a Thread post, with no
//!   word of what the message said: `[thread:<addr>] <who> deleted a message
//!   they sent at <time>. Don't repeat, store or act on what it said.`
//! - **Idempotent:** a second delete answers `already: true` and does
//!   nothing else.
//!
//! A Thread with an agent on another server lives on that server: the app
//! sends this route to that server (a room write), where the same rules run
//! against the login that posted there.

use super::*;

pub(crate) const ROUTE: &str = "/cli/thread/message/delete";
/// `/boot-status` feature key: a client offers Delete message only with it.
pub(crate) const FEATURE: &str = "thread-message-delete-v1";
/// What a list preview says for a deleted post.
pub(crate) const PREVIEW: &str = "Message deleted";
/// The refusal for anyone but the sender (or the owner).
pub(crate) const ONLY_THE_SENDER: &str = "only the person who sent it can delete it";

/// Who is asking, from the credential (never the body).
enum Deleter {
    /// The owner token or a Connect login: its person key.
    Person(String),
    /// An App guest: its pass username.
    Guest(String),
}

pub(crate) fn handle(params: &HashMap<String, String>, session_author: &str) -> CliResponse {
    let addr = opt_param(params, "addr").or_else(|| opt_param(params, "conversation")).unwrap_or_default();
    if addr.trim().is_empty() {
        return usage("missing addr (the Thread's address)");
    }
    let which = match message_ref(params) {
        Ok(w) => w,
        Err(e) => return e,
    };
    // An agent's passport never deletes a person's message.
    if principal_is_bound(params) || crate::caller_workspace::principal_from_params(params).is_some() {
        return error_json("403 Forbidden", "person_only", format!("{ONLY_THE_SENDER}; an agent can't delete a person's message (k2 thread withdraw closes its own cards)"));
    }
    let resolved = match resolve_thread_addr(&addr) {
        Ok(r) => r,
        Err(e) => return e,
    };
    if let Some(r) = refuse_if_imported(&resolved, None) {
        return r;
    }
    if let Err(e) = require_skin_cap_in_resolved(&resolved, k2_core::skin::CAP_THREAD_POST) {
        return e;
    }
    let deleter = match request_skin() {
        Some(pass) => Deleter::Guest(skin_from_stamp(&pass)),
        None => match person_of(session_author) {
            Some(p) => p,
            None => return error_json("403 Forbidden", "not_your_message", ONLY_THE_SENDER),
        },
    };
    let Some(by) = card_actor_author(params, session_author) else {
        return error_json("403 Forbidden", "not_your_message", ONLY_THE_SENDER);
    };
    let deleted = overlay::delete_thread_message(&resolved.conversation_id, &which, by, |doc| may_delete(&deleter, doc));
    let d = match deleted {
        Ok(d) => d,
        Err(e) if e == ONLY_THE_SENDER => return error_json("403 Forbidden", "not_your_message", ONLY_THE_SENDER),
        Err(e) if e == overlay::DELETE_NOT_FOUND => return not_found(format!("no message {} on this Thread", ref_label(&which))),
        Err(e) if e == overlay::DELETE_NOT_A_PERSONS_MESSAGE => {
            return error_json("409 Conflict", "not_a_persons_message", "only a person's own message can be deleted (an agent's post, a card or a notice can't)")
        }
        Err(e) if e == overlay::IMPORTED_READ_ONLY => return imported_read_only(),
        Err(e) => return error_json("500 Internal Server Error", "store", e),
    };
    let already = d.before.is_none();
    if let Some(before) = &d.before {
        after_delete(params, session_author, &resolved, &d, before);
    }
    CliResponse::ok_json(with_moved_from(
        serde_json::json!({
            "ok": true,
            "id": d.doc.id,
            "seq": d.seq,
            "deleted": true,
            "already": already,
            "deletedAt": d.doc.deleted_at,
            "deletedBy": d.doc.deleted_by,
            "conversation_id": resolved.conversation_id,
            "addr": resolved.addr,
        }),
        &resolved,
    ))
}

/// `id` or `seq` (exactly one).
fn message_ref(params: &HashMap<String, String>) -> Result<overlay::MessageRef, CliResponse> {
    let id = opt_param(params, "id").map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let seq = opt_param(params, "seq").map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    match (id, seq) {
        (Some(_), Some(_)) => Err(usage("give the message's id or its seq, not both")),
        (Some(id), None) => Ok(overlay::MessageRef::Id(id)),
        (None, Some(s)) => match s.parse::<i64>() {
            Ok(n) if n >= 1 => Ok(overlay::MessageRef::Seq(n)),
            _ => Err(usage("seq is a whole number >= 1 (the [N] in k2 thread <addr>)")),
        },
        (None, None) => Err(usage("missing id or seq (which message to delete)")),
    }
}

fn ref_label(which: &overlay::MessageRef) -> String {
    match which {
        overlay::MessageRef::Id(id) => id.clone(),
        overlay::MessageRef::Seq(n) => format!("#{n}"),
    }
}

/// The person behind the owner token (`owner`) or a Connect login.
fn person_of(session_author: &str) -> Option<Deleter> {
    let s = session_author.trim();
    if s.is_empty() || s.eq_ignore_ascii_case("owner") {
        return Some(Deleter::Person(k2_core::notify_prefs::OWNER.to_string()));
    }
    let people = k2_core::notify_prefs::People::load().ok()?;
    let viewer = k2_core::permission::card::Viewer::Login { user: s.to_string(), role: String::new() };
    crate::thread_mine::viewer_person(&people, &viewer).map(Deleter::Person)
}

/// The author rule, on the stored doc: the sender, or the server owner.
fn may_delete(deleter: &Deleter, doc: &overlay::OverlayDoc) -> Result<(), String> {
    let refuse = || Err(ONLY_THE_SENDER.to_string());
    let Some(author) = doc.author.as_ref() else { return refuse() };
    match deleter {
        Deleter::Guest(login) => {
            if author.kind == overlay::doc::AUTHOR_GUEST && author.login.trim() == login.trim() {
                Ok(())
            } else {
                refuse()
            }
        }
        Deleter::Person(me) if me == k2_core::notify_prefs::OWNER => Ok(()),
        Deleter::Person(me) => {
            let people = k2_core::notify_prefs::People::load().map_err(|_| ONLY_THE_SENDER.to_string())?;
            let author_json = serde_json::to_value(author).map_err(|_| ONLY_THE_SENDER.to_string())?;
            if crate::thread_mine::is_mine(&people, me, &author_json) {
                Ok(())
            } else {
                refuse()
            }
        }
    }
}

/// The first delete's work outside the store: the live frame, the copies
/// and thumbnails, the compose history, the audit line and the agent's note.
fn after_delete(
    params: &HashMap<String, String>,
    session_author: &str,
    resolved: &ResolvedOverlay,
    d: &overlay::DeletedMessage,
    before: &overlay::OverlayDoc,
) {
    crate::overlay_ws::publish(OverlayFrame {
        rev: 0,
        collection: "thread".to_string(),
        seq: d.seq,
        id: d.doc.id.clone(),
        doc: Some(d.doc.clone()),
        activity: None,
        conversation_id: Some(d.conversation_id.clone()),
    });
    remove_uploaded_copies(&resolved.project_id, &d.conversation_id, before);
    if let Some(body) = before.body.as_deref() {
        if let Err(e) = k2_core::workspace_compose_history::forget_compose_send(&resolved.project_id, body) {
            k2_core::log_debug!("[thread-delete] compose history: {e}");
        }
    }
    let from = card_actor_from(params, resolved, session_author);
    crate::thread_attachments::audit("thread.message.delete", &from, format!("to={} id={} seq={}", resolved.addr, d.doc.id, d.seq));
    tell_the_agent(resolved, &from, before);
}

/// What the agent is told. Names and a time only: never the message.
pub(crate) fn agent_note(deleter: &str, sender: Option<&str>, sent_at: i64) -> String {
    let at = chrono::DateTime::from_timestamp(sent_at, 0)
        .map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_else(|| "an earlier time".to_string());
    let whose = match sender {
        Some(s) if !s.trim().is_empty() && s.trim() != deleter.trim() => format!("a message {} sent", s.trim()),
        _ => "a message they sent".to_string(),
    };
    format!("{deleter} deleted {whose} at {at}. Don't repeat, store or act on what it said.")
}

fn tell_the_agent(resolved: &ResolvedOverlay, from: &str, before: &overlay::OverlayDoc) {
    let addr = resolved.addr.as_str();
    let sender = before.author.as_ref().map(|a| a.name.clone().unwrap_or_else(|| before.from.clone()));
    let job = match request_skin() {
        Some(pass) => {
            let label = guest_stamp(&pass);
            let line = agent_note(&skin_from_stamp(&pass), None, before.created_at);
            let payload = format!("[thread:{addr}] {}", workspace_msg::quote_guest_lines(&line));
            record_test_inject(&workspace_msg::format_sender(workspace_msg::Sender::Guest(&label), &payload, ""));
            ThreadDeliveryJob::new(resolved, payload, &label, "thread", "", None).from_guest()
        }
        None => {
            let line = agent_note(from, sender.as_deref(), before.created_at);
            record_test_inject(&format_thread_compose_pty_line(from, addr, &line));
            ThreadDeliveryJob::new(resolved, format!("[thread:{addr}] {line}"), from, "thread", "", None)
        }
    };
    deliver_thread_later(job);
}

/// The copies K2 uploaded into the workspace for this message
/// (`.k2/downloads/`, `.k2so/downloads/`), when no other live message on
/// this Thread points at the same file, and every cached thumbnail of each
/// attachment. A workspace file the message only pointed at stays.
fn remove_uploaded_copies(project_id: &str, conversation_id: &str, before: &overlay::OverlayDoc) {
    if before.attachments.is_empty() {
        return;
    }
    let root = crate::thread_attachments::project_root(project_id);
    let others: Vec<String> = overlay::read_thread(conversation_id, 0)
        .map(|items| {
            items
                .into_iter()
                .filter(|i| i.id != before.id && !i.doc.deleted)
                .flat_map(|i| i.doc.attachments.into_iter().map(|a| a.path))
                .collect()
        })
        .unwrap_or_default();
    let thumbs = k2_core::overlay::thumbs::cache_dir();
    for a in &before.attachments {
        let real = root.as_deref().and_then(|r| k2_core::overlay::attachments::resolve_now(r, a));
        let mut idents = vec![(a.size, a.modified_at)];
        if let Some(meta) = real.as_ref().and_then(|p| std::fs::metadata(p).ok()) {
            idents.push((meta.len(), k2_core::overlay::attachments::modified_ms(&meta)));
        }
        for (size, modified) in idents {
            for px in [k2_core::overlay::thumbs::PX_SMALL, k2_core::overlay::thumbs::PX_LARGE] {
                let key = k2_core::overlay::thumbs::cache_key(project_id, &a.path, size, modified, px);
                for ext in ["jpg", "png", "svg"] {
                    let _ = std::fs::remove_file(thumbs.join(format!("{key}.{ext}")));
                }
            }
        }
        if !is_uploaded_copy(&a.path) || others.iter().any(|p| p == &a.path) {
            continue;
        }
        if let Some(p) = real {
            match std::fs::remove_file(&p) {
                Ok(()) => crate::session_events::emit_fs_changed_for_paths([p.to_string_lossy().to_string()]),
                Err(e) => k2_core::log_debug!("[thread-delete] remove {}: {e}", a.path),
            }
        }
    }
}

/// A path K2's own upload wrote (the compose bar's attach copies).
pub(crate) fn is_uploaded_copy(rel: &str) -> bool {
    let rel = rel.trim_start_matches("./");
    (rel.starts_with(".k2/downloads/") || rel.starts_with(".k2so/downloads/")) && !rel.split('/').any(|c| c == "..")
}
