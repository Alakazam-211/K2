//! Hosted mail forwards: the k2.db store (migrations 0256, 0260,
//! prd-hostmail-external-forwarding-v1 §3.2, §3.6, §3.7; vs-live FW-A5,
//! FW-A6, FW-A8; Rosson 2026-11-02: several destinations, opt-out).
//!
//! One row per (mailbox, destination); at most one live (not `removed`) row
//! per pair and [`MAX_DESTINATIONS`] per mailbox. The daemon writes the
//! mailbox's Sieve from its live rows (`mail::forward_sync`); nothing here
//! talks to the mail server.
//!
//! ```text
//!   set (same box or outside) ──► active ◄──(resume)── paused ◄──(pause)── active
//!   any live state ──(unset / rm / replaced)──► removed
//!   pending_verify (only from before opt-out) ──► active | removed
//! ```
//!
//! [`transition`] is the only way a row changes state, and it refuses any
//! move the diagram doesn't have. Every change writes one
//! `mail_forward_events` row (the audit list).
//!
//! Outcomes ([`record_outcome`]) are per forwarded message, joined from the
//! mail server's webhook by queue id ([`link_queue`]); the counts feed the
//! caps, the auto-pause and the box brake. Nothing here logs an address.

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value as J;

pub const ST_PENDING_VERIFY: &str = "pending_verify";
pub const ST_ACTIVE: &str = "active";
pub const ST_PAUSED: &str = "paused";
pub const ST_REMOVED: &str = "removed";

pub const KIND_INTERNAL: &str = "internal";
pub const KIND_EXTERNAL: &str = "external";

// ── pause reasons (`pause_reason`) ──────────────────────────────────
/// The destination said the mailbox doesn't exist / is disabled
/// (5.1.x, 5.2.1) [`DEST_GONE_IN_A_ROW`] times in a row.
pub const P_DEST_GONE: &str = "dest_gone";
/// More than [`REFUSED_PCT`]% refused for policy or spam (5.7.x) over 24 h
/// with at least [`REFUSED_MIN`] forwards.
pub const P_REFUSED: &str = "refused";
/// The destination pressed "stop" (its verified address was revoked).
pub const P_REVOKED: &str = "revoked";
/// [`DAILY_CAP`] forwards in the trailing 24 h.
pub const P_DAILY_CAP: &str = "daily_cap";
/// The box brake: the whole box's forwards are being refused.
pub const P_BOX_BRAKE: &str = "box_brake";
/// The mailbox's domain is `receive-only`.
pub const P_RECEIVE_ONLY: &str = "receive_only";
/// Air-gap is on.
pub const P_AIR_GAP: &str = "air_gap";
/// Someone paused it by hand.
pub const P_MANUAL: &str = "manual";
/// K2 couldn't tell the address it can stop this, for [`NOT_TOLD_PAUSE_SECS`].
/// An opt-out with no stop link isn't an opt-out. Lifted when the email goes.
pub const P_NOT_TOLD: &str = "not_told";
/// The mail server is older than the one outside forwards need (or its
/// version is unknown): `k2 hostmail upgrade`.
pub const P_UPGRADE: &str = "upgrade_required";

/// Pauses K2 lifts by itself when the cause goes away (the reconcile).
pub fn auto_lifts(reason: &str) -> bool {
    reason == P_RECEIVE_ONLY || reason == P_AIR_GAP || reason == P_UPGRADE
}

// ── events (`mail_forward_events.event`) ────────────────────────────
pub const E_SET: &str = "set";
pub const E_VERIFY_SENT: &str = "verify_sent";
pub const E_VERIFIED: &str = "verified";
pub const E_ACTIVATED: &str = "activated";
pub const E_PAUSED: &str = "paused";
pub const E_RESUMED: &str = "resumed";
pub const E_REMOVED: &str = "removed";
pub const E_REPLACED: &str = "replaced";
pub const E_DRIFT: &str = "drift";
pub const E_IMPORTED: &str = "imported";
pub const E_KEEP: &str = "keep_changed";
/// The "you've been added" email went out (`detail.via`).
pub const E_NOTICE_SENT: &str = "notice_sent";
/// It didn't; the forward runs anyway and K2 tries again hourly.
pub const E_NOTICE_FAILED: &str = "notice_failed";
/// The owner or an Admin chose a destination again after it pressed Stop.
pub const E_RESTARTED: &str = "restarted";
/// K2 told the owner this row still has no stop link (once).
pub const E_NOT_TOLD: &str = "not_told";
/// K2 told the owner this row's mail can't arrive (once).
pub const E_CANT_ARRIVE: &str = "cant_arrive";
/// A row from before opt-out was recent enough to go active on update.
pub const E_LEGACY_DUE: &str = "legacy_due";
/// That row went active (not a new address).
pub const E_LEGACY_ON: &str = "legacy_on";
/// A row from before opt-out was too old to go active: it stays off.
pub const E_LEGACY_STALE: &str = "legacy_stale";

// ── what K2 told an outside address (`notice_state`) ────────────────
pub const N_SENT: &str = "sent";
pub const N_FAILED: &str = "failed";
pub const N_DUE: &str = "due";
pub const N_STALE: &str = "stale";

// ── outcomes (`mail_forward_outcomes.kind`) ─────────────────────────
pub const O_QUEUED: &str = "queued";
pub const O_DELIVERED: &str = "delivered";
pub const O_REFUSED_USER: &str = "refused_user";
pub const O_REFUSED_POLICY: &str = "refused_policy";
pub const O_FAILED: &str = "failed";

// ── limits (PRD §3.5, §3.6; vs-live Q6, Q7) ─────────────────────────
/// Per mailbox per hour (the mail server's throttle defers above it).
pub const HOURLY_CAP: i64 = 100;
/// Per mailbox per trailing 24 h (K2 pauses at it).
pub const DAILY_CAP: i64 = 500;
/// Per box per hour across all mailboxes (the mail server defers above it).
pub const BOX_HOURLY_CAP: i64 = 1000;
pub const WINDOW_SECS: i64 = 24 * 3600;
pub const DEST_GONE_IN_A_ROW: i64 = 3;
pub const REFUSED_PCT: i64 = 20;
pub const REFUSED_MIN: i64 = 10;
pub const BRAKE_PCT: i64 = 10;
pub const BRAKE_MIN: i64 = 50;
/// Destinations one mailbox may forward to (live rows of any state). The
/// mail server's redirect limit is raised to match
/// (`forward_sync::REDIRECT_LIMIT`).
pub const MAX_DESTINATIONS: usize = 5;
/// New outside addresses per box per trailing 24 h, across every mailbox and
/// caller, unless the owner set another number ([`new_outside_cap`]). There is
/// no confirm step, so this is what bounds how many strangers a careless or
/// hijacked caller can start mailing.
pub const NEW_OUTSIDE_PER_DAY: i64 = 20;
/// The most the owner can set it to.
pub const NEW_OUTSIDE_PER_DAY_MAX: i64 = 1000;
/// Notice emails sent as a forward is added, per box per trailing hour. Past
/// this a new outside forward waits its turn (`due`).
pub const NOTICES_BURST_PER_HOUR: i64 = 60;
/// On the update to opt-out, a `pending_verify` row set within this long goes
/// active. Older ones stay off until someone sets them again.
pub const LEGACY_ON_WINDOW_SECS: i64 = 7 * 86_400;
/// Notice emails sent per pass (one pass a minute): rows waiting their turn go
/// on at this pace, each with its notice first, so a box with many doesn't
/// burst or stall part way. With [`NOTICES_BURST_PER_HOUR`] that's at most 180
/// an hour: the notices share K2's system mail budget (200 an hour,
/// `email_notify::budget`) with password resets, and they go to strangers, so
/// a sudden thousand would hurt the box's mail reputation.
pub const NOTICES_PER_PASS: usize = 2;
/// Owner notices about outside forwards sent one by one per trailing hour.
/// The rest roll into one hourly summary, so a bulk add isn't 300 notes.
pub const OWNER_NOTES_PER_HOUR: i64 = 5;
/// A rolled-up change waits at most this long for its summary.
pub const OWNER_SUMMARY_SECS: i64 = 3600;
/// How long before K2 tries a failed notice email again; the gap grows with
/// how long it has been failing, up to [`NOTICE_RETRY_MAX_SECS`].
pub const NOTICE_RETRY_SECS: i64 = 3600;
pub const NOTICE_RETRY_MAX_SECS: i64 = 6 * 3600;
/// A forward whose address K2 still can't tell after this long pauses.
pub const NOT_TOLD_PAUSE_SECS: i64 = 24 * 3600;
/// Notices one outside address can get from this box in a trailing day, across
/// mailboxes and re-adds: so nobody is mailed the whole box's cap.
pub const NOTICES_PER_DEST_PER_DAY: i64 = 3;

/// Retention.
pub const EVENTS_KEEP_SECS: i64 = 365 * 86_400;
pub const QUEUE_KEEP_SECS: i64 = 7 * 86_400;
pub const OUTCOMES_KEEP_SECS: i64 = 30 * 86_400;
pub const OWNER_NOTES_KEEP_SECS: i64 = 30 * 86_400;

