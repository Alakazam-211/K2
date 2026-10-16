//! The per-call App audit at the gateway (`prd-k2-app-runtime-v1` §5.5,
//! S19) and its alerts.
//!
//! **Recording.** Every request on an App's link becomes one
//! [`k2_core::app_audit::Call`]: the gate opens it ([`begin`]) with the
//! verb, actor, room and bytes in; the response helpers fill the result
//! and bytes out ([`note_response`], called by `routes::http`'s senders);
//! it is closed when the next request on the connection arrives or the
//! connection ends ([`finish`]). Rows go to `~/.k2/apps/app-audit.db`
//! through one writer thread in batches, so the request path never waits
//! on SQLite.
//!
//! **Alerts** (to the App's owning workspace agent, a `k2 msg` wake):
//! - a denial spike: more than 20 refused calls in 5 minutes;
//! - the first successful use of each write verb;
//! - any `pass_wrong_link`;
//! - a quota hit (`rate_limited`, `too_many_sockets`, `too_large`,
//!   `too_many_subscriptions`).
//! Each kind is said at most once per App per 5 minutes (first use: once
//! per verb, ever).
//!
//! ```text
//! GET /cli/app/audit ?app=&since=1h|<epoch-ms>&verb=&denied=1&limit=&verify=1
//! ```

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use k2_core::app_audit::{self as store, Call};
use k2_core::deploy::codes;
use serde_json::{json, Value as J};

use super::gateway::LinkCtx;
use super::routes::{app_for, need_credential, no, Who, R};

pub const ROUTES: &[&str] = &["/cli/app/audit"];

/// A denial spike: more than this many refusals …
pub const SPIKE_DENIALS: usize = 20;
/// … within this window.
pub const SPIKE_WINDOW: Duration = Duration::from_secs(300);
/// Alerts of one kind for one App at most this often.
pub const ALERT_EVERY: Duration = Duration::from_secs(300);

// ── the per-connection call ──────────────────────────────────────────

/// The call a link connection is serving.
struct Open {
    call: Call,
    started: Instant,
    ended: Option<Instant>,
    workspace_id: String,
    /// A WebSocket that took a subscription slot.
    subscribed: bool,
}

tokio::task_local! {
    static CONN: RefCell<Option<Open>>;
}

