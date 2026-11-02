//! Telling an outside forward destination, and stopping
//! (prd-hostmail-external-forwarding-v1 "Ruling, 2026-11-02"; slice V of
//! prd-email-notifications-part2-v1, purpose `forward`).
//!
//! **Opt-out, not opt-in** (Rosson 2026-11-02): a forward goes active at once.
//! An outside address gets one "you've been added" email ([`tell_destination`]),
//! sent BEFORE the mailbox's Sieve is written, so it arrives ahead of the
//! first forwarded message. A failed email doesn't block the forward: the row
//! says `failed`, `forward list` shows it, and K2 tries again hourly
//! ([`minute_pass`]). The store is `k2_core::email_verify`; the pages are
//! `email_notify::verify` (GET renders, only POST stops, so link scanners
//! can't). K2 does NOT put a stop link in the forwarded mail itself: a body
//! footer breaks the sender's DKIM, and a per-destination header would ride
//! into every later copy of the same message (one recipient's stop token in
//! the others' mail). The notice is the one place.
//!
//! **The mail.** K2's fixed text (`verify::compose_forward_notice`): which
//! server, who set it, which mailbox, and a stop link. From the box's system
//! sender; a box without one sends it **from the mailbox itself** (Rosson
//! 2026-10-28, vs-live Q8), by delegation, with no stored copy. Never App or
//! agent text.
//!
//! **The owner hears at once** ([`tell_owner_changed`], [`tell_owner`]: their
//! Inbox, plus email when the owner has a confirmed notice address and the
//! box has a system sender). With no confirm step this is the main protection
//! against someone quietly sending customer mail elsewhere. A bulk add isn't
//! 300 notes: the first [`store::OWNER_NOTES_PER_HOUR`] in an hour go one by
//! one, and the rest roll into one hourly summary with the count and how to
//! list them ([`minute_pass`]). A stop by an outside person is in that summary
//! at the latest. Every change has its Access Audit line, either way.
//!
//! **Stop** ([`on_stopped`]): pauses the pair's forward with reason `revoked`
//! (or removes a row from before opt-out still waiting), and tells the owner.
//! A stopped destination stays out: only the owner or an Admin can choose it
//! again (`--restart-stopped`).
//!
//! **Rows from before opt-out** ([`minute_pass`]): `pending_verify` rows set
//! within `LEGACY_ON_WINDOW_SECS` go active, [`store::NOTICES_PER_PASS`] a
//! minute, each with its notice; they don't count against the daily cap.
//! Older ones stay off and send nothing until someone sets them again. The
//! owner gets one notice listing both.
//!
//! **Pacing.** The owner can raise the daily cap, and a thousand notices at
//! once to strangers would hurt the box's mail reputation and spend the
//! system mail budget password resets share. So once
//! [`store::NOTICES_BURST_PER_HOUR`] went out in the last hour, a new outside
//! forward waits its turn in the same queue (`due`): it isn't on, and the
//! pass emails the address and then turns it on. The notice still comes first.

use std::collections::BTreeMap;

use k2_core::email_verify as vstore;
use k2_core::mail_forward as store;
use serde_json::json;

use crate::email_notify::{self, compose, fail, verify, R};

const P: &str = "[mail/forward-verify]";

/// How the notice email went out.
pub const VIA_SYSTEM: &str = "system";
pub const VIA_MAILBOX: &str = "mailbox";

/// The log purpose of the notice email.
pub const PURPOSE_NOTICE: &str = "forward_notice";

/// What telling an outside address came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Told {
    /// Its `email_verified_addresses` row (what its stop links stop).
    pub verified_id: String,
    /// The email went out this way (`None`: it didn't).
    pub via: Option<&'static str>,
    /// Why it didn't.
    pub error: Option<String>,
}

impl Told {
    pub fn notice_state(&self) -> &'static str {
        if self.via.is_some() {
            store::N_SENT
        } else {
            store::N_FAILED
        }
    }

    /// Record it on the row (with its audit event).
    pub fn record(&self, conn: &rusqlite::Connection, forward_id: &str, actor: &str, now: i64) -> Result<(), String> {
        store::set_notice(conn, forward_id, self.notice_state(), now)?;
        match self.via {
            Some(via) => store::record_event(conn, forward_id, store::E_NOTICE_SENT, actor, &json!({ "via": via }), now),
            None => store::record_event(conn, forward_id, store::E_NOTICE_FAILED, actor, &json!({}), now),
        }
    }
}

