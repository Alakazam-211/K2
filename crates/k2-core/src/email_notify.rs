//! Email Notifications: the k2.db store (migration 0227,
//! prd-email-notifications-core-v1 §5.1).
//!
//! The daemon's `email_notify` module owns the rules (who may send, the
//! fence, budgets, composition); this module is only the rows:
//!
//! - **Senders** (`email_senders`): one row per address, never a
//!   `mail_addresses` row (E1). `owner_kind` is `None` (not given to anyone
//!   yet), `system` (the box's one system sender) or `app` (one App's).
//!   A removed sender stays as `retired` for [`RESERVE_DAYS`] (E26).
//! - **Relays** (`email_relays`): host, port, TLS, username. The password is
//!   in the vault, never here (E12).
//! - **Per-App state** (`email_app_state`).
//! - **The log** (`email_notify_log`): metadata only, never a body (E7).
//! - **Idempotency** (`email_idempotency`), **settings**
//!   (`email_notify_settings`).
//!
//! Nothing here logs an address or a subject (E22).

use rusqlite::{params, Connection, OptionalExtension};

/// A removed sender's address stays reserved this long (E26).
pub const RESERVE_DAYS: i64 = 30;
/// Idempotency keys are remembered this long (E19).
pub const IDEMPOTENCY_SECS: i64 = 24 * 3600;
/// Log retention bounds (Q8: 30 days by default, the owner picks 7–90).
pub const MIN_RETENTION_DAYS: i64 = 7;
pub const MAX_RETENTION_DAYS: i64 = 90;
pub const DEFAULT_RETENTION_DAYS: i64 = 30;

pub const PATH_HOSTED: &str = "hosted";
pub const PATH_RELAY: &str = "relay";
pub const OWNER_SYSTEM: &str = "system";
pub const OWNER_APP: &str = "app";

pub const STATE_READY: &str = "ready";
pub const STATE_NEEDS_DMARC: &str = "needs_dmarc";
pub const STATE_NEEDS_TEST: &str = "needs_test";
pub const STATE_OFF: &str = "off";
pub const STATE_RETIRED: &str = "retired";

pub const ORIGIN_SYSTEM: &str = "system";
pub const ORIGIN_APP: &str = "app";
pub const ORIGIN_AGENT: &str = "agent";

pub const ST_QUEUED: &str = "queued";
pub const ST_SCHEDULED: &str = "scheduled";
pub const ST_HELD: &str = "held";
pub const ST_SUBMITTED: &str = "submitted";
pub const ST_FAILED: &str = "failed";
pub const ST_REFUSED: &str = "refused";

pub const SCOPE_USERS: &str = "users";
pub const SCOPE_ANY: &str = "any";

fn err(what: &str) -> impl Fn(rusqlite::Error) -> String + '_ {
    move |e| format!("email notifications {what}: {e}")
}

fn new_id(prefix: &str) -> String {
    format!("{prefix}_{}", uuid::Uuid::new_v4().simple())
}

fn json_list(v: &[String]) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "[]".to_string())
}

fn parse_list(raw: Option<String>) -> Vec<String> {
    raw.and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok()).unwrap_or_default()
}

// ── senders ──────────────────────────────────────────────────────────

/// One `email_senders` row.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Sender {
    pub id: String,
    pub address: String,
    pub domain: String,
    pub path: String,
    #[serde(skip)]
    pub stalwart_account_id: Option<String>,
    pub relay_id: Option<String>,
    pub owner_kind: Option<String>,
    pub app_service_id: Option<String>,
    pub from_name: String,
    pub from_name_locked: bool,
    pub reply_to: Vec<String>,
    pub state: String,
    pub bounce_hints: i64,
    pub last_sweep_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
    pub retired_at: Option<i64>,
}

impl Sender {
    pub fn is_system(&self) -> bool {
        self.owner_kind.as_deref() == Some(OWNER_SYSTEM)
    }
    pub fn app(&self) -> Option<&str> {
        self.app_service_id.as_deref().filter(|_| self.owner_kind.as_deref() == Some(OWNER_APP))
    }
    pub fn is_live(&self) -> bool {
        self.state != STATE_RETIRED
    }
}

const SENDER_COLS: &str = "id, address, domain, path, stalwart_account_id, relay_id, owner_kind, app_service_id, from_name, \
     from_name_locked, reply_to_json, state, bounce_hints, last_sweep_at, created_at, updated_at, retired_at";

fn map_sender(r: &rusqlite::Row) -> rusqlite::Result<Sender> {
    Ok(Sender {
        id: r.get(0)?,
        address: r.get(1)?,
        domain: r.get(2)?,
        path: r.get(3)?,
        stalwart_account_id: r.get(4)?,
        relay_id: r.get(5)?,
        owner_kind: r.get(6)?,
        app_service_id: r.get(7)?,
        from_name: r.get(8)?,
        from_name_locked: r.get::<_, i64>(9)? != 0,
        reply_to: parse_list(r.get(10)?),
        state: r.get(11)?,
        bounce_hints: r.get(12)?,
        last_sweep_at: r.get(13)?,
        created_at: r.get(14)?,
        updated_at: r.get(15)?,
        retired_at: r.get(16)?,
    })
}

