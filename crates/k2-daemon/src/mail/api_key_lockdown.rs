//! Hosted-mail API key lockdown (0.46.6 security fix).
//!
//! **The gap.** Stalwart's default user role carries `sysApiKeyCreate`,
//! so anyone holding a mailbox's password could mint a Stalwart API key
//! for that mailbox (`x:ApiKey/set` with the mailbox's own login). A key
//! is a separate credential: revoking the password is not, by design,
//! revoking the key.
//!
//! **The fix:**
//!
//! 1. Every mailbox K2 mints carries `sysApiKeyCreate` in
//!    `disabledPermissions` ([`mint_permissions`], merged on top of the
//!    DAV policy and the Sieve lock). Stalwart intersects a credential's
//!    permissions with the account's, so the mailbox can't grant it back.
//! 2. Existing K2 mailboxes get the same `Merge` patch in the boot /
//!    enable / upgrade reconcile ([`reconcile`], run from the Sieve
//!    lockdown's pass so both share one engine lock). One auth-audit line
//!    per mailbox changed ([`AUDIT_EVENT`]). Idempotent: a locked mailbox
//!    is never written again. Keys a mailbox already holds are counted
//!    and reported (owner notice + audit), not destroyed here — the
//!    password rotation revokes them.
//! 3. A mailbox password rotation (`addresses::rotate_address_password`)
//!    destroys every API key on that mailbox after the new password is
//!    set, and reports how many it revoked.
//!
//! K2's own key is untouched: it is minted on the Admin-role service
//! account (`k2-daemon`), never on a mailbox, and the reconcile skips
//! admin/service principals (`dav::protected_reason`).
//!
//! ✔ LIVE (Stalwart 0.16.10 and 0.16.25, 2026-10-28,
//! `tests/mail/stalwart-apikey-probe.py`): the name is accepted at create
//! and as a patch; a locked mailbox's password gets `forbidden` on
//! `x:ApiKey/set create` while its login and mail still work; the service
//! key still mints its own keys; `x:ApiKey/query` + `/set destroy` on the
//! mailbox `accountId` list and revoke exactly its API keys.
//!
//! `k2 hostmail audit-sieve` (`GET /cli/mail/sieve-audit`) reports, per
//! account, whether the mailbox still has the permission and how many API
//! keys it holds (`sieve_lockdown::audit`).

use std::collections::HashMap;

use serde_json::Value;

use crate::mail::dav::{merge_permissions, protected_reason};
use crate::mail::jmap::StalwartClient;

/// The permission a mailbox login loses. Exact Stalwart `Permission`
/// name, live-checked on 0.16.10 and 0.16.25: one unknown name fails a
/// whole `x:Account/set`, so never add a name without a live check.
pub const API_KEY_PERMISSIONS: &[&str] = &["sysApiKeyCreate"];

/// Auth-audit event: K2 removed `sysApiKeyCreate` from a mailbox.
pub const AUDIT_EVENT: &str = "mail-apikey-locked";

const P: &str = "[mail/apikey-lockdown]";

// ── Permissions ─────────────────────────────────────────────────────────

/// The `permissions` that add the API key lock to `existing`, keeping
/// every other entry. `Ok(None)` = already locked.
pub fn lock_permissions(existing: Option<&Value>) -> Result<Option<Value>, String> {
    merge_permissions(existing, API_KEY_PERMISSIONS, &[])
}

/// Does `perms` already deny `sysApiKeyCreate`?
pub fn is_locked(perms: Option<&Value>) -> bool {
    matches!(lock_permissions(perms), Ok(None))
}

/// The `permissions` a NEW mailbox is created with: `base` (DAV policy +
/// Sieve lock, `sieve_lockdown::mint_permissions`) plus the API key lock.
pub fn mint_permissions(base: Value) -> Result<Value, String> {
    Ok(lock_permissions(Some(&base))?.unwrap_or(base))
}

// ── Engine seam ─────────────────────────────────────────────────────────

