//! Host-wide custom-domain inventory (`domain_bindings` / `domain_names`).
//!
//! Three objects (prd-custom-domains-and-certs-v1 D1): zone ≠ binding ≠
//! termination+cert. This module is the binding: an apex attached to
//! THIS daemon, plus hostnames under it. Attach is not NS transfer
//! (`dns_write` is only true after control-plane bind succeeds).

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::mail_domain::normalize_mail_domain;

pub const ROLE_MAIL: &str = "mail";
pub const ROLE_PUBLISH: &str = "publish";
pub const ROLE_DIRECT: &str = "direct";
pub const ROLE_OTHER: &str = "other";

pub const ROLES: &[&str] = &[ROLE_MAIL, ROLE_PUBLISH, ROLE_DIRECT, ROLE_OTHER];

/// A hostname bound to one App (or published `--cmd` service) by its
/// name: role `app:<name>` (0.46.1). The App learns it as a public origin
/// (`K2_APP_ORIGIN` / `K2_APP_ORIGINS`) on its next deploy or restart.
pub const ROLE_APP_PREFIX: &str = "app:";

/// The App a role binds its hostname to (`app:hearth` → `hearth`).
pub fn app_of_role(role: &str) -> Option<&str> {
    role.strip_prefix(ROLE_APP_PREFIX).filter(|n| !n.is_empty())
}

/// Normalize + validate a role. Empty/missing → `other`.
pub fn normalize_role(raw: Option<&str>) -> Result<String, String> {
    let s = raw.map(str::trim).filter(|s| !s.is_empty()).unwrap_or(ROLE_OTHER);
    let lower = s.to_ascii_lowercase();
    if ROLES.iter().any(|r| *r == lower) {
        return Ok(lower);
    }
    if let Some(name) = lower.strip_prefix(ROLE_APP_PREFIX) {
        if crate::published_services::is_valid_name(name) {
            return Ok(lower);
        }
        return Err(format!("role app:<name> needs an App or published service name (got '{s}')"));
    }
    Err(format!(
        "role must be one of {}|app:<name> (got '{s}')",
        ROLES.join("|")
    ))
}

/// The custom hostnames bound to App `name` (role `app:<name>`) whose apex
/// is still attached here, served or not. Sorted; a dangling apex is
/// treated as missing. For the App's ORIGINS use [`live_hostnames_for_app`]
/// (0.46.6, vs-live CD-A4): a bound name that nothing serves is no origin.
pub fn hostnames_for_app(conn: &Connection, name: &str) -> rusqlite::Result<Vec<String>> {
    let role = format!("{ROLE_APP_PREFIX}{}", name.to_ascii_lowercase());
    let mut stmt = conn.prepare(
        "SELECT n.hostname FROM domain_names n JOIN domain_bindings b ON b.apex = n.apex \
         WHERE n.role = ?1 ORDER BY n.hostname",
    )?;
    let rows = stmt.query_map(params![role], |row| row.get::<_, String>(0))?;
    rows.collect()
}

// ── App custom domains (0.46.6, prd-app-custom-domains-v1 §5/§6) ─────

/// `app:` row states. Every other role keeps `state` NULL.
pub const STATE_WAITING_DNS: &str = "waiting_dns";
pub const STATE_VERIFYING: &str = "verifying";
pub const STATE_ISSUING: &str = "issuing";
pub const STATE_LIVE: &str = "live";
pub const STATE_FAILING: &str = "failing";
/// Served by the box's own web server (to the App's front port), not by
/// K2's front. Either grandfathered at the 0.46.6 upgrade (a name bound
/// before the App TLS door existed, 0236) or opted in by name
/// (`k2 app domain add … --external`, `k2 app domain external … on`). It is
/// an origin; K2 never routes, checks or issues for it until it is moved
/// into the door flow (`k2 app domain external … off`, or
/// `k2 app domain check` once the proof passes).
pub const STATE_EXTERNAL: &str = "external";
pub const APP_STATES: &[&str] =
    &[STATE_WAITING_DNS, STATE_VERIFYING, STATE_ISSUING, STATE_LIVE, STATE_FAILING, STATE_EXTERNAL];

/// What DNS showed at the last check: the name points at this box's
/// public IP (path a) or at its `<sub>.k2.dev` (path b, through Connect).
pub const PATH_DIRECT: &str = "direct";
pub const PATH_CONNECT: &str = "connect";

/// The owner name of the TXT proof (§6).
pub const VERIFY_LABEL: &str = "_k2-domain";

/// `_k2-domain.<host>`: where the TXT proof for `host` lives.
pub fn verify_txt_name(hostname: &str) -> String {
    format!("{VERIFY_LABEL}.{}", hostname.trim().trim_end_matches('.').to_ascii_lowercase())
}

/// `k2-verify=<token>`: the TXT proof's value.
pub fn verify_txt_value(token: &str) -> String {
    format!("k2-verify={token}")
}

