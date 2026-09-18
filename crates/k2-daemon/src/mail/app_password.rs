//! `GET|POST /cli/mail/app-password` and `POST /cli/mail/app-password/revoke`.
//!
//! Extra-gate is quota-shaped (`is_mail_manage_surface` + `post_allowed`).
//! Inner lookup is password-rotate: any active hosted row, not quota
//! `can_manage`. Missing `stalwart_account_id` → 502 `engine`.
//! Dual GET+POST on `/cli/mail/app-password` (list vs add). GET revoke → 405.

use std::collections::HashMap;

use crate::cli_response::CliResponse;
use crate::mail::addresses::{self, AddrError};
use crate::mail::domains;
use crate::mail::jmap::{AppPasswordInfo, CreatedAppPassword, StalwartClient};
use k2_core::db::schema::MailAddress;

/// What the route needs from Stalwart. Production is [`StalwartClient`];
/// tests use a recording fake so they never touch a network.
pub trait AppPasswordEngine {
    fn create(&self, account_id: &str, description: &str) -> Result<CreatedAppPassword, String>;
    fn list(&self, account_id: &str) -> Result<Vec<AppPasswordInfo>, String>;
    fn query_ids(&self, account_id: &str) -> Result<Vec<String>, String>;
    fn destroy(&self, account_id: &str, id: &str) -> Result<(), String>;
}

impl AppPasswordEngine for StalwartClient {
    fn create(&self, account_id: &str, description: &str) -> Result<CreatedAppPassword, String> {
        self.app_password_create(account_id, description)
    }
    fn list(&self, account_id: &str) -> Result<Vec<AppPasswordInfo>, String> {
        self.app_password_list(account_id)
    }
    fn query_ids(&self, account_id: &str) -> Result<Vec<String>, String> {
        self.app_password_query_ids(account_id)
    }
    fn destroy(&self, account_id: &str, id: &str) -> Result<(), String> {
        self.app_password_destroy(account_id, id)
    }
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

fn addr_error_response(err: AddrError) -> CliResponse {
    match err {
        AddrError::Usage(hint) => err_json("400 Bad Request", "usage", hint),
        AddrError::NotFound(hint) => err_json("404 Not Found", "not_found", hint),
        AddrError::Exists(hint) => err_json("409 Conflict", "exists", hint),
        AddrError::CapReached(hint) => err_json("409 Conflict", "cap_reached", hint),
        AddrError::NotReady(hint) => err_json("503 Service Unavailable", "not_ready", hint),
        AddrError::Engine(hint) => err_json("502 Bad Gateway", "engine", hint),
    }
}

fn is_cap_error(e: &str) -> bool {
    let l = e.to_ascii_lowercase();
    l.contains("overquota") || l.contains("toomany") || l.contains("maxapppassword")
}

fn map_engine_err(e: String) -> CliResponse {
    if is_cap_error(&e) {
        err_json("400 Bad Request", "usage", e)
    } else {
        err_json("502 Bad Gateway", "engine", e)
    }
}

/// Password-rotate lookup: any active hosted row. Missing account id
/// is 502 `engine`, not quota's 409 `not_ready`.
fn lookup_active_hosted(raw: &str) -> Result<(MailAddress, String), CliResponse> {
    let address = match addresses::normalize_address(raw) {
        Ok(a) => a,
        Err(e) => return Err(addr_error_response(e)),
    };
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        addresses::load_address(&conn, &address)
    };
    let Some(row) = row else {
        return Err(addr_error_response(AddrError::NotFound(format!(
            "no hosted address '{address}'"
        ))));
    };
    if row.status != "active" {
        return Err(addr_error_response(AddrError::NotFound(format!(
            "no hosted address '{address}'"
        ))));
    }
    let account_id = row
        .stalwart_account_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let Some(account_id) = account_id else {
        return Err(addr_error_response(AddrError::Engine(format!(
            "address '{}' has no mail-server account",
            row.address
        ))));
    };
    Ok((row, account_id))
}

fn default_label(raw: Option<&str>) -> String {
    let trimmed = raw.map(str::trim).filter(|s| !s.is_empty());
    trimmed.unwrap_or("k2").to_string()
}

fn list_json(address: &str, rows: &[AppPasswordInfo]) -> serde_json::Value {
    let app_passwords: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "id": r.id,
                "label": r.description,
                "createdAt": r.created_at,
            })
        })
        .collect();
    serde_json::json!({
        "ok": true,
        "address": address,
        "appPasswords": app_passwords,
    })
}

pub(crate) fn list_on(
    engine: &dyn AppPasswordEngine,
    address: &str,
    account_id: &str,
) -> CliResponse {
    match engine.list(account_id) {
        Ok(rows) => CliResponse::ok_json(list_json(address, &rows).to_string()),
        Err(e) => map_engine_err(e),
    }
}

