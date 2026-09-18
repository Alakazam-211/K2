//! `GET|POST /cli/mail/forward` and `POST /cli/mail/forward/unset`.
//!
//! RFC 9661 `SieveScript` named `k2-forward` on the mailbox accountId.
//! `--keep` → `require ["copy"]; redirect :copy`. Default → `redirect`.
//! Unset destroys that script only. Dest must be a minted mailbox.

use std::collections::HashMap;

use crate::cli_response::CliResponse;
use crate::mail::hostmail_auth::{
    account_id_of, authorize_mailbox, engine, err_json, require_active_mint,
};
use crate::mail::sieve_user::{self, ForwardBlock, SievePatch};

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default)]
struct ForwardSetBody {
    address: Option<String>,
    to: Option<String>,
    keep: Option<bool>,
}

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default)]
struct ForwardUnsetBody {
    address: Option<String>,
}

/// GET `/cli/mail/forward?address=` — dest + keep, or none.
pub fn handle_forward_get(params: &HashMap<String, String>) -> CliResponse {
    let address = crate::cli::str_param(params, "address");
    if address.trim().is_empty() {
        return err_json(
            "400 Bad Request",
            "usage",
            "missing 'address' — which hosted mailbox?".to_string(),
        );
    }
    let row = match authorize_mailbox(&address) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let account_id = match account_id_of(&row, &row.address) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let client = match engine() {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    match sieve_user::load_user_sieve(&client, &account_id) {
        Ok((sieve, _)) => match sieve.forward {
            Some(fwd) => CliResponse::ok_json(
                serde_json::json!({
                    "ok": true,
                    "address": row.address,
                    "to": fwd.dest,
                    "keep": fwd.keep,
                })
                .to_string(),
            ),
            None => CliResponse::ok_json(
                serde_json::json!({
                    "ok": true,
                    "address": row.address,
                    "to": serde_json::Value::Null,
                    "keep": false,
                })
                .to_string(),
            ),
        },
        Err(e) => err_json("502 Bad Gateway", "engine", e),
    }
}

/// POST `/cli/mail/forward` `{address, to, keep?: bool}`.
pub fn handle_forward_set(body: &[u8]) -> CliResponse {
    let b: ForwardSetBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let address = b.address.as_deref().map(str::trim).unwrap_or("");
    let to_raw = b.to.as_deref().map(str::trim).unwrap_or("");
    if address.is_empty() {
        return err_json(
            "400 Bad Request",
            "usage",
            "missing 'address' — which hosted mailbox?".to_string(),
        );
    }
    if to_raw.is_empty() {
        return err_json(
            "400 Bad Request",
            "usage",
            "missing 'to' — dest must be a minted mailbox on this host".to_string(),
        );
    }
    let row = match authorize_mailbox(address) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let dest = match require_active_mint(to_raw) {
        Ok(d) => d,
        Err(resp) => return resp,
    };
    let account_id = match account_id_of(&row, &row.address) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let keep = b.keep.unwrap_or(false);
    let client = match engine() {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    if let Err(e) = sieve_user::patch_user_sieve(
        &client,
        &account_id,
        SievePatch::SetForward(ForwardBlock {
            dest: dest.address.clone(),
            keep,
        }),
    ) {
        return err_json("502 Bad Gateway", "engine", e);
    }
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "address": row.address,
            "to": dest.address,
            "keep": keep,
        })
        .to_string(),
    )
}

/// POST `/cli/mail/forward/unset` `{address}` — destroy `k2-forward` only.
pub fn handle_forward_unset(body: &[u8]) -> CliResponse {
    let b: ForwardUnsetBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let address = b.address.as_deref().map(str::trim).unwrap_or("");
    if address.is_empty() {
        return err_json(
            "400 Bad Request",
            "usage",
            "missing 'address' — which hosted mailbox?".to_string(),
        );
    }
    let row = match authorize_mailbox(address) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let account_id = match account_id_of(&row, &row.address) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let client = match engine() {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    match sieve_user::patch_user_sieve(&client, &account_id, SievePatch::UnsetForward) {
        Ok(_) => CliResponse::ok_json(
            serde_json::json!({
                "ok": true,
                "address": row.address,
                "to": serde_json::Value::Null,
            })
            .to_string(),
        ),
        Err(e) => err_json("502 Bad Gateway", "engine", e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_without_address_is_usage_not_405() {
        let r = handle_forward_get(&HashMap::new());
        assert_eq!(r.status, "400 Bad Request");
        assert!(r.body.contains("address"), "{}", r.body);
        assert_ne!(r.status, "405 Method Not Allowed");
    }

    #[test]
    fn set_without_to_is_usage() {
        let r = handle_forward_set(br#"{"address":"a@b.test"}"#);
        assert_eq!(r.status, "400 Bad Request");
        assert!(r.body.contains("to"), "{}", r.body);
    }

    #[test]
    fn set_unminted_addr_is_404() {
        let r = handle_forward_set(
            br#"{"address":"nobody@no-such-fwd.example.test","to":"also-nobody@no-such-fwd.example.test"}"#,
        );
        assert_eq!(r.status, "404 Not Found", "{}", r.body);
    }

    #[test]
    fn set_unminted_dest_is_400() {
        let tag = uuid::Uuid::new_v4().simple();
        let domain = format!("fwd-dest-{tag}.test");
        let addr = format!("bot@{domain}");
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let did = uuid::Uuid::new_v4().to_string();
            let _ = conn.execute(
                "INSERT INTO mail_domains (id, domain, stalwart_domain_id, status, created_at) \
                 VALUES (?1, ?2, ?3, 'verified', ?4)",
                rusqlite::params![did, domain, "sw-fwd", now],
            );
            let id = uuid::Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO mail_addresses (id, address, domain_id, stalwart_account_id, \
                 owner_project_id, status, created_at) VALUES (?1, ?2, ?3, 'acc-fwd', 'ws', 'active', ?4)",
                rusqlite::params![id, addr, did, now],
            )
            .expect("seed");
        }
        let body = serde_json::json!({
            "address": addr,
            "to": format!("ghost@{domain}"),
        });
        let r = handle_forward_set(body.to_string().as_bytes());
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = conn.execute(
                "DELETE FROM mail_addresses WHERE address = ?1",
                rusqlite::params![addr],
            );
            let _ = conn.execute(
                "DELETE FROM mail_domains WHERE domain = ?1",
                rusqlite::params![domain],
            );
        }
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
        assert!(
            r.body.contains("minted") || r.body.contains("not a minted"),
            "{}",
            r.body
        );
    }
}
