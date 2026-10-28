//! Delivery outcomes, suppressions and auto-pause (prd-email-notifications-
//! part2-v1 §4 N6; vs-live EN-A3, EN-A13, EN-A14, EN-A20, EN-A22, EN-A30).
//! The store is `k2_core::email_outcomes` (k2.db 0245).
//!
//! **Where the facts come from.** Every [`PASS_SECS`] the daemon reads each
//! hosted sender account's **Inbox and Junk** for delivery reports (RFC
//! 3464 `message/delivery-status`) and complaint reports (RFC 5965
//! `message/feedback-report`). It downloads only those parts (and, for a
//! complaint, the original's headers), never a returned body, records the
//! outcome per recipient, then destroys the report. A DSN is tied to its
//! send by `Original-Envelope-Id` (K2 submits with `ENVID=<log id>`), a
//! complaint by the original `Message-ID` (or its envelope id). A report
//! naming a send this sender didn't make, or a recipient that send didn't
//! have, is ignored: anyone can mail a sender address, so a forged report
//! can't suppress strangers. Relay senders get no reports (they go to the
//! relay's return path, EN-A22). The hourly 24 h sweep stays the backstop.
//!
//! **Suppressions.** A hard bounce (`Action: failed`, `5.x.x`) suppresses
//! the address box-wide; a complaint suppresses it for that sender. A send
//! drops suppressed recipients from the envelope (status `suppressed`);
//! all of them suppressed: `409 all_suppressed`, nothing sent. K2's own
//! fixed mail to a suppressed address is not sent either, and the caller
//! gets the same answer as a sent one (enumeration-safe).
//!
//! **Auto-pause (core Q11).** Over the trailing 24 h (or since the last
//! resume), with at least 50 recipients handed off: hard bounces over 5% or
//! complaints over 0.1% pause an App's (or agent's) sender: its sends
//! answer `423 email_paused` until the owner resumes it with a fresh
//! confirm (`email-resume`, bound to the sender address). **The system
//! sender is never paused** (Rosson 2026-10-28: pausing it would stop every
//! password reset); it gets a warning notice instead. While an App's sender
//! is paused, K2's own fixed mail for that App rides the system sender
//! (EN-A13); App-written text never does.

use std::collections::{HashMap, HashSet};

use k2_core::email_notify::{self as store, Sender};
use k2_core::email_outcomes as oc;
use serde_json::{json, Value as J};

use super::{db_fail, fail, Fail, R};

/// The report pass's cadence.
pub const PASS_SECS: u64 = 300;
/// Reports read per sender per pass.
pub const REPORTS_PER_PASS: usize = 200;

// ── parsing (pure) ──────────────────────────────────────────────────

/// One RFC 822-style field block: `(lower-case name, value)`, folded
/// lines joined.
fn fields(block: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in block.lines() {
        if line.starts_with([' ', '\t']) {
            if let Some(last) = out.last_mut() {
                last.1.push(' ');
                last.1.push_str(line.trim());
            }
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            out.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
    }
    out
}

fn blocks(text: &str) -> Vec<String> {
    let norm = text.replace("\r\n", "\n");
    norm.split("\n\n").map(str::trim).filter(|b| !b.is_empty()).map(str::to_string).collect()
}

fn get<'a>(f: &'a [(String, String)], name: &str) -> Option<&'a str> {
    f.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

