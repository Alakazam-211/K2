//! IMAP ACL / JMAP `shareWith` between minted Users.
//!
//! Routes `/cli/mail/acl` — never `/cli/mail/access/` (agent grants).
//! Does not write `mail_inbox_grants`. Probe Mailbox/set shareWith;
//! fallback IMAP SETACL. Both mailbox and user are minted.

use std::collections::BTreeSet;
use std::collections::HashMap;

use crate::cli_response::CliResponse;
use crate::mail::hosted::{self, err_json};
use crate::mail::jmap::StalwartClient;
use crate::mail::secrets::SecretStore;

const RIGHTS_CHARS: &str = "lrswipkxtena";

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default)]
struct GrantBody {
    mailbox: Option<String>,
    user: Option<String>,
    rights: Option<String>,
    read: Option<bool>,
    write: Option<bool>,
    post: Option<bool>,
    admin: Option<bool>,
}

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default)]
struct RevokeBody {
    mailbox: Option<String>,
    user: Option<String>,
}

/// Named map from vs-live N14: `--read=lr`, `--write=lrswite`,
/// `--post=p`, `--admin=alrswipkxten`. Raw string is `lrswipkxten` + `a`.
pub(crate) fn map_acl_rights(
    raw: Option<&str>,
    read: bool,
    write: bool,
    post: bool,
    admin: bool,
) -> Result<String, String> {
    let mut set = BTreeSet::new();
    if let Some(raw) = raw.map(str::trim).filter(|s| !s.is_empty()) {
        for c in raw.chars() {
            if !RIGHTS_CHARS.contains(c) {
                return Err(format!(
                    "unknown IMAP ACL right '{c}' — expected subset of {RIGHTS_CHARS}"
                ));
            }
            set.insert(c);
        }
    }
    if read {
        for c in "lr".chars() {
            set.insert(c);
        }
    }
    if write {
        for c in "lrswite".chars() {
            set.insert(c);
        }
    }
    if post {
        set.insert('p');
    }
    if admin {
        for c in "alrswipkxten".chars() {
            set.insert(c);
        }
    }
    if set.is_empty() {
        return Err("give rights (lrswipkxtena) or --read/--write/--post/--admin".to_string());
    }
    Ok(RIGHTS_CHARS.chars().filter(|c| set.contains(c)).collect())
}

pub(crate) fn share_with_from_rights(principal_id: &str, rights: &str) -> serde_json::Value {
    let has = |c: char| rights.contains(c);
    let admin = has('a');
    serde_json::json!({
        principal_id: {
            "mayReadItems": admin || has('l') || has('r'),
            "mayAddItems": admin || has('i'),
            "mayRemoveItems": admin || has('t'),
            "maySetSeen": admin || has('s'),
            "maySetKeywords": admin || has('w'),
            "mayCreateChild": admin || has('k'),
            "mayRename": admin,
            "mayDelete": admin || has('x'),
            "maySubmit": admin || has('p'),
        }
    })
}

fn two_minted(
    mailbox: &str,
    user: &str,
) -> Result<
    (
        k2_core::db::schema::MailAddress,
        k2_core::db::schema::MailAddress,
    ),
    CliResponse,
> {
    let mb = hosted::load_active_hosted(mailbox).map_err(hosted::addr_err)?;
    let usr = hosted::load_active_hosted(user).map_err(hosted::addr_err)?;
    Ok((mb, usr))
}

fn grants_unchanged() {
    // Intentional: this module never writes mail_inbox_grants.
}

fn apply_share_or_imap(
    engine: &StalwartClient,
    mailbox: &k2_core::db::schema::MailAddress,
    user: &k2_core::db::schema::MailAddress,
    rights: Option<&str>,
) -> Result<&'static str, CliResponse> {
    grants_unchanged();
    let account_id = hosted::account_id_of(mailbox)?;
    let sharee = hosted::account_id_of(user)?;
    let inbox = engine
        .mailbox_id_for_role(&account_id, "inbox")
        .map_err(|e| err_json("502 Bad Gateway", "engine", e))?;
    match engine.mailbox_probe_share_with(&account_id, &inbox) {
        Ok(Some(mut existing)) => {
            if let Some(rights) = rights {
                let patch = share_with_from_rights(&sharee, rights);
                if let Some(obj) = existing.as_object_mut() {
                    if let Some(p) = patch.as_object() {
                        for (k, v) in p {
                            obj.insert(k.clone(), v.clone());
                        }
                    }
                } else {
                    existing = patch;
                }
                engine
                    .mailbox_set_share_with(&account_id, &inbox, existing)
                    .map_err(|e| err_json("502 Bad Gateway", "engine", e))?;
            } else {
                // revoke: shareWith/{id} = null
                let mut obj = existing.as_object().cloned().unwrap_or_default();
                obj.insert(sharee, serde_json::Value::Null);
                engine
                    .mailbox_set_share_with(&account_id, &inbox, serde_json::Value::Object(obj))
                    .map_err(|e| err_json("502 Bad Gateway", "engine", e))?;
            }
            Ok("jmap-shareWith")
        }
        Ok(None) => {
            imap_setacl(&mailbox.address, &user.address, rights)?;
            Ok("imap-setacl")
        }
        Err(e) => Err(err_json("502 Bad Gateway", "engine", e)),
    }
}

