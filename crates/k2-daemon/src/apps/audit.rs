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
//! - the first successful use of each write verb, gathered into one wake
//!   per App per [`DIGEST_WINDOW`] (0.46.4, NSI C: a test run's login,
//!   logout and upload were three wakes), and none at all for what one of
//!   the App's own test guests did (someone testing the App);
//! - any `pass_wrong_link`;
//! - a quota hit (`rate_limited`, `too_many_sockets`, `too_large`,
//!   `too_many_subscriptions`).
//! Each kind is said at most once per App per 5 minutes (first use: once
//! per verb, ever).
//!
//! **Refusals are visible** (0.46.4, NSI A): every refused call's row
//! carries K2's error code, whether the gate refused it or the route's
//! handler did (a login's `401` included), and the App's own log (the live
//! deployment's, `k2 app logs`) gets one line per refused call:
//! `[k2] refused <METHOD /path>: <code> (<hint>)`. K2's words only, a pass
//! or key never (redacted), at most [`REFUSED_LINES_PER_MIN`] lines per App
//! a minute (then one line saying how many were left out).
//!
//! ```text
//! GET /cli/app/audit ?app=&since=1h|<epoch-ms>&verb=&denied=1&limit=&verify=1&oldest=1
//! ```
//!
//! `calls` are the newest `limit` rows, newest first; `oldest=1` gives the
//! same rows oldest first (a log to read top to bottom). `order` says which.
//! ```text
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
    /// K2's hint on a refusal (for the App's log line, never stored).
    hint: Option<String>,
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
    let open = Open { call, hint: None, started: Instant::now(), ended: None, workspace_id: ctx.workspace_id.clone(), subscribed: b.subscribed };
    if CONN.try_with(|c| *c.borrow_mut() = Some(open)).is_err() {
        // Off a scoped connection (a unit test): record at once.
        k2_core::log_debug!("[apps/audit] call outside a link connection scope");
    }
}

/// The response for the open call (status line like `200 OK`, and the
/// body sent). A refusal (4xx/5xx) records K2's error code from the body
/// when the gate didn't already (a handler's own refusal, NSI A).
pub fn note_response(status: &str, body: &str) {
    let code: u16 = status.split_whitespace().next().and_then(|c| c.parse().ok()).unwrap_or(0);
    let _ = CONN.try_with(|c| {
        if let Some(o) = c.borrow_mut().as_mut() {
            if o.ended.is_none() {
                if o.call.result == 0 || o.call.code.is_none() {
                    o.call.result = code;
                }
                // A guest login names who signed in (on behalf of): the
                // digest knows the App's own test guests by it (NSI C).
                if code == 200 && o.call.verb == "POST /cli/skin/login" && o.call.on_behalf_of.is_none() {
                    if let Some(p) = serde_json::from_str::<J>(body).ok().and_then(|v| v["principalId"].as_str().map(str::to_string)) {
                        o.call.on_behalf_of = Some(format!("guest:{p}"));
                    }
                }
                if code >= 400 {
                    let (k2_code, hint) = refusal_of(body);
                    if o.call.code.is_none() {
                        o.call.code = Some(k2_code.unwrap_or_else(|| format!("http_{code}")));
                    }
                    if o.hint.is_none() {
                        o.hint = hint;
                    }
                }
                o.call.bytes_out = o.call.bytes_out.saturating_add(body.len() as u64);
                o.ended = Some(Instant::now());
            }
        }
    });
}

/// K2's error code and hint from a refusal body: `{error:{code,hint}}`,
/// `{error:"code"}`, `{code, hint|message}` (the shapes K2's routes use).
/// A code is a short `snake_case` word; anything else is a hint.
pub fn refusal_of(body: &str) -> (Option<String>, Option<String>) {
    let Ok(v) = serde_json::from_str::<J>(body) else { return (None, None) };
    let is_code = |s: &str| !s.is_empty() && s.len() <= 64 && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
    let text = |x: &J| x.as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let (code, hint) = match &v["error"] {
        J::Object(_) => (text(&v["error"]["code"]), text(&v["error"]["hint"]).or_else(|| text(&v["error"]["message"]))),
        J::String(e) if is_code(e) => (Some(e.clone()), text(&v["hint"]).or_else(|| text(&v["message"]))),
        J::String(e) => (text(&v["code"]), Some(e.clone())),
        _ => (text(&v["code"]), text(&v["hint"]).or_else(|| text(&v["message"]))),
    };
    (code.filter(|c| is_code(c)), hint)
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
    if o.call.result >= 400 {
        let line = Refused { app_id: o.call.app_id.clone(), verb: o.call.verb.clone(), code: o.call.code.clone().unwrap_or_default(), hint: o.hint.take() };
        let _ = sender().lock().unwrap_or_else(|e| e.into_inner()).send(Msg::Refused(line));
    }
    record(o.call);
}

