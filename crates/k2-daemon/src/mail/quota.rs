//! `GET|POST /cli/mail/quota` — per-address mailbox disk/message caps.
//!
//! Mail-manage (or owner/admin) may raise an existing hosted mailbox.
//! New mints use the workspace default; mint constants do not retrofit.
//! GET reads Stalwart `x:Account/get` quotas (engine truth). POST
//! patches `x:Account/set` update `quotas: {maxDiskQuota?, maxEmails?}`.
//! 0 = unlimited. Negative refused.

use std::collections::HashMap;

use crate::cli_response::CliResponse;
use crate::mail::access;
use crate::mail::addresses::{self, AddrError};
use crate::mail::domains;
use crate::mail::messages::ReadError;
use k2_core::db::schema::MailAddress;

/// 1 GiB — `gb` × this = bytes.
const GIB: u64 = 1 << 30;

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct QuotaSetBody {
    address: Option<String>,
    bytes: Option<i64>,
    gb: Option<i64>,
    messages: Option<i64>,
}

fn err_json(status: &'static str, code: &str, hint: String) -> CliResponse {
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

fn non_neg(field: &str, n: i64) -> Result<u64, String> {
    if n < 0 {
        return Err(format!(
            "{field} must be a non-negative integer (0 = unlimited), got {n}"
        ));
    }
    Ok(n as u64)
}

/// Resolve POST body into `(bytes?, messages?)`. `bytes` XOR `gb`.
/// At least one of bytes/gb/messages.
pub(crate) fn resolve_quota_patch(
    bytes: Option<i64>,
    gb: Option<i64>,
    messages: Option<i64>,
) -> Result<(Option<u64>, Option<u64>), String> {
    if bytes.is_some() && gb.is_some() {
        return Err(
            "give either 'bytes' or 'gb' (gb × 2^30 = bytes), not both".to_string(),
        );
    }
    let disk = match (bytes, gb) {
        (Some(b), None) => Some(non_neg("bytes", b)?),
        (None, Some(g)) => {
            let g = non_neg("gb", g)?;
            Some(g.checked_mul(GIB).ok_or_else(|| {
                format!("gb {g} overflows maxDiskQuota (gb × 2^30)")
            })?)
        }
        (None, None) => None,
        (Some(_), Some(_)) => unreachable!(),
    };
    let msgs = match messages {
        Some(m) => Some(non_neg("messages", m)?),
        None => None,
    };
    if disk.is_none() && msgs.is_none() {
        return Err(
            "nothing to set — give bytes or gb and/or messages (0 = unlimited)".to_string(),
        );
    }
    Ok((disk, msgs))
}

fn authorize_address(address: &str) -> Result<MailAddress, CliResponse> {
    let address = match addresses::normalize_address(address) {
        Ok(a) => a,
        Err(AddrError::Usage(h)) => return Err(err_json("400 Bad Request", "usage", h)),
        Err(e) => {
            return Err(err_json(
                "400 Bad Request",
                "usage",
                format!("{e:?}"),
            ))
        }
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
            Err(ReadError::Usage(hint)) => {
                return Err(err_json("400 Bad Request", "usage", hint))
            }
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

fn account_id_of(row: &MailAddress, address: &str) -> Result<String, CliResponse> {
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

/// GET `/cli/mail/quota?address=` — Stalwart Account quotas object.
pub fn handle_quota_get(params: &HashMap<String, String>) -> CliResponse {
    let address = crate::cli::str_param(params, "address");
    if address.trim().is_empty() {
        return err_json(
            "400 Bad Request",
            "usage",
            "missing 'address' — which hosted mailbox?".to_string(),
        );
    }
    let row = match authorize_address(&address) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let account_id = match account_id_of(&row, &row.address) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let (engine, _) = match domains::engine_from_db() {
        Ok(e) => e,
        Err(hint) => return err_json("503 Service Unavailable", "not_ready", hint),
    };
    match engine.account_get_quotas(&account_id) {
        Ok(quotas) => CliResponse::ok_json(
            serde_json::json!({
                "ok": true,
                "address": row.address,
                "accountId": account_id,
                "quotas": quotas,
            })
            .to_string(),
        ),
        Err(e) => err_json("502 Bad Gateway", "engine", e),
    }
}

/// POST `/cli/mail/quota` `{address, bytes?: u64, messages?: u64, gb?: u64}`.
pub fn handle_quota_set(body: &[u8]) -> CliResponse {
    let b: QuotaSetBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let address = b
        .address
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("");
    if address.is_empty() {
        return err_json(
            "400 Bad Request",
            "usage",
            "missing 'address' — which hosted mailbox?".to_string(),
        );
    }
    let (disk, msgs) = match resolve_quota_patch(b.bytes, b.gb, b.messages) {
        Ok(v) => v,
        Err(h) => return err_json("400 Bad Request", "usage", h),
    };
    let row = match authorize_address(address) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let account_id = match account_id_of(&row, &row.address) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let (engine, _) = match domains::engine_from_db() {
        Ok(e) => e,
        Err(hint) => return err_json("503 Service Unavailable", "not_ready", hint),
    };
    if let Err(e) = engine.account_set_quotas(&account_id, disk, msgs) {
        return err_json("502 Bad Gateway", "engine", e);
    }
    let quotas = engine
        .account_get_quotas(&account_id)
        .unwrap_or_else(|_| {
            let mut q = serde_json::Map::new();
            if let Some(b) = disk {
                q.insert("maxDiskQuota".to_string(), serde_json::json!(b));
            }
            if let Some(m) = msgs {
                q.insert("maxEmails".to_string(), serde_json::json!(m));
            }
            serde_json::Value::Object(q)
        });
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "address": row.address,
            "accountId": account_id,
            "quotas": quotas,
        })
        .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_patch_bytes_xor_gb_and_zero() {
        assert_eq!(
            resolve_quota_patch(Some(1024), None, None).expect("bytes"),
            (Some(1024), None)
        );
        assert_eq!(
            resolve_quota_patch(None, Some(3), Some(40_000)).expect("gb"),
            (Some(3 * GIB), Some(40_000))
        );
        assert_eq!(
            resolve_quota_patch(Some(0), None, Some(0)).expect("unlimited"),
            (Some(0), Some(0))
        );
        let err = resolve_quota_patch(Some(1), Some(1), None).expect_err("xor");
        assert!(err.contains("bytes") && err.contains("gb"), "{err}");
        let err = resolve_quota_patch(None, None, None).expect_err("empty");
        assert!(err.contains("nothing to set"), "{err}");
    }

    #[test]
    fn resolve_patch_refuses_negative() {
        for (bytes, gb, messages, needle) in [
            (Some(-1), None, None, "bytes"),
            (None, Some(-2), None, "gb"),
            (None, None, Some(-3), "messages"),
        ] {
            let err = resolve_quota_patch(bytes, gb, messages).expect_err("negative");
            assert!(err.contains(needle), "{err}");
            assert!(err.contains("non-negative"), "{err}");
        }
    }

    #[test]
    fn quota_set_without_address_is_usage() {
        let r = handle_quota_set(br#"{"bytes":1024}"#);
        assert_eq!(r.status, "400 Bad Request");
        assert!(r.body.contains("address"), "{}", r.body);
    }

    #[test]
    fn quota_set_without_fields_is_usage() {
        let r = handle_quota_set(br#"{"address":"a@b.test"}"#);
        assert_eq!(r.status, "400 Bad Request");
        assert!(r.body.contains("nothing to set"), "{}", r.body);
    }

    #[test]
    fn quota_set_bytes_and_gb_is_usage() {
        let r = handle_quota_set(br#"{"address":"a@b.test","bytes":1,"gb":1}"#);
        assert_eq!(r.status, "400 Bad Request");
        assert!(r.body.contains("bytes") && r.body.contains("gb"), "{}", r.body);
    }

    #[test]
    fn quota_set_negative_is_usage() {
        let r = handle_quota_set(br#"{"address":"a@b.test","bytes":-1}"#);
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
        assert!(r.body.contains("non-negative"), "{}", r.body);
        let r = handle_quota_set(br#"{"address":"a@b.test","messages":-5}"#);
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
        assert!(r.body.contains("messages"), "{}", r.body);
    }

    #[test]
    fn quota_get_without_address_is_usage() {
        let r = handle_quota_get(&HashMap::new());
        assert_eq!(r.status, "400 Bad Request");
        assert!(r.body.contains("address"), "{}", r.body);
        assert_ne!(r.status, "405 Method Not Allowed");
    }

    #[test]
    fn quota_get_unknown_address_is_not_found() {
        let mut params = HashMap::new();
        params.insert(
            "address".to_string(),
            "no-such-quota-box@example.test".to_string(),
        );
        let r = handle_quota_get(&params);
        assert_eq!(r.status, "404 Not Found", "{}", r.body);
    }
}
