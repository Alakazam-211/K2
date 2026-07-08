//! S6 owner CONFIG surface (PRD §8.3/§8.4/§11 `k2 mail config`) — the
//! ops layer behind `GET /cli/mail/config` + `POST /cli/mail/config/set`
//! (route handlers in [`super::routes_server`]; owner-or-admin gating
//! lives in the dispatcher's POST arm).
//!
//! This is the WRITER for what S5's send path reads: per-domain
//! `send_mode`, `mail_relay_configs` rows (kind-agnostic — V1
//! implements `smtp`, nothing here assumes `kind == smtp` beyond the
//! V1 attach refusal), and the D4/D6 gating settings
//! (`mail_agent_send` / `mail_address_cap`, per-workspace + global).
//!
//! Invariants:
//! - **Domains normalize at every boundary** (pre-mortem #14):
//!   `k2_core::mail_domain::normalize_mail_domain` before any lookup.
//! - **Credentials never touch DB columns, responses, or logs.** A
//!   relay password is stored through the [`SecretStore`] (opaque
//!   `mailsec_*` ref into the 0600 file store); alternatively the
//!   owner supplies a scheme ref (`env:<VAR>` / absolute path). Only
//!   the REF is persisted; config reads report `hasCredentials`, never
//!   the ref target's value.
//! - **Direct mode is doctor-gated** (PRD §8.3/§9): flipping a domain
//!   to `direct` requires a stored server-level doctor run whose grade
//!   is not `fail`; the refusal lists the failing checks and the
//!   provider coaching ([`super::doctor::direct_send_gate`]).
//! - **Relay routes are pushed to Stalwart** when a domain enters
//!   `relay` mode (and re-pushed when its attached config changes),
//!   and CLEARED when it leaves — through the single ⚠ LIVE-BOX
//!   function [`StalwartClient::relay_route_apply`], behind the
//!   [`RelayEngine`] trait so every test injects a recording fake
//!   (no network in tests, ever).
//! - **SPF display follows automatically**: the record table derives
//!   its SPF row at READ time from the CURRENT `send_mode` +
//!   `mail_relay_configs.spf_include`
//!   ([`super::domains::effective_rows`]) — a mode change here is
//!   visible on the next `domain show` with no extra writes.

use crate::mail::domains;
use crate::mail::jmap::{RelayRoute, StalwartClient};
use crate::mail::secrets::SecretStore;

// ── Errors (mapped onto the route layer's stable {code, hint}) ──────────

#[derive(Debug)]
pub enum CfgError {
    /// Bad input → 400 `usage`.
    Usage(String),
    /// Unknown domain / relay config → 404 `not_found`.
    NotFound(String),
    /// A live Stalwart is required but unavailable → 503 `not_ready`.
    NotReady(String),
    /// The direct-mode doctor gate refused → 409 `direct_locked`.
    Locked(String),
    /// Delete refused (config still in use) → 409 `conflict`.
    Conflict(String),
    /// DB / Stalwart failure → 502 `engine`.
    Engine(String),
}

/// Sending-mode vocabulary (PRD §8.3 / 0072 CHECK constraint).
pub const SEND_MODES: [&str; 3] = ["direct", "relay", "receive-only"];
/// Relay-config kinds the 0072 schema anticipates. V1 SUBMITS only
/// over `smtp`; the others may be stored but not attached yet.
pub const RELAY_KINDS: [&str; 4] = ["smtp", "mailgun", "ses", "resend"];

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ── The Stalwart seam (one trait, one LIVE-BOX impl) ────────────────────

/// What the config surface needs from Stalwart: apply (`Some`) or
/// clear (`None`) one domain's smart-host route. Production =
/// [`StalwartClient::relay_route_apply`] (⚠ LIVE-BOX #7); tests inject
/// recording fakes.
pub trait RelayEngine {
    fn apply_relay_route(&self, domain: &str, route: Option<&RelayRoute>) -> Result<(), String>;
}

impl RelayEngine for StalwartClient {
    fn apply_relay_route(&self, domain: &str, route: Option<&RelayRoute>) -> Result<(), String> {
        self.relay_route_apply(domain, route)
    }
}

fn engine_required(engine: Option<&dyn RelayEngine>) -> Result<&dyn RelayEngine, CfgError> {
    engine.ok_or_else(|| {
        CfgError::NotReady(
            "the mail server is not installed/running — relay routes are applied on the \
             live server; start it in Settings → Email first"
                .to_string(),
        )
    })
}

// ── Relay-config rows ───────────────────────────────────────────────────

/// One `mail_relay_configs` row as this module reads it.
#[derive(Debug, Clone)]
pub struct RelayRow {
    pub id: String,
    pub kind: String,
    pub host: Option<String>,
    pub port: Option<i64>,
    pub username: Option<String>,
    pub secret_ref: Option<String>,
    pub tls_kind: Option<String>,
    pub spf_include: Option<String>,
    pub created_at: i64,
}

const RELAY_COLS: &str =
    "id, kind, host, port, username, secret_ref, tls_kind, spf_include, created_at";

fn map_relay_row(r: &rusqlite::Row) -> rusqlite::Result<RelayRow> {
    Ok(RelayRow {
        id: r.get(0)?,
        kind: r.get(1)?,
        host: r.get(2)?,
        port: r.get(3)?,
        username: r.get(4)?,
        secret_ref: r.get(5)?,
        tls_kind: r.get(6)?,
        spf_include: r.get(7)?,
        created_at: r.get(8)?,
    })
}

fn load_relay(conn: &rusqlite::Connection, id: &str) -> Option<RelayRow> {
    conn.query_row(
        &format!("SELECT {RELAY_COLS} FROM mail_relay_configs WHERE id = ?1"),
        rusqlite::params![id],
        map_relay_row,
    )
    .ok()
}

