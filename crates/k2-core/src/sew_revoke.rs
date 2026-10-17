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
    ExternalId(String),
}

impl SewSessionRevoke {
    /// The outbox payload: `{"k2_sid": …}` or `{"external_id": …}`.
    pub fn payload(&self) -> Value {
        match self {
            SewSessionRevoke::K2Sid(s) => json!({ "k2_sid": s }),
            SewSessionRevoke::ExternalId(e) => json!({ "external_id": e }),
        }
    }

    /// Log-safe one-liner (a sid is a hash prefix or a row id, never a
    /// secret).
    pub fn label(&self) -> String {
        match self {
            SewSessionRevoke::K2Sid(s) => format!("k2_sid={s}"),
            SewSessionRevoke::ExternalId(e) => format!("external_id={e}"),
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
    let sink = sink_slot().read().unwrap_or_else(|p| p.into_inner()).clone();
    if let Some(f) = sink {
        return f(conn, op);
    }
    if !crate::tool_hook::enqueue_session_revoke(conn, op)? {
        crate::log_debug!("[sew_revoke] {OP_KIND} {} not queued: Sew is not set up on this box", op.label());
    }
    Ok(())
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
        assert_eq!(skin_sid("abc"), "skn_abc");
        assert_eq!(principal_external_id("p1"), "k2:principal:p1");
    }
}
