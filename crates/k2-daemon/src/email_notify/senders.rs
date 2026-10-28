//! Senders: add, remove, the system sender, and whether one may send now
//! (prd-email-notifications-core-v1 §3.2, E1–E3, E10, E20, E26).
//!
//! Only the owner (or an Admin) reaches these, through
//! `/cli/notifications/email/*` with a fresh confirm where it widens; the
//! route layer checks who. This module owns the rules:
//!
//! - **One owner per address** (E20): an address is a sender at most once,
//!   never also a mailbox (`mail_addresses`, any status) or any Stalwart
//!   principal, alias, list or catch-all (E2).
//! - **Hosted** (Linux with hosted mail): the address's domain is a
//!   Verified hosted domain; K2 creates the account with no credential and
//!   logins off (E3). It is `ready` once DMARC is found (E10), else
//!   `needs_dmarc` (K2 plants `p=none` where it hosts the zone).
//! - **Relay** (any box): the address is on the relay's domain; it is
//!   `needs_test` until a test through the relay is accepted.
//! - **Remove** rename-retires the account and keeps the address reserved
//!   for 30 days (E26). An App that used it turns off.

use serde_json::{json, Value as J};

use k2_core::email_notify::{self as store, Sender};

use super::{db_fail, fail, sender_json, Fail, R};

/// What `sender add` takes.
#[derive(Debug, Clone, Default)]
pub struct AddArgs {
    pub address: String,
    pub name: Option<String>,
    pub name_locked: bool,
    pub reply_to: Vec<String>,
    pub relay: Option<String>,
    /// N7: send from a bare domain whose mail lives elsewhere, through this
    /// hosted bounce subdomain of it (e.g. `bounce.brand.com`).
    pub via: Option<String>,
}

fn usage(h: impl Into<String>) -> Fail {
    fail(400, "usage", h)
}

/// Normalize an address (`local@domain`, punycode domain, no `+tag`).
pub fn normalize(raw: &str) -> R<(String, String, String)> {
    let a = crate::mail::addresses::normalize_address(raw).map_err(|_| usage(format!("{} isn't a valid address", raw.trim())))?;
    let (local, domain) = a.split_once('@').ok_or_else(|| usage("a sender is a full address"))?;
    let local = crate::mail::addresses::validate_local_part(local).map_err(usage)?;
    if local.contains('+') {
        return Err(usage("a sender's address has no +tag"));
    }
    Ok((format!("{local}@{domain}"), local, domain.to_string()))
}

fn reply_to_list(raw: &[String]) -> R<Vec<String>> {
    let list = crate::mail::send::normalize_recipients(raw).map_err(|_| usage("replyTo has an address that isn't valid"))?;
    if list.len() > super::compose::MAX_REPLY_TO {
        return Err(usage("at most 3 reply-to addresses"));
    }
    Ok(list)
}

fn validate_name(raw: &str) -> R<String> {
    crate::mail::send::validate_from_name(raw).map_err(|e| match e {
        crate::mail::send::SendError::Usage(h) => usage(h),
        _ => usage("the name isn't valid"),
    })
}

/// E2: is the address free to become a sender (K2's side)?
fn k2_side_free(conn: &rusqlite::Connection, address: &str) -> R<()> {
    if let Some(s) = store::get_sender_by_address(conn, address).map_err(db_fail)? {
        return Err(if s.state == store::STATE_RETIRED {
            fail(409, "reserved", format!("{address} was a sender until recently; its address stays reserved for 30 days after removal"))
        } else {
            fail(409, "exists", format!("{address} is already a sender ({})", s.id))
        });
    }
    // N7: a send-only apex sender's bounce account is taken too.
    if store::is_reserved(conn, address).map_err(db_fail)? {
        return Err(fail(409, "exists", format!("{address} is already a sender's mail-server account (k2 notifications email sender list)")));
    }
    if crate::mail::addresses::load_address(conn, address).is_some() {
        return Err(fail(
            409,
            "exists",
            format!("{address} is a mailbox (k2 hostmail list): a sender is never a mailbox. Pick another address (e.g. noreply@…)"),
        ));
    }
    Ok(())
}