fn load_all_relays(conn: &rusqlite::Connection) -> Vec<RelayRow> {
    let mut stmt = match conn
        .prepare(&format!("SELECT {RELAY_COLS} FROM mail_relay_configs ORDER BY created_at, id"))
    {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    stmt.query_map([], map_relay_row)
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

/// The wire summary of a relay config — kind + endpoint + username,
/// NEVER the secret (and not even the ref: `hasCredentials` is all a
/// reader needs).
pub fn relay_summary(row: &RelayRow, used_by: &[String]) -> serde_json::Value {
    serde_json::json!({
        "id": row.id,
        "kind": row.kind,
        "host": row.host,
        "port": row.port,
        "username": row.username,
        "tlsKind": row.tls_kind,
        "spfInclude": row.spf_include,
        "hasCredentials": row.secret_ref.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false),
        "usedBy": used_by,
        "createdAt": row.created_at,
    })
}

fn domains_using_relay(conn: &rusqlite::Connection, relay_id: &str) -> Vec<(String, String)> {
    let mut stmt = match conn.prepare(
        "SELECT domain, send_mode FROM mail_domains WHERE relay_config_id = ?1 \
         ORDER BY domain",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    stmt.query_map(rusqlite::params![relay_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

// ── GET /cli/mail/config — the effective configuration ─────────────────

/// The full owner-facing config read: global + per-workspace gating,
/// the §8.4 always-on limits, per-domain send modes, relay-config
/// summaries (no secrets), and the latest server-level doctor grade
/// (the direct-mode gate's context).
pub fn config_json() -> serde_json::Value {
    let settings = k2_core::app_settings::load();

    let (overrides, domains_json, relays_json) = {
        let db = k2_core::db::shared();
        let conn = db.lock();

        // Per-workspace overrides (NULL = inherit — only real
        // overrides ride the wire).
        let mut overrides: Vec<serde_json::Value> = Vec::new();
        if let Ok(mut stmt) = conn.prepare(
            "SELECT path, mail_agent_send, mail_address_cap FROM projects \
             WHERE mail_agent_send IS NOT NULL OR mail_address_cap IS NOT NULL \
             ORDER BY path",
        ) {
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<i64>>(2)?,
                ))
            });
            if let Ok(rows) = rows {
                for (path, send, cap) in rows.flatten() {
                    overrides.push(serde_json::json!({
                        "project": path,
                        "agentSend": send,
                        "addressCap": cap,
                    }));
                }
            }
        }

        let domains_json: Vec<serde_json::Value> = domains::load_all_domains(&conn)
            .iter()
            .map(|d| {
                serde_json::json!({
                    "domain": d.domain,
                    "status": d.status,
                    "sendMode": d.send_mode,
                    "relayConfigId": d.relay_config_id,
                })
            })
            .collect();

        let relays_json: Vec<serde_json::Value> = load_all_relays(&conn)
            .iter()
            .map(|r| {
                let used_by: Vec<String> = domains_using_relay(&conn, &r.id)
                    .into_iter()
                    .map(|(d, _)| d)
                    .collect();
                relay_summary(r, &used_by)
            })
            .collect();

        (overrides, domains_json, relays_json)
    };

    let doctor = super::doctor::latest_run_json(None).ok().flatten().map(|run| {
        serde_json::json!({ "grade": run["grade"], "ranAt": run["ranAt"] })
    });

    serde_json::json!({
        "ok": true,
        "supported": super::supervisor::mail_supported(),
        "agentSend": {
            "default": settings.mail_agent_send,
            "modes": SEND_GATE_MODES,
        },
        "addressCap": { "default": settings.mail_address_cap },
        "workspaceOverrides": overrides,
        "limits": {
            "sendsPerHourPerAddress": super::send::RATE_LIMIT_HOURLY,
            "sendsPerDayPerAddress": super::send::RATE_LIMIT_DAILY,
            "maxRecipients": super::send::MAX_RECIPIENTS,
            "maxMessageBytes": super::send::MAX_MESSAGE_BYTES,
        },
        "domains": domains_json,
        "relayConfigs": relays_json,
        "doctor": doctor,
    })
}

/// The D4 gating vocabulary (mirrors
/// `k2_core::workspace::settings::MAIL_AGENT_SEND_MODES`).
const SEND_GATE_MODES: [&str; 3] = ["off", "approval", "on"];

// ── Relay config CRUD ───────────────────────────────────────────────────

/// The upsert payload (route-parsed from the POST body's `relay`
/// object). `password` and `secret_ref` are MUTUALLY exclusive:
/// `password` is vaulted through the secret store (opaque ref),
/// `secret_ref` is an operator-managed `env:<VAR>` / absolute path.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RelayUpsert {
    pub id: Option<String>,
    pub kind: Option<String>,
    pub host: Option<String>,
    pub port: Option<i64>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub secret_ref: Option<String>,
    pub tls_kind: Option<String>,
    pub spf_include: Option<String>,
}

fn non_empty(s: &Option<String>) -> Option<&str> {
    s.as_deref().map(str::trim).filter(|v| !v.is_empty())
}

