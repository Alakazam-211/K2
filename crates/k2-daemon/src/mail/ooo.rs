//! `GET|POST /cli/mail/ooo` and `POST /cli/mail/ooo/unset`.
//!
//! Out-of-office = Stalwart user Sieve `vacation` (RFC 5230) on a
//! minted mailbox. Lookup is password-rotate style (host-wide active
//! row; 404 if missing/retired). Not quota `canManage`.

use std::collections::HashMap;

use crate::cli_response::CliResponse;
use crate::mail::addresses::{self, AddrError};
use crate::mail::domains;
use crate::mail::sieve_user::{
    self, cap_text, days_in_range, parse_until_date, reject_html, reject_lone_dot_line, OooBlock,
    SievePatch, VACATION_DAYS_DEFAULT,
};
use k2_core::db::schema::MailAddress;

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default)]
struct OooSetBody {
    address: Option<String>,
    text: Option<String>,
    days: Option<i64>,
    until: Option<String>,
}

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default)]
struct OooUnsetBody {
    address: Option<String>,
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

fn usage(hint: String) -> CliResponse {
    err_json("400 Bad Request", "usage", hint)
}

/// Host-wide active hosted row (password-rotate). Not canManage.
fn lookup_active(address: &str) -> Result<MailAddress, CliResponse> {
    let address = match addresses::normalize_address(address) {
        Ok(a) => a,
        Err(AddrError::Usage(h)) => return Err(usage(h)),
        Err(e) => return Err(usage(format!("{e:?}"))),
    };
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        addresses::load_address(&conn, &address)
    };
    let Some(row) = row else {
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

fn require_address(raw: Option<&str>) -> Result<String, CliResponse> {
    let address = raw.map(str::trim).filter(|s| !s.is_empty()).unwrap_or("");
    if address.is_empty() {
        return Err(usage(
            "missing 'address' — which hosted mailbox?".to_string(),
        ));
    }
    Ok(address.to_string())
}

fn engine() -> Result<crate::mail::jmap::StalwartClient, CliResponse> {
    domains::engine_from_db()
        .map(|(e, _)| e)
        .map_err(|hint| err_json("503 Service Unavailable", "not_ready", hint))
}

fn show_json(address: &str, sieve: &sieve_user::UserSieve) -> String {
    match &sieve.ooo {
        Some(ooo) => serde_json::json!({
            "ok": true,
            "address": address,
            "enabled": true,
            "days": ooo.days,
            "until": ooo.until,
            "text": ooo.text,
        })
        .to_string(),
        None => serde_json::json!({
            "ok": true,
            "address": address,
            "enabled": false,
            "days": serde_json::Value::Null,
            "until": serde_json::Value::Null,
            "text": serde_json::Value::Null,
        })
        .to_string(),
    }
}

/// GET `/cli/mail/ooo?address=`
pub fn handle_ooo_get(params: &HashMap<String, String>) -> CliResponse {
    let address = crate::cli::str_param(params, "address");
    let address = match require_address(Some(&address)) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let row = match lookup_active(&address) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let account_id = match account_id_of(&row, &row.address) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let engine = match engine() {
        Ok(e) => e,
        Err(resp) => return resp,
    };
    match sieve_user::load_user_sieve(&engine, &account_id) {
        Ok((sieve, _)) => CliResponse::ok_json(show_json(&row.address, &sieve)),
        Err(e) => err_json("502 Bad Gateway", "engine", e),
    }
}

/// POST `/cli/mail/ooo` `{address, text, days?, until?}`
pub fn handle_ooo_set(body: &[u8]) -> CliResponse {
    let b: OooSetBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let address = match require_address(b.address.as_deref()) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let text = b.text.as_deref().map(str::trim).unwrap_or("");
    if text.is_empty() {
        return usage("missing 'text' — the vacation auto-reply body".to_string());
    }
    if let Err(h) = cap_text(text, "vacation text") {
        return err_json("400 Bad Request", "text_too_long", h);
    }
    if let Err(h) = reject_lone_dot_line(text, "vacation text") {
        return usage(h);
    }
    if let Err(h) = reject_html(text) {
        return usage(h);
    }
    let days = match b.days {
        None => VACATION_DAYS_DEFAULT,
        Some(n) if n < 0 => {
            return usage(format!("--days must be 1–31, got {n}"));
        }
        Some(n) => match days_in_range(n as u32) {
            Ok(d) => d,
            Err(h) => return usage(h),
        },
    };
    let until = match b.until.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        None => None,
        Some(raw) => match parse_until_date(raw) {
            Ok(d) => Some(d),
            Err(h) => return usage(h),
        },
    };
    let row = match lookup_active(&address) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let account_id = match account_id_of(&row, &row.address) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let engine = match engine() {
        Ok(e) => e,
        Err(resp) => return resp,
    };
    let block = OooBlock {
        days,
        text: text.to_string(),
        until,
        from: row.address.clone(),
    };
    match sieve_user::patch_user_sieve(&engine, &account_id, SievePatch::SetOoo(block.clone())) {
        Ok(sieve) => CliResponse::ok_json(show_json(&row.address, &sieve)),
        Err(e) => err_json("502 Bad Gateway", "engine", e),
    }
}

/// POST `/cli/mail/ooo/unset` `{address}`
pub fn handle_ooo_unset(body: &[u8]) -> CliResponse {
    let b: OooUnsetBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let address = match require_address(b.address.as_deref()) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let row = match lookup_active(&address) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let account_id = match account_id_of(&row, &row.address) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let engine = match engine() {
        Ok(e) => e,
        Err(resp) => return resp,
    };
    match sieve_user::patch_user_sieve(&engine, &account_id, SievePatch::UnsetOoo) {
        Ok(sieve) => CliResponse::ok_json(show_json(&row.address, &sieve)),
        Err(e) => err_json("502 Bad Gateway", "engine", e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ooo_set_without_address_is_usage() {
        let r = handle_ooo_set(br#"{"text":"away"}"#);
        assert_eq!(r.status, "400 Bad Request");
        assert!(r.body.contains("address"), "{}", r.body);
    }

    #[test]
    fn ooo_set_without_text_is_usage() {
        let r = handle_ooo_set(br#"{"address":"a@b.test"}"#);
        assert_eq!(r.status, "400 Bad Request");
        assert!(r.body.contains("text"), "{}", r.body);
    }

    #[test]
    fn ooo_set_days_out_of_range_is_400() {
        let r = handle_ooo_set(br#"{"address":"a@b.test","text":"x","days":0}"#);
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
        assert!(
            r.body.contains("1–31") || r.body.contains("days"),
            "{}",
            r.body
        );
        let r = handle_ooo_set(br#"{"address":"a@b.test","text":"x","days":32}"#);
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
    }

    #[test]
    fn ooo_set_until_invalid_is_400() {
        let r = handle_ooo_set(br#"{"address":"a@b.test","text":"x","until":"soon"}"#);
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
        assert!(
            r.body.contains("YYYY-MM-DD") || r.body.contains("until"),
            "{}",
            r.body
        );
    }

    #[test]
    fn ooo_set_lone_dot_is_400() {
        let r = handle_ooo_set(br#"{"address":"a@b.test","text":"hi\n.\nthere"}"#);
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
        assert!(r.body.contains("lone"), "{}", r.body);
    }

    #[test]
    fn ooo_set_unknown_address_is_not_found() {
        let r = handle_ooo_set(br#"{"address":"no-such-ooo@example.test","text":"away"}"#);
        assert_eq!(r.status, "404 Not Found", "{}", r.body);
        assert!(r.body.contains("not_found"), "{}", r.body);
    }

    #[test]
    fn ooo_get_unknown_address_is_not_found() {
        let mut params = HashMap::new();
        params.insert(
            "address".to_string(),
            "no-such-ooo@example.test".to_string(),
        );
        let r = handle_ooo_get(&params);
        assert_eq!(r.status, "404 Not Found", "{}", r.body);
    }

    #[test]
    fn ooo_get_without_address_is_usage_not_405() {
        let r = handle_ooo_get(&HashMap::new());
        assert_eq!(r.status, "400 Bad Request");
        assert_ne!(r.status, "405 Method Not Allowed");
    }

    #[test]
    fn ooo_unset_unknown_is_not_found() {
        let r = handle_ooo_unset(br#"{"address":"ghost-ooo@example.test"}"#);
        assert_eq!(r.status, "404 Not Found", "{}", r.body);
    }

    #[test]
    fn ooo_set_retired_is_not_found() {
        let (name, path) = unique("ooo-ret");
        let project_id = insert_project(&name, &path);
        let addr = format!("box@{name}.example");
        seed_address(&project_id, &addr, "retired", Some("acc-r"));
        let body = serde_json::json!({ "address": addr, "text": "away" }).to_string();
        let r = handle_ooo_set(body.as_bytes());
        assert_eq!(r.status, "404 Not Found", "{}", r.body);
        cleanup_project(&project_id);
    }

    #[test]
    fn ooo_set_active_without_account_is_not_ready() {
        let (name, path) = unique("ooo-na");
        let project_id = insert_project(&name, &path);
        let addr = format!("live@{name}.example");
        seed_address(&project_id, &addr, "active", None);
        let body = serde_json::json!({ "address": addr, "text": "away" }).to_string();
        let r = handle_ooo_set(body.as_bytes());
        assert_eq!(r.status, "409 Conflict", "{}", r.body);
        assert!(r.body.contains("not_ready"), "{}", r.body);
        cleanup_project(&project_id);
    }

    fn unique(label: &str) -> (String, String) {
        let id = uuid::Uuid::new_v4();
        (
            format!("mail-ooo-{label}-{id}"),
            format!("/tmp/mail-ooo-{label}-{}-{id}", std::process::id()),
        )
    }

    fn insert_project(name: &str, path: &str) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, name, path],
        )
        .expect("insert project");
        id
    }

    fn cleanup_project(project_id: &str) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = conn.execute(
            "DELETE FROM mail_addresses WHERE owner_project_id = ?1",
            rusqlite::params![project_id],
        );
        let _ = conn.execute(
            "DELETE FROM projects WHERE id = ?1",
            rusqlite::params![project_id],
        );
    }

    fn seed_address(
        project_id: &str,
        address: &str,
        status: &str,
        stalwart_account_id: Option<&str>,
    ) {
        let id = uuid::Uuid::new_v4().to_string();
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO mail_addresses (id, address, domain_id, stalwart_account_id, \
             owner_project_id, status, created_at) VALUES (?1, ?2, 'dom-x', ?3, ?4, ?5, 100)",
            rusqlite::params![id, address, stalwart_account_id, project_id, status],
        )
        .expect("seed address");
    }
}