/// Open `dest`'s stop-link row and send its notice. `Err` only when K2's own
/// store can't be read; a mail that won't go is a `Told` with `via: None`.
/// `asker`: `owner` | a person key | `agent:<name>`.
pub fn tell_destination(mailbox: &str, account_id: Option<&str>, dest: &str, asker: &str, now: i64) -> R<Told> {
    let (row, stop) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        vstore::open_for_notice(&conn, mailbox, dest, asker, now).map_err(email_notify::db_fail)?
    };
    Ok(match send_notice(mailbox, account_id, &row, asker, &stop) {
        Ok(via) => {
            k2_core::log_debug!("{P} notice for row {} sent ({via})", row.id);
            Told { verified_id: row.id, via: Some(via), error: None }
        }
        Err(f) => {
            k2_core::log_debug!("{P} WARN notice for row {} not sent: {}", row.id, f.code);
            Told { verified_id: row.id, via: None, error: Some(email_notify::send::mask_addresses(&f.hint)) }
        }
    })
}

fn send_notice(mailbox: &str, account_id: Option<&str>, row: &vstore::Verified, asker: &str, stop: &str) -> R<&'static str> {
    if email_notify::airgap_blocks() {
        return Err(fail(409, "air_gap", "air-gap is on: nothing leaves this server"));
    }
    let box_label = crate::app_user_invites::box_label();
    let composed = verify::compose_forward_notice(&box_label, &verify::who(asker, &box_label), mailbox, &row.address, &verify::link(verify::STOP_PATH, stop)?);
    match email_notify::send::sender_for(None).ok().flatten() {
        Some(s) => {
            // A suppressed address gets nothing, and the send answers as if
            // it went (the public doors stay enumeration-safe). Here K2 must
            // not record a notice nobody got.
            let to = vec![row.address.clone()];
            if email_notify::outcomes::split_suppressed(&s, None, &to).0.is_empty() {
                return Err(fail(409, "suppressed", "this address bounced or complained before, so this server won't email it"));
            }
            email_notify::send::system_send_from(&s, PURPOSE_NOTICE, None, &row.address, &composed).map(|_| VIA_SYSTEM)
        }
        None => send_from_mailbox(mailbox, account_id.unwrap_or_default(), &row.address, &row.id, &composed).map(|_| VIA_MAILBOX),
    }
}

/// FW-A16: the notice from the mailbox itself (no system sender), by
/// delegation, with no stored copy.
fn send_from_mailbox(mailbox: &str, account_id: &str, to: &str, row_id: &str, c: &email_notify::send::Composed) -> R<()> {
    if account_id.is_empty() {
        return Err(fail(409, "no_mail_path", "this mailbox has no mail-server account to send the notice from"));
    }
    let engine = email_notify::engine::mail_engine()?;
    let to_list = vec![to.to_string()];
    let mid = crate::mail::send::new_message_id_for(mailbox);
    let none = BTreeMap::new();
    let m = compose::Message {
        from_name: "K2",
        from: mailbox,
        to: &to_list,
        cc: &[],
        subject: &c.subject,
        text: Some(&c.text),
        html: None,
        reply_to: &[],
        headers: &none,
        message_id: &mid,
    };
    engine
        .submit(&email_notify::engine::HostedMail {
            account_id,
            from: mailbox,
            rcpt: &to_list,
            email: compose::email_json(&m),
            params: compose::envelope_params(row_id),
            destroy_copy: true,
        })
        .map_err(|e| {
            k2_core::log_debug!("{P} notice for row {row_id} refused upstream: {}", email_notify::send::mask_addresses(&e));
            fail(502, "failed", email_notify::send::UPSTREAM_HINT)
        })
}

// ── the owner ───────────────────────────────────────────────────────

/// Tell the owner (and Admins, and `extra` logins): their Inbox, plus email
/// when the owner has a confirmed notice address and the box a system sender.
/// A failed email is logged; the Inbox item stands.
pub fn tell_owner(title: &str, body: &str, extra: &[String]) {
    super::forward_outcomes::notice(title, body, extra);
    if let Err(f) = email_owner(title, body) {
        k2_core::log_debug!("{P} WARN owner email: {}", f.code);
    }
}

