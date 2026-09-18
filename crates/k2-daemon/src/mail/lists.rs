//! `GET|POST /cli/mail/list` — Stalwart `x:MailingList` (not aliases).
//!
//! Members = minted active inboxes this cut. Recipients is a set object.
//! 409 vs User / Account alias / catch-all dest / other list. No Account
//! for the list. Never write Domain.aliases.

use std::collections::HashMap;

use crate::cli_response::CliResponse;
use crate::mail::addresses::{self, AddrError};
use crate::mail::domains;
use crate::mail::hosted::{self, err_json};
use crate::mail::jmap::{self, StalwartClient};

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default)]
struct CreateBody {
    address: Option<String>,
    members: Option<Vec<String>>,
}

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default)]
struct MembersBody {
    address: Option<String>,
    add: Option<Vec<String>>,
    remove: Option<Vec<String>>,
}

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default)]
struct DeleteBody {
    address: Option<String>,
}

fn body_address(v: Option<&String>) -> Result<String, CliResponse> {
    let raw = v.map(|s| s.trim()).filter(|s| !s.is_empty()).unwrap_or("");
    if raw.is_empty() {
        return Err(err_json(
            "400 Bad Request",
            "usage",
            "missing 'address' — list address, e.g. team@acme.dev".to_string(),
        ));
    }
    addresses::normalize_address(raw).map_err(hosted::addr_err)
}

fn require_minted_members(members: &[String]) -> Result<Vec<String>, CliResponse> {
    let mut out = Vec::new();
    for m in members {
        let row = hosted::load_active_hosted(m).map_err(|e| match e {
            AddrError::NotFound(h) => err_json(
                "400 Bad Request",
                "usage",
                format!("member must be a minted inbox this cut: {h}"),
            ),
            other => hosted::addr_err(other),
        })?;
        if !out.contains(&row.address) {
            out.push(row.address);
        }
    }
    Ok(out)
}

fn split_list_addr(address: &str) -> Result<(String, String), CliResponse> {
    let Some((local, domain)) = address.split_once('@') else {
        return Err(err_json(
            "400 Bad Request",
            "usage",
            format!("'{address}' is not a full address"),
        ));
    };
    Ok((local.to_string(), domain.to_string()))
}

fn hosted_domain(domain: &str) -> Result<(String, String), CliResponse> {
    let domain = k2_core::mail_domain::normalize_mail_domain(domain)
        .map_err(|h| err_json("400 Bad Request", "usage", h))?;
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        domains::load_domain(&conn, &domain)
    };
    let Some(row) = row else {
        return Err(err_json(
            "404 Not Found",
            "not_found",
            format!("no hosted domain '{domain}'"),
        ));
    };
    let id = row
        .stalwart_domain_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            err_json(
                "409 Conflict",
                "not_ready",
                format!("domain '{domain}' has no mail-server id"),
            )
        })?;
    Ok((domain, id))
}

fn collision_user(address: &str) -> Result<(), CliResponse> {
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        addresses::load_address(&conn, address)
    };
    if let Some(row) = row {
        if row.status == "active" {
            return Err(err_json(
                "409 Conflict",
                "exists",
                format!("'{address}' is already a minted mailbox — lists are not Users"),
            ));
        }
        return Err(err_json(
            "409 Conflict",
            "exists",
            format!("'{address}' is reserved by a retired mailbox"),
        ));
    }
    Ok(())
}

/// Stalwart 0.16 `x:MailingList/query` filter is **text or tenant**
/// (N9). `emailAddress` and `domainId` both 502 `unsupportedFilter`
/// on lztek. Prefer `text` = local-part; on that fail, enumerate with
/// no filter and match locally.
fn query_list_ids(
    engine: &StalwartClient,
    local: &str,
    _domain_id: &str,
) -> Result<Vec<String>, CliResponse> {
    let unsupported = |e: &str| e.to_ascii_lowercase().contains("unsupportedfilter");
    match engine.mailing_list_query(Some(serde_json::json!({ "text": local }))) {
        Ok(ids) => Ok(ids),
        Err(e) if unsupported(&e) => engine
            .mailing_list_query(None)
            .map_err(|e2| err_json("502 Bad Gateway", "engine", e2)),
        Err(e) => Err(err_json("502 Bad Gateway", "engine", e)),
    }
}

fn row_matches_address(row: &jmap::MailingListInfo, address: &str, domain_id: &str) -> bool {
    if row
        .email_address
        .as_deref()
        .is_some_and(|e| e.eq_ignore_ascii_case(address))
    {
        return true;
    }
    let local = address.split('@').next().unwrap_or("");
    !row.name.is_empty()
        && row.name.eq_ignore_ascii_case(local)
        && (row.domain_id.is_empty() || row.domain_id == domain_id)
}