/// Create or update a relay config. Validation errors TEACH (each
/// names the field and the accepted values). Updating a config that
/// relay-mode domains use RE-PUSHES their Stalwart routes (engine
/// required then, and only then). Returns the summary JSON.
pub fn upsert_relay(
    secrets: &dyn SecretStore,
    engine: Option<&dyn RelayEngine>,
    up: &RelayUpsert,
) -> Result<serde_json::Value, CfgError> {
    if up.password.is_some() && up.secret_ref.is_some() {
        return Err(CfgError::Usage(
            "give either 'password' (stored in the daemon's secret store) or 'secretRef' \
             (env:<VAR> or an absolute file path) — not both"
                .to_string(),
        ));
    }
    if let Some(kind) = non_empty(&up.kind) {
        if !RELAY_KINDS.contains(&kind) {
            return Err(CfgError::Usage(format!(
                "unknown relay kind '{kind}' — one of: {}",
                RELAY_KINDS.join(", ")
            )));
        }
    }
    if let Some(port) = up.port {
        if !(1..=65535).contains(&port) {
            return Err(CfgError::Usage(format!(
                "relay port must be 1-65535, got {port}"
            )));
        }
    }
    if let Some(tls) = non_empty(&up.tls_kind) {
        if tls != "implicit" && tls != "starttls" {
            return Err(CfgError::Usage(format!(
                "tlsKind must be 'implicit' (465-style) or 'starttls', got '{tls}'"
            )));
        }
    }
    // Resolve the credential input into the ref we persist. A supplied
    // scheme ref must RESOLVE now (a broken pointer should fail at
    // config time, not at send time) — the error names the ref, never
    // a value.
    let new_secret_ref: Option<String> = if let Some(pw) = up.password.as_deref() {
        if pw.is_empty() {
            return Err(CfgError::Usage("relay password must not be empty".to_string()));
        }
        Some(secrets.store("relay", pw).map_err(CfgError::Engine)?)
    } else if let Some(sref) = non_empty(&up.secret_ref) {
        if !crate::mail::secrets::is_scheme_ref(sref) {
            return Err(CfgError::Usage(format!(
                "secretRef '{sref}' is not a recognized form — env:<VAR> or an absolute \
                 file path"
            )));
        }
        match secrets.resolve(sref) {
            Ok(Some(_)) => {}
            Ok(None) => {
                return Err(CfgError::Usage(format!(
                    "secretRef '{sref}' does not resolve to a credential"
                )))
            }
            Err(e) => {
                return Err(CfgError::Usage(format!("secretRef '{sref}' is unusable: {e}")))
            }
        }
        Some(sref.to_string())
    } else {
        None
    };

    match non_empty(&up.id) {
        // ── UPDATE ──
        Some(id) => {
            let existing = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                load_relay(&conn, id)
            };
            let Some(existing) = existing else {
                return Err(CfgError::NotFound(format!(
                    "relay config '{id}' does not exist — GET /cli/mail/config lists them"
                )));
            };
            let old_secret_ref = existing.secret_ref.clone();
            let merged = RelayRow {
                id: existing.id.clone(),
                kind: non_empty(&up.kind).map(str::to_string).unwrap_or(existing.kind),
                host: non_empty(&up.host).map(str::to_string).or(existing.host),
                port: up.port.or(existing.port),
                username: non_empty(&up.username).map(str::to_string).or(existing.username),
                secret_ref: new_secret_ref.clone().or(existing.secret_ref),
                tls_kind: non_empty(&up.tls_kind).map(str::to_string).or(existing.tls_kind),
                spf_include: non_empty(&up.spf_include)
                    .map(str::to_string)
                    .or(existing.spf_include),
                created_at: existing.created_at,
            };
            // Domains currently RELAYING through this config get the
            // updated route pushed before we persist — fail-closed:
            // a push failure leaves the old config in place.
            let relaying: Vec<String> = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                domains_using_relay(&conn, &merged.id)
                    .into_iter()
                    .filter(|(_, mode)| mode == "relay")
                    .map(|(d, _)| d)
                    .collect()
            };
            if !relaying.is_empty() {
                let route = route_from_relay(secrets, &merged)?;
                let engine = engine_required(engine)?;
                for domain in &relaying {
                    engine
                        .apply_relay_route(domain, Some(&route))
                        .map_err(|e| CfgError::Engine(format!("apply relay route for {domain}: {e}")))?;
                }
            }
            {
                let db = k2_core::db::shared();
                let conn = db.lock();
                conn.execute(
                    "UPDATE mail_relay_configs SET kind = ?1, host = ?2, port = ?3, \
                     username = ?4, secret_ref = ?5, tls_kind = ?6, spf_include = ?7 \
                     WHERE id = ?8",
                    rusqlite::params![
                        merged.kind,
                        merged.host,
                        merged.port,
                        merged.username,
                        merged.secret_ref,
                        merged.tls_kind,
                        merged.spf_include,
                        merged.id
                    ],
                )
                .map_err(|e| CfgError::Engine(format!("relay config update: {e}")))?;
            }
            // Old vaulted secret cleanup when replaced by a new one.
            if let (Some(new), Some(old)) = (new_secret_ref.as_deref(), old_secret_ref.as_deref())
            {
                if new != old && old.starts_with("mailsec_") {
                    let _ = secrets.delete(old);
                }
            }
            Ok(relay_summary(&merged, &relaying))
        }
        // ── CREATE ──
        None => {
            let kind = non_empty(&up.kind).unwrap_or("smtp").to_string();
            if kind == "smtp" {
                // V1's generic-SMTP kind requires the full endpoint at
                // create time (the flow the wizard drives); provider
                // kinds carry their own config later.
                let missing: Vec<&str> = [
                    ("host", non_empty(&up.host).is_none()),
                    ("port", up.port.is_none()),
                    ("username", non_empty(&up.username).is_none()),
                    ("password or secretRef", new_secret_ref.is_none()),
                ]
                .iter()
                .filter(|(_, m)| *m)
                .map(|(f, _)| *f)
                .collect();
                if !missing.is_empty() {
                    return Err(CfgError::Usage(format!(
                        "an smtp relay config needs: {} (missing). Also recommended: \
                         spfInclude (the provider's SPF include string, shown on their \
                         setup screen)",
                        missing.join(", ")
                    )));
                }
            }
            let id = format!(
                "rc_{}",
                &uuid::Uuid::new_v4().simple().to_string()[..12]
            );
            let row = RelayRow {
                id: id.clone(),
                kind,
                host: non_empty(&up.host).map(str::to_string),
                port: up.port,
                username: non_empty(&up.username).map(str::to_string),
                secret_ref: new_secret_ref,
                tls_kind: non_empty(&up.tls_kind).map(str::to_string),
                spf_include: non_empty(&up.spf_include).map(str::to_string),
                created_at: now_secs(),
            };
            {
                let db = k2_core::db::shared();
                let conn = db.lock();
                conn.execute(
                    "INSERT INTO mail_relay_configs (id, kind, host, port, username, \
                     secret_ref, tls_kind, spf_include, created_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    rusqlite::params![
                        row.id,
                        row.kind,
                        row.host,
                        row.port,
                        row.username,
                        row.secret_ref,
                        row.tls_kind,
                        row.spf_include,
                        row.created_at
                    ],
                )
                .map_err(|e| CfgError::Engine(format!("relay config insert: {e}")))?;
            }
            Ok(relay_summary(&row, &[]))
        }
    }
}