fn email_owner(title: &str, body: &str) -> R<()> {
    if email_notify::airgap_blocks() {
        return Ok(());
    }
    let Some(sender) = email_notify::send::sender_for(None)? else { return Ok(()) };
    let now = email_notify::now();
    let (address, stop) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let Some(v) = vstore::verified_for(&conn, vstore::PURPOSE_PERSON_NOTICE, k2_core::notify_prefs::OWNER).map_err(email_notify::db_fail)? else {
            return Ok(());
        };
        let stop = vstore::mint_stop(&conn, &v.id, now).map_err(email_notify::db_fail)?;
        (v.address, stop)
    };
    let box_label = crate::app_user_invites::box_label();
    let text = format!(
        "{body}\n\n--\nK2 on {box_label}. You get these because this is your notice address.\nStop K2's emails to this address: {}\n",
        verify::link(verify::STOP_PATH, &stop)?
    );
    let subject: String = title.chars().filter(|c| !c.is_control()).take(180).collect();
    let composed = email_notify::send::Composed { subject, text, html: None };
    email_notify::send::system_send_from(&sender, PURPOSE_NOTICE, None, &address, &composed).map(|_| ())
}

/// Who set a forward, in words (`set_by`, or a caller's actor).
pub fn actor_words(actor: &str) -> String {
    if let Some(a) = actor.strip_prefix("agent:") {
        format!("the AI agent {a}")
    } else if let Some(p) = actor.strip_prefix("person:") {
        p.to_string()
    } else if actor == "owner" {
        "the owner or an Admin".to_string()
    } else {
        actor.to_string()
    }
}

/// What one forward change did to a mailbox's outside addresses.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OutsideChange {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    /// Chosen again after pressing Stop.
    pub restarted: Vec<String>,
    /// Added or chosen again, but K2's email to it didn't go.
    pub not_told: Vec<String>,
    /// Added, waiting its turn: K2 emails it, then turns it on.
    pub queued: Vec<String>,
    /// The kept copy changed to this.
    pub keep: Option<bool>,
}

impl OutsideChange {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.queued.is_empty() && self.removed.is_empty() && self.restarted.is_empty() && self.keep.is_none()
    }
}

/// An outside forward was added or changed: the owner hears now, and the
/// Access Audit says who (the addresses' domains only).
pub fn tell_owner_changed(mailbox: &str, actor: &str, c: &OutsideChange) {
    if c.is_empty() {
        return;
    }
    let domains = |v: &[String]| v.iter().map(|d| d.rsplit_once('@').map_or(d.as_str(), |(_, x)| x)).collect::<Vec<_>>().join(",");
    email_notify::audit_line(
        "mail-forward-changed",
        actor,
        &format!(
            "mailbox={mailbox} added={} queued={} removed={} restarted={} keep={:?}",
            domains(&c.added),
            domains(&c.queued),
            domains(&c.removed),
            domains(&c.restarted),
            c.keep
        ),
    );
    let mut lines = vec![format!("Where {mailbox} forwards its mail was changed by {}.", actor_words(actor))];
    if !c.added.is_empty() {
        lines.push(format!("Now also forwarding to: {}. K2 doesn't wait for an address to agree.", c.added.join(", ")));
    }
    if !c.queued.is_empty() {
        lines.push(format!(
            "Added, and on within minutes: {}. K2 is pacing these emails, so it emails each address first and then turns it on.",
            c.queued.join(", ")
        ));
    }
    if !c.restarted.is_empty() {
        lines.push(format!("Turned back on after the address pressed Stop: {}.", c.restarted.join(", ")));
    }
    let told: Vec<&str> = c.added.iter().chain(&c.restarted).filter(|d| !c.not_told.contains(d)).map(String::as_str).collect();
    if !told.is_empty() {
        lines.push(format!("K2 emailed {} a link to stop it.", told.join(", ")));
    }
    if !c.not_told.is_empty() {
        lines.push(format!(
            "K2 couldn't email {}, so they have no link to stop it. The forward runs anyway; K2 tries again hourly and forward list shows it.",
            c.not_told.join(", ")
        ));
    }
    if !c.removed.is_empty() {
        lines.push(format!("No longer forwarding to: {}.", c.removed.join(", ")));
    }
    if let Some(keep) = c.keep {
        lines.push(if keep { "A copy now stays in the mailbox.".to_string() } else { "No copy stays in the mailbox now.".to_string() });
    }
    if !c.added.is_empty() || !c.restarted.is_empty() || !c.queued.is_empty() {
        lines.push(
            "Forwarded mail leaves this server without agent-send approval or BCC. Spam, bounces and mail that fails DMARC stay in the mailbox.".to_string(),
        );
        let one = c.added.first().or(c.restarted.first()).or(c.queued.first()).map_or("<address>", String::as_str);
        lines.push(format!("If you didn't expect this, stop it: k2 hostmail forward rm {mailbox} --to {one}"));
    }
    let mut what: Vec<String> = Vec::new();
    for (word, list) in [("added", &c.added), ("added, on within minutes", &c.queued), ("turned back on after Stop", &c.restarted), ("removed", &c.removed)] {
        if !list.is_empty() {
            what.push(format!("{word} {}", list.join(", ")));
        }
    }
    if let Some(keep) = c.keep {
        what.push(if keep { "keeps a copy".to_string() } else { "keeps no copy".to_string() });
    }
    let line = format!("{mailbox}: {} ({})", what.join("; "), actor_words(actor));
    tell_owner_or_roll(store::OWNER_CHANGE, &line, &format!("Outside forwarding changed on {mailbox}"), &lines.join("\n\n"));
}

