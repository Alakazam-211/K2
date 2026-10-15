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
//! and the worker that calls Sew's `POST /mgmt/v1/sessions/revoke`. This
//! module is the small interface between the revoke sites and that
//! outbox: S27 installs a [`Sink`] at boot ([`set_sink`]), and every
//! revoke site calls [`enqueue`] **inside the same Immediate `skin.db`
//! transaction as the revoke write** (S28-A3), passing that connection,
//! so the outbox row commits or rolls back with the revoke.
//!
//! Until S27 lands there is no sink: [`enqueue`] logs the op (never a
//! secret; a sid is a hash) and drops it. That is safe because nothing can
//! create a Sew session without S27/S29 either.
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

/// Install the outbox writer (S27 at boot; tests). Replaces any previous
/// sink.
pub fn set_sink(sink: Sink) {
    *sink_slot().write().unwrap_or_else(|p| p.into_inner()) = Some(sink);
}

/// Remove the sink (tests).
pub fn clear_sink() {
    *sink_slot().write().unwrap_or_else(|p| p.into_inner()) = None;
}

/// Whether an outbox writer is installed. Callers outside `skin.db` use it
/// to avoid opening `skin.db` for nothing.
pub fn sink_installed() -> bool {
    sink_slot().read().unwrap_or_else(|p| p.into_inner()).is_some()
}

/// Queue one op. `conn` must be inside the revoke's transaction. Without a
/// sink (S27 not on this build) the op is logged and dropped.
pub fn enqueue(conn: &Connection, op: &SewSessionRevoke) -> Result<(), String> {
    let sink = sink_slot().read().unwrap_or_else(|p| p.into_inner()).clone();
    match sink {
        Some(f) => f(conn, op),
        None => {
            crate::log_debug!("[sew_revoke] {OP_KIND} {} not queued: no Sew outbox (S27) on this build", op.label());
            Ok(())
        }
    }
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
