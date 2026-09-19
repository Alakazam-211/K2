//! `GET|POST /cli/mail/bans`, `POST /cli/mail/bans/clear`,
//! allowlist list/add/remove, and bans migrate/restore/status
//! (prd-hostmail-bans-v1).
//!
//! Extra-gate is the quota pattern (`is_mail_manage_surface` +
//! `post_allowed`). Clear default = authFailure only; `--all` also
//! portScanning. Allowlist uses ReloadSettings (not ReloadBlockedIps).
//! Migrate snapshots authBanRate only — never authBanPeriod.

use std::collections::HashMap;
use std::net::IpAddr;
use std::str::FromStr;

use crate::cli_response::CliResponse;
use crate::mail::domains;
use crate::mail::jmap::{AuthBanRate, IpListEntry, StalwartClient};

/// Standing allowlist IPs (help + clear warn). Never auto-insert.
pub const STANDING_ALLOWLIST_IPS: &[&str] = &["65.130.10.89", "65.130.229.9"];

/// Rejected wholesale NAT (T-Mobile). Do not allowlist.
const REJECTED_ALLOWLIST_CIDR: &str = "172.56.0.0/16";

const AUTH_FAILURE: &str = "authFailure";

fn err_json(status: &'static str, code: &str, hint: String) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: serde_json::json!({
            "ok": false,
            "error": { "code": code, "hint": hint },
        })
        .to_string(),
    }
}

fn map_engine_err(e: String) -> CliResponse {
    err_json("502 Bad Gateway", "engine", e)
}

fn not_ready(hint: String) -> CliResponse {
    err_json("503 Service Unavailable", "not_ready", hint)
}

fn engine() -> Result<StalwartClient, CliResponse> {
    domains::engine_from_db()
        .map(|(c, _)| c)
        .map_err(not_ready)
}

fn entry_json(e: &IpListEntry) -> serde_json::Value {
    serde_json::json!({
        "id": e.id,
        "address": e.address,
        "reason": e.reason,
        "createdAt": e.created_at,
        "expiresAt": e.expires_at,
    })
}

fn address_eq(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

fn normalize_ip_or_cidr(raw: &str) -> Result<String, String> {
    let s = raw.trim();
    if s.is_empty() {
        return Err("missing IP or CIDR".to_string());
    }
    if s.eq_ignore_ascii_case(REJECTED_ALLOWLIST_CIDR) {
        return Err(format!(
            "refusing to allowlist {REJECTED_ALLOWLIST_CIDR} (T-Mobile CGNAT) — \
             allowlist specific customer egress IPs with --expires instead"
        ));
    }
    if let Some((ip, prefix)) = s.split_once('/') {
        let ip = IpAddr::from_str(ip.trim()).map_err(|_| {
            format!("'{s}' is not a valid IP or CIDR")
        })?;
        let bits: u8 = prefix.trim().parse().map_err(|_| {
            format!("'{s}' is not a valid IP or CIDR")
        })?;
        let max = match ip {
            IpAddr::V4(_) => 32u8,
            IpAddr::V6(_) => 128u8,
        };
        if bits > max {
            return Err(format!("'{s}' is not a valid IP or CIDR"));
        }
        return Ok(format!("{ip}/{bits}"));
    }
    let ip = IpAddr::from_str(s).map_err(|_| format!("'{s}' is not a valid IP or CIDR"))?;
    Ok(ip.to_string())
}

fn list_blocked(engine: &StalwartClient) -> Result<Vec<IpListEntry>, String> {
    let ids = engine.blocked_ip_query()?;
    engine.blocked_ip_get(&ids)
}

fn list_allowed(engine: &StalwartClient) -> Result<Vec<IpListEntry>, String> {
    let ids = engine.allowed_ip_query()?;
    engine.allowed_ip_get(&ids)
}

fn bans_list_json(rows: &[IpListEntry]) -> serde_json::Value {
    serde_json::json!({
        "ok": true,
        "bans": rows.iter().map(entry_json).collect::<Vec<_>>(),
    })
}

fn allowlist_json(rows: &[IpListEntry]) -> serde_json::Value {
    serde_json::json!({
        "ok": true,
        "allowlist": rows.iter().map(entry_json).collect::<Vec<_>>(),
    })
}

/// GET|POST `/cli/mail/bans` — list BlockedIp.
pub fn handle_bans_list(_params: &HashMap<String, String>) -> CliResponse {
    let engine = match engine() {
        Ok(e) => e,
        Err(r) => return r,
    };
    match list_blocked(&engine) {
        Ok(rows) => CliResponse::ok_json(bans_list_json(&rows).to_string()),
        Err(e) => map_engine_err(e),
    }
}

pub fn handle_bans_list_post(_body: &[u8]) -> CliResponse {
    handle_bans_list(&HashMap::new())
}

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default)]
struct ClearBody {
    ip: Option<String>,
    all: Option<bool>,
}