/// A new sender (the daemon checked the address and created the account).
#[derive(Debug, Clone)]
pub struct NewSender<'a> {
    pub address: &'a str,
    pub domain: &'a str,
    pub path: &'a str,
    pub stalwart_account_id: Option<&'a str>,
    pub relay_id: Option<&'a str>,
    pub from_name: &'a str,
    pub from_name_locked: bool,
    pub reply_to: &'a [String],
    pub state: &'a str,
    pub now: i64,
}

pub fn insert_sender(conn: &Connection, s: &NewSender<'_>) -> Result<Sender, String> {
    let id = new_id("ens");
    conn.execute(
        "INSERT INTO email_senders (id, address, domain, path, stalwart_account_id, relay_id, owner_kind, app_service_id, \
           from_name, from_name_locked, reply_to_json, state, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, NULL, ?7, ?8, ?9, ?10, ?11, ?11)",
        params![
            id,
            s.address,
            s.domain,
            s.path,
            s.stalwart_account_id,
            s.relay_id,
            s.from_name,
            s.from_name_locked as i64,
            json_list(s.reply_to),
            s.state,
            s.now
        ],
    )
    .map_err(err("sender insert"))?;
    get_sender(conn, &id)?.ok_or_else(|| "email notifications sender insert: row vanished".to_string())
}

pub fn get_sender(conn: &Connection, id: &str) -> Result<Option<Sender>, String> {
    conn.query_row(&format!("SELECT {SENDER_COLS} FROM email_senders WHERE id = ?1"), params![id], map_sender)
        .optional()
        .map_err(err("sender read"))
}

/// The row for `address` (any state, retired included: it reserves the
/// address).
pub fn get_sender_by_address(conn: &Connection, address: &str) -> Result<Option<Sender>, String> {
    conn.query_row(
        &format!("SELECT {SENDER_COLS} FROM email_senders WHERE address = ?1"),
        params![address.trim().to_ascii_lowercase()],
        map_sender,
    )
    .optional()
    .map_err(err("sender read"))
}

/// A sender by id or by address.
pub fn find_sender(conn: &Connection, id_or_address: &str) -> Result<Option<Sender>, String> {
    let t = id_or_address.trim();
    if t.contains('@') {
        get_sender_by_address(conn, t)
    } else {
        get_sender(conn, t)
    }
}

pub fn list_senders(conn: &Connection, include_retired: bool) -> Result<Vec<Sender>, String> {
    let sql = if include_retired {
        format!("SELECT {SENDER_COLS} FROM email_senders ORDER BY created_at, address")
    } else {
        format!("SELECT {SENDER_COLS} FROM email_senders WHERE state <> 'retired' ORDER BY created_at, address")
    };
    let mut st = conn.prepare(&sql).map_err(err("sender list"))?;
    let rows = st.query_map([], map_sender).map_err(err("sender list"))?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(err("sender list"))
}

/// The box's system sender (live), if one is set.
pub fn system_sender(conn: &Connection) -> Result<Option<Sender>, String> {
    conn.query_row(
        &format!("SELECT {SENDER_COLS} FROM email_senders WHERE owner_kind = 'system' AND state <> 'retired' LIMIT 1"),
        [],
        map_sender,
    )
    .optional()
    .map_err(err("system sender read"))
}

/// The sender granted to App `app` (live), if any.
pub fn app_sender(conn: &Connection, app: &str) -> Result<Option<Sender>, String> {
    conn.query_row(
        &format!(
            "SELECT {SENDER_COLS} FROM email_senders WHERE owner_kind = 'app' AND app_service_id = ?1 AND state <> 'retired' \
             ORDER BY created_at LIMIT 1"
        ),
        params![app],
        map_sender,
    )
    .optional()
    .map_err(err("App sender read"))
}

/// Every live sender on `domain`.
pub fn senders_on_domain(conn: &Connection, domain: &str) -> Result<Vec<Sender>, String> {
    let mut st = conn
        .prepare(&format!("SELECT {SENDER_COLS} FROM email_senders WHERE domain = ?1 AND state <> 'retired'"))
        .map_err(err("sender list"))?;
    let rows = st.query_map(params![domain.trim().to_ascii_lowercase()], map_sender).map_err(err("sender list"))?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(err("sender list"))
}

/// Give a sender to the system (`Some("system")`, `None`), an App
/// (`Some("app")`, `Some(app)`) or nobody (`None`, `None`).
pub fn set_sender_owner(conn: &Connection, id: &str, kind: Option<&str>, app: Option<&str>, now: i64) -> Result<(), String> {
    let n = conn
        .execute(
            "UPDATE email_senders SET owner_kind = ?2, app_service_id = ?3, updated_at = ?4 WHERE id = ?1 AND state <> 'retired'",
            params![id, kind, app, now],
        )
        .map_err(err("sender owner"))?;
    if n == 1 {
        Ok(())
    } else {
        Err("email notifications sender owner: no live sender by that id".to_string())
    }
}