/// Delete a relay config. Refuses (409) while any domain still
/// references it — the refusal lists them. Vaulted (`mailsec_*`)
/// credentials are removed from the store; operator-managed scheme
/// refs are never touched.
pub fn delete_relay(secrets: &dyn SecretStore, id: &str) -> Result<serde_json::Value, CfgError> {
    let (row, used_by) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let Some(row) = load_relay(&conn, id) else {
            return Err(CfgError::NotFound(format!("relay config '{id}' does not exist")));
        };
        (row, domains_using_relay(&conn, id))
    };
    if !used_by.is_empty() {
        let names: Vec<String> = used_by.iter().map(|(d, _)| d.clone()).collect();
        return Err(CfgError::Conflict(format!(
            "relay config '{id}' is still attached to: {} — switch those domains' send \
             mode (or relay) first",
            names.join(", ")
        )));
    }
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "DELETE FROM mail_relay_configs WHERE id = ?1",
            rusqlite::params![id],
        )
        .map_err(|e| CfgError::Engine(format!("relay config delete: {e}")))?;
    }
    if let Some(sref) = row.secret_ref.as_deref() {
        if sref.starts_with("mailsec_") {
            let _ = secrets.delete(sref);
        }
    }
    Ok(serde_json::json!({ "deleted": id }))
}

/// Build the Stalwart route from a relay row (smtp kind only —
/// callers enforce that). Resolves the credential through the secret
/// store; the value lives only inside the returned route.
fn route_from_relay(secrets: &dyn SecretStore, row: &RelayRow) -> Result<RelayRoute, CfgError> {
    if row.kind != "smtp" {
        return Err(CfgError::Usage(format!(
            "relay kind '{}' is not supported yet (V1 relays over generic SMTP)",
            row.kind
        )));
    }
    let host = row
        .host
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| incomplete("host"))?;
    let port = row
        .port
        .filter(|p| (1..=65535).contains(p))
        .ok_or_else(|| incomplete("port"))?;
    let username = row
        .username
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| incomplete("username"))?;
    let sref = row
        .secret_ref
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| incomplete("credentials"))?;
    let password = match secrets.resolve(sref) {
        Ok(Some(pw)) => pw,
        Ok(None) => {
            return Err(CfgError::Usage(format!(
                "relay config '{}': its credentials are missing from the secret store — \
                 set a new password",
                row.id
            )))
        }
        Err(e) => {
            return Err(CfgError::Engine(format!(
                "relay config '{}': could not resolve credentials: {e}",
                row.id
            )))
        }
    };
    // Default TLS posture: implicit on :465, STARTTLS elsewhere —
    // never plaintext-only (tlsKind pins it explicitly).
    let implicit_tls = match row.tls_kind.as_deref() {
        Some("implicit") => true,
        Some("starttls") => false,
        _ => port == 465,
    };
    Ok(RelayRoute {
        host: host.to_string(),
        port: port as u16,
        username: username.to_string(),
        password,
        implicit_tls,
    })
}

fn incomplete(what: &str) -> CfgError {
    CfgError::Usage(format!(
        "the relay config is incomplete — {what} is required before a domain can relay \
         through it"
    ))
}

// ── Per-domain send mode ────────────────────────────────────────────────

/// Set one domain's send mode (PRD §8.3 D1). `direct` is DOCTOR-GATED;
/// `relay` needs an attached, complete, V1-supported config and pushes
/// the Stalwart route; leaving `relay` clears the route. Returns
/// `{domain, sendMode, relayConfigId, spfNote}` — the SPF row in the
/// record table follows automatically at read time.
pub fn set_send_mode(
    secrets: &dyn SecretStore,
    engine: Option<&dyn RelayEngine>,
    raw_domain: &str,
    mode: &str,
    relay_config_id: Option<&str>,
) -> Result<serde_json::Value, CfgError> {
    let domain =
        k2_core::mail_domain::normalize_mail_domain(raw_domain).map_err(CfgError::Usage)?;
    if !SEND_MODES.contains(&mode) {
        return Err(CfgError::Usage(format!(
            "sendMode must be one of: {} — got '{mode}'",
            SEND_MODES.join(", ")
        )));
    }
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        domains::load_domain(&conn, &domain)
    };
    let Some(row) = row else {
        return Err(CfgError::NotFound(format!(
            "domain '{domain}' is not hosted here — add it first (k2 mail domain add)"
        )));
    };

    let mut new_relay_id: Option<String> = row.relay_config_id.clone();
    let mut spf_note: Option<String> = None;

    match mode {
        "direct" => {
            // The doctor gate (PRD §8.3/§9): a failing grade on the
            // direct-send prerequisites keeps the toggle locked, with
            // the failing checks + provider coaching in the refusal.
            super::doctor::direct_send_gate().map_err(CfgError::Locked)?;
            if row.send_mode == "relay" {
                engine_required(engine)?
                    .apply_relay_route(&domain, None)
                    .map_err(|e| CfgError::Engine(format!("clear relay route: {e}")))?;
            }
            spf_note = Some(
                "SPF for this domain now expects the direct form (v=spf1 mx -all) — \
                 re-check its records"
                    .to_string(),
            );
        }
        "relay" => {
            let attach_id = relay_config_id
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .or_else(|| row.relay_config_id.clone());
            let Some(attach_id) = attach_id else {
                return Err(CfgError::Usage(format!(
                    "domain '{domain}' has no relay config — create one in the same call \
                     (the 'relay' object) or pass 'relayConfigId'"
                )));
            };
            let relay = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                load_relay(&conn, &attach_id)
            };
            let Some(relay) = relay else {
                return Err(CfgError::NotFound(format!(
                    "relay config '{attach_id}' does not exist — GET /cli/mail/config \
                     lists them"
                )));
            };
            let route = route_from_relay(secrets, &relay)?;
            engine_required(engine)?
                .apply_relay_route(&domain, Some(&route))
                .map_err(|e| CfgError::Engine(format!("apply relay route: {e}")))?;
            new_relay_id = Some(attach_id);
            spf_note = Some(match relay.spf_include.as_deref().filter(|s| !s.trim().is_empty()) {
                Some(inc) => format!(
                    "SPF for this domain now expects 'v=spf1 mx include:{inc} ~all' — \
                     re-check its records"
                ),
                None => "this relay config has no spfInclude yet — set it (from your \
                         provider's setup screen) so the SPF record row can include the \
                         relay"
                    .to_string(),
            });
        }
        // receive-only
        _ => {
            if row.send_mode == "relay" {
                engine_required(engine)?
                    .apply_relay_route(&domain, None)
                    .map_err(|e| CfgError::Engine(format!("clear relay route: {e}")))?;
            }
        }
    }

    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "UPDATE mail_domains SET send_mode = ?1, relay_config_id = ?2 WHERE id = ?3",
            rusqlite::params![mode, new_relay_id, row.id],
        )
        .map_err(|e| CfgError::Engine(format!("send mode update: {e}")))?;
    }
    Ok(serde_json::json!({
        "domain": domain,
        "sendMode": mode,
        "relayConfigId": new_relay_id,
        "spfNote": spf_note,
    }))
}