fn collision_engine(
    engine: &StalwartClient,
    address: &str,
    domain_id: &str,
) -> Result<(), CliResponse> {
    let local = address.split('@').next().unwrap_or("");
    let found = query_list_ids(engine, local, domain_id)?;
    if !found.is_empty() {
        let rows = engine
            .mailing_list_get(&found)
            .map_err(|e| err_json("502 Bad Gateway", "engine", e))?;
        if rows.iter().any(|r| row_matches_address(r, address, domain_id)) {
            return Err(err_json(
                "409 Conflict",
                "exists",
                format!("'{address}' is already a mailing list"),
            ));
        }
    }
    match engine.domain_get_catchall(domain_id) {
        Ok(Some(dest)) if dest.eq_ignore_ascii_case(address) => {
            return Err(err_json(
                "409 Conflict",
                "exists",
                format!("'{address}' is this domain's catch-all destination"),
            ));
        }
        Ok(_) => {}
        Err(e) => return Err(err_json("502 Bad Gateway", "engine", e)),
    }
    let accounts = engine
        .account_query_ids_for_domain(domain_id)
        .map_err(|e| err_json("502 Bad Gateway", "engine", e))?;
    for id in accounts {
        let aliases = engine
            .account_get_aliases(&id)
            .map_err(|e| err_json("502 Bad Gateway", "engine", e))?;
        for a in aliases {
            if !a.enabled {
                continue;
            }
            // Alias local-part on this domain. Stalwart domainId is not a DNS name.
            let list_domain = address.split('@').nth(1).unwrap_or("");
            let alias_addr = format!("{}@{list_domain}", a.name);
            if alias_addr.eq_ignore_ascii_case(address) {
                return Err(err_json(
                    "409 Conflict",
                    "exists",
                    format!("'{address}' is already an Account alias"),
                ));
            }
        }
    }
    Ok(())
}

fn find_list(engine: &StalwartClient, address: &str) -> Result<jmap::MailingListInfo, CliResponse> {
    let (local, domain) = split_list_addr(address)?;
    let (_domain, domain_id) = hosted_domain(&domain)?;
    let ids = query_list_ids(engine, &local, &domain_id)?;
    let rows = engine
        .mailing_list_get(&ids)
        .map_err(|e| err_json("502 Bad Gateway", "engine", e))?;
    rows.into_iter()
        .find(|r| row_matches_address(r, address, &domain_id))
        .ok_or_else(|| {
            err_json(
                "404 Not Found",
                "not_found",
                format!("no mailing list '{address}'"),
            )
        })
}

fn list_json(row: &jmap::MailingListInfo) -> serde_json::Value {
    serde_json::json!({
        "ok": true,
        "id": row.id,
        "address": row.email_address.clone().unwrap_or_else(|| row.name.clone()),
        "name": row.name,
        "members": jmap::recipients_addrs(&row.recipients),
    })
}

/// GET `/cli/mail/list?address=` — show.
pub fn handle_list_get(params: &HashMap<String, String>) -> CliResponse {
    let address = crate::cli::str_param(params, "address");
    if address.trim().is_empty() {
        return err_json(
            "400 Bad Request",
            "usage",
            "missing 'address' — which mailing list?".to_string(),
        );
    }
    let address = match addresses::normalize_address(&address) {
        Ok(a) => a,
        Err(e) => return hosted::addr_err(e),
    };
    let (engine, _) = match hosted::engine() {
        Ok(e) => e,
        Err(r) => return r,
    };
    match find_list(&engine, &address) {
        Ok(row) => CliResponse::ok_json(list_json(&row).to_string()),
        Err(r) => r,
    }
}

/// GET `/cli/mail/list/members?address=`
pub fn handle_members_get(params: &HashMap<String, String>) -> CliResponse {
    handle_list_get(params)
}

/// POST `/cli/mail/list` create `{address, members:[]}`
pub fn handle_list_create(body: &[u8]) -> CliResponse {
    let b: CreateBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => {
            return err_json(
                "400 Bad Request",
                "usage",
                format!("invalid JSON body: {e}"),
            )
        }
    };
    let address = match body_address(b.address.as_ref()) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let members = b.members.unwrap_or_default();
    let members = match require_minted_members(&members) {
        Ok(m) => m,
        Err(r) => return r,
    };
    if let Err(r) = collision_user(&address) {
        return r;
    }
    let (_local, domain) = match split_list_addr(&address) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let (_domain, domain_id) = match hosted_domain(&domain) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let (engine, _) = match hosted::engine() {
        Ok(e) => e,
        Err(r) => return r,
    };
    if let Err(r) = collision_engine(&engine, &address, &domain_id) {
        return r;
    }
    let local = address.split('@').next().unwrap_or("").to_string();
    let recips = jmap::recipients_set_object(&members);
    match engine.mailing_list_create(&local, &domain_id, &recips) {
        Ok(id) => CliResponse::ok_json(
            serde_json::json!({
                "ok": true,
                "id": id,
                "address": address,
                "members": members,
            })
            .to_string(),
        ),
        Err(e) => err_json("502 Bad Gateway", "engine", e),
    }
}

