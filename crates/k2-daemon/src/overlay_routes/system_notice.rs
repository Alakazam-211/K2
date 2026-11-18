//! K2's own Thread lines (thread-system-notice-v1, 0.46.15): the one write
//! every notice poster goes through, the delivery notice with "Send again"
//! and "Open its terminal", and the refusal that keeps `systemNotice` out
//! of every caller's Thread write. "Send again" itself is
//! [`super::notice_resend`].

use k2_core::product_name;

use super::*;

/// `/boot-status` feature key (thread-system-notice-v1): K2's own Thread
/// lines carry `author: {kind: "system", login: "k2"}` and `systemNotice:
/// {kind, code, actions?}`, and `POST /cli/thread/resend` sends a
/// delivery notice's message again.
pub(crate) const FEATURE_THREAD_SYSTEM_NOTICE: &str = "thread-system-notice-v1";

/// Every K2 notice poster's one write (thread-system-notice-v1): resolve
/// `addr`, post K2's line with `notice` (and the delivery its "Send again"
/// sends, when `resend` is given), publish under the write's lock.
/// Returns the resolved Thread and the doc id.
pub(super) fn post_system_line(
    addr: &str,
    text: &str,
    via: &str,
    notice: overlay::SystemNotice,
    resend: Option<&str>,
) -> Result<(ResolvedOverlay, String), String> {
    let resolved = resolve_addr(addr).map_err(|e| e.body)?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    let (item, links) = overlay::post_system_notice(
        &conn,
        &resolved.conversation_id,
        &resolved.project_id,
        &resolved.addr,
        text,
        via,
        notice,
        resend,
    )?;
    // inbox-catchup-v1 (0.46.3): publish while the write's lock is held,
    // so bus (and `rev`) order is commit order.
    crate::overlay_ws::emit_links(&links, &item.doc);
    drop(conn);
    Ok((resolved, item.id))
}

impl ThreadDeliveryJob {
    /// thread-system-notice-v1 "Send again": the same line as a fresh first
    /// delivery (its own take check and one retry), with no Thread turn of
    /// its own (the first one ran its course). The post it carries stays
    /// named, so a post deleted since is never sent.
    fn for_resend(&self) -> Self {
        let mut job = self.clone();
        job.retry = false;
        job.turn = None;
        job
    }
}

impl NotTaken {
    /// The notice's [`overlay::SystemNotice::code`] (thread-system-notice-v1).
    fn code(self) -> &'static str {
        match self {
            NotTaken::NeverReadIn => "delivery.not_taken.never_read_in",
            NotTaken::RetryFailed => "delivery.not_taken.retry_failed",
            NotTaken::RestartMidDelivery => "delivery.not_taken.restart_mid_delivery",
            NotTaken::RestartTooOld => "delivery.not_taken.restart_too_old",
        }
    }
}

/// A notice in the job's Thread (no inject, no wake, no push): K2's
/// delivery notice (thread-system-notice-v1) with "Send again" (the same
/// line, sent fresh: [`ThreadDeliveryJob::for_resend`]) and "Open its
/// terminal".
pub(super) fn post_not_taken_notice(job: &ThreadDeliveryJob, why: NotTaken) {
    // A deleted post is never quoted back into the Thread.
    if job.source_deleted() {
        return;
    }
    let text = not_taken_text(&job.addr, &notice_quote(&job.payload), why);
    let notice = overlay::SystemNotice::delivery(why.code()).with_open_terminal(&job.addr);
    let resend = match serde_json::to_string(&job.for_resend()) {
        Ok(j) => Some(j),
        Err(e) => {
            k2_core::log_debug!("[thread-delivery] the notice for addr={} gets no Send again: {e}", job.addr);
            None
        }
    };
    match post_system_line(&job.addr, &text, "thread", notice, resend.as_deref()) {
        Ok((_, id)) => k2_core::log_debug!("[thread-delivery] told the Thread ({why:?}) addr={} doc={id}", job.addr),
        Err(e) => k2_core::log_debug!("[thread-delivery] couldn't tell the Thread ({why:?}) addr={}: {e}", job.addr),
    }
}

/// The request keys that would name K2's own notice (thread-system-notice-v1).
pub(crate) const SYSTEM_NOTICE_PARAMS: &[&str] = &["systemNotice", "system_notice"];

/// The `system_notice_forbidden` hint.
const NOTICE_FIELD_REFUSED: &str =
    concat!("only ", product_name!(), " itself posts ", product_name!(), " notices; send the message without systemNotice");

/// 400 `system_notice_forbidden` when a Thread write carries K2's notice
/// field (a query key, a JSON body key of any type, a form body key): only
/// K2 itself writes a notice.
pub(crate) fn refuse_system_notice_param(params: &HashMap<String, String>, body: &[u8]) -> Option<CliResponse> {
    let named = |k: &str| SYSTEM_NOTICE_PARAMS.contains(&k);
    let in_body = match serde_json::from_slice::<serde_json::Value>(body) {
        Ok(serde_json::Value::Object(obj)) => obj.keys().any(|k| named(k)),
        Ok(_) => false,
        Err(_) => crate::routes::http::parse_form_body(body).iter().any(|(k, _)| named(k)),
    };
    (in_body || params.keys().any(|k| named(k))).then(|| {
        error_json("400 Bad Request", "system_notice_forbidden", NOTICE_FIELD_REFUSED)
    })
}
