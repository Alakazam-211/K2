//! Per-App quotas at the gateway link (`prd-k2-app-runtime-v1` §5.4, S19;
//! OWASP API4).
//!
//! | Quota | Default | Refusal |
//! |---|---|---|
//! | Requests/s | 50 sustained, burst 200 | `429 rate_limited` (+ `retryAfterMs`) |
//! | Sockets (HTTP + WS) | 64 | `429 too_many_sockets` |
//! | Body size | 8 MiB; fs upload verbs 100 MiB | `413 too_large` |
//! | Live subscriptions (WS) | 256 | `429 too_many_subscriptions` |
//!
//! The owner overrides any of them per App (`apps.quota_json`, 0173):
//! `k2 app quota <app> --rps N --burst N --sockets N --body 8M --upload 100M
//! --subscriptions N`. Lowering a quota narrows and applies at once for
//! anyone who may manage the App; raising one past the defaults is the
//! owner's (anyone else's raise is a held `quota` request). `--reset`
//! returns to the defaults (a raise if the override was lower).
//!
//! Not here: the per-subscription send buffer (1 MiB, drop-and-resync,
//! the activity socket's lag rule) and the `db.query` statement timeout
//! (10 s) live with their routes (lane 3's sockets and SQL lane).
//!
//! ```text
//! GET  /cli/app/quota      ?app=        the effective quotas, the override, live use
//! POST /cli/app/quota/set  {app, rps?, burst?, sockets?, bodyBytes?, uploadBytes?, subscriptions?, reset?, reason?}
//! ```

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use k2_core::app_grace::requests as reqs;
use k2_core::deploy::{self, codes};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as J};

use super::routes::{actor, app_for, db_err, json_body, need_credential, no, s, Who, R};

pub const ROUTES: &[&str] = &["/cli/app/quota", "/cli/app/quota/set"];
pub const POST_ROUTES: &[&str] = &["/cli/app/quota/set"];

const MIB: u64 = 1024 * 1024;

/// One App's quotas (§5.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Quotas {
    /// Sustained requests per second.
    pub rps: u32,
    /// Bucket size (the burst a quiet App may spend at once).
    pub burst: u32,
    /// Concurrent connections to the link (HTTP + WS).
    pub sockets: u32,
    /// Largest request body.
    pub body_bytes: u64,
    /// Largest body on the fs upload verbs.
    pub upload_bytes: u64,
    /// Live WebSocket subscriptions.
    pub subscriptions: u32,
}

pub const DEFAULTS: Quotas =
    Quotas { rps: 50, burst: 200, sockets: 64, body_bytes: 8 * MIB, upload_bytes: 100 * MIB, subscriptions: 256 };

/// The owner's override (any field absent = the default).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct Override {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rps: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub burst: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sockets: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upload_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscriptions: Option<u32>,
}

/// Hard ceilings no override passes (keeps one App from starving the
/// daemon whatever the owner types).
pub const CEILING: Quotas =
    Quotas { rps: 5_000, burst: 20_000, sockets: 1_024, body_bytes: 256 * MIB, upload_bytes: 4 * 1024 * MIB, subscriptions: 4_096 };

impl Override {
    pub fn apply(&self, base: Quotas) -> Quotas {
        Quotas {
            rps: self.rps.unwrap_or(base.rps),
            burst: self.burst.unwrap_or(base.burst),
            sockets: self.sockets.unwrap_or(base.sockets),
            body_bytes: self.body_bytes.unwrap_or(base.body_bytes),
            upload_bytes: self.upload_bytes.unwrap_or(base.upload_bytes),
            subscriptions: self.subscriptions.unwrap_or(base.subscriptions),
        }
    }

    pub fn is_empty(&self) -> bool {
        *self == Override::default()
    }

    /// `Err` names the first field out of range (1 … the ceiling).
    pub fn validate(&self) -> Result<(), String> {
        let chk = |name: &str, v: Option<u64>, max: u64| match v {
            Some(0) => Err(format!("{name} must be at least 1")),
            Some(x) if x > max => Err(format!("{name} is at most {max}")),
            _ => Ok(()),
        };
        chk("rps", self.rps.map(u64::from), CEILING.rps as u64)?;
        chk("burst", self.burst.map(u64::from), CEILING.burst as u64)?;
        chk("sockets", self.sockets.map(u64::from), CEILING.sockets as u64)?;
        chk("bodyBytes", self.body_bytes, CEILING.body_bytes)?;
        chk("uploadBytes", self.upload_bytes, CEILING.upload_bytes)?;
        chk("subscriptions", self.subscriptions.map(u64::from), CEILING.subscriptions as u64)?;
        Ok(())
    }
}