fn err(what: &str) -> impl Fn(rusqlite::Error) -> String + '_ {
    move |e| format!("mail forwards {what}: {e}")
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Forward {
    pub id: String,
    /// `mail_addresses.id` of the forwarding mailbox.
    pub address_id: String,
    /// The forwarding mailbox's address.
    pub address: String,
    pub dest: String,
    /// [`KIND_INTERNAL`] | [`KIND_EXTERNAL`].
    pub dest_kind: String,
    /// `email_verified_addresses.id` (outside destinations).
    pub verified_address_id: Option<String>,
    pub keep: bool,
    pub state: String,
    pub set_by: String,
    pub pause_reason: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub activated_at: Option<i64>,
    pub paused_at: Option<i64>,
    pub removed_at: Option<i64>,
    /// What K2 told an outside address: [`N_SENT`], [`N_FAILED`], [`N_DUE`],
    /// [`N_STALE`]; `None` for same-box forwards and older confirmed ones.
    pub notice_state: Option<String>,
    pub notice_at: Option<i64>,
    /// The audit actor who paused it (`None`: not paused, or from before this).
    pub paused_by: Option<String>,
    /// Since when K2 hasn't managed to email this address.
    pub notice_failed_at: Option<i64>,
    /// When K2 tries that email next.
    pub notice_next_at: Option<i64>,
}

impl Forward {
    pub fn is_external(&self) -> bool {
        self.dest_kind == KIND_EXTERNAL
    }
    pub fn is_live(&self) -> bool {
        self.state != ST_REMOVED
    }
    pub fn is_active(&self) -> bool {
        self.state == ST_ACTIVE
    }
}

const COLS: &str = "id, address_id, address, dest, dest_kind, verified_address_id, keep, state, set_by, pause_reason, \
                    created_at, updated_at, activated_at, paused_at, removed_at, notice_state, notice_at, paused_by, notice_failed_at, notice_next_at";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Forward> {
    Ok(Forward {
        id: r.get(0)?,
        address_id: r.get(1)?,
        address: r.get(2)?,
        dest: r.get(3)?,
        dest_kind: r.get(4)?,
        verified_address_id: r.get(5)?,
        keep: r.get::<_, i64>(6)? == 1,
        state: r.get(7)?,
        set_by: r.get(8)?,
        pause_reason: r.get(9)?,
        created_at: r.get(10)?,
        updated_at: r.get(11)?,
        activated_at: r.get(12)?,
        paused_at: r.get(13)?,
        removed_at: r.get(14)?,
        notice_state: r.get(15)?,
        notice_at: r.get(16)?,
        paused_by: r.get(17)?,
        notice_failed_at: r.get(18)?,
        notice_next_at: r.get(19)?,
    })
}

fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub fn get(conn: &Connection, id: &str) -> Result<Option<Forward>, String> {
    conn.query_row(&format!("SELECT {COLS} FROM mail_forwards WHERE id = ?1"), params![id], row)
        .optional()
        .map_err(err("get"))
}

/// The mailbox's live (not removed) forwards, oldest first (the order the
/// Sieve lists them in, so a rewrite is byte-stable).
pub fn live_rows(conn: &Connection, address_id: &str) -> Result<Vec<Forward>, String> {
    let mut st = conn
        .prepare(&format!("SELECT {COLS} FROM mail_forwards WHERE address_id = ?1 AND state <> 'removed' ORDER BY created_at, rowid"))
        .map_err(err("live"))?;
    let rows = st.query_map(params![address_id], row).map_err(err("live"))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(err("live"))
}

/// The mailbox's oldest live forward (a mailbox with one destination: its
/// forward).
pub fn live_for_address(conn: &Connection, address_id: &str) -> Result<Option<Forward>, String> {
    Ok(live_rows(conn, address_id)?.into_iter().next())
}

/// The live row of (mailbox address, destination), if any.
pub fn live_for_pair(conn: &Connection, address: &str, dest: &str) -> Result<Option<Forward>, String> {
    conn.query_row(
        &format!("SELECT {COLS} FROM mail_forwards WHERE address = ?1 AND dest = ?2 AND state <> 'removed'"),
        params![address, dest],
        row,
    )
    .optional()
    .map_err(err("pair"))
}

/// The newest row of the mailbox (live or removed), for the panel.
pub fn latest_for_address(conn: &Connection, address_id: &str) -> Result<Option<Forward>, String> {
    conn.query_row(
        &format!(
            "SELECT {COLS} FROM mail_forwards WHERE address_id = ?1 \
             ORDER BY (state <> 'removed') DESC, updated_at DESC, id DESC LIMIT 1"
        ),
        params![address_id],
        row,
    )
    .optional()
    .map_err(err("latest"))
}

/// Live rows whose verified address is `verified_id` (the stop link).
pub fn live_for_verified(conn: &Connection, verified_id: &str) -> Result<Vec<Forward>, String> {
    let mut st = conn
        .prepare(&format!("SELECT {COLS} FROM mail_forwards WHERE verified_address_id = ?1 AND state <> 'removed' ORDER BY id"))
        .map_err(err("by verified"))?;
    let rows = st.query_map(params![verified_id], row).map_err(err("by verified"))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(err("by verified"))
}

/// Live `pending_verify` rows of one mailbox address and destination.
pub fn pending_for(conn: &Connection, address: &str, dest: &str) -> Result<Vec<Forward>, String> {
    let mut st = conn
        .prepare(&format!(
            "SELECT {COLS} FROM mail_forwards WHERE address = ?1 AND dest = ?2 AND state = 'pending_verify' ORDER BY id"
        ))
        .map_err(err("pending"))?;
    let rows = st.query_map(params![address, dest], row).map_err(err("pending"))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(err("pending"))
}

#[derive(Debug, Clone, Default)]
pub struct ListFilter<'a> {
    /// One state only.
    pub state: Option<&'a str>,
    /// Outside destinations only.
    pub external_only: bool,
    /// Removed rows too.
    pub include_removed: bool,
}

pub fn list(conn: &Connection, f: &ListFilter<'_>) -> Result<Vec<Forward>, String> {
    let mut sql = format!("SELECT {COLS} FROM mail_forwards WHERE 1 = 1");
    if !f.include_removed && f.state != Some(ST_REMOVED) {
        sql.push_str(" AND state <> 'removed'");
    }
    if f.external_only {
        sql.push_str(" AND dest_kind = 'external'");
    }
    if f.state.is_some() {
        sql.push_str(" AND state = ?1");
    }
    sql.push_str(" ORDER BY address, updated_at DESC, id");
    let mut st = conn.prepare(&sql).map_err(err("list"))?;
    let rows = match f.state {
        Some(s) => st.query_map(params![s], row).map_err(err("list"))?.collect::<Result<Vec<_>, _>>(),
        None => st.query_map([], row).map_err(err("list"))?.collect::<Result<Vec<_>, _>>(),
    };
    rows.map_err(err("list"))
}

/// Every mailbox (`address_id`) that has any forward row: the reconcile's
/// work list (a removed row's mailbox still needs its block gone).
pub fn address_ids_with_rows(conn: &Connection, since_removed: i64) -> Result<Vec<String>, String> {
    let mut st = conn
        .prepare(
            "SELECT DISTINCT address_id FROM mail_forwards \
             WHERE state <> 'removed' OR removed_at >= ?1 ORDER BY address_id",
        )
        .map_err(err("work list"))?;
    let rows = st.query_map(params![since_removed], |r| r.get::<_, String>(0)).map_err(err("work list"))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(err("work list"))
}

/// A new forward.
#[derive(Debug, Clone)]
pub struct NewForward<'a> {
    pub address_id: &'a str,
    pub address: &'a str,
    pub dest: &'a str,
    pub dest_kind: &'a str,
    pub keep: bool,
    pub set_by: &'a str,
    /// [`ST_PENDING_VERIFY`] or [`ST_ACTIVE`] (an internal one, or an
    /// outside one whose address is already verified).
    pub state: &'a str,
    pub verified_address_id: Option<&'a str>,
}

/// A live row is replaced or dropped from a `set`'s list (event [`E_REPLACED`]).
pub fn retire(conn: &Connection, old: &Forward, actor: &str, now: i64) -> Result<(), String> {
    conn.execute(
        "UPDATE mail_forwards SET state = 'removed', pause_reason = NULL, removed_at = ?2, updated_at = ?2 WHERE id = ?1",
        params![old.id, now],
    )
    .map_err(err("replace"))?;
    record_event(conn, &old.id, E_REPLACED, actor, &serde_json::json!({ "from": old.state }), now)
}

/// Insert a forward as the mailbox's only one: every other live row of the
/// mailbox is retired first (`removed`, event [`E_REPLACED`]). One [`E_SET`]
/// event (and [`E_ACTIVATED`] when it starts active).
pub fn create(conn: &Connection, n: &NewForward<'_>, now: i64) -> Result<Forward, String> {
    check_new(n)?;
    let tx = conn.unchecked_transaction().map_err(err("create"))?;
    for old in live_rows(&tx, n.address_id)? {
        retire(&tx, &old, n.set_by, now)?;
    }
    let f = insert_new(&tx, n, now)?;
    tx.commit().map_err(err("create"))?;
    Ok(f)
}

/// Add a destination beside the mailbox's others. A live row of the same
/// destination is retired first; [`MAX_DESTINATIONS`] holds.
pub fn add(conn: &Connection, n: &NewForward<'_>, now: i64) -> Result<Forward, String> {
    check_new(n)?;
    let tx = conn.unchecked_transaction().map_err(err("add"))?;
    let live = live_rows(&tx, n.address_id)?;
    let (same, others): (Vec<_>, Vec<_>) = live.into_iter().partition(|f| f.dest == n.dest);
    if others.len() >= MAX_DESTINATIONS {
        return Err(format!("mail forwards: a mailbox forwards to at most {MAX_DESTINATIONS} addresses"));
    }
    for old in &same {
        retire(&tx, old, n.set_by, now)?;
    }
    let f = insert_new(&tx, n, now)?;
    tx.commit().map_err(err("add"))?;
    Ok(f)
}

