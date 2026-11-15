//! `k2 app requests`: the per-request log of visitor requests to an App
//! (store: `k2_core::app_requests`, `~/.k2/apps/app-requests.db`).
//!
//! **Recording.** The forwarder (`client_ip::forward_diverted`, reached
//! from `router::splice_doors` on every App connection it parses) hands
//! each finished request to [`sink_for`]'s sink, and the router reports
//! K2's own 502 and 503 ([`k2_answered`]). [`record`] only puts the row
//! on a bounded in-memory queue ([`QUEUE_CAP`]): it never waits, and when
//! the queue is full the row is dropped and counted ([`dropped`], shown by
//! the route). One writer thread drains the queue in batches of up to
//! [`BATCH`] rows per transaction, looks up the deployment (by the
//! backend's port), turns the visitor's address into the day's visitor
//! key, and prunes ([`k2_core::app_requests::prune`]) every hour and after
//! every [`PRUNE_EVERY_ROWS`] rows. Nothing on a connection's task touches
//! SQLite or the main database.
//!
//! **Visitor keys.** A random 32-byte key per UTC day, made in memory by
//! the writer and never written anywhere. A restart starts a new key, so
//! the same visitor gets a new key after a restart too.
//!
//! **The route** (read only):
//! ```text
//! GET /cli/app/requests ?app=&since=1h|<epoch-ms>&until=&status=4xx|5xx|N&path_prefix=&method=&limit=
//! ```
//! The owner, an Admin, a person in a K2 shell tab, and the App's own
//! workspace agent (its own Apps only). Never a Member, a guest or an App
//! pass.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use k2_core::app_requests::{self as store, codes as rcodes, Request, StatusFilter};
use k2_core::connect_users::Role;
use k2_core::deploy::codes;
use serde_json::{json, Value as J};
use sha2::Digest as _;

use super::routes::{app_for, need_credential, no, Who, R};

pub const ROUTES: &[&str] = &["/cli/app/requests"];

/// Rows waiting for the writer. Past it, rows are dropped and counted.
pub const QUEUE_CAP: usize = 20_000;
/// Rows per write transaction.
pub const BATCH: usize = 1_000;
/// Prune after this many rows, as well as every hour.
pub const PRUNE_EVERY_ROWS: u64 = 50_000;
const PRUNE_EVERY: Duration = Duration::from_secs(3600);

/// Rows that never reached the file: the queue was full, or a write failed.
static DROPPED: AtomicU64 = AtomicU64::new(0);

/// Rows dropped since the daemon started.
pub fn dropped() -> u64 {
    DROPPED.load(Ordering::Relaxed)
}

/// A row on its way to the writer: the raw visitor address stays in memory
/// only; the writer stores its key.
pub struct Pending {
    pub app_id: Arc<str>,
    /// The backend port it went to (the deployment is looked up by it).
    pub port: Option<u16>,
    pub rec: crate::client_ip::RequestRecord,
}

enum Msg {
    Row(Pending),
    Flush(SyncSender<()>),
    /// Tests: prune now (with these limits) and say how many rows went.
    #[cfg(test)]
    Prune { now_ms: i64, keep_days: i64, max_per_app: i64, done: SyncSender<usize> },
}

fn sender() -> &'static SyncSender<Msg> {
    static TX: OnceLock<SyncSender<Msg>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = mpsc::sync_channel(QUEUE_CAP);
        let _ = std::thread::Builder::new().name("app-requests".into()).spawn(move || writer(rx));
        tx
    })
}