/// The fields of `next` that are higher than in `cur` (a widening).
pub fn raises(cur: &Quotas, next: &Quotas) -> Vec<String> {
    let mut out = Vec::new();
    let mut f = |name: &str, a: u64, b: u64| {
        if b > a {
            out.push(format!("{name} {a} → {b}"));
        }
    };
    f("rps", cur.rps.into(), next.rps.into());
    f("burst", cur.burst.into(), next.burst.into());
    f("sockets", cur.sockets.into(), next.sockets.into());
    f("bodyBytes", cur.body_bytes, next.body_bytes);
    f("uploadBytes", cur.upload_bytes, next.upload_bytes);
    f("subscriptions", cur.subscriptions.into(), next.subscriptions.into());
    out
}

/// The App's stored override.
pub fn load_override(conn: &Connection, app_id: &str) -> rusqlite::Result<Override> {
    let raw: Option<Option<String>> = conn.query_row("SELECT quota_json FROM apps WHERE app_id = ?1", params![app_id], |r| r.get(0)).optional()?;
    Ok(raw.flatten().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default())
}

pub fn store_override(conn: &Connection, app_id: &str, o: &Override) -> rusqlite::Result<()> {
    let v = if o.is_empty() { None } else { Some(serde_json::to_string(o).unwrap_or_default()) };
    conn.execute("UPDATE apps SET quota_json = ?2, updated_at = ?3 WHERE app_id = ?1", params![app_id, v, deploy::now()])?;
    invalidate(app_id);
    Ok(())
}

// ── live state (per App) ─────────────────────────────────────────────

pub(crate) struct Bucket {
    tokens: f64,
    at: Instant,
}

#[derive(Default)]
struct Live {
    bucket: Option<Bucket>,
    subscriptions: u32,
    /// Cached effective quotas (dropped on any override change).
    quotas: Option<Quotas>,
}

fn live() -> &'static Mutex<HashMap<String, Live>> {
    static L: OnceLock<Mutex<HashMap<String, Live>>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Forget the cached quotas of an App (an override changed).
pub fn invalidate(app_id: &str) {
    if let Some(l) = live().lock().unwrap_or_else(|e| e.into_inner()).get_mut(app_id) {
        l.quotas = None;
    }
}

/// The effective quotas of an App (cached).
pub fn quotas_for(app_id: &str) -> Quotas {
    if let Some(q) = live().lock().unwrap_or_else(|e| e.into_inner()).get(app_id).and_then(|l| l.quotas) {
        return q;
    }
    let o = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        load_override(&conn, app_id).unwrap_or_default()
    };
    let q = o.apply(DEFAULTS);
    live().lock().unwrap_or_else(|e| e.into_inner()).entry(app_id.to_string()).or_default().quotas = Some(q);
    q
}

/// Token bucket: take one request. `Err(ms)` = wait this long.
pub(crate) fn take_at(bucket: &mut Option<Bucket>, q: &Quotas, now: Instant) -> Result<(), u64> {
    let rate = q.rps.max(1) as f64;
    let cap = q.burst.max(1) as f64;
    let b = bucket.get_or_insert(Bucket { tokens: cap, at: now });
    let dt = now.saturating_duration_since(b.at).as_secs_f64();
    b.tokens = (b.tokens + dt * rate).min(cap);
    b.at = now;
    if b.tokens >= 1.0 {
        b.tokens -= 1.0;
        Ok(())
    } else {
        Err((((1.0 - b.tokens) / rate) * 1000.0).ceil().max(1.0) as u64)
    }
}

/// One request on `app_id`'s link: `Err(retry_after_ms)` when over rate.
pub fn take(app_id: &str) -> Result<(), (u64, Quotas)> {
    let q = quotas_for(app_id);
    let mut map = live().lock().unwrap_or_else(|e| e.into_inner());
    let l = map.entry(app_id.to_string()).or_default();
    take_at(&mut l.bucket, &q, Instant::now()).map_err(|ms| (ms, q))
}

/// A WebSocket subscription opens: `false` = over the cap.
pub fn subscribe(app_id: &str) -> bool {
    let q = quotas_for(app_id);
    let mut map = live().lock().unwrap_or_else(|e| e.into_inner());
    let l = map.entry(app_id.to_string()).or_default();
    if l.subscriptions >= q.subscriptions {
        return false;
    }
    l.subscriptions += 1;
    true
}

/// A subscription closed.
pub fn unsubscribe(app_id: &str) {
    if let Some(l) = live().lock().unwrap_or_else(|e| e.into_inner()).get_mut(app_id) {
        l.subscriptions = l.subscriptions.saturating_sub(1);
    }
}