// ── Gating settings (D4/D6) ─────────────────────────────────────────────

/// Per-workspace `mail_agent_send` / `mail_address_cap` overrides.
/// `workspace_path` is ALREADY resolved by the route layer (identity
/// from the token/registry, never trusted raw). Values are validated
/// by `k2_core::workspace::settings::update_project_setting` (a bad
/// write is refused loudly, never stored).
pub fn set_workspace_gating(
    workspace_path: &str,
    agent_send: Option<&str>,
    address_cap: Option<i64>,
) -> Result<serde_json::Value, CfgError> {
    if agent_send.is_none() && address_cap.is_none() {
        return Err(CfgError::Usage(
            "nothing to set for the workspace — give 'agentSend' (off|approval|on) \
             and/or 'addressCap' (0 = unlimited)"
                .to_string(),
        ));
    }
    if let Some(mode) = agent_send {
        k2_core::workspace::settings::update_project_setting(
            workspace_path,
            "mail_agent_send",
            mode,
        )
        .map_err(CfgError::Usage)?;
    }
    if let Some(cap) = address_cap {
        if cap < 0 {
            return Err(CfgError::Usage(format!(
                "addressCap must be a non-negative integer (0 = unlimited), got {cap}"
            )));
        }
        k2_core::workspace::settings::update_project_setting(
            workspace_path,
            "mail_address_cap",
            &cap.to_string(),
        )
        .map_err(CfgError::Usage)?;
    }
    Ok(serde_json::json!({
        "project": workspace_path,
        "agentSend": agent_send,
        "addressCap": address_cap,
    }))
}

/// The GLOBAL defaults (`AppSettings.mail_agent_send` /
/// `mail_address_cap`) — validated HERE (AppSettings stores plain
/// typed fields; a bad value must never be persisted for the
/// fail-closed readers to trip over).
pub fn set_global_defaults(
    agent_send: Option<&str>,
    address_cap: Option<i64>,
) -> Result<serde_json::Value, CfgError> {
    if agent_send.is_none() && address_cap.is_none() {
        return Err(CfgError::Usage(
            "nothing to set in defaults — give 'agentSend' (off|approval|on) and/or \
             'addressCap' (0 = unlimited)"
                .to_string(),
        ));
    }
    let mut partial = serde_json::Map::new();
    if let Some(mode) = agent_send {
        if !SEND_GATE_MODES.contains(&mode) {
            return Err(CfgError::Usage(format!(
                "agentSend must be 'off', 'approval', or 'on', got '{mode}'"
            )));
        }
        partial.insert("mailAgentSend".to_string(), serde_json::json!(mode));
    }
    if let Some(cap) = address_cap {
        if !(0..=u32::MAX as i64).contains(&cap) {
            return Err(CfgError::Usage(format!(
                "addressCap must be a non-negative integer (0 = unlimited), got {cap}"
            )));
        }
        partial.insert("mailAddressCap".to_string(), serde_json::json!(cap));
    }
    k2_core::app_settings::update(serde_json::Value::Object(partial))
        .map_err(CfgError::Engine)?;
    Ok(serde_json::json!({
        "defaults": { "agentSend": agent_send, "addressCap": address_cap },
    }))
}