pub fn set_sender_state(conn: &Connection, id: &str, state: &str, now: i64) -> Result<(), String> {
    conn.execute(
        "UPDATE email_senders SET state = ?2, updated_at = ?3 WHERE id = ?1 AND state <> 'retired'",
        params![id, state, now],
    )
    .map_err(err("sender state"))?;
    Ok(())
}

pub fn set_sender_name(conn: &Connection, id: &str, name: &str, locked: bool, reply_to: &[String], now: i64) -> Result<(), String> {
    conn.execute(
        "UPDATE email_senders SET from_name = ?2, from_name_locked = ?3, reply_to_json = ?4, updated_at = ?5 WHERE id = ?1",
        params![id, name, locked as i64, json_list(reply_to), now],
    )
    .map_err(err("sender name"))?;
    Ok(())
}

/// Retire a sender (E26): nobody owns it, it can't send, and its address
/// stays reserved until [`purge_retired`] runs [`RESERVE_DAYS`] later. An
/// App that used it loses it (its email turns off).
pub fn retire_sender(conn: &Connection, id: &str, now: i64) -> Result<(), String> {
    conn.execute(
        "UPDATE email_senders SET state = 'retired', owner_kind = NULL, app_service_id = NULL, retired_at = ?2, updated_at = ?2 \
         WHERE id = ?1",
        params![id, now],
    )
    .map_err(err("sender retire"))?;
    conn.execute(
        "UPDATE email_app_state SET enabled = 0, sender_id = NULL, off_reason = 'sender_removed', updated_at = ?2 WHERE sender_id = ?1",
        params![id, now],
    )
    .map_err(err("sender retire"))?;
    Ok(())
}

/// Retired senders whose reservation ended (`retired_at` before
/// `before`): their rows go, freeing the address. Returns how many.
pub fn purge_retired(conn: &Connection, before: i64) -> Result<usize, String> {
    conn.execute("DELETE FROM email_senders WHERE state = 'retired' AND retired_at IS NOT NULL AND retired_at < ?1", params![before])
        .map_err(err("sender purge"))
}

/// The sweep's mark (E5): DSNs found, when it ran.
pub fn record_sweep(conn: &Connection, id: &str, dsns: i64, now: i64) -> Result<(), String> {
    conn.execute(
        "UPDATE email_senders SET bounce_hints = bounce_hints + ?2, last_sweep_at = ?3 WHERE id = ?1",
        params![id, dsns, now],
    )
    .map_err(err("sender sweep mark"))?;
    Ok(())
}

/// Is `domain` carrying a live sender (the E4 integrity fence)?
pub fn domain_has_sender(conn: &Connection, domain: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT COUNT(*) FROM email_senders WHERE domain = ?1 AND state <> 'retired'",
        params![domain.trim().to_ascii_lowercase()],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n > 0)
    .map_err(err("domain fence"))
}

/// Is `address` a sender's address, live or still reserved (E2)?
pub fn is_reserved(conn: &Connection, address: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT COUNT(*) FROM email_senders WHERE address = ?1",
        params![address.trim().to_ascii_lowercase()],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n > 0)
    .map_err(err("reserved check"))
}

// ── relays ───────────────────────────────────────────────────────────

/// One `email_relays` row. The password is the vault's.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Relay {
    pub id: String,
    pub host: String,
    pub port: u16,
    pub tls: String,
    pub username: String,
    pub from_domain: String,
    pub last_test_at: Option<i64>,
    pub last_test_ok: Option<bool>,
    pub created_at: i64,
}

const RELAY_COLS: &str = "id, host, port, tls, username, from_domain, last_test_at, last_test_ok, created_at";

fn map_relay(r: &rusqlite::Row) -> rusqlite::Result<Relay> {
    Ok(Relay {
        id: r.get(0)?,
        host: r.get(1)?,
        port: r.get::<_, i64>(2)?.clamp(0, 65535) as u16,
        tls: r.get(3)?,
        username: r.get(4)?,
        from_domain: r.get(5)?,
        last_test_at: r.get(6)?,
        last_test_ok: r.get::<_, Option<i64>>(7)?.map(|v| v != 0),
        created_at: r.get(8)?,
    })
}

pub fn insert_relay(conn: &Connection, host: &str, port: u16, tls: &str, username: &str, from_domain: &str, now: i64) -> Result<Relay, String> {
    let id = new_id("enr");
    conn.execute(
        "INSERT INTO email_relays (id, host, port, tls, username, from_domain, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![id, host, port as i64, tls, username, from_domain, now],
    )
    .map_err(err("relay insert"))?;
    get_relay(conn, &id)?.ok_or_else(|| "email notifications relay insert: row vanished".to_string())
}