/// POST `/cli/mail/bans/clear` `{ip, all?: bool}`.
pub fn handle_bans_clear(body: &[u8]) -> CliResponse {
    let b: ClearBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => {
            return err_json(
                "400 Bad Request",
                "usage",
                format!("invalid JSON body: {e}"),
            )
        }
    };
    let ip = match b.ip.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(ip) => match normalize_ip_or_cidr(ip) {
            Ok(v) => v,
            Err(h) => return err_json("400 Bad Request", "usage", h),
        },
        None => {
            return err_json(
                "400 Bad Request",
                "usage",
                "missing 'ip' — which BlockedIp address to clear?".to_string(),
            )
        }
    };
    // clear takes a host IP, not a CIDR.
    if ip.contains('/') {
        return err_json(
            "400 Bad Request",
            "usage",
            "bans clear takes a single IP, not a CIDR".to_string(),
        );
    }
    let clear_all = b.all.unwrap_or(false);
    let engine = match engine() {
        Ok(e) => e,
        Err(r) => return r,
    };
    let rows = match list_blocked(&engine) {
        Ok(r) => r,
        Err(e) => return map_engine_err(e),
    };
    let mut destroy_ids = Vec::new();
    for row in &rows {
        if !address_eq(&row.address, &ip) {
            continue;
        }
        let reason = row.reason.as_deref().unwrap_or("");
        if clear_all || reason == AUTH_FAILURE {
            destroy_ids.push(row.id.clone());
        }
    }
    let destroyed = if destroy_ids.is_empty() {
        Vec::new()
    } else {
        match engine.blocked_ip_destroy(&destroy_ids) {
            Ok(d) => d,
            Err(e) => return map_engine_err(e),
        }
    };
    if let Err(e) = engine.action_reload_blocked_ips() {
        return map_engine_err(e);
    }
    let allow = match list_allowed(&engine) {
        Ok(a) => a,
        Err(e) => return map_engine_err(e),
    };
    let mut warnings = Vec::new();
    for standing in STANDING_ALLOWLIST_IPS {
        if !allow.iter().any(|e| address_eq(&e.address, standing)) {
            warnings.push(format!(
                "standing IP {standing} is not on the allowlist — pair clear with \
                 `k2 hostmail allowlist add {standing}` (unban does not reset fail counters)"
            ));
        }
    }
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "ip": ip,
            "all": clear_all,
            "destroyed": destroyed,
            "warnings": warnings,
        })
        .to_string(),
    )
}

/// GET|POST `/cli/mail/allowlist`.
pub fn handle_allowlist_list(_params: &HashMap<String, String>) -> CliResponse {
    let engine = match engine() {
        Ok(e) => e,
        Err(r) => return r,
    };
    match list_allowed(&engine) {
        Ok(rows) => CliResponse::ok_json(allowlist_json(&rows).to_string()),
        Err(e) => map_engine_err(e),
    }
}

pub fn handle_allowlist_list_post(_body: &[u8]) -> CliResponse {
    handle_allowlist_list(&HashMap::new())
}

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct AllowAddBody {
    address: Option<String>,
    reason: Option<String>,
    expires_at: Option<String>,
}

