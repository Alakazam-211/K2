//! Budgets (prd-email-notifications-core-v1 E17, Q12). Counted in
//! **envelope recipients** over the log's rows (restart-proof), never in
//! calls; scheduled and held sends count when they are made; idempotent
//! replays don't count again (they never reach here). An unreadable count
//! refuses (fail-closed).
//!
//! | bucket | limit |
//! |---|---|
//! | App (`users` scope), its own sends and its agent's | 100/h, 1,000/day |
//! | App with scope `any` | 30/h, 300/day |
//! | warm-up: App sends from a domain verified < 14 days ago (relay: a sender < 14 days old) | 20/h, 200/day |
//! | K2's system mail (exempt from warm-up) | 200/h |
//! | everything on the box | 1,000/h |

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

fn limited(what: &str, used: i64, limit: i64, window: i64, oldest: Option<i64>, now: i64) -> Fail {
    let retry_secs = oldest.map(|o| (o + window - now).max(1)).unwrap_or(window);
    let mut f = fail(
        429,
        "rate_limited",
        format!("{what}: {used} of {limit} recipients in the last {}; try again later", if window == HOUR { "hour" } else { "24 hours" }),
    );
    f.retry_after_ms = Some((retry_secs as u64).saturating_mul(1000));
    f
}

fn check_one(conn: &rusqlite::Connection, b: &Bucket<'_>, what: &str, limit: i64, window: i64, n: i64, now: i64) -> R<()> {
    let (used, oldest) = store::sum_recipients(conn, b, now - window)
        .map_err(|e| fail(503, "failed", format!("K2 couldn't read the send budget, so it didn't send (fail-closed): {e}")))?;
    if used + n > limit {
        return Err(limited(what, used, limit, window, oldest, now));
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

/// Would `n` more recipients fit every bucket this send counts in?
pub fn check(kind: &Kind<'_>, n: usize, now: i64) -> R<()> {
    let n = n as i64;
    let db = k2_core::db::shared();
    let conn = db.lock();
    check_one(&conn, &Bucket::Box, "this server's notification budget", BOX_HOURLY, HOUR, n, now)?;
    match kind {
        Kind::System => check_one(&conn, &Bucket::System, "K2's system mail budget", SYSTEM_HOURLY, HOUR, n, now),
        Kind::App { app, scope_any, warmup_domain } => {
            check_one(&conn, &Bucket::App(app), "this App's email budget", APP_HOURLY, HOUR, n, now)?;
            check_one(&conn, &Bucket::App(app), "this App's email budget", APP_DAILY, DAY, n, now)?;
            if *scope_any {
                check_one(&conn, &Bucket::AppAny(app), "this App's any-address budget", ANY_HOURLY, HOUR, n, now)?;
                check_one(&conn, &Bucket::AppAny(app), "this App's any-address budget", ANY_DAILY, DAY, n, now)?;
            }
            if let Some(d) = warmup_domain {
                check_one(&conn, &Bucket::Domain(d), "the new domain's warm-up budget (its first 14 days)", WARMUP_HOURLY, HOUR, n, now)?;
                check_one(&conn, &Bucket::Domain(d), "the new domain's warm-up budget (its first 14 days)", WARMUP_DAILY, DAY, n, now)?;
            }
            Ok(())
        }
    }
}