pub fn get_relay(conn: &Connection, id: &str) -> Result<Option<Relay>, String> {
    conn.query_row(&format!("SELECT {RELAY_COLS} FROM email_relays WHERE id = ?1"), params![id], map_relay)
        .optional()
        .map_err(err("relay read"))
}

pub fn list_relays(conn: &Connection) -> Result<Vec<Relay>, String> {
    let mut st = conn.prepare(&format!("SELECT {RELAY_COLS} FROM email_relays ORDER BY created_at")).map_err(err("relay list"))?;
    let rows = st.query_map([], map_relay).map_err(err("relay list"))?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(err("relay list"))
}

pub fn delete_relay(conn: &Connection, id: &str) -> Result<bool, String> {
    conn.execute("DELETE FROM email_relays WHERE id = ?1", params![id]).map(|n| n == 1).map_err(err("relay delete"))
}

pub fn record_relay_test(conn: &Connection, id: &str, ok: bool, now: i64) -> Result<(), String> {
    conn.execute("UPDATE email_relays SET last_test_at = ?2, last_test_ok = ?3 WHERE id = ?1", params![id, now, ok as i64])
        .map_err(err("relay test mark"))?;
    Ok(())
}

/// Live senders on relay `id`.
pub fn relay_in_use(conn: &Connection, id: &str) -> Result<bool, String> {
    conn.query_row("SELECT COUNT(*) FROM email_senders WHERE relay_id = ?1 AND state <> 'retired'", params![id], |r| r.get::<_, i64>(0))
        .map(|n| n > 0)
        .map_err(err("relay use"))
}

// ── per-App state ────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppState {
    pub app_service_id: String,
    pub enabled: bool,
    pub sender_id: Option<String>,
    pub recipient_scope: String,
    pub turned_on_at: Option<i64>,
    pub turned_on_by: Option<String>,
    pub off_reason: Option<String>,
    pub updated_at: i64,
}

impl AppState {
    pub fn off(app: &str) -> AppState {
        AppState {
            app_service_id: app.to_string(),
            enabled: false,
            sender_id: None,
            recipient_scope: SCOPE_USERS.to_string(),
            turned_on_at: None,
            turned_on_by: None,
            off_reason: None,
            updated_at: 0,
        }
    }
}

pub fn app_state(conn: &Connection, app: &str) -> Result<AppState, String> {
    let row = conn
        .query_row(
            "SELECT app_service_id, enabled, sender_id, recipient_scope, turned_on_at, turned_on_by, off_reason, updated_at \
             FROM email_app_state WHERE app_service_id = ?1",
            params![app],
            |r| {
                Ok(AppState {
                    app_service_id: r.get(0)?,
                    enabled: r.get::<_, i64>(1)? != 0,
                    sender_id: r.get(2)?,
                    recipient_scope: r.get(3)?,
                    turned_on_at: r.get(4)?,
                    turned_on_by: r.get(5)?,
                    off_reason: r.get(6)?,
                    updated_at: r.get(7)?,
                })
            },
        )
        .optional()
        .map_err(err("App state read"))?;
    Ok(row.unwrap_or_else(|| AppState::off(app)))
}

/// Turn an App's email on with `sender_id` and `scope` (the owner's
/// confirmed step). The sender row is given to the App in the same
/// transaction.
pub fn set_app_on(conn: &Connection, app: &str, sender_id: &str, scope: &str, by: &str, now: i64) -> Result<(), String> {
    let tx = conn.unchecked_transaction().map_err(err("App on"))?;
    // A sender the App held before (a different one) goes back to nobody.
    tx.execute(
        "UPDATE email_senders SET owner_kind = NULL, app_service_id = NULL, updated_at = ?3 \
         WHERE owner_kind = 'app' AND app_service_id = ?1 AND id <> ?2",
        params![app, sender_id, now],
    )
    .map_err(err("App on"))?;
    let n = tx
        .execute(
            "UPDATE email_senders SET owner_kind = 'app', app_service_id = ?2, updated_at = ?3 \
             WHERE id = ?1 AND state <> 'retired' AND (owner_kind IS NULL OR (owner_kind = 'app' AND app_service_id = ?2))",
            params![sender_id, app, now],
        )
        .map_err(err("App on"))?;
    if n != 1 {
        return Err("email notifications App on: the sender is someone else's or gone".to_string());
    }
    tx.execute(
        "INSERT INTO email_app_state (app_service_id, enabled, sender_id, recipient_scope, turned_on_at, turned_on_by, off_reason, updated_at) \
         VALUES (?1, 1, ?2, ?3, ?4, ?5, NULL, ?4) \
         ON CONFLICT(app_service_id) DO UPDATE SET enabled = 1, sender_id = ?2, recipient_scope = ?3, turned_on_at = ?4, \
           turned_on_by = ?5, off_reason = NULL, updated_at = ?4",
        params![app, sender_id, scope, now, by],
    )
    .map_err(err("App on"))?;
    tx.commit().map_err(err("App on"))
}