fn check_new(n: &NewForward<'_>) -> Result<(), String> {
    if n.dest_kind != KIND_INTERNAL && n.dest_kind != KIND_EXTERNAL {
        return Err(format!("mail forwards: unknown destination kind '{}'", n.dest_kind));
    }
    if n.state != ST_PENDING_VERIFY && n.state != ST_ACTIVE {
        return Err(format!("mail forwards: a new forward can't start '{}'", n.state));
    }
    if n.dest_kind == KIND_EXTERNAL && n.state == ST_ACTIVE && n.verified_address_id.is_none() {
        return Err("mail forwards: an outside destination starts active only with its stop-link row".to_string());
    }
    Ok(())
}

fn insert_new(tx: &Connection, n: &NewForward<'_>, now: i64) -> Result<Forward, String> {
    let id = new_id();
    let activated = (n.state == ST_ACTIVE).then_some(now);
    tx.execute(
        "INSERT INTO mail_forwards (id, address_id, address, dest, dest_kind, verified_address_id, keep, state, set_by, \
         created_at, updated_at, activated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10, ?11)",
        params![
            id,
            n.address_id,
            n.address,
            n.dest,
            n.dest_kind,
            n.verified_address_id,
            n.keep as i64,
            n.state,
            n.set_by,
            now,
            activated
        ],
    )
    .map_err(err("create"))?;
    record_event(tx, &id, E_SET, n.set_by, &serde_json::json!({ "kind": n.dest_kind, "keep": n.keep, "state": n.state }), now)?;
    if n.state == ST_ACTIVE {
        record_event(tx, &id, E_ACTIVATED, n.set_by, &serde_json::json!({}), now)?;
    }
    get(tx, &id)?.ok_or_else(|| "mail forwards: the new row vanished".to_string())
}

/// New outside addresses added on this box since `since` (every mailbox).
/// Rows from before opt-out turning on and imported rows aren't new
/// addresses. A row removed and added again counts again: each add mails a
/// stranger a notice.
pub fn new_outside_since(conn: &Connection, since: i64) -> Result<i64, String> {
    conn.query_row(
        "SELECT COUNT(*) FROM mail_forward_events e JOIN mail_forwards f ON f.id = e.forward_id \
         WHERE e.event = 'set' AND f.dest_kind = 'external' AND e.at >= ?1 \
         AND NOT EXISTS (SELECT 1 FROM mail_forward_events x WHERE x.forward_id = e.forward_id \
         AND x.event IN ('legacy_due', 'legacy_stale', 'imported'))",
        params![since],
        |r| r.get(0),
    )
    .map_err(err("new outside"))
}

/// How many new outside addresses this box may add a day: what the owner set
/// (`mail_server.forward_new_per_day`), or [`NEW_OUTSIDE_PER_DAY`].
pub fn new_outside_cap(conn: &Connection) -> Result<i64, String> {
    conn.query_row("SELECT forward_new_per_day FROM mail_server WHERE id = 1", [], |r| r.get::<_, Option<i64>>(0))
        .optional()
        .map(|v| v.flatten().unwrap_or(NEW_OUTSIDE_PER_DAY))
        .map_err(err("new outside cap"))
}

/// Set it. `Ok(false)`: no mail server here yet.
pub fn set_new_outside_cap(conn: &Connection, per_day: i64) -> Result<bool, String> {
    let n = conn
        .execute("UPDATE mail_server SET forward_new_per_day = ?1 WHERE id = 1", params![per_day])
        .map_err(err("new outside cap"))?;
    Ok(n == 1)
}

/// Notice emails K2 tried since `since` (sent or failed).
pub fn notices_since(conn: &Connection, since: i64) -> Result<i64, String> {
    conn.query_row(
        "SELECT COUNT(*) FROM mail_forward_events WHERE event IN ('notice_sent', 'notice_failed') AND at >= ?1",
        params![since],
        |r| r.get(0),
    )
    .map_err(err("notices"))
}

/// Rows waiting their turn.
pub fn due_waiting(conn: &Connection) -> Result<i64, String> {
    conn.query_row("SELECT COUNT(*) FROM mail_forwards WHERE state = 'pending_verify' AND notice_state = 'due'", [], |r| r.get(0))
        .map_err(err("waiting"))
}

/// Has this row an event from before opt-out's sort (`legacy_due`)?
pub fn is_legacy_due(conn: &Connection, id: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM mail_forward_events WHERE forward_id = ?1 AND event = 'legacy_due')",
        params![id],
        |r| r.get(0),
    )
    .map_err(err("legacy"))
}

/// Record what K2 told an outside address. A notice that went clears the
/// failure clock.
pub fn set_notice(conn: &Connection, id: &str, state: &str, now: i64) -> Result<(), String> {
    conn.execute(
        "UPDATE mail_forwards SET notice_state = ?2, notice_at = ?3, \
         notice_failed_at = CASE WHEN ?2 = 'sent' THEN NULL ELSE notice_failed_at END, \
         notice_next_at = CASE WHEN ?2 = 'sent' THEN NULL ELSE notice_next_at END WHERE id = ?1",
        params![id, state, now],
    )
    .map_err(err("notice"))?;
    Ok(())
}

/// The notice didn't go: start or keep the failure clock and set the next try.
/// A row still waiting its turn stays `due` (it isn't on); any other is `failed`.
pub fn notice_failed(conn: &Connection, id: &str, now: i64) -> Result<(), String> {
    let since: Option<i64> = conn
        .query_row("SELECT notice_failed_at FROM mail_forwards WHERE id = ?1", params![id], |r| r.get(0))
        .optional()
        .map_err(err("notice"))?
        .flatten();
    let since = since.unwrap_or(now);
    let gap = (now - since).clamp(NOTICE_RETRY_SECS, NOTICE_RETRY_MAX_SECS);
    conn.execute(
        "UPDATE mail_forwards SET notice_state = CASE WHEN notice_state = 'due' THEN 'due' ELSE 'failed' END, \
         notice_at = ?2, notice_failed_at = ?3, notice_next_at = ?4 WHERE id = ?1",
        params![id, now, since, now + gap],
    )
    .map_err(err("notice"))?;
    Ok(())
}

/// Has this row an event of this kind?
pub fn has_event(conn: &Connection, id: &str, event: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM mail_forward_events WHERE forward_id = ?1 AND event = ?2)",
        params![id, event],
        |r| r.get(0),
    )
    .map_err(err("events"))
}

/// Notices this address got from this box since `since`, across mailboxes.
pub fn notices_to_dest_since(conn: &Connection, dest: &str, since: i64) -> Result<i64, String> {
    conn.query_row(
        "SELECT COUNT(*) FROM mail_forward_events e JOIN mail_forwards f ON f.id = e.forward_id \
         WHERE f.dest = ?1 AND e.event = 'notice_sent' AND e.at >= ?2",
        params![dest, since],
        |r| r.get(0),
    )
    .map_err(err("notices"))
}

/// Pauses a person must undo: K2 stopped this for a reason an agent may not
/// wave away. What a stop, a hold or a person's own pause isn't.
pub fn is_automatic_pause(reason: &str) -> bool {
    matches!(reason, P_DEST_GONE | P_REFUSED | P_DAILY_CAP | P_BOX_BRAKE | P_NOT_TOLD)
}

/// An automatic pause of (mailbox address, destination) since `since` with no
/// resume after it, on any row of the pair, live or removed: `rm` and `add`
/// don't wash one out.
pub fn recent_automatic_pause(conn: &Connection, address: &str, dest: &str, since: i64) -> Result<Option<String>, String> {
    let mut st = conn
        .prepare(
            "SELECT e.detail_json FROM mail_forward_events e JOIN mail_forwards f ON f.id = e.forward_id \
             WHERE f.address = ?1 AND f.dest = ?2 AND e.event = 'paused' AND e.at >= ?3 \
             AND NOT EXISTS (SELECT 1 FROM mail_forward_events r WHERE r.forward_id = e.forward_id AND r.event = 'resumed' AND r.at >= e.at) \
             ORDER BY e.at DESC, e.rowid DESC",
        )
        .map_err(err("recent pause"))?;
    let rows = st.query_map(params![address, dest, since], |r| r.get::<_, String>(0)).map_err(err("recent pause"))?;
    for raw in rows {
        let raw = raw.map_err(err("recent pause"))?;
        let reason = serde_json::from_str::<J>(&raw).ok().and_then(|v| v.get("reason").and_then(J::as_str).map(str::to_string));
        if let Some(r) = reason.filter(|r| is_automatic_pause(r)) {
            return Ok(Some(r));
        }
    }
    Ok(None)
}

/// Where the mailbox's daily count starts: the last 24 hours, across every row
/// it had, or a person's resume of a `daily_cap` pause if that's later.
pub fn daily_window_start(conn: &Connection, address_id: &str, now: i64) -> Result<i64, String> {
    let resumed: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(r.at), 0) FROM mail_forward_events r JOIN mail_forwards f ON f.id = r.forward_id \
             WHERE f.address_id = ?1 AND r.event = 'resumed' AND ( \
               SELECT p.detail_json FROM mail_forward_events p WHERE p.forward_id = r.forward_id AND p.event = 'paused' \
               AND p.at <= r.at ORDER BY p.at DESC, p.rowid DESC LIMIT 1) LIKE '%\"reason\":\"daily_cap\"%'",
            params![address_id],
            |r| r.get(0),
        )
        .map_err(err("daily window"))?;
    Ok((now - WINDOW_SECS).max(resumed))
}

/// The mailboxes (`address_id`) whose forwards were refused since `since`.
pub fn refused_mailboxes_since(conn: &Connection, since: i64) -> Result<Vec<String>, String> {
    let mut st = conn
        .prepare(
            "SELECT DISTINCT f.address_id FROM mail_forward_outcomes o JOIN mail_forwards f ON f.id = o.forward_id \
             WHERE o.at >= ?1 AND o.kind IN ('refused_user', 'refused_policy', 'failed') ORDER BY f.address_id",
        )
        .map_err(err("refused"))?;
    let rows = st.query_map(params![since], |r| r.get::<_, String>(0)).map_err(err("refused"))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(err("refused"))
}