/// POST `/cli/mail/allowlist/add`.
pub fn handle_allowlist_add(body: &[u8]) -> CliResponse {
    let b: AllowAddBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => {
            return err_json(
                "400 Bad Request",
                "usage",
                format!("invalid JSON body: {e}"),
            )
        }
    };
    let address = match b
        .address
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(a) => match normalize_ip_or_cidr(a) {
            Ok(v) => v,
            Err(h) => return err_json("400 Bad Request", "usage", h),
        },
        None => {
            return err_json(
                "400 Bad Request",
                "usage",
                "missing 'address' — IP or CIDR to allowlist".to_string(),
            )
        }
    };
    let reason = b
        .reason
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("migration");
    let expires = b
        .expires_at
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if let Some(exp) = expires {
        if chrono_parse_rfc3339(exp).is_err() {
            return err_json(
                "400 Bad Request",
                "usage",
                format!("expiresAt must be RFC3339 (got '{exp}')"),
            );
        }
    }
    let engine = match engine() {
        Ok(e) => e,
        Err(r) => return r,
    };
    let id = match engine.allowed_ip_create(&address, Some(reason), expires) {
        Ok(id) => id,
        Err(e) => return map_engine_err(e),
    };
    if let Err(e) = engine.action_reload_settings() {
        return map_engine_err(e);
    }
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "id": id,
            "address": address,
            "reason": reason,
            "expiresAt": expires,
        })
        .to_string(),
    )
}

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default)]
struct AllowRemoveBody {
    address: Option<String>,
}

/// POST `/cli/mail/allowlist/remove`.
pub fn handle_allowlist_remove(body: &[u8]) -> CliResponse {
    let b: AllowRemoveBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => {
            return err_json(
                "400 Bad Request",
                "usage",
                format!("invalid JSON body: {e}"),
            )
        }
    };
    let address = match b
        .address
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(a) => match normalize_ip_or_cidr(a) {
            Ok(v) => v,
            Err(h) => return err_json("400 Bad Request", "usage", h),
        },
        None => {
            return err_json(
                "400 Bad Request",
                "usage",
                "missing 'address' — IP or CIDR to remove".to_string(),
            )
        }
    };
    let engine = match engine() {
        Ok(e) => e,
        Err(r) => return r,
    };
    let rows = match list_allowed(&engine) {
        Ok(r) => r,
        Err(e) => return map_engine_err(e),
    };
    let ids: Vec<String> = rows
        .iter()
        .filter(|e| address_eq(&e.address, &address))
        .map(|e| e.id.clone())
        .collect();
    if ids.is_empty() {
        return err_json(
            "404 Not Found",
            "not_found",
            format!("no allowlist entry for '{address}'"),
        );
    }
    let destroyed = match engine.allowed_ip_destroy(&ids) {
        Ok(d) => d,
        Err(e) => return map_engine_err(e),
    };
    if let Err(e) = engine.action_reload_settings() {
        return map_engine_err(e);
    }
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "address": address,
            "destroyed": destroyed,
        })
        .to_string(),
    )
}

// ── Migrate snapshot ────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BansMigrateState {
    /// Snapshotted rate; JSON null when Stalwart already had null.
    pub auth_ban_rate: Option<AuthBanRateWire>,
    pub restore_at: i64,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthBanRateWire {
    pub count: u64,
    pub period: u64,
}

impl From<&AuthBanRate> for AuthBanRateWire {
    fn from(r: &AuthBanRate) -> Self {
        Self {
            count: r.count,
            period: r.period,
        }
    }
}

impl From<&AuthBanRateWire> for AuthBanRate {
    fn from(r: &AuthBanRateWire) -> Self {
        Self {
            count: r.count,
            period: r.period,
        }
    }
}

pub fn parse_bans_migrate_json(raw: &str) -> Result<BansMigrateState, String> {
    serde_json::from_str(raw).map_err(|e| format!("bans_migrate_json: {e}"))
}