fn live_subscriptions(app_id: &str) -> u32 {
    live().lock().unwrap_or_else(|e| e.into_inner()).get(app_id).map(|l| l.subscriptions).unwrap_or(0)
}

/// Is `path` one of the fs upload verbs (the bigger body limit)?
pub fn is_upload(path: &str) -> bool {
    matches!(path, "/cli/fs/write-file" | "/cli/fs/upload-binary" | "/cli/fs/upload-chunk")
}

/// The body limit for a route.
pub fn body_limit(q: &Quotas, path: &str) -> u64 {
    if is_upload(path) {
        q.upload_bytes
    } else {
        q.body_bytes
    }
}

// ── route ────────────────────────────────────────────────────────────

pub(crate) fn handle(path: &str, who: &Who, body: &[u8], params: &HashMap<String, String>) -> R<J> {
    match path {
        "/cli/app/quota" => show(who, params),
        "/cli/app/quota/set" => set(who, body),
        _ => Err(no("not_found", 404, "no such App route")),
    }
}

fn show(who: &Who, params: &HashMap<String, String>) -> R<J> {
    need_credential(who)?;
    let app = app_for(who, params.get("app").map(String::as_str))?;
    let o = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        load_override(&conn, &app.app_id).map_err(db_err)?
    };
    Ok(json!({
        "ok": true,
        "app": app.app_id,
        "quotas": o.apply(DEFAULTS),
        "defaults": DEFAULTS,
        "override": o,
        "live": {
            "connections": super::gateway::connections(&app.app_id),
            "subscriptions": live_subscriptions(&app.app_id),
        },
    }))
}

fn u(v: &J, k: &str) -> R<Option<u64>> {
    match v.get(k) {
        None | Some(J::Null) => Ok(None),
        Some(J::Number(n)) => n.as_u64().map(Some).ok_or_else(|| no(codes::BAD_REQUEST, 400, format!("{k} is a whole number"))),
        Some(J::String(s)) => parse_size(s).map(Some).ok_or_else(|| no(codes::BAD_REQUEST, 400, format!("{k}: '{s}' isn't a number or a size (512K, 8M, 1G)"))),
        _ => Err(no(codes::BAD_REQUEST, 400, format!("{k} is a number"))),
    }
}

/// `4096`, `512K`, `8M`, `1G` → bytes.
pub fn parse_size(raw: &str) -> Option<u64> {
    let t = raw.trim();
    let (num, mult) = match t.chars().last()? {
        'K' | 'k' => (&t[..t.len() - 1], 1u64 << 10),
        'M' | 'm' => (&t[..t.len() - 1], 1u64 << 20),
        'G' | 'g' => (&t[..t.len() - 1], 1u64 << 30),
        _ => (t, 1),
    };
    num.trim().parse::<u64>().ok()?.checked_mul(mult)
}

fn u32f(v: &J, k: &str) -> R<Option<u32>> {
    u(v, k)?.map(|x| u32::try_from(x).map_err(|_| no(codes::BAD_REQUEST, 400, format!("{k} is too large")))).transpose()
}

/// POST `/cli/app/quota/set`: narrowing applies now (anyone who manages the
/// App); a raise is the owner's, anyone else's is held (`quota`).
fn set(who: &Who, body: &[u8]) -> R<J> {
    super::routes::need_manage(who)?;
    let v = json_body(body)?;
    let app = app_for(who, s(&v, "app"))?;
    let cur_o = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        load_override(&conn, &app.app_id).map_err(db_err)?
    };
    let next_o = if v.get("reset").and_then(J::as_bool) == Some(true) {
        Override::default()
    } else {
        let mut o = cur_o.clone();
        if let Some(x) = u32f(&v, "rps")? {
            o.rps = Some(x);
        }
        if let Some(x) = u32f(&v, "burst")? {
            o.burst = Some(x);
        }
        if let Some(x) = u32f(&v, "sockets")? {
            o.sockets = Some(x);
        }
        if let Some(x) = u(&v, "bodyBytes")? {
            o.body_bytes = Some(x);
        }
        if let Some(x) = u(&v, "uploadBytes")? {
            o.upload_bytes = Some(x);
        }
        if let Some(x) = u32f(&v, "subscriptions")? {
            o.subscriptions = Some(x);
        }
        o
    };
    next_o.validate().map_err(|e| no(codes::BAD_REQUEST, 400, e))?;
    let (cur, next) = (cur_o.apply(DEFAULTS), next_o.apply(DEFAULTS));
    let up = raises(&cur, &next);
    let owner = matches!(who, Who::Owner | Who::Login { role: k2_core::connect_users::Role::Owner, .. } | Who::Shell { .. });
    if !up.is_empty() && !owner {
        let summary = format!("raise {}'s quotas: {}", app.app_id, up.join(", "));
        let r = super::approvals::file(&super::approvals::Filing {
            who,
            kind: reqs::KIND_QUOTA,
            app: Some(&app),
            label: &app.app_id,
            workspace: &app.owner_workspace,
            body: json!({ "override": next_o }),
            diff: json!({ "quotas": { "from": cur, "to": next }, "lines": up }),
            summary,
            reasons: vec!["raising an App's quotas is the owner's".into()],
            reason: s(&v, "reason"),
        })?;
        return Ok(super::approvals::held_answer(&r));
    }
    apply(&app.app_id, &next_o, &actor(who))?;
    Ok(json!({ "ok": true, "app": app.app_id, "quotas": next, "override": next_o, "raised": up }))
}