/// POST `/cli/mail/list/members` `{address, add?:[], remove?:[]}`
pub fn handle_members_post(body: &[u8]) -> CliResponse {
    let b: MembersBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => {
            return err_json(
                "400 Bad Request",
                "usage",
                format!("invalid JSON body: {e}"),
            )
        }
    };
    let address = match body_address(b.address.as_ref()) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let add = b.add.unwrap_or_default();
    let remove = b.remove.unwrap_or_default();
    if add.is_empty() && remove.is_empty() {
        return err_json(
            "400 Bad Request",
            "usage",
            "give add[] and/or remove[] members".to_string(),
        );
    }
    let add = match require_minted_members(&add) {
        Ok(m) => m,
        Err(r) => return r,
    };
    let remove: Vec<String> = remove
        .into_iter()
        .filter_map(|s| addresses::normalize_address(&s).ok())
        .collect();
    let (engine, _) = match hosted::engine() {
        Ok(e) => e,
        Err(r) => return r,
    };
    let row = match find_list(&engine, &address) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let mut members = jmap::recipients_addrs(&row.recipients);
    for r in &remove {
        members.retain(|m| !m.eq_ignore_ascii_case(r));
    }
    for a in &add {
        if !members.iter().any(|m| m.eq_ignore_ascii_case(a)) {
            members.push(a.clone());
        }
    }
    let recips = jmap::recipients_set_object(&members);
    if let Err(e) = engine.mailing_list_set_recipients(&row.id, &recips) {
        return err_json("502 Bad Gateway", "engine", e);
    }
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "address": address,
            "members": members,
        })
        .to_string(),
    )
}

/// POST `/cli/mail/list/delete` `{address}`
pub fn handle_list_delete(body: &[u8]) -> CliResponse {
    let b: DeleteBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => {
            return err_json(
                "400 Bad Request",
                "usage",
                format!("invalid JSON body: {e}"),
            )
        }
    };
    let address = match body_address(b.address.as_ref()) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let (engine, _) = match hosted::engine() {
        Ok(e) => e,
        Err(r) => return r,
    };
    let row = match find_list(&engine, &address) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    if let Err(e) = engine.mailing_list_destroy(&row.id) {
        return err_json("502 Bad Gateway", "engine", e);
    }
    CliResponse::ok_json(
        serde_json::json!({ "ok": true, "address": address, "deleted": true }).to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_without_address_is_usage() {
        let r = handle_list_create(br#"{"members":["a@b.test"]}"#);
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
        assert!(r.body.contains("address"), "{}", r.body);
    }

    #[test]
    fn members_without_add_or_remove_is_usage() {
        let r = handle_members_post(br#"{"address":"team@b.test"}"#);
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
    }

    #[test]
    fn delete_without_address_is_usage() {
        let r = handle_list_delete(br#"{}"#);
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
    }

    #[test]
    fn get_without_address_is_usage_not_405() {
        let r = handle_list_get(&HashMap::new());
        assert_eq!(r.status, "400 Bad Request");
        assert_ne!(r.status, "405 Method Not Allowed");
    }

    #[test]
    fn external_member_is_400() {
        let r = handle_list_create(
            br#"{"address":"team@example.test","members":["nobody@example.test"]}"#,
        );
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
        assert!(
            r.body.contains("minted") || r.body.contains("member"),
            "{}",
            r.body
        );
    }

    #[test]
    fn recipients_set_object_is_not_array() {
        let v = jmap::recipients_set_object(&["a@b.test".into(), "c@d.test".into()]);
        assert!(v.is_object(), "{v}");
        assert_eq!(v["a@b.test"], true);
        assert!(v.as_array().is_none());
    }

    #[test]
    fn create_collides_with_minted_user_409() {
        let addr = format!(
            "list-collide-{}@example.test",
            uuid::Uuid::new_v4().simple()
        );
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO mail_addresses (id, address, domain_id, stalwart_account_id, \
                 owner_project_id, status, created_at, primary_can_manage, primary_can_delete) \
                 VALUES (?1, ?2, 'dom-x', 'acc-x', 'proj-x', 'active', 1, 1, 1)",
                rusqlite::params![uuid::Uuid::new_v4().to_string(), addr],
            )
            .expect("seed user");
        }
        let body = serde_json::json!({ "address": addr, "members": [] });
        let r = handle_list_create(body.to_string().as_bytes());
        assert_eq!(r.status, "409 Conflict", "{}", r.body);
        assert!(
            r.body.contains("exists") || r.body.contains("mailbox"),
            "{}",
            r.body
        );
    }
}