/// The owner's own address when it is on `domain` (or its organization):
/// the DMARC `rua` K2 may plant without an external authorization.
fn rua_for(conn: &rusqlite::Connection, domain: &str) -> Option<String> {
    let owner = store::settings(conn).ok()?.owner_email?;
    let od = owner.split_once('@')?.1.to_ascii_lowercase();
    (od == domain || od == super::dmarc::org_domain(domain)).then_some(owner)
}

/// `k2 notifications email sender add`. The caller is the owner (or an
/// Admin) with a fresh confirm.
pub fn add(a: &AddArgs, actor: &str, now: i64) -> R<J> {
    let (address, local, domain) = normalize(&a.address)?;
    let name = match a.name.as_deref() {
        Some(n) => validate_name(n)?,
        None => domain.clone(),
    };
    let reply_to = reply_to_list(&a.reply_to)?;
    if let Some(via) = a.via.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        if a.relay.as_deref().is_some_and(|r| !r.trim().is_empty()) {
            return Err(usage("--via is for hosted mail; a relay sender sends from its own domain"));
        }
        return add_send_only(&address, &local, &domain, via, &name, a.name_locked, &reply_to, actor, now);
    }
    let relay = match a.relay.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(id) => {
            let db = k2_core::db::shared();
            let conn = db.lock();
            Some(store::get_relay(&conn, id).map_err(db_fail)?.ok_or_else(|| fail(404, "not_found", format!("no relay {id} (k2 notifications email relay list)")))?)
        }
        None => None,
    };
    if let Some(r) = &relay {
        if domain != r.from_domain && !domain.ends_with(&format!(".{}", r.from_domain)) {
            return Err(usage(format!("relay {} sends for {}; {address} isn't on that domain", r.id, r.from_domain)));
        }
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_side_free(&conn, &address)?;
        let s = store::insert_sender(
            &conn,
            &store::NewSender {
                address: &address,
                domain: &domain,
                path: store::PATH_RELAY,
                stalwart_account_id: None,
                relay_id: Some(&r.id),
                from_name: &name,
                from_name_locked: a.name_locked,
                reply_to: &reply_to,
                state: store::STATE_NEEDS_TEST,
                now,
                envelope_address: None,
            },
        )
        .map_err(|e| if e.contains("UNIQUE") { fail(409, "exists", format!("{address} is already a sender")) } else { db_fail(e) })?;
        drop(conn);
        super::audit_line("email-sender-add", actor, &format!("sender={} path=relay relay={}", s.id, r.id));
        let mut v = sender_json(&s);
        v["next"] = json!(format!("k2 notifications email relay test {} --from {address} (sends one test to your own address)", r.id));
        return Ok(json!({ "ok": true, "sender": v }));
    }
    // Hosted.
    if !super::hosted_supported() {
        return Err(fail(
            409,
            "no_mail_path",
            "this server has no hosted mail (a Mac, or Linux without it): add an SMTP relay first \
             (k2 notifications email relay add) and pass --relay <id>",
        ));
    }
    let (row, rua) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_side_free(&conn, &address)?;
        let row = crate::mail::domains::load_domain(&conn, &domain).ok_or_else(|| {
            fail(
                409,
                "domain_not_ready",
                format!("{domain} isn't a hosted domain here: k2 hostmail domain add {domain}, publish its records, then k2 hostmail domain check {domain}"),
            )
        })?;
        (row, rua_for(&conn, &domain))
    };
    if row.status != "verified" {
        return Err(fail(409, "domain_not_ready", format!("{domain} isn't Verified yet (k2 hostmail domain check {domain})")));
    }
    let Some(stw_domain) = row.stalwart_domain_id.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
        return Err(fail(409, "domain_not_ready", format!("{domain} has no mail-server id; remove and re-add it")));
    };
    let engine = super::engine::mail_engine()?;
    if engine.address_taken(&address, stw_domain).map_err(|e| fail(502, "failed", format!("the mail server didn't answer: {e}")))? {
        return Err(fail(409, "exists", format!("{address} is already an account, alias, list or catch-all on the mail server")));
    }
    let account = engine.create_sender_account(&local, stw_domain).map_err(|e| fail(502, "failed", format!("the mail server refused the sender account: {e}")))?;
    let inserted = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::insert_sender(
            &conn,
            &store::NewSender {
                address: &address,
                domain: &domain,
                path: store::PATH_HOSTED,
                stalwart_account_id: Some(&account),
                relay_id: None,
                from_name: &name,
                from_name_locked: a.name_locked,
                reply_to: &reply_to,
                state: store::STATE_NEEDS_DMARC,
                now,
                envelope_address: None,
            },
        )
    };
    let s = match inserted {
        Ok(s) => s,
        Err(e) => {
            // No row, no account: compensate (rename-retire).
            if let Err(r) = engine.retire_account(&account) {
                k2_core::log_debug!("[email-notify] WARN sender add rolled back but the account wasn't retired: {r}");
            }
            return Err(if e.contains("UNIQUE") { fail(409, "exists", format!("{address} is already a sender")) } else { db_fail(e) });
        }
    };
    super::audit_line("email-sender-add", actor, &format!("sender={} path=hosted", s.id));
    // E10: ready once DMARC is found; plant p=none where K2 hosts the zone.
    let mut planted = false;
    let state = match super::dmarc::check(&domain, now) {
        Ok(_) => store::STATE_READY,
        Err(f) if f.code == "dmarc_missing" => {
            planted = super::dmarc::plant(&domain, rua.as_deref()).unwrap_or_else(|e| {
                k2_core::log_debug!("[email-notify] DMARC plant for sender {}: {e}", s.id);
                false
            });
            store::STATE_NEEDS_DMARC
        }
        Err(_) => store::STATE_NEEDS_DMARC,
    };
    let s = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::set_sender_state(&conn, &s.id, state, now).map_err(db_fail)?;
        store::get_sender(&conn, &s.id).map_err(db_fail)?.unwrap_or(s)
    };
    let mut v = sender_json(&s);
    if state == store::STATE_NEEDS_DMARC {
        v["dmarcPlanted"] = json!(planted);
        v["next"] = json!(if planted {
            format!("K2 published _dmarc.{domain} \"v=DMARC1; p=none\"; once it is visible: k2 notifications email sender check {address}")
        } else {
            format!("publish a DMARC record (any policy), e.g. _dmarc.{domain} TXT \"v=DMARC1; p=none\", then k2 notifications email sender check {address}")
        });
    }
    Ok(json!({ "ok": true, "sender": v }))
}

