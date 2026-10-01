//! App heartbeats surface (`prd-app-heartbeats-surface-v1.md` AH1–AH25,
//! vs-live AH26–AH34, Rosson R1–R3).
//!
//! An app pass (`k2skn_`) with `heartbeats:read` / `heartbeats:write` on a
//! room reaches that room's heartbeats through these arms, which sit
//! before the `/cli/heartbeat/` prefix arm in the dispatcher (AH4).
//!
//! - **Jail (AH3).** The guest names a room (`workspace`), never a path.
//!   Each arm resolves the room with `resolve_skin_workspace`, then calls
//!   core with that room root. No guest `project`, `project_path`,
//!   `project_id`, `force`, or other key is ever forwarded.
//! - **Caps (AH2).** Room first (403 `skin_room`), then the cap in that
//!   room (403 `missing capability <cap>`).
//! - **Projection (AH8, AH10, AH33).** Guest rows and replies carry only
//!   the listed keys. Any string holding the room root has it rewritten
//!   to `.`.
//! - **Writes (AH11–AH15, AH26, R1, R3).** Same core calls as the owner,
//!   plus: instructions required on add, an explicit `enabled` boolean, a
//!   5-minute interval floor, and "remove" is archive.
//! - **Fire now (AH16, AH27, R3).** Pre-checks, then the manual launch
//!   with a lease claim that refuses while any fire is in flight. Rate
//!   limited per heartbeat per app login and per room.
//! - **Audit (AH18).** Every write and fire runs under the app actor
//!   (`app:<username>` / `app-token:<name>`).

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde_json::{json, Map, Value};

use k2_core::db::schema::{AgentHeartbeat, HeartbeatFire};
use k2_core::heartbeats as hb;
use k2_core::skin::{SkinPass, CAP_FILES_READ, CAP_HEARTBEATS_READ, CAP_HEARTBEATS_WRITE};

use crate::cli_response::CliResponse;

/// AH5 — `GET` reads (`heartbeats:read`).
pub const READ_PATHS: [&str; 4] = [
    "/cli/heartbeat/list",
    "/cli/heartbeat/show",
    "/cli/heartbeat/status",
    "/cli/heartbeat/fires-list",
];

/// AH6 — `POST` writes with a JSON body (`heartbeats:write`).
pub const WRITE_PATHS: [&str; 6] = [
    "/cli/heartbeat/add",
    "/cli/heartbeat/edit",
    "/cli/heartbeat/enable",
    "/cli/heartbeat/rename",
    "/cli/heartbeat/archive",
    "/cli/heartbeat/fire",
];

/// AH11 — the WAKEUP.md body an app may send, in bytes.
pub const MAX_INSTRUCTIONS_BYTES: usize = 16 * 1024;

/// R3 — the shortest interval an app heartbeat may use (5 minutes). The
/// owner floor stays [`hb::MIN_EVERY_SECONDS`].
pub const APP_MIN_EVERY_SECONDS: u64 = 300;

/// R3 — fire now: once per heartbeat per app login per 5 minutes.
pub const FIRE_PER_HEARTBEAT_WINDOW: Duration = Duration::from_secs(300);
/// R3 — fire now: 12 per room per hour, across every pass.
pub const FIRE_PER_ROOM_MAX: usize = 12;
pub const FIRE_PER_ROOM_WINDOW: Duration = Duration::from_secs(3600);
/// AH17 — other writes: 60 per pass per room per 10 minutes.
pub const WRITES_PER_PASS_MAX: usize = 60;
pub const WRITES_PER_PASS_WINDOW: Duration = Duration::from_secs(600);

/// History default and cap (AH10).
const FIRES_LIST_DEFAULT: i64 = 50;
const HISTORY_MAX: i64 = 200;

/// True for the ten app doors (AH5 + AH6). Every other heartbeat path is
/// never an app door (AH7).
pub fn is_app_door(path: &str) -> bool {
    READ_PATHS.contains(&path) || WRITE_PATHS.contains(&path)
}

// ── Responses ──────────────────────────────────────────────────────────

fn json_response(status: &'static str, body: Value) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: body.to_string(),
    }
}

fn err(status: &'static str, code: &str) -> CliResponse {
    json_response(status, json!({ "error": code }))
}

fn bad_request(root: &str, msg: &str) -> CliResponse {
    json_response(
        "400 Bad Request",
        json!({ "error": sanitize(root, msg) }),
    )
}

/// `404 no_such_heartbeat` (AH9, AH13). Also used for the owner route.
pub fn no_such_heartbeat_response(message: &str) -> CliResponse {
    json_response(
        "404 Not Found",
        json!({ "error": hb::NO_SUCH_HEARTBEAT, "message": message }),
    )
}