/// A fresh random 128-bit token (32 lowercase hex chars).
pub fn new_verify_token() -> String {
    let mut b = [0u8; 16];
    if getrandom::getrandom(&mut b).is_err() {
        // Never a predictable token: fall back to a v4 uuid's randomness.
        b = *uuid::Uuid::new_v4().as_bytes();
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// One `app:` hostname with its serving state (0236).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AppDomain {
    pub hostname: String,
    pub apex: String,
    /// The App id (`app:<id>` → `<id>`).
    pub app: String,
    pub state: String,
    pub verify_token: String,
    pub verified_at: Option<i64>,
    pub checked_at: Option<i64>,
    pub path: Option<String>,
    pub is_primary: bool,
    pub last_error: Option<String>,
    pub failing_since: Option<i64>,
    /// The apex row still exists (a dangling apex serves nothing).
    pub apex_attached: bool,
}

impl AppDomain {
    /// Serving now: state `live` on an attached apex.
    pub fn is_live(&self) -> bool {
        self.state == STATE_LIVE && self.apex_attached
    }
    /// Served by the box's own proxy, not by K2 (grandfathered, 0236).
    pub fn is_external(&self) -> bool {
        self.state == STATE_EXTERNAL && self.apex_attached
    }
    /// An origin of its App: served by K2 (`live`) or by the box's own
    /// proxy (`external`), on an attached apex.
    pub fn is_origin(&self) -> bool {
        self.is_live() || self.is_external()
    }
    /// The TXT record to set: (name, value).
    pub fn txt(&self) -> (String, String) {
        (verify_txt_name(&self.hostname), verify_txt_value(&self.verify_token))
    }
}

const APP_DOMAIN_COLS: &str = "n.hostname, n.apex, n.role, n.state, n.verify_token, n.verified_at, n.checked_at, \
     n.path, n.is_primary, n.last_error, n.failing_since, (b.apex IS NOT NULL)";

fn app_domain_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AppDomain> {
    let role: String = row.get(2)?;
    Ok(AppDomain {
        hostname: row.get(0)?,
        apex: row.get(1)?,
        app: app_of_role(&role).unwrap_or("").to_string(),
        state: row.get::<_, Option<String>>(3)?.unwrap_or_else(|| STATE_WAITING_DNS.to_string()),
        verify_token: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
        verified_at: row.get(5)?,
        checked_at: row.get(6)?,
        path: row.get(7)?,
        is_primary: row.get::<_, i64>(8)? != 0,
        last_error: row.get(9)?,
        failing_since: row.get(10)?,
        apex_attached: row.get::<_, i64>(11)? != 0,
    })
}

/// Every `app:` hostname on this box, by hostname.
pub fn app_domains(conn: &Connection) -> rusqlite::Result<Vec<AppDomain>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {APP_DOMAIN_COLS} FROM domain_names n LEFT JOIN domain_bindings b ON b.apex = n.apex \
         WHERE n.role LIKE 'app:%' ORDER BY n.hostname"
    ))?;
    let rows = stmt.query_map([], app_domain_from_row)?;
    rows.collect()
}

/// The `app:` hostnames bound to App `app`, by hostname.
pub fn app_domains_for(conn: &Connection, app: &str) -> rusqlite::Result<Vec<AppDomain>> {
    let role = format!("{ROLE_APP_PREFIX}{}", app.to_ascii_lowercase());
    let mut stmt = conn.prepare(&format!(
        "SELECT {APP_DOMAIN_COLS} FROM domain_names n LEFT JOIN domain_bindings b ON b.apex = n.apex \
         WHERE n.role = ?1 ORDER BY n.hostname"
    ))?;
    let rows = stmt.query_map(params![role], app_domain_from_row)?;
    rows.collect()
}

/// One `app:` hostname (`None` = not bound, or bound to another role).
pub fn app_domain(conn: &Connection, hostname: &str) -> rusqlite::Result<Option<AppDomain>> {
    conn.query_row(
        &format!(
            "SELECT {APP_DOMAIN_COLS} FROM domain_names n LEFT JOIN domain_bindings b ON b.apex = n.apex \
             WHERE n.hostname = ?1 AND n.role LIKE 'app:%'"
        ),
        params![hostname],
        app_domain_from_row,
    )
    .optional()
}

/// The hostnames K2 itself serves for App `app` now (state `live` on an
/// attached apex; the App TLS door). Sorted.
pub fn live_hostnames_for_app(conn: &Connection, app: &str) -> rusqlite::Result<Vec<String>> {
    Ok(app_domains_for(conn, app)?.into_iter().filter(AppDomain::is_live).map(|d| d.hostname).collect())
}

/// The App's custom ORIGINS (vs-live CD-A4): the names actually served,
/// by K2 (`live`) or, grandfathered at the upgrade, by the box's own proxy
/// (`external`). A bound name nothing serves yet is not one. Sorted.
pub fn origin_hostnames_for_app(conn: &Connection, app: &str) -> rusqlite::Result<Vec<String>> {
    Ok(app_domains_for(conn, app)?.into_iter().filter(AppDomain::is_origin).map(|d| d.hostname).collect())
}

/// The App's primary custom name: marked `is_primary` AND an origin.
/// `None` means the App's own K2 label is its primary origin (§8.2).
pub fn primary_hostname_for_app(conn: &Connection, app: &str) -> rusqlite::Result<Option<String>> {
    Ok(app_domains_for(conn, app)?.into_iter().find(|d| d.is_primary && d.is_origin()).map(|d| d.hostname))
}

/// Mark `hostname` as App `app`'s primary name (clears the others).
/// `Ok(false)` = `hostname` is not one of `app`'s names.
pub fn set_primary(conn: &Connection, app: &str, hostname: &str) -> rusqlite::Result<bool> {
    let role = format!("{ROLE_APP_PREFIX}{}", app.to_ascii_lowercase());
    let tx = conn.unchecked_transaction()?;
    let mine: i64 = tx.query_row(
        "SELECT COUNT(*) FROM domain_names WHERE hostname = ?1 AND role = ?2",
        params![hostname, role],
        |r| r.get(0),
    )?;
    if mine == 0 {
        return Ok(false);
    }
    tx.execute("UPDATE domain_names SET is_primary = 0 WHERE role = ?1 AND hostname != ?2", params![role, hostname])?;
    tx.execute("UPDATE domain_names SET is_primary = 1 WHERE hostname = ?1", params![hostname])?;
    tx.commit()?;
    Ok(true)
}

/// Move an `app:` hostname to `state`, with `last_error` (None clears it).
/// Returns whether a row changed. Refuses an unknown state.
pub fn set_app_state(conn: &Connection, hostname: &str, state: &str, last_error: Option<&str>) -> rusqlite::Result<bool> {
    if !APP_STATES.contains(&state) {
        return Err(rusqlite::Error::InvalidParameterName(format!("unknown App domain state '{state}'")));
    }
    let n = conn.execute(
        "UPDATE domain_names SET state = ?2, last_error = ?3 WHERE hostname = ?1 AND role LIKE 'app:%'",
        params![hostname, state, last_error],
    )?;
    Ok(n > 0)
}