/// N7 (prd-email-notifications-part2-v1 §4 N7, vs-live EN-A8–EN-A11):
/// `noreply@brand.com` while `brand.com`'s mail stays elsewhere. The apex
/// is never added to the mail server (probe P4b: then the box refuses
/// outsiders relaying to it, and its own mailboxes reach the apex by MX).
/// The account, its Identity, the envelope MAIL FROM and the DKIM
/// signature are `<local>@<via>`, a Verified hosted subdomain of the apex;
/// K2 stamps the header From. DMARC passes on relaxed alignment only, so
/// an apex with both `adkim=s` and `aspf=s` is refused (Rosson
/// 2026-10-28). K2 never writes the apex's DMARC, MX or SPF.
#[allow(clippy::too_many_arguments)]
fn add_send_only(address: &str, local: &str, apex: &str, via: &str, name: &str, name_locked: bool, reply_to: &[String], actor: &str, now: i64) -> R<J> {
    let via = via.trim().trim_end_matches('.').to_ascii_lowercase();
    if !via.ends_with(&format!(".{apex}")) {
        return Err(usage(format!("--via is a subdomain of {apex} (e.g. bounce.{apex}), so DMARC aligns")));
    }
    if !super::hosted_supported() {
        return Err(fail(409, "no_mail_path", "sending from a bare domain needs hosted mail on this server"));
    }
    let envelope = format!("{local}@{via}");
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if crate::mail::domains::load_domain(&conn, apex).is_some() {
            return Err(fail(
                409,
                "apex_hosted",
                format!("{apex} is hosted here, so its senders don't need --via: k2 notifications email sender add {address}"),
            ));
        }
        k2_side_free(&conn, address)?;
        k2_side_free(&conn, &envelope)?;
        crate::mail::domains::load_domain(&conn, &via).ok_or_else(|| {
            fail(
                409,
                "domain_not_ready",
                format!("{via} isn't a hosted domain here: k2 hostmail domain add {via}, publish its records (MX, SPF, DKIM), then k2 hostmail domain check {via}"),
            )
        })?
    };
    if row.status != "verified" {
        return Err(fail(409, "domain_not_ready", format!("{via} isn't Verified yet (k2 hostmail domain check {via})")));
    }
    let Some(stw_domain) = row.stalwart_domain_id.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
        return Err(fail(409, "domain_not_ready", format!("{via} has no mail-server id; remove and re-add it")));
    };
    // Rosson's rule 4: a strict-on-both apex can't align: refuse before
    // anything is created. No record yet is allowed (state needs_dmarc).
    super::dmarc::forget(apex);
    if let Ok(found) = super::dmarc::check(apex, now) {
        if found.strict {
            return Err(dmarc_strict(apex, &found.at));
        }
    }
    let engine = super::engine::mail_engine()?;
    if engine.address_taken(&envelope, stw_domain).map_err(|e| fail(502, "failed", format!("the mail server didn't answer: {e}")))? {
        return Err(fail(409, "exists", format!("{envelope} is already an account, alias, list or catch-all on the mail server")));
    }
    let account = engine.create_sender_account(local, stw_domain).map_err(|e| fail(502, "failed", format!("the mail server refused the sender account: {e}")))?;
    let inserted = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::insert_sender(
            &conn,
            &store::NewSender {
                address,
                domain: apex,
                path: store::PATH_HOSTED,
                stalwart_account_id: Some(&account),
                relay_id: None,
                from_name: name,
                from_name_locked: name_locked,
                reply_to,
                state: store::STATE_NEEDS_DMARC,
                now,
                envelope_address: Some(&envelope),
            },
        )
    };
    let s = match inserted {
        Ok(s) => s,
        Err(e) => {
            if let Err(r) = engine.retire_account(&account) {
                k2_core::log_debug!("[email-notify] WARN send-only sender add rolled back but the account wasn't retired: {r}");
            }
            return Err(if e.contains("UNIQUE") { fail(409, "exists", format!("{address} is already a sender")) } else { db_fail(e) });
        }
    };
    super::audit_line("email-sender-add", actor, &format!("sender={} path=hosted send_only=1", s.id));
    let state = if super::dmarc::check(apex, now).is_ok() { store::STATE_READY } else { store::STATE_NEEDS_DMARC };
    let s = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::set_sender_state(&conn, &s.id, state, now).map_err(db_fail)?;
        store::get_sender(&conn, &s.id).map_err(db_fail)?.unwrap_or(s)
    };
    let mut v = sender_json(&s);
    if state == store::STATE_NEEDS_DMARC {
        v["next"] = json!(format!(
            "publish a DMARC record at {apex}'s DNS (K2 never writes it), e.g. _dmarc.{apex} TXT \"v=DMARC1; p=none\" (adkim and aspf not both s), \
             then k2 notifications email sender check {address}"
        ));
    }
    Ok(json!({ "ok": true, "sender": v }))
}

