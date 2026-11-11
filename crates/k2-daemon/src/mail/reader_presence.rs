//! `GET /cli/mail/reader/presence` — does this server have mail at all?
//! (Rosson, 2026-11-10: "The mail icon should only pop up when/if there is
//! a linked inbox or if there is mail hosting enabled.")
//!
//! The client's Mail tab shows for an Owner, Admin or Member login only
//! when this answers `hosting` or `linkedInboxes`. Local state only: one
//! read of the `mail_server` row, one `EXISTS` over the linked inboxes and
//! the reader's own inbox catalog for `visible`. No JMAP, IMAP or Graph
//! call, and no access-log row (it opens no inbox; it counts them).
//!
//! Same door as every reader route: the owner token or a Connect login at
//! Member or above (`route_policy`); an agent passport gets 403
//! `owner_only` (the dispatcher arm and the cell socket).
//!
//! **Live.** Hosting turning on or off already reaches clients as
//! `mail_changed` (`server-state-changed`, from the supervisor's state
//! hook). Linking or unlinking an inbox emits `mail_changed` with
//! [`INBOXES_CHANGED`] ([`inboxes_changed`], from `mail::external`), so the
//! tab follows without a reload.

use crate::cli_response::CliResponse;
use crate::mail::mailbox_guard;
use crate::mail::routes_reader;

pub const ROUTE: &str = "/cli/mail/reader/presence";

/// The `mail_changed` reason for a linked inbox added or removed.
pub const INBOXES_CHANGED: &str = "inboxes-changed";

/// Hosted mail is on for this box: the `mail_server` row exists and the
/// owner hasn't disabled it. Installing, running, degraded, stopped and
/// error all count (the owner turned it on; the Email page explains the
/// state). No row = never installed or uninstalled.
fn hosting_enabled(conn: &rusqlite::Connection) -> bool {
    conn.query_row("SELECT status FROM mail_server WHERE id = 1", [], |r| r.get::<_, String>(0))
        .map(|status| status != "disabled")
        .unwrap_or(false)
}

/// Any linked inbox (Gmail, Microsoft, IMAP) on this server, whoever's.
fn any_linked_inbox(conn: &rusqlite::Connection) -> bool {
    conn.query_row("SELECT EXISTS(SELECT 1 FROM mail_external_inboxes)", [], |r| r.get::<_, i64>(0))
        .map(|n| n != 0)
        .unwrap_or(false)
}

/// GET: `{ ok, hosting, linkedInboxes, visible }`. `visible` is how many
/// inboxes this viewer could open in the reader (the reader rule: the
/// Owner and Admins every inbox, anyone else their own person mailboxes).
pub fn handle() -> CliResponse {
    let Some(viewer) = mailbox_guard::viewer_info() else {
        return routes_reader::agent_refusal();
    };
    let (hosting, linked) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        (hosting_enabled(&conn), any_linked_inbox(&conn))
    };
    let visible = routes_reader::catalog()
        .iter()
        .filter(|i| viewer.opens_any || i.own(viewer.person.as_deref()))
        .count();
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "hosting": hosting,
            "linkedInboxes": linked,
            "visible": visible,
        })
        .to_string(),
    )
}

/// Tell clients a linked inbox was added or removed (refetch signal only:
/// no address on the bus).
pub fn inboxes_changed() {
    let _ = crate::session_events::emit(crate::session_events::SessionEvent::MailChanged {
        reason: INBOXES_CHANGED.to_string(),
    });
}