pub fn bans_migrate_json_roundtrip(state: &BansMigrateState) -> Result<BansMigrateState, String> {
    let s = serde_json::to_string(state).map_err(|e| e.to_string())?;
    parse_bans_migrate_json(&s)
}

fn load_migrate_state() -> Option<BansMigrateState> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let raw: Option<String> = conn
        .query_row(
            "SELECT bans_migrate_json FROM mail_server WHERE id = 1",
            [],
            |r| r.get(0),
        )
        .ok()
        .flatten();
    raw.as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| parse_bans_migrate_json(s).ok())
}

fn store_migrate_state(state: Option<&BansMigrateState>) -> Result<(), String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    match state {
        None => {
            conn.execute(
                "UPDATE mail_server SET bans_migrate_json = NULL, updated_at = ?1 WHERE id = 1",
                rusqlite::params![now],
            )
            .map_err(|e| format!("clear bans_migrate_json: {e}"))?;
        }
        Some(s) => {
            let json = serde_json::to_string(s).map_err(|e| e.to_string())?;
            conn.execute(
                "UPDATE mail_server SET bans_migrate_json = ?1, updated_at = ?2 WHERE id = 1",
                rusqlite::params![json, now],
            )
            .map_err(|e| format!("store bans_migrate_json: {e}"))?;
        }
    }
    Ok(())
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn chrono_parse_rfc3339(s: &str) -> Result<(), String> {
    // Accept RFC3339 / ISO-8601 with Z or offset without pulling chrono
    // if the workspace already has time helpers — keep a strict shape.
    let t = s.trim();
    if t.len() < 20 || !t.contains('T') {
        return Err("not RFC3339".into());
    }
    // Digit-ish YYYY-MM-DDTHH:MM:SS…
    let bytes = t.as_bytes();
    if !(bytes[0].is_ascii_digit()
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && (bytes[10] == b'T' || bytes[10] == b't'))
    {
        return Err("not RFC3339".into());
    }
    Ok(())
}

fn migrate_status_json(state: Option<&BansMigrateState>) -> serde_json::Value {
    match state {
        None => serde_json::json!({
            "ok": true,
            "active": false,
            "restoreAt": null,
            "remainingSeconds": null,
            "authBanRateSnapshot": null,
        }),
        Some(s) => {
            let now = now_secs();
            let remaining = (s.restore_at - now).max(0);
            serde_json::json!({
                "ok": true,
                "active": true,
                "restoreAt": s.restore_at,
                "remainingSeconds": remaining,
                "authBanRateSnapshot": s.auth_ban_rate,
                "overdue": now >= s.restore_at,
            })
        }
    }
}

/// GET|POST `/cli/mail/bans/migrate` — status when no hours body /
/// GET; POST with `{hours}` starts the window.
pub fn handle_migrate_status(_params: &HashMap<String, String>) -> CliResponse {
    CliResponse::ok_json(migrate_status_json(load_migrate_state().as_ref()).to_string())
}

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default)]
struct MigrateBody {
    hours: Option<u64>,
}