/// Mark an `app:` hostname as served by the box's own web server (`on`:
/// `external`, an origin; no checks, no certificate, no front route), or
/// hand it back to K2's flow (`off`: `waiting_dns` with no check history,
/// so the next check runs at once). A run of failed checks and the last
/// error are cleared either way; the TXT token is kept. Returns whether a
/// row changed (`false`: not an `app:` name).
pub fn set_app_external(conn: &Connection, hostname: &str, on: bool) -> rusqlite::Result<bool> {
    let n = if on {
        conn.execute(
            "UPDATE domain_names SET state = ?2, last_error = NULL, failing_since = NULL \
             WHERE hostname = ?1 AND role LIKE 'app:%'",
            params![hostname, STATE_EXTERNAL],
        )?
    } else {
        conn.execute(
            "UPDATE domain_names SET state = ?2, last_error = NULL, failing_since = NULL, checked_at = NULL, \
               verified_at = NULL, path = NULL, verify_token = COALESCE(verify_token, ?3) \
             WHERE hostname = ?1 AND role LIKE 'app:%'",
            params![hostname, STATE_WAITING_DNS, new_verify_token()],
        )?
    };
    Ok(n > 0)
}

/// What one DNS check found, written in one statement.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CheckRecord {
    pub checked_at: i64,
    /// Set when the TXT proof and DNS both passed.
    pub verified_at: Option<i64>,
    /// What DNS showed (`None` keeps the last one).
    pub path: Option<String>,
    /// `Some(t)`: this check failed and the failing run started at `t`.
    /// `None`: this check passed (the run is cleared).
    pub failing_since: Option<i64>,
}

/// Store a check's outcome on an `app:` row (the state is set separately).
pub fn record_app_check(conn: &Connection, hostname: &str, c: &CheckRecord) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "UPDATE domain_names SET checked_at = ?2, verified_at = COALESCE(?3, verified_at), \
           path = COALESCE(?4, path), failing_since = ?5 \
         WHERE hostname = ?1 AND role LIKE 'app:%'",
        params![hostname, c.checked_at, c.verified_at, c.path, c.failing_since],
    )?;
    Ok(n > 0)
}

/// True iff `hostname` is the apex or a label under it.
pub fn hostname_under_apex(hostname: &str, apex: &str) -> bool {
    hostname == apex || hostname.ends_with(&format!(".{apex}"))
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Control-plane zone status: records are live (NS delegated to k2.dev).
pub const ZONE_STATUS_ACTIVE: &str = "active";
/// Control-plane zone status: zone exists in the k2.dev account but the
/// registrar nameservers do not point at k2.dev yet. No record writes.
pub const ZONE_STATUS_PENDING_NS: &str = "pending_ns";
/// Control-plane zone status: the zone is winding down (plan ended).
/// Records still serve, but no writes.
pub const ZONE_STATUS_WINDDOWN: &str = "winddown";
/// Control-plane zone status: suspended by k2.dev. No writes.
pub const ZONE_STATUS_SUSPENDED: &str = "suspended";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DomainBinding {
    pub apex: String,
    pub zone_id: Option<String>,
    pub dns_write: bool,
    pub created_at: i64,
    /// `active` | `pending_ns` from k2.dev bind (A8.1). `None` for BYO
    /// rows and rows bound before A8.1 (0127).
    #[serde(default)]
    pub status: Option<String>,
    /// Nameservers k2.dev wants at the registrar (A8.1).
    #[serde(default)]
    pub nameservers: Vec<String>,
    /// k2.dev auto-added this zone to the account on bind (`created:true`).
    #[serde(default)]
    pub auto_created: bool,
}

impl DomainBinding {
    /// True when the zone is in the k2.dev account but its nameservers do
    /// not point at k2.dev yet (record writes are refused).
    pub fn is_pending_ns(&self) -> bool {
        self.status.as_deref() == Some(ZONE_STATUS_PENDING_NS)
    }

    /// True when k2.dev holds the zone read-only (`winddown` / `suspended`).
    pub fn is_readonly(&self) -> bool {
        matches!(
            self.status.as_deref(),
            Some(ZONE_STATUS_WINDDOWN) | Some(ZONE_STATUS_SUSPENDED)
        )
    }
}

/// Everything a successful control-plane bind says about the zone.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BoundZone {
    pub zone_id: Option<String>,
    pub status: Option<String>,
    pub nameservers: Vec<String>,
    pub dns_write: bool,
    pub auto_created: bool,
}

const BINDING_COLS: &str =
    "apex, zone_id, dns_write, created_at, status, nameservers, auto_created";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DomainName {
    pub hostname: String,
    pub apex: String,
    pub role: String,
}

impl DomainBinding {
    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        let dns_write: i64 = row.get(2)?;
        let ns_json: Option<String> = row.get(5)?;
        let nameservers = ns_json
            .as_deref()
            .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
            .unwrap_or_default();
        let auto_created: i64 = row.get(6)?;
        Ok(Self {
            apex: row.get(0)?,
            zone_id: row.get(1)?,
            dns_write: dns_write != 0,
            created_at: row.get(3)?,
            status: row.get(4)?,
            nameservers,
            auto_created: auto_created != 0,
        })
    }
}

pub fn list_bindings(conn: &Connection) -> rusqlite::Result<Vec<DomainBinding>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {BINDING_COLS} FROM domain_bindings ORDER BY apex"
    ))?;
    let rows = stmt.query_map([], DomainBinding::from_row)?;
    rows.collect()
}

pub fn get_binding(conn: &Connection, apex: &str) -> rusqlite::Result<Option<DomainBinding>> {
    conn.query_row(
        &format!("SELECT {BINDING_COLS} FROM domain_bindings WHERE apex = ?1"),
        params![apex],
        DomainBinding::from_row,
    )
    .optional()
}

