//! Verified addresses: the k2.db store (migration 0244,
//! prd-email-notifications-part2-v1 §4 V).
//!
//! K2 mails an address on someone's behalf (a person's agent notices, a
//! mailbox's forward) only after the address's owner clicked a confirm
//! link. This module is the rows and the token rules; the daemon's
//! `email_notify::verify` sends the mail and serves the page.
//!
//! - A **request** ([`issue`]) makes one `pending` row with a confirm token
//!   (`k2ev_…`, valid [`LINK_SECS`]) and a stop token (`k2es_…`). Only
//!   their SHA-256 is kept. A re-ask for the same address while pending
//!   replaces the row (the old link dies); after expiry it is a new row.
//! - **Confirm** ([`confirm`]) takes the confirm token once. For
//!   `person_notice` a person has one notice address: confirming a new one
//!   retires the old one (`revoked_by = replaced`).
//! - **Stop** ([`stop`]): every mail to the address carries its own stop
//!   token ([`mint_stop`]); any of them revokes the address
//!   (`revoked_by = recipient`). The subject, the owner or an Admin revoke
//!   with [`revoke`].
//! - **Limits**: [`MAX_PENDING_PER_SUBJECT`] pending per (purpose, subject),
//!   [`MAX_MAILS_PER_HOUR`] verify mails per box per hour.
//!
//! Nothing here logs an address.

use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};

pub const PURPOSE_PERSON_NOTICE: &str = "person_notice";
pub const PURPOSE_FORWARD: &str = "forward";

pub const ST_PENDING: &str = "pending";
pub const ST_VERIFIED: &str = "verified";
pub const ST_REVOKED: &str = "revoked";
pub const ST_EXPIRED: &str = "expired";

/// A confirm link works this long.
pub const LINK_SECS: i64 = 24 * 3600;
/// Pending requests per (purpose, subject).
pub const MAX_PENDING_PER_SUBJECT: i64 = 3;
/// Verify mails per box per hour.
pub const MAX_MAILS_PER_HOUR: i64 = 10;

pub const CONFIRM_PREFIX: &str = "k2ev_";
pub const STOP_PREFIX: &str = "k2es_";

/// `revoked_by` values K2 writes itself.
pub const BY_RECIPIENT: &str = "recipient";
pub const BY_REPLACED: &str = "replaced";
pub const BY_REISSUED: &str = "reissued";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verified {
    pub id: String,
    pub address: String,
    pub purpose: String,
    pub subject_ref: String,
    pub requested_by: String,
    pub state: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub verified_at: Option<i64>,
    pub revoked_at: Option<i64>,
    pub revoked_by: Option<String>,
}

impl Verified {
    pub fn is_verified(&self) -> bool {
        self.state == ST_VERIFIED
    }
}

