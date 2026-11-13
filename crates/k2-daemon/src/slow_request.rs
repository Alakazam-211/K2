//! Slow `/cli/*` requests leave a line in the daemon log.
//!
//! A stuck request used to leave no trace: when one workspace's
//! `k2 heartbeat session` and `k2 inbox` calls hung until the client gave
//! up, nothing in the log said which route was waiting, for whom, or for
//! how long. Now:
//!
//! - a `/cli/*` request that takes [`SLOW_AFTER`] or longer logs one line
//!   when it ends: method, route, workspace, elapsed ms, status;
//! - one still running at [`STILL_RUNNING_AFTER`] logs one line then,
//!   naming the route and workspace, so a request that never ends is seen.
//!
//! What is logged: the route is the path only (never the query string, a
//! body or a token); the workspace is the request's `project=` (the TCP
//! port) or the passport's workspace id (a cell socket).
//!
//! Cost on the fast path: two `Instant::now()` calls, one timer entry (no
//! allocation) and a fixed-size copy of the method, route and workspace
//! into a connection-scoped [`Trace`]. Text is only built for a slow one.
//!
//! Not watched: WebSocket upgrades (a socket open for hours is not a slow
//! request) and long polls (`…/wait` routes hold a request open on
//! purpose).

use std::cell::RefCell;
use std::future::Future;
use std::time::Duration;
// tokio's clock (the same as std's outside tests; tests pause it).
use tokio::time::Instant;

/// A `/cli/*` request this slow or slower logs one line when it ends.
pub const SLOW_AFTER: Duration = Duration::from_secs(5);
/// A request still running this long logs one line right then.
pub const STILL_RUNNING_AFTER: Duration = Duration::from_secs(30);

const METHOD_CAP: usize = 8;
const ROUTE_CAP: usize = 112;
const WORKSPACE_CAP: usize = 192;

/// Which door the request came in by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Via {
    /// The daemon's TCP port (owner token, Connect login, `?token=` passport).
    Tcp,
    /// A session's cell socket (the agent's passport as Bearer).
    Cell,
}

impl Via {
    fn as_str(self) -> &'static str {
        match self {
            Via::Tcp => "tcp",
            Via::Cell => "cell",
        }
    }
}

/// What a slow-request line names. Fixed-size: no allocation per request.
#[derive(Clone, Copy)]
pub struct Trace {
    via: Via,
    method: [u8; METHOD_CAP],
    method_len: u8,
    route: [u8; ROUTE_CAP],
    route_len: u8,
    /// TCP: the raw (still percent-encoded) `project=` value; decoded only
    /// when a line is written. Cell: the passport's workspace id.
    workspace: [u8; WORKSPACE_CAP],
    workspace_len: u8,
    workspace_encoded: bool,
    /// The answer's HTTP status (0 = none sent yet).
    status: u16,
    /// A `/cli/*` request that is neither an upgrade nor a long poll.
    watched: bool,
}

impl Default for Trace {
    fn default() -> Self {
        Trace {
            via: Via::Tcp,
            method: [0; METHOD_CAP],
            method_len: 0,
            route: [0; ROUTE_CAP],
            route_len: 0,
            workspace: [0; WORKSPACE_CAP],
            workspace_len: 0,
            workspace_encoded: false,
            status: 0,
            watched: false,
        }
    }
}

/// Copy as much of `src` as fits into `dst`, cut at a UTF-8 boundary.
fn fill(dst: &mut [u8], src: &str) -> u8 {
    let mut n = src.len().min(dst.len()).min(u8::MAX as usize);
    while n > 0 && !src.is_char_boundary(n) {
        n -= 1;
    }
    dst[..n].copy_from_slice(&src.as_bytes()[..n]);
    n as u8
}

fn text(buf: &[u8], len: u8) -> &str {
    std::str::from_utf8(&buf[..len as usize]).unwrap_or("?")
}

/// The raw value of `key=` in a query string (no decoding, no allocation).
fn query_value<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key && !v.is_empty()).then_some(v)
    })
}

/// Is `path` a route the slow-request lines cover? `/cli/*`, minus long
/// polls (a `…/wait` route holds the request open on purpose).
pub fn is_watched_route(path: &str) -> bool {
    path.starts_with("/cli/") && !path.ends_with("/wait")
}

