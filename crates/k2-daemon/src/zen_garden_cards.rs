//! K2's own Garden cards: `garden.access` and `garden.delete`
//! (prd-garden-access-v1 §1.5, §1.5a, G14, G17). **Slice A4 owns this file.**
//!
//! This is A2's stub of the seam A4 published (scratchpad `a4-asks.md`),
//! so A2's routes build and run before A4 lands. Integration takes A4's
//! file over this one. The stub presents no card (`{"card": null, "asked":
//! false}`); `access_entry` goes straight to A2's list door
//! (`zen_access_routes::list_change`); `delete_entry` is A4's.

use serde_json::{json, Value as J};

use crate::cli_response::CliResponse;
use crate::sidecar_routes::Caller;

/// Which card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GardenAsk {
    Access,
    Delete,
}

/// Present K2's card for (`kind`, workspace, Garden). Returns the value of
/// the answer's `card` field: `{"card": "<doc id>"|null, …}`. `made`: right
/// after `garden/new`. `explicit`: `garden/new`, `k2 zen garden access <g>
/// ask` and `k2 zen garden delete <g>` (a refusal is not explicit).
pub(crate) fn present(kind: GardenAsk, workspace_id: &str, garden_id: &str, made: bool, explicit: bool) -> J {
    let _ = (kind, workspace_id, garden_id, made, explicit);
    json!({ "card": J::Null, "asked": false })
}

/// The one entry for the list (Settings, the card, the twin): the
/// GardenOwner check, then add or remove.
pub(crate) fn access_entry(caller: &Caller, garden: &str, agent_project_id: &str, allow: bool, ingress: &str) -> CliResponse {
    let _ = ingress;
    crate::zen_access_routes::list_change(
        caller,
        garden,
        agent_project_id,
        allow,
        k2_core::zen::garden_access::Via::Settings,
        None,
    )
}