// ── the App's log: one line per refused call (NSI A) ─────────────────

/// Lines a minute one App's log gets about refused calls.
pub const REFUSED_LINES_PER_MIN: usize = 20;
/// The hint in a log line is cut to this many characters.
const HINT_MAX: usize = 200;
/// The verb (method and path) in a log line is cut to this many characters.
const VERB_MAX: usize = 120;

struct Refused {
    app_id: String,
    verb: String,
    code: String,
    hint: Option<String>,
}

/// K2's words for the App's log: one line, no control characters, no
/// pass or key, cut to [`HINT_MAX`]. A word is redacted when it, or any
/// part of it after `=`, `:` or `?` (`token=…`), is shaped like a K2 pass or
/// key (`k2skn_…`, `k2sk_…`, `k2skr_…`, `k2hs_…`), a JWT (`eyJ…`), or a
/// long opaque token: 32+ characters of letters, digits, `_`, `-` or `.`
/// (security review, 0.46.4: JWT-shaped tokens carry `.` and `-`).
pub fn log_safe(raw: &str) -> String {
    log_safe_cut(raw, HINT_MAX)
}

fn secret_shaped(part: &str) -> bool {
    let bare = part.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_');
    ["k2skn_", "k2sk_", "k2skr_", "k2hs_"].iter().any(|p| bare.starts_with(p))
        || (bare.starts_with("eyJ") && bare.len() >= 16)
        || (bare.len() >= 32 && bare.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')))
}

/// [`log_safe`], cut to `max` characters.
fn log_safe_cut(raw: &str, max: usize) -> String {
    let flat: String = raw.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let mut out: Vec<String> = Vec::new();
    for w in flat.split_whitespace() {
        let secret = secret_shaped(w) || w.split(['=', ':', '?', '&']).any(secret_shaped);
        out.push(if secret { "[redacted]".to_string() } else { w.to_string() });
    }
    let mut s = out.join(" ");
    if s.chars().count() > max {
        s = s.chars().take(max).collect::<String>() + "…";
    }
    s
}

/// The line itself (pure: tests and the writer).
pub fn refused_line(verb: &str, code: &str, hint: Option<&str>) -> String {
    let code = if code.is_empty() { "refused" } else { code };
    // The verb is the caller's path: the same cleaning as the hint.
    let verb = log_safe_cut(verb, VERB_MAX);
    match hint.map(log_safe).filter(|h| !h.is_empty()) {
        Some(h) => format!("[k2] refused {verb}: {code} ({h})"),
        None => format!("[k2] refused {verb}: {code}"),
    }
}

/// Per App: (minute started, lines written, lines left out).
type RefusedBudget = HashMap<String, (Instant, usize, usize)>;

/// The App's current log file: its live deployment's, else its newest.
fn app_log_path(app_id: &str) -> Option<std::path::PathBuf> {
    let h = k2_core::db::shared();
    let conn = h.lock();
    let app = k2_core::deploy::store::get_app(&conn, app_id).ok()??;
    let dep = match app.live_deployment {
        Some(d) => d,
        None => k2_core::deploy::store::list_deployments(&conn, app_id, 1).ok()?.into_iter().next()?.id,
    };
    Some(k2_core::deploy::deployment_log_path(app_id, &dep))
}

fn append_log(app_id: &str, line: &str) {
    use std::io::Write as _;
    let Some(p) = app_log_path(app_id) else { return };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
        let _ = writeln!(f, "{line}");
    }
}