/// Does the request head ask for a WebSocket (or any) upgrade?
fn is_upgrade(head: &str) -> bool {
    crate::routes::http::extract_header(head, "upgrade").is_some_and(|v| !v.trim().is_empty())
}

impl Trace {
    /// A trace for one request. `path_and_query` may carry a query string:
    /// only the path is kept. `workspace` is taken as given (a cell's id).
    pub fn new(via: Via, method: &str, path_and_query: &str, workspace: &str) -> Trace {
        let path = path_and_query.split('?').next().unwrap_or(path_and_query);
        let mut t = Trace { via, ..Trace::default() };
        t.method_len = fill(&mut t.method, method);
        t.route_len = fill(&mut t.route, path);
        t.workspace_len = fill(&mut t.workspace, workspace);
        t.watched = is_watched_route(path);
        t
    }

    /// A TCP request: the workspace is its `project=` (or `project_path=`).
    fn from_tcp(method: &str, path_and_query: &str, head: &str) -> Trace {
        let (path, query) = path_and_query.split_once('?').unwrap_or((path_and_query, ""));
        let project = query_value(query, "project").or_else(|| query_value(query, "project_path")).unwrap_or("");
        let mut t = Trace::new(Via::Tcp, method, path, project);
        t.workspace_encoded = true;
        if t.watched && is_upgrade(head) {
            t.watched = false;
        }
        t
    }

    pub fn set_status(&mut self, status: u16) {
        self.status = status;
    }

    pub fn watched(&self) -> bool {
        self.watched
    }

    fn workspace_text(&self) -> String {
        let raw = text(&self.workspace, self.workspace_len);
        if raw.is_empty() {
            return "-".to_string();
        }
        if !self.workspace_encoded {
            return raw.to_string();
        }
        k2_core::agent_hooks::parse_query_params(&format!("?w={raw}"))
            .remove("w")
            .unwrap_or_else(|| raw.to_string())
    }

    /// The line for a request that ended after [`SLOW_AFTER`] or more.
    pub fn slow_line(&self, elapsed: Duration) -> String {
        let status = match self.status {
            0 => "-".to_string(),
            s => s.to_string(),
        };
        format!(
            "[daemon/slow-request] {} {} workspace={} via={} took {} ms status={}",
            text(&self.method, self.method_len),
            text(&self.route, self.route_len),
            self.workspace_text(),
            self.via.as_str(),
            elapsed.as_millis(),
            status,
        )
    }

    /// The line for a request still running at [`STILL_RUNNING_AFTER`].
    pub fn still_running_line(&self, elapsed: Duration) -> String {
        format!(
            "[daemon/slow-request] {} {} workspace={} via={} still running after {} s",
            text(&self.method, self.method_len),
            text(&self.route, self.route_len),
            self.workspace_text(),
            self.via.as_str(),
            elapsed.as_secs(),
        )
    }
}

tokio::task_local! {
    static CURRENT: RefCell<Trace>;
}

/// Run one TCP connection with its request trace (the dispatcher's loop).
pub async fn scope<F: Future>(f: F) -> F::Output {
    CURRENT.scope(RefCell::new(Trace::default()), f).await
}

/// The dispatcher, once a request's method and target are parsed. `head`
/// is the request head (the upgrade check reads it).
pub fn note_tcp(method: &str, path_and_query: &str, head: &str) {
    let _ = CURRENT.try_with(|c| *c.borrow_mut() = Trace::from_tcp(method, path_and_query, head));
}

/// The response writers: the status of the answer being sent
/// (`"200 OK"` → 200). A no-op outside a TCP connection.
pub fn note_status(status: &str) {
    let code = status_code(status).unwrap_or(0);
    let _ = CURRENT.try_with(|c| c.borrow_mut().status = code);
}

/// The dispatcher's keep-alive wait: true once the next request's first
/// bytes are here (its clock starts then); false on an idle timeout, EOF
/// or an error. `handle_one_request` then peeks the head at once.
pub async fn request_arrived(stream: &tokio::net::TcpStream, idle: Duration) -> bool {
    let mut first = [0u8; 1];
    matches!(tokio::time::timeout(idle, stream.peek(&mut first)).await, Ok(Ok(n)) if n > 0)
}

fn current_tcp() -> Option<Trace> {
    CURRENT.try_with(|c| *c.borrow()).ok()
}