/// Turn an App's email off (the sender stays the App's, so turning it
/// back on keeps the same From).
pub fn set_app_off(conn: &Connection, app: &str, reason: &str, now: i64) -> Result<(), String> {
    conn.execute(
        "INSERT INTO email_app_state (app_service_id, enabled, recipient_scope, off_reason, updated_at) VALUES (?1, 0, 'users', ?2, ?3) \
         ON CONFLICT(app_service_id) DO UPDATE SET enabled = 0, off_reason = ?2, updated_at = ?3",
        params![app, reason, now],
    )
    .map_err(err("App off"))?;
    Ok(())
}

/// An App went away: its state goes and its sender returns to nobody.
pub fn forget_app(conn: &Connection, app: &str, now: i64) -> Result<(), String> {
    conn.execute("DELETE FROM email_app_state WHERE app_service_id = ?1", params![app]).map_err(err("App forget"))?;
    conn.execute(
        "UPDATE email_senders SET owner_kind = NULL, app_service_id = NULL, updated_at = ?2 WHERE owner_kind = 'app' AND app_service_id = ?1",
        params![app, now],
    )
    .map_err(err("App forget"))?;
    conn.execute("DELETE FROM email_idempotency WHERE app_service_id = ?1", params![app]).map_err(err("App forget"))?;
    Ok(())
}

// ── the log ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogRow {
    pub id: String,
    pub sender_id: String,
    pub origin: String,
    pub app_service_id: Option<String>,
    pub purpose: Option<String>,
    pub to: Vec<String>,
    pub recipient_count: i64,
    pub subject: String,
    pub tags: Vec<String>,
    pub status: String,
    pub error_code: Option<String>,
    pub message_id: Option<String>,
    pub send_after: Option<i64>,
    pub outbound_id: Option<String>,
    pub scope_any: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

const LOG_COLS: &str = "id, sender_id, origin, app_service_id, purpose, to_json, recipient_count, subject, tags_json, status, \
     error_code, message_id, send_after, outbound_id, scope_any, created_at, updated_at";

fn map_log(r: &rusqlite::Row) -> rusqlite::Result<LogRow> {
    Ok(LogRow {
        id: r.get(0)?,
        sender_id: r.get(1)?,
        origin: r.get(2)?,
        app_service_id: r.get(3)?,
        purpose: r.get(4)?,
        to: parse_list(r.get(5)?),
        recipient_count: r.get(6)?,
        subject: r.get(7)?,
        tags: parse_list(r.get(8)?),
        status: r.get(9)?,
        error_code: r.get(10)?,
        message_id: r.get(11)?,
        send_after: r.get(12)?,
        outbound_id: r.get(13)?,
        scope_any: r.get::<_, i64>(14)? != 0,
        created_at: r.get(15)?,
        updated_at: r.get(16)?,
    })
}

/// A new log row. `to` is every envelope recipient (to + cc + bcc).
#[derive(Debug, Clone)]
pub struct NewLog<'a> {
    pub sender_id: &'a str,
    pub origin: &'a str,
    pub app_service_id: Option<&'a str>,
    pub purpose: Option<&'a str>,
    pub to: &'a [String],
    pub subject: &'a str,
    pub tags: &'a [String],
    pub status: &'a str,
    pub error_code: Option<&'a str>,
    pub message_id: Option<&'a str>,
    pub send_after: Option<i64>,
    pub scope_any: bool,
    pub now: i64,
}

pub fn insert_log(conn: &Connection, l: &NewLog<'_>) -> Result<String, String> {
    let id = new_id("enl");
    conn.execute(
        "INSERT INTO email_notify_log (id, sender_id, origin, app_service_id, purpose, to_json, recipient_count, subject, tags_json, \
           status, error_code, message_id, send_after, scope_any, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?15)",
        params![
            id,
            l.sender_id,
            l.origin,
            l.app_service_id,
            l.purpose,
            json_list(l.to),
            l.to.len() as i64,
            l.subject,
            json_list(l.tags),
            l.status,
            l.error_code,
            l.message_id,
            l.send_after,
            l.scope_any as i64,
            l.now
        ],
    )
    .map_err(err("log insert"))?;
    Ok(id)
}

/// Compare-and-set a row's status. `Ok(false)`: it wasn't `from`.
pub fn set_log_status(conn: &Connection, id: &str, from: &str, to: &str, error_code: Option<&str>, now: i64) -> Result<bool, String> {
    conn.execute(
        "UPDATE email_notify_log SET status = ?3, error_code = COALESCE(?4, error_code), updated_at = ?5 WHERE id = ?1 AND status = ?2",
        params![id, from, to, error_code, now],
    )
    .map(|n| n == 1)
    .map_err(err("log status"))
}

/// `from` → `submitted`, with the Message-ID K2 stamped.
pub fn set_log_submitted(conn: &Connection, id: &str, from: &str, message_id: &str, now: i64) -> Result<bool, String> {
    conn.execute(
        "UPDATE email_notify_log SET status = 'submitted', message_id = ?3, updated_at = ?4 WHERE id = ?1 AND status = ?2",
        params![id, from, message_id, now],
    )
    .map(|n| n == 1)
    .map_err(err("log status"))
}

