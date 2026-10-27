//! Budgets (prd-email-notifications-core-v1 E17, Q12). Counted in
//! **envelope recipients** over the log's rows (restart-proof), never in
//! calls; idempotent replays don't count again (they never reach here).
//! An unreadable count refuses (fail-closed).
//!
//! | bucket | limit |
//! |---|---|
//! | App (`users` scope), its own sends and its agent's | 100/h, 1,000/day |
//! | App with scope `any` | 30/h, 300/day |
//! | warm-up: App sends from a domain verified < 14 days ago (relay: a sender < 14 days old) | 20/h, 200/day |
//! | K2's system mail (exempt from warm-up) | 200/h |
//! | everything on the box | 1,000/h |
//!
//! **When a send counts (security review M1).** Every row counts at its
//! *effective send time*: `sendAt` for a scheduled send, else when it was
//! made. A send at time `t` must fit every window that could hold it, so
//! a check counts the rows in `[t − window, t + window]`: piling scheduled
//! sends onto one time (or around it) can't add up past a limit. The
//! flusher checks again when a scheduled send is due and defers it while
//! it doesn't fit. On top, an App may keep at most one day's budget
//! waiting in scheduled sends ([`check_pending`]).
//!
//! Removing an App user never gives budget back (review H1): their rows
//! stay, with the address blanked.

use k2_core::email_notify::{self as store, Bucket};

use super::{fail, Fail, R};

pub const HOUR: i64 = 3600;
pub const DAY: i64 = 86_400;
pub const APP_HOURLY: i64 = 100;
pub const APP_DAILY: i64 = 1_000;
pub const ANY_HOURLY: i64 = 30;
pub const ANY_DAILY: i64 = 300;
pub const WARMUP_HOURLY: i64 = 20;
pub const WARMUP_DAILY: i64 = 200;
pub const WARMUP_SECS: i64 = 14 * DAY;
pub const SYSTEM_HOURLY: i64 = 200;
pub const BOX_HOURLY: i64 = 1_000;

/// Who a send is for, as far as budgets go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind<'a> {
    /// K2's own mail (system sender or an App's sender for K2's text).
    System,
    /// An App's send (its link, key, or its agent/owner via the CLI).
    App { app: &'a str, scope_any: bool, warmup_domain: Option<&'a str> },
}

fn limited(what: &str, used: i64, limit: i64, window: i64, oldest: Option<i64>, at: i64) -> Fail {
    let retry_secs = oldest.map(|o| (o + window - at).max(1)).unwrap_or(window);
    let mut f = fail(
        429,
        "rate_limited",
        format!("{what}: {used} of {limit} recipients within {} of this send; try again later", if window == HOUR { "an hour" } else { "a day" }),
    );
    f.retry_after_ms = Some((retry_secs as u64).saturating_mul(1000));
    f
}

fn check_one(conn: &rusqlite::Connection, b: &Bucket<'_>, what: &str, limit: i64, window: i64, n: i64, at: i64) -> R<()> {
    let (used, oldest) = store::sum_recipients(conn, b, at - window + 1, at + window - 1)
        .map_err(|e| fail(503, "failed", format!("K2 couldn't read the send budget, so it didn't send (fail-closed): {e}")))?;
    if used + n > limit {
        return Err(limited(what, used, limit, window, oldest, at));
    }
    Ok(())
}

/// Is `domain`'s sender (hosted: the domain's `verified_at`; relay: the
/// sender's `created_at`) still warming up at `now`?
pub fn warming_up(sender: &store::Sender, now: i64) -> bool {
    let since = if sender.path == store::PATH_RELAY {
        Some(sender.created_at)
    } else {
        let db = k2_core::db::shared();
        let conn = db.lock();
        crate::mail::domains::load_domain(&conn, &sender.domain).and_then(|d| d.verified_at)
    };
    since.is_some_and(|t| now - t < WARMUP_SECS)
}

/// Would `n` more recipients, sent at `at`, fit every bucket this send
/// counts in? `n = 0` re-checks a row that is already counted (the
/// flusher).
pub fn check(kind: &Kind<'_>, n: usize, at: i64) -> R<()> {
    let n = n as i64;
    let db = k2_core::db::shared();
    let conn = db.lock();
    check_one(&conn, &Bucket::Box, "this server's notification budget", BOX_HOURLY, HOUR, n, at)?;
    match kind {
        Kind::System => check_one(&conn, &Bucket::System, "K2's system mail budget", SYSTEM_HOURLY, HOUR, n, at),
        Kind::App { app, scope_any, warmup_domain } => {
            check_one(&conn, &Bucket::App(app), "this App's email budget", APP_HOURLY, HOUR, n, at)?;
            check_one(&conn, &Bucket::App(app), "this App's email budget", APP_DAILY, DAY, n, at)?;
            if *scope_any {
                check_one(&conn, &Bucket::AppAny(app), "this App's any-address budget", ANY_HOURLY, HOUR, n, at)?;
                check_one(&conn, &Bucket::AppAny(app), "this App's any-address budget", ANY_DAILY, DAY, n, at)?;
            }
            if let Some(d) = warmup_domain {
                check_one(&conn, &Bucket::Domain(d), "the new domain's warm-up budget (its first 14 days)", WARMUP_HOURLY, HOUR, n, at)?;
                check_one(&conn, &Bucket::Domain(d), "the new domain's warm-up budget (its first 14 days)", WARMUP_DAILY, DAY, n, at)?;
            }
            Ok(())
        }
    }
}

/// The most an App may keep waiting in scheduled sends: one day's budget
/// for its scope (M1).
pub fn pending_cap(kind: &Kind<'_>) -> i64 {
    match kind {
        Kind::System => 0,
        Kind::App { scope_any, warmup_domain, .. } => {
            let mut cap = APP_DAILY;
            if *scope_any {
                cap = cap.min(ANY_DAILY);
            }
            if warmup_domain.is_some() {
                cap = cap.min(WARMUP_DAILY);
            }
            cap
        }
    }
}

/// Would `n` more scheduled recipients fit what an App may keep waiting?
pub fn check_pending(kind: &Kind<'_>, n: usize) -> R<()> {
    let Kind::App { app, .. } = kind else { return Err(fail(400, "usage", "K2's own mail is never scheduled")) };
    let cap = pending_cap(kind);
    let waiting = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::scheduled_pending(&conn, app)
            .map_err(|e| fail(503, "failed", format!("K2 couldn't read the send budget, so it didn't send (fail-closed): {e}")))?
    };
    if waiting + n as i64 > cap {
        let mut f = fail(
            429,
            "rate_limited",
            format!("this App already has {waiting} recipients waiting in scheduled email (at most {cap}, one day's budget); send some first"),
        );
        f.retry_after_ms = Some(HOUR as u64 * 1000);
        return Err(f);
    }
    Ok(())
}
