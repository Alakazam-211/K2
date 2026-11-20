//! Sew S28 §12 / K2 work item K8: `sew_session_revoke` ops.
//!
//! When a K2 session that could hold a Sew chat session dies (an App
//! logout, an owner revoking a pass, a password change, a removed person,
//! a Connect logout or "log out everywhere"), Sew must end the matching
//! Sew sessions. K2 queues one op per death:
//!
//! ```text
//! op      = "sew_session_revoke"
//! payload = {"k2_sid": "skn_…" | "cu_…"}  or  {"external_id": "k2:principal:<id>"}
//! ```
//!
//! **The outbox is S27's.** S27 owns the `skin.db` table `tool_outbox`
//! and the worker that calls Sew's `POST /mgmt/v1/sessions/revoke`
//! (`Idempotency-Key` = the row's key; `200 {revoked: n}` for any `n`, or
//! a 404, completes the op; anything else retries with S27's backoff).
//! Every revoke site calls [`enqueue`] **inside the same Immediate
//! `skin.db` transaction as the revoke write** (S28-A3), passing that
//! connection, so the outbox row commits or rolls back with the revoke.
//!
//! The writer is S27's outbox ([`crate::tool_hook::enqueue_session_revoke`])
//! unless a test installs its own [`Sink`] ([`set_sink`]). On a box where
//! Sew was never set up (no chat target row) the op is logged (never a
//! secret; a sid is a hash) and dropped: no Sew session can exist there.
//!
//! Connect sessions live in a JSON file, not `skin.db`, so their ops follow
//! the write instead of sharing its transaction
//! ([`crate::skin::enqueue_sew_revokes`]).

use std::sync::{Arc, RwLock};

use rusqlite::Connection;
use serde_json::{json, Value};

/// The outbox op kind (reserved by S27, A12).
pub const OP_KIND: &str = "sew_session_revoke";

/// One revoke op. Exactly one of the two payload forms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SewSessionRevoke {
    /// End the Sew sessions minted from one K2 session (`skn_…`, `cu_…`).
    K2Sid(String),
    /// End every Sew session of one external id (`k2:principal:<id>`).
    ///
    /// `k2:owner` ends EVERY owner session, including the ones minted from
    /// the owner's linked Connect login (its passes name `k2:owner` with a
    /// `cu_…` sid). That is intended: one person, one member. To end just
    /// one of those, revoke its `K2Sid` (`cu_…`) instead.
    ExternalId(String),
    /// End every Sew session whose pass carried this `app` claim (Sew
    /// 0.6.0 I8, flag `app_governed_spaces`): an App's chat key was
    /// revoked, or the App was uninstalled. Never sent on a key rotation.
    AppId(String),
}

impl SewSessionRevoke {
    /// The outbox payload: `{"k2_sid": …}` or `{"external_id": …}`.
    pub fn payload(&self) -> Value {
        match self {
            SewSessionRevoke::K2Sid(s) => json!({ "k2_sid": s }),
            SewSessionRevoke::ExternalId(e) => json!({ "external_id": e }),
            SewSessionRevoke::AppId(a) => json!({ "app_id": a }),
        }
    }

    /// Log-safe one-liner (a sid is a hash prefix or a row id, never a
    /// secret).
    pub fn label(&self) -> String {
        match self {
            SewSessionRevoke::K2Sid(s) => format!("k2_sid={s}"),
            SewSessionRevoke::ExternalId(e) => format!("external_id={e}"),
            SewSessionRevoke::AppId(a) => format!("app_id={a}"),
        }
    }
}

/// The `skn_` sid of a skin login session row (`tokens.id`).
pub fn skin_sid(token_id: &str) -> String {
    format!("skn_{token_id}")
}

/// The external id of a skin principal.
pub fn principal_external_id(principal_id: &str) -> String {
    format!("k2:principal:{principal_id}")
}

/// Writes one op into the outbox, inside the caller's open transaction on
/// `skin.db` (`conn`). An error rolls the whole revoke back.
pub type Sink = Arc<dyn Fn(&Connection, &SewSessionRevoke) -> Result<(), String> + Send + Sync>;

fn sink_slot() -> &'static RwLock<Option<Sink>> {
    static SLOT: RwLock<Option<Sink>> = RwLock::new(None);
    &SLOT
}