/// Queue one row. Never blocks: a full queue drops it (counted).
pub fn record(p: Pending) {
    match sender().try_send(Msg::Row(p)) {
        Ok(()) => {}
        Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
            DROPPED.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Wait (at most `wait`) until every row queued so far is written.
pub fn flush(wait: Duration) {
    let (tx, rx) = mpsc::sync_channel(1);
    let deadline = Instant::now() + wait;
    let mut msg = Msg::Flush(tx);
    // The queue may be full: retry until there's room or the time is up.
    loop {
        match sender().try_send(msg) {
            Ok(()) => break,
            Err(TrySendError::Full(m)) if Instant::now() < deadline => {
                msg = m;
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(_) => return,
        }
    }
    let _ = rx.recv_timeout(deadline.saturating_duration_since(Instant::now()));
}

/// Tests: run the writer's prune now with these limits; how many rows went.
#[cfg(test)]
pub(crate) fn prune_now(now_ms: i64, keep_days: i64, max_per_app: i64) -> usize {
    let (tx, rx) = mpsc::sync_channel(1);
    sender().send(Msg::Prune { now_ms, keep_days, max_per_app, done: tx }).expect("the writer runs");
    rx.recv_timeout(Duration::from_secs(10)).expect("the writer pruned")
}

/// The forwarder's sink for one App connection to `port`.
pub fn sink_for(app_id: &str, port: Option<u16>) -> crate::client_ip::RequestSink {
    let app: Arc<str> = Arc::from(app_id);
    Arc::new(move |rec| record(Pending { app_id: app.clone(), port, rec }))
}

/// K2 answered a connection itself before the App had it (502 not
/// answering, 503 not running). `head`: the bytes read of the request (its
/// method and path come from the first line; nothing else is kept).
pub fn k2_answered(
    app_id: &str,
    port: Option<u16>,
    visitor: Option<IpAddr>,
    head: &[u8],
    answer_len: usize,
    status: u16,
    code: &'static str,
    started: Instant,
    at_ms: i64,
) {
    let (method, path) = first_line(head);
    let head_len = head.windows(4).position(|w| w == b"\r\n\r\n").map_or(head.len(), |i| i + 4);
    let ms = started.elapsed().as_millis() as u64;
    record(Pending {
        app_id: Arc::from(app_id),
        port,
        rec: crate::client_ip::RequestRecord {
            at_ms,
            method,
            path,
            status,
            k2_code: Some(code),
            bytes_in: head_len as u64,
            bytes_out: answer_len as u64,
            head_ms: Some(ms),
            total_ms: ms,
            closed_early: None,
            ws: None,
            visitor,
        },
    });
}

/// `(method, safe path)` of the request line at the start of `head`
/// (`("", "")` when there isn't one).
fn first_line(head: &[u8]) -> (String, String) {
    let end = head.windows(2).position(|w| w == b"\r\n").unwrap_or(head.len());
    let line = String::from_utf8_lossy(&head[..end.min(16 * 1024)]);
    let mut parts = line.split(' ');
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("");
    let is_token = !method.is_empty() && method.len() <= 16 && method.bytes().all(|b| b.is_ascii_alphabetic());
    if !is_token || target.is_empty() {
        return (String::new(), String::new());
    }
    (method.to_string(), store::safe_path(target))
}

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

// ── the writer ───────────────────────────────────────────────────────

/// Today's visitor key (memory only; a new one each UTC day).
struct DayKeys {
    day: i64,
    key: [u8; 32],
}

impl DayKeys {
    fn new() -> Self {
        DayKeys { day: i64::MIN, key: [0; 32] }
    }

    fn key_for(&mut self, at_ms: i64) -> &[u8; 32] {
        let day = at_ms.div_euclid(86_400_000);
        if day != self.day {
            let mut k = [0u8; 32];
            if getrandom::getrandom(&mut k).is_err() {
                // No OS randomness: a key from the clock, still memory only.
                k.copy_from_slice(&sha2::Sha256::digest(format!("{:?}{:?}", Instant::now(), std::time::SystemTime::now()).as_bytes()));
            }
            self.key = k;
            self.day = day;
        }
        &self.key
    }
}

/// The deployment serving `(app, port)`, looked up off the hot path and
/// kept 30 s.
struct Deployments(HashMap<(Arc<str>, Option<u16>), (Instant, Option<String>)>);

impl Deployments {
    fn get(&mut self, app: &Arc<str>, port: Option<u16>) -> Option<String> {
        if let Some((at, d)) = self.0.get(&(app.clone(), port)) {
            if at.elapsed() < Duration::from_secs(30) {
                return d.clone();
            }
        }
        let d = lookup_deployment(app, port);
        if self.0.len() > 4096 {
            self.0.clear();
        }
        self.0.insert((app.clone(), port), (Instant::now(), d.clone()));
        d
    }
}

fn lookup_deployment(app: &str, port: Option<u16>) -> Option<String> {
    let port = port?;
    let h = k2_core::db::shared();
    let conn = h.lock();
    let deps = k2_core::deploy::store::list_deployments(&conn, app, 20).ok()?;
    deps.into_iter().find(|d| d.port == Some(port as i64)).map(|d| d.id).or_else(|| {
        k2_core::deploy::store::get_app(&conn, app).ok().flatten().and_then(|a| a.live_deployment)
    })
}

fn writer(rx: Receiver<Msg>) {
    let mut conn: Option<(std::path::PathBuf, rusqlite::Connection)> = None;
    let mut keys = DayKeys::new();
    let mut deps = Deployments(HashMap::new());
    // `None`: prune with the first batch (a restart prunes too).
    let mut last_prune: Option<Instant> = None;
    let mut since_prune = 0u64;
    loop {
        let first = match rx.recv_timeout(PRUNE_EVERY) {
            Ok(m) => Some(m),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let mut rows: Vec<Request> = Vec::new();
        let mut flushes = Vec::new();
        #[cfg(test)]
        let mut prunes = Vec::new();
        let mut next = first;
        while let Some(m) = next.take() {
            match m {
                Msg::Row(p) => rows.push(to_row(p, &mut keys, &mut deps)),
                Msg::Flush(t) => flushes.push(t),
                #[cfg(test)]
                Msg::Prune { now_ms, keep_days, max_per_app, done } => prunes.push((now_ms, keep_days, max_per_app, done)),
            }
            if rows.len() < BATCH {
                next = rx.try_recv().ok();
            }
        }
        // The home can change under tests (TempHome): reopen on a new path.
        let path = store::db_path();
        if !writable(&path) {
            DROPPED.fetch_add(rows.len() as u64, Ordering::Relaxed);
            for f in flushes {
                let _ = f.send(());
            }
            continue;
        }
        if conn.as_ref().is_none_or(|(p, _)| *p != path) {
            conn = match store::open(&path) {
                Ok(c) => Some((path.clone(), c)),
                Err(e) => {
                    k2_core::log_debug!("[apps/requests] open {}: {e}", path.display());
                    None
                }
            };
        }
        match conn.as_ref() {
            Some((_, c)) => {
                if let Err(e) = store::append(c, &rows) {
                    DROPPED.fetch_add(rows.len() as u64, Ordering::Relaxed);
                    k2_core::log_debug!("[apps/requests] append {} rows: {e}", rows.len());
                }
                #[cfg(test)]
                for (now, keep, max, done) in prunes.drain(..) {
                    let _ = done.send(store::prune(c, now, keep, max).unwrap_or_else(|e| panic!("prune: {e}")));
                }
                since_prune += rows.len() as u64;
                if last_prune.is_none_or(|t| t.elapsed() >= PRUNE_EVERY) || since_prune >= PRUNE_EVERY_ROWS {
                    last_prune = Some(Instant::now());
                    since_prune = 0;
                    if let Err(e) = store::prune(c, now_ms(), store::KEEP_DAYS, store::MAX_ROWS_PER_APP) {
                        k2_core::log_debug!("[apps/requests] prune: {e}");
                    }
                }
            }
            None => {
                DROPPED.fetch_add(rows.len() as u64, Ordering::Relaxed);
            }
        }
        for f in flushes {
            let _ = f.send(());
        }
    }
}

/// A test build writes only inside a temp home: a row that arrives after a
/// test's `TempHome` restored `$HOME` must never reach the real `~/.k2`.
#[cfg(test)]
fn writable(path: &std::path::Path) -> bool {
    !k2_core::test_isolation::store_in_real_home(path, k2_core::test_isolation::real_home().as_deref())
}

#[cfg(not(test))]
fn writable(_path: &std::path::Path) -> bool {
    true
}

fn to_row(p: Pending, keys: &mut DayKeys, deps: &mut Deployments) -> Request {
    let r = p.rec;
    Request {
        at_ms: r.at_ms,
        app_id: p.app_id.to_string(),
        deployment: deps.get(&p.app_id, p.port),
        method: r.method,
        path: r.path,
        status: r.status,
        k2_code: r.k2_code.map(str::to_string),
        bytes_in: r.bytes_in,
        bytes_out: r.bytes_out,
        head_ms: r.head_ms,
        total_ms: r.total_ms,
        closed_early: r.closed_early.map(str::to_string),
        ws: r.ws.map(str::to_string),
        visitor: r.visitor.map(|ip| store::visitor_key(keys.key_for(r.at_ms), ip)),
    }
}

// ── route ────────────────────────────────────────────────────────────

/// The owner, an Admin, a person in a K2 shell tab, or the App's own agent
/// (`app_for` keeps an agent to its own workspace's Apps).
fn may_read(who: &Who) -> R<()> {
    need_credential(who)?;
    match who {
        Who::Login { role, .. } if *role < Role::Admin => {
            Err(no(codes::FORBIDDEN, 403, "an App's request log is the owner's, an Admin's and the App's own agent's"))
        }
        _ => Ok(()),
    }
}

fn time_param(params: &HashMap<String, String>, key: &str) -> R<Option<i64>> {
    match params.get(key).map(String::as_str).filter(|s| !s.trim().is_empty()) {
        None => Ok(None),
        Some(s) => super::audit::parse_since(s, now_ms())
            .map(Some)
            .ok_or_else(|| no(codes::BAD_REQUEST, 400, format!("{key} is 30m, 1h, 2d or epoch ms, not '{s}'"))),
    }
}

pub(crate) fn handle(who: &Who, params: &HashMap<String, String>) -> R<J> {
    may_read(who)?;
    let app = app_for(who, params.get("app").map(String::as_str))?;
    let since_ms = time_param(params, "since")?;
    let until_ms = time_param(params, "until")?;
    let status = match params.get("status").map(String::as_str).filter(|s| !s.trim().is_empty()) {
        None => None,
        Some(s) => Some(StatusFilter::parse(s).ok_or_else(|| no(codes::BAD_REQUEST, 400, format!("status is 2xx, 4xx, 5xx or a code like 404, not '{s}'")))?),
    };
    let method = params.get("method").map(|m| m.trim()).filter(|m| !m.is_empty());
    if let Some(m) = method {
        if m.len() > 16 || !m.bytes().all(|b| b.is_ascii_alphabetic()) {
            return Err(no(codes::BAD_REQUEST, 400, format!("method is a word like GET or POST, not '{m}'")));
        }
    }
    let path_prefix = params.get("path_prefix").map(|p| p.trim()).filter(|p| !p.is_empty());
    let limit = match params.get("limit").map(|l| l.trim()).filter(|l| !l.is_empty()) {
        None => 100,
        Some(l) => l.parse::<i64>().ok().filter(|n| (1..=1000).contains(n)).ok_or_else(|| no(codes::BAD_REQUEST, 400, format!("limit is 1 to 1000, not '{l}'")))?,
    };
    flush(Duration::from_secs(2));
    let conn = store::open(&store::db_path()).map_err(|e| no("requests_unavailable", 503, format!("app-requests.db: {e}")))?;
    let q = store::Query { app_id: &app.app_id, since_ms, until_ms, status, path_prefix, method, limit };
    let rows = store::list(&conn, &q).map_err(|e| no("requests_unavailable", 503, e.to_string()))?;
    let totals = store::totals(&conn, &q).map_err(|e| no("requests_unavailable", 503, e.to_string()))?;
    Ok(json!({
        "ok": true,
        "app": app.app_id,
        "requests": rows,
        "order": "newest first",
        "totals": totals,
        "dropped": dropped(),
        "keep": { "days": store::KEEP_DAYS, "rowsPerApp": store::MAX_ROWS_PER_APP },
        "stored": "method, path without its query, status, K2's code, bytes, timings, who closed early, a daily visitor key; never query strings, header values, bodies or IP addresses",
        "codes": [rcodes::BAD_REQUEST, rcodes::HEAD_TOO_LARGE, rcodes::TOO_MANY_WRITES, rcodes::APP_NOT_ANSWERING, rcodes::APP_NOT_RUNNING, rcodes::SIGN_IN_DOOR, rcodes::K2_ANSWER],
        "file": store::db_path(),
    }))
}

#[cfg(test)]
#[path = "requests_tests.rs"]
mod tests;