// ──────────────────────────────────────────────────────────────────────
// Inline unit tests — recording fakes only (no network, house rules)
// ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Recording relay engine: captures every apply/clear; can refuse.
    #[derive(Default)]
    pub(crate) struct FakeRelayEngine {
        pub calls: Mutex<Vec<(String, Option<String>)>>, // (domain, Some(host)|None)
        pub fail: bool,
    }

    impl RelayEngine for FakeRelayEngine {
        fn apply_relay_route(
            &self,
            domain: &str,
            route: Option<&RelayRoute>,
        ) -> Result<(), String> {
            if self.fail {
                return Err("stalwart said no".to_string());
            }
            self.calls
                .lock()
                .unwrap()
                .push((domain.to_string(), route.map(|r| r.host.clone())));
            Ok(())
        }
    }

    /// In-memory secret store (mirrors the send.rs test fakes).
    #[derive(Default)]
    pub(crate) struct MapSecrets {
        pub map: Mutex<std::collections::HashMap<String, String>>,
    }

    impl SecretStore for MapSecrets {
        fn store(&self, kind: &str, secret: &str) -> Result<String, String> {
            let sref = format!("mailsec_{kind}_{}", self.map.lock().unwrap().len());
            self.map.lock().unwrap().insert(sref.clone(), secret.to_string());
            Ok(sref)
        }
        fn resolve(&self, sref: &str) -> Result<Option<String>, String> {
            if crate::mail::secrets::is_scheme_ref(sref) {
                return crate::mail::secrets::resolve_secret_ref(sref).map(Some);
            }
            Ok(self.map.lock().unwrap().get(sref).cloned())
        }
        fn delete(&self, sref: &str) -> Result<(), String> {
            self.map.lock().unwrap().remove(sref);
            Ok(())
        }
    }

    fn cleanup_domain(domain: &str) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = conn.execute(
            "DELETE FROM mail_domains WHERE domain = ?1",
            rusqlite::params![domain],
        );
    }

    fn cleanup_relay(id: &str) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = conn.execute(
            "DELETE FROM mail_relay_configs WHERE id = ?1",
            rusqlite::params![id],
        );
    }

    fn seed_domain(domain: &str, send_mode: &str, relay_id: Option<&str>) {
        cleanup_domain(domain);
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO mail_domains (id, domain, send_mode, relay_config_id, status, \
             created_at) VALUES (?1, ?2, ?3, ?4, 'verified', 100)",
            rusqlite::params![format!("dom-{domain}"), domain, send_mode, relay_id],
        )
        .expect("seed domain");
    }

    fn seed_doctor_run(grade: &str) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = conn.execute("DELETE FROM mail_doctor_runs WHERE domain_id IS NULL", []);
        conn.execute(
            "INSERT INTO mail_doctor_runs (id, domain_id, results_json, grade, ran_at) \
             VALUES (?1, NULL, ?2, ?3, ?4)",
            rusqlite::params![
                format!("mdr-test-{grade}"),
                r#"{"checks":[{"id":"outbound-25","status":"fail","gatesDirect":true}],"directBlockers":["outbound-25"]}"#,
                grade,
                now_secs()
            ],
        )
        .expect("seed doctor run");
    }

    fn clear_doctor_runs() {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = conn.execute("DELETE FROM mail_doctor_runs WHERE domain_id IS NULL", []);
    }

    // ── relay CRUD ──

    #[test]
    fn relay_create_validates_teachingly_and_never_leaks_the_password() {
        let secrets = MapSecrets::default();
        // Both credential forms at once → usage.
        let up = RelayUpsert {
            password: Some("pw".into()),
            secret_ref: Some("env:X".into()),
            ..Default::default()
        };
        let err = upsert_relay(&secrets, None, &up).expect_err("refuse");
        assert!(matches!(&err, CfgError::Usage(h) if h.contains("not both")), "{err:?}");

        // Bad kind / port / tls each teach.
        for (up, needle) in [
            (RelayUpsert { kind: Some("sendgrid".into()), ..Default::default() }, "unknown relay kind"),
            (RelayUpsert { port: Some(0), ..Default::default() }, "1-65535"),
            (RelayUpsert { tls_kind: Some("ssl".into()), ..Default::default() }, "tlsKind"),
        ] {
            let err = upsert_relay(&secrets, None, &up).expect_err("refuse");
            assert!(matches!(&err, CfgError::Usage(h) if h.contains(needle)), "{err:?}");
        }

        // Incomplete smtp create names every missing field.
        let up = RelayUpsert { host: Some("smtp.x.example".into()), ..Default::default() };
        let err = upsert_relay(&secrets, None, &up).expect_err("refuse");
        assert!(
            matches!(&err, CfgError::Usage(h) if h.contains("port") && h.contains("username")
                && h.contains("password or secretRef")),
            "{err:?}"
        );

        // Complete create: password is vaulted; the summary carries NO
        // secret and no ref.
        let up = RelayUpsert {
            host: Some("smtp.mailgun.org".into()),
            port: Some(587),
            username: Some("postmaster@acme.dev".into()),
            password: Some("s3cret-pw".into()),
            spf_include: Some("mailgun.org".into()),
            ..Default::default()
        };
        let v = upsert_relay(&secrets, None, &up).expect("create");
        let id = v["id"].as_str().expect("id").to_string();
        assert!(id.starts_with("rc_"), "{id}");
        assert_eq!(v["kind"], "smtp");
        assert_eq!(v["hasCredentials"], true);
        assert!(!v.to_string().contains("s3cret-pw"), "password must never ride the wire");
        assert!(!v.to_string().contains("mailsec_"), "nor the vault ref");
        // The vault holds it under the persisted ref.
        let sref: String = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.query_row(
                "SELECT secret_ref FROM mail_relay_configs WHERE id = ?1",
                rusqlite::params![id],
                |r| r.get(0),
            )
            .expect("row")
        };
        assert!(sref.starts_with("mailsec_relay_"), "{sref}");
        assert_eq!(secrets.resolve(&sref).unwrap().as_deref(), Some("s3cret-pw"));
        cleanup_relay(&id);
    }

    #[test]
    fn relay_update_repushes_routes_for_relaying_domains_fail_closed() {
        let secrets = MapSecrets::default();
        let up = RelayUpsert {
            host: Some("smtp.old.example".into()),
            port: Some(587),
            username: Some("u".into()),
            password: Some("pw".into()),
            ..Default::default()
        };
        let id = upsert_relay(&secrets, None, &up).expect("create")["id"]
            .as_str()
            .unwrap()
            .to_string();
        seed_domain("cfg-repush.example", "relay", Some(&id));
        seed_domain("cfg-idle.example", "receive-only", Some(&id));

        // Update without an engine while a domain relays → not_ready,
        // and the row is UNCHANGED (fail-closed).
        let up = RelayUpsert { id: Some(id.clone()), host: Some("smtp.new.example".into()), ..Default::default() };
        let err = upsert_relay(&secrets, None, &up).expect_err("refuse");
        assert!(matches!(err, CfgError::NotReady(_)), "{err:?}");
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let host: String = conn
                .query_row(
                    "SELECT host FROM mail_relay_configs WHERE id = ?1",
                    rusqlite::params![id],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(host, "smtp.old.example", "failed update must not persist");
        }

        // With an engine: the RELAYING domain gets the new route
        // pushed; the receive-only one does not.
        let engine = FakeRelayEngine::default();
        let v = upsert_relay(&secrets, Some(&engine), &up).expect("update");
        assert_eq!(v["host"], "smtp.new.example");
        let calls = engine.calls.lock().unwrap().clone();
        assert_eq!(
            calls,
            vec![("cfg-repush.example".to_string(), Some("smtp.new.example".to_string()))]
        );

        // A refusing engine fails the update closed.
        let engine = FakeRelayEngine { fail: true, ..Default::default() };
        let up = RelayUpsert { id: Some(id.clone()), host: Some("smtp.newer.example".into()), ..Default::default() };
        let err = upsert_relay(&secrets, Some(&engine), &up).expect_err("refuse");
        assert!(matches!(err, CfgError::Engine(_)), "{err:?}");

        cleanup_domain("cfg-repush.example");
        cleanup_domain("cfg-idle.example");
        cleanup_relay(&id);
    }

    #[test]
    fn relay_delete_refuses_while_attached_then_deletes_and_purges_vault() {
        let secrets = MapSecrets::default();
        let up = RelayUpsert {
            host: Some("h.example".into()),
            port: Some(587),
            username: Some("u".into()),
            password: Some("pw".into()),
            ..Default::default()
        };
        let id = upsert_relay(&secrets, None, &up).expect("create")["id"]
            .as_str()
            .unwrap()
            .to_string();
        seed_domain("cfg-del.example", "relay", Some(&id));

        let err = delete_relay(&secrets, &id).expect_err("refuse while attached");
        assert!(
            matches!(&err, CfgError::Conflict(h) if h.contains("cfg-del.example")),
            "{err:?}"
        );

        cleanup_domain("cfg-del.example");
        let v = delete_relay(&secrets, &id).expect("delete");
        assert_eq!(v["deleted"], id.as_str());
        assert!(secrets.map.lock().unwrap().is_empty(), "vaulted secret purged");
        let err = delete_relay(&secrets, &id).expect_err("gone");
        assert!(matches!(err, CfgError::NotFound(_)), "{err:?}");
    }

    // ── send mode ──

    #[test]
    fn send_mode_validates_domain_and_mode_at_the_boundary() {
        let secrets = MapSecrets::default();
        let err = set_send_mode(&secrets, None, "not a domain!", "direct", None)
            .expect_err("refuse");
        assert!(matches!(err, CfgError::Usage(_)), "{err:?}");
        let err = set_send_mode(&secrets, None, "ghost-cfg.example", "sideways", None)
            .expect_err("refuse");
        assert!(matches!(&err, CfgError::Usage(h) if h.contains("direct, relay, receive-only")), "{err:?}");
        let err = set_send_mode(&secrets, None, "ghost-cfg.example", "receive-only", None)
            .expect_err("refuse");
        assert!(matches!(&err, CfgError::NotFound(h) if h.contains("k2 mail domain add")), "{err:?}");
    }

    #[test]
    fn direct_mode_is_doctor_gated_with_coaching() {
        let _g = crate::mail::mail_server_test_lock();
        let secrets = MapSecrets::default();
        seed_domain("cfg-direct.example", "receive-only", None);

        // No doctor run on file → locked, names the doctor verb.
        clear_doctor_runs();
        let err = set_send_mode(&secrets, None, "cfg-direct.example", "direct", None)
            .expect_err("locked");
        assert!(matches!(&err, CfgError::Locked(h) if h.contains("k2 mail doctor")), "{err:?}");

        // Failing grade → locked, lists blockers + provider realities.
        seed_doctor_run("fail");
        let err = set_send_mode(&secrets, None, "cfg-direct.example", "direct", None)
            .expect_err("locked");
        let CfgError::Locked(hint) = &err else { panic!("{err:?}") };
        assert!(hint.contains("outbound-25"), "{hint}");
        assert!(hint.contains("GCP") && hint.contains("Hetzner"), "{hint}");
        assert!(hint.contains("Relay mode works everywhere"), "{hint}");

        // Passing grade → allowed (no engine needed: never relayed).
        seed_doctor_run("pass");
        let v = set_send_mode(&secrets, None, "cfg-direct.example", "direct", None)
            .expect("direct unlocks");
        assert_eq!(v["sendMode"], "direct");
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let row = domains::load_domain(&conn, "cfg-direct.example").unwrap();
            assert_eq!(row.send_mode, "direct");
        }

        clear_doctor_runs();
        cleanup_domain("cfg-direct.example");
    }

    #[test]
    fn relay_mode_pushes_the_route_and_leaving_relay_clears_it() {
        let _g = crate::mail::mail_server_test_lock();
        let secrets = MapSecrets::default();
        let up = RelayUpsert {
            host: Some("smtp.relay.example".into()),
            port: Some(465),
            username: Some("u".into()),
            password: Some("pw".into()),
            spf_include: Some("relay.example".into()),
            ..Default::default()
        };
        let rc = upsert_relay(&secrets, None, &up).expect("create")["id"]
            .as_str()
            .unwrap()
            .to_string();
        seed_domain("cfg-relay.example", "receive-only", None);

        // No config attached and none given → teaching usage error.
        let err = set_send_mode(&secrets, None, "cfg-relay.example", "relay", None)
            .expect_err("refuse");
        assert!(matches!(&err, CfgError::Usage(h) if h.contains("relayConfigId")), "{err:?}");

        // No engine → not_ready and the domain row is untouched.
        let err = set_send_mode(&secrets, None, "cfg-relay.example", "relay", Some(&rc))
            .expect_err("refuse");
        assert!(matches!(err, CfgError::NotReady(_)), "{err:?}");

        // Engine present → route pushed, row updated, SPF note teaches.
        let engine = FakeRelayEngine::default();
        let v = set_send_mode(&secrets, Some(&engine), "CFG-RELAY.example.", "relay", Some(&rc))
            .expect("relay set (normalized input)");
        assert_eq!(v["sendMode"], "relay");
        assert_eq!(v["relayConfigId"], rc.as_str());
        assert!(v["spfNote"].as_str().unwrap().contains("include:relay.example"), "{v}");
        assert_eq!(
            engine.calls.lock().unwrap().clone(),
            vec![("cfg-relay.example".to_string(), Some("smtp.relay.example".to_string()))]
        );

        // Leaving relay (→ receive-only) clears the route.
        let engine = FakeRelayEngine::default();
        let v = set_send_mode(&secrets, Some(&engine), "cfg-relay.example", "receive-only", None)
            .expect("clear");
        assert_eq!(v["sendMode"], "receive-only");
        assert_eq!(
            engine.calls.lock().unwrap().clone(),
            vec![("cfg-relay.example".to_string(), None)]
        );

        cleanup_domain("cfg-relay.example");
        cleanup_relay(&rc);
    }

    #[test]
    fn relay_mode_refuses_v1_unsupported_kinds_and_broken_creds() {
        let _g = crate::mail::mail_server_test_lock();
        let secrets = MapSecrets::default();
        // A stored (schema-legal) provider kind cannot ATTACH in V1.
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO mail_relay_configs (id, kind, created_at) \
                 VALUES ('rc-test-mg', 'mailgun', 100)",
                [],
            )
            .expect("seed provider kind");
        }
        seed_domain("cfg-kind.example", "receive-only", None);
        let engine = FakeRelayEngine::default();
        let err = set_send_mode(&secrets, Some(&engine), "cfg-kind.example", "relay", Some("rc-test-mg"))
            .expect_err("refuse");
        assert!(matches!(&err, CfgError::Usage(h) if h.contains("not supported yet")), "{err:?}");
        assert!(engine.calls.lock().unwrap().is_empty(), "nothing pushed");

        // Credentials that no longer resolve refuse (masked: the hint
        // names the config, never a value).
        let up = RelayUpsert {
            host: Some("h.example".into()),
            port: Some(587),
            username: Some("u".into()),
            password: Some("pw".into()),
            ..Default::default()
        };
        let rc = upsert_relay(&secrets, None, &up).expect("create")["id"]
            .as_str()
            .unwrap()
            .to_string();
        secrets.map.lock().unwrap().clear(); // vault purged behind our back
        let err = set_send_mode(&secrets, Some(&engine), "cfg-kind.example", "relay", Some(&rc))
            .expect_err("refuse");
        assert!(matches!(&err, CfgError::Usage(h) if h.contains("missing from the secret store")), "{err:?}");

        cleanup_domain("cfg-kind.example");
        cleanup_relay("rc-test-mg");
        cleanup_relay(&rc);
    }

    // ── gating settings ──

    #[test]
    fn workspace_and_global_gating_validate_and_persist() {
        // Workspace: unknown path surfaces the core error; nothing-to-
        // set teaches.
        let err = set_workspace_gating("/no/such/workspace", Some("approval"), None)
            .expect_err("refuse");
        assert!(matches!(err, CfgError::Usage(_)), "{err:?}");
        let err = set_workspace_gating("/any", None, None).expect_err("refuse");
        assert!(matches!(&err, CfgError::Usage(h) if h.contains("agentSend")), "{err:?}");

        // Globals: bad values refuse loudly (validated BEFORE any
        // write, so no temp home needed for the refusals) …
        let err = set_global_defaults(Some("always"), None).expect_err("refuse");
        assert!(matches!(&err, CfgError::Usage(h) if h.contains("'off', 'approval', or 'on'")), "{err:?}");
        let err = set_global_defaults(None, Some(-3)).expect_err("refuse");
        assert!(matches!(err, CfgError::Usage(_)), "{err:?}");

        // … good values persist and round-trip through AppSettings —
        // inside a sandboxed $HOME (never the dev box's live
        // settings.json).
        crate::test_support::with_temp_home(|| {
            let v = set_global_defaults(Some("approval"), Some(9)).expect("set");
            assert_eq!(v["defaults"]["agentSend"], "approval");
            let after = k2_core::app_settings::load();
            assert_eq!(after.mail_agent_send, "approval");
            assert_eq!(after.mail_address_cap, 9);
        });
    }

    // ── the GET shape ──

    #[test]
    fn config_json_reports_summaries_without_secrets() {
        let _g = crate::mail::mail_server_test_lock();
        let secrets = MapSecrets::default();
        let up = RelayUpsert {
            host: Some("smtp.cfgget.example".into()),
            port: Some(587),
            username: Some("user@cfgget".into()),
            password: Some("super-secret-pw".into()),
            spf_include: Some("cfgget.example".into()),
            ..Default::default()
        };
        let rc = upsert_relay(&secrets, None, &up).expect("create")["id"]
            .as_str()
            .unwrap()
            .to_string();
        seed_domain("cfg-get.example", "relay", Some(&rc));
        seed_doctor_run("warn");

        let v = config_json();
        assert_eq!(v["ok"], true);
        assert_eq!(v["supported"], cfg!(target_os = "linux"));
        assert!(v["agentSend"]["default"].is_string());
        assert!(v["limits"]["sendsPerHourPerAddress"].as_u64().unwrap() > 0);
        let dom = v["domains"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["domain"] == "cfg-get.example")
            .expect("domain listed");
        assert_eq!(dom["sendMode"], "relay");
        assert_eq!(dom["relayConfigId"], rc.as_str());
        let relay = v["relayConfigs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == rc.as_str())
            .expect("relay listed");
        assert_eq!(relay["hasCredentials"], true);
        assert_eq!(relay["usedBy"][0], "cfg-get.example");
        assert!(!v.to_string().contains("super-secret-pw"), "no secret on the wire");
        assert!(!v.to_string().contains("mailsec_"), "no vault refs on the wire");
        assert_eq!(v["doctor"]["grade"], "warn");

        clear_doctor_runs();
        cleanup_domain("cfg-get.example");
        cleanup_relay(&rc);
    }
}