fn not_found() -> CliResponse {
    err("404 Not Found", hb::NO_SUCH_HEARTBEAT)
}

fn method_not_allowed() -> CliResponse {
    err("405 Method Not Allowed", "method not allowed for this route")
}

fn rate_limited(retry_after_secs: u64) -> CliResponse {
    json_response(
        "429 Too Many Requests",
        json!({ "error": "rate_limited", "retryAfterSecs": retry_after_secs }),
    )
}

/// AH17 — the `Retry-After` line for a 429 from these arms. Empty for any
/// other response.
pub fn extra_headers(resp: &CliResponse) -> String {
    if !resp.status.starts_with("429") {
        return String::new();
    }
    let secs = serde_json::from_str::<Value>(&resp.body)
        .ok()
        .and_then(|v| v.get("retryAfterSecs").and_then(Value::as_u64));
    match secs {
        Some(s) => format!("Retry-After: {s}\r\n"),
        None => String::new(),
    }
}

// ── Sanitizer (AH10, AH33) ──────────────────────────────────────────────

/// Rewrite the room root (as stored and canonicalized) to `.`.
pub fn sanitize(root: &str, s: &str) -> String {
    let mut out = s.to_string();
    let mut roots: Vec<String> = Vec::new();
    if let Ok(c) = std::fs::canonicalize(root) {
        roots.push(c.to_string_lossy().into_owned());
    }
    roots.push(root.to_string());
    roots.sort_by_key(|r| std::cmp::Reverse(r.len()));
    for r in roots {
        let r = r.trim_end_matches('/');
        if r.is_empty() || r == "." {
            continue;
        }
        out = out.replace(r, ".");
    }
    out
}

fn sanitize_opt(root: &str, s: Option<&str>) -> Value {
    match s {
        Some(v) => Value::String(sanitize(root, v)),
        None => Value::Null,
    }
}

// ── Projection (AH8, AH10) ──────────────────────────────────────────────

/// AH8 — the guest row. Never the table.
pub fn guest_row(root: &str, h: &AgentHeartbeat) -> Value {
    let spec = serde_json::from_str::<Value>(&h.spec_json).unwrap_or(Value::Null);
    json!({
        "name": h.name,
        "frequency": h.frequency,
        "spec": spec,
        "enabled": h.enabled,
        "createdAt": h.created_at,
        "lastFired": h.last_fired,
        "nextFireAt": h.next_fire_at,
        "waitReason": h.wait_reason,
        "waitDetail": sanitize_opt(root, h.wait_detail.as_deref()),
        "waitSince": h.wait_since,
        "disabledReason": h.disabled_reason,
        "scheduleError": sanitize_opt(root, h.schedule_error.as_deref()),
        "firing": h.in_flight_started_at.is_some(),
        "delivery": if h.use_workspace_session { "pinned" } else { "own" },
    })
}

/// AH10 — the guest history row.
pub fn guest_fire(root: &str, f: &HeartbeatFire) -> Value {
    json!({
        "firedAt": f.fired_at,
        "name": f.schedule_name,
        "decision": f.decision,
        "reason": sanitize_opt(root, f.reason.as_deref()),
        "durationMs": f.duration_ms,
        "actor": f.actor,
    })
}

// ── Rate limits (AH17, R3) ──────────────────────────────────────────────

/// In-memory, process-local sliding windows (the `login_throttle.rs`
/// pattern). Owner calls never reach it.
#[derive(Default)]
pub struct Limiter {
    hits: HashMap<String, VecDeque<Instant>>,
}

impl Limiter {
    fn prune(q: &mut VecDeque<Instant>, now: Instant, window: Duration) {
        while let Some(front) = q.front() {
            if now.duration_since(*front) >= window {
                q.pop_front();
            } else {
                break;
            }
        }
    }

    /// Seconds until a slot frees, or `None` when `key` is under `max`.
    fn retry_after(&mut self, key: &str, max: usize, window: Duration, now: Instant) -> Option<u64> {
        let q = self.hits.entry(key.to_string()).or_default();
        Self::prune(q, now, window);
        if q.len() < max {
            return None;
        }
        let oldest = *q.front()?;
        let wait = window.saturating_sub(now.duration_since(oldest));
        Some(wait.as_secs().max(1))
    }

    fn record(&mut self, key: &str, now: Instant) {
        self.hits.entry(key.to_string()).or_default().push_back(now);
    }

    /// Undo the newest hit for `key` (a fire the lease refused).
    fn refund(&mut self, key: &str) {
        if let Some(q) = self.hits.get_mut(key) {
            q.pop_back();
        }
    }
}