/// Run `fut` (one link connection) with its call slot; the last call is
/// closed when the connection ends.
pub async fn scope<F: std::future::Future<Output = ()>>(fut: F) {
    CONN.scope(RefCell::new(None), async move {
        fut.await;
        finish();
    })
    .await;
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// What the gate knows when a request arrives.
pub struct Begin<'a> {
    pub method: &'a str,
    pub path: &'a str,
    pub actor: &'a str,
    pub room: Option<String>,
    pub bytes_in: u64,
    /// Set when the gate refuses it.
    pub refused: Option<(u16, &'a str)>,
    pub subscribed: bool,
}

/// Open the call for a request on `ctx`'s link (closing the previous one
/// on this connection).
pub fn begin(ctx: &LinkCtx, b: Begin<'_>) {
    finish();
    let call = Call {
        at_ms: now_ms(),
        app_id: ctx.app_id.clone(),
        seed: live_seed(&ctx.app_id),
        actor: b.actor.to_string(),
        on_behalf_of: None,
        verb: format!("{} {}", b.method, b.path),
        room: b.room,
        result: b.refused.map(|r| r.0).unwrap_or(0),
        code: b.refused.map(|r| r.1.to_string()),
        bytes_in: b.bytes_in,
        bytes_out: 0,
        latency_ms: 0,
    };
    let open = Open { call, started: Instant::now(), ended: None, workspace_id: ctx.workspace_id.clone(), subscribed: b.subscribed };
    if CONN.try_with(|c| *c.borrow_mut() = Some(open)).is_err() {
        // Off a scoped connection (a unit test): record at once.
        k2_core::log_debug!("[apps/audit] call outside a link connection scope");
    }
}

/// The response for the open call (status line like `200 OK`).
pub fn note_response(status: &str, bytes_out: usize) {
    let code: u16 = status.split_whitespace().next().and_then(|c| c.parse().ok()).unwrap_or(0);
    let _ = CONN.try_with(|c| {
        if let Some(o) = c.borrow_mut().as_mut() {
            if o.ended.is_none() {
                if o.call.result == 0 || o.call.code.is_none() {
                    o.call.result = code;
                }
                o.call.bytes_out = o.call.bytes_out.saturating_add(bytes_out as u64);
                o.ended = Some(Instant::now());
            }
        }
    });
}

/// Is there an open call on this task (a link connection)?
pub fn in_call() -> bool {
    CONN.try_with(|c| c.borrow().is_some()).unwrap_or(false)
}

/// Close the open call (if any) and queue its row.
pub fn finish() {
    let open = CONN.try_with(|c| c.borrow_mut().take()).ok().flatten();
    let Some(mut o) = open else { return };
    let end = o.ended.unwrap_or_else(Instant::now);
    o.call.latency_ms = end.saturating_duration_since(o.started).as_millis() as u64;
    if o.subscribed {
        super::quotas::unsubscribe(&o.call.app_id);
        if o.call.result == 0 {
            o.call.result = 101;
        }
    }
    observe(&o.workspace_id, &o.call);
    record(o.call);
}

fn live_seed(app_id: &str) -> Option<String> {
    static CACHE: OnceLock<Mutex<HashMap<String, (Instant, Option<String>)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some((at, s)) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(app_id) {
        if at.elapsed() < Duration::from_secs(5) {
            return s.clone();
        }
    }
    let s = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        k2_core::deploy::store::get_app(&conn, app_id).ok().flatten().and_then(|a| a.live_seed)
    };
    cache.lock().unwrap_or_else(|e| e.into_inner()).insert(app_id.to_string(), (Instant::now(), s.clone()));
    s
}

// ── the writer ───────────────────────────────────────────────────────

enum Msg {
    Call(Call),
    Flush(SyncSender<()>),
}

fn sender() -> &'static Mutex<Sender<Msg>> {
    static TX: OnceLock<Mutex<Sender<Msg>>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        let _ = std::thread::Builder::new().name("app-audit".into()).spawn(move || writer(rx));
        Mutex::new(tx)
    })
}

/// Queue one row (never blocks on SQLite).
pub fn record(call: Call) {
    let _ = sender().lock().unwrap_or_else(|e| e.into_inner()).send(Msg::Call(call));
}

/// Wait until every queued row is written (tests, `k2 app audit`).
pub fn flush() {
    let (tx, rx) = mpsc::sync_channel(1);
    if sender().lock().unwrap_or_else(|e| e.into_inner()).send(Msg::Flush(tx)).is_ok() {
        let _ = rx.recv_timeout(Duration::from_secs(10));
    }
}

fn writer(rx: Receiver<Msg>) {
    let mut conn: Option<(std::path::PathBuf, rusqlite::Connection)> = None;
    let mut last_prune = Instant::now() - Duration::from_secs(86_400);
    while let Ok(first) = rx.recv() {
        let mut batch = Vec::new();
        let mut flushes = Vec::new();
        let mut next = Some(first);
        while let Some(m) = next.take() {
            match m {
                Msg::Call(c) => batch.push(c),
                Msg::Flush(t) => flushes.push(t),
            }
            if batch.len() < 500 {
                next = rx.try_recv().ok();
            }
        }
        // The home can change under tests (TempHome): reopen on a new path.
        let path = store::db_path();
        if conn.as_ref().is_none_or(|(p, _)| *p != path) {
            conn = match store::open(&path) {
                Ok(c) => Some((path.clone(), c)),
                Err(e) => {
                    k2_core::log_debug!("[apps/audit] open {}: {e}", path.display());
                    None
                }
            };
        }
        if let Some((_, c)) = conn.as_ref() {
            first_uses(c, &batch);
            if let Err(e) = store::append(c, &batch) {
                k2_core::log_debug!("[apps/audit] append {} rows: {e}", batch.len());
            }
            if last_prune.elapsed() > Duration::from_secs(3600) {
                last_prune = Instant::now();
                if let Err(e) = store::prune(c, now_ms()) {
                    k2_core::log_debug!("[apps/audit] prune: {e}");
                }
            }
        }
        for f in flushes {
            let _ = f.send(());
        }
    }
}