/// The zone a DNS name lives in, out of `apexes`: the name itself, else the
/// LONGEST apex it is a dot-suffix of (`notify.brand.com` under
/// `brand.com`, never under `rand.com`). Returns `(apex, sub-labels)`,
/// sub-labels empty for the apex itself (Email Notifications E11: a
/// sending subdomain's DKIM and autoconfig rows go in its parent's zone).
pub fn zone_for_name<'a>(name: &str, apexes: impl IntoIterator<Item = &'a str>) -> Option<(&'a str, String)> {
    let n = name.trim().trim_end_matches('.').to_ascii_lowercase();
    if n.is_empty() {
        return None;
    }
    let mut best: Option<(&'a str, String)> = None;
    for apex in apexes {
        let a = apex.trim().trim_end_matches('.').to_ascii_lowercase();
        if a.is_empty() {
            continue;
        }
        let sub = if n == a {
            String::new()
        } else if let Some(prefix) = n.strip_suffix(&format!(".{a}")) {
            prefix.to_string()
        } else {
            continue;
        };
        if best.as_ref().is_none_or(|(b, _)| a.len() > b.trim_end_matches('.').len()) {
            best = Some((apex, sub));
        }
    }
    best
}

/// [`zone_for_name`] over this box's domain bindings: the binding a mail
/// domain's records go in, and the sub-labels to prefix record names with.
pub fn binding_for_name(conn: &Connection, name: &str) -> rusqlite::Result<Option<(DomainBinding, String)>> {
    let all = list_bindings(conn)?;
    let Some((apex, sub)) = zone_for_name(name, all.iter().map(|b| b.apex.as_str())) else { return Ok(None) };
    let apex = apex.to_string();
    Ok(all.into_iter().find(|b| b.apex == apex).map(|b| (b, sub)))
}

pub fn get_binding_by_zone_id(conn: &Connection, zone_id: &str) -> rusqlite::Result<Option<DomainBinding>> {
    let z = zone_id.trim();
    if z.is_empty() {
        return Ok(None);
    }
    conn.query_row(
        &format!("SELECT {BINDING_COLS} FROM domain_bindings WHERE zone_id = ?1"),
        params![z],
        DomainBinding::from_row,
    )
    .optional()
}

/// Insert or refresh a BYO / legacy binding (no control-plane zone
/// status). Idempotent on apex. Clears any A8.1 status so a row that
/// k2.dev no longer owns stops reading as pending/active.
pub fn upsert_binding(
    conn: &Connection,
    apex: &str,
    zone_id: Option<&str>,
    dns_write: bool,
) -> rusqlite::Result<DomainBinding> {
    upsert_binding_zone(
        conn,
        apex,
        &BoundZone {
            zone_id: zone_id.map(str::to_string),
            status: None,
            nameservers: Vec::new(),
            dns_write,
            auto_created: false,
        },
    )
}

/// Insert or refresh a binding with everything k2.dev bind returned
/// (A8.1: status, nameservers, created). Idempotent on apex.
/// `auto_created` is sticky: a later re-bind that omits `created` keeps
/// the note that k2.dev added the zone.
pub fn upsert_binding_zone(
    conn: &Connection,
    apex: &str,
    zone: &BoundZone,
) -> rusqlite::Result<DomainBinding> {
    let now = now_unix();
    let ns_json: Option<String> = if zone.nameservers.is_empty() {
        None
    } else {
        Some(serde_json::to_string(&zone.nameservers).unwrap_or_else(|_| "[]".into()))
    };
    let keep_created = zone.status.is_some();
    conn.execute(
        "INSERT INTO domain_bindings \
           (apex, zone_id, dns_write, created_at, status, nameservers, auto_created) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
         ON CONFLICT(apex) DO UPDATE SET \
           zone_id = excluded.zone_id, \
           dns_write = excluded.dns_write, \
           status = excluded.status, \
           nameservers = excluded.nameservers, \
           auto_created = CASE WHEN ?8 THEN MAX(domain_bindings.auto_created, excluded.auto_created) \
                               ELSE excluded.auto_created END",
        params![
            apex,
            zone.zone_id.as_deref(),
            if zone.dns_write { 1 } else { 0 },
            now,
            zone.status.as_deref(),
            ns_json,
            if zone.auto_created { 1 } else { 0 },
            keep_created,
        ],
    )?;
    Ok(get_binding(conn, apex)?.expect("just upserted"))
}

/// Store a fresh control-plane status on a bound row (prd-dns-pending-and-
/// cutover-safety-v1 P1/P2: the box's own re-check). Touches only `status`
/// and `dns_write`; nameservers, `auto_created` and `created_at` stay. The
/// row must still carry `zone_id` (a row re-bound to another zone in the
/// meantime is left alone). Returns true when the row changed.
pub fn set_zone_status(
    conn: &Connection,
    apex: &str,
    zone_id: &str,
    status: &str,
    dns_write: bool,
) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "UPDATE domain_bindings SET status = ?3, dns_write = ?4 \
         WHERE apex = ?1 AND zone_id = ?2 \
           AND (status IS NOT ?3 OR dns_write != ?4)",
        params![apex, zone_id, status, if dns_write { 1 } else { 0 }],
    )?;
    Ok(n > 0)
}

/// Remove apex and cascade hostnames. Returns whether a row existed.
pub fn remove_binding(conn: &Connection, apex: &str) -> rusqlite::Result<bool> {
    conn.execute(
        "DELETE FROM domain_names WHERE apex = ?1",
        params![apex],
    )?;
    let n = conn.execute(
        "DELETE FROM domain_bindings WHERE apex = ?1",
        params![apex],
    )?;
    Ok(n > 0)
}

pub fn list_names(conn: &Connection) -> rusqlite::Result<Vec<DomainName>> {
    let mut stmt = conn.prepare(
        "SELECT hostname, apex, role FROM domain_names ORDER BY apex, hostname",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(DomainName {
            hostname: row.get(0)?,
            apex: row.get(1)?,
            role: row.get(2)?,
        })
    })?;
    rows.collect()
}

pub fn list_names_for_apex(conn: &Connection, apex: &str) -> rusqlite::Result<Vec<DomainName>> {
    let mut stmt = conn.prepare(
        "SELECT hostname, apex, role FROM domain_names WHERE apex = ?1 ORDER BY hostname",
    )?;
    let rows = stmt.query_map(params![apex], |row| {
        Ok(DomainName {
            hostname: row.get(0)?,
            apex: row.get(1)?,
            role: row.get(2)?,
        })
    })?;
    rows.collect()
}