/// Refused rows an App made since `since` (the refusal-log flood guard).
pub fn count_refused(conn: &Connection, app: &str, since: i64) -> Result<i64, String> {
    conn.query_row(
        "SELECT COUNT(*) FROM email_notify_log WHERE app_service_id = ?1 AND status = 'refused' AND created_at >= ?2",
        params![app, since],
        |r| r.get(0),
    )
    .map_err(err("refused count"))
}

/// Held rows (E15) and their mail_outbound ids, oldest first.
pub fn held_rows(conn: &Connection, limit: usize) -> Result<Vec<LogRow>, String> {
    let mut st = conn
        .prepare(&format!("SELECT {LOG_COLS} FROM email_notify_log WHERE status = 'held' ORDER BY created_at LIMIT ?1"))
        .map_err(err("held list"))?;
    let rows = st.query_map(params![limit as i64], map_log).map_err(err("held list"))?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(err("held list"))
}

pub fn set_log_outbound(conn: &Connection, id: &str, outbound_id: &str) -> Result<(), String> {
    conn.execute("UPDATE email_notify_log SET outbound_id = ?2 WHERE id = ?1", params![id, outbound_id]).map_err(err("log outbound"))?;
    Ok(())
}

pub fn get_log(conn: &Connection, id: &str) -> Result<Option<LogRow>, String> {
    conn.query_row(&format!("SELECT {LOG_COLS} FROM email_notify_log WHERE id = ?1"), params![id], map_log)
        .optional()
        .map_err(err("log read"))
}

/// The row a held mail_outbound message belongs to.
pub fn log_for_outbound(conn: &Connection, outbound_id: &str) -> Result<Option<LogRow>, String> {
    conn.query_row(&format!("SELECT {LOG_COLS} FROM email_notify_log WHERE outbound_id = ?1"), params![outbound_id], map_log)
        .optional()
        .map_err(err("log read"))
}

/// A log listing filter.
#[derive(Debug, Clone, Default)]
pub struct LogFilter<'a> {
    pub app: Option<&'a str>,
    pub status: Option<&'a str>,
    pub tag: Option<&'a str>,
    /// Rows created before this (the `before` cursor).
    pub before: Option<i64>,
    pub limit: usize,
}

/// Newest first.
pub fn list_log(conn: &Connection, f: &LogFilter<'_>) -> Result<Vec<LogRow>, String> {
    let limit = f.limit.clamp(1, 500) as i64;
    let mut st = conn
        .prepare(&format!(
            "SELECT {LOG_COLS} FROM email_notify_log \
             WHERE (?1 IS NULL OR app_service_id = ?1) AND (?2 IS NULL OR status = ?2) AND (?3 IS NULL OR created_at < ?3) \
             ORDER BY created_at DESC, id DESC LIMIT ?4"
        ))
        .map_err(err("log list"))?;
    let rows = st.query_map(params![f.app, f.status, f.before, limit * 4], map_log).map_err(err("log list"))?;
    let mut out = Vec::new();
    for r in rows {
        let r = r.map_err(err("log list"))?;
        if f.tag.is_some_and(|t| !r.tags.iter().any(|x| x == t)) {
            continue;
        }
        out.push(r);
        if out.len() as i64 >= limit {
            break;
        }
    }
    Ok(out)
}

/// Scheduled rows whose time came.
pub fn due_scheduled(conn: &Connection, now: i64, limit: usize) -> Result<Vec<LogRow>, String> {
    let mut st = conn
        .prepare(&format!(
            "SELECT {LOG_COLS} FROM email_notify_log WHERE status = 'scheduled' AND send_after IS NOT NULL AND send_after <= ?1 \
             ORDER BY send_after LIMIT ?2"
        ))
        .map_err(err("due list"))?;
    let rows = st.query_map(params![now, limit as i64], map_log).map_err(err("due list"))?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(err("due list"))
}

/// Rows older than `before` go (the retention sweep, E22).
pub fn purge_log_before(conn: &Connection, before: i64) -> Result<usize, String> {
    conn.execute("DELETE FROM email_notify_log WHERE created_at < ?1 AND status NOT IN ('scheduled', 'held')", params![before])
        .map_err(err("log purge"))
}

/// Every log row naming `address` goes (an App user was removed, E22).
pub fn purge_recipient(conn: &Connection, address: &str) -> Result<usize, String> {
    let a = address.trim().to_ascii_lowercase();
    if a.is_empty() || !a.contains('@') {
        return Ok(0);
    }
    let needle = serde_json::to_string(&a).unwrap_or_default();
    conn.execute("DELETE FROM email_notify_log WHERE instr(to_json, ?1) > 0", params![needle]).map_err(err("log purge"))
}

