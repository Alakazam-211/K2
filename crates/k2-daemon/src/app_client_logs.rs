//! App client logs (0.46.9, feature `app-client-logs-v1`, migration 0269).
//!
//! ```text
//! POST /cli/skin/client-logs {appVersion, os, kind?, message?, lines?, device?, at?}
//!      a signed-in person's own session pass, any door                → {id, lines, bytes, redacted, truncated}
//! GET  /cli/app/client-logs?app=[&user=][&kind=crash|log][&since=secs][&limit=N]
//!      the owner, any login, the App's own agent                      → {bundles, framed}
//! ```
//!
//! A device's crash or log bundle reaches the App's own server, never a
//! third party. The device redacts before it sends (`k2 study app-api`,
//! CLIENT LOGS); K2 redacts again ([`k2_core::app_client_logs::clean_bundle`]):
//! known token prefixes (`k2skn_`, `sk-`, JWTs…), `Bearer` values,
//! `key=value` secrets, URL user info and query strings, long random runs,
//! PEM blocks and any value K2's vault holds. Caps: a 128 KiB body, 64 KiB
//! of text kept (the newest lines win), 1,000 lines, 4,000 characters of
//! message. Limits: [`PER_PERSON_PER_HOUR`] bundles per person and
//! [`PER_APP_PER_HOUR`] per App (`429 rate_limited`, `retryAfterSecs`).
//! Readers get the bundles framed as untrusted text: a device wrote them.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use k2_core::app_client_logs::{self as store, Incoming};
use k2_core::skin::{self, SkinPass};
use serde_json::{json, Value};

use crate::cli_response::CliResponse;
use crate::compute_routes::Who;

pub const FEATURE: &str = "app-client-logs-v1";
pub const SEND: &str = "/cli/skin/client-logs";
pub const LIST: &str = "/cli/app/client-logs";
pub const PASS_ROUTES: &[&str] = &[SEND];
pub const MAX_BODY: usize = store::MAX_BODY;
/// Bundles one person may send to one App per hour.
pub const PER_PERSON_PER_HOUR: usize = 20;
/// Bundles one App may take per hour, from all its people.
pub const PER_APP_PER_HOUR: usize = 300;
const WINDOW_SECS: u64 = 3600;
/// What one framed listing carries at most.
const FRAME_CAP: usize = 256 * 1024;

/// Where a request came in.
#[derive(Debug, Clone, Default)]
pub struct Ctx {
    pub link_app: Option<String>,
}

fn refusal(status: &'static str, code: &str, hint: &str) -> CliResponse {
    CliResponse { status, content_type: "application/json", body: json!({ "ok": false, "error": { "code": code, "hint": hint } }).to_string() }
}

fn usage(hint: &str) -> CliResponse {
    refusal("400 Bad Request", "usage", hint)
}

pub fn guest_required() -> CliResponse {
    refusal("403 Forbidden", "guest_required", "client logs come from a signed-in person's own device: send their session pass (Authorization: Bearer k2skn_…)")
}

pub fn pass_required() -> CliResponse {
    refusal("401 Unauthorized", "pass_required", "send the person's session pass (Authorization: Bearer k2skn_…)")
}

pub fn too_large() -> CliResponse {
    refusal("413 Payload Too Large", "too_large", "a client log bundle is at most 128 KiB: send the newest lines")
}

fn answer(r: Result<Value, CliResponse>) -> CliResponse {
    match r {
        Ok(v) => CliResponse::ok_json(v.to_string()),
        Err(e) => e,
    }
}

// ── rate limits (memory only) ────────────────────────────────────────

fn buckets() -> &'static Mutex<HashMap<String, Vec<Instant>>> {
    static B: OnceLock<Mutex<HashMap<String, Vec<Instant>>>> = OnceLock::new();
    B.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Take one send for `person` in `app`, or the seconds until one frees.