fn emit_log(line: &str) {
    k2_core::log_debug!("{line}");
}

/// The thresholds (the constants in production; short ones in tests).
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub slow: Duration,
    pub still_running: Duration,
}

pub const LIMITS: Limits = Limits { slow: SLOW_AFTER, still_running: STILL_RUNNING_AFTER };

/// Serve one TCP request (`fut` = the dispatcher's handler for it), with the
/// slow-request lines. The trace is reset first; the handler fills it.
pub async fn watch_tcp<F: Future>(fut: F) -> F::Output {
    let _ = CURRENT.try_with(|c| *c.borrow_mut() = Trace::default());
    watch_with(fut, LIMITS, current_tcp, |_| None, &emit_log).await
}

/// Serve one cell-socket request with the slow-request lines. `status_of`
/// reads the status from the handler's answer.
pub async fn watch_cell<F: Future>(
    fut: F,
    trace: Trace,
    status_of: impl FnOnce(&F::Output) -> Option<u16>,
) -> F::Output {
    watch_with(fut, LIMITS, move || Some(trace), status_of, &emit_log).await
}

/// The watcher itself. `current` reads the trace when a line is due (the
/// handler may fill it while it runs); `status_of` may supply the status
/// from the answer; `emit` writes a line.
pub async fn watch_with<F: Future>(
    fut: F,
    limits: Limits,
    current: impl Fn() -> Option<Trace>,
    status_of: impl FnOnce(&F::Output) -> Option<u16>,
    emit: &(dyn Fn(&str) + Sync),
) -> F::Output {
    let started = Instant::now();
    let mut fut = std::pin::pin!(fut);
    let out = match tokio::time::timeout(limits.still_running, fut.as_mut()).await {
        Ok(out) => out,
        Err(_) => {
            if let Some(t) = current().filter(Trace::watched) {
                emit(&t.still_running_line(started.elapsed()));
            }
            fut.await
        }
    };
    let elapsed = started.elapsed();
    if elapsed >= limits.slow {
        if let Some(mut t) = current().filter(Trace::watched) {
            if let Some(s) = status_of(&out) {
                t.set_status(s);
            }
            emit(&t.slow_line(elapsed));
        }
    }
    out
}

