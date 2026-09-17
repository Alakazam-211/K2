//! `POST /cli/mail/import` — Maildir and/or IMAP into a hosted address
//! (prd-hostmail-agent-cli-v1 C6/C22).
//!
//! Mail-manage (or owner/admin) may import into addresses that workspace
//! can manage. Does not rsync into SST. GET on this path is 405.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::cli_response::CliResponse;
use crate::mail::access;
use crate::mail::addresses;
use crate::mail::domains;
use crate::mail::jmap::{MailboxInfo, StalwartClient};
use crate::mail::messages::ReadError;

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct ImportBody {
    address: Option<String>,
    maildir: Option<String>,
    imapsync: Option<ImapsyncBody>,
    project: Option<String>,
    skip_inbox: bool,
}

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default)]
struct ImapsyncBody {
    host: Option<String>,
    user: Option<String>,
    password: Option<String>,
    port: Option<u16>,
}

/// One Maildir / Maildir++ message: path + folder key. Bytes stay on disk
/// until [`handle_import`] `fs::read`s a single file, imports, and drops it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaildirEntry {
    pub path: PathBuf,
    /// `INBOX` for top-level `cur/`+`new/`; Maildir++ name with leading `.` stripped.
    pub folder: String,
}

/// JMAP destination for a Maildir++ folder name (leading `.` already stripped
/// or ignored). Roles use [`StalwartClient::mailbox_role_id`]; anything else
/// matches [`StalwartClient::mailbox_list`] `name` or [`StalwartClient::mailbox_create`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FolderTarget {
    Role {
        role: &'static str,
        create_name: &'static str,
    },
    Named(String),
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

fn skip_maildir_file_name(name: &str) -> bool {
    let base = name
        .split_once(':')
        .map(|(b, _)| b)
        .unwrap_or(name)
        .rsplit('/')
        .next()
        .unwrap_or(name);
    let lower = base.to_ascii_lowercase();
    lower.starts_with("dovecot") || lower == "maildirsize" || lower == "subscriptions"
}

fn has_cur_or_new(dir: &Path) -> bool {
    dir.join("cur").is_dir() || dir.join("new").is_dir()
}

fn collect_cur_new(
    folder_dir: &Path,
    folder: &str,
    out: &mut Vec<MaildirEntry>,
) -> Result<(), String> {
    for sub in ["new", "cur"] {
        let dir = folder_dir.join(sub);
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
            let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if skip_maildir_file_name(name) {
                continue;
            }
            let empty = ent.metadata().map(|m| m.len() == 0).unwrap_or(false);
            if empty {
                continue;
            }
            out.push(MaildirEntry {
                path: p,
                folder: folder.to_string(),
            });
        }
    }
    Ok(())
}