fn take(app: &str, person: &str) -> Result<(), u64> {
    let mut b = buckets().lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    let keys = [(format!("p:{app}:{person}"), PER_PERSON_PER_HOUR), (format!("a:{app}"), PER_APP_PER_HOUR)];
    let mut wait = 0u64;
    for (k, max) in &keys {
        let v = b.entry(k.clone()).or_default();
        v.retain(|t| now.duration_since(*t).as_secs() < WINDOW_SECS);
        if v.len() >= *max {
            let oldest = v.first().copied().unwrap_or(now);
            wait = wait.max(WINDOW_SECS.saturating_sub(now.duration_since(oldest).as_secs()).max(1));
        }
    }
    if wait > 0 {
        return Err(wait);
    }
    for (k, _) in &keys {
        b.entry(k.clone()).or_default().push(now);
    }
    // Keep the map small: drop keys whose window is empty.
    if b.len() > 10_000 {
        b.retain(|_, v| v.iter().any(|t| now.duration_since(*t).as_secs() < WINDOW_SECS));
    }
    Ok(())
}

/// Tests: forget every bucket.
pub fn reset_limits_for_tests() {
    buckets().lock().unwrap_or_else(|e| e.into_inner()).clear();
}

// ── the person's route ───────────────────────────────────────────────

pub fn handle_pass(path: &str, pass: &SkinPass, is_post: bool, body: &[u8], ctx: &Ctx) -> CliResponse {
    match (path, is_post) {
        (SEND, true) => answer(send(pass, body, ctx)),
        _ => CliResponse::method_not_allowed(),
    }
}

fn opt_text(v: &Value, key: &str) -> Result<Option<String>, CliResponse> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(usage(&format!("{key} must be text"))),
    }
}

fn send(pass: &SkinPass, body: &[u8], ctx: &Ctx) -> Result<Value, CliResponse> {
    let pid = pass.principal_id.as_deref().filter(|_| pass.session).ok_or_else(guest_required)?.to_string();
    let app = pass.app.trim().to_string();
    if app.is_empty() || app == skin::HOST_APP_ID {
        return Err(refusal("403 Forbidden", "not_for_host", "client logs are for a published App's own people, never the box login"));
    }
    if ctx.link_app.as_deref().is_some_and(|l| l != app) {
        return Err(guest_required());
    }
    let v: Value = serde_json::from_slice(body).ok().filter(Value::is_object).ok_or_else(|| usage("the body must be a JSON object"))?;
    const KNOWN: &[&str] = &["kind", "appVersion", "os", "device", "message", "lines", "at"];
    if let Some(k) = v.as_object().and_then(|o| o.keys().find(|k| !KNOWN.contains(&k.as_str()))) {
        let shown: String = k.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').take(32).collect();
        return Err(usage(&format!("unknown field {shown} (known: {})", KNOWN.join(", "))));
    }
    let lines = match v.get("lines") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(a)) => {
            let mut out = Vec::with_capacity(a.len());
            for x in a {
                match x.as_str() {
                    Some(s) => out.push(s.to_string()),
                    None => return Err(usage("lines is a list of text lines")),
                }
            }
            out
        }
        Some(_) => return Err(usage("lines is a list of text lines")),
    };
    let client_at = match v.get("at") {
        None | Some(Value::Null) => None,
        Some(x) => Some(x.as_i64().ok_or_else(|| usage("at is seconds since the epoch"))?),
    };
    let incoming = Incoming {
        kind: opt_text(&v, "kind")?.unwrap_or_default(),
        app_version: opt_text(&v, "appVersion")?.unwrap_or_default(),
        os: opt_text(&v, "os")?.unwrap_or_default(),
        device: opt_text(&v, "device")?,
        message: opt_text(&v, "message")?,
        lines,
        client_at,
    };
    let clean = store::clean_bundle(incoming).map_err(|e| usage(&e))?;
    let username = skin::app_pairing::person(&pid).map_err(CliResponse::internal_error)?.ok_or_else(guest_required)?.username;
    if let Err(wait) = take(&app, &pid) {
        return Err(CliResponse {
            status: "429 Too Many Requests",
            content_type: "application/json",
            body: json!({ "ok": false, "error": {
                "code": "rate_limited",
                "hint": format!("at most {PER_PERSON_PER_HOUR} bundles per person and {PER_APP_PER_HOUR} per App each hour: keep this one and send it later"),
                "retryAfterSecs": wait,
            } })
            .to_string(),
        });
    }
    let id = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::insert(&conn, &app, &pid, &username, &clean).map_err(CliResponse::internal_error)?
    };
    Ok(json!({
        "ok": true,
        "id": id,
        "lines": clean.lines.len(),
        "bytes": clean.bytes,
        "redacted": clean.redacted,
        "truncated": clean.truncated,
    }))
}

