//! Delivery outcomes, suppressions and sender pauses: the k2.db store
//! (migration 0245, prd-email-notifications-part2-v1 §4 N6).
//!
//! The daemon's `email_notify::outcomes` reads the reports and applies the
//! rules; this module is only the rows and the counts.
//!
//! - **Outcomes**: one row per (log row, recipient, kind); a repeat report
//!   is a no-op. Recipients are SHA-256 of the lower-case address plus the
//!   domain, never the address.
//! - **Suppressions**: `box` (a hard bounce: nobody here sends to it),
//!   `sender` (a complaint or an unsubscribe: that sender doesn't), `app`
//!   (later). Manual entries are the owner's.
//! - **Pauses**: one live pause per sender at most; a resume closes it and
//!   starts a new counting window.
//!
//! Nothing here logs an address.

use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};

pub const K_DELIVERED: &str = "delivered";
pub const K_DELAYED: &str = "delayed";
pub const K_HARD: &str = "bounced_hard";
pub const K_SOFT: &str = "bounced_soft";
pub const K_COMPLAINED: &str = "complained";
pub const K_SUPPRESSED: &str = "suppressed";

pub const SRC_DSN: &str = "dsn";
pub const SRC_ARF: &str = "arf";
pub const SRC_WEBHOOK: &str = "webhook";
pub const SRC_K2: &str = "k2";

pub const SCOPE_BOX: &str = "box";
pub const SCOPE_SENDER: &str = "sender";
pub const SCOPE_APP: &str = "app";

pub const R_HARD: &str = "hard_bounce";
pub const R_COMPLAINT: &str = "complaint";
pub const R_UNSUBSCRIBE: &str = "unsubscribe";
pub const R_MANUAL: &str = "manual";

/// The auto-pause window (core Q11).
pub const WINDOW_SECS: i64 = 24 * 3600;
/// No pause below this many recipients in the window.
pub const MIN_SENT: i64 = 50;

fn err(what: &str) -> impl Fn(rusqlite::Error) -> String + '_ {
    move |e| format!("email outcomes {what}: {e}")
}