pub fn handle_migrate_post(body: &[u8]) -> CliResponse {
    // Empty / `{}` → status (dual GET+POST status). `{hours:N}` → start.
    let b: MigrateBody = if body.is_empty() {
        MigrateBody::default()
    } else {
        match serde_json::from_slice(body) {
            Ok(b) => b,
            Err(e) => {
                return err_json(
                    "400 Bad Request",
                    "usage",
                    format!("invalid JSON body: {e}"),
                )
            }
        }
    };
    let Some(hours) = b.hours else {
        return handle_migrate_status(&HashMap::new());
    };
    if hours == 0 {
        return err_json(
            "400 Bad Request",
            "usage",
            "hours must be >= 1".to_string(),
        );
    }
    if load_migrate_state().is_some() {
        return err_json(
            "409 Conflict",
            "exists",
            "a bans migrate window is already active — \
             `k2 hostmail bans migrate status` or `migrate restore`"
                .to_string(),
        );
    }
    let engine = match engine() {
        Ok(e) => e,
        Err(r) => return r,
    };
    let security = match engine.security_get() {
        Ok(s) => s,
        Err(e) => return map_engine_err(e),
    };
    let snapshot = security.auth_ban_rate.as_ref().map(AuthBanRateWire::from);
    if let Err(e) = engine.security_set_auth_ban_rate(None) {
        return map_engine_err(e);
    }
    if let Err(e) = engine.action_reload_settings() {
        return map_engine_err(e);
    }
    let restore_at = now_secs().saturating_add((hours as i64).saturating_mul(3600));
    let state = BansMigrateState {
        auth_ban_rate: snapshot.clone(),
        restore_at,
    };
    if let Err(e) = store_migrate_state(Some(&state)) {
        return map_engine_err(e);
    }
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "active": true,
            "hours": hours,
            "restoreAt": restore_at,
            "authBanRateSnapshot": snapshot,
            "authBanRate": null,
        })
        .to_string(),
    )
}

/// Restore snapshotted authBanRate. Missing snapshot → scream (B27).
pub fn restore_migrate_now(engine: &StalwartClient) -> Result<serde_json::Value, String> {
    let Some(state) = load_migrate_state() else {
        return Err(
            "no bans migrate snapshot on file — nothing to restore \
             (run `k2 hostmail bans migrate --hours N` first)"
                .to_string(),
        );
    };
    let Some(ref wire) = state.auth_ban_rate else {
        // Snapshot recorded that Stalwart already had null — leave null,
        // clear the window. Do not invent stock 100/day.
        store_migrate_state(None)?;
        return Ok(serde_json::json!({
            "ok": true,
            "restored": false,
            "authBanRate": null,
            "hint": "snapshot had authBanRate null — left null; migrate window cleared",
        }));
    };
    let rate = AuthBanRate::from(wire);
    engine.security_set_auth_ban_rate(Some(&rate))?;
    engine.action_reload_settings()?;
    store_migrate_state(None)?;
    Ok(serde_json::json!({
        "ok": true,
        "restored": true,
        "authBanRate": wire,
    }))
}

/// POST `/cli/mail/bans/migrate/restore`.
pub fn handle_migrate_restore(_body: &[u8]) -> CliResponse {
    let engine = match engine() {
        Ok(e) => e,
        Err(r) => return r,
    };
    match restore_migrate_now(&engine) {
        Ok(v) => CliResponse::ok_json(v.to_string()),
        Err(e) if e.contains("no bans migrate snapshot") => {
            err_json("404 Not Found", "not_found", e)
        }
        Err(e) => map_engine_err(e),
    }
}

/// Health-loop tick: if restoreAt elapsed, restore snapshotted rate.
/// Missing snapshot with an active row → scream in logs, clear row only
/// when rate was explicitly null; otherwise leave for operator.
pub fn tick_bans_migrate_restore() {
    let Some(state) = load_migrate_state() else {
        return;
    };
    if now_secs() < state.restore_at {
        return;
    }
    let engine = match domains::engine_from_db() {
        Ok((c, _)) => c,
        Err(e) => {
            eprintln!("[mail] bans migrate auto-restore: engine not ready: {e}");
            return;
        }
    };
    match restore_migrate_now(&engine) {
        Ok(_) => eprintln!(
            "[mail] bans migrate auto-restore completed (restoreAt={})",
            state.restore_at
        ),
        Err(e) => eprintln!("[mail] bans migrate auto-restore FAILED: {e}"),
    }
}