pub fn get_name(conn: &Connection, hostname: &str) -> rusqlite::Result<Option<DomainName>> {
    conn.query_row(
        "SELECT hostname, apex, role FROM domain_names WHERE hostname = ?1",
        params![hostname],
        |row| {
            Ok(DomainName {
                hostname: row.get(0)?,
                apex: row.get(1)?,
                role: row.get(2)?,
            })
        },
    )
    .optional()
}

/// Insert or re-role a hostname. An `app:` role starts (or restarts, when
/// the role or apex changes) at `waiting_dns` with a fresh TXT token, not
/// primary; re-adding the same name with the same role and apex keeps its
/// state. Any other role clears the App columns.
pub fn upsert_name(conn: &Connection, hostname: &str, apex: &str, role: &str) -> rusqlite::Result<DomainName> {
    let is_app = app_of_role(role).is_some();
    let state = is_app.then_some(STATE_WAITING_DNS);
    let token = is_app.then(new_verify_token);
    // `same`: the row keeps its App columns only when role and apex stay.
    const SAME: &str = "(domain_names.role = excluded.role AND domain_names.apex = excluded.apex)";
    conn.execute(
        &format!(
            "INSERT INTO domain_names (hostname, apex, role, state, verify_token) VALUES (?1, ?2, ?3, ?4, ?5) \
             ON CONFLICT(hostname) DO UPDATE SET \
               state = CASE WHEN {SAME} THEN COALESCE(domain_names.state, excluded.state) ELSE excluded.state END, \
               verify_token = CASE WHEN {SAME} THEN COALESCE(domain_names.verify_token, excluded.verify_token) \
                                   ELSE excluded.verify_token END, \
               verified_at = CASE WHEN {SAME} THEN domain_names.verified_at END, \
               checked_at = CASE WHEN {SAME} THEN domain_names.checked_at END, \
               path = CASE WHEN {SAME} THEN domain_names.path END, \
               is_primary = CASE WHEN {SAME} THEN domain_names.is_primary ELSE 0 END, \
               last_error = CASE WHEN {SAME} THEN domain_names.last_error END, \
               failing_since = CASE WHEN {SAME} THEN domain_names.failing_since END, \
               apex = excluded.apex, role = excluded.role"
        ),
        params![hostname, apex, role, state, token],
    )?;
    Ok(get_name(conn, hostname)?.expect("just upserted"))
}

pub fn remove_name(conn: &Connection, hostname: &str) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "DELETE FROM domain_names WHERE hostname = ?1",
        params![hostname],
    )?;
    Ok(n > 0)
}

/// True when this box may write the zone (`dns_write=1` and matching zone id).
pub fn zone_attached_for_write(conn: &Connection, zone_id: &str) -> rusqlite::Result<bool> {
    Ok(get_binding_by_zone_id(conn, zone_id)?
        .map(|b| b.dns_write)
        .unwrap_or(false))
}

/// True when this box may write the apex (`dns_write=1`).
pub fn apex_attached_for_write(conn: &Connection, apex: &str) -> rusqlite::Result<bool> {
    Ok(get_binding(conn, apex)?.map(|b| b.dns_write).unwrap_or(false))
}

/// Normalize a user-supplied apex (IDNA punycode).
pub fn normalize_apex(input: &str) -> Result<String, String> {
    normalize_mail_domain(input)
}

