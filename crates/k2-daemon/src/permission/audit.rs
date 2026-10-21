//! A2 — Access Audit lines for permission cards (PRD rev 3.1 §6.5).
//!
//! Every event is one `auth_audit` line: `event` is one of
//! `k2_core::permission::audit_event` (`permission-request|answer|note|undo|
//! switch`), `user` who did it (`owner`, `login:<name>`, `agent:<handle>`,
//! `k2`), and `outcome` `"<request id> <action> <state> via=<via>"` plus,
//! for a note or an undo, what happened. Settings → Access Audit filters
//! on the `permission-` prefix (lane D2).
//!
//! A1 writes the request and answer lines (`answer::audit`, same shape);
//! lane B writes `permission-switch`; this module writes note and undo. The
//! note's text never goes into the audit (only that one was written,
//! replaced or deleted).

use k2_core::permission::audit_event;
use k2_core::permission::request::PermissionRequest;

use super::actions::Answerer;

/// `(user, via)` for a person: `owner` / `owner-token`, `login:<name>` /
/// `login` (A1's convention).
pub(crate) fn who(answerer: &Answerer) -> (String, &'static str) {
    match answerer {
        Answerer::OwnerToken => ("owner".to_string(), "owner-token"),
        Answerer::Login { user, .. } => (format!("login:{user}"), "login"),
    }
}

fn line(event: &str, user: &str, via: &str, req: &PermissionRequest, what: &str) {
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
        event,
        user,
        format!("{} {} {} via={via} {what}", req.display_id(), req.action, req.state.as_str()),
        "-",
        "-",
        "cli",
    ));
}

/// What a note write did (never the text).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NoteChange {
    Written,
    Replaced,
    Deleted,
}

impl NoteChange {
    fn as_str(self) -> &'static str {
        match self {
            NoteChange::Written => "note=written",
            NoteChange::Replaced => "note=replaced",
            NoteChange::Deleted => "note=deleted",
        }
    }
}

/// `permission-note`.
pub(crate) fn note(by: &Answerer, req: &PermissionRequest, change: NoteChange) {
    let (user, via) = who(by);
    line(audit_event::NOTE, &user, via, req, change.as_str());
}

/// `permission-undo`: "Turn off again" ran (`applied`) or the twin's route
/// entry refused it.
pub(crate) fn undo(by: &Answerer, req: &PermissionRequest, twin: &str, outcome: &str) {
    let (user, via) = who(by);
    line(audit_event::UNDO, &user, via, req, &format!("twin={twin} undo={outcome}"));
}
