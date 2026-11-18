//! The per-request log of visitor requests to an App (`k2 app requests`).
//!
//! The App audit (`app_audit`) records what an App asks of K2. This is the
//! other direction: every request a visitor's browser (or phone app) sends
//! to the App through K2's forwarder, one row each, so an operator or the
//! App's own agent can tell whether a request reached the App and how it
//! ended.
//!
//! **One row per request** on the App's connection (keep-alive gives one
//! row each): time (UTC ms), App, deployment, method, the path WITHOUT
//! its query string or fragment ([`safe_path`]), the response status (the
//! App's, or K2's own answer with K2's code in `k2_code`), request bytes
//! in, response bytes out, milliseconds to the response head and to the
//! end, who closed early (`client`, `app`, or `k2` when K2 cut it: a
//! drain), and a visitor key. A WebSocket gives one row at the upgrade
//! (`ws = upgrade`, status 101) and one when the socket closes (`ws =
//! close`, the bytes each way after the upgrade).
//!
//! **Never stored:** query strings, fragments, header values (cookies and
//! `Authorization` included), bodies, raw IP addresses. The visitor key is
//! a keyed hash of the address under a key that lives only in the daemon's
//! memory and changes every UTC day ([`visitor_key`]): requests group by
//! visitor within a day, and nothing in this file turns back into an
//! address (the key is never written anywhere).
//!
//! **Bounded:** its own file, `~/.k2/apps/app-requests.db` (owner-only,
//! its own schema version like `app-audit.db`), so a busy App never grows
//! `k2so.db`. Rows are kept [`KEEP_DAYS`] days and at most
//! [`MAX_ROWS_PER_APP`] per App (the newest are kept); the daemon's writer
//! prunes ([`prune`]).

use std::net::IpAddr;
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection};
use serde::Serialize;
use sha2::{Digest, Sha256};

/// `PRAGMA user_version` of a current file.
pub const SCHEMA_VERSION: i64 = 1;
/// Rows are kept this many days.
pub const KEEP_DAYS: i64 = 7;
/// At most this many rows per App (the newest). About 200 bytes a row with
/// its indexes, so at most ~40 MB per App: a week of a small App's traffic
/// (about 20 requests a minute, all week), or the last several hours of a
/// busy one, which is the window someone debugging a request looks at.
pub const MAX_ROWS_PER_APP: i64 = 200_000;
/// The longest path kept (characters); longer ones end in `…`.
pub const MAX_PATH_CHARS: usize = 512;

/// K2's codes on a row K2 answered itself (the App never saw the request,
/// or never answered it).
pub mod codes {
    /// Malformed or ambiguous HTTP framing (400).
    pub const BAD_REQUEST: &str = "bad_request";
    /// A request head over K2's limit (431).
    pub const HEAD_TOO_LARGE: &str = "head_too_large";
    /// The visitor is over the guard's write limit (429).
    pub const TOO_MANY_WRITES: &str = "too_many_writes";
    /// The App's port didn't take the connection (502).
    pub const APP_NOT_ANSWERING: &str = "app_not_answering";
    /// Nothing of the App is running (503).
    pub const APP_NOT_RUNNING: &str = "app_not_running";
    /// K2's sign-in door answered (`/.k2/auth/*`).
    pub const SIGN_IN_DOOR: &str = "sign_in_door";
    /// K2's own answer on the App's host (the pairing link files).
    pub const K2_ANSWER: &str = "k2_answer";
    /// K2's gate on a published site.
    pub const SITE_GATE: &str = "site_gate";
}

/// `~/.k2/apps/app-requests.db`.
pub fn db_path() -> PathBuf {
    crate::paths::k2_home().join("apps").join("app-requests.db")
}

