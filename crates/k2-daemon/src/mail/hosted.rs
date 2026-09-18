//! Host-wide mail_manage lookup — password-rotate style, not canManage.
//!
//! Next-CLI list/queue/autoconfig/spam-allow and ACL/spam-train take a
//! minted mailbox. Unknown or retired → 404 `not_found`. Missing
//! `stalwart_account_id` → 409 `not_ready`.

use crate::cli_response::CliResponse;
use crate::mail::addresses::{self, AddrError};
use crate::mail::domains;
use crate::mail::jmap::StalwartClient;
use k2_core::db::schema::MailAddress;

pub(crate) fn err_json(status: &'static str, code: &str, hint: String) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: serde_json::json!({
            "ok": false,
            "error": { "code": code, "hint": hint },
        })
        .to_string(),
    }
}

pub(crate) fn addr_err(err: AddrError) -> CliResponse {
    match err {
        AddrError::Usage(hint) => err_json("400 Bad Request", "usage", hint),
        AddrError::NotFound(hint) => err_json("404 Not Found", "not_found", hint),
        AddrError::Exists(hint) => err_json("409 Conflict", "exists", hint),
        AddrError::CapReached(hint) => err_json("409 Conflict", "cap_reached", hint),
        AddrError::NotReady(hint) => err_json("503 Service Unavailable", "not_ready", hint),
        AddrError::Engine(hint) => err_json("502 Bad Gateway", "engine", hint),
    }
}

/// Active hosted mailbox by address. Retired/unknown → NotFound.
pub(crate) fn load_active_hosted(address: &str) -> Result<MailAddress, AddrError> {
    let address = addresses::normalize_address(address)?;
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        addresses::load_address(&conn, &address)
    };
    let Some(row) = row else {
        return Err(AddrError::NotFound(format!(
            "no hosted address '{address}'"
        )));
    };
    if row.status != "active" {
        return Err(AddrError::NotFound(format!(
            "no hosted address '{address}'"
        )));
    }
    Ok(row)
}

pub(crate) fn account_id_of(row: &MailAddress) -> Result<String, CliResponse> {
    row.stalwart_account_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            err_json(
                "409 Conflict",
                "not_ready",
                format!("address '{}' has no mail-server account", row.address),
            )
        })
}

pub(crate) fn engine() -> Result<(StalwartClient, Option<String>), CliResponse> {
    domains::engine_from_db().map_err(|hint| err_json("503 Service Unavailable", "not_ready", hint))
}
