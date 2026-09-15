//! `POST /cli/mail/import` — Maildir and/or IMAP into a hosted address
//! (prd-hostmail-agent-cli-v1 C6/C22).
//!
//! Mail-manage (or owner/admin) may import into addresses that workspace
//! can manage. Does not rsync into SST. GET on this path is 405.

use std::fs;
use std::path::Path;

use crate::cli_response::CliResponse;
use crate::mail::access;
use crate::mail::addresses;
use crate::mail::domains;
use crate::mail::jmap::StalwartClient;
use crate::mail::messages::ReadError;

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct ImportBody {
    address: Option<String>,
    maildir: Option<String>,
    imapsync: Option<ImapsyncBody>,
    project: Option<String>,
}

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default)]
struct ImapsyncBody {
    host: Option<String>,
    user: Option<String>,
    password: Option<String>,
    port: Option<u16>,
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

/// Walk a Maildir (`new/` + `cur/`; ignore `tmp/`) and return RFC822
/// file bytes. Missing path is a usage error.
pub fn read_maildir(path: &Path) -> Result<Vec<(String, Vec<u8>)>, String> {
    if !path.is_dir() {
        return Err(format!("maildir '{}' is not a directory", path.display()));
    }
    let mut out = Vec::new();
    for sub in ["new", "cur"] {
        let dir = path.join(sub);
        if !dir.is_dir() {
            continue;
        }
        let entries = fs::read_dir(&dir).map_err(|e| format!("read {}: {e}", dir.display()))?;
        for ent in entries {
            let ent = ent.map_err(|e| format!("read {}: {e}", dir.display()))?;
            let p = ent.path();
            if !p.is_file() {
                continue;
            }
            let bytes = fs::read(&p).map_err(|e| format!("read {}: {e}", p.display()))?;
            if bytes.is_empty() {
                continue;
            }
            out.push((p.display().to_string(), bytes));
        }
    }
    if out.is_empty() && !path.join("new").is_dir() && !path.join("cur").is_dir() {
        return Err(format!(
            "maildir '{}' has no new/ or cur/ — pass a Maildir, not a mailbox file",
            path.display()
        ));
    }
    Ok(out)
}

/// Import one RFC822 into the hosted account via JMAP Email/import.
pub fn import_rfc822(
    client: &StalwartClient,
    account_id: &str,
    rfc822: &[u8],
) -> Result<(), String> {
    let blob_id = client.blob_upload(account_id, rfc822)?;
    let inbox = client.mailbox_inbox_id(account_id)?;
    client.email_import(account_id, &blob_id, &inbox)
}

fn authorize_address(address: &str) -> Result<k2_core::db::schema::MailAddress, CliResponse> {
    let principal = crate::caller_workspace::request_principal();
    let Some(row) = ({
        let db = k2_core::db::shared();
        let conn = db.lock();
        addresses::load_address(&conn, address)
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
    if let Some(p) = principal {
        match access::can_manage(&p.workspace_uuid, address) {
            Ok(_) => Ok(row),
            Err(ReadError::NotFound(hint)) => Err(err_json("403 Forbidden", "forbidden", hint)),
            Err(ReadError::Usage(hint)) => Err(err_json("400 Bad Request", "usage", hint)),
            Err(ReadError::Engine(hint)) => Err(err_json("502 Bad Gateway", "engine", hint)),
        }
    } else {
        Ok(row)
    }
}

/// POST `/cli/mail/import` `{address, maildir? | imapsync?}`.
pub fn handle_import(body: &[u8]) -> CliResponse {
    let b: ImportBody = match serde_json::from_slice(body) {
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
            "missing 'address' — import into which hosted address?".to_string(),
        );
    }
    let has_maildir = b
        .maildir
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .is_some();
    let has_imap = b
        .imapsync
        .as_ref()
        .and_then(|i| i.host.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .is_some();
    if !has_maildir && !has_imap {
        return err_json(
            "400 Bad Request",
            "usage",
            "pass maildir and/or imapsync (from=host)".to_string(),
        );
    }
    let _ = b.project;
    let row = match authorize_address(address) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let Some(account_id) = row
        .stalwart_account_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return err_json(
            "409 Conflict",
            "not_ready",
            format!("address '{address}' has no mail-server account"),
        );
    };

    let mut messages: Vec<(String, Vec<u8>)> = Vec::new();
    if let Some(dir) = b.maildir.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        match read_maildir(Path::new(dir)) {
            Ok(m) => messages.extend(m),
            Err(e) => return err_json("400 Bad Request", "usage", e),
        }
    }
    if let Some(imap) = &b.imapsync {
        match fetch_imapsync(imap) {
            Ok(m) => messages.extend(m),
            Err(e) => return err_json("502 Bad Gateway", "engine", e),
        }
    }

    let client = match domains::engine_from_db() {
        Ok((c, _)) => c,
        Err(e) => {
            return err_json(
                "409 Conflict",
                "not_ready",
                format!("cannot import while the mail server is down: {e}"),
            )
        }
    };

    let mut imported = 0u32;
    let mut failed = 0u32;
    let mut last_err = None;
    for (_name, bytes) in &messages {
        match import_rfc822(&client, account_id, bytes) {
            Ok(()) => imported += 1,
            Err(e) => {
                failed += 1;
                last_err = Some(e);
            }
        }
    }

    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "address": address,
            "imported": imported,
            "failed": failed,
            "scanned": messages.len(),
            "lastError": last_err,
            "hint": format!("imported {imported} message(s) into {address}"),
        })
        .to_string(),
    )
}

fn fetch_imapsync(imap: &ImapsyncBody) -> Result<Vec<(String, Vec<u8>)>, String> {
    let host = imap
        .host
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "imapsync.host is required".to_string())?;
    let user = imap
        .user
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "imapsync.user is required".to_string())?;
    let password = imap
        .password
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "imapsync.password is required".to_string())?;
    let port = imap.port.unwrap_or(993);
    crate::mail::external_imap::fetch_all_rfc822(host, port, user, password)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_maildir_walks_new_and_cur() {
        let dir = std::env::temp_dir().join(format!("k2-maildir-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("new")).unwrap();
        fs::create_dir_all(dir.join("cur")).unwrap();
        fs::create_dir_all(dir.join("tmp")).unwrap();
        fs::write(dir.join("new/a"), b"From: a\r\n\r\nhello").unwrap();
        fs::write(dir.join("cur/b:2,S"), b"From: b\r\n\r\nworld").unwrap();
        fs::write(dir.join("tmp/ignored"), b"nope").unwrap();
        let msgs = read_maildir(&dir).expect("walk");
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(msgs.len(), 2, "{msgs:?}");
    }

    #[test]
    fn read_maildir_missing_is_usage() {
        let err = read_maildir(Path::new("/no/such/maildir")).unwrap_err();
        assert!(err.contains("not a directory"), "{err}");
    }

    #[test]
    fn import_without_address_is_usage() {
        let r = handle_import(br#"{"maildir":"/tmp"}"#);
        assert_eq!(r.status, "400 Bad Request");
        assert!(r.body.contains("address"), "{}", r.body);
    }

    #[test]
    fn import_without_source_is_usage() {
        let r = handle_import(br#"{"address":"a@b.test"}"#);
        assert_eq!(r.status, "400 Bad Request");
        assert!(r.body.contains("maildir"), "{}", r.body);
    }
}