/// Store an override (a direct set, or an approved `quota` request).
pub(crate) fn apply(app_id: &str, o: &Override, by: &str) -> R<()> {
    o.validate().map_err(|e| no(codes::BAD_REQUEST, 400, e))?;
    let h = k2_core::db::shared();
    let conn = h.lock();
    store_override(&conn, app_id, o).map_err(db_err)?;
    let _ = deploy::audit::append(&conn, deploy::audit::SUBJECT_APP, app_id, "quota", by, None, &json!({ "override": o }));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn defaults_and_overrides() {
        assert_eq!(DEFAULTS.rps, 50);
        assert_eq!(DEFAULTS.burst, 200);
        assert_eq!(DEFAULTS.sockets, 64);
        assert_eq!(DEFAULTS.body_bytes, 8 * MIB);
        assert_eq!(DEFAULTS.upload_bytes, 100 * MIB);
        assert_eq!(DEFAULTS.subscriptions, 256);
        let o: Override = serde_json::from_str(r#"{"rps":10,"bodyBytes":1048576}"#).unwrap();
        let q = o.apply(DEFAULTS);
        assert_eq!((q.rps, q.burst, q.body_bytes), (10, 200, MIB));
        assert!(serde_json::from_str::<Override>(r#"{"rps":10,"nope":1}"#).is_err(), "unknown fields refused");
        assert!(Override { rps: Some(0), ..Default::default() }.validate().is_err());
        assert!(Override { sockets: Some(CEILING.sockets + 1), ..Default::default() }.validate().is_err());
        assert!(Override { sockets: Some(CEILING.sockets), ..Default::default() }.validate().is_ok());
        assert_eq!(raises(&DEFAULTS, &q), Vec::<String>::new(), "lowering is no raise");
        let up = Override { rps: Some(100), subscriptions: Some(10), ..Default::default() }.apply(DEFAULTS);
        assert_eq!(raises(&DEFAULTS, &up), vec!["rps 50 → 100".to_string()]);
        assert_eq!(body_limit(&DEFAULTS, "/cli/fs/upload-chunk"), 100 * MIB);
        assert_eq!(parse_size("1K"), Some(1024));
        assert_eq!(parse_size("8M"), Some(8 * MIB));
        assert_eq!(parse_size("2g"), Some(2 << 30));
        assert_eq!(parse_size("4096"), Some(4096));
        assert_eq!(parse_size("lots"), None);
        assert_eq!(parse_size(""), None);
        assert_eq!(body_limit(&DEFAULTS, "/cli/thread"), 8 * MIB);
    }

    #[test]
    fn the_bucket_allows_the_burst_then_the_sustained_rate() {
        let q = Quotas { rps: 50, burst: 200, ..DEFAULTS };
        let t0 = Instant::now();
        let mut b = None;
        for i in 0..200 {
            assert!(take_at(&mut b, &q, t0).is_ok(), "burst request {i}");
        }
        let wait = take_at(&mut b, &q, t0).unwrap_err();
        assert!((1..=20).contains(&wait), "retry after ~20 ms at 50/s, got {wait}");
        // One second later: 50 more, then refused again.
        let t1 = t0 + Duration::from_secs(1);
        for i in 0..50 {
            assert!(take_at(&mut b, &q, t1).is_ok(), "sustained request {i}");
        }
        assert!(take_at(&mut b, &q, t1).is_err());
        // A long quiet spell refills only to the burst.
        let t2 = t1 + Duration::from_secs(3600);
        for _ in 0..200 {
            take_at(&mut b, &q, t2).unwrap();
        }
        assert!(take_at(&mut b, &q, t2).is_err());
    }
}
