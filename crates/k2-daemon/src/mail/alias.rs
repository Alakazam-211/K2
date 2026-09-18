//! `GET|POST /cli/mail/alias` and `POST /cli/mail/alias/remove`.
//!
//! Extra `Account.aliases` EmailAlias on an existing User mailbox.
//! Not a mint: no sqlite row, no cap, no password, not IMAP login.
//! Collision vs other `mail_addresses` and other Account aliases.

use std::collections::HashMap;

use crate::cli_response::CliResponse;
use crate::mail::addresses::{self, AddrError};
use crate::mail::domains;
use crate::mail::hostmail_auth::{account_id_of, authorize_mailbox, engine, err_json};
use crate::mail::jmap::{alias_collides, AccountUser, EmailAlias};

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default)]
struct AliasBody {
    mailbox: Option<String>,
    alias: Option<String>,
}

fn split_addr(address: &str) -> Option<(&str, &str)> {
    address.split_once('@')
}

fn domain_id_for_k2_domain(domain: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    domains::load_domain(&conn, domain)
        .and_then(|d| d.stalwart_domain_id)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn mint_taken(address: &str) -> bool {
    let db = k2_core::db::shared();
    let conn = db.lock();
    addresses::load_address(&conn, address).is_some()
}

fn normalize_alias(raw: &str) -> Result<String, CliResponse> {
    let address = match addresses::normalize_address(raw) {
        Ok(a) => a,
        Err(AddrError::Usage(h)) => return Err(err_json("400 Bad Request", "usage", h)),
        Err(e) => return Err(err_json("400 Bad Request", "usage", format!("{e:?}"))),
    };
    let (local, _) = split_addr(&address).ok_or_else(|| {
        err_json(
            "400 Bad Request",
            "usage",
            format!("'{raw}' is not a full address"),
        )
    })?;
    if local.contains('+') {
        return Err(err_json(
            "400 Bad Request",
            "usage",
            "do not mint a +tag as an alias — plus-addressing already lands in the mailbox"
                .to_string(),
        ));
    }
    if let Err(h) = addresses::validate_local_part(local) {
        return Err(err_json("400 Bad Request", "usage", h));
    }
    Ok(address)
}

fn full_alias_addrs(user: &AccountUser, domain_name: &str) -> Vec<String> {
    user.aliases
        .iter()
        .map(|a| format!("{}@{domain_name}", a.name))
        .collect()
}

/// GET `/cli/mail/alias?mailbox=` — extra EmailAlias addresses (not the primary).
pub fn handle_alias_list(params: &HashMap<String, String>) -> CliResponse {
    let mailbox = crate::cli::str_param(params, "mailbox");
    if mailbox.trim().is_empty() {
        return err_json(
            "400 Bad Request",
            "usage",
            "missing 'mailbox' — which hosted mailbox?".to_string(),
        );
    }
    let row = match authorize_mailbox(&mailbox) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let account_id = match account_id_of(&row, &row.address) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let (_, domain) = split_addr(&row.address).unwrap_or(("", ""));
    let client = match engine() {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    match client.account_get_user(&account_id) {
        Ok(user) => {
            let aliases = full_alias_addrs(&user, domain);
            CliResponse::ok_json(
                serde_json::json!({
                    "ok": true,
                    "mailbox": row.address,
                    "aliases": aliases,
                })
                .to_string(),
            )
        }
        Err(e) => err_json("502 Bad Gateway", "engine", e),
    }
}

/// POST `/cli/mail/alias` `{mailbox, alias}` — add EmailAlias. Not a mint.
pub fn handle_alias_add(body: &[u8]) -> CliResponse {
    let b: AliasBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let mailbox_raw = b.mailbox.as_deref().map(str::trim).unwrap_or("");
    let alias_raw = b.alias.as_deref().map(str::trim).unwrap_or("");
    if mailbox_raw.is_empty() || alias_raw.is_empty() {
        return err_json(
            "400 Bad Request",
            "usage",
            "need 'mailbox' and 'alias' (full addresses). Alias is not a mint — use k2 hostmail create for a mailbox."
                .to_string(),
        );
    }
    let alias = match normalize_alias(alias_raw) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    let row = match authorize_mailbox(mailbox_raw) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let (mbox_local, mbox_domain) = split_addr(&row.address).unwrap_or(("", ""));
    let (alias_local, alias_domain) = split_addr(&alias).unwrap_or(("", ""));
    if alias_domain != mbox_domain {
        return err_json(
            "400 Bad Request",
            "usage",
            format!(
                "alias '{alias}' must be on the same domain as mailbox '{}'",
                row.address
            ),
        );
    }
    if alias == row.address || alias_local == mbox_local {
        return err_json(
            "400 Bad Request",
            "usage",
            "cannot add the mailbox's primary mint address as an alias".to_string(),
        );
    }
    if mint_taken(&alias) {
        return err_json(
            "409 Conflict",
            "exists",
            format!("'{alias}' is already a minted mailbox"),
        );
    }
    let account_id = match account_id_of(&row, &row.address) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let domain_id = match domain_id_for_k2_domain(mbox_domain) {
        Some(id) => id,
        None => {
            return err_json(
                "409 Conflict",
                "not_ready",
                format!("domain '{mbox_domain}' has no mail-server id"),
            )
        }
    };
    let client = match engine() {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    let users = match client.account_list_users() {
        Ok(u) => u,
        Err(e) => return err_json("502 Bad Gateway", "engine", e),
    };
    if alias_collides(&account_id, alias_local, &domain_id, &users) {
        return err_json(
            "409 Conflict",
            "exists",
            format!("'{alias}' is already an alias on another mailbox"),
        );
    }
    let mut user = match client.account_get_user(&account_id) {
        Ok(u) => u,
        Err(e) => return err_json("502 Bad Gateway", "engine", e),
    };
    if user
        .aliases
        .iter()
        .any(|a| a.name == alias_local && a.domain_id == domain_id)
    {
        let aliases = full_alias_addrs(&user, mbox_domain);
        return CliResponse::ok_json(
            serde_json::json!({
                "ok": true,
                "mailbox": row.address,
                "alias": alias,
                "existing": true,
                "aliases": aliases,
            })
            .to_string(),
        );
    }
    user.aliases.push(EmailAlias {
        name: alias_local.to_string(),
        domain_id,
        enabled: true,
    });
    if let Err(e) = client.account_set_aliases(&account_id, &user.aliases) {
        return err_json("502 Bad Gateway", "engine", e);
    }
    let aliases = full_alias_addrs(&user, mbox_domain);
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "mailbox": row.address,
            "alias": alias,
            "existing": false,
            "aliases": aliases,
        })
        .to_string(),
    )
}