/// Would a same-box forward from `from` to `to` close a loop: can mail already
/// get from `to` back to `from` through live same-box rows?
pub fn same_box_loop(conn: &Connection, from: &str, to: &str) -> Result<bool, String> {
    let mut seen: Vec<String> = vec![to.to_string()];
    let mut i = 0;
    while i < seen.len() {
        if seen[i] == from {
            return Ok(true);
        }
        let mut st = conn
            .prepare("SELECT dest FROM mail_forwards WHERE address = ?1 AND dest_kind = 'internal' AND state = 'active'")
            .map_err(err("loop"))?;
        let next = st
            .query_map(params![seen[i]], |r| r.get::<_, String>(0))
            .map_err(err("loop"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(err("loop"))?;
        for n in next {
            if !seen.contains(&n) {
                seen.push(n);
            }
        }
        i += 1;
    }
    Ok(false)
}

/// Limits saved on the mail server but not confirmed loaded: survives a restart.
pub fn reload_owed(conn: &Connection) -> bool {
    conn.query_row("SELECT forward_reload_owed FROM mail_server WHERE id = 1", [], |r| r.get::<_, i64>(0)).is_ok_and(|n| n != 0)
}

pub fn set_reload_owed(conn: &Connection, owed: bool) -> Result<(), String> {
    conn.execute("UPDATE mail_server SET forward_reload_owed = ?1 WHERE id = 1", params![owed as i64]).map_err(err("reload"))?;
    Ok(())
}

/// Why delivery outcomes aren't reaching K2 (`None`: nothing known wrong).
pub fn webhook_error(conn: &Connection) -> Option<String> {
    conn.query_row("SELECT forward_webhook_error FROM mail_server WHERE id = 1", [], |r| r.get::<_, Option<String>>(0)).ok().flatten()
}

/// Record it. `Ok(true)`: this changed the answer (so someone should hear).
pub fn set_webhook_error(conn: &Connection, error: Option<&str>) -> Result<bool, String> {
    let before = webhook_error(conn);
    conn.execute("UPDATE mail_server SET forward_webhook_error = ?1 WHERE id = 1", params![error]).map_err(err("webhook"))?;
    Ok(before.is_some() != error.is_some())
}

/// Sort the rows from before opt-out that K2 hasn't looked at yet: set within
/// [`LEGACY_ON_WINDOW_SECS`] → [`N_DUE`] (go active, paced); older →
/// [`N_STALE`] (stay off). Returns the rows this call sorted: (due, stale).
pub fn legacy_sort(conn: &Connection, now: i64) -> Result<(Vec<Forward>, Vec<Forward>), String> {
    let mut st = conn
        .prepare(&format!("SELECT {COLS} FROM mail_forwards WHERE state = 'pending_verify' AND notice_state IS NULL ORDER BY created_at, rowid"))
        .map_err(err("legacy"))?;
    let rows = st.query_map([], row).map_err(err("legacy"))?.collect::<Result<Vec<_>, _>>().map_err(err("legacy"))?;
    let (mut due, mut stale) = (Vec::new(), Vec::new());
    for f in rows {
        if f.created_at >= now - LEGACY_ON_WINDOW_SECS {
            set_notice(conn, &f.id, N_DUE, now)?;
            record_event(conn, &f.id, E_LEGACY_DUE, "k2", &serde_json::json!({}), now)?;
            due.push(f);
        } else {
            set_notice(conn, &f.id, N_STALE, now)?;
            record_event(conn, &f.id, E_LEGACY_STALE, "k2", &serde_json::json!({}), now)?;
            stale.push(f);
        }
    }
    Ok((due, stale))
}

/// The next rows waiting their turn, oldest first.
pub fn legacy_due(conn: &Connection, now: i64, limit: usize) -> Result<Vec<Forward>, String> {
    let mut st = conn
        .prepare(&format!(
            "SELECT {COLS} FROM mail_forwards WHERE state = 'pending_verify' AND notice_state = 'due' \
             AND COALESCE(notice_next_at, 0) <= ?1 ORDER BY created_at, rowid LIMIT ?2"
        ))
        .map_err(err("legacy due"))?;
    let rows = st.query_map(params![now, limit as i64], row).map_err(err("legacy due"))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(err("legacy due"))
}

/// Outside forwards whose notice failed and whose next try is due: active
/// ones, and ones K2 paused for it.
pub fn failed_notices(conn: &Connection, now: i64, limit: usize) -> Result<Vec<Forward>, String> {
    let mut st = conn
        .prepare(&format!(
            "SELECT {COLS} FROM mail_forwards WHERE dest_kind = 'external' AND notice_state = 'failed' \
             AND (state = 'active' OR (state = 'paused' AND pause_reason = 'not_told')) \
             AND COALESCE(notice_next_at, 0) <= ?1 ORDER BY notice_next_at, rowid LIMIT ?2"
        ))
        .map_err(err("failed notices"))?;
    let rows = st.query_map(params![now, limit as i64], row).map_err(err("failed notices"))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(err("failed notices"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransitionError {
    NotFound,
    /// (from, to): the diagram has no such move.
    Illegal(String, String),
    Db(String),
}

impl std::fmt::Display for TransitionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransitionError::NotFound => write!(f, "no such forward"),
            TransitionError::Illegal(a, b) => write!(f, "a forward can't go from {a} to {b}"),
            TransitionError::Db(e) => write!(f, "{e}"),
        }
    }
}

/// Is `from → to` a move the state diagram has?
pub fn legal(from: &str, to: &str) -> bool {
    matches!(
        (from, to),
        (ST_PENDING_VERIFY, ST_ACTIVE)
            | (ST_PENDING_VERIFY, ST_REMOVED)
            | (ST_ACTIVE, ST_PAUSED)
            | (ST_ACTIVE, ST_REMOVED)
            | (ST_PAUSED, ST_ACTIVE)
            | (ST_PAUSED, ST_REMOVED)
            | (ST_PAUSED, ST_PAUSED)
    )
}

/// What a [`transition`] carries.
#[derive(Debug, Clone, Default)]
pub struct Change<'a> {
    /// Required for [`ST_PAUSED`].
    pub reason: Option<&'a str>,
    /// Set with the move to [`ST_ACTIVE`] from `pending_verify`.
    pub verified_address_id: Option<&'a str>,
    pub actor: &'a str,
    pub detail: Option<J>,
}

/// Move one forward. `paused → paused` only changes the reason. The event
/// written: `activated` (from pending_verify, plus `verified`), `resumed`
/// (paused → active), `paused`, `removed`.
pub fn transition(conn: &Connection, id: &str, to: &str, c: &Change<'_>, now: i64) -> Result<Forward, TransitionError> {
    let db = |e: String| TransitionError::Db(e);
    let cur = get(conn, id).map_err(db)?.ok_or(TransitionError::NotFound)?;
    if !legal(&cur.state, to) {
        return Err(TransitionError::Illegal(cur.state.clone(), to.to_string()));
    }
    let detail = c.detail.clone().unwrap_or_else(|| serde_json::json!({}));
    let tx = conn.unchecked_transaction().map_err(|e| TransitionError::Db(e.to_string()))?;
    let run = |sql: &str, p: &[&dyn rusqlite::ToSql]| tx.execute(sql, p).map_err(|e| TransitionError::Db(format!("mail forwards move: {e}")));
    match to {
        ST_ACTIVE => {
            let verified = c.verified_address_id.or(cur.verified_address_id.as_deref());
            if cur.is_external() && verified.is_none() {
                return Err(TransitionError::Illegal(cur.state.clone(), "active (unverified)".to_string()));
            }
            run(
                "UPDATE mail_forwards SET state = 'active', pause_reason = NULL, paused_at = NULL, paused_by = NULL, verified_address_id = ?2, \
                 activated_at = COALESCE(activated_at, ?3), updated_at = ?3 WHERE id = ?1",
                &[&id, &verified, &now],
            )?;
            if cur.state == ST_PENDING_VERIFY {
                record_event(&tx, id, E_VERIFIED, c.actor, &serde_json::json!({}), now).map_err(db)?;
                record_event(&tx, id, E_ACTIVATED, c.actor, &detail, now).map_err(db)?;
            } else {
                record_event(&tx, id, E_RESUMED, c.actor, &detail, now).map_err(db)?;
            }
        }
        ST_PAUSED => {
            let reason = c.reason.filter(|r| !r.is_empty()).ok_or_else(|| TransitionError::Illegal(cur.state.clone(), "paused (no reason)".to_string()))?;
            run(
                "UPDATE mail_forwards SET state = 'paused', pause_reason = ?2, paused_at = COALESCE(paused_at, ?3), paused_by = ?4, updated_at = ?3 WHERE id = ?1",
                &[&id, &reason, &now, &c.actor],
            )?;
            let mut d = detail.clone();
            if let J::Object(m) = &mut d {
                m.insert("reason".to_string(), J::String(reason.to_string()));
            }
            record_event(&tx, id, E_PAUSED, c.actor, &d, now).map_err(db)?;
        }
        ST_REMOVED => {
            run(
                "UPDATE mail_forwards SET state = 'removed', pause_reason = NULL, removed_at = ?2, updated_at = ?2 WHERE id = ?1",
                &[&id, &now],
            )?;
            record_event(&tx, id, E_REMOVED, c.actor, &detail, now).map_err(db)?;
        }
        other => return Err(TransitionError::Illegal(cur.state.clone(), other.to_string())),
    }
    tx.commit().map_err(|e| TransitionError::Db(e.to_string()))?;
    get(conn, id).map_err(db)?.ok_or(TransitionError::NotFound)
}

/// Change the keep-a-copy setting of a live row.
pub fn set_keep(conn: &Connection, id: &str, keep: bool, actor: &str, now: i64) -> Result<bool, String> {
    let n = conn
        .execute(
            "UPDATE mail_forwards SET keep = ?2, updated_at = ?3 WHERE id = ?1 AND state <> 'removed' AND keep <> ?2",
            params![id, keep as i64, now],
        )
        .map_err(err("keep"))?;
    if n == 1 {
        record_event(conn, id, E_KEEP, actor, &serde_json::json!({ "keep": keep }), now)?;
    }
    Ok(n == 1)
}

// ── audit ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub id: String,
    pub forward_id: String,
    pub event: String,
    pub actor: String,
    pub detail: J,
    pub at: i64,
}