const SCHEMA_V1: &str = "
CREATE TABLE IF NOT EXISTS requests (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  at_ms INTEGER NOT NULL,
  app_id TEXT NOT NULL,
  deployment TEXT,
  method TEXT NOT NULL,
  path TEXT NOT NULL,
  status INTEGER NOT NULL,
  k2_code TEXT,
  bytes_in INTEGER NOT NULL,
  bytes_out INTEGER NOT NULL,
  head_ms INTEGER,
  total_ms INTEGER NOT NULL,
  closed_early TEXT,
  ws TEXT,
  visitor TEXT
);
CREATE INDEX IF NOT EXISTS requests_app_at ON requests (app_id, at_ms);
CREATE INDEX IF NOT EXISTS requests_app ON requests (app_id);
";

/// Open (and create or upgrade) the file at `path`, owner-only on Unix.
pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let conn = Connection::open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(windows)]
    let _ = crate::private_fs::make_private_file(path);
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")?;
    migrate(&conn)?;
    Ok(conn)
}

/// Bring a connection's schema to [`SCHEMA_VERSION`]; a newer file is
/// refused rather than written with an old shape.
pub fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if v > SCHEMA_VERSION {
        return Err(rusqlite::Error::InvalidParameterName(format!(
            "app-requests.db is schema {v}; this K2 knows {SCHEMA_VERSION} (update K2)"
        )));
    }
    if v < 1 {
        conn.execute_batch(SCHEMA_V1)?;
        conn.execute_batch("PRAGMA user_version = 1")?;
    }
    Ok(())
}

/// One request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub at_ms: i64,
    #[serde(rename = "app")]
    pub app_id: String,
    pub deployment: Option<String>,
    pub method: String,
    pub path: String,
    /// The status the visitor got (`0`: no response head at all).
    pub status: u16,
    /// Set when K2 answered instead of the App ([`codes`]).
    pub k2_code: Option<String>,
    pub bytes_in: u64,
    pub bytes_out: u64,
    /// Milliseconds to the response head (`None`: none came).
    pub head_ms: Option<u64>,
    pub total_ms: u64,
    /// `client`, `app` or `k2` (K2 cut it), when the response didn't finish.
    pub closed_early: Option<String>,
    /// `upgrade` or `close` for a WebSocket's two rows.
    pub ws: Option<String>,
    /// [`visitor_key`]: never an address. `None`: K2 didn't know the visitor.
    pub visitor: Option<String>,
}

/// The path of a request target with nothing a log must not keep: no
/// query string, no fragment, no `;` parameters (session ids ride there),
/// no scheme or host (absolute-form targets), a segment that is a K2 pass
/// or a JWT replaced by `[redacted]`, control characters dropped, at most
/// [`MAX_PATH_CHARS`] characters.
pub fn safe_path(target: &str) -> String {
    let cut = target.find(['?', '#']).map_or(target, |i| &target[..i]);
    let path = match cut.split_once("://") {
        Some((scheme, rest)) if !scheme.is_empty() && scheme.bytes().all(|b| b.is_ascii_alphabetic()) => {
            rest.find('/').map_or("/", |i| &rest[i..])
        }
        _ => cut,
    };
    let mut out = String::with_capacity(path.len().min(MAX_PATH_CHARS + 4));
    for (i, seg) in path.split('/').enumerate() {
        if i > 0 {
            out.push('/');
        }
        let seg = seg.split(';').next().unwrap_or("");
        if secret_shaped(seg) {
            out.push_str("[redacted]");
        } else {
            out.extend(seg.chars().filter(|c| !c.is_control()));
        }
    }
    if out.chars().count() > MAX_PATH_CHARS {
        out = out.chars().take(MAX_PATH_CHARS).collect::<String>() + "…";
    }
    out
}

/// A K2 pass or key, or a JWT.
fn secret_shaped(seg: &str) -> bool {
    ["k2skn_", "k2sk_", "k2skr_", "k2hs_", "k2lgn_", "k2chg_", "k2inv_", "k2rs_"].iter().any(|p| seg.starts_with(p))
        || (seg.starts_with("eyJ") && seg.len() >= 16)
}