/// `rfc822; a@b` → `a@b` (lower-case).
fn typed_address(v: &str) -> String {
    let a = v.split_once(';').map(|(_, a)| a).unwrap_or(v);
    a.trim().trim_matches(|c| c == '<' || c == '>').to_ascii_lowercase()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DsnRecipient {
    pub address: String,
    pub action: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dsn {
    pub envelope_id: Option<String>,
    pub recipients: Vec<DsnRecipient>,
}

/// An RFC 3464 `message/delivery-status` body.
pub fn parse_dsn(text: &str) -> Dsn {
    let bs = blocks(text);
    let mut envelope_id = None;
    let mut recipients = Vec::new();
    for (i, b) in bs.iter().enumerate() {
        let f = fields(b);
        if i == 0 {
            envelope_id = get(&f, "original-envelope-id").map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        }
        let rcpt = get(&f, "final-recipient").or_else(|| get(&f, "original-recipient"));
        if let (Some(r), Some(action)) = (rcpt, get(&f, "action")) {
            recipients.push(DsnRecipient {
                address: typed_address(r),
                action: action.trim().to_ascii_lowercase(),
                status: get(&f, "status").unwrap_or("").split_whitespace().next().unwrap_or("").to_string(),
            });
        }
    }
    Dsn { envelope_id, recipients }
}

/// The outcome kind for one DSN recipient (`None`: nothing to record).
pub fn dsn_kind(r: &DsnRecipient) -> Option<&'static str> {
    match r.action.as_str() {
        "failed" if r.status.starts_with('5') => Some(oc::K_HARD),
        "failed" => Some(oc::K_SOFT),
        "delayed" => Some(oc::K_DELAYED),
        "delivered" | "relayed" | "expanded" => Some(oc::K_DELIVERED),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arf {
    pub feedback_type: String,
    pub rcpt: Option<String>,
    pub envelope_id: Option<String>,
    pub message_id: Option<String>,
}

/// An RFC 5965 `message/feedback-report` body plus, when the report
/// carried it, the original message's header block.
pub fn parse_arf(report: &str, original_headers: Option<&str>) -> Arf {
    let f: Vec<(String, String)> = blocks(report).iter().flat_map(|b| fields(b)).collect();
    let message_id = original_headers.and_then(|h| {
        let head = h.replace("\r\n", "\n");
        let head = head.split("\n\n").next().unwrap_or("").to_string();
        fields(&head).into_iter().find(|(k, _)| k == "message-id").map(|(_, v)| norm_mid(&v))
    });
    Arf {
        feedback_type: get(&f, "feedback-type").unwrap_or("").trim().to_ascii_lowercase(),
        rcpt: get(&f, "original-rcpt-to").map(typed_address).filter(|a| a.contains('@')),
        envelope_id: get(&f, "original-envelope-id").map(|v| v.trim().to_string()).filter(|v| !v.is_empty()),
        message_id,
    }
}

/// `<id@host>` → `id@host`.
pub fn norm_mid(v: &str) -> String {
    v.trim().trim_start_matches('<').trim_end_matches('>').trim().to_ascii_lowercase()
}

// ── the pass ────────────────────────────────────────────────────────

/// What one report did (for the pass's counts and the tests).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applied {
    /// Outcomes recorded (count).
    Recorded(usize),
    /// Not one of this sender's sends, or no known recipient: ignored
    /// (the report is still destroyed).
    Ignored,
}

fn log_for_envelope(conn: &rusqlite::Connection, sender_id: &str, envelope_id: &str) -> Option<store::LogRow> {
    store::get_log(conn, envelope_id).ok().flatten().filter(|r| r.sender_id == sender_id)
}

fn log_for_message_id(conn: &rusqlite::Connection, sender_id: &str, mid: &str) -> Option<store::LogRow> {
    let id: Option<String> = conn
        .query_row(
            "SELECT id FROM email_notify_log WHERE sender_id = ?1 AND lower(trim(message_id, '<> ')) = ?2 LIMIT 1",
            rusqlite::params![sender_id, mid],
            |r| r.get(0),
        )
        .ok();
    id.and_then(|id| store::get_log(conn, &id).ok().flatten())
}

fn has_rcpt(row: &store::LogRow, a: &str) -> bool {
    row.to.iter().any(|t| t.trim().eq_ignore_ascii_case(a))
}

/// Apply one report for sender `s` (no I/O but the db).
pub fn apply(s: &Sender, report: &super::engine::RawReport, now: i64) -> R<Applied> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    match report.kind {
        super::engine::ReportKind::Dsn => {
            let d = parse_dsn(&report.report);
            let Some(row) = d.envelope_id.as_deref().and_then(|e| log_for_envelope(&conn, &s.id, e)) else {
                return Ok(Applied::Ignored);
            };
            let mut n = 0;
            for r in d.recipients.iter().filter(|r| has_rcpt(&row, &r.address)) {
                let Some(kind) = dsn_kind(r) else { continue };
                if oc::record(&conn, &row.id, &s.id, &r.address, kind, Some(&r.status), oc::SRC_DSN, now).map_err(db_fail)? {
                    n += 1;
                }
                if kind == oc::K_HARD {
                    oc::suppress(&conn, &r.address, oc::SCOPE_BOX, "", oc::R_HARD, "dsn", now).map_err(db_fail)?;
                }
            }
            Ok(if n == 0 { Applied::Ignored } else { Applied::Recorded(n) })
        }
        super::engine::ReportKind::Arf => {
            let a = parse_arf(&report.report, report.original_headers.as_deref());
            if a.feedback_type == "not-spam" {
                return Ok(Applied::Ignored);
            }
            let row = a
                .envelope_id
                .as_deref()
                .and_then(|e| log_for_envelope(&conn, &s.id, e))
                .or_else(|| a.message_id.as_deref().and_then(|m| log_for_message_id(&conn, &s.id, m)));
            let Some(row) = row else { return Ok(Applied::Ignored) };
            let rcpt = match a.rcpt {
                Some(r) if has_rcpt(&row, &r) => r,
                Some(_) => return Ok(Applied::Ignored),
                None if row.to.len() == 1 => row.to[0].trim().to_ascii_lowercase(),
                None => return Ok(Applied::Ignored),
            };
            let n = oc::record(&conn, &row.id, &s.id, &rcpt, oc::K_COMPLAINED, Some(&a.feedback_type), oc::SRC_ARF, now).map_err(db_fail)?;
            oc::suppress(&conn, &rcpt, oc::SCOPE_SENDER, &s.id, oc::R_COMPLAINT, "arf", now).map_err(db_fail)?;
            Ok(if n { Applied::Recorded(1) } else { Applied::Ignored })
        }
    }
}

/// One pass over every live hosted sender. Returns `(reports read,
/// outcomes recorded)`.
pub fn pass(now: i64) -> (usize, usize) {
    if !super::box_enabled() || !super::hosted_supported() {
        return (0, 0);
    }
    let senders: Vec<Sender> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::list_senders(&conn, false).unwrap_or_default()
    };
    let hosted: Vec<Sender> =
        senders.into_iter().filter(|s| s.path == store::PATH_HOSTED && s.stalwart_account_id.is_some() && s.is_live()).collect();
    if hosted.is_empty() {
        return (0, 0);
    }
    let engine = match super::engine::mail_engine() {
        Ok(e) => e,
        Err(f) => {
            k2_core::log_debug!("[email-notify] report pass skipped: {}", f.code);
            return (0, 0);
        }
    };
    let (mut read, mut recorded) = (0, 0);
    for s in &hosted {
        let acct = s.stalwart_account_id.as_deref().unwrap_or_default();
        let reports = match engine.fetch_reports(acct, REPORTS_PER_PASS) {
            Ok(r) => r,
            Err(e) => {
                k2_core::log_debug!("[email-notify] WARN report read for sender {}: {}", s.id, super::send::mask_addresses(&e));
                continue;
            }
        };
        let mut done = Vec::new();
        let mut touched = false;
        for r in &reports {
            read += 1;
            match apply(s, r, now) {
                Ok(Applied::Recorded(n)) => {
                    recorded += n;
                    touched = true;
                    done.push(r.email_id.clone());
                }
                Ok(Applied::Ignored) => done.push(r.email_id.clone()),
                // Kept for the next pass (the store refused).
                Err(f) => k2_core::log_debug!("[email-notify] WARN report for sender {}: {}", s.id, f.code),
            }
        }
        if !done.is_empty() {
            if let Err(e) = engine.destroy_emails(acct, &done) {
                k2_core::log_debug!("[email-notify] WARN report destroy for sender {}: {}", s.id, super::send::mask_addresses(&e));
            }
        }
        if touched {
            evaluate(s, now);
        }
    }
    (read, recorded)
}