fn imap_setacl(mailbox: &str, identifier: &str, rights: Option<&str>) -> Result<(), CliResponse> {
    // Loopback IMAP SETACL (RFC 4314). Fail loud — do not invent x:Acl.
    // Uses the mailbox's vaulted secret. K2 never EXPUNGEs.
    let row = hosted::load_active_hosted(mailbox).map_err(hosted::addr_err)?;
    let secret = crate::mail::secrets::FileSecretStore::default()
        .resolve(&format!("account-{}", row.id))
        .map_err(|e| err_json("502 Bad Gateway", "engine", e))?;
    let Some(password) = secret.filter(|s| !s.is_empty()) else {
        return Err(err_json(
            "502 Bad Gateway",
            "engine",
            "Mailbox/set shareWith unsupported (unknownProperty); IMAP SETACL needs the mailbox secret (rotate, then retry)".to_string(),
        ));
    };
    let (engine, hostname) = hosted::engine()?;
    let host = hostname
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "127.0.0.1".to_string());
    let cmd = match rights {
        Some(r) => format!("SETACL INBOX {identifier} {r}"),
        None => format!("DELETEACL INBOX {identifier}"),
    };
    let _ = engine; // JMAP already probed; IMAP is the fallback wire.
    match imap_run(&host, 143, mailbox, &password, &cmd) {
        Ok(()) => Ok(()),
        Err(e) => Err(err_json(
            "502 Bad Gateway",
            "engine",
            format!("Mailbox/set shareWith unsupported (unknownProperty); IMAP SETACL failed: {e}"),
        )),
    }
}

fn imap_run(host: &str, port: u16, user: &str, password: &str, cmd: &str) -> Result<(), String> {
    use imap::{ClientBuilder, ConnectionMode, TlsKind};
    let client = ClientBuilder::new(host, port)
        .mode(ConnectionMode::StartTls)
        .tls_kind(TlsKind::Rust)
        .connect()
        .map_err(|e| format!("IMAP connect {host}:{port}: {e}"))?;
    let mut session = client
        .login(user, password)
        .map_err(|(e, _)| format!("IMAP LOGIN: {e}"))?;
    session
        .run_command_and_check_ok(cmd)
        .map_err(|e| format!("IMAP {cmd}: {e}"))?;
    let _ = session.logout();
    Ok(())
}

/// GET `/cli/mail/acl?mailbox=`
pub fn handle_acl_get(params: &HashMap<String, String>) -> CliResponse {
    let mailbox = crate::cli::str_param(params, "mailbox");
    if mailbox.trim().is_empty() {
        return err_json(
            "400 Bad Request",
            "usage",
            "missing 'mailbox' — minted inbox to show ACL on".to_string(),
        );
    }
    let row = match hosted::load_active_hosted(&mailbox) {
        Ok(r) => r,
        Err(e) => return hosted::addr_err(e),
    };
    let account_id = match hosted::account_id_of(&row) {
        Ok(id) => id,
        Err(r) => return r,
    };
    let (engine, _) = match hosted::engine() {
        Ok(e) => e,
        Err(r) => return r,
    };
    let inbox = match engine.mailbox_id_for_role(&account_id, "inbox") {
        Ok(id) => id,
        Err(e) => return err_json("502 Bad Gateway", "engine", e),
    };
    match engine.mailbox_probe_share_with(&account_id, &inbox) {
        Ok(Some(share)) => CliResponse::ok_json(
            serde_json::json!({
                "ok": true,
                "mailbox": row.address,
                "via": "jmap-shareWith",
                "shareWith": share,
            })
            .to_string(),
        ),
        Ok(None) => CliResponse::ok_json(
            serde_json::json!({
                "ok": true,
                "mailbox": row.address,
                "via": "imap-setacl",
                "shareWith": serde_json::Value::Null,
                "hint": "JMAP shareWith unknownProperty — show via IMAP GETACL (IMAP fallback)",
            })
            .to_string(),
        ),
        Err(e) => err_json("502 Bad Gateway", "engine", e),
    }
}

