//! Shared HTTP helpers for hostmail catch-all / alias / forward.
//! Copy of quota's mailbox authorize (workspace can_manage OR minting
//! workspace) so those verbs stay mail_manage, not leftover M6.

use crate::cli_response::CliResponse;
use crate::mail::access;
use crate::mail::addresses::{self, AddrError};
use crate::mail::domains;
use crate::mail::jmap::StalwartClient;
use crate::mail::messages::ReadError;
use k2_core::db::schema::{MailAddress, MailDomain};

pub fn err_json(status: &'static str, code: &str, hint: String) -> CliResponse {
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

pub fn engine() -> Result<StalwartClient, CliResponse> {
    match domains::engine_from_db() {
        Ok((e, _)) => Ok(e),
        Err(hint) => Err(err_json("503 Service Unavailable", "not_ready", hint)),
    }
}

pub fn account_id_of(row: &MailAddress, address: &str) -> Result<String, CliResponse> {
    row.stalwart_account_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            err_json(
                "409 Conflict",
                "not_ready",
                format!("address '{address}' has no mail-server account"),
            )
        })
}

/// Active minted mailbox, optionally ACL'd to the calling workspace.
pub fn authorize_mailbox(raw: &str) -> Result<MailAddress, CliResponse> {
    let address = match addresses::normalize_address(raw) {
        Ok(a) => a,
        Err(AddrError::Usage(h)) => return Err(err_json("400 Bad Request", "usage", h)),
        Err(e) => return Err(err_json("400 Bad Request", "usage", format!("{e:?}"))),
    };
    let Some(row) = ({
        let db = k2_core::db::shared();
        let conn = db.lock();
        addresses::load_address(&conn, &address)
    }) else {
        return Err(err_json(
            "404 Not Found",
            "not_found",
            format!("no hosted address '{address}'"),
        ));
    };
    if row.status != "active" {
        return Err(err_json(
            "404 Not Found",
            "not_found",
            format!("no hosted address '{address}'"),
        ));
    }
    if let Some(p) = crate::caller_workspace::request_principal() {
        let manage_ok = match access::can_manage(&p.workspace_uuid, &address) {
            Ok(_) => true,
            Err(ReadError::NotFound(_)) => false,
            Err(ReadError::Usage(hint)) => return Err(err_json("400 Bad Request", "usage", hint)),
            Err(ReadError::Engine(hint)) => {
                return Err(err_json("502 Bad Gateway", "engine", hint))
            }
        };
        let minting_ok = row.owner_project_id == p.workspace_uuid;
        if !manage_ok && !minting_ok {
            return Err(err_json(
                "403 Forbidden",
                "forbidden",
                format!("no hosted address '{address}' this workspace can manage"),
            ));
        }
    }
    Ok(row)
}

/// Active minted mailbox with no workspace ACL (dest existence).
pub fn require_active_mint(raw: &str) -> Result<MailAddress, CliResponse> {
    let address = match addresses::normalize_address(raw) {
        Ok(a) => a,
        Err(AddrError::Usage(h)) => return Err(err_json("400 Bad Request", "usage", h)),
        Err(e) => return Err(err_json("400 Bad Request", "usage", format!("{e:?}"))),
    };
    let Some(row) = ({
        let db = k2_core::db::shared();
        let conn = db.lock();
        addresses::load_address(&conn, &address)
    }) else {
        return Err(err_json(
            "400 Bad Request",
            "usage",
            format!("'{address}' is not a minted mailbox on this host"),
        ));
    };
    if row.status != "active" {
        return Err(err_json(
            "400 Bad Request",
            "usage",
            format!("'{address}' is not a minted mailbox on this host"),
        ));
    }
    Ok(row)
}

pub fn load_hosted_domain(raw: &str) -> Result<MailDomain, CliResponse> {
    let domain = match k2_core::mail_domain::normalize_mail_domain(raw) {
        Ok(d) => d,
        Err(h) => return Err(err_json("400 Bad Request", "usage", h)),
    };
    let Some(row) = ({
        let db = k2_core::db::shared();
        let conn = db.lock();
        domains::load_domain(&conn, &domain)
    }) else {
        return Err(err_json(
            "404 Not Found",
            "not_found",
            format!("no hosted domain '{domain}'"),
        ));
    };
    Ok(row)
}

pub fn stalwart_domain_id(row: &MailDomain) -> Result<String, CliResponse> {
    row.stalwart_domain_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            err_json(
                "409 Conflict",
                "not_ready",
                format!("domain '{}' has no mail-server id", row.domain),
            )
        })
}