// ── alerts ───────────────────────────────────────────────────────────

#[derive(Default)]
struct AlertState {
    denials: HashMap<String, Vec<Instant>>,
    said: HashMap<(String, String), Instant>,
    /// App → workspace (for first-use alerts raised by the writer).
    ws: HashMap<String, String>,
    /// (App, verb) write verbs already used.
    used: HashSet<(String, String)>,
}

fn alerts() -> &'static Mutex<AlertState> {
    static A: OnceLock<Mutex<AlertState>> = OnceLock::new();
    A.get_or_init(|| Mutex::new(AlertState::default()))
}

/// Say `text` to the App's workspace unless `kind` was said for this App
/// in the last [`ALERT_EVERY`].
fn alert_once(app_id: &str, ws: &str, kind: &str, text: &str) {
    {
        let mut a = alerts().lock().unwrap_or_else(|e| e.into_inner());
        let key = (app_id.to_string(), kind.to_string());
        if a.said.get(&key).is_some_and(|t| t.elapsed() < ALERT_EVERY) {
            return;
        }
        a.said.insert(key, Instant::now());
    }
    super::alerts::notify(ws, text);
}

const QUOTA_CODES: &[&str] = &["rate_limited", "too_many_sockets", "too_large", "too_many_subscriptions"];

/// Watch each finished call for alert conditions.
fn observe(ws: &str, call: &Call) {
    let app = call.app_id.as_str();
    alerts().lock().unwrap_or_else(|e| e.into_inner()).ws.insert(app.to_string(), ws.to_string());
    if let Some(code) = call.code.as_deref() {
        if code == "pass_wrong_link" {
            alert_once(
                app,
                ws,
                "pass_wrong_link",
                &format!("[k2 apps] {app}: a pass for another link or App was presented on its gateway link ({}). k2 app audit {app} --denied", call.verb),
            );
        }
        if QUOTA_CODES.contains(&code) {
            alert_once(
                app,
                ws,
                &format!("quota:{code}"),
                &format!("[k2 apps] {app} hit a quota ({code}) on {}. k2 app quota {app} shows the limits; k2 app audit {app} --denied", call.verb),
            );
        }
    }
    if call.result >= 400 {
        let spike = {
            let mut a = alerts().lock().unwrap_or_else(|e| e.into_inner());
            let v = a.denials.entry(app.to_string()).or_default();
            v.retain(|t| t.elapsed() < SPIKE_WINDOW);
            v.push(Instant::now());
            v.len() > SPIKE_DENIALS
        };
        if spike {
            alert_once(
                app,
                ws,
                "denial_spike",
                &format!("[k2 apps] {app}: more than {SPIKE_DENIALS} refused calls in 5 minutes. k2 app audit {app} --denied --since 5m"),
            );
        }
    }
}

/// First successful use of each write verb (checked by the writer against
/// the stored history, so a daemon restart doesn't repeat it).
fn first_uses(conn: &rusqlite::Connection, batch: &[Call]) {
    for c in batch {
        if !c.verb.starts_with("POST ") || c.result == 0 || c.result >= 400 {
            continue;
        }
        let key = (c.app_id.clone(), c.verb.clone());
        let ws = {
            let a = alerts().lock().unwrap_or_else(|e| e.into_inner());
            if a.used.contains(&key) {
                continue;
            }
            a.ws.get(&c.app_id).cloned()
        };
        let seen: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM calls WHERE app_id = ?1 AND verb = ?2 AND result > 0 AND result < 400) \
                 OR EXISTS(SELECT 1 FROM daily WHERE app_id = ?1 AND verb = ?2 AND calls > denied)",
                rusqlite::params![c.app_id, c.verb],
                |r| r.get(0),
            )
            .unwrap_or(true);
        alerts().lock().unwrap_or_else(|e| e.into_inner()).used.insert(key);
        if !seen {
            if let Some(ws) = ws {
                super::alerts::notify(
                    &ws,
                    &format!("[k2 apps] {} used a write verb for the first time: {}. k2 app audit {} --verb '{}'", c.app_id, c.verb, c.app_id, c.verb),
                );
            }
        }
    }
}