fn limiter() -> &'static Mutex<Limiter> {
    static L: OnceLock<Mutex<Limiter>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(Limiter::default()))
}

/// Test hook: forget every window.
#[allow(dead_code)]
pub fn reset_rate_limits_for_tests() {
    limiter().lock().hits.clear();
}

/// "Per app login": the principal for a login pass (so a re-login does not
/// reset the window), the token id for a platform token.
fn login_key(pass: &SkinPass) -> String {
    match pass.principal_id.as_deref() {
        Some(p) if !p.is_empty() => format!("principal:{p}"),
        _ => format!("token:{}", pass.id),
    }
}

struct FireReservation {
    per_heartbeat: String,
    per_room: String,
}

/// Reserve one fire against both windows, atomically. `Err(secs)` = 429.
fn reserve_fire(pass: &SkinPass, project_id: &str, name: &str) -> Result<FireReservation, u64> {
    let per_heartbeat = format!("fire:{}:{project_id}:{name}", login_key(pass));
    let per_room = format!("fire-room:{project_id}");
    let now = Instant::now();
    let mut l = limiter().lock();
    let a = l.retry_after(&per_heartbeat, 1, FIRE_PER_HEARTBEAT_WINDOW, now);
    let b = l.retry_after(&per_room, FIRE_PER_ROOM_MAX, FIRE_PER_ROOM_WINDOW, now);
    if let Some(secs) = a.into_iter().chain(b).max() {
        return Err(secs);
    }
    l.record(&per_heartbeat, now);
    l.record(&per_room, now);
    Ok(FireReservation {
        per_heartbeat,
        per_room,
    })
}

fn refund_fire(r: &FireReservation) {
    let mut l = limiter().lock();
    l.refund(&r.per_heartbeat);
    l.refund(&r.per_room);
}

fn reserve_write(pass: &SkinPass, project_id: &str) -> Result<(), u64> {
    let key = format!("write:{}:{project_id}", login_key(pass));
    let now = Instant::now();
    let mut l = limiter().lock();
    if let Some(secs) = l.retry_after(&key, WRITES_PER_PASS_MAX, WRITES_PER_PASS_WINDOW, now) {
        return Err(secs);
    }
    l.record(&key, now);
    Ok(())
}

// ── Body helpers ────────────────────────────────────────────────────────

/// AH6 owner/Connect/passport POST: fold a JSON object body into the
/// params. The query wins on a clash; `null` is skipped; objects and
/// arrays (e.g. `spec`) become their JSON text.
pub fn fold_json_body(params: &mut HashMap<String, String>, body: &[u8]) {
    if body.is_empty() {
        return;
    }
    let Ok(Value::Object(map)) = serde_json::from_slice::<Value>(body) else {
        return;
    };
    for (k, v) in map {
        if params.contains_key(&k) {
            continue;
        }
        let s = match v {
            Value::Null => continue,
            Value::String(s) => s,
            Value::Bool(b) => b.to_string(),
            Value::Number(n) => n.to_string(),
            other => other.to_string(),
        };
        params.insert(k, s);
    }
}

fn str_field(body: &Map<String, Value>, key: &str) -> Option<String> {
    body.get(key)
        .and_then(Value::as_str)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// `spec` as stored JSON text: an object is serialized, a string is taken
/// as JSON text, absent = `None`.
fn spec_field(body: &Map<String, Value>) -> Option<String> {
    match body.get("spec") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(other) => Some(other.to_string()),
    }
}

/// R3 — the app interval floor (hourly `every_seconds` ≥ 5 min). Other
/// frequencies run at most once a day.
fn check_app_floor(frequency: &str, spec_json: &str) -> Result<(), String> {
    if frequency != "hourly" || spec_json.trim().is_empty() {
        return Ok(());
    }
    let v: Value = serde_json::from_str(spec_json)
        .map_err(|e| format!("schedule spec is not valid JSON: {e}"))?;
    if let Some(every) = v.get("every_seconds") {
        let ok = every
            .as_u64()
            .map(|n| n >= APP_MIN_EVERY_SECONDS)
            .unwrap_or(false);
        if !ok {
            return Err(format!(
                "interval_too_short: app heartbeats fire at most every {APP_MIN_EVERY_SECONDS} seconds (got {every})"
            ));
        }
    }
    Ok(())
}

fn check_instructions(text: &str) -> Result<(), CliResponse> {
    if text.trim().is_empty() {
        return Err(err("400 Bad Request", "instructions_required"));
    }
    if text.len() > MAX_INSTRUCTIONS_BYTES {
        return Err(json_response(
            "400 Bad Request",
            json!({ "error": "instructions_too_large", "maxBytes": MAX_INSTRUCTIONS_BYTES }),
        ));
    }
    Ok(())
}