// ── the reader's route ───────────────────────────────────────────────

fn app_of_workspace(app: &str, ws: &str) -> bool {
    let db = k2_core::db::shared();
    let conn = db.lock();
    matches!(k2_core::published_services::get_by_id(&conn, app), Ok(Some(svc)) if svc.project_id == ws)
}

fn when(secs: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs, 0).map(|t| t.format("%Y-%m-%d %H:%M:%S UTC").to_string()).unwrap_or_else(|| secs.to_string())
}

/// The bundles as one text block, oldest first inside each bundle.
fn render(bundles: &[store::Bundle]) -> String {
    let mut out = String::new();
    for b in bundles {
        out.push_str(&format!(
            "── {} {} {} app {} on {}{}{}{}\n",
            when(b.received_at),
            b.kind,
            b.username,
            b.app_version,
            b.os,
            b.device.as_deref().map(|d| format!(" ({d})")).unwrap_or_default(),
            if b.redacted > 0 { format!(", {} redacted", b.redacted) } else { String::new() },
            if b.truncated { ", cut to fit" } else { "" },
        ));
        if let Some(m) = &b.message {
            out.push_str(m);
            out.push('\n');
        }
        for l in &b.lines {
            out.push_str(l);
            out.push('\n');
        }
    }
    out
}

fn list(who: &Who, params: &HashMap<String, String>) -> Result<Value, CliResponse> {
    let raw = params.get("app").map(|s| s.trim()).filter(|s| !s.is_empty()).ok_or_else(|| usage("name the App (app)"))?;
    let app = crate::skin_routes::resolve_app_target(raw).map_err(|e| usage(&e))?;
    match who {
        Who::Owner | Who::Shell { .. } | Who::Login { .. } => {}
        Who::Agent { project_id, .. } if app_of_workspace(&app, project_id) => {}
        Who::Agent { .. } => return Err(refusal("403 Forbidden", "outside_workspace", "an agent reads client logs only for its own workspace's Apps")),
        Who::Wrong | Who::Invalid => return Err(refusal("401 Unauthorized", "unauthorized", "invalid or missing auth token")),
    }
    let kind = params.get("kind").map(|s| s.trim()).filter(|s| !s.is_empty());
    if let Some(k) = kind {
        if k != store::KIND_CRASH && k != store::KIND_LOG {
            return Err(usage("kind is crash or log"));
        }
    }
    let since = match params.get("since").map(|s| s.trim()).filter(|s| !s.is_empty()) {
        Some(s) => Some(s.parse::<i64>().map_err(|_| usage("since is seconds since the epoch"))?),
        None => None,
    };
    let limit = match params.get("limit").map(|s| s.trim()).filter(|s| !s.is_empty()) {
        Some(s) => s.parse::<usize>().map_err(|_| usage("limit is a number (1 to 500)"))?,
        None => 20,
    };
    let q = store::Query {
        username: params.get("user").map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
        kind: kind.map(str::to_string),
        since,
        limit,
    };
    let (bundles, keep) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        (store::list(&conn, &app, &q).map_err(CliResponse::internal_error)?, store::keep_days(&conn, &app).map_err(CliResponse::internal_error)?.0)
    };
    let mut oldest_first = bundles.clone();
    oldest_first.reverse();
    let framed = k2_core::deploy::framing::frame(&app, "client-logs", &render(&oldest_first), FRAME_CAP);
    Ok(json!({ "ok": true, "app": app, "keepDays": keep, "count": bundles.len(), "bundles": bundles, "framed": framed }))
}

/// `GET /cli/app/client-logs` (blocking).
pub fn handle_list(who: &Who, is_post: bool, params: &HashMap<String, String>) -> CliResponse {
    if is_post {
        return CliResponse::method_not_allowed();
    }
    answer(list(who, params))
}

/// Hourly: every App's bundles past its retention go.
pub fn sweep() {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let db = k2_core::db::shared();
    let conn = db.lock();
    match store::sweep(&conn, now) {
        Ok(n) if n > 0 => k2_core::log_debug!("[client-logs] {n} bundle(s) past retention went"),
        Ok(_) => {}
        Err(e) => k2_core::log_debug!("[client-logs] WARN sweep: {e}"),
    }
}