/// The visitor key of `ip` under one day's `key`: 16 hex characters of
/// `sha256(key || "k2-visitor-v1" || ip)`. The same address gives the same
/// key all day; without the key (memory only, a new one each UTC day)
/// nothing maps it back.
pub fn visitor_key(key: &[u8; 32], ip: IpAddr) -> String {
    let ip = match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(IpAddr::V6(v6)),
        v4 => v4,
    };
    let mut h = Sha256::new();
    h.update(key);
    h.update(b"k2-visitor-v1");
    h.update(ip.to_string().as_bytes());
    h.finalize()[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// Append rows in one transaction.
pub fn append(conn: &Connection, rows: &[Request]) -> rusqlite::Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let tx = conn.unchecked_transaction()?;
    {
        let mut st = tx.prepare_cached(
            "INSERT INTO requests (at_ms, app_id, deployment, method, path, status, k2_code, bytes_in, bytes_out, head_ms, \
             total_ms, closed_early, ws, visitor) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        )?;
        for r in rows {
            st.execute(params![
                r.at_ms,
                r.app_id,
                r.deployment,
                r.method,
                r.path,
                r.status as i64,
                r.k2_code,
                r.bytes_in.min(i64::MAX as u64) as i64,
                r.bytes_out.min(i64::MAX as u64) as i64,
                r.head_ms.map(|v| v.min(i64::MAX as u64) as i64),
                r.total_ms.min(i64::MAX as u64) as i64,
                r.closed_early,
                r.ws,
                r.visitor
            ])?;
        }
    }
    tx.commit()
}

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Request> {
    Ok(Request {
        at_ms: r.get(0)?,
        app_id: r.get(1)?,
        deployment: r.get(2)?,
        method: r.get(3)?,
        path: r.get(4)?,
        status: r.get::<_, i64>(5)?.clamp(0, u16::MAX as i64) as u16,
        k2_code: r.get(6)?,
        bytes_in: r.get::<_, i64>(7)?.max(0) as u64,
        bytes_out: r.get::<_, i64>(8)?.max(0) as u64,
        head_ms: r.get::<_, Option<i64>>(9)?.map(|v| v.max(0) as u64),
        total_ms: r.get::<_, i64>(10)?.max(0) as u64,
        closed_early: r.get(11)?,
        ws: r.get(12)?,
        visitor: r.get(13)?,
    })
}

const COLS: &str =
    "at_ms, app_id, deployment, method, path, status, k2_code, bytes_in, bytes_out, head_ms, total_ms, closed_early, ws, visitor";

/// Which statuses [`list`] returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusFilter {
    /// `4xx`: 400–499 (`class` = 4).
    Class(u16),
    Exact(u16),
}

impl StatusFilter {
    /// `4xx`, `5XX`, `404`, `0` (no response).
    pub fn parse(raw: &str) -> Option<Self> {
        let t = raw.trim().to_ascii_lowercase();
        if let Some(d) = t.strip_suffix("xx") {
            let c: u16 = d.parse().ok()?;
            return (1..=5).contains(&c).then_some(StatusFilter::Class(c));
        }
        let n: u16 = t.parse().ok()?;
        (n == 0 || (100..=599).contains(&n)).then_some(StatusFilter::Exact(n))
    }

    fn range(self) -> (i64, i64) {
        match self {
            StatusFilter::Class(c) => (c as i64 * 100, c as i64 * 100 + 99),
            StatusFilter::Exact(n) => (n as i64, n as i64),
        }
    }
}

/// Which rows [`list`] returns.
#[derive(Debug, Clone, Default)]
pub struct Query<'a> {
    pub app_id: &'a str,
    pub since_ms: Option<i64>,
    pub until_ms: Option<i64>,
    pub status: Option<StatusFilter>,
    /// Paths starting with this.
    pub path_prefix: Option<&'a str>,
    /// Exactly this method (any case).
    pub method: Option<&'a str>,
    pub limit: i64,
}