/// A new request: the row plus the two tokens, shown once.
#[derive(Debug, Clone)]
pub struct Issued {
    pub row: Verified,
    pub token: String,
    pub stop: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IssueError {
    /// The address is already verified for this subject (no mail sent).
    AlreadyVerified(Verified),
    /// A limit: (code, hint).
    Limit(&'static str, String),
    Bad(String),
    Db(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmError {
    /// Unknown, used or revoked token.
    Invalid,
    Expired,
    Db(String),
}

fn err(what: &str) -> impl Fn(rusqlite::Error) -> String + '_ {
    move |e| format!("verified addresses {what}: {e}")
}

pub fn sha256_hex(s: &str) -> String {
    Sha256::digest(s.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

fn random_b64(n: usize) -> Result<String, String> {
    use base64::Engine as _;
    let mut b = vec![0u8; n];
    getrandom::getrandom(&mut b).map_err(|e| format!("random: {e}"))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b))
}

fn new_token(prefix: &str) -> Result<String, String> {
    Ok(format!("{prefix}{}", random_b64(32)?))
}

/// Does `t` look like a token K2 made (`prefix` + 43 base64url chars)?
/// Anything else is refused before the store is read, and is safe to put
/// back into a page.
pub fn token_shape_ok(t: &str, prefix: &str) -> bool {
    t.strip_prefix(prefix)
        .is_some_and(|rest| rest.len() == 43 && rest.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'))
}

pub fn normalize_address(a: &str) -> String {
    a.trim().to_ascii_lowercase()
}

const COLS: &str = "id, address, purpose, subject_ref, requested_by, state, created_at, expires_at, verified_at, revoked_at, revoked_by";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Verified> {
    Ok(Verified {
        id: r.get(0)?,
        address: r.get(1)?,
        purpose: r.get(2)?,
        subject_ref: r.get(3)?,
        requested_by: r.get(4)?,
        state: r.get(5)?,
        created_at: r.get(6)?,
        expires_at: r.get(7)?,
        verified_at: r.get(8)?,
        revoked_at: r.get(9)?,
        revoked_by: r.get(10)?,
    })
}

pub fn valid_purpose(p: &str) -> bool {
    p == PURPOSE_PERSON_NOTICE || p == PURPOSE_FORWARD
}

/// Mark every pending row past its link expiry `expired` (its token dies).
pub fn expire_due(conn: &Connection, now: i64) -> Result<usize, String> {
    conn.execute(
        "UPDATE email_verified_addresses SET state = 'expired', token_sha256 = NULL \
         WHERE state = 'pending' AND expires_at <= ?1",
        params![now],
    )
    .map_err(err("expire"))
}

pub fn get(conn: &Connection, id: &str) -> Result<Option<Verified>, String> {
    conn.query_row(&format!("SELECT {COLS} FROM email_verified_addresses WHERE id = ?1"), params![id], row)
        .optional()
        .map_err(err("get"))
}

/// The live (pending or verified) rows of one subject, newest first.
pub fn live_for(conn: &Connection, purpose: &str, subject_ref: &str, now: i64) -> Result<Vec<Verified>, String> {
    expire_due(conn, now)?;
    let mut st = conn
        .prepare(&format!(
            "SELECT {COLS} FROM email_verified_addresses \
             WHERE purpose = ?1 AND subject_ref = ?2 AND state IN ('pending', 'verified') ORDER BY created_at DESC, id DESC"
        ))
        .map_err(err("list"))?;
    let rows = st.query_map(params![purpose, subject_ref], row).map_err(err("list"))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(err("list"))
}

/// The subject's verified address for `purpose` (the newest).
pub fn verified_for(conn: &Connection, purpose: &str, subject_ref: &str) -> Result<Option<Verified>, String> {
    conn.query_row(
        &format!(
            "SELECT {COLS} FROM email_verified_addresses WHERE purpose = ?1 AND subject_ref = ?2 AND state = 'verified' \
             ORDER BY verified_at DESC, id DESC LIMIT 1"
        ),
        params![purpose, subject_ref],
        row,
    )
    .optional()
    .map_err(err("verified"))
}

/// Is `address` verified for (purpose, subject)? Fail closed.
pub fn is_verified(conn: &Connection, purpose: &str, subject_ref: &str, address: &str) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM email_verified_addresses \
         WHERE purpose = ?1 AND subject_ref = ?2 AND address = ?3 AND state = 'verified'",
        params![purpose, subject_ref, normalize_address(address)],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n > 0)
    .unwrap_or(false)
}

/// A new confirm request for `address` (already checked by the caller as
/// an address K2 may mail). See the module doc for the rules.
pub fn issue(
    conn: &Connection,
    purpose: &str,
    subject_ref: &str,
    address: &str,
    requested_by: &str,
    now: i64,
) -> Result<Issued, IssueError> {
    if !valid_purpose(purpose) {
        return Err(IssueError::Bad(format!("unknown purpose {purpose}")));
    }
    let subject_ref = subject_ref.trim();
    if subject_ref.is_empty() {
        return Err(IssueError::Bad("no subject".into()));
    }
    let address = normalize_address(address);
    expire_due(conn, now).map_err(IssueError::Db)?;
    let live = live_for(conn, purpose, subject_ref, now).map_err(IssueError::Db)?;
    if let Some(v) = live.iter().find(|v| v.address == address && v.is_verified()) {
        return Err(IssueError::AlreadyVerified(v.clone()));
    }
    let same_pending = live.iter().find(|v| v.address == address && v.state == ST_PENDING).cloned();
    let other_pending = live.iter().filter(|v| v.state == ST_PENDING && v.address != address).count() as i64;
    if other_pending >= MAX_PENDING_PER_SUBJECT {
        return Err(IssueError::Limit(
            "too_many_pending",
            format!("{MAX_PENDING_PER_SUBJECT} addresses are already waiting for a confirm; confirm one or wait for its link to expire"),
        ));
    }
    let hour: i64 = conn
        .query_row("SELECT COUNT(*) FROM email_verified_addresses WHERE created_at > ?1", params![now - 3600], |r| r.get(0))
        .map_err(|e| IssueError::Db(err("rate")(e)))?;
    if hour >= MAX_MAILS_PER_HOUR {
        return Err(IssueError::Limit(
            "rate_limited",
            format!("this server sends at most {MAX_MAILS_PER_HOUR} confirm emails an hour; try again later"),
        ));
    }
    let token = new_token(CONFIRM_PREFIX).map_err(IssueError::Db)?;
    let stop = new_token(STOP_PREFIX).map_err(IssueError::Db)?;
    let id = format!("eva_{}", random_b64(12).map_err(IssueError::Db)?);
    let tx = conn.unchecked_transaction().map_err(|e| IssueError::Db(err("tx")(e)))?;
    if let Some(old) = same_pending {
        tx.execute(
            "UPDATE email_verified_addresses SET state = 'revoked', token_sha256 = NULL, revoked_at = ?2, revoked_by = ?3 WHERE id = ?1",
            params![old.id, now, BY_REISSUED],
        )
        .map_err(|e| IssueError::Db(err("reissue")(e)))?;
    }
    tx.execute(
        "INSERT INTO email_verified_addresses (id, address, purpose, subject_ref, requested_by, token_sha256, state, created_at, expires_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', ?7, ?8)",
        params![id, address, purpose, subject_ref, requested_by, sha256_hex(&token), now, now + LINK_SECS],
    )
    .map_err(|e| IssueError::Db(err("insert")(e)))?;
    tx.execute(
        "INSERT INTO email_verified_stops (stop_sha256, address_id, created_at) VALUES (?1, ?2, ?3)",
        params![sha256_hex(&stop), id, now],
    )
    .map_err(|e| IssueError::Db(err("stop")(e)))?;
    tx.commit().map_err(|e| IssueError::Db(err("commit")(e)))?;
    let row = get(conn, &id).map_err(IssueError::Db)?.ok_or_else(|| IssueError::Db("the new row is gone".into()))?;
    Ok(Issued { row, token, stop })
}

/// A new stop token for a later mail to a live address.
pub fn mint_stop(conn: &Connection, address_id: &str, now: i64) -> Result<String, String> {
    let stop = new_token(STOP_PREFIX)?;
    conn.execute(
        "INSERT INTO email_verified_stops (stop_sha256, address_id, created_at) VALUES (?1, ?2, ?3)",
        params![sha256_hex(&stop), address_id, now],
    )
    .map_err(err("mint stop"))?;
    Ok(stop)
}

/// The pending row a confirm token names (read-only; for the page).
pub fn peek(conn: &Connection, token: &str, now: i64) -> Result<Option<Verified>, String> {
    if !token_shape_ok(token, CONFIRM_PREFIX) {
        return Ok(None);
    }
    let v = conn
        .query_row(
            &format!("SELECT {COLS} FROM email_verified_addresses WHERE token_sha256 = ?1 AND state = 'pending'"),
            params![sha256_hex(token)],
            row,
        )
        .optional()
        .map_err(err("peek"))?;
    Ok(v.filter(|v| v.expires_at > now))
}

/// Take a confirm token (POST only).
pub fn confirm(conn: &Connection, token: &str, now: i64) -> Result<Verified, ConfirmError> {
    if !token_shape_ok(token, CONFIRM_PREFIX) {
        return Err(ConfirmError::Invalid);
    }
    let v = conn
        .query_row(
            &format!("SELECT {COLS} FROM email_verified_addresses WHERE token_sha256 = ?1"),
            params![sha256_hex(token)],
            row,
        )
        .optional()
        .map_err(|e| ConfirmError::Db(err("confirm")(e)))?
        .ok_or(ConfirmError::Invalid)?;
    if v.state != ST_PENDING {
        return Err(ConfirmError::Invalid);
    }
    if v.expires_at <= now {
        expire_due(conn, now).map_err(ConfirmError::Db)?;
        return Err(ConfirmError::Expired);
    }
    let tx = conn.unchecked_transaction().map_err(|e| ConfirmError::Db(err("tx")(e)))?;
    if v.purpose == PURPOSE_PERSON_NOTICE {
        tx.execute(
            "UPDATE email_verified_addresses SET state = 'revoked', token_sha256 = NULL, verified_at = NULL, revoked_at = ?3, revoked_by = ?4 \
             WHERE purpose = ?1 AND subject_ref = ?2 AND state = 'verified'",
            params![v.purpose, v.subject_ref, now, BY_REPLACED],
        )
        .map_err(|e| ConfirmError::Db(err("replace")(e)))?;
    }
    let n = tx
        .execute(
            "UPDATE email_verified_addresses SET state = 'verified', token_sha256 = NULL, verified_at = ?2 WHERE id = ?1 AND state = 'pending'",
            params![v.id, now],
        )
        .map_err(|e| ConfirmError::Db(err("confirm")(e)))?;
    if n != 1 {
        return Err(ConfirmError::Invalid);
    }
    tx.commit().map_err(|e| ConfirmError::Db(err("commit")(e)))?;
    get(conn, &v.id).map_err(ConfirmError::Db)?.ok_or(ConfirmError::Invalid)
}

/// The live row a stop token names (read-only; for the page).
pub fn peek_stop(conn: &Connection, stop: &str) -> Result<Option<Verified>, String> {
    if !token_shape_ok(stop, STOP_PREFIX) {
        return Ok(None);
    }
    conn.query_row(
        &format!(
            "SELECT {} FROM email_verified_addresses a JOIN email_verified_stops s ON s.address_id = a.id \
             WHERE s.stop_sha256 = ?1",
            COLS.split(", ").map(|c| format!("a.{c}")).collect::<Vec<_>>().join(", ")
        ),
        params![sha256_hex(stop)],
        row,
    )
    .optional()
    .map_err(err("peek stop"))
}

/// Take a stop token: the address is revoked (pending or verified).
/// `Ok(None)`: no such token. A second stop answers the same row again.
pub fn stop(conn: &Connection, stop: &str, now: i64) -> Result<Option<Verified>, String> {
    let Some(v) = peek_stop(conn, stop)? else { return Ok(None) };
    if v.state == ST_PENDING || v.state == ST_VERIFIED {
        revoke(conn, &v.id, BY_RECIPIENT, now)?;
    }
    get(conn, &v.id)
}

/// Revoke a live row. `Ok(false)`: it was not live.
pub fn revoke(conn: &Connection, id: &str, by: &str, now: i64) -> Result<bool, String> {
    let n = conn
        .execute(
            "UPDATE email_verified_addresses SET state = 'revoked', token_sha256 = NULL, verified_at = NULL, revoked_at = ?2, revoked_by = ?3 \
             WHERE id = ?1 AND state IN ('pending', 'verified')",
            params![id, now, by],
        )
        .map_err(err("revoke"))?;
    Ok(n == 1)
}

/// Forget dead rows (revoked or expired) older than `days`; their stop
/// tokens go with them.
pub fn purge_dead(conn: &Connection, older_than: i64) -> Result<usize, String> {
    conn.execute(
        "DELETE FROM email_verified_stops WHERE address_id IN (SELECT id FROM email_verified_addresses \
         WHERE state IN ('revoked', 'expired') AND COALESCE(revoked_at, expires_at) < ?1)",
        params![older_than],
    )
    .map_err(err("purge stops"))?;
    conn.execute(
        "DELETE FROM email_verified_addresses WHERE state IN ('revoked', 'expired') AND COALESCE(revoked_at, expires_at) < ?1",
        params![older_than],
    )
    .map_err(err("purge"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().expect("mem");
        c.execute_batch("PRAGMA foreign_keys = ON;").expect("fk");
        for stmt in include_str!("../drizzle_sql/0244_email_verified_addresses.sql").split("--> statement-breakpoint") {
            c.execute_batch(stmt).expect("0244");
        }
        c
    }

    #[test]
    fn a_request_confirms_once_by_post_and_keeps_only_hashes() {
        let c = conn();
        let i = issue(&c, PURPOSE_PERSON_NOTICE, "owner", " Me@Example.Test ", "owner", 1000).expect("issue");
        assert!(token_shape_ok(&i.token, CONFIRM_PREFIX) && token_shape_ok(&i.stop, STOP_PREFIX));
        assert_eq!(i.row.address, "me@example.test");
        assert_eq!(i.row.state, ST_PENDING);
        assert_eq!(i.row.expires_at, 1000 + LINK_SECS);
        let raw: String = c.query_row("SELECT token_sha256 FROM email_verified_addresses", [], |r| r.get(0)).expect("hash");
        assert_eq!(raw, sha256_hex(&i.token), "only the hash is stored");
        assert!(!raw.contains(&i.token));
        assert_eq!(peek(&c, &i.token, 1001).expect("peek").expect("row").id, i.row.id);
        assert!(!is_verified(&c, PURPOSE_PERSON_NOTICE, "owner", "me@example.test"));
        let v = confirm(&c, &i.token, 1002).expect("confirm");
        assert!(v.is_verified() && v.verified_at == Some(1002));
        assert!(is_verified(&c, PURPOSE_PERSON_NOTICE, "owner", "ME@example.test"));
        assert_eq!(confirm(&c, &i.token, 1003), Err(ConfirmError::Invalid), "once");
        assert_eq!(peek(&c, &i.token, 1003).expect("peek"), None);
        assert_eq!(confirm(&c, "k2ev_short", 1003), Err(ConfirmError::Invalid));
        // Asking again for a verified address sends nothing.
        assert!(matches!(issue(&c, PURPOSE_PERSON_NOTICE, "owner", "me@example.test", "owner", 1004), Err(IssueError::AlreadyVerified(_))));
    }

    #[test]
    fn links_expire_and_a_re_ask_replaces_the_pending_row() {
        let c = conn();
        let a = issue(&c, PURPOSE_PERSON_NOTICE, "ann", "a@x.test", "ann", 0).expect("a");
        let b = issue(&c, PURPOSE_PERSON_NOTICE, "ann", "a@x.test", "ann", 10).expect("re-ask");
        assert_eq!(confirm(&c, &a.token, 20), Err(ConfirmError::Invalid), "the old link died");
        assert_eq!(get(&c, &a.row.id).expect("get").expect("row").revoked_by.as_deref(), Some(BY_REISSUED));
        assert_eq!(confirm(&c, &b.token, 10 + LINK_SECS), Err(ConfirmError::Expired));
        assert_eq!(get(&c, &b.row.id).expect("get").expect("row").state, ST_EXPIRED);
        let c2 = issue(&c, PURPOSE_PERSON_NOTICE, "ann", "a@x.test", "ann", 20 + LINK_SECS).expect("after expiry");
        assert_ne!(c2.row.id, b.row.id, "a new row");
    }

    #[test]
    fn limits_three_pending_per_subject_and_ten_mails_an_hour() {
        let c = conn();
        for n in 0..3 {
            issue(&c, PURPOSE_PERSON_NOTICE, "bo", &format!("b{n}@x.test"), "bo", 100 + n).expect("pending");
        }
        assert!(matches!(issue(&c, PURPOSE_PERSON_NOTICE, "bo", "b9@x.test", "bo", 110), Err(IssueError::Limit("too_many_pending", _))));
        // Re-asking for one already pending is fine (it replaces itself).
        issue(&c, PURPOSE_PERSON_NOTICE, "bo", "b0@x.test", "bo", 111).expect("re-ask");
        for n in 0..5 {
            issue(&c, PURPOSE_PERSON_NOTICE, &format!("p{n}"), "z@x.test", "x", 120 + n).expect("box");
        }
        // 4 for bo + 5 = 9 this hour; one more fits, then the box limit.
        issue(&c, PURPOSE_FORWARD, "box@brand.test", "out@x.test", "owner", 130).expect("tenth");
        assert!(matches!(issue(&c, PURPOSE_FORWARD, "box@brand.test", "out2@x.test", "owner", 131), Err(IssueError::Limit("rate_limited", _))));
        issue(&c, PURPOSE_FORWARD, "box@brand.test", "out2@x.test", "owner", 131 + 3600).expect("next hour");
    }

    #[test]
    fn a_person_has_one_notice_address_and_any_stop_link_revokes() {
        let c = conn();
        let a = issue(&c, PURPOSE_PERSON_NOTICE, "cy", "old@x.test", "cy", 0).expect("a");
        confirm(&c, &a.token, 1).expect("ok");
        let b = issue(&c, PURPOSE_PERSON_NOTICE, "cy", "new@x.test", "cy", 2).expect("b");
        assert!(is_verified(&c, PURPOSE_PERSON_NOTICE, "cy", "old@x.test"), "the old one works until the new one is confirmed");
        confirm(&c, &b.token, 3).expect("ok");
        assert!(!is_verified(&c, PURPOSE_PERSON_NOTICE, "cy", "old@x.test"));
        assert_eq!(get(&c, &a.row.id).unwrap().unwrap().revoked_by.as_deref(), Some(BY_REPLACED));
        assert_eq!(verified_for(&c, PURPOSE_PERSON_NOTICE, "cy").unwrap().unwrap().address, "new@x.test");
        // A later mail's own stop link, and the first mail's, both stop it.
        let later = mint_stop(&c, &b.row.id, 4).expect("stop");
        assert_eq!(peek_stop(&c, &later).unwrap().unwrap().id, b.row.id);
        let v = stop(&c, &b.stop, 5).expect("stop").expect("row");
        assert_eq!((v.state.as_str(), v.revoked_by.as_deref()), (ST_REVOKED, Some(BY_RECIPIENT)));
        assert_eq!(stop(&c, &later, 6).expect("again").expect("row").revoked_at, Some(5), "idempotent");
        assert_eq!(stop(&c, "k2es_nope", 6).expect("bad"), None);
        // Forward addresses are not exclusive.
        for a in ["f1@x.test", "f2@x.test"] {
            let i = issue(&c, PURPOSE_FORWARD, "box@brand.test", a, "owner", 10).expect("f");
            confirm(&c, &i.token, 11).expect("ok");
        }
        assert!(is_verified(&c, PURPOSE_FORWARD, "box@brand.test", "f1@x.test") && is_verified(&c, PURPOSE_FORWARD, "box@brand.test", "f2@x.test"));
        assert_eq!(purge_dead(&c, 100).expect("purge"), 2, "old + revoked new");
        assert_eq!(peek_stop(&c, &later).unwrap(), None, "stop rows go with their address");
    }

    #[test]
    fn token_shapes_are_strict() {
        assert!(!token_shape_ok("k2ev_<script>", CONFIRM_PREFIX));
        assert!(!token_shape_ok(&format!("k2es_{}", "a".repeat(43)), CONFIRM_PREFIX));
        assert!(token_shape_ok(&format!("k2ev_{}", "a-_9".repeat(10) + "abc"), CONFIRM_PREFIX));
        assert!(!valid_purpose("anything"));
    }
}