fn dmarc_strict(apex: &str, at: &str) -> Fail {
    fail(
        409,
        "dmarc_strict",
        format!(
            "{at} sets adkim=s and aspf=s, so mail signed and sent through a subdomain can never pass DMARC for {apex}. \
             Relax one of them (adkim=r or aspf=r), or send from a subdomain of {apex} hosted here instead"
        ),
    )
}

/// Find a live sender by id or address.
pub fn find_live(id_or_address: &str) -> R<Sender> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    match store::find_sender(&conn, id_or_address).map_err(db_fail)? {
        Some(s) if s.is_live() => Ok(s),
        _ => Err(fail(404, "not_found", format!("no sender {} (k2 notifications email sender list)", id_or_address.trim()))),
    }
}

/// `sender check`: re-evaluate DMARC (hosted) and say what's missing.
pub fn recheck(id_or_address: &str, now: i64) -> R<J> {
    let s = find_live(id_or_address)?;
    super::dmarc::forget(&s.domain);
    let readiness = ready(&s, now);
    let s = find_live(&s.id)?;
    let mut v = sender_json(&s);
    if let Err(f) = &readiness {
        v["notReady"] = json!({ "code": f.code, "hint": f.hint });
    }
    Ok(json!({ "ok": true, "sender": v }))
}