/// One audit row. `detail` holds ids, counts and reasons, never an address.
pub fn record_event(conn: &Connection, forward_id: &str, event: &str, actor: &str, detail: &J, now: i64) -> Result<(), String> {
    conn.execute(
        "INSERT INTO mail_forward_events (id, forward_id, event, actor, detail_json, at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![new_id(), forward_id, event, actor, detail.to_string(), now],
    )
    .map_err(err("event"))?;
    Ok(())
}

/// The audit list of one mailbox (all its forwards), newest first.
pub fn events_for_address(conn: &Connection, address_id: &str, limit: usize) -> Result<Vec<Event>, String> {
    let mut st = conn
        .prepare(
            "SELECT e.id, e.forward_id, e.event, e.actor, e.detail_json, e.at FROM mail_forward_events e \
             JOIN mail_forwards f ON f.id = e.forward_id WHERE f.address_id = ?1 \
             ORDER BY e.at DESC, e.rowid DESC LIMIT ?2",
        )
        .map_err(err("events"))?;
    let rows = st
        .query_map(params![address_id, limit as i64], |r| {
            let raw: String = r.get(4)?;
            Ok(Event {
                id: r.get(0)?,
                forward_id: r.get(1)?,
                event: r.get(2)?,
                actor: r.get(3)?,
                detail: serde_json::from_str(&raw).unwrap_or(J::Null),
                at: r.get(5)?,
            })
        })
        .map_err(err("events"))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(err("events"))
}

// ── the webhook join and outcomes ───────────────────────────────────

/// Keep queue id → forward. `Ok(false)`: already linked.
pub fn link_queue(conn: &Connection, queue_id: &str, forward_id: &str, now: i64) -> Result<bool, String> {
    let n = conn
        .execute(
            "INSERT OR IGNORE INTO mail_forward_queue (queue_id, forward_id, at) VALUES (?1, ?2, ?3)",
            params![queue_id, forward_id, now],
        )
        .map_err(err("link"))?;
    Ok(n == 1)
}

pub fn forward_for_queue(conn: &Connection, queue_id: &str) -> Result<Option<String>, String> {
    conn.query_row("SELECT forward_id FROM mail_forward_queue WHERE queue_id = ?1", params![queue_id], |r| r.get(0))
        .optional()
        .map_err(err("join"))
}

/// Record one outcome. With a queue id, a repeat (same queue id, same
/// kind) is a no-op: `Ok(false)`.
pub fn record_outcome(conn: &Connection, forward_id: &str, queue_id: Option<&str>, kind: &str, code: Option<&str>, now: i64) -> Result<bool, String> {
    let n = conn
        .execute(
            "INSERT OR IGNORE INTO mail_forward_outcomes (forward_id, queue_id, kind, code, at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![forward_id, queue_id, kind, code, now],
        )
        .map_err(err("outcome"))?;
    Ok(n == 1)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Counts {
    pub queued: i64,
    pub delivered: i64,
    pub refused_user: i64,
    pub refused_policy: i64,
    pub failed: i64,
}

impl Counts {
    /// Every refusal (for the box brake).
    pub fn refused(&self) -> i64 {
        self.refused_user + self.refused_policy + self.failed
    }
}

fn counts_where(conn: &Connection, filter: &str, p: &[&dyn rusqlite::ToSql]) -> Result<Counts, String> {
    let mut st = conn
        .prepare(&format!("SELECT kind, COUNT(*) FROM mail_forward_outcomes WHERE {filter} GROUP BY kind"))
        .map_err(err("counts"))?;
    let rows = st.query_map(p, |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))).map_err(err("counts"))?;
    let mut c = Counts::default();
    for r in rows {
        let (k, n) = r.map_err(err("counts"))?;
        match k.as_str() {
            O_QUEUED => c.queued = n,
            O_DELIVERED => c.delivered = n,
            O_REFUSED_USER => c.refused_user = n,
            O_REFUSED_POLICY => c.refused_policy = n,
            O_FAILED => c.failed = n,
            _ => {}
        }
    }
    Ok(c)
}

/// One forward's outcomes since `since`.
pub fn counts(conn: &Connection, forward_id: &str, since: i64) -> Result<Counts, String> {
    counts_where(conn, "forward_id = ?1 AND at >= ?2", &[&forward_id, &since])
}

/// Copies the mailbox handed to the mail server since `since`, across all its
/// destinations (the daily cap counts every outgoing copy).
pub fn mailbox_queued(conn: &Connection, address_id: &str, since: i64) -> Result<i64, String> {
    conn.query_row(
        "SELECT COUNT(*) FROM mail_forward_outcomes o JOIN mail_forwards f ON f.id = o.forward_id \
         WHERE f.address_id = ?1 AND o.kind = 'queued' AND o.at >= ?2",
        params![address_id, since],
        |r| r.get(0),
    )
    .map_err(err("mailbox count"))
}

/// Every forward's outcomes since `since` (the box brake).
pub fn box_counts(conn: &Connection, since: i64) -> Result<Counts, String> {
    counts_where(conn, "at >= ?1", &[&since])
}

/// How many `refused_user` outcomes the forward got in a row, newest
/// first, stopping at its last `delivered` (and at its last resume).
pub fn user_refusals_in_a_row(conn: &Connection, forward_id: &str, since: i64) -> Result<i64, String> {
    let mut st = conn
        .prepare(
            "SELECT kind FROM mail_forward_outcomes WHERE forward_id = ?1 AND at >= ?2 AND kind IN ('delivered', 'refused_user') \
             ORDER BY at DESC, id DESC",
        )
        .map_err(err("in a row"))?;
    let rows = st.query_map(params![forward_id, since], |r| r.get::<_, String>(0)).map_err(err("in a row"))?;
    let mut n = 0;
    for k in rows {
        if k.map_err(err("in a row"))? != O_REFUSED_USER {
            break;
        }
        n += 1;
    }
    Ok(n)
}

/// `mail_forward_owner_notes.kind`. Each kind has its own hourly slots: a pile of
/// keep flips or removals can't spend the ones new addresses use.
pub const OWNER_ADD: &str = "add";
pub const OWNER_CHANGE: &str = "change";
pub const OWNER_STOP: &str = "stop";

// ── a destination that can't arrive ─────────────────────────────────

/// Mail to `dest` can't arrive: this box hosts its domain (a `mail_domains`
/// row, or the mail host name `host`) but has no such active mailbox. The mail
/// server delivers its own domains itself, whatever their MX says (probe X12),
/// so that mail bounces here and never reaches another provider. Fail closed
/// reads as "can arrive": this only ever adds a warning.
pub fn dest_cant_arrive(conn: &Connection, dest: &str, host: Option<&str>) -> bool {
    let Some((_, domain)) = dest.rsplit_once('@') else { return false };
    let domain = domain.trim_end_matches('.').to_ascii_lowercase();
    let hosted = host.is_some_and(|h| h == domain)
        || conn
            .query_row("SELECT COUNT(*) FROM mail_domains WHERE lower(domain) = ?1", params![domain], |r| r.get::<_, i64>(0))
            .is_ok_and(|n| n > 0);
    if !hosted {
        return false;
    }
    conn.query_row(
        "SELECT COUNT(*) FROM mail_addresses WHERE lower(address) = ?1 AND status = 'active'",
        params![dest.to_ascii_lowercase()],
        |r| r.get::<_, i64>(0),
    )
    .is_ok_and(|n| n == 0)
}

/// Live rows whose mail can't arrive that the owner hasn't been told about.
pub fn cant_arrive_unnoted(conn: &Connection, host: Option<&str>) -> Result<Vec<Forward>, String> {
    let mut st = conn
        .prepare(&format!(
            "SELECT {COLS} FROM mail_forwards f WHERE state <> 'removed' \
             AND NOT EXISTS (SELECT 1 FROM mail_forward_events e WHERE e.forward_id = f.id AND e.event = 'cant_arrive') \
             ORDER BY address, created_at, rowid"
        ))
        .map_err(err("cant arrive"))?;
    let rows = st.query_map([], row).map_err(err("cant arrive"))?.collect::<Result<Vec<_>, _>>().map_err(err("cant arrive"))?;
    Ok(rows.into_iter().filter(|f| dest_cant_arrive(conn, &f.dest, host)).collect())
}

// ── what the owner hears ────────────────────────────────────────────

/// Note a change for the owner. `Ok(true)`: tell them now (the hour's first
/// few). `Ok(false)`: it waits for the summary.
pub fn owner_note(conn: &Connection, kind: &str, line: &str, now: i64) -> Result<bool, String> {
    let told: i64 = conn
        .query_row("SELECT COUNT(*) FROM mail_forward_owner_notes WHERE rolled = 0 AND kind = ?2 AND at > ?1", params![now - 3600, kind], |r| r.get(0))
        .map_err(err("owner note"))?;
    let tell_now = told < OWNER_NOTES_PER_HOUR;
    conn.execute(
        "INSERT INTO mail_forward_owner_notes (at, kind, line, rolled, told_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![now, kind, line, !tell_now as i64, tell_now.then_some(now)],
    )
    .map_err(err("owner note"))?;
    Ok(tell_now)
}