/// Walk a Maildir (`new/` + `cur/` as `INBOX`) plus immediate Maildir++
/// `.Name/{new,cur}` folders. Does **not** read RFC822 bytes. Skips `tmp/`,
/// `dovecot*`, `maildirsize`, and `subscriptions`. `skip_inbox` omits top-level
/// `cur/`+`new/` (folders-only second pass).
pub fn walk_maildir(path: &Path, skip_inbox: bool) -> Result<Vec<MaildirEntry>, String> {
    if !path.is_dir() {
        return Err(format!("maildir '{}' is not a directory", path.display()));
    }
    let mut out = Vec::new();
    if !skip_inbox {
        collect_cur_new(path, "INBOX", &mut out)?;
    }
    let entries = fs::read_dir(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    for ent in entries {
        let ent = ent.map_err(|e| format!("read {}: {e}", path.display()))?;
        let p = ent.path();
        if !p.is_dir() {
            continue;
        }
        let name = match p.file_name().and_then(|s| s.to_str()) {
            Some(n) if n.starts_with('.') && n.len() > 1 => n,
            _ => continue,
        };
        if !has_cur_or_new(&p) {
            continue;
        }
        let folder = name.trim_start_matches('.').to_string();
        if folder.is_empty() {
            continue;
        }
        collect_cur_new(&p, &folder, &mut out)?;
    }
    if out.is_empty() && !has_cur_or_new(path) && !skip_inbox {
        return Err(format!(
            "maildir '{}' has no new/ or cur/ — pass a Maildir, not a mailbox file",
            path.display()
        ));
    }
    Ok(out)
}

/// Map a Maildir++ folder name (optional leading `.`) onto a JMAP role or
/// a create/list name. Case-insensitive for the well-known set.
pub(crate) fn map_maildir_folder(name: &str) -> FolderTarget {
    let n = name.trim().trim_start_matches('.');
    if n.is_empty() || n.eq_ignore_ascii_case("INBOX") {
        return FolderTarget::Role {
            role: "inbox",
            create_name: "Inbox",
        };
    }
    let lower = n.to_ascii_lowercase();
    match lower.as_str() {
        "sent" | "sent messages" | "sent items" | "lzteksent" => FolderTarget::Role {
            role: "sent",
            create_name: "Sent",
        },
        "drafts" => FolderTarget::Role {
            role: "drafts",
            create_name: "Drafts",
        },
        "trash" | "deleted items" => FolderTarget::Role {
            role: "trash",
            create_name: "Trash",
        },
        "junk" | "junk mail" | "spam" => FolderTarget::Role {
            role: "junk",
            create_name: "Junk",
        },
        _ => FolderTarget::Named(n.to_string()),
    }
}

struct FolderIdCache {
    ids: HashMap<String, String>,
    list: Option<Vec<MailboxInfo>>,
}

impl FolderIdCache {
    fn new() -> Self {
        Self {
            ids: HashMap::new(),
            list: None,
        }
    }

    fn mailbox_list(
        &mut self,
        client: &StalwartClient,
        account_id: &str,
    ) -> Result<&[MailboxInfo], String> {
        if self.list.is_none() {
            self.list = Some(client.mailbox_list(account_id)?);
        }
        Ok(self.list.as_deref().unwrap_or(&[]))
    }

    fn find_named(list: &[MailboxInfo], name: &str) -> Option<String> {
        list.iter()
            .find(|m| m.name.eq_ignore_ascii_case(name))
            .map(|m| m.id.clone())
    }

    fn resolve(
        &mut self,
        client: &StalwartClient,
        account_id: &str,
        folder: &str,
    ) -> Result<String, String> {
        let cache_key = folder.trim().trim_start_matches('.').to_ascii_lowercase();
        if let Some(id) = self.ids.get(&cache_key) {
            return Ok(id.clone());
        }
        let target = map_maildir_folder(folder);
        let id = match target {
            FolderTarget::Role { role, create_name } => {
                if let Some(id) = client.mailbox_role_id(account_id, role)? {
                    id
                } else {
                    let named = {
                        let list = self.mailbox_list(client, account_id)?;
                        Self::find_named(list, create_name)
                    };
                    match named {
                        Some(id) => id,
                        None => {
                            let created = client.mailbox_create(account_id, create_name)?;
                            self.list = None;
                            created
                        }
                    }
                }
            }
            FolderTarget::Named(name) => {
                let named = {
                    let list = self.mailbox_list(client, account_id)?;
                    Self::find_named(list, &name)
                };
                match named {
                    Some(id) => id,
                    None => {
                        let created = client.mailbox_create(account_id, &name)?;
                        self.list = None;
                        created
                    }
                }
            }
        };
        self.ids.insert(cache_key, id.clone());
        Ok(id)
    }
}

/// Import one RFC822 into `mailbox_id` via JMAP Email/import.
pub fn import_rfc822(
    client: &StalwartClient,
    account_id: &str,
    rfc822: &[u8],
    mailbox_id: &str,
) -> Result<(), String> {
    let blob_id = client.blob_upload(account_id, rfc822)?;
    client.email_import(account_id, &blob_id, mailbox_id)
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

/// POST `/cli/mail/import` `{address, maildir? | imapsync?, skipInbox?}`.
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

    let mut folders = FolderIdCache::new();
    let mut imported = 0u32;
    let mut failed = 0u32;
    let mut scanned = 0u32;
    let mut last_err = None;

    if let Some(dir) = b
        .maildir
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let entries = match walk_maildir(Path::new(dir), b.skip_inbox) {
            Ok(e) => e,
            Err(e) => return err_json("400 Bad Request", "usage", e),
        };
        for entry in entries {
            scanned += 1;
            let bytes = match fs::read(&entry.path) {
                Ok(b) => b,
                Err(e) => {
                    failed += 1;
                    last_err = Some(format!("read {}: {e}", entry.path.display()));
                    continue;
                }
            };
            if bytes.is_empty() {
                continue;
            }
            let mailbox_id = match folders.resolve(&client, account_id, &entry.folder) {
                Ok(id) => id,
                Err(e) => {
                    failed += 1;
                    last_err = Some(e);
                    continue;
                }
            };
            match import_rfc822(&client, account_id, &bytes, &mailbox_id) {
                Ok(()) => imported += 1,
                Err(e) => {
                    failed += 1;
                    last_err = Some(e);
                }
            }
        }
    }
    if let Some(imap) = &b.imapsync {
        match fetch_imapsync(imap) {
            Ok(messages) => {
                let inbox = match folders.resolve(&client, account_id, "INBOX") {
                    Ok(id) => id,
                    Err(e) => {
                        return err_json("502 Bad Gateway", "engine", e);
                    }
                };
                for (_name, bytes) in messages {
                    scanned += 1;
                    match import_rfc822(&client, account_id, &bytes, &inbox) {
                        Ok(()) => imported += 1,
                        Err(e) => {
                            failed += 1;
                            last_err = Some(e);
                        }
                    }
                }
            }
            Err(e) => return err_json("502 Bad Gateway", "engine", e),
        }
    }

    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "address": address,
            "imported": imported,
            "failed": failed,
            "scanned": scanned,
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

    fn tmp_maildir(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "k2-maildir-{}-{}-{tag}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn write_msg(path: &Path, body: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, body).unwrap();
    }

    #[test]
    fn walk_maildir_walks_new_and_cur() {
        let dir = tmp_maildir("curnew");
        let _ = fs::remove_dir_all(&dir);
        write_msg(&dir.join("new/a"), b"From: a\r\n\r\nhello");
        write_msg(&dir.join("cur/b:2,S"), b"From: b\r\n\r\nworld");
        write_msg(&dir.join("tmp/ignored"), b"nope");
        let msgs = walk_maildir(&dir, false).expect("walk");
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(msgs.len(), 2, "{msgs:?}");
        assert!(msgs.iter().all(|e| e.folder == "INBOX"), "{msgs:?}");
    }

    #[test]
    fn walk_maildir_missing_is_usage() {
        let err = walk_maildir(Path::new("/no/such/maildir"), false).unwrap_err();
        assert!(err.contains("not a directory"), "{err}");
    }

    #[test]
    fn walk_maildir_plus_plus_counts_sent_skips_tmp_and_dovecot() {
        let dir = tmp_maildir("pp");
        let _ = fs::remove_dir_all(&dir);
        write_msg(&dir.join("cur/inbox1"), b"From: a\r\n\r\nin");
        write_msg(&dir.join("tmp/ignored"), b"nope");
        write_msg(&dir.join("cur/dovecot-uidlist"), b"uids");
        write_msg(&dir.join("dovecot-uidlist"), b"root");
        write_msg(&dir.join("maildirsize"), b"1");
        write_msg(&dir.join("subscriptions"), b"INBOX");
        write_msg(&dir.join(".Sent/cur/s1:2,S"), b"From: s\r\n\r\nsent");
        write_msg(&dir.join(".Sent/tmp/ignored"), b"nope");
        write_msg(&dir.join(".Archive/new/a1"), b"From: ar\r\n\r\narch");

        let entries = walk_maildir(&dir, false).expect("walk");
        let skipped = walk_maildir(&dir, true).expect("skipInbox");

        assert_eq!(entries.len(), 3, "{entries:?}");
        let mut folders: Vec<&str> = entries.iter().map(|e| e.folder.as_str()).collect();
        folders.sort_unstable();
        assert_eq!(folders, ["Archive", "INBOX", "Sent"], "{entries:?}");
        assert!(
            entries.iter().all(|e| e.path.is_file()),
            "walker yields paths, not bytes: {entries:?}"
        );

        assert_eq!(skipped.len(), 2, "{skipped:?}");
        assert!(
            skipped.iter().all(|e| e.folder != "INBOX"),
            "skipInbox must omit top-level cur/new: {skipped:?}"
        );
        let mut skip_folders: Vec<&str> = skipped.iter().map(|e| e.folder.as_str()).collect();
        skip_folders.sort_unstable();
        assert_eq!(skip_folders, ["Archive", "Sent"], "{skipped:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn walk_maildir_skip_inbox_omits_top_level_only() {
        let dir = tmp_maildir("skip");
        let _ = fs::remove_dir_all(&dir);
        write_msg(&dir.join("cur/only-inbox"), b"From: i\r\n\r\nin");
        write_msg(&dir.join(".Drafts/cur/d1"), b"From: d\r\n\r\ndraft");
        let skipped = walk_maildir(&dir, true).expect("skip");
        let full = walk_maildir(&dir, false).expect("full");
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(full.len(), 2, "{full:?}");
        assert_eq!(skipped.len(), 1, "{skipped:?}");
        assert_eq!(skipped[0].folder, "Drafts");
    }

    #[test]
    fn walk_maildir_entries_are_paths_not_loaded_bytes() {
        assert!(
            std::mem::size_of::<MaildirEntry>() < 1024,
            "MaildirEntry must not carry RFC822 bodies"
        );
        let dir = tmp_maildir("stream");
        let _ = fs::remove_dir_all(&dir);
        write_msg(&dir.join("cur/one"), b"From: a\r\n\r\none");
        write_msg(&dir.join("cur/two"), b"From: b\r\n\r\ntwo");
        let entries = walk_maildir(&dir, false).expect("walk");
        assert_eq!(entries.len(), 2);
        for e in entries {
            let bytes = fs::read(&e.path).expect("one file");
            assert!(!bytes.is_empty());
            drop(bytes);
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn folder_map_sent_messages_is_sent_role() {
        match map_maildir_folder(".Sent Messages") {
            FolderTarget::Role { role, create_name } => {
                assert_eq!(role, "sent");
                assert_eq!(create_name, "Sent");
            }
            other => panic!("expected sent role, got {other:?}"),
        }
        for name in ["Sent", "sent items", "lztekSent", ".lztekSent"] {
            match map_maildir_folder(name) {
                FolderTarget::Role { role, .. } => assert_eq!(role, "sent", "{name}"),
                other => panic!("{name}: {other:?}"),
            }
        }
    }

    #[test]
    fn folder_map_archive_is_named_create_or_list() {
        match map_maildir_folder(".Archive") {
            FolderTarget::Named(n) => assert_eq!(n, "Archive"),
            other => panic!("expected Named(Archive), got {other:?}"),
        }
        match map_maildir_folder("Archive") {
            FolderTarget::Named(n) => assert_eq!(n, "Archive"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn folder_map_well_known_roles() {
        assert!(matches!(
            map_maildir_folder("INBOX"),
            FolderTarget::Role { role: "inbox", .. }
        ));
        assert!(matches!(
            map_maildir_folder("Drafts"),
            FolderTarget::Role { role: "drafts", .. }
        ));
        assert!(matches!(
            map_maildir_folder("Deleted Items"),
            FolderTarget::Role { role: "trash", .. }
        ));
        assert!(matches!(
            map_maildir_folder("Junk Mail"),
            FolderTarget::Role { role: "junk", .. }
        ));
        assert!(matches!(
            map_maildir_folder("spam"),
            FolderTarget::Role { role: "junk", .. }
        ));
    }

    #[test]
    fn skip_inbox_json_defaults_false_and_parses_true() {
        let a: ImportBody = serde_json::from_slice(br#"{"address":"a@b.test"}"#).unwrap();
        assert!(!a.skip_inbox);
        let b: ImportBody = serde_json::from_slice(br#"{"skipInbox":true}"#).unwrap();
        assert!(b.skip_inbox);
        let c: ImportBody = serde_json::from_slice(br#"{"skipInbox":false}"#).unwrap();
        assert!(!c.skip_inbox);
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