/// `sender rm`: rename-retire the account (hosted) and the row (E26).
pub fn remove(id_or_address: &str, actor: &str, now: i64) -> R<J> {
    let s = find_live(id_or_address)?;
    if let Some(acct) = s.stalwart_account_id.as_deref().filter(|_| s.path == store::PATH_HOSTED) {
        let engine = super::engine::mail_engine()?;
        engine.retire_account(acct).map_err(|e| fail(502, "failed", format!("the mail server didn't retire the sender account ({e}); nothing changed, try again")))?;
    }
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::retire_sender(&conn, &s.id, now).map_err(db_fail)?;
    }
    super::audit_line("email-sender-rm", actor, &format!("sender={} was={}", s.id, s.owner_kind.as_deref().unwrap_or("unassigned")));
    if let Some(app) = s.app() {
        if let Some(t) = crate::app_push::published(app) {
            crate::apps::alerts::note(&t.project_id, "App email off", &format!("The owner removed {}'s sender; its email is off.", t.name));
        }
    }
    Ok(json!({ "ok": true, "removed": s.id, "address": s.address, "reservedDays": store::RESERVE_DAYS }))
}

/// `system-sender set`: the box's one system sender (E20: never an App's).
pub fn set_system(id_or_address: &str, actor: &str, now: i64) -> R<J> {
    let s = find_live(id_or_address)?;
    if s.app().is_some() {
        return Err(fail(409, "sender_in_use", format!("{} is App {}'s sender; the system sender is its own address", s.address, s.app().unwrap_or(""))));
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    let tx = conn.unchecked_transaction().map_err(db_fail)?;
    if let Some(old) = store::system_sender(&tx).map_err(db_fail)? {
        if old.id != s.id {
            store::set_sender_owner(&tx, &old.id, None, None, now).map_err(db_fail)?;
        }
    }
    store::set_sender_owner(&tx, &s.id, Some(store::OWNER_SYSTEM), None, now).map_err(db_fail)?;
    tx.commit().map_err(db_fail)?;
    let s = store::get_sender(&conn, &s.id).map_err(db_fail)?.ok_or_else(|| db_fail("the sender vanished"))?;
    drop(conn);
    super::audit_line("email-system-sender-set", actor, &format!("sender={}", s.id));
    Ok(json!({ "ok": true, "systemSender": sender_json(&s) }))
}

/// `system-sender clear`.
pub fn clear_system(actor: &str, now: i64) -> R<J> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let Some(old) = store::system_sender(&conn).map_err(db_fail)? else { return Ok(json!({ "ok": true, "systemSender": J::Null })) };
    store::set_sender_owner(&conn, &old.id, None, None, now).map_err(db_fail)?;
    drop(conn);
    super::audit_line("email-system-sender-clear", actor, &format!("sender={}", old.id));
    Ok(json!({ "ok": true, "systemSender": J::Null, "was": old.address }))
}

