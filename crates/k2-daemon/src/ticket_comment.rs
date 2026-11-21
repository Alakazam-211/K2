//! One message on a ticket, whichever door it comes through: the ticket
//! route (`POST /cli/feedback/comment`, the App contract, the phone, `k2
//! tickets comment`) and, since 0.46.17 (ticket-thread-v1), a Thread post
//! to `ticket:<id>` (`POST /cli/thread/post`). Both run [`comment_core`],
//! so every ticket rule holds on both:
//!
//! - **redaction** (the core stores the body redacted);
//! - a **person's** message: an option pick IS the answer (`answered`,
//!   `FeedbackAnswered`, `ask --wait` unblocks); free text moves the ticket
//!   to `needs_discussion` (reopening a resolved one,
//!   `FeedbackStatusChanged`); then it is delivered to the ticket's agent as
//!   `[feedback:<short id>] …` (best effort, after the store);
//! - an **agent's** message is stored only (no delivery, no status move);
//! - every stored message fires `FeedbackCommented`, one `TicketChanged`,
//!   and the phone push to the assignees.
//!
//! The ticket's messages live in its Thread conversation (see
//! `k2_core::feedback_thread`), so the stored message is also a Thread item.

use std::path::PathBuf;

use k2_core::feedback::{self, CommentBy, CommentWrite, FeedbackComment, FeedbackItem, PrefixError};
use k2_core::overlay::{ClientIdHit, ClientMsgKey, PostAuthor, ThreadAttachment};
use k2_core::skin::SkinPass;

use crate::cli_response::CliResponse;
use crate::feedback_routes::{self as fr, prefix_error_response, usage_error};

/// `POST /cli/feedback/comment` body. `author` defaults to `owner`
/// (the renderer's thread panel posts author-less); agents pass their
/// own name via the CLI — that's how human and agent comments are told
/// apart.
#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct CommentBody {
    id: String,
    body: String,
    author: Option<String>,
    /// The person picked one of the ticket's options (a structured
    /// `--options` button or a brief `k2-options` item). An option pick
    /// ANSWERS the ticket; free text (false / absent) opens a discussion.
    /// Ignored on agent-authored comments.
    option_pick: bool,
    /// Files on this message (0.46.6, TK4/TK5). With files, `body` may be
    /// empty (not on an option pick, which needs the option's text).
    attachments: Option<serde_json::Value>,
}

/// The status a HUMAN comment moves a ticket to. An option pick is the
/// answer (`answered`); free text means the person wants to talk it over
/// (`needs_discussion`). The agent settles a discussion later with
/// `k2 tickets resolve <id> --answered "<outcome>"` or plain `resolve`.
pub(crate) fn human_comment_status(option_pick: bool) -> &'static str {
    if option_pick {
        "answered"
    } else {
        "needs_discussion"
    }
}

/// One ticket message, checked and ready to store.
pub(crate) struct CommentInput<'a> {
    /// The ticket as loaded (FULL id), after the door's access check.
    pub ticket: FeedbackItem,
    /// The stored name and the credential's Thread stamp.
    pub by: CommentBy,
    /// A person (or an App guest): the ticket rules for people apply.
    pub is_human: bool,
    pub skin: Option<&'a SkinPass>,
    pub body: String,
    /// Snapshots checked against the TICKET's workspace, and its root.
    pub files: Vec<ThreadAttachment>,
    pub files_root: Option<PathBuf>,
    /// Who, on the attachment audit line.
    pub who: String,
    pub option_pick: bool,
    /// A Thread post's send-once key.
    pub client: Option<ClientMsgKey>,
}

/// What [`comment_core`] did.
pub(crate) enum CommentDone {
    Stored {
        item: FeedbackItem,
        comment: FeedbackComment,
        /// The message's Thread seq in `ticket:<id>`.
        seq: i64,
        answered: bool,
        /// `(delivered, reason, session)` for a person's message; `None`
        /// for an agent's (stored only).
        delivery: Option<(bool, Option<String>, Option<String>)>,
    },
    /// The send-once key was kept already: nothing stored, nothing sent.
    Duplicate(ClientIdHit),
}

fn store_message(input: &CommentInput<'_>) -> Result<Result<(FeedbackComment, i64), ClientIdHit>, CliResponse> {
    match feedback::add_comment_by(&input.ticket.id, &input.by, &input.body, &input.files, input.client.as_ref()) {
        Ok(CommentWrite::Stored(c, seq)) => Ok(Ok((c, seq))),
        Ok(CommentWrite::Duplicate(hit)) => Ok(Err(hit)),
        Err(e) => Err(usage_error(e)),
    }
}

