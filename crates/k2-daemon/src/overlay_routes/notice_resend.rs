//! `POST /cli/thread/resend` (thread-system-notice-v1, 0.46.15): "Send
//! again" on K2's delivery notice.
//!
//! When a Thread line may not have reached the agent (typed in twice and
//! never read in, the retry couldn't be typed in, K2 restarted mid-type or
//! with the line over an hour old), K2 posts a delivery notice
//! (`systemNotice.kind = "delivery"`). Its first action is
//! `{id: "resend", route: "/cli/thread/resend", body: {thread, doc}}`; K2
//! kept the delivery beside the notice when it posted it.
//!
//! Body `{thread, doc}`: the Thread's address (`addr` is accepted too) and
//! the notice's doc id.
//!
//! - **Who:** a person who may post in that Thread (the owner token or a
//!   Connect login, Member and up). An agent passport is refused
//!   (`person_only`: the notice is for the person who sent the line), and
//!   so is an App pass (the route has no catalog row; the dispatcher's
//!   Thread arm refuses it first).
//! - **What:** the original line (the sender's own post, as it was typed
//!   in the first time) is queued again on the agent's delivery lane as a
//!   fresh first delivery: it gets its own take check and one retry, and a
//!   notice of its own if that is not taken either. A post the person
//!   deleted since is never sent (409 `source_deleted`).
//! - **Once:** the first press marks the button done (`doneAt`, `doneBy`)
//!   in the store write that takes the kept delivery, and one Thread frame
//!   carries the changed notice. A second press is 409 `already_resent`
//!   with the first press's `doneAt`, and sends nothing.

use k2_core::product_name;

use super::*;

pub(crate) const ROUTE: &str = k2_core::overlay::doc::NOTICE_RESEND_ROUTE;

pub(crate) fn handle(params: &HashMap<String, String>, session_author: &str) -> CliResponse {
    let thread = opt_param(params, "thread").or_else(|| opt_param(params, "addr")).unwrap_or_default();
    if thread.trim().is_empty() {
        return usage("missing thread (the Thread's address)");
    }
    let doc_id = opt_param(params, "doc").map(|s| s.trim().to_string()).unwrap_or_default();
    if doc_id.is_empty() {
        return usage("missing doc (the notice's id)");
    }
    if principal_is_bound(params) || crate::caller_workspace::principal_from_params(params).is_some() {
        return error_json("403 Forbidden", "person_only", "Send again is for the person who sent the message; an agent can't press it");
    }
    if request_skin().is_some() {
        return error_json("403 Forbidden", "person_only", concat!("Send again is for people signed in to ", product_name!(), ", not App passes"));
    }
    let resolved = match resolve_thread_addr(&thread) {
        Ok(r) => r,
        Err(e) => return e,
    };
    if let Some(r) = refuse_if_imported(&resolved, Some(&doc_id)) {
        return r;
    }
    let Some(by) = card_actor_author(params, session_author) else {
        return error_json("403 Forbidden", "person_only", concat!("Send again is for people signed in to ", product_name!()));
    };
    // A post deleted since is never sent again (and the button stays, so
    // nothing claims a send that didn't happen).
    match overlay::notice_resend_job(&doc_id) {
        Ok(Some(json)) => match serde_json::from_str::<ThreadDeliveryJob>(&json) {
            Ok(job) if job.source_deleted() => {
                return error_json("409 Conflict", "source_deleted", concat!("the message was deleted, so ", product_name!(), " won't send it again"))
            }
            Ok(_) => {}
            Err(e) => return error_json("500 Internal Server Error", "store", format!("unreadable delivery: {e}")),
        },
        Ok(None) => {}
        Err(e) => return error_json("500 Internal Server Error", "store", e),
    }
    let claim = match overlay::claim_notice_resend(&resolved.conversation_id, &doc_id, by) {
        Ok(c) => c,
        Err(e) if e == overlay::NOTICE_NOT_FOUND => return not_found(format!("no notice {doc_id} on this Thread")),
        Err(e) if e == overlay::NOTICE_NOT_RESENDABLE => {
            return error_json("409 Conflict", "not_resendable", concat!("this isn't a ", product_name!(), " notice with a message to send again"))
        }
        Err(e) => return error_json("500 Internal Server Error", "store", e),
    };
    let (item, job) = match claim {
        overlay::ResendClaim::Already { done_at, .. } => {
            return CliResponse {
                status: "409 Conflict",
                content_type: "application/json",
                body: serde_json::json!({
                    "ok": false,
                    "error": {"code": "already_resent", "hint": concat!(product_name!(), " already sent this message again")},
                    "doneAt": done_at,
                })
                .to_string(),
            }
        }
        overlay::ResendClaim::Claimed { item, job } => (item, job),
    };
    let mut job: ThreadDeliveryJob = match serde_json::from_str(&job) {
        Ok(j) => j,
        Err(e) => return error_json("500 Internal Server Error", "store", format!("unreadable delivery: {e}")),
    };
    job.sink = delivery_sink();
    let link = k2_core::overlay::OverlayLink {
        collection: "thread",
        seq: item.seq,
        id: item.id.clone(),
        conversation_id: item.conversation_id.clone(),
    };
    crate::overlay_ws::emit_links(&[link], &item.doc);
    k2_core::log_debug!("[thread-delivery] Send again pressed on notice {} addr={}", item.id, job.addr);
    let done_at = item.doc.system_notice.as_ref().and_then(|n| n.resend()).and_then(|a| a.done_at);
    deliver_thread_later(job);
    CliResponse::ok_json(with_moved_from(
        serde_json::json!({
            "ok": true,
            "resent": true,
            "id": item.id,
            "seq": item.seq,
            "doneAt": done_at,
            "conversation_id": resolved.conversation_id,
            "addr": resolved.addr,
        }),
        &resolved,
    ))
}