// ── pauses ──────────────────────────────────────────────────────────

/// When the system sender was last warned (once a day at most).
static SYSTEM_WARNED: std::sync::Mutex<Option<i64>> = std::sync::Mutex::new(None);

fn owner_label(s: &Sender) -> String {
    match s.app() {
        Some(app) => crate::app_push::published(app).map(|p| format!("App {}", p.name)).unwrap_or_else(|| format!("App {app}")),
        None if s.is_system() => "the system sender".to_string(),
        None => "an unassigned sender".to_string(),
    }
}

/// Check one sender's rate after new outcomes: pause it (never the system
/// sender, which gets a warning) and tell the owner.
pub fn evaluate(s: &Sender, now: i64) -> Option<&'static str> {
    let (rate, reason) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let from = oc::window_start(&conn, &s.id, now).ok()?;
        let (sent, hard, complained) = oc::window(&conn, &s.id, from).ok()?;
        let reason = oc::trips(sent, hard, complained)?;
        (json!({ "sent": sent, "hardBounces": hard, "complaints": complained, "since": from }), reason)
    };
    let (sent, hard, complained) = (rate["sent"].as_i64().unwrap_or(0), rate["hardBounces"].as_i64().unwrap_or(0), rate["complaints"].as_i64().unwrap_or(0));
    let what = if reason == "hard_bounces" {
        format!("{hard} of {sent} recipients hard-bounced")
    } else {
        format!("{complained} of {sent} recipients reported it as spam")
    };
    if s.is_system() {
        let mut last = SYSTEM_WARNED.lock().unwrap_or_else(|e| e.into_inner());
        if last.is_some_and(|t| now - t < oc::WINDOW_SECS) {
            return Some(reason);
        }
        *last = Some(now);
        notice(
            &format!("Warning: {}'s email is bouncing", s.address),
            &format!(
                "The system sender {} isn't paused (it carries every password reset on this server), but {what} in the last 24 hours. \
                 Check what is sending through it: k2 notifications email log.",
                s.address
            ),
        );
        return Some(reason);
    }
    let paused = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        oc::pause(&conn, &s.id, reason, &rate.to_string(), now)
    };
    match paused {
        Ok(Some(_)) => {
            super::audit_line("email-sender-paused", "k2", &format!("sender={} reason={reason} rate={rate}", s.id));
            let label = owner_label(s);
            notice(
                &format!("{label}'s email is paused"),
                &format!(
                    "{label}'s email from {} is paused: {what} in 24 hours. Its sends now answer email_paused. \
                     Fix the list, then resume it from your own terminal: k2 notifications email sender resume {} \
                     (it asks you to confirm). Suppressed addresses: k2 notifications email suppressions.",
                    s.address, s.address
                ),
            );
            if let Some(app) = s.app() {
                if let Some(p) = crate::app_push::published(app) {
                    crate::apps::alerts::note(
                        &p.project_id,
                        "App email paused",
                        &format!("{label}'s email is paused: {what} in 24 hours. Only the owner can resume it. Check who the App mails."),
                    );
                }
            }
            Some(reason)
        }
        Ok(None) => Some(reason),
        Err(e) => {
            k2_core::log_debug!("[email-notify] WARN pause of sender {}: {e}", s.id);
            None
        }
    }
}