/// What a budget counts (E17): envelope recipients over log rows that
/// were (or will be) handed off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Bucket<'a> {
    /// One App's sends (its own and its agent's).
    App(&'a str),
    /// One App's sends while its recipient scope is `any`.
    AppAny(&'a str),
    /// App-made sends from senders on one domain (warm-up).
    Domain(&'a str),
    /// K2's own system mail.
    System,
    /// Everything on the box.
    Box,
}

/// `(recipients counted since `since`, the oldest counted row's time)`.
pub fn sum_recipients(conn: &Connection, bucket: &Bucket<'_>, since: i64) -> Result<(i64, Option<i64>), String> {
    let live = "status IN ('queued', 'scheduled', 'held', 'submitted')";
    let (sql, arg): (String, Option<&str>) = match bucket {
        Bucket::App(a) => (format!("app_service_id = ?2 AND origin IN ('app', 'agent') AND {live}"), Some(a)),
        Bucket::AppAny(a) => (format!("app_service_id = ?2 AND origin IN ('app', 'agent') AND scope_any = 1 AND {live}"), Some(a)),
        Bucket::Domain(d) => (
            format!("origin IN ('app', 'agent') AND {live} AND sender_id IN (SELECT id FROM email_senders WHERE domain = ?2)"),
            Some(d),
        ),
        Bucket::System => (format!("origin = 'system' AND {live}"), None),
        Bucket::Box => (live.to_string(), None),
    };
    let q = format!("SELECT COALESCE(SUM(recipient_count), 0), MIN(created_at) FROM email_notify_log WHERE created_at >= ?1 AND {sql}");
    let r = match arg {
        Some(a) => conn.query_row(&q, params![since, a], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?))),
        None => conn.query_row(&q, params![since], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?))),
    };
    r.map_err(err("budget count"))
}

// ── idempotency ──────────────────────────────────────────────────────

/// `(request sha256, log id)` for an App's key seen since `since`.
pub fn idempotency_get(conn: &Connection, app: &str, key: &str, since: i64) -> Result<Option<(String, String)>, String> {
    conn.query_row(
        "SELECT request_sha256, log_id FROM email_idempotency WHERE app_service_id = ?1 AND key = ?2 AND created_at >= ?3",
        params![app, key, since],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .optional()
    .map_err(err("idempotency read"))
}

/// Remember a key (an older, expired row with the same key is replaced).
pub fn idempotency_put(conn: &Connection, app: &str, key: &str, sha: &str, log_id: &str, now: i64) -> Result<(), String> {
    conn.execute(
        "INSERT INTO email_idempotency (app_service_id, key, request_sha256, log_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5) \
         ON CONFLICT(app_service_id, key) DO UPDATE SET request_sha256 = ?3, log_id = ?4, created_at = ?5",
        params![app, key, sha, log_id, now],
    )
    .map_err(err("idempotency write"))?;
    Ok(())
}

pub fn idempotency_prune(conn: &Connection, before: i64) -> Result<usize, String> {
    conn.execute("DELETE FROM email_idempotency WHERE created_at < ?1", params![before]).map_err(err("idempotency prune"))
}

// ── settings ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub log_retention_days: i64,
    pub owner_email: Option<String>,
}

pub fn settings(conn: &Connection) -> Result<Settings, String> {
    let row = conn
        .query_row("SELECT log_retention_days, owner_email FROM email_notify_settings WHERE id = 1", [], |r| {
            Ok(Settings { log_retention_days: r.get(0)?, owner_email: r.get(1)? })
        })
        .optional()
        .map_err(err("settings read"))?;
    Ok(row.unwrap_or(Settings { log_retention_days: DEFAULT_RETENTION_DAYS, owner_email: None }))
}

pub fn set_retention(conn: &Connection, days: i64, now: i64) -> Result<(), String> {
    if !(MIN_RETENTION_DAYS..=MAX_RETENTION_DAYS).contains(&days) {
        return Err(format!("log retention is {MIN_RETENTION_DAYS}–{MAX_RETENTION_DAYS} days"));
    }
    conn.execute(
        "INSERT INTO email_notify_settings (id, log_retention_days, updated_at) VALUES (1, ?1, ?2) \
         ON CONFLICT(id) DO UPDATE SET log_retention_days = ?1, updated_at = ?2",
        params![days, now],
    )
    .map_err(err("settings write"))?;
    Ok(())
}