/// What the reconcile and the audit need from Stalwart. Production = the
/// JMAP client (Admin-role service key, delegated `accountId`); tests use
/// a recording fake.
pub trait ApiKeyLockApi {
    /// `x:Account/get` rows (`id`, `@type`, `name`, `roles`, `permissions`).
    fn key_account_rows(&self, ids: &[String]) -> Result<Vec<Value>, String>;
    fn key_set_permissions(&self, id: &str, permissions: &Value) -> Result<(), String>;
    /// API key ids on one account (`x:ApiKey/query`).
    fn api_key_ids(&self, account_id: &str) -> Result<Vec<String>, String>;
}

impl ApiKeyLockApi for StalwartClient {
    fn key_account_rows(&self, ids: &[String]) -> Result<Vec<Value>, String> {
        self.account_get_permissions(ids)
    }
    fn key_set_permissions(&self, id: &str, permissions: &Value) -> Result<(), String> {
        self.account_set_permissions(id, permissions)
    }
    fn api_key_ids(&self, account_id: &str) -> Result<Vec<String>, String> {
        self.api_key_query_ids(account_id)
    }
}

// ── Reconcile (boot / enable / upgrade) ─────────────────────────────────

/// One mailbox the reconcile locked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Locked {
    pub account_id: String,
    pub address: Option<String>,
    /// API keys the mailbox held when it was locked. `Err` = the list
    /// call failed (reported, never treated as zero).
    pub api_keys: Result<usize, String>,
}

impl Locked {
    /// The auth-audit `detail` for this change.
    pub fn audit_detail(&self, at: &str) -> Value {
        let (keys, err) = match &self.api_keys {
            Ok(n) => (serde_json::json!(n), Value::Null),
            Err(e) => (Value::Null, serde_json::json!(e)),
        };
        serde_json::json!({
            "accountId": self.account_id,
            "address": self.address,
            "permission": "sysApiKeyCreate",
            "action": "disabled",
            "apiKeys": keys,
            "apiKeysError": err,
            "at": at,
        })
    }
}

#[derive(Debug, Default)]
pub struct ReconcileReport {
    pub accounts: usize,
    pub locked: Vec<Locked>,
    pub already_locked: usize,
    pub lock_failed: Vec<(String, String)>,
    pub skipped: Vec<(String, String)>,
}

impl ReconcileReport {
    pub fn lines(&self) -> Vec<String> {
        let with_keys = self.locked.iter().filter(|l| matches!(l.api_keys, Ok(n) if n > 0)).count();
        let mut out = vec![format!(
            "{P} {} mailbox(es): {} locked now ({with_keys} holding API keys), {} already locked, \
             {} lock failed, {} skipped",
            self.accounts,
            self.locked.len(),
            self.already_locked,
            self.lock_failed.len(),
            self.skipped.len()
        )];
        for (id, e) in &self.lock_failed {
            out.push(format!("{P} account {id} lock FAILED (retried at the next start): {e}"));
        }
        for l in &self.locked {
            if let Err(e) = &l.api_keys {
                out.push(format!("{P} account {} locked; listing its API keys FAILED: {e}", l.account_id));
            }
        }
        out
    }
}

/// Remove `sysApiKeyCreate` from every K2-minted mailbox in `accounts`
/// (`(account id, address)`). `record` is called once per mailbox it
/// changed, after the change. Never stops at the first failure; a
/// mailbox already locked is not written.
pub fn reconcile(
    api: &dyn ApiKeyLockApi,
    accounts: &[(String, Option<String>)],
    record: &mut dyn FnMut(&Locked),
) -> ReconcileReport {
    let mut report = ReconcileReport { accounts: accounts.len(), ..ReconcileReport::default() };
    if accounts.is_empty() {
        return report;
    }
    let ids: Vec<String> = accounts.iter().map(|(id, _)| id.clone()).collect();
    let rows = match api.key_account_rows(&ids) {
        Ok(r) => r,
        Err(e) => {
            report.lock_failed = ids.into_iter().map(|id| (id, e.clone())).collect();
            return report;
        }
    };
    let by_id: HashMap<&str, &Value> = rows
        .iter()
        .filter_map(|r| r.get("id").and_then(|v| v.as_str()).map(|id| (id, r)))
        .collect();
    for (id, address) in accounts {
        let Some(row) = by_id.get(id.as_str()) else {
            report.skipped.push((id.clone(), "not on the mail server".to_string()));
            continue;
        };
        if let Some(why) = protected_reason(row) {
            report.skipped.push((id.clone(), why));
            continue;
        }
        let next = match lock_permissions(row.get("permissions")) {
            Ok(None) => {
                report.already_locked += 1;
                continue;
            }
            Ok(Some(next)) => next,
            Err(e) => {
                report.lock_failed.push((id.clone(), e));
                continue;
            }
        };
        if let Err(e) = api.key_set_permissions(id, &next) {
            report.lock_failed.push((id.clone(), e));
            continue;
        }
        let locked = Locked {
            account_id: id.clone(),
            address: address.clone(),
            api_keys: api.api_key_ids(id).map(|k| k.len()),
        };
        record(&locked);
        report.locked.push(locked);
    }
    report
}