pub fn address_sha256(address: &str) -> String {
    Sha256::digest(address.trim().to_ascii_lowercase().as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

pub fn domain_of(address: &str) -> String {
    address.trim().rsplit_once('@').map(|(_, d)| d.to_ascii_lowercase()).unwrap_or_default()
}

/// Record one outcome. `Ok(false)`: already recorded.
pub fn record(conn: &Connection, log_id: &str, sender_id: &str, rcpt: &str, kind: &str, code: Option<&str>, source: &str, at: i64) -> Result<bool, String> {
    let n = conn
        .execute(
            "INSERT OR IGNORE INTO email_notify_outcomes (log_id, sender_id, rcpt_sha256, rcpt_domain, kind, code, source, at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![log_id, sender_id, address_sha256(rcpt), domain_of(rcpt), kind, code, source, at],
        )
        .map_err(err("record"))?;
    Ok(n == 1)
}

/// `(rcpt_sha256, kind)` for one log row.
pub fn for_log(conn: &Connection, log_id: &str) -> Result<Vec<(String, String)>, String> {
    let mut st = conn
        .prepare("SELECT rcpt_sha256, kind FROM email_notify_outcomes WHERE log_id = ?1 ORDER BY id")
        .map_err(err("for log"))?;
    let rows = st.query_map(params![log_id], |r| Ok((r.get(0)?, r.get(1)?))).map_err(err("for log"))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(err("for log"))
}

/// `(bounced recipients, complained recipients)` for one log row.
pub fn counts_for_log(conn: &Connection, log_id: &str) -> Result<(i64, i64), String> {
    conn.query_row(
        "SELECT COUNT(DISTINCT CASE WHEN kind = 'bounced_hard' OR kind = 'bounced_soft' THEN rcpt_sha256 END), \
                COUNT(DISTINCT CASE WHEN kind = 'complained' THEN rcpt_sha256 END) \
         FROM email_notify_outcomes WHERE log_id = ?1",
        params![log_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .map_err(err("counts"))
}

/// The rate inputs for one sender since `from`: `(recipients handed
/// off, hard-bounced recipients, complaining recipients)`.
pub fn window(conn: &Connection, sender_id: &str, from: i64) -> Result<(i64, i64, i64), String> {
    let sent: i64 = conn
        .query_row(
            "SELECT COALESCE(SUM(recipient_count), 0) FROM email_notify_log \
             WHERE sender_id = ?1 AND status = 'submitted' AND updated_at >= ?2",
            params![sender_id, from],
            |r| r.get(0),
        )
        .map_err(err("window sent"))?;
    let (hard, complained): (i64, i64) = conn
        .query_row(
            "SELECT COUNT(DISTINCT CASE WHEN kind = 'bounced_hard' THEN log_id || ':' || rcpt_sha256 END), \
                    COUNT(DISTINCT CASE WHEN kind = 'complained' THEN log_id || ':' || rcpt_sha256 END) \
             FROM email_notify_outcomes WHERE sender_id = ?1 AND at >= ?2",
            params![sender_id, from],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(err("window outcomes"))?;
    Ok((sent, hard, complained))
}

/// Pause when hard bounces are over 5% or complaints over 0.1% of at
/// least [`MIN_SENT`] recipients. `Some(reason)` when it trips.
pub fn trips(sent: i64, hard: i64, complained: i64) -> Option<&'static str> {
    if sent < MIN_SENT {
        return None;
    }
    if hard * 100 > sent * 5 {
        return Some("hard_bounces");
    }
    if complained * 1000 > sent {
        return Some("complaints");
    }
    None
}

// ── suppressions ─────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suppression {
    pub address_sha256: String,
    pub domain: String,
    pub scope: String,
    pub scope_ref: String,
    pub reason: String,
    pub created_at: i64,
    pub created_by: String,
}

fn sup_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Suppression> {
    Ok(Suppression {
        address_sha256: r.get(0)?,
        domain: r.get(1)?,
        scope: r.get(2)?,
        scope_ref: r.get(3)?,
        reason: r.get(4)?,
        created_at: r.get(5)?,
        created_by: r.get(6)?,
    })
}

/// Add a suppression. `Ok(false)`: it was already there.
pub fn suppress(conn: &Connection, address: &str, scope: &str, scope_ref: &str, reason: &str, by: &str, now: i64) -> Result<bool, String> {
    let scope_ref = if scope == SCOPE_BOX { "" } else { scope_ref };
    let n = conn
        .execute(
            "INSERT OR IGNORE INTO email_suppressions (address_sha256, domain, scope, scope_ref, reason, created_at, created_by) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![address_sha256(address), domain_of(address), scope, scope_ref, reason, now, by],
        )
        .map_err(err("suppress"))?;
    Ok(n == 1)
}

/// Remove suppressions of `address`: one scope, or every scope.
pub fn unsuppress(conn: &Connection, address: &str, scope: Option<(&str, &str)>) -> Result<usize, String> {
    let h = address_sha256(address);
    match scope {
        Some((s, r)) => conn.execute(
            "DELETE FROM email_suppressions WHERE address_sha256 = ?1 AND scope = ?2 AND scope_ref = ?3",
            params![h, s, if s == SCOPE_BOX { "" } else { r }],
        ),
        None => conn.execute("DELETE FROM email_suppressions WHERE address_sha256 = ?1", params![h]),
    }
    .map_err(err("unsuppress"))
}

/// Is `address` suppressed for mail from `sender_id` (and `app`)?
/// Returns the reason. Fail closed: an unreadable store suppresses.
pub fn suppressed_for(conn: &Connection, address: &str, sender_id: &str, app: Option<&str>) -> Option<String> {
    conn.query_row(
        "SELECT reason FROM email_suppressions WHERE address_sha256 = ?1 AND \
           (scope = 'box' OR (scope = 'sender' AND scope_ref = ?2) OR (scope = 'app' AND scope_ref = ?3)) \
         ORDER BY CASE scope WHEN 'box' THEN 0 ELSE 1 END LIMIT 1",
        params![address_sha256(address), sender_id, app.unwrap_or("")],
        |r| r.get::<_, String>(0),
    )
    .optional()
    .unwrap_or_else(|_| Some("unreadable".to_string()))
}

/// Suppressions, newest first; `address` narrows to one address.
pub fn list_suppressions(conn: &Connection, address: Option<&str>, limit: usize) -> Result<Vec<Suppression>, String> {
    let sql = "SELECT address_sha256, domain, scope, scope_ref, reason, created_at, created_by FROM email_suppressions";
    let mut out = Vec::new();
    match address {
        Some(a) => {
            let mut st = conn
                .prepare(&format!("{sql} WHERE address_sha256 = ?1 ORDER BY created_at DESC LIMIT ?2"))
                .map_err(err("list"))?;
            for r in st.query_map(params![address_sha256(a), limit as i64], sup_row).map_err(err("list"))? {
                out.push(r.map_err(err("list"))?);
            }
        }
        None => {
            let mut st = conn.prepare(&format!("{sql} ORDER BY created_at DESC LIMIT ?1")).map_err(err("list"))?;
            for r in st.query_map(params![limit as i64], sup_row).map_err(err("list"))? {
                out.push(r.map_err(err("list"))?);
            }
        }
    }
    Ok(out)
}

// ── pauses ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pause {
    pub id: i64,
    pub sender_id: String,
    pub paused_at: i64,
    pub reason: String,
    pub rate_json: String,
    pub resumed_at: Option<i64>,
    pub resumed_by: Option<String>,
}

const PAUSE_COLS: &str = "id, sender_id, paused_at, reason, rate_json, resumed_at, resumed_by";

fn pause_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Pause> {
    Ok(Pause {
        id: r.get(0)?,
        sender_id: r.get(1)?,
        paused_at: r.get(2)?,
        reason: r.get(3)?,
        rate_json: r.get(4)?,
        resumed_at: r.get(5)?,
        resumed_by: r.get(6)?,
    })
}

/// The sender's live pause, if any. Fail closed: an unreadable store is
/// treated as paused by the caller ([`Err`]).
pub fn live_pause(conn: &Connection, sender_id: &str) -> Result<Option<Pause>, String> {
    conn.query_row(
        &format!("SELECT {PAUSE_COLS} FROM email_sender_pauses WHERE sender_id = ?1 AND resumed_at IS NULL"),
        params![sender_id],
        pause_row,
    )
    .optional()
    .map_err(err("pause"))
}

/// When the sender's counting window starts: the later of `now - 24 h`
/// and its last resume.
pub fn window_start(conn: &Connection, sender_id: &str, now: i64) -> Result<i64, String> {
    let last: Option<i64> = conn
        .query_row("SELECT MAX(resumed_at) FROM email_sender_pauses WHERE sender_id = ?1", params![sender_id], |r| r.get(0))
        .map_err(err("window start"))?;
    // Strictly after the resume (second resolution): what was counted
    // before it never counts again.
    Ok(last.map_or(now - WINDOW_SECS, |r| (r + 1).max(now - WINDOW_SECS)))
}

/// Pause a sender. `Ok(None)`: it was already paused.
pub fn pause(conn: &Connection, sender_id: &str, reason: &str, rate_json: &str, now: i64) -> Result<Option<Pause>, String> {
    let n = conn
        .execute(
            "INSERT OR IGNORE INTO email_sender_pauses (sender_id, paused_at, reason, rate_json) VALUES (?1, ?2, ?3, ?4)",
            params![sender_id, now, reason, rate_json],
        )
        .map_err(err("pause"))?;
    if n == 0 {
        return Ok(None);
    }
    live_pause(conn, sender_id)
}

/// Resume a paused sender. `Ok(false)`: it wasn't paused.
pub fn resume(conn: &Connection, sender_id: &str, by: &str, now: i64) -> Result<bool, String> {
    let n = conn
        .execute(
            "UPDATE email_sender_pauses SET resumed_at = ?2, resumed_by = ?3 WHERE sender_id = ?1 AND resumed_at IS NULL",
            params![sender_id, now, by],
        )
        .map_err(err("resume"))?;
    Ok(n == 1)
}

/// EN-A21: an App user's removal forgets their outcome rows (the
/// suppression, a hash only, stays).
pub fn forget_recipient(conn: &Connection, address: &str) -> Result<usize, String> {
    conn.execute("DELETE FROM email_notify_outcomes WHERE rcpt_sha256 = ?1", params![address_sha256(address)])
        .map_err(err("forget"))
}

/// Outcome rows whose log row is gone (retention).
pub fn purge_orphans(conn: &Connection) -> Result<usize, String> {
    conn.execute("DELETE FROM email_notify_outcomes WHERE log_id NOT IN (SELECT id FROM email_notify_log)", [])
        .map_err(err("purge"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().expect("mem");
        for f in [
            include_str!("../drizzle_sql/0227_email_notifications.sql"),
            include_str!("../drizzle_sql/0245_email_notify_outcomes.sql"),
        ] {
            for stmt in f.split("--> statement-breakpoint") {
                c.execute_batch(stmt).expect("schema");
            }
        }
        c
    }

    #[test]
    fn outcomes_are_hashed_once_per_kind_and_counted_per_row() {
        let c = conn();
        assert!(record(&c, "enl_1", "s1", "A@X.test", K_HARD, Some("5.1.1"), SRC_DSN, 10).unwrap());
        assert!(!record(&c, "enl_1", "s1", "a@x.test", K_HARD, Some("5.1.1"), SRC_DSN, 11).unwrap(), "a repeat report is a no-op");
        record(&c, "enl_1", "s1", "b@y.test", K_COMPLAINED, None, SRC_ARF, 12).unwrap();
        record(&c, "enl_1", "s1", "c@y.test", K_DELIVERED, None, SRC_WEBHOOK, 12).unwrap();
        let rows = for_log(&c, "enl_1").unwrap();
        assert_eq!(rows.len(), 3);
        assert!(rows.iter().all(|(h, _)| h.len() == 64 && !h.contains('@')), "hashes only");
        assert_eq!(counts_for_log(&c, "enl_1").unwrap(), (1, 1));
        let dom: String = c.query_row("SELECT rcpt_domain FROM email_notify_outcomes WHERE kind = 'bounced_hard'", [], |r| r.get(0)).unwrap();
        assert_eq!(dom, "x.test");
        assert_eq!(forget_recipient(&c, "a@x.test").unwrap(), 1);
    }

    #[test]
    fn the_pause_rule_needs_fifty_and_five_percent_or_a_tenth_of_a_percent() {
        assert_eq!(trips(49, 49, 49), None, "under the minimum");
        assert_eq!(trips(100, 5, 0), None, "exactly 5% is not over");
        assert_eq!(trips(100, 6, 0), Some("hard_bounces"));
        assert_eq!(trips(1000, 0, 1), None, "exactly 0.1%");
        assert_eq!(trips(1000, 0, 2), Some("complaints"));
        assert_eq!(trips(80, 7, 0), Some("hard_bounces"), "the PRD's 7 of 80");
    }

    #[test]
    fn suppressions_by_scope_and_one_live_pause_with_a_fresh_window() {
        let c = conn();
        assert!(suppress(&c, "Hard@X.test", SCOPE_BOX, "ignored", R_HARD, "dsn", 1).unwrap());
        assert!(!suppress(&c, "hard@x.test", SCOPE_BOX, "", R_HARD, "dsn", 2).unwrap());
        suppress(&c, "c@x.test", SCOPE_SENDER, "s1", R_COMPLAINT, "arf", 3).unwrap();
        assert_eq!(suppressed_for(&c, "hard@x.test", "any", None).as_deref(), Some(R_HARD), "box-wide");
        assert_eq!(suppressed_for(&c, "c@x.test", "s1", None).as_deref(), Some(R_COMPLAINT));
        assert_eq!(suppressed_for(&c, "c@x.test", "s2", None), None, "another sender may");
        assert_eq!(list_suppressions(&c, Some("C@x.test"), 10).unwrap().len(), 1);
        assert_eq!(list_suppressions(&c, None, 10).unwrap().len(), 2);
        assert_eq!(unsuppress(&c, "c@x.test", Some((SCOPE_SENDER, "s1"))).unwrap(), 1);
        assert_eq!(unsuppress(&c, "hard@x.test", None).unwrap(), 1);
        // Pauses.
        assert_eq!(window_start(&c, "s1", 100_000).unwrap(), 100_000 - WINDOW_SECS);
        let p = pause(&c, "s1", "hard_bounces", "{}", 50_000).unwrap().expect("paused");
        assert_eq!(p.reason, "hard_bounces");
        assert!(pause(&c, "s1", "complaints", "{}", 50_001).unwrap().is_none(), "one live pause");
        assert!(resume(&c, "s1", "owner", 60_000).unwrap());
        assert!(!resume(&c, "s1", "owner", 60_001).unwrap());
        assert!(live_pause(&c, "s1").unwrap().is_none());
        assert_eq!(window_start(&c, "s1", 70_000).unwrap(), 60_001, "a resume starts a new window");
    }
}