pub(crate) fn add_on(
    engine: &dyn AppPasswordEngine,
    address: &str,
    account_id: &str,
    label: &str,
) -> CliResponse {
    match engine.create(account_id, label) {
        Ok(created) => CliResponse::ok_json(
            serde_json::json!({
                "ok": true,
                "address": address,
                "id": created.id,
                "label": label,
                "secret": created.secret,
            })
            .to_string(),
        ),
        Err(e) => map_engine_err(e),
    }
}

pub(crate) fn revoke_on(
    engine: &dyn AppPasswordEngine,
    address: &str,
    account_id: &str,
    id: &str,
) -> CliResponse {
    let ids = match engine.query_ids(account_id) {
        Ok(ids) => ids,
        Err(e) => return map_engine_err(e),
    };
    if !ids.iter().any(|owned| owned == id) {
        return err_json(
            "404 Not Found",
            "not_found",
            format!("no app password '{id}' on '{address}'"),
        );
    }
    match engine.destroy(account_id, id) {
        Ok(()) => CliResponse::ok_json(
            serde_json::json!({
                "ok": true,
                "address": address,
                "id": id,
                "revoked": true,
            })
            .to_string(),
        ),
        Err(e) => map_engine_err(e),
    }
}

/// GET `/cli/mail/app-password?address=` — list (no secrets).
pub fn handle_app_password_get(params: &HashMap<String, String>) -> CliResponse {
    let address = crate::cli::str_param(params, "address");
    if address.trim().is_empty() {
        return err_json(
            "400 Bad Request",
            "usage",
            "missing 'address' — which hosted mailbox?".to_string(),
        );
    }
    let (row, account_id) = match lookup_active_hosted(&address) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let (engine, _) = match domains::engine_from_db() {
        Ok(e) => e,
        Err(hint) => return err_json("503 Service Unavailable", "not_ready", hint),
    };
    list_on(&engine, &row.address, &account_id)
}

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default)]
struct AddBody {
    address: String,
    label: Option<String>,
}

/// POST `/cli/mail/app-password` `{address, label?}` — add; secret once.
pub fn handle_app_password_add(body: &[u8]) -> CliResponse {
    let b: AddBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    if b.address.trim().is_empty() {
        return err_json(
            "400 Bad Request",
            "usage",
            "missing 'address' — which hosted mailbox?".to_string(),
        );
    }
    let label = default_label(b.label.as_deref());
    let (row, account_id) = match lookup_active_hosted(&b.address) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let (engine, _) = match domains::engine_from_db() {
        Ok(e) => e,
        Err(hint) => return err_json("503 Service Unavailable", "not_ready", hint),
    };
    add_on(&engine, &row.address, &account_id, &label)
}

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default)]
struct RevokeBody {
    address: String,
    id: String,
}