/// The owner notice for a pass that locked mailboxes holding API keys.
/// `None` = nothing to tell (a pass that locked nothing, or locked only
/// mailboxes without keys). Runs once in practice: a locked mailbox is
/// never locked again.
pub fn notice_text(report: &ReconcileReport) -> Option<(String, String)> {
    let holders: Vec<&Locked> =
        report.locked.iter().filter(|l| matches!(l.api_keys, Ok(n) if n > 0)).collect();
    if holders.is_empty() {
        return None;
    }
    let title = format!("Hosted mail: {} mailbox(es) hold API keys their users created", holders.len());
    let mut body = String::from(
        "Mailbox users can no longer create mail server API keys for their own mailbox. \
         K2 never creates keys on a mailbox, so the keys below were created by someone \
         with that mailbox's password. A key keeps working until it is revoked.\n\n\
         To revoke them, set a new password: `k2 hostmail password rotate <address>` \
         revokes every API key on that mailbox.\n\n",
    );
    for l in holders {
        let who = l.address.clone().unwrap_or_else(|| format!("account {}", l.account_id));
        let n = l.api_keys.as_ref().copied().unwrap_or(0);
        body.push_str(&format!("- {who}: {n} API key(s)\n"));
    }
    body.push_str(&format!(
        "\nEach change is recorded in the auth audit log (~/.k2/auth-audit.jsonl, event \
         {AUDIT_EVENT}). `k2 hostmail audit-sieve` lists the mailboxes that still hold keys."
    ));
    Some((title, body))
}

/// The live pass, called from `sieve_lockdown::run_live` with the engine
/// lock held. Logs, records each change, files at most one notice.
pub fn run_with(client: &StalwartClient, accounts: &[(String, Option<String>)], reason: &str) {
    let mut record = |l: &Locked| record_live(l);
    let report = reconcile(client, accounts, &mut record);
    for line in report.lines() {
        k2_core::log_debug!("{line} ({reason})");
    }
    if let Some((title, body)) = notice_text(&report) {
        if let Err(e) = crate::feedback_routes::file_owner_notice(
            k2_core::db::AUDIT_SENTINEL_IDS[0],
            "k2-mail",
            &title,
            &body,
        ) {
            k2_core::log_debug!("{P} owner notice FAILED: {e}");
        }
    }
}

/// One audit line per mailbox changed: the auth-audit log, and the
/// daemon log always (so a full disk never hides it).
fn record_live(l: &Locked) {
    let at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let detail = l.audit_detail(&at);
    k2_core::log_debug!("{P} {AUDIT_EVENT}: {detail}");
    let who = l.address.clone().unwrap_or_else(|| l.account_id.clone());
    let ev = k2_core::auth_audit::AuditEvent::new(AUDIT_EVENT, &who, "locked", "daemon", "-", "daemon");
    if let Err(e) = k2_core::auth_audit::try_record_detail(&ev, detail) {
        k2_core::log_debug!("{P} WARN auth-audit write failed (the line above is the record): {e}");
    }
}

// ── Rotation revoke ─────────────────────────────────────────────────────

/// What a password rotation did to the mailbox's API keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Revoke {
    /// Keys that existed before the rotation (or were found after it)
    /// and are gone now.
    pub revoked: usize,
    /// Set when a list/destroy call failed: keys may still work.
    pub error: Option<String>,
}