/// Write one refused call's line, inside the App's budget.
fn log_refused(budget: &mut RefusedBudget, r: Refused) {
    let e = budget.entry(r.app_id.clone()).or_insert((Instant::now(), 0, 0));
    if e.0.elapsed() >= Duration::from_secs(60) {
        if e.2 > 0 {
            append_log(&r.app_id, &format!("[k2] {} more refused call(s) in the last minute not shown here (k2 app audit {} --denied)", e.2, r.app_id));
        }
        *e = (Instant::now(), 0, 0);
    }
    if e.1 >= REFUSED_LINES_PER_MIN {
        e.2 += 1;
        return;
    }
    e.1 += 1;
    append_log(&r.app_id, &refused_line(&r.verb, &r.code, r.hint.as_deref()));
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
    Refused(Refused),
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
    let mut budget: RefusedBudget = HashMap::new();
    loop {
        // A tick even when no call arrives, so a digest goes out on time.
        let tick = digest_window().min(Duration::from_secs(15)).max(Duration::from_millis(200));
        let first = match rx.recv_timeout(tick) {
            Ok(m) => m,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                flush_digests(false);
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let mut batch = Vec::new();
        let mut flushes = Vec::new();
        let mut next = Some(first);
        while let Some(m) = next.take() {
            match m {
                Msg::Call(c) => batch.push(c),
                Msg::Refused(r) => log_refused(&mut budget, r),
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
            flush_digests(false);
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
        if seen {
            continue;
        }
        // NSI C: a call as one of the App's own test guests is someone
        // testing the App: no wake about it.
        if by_own_test_guest(c) {
            continue;
        }
        if let Some(ws) = ws {
            let mut d = digest().lock().unwrap_or_else(|e| e.into_inner());
            let e = d.entry(c.app_id.clone()).or_insert_with(|| Digest { ws, since: Instant::now(), verbs: Vec::new() });
            if !e.verbs.contains(&c.verb) {
                e.verbs.push(c.verb.clone());
            }
        }
    }
}

// ── first write verbs: one digest per App (NSI C) ────────────────────

/// First write verbs are told in one wake per App per this window
/// (`K2_APPS_FIRST_USE_DIGEST_SECS` overrides, for tests).
pub const DIGEST_WINDOW: Duration = Duration::from_secs(180);

fn digest_window() -> Duration {
    std::env::var("K2_APPS_FIRST_USE_DIGEST_SECS").ok().and_then(|v| v.parse::<u64>().ok()).map(Duration::from_secs).unwrap_or(DIGEST_WINDOW)
}

struct Digest {
    ws: String,
    since: Instant,
    verbs: Vec<String>,
}

fn digest() -> &'static Mutex<HashMap<String, Digest>> {
    static D: OnceLock<Mutex<HashMap<String, Digest>>> = OnceLock::new();
    D.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The digest's one line for an App (pure).
pub fn digest_text(app: &str, verbs: &[String]) -> String {
    let shown: Vec<&str> = verbs.iter().take(8).map(String::as_str).collect();
    let more = if verbs.len() > shown.len() { format!(" and {} more", verbs.len() - shown.len()) } else { String::new() };
    format!(
        "[k2 apps] {app} used {} write verb{} for the first time: {}{more}. k2 app audit {app}",
        verbs.len(),
        if verbs.len() == 1 { "" } else { "s" },
        shown.join(", ")
    )
}

/// Wake each App's agent once with the first write verbs its window
/// gathered (called by the writer on every batch and tick).
fn flush_digests(force: bool) {
    let window = digest_window();
    let due: Vec<(String, Digest)> = {
        let mut d = digest().lock().unwrap_or_else(|e| e.into_inner());
        let ready: Vec<String> = d.iter().filter(|(_, g)| force || g.since.elapsed() >= window).map(|(k, _)| k.clone()).collect();
        ready.into_iter().filter_map(|k| d.remove(&k).map(|g| (k, g))).collect()
    };
    for (app, g) in due {
        if !g.verbs.is_empty() {
            super::alerts::notify(&g.ws, &digest_text(&app, &g.verbs));
        }
    }
}

/// A call made as one of the App's own test guests (the actor, or the
/// guest a login signed in): someone testing the App, not a user.
fn by_own_test_guest(c: &Call) -> bool {
    let principal = [c.actor.as_str(), c.on_behalf_of.as_deref().unwrap_or("")].into_iter().find_map(|a| a.strip_prefix("guest:")).map(str::to_string);
    let Some(p) = principal else { return false };
    let Ok(Some(t)) = k2_core::skin::test_guests::get(&p) else { return false };
    let svc = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        k2_core::deploy::store::get_app(&conn, &c.app_id).ok().flatten().map(|a| a.service_id)
    };
    svc.as_deref() == Some(t.app.as_str())
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
    let mut rows = store::list(&conn, &q).map_err(|e| no("audit_unavailable", 503, e.to_string()))?;
    // NSI 0.46.4 #5: the newest `limit` rows either way; `oldest=1` reads
    // them top to bottom.
    let oldest_first = params.get("oldest").is_some_and(|v| matches!(v.as_str(), "1" | "true" | "yes"));
    if oldest_first {
        rows.reverse();
    }
    let (calls, refused, bin, bout) = store::totals(&conn, &app.app_id, since_ms.unwrap_or(0)).map_err(|e| no("audit_unavailable", 503, e.to_string()))?;
    let mut out = json!({
        "ok": true,
        "app": app.app_id,
        "calls": rows,
        "order": if oldest_first { "oldest first" } else { "newest first" },
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
    fn refusals_give_their_code_and_hint() {
        assert_eq!(refusal_of(r#"{"error":{"code":"skin_room","hint":"not your room"}}"#), (Some("skin_room".into()), Some("not your room".into())));
        assert_eq!(refusal_of(r#"{"error":"invalid_credentials"}"#), (Some("invalid_credentials".into()), None));
        assert_eq!(refusal_of(r#"{"error":"Invalid username or password"}"#), (None, Some("Invalid username or password".into())));
        assert_eq!(refusal_of(r#"{"ok":false,"code":"locked","message":"try later"}"#), (Some("locked".into()), Some("try later".into())));
        assert_eq!(refusal_of("not json"), (None, None));
        assert_eq!(refusal_of(r#"{"error":{"code":"Not A Code"}}"#).0, None, "only snake_case words are codes");
    }

    #[test]
    fn the_log_line_names_the_route_and_code_and_never_a_pass() {
        assert_eq!(
            refused_line("POST /cli/skin/login", "pass_required", Some("send the App pass")),
            "[k2] refused POST /cli/skin/login: pass_required (send the App pass)"
        );
        assert_eq!(refused_line("GET /cli/thread", "", None), "[k2] refused GET /cli/thread: refused");
        let l = refused_line("GET /gw/me", "pass_invalid", Some("the pass k2skn_abcDEF123 is\nexpired; key k2sk_zz9 too"));
        assert!(!l.contains("k2skn_") && !l.contains("k2sk_") && !l.contains('\n'), "{l}");
        assert!(l.contains("[redacted]"), "{l}");
        let long = "x ".repeat(400);
        assert!(log_safe(&long).chars().count() <= HINT_MAX + 1);
        assert!(log_safe(&"a".repeat(40)).contains("[redacted]"), "a long opaque token-like word is redacted");
        // JWT-shaped and query-borne tokens; the verb is cleaned and cut too.
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJxYTEifQ.c2lnbmF0dXJlLXBhcnQtLXdpdGgtZGFzaGVz";
        assert_eq!(log_safe(&format!("bad {jwt}")), "bad [redacted]");
        assert_eq!(log_safe("dash-and-dot.token-0123456789abcdef.0123456789"), "[redacted]");
        assert_eq!(log_safe("token=0123456789abcdef0123456789abcdef"), "[redacted]");
        assert_eq!(log_safe("see k2 app logs myapp"), "see k2 app logs myapp", "plain words stay");
        let l = refused_line(&format!("GET /gw/x?token={jwt}\nSPOOF"), "pass_invalid", None);
        assert!(!l.contains("eyJ") && !l.contains('\n'), "{l}");
        assert!(l.starts_with("[k2] refused [redacted] SPOOF: pass_invalid") || l.starts_with("[k2] refused GET [redacted] SPOOF: pass_invalid"), "{l}");
        let l = refused_line(&format!("GET /{}", "p/".repeat(200)), "not_found", None);
        assert!(l.chars().count() <= "[k2] refused : not_found".len() + VERB_MAX + 1, "{l}");
    }

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