// ── Row lookups ─────────────────────────────────────────────────────────

/// A non-archived row in the room, or 404 (archived and unknown look the
/// same to an app).
fn active_row(project_id: &str, name: &str) -> Result<AgentHeartbeat, CliResponse> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    match AgentHeartbeat::get_by_name(&conn, project_id, name) {
        Ok(Some(h)) if h.archived_at.is_none() => {
            let mut rows = vec![h];
            hb::wait::overlay_no_ticks_now(&conn, &mut rows);
            Ok(rows.remove(0))
        }
        Ok(_) => Err(not_found()),
        Err(e) => Err(CliResponse::internal_error(e)),
    }
}

fn row_reply(root: &str, project_id: &str, name: &str) -> CliResponse {
    match active_row(project_id, name) {
        Ok(h) => json_response("200 OK", guest_row(root, &h)),
        Err(r) => r,
    }
}

fn core_err(root: &str, e: &str) -> CliResponse {
    if hb::is_no_such_heartbeat(e) {
        return not_found();
    }
    if e.starts_with("instructions_required") {
        return err("400 Bad Request", "instructions_required");
    }
    bad_request(root, e)
}

// ── The arm ─────────────────────────────────────────────────────────────

struct Room {
    project_id: String,
    root: String,
}

/// Dispatch one app request. `pass` is `None` when the `k2skn_` token did
/// not resolve (revoked / expired → 401).
pub fn handle(
    method: &str,
    path: &str,
    query: &HashMap<String, String>,
    raw_body: &[u8],
    pass: Option<SkinPass>,
) -> CliResponse {
    let Some(pass) = pass else {
        return crate::skin_routes::revoked_skin_response();
    };
    let is_read = READ_PATHS.contains(&path);
    if !is_read && !WRITE_PATHS.contains(&path) {
        return crate::cli::CliResponse::forbidden();
    }
    if (is_read && method != "GET") || (!is_read && method != "POST") {
        return method_not_allowed();
    }
    let cap = if is_read {
        CAP_HEARTBEATS_READ
    } else {
        CAP_HEARTBEATS_WRITE
    };
    if !pass.dispatcher_admits_cap(cap) {
        return crate::skin_routes::missing_cap_response(cap);
    }
    let body: Map<String, Value> = if is_read {
        Map::new()
    } else {
        match serde_json::from_slice::<Value>(raw_body) {
            Ok(Value::Object(m)) => m,
            Ok(_) => return err("400 Bad Request", "body must be a JSON object"),
            Err(e) => {
                return json_response(
                    "400 Bad Request",
                    json!({ "error": format!("invalid JSON body: {e}") }),
                )
            }
        }
    };
    let workspace = str_field(&body, "workspace").or_else(|| {
        query
            .get("workspace")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    });
    let Some(workspace) = workspace else {
        return err("400 Bad Request", "missing workspace");
    };
    let resolved = match crate::fs_routes::resolve_skin_workspace(&pass, &workspace) {
        Ok(r) => r,
        Err(r) => return r,
    };
    if !pass.has_cap_in_room(&resolved.project_id, cap) {
        return crate::skin_routes::missing_cap_response(cap);
    }
    let room = Room {
        project_id: resolved.project_id,
        root: resolved.path,
    };
    let actor = crate::routes::http::app_actor_label(&pass);
    crate::heartbeat_routes::with_request_actor(Some(actor), || {
        if is_read {
            handle_read(path, query, &pass, &room)
        } else if path == "/cli/heartbeat/fire" {
            handle_fire(&body, &pass, &room)
        } else {
            if let Err(secs) = reserve_write(&pass, &room.project_id) {
                return rate_limited(secs);
            }
            handle_write(path, &body, &room)
        }
    })
}

