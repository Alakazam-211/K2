//! The Thread attachment door (`GET /cli/thread/attachment`, `POST
//! /cli/thread/attachment/token`): the Thread's own read gate, then
//! `thread_attachments` adds the viewer's file rights. A ticket's Thread
//! (`ticket:<id>`, 0.46.17) gates on the ticket's rights (`ticket_addr`).

use super::*;

/// `GET /cli/thread/attachment` (thread-attachments-v1): the Thread's own
/// read gate (an App guest: its room's pinned Chat and `thread:read`),
/// then `thread_attachments::handle_get` adds the viewer's file rights.
/// Agent passports are refused: an agent reads its workspace's files from
/// disk.
pub(super) fn handle_get_attachment(params: &HashMap<String, String>) -> CliResponse {
    attachment_door(params, false)
}

/// `POST /cli/thread/attachment/token` (0.46.9): the attachment door's own
/// read gate, then a download token (`thread_attachments::handle_token`).
pub(super) fn handle_attachment_token(params: &HashMap<String, String>) -> CliResponse {
    attachment_door(params, true)
}

fn attachment_door(params: &HashMap<String, String>, token: bool) -> CliResponse {
    let addr = opt_param(params, "addr")
        .filter(|s| !s.trim().is_empty())
        .or_else(|| opt_param(params, "conversationId"))
        .unwrap_or_default();
    if addr.trim().is_empty() {
        return usage("missing addr (or conversationId)");
    }
    if crate::caller_workspace::principal_from_params(params).is_some() {
        return forbidden("an agent reads attached files from disk; this door is for people");
    }
    let resolved = match resolve_thread_addr(&addr) {
        Ok(r) => r,
        Err(e) => return e,
    };
    // This route's refusals all carry a code (`cap_not_granted`, and the
    // resolver's own `skin_room`), so a phone never string-matches.
    if require_skin_cap_in_resolved(&resolved, k2_core::skin::CAP_THREAD_READ).is_err() {
        return crate::thread_attachments::cap_refused(k2_core::skin::CAP_THREAD_READ);
    }
    let pass = request_skin();
    let thread = crate::thread_attachments::ResolvedThread {
        conversation_id: resolved.conversation_id.clone(),
        project_id: resolved.project_id.clone(),
    };
    if token {
        crate::thread_attachments::handle_token(params, &thread, pass.as_ref())
    } else {
        crate::thread_attachments::handle_get(params, &thread, pass.as_ref())
    }
}