/// `"200 OK"` → 200.
pub fn status_code(status: &str) -> Option<u16> {
    status.split_whitespace().next().and_then(|c| c.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn sink() -> (Arc<Mutex<Vec<String>>>, impl Fn(&str) + Sync) {
        let lines = Arc::new(Mutex::new(Vec::new()));
        let l = lines.clone();
        (lines, move |s: &str| l.lock().expect("lines").push(s.to_string()))
    }

    const SHORT: Limits = Limits { slow: Duration::from_millis(5_000), still_running: Duration::from_millis(30_000) };

    #[tokio::test(start_paused = true)]
    async fn a_fast_request_logs_nothing() {
        let (lines, emit) = sink();
        let t = Trace::new(Via::Cell, "GET", "/cli/inbox/list", "ws-1");
        let out = watch_with(async { 7 }, SHORT, move || Some(t), |_| Some(200), &emit).await;
        assert_eq!(out, 7);
        assert!(lines.lock().expect("lines").is_empty(), "{:?}", lines.lock().expect("lines"));
    }

    #[tokio::test(start_paused = true)]
    async fn a_slow_request_logs_one_line_when_it_ends() {
        let (lines, emit) = sink();
        let t = Trace::new(Via::Cell, "GET", "/cli/heartbeat/list?token=secret-tok&name=x", "ws-123");
        let out = watch_with(
            async {
                tokio::time::sleep(Duration::from_secs(6)).await;
                "done"
            },
            SHORT,
            move || Some(t),
            |_| Some(200),
            &emit,
        )
        .await;
        assert_eq!(out, "done");
        let lines = lines.lock().expect("lines").clone();
        assert_eq!(lines.len(), 1, "{lines:?}");
        let l = &lines[0];
        assert!(l.contains("GET /cli/heartbeat/list workspace=ws-123 via=cell took 6"), "{l}");
        assert!(l.ends_with("status=200"), "{l}");
        assert!(!l.contains("secret-tok") && !l.contains("name=x") && !l.contains('?'), "no query values: {l}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_request_still_running_at_30s_logs_then_and_again_when_it_ends() {
        let (lines, emit) = sink();
        let t = Trace::new(Via::Cell, "POST", "/cli/heartbeat/set-session", "ws-9");
        watch_with(tokio::time::sleep(Duration::from_secs(95)), SHORT, move || Some(t), |_| Some(504), &emit).await;
        let lines = lines.lock().expect("lines").clone();
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert_eq!(
            lines[0],
            "[daemon/slow-request] POST /cli/heartbeat/set-session workspace=ws-9 via=cell still running after 30 s"
        );
        assert!(lines[1].contains("took 95000 ms status=504"), "{}", lines[1]);
    }

    #[tokio::test(start_paused = true)]
    async fn routes_outside_cli_and_long_polls_are_not_watched() {
        for route in ["/events", "/v1/sandboxes", "/cli/mail/wait", "/cli/sim/wait"] {
            let (lines, emit) = sink();
            let t = Trace::new(Via::Tcp, "GET", route, "");
            watch_with(tokio::time::sleep(Duration::from_secs(40)), SHORT, move || Some(t), |_| None, &emit).await;
            assert!(lines.lock().expect("lines").is_empty(), "{route}: {:?}", lines.lock().expect("lines"));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn tcp_trace_names_the_project_and_the_status_the_writer_noted() {
        let (lines, emit) = sink();
        let head = "GET /cli/inbox/list?token=owner-secret&project=%2FUsers%2Facme%2Fwork%20space HTTP/1.1\r\nHost: x\r\n\r\n";
        let line = scope(async {
            let _ = CURRENT.try_with(|c| *c.borrow_mut() = Trace::default());
            watch_with(
                async {
                    note_tcp("GET", "/cli/inbox/list?token=owner-secret&project=%2FUsers%2Facme%2Fwork%20space", head);
                    tokio::time::sleep(Duration::from_secs(8)).await;
                    note_status("200 OK");
                },
                SHORT,
                current_tcp,
                |_| None,
                &emit,
            )
            .await;
            lines.lock().expect("lines").clone()
        })
        .await;
        assert_eq!(line.len(), 1, "{line:?}");
        assert!(
            line[0].starts_with("[daemon/slow-request] GET /cli/inbox/list workspace=/Users/acme/work space via=tcp took 8"),
            "{}",
            line[0]
        );
        assert!(line[0].ends_with("status=200"), "{}", line[0]);
        assert!(!line[0].contains("owner-secret"), "{}", line[0]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_websocket_upgrade_is_not_watched() {
        let (lines, emit) = sink();
        let head = "GET /cli/sessions/events?path=x HTTP/1.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n";
        scope(async {
            watch_with(
                async {
                    note_tcp("GET", "/cli/sessions/events?path=x", head);
                    tokio::time::sleep(Duration::from_secs(3600)).await;
                },
                SHORT,
                current_tcp,
                |_| None,
                &emit,
            )
            .await;
        })
        .await;
        assert!(lines.lock().expect("lines").is_empty(), "{:?}", lines.lock().expect("lines"));
    }

    #[tokio::test(start_paused = true)]
    async fn watch_tcp_resets_the_previous_requests_trace() {
        // A keep-alive connection: a slow /cli request, then a slow one the
        // dispatcher never noted (it failed before parsing) logs nothing.
        scope(async {
            watch_tcp(async {
                note_tcp("GET", "/cli/heartbeat/list", "");
            })
            .await;
            let _ = watch_tcp(async {}).await;
            let t = current_tcp().expect("in scope");
            assert!(!t.watched(), "the old request's trace was reset");
        })
        .await;
    }

    #[test]
    fn long_values_are_cut_at_a_char_boundary() {
        let long = format!("/cli/{}", "é".repeat(200));
        let t = Trace::new(Via::Cell, "GET", &long, &"w".repeat(500));
        let line = t.slow_line(Duration::from_secs(6));
        assert!(line.contains("/cli/éé"), "{line}");
        assert!(line.len() < 600, "{}", line.len());
    }

    #[test]
    fn status_code_reads_the_leading_number() {
        assert_eq!(status_code("200 OK"), Some(200));
        assert_eq!(status_code("503 Service Unavailable"), Some(503));
        assert_eq!(status_code(""), None);
    }
}