/// Doctor extras: authFailure bans + migrate overdue. `gates_direct: false`.
/// Engine down → unknown.
pub fn doctor_ban_checks() -> Vec<crate::mail::doctor::DoctorCheck> {
    use crate::mail::doctor::{DoctorCheck, ST_UNKNOWN, ST_WARN, ST_PASS, ST_INFO};

    let mut out = Vec::new();
    match domains::engine_from_db() {
        Err(_) => {
            out.push(DoctorCheck {
                id: "auth-bans".into(),
                label: "Auth-failure IP bans".into(),
                status: ST_UNKNOWN,
                detail: "mail engine not reachable — cannot list BlockedIp".into(),
                gates_direct: false,
            });
        }
        Ok((engine, _)) => match list_blocked(&engine) {
            Err(e) => out.push(DoctorCheck {
                id: "auth-bans".into(),
                label: "Auth-failure IP bans".into(),
                status: ST_UNKNOWN,
                detail: format!("BlockedIp query failed: {e}"),
                gates_direct: false,
            }),
            Ok(rows) => {
                let auth: Vec<_> = rows
                    .iter()
                    .filter(|r| r.reason.as_deref() == Some(AUTH_FAILURE))
                    .collect();
                if auth.is_empty() {
                    out.push(DoctorCheck {
                        id: "auth-bans".into(),
                        label: "Auth-failure IP bans".into(),
                        status: ST_PASS,
                        detail: "no authFailure BlockedIp entries".into(),
                        gates_direct: false,
                    });
                } else {
                    let addrs: Vec<_> = auth.iter().map(|r| r.address.as_str()).collect();
                    out.push(DoctorCheck {
                        id: "auth-bans".into(),
                        label: "Auth-failure IP bans".into(),
                        status: ST_WARN,
                        detail: format!(
                            "{} authFailure ban(s): {} — clear + allowlist \
                             (`k2 hostmail bans clear` / `allowlist add`); \
                             unban does not reset fail counters",
                            auth.len(),
                            addrs.join(", ")
                        ),
                        gates_direct: false,
                    });
                }
            }
        },
    }

    match load_migrate_state() {
        None => out.push(DoctorCheck {
            id: "bans-migrate".into(),
            label: "Auth-ban migrate window".into(),
            status: ST_INFO,
            detail: "no active bans migrate window".into(),
            gates_direct: false,
        }),
        Some(s) if now_secs() >= s.restore_at => out.push(DoctorCheck {
            id: "bans-migrate".into(),
            label: "Auth-ban migrate window".into(),
            status: ST_WARN,
            detail: format!(
                "migrate window overdue (restoreAt={}) — daemon should auto-restore; \
                 run `k2 hostmail bans migrate restore` if still open",
                s.restore_at
            ),
            gates_direct: false,
        }),
        Some(s) => out.push(DoctorCheck {
            id: "bans-migrate".into(),
            label: "Auth-ban migrate window".into(),
            status: ST_INFO,
            detail: format!(
                "active — restores in {}s (restoreAt={})",
                (s.restore_at - now_secs()).max(0),
                s.restore_at
            ),
            gates_direct: false,
        }),
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrate_json_round_trip() {
        let state = BansMigrateState {
            auth_ban_rate: Some(AuthBanRateWire {
                count: 100,
                period: 86400000,
            }),
            restore_at: 1_800_000_000,
        };
        let back = bans_migrate_json_roundtrip(&state).expect("roundtrip");
        assert_eq!(back, state);
        let null_rate = BansMigrateState {
            auth_ban_rate: None,
            restore_at: 1_800_000_000,
        };
        let back = bans_migrate_json_roundtrip(&null_rate).expect("null rate");
        assert_eq!(back.auth_ban_rate, None);
    }

    #[test]
    fn reject_tmobile_cidr() {
        let err = normalize_ip_or_cidr("172.56.0.0/16").expect_err("reject");
        assert!(err.contains("172.56.0.0/16"), "{err}");
        assert!(normalize_ip_or_cidr("65.130.10.89").is_ok());
        assert!(normalize_ip_or_cidr("10.0.0.0/8").is_ok());
    }

    #[test]
    fn standing_ips_are_documented_constants() {
        assert_eq!(STANDING_ALLOWLIST_IPS, &["65.130.10.89", "65.130.229.9"]);
    }
}