fn handle_read(
    path: &str,
    query: &HashMap<String, String>,
    pass: &SkinPass,
    room: &Room,
) -> CliResponse {
    let root = room.root.as_str();
    match path {
        "/cli/heartbeat/list" => match hb::k2so_heartbeat_list(root.to_string()) {
            Ok(rows) => json_response(
                "200 OK",
                Value::Array(rows.iter().map(|h| guest_row(root, h)).collect()),
            ),
            Err(e) => core_err(root, &e),
        },
        "/cli/heartbeat/show" => {
            let Some(name) = query.get("name").map(|s| s.trim()).filter(|s| !s.is_empty()) else {
                return err("400 Bad Request", "missing name");
            };
            let h = match active_row(&room.project_id, name) {
                Ok(h) => h,
                Err(r) => return r,
            };
            let mut v = guest_row(root, &h);
            // AH9 / R2: the WAKEUP.md body follows file access.
            if pass.has_cap_in_room(&room.project_id, CAP_FILES_READ) {
                match hb::k2so_heartbeat_instructions(root, name) {
                    Ok((body, rel)) => {
                        v["instructions"] = Value::String(body);
                        v["wakeupPath"] = Value::String(rel);
                    }
                    Err(e) => return core_err(root, &e),
                }
            } else {
                v["instructionsHidden"] = Value::Bool(true);
            }
            json_response("200 OK", v)
        }
        "/cli/heartbeat/status" => {
            let Some(name) = query.get("name").map(|s| s.trim()).filter(|s| !s.is_empty()) else {
                return err("400 Bad Request", "missing name");
            };
            let limit = query
                .get("limit")
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or(10)
                .clamp(1, HISTORY_MAX);
            let rows = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                HeartbeatFire::list_by_schedule_name(&conn, &room.project_id, name, limit)
            };
            match rows {
                Ok(rows) => json_response(
                    "200 OK",
                    Value::Array(rows.iter().map(|f| guest_fire(root, f)).collect()),
                ),
                Err(e) => CliResponse::internal_error(e),
            }
        }
        _ => {
            let limit = query
                .get("limit")
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or(FIRES_LIST_DEFAULT)
                .clamp(1, HISTORY_MAX);
            match hb::k2so_heartbeat_fires_list(root.to_string(), Some(limit)) {
                Ok(rows) => json_response(
                    "200 OK",
                    Value::Array(rows.iter().map(|f| guest_fire(root, f)).collect()),
                ),
                Err(e) => core_err(root, &e),
            }
        }
    }
}

fn handle_write(path: &str, body: &Map<String, Value>, room: &Room) -> CliResponse {
    let root = room.root.as_str();
    match path {
        "/cli/heartbeat/add" => {
            let Some(name) = str_field(body, "name") else {
                return err("400 Bad Request", "missing name");
            };
            let Some(frequency) = str_field(body, "frequency") else {
                return err("400 Bad Request", "missing frequency");
            };
            let spec = spec_field(body).unwrap_or_else(|| "{}".to_string());
            let instructions = body
                .get("instructions")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if let Err(r) = check_instructions(&instructions) {
                return r;
            }
            if let Err(e) = check_app_floor(&frequency, &spec) {
                return bad_request(root, &e);
            }
            let result = crate::heartbeat_routes::with_change(
                root,
                &name,
                "added",
                hb::k2so_heartbeat_add_with_instructions(
                    root.to_string(),
                    name.clone(),
                    frequency,
                    spec,
                    Some(instructions),
                )
                .map(|v| v.to_string()),
            );
            match result {
                Ok(_) => row_reply(root, &room.project_id, &name),
                Err(e) => core_err(root, &e),
            }
        }
        "/cli/heartbeat/edit" => {
            let Some(name) = str_field(body, "name") else {
                return err("400 Bad Request", "missing name");
            };
            let current = match active_row(&room.project_id, &name) {
                Ok(h) => h,
                Err(r) => return r,
            };
            let mut frequency = str_field(body, "frequency");
            let spec = spec_field(body);
            if frequency.is_none() && spec.is_some() {
                frequency = Some(current.frequency.clone());
            }
            let instructions = match body.get("instructions") {
                None | Some(Value::Null) => None,
                Some(Value::String(s)) => Some(s.clone()),
                Some(_) => return err("400 Bad Request", "instructions must be a string"),
            };
            if frequency.is_none() && instructions.is_none() {
                return err("400 Bad Request", "nothing to edit");
            }
            if let Some(text) = instructions.as_deref() {
                if let Err(r) = check_instructions(text) {
                    return r;
                }
            }
            if let Some(freq) = frequency.as_deref() {
                let spec_text = spec.clone().unwrap_or_else(|| current.spec_json.clone());
                if let Err(e) = check_app_floor(freq, &spec_text) {
                    return bad_request(root, &e);
                }
            }
            let spec = match (&frequency, spec) {
                (Some(_), None) => Some(current.spec_json.clone()),
                (_, s) => s,
            };
            match crate::heartbeat_routes::apply_edit(root, &name, frequency, spec, instructions) {
                Ok(reason) => {
                    let _ = crate::heartbeat_routes::with_change(
                        root,
                        &name,
                        reason,
                        Ok(String::new()),
                    );
                    row_reply(root, &room.project_id, &name)
                }
                Err(e) => core_err(root, &e),
            }
        }
        "/cli/heartbeat/enable" => {
            let Some(name) = str_field(body, "name") else {
                return err("400 Bad Request", "missing name");
            };
            // AH26: an explicit boolean. A dropped field never enables.
            let Some(enabled) = body.get("enabled").and_then(Value::as_bool) else {
                return err("400 Bad Request", "enabled must be true or false");
            };
            if let Err(r) = active_row(&room.project_id, &name) {
                return r;
            }
            let result = crate::heartbeat_routes::with_change(
                root,
                &name,
                if enabled { "enabled" } else { "disabled" },
                hb::k2so_heartbeat_set_enabled(root.to_string(), name.clone(), enabled)
                    .map(|_| String::new()),
            );
            match result {
                Ok(_) => row_reply(root, &room.project_id, &name),
                Err(e) => core_err(root, &e),
            }
        }
        "/cli/heartbeat/rename" => {
            let Some(from) = str_field(body, "from") else {
                return err("400 Bad Request", "missing from");
            };
            let Some(to) = str_field(body, "to") else {
                return err("400 Bad Request", "missing to");
            };
            if let Err(r) = active_row(&room.project_id, &from) {
                return r;
            }
            let reason = format!("renamed from {from}");
            let result = crate::heartbeat_routes::with_change(
                root,
                &to,
                &reason,
                hb::k2so_heartbeat_rename(root.to_string(), from, to.clone()).map(|_| String::new()),
            );
            match result {
                Ok(_) => row_reply(root, &room.project_id, &to),
                Err(e) => core_err(root, &e),
            }
        }
        // AH14 / R1: an app's "remove" is a soft archive.
        _ => {
            let Some(name) = str_field(body, "name") else {
                return err("400 Bad Request", "missing name");
            };
            if let Err(r) = active_row(&room.project_id, &name) {
                return r;
            }
            let result = crate::heartbeat_routes::with_change(
                root,
                &name,
                "archived",
                hb::k2so_heartbeat_archive(root.to_string(), name.clone()).map(|_| String::new()),
            );
            match result {
                Ok(_) => json_response("200 OK", json!({ "success": true, "name": name, "archived": true })),
                Err(e) => core_err(root, &e),
            }
        }
    }
}