/// After the new password is set: destroy every API key still on the
/// mailbox. `before` = the keys listed just before the password change
/// (Stalwart 0.16 drops them with a whole-`credentials` write; this
/// counts them and still holds if it ever stops doing that). `list` /
/// `destroy` are the engine's `x:ApiKey/query` / `/set destroy`.
pub fn revoke_after_rotate(
    before: Result<Vec<String>, String>,
    list: &dyn Fn() -> Result<Vec<String>, String>,
    destroy: &dyn Fn(&[String]) -> Result<(), String>,
) -> Revoke {
    let mut errors: Vec<String> = Vec::new();
    let mut gone: Vec<String> = match before {
        Ok(ids) => ids,
        Err(e) => {
            errors.push(format!("listing API keys before the password change: {e}"));
            Vec::new()
        }
    };
    match list() {
        Ok(left) => match destroy(&left) {
            Ok(()) => {
                for id in left {
                    if !gone.contains(&id) {
                        gone.push(id);
                    }
                }
            }
            Err(e) => {
                gone.retain(|id| !left.contains(id));
                errors.push(format!("destroying {} API key(s): {e}", left.len()));
            }
        },
        Err(e) => {
            // Can't tell what is left: count nothing as revoked.
            gone.clear();
            errors.push(format!("listing API keys after the password change: {e}"));
        }
    }
    Revoke {
        revoked: gone.len(),
        error: if errors.is_empty() { None } else { Some(errors.join("; ")) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn mint_permissions_add_the_key_lock_on_top_of_dav_and_sieve() {
        let base = crate::mail::sieve_lockdown::mint_permissions(Some(serde_json::json!({
            "@type": "Merge",
            "enabledPermissions": {},
            "disabledPermissions": { "davFileGet": true },
        })))
        .expect("sieve");
        let p = mint_permissions(base).expect("lock");
        assert_eq!(p["@type"], "Merge");
        assert_eq!(p["disabledPermissions"]["sysApiKeyCreate"], true, "{p}");
        assert_eq!(p["disabledPermissions"]["davFileGet"], true, "DAV kept: {p}");
        assert!(crate::mail::sieve_lockdown::is_locked(Some(&p)), "Sieve kept: {p}");
        assert!(is_locked(Some(&p)));
        // Idempotent.
        assert_eq!(mint_permissions(p.clone()).expect("same"), p);
        assert!(!is_locked(None));
        assert!(!is_locked(Some(&serde_json::json!({ "@type": "Inherit" }))));
        // An operator's Replace list: the name leaves `enabled`.
        let rep = serde_json::json!({
            "@type": "Replace",
            "enabledPermissions": { "sysApiKeyCreate": true, "emailSend": true },
            "disabledPermissions": {},
        });
        let r = lock_permissions(Some(&rep)).expect("merge").expect("changed");
        assert_eq!(r["@type"], "Replace");
        assert!(r["enabledPermissions"].get("sysApiKeyCreate").is_none(), "{r}");
        assert_eq!(r["enabledPermissions"]["emailSend"], true);
    }

    #[test]
    fn lock_survives_a_dav_policy_flip() {
        let locked = mint_permissions(crate::mail::sieve_lockdown::mint_permissions(None).unwrap()).unwrap();
        for (cal, files) in [
            (crate::mail::dav::OnOff::On, crate::mail::dav::OnOff::On),
            (crate::mail::dav::OnOff::Off, crate::mail::dav::OnOff::Off),
        ] {
            let p = crate::mail::dav::DavPolicy { calendars: cal, files, applied_at: 1, backfilled_at: None };
            let after = merge_permissions(Some(&locked), &p.denied(), &p.allowed())
                .expect("merge")
                .unwrap_or(locked.clone());
            assert!(is_locked(Some(&after)), "{after}");
        }
    }

    // ── reconcile against a recording fake ──

    #[derive(Default)]
    struct Fake {
        rows: RefCell<Vec<Value>>,
        keys: HashMap<String, Vec<String>>,
        log: RefCell<Vec<String>>,
        fail_set: Option<String>,
        fail_list: Option<String>,
    }

    impl ApiKeyLockApi for Fake {
        fn key_account_rows(&self, ids: &[String]) -> Result<Vec<Value>, String> {
            Ok(self.rows.borrow().iter().filter(|r| ids.iter().any(|i| r["id"] == i.as_str())).cloned().collect())
        }
        fn key_set_permissions(&self, id: &str, p: &Value) -> Result<(), String> {
            self.log.borrow_mut().push(format!("perm:{id}"));
            if self.fail_set.as_deref() == Some(id) {
                return Err("x:Account/set update rejected — forbidden".into());
            }
            for r in self.rows.borrow_mut().iter_mut() {
                if r["id"] == id {
                    r["permissions"] = p.clone();
                }
            }
            Ok(())
        }
        fn api_key_ids(&self, a: &str) -> Result<Vec<String>, String> {
            self.log.borrow_mut().push(format!("keys:{a}"));
            if self.fail_list.as_deref() == Some(a) {
                return Err("x:ApiKey/query: JMAP error 'serverFail'".into());
            }
            Ok(self.keys.get(a).cloned().unwrap_or_default())
        }
    }

    fn user_row(id: &str, perms: Value) -> Value {
        serde_json::json!({ "id": id, "@type": "User", "name": id, "permissions": perms })
    }

    fn accounts(ids: &[&str]) -> Vec<(String, Option<String>)> {
        ids.iter().map(|i| (i.to_string(), Some(format!("{i}@example.test")))).collect()
    }

    #[test]
    fn reconcile_locks_existing_mailboxes_once_and_records_each_change() {
        let sieve_only = crate::mail::sieve_lockdown::mint_permissions(None).unwrap();
        let fake = Fake {
            rows: RefCell::new(vec![
                user_row("a", Value::Null),
                user_row("b", sieve_only.clone()),
                user_row("c", mint_permissions(sieve_only.clone()).unwrap()),
                serde_json::json!({ "id": "svc", "@type": "User", "name": "k2-daemon",
                    "roles": { "@type": "Admin" } }),
            ]),
            keys: [("a".to_string(), vec!["k1".to_string(), "k2".to_string()])].into_iter().collect(),
            ..Fake::default()
        };
        let mut recorded: Vec<Locked> = Vec::new();
        let report = {
            let mut rec = |l: &Locked| recorded.push(l.clone());
            reconcile(&fake, &accounts(&["a", "b", "c", "svc", "gone"]), &mut rec)
        };
        assert_eq!(report.locked.iter().map(|l| l.account_id.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(report.already_locked, 1, "c was locked at mint");
        assert_eq!(report.skipped.len(), 2, "svc (admin) and gone (not on the server): {:?}", report.skipped);
        assert!(report.lock_failed.is_empty());
        assert_eq!(recorded, report.locked, "one audit line per mailbox changed");
        assert_eq!(recorded[0].api_keys, Ok(2));
        assert_eq!(recorded[1].api_keys, Ok(0));
        // The Sieve lock on b is kept; the service account is never written.
        let rows = fake.rows.borrow();
        let b = rows.iter().find(|r| r["id"] == "b").unwrap();
        assert!(is_locked(Some(&b["permissions"])));
        assert!(crate::mail::sieve_lockdown::is_locked(Some(&b["permissions"])), "{b}");
        drop(rows);
        assert_eq!(*fake.log.borrow(), ["perm:a", "keys:a", "perm:b", "keys:b"]);
        let d = recorded[0].audit_detail("t");
        assert_eq!(d["apiKeys"], 2);
        assert_eq!(d["address"], "a@example.test");
        assert_eq!(d["permission"], "sysApiKeyCreate");

        // Idempotent: a second pass writes nothing and records nothing.
        fake.log.borrow_mut().clear();
        let mut n = 0;
        let again = {
            let mut rec = |_: &Locked| n += 1;
            reconcile(&fake, &accounts(&["a", "b", "c"]), &mut rec)
        };
        assert_eq!(n, 0);
        assert!(again.locked.is_empty());
        assert_eq!(again.already_locked, 3);
        assert!(fake.log.borrow().is_empty(), "no write, no list: {:?}", fake.log.borrow());
        assert!(notice_text(&again).is_none());
    }

    #[test]
    fn reconcile_reports_failures_and_keeps_going() {
        let fake = Fake {
            rows: RefCell::new(vec![user_row("a", Value::Null), user_row("b", Value::Null)]),
            fail_set: Some("a".into()),
            fail_list: Some("b".into()),
            ..Fake::default()
        };
        let mut recorded = Vec::new();
        let report = {
            let mut rec = |l: &Locked| recorded.push(l.clone());
            reconcile(&fake, &accounts(&["a", "b"]), &mut rec)
        };
        assert_eq!(report.lock_failed.len(), 1);
        assert_eq!(report.lock_failed[0].0, "a");
        assert_eq!(recorded.len(), 1, "only b changed");
        assert!(recorded[0].api_keys.is_err(), "a failed list is reported, never zero");
        let lines = report.lines().join("\n");
        assert!(lines.contains("lock FAILED") && lines.contains("listing its API keys FAILED"), "{lines}");
    }

    #[test]
    fn notice_names_only_mailboxes_holding_keys() {
        let report = ReconcileReport {
            accounts: 3,
            locked: vec![
                Locked { account_id: "a".into(), address: Some("a@example.test".into()), api_keys: Ok(2) },
                Locked { account_id: "b".into(), address: Some("b@example.test".into()), api_keys: Ok(0) },
            ],
            ..ReconcileReport::default()
        };
        let (title, body) = notice_text(&report).expect("notice");
        assert!(title.contains("1 mailbox(es)"), "{title}");
        assert!(body.contains("a@example.test: 2 API key(s)"), "{body}");
        assert!(!body.contains("b@example.test"), "{body}");
        assert!(body.contains("k2 hostmail password rotate"), "{body}");
        let none = ReconcileReport { locked: vec![report.locked[1].clone()], ..ReconcileReport::default() };
        assert!(notice_text(&none).is_none());
    }

    // ── rotation revoke ──

    #[test]
    fn revoke_destroys_what_is_left_and_counts_what_was_there() {
        let calls = RefCell::new(Vec::<String>::new());
        // Stalwart 0.16: the whole-credentials write already dropped them.
        let r = revoke_after_rotate(
            Ok(vec!["k1".into(), "k2".into()]),
            &|| Ok(vec![]),
            &|ids| {
                calls.borrow_mut().push(format!("destroy:{}", ids.join(",")));
                Ok(())
            },
        );
        assert_eq!(r, Revoke { revoked: 2, error: None });
        // A server that keeps keys across a password write: destroyed here.
        let r = revoke_after_rotate(
            Ok(vec!["k1".into()]),
            &|| Ok(vec!["k1".into(), "k3".into()]),
            &|ids| {
                calls.borrow_mut().push(format!("destroy:{}", ids.join(",")));
                Ok(())
            },
        );
        assert_eq!(r, Revoke { revoked: 2, error: None });
        assert_eq!(*calls.borrow(), ["destroy:", "destroy:k1,k3"]);
        // No keys at all.
        assert_eq!(revoke_after_rotate(Ok(vec![]), &|| Ok(vec![]), &|_| Ok(())), Revoke { revoked: 0, error: None });
    }

    #[test]
    fn revoke_failures_are_loud_and_never_overcount() {
        let r = revoke_after_rotate(Ok(vec!["k1".into()]), &|| Ok(vec!["k1".into()]), &|_| Err("forbidden".into()));
        assert_eq!(r.revoked, 0, "k1 is still there");
        assert!(r.error.as_deref().unwrap().contains("destroying 1 API key(s): forbidden"), "{r:?}");
        let r = revoke_after_rotate(Ok(vec!["k1".into()]), &|| Err("serverFail".into()), &|_| Ok(()));
        assert_eq!(r.revoked, 0, "unknown state counts nothing");
        assert!(r.error.as_deref().unwrap().contains("after the password change"), "{r:?}");
        let r = revoke_after_rotate(Err("serverFail".into()), &|| Ok(vec!["k9".into()]), &|_| Ok(()));
        assert_eq!(r.revoked, 1, "the ones destroyed after still count");
        assert!(r.error.as_deref().unwrap().contains("before the password change"), "{r:?}");
    }
}