/// Store one ticket message and run every ticket rule (module docs).
pub(crate) fn comment_core(input: CommentInput<'_>) -> Result<CommentDone, CliResponse> {
    let id = input.ticket.id.clone();
    if !input.is_human {
        let (c, seq) = match store_message(&input)? {
            Ok(pair) => pair,
            Err(hit) => return Ok(CommentDone::Duplicate(hit)),
        };
        crate::ticket_attachments::audit_post(&id, &c.id, &input.who, &c.attachments);
        fr::emit_commented(&id, &c.author);
        let item = feedback::get_item(&id).ok_or_else(|| usage_error("feedback row vanished after comment"))?;
        fr::emit_ticket_changed(&item, fr::CHANGE_COMMENTED, Some(fr::VIA_AGENT));
        // Push assignees (or all devices if unassigned) on thread activity.
        crate::push_routes::notify_feedback_commented(&id, &item.assignees);
        return Ok(CommentDone::Stored { item, comment: c, seq, answered: false, delivery: None });
    }

    // A person. An OPTION PICK is the answer (status answered, --wait
    // prints it). FREE TEXT opens a discussion: the ticket moves to
    // needs_discussion (a resolved one reopens) until the agent settles it.
    let before = input.ticket.clone();
    let answers = input.option_pick;
    let target_status = human_comment_status(answers);
    let (comment, seq) = match store_message(&input)? {
        Ok(pair) => pair,
        Err(hit) => return Ok(CommentDone::Duplicate(hit)),
    };
    let item = if answers {
        feedback::record_answer(&id, &comment.body).map_err(usage_error)?
    } else if before.status == target_status {
        feedback::get_item(&id).ok_or_else(|| usage_error("feedback row vanished after comment"))?
    } else {
        feedback::set_status(&id, target_status).map_err(usage_error)?
    };
    let status_changed = !answers && before.status != item.status;

    // FeedbackAnswered BEFORE the injection so `ask --wait` pollers
    // unblock even if delivery is slow (a wake can take seconds).
    let (_, path) = fr::project_name_path(&item.project_id);
    if answers {
        k2_core::agent_hooks::emit(
            k2_core::agent_hooks::HookEvent::FeedbackAnswered,
            serde_json::json!({ "id": item.id, "projectPath": path }),
        );
    }
    // Free text moved the ticket to needs_discussion: every window's
    // list + waiting-count badge refresh on the status bus.
    if status_changed {
        k2_core::agent_hooks::emit(
            k2_core::agent_hooks::HookEvent::FeedbackStatusChanged,
            serde_json::json!({ "id": item.id, "projectPath": path, "status": item.status }),
        );
    }
    // feedback:commented rides every stored comment, before the injection,
    // so an open thread panel refreshes without waiting on a slow wake.
    fr::emit_commented(&item.id, &comment.author);
    if answers {
        fr::emit_ticket_changed(&item, fr::CHANGE_ANSWERED, Some(fr::VIA_OPTION_PICK));
    } else {
        fr::emit_ticket_changed(&item, fr::CHANGE_COMMENTED, Some(fr::VIA_FREE_TEXT));
    }
    crate::push_routes::notify_feedback_commented(&item.id, &item.assignees);
    crate::ticket_attachments::audit_post(&item.id, &comment.id, &input.who, &comment.attachments);

    // Shared F3 delivery: the stored author IS the stamp (0.46.0 rule).
    // Files reach the agent as `Attached: <abs path>` lines, never stored.
    let delivery = fr::deliver_ticket_words(
        &item,
        input.skin,
        &comment.author,
        &comment.body,
        input.files_root.as_deref(),
        &comment.attachments,
    );
    Ok(CommentDone::Stored { item, comment, seq, answered: answers, delivery: Some(delivery) })
}

/// The Thread stamp of a person's ticket message: an App guest's (as its
/// Thread posts), the owner token's, else the Connect login's.
pub(crate) fn person_stamp(skin: Option<&SkinPass>, session_author: &str) -> PostAuthor {
    match skin {
        Some(pass) => PostAuthor::guest(&crate::overlay_routes::skin_from_stamp(pass)),
        None => {
            let s = session_author.trim();
            if s.is_empty() || crate::session_author::is_owner_token(s) {
                PostAuthor::owner()
            } else {
                PostAuthor::user(s)
            }
        }
    }
}