/// May sender `s` send right now (§5.2 step 4)? Hosted: hosted mail here,
/// Stalwart up, the domain Verified, its send mode resolves, DMARC found.
/// Relay: the vault open, the password there, the last test passed. A
/// `needs_dmarc` sender whose record appeared becomes `ready`.
pub fn ready(s: &Sender, now: i64) -> R<()> {
    match s.state.as_str() {
        store::STATE_RETIRED => return Err(fail(409, "no_sender", "that sender was removed")),
        store::STATE_OFF => return Err(fail(409, "no_sender", "that sender is off")),
        _ => {}
    }
    if s.path == store::PATH_RELAY {
        if s.state == store::STATE_NEEDS_TEST {
            return Err(fail(409, "relay_not_ready", format!("{} hasn't passed a relay test yet (k2 notifications email relay test)", s.address)));
        }
        let relay = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            store::get_relay(&conn, s.relay_id.as_deref().unwrap_or("")).map_err(db_fail)?
        };
        let Some(relay) = relay else { return Err(fail(409, "relay_not_ready", "the sender's relay was removed")) };
        if relay.last_test_ok != Some(true) {
            return Err(fail(409, "relay_not_ready", format!("relay {}'s last test didn't pass (k2 notifications email relay test {})", relay.id, relay.id)));
        }
        match super::engine::relay_creds().get(&relay.id) {
            Ok(Some(_)) => {}
            Ok(None) => return Err(fail(409, "relay_not_ready", format!("relay {}'s login is missing; add the relay again", relay.id))),
            Err(e) => return Err(e.to_fail()),
        }
        return Ok(());
    }
    if !super::hosted_supported() {
        return Err(fail(409, "no_mail_path", "this server has no hosted mail; use a relay sender"));
    }
    if s.stalwart_account_id.as_deref().is_none_or(str::is_empty) {
        return Err(fail(409, "domain_not_ready", "the sender has no mail-server account; remove it and add it again"));
    }
    // The hosted domain is the account's: the bounce subdomain for a
    // send-only apex sender (N7), else the address's own.
    let hosted = s.envelope_domain().to_string();
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        crate::mail::domains::load_domain(&conn, &hosted)
    };
    let Some(row) = row else { return Err(fail(409, "domain_not_ready", format!("{hosted} isn't hosted here any more"))) };
    if row.status != "verified" {
        return Err(fail(409, "domain_not_ready", format!("{hosted} isn't Verified (k2 hostmail domain check {hosted})")));
    }
    #[cfg(not(test))]
    {
        // The domain's send mode (receive-only refuses; a relay route must
        // resolve). Tests have no mail secrets store. EN-A9: the envelope
        // domain's mode governs.
        if let Err(e) = crate::mail::send::check_domain_send_mode(&crate::mail::secrets::FileSecretStore::default(), s.envelope()) {
            let hint = match e {
                crate::mail::send::SendError::SendMode(h) | crate::mail::send::SendError::Engine(h) => h,
                _ => "the domain can't send".to_string(),
            };
            return Err(fail(409, "domain_not_ready", hint));
        }
    }
    #[cfg(test)]
    {
        if row.send_mode == "receive-only" {
            return Err(fail(409, "domain_not_ready", format!("domain '{hosted}' is receive-only")));
        }
    }
    // DMARC is the From domain's (the apex for a send-only sender).
    let found = super::dmarc::check(&s.domain, now)?;
    if s.is_send_only() && found.strict {
        return Err(dmarc_strict(&s.domain, &found.at));
    }
    if s.state == store::STATE_NEEDS_DMARC {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::set_sender_state(&conn, &s.id, store::STATE_READY, now).map_err(db_fail)?;
    }
    Ok(())
}

/// The system sender, if one is set.
pub fn system() -> R<Option<Sender>> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::system_sender(&conn).map_err(db_fail)
}

/// Every sender, retired ones included when asked.
pub fn list(include_retired: bool) -> R<Vec<Sender>> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::list_senders(&conn, include_retired).map_err(db_fail)
}