const WHERE: &str = "app_id = ?1 AND (?2 IS NULL OR at_ms >= ?2) AND (?3 IS NULL OR at_ms <= ?3) \
                     AND (?4 IS NULL OR status BETWEEN ?4 AND ?5) \
                     AND (?6 IS NULL OR substr(path, 1, length(?6)) = ?6) AND (?7 IS NULL OR method = upper(?7))";

/// The newest `limit` matching rows, newest first.
pub fn list(conn: &Connection, q: &Query<'_>) -> rusqlite::Result<Vec<Request>> {
    let (lo, hi) = q.status.map(StatusFilter::range).unzip();
    let mut st = conn.prepare(&format!("SELECT {COLS} FROM requests WHERE {WHERE} ORDER BY at_ms DESC, id DESC LIMIT ?8"))?;
    let rows = st.query_map(
        params![q.app_id, q.since_ms, q.until_ms, lo, hi, q.path_prefix, q.method, q.limit.clamp(1, 1000)],
        row,
    )?;
    rows.collect()
}

/// Totals over every row [`list`] would match (not only the `limit`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Totals {
    pub requests: i64,
    pub ok: i64,
    pub client_errors: i64,
    pub server_errors: i64,
    /// Answered by K2 instead of the App.
    pub k2_answered: i64,
    pub closed_early: i64,
    pub bytes_in: i64,
    pub bytes_out: i64,
}

pub fn totals(conn: &Connection, q: &Query<'_>) -> rusqlite::Result<Totals> {
    let (lo, hi) = q.status.map(StatusFilter::range).unzip();
    conn.query_row(
        &format!(
            "SELECT COUNT(*), COALESCE(SUM(status BETWEEN 100 AND 399), 0), COALESCE(SUM(status BETWEEN 400 AND 499), 0), \
             COALESCE(SUM(status >= 500), 0), COALESCE(SUM(k2_code IS NOT NULL), 0), COALESCE(SUM(closed_early IS NOT NULL), 0), \
             COALESCE(SUM(bytes_in), 0), COALESCE(SUM(bytes_out), 0) FROM requests WHERE {WHERE}"
        ),
        params![q.app_id, q.since_ms, q.until_ms, lo, hi, q.path_prefix, q.method],
        |r| {
            Ok(Totals {
                requests: r.get(0)?,
                ok: r.get(1)?,
                client_errors: r.get(2)?,
                server_errors: r.get(3)?,
                k2_answered: r.get(4)?,
                closed_early: r.get(5)?,
                bytes_in: r.get(6)?,
                bytes_out: r.get(7)?,
            })
        },
    )
}

/// Delete every row of App `app_id` (0.46.16: an uninstall ends its log, so
/// a reinstall under the same id starts empty). Returns how many went.
pub fn forget_app(conn: &Connection, app_id: &str) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM requests WHERE app_id = ?1", params![app_id])
}

/// Delete rows older than `keep_days` and, per App, all but the newest
/// `max_per_app`. Returns how many rows went.
pub fn prune(conn: &Connection, now_ms: i64, keep_days: i64, max_per_app: i64) -> rusqlite::Result<usize> {
    let apps: Vec<String> = {
        let mut st = conn.prepare("SELECT DISTINCT app_id FROM requests")?;
        let rows = st.query_map([], |r| r.get(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let cut = now_ms - keep_days * 86_400_000;
    let tx = conn.unchecked_transaction()?;
    let mut n = 0;
    for app in &apps {
        n += tx.execute("DELETE FROM requests WHERE app_id = ?1 AND at_ms < ?2", params![app, cut])?;
        // The id of the oldest row to keep; everything before it goes.
        n += tx.execute(
            "DELETE FROM requests WHERE app_id = ?1 AND id < \
             (SELECT id FROM requests WHERE app_id = ?1 ORDER BY id DESC LIMIT 1 OFFSET ?2)",
            params![app, (max_per_app - 1).max(0)],
        )?;
    }
    tx.commit()?;
    Ok(n)
}

#[cfg(test)]
#[path = "app_requests_tests.rs"]
mod tests;
