//! The person's order and pins in each Garden's Agents widget, on this
//! computer (0.46.4; Baden via Appa 2026-10-22; "agent-readable, not
//! settable"). The state is `k2_core::zen::agents_order`'s
//! `agents-order.json`.
//!
//! - `GET /cli/zen/agents/order[?garden=<id or name>]`: the owner token
//!   (the app, a terminal outside K2) or a local session passport (an agent
//!   or a K2 terminal: `k2 zen agents-order`). Anything else gets 403
//!   `zen_local_only`, like every Zen route.
//! - `POST /cli/zen/agents/order/set {garden, order?, pins?}`: the person
//!   only, i.e. this computer's owner token. A passport, a Connect login or
//!   an app pass gets 403 `owner_only`, with an audit line. A GET is 405
//!   (the dispatcher, [`crate::zen_routes::POST_ROUTES`]); the body is
//!   capped at [`MAX_BODY`]. A change emits ONE payload-free
//!   `zen_agents_order_changed`, so every window re-reads; it is never a
//!   `zen_changed` (that remounts the page).
//!
//! `zen_routes::handle` hands both paths here before any other Zen rule.

use std::collections::HashMap;

use serde::{Deserialize, Deserializer};
use serde_json::json;

use k2_core::zen::agents_order::AgentsViewChange;
use k2_core::zen::{ZenError, ZenFiles};

use crate::cli_response::CliResponse;
use crate::sidecar_routes::Caller;

/// The read.
pub const ROUTE: &str = "/cli/zen/agents/order";
/// The person's write.
pub const SET_ROUTE: &str = "/cli/zen/agents/order/set";

/// Two lists of at most 200 addresses of at most 256 bytes, with room for
/// JSON escapes; anything bigger is 413.
pub const MAX_BODY: usize = 160 * 1024;

const OWNER_ONLY: &str = "Only the person sets the order and pins in a Garden's Agents widget, by dragging or pinning there on this computer. Agents can read them (k2 zen agents-order) but can't set them.";

pub fn is_route(path: &str) -> bool {
    path == ROUTE || path == SET_ROUTE
}

pub fn too_large() -> CliResponse {
    CliResponse {
        status: "413 Payload Too Large",
        content_type: "application/json",
        body: json!({ "error": "body_too_large", "message": "agents/order/set bodies must be under 160 KB." }).to_string(),
    }
}

/// `"order": null` is `Some(None)` (clear); an absent key is `None` (keep).
fn present<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Option<Vec<String>>>, D::Error> {
    Option::<Vec<String>>::deserialize(d).map(Some)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetBody {
    garden: String,
    #[serde(default, deserialize_with = "present")]
    order: Option<Option<Vec<String>>>,
    #[serde(default, deserialize_with = "present")]
    pins: Option<Option<Vec<String>>>,
}

fn ok(body: serde_json::Value) -> CliResponse {
    CliResponse { status: "200 OK", content_type: "application/json", body: body.to_string() }
}

/// `owner_token`: the request presented this computer's owner token and
/// nothing else ([`crate::zen_routes::handle`]'s rule).
pub fn handle(
    path: &str,
    owner_token: bool,
    caller: &Caller,
    ingress: &str,
    params: &HashMap<String, String>,
    body: &[u8],
) -> CliResponse {
    let f = ZenFiles::local();
    if path == ROUTE {
        if !owner_token && !crate::zen_routes::is_local_passport(caller) {
            return crate::zen_routes::local_only();
        }
        let garden = params.get("garden").map(|s| s.trim()).filter(|s| !s.is_empty());
        return match f.agents_order_json(garden) {
            Ok(v) => ok(v),
            Err(e) => crate::zen_routes::error_response(e),
        };
    }
    if !owner_token {
        crate::zen_routes::audit_refusal(
            "zen.agents_order.refused",
            caller,
            format!("{path}: the person only"),
            ingress,
        );
        return crate::zen_routes::error_response(ZenError::OwnerOnly(OWNER_ONLY.into()));
    }
    let b: SetBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => {
            return crate::zen_routes::error_response(ZenError::BadRequest(format!(
                "agents/order/set takes {{garden, order?, pins?}} (a list of row addresses, or null to clear): {e}"
            )))
        }
    };
    match f.set_agents_view(&b.garden, AgentsViewChange { order: b.order, pins: b.pins }) {
        Ok((row, changed)) => {
            if changed {
                let _ = crate::session_events::emit(crate::session_events::SessionEvent::ZenAgentsOrderChanged {});
            }
            ok(json!({ "ok": true, "garden": row, "changed": changed }))
        }
        Err(e) => crate::zen_routes::error_response(e),
    }
}

#[cfg(test)]
#[path = "zen_agents_order_routes_tests.rs"]
mod tests;
