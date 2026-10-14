//! Garden v3 data sources, daemon half (prd-garden-v3-external-data-cards-v1;
//! build contract `contract-sources-v1` §3.1, §3.4).
//!
//! The daemon owns the poller, the network, secret binding and trigger
//! wakes; `k2_core::sources` owns the pure maths. Routes are in
//! `crate::sources_routes`.
//!
//! C0 skeleton: nothing polls yet ([`start`] does nothing); B1 lands the
//! scheduler, budget, fetch and secrets, B2 the wake pipeline and routes.

use std::sync::atomic::AtomicBool;
use std::sync::OnceLock;

use k2_core::sources::{SourceRow, Transition};

use crate::cli_response::CliResponse;

pub mod approve_routes;
pub mod fetch;
pub mod secrets;
pub mod tinker_routes;
pub mod trigger_routes;
pub mod wake;

/// Set by the 0.46.0 hand-off when present [GV63]: fires are suppressed
/// with `draining` while it is up.
pub static DRAINING: AtomicBool = AtomicBool::new(false);

/// A refusal from a sources handler: the error envelope
/// `{"error": {"code", "hint", ...extra}}` with its HTTP status.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceErr {
    pub code: &'static str,
    pub status: u16,
    pub hint: String,
    /// Extra envelope fields (`field`, `specCode`, `untilAt`,
    /// `retryAfterSecs`, `removedAt`, `role`, `rootKey`, …).
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl SourceErr {
    pub fn new(code: &'static str, status: u16, hint: impl Into<String>) -> SourceErr {
        SourceErr { code, status, hint: hint.into(), extra: serde_json::Map::new() }
    }

    /// Add one extra envelope field.
    pub fn with(mut self, key: &str, value: impl Into<serde_json::Value>) -> SourceErr {
        self.extra.insert(key.to_string(), value.into());
        self
    }

    pub fn body(&self) -> serde_json::Value {
        let mut e = serde_json::Map::new();
        e.insert("code".into(), self.code.into());
        e.insert("hint".into(), self.hint.clone().into());
        for (k, v) in &self.extra {
            e.insert(k.clone(), v.clone());
        }
        serde_json::json!({ "error": e })
    }

    pub fn response(&self) -> CliResponse {
        CliResponse { status: status_line(self.status), content_type: "application/json", body: self.body().to_string() }
    }
}

/// 501 `not_built`: C0 stubs only; never in a release.
pub fn not_built() -> SourceErr {
    SourceErr::new("not_built", 501, "this part of data sources is not built yet")
}

pub(crate) fn status_line(code: u16) -> &'static str {
    match code {
        200 => "200 OK",
        400 => "400 Bad Request",
        401 => "401 Unauthorized",
        403 => "403 Forbidden",
        404 => "404 Not Found",
        405 => "405 Method Not Allowed",
        409 => "409 Conflict",
        410 => "410 Gone",
        413 => "413 Payload Too Large",
        423 => "423 Locked",
        429 => "429 Too Many Requests",
        501 => "501 Not Implemented",
        503 => "503 Service Unavailable",
        _ => "500 Internal Server Error",
    }
}

/// The poller handle.
pub struct Engine {
    _private: (),
}

static ENGINE: OnceLock<Engine> = OnceLock::new();

pub fn engine() -> &'static Engine {
    ENGINE.get_or_init(|| Engine { _private: () })
}

impl Engine {
    /// Poll a source now (Refresh, approve, edit). Returns `nextAt`.
    ///
    /// C0 skeleton: 501 (B1 replaces it).
    pub fn poll_now(&self, source_id: &str) -> Result<i64, SourceErr> {
        let _ = source_id;
        Err(not_built())
    }

    /// Hard transitions after a state commit: hands them to the trigger
    /// pipeline (B2).
    pub fn on_transitions(&self, source: &SourceRow, ts: &[Transition]) {
        let now = chrono::Utc::now().timestamp();
        wake::handle_transitions(source, ts, now);
    }
}

/// Start the poller. C0 skeleton: nothing runs yet (B1 wires the scheduler
/// and calls this from daemon start).
pub fn start() {}

/// Announce a change: `sources_changed {rev}` to every subscriber. B1 adds
/// the 500 ms coalescing (last `rev` wins).
pub fn notify_changed(rev: i64) {
    let _ = crate::session_events::emit(crate::session_events::SessionEvent::SourcesChanged { rev });
}