/// Normalize a hostname FQDN (same IDNA helper as apex).
pub fn normalize_hostname(input: &str) -> Result<String, String> {
    normalize_mail_domain(input)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> Connection {
        crate::db::isolated_test_connection()
    }

    /// Email Notifications E11: a sending subdomain's records go in the
    /// longest bound zone it sits under, named relative to it.
    #[test]
    fn a_subdomain_finds_its_longest_bound_zone() {
        let apexes = ["brand.com", "notify.brand.com.example", "rand.com", "deep.brand.com"];
        assert_eq!(zone_for_name("notify.brand.com", apexes), Some(("brand.com", "notify".to_string())));
        assert_eq!(zone_for_name("x.deep.brand.com.", apexes), Some(("deep.brand.com", "x".to_string())));
        assert_eq!(zone_for_name("Brand.COM", apexes), Some(("brand.com", String::new())));
        assert_eq!(zone_for_name("abrand.com", apexes), None, "a label boundary, not a string suffix");
        assert_eq!(zone_for_name("", apexes), None);
        let conn = fresh();
        upsert_binding(&conn, "brand.com", Some("z1"), true).unwrap();
        let (b, sub) = binding_for_name(&conn, "notify.brand.com").unwrap().expect("bound");
        assert_eq!((b.apex.as_str(), sub.as_str()), ("brand.com", "notify"));
        assert!(binding_for_name(&conn, "brand.org").unwrap().is_none());
    }

    #[test]
    fn migration_creates_tables() {
        let conn = fresh();
        for table in ["domain_bindings", "domain_names"] {
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    params![table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "{table}");
        }
    }

    #[test]
    fn hostname_under_apex_is_strict() {
        assert!(hostname_under_apex("example.com", "example.com"));
        assert!(hostname_under_apex("mail.example.com", "example.com"));
        assert!(hostname_under_apex("a.b.example.com", "example.com"));
        assert!(!hostname_under_apex("notexample.com", "example.com"));
        assert!(!hostname_under_apex("example.com.evil.com", "example.com"));
        assert!(!hostname_under_apex("mail.other.com", "example.com"));
    }

    #[test]
    fn upsert_is_idempotent_and_remove_cascades() {
        let conn = fresh();
        let b = upsert_binding(&conn, "example.com", Some("z1"), true).unwrap();
        assert_eq!(b.apex, "example.com");
        assert_eq!(b.zone_id.as_deref(), Some("z1"));
        assert!(b.dns_write);
        upsert_binding(&conn, "example.com", Some("z1"), true).unwrap();
        assert_eq!(list_bindings(&conn).unwrap().len(), 1);

        upsert_name(&conn, "mail.example.com", "example.com", ROLE_MAIL).unwrap();
        upsert_name(&conn, "example.com", "example.com", ROLE_OTHER).unwrap();
        assert_eq!(list_names_for_apex(&conn, "example.com").unwrap().len(), 2);

        assert!(remove_binding(&conn, "example.com").unwrap());
        assert!(list_bindings(&conn).unwrap().is_empty());
        assert!(list_names(&conn).unwrap().is_empty());
    }

    #[test]
    fn zone_write_gate() {
        let conn = fresh();
        upsert_binding(&conn, "byo.example", None, false).unwrap();
        upsert_binding(&conn, "hosted.example", Some("zone-hosted"), true).unwrap();
        assert!(!apex_attached_for_write(&conn, "byo.example").unwrap());
        assert!(apex_attached_for_write(&conn, "hosted.example").unwrap());
        assert!(!zone_attached_for_write(&conn, "zone-hosted-nope").unwrap());
        assert!(zone_attached_for_write(&conn, "zone-hosted").unwrap());
        assert!(!apex_attached_for_write(&conn, "missing.example").unwrap());
    }

    #[test]
    fn set_zone_status_touches_only_status_and_write() {
        let conn = fresh();
        let b = upsert_binding_zone(
            &conn,
            "szs.example",
            &BoundZone {
                zone_id: Some("z-szs".into()),
                status: Some(ZONE_STATUS_PENDING_NS.into()),
                nameservers: vec!["ns1.k2.dev".into()],
                dns_write: false,
                auto_created: true,
            },
        )
        .unwrap();
        assert!(b.is_pending_ns());
        assert!(!b.is_readonly());
        // Wrong zone id: untouched.
        assert!(!set_zone_status(&conn, "szs.example", "z-other", ZONE_STATUS_ACTIVE, true).unwrap());
        assert!(get_binding(&conn, "szs.example").unwrap().unwrap().is_pending_ns());
        // Flip.
        assert!(set_zone_status(&conn, "szs.example", "z-szs", ZONE_STATUS_ACTIVE, true).unwrap());
        let after = get_binding(&conn, "szs.example").unwrap().unwrap();
        assert_eq!(after.status.as_deref(), Some(ZONE_STATUS_ACTIVE));
        assert!(after.dns_write);
        assert_eq!(after.nameservers, vec!["ns1.k2.dev".to_string()]);
        assert!(after.auto_created);
        assert_eq!(after.created_at, b.created_at);
        // Same values again: no change reported.
        assert!(!set_zone_status(&conn, "szs.example", "z-szs", ZONE_STATUS_ACTIVE, true).unwrap());
        // Read-only states.
        assert!(set_zone_status(&conn, "szs.example", "z-szs", ZONE_STATUS_WINDDOWN, false).unwrap());
        let ro = get_binding(&conn, "szs.example").unwrap().unwrap();
        assert!(ro.is_readonly());
        assert!(!ro.dns_write);
        assert!(set_zone_status(&conn, "szs.example", "z-szs", ZONE_STATUS_SUSPENDED, false).unwrap());
        assert!(get_binding(&conn, "szs.example").unwrap().unwrap().is_readonly());
        // Missing row: nothing.
        assert!(!set_zone_status(&conn, "nope.example", "z-szs", ZONE_STATUS_ACTIVE, true).unwrap());
    }

    #[test]
    fn role_normalize() {
        assert_eq!(normalize_role(None).unwrap(), ROLE_OTHER);
        assert_eq!(normalize_role(Some("MAIL")).unwrap(), ROLE_MAIL);
        assert!(normalize_role(Some("wildcard")).is_err());
        assert_eq!(normalize_role(Some("App:Hearth")).unwrap(), "app:hearth");
        assert_eq!(app_of_role("app:hearth"), Some("hearth"));
        assert_eq!(app_of_role(ROLE_PUBLISH), None);
        let e = normalize_role(Some("app:")).unwrap_err();
        assert!(e.contains("app:<name>"), "{e}");
        assert!(normalize_role(Some("app:no_underscores")).is_err());
    }

    #[test]
    fn hostnames_for_app_are_its_bound_names_on_attached_apexes() {
        let conn = fresh();
        upsert_binding(&conn, "example.com", None, false).unwrap();
        upsert_name(&conn, "hearth.example.com", "example.com", "app:hearth").unwrap();
        upsert_name(&conn, "chat.example.com", "example.com", "app:hearth").unwrap();
        upsert_name(&conn, "other.example.com", "example.com", "app:other").unwrap();
        upsert_name(&conn, "www.example.com", "example.com", ROLE_PUBLISH).unwrap();
        // A dangling apex (no binding row) counts as missing.
        upsert_name(&conn, "hearth.gone.test", "gone.test", "app:hearth").unwrap();
        assert_eq!(
            hostnames_for_app(&conn, "hearth").unwrap(),
            vec!["chat.example.com".to_string(), "hearth.example.com".to_string()]
        );
        assert_eq!(hostnames_for_app(&conn, "nobody").unwrap(), Vec::<String>::new());
    }

    /// 0236: an `app:` name starts at `waiting_dns` with its own random
    /// token; re-adding keeps it; a re-role or a move to another App starts
    /// over (new token, not primary); another role clears the App columns.
    #[test]
    fn app_names_start_waiting_with_a_token_and_reset_on_re_role() {
        let conn = fresh();
        upsert_binding(&conn, "example.com", None, false).unwrap();
        upsert_name(&conn, "a.example.com", "example.com", "app:hearth").unwrap();
        upsert_name(&conn, "b.example.com", "example.com", "app:hearth").unwrap();
        let a = app_domain(&conn, "a.example.com").unwrap().expect("bound");
        let b = app_domain(&conn, "b.example.com").unwrap().expect("bound");
        assert_eq!(a.state, STATE_WAITING_DNS);
        assert_eq!(a.app, "hearth");
        assert_eq!(a.verify_token.len(), 32, "128-bit hex token: {}", a.verify_token);
        assert!(a.verify_token.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a.verify_token, b.verify_token, "per-name tokens");
        assert_eq!(a.txt(), ("_k2-domain.a.example.com".to_string(), format!("k2-verify={}", a.verify_token)));
        assert!(!a.is_primary);

        // Same role again: nothing changes.
        set_app_state(&conn, "a.example.com", STATE_LIVE, None).unwrap();
        assert!(set_primary(&conn, "hearth", "a.example.com").unwrap());
        upsert_name(&conn, "a.example.com", "example.com", "app:hearth").unwrap();
        let again = app_domain(&conn, "a.example.com").unwrap().unwrap();
        assert_eq!((again.state.as_str(), again.verify_token.as_str(), again.is_primary), (STATE_LIVE, a.verify_token.as_str(), true));

        // Moved to another App: starts over.
        upsert_name(&conn, "a.example.com", "example.com", "app:other").unwrap();
        let moved = app_domain(&conn, "a.example.com").unwrap().unwrap();
        assert_eq!(moved.state, STATE_WAITING_DNS);
        assert_ne!(moved.verify_token, a.verify_token, "a new token");
        assert!(!moved.is_primary, "not primary on its new App");

        // Re-roled to mail: no App columns, and no longer an App domain.
        upsert_name(&conn, "a.example.com", "example.com", ROLE_MAIL).unwrap();
        assert!(app_domain(&conn, "a.example.com").unwrap().is_none());
        let (st, tok): (Option<String>, Option<String>) = conn
            .query_row("SELECT state, verify_token FROM domain_names WHERE hostname = 'a.example.com'", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!((st, tok), (None, None));
    }

    /// vs-live CD-A4: only `live` names on an attached apex are origins;
    /// the primary is the one marked AND live, never the first by name.
    #[test]
    fn only_live_names_count_and_the_primary_must_be_live() {
        let conn = fresh();
        upsert_binding(&conn, "example.com", None, false).unwrap();
        for h in ["aaa.example.com", "hearth.example.com", "zzz.example.com"] {
            upsert_name(&conn, h, "example.com", "app:hearth").unwrap();
        }
        upsert_name(&conn, "hearth.gone.test", "gone.test", "app:hearth").unwrap();
        assert!(live_hostnames_for_app(&conn, "hearth").unwrap().is_empty(), "nothing serves them yet");
        assert_eq!(primary_hostname_for_app(&conn, "hearth").unwrap(), None);

        set_app_state(&conn, "zzz.example.com", STATE_LIVE, None).unwrap();
        set_app_state(&conn, "hearth.example.com", STATE_LIVE, None).unwrap();
        set_app_state(&conn, "hearth.gone.test", STATE_LIVE, None).unwrap();
        assert_eq!(
            live_hostnames_for_app(&conn, "hearth").unwrap(),
            vec!["hearth.example.com".to_string(), "zzz.example.com".to_string()],
            "live on an attached apex only (aaa is waiting; gone.test has no apex)"
        );
        assert_eq!(primary_hostname_for_app(&conn, "hearth").unwrap(), None, "nobody picked one");

        // A primary that is not live does not count.
        assert!(set_primary(&conn, "hearth", "aaa.example.com").unwrap());
        assert_eq!(primary_hostname_for_app(&conn, "hearth").unwrap(), None);
        assert!(set_primary(&conn, "hearth", "zzz.example.com").unwrap());
        assert_eq!(primary_hostname_for_app(&conn, "hearth").unwrap(), Some("zzz.example.com".to_string()));
        let primaries: i64 = conn
            .query_row("SELECT COUNT(*) FROM domain_names WHERE role = 'app:hearth' AND is_primary = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(primaries, 1, "one primary per App");
        assert!(!set_primary(&conn, "other", "zzz.example.com").unwrap(), "not that App's name");
        assert!(!set_primary(&conn, "hearth", "nope.example.com").unwrap());

        assert!(set_app_state(&conn, "zzz.example.com", "bogus", None).is_err(), "unknown state refused");
        set_app_state(&conn, "zzz.example.com", STATE_FAILING, Some("DNS now points at 203.0.113.9")).unwrap();
        let z = app_domain(&conn, "zzz.example.com").unwrap().unwrap();
        assert_eq!(z.last_error.as_deref(), Some("DNS now points at 203.0.113.9"));
        assert_eq!(primary_hostname_for_app(&conn, "hearth").unwrap(), None, "a failing primary is no origin");
    }

    #[test]
    fn a_check_record_keeps_the_last_proof_and_path() {
        let conn = fresh();
        upsert_binding(&conn, "example.com", None, false).unwrap();
        upsert_name(&conn, "a.example.com", "example.com", "app:hearth").unwrap();
        record_app_check(
            &conn,
            "a.example.com",
            &CheckRecord { checked_at: 100, verified_at: Some(100), path: Some(PATH_DIRECT.into()), failing_since: None },
        )
        .unwrap();
        record_app_check(&conn, "a.example.com", &CheckRecord { checked_at: 200, verified_at: None, path: None, failing_since: Some(200) })
            .unwrap();
        let a = app_domain(&conn, "a.example.com").unwrap().unwrap();
        assert_eq!(
            (a.checked_at, a.verified_at, a.path.as_deref(), a.failing_since),
            (Some(200), Some(100), Some(PATH_DIRECT), Some(200))
        );
        assert!(!record_app_check(&conn, "nope.example.com", &CheckRecord::default()).unwrap());
    }

    /// 0236 on a database that already has `app:` rows (bound before
    /// 0.46.6; a box may serve them through its own proxy, PRD path 4):
    /// they are GRANDFATHERED `external`, stay the App's origins, and the
    /// name that was K2_APP_ORIGIN before (the first by name on an attached
    /// apex) stays primary. Names bound after the upgrade start
    /// `waiting_dns`. Other roles stay NULL.
    #[test]
    fn migration_0236_grandfathers_existing_app_rows_as_external_origins() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::run_migrations_through(&conn, Some("0231_zen_version_carry")).unwrap();
        conn.execute_batch(
            "INSERT INTO domain_bindings (apex) VALUES ('example.com');
             INSERT INTO domain_bindings (apex) VALUES ('nsihomes.com');
             INSERT INTO domain_names (hostname, apex, role) VALUES ('nsihomes.com', 'nsihomes.com', 'app:agentnsi');
             INSERT INTO domain_names (hostname, apex, role) VALUES ('www.nsihomes.com', 'nsihomes.com', 'app:agentnsi');
             INSERT INTO domain_names (hostname, apex, role) VALUES ('a.gone.test', 'gone.test', 'app:agentnsi');
             INSERT INTO domain_names (hostname, apex, role) VALUES ('b.example.com', 'example.com', 'app:hearth');
             INSERT INTO domain_names (hostname, apex, role) VALUES ('mail.example.com', 'example.com', 'mail');",
        )
        .unwrap();
        // What the App had before 0.46.6 (every bound name, first by name).
        assert_eq!(
            hostnames_for_app(&conn, "agentnsi").unwrap(),
            vec!["nsihomes.com".to_string(), "www.nsihomes.com".to_string()]
        );
        crate::db::run_migrations(&conn).unwrap();
        let a = app_domain(&conn, "nsihomes.com").unwrap().expect("still bound");
        assert_eq!(a.state, STATE_EXTERNAL);
        assert!(a.is_external() && a.is_origin() && !a.is_live(), "served by its own proxy, not by K2");
        assert_eq!(a.verify_token.len(), 32);
        assert!(a.is_primary, "the pre-upgrade K2_APP_ORIGIN stays primary");
        assert!(!app_domain(&conn, "www.nsihomes.com").unwrap().unwrap().is_primary);
        assert_eq!(
            origin_hostnames_for_app(&conn, "agentnsi").unwrap(),
            vec!["nsihomes.com".to_string(), "www.nsihomes.com".to_string()],
            "the same origins as before the upgrade"
        );
        assert_eq!(primary_hostname_for_app(&conn, "agentnsi").unwrap(), Some("nsihomes.com".to_string()));
        assert!(live_hostnames_for_app(&conn, "agentnsi").unwrap().is_empty(), "K2 serves none of them");
        let gone = app_domain(&conn, "a.gone.test").unwrap().unwrap();
        assert!(!gone.is_primary && !gone.is_origin(), "a dangling apex was no origin before either");
        assert_eq!(primary_hostname_for_app(&conn, "hearth").unwrap(), Some("b.example.com".to_string()));
        // Bound after the upgrade: waiting_dns, no origin.
        upsert_name(&conn, "shop.nsihomes.com", "nsihomes.com", "app:agentnsi").unwrap();
        assert_eq!(app_domain(&conn, "shop.nsihomes.com").unwrap().unwrap().state, STATE_WAITING_DNS);
        assert_eq!(origin_hostnames_for_app(&conn, "agentnsi").unwrap().len(), 2);
        // Re-adding a grandfathered name keeps it external.
        upsert_name(&conn, "nsihomes.com", "nsihomes.com", "app:agentnsi").unwrap();
        assert_eq!(app_domain(&conn, "nsihomes.com").unwrap().unwrap().state, STATE_EXTERNAL);
        let st: Option<String> = conn
            .query_row("SELECT state FROM domain_names WHERE hostname = 'mail.example.com'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(st, None);
    }

    /// After the upgrade, a name the box's own web server serves is opted
    /// in by name (`external`, an origin at once) and handed back to K2's
    /// flow (`waiting_dns`, no origin until live, CD-A4); a non-App name
    /// is never touched.
    #[test]
    fn a_name_is_marked_external_and_handed_back() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::run_migrations(&conn).unwrap();
        upsert_binding(&conn, "example.com", None, false).unwrap();
        upsert_name(&conn, "shop2.example.com", "example.com", "app:shop").unwrap();
        upsert_name(&conn, "mail.example.com", "example.com", "mail").unwrap();
        record_app_check(
            &conn,
            "shop2.example.com",
            &CheckRecord { checked_at: 50, verified_at: None, path: None, failing_since: Some(40) },
        )
        .unwrap();
        set_app_state(&conn, "shop2.example.com", STATE_WAITING_DNS, Some("no A record yet")).unwrap();
        assert!(origin_hostnames_for_app(&conn, "shop").unwrap().is_empty(), "bound, not served: no origin");

        assert!(set_app_external(&conn, "shop2.example.com", true).unwrap());
        let d = app_domain(&conn, "shop2.example.com").unwrap().expect("bound");
        assert_eq!((d.state.as_str(), d.last_error.as_deref(), d.failing_since), (STATE_EXTERNAL, None, None));
        assert!(d.is_external() && d.is_origin() && !d.is_live());
        assert_eq!(origin_hostnames_for_app(&conn, "shop").unwrap(), vec!["shop2.example.com".to_string()]);
        assert!(live_hostnames_for_app(&conn, "shop").unwrap().is_empty(), "K2 doesn't serve it");
        // Re-adding the name keeps it external.
        upsert_name(&conn, "shop2.example.com", "example.com", "app:shop").unwrap();
        assert_eq!(app_domain(&conn, "shop2.example.com").unwrap().unwrap().state, STATE_EXTERNAL);

        let token = d.verify_token.clone();
        assert_eq!(token.len(), 32);
        assert!(set_app_external(&conn, "shop2.example.com", false).unwrap());
        let d = app_domain(&conn, "shop2.example.com").unwrap().unwrap();
        assert_eq!((d.state.as_str(), d.checked_at, d.verified_at), (STATE_WAITING_DNS, None, None));
        assert_eq!(d.verify_token, token, "a TXT proof the person already set stays valid");
        assert!(!d.is_origin());
        assert!(origin_hostnames_for_app(&conn, "shop").unwrap().is_empty());

        assert!(!set_app_external(&conn, "mail.example.com", true).unwrap(), "only app: names");
        assert!(!set_app_external(&conn, "nope.example.com", true).unwrap());
        let st: Option<String> = conn
            .query_row("SELECT state FROM domain_names WHERE hostname = 'mail.example.com'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(st, None);
    }
}