pub fn set_owner_email(conn: &Connection, email: Option<&str>, now: i64) -> Result<(), String> {
    conn.execute(
        "INSERT INTO email_notify_settings (id, log_retention_days, owner_email, updated_at) VALUES (1, 30, ?1, ?2) \
         ON CONFLICT(id) DO UPDATE SET owner_email = ?1, updated_at = ?2",
        params![email, now],
    )
    .map_err(err("settings write"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sender(conn: &Connection, addr: &str, now: i64) -> Sender {
        insert_sender(
            conn,
            &NewSender {
                address: addr,
                domain: addr.split_once('@').map(|(_, d)| d).unwrap_or(""),
                path: PATH_HOSTED,
                stalwart_account_id: Some("acct1"),
                relay_id: None,
                from_name: "Brand",
                from_name_locked: false,
                reply_to: &[],
                state: STATE_READY,
                now,
            },
        )
        .expect("insert")
    }

    #[test]
    fn one_owner_per_address_and_one_system_sender() {
        let _db = crate::db::scoped_for_test();
        let h = crate::db::shared();
        let conn = h.lock();
        let a = sender(&conn, "noreply@notify.brand.test", 100);
        let b = sender(&conn, "hello@notify.brand.test", 100);
        assert!(
            insert_sender(
                &conn,
                &NewSender {
                    address: "noreply@notify.brand.test",
                    domain: "notify.brand.test",
                    path: PATH_HOSTED,
                    stalwart_account_id: None,
                    relay_id: None,
                    from_name: "x",
                    from_name_locked: false,
                    reply_to: &[],
                    state: STATE_READY,
                    now: 1,
                },
            )
            .is_err(),
            "the address is unique"
        );
        set_sender_owner(&conn, &a.id, Some(OWNER_SYSTEM), None, 101).expect("system");
        assert!(set_sender_owner(&conn, &b.id, Some(OWNER_SYSTEM), None, 101).is_err(), "one live system sender");
        assert!(set_sender_owner(&conn, &b.id, Some(OWNER_APP), None, 101).is_err(), "an App sender names its App");
        assert_eq!(system_sender(&conn).expect("read").expect("set").id, a.id);
        set_app_on(&conn, "svc_shop", &b.id, SCOPE_USERS, "owner", 102).expect("on");
        assert!(set_app_on(&conn, "svc_crm", &b.id, SCOPE_USERS, "owner", 103).is_err(), "another App's sender");
        assert!(set_app_on(&conn, "svc_crm", &a.id, SCOPE_USERS, "owner", 103).is_err(), "the system sender");
        assert_eq!(app_sender(&conn, "svc_shop").expect("read").expect("set").id, b.id);
        assert!(app_state(&conn, "svc_shop").expect("state").enabled);
        retire_sender(&conn, &b.id, 200).expect("retire");
        let st = app_state(&conn, "svc_shop").expect("state");
        assert!(!st.enabled && st.sender_id.is_none(), "retiring the sender turns the App off: {st:?}");
        assert!(is_reserved(&conn, "noreply@notify.brand.test").expect("r"));
        assert!(is_reserved(&conn, "HELLO@notify.brand.test").expect("r"), "a retired address stays reserved");
        assert_eq!(purge_retired(&conn, 200).expect("purge"), 0, "not yet");
        assert_eq!(purge_retired(&conn, 201).expect("purge"), 1);
        assert!(!is_reserved(&conn, "hello@notify.brand.test").expect("r"));
    }

    #[test]
    fn budgets_count_recipients_and_skip_refused_rows() {
        let _db = crate::db::scoped_for_test();
        let h = crate::db::shared();
        let conn = h.lock();
        let s = sender(&conn, "noreply@brand.test", 1);
        let to3 = vec!["a@x.test".to_string(), "b@x.test".to_string(), "c@x.test".to_string()];
        let to1 = vec!["a@x.test".to_string()];
        let mk = |to: &[String], status: &str, origin: &str, any: bool, now: i64| {
            insert_log(
                &conn,
                &NewLog {
                    sender_id: &s.id,
                    origin,
                    app_service_id: Some("svc_a"),
                    purpose: None,
                    to,
                    subject: "",
                    tags: &[],
                    status,
                    error_code: None,
                    message_id: None,
                    send_after: None,
                    scope_any: any,
                    now,
                },
            )
            .expect("log")
        };
        mk(&to3, ST_SUBMITTED, ORIGIN_APP, false, 1000);
        mk(&to1, ST_REFUSED, ORIGIN_APP, false, 1001);
        mk(&to1, ST_FAILED, ORIGIN_AGENT, false, 1002);
        mk(&to1, ST_HELD, ORIGIN_AGENT, true, 1003);
        mk(&to1, ST_SUBMITTED, ORIGIN_SYSTEM, false, 1004);
        assert_eq!(sum_recipients(&conn, &Bucket::App("svc_a"), 0).expect("n"), (4, Some(1000)));
        assert_eq!(sum_recipients(&conn, &Bucket::AppAny("svc_a"), 0).expect("n").0, 1);
        assert_eq!(sum_recipients(&conn, &Bucket::Domain("brand.test"), 0).expect("n").0, 4);
        assert_eq!(sum_recipients(&conn, &Bucket::System, 0).expect("n").0, 1);
        assert_eq!(sum_recipients(&conn, &Bucket::Box, 0).expect("n").0, 5);
        assert_eq!(sum_recipients(&conn, &Bucket::Box, 1002).expect("n"), (2, Some(1003)));
        assert_eq!(purge_recipient(&conn, "B@x.test").expect("purge"), 1, "only the row naming b");
    }
}