fn notice(title: &str, body: &str) {
    if let Err(e) = crate::feedback_routes::file_owner_notice(k2_core::db::AUDIT_SENTINEL_IDS[0], "k2-email", title, body) {
        k2_core::log_debug!("[email-notify] WARN owner notice: {e}");
    }
}

/// Is this sender paused? The system sender never is. Fail closed.
pub fn is_paused(s: &Sender) -> bool {
    if s.is_system() {
        return false;
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    !matches!(oc::live_pause(&conn, &s.id), Ok(None))
}

/// `423 email_paused` for a paused sender (no `retryAfterMs`: only the
/// owner's resume ends it).
pub fn refuse_if_paused(s: &Sender) -> R<()> {
    if is_paused(s) {
        return Err(fail(
            423,
            "email_paused",
            "this sender is paused because too many recipients bounced or complained; the owner resumes it (k2 notifications email sender resume)",
        ));
    }
    Ok(())
}

/// The owner resumes a paused sender (the route checked the confirm).
pub fn resume(s: &Sender, by: &str, now: i64) -> R<J> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    if !oc::resume(&conn, &s.id, by, now).map_err(db_fail)? {
        return Err(fail(409, "not_paused", format!("{} isn't paused", s.address)));
    }
    drop(conn);
    super::audit_line("email-sender-resumed", by, &format!("sender={}", s.id));
    Ok(json!({ "ok": true, "sender": s.address, "paused": false }))
}

/// The pause shown on a sender (`None`: not paused).
pub fn pause_json(s: &Sender) -> J {
    let db = k2_core::db::shared();
    let conn = db.lock();
    match oc::live_pause(&conn, &s.id) {
        Ok(Some(p)) => json!({ "pausedAt": p.paused_at, "reason": p.reason, "rate": serde_json::from_str::<J>(&p.rate_json).unwrap_or(J::Null) }),
        _ => J::Null,
    }
}

// ── suppressions in the send path ───────────────────────────────────

/// Split an envelope into (send to, suppressed).
pub fn split_suppressed(s: &Sender, app: Option<&str>, envelope: &[String]) -> (Vec<String>, Vec<String>) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    envelope.iter().cloned().partition(|a| oc::suppressed_for(&conn, a, &s.id, app).is_none())
}