/// Tell the owner now if this hour's first few aren't spent, else leave it for
/// the summary. If K2's own record can't be read, tell them now.
fn tell_owner_or_roll(kind: &str, line: &str, title: &str, body: &str) {
    let now = email_notify::now();
    let tell_now = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::owner_note(&conn, kind, line, now).unwrap_or(true)
    };
    if tell_now {
        tell_owner(title, body, &[]);
    }
}

/// How many waiting changes a summary spells out (every stop is, always).
const SUMMARY_LINES: usize = 20;

/// The hourly summary: the count, every stop by name, a sample of the rest,
/// and how to list them.
pub fn flush_owner_summary(now: i64) {
    let due = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        match store::owner_summary_due(&conn, now) {
            Ok(v) => v,
            Err(e) => {
                k2_core::log_debug!("{P} WARN owner summary: {e}");
                return;
            }
        }
    };
    if due.is_empty() {
        return;
    }
    let (stops, changes): (Vec<_>, Vec<_>) = due.iter().partition(|(k, _)| k == store::OWNER_STOP);
    let mut lines = vec![format!(
        "{} more change{} to outside forwarding on this server. K2 told you about the first {} in the hour one by one and rolled up the rest.",
        due.len(),
        if due.len() == 1 { "" } else { "s" },
        store::OWNER_NOTES_PER_HOUR
    )];
    if !stops.is_empty() {
        lines.push(format!(
            "Stopped by the address itself ({}). K2 won't forward to these again unless the owner or an Admin chooses to:\n{}",
            stops.len(),
            stops.iter().map(|(_, l)| l.as_str()).collect::<Vec<_>>().join("\n")
        ));
    }
    if !changes.is_empty() {
        let shown: Vec<&str> = changes.iter().take(SUMMARY_LINES).map(|(_, l)| l.as_str()).collect();
        let more = changes.len().saturating_sub(SUMMARY_LINES);
        lines.push(format!(
            "Changes ({}):\n{}{}",
            changes.len(),
            shown.join("\n"),
            if more > 0 { format!("\nand {more} more") } else { String::new() }
        ));
    }
    lines.push(
        "See every forward: k2 hostmail forward list. See one mailbox's history: k2 hostmail forward show <mailbox>. Each change is also in the access audit.".to_string(),
    );
    tell_owner(&format!("{} more outside forwarding change{}", due.len(), if due.len() == 1 { "" } else { "s" }), &lines.join("\n\n"), &[]);
}