/// The waiting changes, once the oldest has waited its time: `(kind, line)`,
/// oldest first, marked told. Empty: nothing due yet.
pub fn owner_summary_due(conn: &Connection, now: i64) -> Result<Vec<(String, String)>, String> {
    let oldest: Option<i64> = conn
        .query_row("SELECT MIN(at) FROM mail_forward_owner_notes WHERE told_at IS NULL", [], |r| r.get(0))
        .map_err(err("owner summary"))?;
    if oldest.is_none_or(|o| o > now - OWNER_SUMMARY_SECS) {
        return Ok(Vec::new());
    }
    let mut st = conn
        .prepare("SELECT kind, line FROM mail_forward_owner_notes WHERE told_at IS NULL ORDER BY at, id")
        .map_err(err("owner summary"))?;
    let rows = st
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(err("owner summary"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(err("owner summary"))?;
    conn.execute("UPDATE mail_forward_owner_notes SET told_at = ?1 WHERE told_at IS NULL", params![now]).map_err(err("owner summary"))?;
    Ok(rows)
}

/// Drop rows past their keep time.
pub fn purge(conn: &Connection, now: i64) -> Result<(), String> {
    conn.execute("DELETE FROM mail_forward_owner_notes WHERE told_at IS NOT NULL AND at < ?1", params![now - OWNER_NOTES_KEEP_SECS])
        .map_err(err("purge"))?;
    conn.execute("DELETE FROM mail_forward_queue WHERE at < ?1", params![now - QUEUE_KEEP_SECS]).map_err(err("purge"))?;
    conn.execute("DELETE FROM mail_forward_outcomes WHERE at < ?1", params![now - OUTCOMES_KEEP_SECS]).map_err(err("purge"))?;
    conn.execute("DELETE FROM mail_forward_events WHERE at < ?1", params![now - EVENTS_KEEP_SECS]).map_err(err("purge"))?;
    Ok(())
}

// ── the person link (FW-A8) ─────────────────────────────────────────

/// The person key that owns a mailbox (`None`: not linked).
pub fn person_principal(conn: &Connection, address_id: &str) -> Result<Option<String>, String> {
    conn.query_row("SELECT person_principal FROM mail_addresses WHERE id = ?1", params![address_id], |r| r.get::<_, Option<String>>(0))
        .optional()
        .map(|o| o.flatten().filter(|s| !s.trim().is_empty()))
        .map_err(err("person"))
}

/// Link (or unlink with `None`) a mailbox to a person key.
pub fn set_person_principal(conn: &Connection, address_id: &str, person: Option<&str>) -> Result<bool, String> {
    let n = conn
        .execute("UPDATE mail_addresses SET person_principal = ?2 WHERE id = ?1", params![address_id, person])
        .map_err(err("person link"))?;
    Ok(n == 1)
}

/// `(address_id, address)` of the active mailboxes linked to `person`.
pub fn mailboxes_of_person(conn: &Connection, person: &str) -> Result<Vec<(String, String)>, String> {
    let mut st = conn
        .prepare("SELECT id, address FROM mail_addresses WHERE person_principal = ?1 AND status = 'active' ORDER BY address")
        .map_err(err("person mailboxes"))?;
    let rows = st.query_map(params![person], |r| Ok((r.get(0)?, r.get(1)?))).map_err(err("person mailboxes"))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(err("person mailboxes"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        crate::db::run_migrations(&c).unwrap();
        c
    }

    fn mailbox(c: &Connection, addr: &str) -> String {
        let id = new_id();
        c.execute(
            "INSERT INTO mail_addresses (id, address, domain_id, stalwart_account_id, owner_project_id, status, created_at) \
             VALUES (?1, ?2, 'd', 'acc', 'ws', 'active', 1)",
            params![id, addr],
        )
        .unwrap();
        id
    }

    fn ext<'a>(aid: &'a str, addr: &'a str, dest: &'a str) -> NewForward<'a> {
        NewForward {
            address_id: aid,
            address: addr,
            dest,
            dest_kind: KIND_EXTERNAL,
            keep: true,
            set_by: "owner",
            state: ST_PENDING_VERIFY,
            verified_address_id: None,
        }
    }

    fn actor() -> Change<'static> {
        Change { actor: "owner", ..Default::default() }
    }

    #[test]
    fn outside_forward_walks_the_diagram() {
        let c = conn();
        let aid = mailbox(&c, "sales@brand.test");
        let f = create(&c, &ext(&aid, "sales@brand.test", "me@outside.test"), 10).unwrap();
        assert_eq!(f.state, ST_PENDING_VERIFY);
        // No activation without a verified address.
        let e = transition(&c, &f.id, ST_ACTIVE, &actor(), 11).unwrap_err();
        assert!(matches!(e, TransitionError::Illegal(_, _)), "{e:?}");
        // pending → paused is not a move.
        let e = transition(&c, &f.id, ST_PAUSED, &Change { reason: Some(P_MANUAL), ..actor() }, 11).unwrap_err();
        assert!(matches!(e, TransitionError::Illegal(_, _)), "{e:?}");
        let f = transition(&c, &f.id, ST_ACTIVE, &Change { verified_address_id: Some("v1"), actor: "recipient", ..Default::default() }, 12).unwrap();
        assert_eq!((f.state.as_str(), f.activated_at), (ST_ACTIVE, Some(12)));
        // Pausing needs a reason.
        assert!(transition(&c, &f.id, ST_PAUSED, &actor(), 13).is_err());
        let f = transition(&c, &f.id, ST_PAUSED, &Change { reason: Some(P_DEST_GONE), ..actor() }, 14).unwrap();
        assert_eq!(f.pause_reason.as_deref(), Some(P_DEST_GONE));
        let f = transition(&c, &f.id, ST_ACTIVE, &actor(), 15).unwrap();
        assert_eq!(f.pause_reason, None);
        let f = transition(&c, &f.id, ST_REMOVED, &actor(), 16).unwrap();
        assert_eq!(f.removed_at, Some(16));
        let e = transition(&c, &f.id, ST_ACTIVE, &actor(), 17).unwrap_err();
        assert!(matches!(e, TransitionError::Illegal(_, _)), "removed is final: {e:?}");
        let ev: Vec<String> = events_for_address(&c, &aid, 50).unwrap().into_iter().rev().map(|e| e.event).collect();
        assert_eq!(ev, vec![E_SET, E_VERIFIED, E_ACTIVATED, E_PAUSED, E_RESUMED, E_REMOVED]);
    }

    #[test]
    fn the_schema_refuses_an_unverified_active_outside_row() {
        let c = conn();
        let aid = mailbox(&c, "a@brand.test");
        let mut n = ext(&aid, "a@brand.test", "x@outside.test");
        n.state = ST_ACTIVE;
        assert!(create(&c, &n, 1).is_err(), "store refuses");
        let raw = c.execute(
            "INSERT INTO mail_forwards (id, address_id, address, dest, dest_kind, keep, state, set_by, created_at, updated_at) \
             VALUES ('x', ?1, 'a@brand.test', 'x@outside.test', 'external', 1, 'active', 'owner', 1, 1)",
            params![aid],
        );
        assert!(raw.is_err(), "the CHECK refuses it too");
    }

    #[test]
    fn create_replaces_every_live_row_and_add_keeps_the_others() {
        let c = conn();
        let aid = mailbox(&c, "b@brand.test");
        let a = create(&c, &ext(&aid, "b@brand.test", "one@outside.test"), 1).unwrap();
        let b = add(&c, &ext(&aid, "b@brand.test", "two@outside.test"), 2).unwrap();
        let live: Vec<String> = live_rows(&c, &aid).unwrap().into_iter().map(|f| f.dest).collect();
        assert_eq!(live, vec!["one@outside.test", "two@outside.test"], "oldest first");
        assert_eq!(get(&c, &a.id).unwrap().unwrap().state, ST_PENDING_VERIFY, "add leaves the first alone");
        // The same destination twice: one live row (the index refuses a second).
        let dup = c.execute(
            "INSERT INTO mail_forwards (id, address_id, address, dest, dest_kind, keep, state, set_by, created_at, updated_at) \
             VALUES ('y', ?1, 'b@brand.test', 'two@outside.test', 'external', 1, 'pending_verify', 'owner', 1, 1)",
            params![aid],
        );
        assert!(dup.is_err(), "the index refuses a second live row of one destination");
        let again = add(&c, &ext(&aid, "b@brand.test", "two@outside.test"), 3).unwrap();
        assert_eq!(get(&c, &b.id).unwrap().unwrap().state, ST_REMOVED, "add retires the same destination's old row");
        assert_eq!(live_rows(&c, &aid).unwrap().len(), 2);
        assert_eq!(live_for_pair(&c, "b@brand.test", "two@outside.test").unwrap().unwrap().id, again.id);
        // create leaves exactly one.
        let d = create(&c, &ext(&aid, "b@brand.test", "three@outside.test"), 4).unwrap();
        assert_eq!(live_rows(&c, &aid).unwrap().into_iter().map(|f| f.id).collect::<Vec<_>>(), vec![d.id]);
    }

    #[test]
    fn add_holds_the_destination_cap_and_a_row_from_before_0260_is_unchanged() {
        let c = conn();
        let aid = mailbox(&c, "cap@brand.test");
        for i in 0..MAX_DESTINATIONS {
            add(&c, &ext(&aid, "cap@brand.test", &format!("d{i}@outside.test")), i as i64).unwrap();
        }
        let e = add(&c, &ext(&aid, "cap@brand.test", "one-more@outside.test"), 10).unwrap_err();
        assert!(e.contains(&format!("at most {MAX_DESTINATIONS}")), "{e}");
        assert_eq!(live_rows(&c, &aid).unwrap().len(), MAX_DESTINATIONS, "the refused add changed nothing");
        // Re-adding one already there isn't over the cap.
        add(&c, &ext(&aid, "cap@brand.test", "d0@outside.test"), 11).unwrap();
        // A row written the way 0256 wrote it reads back with no notice state.
        let bid = mailbox(&c, "old@brand.test");
        c.execute(
            "INSERT INTO mail_forwards (id, address_id, address, dest, dest_kind, verified_address_id, keep, state, set_by, created_at, updated_at, activated_at) \
             VALUES ('old', ?1, 'old@brand.test', 'me@outside.test', 'external', 'v', 0, 'active', 'owner', 5, 5, 5)",
            params![bid],
        )
        .unwrap();
        let f = live_for_address(&c, &bid).unwrap().unwrap();
        assert_eq!((f.state.as_str(), f.keep, f.notice_state, f.notice_at), (ST_ACTIVE, false, None, None));
    }

    #[test]
    fn new_outside_counts_adds_but_not_rows_from_before_opt_out_or_same_box_ones() {
        let c = conn();
        let aid = mailbox(&c, "n@brand.test");
        assert_eq!(new_outside_since(&c, 0).unwrap(), 0);
        let mut n = ext(&aid, "n@brand.test", "a@outside.test");
        n.state = ST_ACTIVE;
        n.verified_address_id = Some("v");
        let a = add(&c, &n, 10).unwrap();
        assert_eq!(new_outside_since(&c, 0).unwrap(), 1);
        assert_eq!(new_outside_since(&c, 11).unwrap(), 0, "outside the window");
        let mut same = ext(&aid, "n@brand.test", "team@brand.test");
        same.state = ST_ACTIVE;
        same.dest_kind = KIND_INTERNAL;
        same.verified_address_id = None;
        add(&c, &same, 10).unwrap();
        assert_eq!(new_outside_since(&c, 0).unwrap(), 1, "same-box isn't counted");
        // Removed and added again counts again.
        transition(&c, &a.id, ST_REMOVED, &actor(), 12).unwrap();
        add(&c, &n, 13).unwrap();
        assert_eq!(new_outside_since(&c, 0).unwrap(), 2);
        // A row from before opt-out, sorted due, doesn't count, before or after it turns on.
        let lid = mailbox(&c, "l@brand.test");
        let l = add(&c, &ext(&lid, "l@brand.test", "z@outside.test"), 20).unwrap();
        assert_eq!(new_outside_since(&c, 0).unwrap(), 3, "unsorted yet");
        let (due, stale) = legacy_sort(&c, 30).unwrap();
        assert_eq!((due.len(), stale.len()), (1, 0));
        assert_eq!(new_outside_since(&c, 0).unwrap(), 2);
        record_event(&c, &l.id, E_LEGACY_ON, "k2", &serde_json::json!({}), 31).unwrap();
        set_notice(&c, &l.id, N_SENT, 31).unwrap();
        assert_eq!(new_outside_since(&c, 0).unwrap(), 2);
        // A new row waiting its turn does count: it's a new address.
        let qid = mailbox(&c, "q@brand.test");
        let q = add(&c, &ext(&qid, "q@brand.test", "w@outside.test"), 40).unwrap();
        set_notice(&c, &q.id, N_DUE, 40).unwrap();
        assert_eq!(new_outside_since(&c, 0).unwrap(), 3);
        assert_eq!(due_waiting(&c).unwrap(), 1);
    }

    #[test]
    fn rows_from_before_opt_out_sort_by_age_once() {
        let c = conn();
        let now = 100 * 86_400;
        let fresh_id = mailbox(&c, "f@brand.test");
        let edge_id = mailbox(&c, "e@brand.test");
        let old_id = mailbox(&c, "o@brand.test");
        let fresh = create(&c, &ext(&fresh_id, "f@brand.test", "a@outside.test"), now - 86_400).unwrap();
        let edge = create(&c, &ext(&edge_id, "e@brand.test", "a@outside.test"), now - LEGACY_ON_WINDOW_SECS).unwrap();
        let old = create(&c, &ext(&old_id, "o@brand.test", "a@outside.test"), now - LEGACY_ON_WINDOW_SECS - 1).unwrap();
        let (due, stale) = legacy_sort(&c, now).unwrap();
        assert_eq!(due.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), vec![edge.id.as_str(), fresh.id.as_str()], "oldest first");
        assert_eq!(stale.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), vec![old.id.as_str()]);
        let again = legacy_sort(&c, now + 1).unwrap();
        assert!(again.0.is_empty() && again.1.is_empty(), "sorted once");
        assert_eq!(legacy_due(&c, now, 1).unwrap().len(), 1, "paced by the limit");
        assert_eq!(get(&c, &old.id).unwrap().unwrap().state, ST_PENDING_VERIFY, "stays off");
    }

    #[test]
    fn the_cap_is_the_default_until_the_owner_sets_it() {
        let c = conn();
        assert_eq!(new_outside_cap(&c).unwrap(), NEW_OUTSIDE_PER_DAY, "no mail server row");
        assert!(!set_new_outside_cap(&c, 50).unwrap(), "nothing to set on");
        c.execute("INSERT INTO mail_server (id, status, pinned_version, updated_at) VALUES (1, 'running', '0.16.25', 1)", []).unwrap();
        assert_eq!(new_outside_cap(&c).unwrap(), NEW_OUTSIDE_PER_DAY, "unset");
        assert!(set_new_outside_cap(&c, 300).unwrap());
        assert_eq!(new_outside_cap(&c).unwrap(), 300);
        assert!(MAX_DESTINATIONS == 5, "the per-mailbox limit doesn't move with it");
    }

    #[test]
    fn a_failed_notice_backs_off_and_the_clock_clears_when_it_goes() {
        let c = conn();
        let aid = mailbox(&c, "b@brand.test");
        let mut n = ext(&aid, "b@brand.test", "a@outside.test");
        n.state = ST_ACTIVE;
        n.verified_address_id = Some("v");
        let f = add(&c, &n, 1).unwrap();
        let t = 100_000;
        notice_failed(&c, &f.id, t).unwrap();
        let r = get(&c, &f.id).unwrap().unwrap();
        assert_eq!((r.notice_state.as_deref(), r.notice_failed_at, r.notice_next_at), (Some(N_FAILED), Some(t), Some(t + NOTICE_RETRY_SECS)));
        assert!(failed_notices(&c, t + NOTICE_RETRY_SECS - 1, 5).unwrap().is_empty(), "not yet");
        assert_eq!(failed_notices(&c, t + NOTICE_RETRY_SECS, 5).unwrap().len(), 1);
        // The gap grows with how long it has been failing, up to the most.
        notice_failed(&c, &f.id, t + 3 * 3600).unwrap();
        assert_eq!(get(&c, &f.id).unwrap().unwrap().notice_next_at, Some(t + 6 * 3600));
        notice_failed(&c, &f.id, t + 20 * 3600).unwrap();
        assert_eq!(get(&c, &f.id).unwrap().unwrap().notice_next_at, Some(t + 20 * 3600 + NOTICE_RETRY_MAX_SECS));
        assert_eq!(get(&c, &f.id).unwrap().unwrap().notice_failed_at, Some(t), "the clock doesn't restart");
        set_notice(&c, &f.id, N_SENT, t + 21 * 3600).unwrap();
        let r = get(&c, &f.id).unwrap().unwrap();
        assert_eq!((r.notice_state.as_deref(), r.notice_failed_at, r.notice_next_at), (Some(N_SENT), None, None));
    }

    #[test]
    fn who_paused_is_kept_and_cleared() {
        let c = conn();
        let aid = mailbox(&c, "w@brand.test");
        let mut n = ext(&aid, "w@brand.test", "a@outside.test");
        n.state = ST_ACTIVE;
        n.verified_address_id = Some("v");
        let f = add(&c, &n, 1).unwrap();
        let p = transition(&c, &f.id, ST_PAUSED, &Change { reason: Some(P_MANUAL), actor: "agent:it", ..Default::default() }, 2).unwrap();
        assert_eq!(p.paused_by.as_deref(), Some("agent:it"));
        let r = transition(&c, &f.id, ST_ACTIVE, &Change { actor: "owner", ..Default::default() }, 3).unwrap();
        assert_eq!(r.paused_by, None);
    }

    #[test]
    fn an_automatic_pause_outlives_rm_and_add_and_the_count_spans_rows() {
        let c = conn();
        let aid = mailbox(&c, "r@brand.test");
        let (mb, dest) = ("r@brand.test", "a@outside.test");
        let mut n = ext(&aid, mb, dest);
        n.state = ST_ACTIVE;
        n.verified_address_id = Some("v");
        let f = add(&c, &n, 1).unwrap();
        record_outcome(&c, &f.id, Some("1"), O_QUEUED, None, 5).unwrap();
        transition(&c, &f.id, ST_PAUSED, &Change { reason: Some(P_DAILY_CAP), actor: "k2", ..Default::default() }, 10).unwrap();
        transition(&c, &f.id, ST_REMOVED, &actor(), 11).unwrap();
        assert_eq!(recent_automatic_pause(&c, mb, dest, 0).unwrap().as_deref(), Some(P_DAILY_CAP));
        assert_eq!(recent_automatic_pause(&c, mb, dest, 11).unwrap(), None, "older than the window");
        assert_eq!(recent_automatic_pause(&c, mb, "other@outside.test", 0).unwrap(), None);
        // The removed row's copies still count for the mailbox.
        assert_eq!(mailbox_queued(&c, &aid, daily_window_start(&c, &aid, 100).unwrap()).unwrap(), 1);
        // A manual pause isn't automatic.
        let g = add(&c, &n, 20).unwrap();
        transition(&c, &g.id, ST_PAUSED, &Change { reason: Some(P_MANUAL), actor: "owner", ..Default::default() }, 21).unwrap();
        assert_eq!(recent_automatic_pause(&c, mb, dest, 15).unwrap(), None);
        // A person's resume of a daily_cap pause starts the count again.
        let h = add(&c, &ext(&aid, mb, "h@outside.test"), 22).unwrap();
        let v = Change { verified_address_id: Some("v"), actor: "k2", ..Default::default() };
        transition(&c, &h.id, ST_ACTIVE, &v, 23).unwrap();
        transition(&c, &h.id, ST_PAUSED, &Change { reason: Some(P_DAILY_CAP), actor: "k2", ..Default::default() }, 24).unwrap();
        transition(&c, &h.id, ST_ACTIVE, &actor(), 30).unwrap();
        assert_eq!(daily_window_start(&c, &aid, 100).unwrap(), 30);
    }

    #[test]
    fn a_same_box_loop_is_found_through_any_number_of_mailboxes() {
        let c = conn();
        let (a, b) = (mailbox(&c, "a@brand.test"), mailbox(&c, "b@brand.test"));
        let mut n = ext(&a, "a@brand.test", "b@brand.test");
        n.dest_kind = KIND_INTERNAL;
        n.state = ST_ACTIVE;
        add(&c, &n, 1).unwrap();
        let mut m = ext(&b, "b@brand.test", "c@brand.test");
        m.dest_kind = KIND_INTERNAL;
        m.state = ST_ACTIVE;
        add(&c, &m, 1).unwrap();
        assert!(same_box_loop(&c, "a@brand.test", "a@brand.test").unwrap());
        assert!(same_box_loop(&c, "c@brand.test", "a@brand.test").unwrap(), "c → a → b → c");
        assert!(!same_box_loop(&c, "a@brand.test", "c@brand.test").unwrap());
    }

    #[test]
    fn the_owner_hears_the_first_few_then_one_summary() {
        let c = conn();
        let t = 100_000;
        for i in 0..OWNER_NOTES_PER_HOUR {
            assert!(owner_note(&c, "change", &format!("c{i}"), t + i).unwrap(), "{i}");
        }
        assert!(!owner_note(&c, "change", "rolled-1", t + 10).unwrap());
        assert!(!owner_note(&c, "change", "rolled-2", t + 11).unwrap());
        assert!(owner_summary_due(&c, t + 10 + OWNER_SUMMARY_SECS - 1).unwrap().is_empty(), "not an hour yet");
        let got = owner_summary_due(&c, t + 10 + OWNER_SUMMARY_SECS).unwrap();
        assert_eq!(got, vec![("change".to_string(), "rolled-1".to_string()), ("change".to_string(), "rolled-2".to_string())]);
        assert!(owner_summary_due(&c, t + 20 + OWNER_SUMMARY_SECS).unwrap().is_empty(), "told once");
        // Each kind has its own slots: changes didn't spend the adds'.
        assert!(owner_note(&c, OWNER_ADD, "add", t + 12).unwrap());
        assert!(owner_note(&c, OWNER_STOP, "stop", t + 13).unwrap());
        // An hour on, the next few go one by one again.
        assert!(owner_note(&c, "change", "later", t + 5 + 3600).unwrap());
        purge(&c, t + OWNER_NOTES_KEEP_SECS + 4000).unwrap();
        let left: i64 = c.query_row("SELECT COUNT(*) FROM mail_forward_owner_notes", [], |r| r.get(0)).unwrap();
        assert_eq!(left, 0, "purged");
        // Stops roll past their own slots too.
        for i in 0..OWNER_NOTES_PER_HOUR {
            assert!(owner_note(&c, OWNER_STOP, &format!("s{i}"), t + OWNER_NOTES_KEEP_SECS + 5000).unwrap());
        }
        assert!(!owner_note(&c, OWNER_STOP, "one more", t + OWNER_NOTES_KEEP_SECS + 5000).unwrap());
    }

    #[test]
    fn a_destination_on_a_hosted_domain_with_no_mailbox_can_t_arrive() {
        let c = conn();
        c.execute("INSERT INTO mail_domains (id, domain, send_mode, status, created_at) VALUES ('d', 'Brand.Test', 'direct', 'verified', 1)", []).unwrap();
        let aid = mailbox(&c, "real@brand.test");
        assert!(dest_cant_arrive(&c, "ghost@brand.test", None));
        assert!(!dest_cant_arrive(&c, "real@brand.test", None), "a mailbox here");
        assert!(!dest_cant_arrive(&c, "me@outside.test", None));
        assert!(dest_cant_arrive(&c, "x@mail.host.test", Some("mail.host.test")), "the mail host name");
        let f = add(&c, &ext(&aid, "real@brand.test", "ghost@brand.test"), 1).unwrap();
        add(&c, &ext(&aid, "real@brand.test", "me@outside.test"), 1).unwrap();
        let rows = cant_arrive_unnoted(&c, None).unwrap();
        assert_eq!(rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), vec![f.id.as_str()]);
        record_event(&c, &f.id, E_CANT_ARRIVE, "k2", &serde_json::json!({}), 2).unwrap();
        assert!(cant_arrive_unnoted(&c, None).unwrap().is_empty(), "told once");
        // A mailbox retired later makes a same-box row one of these.
        let bid = mailbox(&c, "b@brand.test");
        let mut same = ext(&bid, "b@brand.test", "real@brand.test");
        same.dest_kind = KIND_INTERNAL;
        same.state = ST_ACTIVE;
        let g = add(&c, &same, 3).unwrap();
        c.execute("UPDATE mail_addresses SET status = 'retired' WHERE address = 'real@brand.test'", []).unwrap();
        assert_eq!(cant_arrive_unnoted(&c, None).unwrap().into_iter().map(|r| r.id).collect::<Vec<_>>(), vec![g.id]);
    }

    #[test]
    fn the_notice_pace_stays_under_the_system_mail_budget() {
        // 200 an hour is email_notify::budget::SYSTEM_HOURLY (daemon crate).
        assert!(NOTICES_BURST_PER_HOUR + 60 * NOTICES_PER_PASS as i64 <= 180, "leaves room for password resets");
    }

    #[test]
    fn the_mailbox_count_adds_up_every_destination() {
        let c = conn();
        let aid = mailbox(&c, "m@brand.test");
        let a = add(&c, &ext(&aid, "m@brand.test", "a@outside.test"), 1).unwrap();
        let b = add(&c, &ext(&aid, "m@brand.test", "b@outside.test"), 1).unwrap();
        record_outcome(&c, &a.id, Some("1"), O_QUEUED, None, 5).unwrap();
        record_outcome(&c, &b.id, Some("2"), O_QUEUED, None, 5).unwrap();
        record_outcome(&c, &b.id, Some("2"), O_DELIVERED, None, 6).unwrap();
        assert_eq!(mailbox_queued(&c, &aid, 0).unwrap(), 2);
        assert_eq!(mailbox_queued(&c, &aid, 6).unwrap(), 0);
    }

    #[test]
    fn outcomes_count_and_dedupe() {
        let c = conn();
        let aid = mailbox(&c, "c@brand.test");
        let f = create(&c, &ext(&aid, "c@brand.test", "d@outside.test"), 1).unwrap();
        assert!(link_queue(&c, "111", &f.id, 5).unwrap());
        assert!(!link_queue(&c, "111", &f.id, 6).unwrap());
        assert_eq!(forward_for_queue(&c, "111").unwrap().as_deref(), Some(f.id.as_str()));
        assert!(record_outcome(&c, &f.id, Some("111"), O_QUEUED, None, 5).unwrap());
        assert!(!record_outcome(&c, &f.id, Some("111"), O_QUEUED, None, 5).unwrap(), "repeat event");
        record_outcome(&c, &f.id, Some("112"), O_DELIVERED, None, 6).unwrap();
        record_outcome(&c, &f.id, Some("113"), O_REFUSED_USER, Some("550"), 7).unwrap();
        record_outcome(&c, &f.id, Some("114"), O_REFUSED_USER, Some("550"), 8).unwrap();
        let k = counts(&c, &f.id, 0).unwrap();
        assert_eq!((k.queued, k.delivered, k.refused_user), (1, 1, 2));
        assert_eq!(user_refusals_in_a_row(&c, &f.id, 0).unwrap(), 2);
        record_outcome(&c, &f.id, Some("115"), O_DELIVERED, None, 9).unwrap();
        assert_eq!(user_refusals_in_a_row(&c, &f.id, 0).unwrap(), 0, "a delivery breaks the run");
        assert_eq!(box_counts(&c, 0).unwrap().refused(), 2);
        purge(&c, 6 + QUEUE_KEEP_SECS).unwrap();
        assert_eq!(forward_for_queue(&c, "111").unwrap(), None);
    }

    #[test]
    fn person_link() {
        let c = conn();
        let aid = mailbox(&c, "p@brand.test");
        assert_eq!(person_principal(&c, &aid).unwrap(), None);
        assert!(set_person_principal(&c, &aid, Some("pat")).unwrap());
        assert_eq!(person_principal(&c, &aid).unwrap().as_deref(), Some("pat"));
        assert_eq!(mailboxes_of_person(&c, "pat").unwrap(), vec![(aid.clone(), "p@brand.test".to_string())]);
        set_person_principal(&c, &aid, None).unwrap();
        assert!(mailboxes_of_person(&c, "pat").unwrap().is_empty());
    }
}