fn handle_fire(body: &Map<String, Value>, pass: &SkinPass, room: &Room) -> CliResponse {
    let root = room.root.as_str();
    // AH16: an app never sends `force`; it is not read.
    let Some(name) = str_field(body, "name") else {
        return err("400 Bad Request", "missing name");
    };
    let h = match active_row(&room.project_id, &name) {
        Ok(h) => h,
        Err(r) => return r,
    };
    if !h.enabled {
        return err("409 Conflict", "disabled");
    }
    if h.in_flight_started_at.is_some() {
        return err("409 Conflict", "in_flight");
    }
    if h.wait_reason.as_deref() == Some("backoff") {
        return json_response(
            "409 Conflict",
            json!({ "error": "backoff", "nextFireAt": h.next_fire_at }),
        );
    }
    match hb::k2so_heartbeat_instructions(root, &name) {
        Ok((text, _)) if text.trim().is_empty() => return err("409 Conflict", "wakeup_empty"),
        Ok(_) => {}
        Err(e) => return core_err(root, &e),
    }
    let reservation = match reserve_fire(pass, &room.project_id, &name) {
        Ok(r) => r,
        Err(secs) => return rate_limited(secs),
    };
    // AH27: the lease claim refuses under ANY concurrency policy, inside
    // the lease transaction.
    let out = crate::heartbeat_launch::smart_launch_app(root, &name);
    let decision = out
        .get("decision")
        .and_then(Value::as_str)
        .unwrap_or("error")
        .to_string();
    if decision == "skipped_locked" {
        refund_fire(&reservation);
        return err("409 Conflict", "in_flight");
    }
    let reason = out.get("reason").and_then(Value::as_str);
    // AH33: never terminalId / projectPath / agentName / sessionId.
    json_response(
        "200 OK",
        json!({
            "success": out.get("success").and_then(Value::as_bool).unwrap_or(false),
            "decision": decision,
            "reason": sanitize_opt(root, reason),
            "name": name,
        }),
    )
}

/// `true` when `rel` (workspace-relative, as a guest sent it) lexically
/// names a path at or under `.k2/heartbeats` (R2). ASCII case is folded so
/// a case-insensitive disk cannot dodge it.
pub fn lexically_in_heartbeats(rel: &str) -> bool {
    let comps = normal_components(rel);
    comps.len() >= 2 && comps[0] == ".k2" && comps[1] == "heartbeats"
}

/// `true` when `rel` names `.`, `.k2`, or a path at or under
/// `.k2/heartbeats` (moving or renaming any of those moves the folder).
pub fn lexically_touches_heartbeats(rel: &str) -> bool {
    let comps = normal_components(rel);
    comps.is_empty()
        || (comps.len() == 1 && comps[0] == ".k2")
        || lexically_in_heartbeats(rel)
}