/// Rows whose mail can't arrive (their domain came to this box, or their
/// mailbox here went away, after they were set): K2 leaves them as they are,
/// because they must not silently change, and tells the owner once. The bounces
/// pause an outside one (`dest_gone`); `forward list` names them all.
pub fn note_cant_arrive(host: Option<&str>, now: i64) -> usize {
    let rows = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        match store::cant_arrive_unnoted(&conn, host) {
            Ok(rows) => {
                for f in &rows {
                    if let Err(e) = store::record_event(&conn, &f.id, store::E_CANT_ARRIVE, super::forward_sync::ACTOR_K2, &json!({}), now) {
                        k2_core::log_debug!("{P} WARN forward {}: {e}", f.id);
                    }
                }
                rows
            }
            Err(e) => {
                k2_core::log_debug!("{P} WARN can't-arrive check: {e}");
                return 0;
            }
        }
    };
    if rows.is_empty() {
        return 0;
    }
    tell_owner(
        "Forwards whose mail can't arrive",
        &format!(
            "This server hosts these destinations' domains but has no such mailboxes, so their mail bounces here and never reaches another provider, whatever the domain's MX says: {}.\n\nK2 hasn't changed them. Remove one (k2 hostmail forward rm <mailbox> --to <address>) or create the mailbox. See them: k2 hostmail forward list.",
            pairs(&rows)
        ),
        &[],
    );
    rows.len()
}

// ── stop ────────────────────────────────────────────────────────────

/// The destination pressed Stop: K2 forwards nothing more there, and the
/// owner hears.
pub fn on_stopped(v: &vstore::Verified) {
    let live = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::live_for_pair(&conn, &v.subject_ref, &v.address).ok().flatten()
    };
    if let Some(f) = &live {
        let r = match f.state.as_str() {
            store::ST_PENDING_VERIFY => super::forward_sync::transition_and_sync(
                &f.id,
                store::ST_REMOVED,
                &store::Change { actor: "recipient", detail: Some(json!({ "why": "stopped_before_on" })), ..Default::default() },
            ),
            _ => super::forward_sync::pause(&f.id, store::P_REVOKED, "recipient", None),
        };
        if let Err(e) = r {
            k2_core::log_debug!("{P} forward {} stop: {}", f.id, email_notify::send::mask_addresses(&e.to_string()));
        }
    }
    tell_owner_or_roll(
        store::OWNER_STOP,
        &format!("{} pressed Stop for {}", v.address, v.subject_ref),
        &format!("{} stopped forwarding from {}", v.address, v.subject_ref),
        &format!(
            "{addr} pressed Stop on the link in K2's email, so {mb} no longer forwards to it. New mail stays in {mb}; nothing is lost, and its other destinations are untouched.\n\n\
             K2 won't forward there again unless the owner or an Admin chooses to, and they should ask {addr} first: k2 hostmail forward add {mb} --to {addr} --restart-stopped",
            addr = v.address,
            mb = v.subject_ref
        ),
    );
}

// ── the minute's work ───────────────────────────────────────────────

/// `set_by` → who asked, for the notice.
fn asker_of(set_by: &str) -> String {
    if set_by == "owner" {
        k2_core::notify_prefs::OWNER.to_string()
    } else if let Some(p) = set_by.strip_prefix("person:") {
        p.to_string()
    } else {
        set_by.to_string()
    }
}

/// What one pass did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pass {
    pub turned_on: usize,
    pub left_off: usize,
    pub retried: usize,
}

/// One minute's work: sort rows from before opt-out (one owner notice for
/// whatever this pass sorted), turn on the next few waiting their turn, retry
/// failed notices with what's left of the pace.
/// Air-gap on: nothing (nothing can be told, and nothing may leave).
pub fn minute_pass(now: i64) -> Pass {
    let mut pass = Pass::default();
    // The owner's Inbox isn't mail leaving the box: this runs under air-gap too.
    flush_owner_summary(now);
    if email_notify::airgap_blocks() {
        return pass;
    }
    let (due, stale, next, failed) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let (due, stale) = match store::legacy_sort(&conn, now) {
            Ok(v) => v,
            Err(e) => {
                k2_core::log_debug!("{P} WARN sort: {e}");
                return pass;
            }
        };
        let next = store::legacy_due(&conn, store::NOTICES_PER_PASS).unwrap_or_default();
        let failed = store::failed_notices(&conn, now, store::NOTICES_PER_PASS - next.len()).unwrap_or_default();
        (due, stale, next, failed)
    };
    pass.left_off = stale.len();
    tell_owner_sorted(&due, &stale);
    for f in &next {
        pass.turned_on += turn_on_due(f, now) as usize;
    }
    for f in &failed {
        pass.retried += retry_notice(f, now) as usize;
    }
    pass
}

