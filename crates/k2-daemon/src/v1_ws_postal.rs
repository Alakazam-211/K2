//! `POST /v1/w/<ws>/message` with `kind: "postal"` (prd-postal-in-k2-v1 S0):
//! p5's knock for a Postal message it received for a linked workspace.
//!
//! Until K2 speaks Postal itself, p5 receives Postal mail on the box and
//! hands each message to K2 through this door with the scoped key `k2
//! postal link-p5` made (never the owner token). The rules:
//!
//! - **Who:** only an API key labelled `postal-link:<ws>` for this very
//!   workspace (P15, P21). The owner token or any other key is 403, so no
//!   other `/v1` caller can wear a `[postal …]` label.
//! - **From:** must parse as a Postal address (`handle::label.postal.bot`),
//!   else 400. It is p5's word for the sender (p5 checked the signature), so
//!   the record keeps the key as the actor.
//! - **What the agent sees:** K2's own label on its own line, then every
//!   line of the body quoted, the first too, with every control, bidi and
//!   invisible character gone ([`crate::workspace_msg::Sender::Postal`]).
//!   Never a command.
//! - **Once (P18):** p5 sends its Postal message id as `idempotency_key`;
//!   a knock repeated with that id answers the first answer instead of
//!   typing the message twice: from memory for 10 minutes (send-once), and
//!   after that (or a restart) from the delivery record, which holds each
//!   Postal message id once (0310). A record whose try never reached the
//!   agent lets the id go, so the next knock delivers it.
//! - The consent and busy gates are the same as any `/v1` message.

use crate::cli_response::CliResponse;
use crate::routes::http::V1Principal;
use k2_core::cli_name;

/// The send-once route name (apart from plain `/v1` messages).
const ONCE_ROUTE: &str = "/v1/w/message#postal";

fn refuse(status: &'static str, code: &str, hint: &str) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: serde_json::json!({ "delivered": false, "error": { "code": code, "hint": hint } }).to_string(),
    }
}

/// The key's id when it is this workspace's p5 link key.
fn link_key_for(principal: &V1Principal, project_path: &str) -> Option<String> {
    let V1Principal::Api(p) = principal else { return None };
    let label = k2_core::api_keys::key_label(&p.id)?;
    let ws = label.strip_prefix(k2_core::postal::LINK_LABEL_PREFIX)?;
    (crate::workspace_msg::resolve_workspace(ws).as_deref() == Some(project_path)).then(|| p.id.clone())
}

/// The `kind: "postal"` body, after the door's capability and workspace
/// grant passed (`project_path` is the granted workspace).
pub(crate) fn handle(principal: &V1Principal, project_path: &str, v: &serde_json::Value) -> CliResponse {
    let Some(key_id) = link_key_for(principal, project_path) else {
        return refuse(
            "403 Forbidden",
            "postal_link_only",
            concat!("kind \"postal\" is accepted only from the p5 link key made for this workspace (", cli_name!(), " postal link-p5 <workspace>)"),
        );
    };
    let Some(from) = v.get("from").and_then(|x| x.as_str()).and_then(k2_core::postal::parse_address) else {
        return refuse("400 Bad Request", "bad_postal_address", "from must be a Postal address: handle::label.postal.bot");
    };
    if v.get("command").and_then(|x| x.as_str()).is_some_and(|c| !c.trim().is_empty()) {
        return refuse("400 Bad Request", "postal_no_command", "a Postal message is never a command");
    }
    let raw = v.get("text").and_then(|x| x.as_str()).unwrap_or("");
    let text = raw.trim_end().trim_start_matches(['\n', '\r']);
    if text.trim().is_empty() {
        return refuse("400 Bad Request", "usage", "Missing 'text'");
    }
    let key = v.get(crate::send_once::PARAM).and_then(|x| x.as_str()).map(str::trim).filter(|s| !s.is_empty());
    if key.is_some_and(|k| !crate::send_once::is_valid(k)) {
        return refuse(
            "400 Bad Request",
            "usage",
            "idempotency_key must be 8-64 characters of A-Z, a-z, 0-9, '-' or '_' (a Postal message id fits)",
        );
    }
    if let Some(refused) = crate::v1_ws_message::consent_and_busy(project_path) {
        return refused;
    }
    let wake = v.get("wake").and_then(|x| x.as_bool()).unwrap_or(true);
    let on_behalf_of = v
        .get("onBehalfOf")
        .and_then(|x| x.as_str())
        .map(|s| s.trim().chars().take(200).collect::<String>())
        .filter(|s| !s.is_empty());
    let address = from.as_string();
    let sender = crate::hire_routes::Sender {
        workspace_id: None,
        via: "api-key",
        actor: format!("api-key:{key_id}"),
        on_behalf_of,
    };
    crate::send_once::once_keyed(ONCE_ROUTE, &format!("k:{key_id}"), key, || {
        if let Some(done) = key.and_then(|k| already_delivered(k, &address)) {
            return done;
        }
        let (result, delivery) = crate::hire_routes::deliver_recorded_postal(project_path, text, &address, wake, &sender, key);
        CliResponse::ok_json(
            serde_json::json!({
                "delivered": result.success,
                "mode": "live",
                "kind": "postal",
                "from": address,
                "result": crate::v1_ws_message::public_result(&result),
                "deliveryId": delivery,
            })
            .to_string(),
        )
    })
}

/// The answer for a Postal message id K2 already typed in (`duplicate`), or
/// `None` to deliver: no record, or one whose try never reached the agent
/// (its id is released for this try).
fn already_delivered(postal_msg_id: &str, address: &str) -> Option<CliResponse> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let d = k2_core::hire::deliveries::by_postal_msg_id(&conn, postal_msg_id)?;
    if !d.delivered {
        k2_core::hire::deliveries::release_postal_msg_id(&conn, &d.id);
        return None;
    }
    Some(CliResponse::ok_json(
        serde_json::json!({
            "delivered": true,
            "mode": "live",
            "kind": "postal",
            "from": address,
            "duplicate": true,
            "deliveryId": d.id,
        })
        .to_string(),
    ))
}

#[cfg(test)]
#[path = "v1_ws_postal_tests.rs"]
mod tests;