fn normal_components(rel: &str) -> Vec<String> {
    Path::new(rel.trim())
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => Some(s.to_string_lossy().to_ascii_lowercase()),
            _ => None,
        })
        .collect()
}

/// R2 — the heartbeat name when `rel` is exactly
/// `.k2/heartbeats/<name>/WAKEUP.md` (case as written).
pub fn exact_wakeup_name(rel: &str) -> Option<String> {
    let comps: Vec<String> = Path::new(rel.trim())
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            std::path::Component::CurDir => None,
            _ => Some(String::from("\u{0}")),
        })
        .collect();
    match comps.as_slice() {
        [k2, hbs, name, file]
            if k2 == ".k2" && hbs == "heartbeats" && file == "WAKEUP.md" =>
        {
            Some(name.clone())
        }
        _ => None,
    }
}

/// R2 — `true` when the absolute `abs` (already jailed) is at or under the
/// room's `.k2/heartbeats`, or (when `ancestors` is set) is the room root
/// or `.k2` itself. Compared canonically and ASCII case-folded, so a
/// symlink or a case variant inside the room cannot reach the folder.
pub fn abs_touches_heartbeats(root: &str, abs: &Path, ancestors: bool) -> bool {
    let canon_root = std::fs::canonicalize(root).unwrap_or_else(|_| Path::new(root).to_path_buf());
    let canon = std::fs::canonicalize(abs).unwrap_or_else(|_| {
        // A not-yet-created leaf: canonicalize the parent, keep the name.
        match (abs.parent(), abs.file_name()) {
            (Some(parent), Some(name)) => std::fs::canonicalize(parent)
                .map(|p| p.join(name))
                .unwrap_or_else(|_| abs.to_path_buf()),
            _ => abs.to_path_buf(),
        }
    });
    let fold = |p: &Path| p.to_string_lossy().replace('\\', "/").to_ascii_lowercase();
    let hb_dir = fold(&canon_root.join(".k2").join("heartbeats"));
    let k2_dir = fold(&canon_root.join(".k2"));
    let root_s = fold(&canon_root);
    let target = fold(&canon);
    if target == hb_dir || target.starts_with(&format!("{hb_dir}/")) {
        return true;
    }
    ancestors && (target == k2_dir || target == root_s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_doors_are_exactly_the_ten() {
        for p in READ_PATHS.iter().chain(WRITE_PATHS.iter()) {
            assert!(is_app_door(p), "{p}");
        }
        for p in [
            "/cli/heartbeat/remove",
            "/cli/heartbeat/launch",
            "/cli/heartbeat/unarchive",
            "/cli/heartbeat/list-archived",
            "/cli/heartbeat/scheduler-status",
            "/cli/heartbeat/set-session",
            "/cli/heartbeat/set-use-workspace-session",
            "/cli/heartbeat/active-session",
            "/cli/heartbeat/list-all",
            "/cli/heartbeat/wake",
            "/cli/heartbeat-log",
        ] {
            assert!(!is_app_door(p), "{p} must not be an app door");
        }
    }

    #[test]
    fn sanitize_rewrites_the_room_root() {
        assert_eq!(
            sanitize("/tmp/room-x", "WAKEUP.md missing at /tmp/room-x/.k2/heartbeats/a/WAKEUP.md"),
            "WAKEUP.md missing at ./.k2/heartbeats/a/WAKEUP.md"
        );
        assert_eq!(sanitize("/tmp/room-x", "no path"), "no path");
    }

    #[test]
    fn app_floor_is_five_minutes_r3() {
        assert!(check_app_floor("hourly", r#"{"every_seconds":299}"#).is_err());
        assert!(check_app_floor("hourly", r#"{"every_seconds":60}"#).is_err());
        assert!(check_app_floor("hourly", r#"{"every_seconds":300}"#).is_ok());
        assert!(check_app_floor("hourly", "{}").is_ok());
        assert!(check_app_floor("daily", r#"{"time":"07:00"}"#).is_ok());
    }

    #[test]
    fn heartbeats_area_paths_r2() {
        assert!(lexically_in_heartbeats(".k2/heartbeats/a/WAKEUP.md"));
        assert!(lexically_in_heartbeats("./.k2/heartbeats"));
        assert!(lexically_in_heartbeats(".K2/Heartbeats/a/x.md"));
        assert!(!lexically_in_heartbeats(".k2/wiki/a.md"));
        assert!(!lexically_in_heartbeats("notes/.k2/heartbeats/a"));
        assert!(lexically_touches_heartbeats(".k2"));
        assert!(lexically_touches_heartbeats("."));
        assert!(!lexically_touches_heartbeats(".k2/wiki"));
        assert_eq!(
            exact_wakeup_name(".k2/heartbeats/inbox-sweep/WAKEUP.md").as_deref(),
            Some("inbox-sweep")
        );
        assert_eq!(
            exact_wakeup_name("./.k2/heartbeats/inbox-sweep/WAKEUP.md").as_deref(),
            Some("inbox-sweep")
        );
        assert_eq!(exact_wakeup_name(".k2/heartbeats/inbox-sweep/notes.md"), None);
        assert_eq!(exact_wakeup_name(".k2/heartbeats/inbox-sweep/wakeup.md"), None);
        assert_eq!(exact_wakeup_name(".k2/heartbeats/a/b/WAKEUP.md"), None);
        assert_eq!(exact_wakeup_name(".k2/heartbeats/../x/WAKEUP.md"), None);
    }

    #[test]
    fn limiter_windows_and_refund() {
        let mut l = Limiter::default();
        let now = Instant::now();
        assert_eq!(l.retry_after("k", 1, Duration::from_secs(300), now), None);
        l.record("k", now);
        let secs = l
            .retry_after("k", 1, Duration::from_secs(300), now)
            .expect("second hit inside the window is limited");
        assert_eq!(secs, 300);
        l.refund("k");
        assert_eq!(l.retry_after("k", 1, Duration::from_secs(300), now), None);
    }

    fn pass(principal: &str) -> SkinPass {
        SkinPass {
            id: format!("tok-{principal}"),
            principal_id: Some(principal.to_string()),
            username: principal.to_string(),
            caps: vec![CAP_HEARTBEATS_WRITE.to_string()],
            rooms: vec![],
            session: true,
            room_policy: Default::default(),
        }
    }

    /// R3: one fire per heartbeat per app login per 5 min; 12 per room per
    /// hour across logins; a refused lease gives the slot back.
    #[test]
    fn fire_limits_per_heartbeat_per_login_and_per_room_r3() {
        let room = format!("room-{}", uuid::Uuid::new_v4());
        let bob = pass(&format!("bob-{}", uuid::Uuid::new_v4()));
        let carl = pass(&format!("carl-{}", uuid::Uuid::new_v4()));

        let first = reserve_fire(&bob, &room, "hb-0").expect("first fire");
        let secs = reserve_fire(&bob, &room, "hb-0")
            .err()
            .expect("second fire inside 300 s is limited");
        assert!(secs > 0 && secs <= 300, "{secs}");
        // Another login may fire the same heartbeat.
        reserve_fire(&carl, &room, "hb-0").expect("other login, same heartbeat");
        // A refund (lease refused) frees bob's slot again.
        refund_fire(&first);
        reserve_fire(&bob, &room, "hb-0").expect("refunded slot");

        // Room total: 3 used so far (bob hb-0 refunded then re-used, carl hb-0).
        for i in 1..=10 {
            reserve_fire(&bob, &room, &format!("hb-{i}")).expect("under the room cap");
        }
        let secs = reserve_fire(&carl, &room, "hb-99")
            .err()
            .expect("13th fire in the room inside an hour is limited");
        assert!(secs > 0 && secs <= 3600, "{secs}");
        // Another room is unaffected.
        let other = format!("room-{}", uuid::Uuid::new_v4());
        reserve_fire(&carl, &other, "hb-99").expect("other room");
    }

    #[test]
    fn write_limit_is_sixty_per_login_per_room() {
        let room = format!("room-{}", uuid::Uuid::new_v4());
        let bob = pass(&format!("bob-{}", uuid::Uuid::new_v4()));
        for _ in 0..WRITES_PER_PASS_MAX {
            reserve_write(&bob, &room).expect("under the write cap");
        }
        let secs = reserve_write(&bob, &room).err().expect("61st write is limited");
        assert!(secs > 0 && secs <= 600, "{secs}");
    }

    #[test]
    fn fold_json_body_query_wins_and_spec_is_json_text() {
        let mut params = HashMap::new();
        params.insert("name".to_string(), "from-query".to_string());
        fold_json_body(
            &mut params,
            br#"{"name":"from-body","enabled":false,"spec":{"every_seconds":900},"x":null}"#,
        );
        assert_eq!(params.get("name").map(String::as_str), Some("from-query"));
        assert_eq!(params.get("enabled").map(String::as_str), Some("false"));
        assert_eq!(
            params.get("spec").map(String::as_str),
            Some(r#"{"every_seconds":900}"#)
        );
        assert!(!params.contains_key("x"));
    }
}