/// POST `/cli/mail/alias/remove` `{mailbox, alias}`. Cannot drop the primary mint.
pub fn handle_alias_remove(body: &[u8]) -> CliResponse {
    let b: AliasBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let mailbox_raw = b.mailbox.as_deref().map(str::trim).unwrap_or("");
    let alias_raw = b.alias.as_deref().map(str::trim).unwrap_or("");
    if mailbox_raw.is_empty() || alias_raw.is_empty() {
        return err_json(
            "400 Bad Request",
            "usage",
            "need 'mailbox' and 'alias'".to_string(),
        );
    }
    let row = match authorize_mailbox(mailbox_raw) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let alias = match addresses::normalize_address(alias_raw) {
        Ok(a) => a,
        Err(AddrError::Usage(h)) => return err_json("400 Bad Request", "usage", h),
        Err(e) => return err_json("400 Bad Request", "usage", format!("{e:?}")),
    };
    if alias == row.address {
        return err_json(
            "400 Bad Request",
            "usage",
            "cannot remove the mailbox's primary mint address this way — use k2 hostmail delete"
                .to_string(),
        );
    }
    let (_, mbox_domain) = split_addr(&row.address).unwrap_or(("", ""));
    let (alias_local, alias_domain) = split_addr(&alias).unwrap_or(("", ""));
    if alias_domain != mbox_domain {
        return err_json(
            "400 Bad Request",
            "usage",
            format!(
                "alias '{alias}' must be on the same domain as mailbox '{}'",
                row.address
            ),
        );
    }
    let account_id = match account_id_of(&row, &row.address) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let client = match engine() {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    let mut user = match client.account_get_user(&account_id) {
        Ok(u) => u,
        Err(e) => return err_json("502 Bad Gateway", "engine", e),
    };
    let before = user.aliases.len();
    user.aliases
        .retain(|a| !(a.name == alias_local && a.domain_id == user.domain_id));
    if user.aliases.len() == before {
        return err_json(
            "404 Not Found",
            "not_found",
            format!("'{alias}' is not an alias on '{}'", row.address),
        );
    }
    if let Err(e) = client.account_set_aliases(&account_id, &user.aliases) {
        return err_json("502 Bad Gateway", "engine", e);
    }
    let aliases = full_alias_addrs(&user, mbox_domain);
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "mailbox": row.address,
            "removed": alias,
            "aliases": aliases,
        })
        .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed_mailbox(address: &str, account_id: &str) {
        let (local, domain) = address.split_once('@').expect("addr");
        let db = k2_core::db::shared();
        let conn = db.lock();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let domain_id = uuid::Uuid::new_v4().to_string();
        let _ = conn.execute(
            "INSERT OR IGNORE INTO mail_domains (id, domain, stalwart_domain_id, status, created_at) \
             VALUES (?1, ?2, ?3, 'verified', ?4)",
            rusqlite::params![domain_id, domain, format!("sw-{domain}"), now],
        );
        let domain_row_id: String = conn
            .query_row(
                "SELECT id FROM mail_domains WHERE domain = ?1",
                rusqlite::params![domain],
                |r| r.get(0),
            )
            .expect("domain row");
        let id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO mail_addresses (id, address, domain_id, stalwart_account_id, \
             owner_project_id, status, created_at) VALUES (?1, ?2, ?3, ?4, ?5, 'active', ?6)",
            rusqlite::params![id, address, domain_row_id, account_id, "ws-alias-test", now],
        )
        .unwrap_or_else(|e| panic!("seed {local}: {e}"));
    }

    fn cleanup(address: &str) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = conn.execute(
            "DELETE FROM mail_addresses WHERE address = ?1",
            rusqlite::params![address],
        );
    }

    #[test]
    fn list_without_mailbox_is_usage_not_405() {
        let r = handle_alias_list(&HashMap::new());
        assert_eq!(r.status, "400 Bad Request");
        assert!(r.body.contains("mailbox"), "{}", r.body);
        assert_ne!(r.status, "405 Method Not Allowed");
    }

    #[test]
    fn add_plus_tag_is_usage() {
        let r = handle_alias_add(
            br#"{"mailbox":"bot@alias-plus.test","alias":"bot+tag@alias-plus.test"}"#,
        );
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
        assert!(
            r.body.contains('+') || r.body.contains("plus"),
            "{}",
            r.body
        );
    }

    #[test]
    fn add_unknown_mailbox_is_not_found() {
        let r = handle_alias_add(
            br#"{"mailbox":"nobody@no-such-alias-box.example.test","alias":"sales@no-such-alias-box.example.test"}"#,
        );
        assert_eq!(r.status, "404 Not Found", "{}", r.body);
    }

    #[test]
    fn add_cross_domain_is_usage() {
        let a = format!("bot@alias-xd-{}.test", uuid::Uuid::new_v4().simple());
        seed_mailbox(&a, "acc-xd");
        let body = serde_json::json!({"mailbox": a, "alias": "sales@other.test"});
        let r = handle_alias_add(body.to_string().as_bytes());
        cleanup(&a);
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
        assert!(r.body.contains("same domain"), "{}", r.body);
    }

    #[test]
    fn add_collision_with_other_mint_is_409() {
        let tag = uuid::Uuid::new_v4().simple();
        let domain = format!("alias-col-{tag}.test");
        let a = format!("bot@{domain}");
        let b = format!("sales@{domain}");
        seed_mailbox(&a, "acc-a");
        seed_mailbox(&b, "acc-b");
        let body = serde_json::json!({"mailbox": a, "alias": b});
        let r = handle_alias_add(body.to_string().as_bytes());
        cleanup(&a);
        cleanup(&b);
        assert_eq!(r.status, "409 Conflict", "{}", r.body);
        assert!(
            r.body.contains("minted") || r.body.contains("exists"),
            "{}",
            r.body
        );
    }

    #[test]
    fn remove_primary_is_usage() {
        let tag = uuid::Uuid::new_v4().simple();
        let a = format!("bot@alias-pri-{tag}.test");
        seed_mailbox(&a, "acc-pri");
        let body = serde_json::json!({"mailbox": a, "alias": a});
        let r = handle_alias_remove(body.to_string().as_bytes());
        cleanup(&a);
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
        assert!(r.body.contains("primary"), "{}", r.body);
    }

    #[test]
    fn alias_collides_other_account() {
        let users = vec![
            AccountUser {
                id: "me".into(),
                name: "bot".into(),
                domain_id: "d1".into(),
                aliases: vec![],
            },
            AccountUser {
                id: "other".into(),
                name: "help".into(),
                domain_id: "d1".into(),
                aliases: vec![EmailAlias {
                    name: "sales".into(),
                    domain_id: "d1".into(),
                    enabled: true,
                }],
            },
        ];
        assert!(alias_collides("me", "sales", "d1", &users));
        assert!(!alias_collides("other", "sales", "d1", &users));
        assert!(!alias_collides("me", "info", "d1", &users));
        assert!(alias_collides("me", "help", "d1", &users));
    }
}
