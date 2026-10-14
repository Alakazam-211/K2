//! `POST /cli/zen/call`: the widget door (contract §7.1; PRD §6.1–§6.3).
//! C0 → B2.
//!
//! Checks, in order: owner token only (`owner_only`); the Origin gate
//! (Tauri origins only; `Origin: null` or several Origin headers →
//! `origin_refused`; none = a local owner-token process, audited); the
//! placement → widget → live hash (`stale_widget`); the verb is a
//! `garden-local` catalog row whose cap is in the manifest's effective caps
//! after the Sources taint (`cap_not_granted`); the placement isn't paused
//! (`paused`); the maker's rights for `store.*` (`author_unknown`,
//! `not_granted`); budgets; then the store. Writes store
//! `Actor::Widget { widget }`.

use serde::Deserialize;

use k2_core::garden_data::{not_built, DataError};

use crate::cli_response::CliResponse;
use crate::garden_data::{refused, response};
use crate::zen_data_routes::DataWho;

/// The request body (≤ 64 KiB).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallBody {
    pub garden: String,
    pub placement: String,
    /// The live bundle hash the frame runs.
    pub hash: String,
    /// A `garden-local` catalog verb (`store.put`, `save.get`, `viewer.get`, …).
    pub verb: String,
    #[serde(default)]
    pub args: Vec<serde_json::Value>,
}

/// One widget call. Answers `{"ok": true, "result": …}` or the envelope.
pub fn handle(who: &DataWho, _headers: &str, _body: &[u8]) -> CliResponse {
    if !who.is_owner() {
        return response(&refused(
            "owner_only",
            403,
            "only the K2 app on this computer calls widget data; agents use `k2 store --widget`",
        ));
    }
    let e: DataError = not_built();
    response(&e)
}
