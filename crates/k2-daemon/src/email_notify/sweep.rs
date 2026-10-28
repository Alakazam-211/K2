//! The daemon's Email Notifications loops (headless: no client needed).
//!
//! - Every [`TICK_SECS`]: scheduled sends that are due, then kept bodies and
//!   approvals a removed App user left ([`super::send::tick`]).
//! - Every 5 minutes (N6): the delivery and complaint reports in each hosted
//!   sender's Inbox and Junk ([`super::outcomes::pass`]); a bad rate pauses
//!   an App's sender.
//! - Hourly:
//!   - E5: destroy everything older than 24 h in each hosted sender's
//!     mailbox (DSNs, ARF reports, replies to a no-reply address, a draft a
//!     refused submission left) and count delivery reports as a bounce hint;
//!     a retired sender's mailbox is emptied completely until its row goes
//!     (security review L3);
//!   - E22: log rows past the owner's retention (30 days by default);
//!   - E19: idempotency keys past 24 h;
//!   - E26: retired senders whose 30-day reservation ended;
//!   - E15: held rows whose approval was denied, expired or failed become `refused`;
//!   - V: confirm links past 24 h expire; dead verified-address rows go after 90 days.

use k2_core::email_notify as store;

/// The scheduled-send tick (the mail flusher's cadence).
pub const TICK_SECS: u64 = 15;
/// Sender mailboxes keep at most this much (E5).
pub const MAILBOX_KEEP_SECS: i64 = 24 * 3600;

/// One hourly pass. Returns `(messages destroyed, log rows purged)`.
pub fn hourly(now: i64) -> (u64, usize) {
    let mut destroyed = 0u64;
    // E5: sender mailboxes.
    let senders = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::list_senders(&conn, true).unwrap_or_default()
    };
    let hosted: Vec<_> = senders.into_iter().filter(|s| s.path == store::PATH_HOSTED && s.stalwart_account_id.is_some()).collect();
    if !hosted.is_empty() && super::hosted_supported() {
        match super::engine::mail_engine() {
            Ok(engine) => {
                for s in &hosted {
                    let acct = s.stalwart_account_id.as_deref().unwrap_or_default();
                    // L3: a retired sender's mailbox is emptied completely until its row is purged.
                    let before = if s.is_live() { now - MAILBOX_KEEP_SECS } else { now };
                    match engine.sweep_account(acct, before) {
                        Ok((gone, dsns)) => {
                            destroyed += gone;
                            let db = k2_core::db::shared();
                            let conn = db.lock();
                            if let Err(e) = store::record_sweep(&conn, &s.id, dsns as i64, now) {
                                k2_core::log_debug!("[email-notify] WARN sweep mark {}: {e}", s.id);
                            }
                        }
                        Err(e) => k2_core::log_debug!("[email-notify] WARN sweep of sender {}: {e}", s.id),
                    }
                }
            }
            Err(f) => k2_core::log_debug!("[email-notify] sweep skipped: {}", f.code),
        }
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    // E22: retention.
    let days = store::settings(&conn).map(|s| s.log_retention_days).unwrap_or(store::DEFAULT_RETENTION_DAYS);
    let purged = store::purge_log_before(&conn, now - days * 86_400).unwrap_or_else(|e| {
        k2_core::log_debug!("[email-notify] WARN log retention: {e}");
        0
    });
    // E19 and E26.
    if let Err(e) = store::idempotency_prune(&conn, now - store::IDEMPOTENCY_SECS) {
        k2_core::log_debug!("[email-notify] WARN idempotency prune: {e}");
    }
    match store::purge_retired(&conn, now - store::RESERVE_DAYS * 86_400) {
        Ok(n) if n > 0 => k2_core::log_debug!("[email-notify] released {n} retired sender(s) after their reservation"),
        Ok(_) => {}
        Err(e) => k2_core::log_debug!("[email-notify] WARN retired purge: {e}"),
    }
    // E15: held rows whose approval ended without a send.
    if let Ok(held) = store::held_rows(&conn, 500) {
        for r in held {
            let Some(ob) = r.outbound_id.as_deref() else { continue };
            let status: Option<String> = conn
                .query_row("SELECT status FROM mail_outbound WHERE id = ?1", rusqlite::params![ob], |x| x.get(0))
                .ok();
            let end = match status.as_deref() {
                None => Some("lost"),
                Some("denied") => Some("denied"),
                Some("expired") => Some("expired"),
                Some("failed") => Some("failed"),
                _ => None,
            };
            if let Some(code) = end {
                let _ = store::set_log_status(&conn, &r.id, store::ST_HELD, store::ST_REFUSED, Some(code), now);
            }
        }
    }
    drop(conn);
    // Verified addresses (slice V): expire due links, forget long-dead rows.
    super::verify::hourly(now);
    // N6: outcome rows go with their log row (retention).
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if let Err(e) = k2_core::email_outcomes::purge_orphans(&conn) {
            k2_core::log_debug!("[email-notify] WARN outcome purge: {e}");
        }
    }
    if destroyed > 0 || purged > 0 {
        k2_core::log_debug!("[email-notify] hourly: {destroyed} sender-mailbox message(s) destroyed, {purged} log row(s) past retention");
    }
    (destroyed, purged)
}

/// Start both loops (called once from the daemon's boot, after the
/// readiness gate).
pub fn spawn() {
    tokio::spawn(async move {
        loop {
            let _ = tokio::task::spawn_blocking(|| super::send::tick(super::now())).await;
            tokio::time::sleep(std::time::Duration::from_secs(TICK_SECS)).await;
        }
    });
    tokio::spawn(async move {
        // N6: delivery and complaint reports, every 5 minutes (headless).
        tokio::time::sleep(std::time::Duration::from_secs(120)).await;
        loop {
            match tokio::task::spawn_blocking(|| super::outcomes::pass(super::now())).await {
                Ok((read, recorded)) if read > 0 => {
                    k2_core::log_debug!("[email-notify] reports: {read} read, {recorded} outcome(s) recorded")
                }
                Ok(_) => {}
                Err(e) => k2_core::log_debug!("[email-notify] WARN report pass join: {e}"),
            }
            tokio::time::sleep(std::time::Duration::from_secs(super::outcomes::PASS_SECS)).await;
        }
    });
    tokio::spawn(async move {
        // First pass ~7 minutes after boot, then hourly.
        tokio::time::sleep(std::time::Duration::from_secs(420)).await;
        loop {
            if let Err(e) = tokio::task::spawn_blocking(|| hourly(super::now())).await {
                k2_core::log_debug!("[email-notify] WARN hourly join: {e}");
            }
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
        }
    });
}