/// POST `/cli/mail/app-password/revoke` `{address, id}` — destroy after
/// query proves the id belongs to that mailbox.
pub fn handle_app_password_revoke(body: &[u8]) -> CliResponse {
    let b: RevokeBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    if b.address.trim().is_empty() {
        return err_json(
            "400 Bad Request",
            "usage",
            "missing 'address' — which hosted mailbox?".to_string(),
        );
    }
    if b.id.trim().is_empty() {
        return err_json(
            "400 Bad Request",
            "usage",
            "missing 'id' — which app password to revoke?".to_string(),
        );
    }
    let (row, account_id) = match lookup_active_hosted(&b.address) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let (engine, _) = match domains::engine_from_db() {
        Ok(e) => e,
        Err(hint) => return err_json("503 Service Unavailable", "not_ready", hint),
    };
    revoke_on(&engine, &row.address, &account_id, b.id.trim())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Record {
        id: String,
        description: String,
        created_at: serde_json::Value,
        secret: String,
    }

    struct FakeEngine {
        account_id: String,
        rows: Mutex<Vec<Record>>,
        destroyed: Mutex<Vec<String>>,
        fail_create: Option<String>,
        next_id: Mutex<u32>,
    }

    impl FakeEngine {
        fn empty(account_id: &str) -> Self {
            Self {
                account_id: account_id.to_string(),
                rows: Mutex::new(Vec::new()),
                destroyed: Mutex::new(Vec::new()),
                fail_create: None,
                next_id: Mutex::new(0),
            }
        }
        fn with_row(account_id: &str, id: &str, label: &str, secret: &str) -> Self {
            let f = Self::empty(account_id);
            f.rows.lock().unwrap().push(Record {
                id: id.to_string(),
                description: label.to_string(),
                created_at: serde_json::json!("2026-09-18T00:00:00Z"),
                secret: secret.to_string(),
            });
            f
        }
    }

    impl AppPasswordEngine for FakeEngine {
        fn create(
            &self,
            account_id: &str,
            description: &str,
        ) -> Result<CreatedAppPassword, String> {
            assert_eq!(account_id, self.account_id, "must use mailbox accountId");
            if let Some(e) = &self.fail_create {
                return Err(e.clone());
            }
            let mut n = self.next_id.lock().unwrap();
            *n += 1;
            let id = format!("ap-{n}");
            let secret = format!("app_secret-{n}");
            self.rows.lock().unwrap().push(Record {
                id: id.clone(),
                description: description.to_string(),
                created_at: serde_json::json!("2026-09-18T00:00:00Z"),
                secret: secret.clone(),
            });
            Ok(CreatedAppPassword { id, secret })
        }
        fn list(&self, account_id: &str) -> Result<Vec<AppPasswordInfo>, String> {
            assert_eq!(account_id, self.account_id);
            Ok(self
                .rows
                .lock()
                .unwrap()
                .iter()
                .map(|r| AppPasswordInfo {
                    id: r.id.clone(),
                    description: r.description.clone(),
                    created_at: r.created_at.clone(),
                })
                .collect())
        }
        fn query_ids(&self, account_id: &str) -> Result<Vec<String>, String> {
            assert_eq!(account_id, self.account_id);
            Ok(self
                .rows
                .lock()
                .unwrap()
                .iter()
                .map(|r| r.id.clone())
                .collect())
        }
        fn destroy(&self, account_id: &str, id: &str) -> Result<(), String> {
            assert_eq!(account_id, self.account_id);
            self.destroyed.lock().unwrap().push(id.to_string());
            self.rows.lock().unwrap().retain(|r| r.id != id);
            Ok(())
        }
    }

    fn unique(label: &str) -> String {
        format!("{label}-{}", uuid::Uuid::new_v4().simple())
    }

    fn seed_domain(domain: &str) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO mail_domains (id, domain, stalwart_domain_id, status, created_at) \
             VALUES (?1, ?2, 'stw-d', 'pending', 100)",
            rusqlite::params![id, domain],
        )
        .expect("seed domain");
        id
    }

    fn seed_address(
        address: &str,
        domain_id: &str,
        project_id: &str,
        stalwart_account_id: Option<&str>,
        status: &str,
    ) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO mail_addresses (id, address, domain_id, stalwart_account_id, \
             owner_project_id, status, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 100)",
            rusqlite::params![
                id,
                address,
                domain_id,
                stalwart_account_id,
                project_id,
                status
            ],
        )
        .expect("seed address");
        id
    }

    fn cleanup_domain(domain: &str) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = conn.execute(
            "DELETE FROM mail_addresses WHERE domain_id IN \
             (SELECT id FROM mail_domains WHERE domain = ?1)",
            rusqlite::params![domain],
        );
        let _ = conn.execute(
            "DELETE FROM mail_domains WHERE domain = ?1",
            rusqlite::params![domain],
        );
    }

    #[test]
    fn get_without_address_is_usage_not_405() {
        let r = handle_app_password_get(&HashMap::new());
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
        assert_ne!(r.status, "405 Method Not Allowed");
        assert!(r.body.contains("address"), "{}", r.body);
    }

    #[test]
    fn add_without_address_is_usage() {
        let r = handle_app_password_add(br#"{"label":"phone"}"#);
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
        assert!(r.body.contains("address"), "{}", r.body);
    }

    #[test]
    fn revoke_without_id_is_usage() {
        let r = handle_app_password_revoke(br#"{"address":"a@b.test"}"#);
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
        assert!(r.body.contains("id"), "{}", r.body);
    }

    #[test]
    fn unminted_retired_are_not_found() {
        let domain = unique("ap-miss") + ".example";
        cleanup_domain(&domain);
        let domain_id = seed_domain(&domain);
        let project = unique("proj");
        seed_address(
            &format!("gone@{domain}"),
            &domain_id,
            &project,
            Some("acc-gone"),
            "retired",
        );
        let r = handle_app_password_get(&HashMap::from([(
            "address".to_string(),
            format!("gone@{domain}"),
        )]));
        assert_eq!(r.status, "404 Not Found", "{}", r.body);
        assert!(r.body.contains("not_found"), "{}", r.body);

        let r = handle_app_password_add(
            serde_json::json!({ "address": format!("ghost@{domain}") })
                .to_string()
                .as_bytes(),
        );
        assert_eq!(r.status, "404 Not Found", "{}", r.body);
        cleanup_domain(&domain);
    }

    #[test]
    fn missing_stalwart_account_id_is_502_engine() {
        let domain = unique("ap-noid") + ".example";
        cleanup_domain(&domain);
        let domain_id = seed_domain(&domain);
        let addr = format!("box@{domain}");
        seed_address(&addr, &domain_id, &unique("proj"), None, "active");
        let r = handle_app_password_get(&HashMap::from([("address".to_string(), addr.clone())]));
        assert_eq!(r.status, "502 Bad Gateway", "{}", r.body);
        let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(v["error"]["code"], "engine");
        assert_ne!(v["error"]["code"], "not_ready");
        cleanup_domain(&domain);
    }

    #[test]
    fn add_returns_secret_once_list_has_no_secret() {
        let engine = FakeEngine::empty("acc-1");
        let add = add_on(&engine, "bot@acme.test", "acc-1", "Mail.app");
        assert_eq!(add.status, "200 OK", "{}", add.body);
        let v: serde_json::Value = serde_json::from_str(&add.body).unwrap();
        assert_eq!(v["ok"], true);
        let id = v["id"].as_str().unwrap().to_string();
        let secret = v["secret"].as_str().unwrap().to_string();
        assert!(secret.starts_with("app_"), "{secret}");
        assert_eq!(v["label"], "Mail.app");
        assert_eq!(engine.rows.lock().unwrap()[0].secret, secret);

        let list = list_on(&engine, "bot@acme.test", "acc-1");
        assert_eq!(list.status, "200 OK", "{}", list.body);
        let lv: serde_json::Value = serde_json::from_str(&list.body).unwrap();
        assert_eq!(lv["ok"], true);
        assert_eq!(lv["address"], "bot@acme.test");
        let row = &lv["appPasswords"][0];
        assert_eq!(row["id"], id);
        assert_eq!(row["label"], "Mail.app");
        assert!(row.get("secret").is_none(), "{}", list.body);
        assert!(
            !list.body.contains(&secret),
            "list must not contain the secret substring: {}",
            list.body
        );
        assert!(!list.body.contains("\"secret\""), "{}", list.body);
    }

    #[test]
    fn default_label_is_k2() {
        assert_eq!(default_label(None), "k2");
        assert_eq!(default_label(Some("")), "k2");
        assert_eq!(default_label(Some("  ")), "k2");
        assert_eq!(default_label(Some("phone")), "phone");
    }

    #[test]
    fn revoke_unknown_id_is_404_and_does_not_destroy() {
        let engine = FakeEngine::with_row("acc-1", "ap-mine", "k2", "app_x");
        let r = revoke_on(&engine, "bot@acme.test", "acc-1", "foreign-id");
        assert_eq!(r.status, "404 Not Found", "{}", r.body);
        assert!(engine.destroyed.lock().unwrap().is_empty());
        assert_eq!(engine.rows.lock().unwrap().len(), 1);
    }

    #[test]
    fn revoke_owned_id_destroys() {
        let engine = FakeEngine::with_row("acc-1", "ap-mine", "k2", "app_x");
        let r = revoke_on(&engine, "bot@acme.test", "acc-1", "ap-mine");
        assert_eq!(r.status, "200 OK", "{}", r.body);
        assert_eq!(engine.destroyed.lock().unwrap().as_slice(), ["ap-mine"]);
        assert!(engine.rows.lock().unwrap().is_empty());
    }

    #[test]
    fn over_cap_is_400_with_engine_seterror() {
        let mut engine = FakeEngine::empty("acc-1");
        engine.fail_create =
            Some("x:AppPassword/set create rejected — overQuota: maxAppPasswords".to_string());
        let r = add_on(&engine, "bot@acme.test", "acc-1", "k2");
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
        let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(v["error"]["code"], "usage");
        assert!(
            v["error"]["hint"].as_str().unwrap().contains("overQuota"),
            "{}",
            r.body
        );
    }

    #[test]
    fn forbidden_create_is_502_engine() {
        let mut engine = FakeEngine::empty("acc-1");
        engine.fail_create =
            Some("x:AppPassword/set: JMAP error 'forbidden': managed by users".to_string());
        let r = add_on(&engine, "bot@acme.test", "acc-1", "k2");
        assert_eq!(r.status, "502 Bad Gateway", "{}", r.body);
        let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(v["error"]["code"], "engine");
    }

    #[test]
    fn list_after_create_still_has_id_when_primary_not_destroyed() {
        // Recorded survive: create then list (rotate is credentials.0
        // only and does not call destroy).
        let engine = FakeEngine::empty("acc-1");
        let add = add_on(&engine, "bot@acme.test", "acc-1", "k2");
        let id = serde_json::from_str::<serde_json::Value>(&add.body).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(engine.destroyed.lock().unwrap().is_empty());
        let list = list_on(&engine, "bot@acme.test", "acc-1");
        let lv: serde_json::Value = serde_json::from_str(&list.body).unwrap();
        assert_eq!(lv["appPasswords"][0]["id"], id);
    }
}