/// Replace the outbox writer (tests record ops this way). Replaces any
/// previous sink; [`clear_sink`] goes back to S27's outbox.
pub fn set_sink(sink: Sink) {
    *sink_slot().write().unwrap_or_else(|p| p.into_inner()) = Some(sink);
}

/// Back to the real writer, S27's outbox (tests).
pub fn clear_sink() {
    *sink_slot().write().unwrap_or_else(|p| p.into_inner()) = None;
}

/// Whether a test sink replaced S27's outbox.
pub fn sink_installed() -> bool {
    sink_slot().read().unwrap_or_else(|p| p.into_inner()).is_some()
}

/// Whether an op could be queued at all, without opening `skin.db`: a test
/// sink, or a `skin.db` that exists (the chat target lives there). Callers
/// outside `skin.db` (Connect logouts) use it so a box without Sew never
/// creates `skin.db` for this.
pub fn writer_available() -> bool {
    sink_installed() || crate::tool_hook::skin_store_exists()
}

/// Queue one op. `conn` must be inside the revoke's transaction. Without a
/// chat target (Sew never set up here) the op is logged and dropped.
pub fn enqueue(conn: &Connection, op: &SewSessionRevoke) -> Result<(), String> {
    if !enqueue_reporting(conn, op)? {
        crate::log_debug!("[sew_revoke] {OP_KIND} {} not queued: Sew is not set up on this box", op.label());
    }
    Ok(())
}

/// [`enqueue`], saying whether the op was queued: `Ok(false)` when Sew was
/// never set up here (no chat target row), so nothing was written.
pub fn enqueue_reporting(conn: &Connection, op: &SewSessionRevoke) -> Result<bool, String> {
    let sink = sink_slot().read().unwrap_or_else(|p| p.into_inner()).clone();
    if let Some(f) = sink {
        f(conn, op)?;
        return Ok(true);
    }
    crate::tool_hook::enqueue_session_revoke(conn, op)
}

/// Queue one op on its own, outside any other write (an owner ending owner
/// chat sessions: `k2 hostchat owner-sessions end`), in its own Immediate
/// `skin.db` transaction. `Ok(false)` when Sew was never set up on this
/// box: there is no Sew session to end and nothing was queued.
pub fn enqueue_alone(op: &SewSessionRevoke) -> Result<bool, String> {
    if !writer_available() {
        return Ok(false);
    }
    crate::skin::with_conn(|conn| {
        let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| format!("skin begin immediate: {e}"))?;
        let queued = enqueue_reporting(&tx, op)?;
        tx.commit().map_err(|e| format!("skin sew revoke commit: {e}"))?;
        Ok(queued)
    })
}

/// Where a queued op stands: the state (`done`, `pending`, `blocked`, ...)
/// of S27's outbox rows for `op`'s subject queued at or after `since`
/// (Unix seconds). A `done` row wins (Sew took an end that covers it);
/// else the newest row's state. `None`: no such row, or no `skin.db` here
/// (this read never creates one).
pub fn outbox_state(op: &SewSessionRevoke, since: i64) -> Result<Option<String>, String> {
    use rusqlite::OptionalExtension;
    if !crate::tool_hook::skin_store_exists() {
        return Ok(None);
    }
    let subject = crate::tool_hook::revoke_subject_key(op);
    crate::skin::with_skin_conn(|conn| {
        conn.query_row(
            "SELECT state FROM tool_outbox
             WHERE tool = ?1 AND op = ?2 AND subject_key = ?3 AND created_at >= ?4
             ORDER BY (state = 'done') DESC, seq DESC LIMIT 1",
            rusqlite::params![crate::tool_hook::CHAT_KIND, OP_KIND, subject, since],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| format!("tool_outbox revoke state: {e}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_has_exactly_one_form() {
        assert_eq!(SewSessionRevoke::K2Sid("skn_x".into()).payload(), json!({"k2_sid": "skn_x"}));
        assert_eq!(
            SewSessionRevoke::ExternalId("k2:principal:p".into()).payload(),
            json!({"external_id": "k2:principal:p"})
        );
        assert_eq!(SewSessionRevoke::AppId("svc".into()).payload(), json!({"app_id": "svc"}));
        assert_eq!(skin_sid("abc"), "skn_abc");
        assert_eq!(principal_external_id("p1"), "k2:principal:p1");
    }
}