/// POST `/cli/mail/acl` grant `{mailbox, user, rights}`
pub fn handle_acl_grant(body: &[u8]) -> CliResponse {
    let b: GrantBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => {
            return err_json(
                "400 Bad Request",
                "usage",
                format!("invalid JSON body: {e}"),
            )
        }
    };
    let mailbox = b.mailbox.as_deref().unwrap_or("").trim();
    let user = b.user.as_deref().unwrap_or("").trim();
    if mailbox.is_empty() || user.is_empty() {
        return err_json(
            "400 Bad Request",
            "usage",
            "missing 'mailbox' and 'user' — both minted inboxes".to_string(),
        );
    }
    let rights = match map_acl_rights(
        b.rights.as_deref(),
        b.read.unwrap_or(false),
        b.write.unwrap_or(false),
        b.post.unwrap_or(false),
        b.admin.unwrap_or(false),
    ) {
        Ok(r) => r,
        Err(h) => return err_json("400 Bad Request", "usage", h),
    };
    let (mb, usr) = match two_minted(mailbox, user) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let (engine, _) = match hosted::engine() {
        Ok(e) => e,
        Err(r) => return r,
    };
    let via = match apply_share_or_imap(&engine, &mb, &usr, Some(&rights)) {
        Ok(v) => v,
        Err(r) => return r,
    };
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "mailbox": mb.address,
            "user": usr.address,
            "rights": rights,
            "via": via,
            "agentGrant": false,
        })
        .to_string(),
    )
}

/// POST `/cli/mail/acl/revoke` `{mailbox, user}`
pub fn handle_acl_revoke(body: &[u8]) -> CliResponse {
    let b: RevokeBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => {
            return err_json(
                "400 Bad Request",
                "usage",
                format!("invalid JSON body: {e}"),
            )
        }
    };
    let mailbox = b.mailbox.as_deref().unwrap_or("").trim();
    let user = b.user.as_deref().unwrap_or("").trim();
    if mailbox.is_empty() || user.is_empty() {
        return err_json(
            "400 Bad Request",
            "usage",
            "missing 'mailbox' and 'user' — both minted inboxes".to_string(),
        );
    }
    let (mb, usr) = match two_minted(mailbox, user) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let (engine, _) = match hosted::engine() {
        Ok(e) => e,
        Err(r) => return r,
    };
    let via = match apply_share_or_imap(&engine, &mb, &usr, None) {
        Ok(v) => v,
        Err(r) => return r,
    };
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "mailbox": mb.address,
            "user": usr.address,
            "revoked": true,
            "via": via,
            "agentGrant": false,
        })
        .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_rights_map_n14() {
        assert_eq!(
            map_acl_rights(None, true, false, false, false).unwrap(),
            "lr"
        );
        assert_eq!(
            map_acl_rights(None, false, true, false, false).unwrap(),
            "lrswite"
        );
        assert_eq!(
            map_acl_rights(None, false, false, true, false).unwrap(),
            "p"
        );
        let admin = map_acl_rights(None, false, false, false, true).unwrap();
        assert!(admin.contains('a'), "{admin}");
        assert!(
            admin.contains('e'),
            "expunge right is the client's; K2 never EXPUNGEs: {admin}"
        );
        for c in "lrswipkxten".chars() {
            assert!(admin.contains(c), "admin missing {c}: {admin}");
        }
        assert_eq!(
            map_acl_rights(Some("lrswipkxten"), false, false, false, false).unwrap(),
            "lrswipkxten"
        );
        assert!(map_acl_rights(None, false, false, false, false).is_err());
        assert!(map_acl_rights(Some("z"), false, false, false, false).is_err());
    }

    #[test]
    fn grant_unminted_is_not_found() {
        let r = handle_acl_grant(
            br#"{"mailbox":"no-acl-mb@example.test","user":"no-acl-u@example.test","read":true}"#,
        );
        assert_eq!(r.status, "404 Not Found", "{}", r.body);
        assert!(!r.body.contains("mail_inbox_grants"), "{}", r.body);
    }

    #[test]
    fn get_without_mailbox_is_usage_not_405() {
        let r = handle_acl_get(&HashMap::new());
        assert_eq!(r.status, "400 Bad Request");
        assert_ne!(r.status, "405 Method Not Allowed");
    }

    #[test]
    fn share_with_patch_uses_principal_id() {
        let v = share_with_from_rights("acc-b", "lr");
        assert_eq!(v["acc-b"]["mayReadItems"], true);
        assert_eq!(v["acc-b"]["mayAddItems"], false);
    }
}
