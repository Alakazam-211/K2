//! `/cli/sew/owner-sessions*`: the owner chat sessions K2 opened with an
//! owner handoff code, listed and ended by a person
//! (`k2 hostchat owner-sessions`).
//!
//! - `GET /cli/sew/owner-sessions`: K2's record
//!   ([`crate::chat_pass::owner_sessions`]): sid, who minted the code,
//!   when it was minted and redeemed, and whether an end was queued and
//!   reached Sew.
//! - `POST /cli/sew/owner-sessions/revoke`, body `{"sid":"own_…"}` or
//!   `{"all":true}`, plus `confirmToken`: queues one `sew_session_revoke`
//!   on S27's outbox: `k2_sid` = that sid, or `external_id` = `k2:owner`
//!   for all (which also ends the sessions of the owner's linked Connect
//!   logins). Sew ends a revoked session at once (its sockets close, its
//!   next call is refused); the only wait is the outbox's delivery.
//!
//! **Who:** the owner token or an Owner/Admin Connect login. Agents and
//! K2 shell tabs get 403 `person_only` (and these paths are never agent
//! verbs on the cell socket); app passes and API keys 403
//! `wrong_credential`. **Ending is person-only:** a fresh vault confirm
//! for action `chat-owner-sessions-end`, bound to the sid or `all`
//! (403 `confirm_required` names both, and `k2 hostchat` asks for it
//! from a terminal outside K2).

use serde::Deserialize;
use serde_json::{json, Value};

use k2_core::connect_users::Role;
use k2_core::sew_revoke::SewSessionRevoke;

use super::routes::{credential, internal, now, parse_body, usage, ChatErr, R};
use crate::chat_pass::owner_sessions::{self, Entry, Via};
use crate::compute_routes::Who;

pub const LIST: &str = "/cli/sew/owner-sessions";
pub const REVOKE: &str = "/cli/sew/owner-sessions/revoke";

/// The routes this module serves.
pub const ROUTES: &[&str] = &[LIST, REVOKE];
/// POST-only rows (405 on GET).
pub const POST_ROUTES: &[&str] = &[REVOKE];

/// How soon an end takes effect (the outbox worker's timing: an enqueue
/// wakes it, failures retry at 2 s doubling to every 5 minutes, with no
/// limit on attempts).
pub const TAKES_EFFECT: &str = "Ending takes effect as soon as Sew receives it, normally within seconds. If Sew is down, \
     the end stays queued and is retried (after 2 s, doubling up to every 5 minutes) until Sew takes it.";

/// A person who may see and end the owner's chat sessions.
struct Person {
    /// `owner` or `login:<username>` (the record and the audit).
    label: String,
    /// The vault confirm's actor stamp.
    stamp: String,
    /// The audit's actor kind.
    kind: &'static str,
}

fn person(who: &Who) -> R<Person> {
    credential(who)?;
    match who {
        Who::Owner => Ok(Person { label: "owner".into(), stamp: "owner".into(), kind: "owner_token" }),
        Who::Login { username, role } if *role >= Role::Admin => {
            Ok(Person { label: format!("login:{username}"), stamp: format!("connect:{username}"), kind: "login" })
        }
        Who::Agent { .. } | Who::Shell { .. } => Err(ChatErr::new(
            "person_only",
            403,
            "the owner's chat sessions are for a person to see and end, never an agent",
        )),
        _ => Err(ChatErr::new("owner_only", 403, "only the owner or an admin sees and ends the owner's chat sessions")),
    }
}

pub(crate) fn handle(path: &str, who: &Who, body: &[u8]) -> R<Value> {
    let p = person(who)?;
    match path {
        LIST => list(),
        REVOKE => revoke(&p, body),
        _ => Err(ChatErr::new("not_found", 404, "no such route")),
    }
}

/// The outbox op an end of `e` was queued as.
fn op_for(sid: &str, via: Via) -> SewSessionRevoke {
    match via {
        Via::Sid => SewSessionRevoke::K2Sid(sid.to_string()),
        Via::All => SewSessionRevoke::ExternalId(crate::chat_pass::identity::OWNER_SUB.to_string()),
    }
}

fn entry_json(e: &Entry) -> R<Value> {
    let mut v = json!({
        "sid": e.sid,
        "mintedBy": e.minted_by,
        "mintedAt": e.minted_at,
        "redeemedAt": e.redeemed_at,
        "endQueued": e.end.is_some(),
        "delivered": false,
    });
    if let Some(end) = &e.end {
        let state = k2_core::sew_revoke::outbox_state(&op_for(&e.sid, end.via), end.queued_at).map_err(internal)?;
        v["delivered"] = json!(state.as_deref() == Some(k2_core::tool_hook::outbox::STATE_DONE));
        v["end"] = json!({
            "queuedAt": end.queued_at,
            "via": end.via,
            "by": end.by,
            "outbox": state,
        });
    }
    Ok(v)
}

fn list() -> R<Value> {
    let rows = owner_sessions::list(now()).map_err(internal)?;
    let items = rows.iter().map(entry_json).collect::<R<Vec<_>>>()?;
    Ok(json!({ "ok": true, "sessions": items, "takesEffect": TAKES_EFFECT }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RevokeBody {
    #[serde(default)]
    sid: Option<String>,
    #[serde(default)]
    all: bool,
    #[serde(default)]
    confirm_token: Option<String>,
}

/// `own_` + 32 lowercase hex ([`crate::chat_pass::handoff::sid_for`]).
fn is_own_sid(s: &str) -> bool {
    s.strip_prefix("own_").is_some_and(|h| h.len() == 32 && h.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f')))
}

fn revoke(p: &Person, body: &[u8]) -> R<Value> {
    let b: RevokeBody = parse_body(body)?;
    let sid = b.sid.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let (op, secret) = match (sid, b.all) {
        (Some(s), false) => {
            if !is_own_sid(s) {
                return Err(usage(format!("{s:?} is not an owner session's sid (own_ and 32 hex characters)")));
            }
            (op_for(s, Via::Sid), s)
        }
        (None, true) => (op_for("", Via::All), "all"),
        _ => return Err(usage("name one session ({\"sid\":\"own_…\"}) or every one ({\"all\":true})")),
    };
    let t = now();
    if let Some(s) = sid {
        // Before the confirm is spent.
        if owner_sessions::get(s, t).map_err(internal)?.is_none() {
            return Err(ChatErr::new(
                "not_found",
                404,
                format!(
                    "{} has no record of owner session {s} (records last 30 days); `end --all` ends every owner session, listed or not",
                    k2_core::brand::PRODUCT_NAME
                ),
            ));
        }
    }
    let method =
        super::routes_approvals::take_confirm(&p.stamp, b.confirm_token.as_deref(), crate::chat_pass::handoff::END_CONFIRM_ACTION, secret)?;
    let queued = k2_core::sew_revoke::enqueue_alone(&op).map_err(internal)?;
    if !queued {
        return Err(ChatErr::new(
            "chat_not_set_up",
            409,
            "chat and voice was never set up on this box, so no owner chat session is live here; nothing was queued",
        ));
    }
    let ended = owner_sessions::mark_end(sid, &p.label, t).map_err(internal)?;
    let admin = super::routes_admin::Admin::Person { label: p.label.clone(), kind: p.kind };
    super::routes_admin::audit(&admin, None, "owner_sessions.end", None, Some(secret), Some(method.clone()), "ok", None)?;
    Ok(json!({
        "ok": true,
        "queued": op.label(),
        "ended": ended,
        "confirmedBy": method,
        "takesEffect": TAKES_EFFECT,
    }))
}