// ── route ────────────────────────────────────────────────────────────

/// `1h`, `30m`, `2d`, `45s`, or epoch milliseconds.
pub fn parse_since(raw: &str, now_ms: i64) -> Option<i64> {
    let t = raw.trim();
    if t.is_empty() || !t.is_ascii() {
        return None;
    }
    if let Ok(n) = t.parse::<i64>() {
        return Some(n);
    }
    let (num, unit) = t.split_at(t.len() - 1);
    let n: i64 = num.parse().ok()?;
    let ms = match unit {
        "s" => 1_000,
        "m" => 60_000,
        "h" => 3_600_000,
        "d" => 86_400_000,
        _ => return None,
    };
    Some(now_ms - n * ms)
}

pub(crate) fn handle(who: &Who, params: &HashMap<String, String>) -> R<J> {
    need_credential(who)?;
    let app = app_for(who, params.get("app").map(String::as_str))?;
    let since_ms = match params.get("since").map(String::as_str).filter(|s| !s.trim().is_empty()) {
        None => None,
        Some(s) => Some(parse_since(s, now_ms()).ok_or_else(|| no(codes::BAD_REQUEST, 400, format!("since is 30m, 1h, 2d or epoch ms, not '{s}'")))?),
    };
    let limit = params.get("limit").and_then(|l| l.parse::<i64>().ok()).unwrap_or(100);
    let denied = params.get("denied").is_some_and(|v| matches!(v.as_str(), "1" | "true" | "yes"));
    let verify = params.get("verify").is_some_and(|v| matches!(v.as_str(), "1" | "true" | "yes"));
    flush();
    let conn = store::open(&store::db_path()).map_err(|e| no("audit_unavailable", 503, format!("app-audit.db: {e}")))?;
    let q = store::Query { app_id: &app.app_id, since_ms, verb: params.get("verb").map(String::as_str).filter(|v| !v.is_empty()), denied_only: denied, limit };
    let rows = store::list(&conn, &q).map_err(|e| no("audit_unavailable", 503, e.to_string()))?;
    let (calls, refused, bin, bout) = store::totals(&conn, &app.app_id, since_ms.unwrap_or(0)).map_err(|e| no("audit_unavailable", 503, e.to_string()))?;
    let mut out = json!({
        "ok": true,
        "app": app.app_id,
        "calls": rows,
        "totals": { "calls": calls, "denied": refused, "bytesIn": bin, "bytesOut": bout },
        "summaries": store::summaries(&conn, &app.app_id, 60).unwrap_or_default(),
        "file": store::db_path(),
    });
    if verify {
        let mut days = Vec::new();
        for d in store::days(&conn).map_err(|e| no("audit_unavailable", 503, e.to_string()))? {
            let v = store::verify_day(&conn, &d).map_err(|e| no("audit_unavailable", 503, e.to_string()))?;
            days.push(match v {
                Ok(n) => json!({ "day": d, "ok": true, "rows": n }),
                Err(seq) => json!({ "day": d, "ok": false, "brokenAt": seq }),
            });
        }
        out["chain"] = json!(days);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn since_parses_relative_and_absolute() {
        assert_eq!(parse_since("1h", 10_000_000), Some(10_000_000 - 3_600_000));
        assert_eq!(parse_since("30m", 10_000_000), Some(10_000_000 - 1_800_000));
        assert_eq!(parse_since("2d", 1_000_000_000), Some(1_000_000_000 - 172_800_000));
        assert_eq!(parse_since("1760000000000", 0), Some(1_760_000_000_000));
        assert_eq!(parse_since("soon", 0), None);
        assert_eq!(parse_since("", 0), None);
    }
}