/// Handler for `POST /cli/feedback/comment` (see `feedback_routes`'s docs
/// on `handle_comment_gated`). Same request and answer as before 0.46.17.
pub(crate) fn handle(body: &[u8], session_author: &str, skin: Option<&SkinPass>, agent: Option<&str>) -> CliResponse {
    let b: CommentBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return usage_error(format!("invalid JSON body: {e}")),
    };
    if b.id.is_empty() {
        return usage_error("missing 'id' (a feedback id or unique prefix)");
    }
    let attach_reqs = match crate::ticket_attachments::parse_field(b.attachments.as_ref()) {
        Ok(r) => r,
        Err(e) => return e,
    };
    // TK5: files may stand in for the text, except on an option pick.
    if b.body.trim().is_empty() && (attach_reqs.is_empty() || b.option_pick) {
        return usage_error("comment requires non-empty <text>");
    }
    let (full_id, author, is_human, stamp) = if let Some(pass) = skin {
        let item = match fr::skin_load_item(pass, &b.id, crate::skin_routes::TICKETS_POST) {
            Ok(item) => item,
            Err(e) => return e,
        };
        // Skin: session_author = pass.username. Never body `author`, never "owner".
        (item.id, pass.username.clone(), true, Some(person_stamp(skin, "")))
    } else {
        let full_id = match feedback::resolve_id_prefix(&b.id) {
            Ok(f) => f,
            Err(e) => return prefix_error_response(&b.id, e),
        };
        // Body `author` is the agent-CLI path; an agent's passport (`agent`) is
        // always its stamped name, never a person. UI human posts omit it: the
        // token identity, so a Connect user is stored as themselves, not the owner.
        let author_given = agent.or_else(|| b.author.as_deref().map(str::trim).filter(|s| !s.is_empty()));
        let session = session_author.trim();
        let author = author_given.unwrap_or(if session.is_empty() { "owner" } else { session });
        let is_human = agent.is_none() && (author_given.is_none() || author == "owner");
        // 0.46.0 stamp rule: a human comment is stored as the sender's
        // username (the owner token → the linked username, else `owner`).
        let (author, stamp) = if is_human {
            (
                k2_core::person_name::sender_identity_now(k2_core::person_name::Sender::from_session_author(author)),
                Some(person_stamp(None, session)),
            )
        } else {
            (author.to_string(), agent.map(PostAuthor::agent))
        };
        (full_id, author, is_human, stamp)
    };

    // Files: checked and snapshotted against the TICKET's workspace (not
    // the caller's) before anything is stored (TK3/TK4).
    let Some(ticket) = feedback::get_item(&full_id) else {
        return prefix_error_response(&b.id, PrefixError::NotFound);
    };
    let who = match skin {
        Some(p) => format!("guest:{}", p.username.trim()),
        None => author.clone(),
    };
    let (files, files_root) =
        match crate::ticket_attachments::check_and_snapshot(&full_id, &ticket.project_id, skin, &attach_reqs, &who) {
            Ok(pair) => pair,
            Err(e) => return e,
        };
    let project_id = ticket.project_id.clone();
    let input = CommentInput {
        ticket,
        by: CommentBy { name: author, author: stamp },
        is_human,
        skin,
        body: b.body.clone(),
        files,
        files_root,
        who,
        option_pick: b.option_pick,
        client: None,
    };
    let files_view = |atts: &[ThreadAttachment]| {
        serde_json::Value::Array(crate::ticket_attachments::views(&project_id, atts, skin, true))
    };
    match comment_core(input) {
        Err(e) => e,
        Ok(CommentDone::Duplicate(_)) => usage_error("comment: a keyless post found a client key"),
        // Agent comment — store only, current shape (no delivery fields).
        Ok(CommentDone::Stored { item, comment, delivery: None, .. }) => CliResponse::ok_json(
            serde_json::json!({
                "ok": true,
                "id": item.id,
                "commentId": comment.id,
                "author": comment.author,
                "attachments": files_view(&comment.attachments),
            })
            .to_string(),
        ),
        Ok(CommentDone::Stored { item, comment, answered, delivery: Some((delivered, reason, session)), .. }) => {
            CliResponse::ok_json(
                serde_json::json!({
                    "ok": true,
                    "id": item.id,
                    "commentId": comment.id,
                    "author": comment.author,
                    "answered": answered,
                    "status": item.status,
                    "delivered": delivered,
                    "deliveryReason": reason,
                    "deliveredSessionId": session,
                    "attachments": files_view(&comment.attachments),
                })
                .to_string(),
            )
        }
    }
}