fn pairs(rows: &[store::Forward]) -> String {
    rows.iter().map(|f| format!("{} to {}", f.address, f.dest)).collect::<Vec<_>>().join("; ")
}

fn tell_owner_sorted(due: &[store::Forward], stale: &[store::Forward]) {
    if due.is_empty() && stale.is_empty() {
        return;
    }
    let mut lines = vec!["K2 no longer asks an outside address to confirm before it forwards mail there: it tells the address, with a link to stop.".to_string()];
    if !due.is_empty() {
        lines.push(format!(
            "Turning on over the next few minutes, because they were set in the last {} days and were still waiting: {}. Each address gets an email with a stop link first.",
            store::LEGACY_ON_WINDOW_SECS / 86_400,
            pairs(due)
        ));
    }
    if !stale.is_empty() {
        lines.push(format!(
            "Not turned on because they're old (set more than {} days ago): {}. They send nothing. To turn one on, set it again: k2 hostmail forward add <mailbox> --to <address>.",
            store::LEGACY_ON_WINDOW_SECS / 86_400,
            pairs(stale)
        ));
    }
    tell_owner("Forwards that were waiting for a confirm", &lines.join("\n\n"), &[]);
}

fn turn_on_due(f: &store::Forward, now: i64) -> bool {
    // Stopped stays stopped (the stop link usually removed the row already).
    let stopped = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        vstore::is_stopped(&conn, &f.address, &f.dest)
    };
    if stopped {
        let r = super::forward_sync::transition_and_sync(
            &f.id,
            store::ST_REMOVED,
            &store::Change { actor: super::forward_sync::ACTOR_K2, detail: Some(json!({ "why": "stopped_before_on" })), ..Default::default() },
        );
        if let Err(e) = r {
            k2_core::log_debug!("{P} forward {} stopped: {}", f.id, email_notify::send::mask_addresses(&e.to_string()));
        }
        return false;
    }
    let account = super::forward_sync::mailbox(&f.address_id).ok().flatten().and_then(|m| m.account_id);
    let told = match tell_destination(&f.address, account.as_deref(), &f.dest, &asker_of(&f.set_by), now) {
        Ok(t) => t,
        Err(e) => {
            k2_core::log_debug!("{P} WARN forward {}: {}", f.id, e.code);
            return false;
        }
    };
    let id = f.id.clone();
    let r = super::forward_sync::change_and_sync(&f.address_id, |c, now| {
        let row = store::transition(
            c,
            &id,
            store::ST_ACTIVE,
            &store::Change {
                verified_address_id: Some(&told.verified_id),
                actor: super::forward_sync::ACTOR_K2,
                detail: Some(json!({ "why": "opt_out_update" })),
                ..Default::default()
            },
            now,
        )?;
        told.record(c, &row.id, super::forward_sync::ACTOR_K2, now).map_err(store::TransitionError::Db)?;
        if store::is_legacy_due(c, &row.id).map_err(store::TransitionError::Db)? {
            store::record_event(c, &row.id, store::E_LEGACY_ON, super::forward_sync::ACTOR_K2, &json!({}), now).map_err(store::TransitionError::Db)?;
        }
        store::get(c, &row.id).map_err(store::TransitionError::Db)?.ok_or(store::TransitionError::NotFound)
    });
    match r {
        Ok(_) | Err(super::forward_sync::ChangeError::Sieve(..)) => true,
        Err(e) => {
            k2_core::log_debug!("{P} WARN forward {} not turned on: {}", f.id, email_notify::send::mask_addresses(&e.to_string()));
            false
        }
    }
}

fn retry_notice(f: &store::Forward, now: i64) -> bool {
    let account = super::forward_sync::mailbox(&f.address_id).ok().flatten().and_then(|m| m.account_id);
    let Ok(told) = tell_destination(&f.address, account.as_deref(), &f.dest, &asker_of(&f.set_by), now) else { return false };
    let db = k2_core::db::shared();
    let conn = db.lock();
    if let Err(e) = told.record(&conn, &f.id, super::forward_sync::ACTOR_K2, now) {
        k2_core::log_debug!("{P} WARN forward {} notice state: {e}", f.id);
    }
    told.via.is_some()
}