/// Record the recipients a send dropped.
pub fn record_suppressed(log_id: &str, s: &Sender, dropped: &[String], now: i64) {
    if dropped.is_empty() {
        return;
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    for a in dropped {
        if let Err(e) = oc::record(&conn, log_id, &s.id, a, oc::K_SUPPRESSED, None, oc::SRC_K2, now) {
            k2_core::log_debug!("[email-notify] WARN suppressed outcome for log {log_id}: {e}");
        }
    }
}

pub fn all_suppressed() -> Fail {
    fail(409, "all_suppressed", "every recipient is suppressed (a hard bounce, a complaint or an unsubscribe); nothing was sent")
}

// ── what Apps see ───────────────────────────────────────────────────

/// Per-recipient status for `email.status`: the strongest outcome wins
/// (suppressed, complained, bounced, delivered, delayed), else the row's
/// own status (`submitted` once handed off).
pub fn recipients_json(row: &store::LogRow) -> J {
    let rank = |k: &str| match k {
        oc::K_SUPPRESSED => 6,
        oc::K_COMPLAINED => 5,
        oc::K_HARD | oc::K_SOFT => 4,
        oc::K_DELIVERED => 3,
        oc::K_DELAYED => 2,
        _ => 0,
    };
    let word = |k: &str| match k {
        oc::K_HARD | oc::K_SOFT => "bounced",
        k => k,
    };
    let outcomes = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        oc::for_log(&conn, &row.id).unwrap_or_default()
    };
    let mut best: HashMap<String, String> = HashMap::new();
    for (h, k) in outcomes {
        let e = best.entry(h).or_default();
        if rank(&k) > rank(e) {
            *e = k;
        }
    }
    let mut seen = HashSet::new();
    let list: Vec<J> = row
        .to
        .iter()
        .filter(|t| seen.insert(t.to_ascii_lowercase()))
        .map(|t| {
            let st = best.get(&oc::address_sha256(t)).map(|k| word(k).to_string()).unwrap_or_else(|| row.status.clone());
            json!({ "to": t, "status": st })
        })
        .collect();
    J::Array(list)
}

/// `(bounced, complained)` recipients of one row, for `email.log`.
pub fn counts(row_id: &str) -> (i64, i64) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    oc::counts_for_log(&conn, row_id).unwrap_or((0, 0))
}

#[cfg(test)]
mod parse_tests {
    use super::*;

    /// Probe P2 shape (Stalwart 0.16.25, RET=HDRS): per-message block,
    /// then one block per recipient.
    const DSN: &str = "Reporting-MTA: dns; mail.example.test\r\nOriginal-Envelope-Id: enl_abc123\r\nArrival-Date: Tue, 27 Oct 2026 12:00:00 +0000\r\n\r\n\
Final-Recipient: rfc822; Bounce-X@sink.example\r\nAction: failed\r\nStatus: 5.1.2\r\nDiagnostic-Code: smtp; 550 5.1.2 Mailbox does not\r\n  exist\r\nRemote-MTA: dns; sink.example\r\n\r\n\
Final-Recipient: rfc822; slow@sink.example\r\nAction: delayed\r\nStatus: 4.4.1\r\n";

    #[test]
    fn a_dsn_names_its_send_and_each_recipients_outcome() {
        let d = parse_dsn(DSN);
        assert_eq!(d.envelope_id.as_deref(), Some("enl_abc123"));
        assert_eq!(d.recipients.len(), 2);
        assert_eq!(d.recipients[0], DsnRecipient { address: "bounce-x@sink.example".into(), action: "failed".into(), status: "5.1.2".into() });
        assert_eq!(dsn_kind(&d.recipients[0]), Some(oc::K_HARD));
        assert_eq!(dsn_kind(&d.recipients[1]), Some(oc::K_DELAYED));
        assert_eq!(dsn_kind(&DsnRecipient { address: "a@b".into(), action: "failed".into(), status: "4.2.2".into() }), Some(oc::K_SOFT));
        assert_eq!(dsn_kind(&DsnRecipient { address: "a@b".into(), action: "delivered".into(), status: "2.0.0".into() }), Some(oc::K_DELIVERED));
        assert_eq!(parse_dsn("garbage").recipients, vec![]);
    }

    #[test]
    fn an_arf_names_its_type_recipient_and_the_original_message_id() {
        let report = "Feedback-Type: abuse\nUser-Agent: SomeFBL/1.0\nVersion: 1\nOriginal-Rcpt-To: <User@Big.example>\nArrival-Date: x\n";
        let headers = "Received: x\r\nMessage-ID: <K2.1@notify.brand.test>\r\nSubject: hi\r\n\r\nbody never read";
        let a = parse_arf(report, Some(headers));
        assert_eq!(a.feedback_type, "abuse");
        assert_eq!(a.rcpt.as_deref(), Some("user@big.example"));
        assert_eq!(a.message_id.as_deref(), Some("k2.1@notify.brand.test"));
        assert_eq!(a.envelope_id, None);
        assert_eq!(norm_mid(" <A@B> "), "a@b");
    }
}
