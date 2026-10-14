//! Garden data, daemon half (prd-gardens-local-services-v1 1A;
//! contract-garden-data-v1). C0 → B1.
//!
//! - [`store`]: the process-wide `SqliteStore` under `~/.k2/stores/`.
//! - [`start`]: the hourly reaper / orphan / snapshot ticker.
//! - [`notify_changed`]: after every committed write to a `data:` space,
//!   emit `garden_data_changed {space, rev}` (coalesced per space, 100 ms,
//!   trailing edge always sent) and wake `changes?wait=` waiters.
//! - [`response`]: `DataError` → the `{"error": {...}}` envelope.
//!
//! Handlers live beside this file, one per owner (contract §3.1). The route
//! table and caller classifier are `crate::zen_data_routes`.

use std::path::PathBuf;
use std::sync::OnceLock;

use k2_core::garden_data::sqlite::SqliteStore;
use k2_core::garden_data::DataError;

use crate::cli_response::CliResponse;

pub mod agent_routes;
pub mod audit;
pub mod budget;
pub mod call;
pub mod export_routes;
pub mod grant_routes;
pub mod tx_routes;

/// `~/.k2/stores/` (0700): `gardens.sqlite` and `.snapshots/`.
pub fn stores_dir() -> PathBuf {
    k2_core::paths::k2_home().join("stores")
}

/// The process-wide Garden data store (opened lazily by B1's backend).
pub fn store() -> &'static SqliteStore {
    static STORE: OnceLock<SqliteStore> = OnceLock::new();
    STORE.get_or_init(|| SqliteStore::new(&stores_dir()))
}

/// Start the reaper / orphan / snapshot ticker (B1; snapshots are B3's
/// `k2_core::garden_data::snapshots`). C0: nothing runs.
pub fn start() {}

/// A committed write to `space` reached `rev`: push and wake waiters.
/// Never called for `save:` spaces. C0: a no-op (B1 coalesces and emits
/// `SessionEvent::GardenDataChanged`).
pub fn notify_changed(_space: &str, _rev: i64) {}

/// Block up to `wait_secs` until `space`'s rev passes `since` (the
/// `changes?wait=` long-poll). Answers true when it moved. C0: never waits.
pub fn wait_for_change(_space: &str, _since: i64, _wait_secs: u32) -> bool {
    false
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
        507 => "507 Insufficient Storage",
        _ => "500 Internal Server Error",
    }
}

/// The error envelope for `e` (contract §4.6).
pub fn response(e: &DataError) -> CliResponse {
    CliResponse { status: status_line(e.status()), content_type: "application/json", body: e.envelope().to_string() }
}

/// A fixed refusal (`apps_not_allowed`, `zen_local_only`, `owner_only`, …).
pub fn refused(code: &'static str, status: u16, hint: impl Into<String>) -> DataError {
    DataError::Refused { code, status, hint: hint.into() }
}

/// 200 with a JSON body.
pub fn ok(v: serde_json::Value) -> CliResponse {
    CliResponse::ok_json(v.to_string())
}
