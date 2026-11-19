//! The k5 CLI's usage counters, daemon side (R1 slice 10).
//!
//! The CLI says three things about a call in request headers:
//!
//! - `X-K5-Cli-Via: fallthrough` (exactly) with `X-K5-Cli-Noun: <noun>`:
//!   the native `k5` binary handed this call to the shell CLI. Counted per
//!   noun (`^[a-z][a-z0-9-]{0,31}$`, else `other`; at most 256 nouns).
//! - `X-K5-Would-Refuse: 1` (exactly): an owner-token call from inside an
//!   agent session, which a later release refuses. Counted per workspace,
//!   and logged once per workspace per boot.
//! - `X-K5-Owner-Source: disk` (exactly): an owner-token call that read the
//!   token from disk. Counted per workspace.
//!
//! A header is a client's claim. It is read only after the request's own
//! credential checked out (an unauthenticated request counts nothing), and
//! it never grants or denies anything. Only the keys are kept: no
//! arguments, paths, pids or tool names.
//!
//! Counts live in memory and reach the table (`k2_core::cli_usage`,
//! migration 0308) every [`FLUSH_EVERY`], at shutdown, and when the owner
//! reads them (`GET /cli/cli-usage`). Nothing is written on a request.
//! `/boot-status`'s full view shows the totals (`cliUsage`) from memory.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use std::time::Duration;

use k2_core::cli_usage::{self as store, Row, FALLTHROUGH, MAX_NOUNS, OTHER, OWNER_FROM_DISK, WOULD_REFUSE};
use rusqlite::Connection;

use crate::cli_response::CliResponse;

/// The owner's read of every row.
pub const ROUTE: &str = "/cli/cli-usage";
/// How often the counts in memory are written to the table.
pub const FLUSH_EVERY: Duration = Duration::from_secs(60);
/// The response header line on every `/cli/*` answer to the CLI: this
/// daemon's version, the one `/boot-status` reports.
pub const SERVER_VERSION_HEADER: &str = concat!("X-K5-Server-Version: ", env!("CARGO_PKG_VERSION"), "\r\n");

/// What one request's `X-K5-*` headers claim.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Claims {
    /// The noun key, when `X-K5-Cli-Via` is exactly `fallthrough`.
    pub fallthrough: Option<String>,
    pub would_refuse: bool,
    pub owner_from_disk: bool,
}

impl Claims {
    /// The claims in a request head; `None` when it makes none (the cheap
    /// case: no `X-K5-` header at all is one scan of the head).
    pub fn from_head(head: &str) -> Option<Claims> {
        // Headers only: body bytes that arrived with the head never count.
        let head = crate::routes::http::request_head(head);
        if !head.as_bytes().windows(5).any(|w| w.eq_ignore_ascii_case(b"x-k5-")) {
            return None;
        }
        let h = |name| crate::routes::http::extract_header(head, name);
        let claims = Claims {
            fallthrough: (h("x-k5-cli-via") == Some("fallthrough"))
                .then(|| store::noun_key(h("x-k5-cli-noun")).to_string()),
            would_refuse: h("x-k5-would-refuse") == Some("1"),
            owner_from_disk: h("x-k5-owner-source") == Some("disk"),
        };
        (claims != Claims::default()).then_some(claims)
    }

    fn per_workspace(&self) -> bool {
        self.would_refuse || self.owner_from_disk
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Tally {
    count: i64,
    first_at: i64,
    last_at: i64,
}

type Key = (&'static str, String);

struct Inner {
    /// Counted since the last flush.
    pending: BTreeMap<Key, Tally>,
    /// The table's counts at the last flush (`/boot-status` reads these and
    /// `pending`, never the database).
    stored: BTreeMap<Key, i64>,
    /// Workspaces whose would-refuse line was logged this boot.
    logged: BTreeSet<String>,
}

impl Inner {
    fn holds(&self, kind: &'static str, key: &str) -> bool {
        let k = (kind, key.to_string());
        self.pending.contains_key(&k) || self.stored.contains_key(&k)
    }

    /// Distinct fallthrough nouns held, stored or pending (not `other`).
    fn nouns(&self) -> usize {
        let noun = |k: &&Key| k.0 == FALLTHROUGH && k.1 != OTHER;
        self.stored.keys().filter(noun).count()
            + self.pending.keys().filter(noun).filter(|k| !self.stored.contains_key(*k)).count()
    }
}

/// The counts in memory. One per daemon ([`COUNTER`]); tests make their own.
pub struct Counter {
    inner: Mutex<Inner>,
}

impl Default for Counter {
    fn default() -> Self {
        Self::new()
    }
}

impl Counter {
    pub const fn new() -> Self {
        Counter { inner: Mutex::new(Inner { pending: BTreeMap::new(), stored: BTreeMap::new(), logged: BTreeSet::new() }) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Count one call. A fallthrough noun not seen yet, while [`MAX_NOUNS`]
    /// nouns are already held (stored or pending), counts as `other`.
    fn add(&self, kind: &'static str, key: &str, now: i64) {
        let mut g = self.lock();
        let inner: &mut Inner = &mut g;
        let past_cap = kind == FALLTHROUGH && key != OTHER && !inner.holds(kind, key) && inner.nouns() >= MAX_NOUNS;
        let key = if past_cap { OTHER } else { key };
        let t = inner.pending.entry((kind, key.to_string())).or_insert(Tally { count: 0, first_at: now, last_at: now });
        t.count += 1;
        t.first_at = t.first_at.min(now);
        t.last_at = t.last_at.max(now);
    }

    /// True the first time `workspace` is named here.
    fn first_would_refuse(&self, workspace: &str) -> bool {
        self.lock().logged.insert(workspace.to_string())
    }

    /// Write the pending counts to the table, then read its totals back.
    /// On a failed write the counts stay pending for the next flush.
    pub fn flush_to(&self, conn: &Connection) -> rusqlite::Result<()> {
        // Held across the write so a reader never sees counts in neither
        // place. Lock order is database, then this (the request path takes
        // the database first and lets go before counting).
        let mut g = self.lock();
        let batch: Vec<Row> = g
            .pending
            .iter()
            .map(|((kind, key), t)| Row {
                kind: kind.to_string(),
                key: key.clone(),
                count: t.count,
                first_at: t.first_at,
                last_at: t.last_at,
            })
            .collect();
        store::flush(conn, &batch)?;
        g.pending.clear();
        g.stored = store::list(conn)?
            .into_iter()
            .filter_map(|r| Some(((kind_of(&r.kind)?, r.key), r.count)))
            .collect();
        Ok(())
    }

    /// `/boot-status` `cliUsage`: fallthrough per noun, the other two as
    /// totals.
    pub fn boot_status_value(&self) -> serde_json::Value {
        let g = self.lock();
        let mut nouns: BTreeMap<String, i64> = BTreeMap::new();
        let (mut would_refuse, mut owner_from_disk) = (0i64, 0i64);
        let all = g.stored.iter().map(|(k, n)| (k, *n)).chain(g.pending.iter().map(|(k, t)| (k, t.count)));
        for ((kind, key), n) in all {
            match *kind {
                FALLTHROUGH => *nouns.entry(key.clone()).or_insert(0) += n,
                WOULD_REFUSE => would_refuse += n,
                OWNER_FROM_DISK => owner_from_disk += n,
                _ => {}
            }
        }
        serde_json::json!({ "fallthrough": nouns, "wouldRefuse": would_refuse, "ownerFromDisk": owner_from_disk })
    }
}

fn kind_of(kind: &str) -> Option<&'static str> {
    [FALLTHROUGH, WOULD_REFUSE, OWNER_FROM_DISK].into_iter().find(|k| *k == kind)
}

/// The daemon's counts.
pub static COUNTER: Counter = Counter::new();

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Count `claims`. `workspace` is the token the per-workspace kinds key on
/// (a passport's workspace id, or the request's `project=`); `resolve`
/// turns it into `(workspace id, a name for the log)`, `None` = `other`.
fn record_with(
    counter: &Counter,
    claims: &Claims,
    workspace: Option<&str>,
    now: i64,
    resolve: impl FnOnce(&str) -> Option<(String, String)>,
) {
    if let Some(noun) = &claims.fallthrough {
        counter.add(FALLTHROUGH, noun, now);
    }
    if !claims.per_workspace() {
        return;
    }
    let (key, label) = workspace
        .map(str::trim)
        .filter(|w| !w.is_empty())
        .and_then(resolve)
        .unwrap_or_else(|| (OTHER.to_string(), "an unknown workspace".to_string()));
    if claims.would_refuse {
        counter.add(WOULD_REFUSE, &key, now);
        if counter.first_would_refuse(&key) {
            k2_core::log_debug!("[cli-usage] would refuse: an owner-token call from inside an agent session in {label}");
        }
    }
    if claims.owner_from_disk {
        counter.add(OWNER_FROM_DISK, &key, now);
    }
}

/// A workspace token (id, path, handle…) to its id and a name for the log,
/// when it names one workspace. So per-workspace rows are at most the
/// workspace count plus `other`.
fn resolve_workspace(conn: &Connection, token: &str) -> Option<(String, String)> {
    use k2_core::workspace::handle::{resolve_workspace_token, WorkspaceTokenResolve};
    let WorkspaceTokenResolve::Found { path } = resolve_workspace_token(conn, token) else {
        return None;
    };
    conn.query_row(
        "SELECT id, COALESCE(NULLIF(TRIM(handle), ''), name) FROM projects WHERE path = ?1",
        [path],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .ok()
}

fn record(claims: &Claims, workspace: Option<&str>) {
    record_with(&COUNTER, claims, workspace, now(), |w| {
        let db = k2_core::db::shared();
        let conn = db.lock();
        resolve_workspace(&conn, w)
    });
}

/// Who a TCP request's credential is, for counting: `None` when it doesn't
/// authenticate (count nothing); else the workspace token to key on (a
/// passport's own workspace, else the request's `project=`).
fn tcp_caller(auth_query: &str, owner_token: &str) -> Option<Option<String>> {
    use crate::routes::http::{ct_eq_token, extract_token};
    let tok = extract_token(auth_query).map(str::trim).filter(|t| !t.is_empty())?;
    if ct_eq_token(tok, owner_token) || k2_core::connect_users::validate_session(tok).is_some() {
        let params = k2_core::agent_hooks::parse_query_params(&format!("?{auth_query}"));
        return Some(crate::routes::http::project_param(&params));
    }
    let hook = crate::session_token::validate_hook(tok)?;
    Some(Some(hook.principal.workspace_uuid))
}

/// The TCP door: called after the dispatcher's gates, with the request
/// head and its folded credential query (`effective_auth_query`). A count
/// only: it never gates anything.
pub async fn note_tcp(head: &str, auth_query: &str, owner_token: &str) {
    let Some(claims) = Claims::from_head(head) else { return };
    let (query, owner) = (auth_query.to_string(), owner_token.to_string());
    let _ = tokio::task::spawn_blocking(move || {
        if let Some(workspace) = tcp_caller(&query, &owner) {
            record(&claims, workspace.as_deref());
        }
    })
    .await;
}

/// The cell socket: called once the passport is authorized for this cell.
pub async fn note_cell(head: &str, workspace_uuid: &str) {
    let Some(claims) = Claims::from_head(head) else { return };
    let ws = workspace_uuid.to_string();
    let _ = tokio::task::spawn_blocking(move || record(&claims, Some(&ws))).await;
}

/// Write the daemon's pending counts to the table.
pub fn flush() -> rusqlite::Result<()> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    COUNTER.flush_to(&conn)
}

/// Flush now (which also loads the stored totals for `/boot-status`), then
/// every [`FLUSH_EVERY`]. The shutdown flush is `daemon_main`'s.
pub fn spawn_flush_loop() {
    let spawned = std::thread::Builder::new().name("k2-cli-usage".into()).spawn(|| loop {
        if let Err(e) = flush() {
            k2_core::log_debug!("[cli-usage] flush failed (kept for the next one): {e}");
        }
        std::thread::sleep(FLUSH_EVERY);
    });
    if let Err(e) = spawned {
        k2_core::log_debug!("[cli-usage] the flush thread did not start: {e}");
    }
}

/// `/boot-status` full view only.
pub fn boot_status_value() -> serde_json::Value {
    COUNTER.boot_status_value()
}

/// `GET /cli/cli-usage`: the owner (token, or an Owner login) reads every
/// row, after this boot's pending counts are written.
/// The TCP door's arm for [`ROUTE`] (`routes/dispatcher.rs`): the
/// request's body is drained, [`handle`] runs off the async threads, and
/// its answer goes back.
pub(crate) async fn serve_tcp(stream: &mut tokio::net::TcpStream, buf: &mut [u8], query: &str, owner_token: &str) {
    use tokio::io::AsyncReadExt;
    let _ = stream.read(buf).await;
    let (q, owner) = (query.to_string(), owner_token.to_string());
    let r = tokio::task::spawn_blocking(move || handle(&q, &owner))
        .await
        .unwrap_or_else(|e| CliResponse::internal_error(format!("worker join: {e}")));
    crate::routes::http::send_response(stream, r.status, r.content_type, &r.body).await;
}

pub fn handle(auth_query: &str, owner_token: &str) -> CliResponse {
    if crate::routes::http::owner_role_identity(auth_query, owner_token).is_none() {
        return CliResponse {
            status: "403 Forbidden",
            content_type: "application/json",
            body: r#"{"error":"only the owner can read the CLI usage counts"}"#.to_string(),
        };
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    if let Err(e) = COUNTER.flush_to(&conn) {
        return CliResponse::internal_error(format!("could not write the CLI usage counts: {e}"));
    }
    match store::list(&conn) {
        Ok(rows) => CliResponse::ok_json(serde_json::json!({ "rows": rows }).to_string()),
        Err(e) => CliResponse::internal_error(format!("could not read the CLI usage counts: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head(lines: &[&str]) -> String {
        let mut h = "GET /cli/projects/list?token=t HTTP/1.1\r\nHost: 127.0.0.1\r\n".to_string();
        for l in lines {
            h.push_str(l);
            h.push_str("\r\n");
        }
        h.push_str("\r\n");
        h
    }

    fn table() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(include_str!("../../k2-core/drizzle_sql/0308_cli_usage_counts.sql")).unwrap();
        c
    }

    fn claims(lines: &[&str]) -> Option<Claims> {
        Claims::from_head(&head(lines))
    }

    #[test]
    fn no_k5_header_claims_nothing() {
        assert_eq!(claims(&[]), None);
        assert_eq!(claims(&["X-K2-Tty: 1", "User-Agent: curl/8"]), None);
    }

    #[test]
    fn only_the_exact_values_count() {
        let via = |v: &str| claims(&[&format!("X-K5-Cli-Via: {v}"), "X-K5-Cli-Noun: msg"]);
        assert_eq!(via("fallthrough"), Some(Claims { fallthrough: Some("msg".into()), ..Claims::default() }));
        // Header names are case-insensitive; values are exact.
        assert_eq!(
            claims(&["x-k5-cli-via: fallthrough", "x-k5-cli-noun: msg"]),
            Some(Claims { fallthrough: Some("msg".into()), ..Claims::default() })
        );
        for v in ["native", "Fallthrough", "fallthrough, native", "fallthrough1"] {
            assert_eq!(via(v), None, "Via {v:?}");
        }
        let wr = |v: &str| claims(&[&format!("X-K5-Would-Refuse: {v}")]);
        assert_eq!(wr("1"), Some(Claims { would_refuse: true, ..Claims::default() }));
        for v in ["0", "true", "yes", "11", "1 1"] {
            assert_eq!(wr(v), None, "Would-Refuse {v:?}");
        }
        let os = |v: &str| claims(&[&format!("X-K5-Owner-Source: {v}")]);
        assert_eq!(os("disk"), Some(Claims { owner_from_disk: true, ..Claims::default() }));
        for v in ["Disk", "env", "keychain", "disk2"] {
            assert_eq!(os(v), None, "Owner-Source {v:?}");
        }
        // The noun alone is no claim.
        assert_eq!(claims(&["X-K5-Cli-Noun: msg"]), None);
        // All three at once.
        assert_eq!(
            claims(&["X-K5-Cli-Via: fallthrough", "X-K5-Cli-Noun: inbox", "X-K5-Would-Refuse: 1", "X-K5-Owner-Source: disk"]),
            Some(Claims { fallthrough: Some("inbox".into()), would_refuse: true, owner_from_disk: true })
        );
    }

    #[test]
    fn a_bad_or_missing_noun_counts_as_other() {
        let long = "x".repeat(33);
        for noun in [None, Some("Msg"), Some("msg_x"), Some("a/b"), Some(long.as_str())] {
            let mut lines = vec!["X-K5-Cli-Via: fallthrough".to_string()];
            if let Some(n) = noun {
                lines.push(format!("X-K5-Cli-Noun: {n}"));
            }
            let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
            assert_eq!(claims(&refs).and_then(|c| c.fallthrough).as_deref(), Some(OTHER), "{noun:?}");
        }
    }

    #[test]
    fn body_bytes_after_the_head_never_count() {
        let blob = format!("{}X-K5-Would-Refuse: 1\r\n", head(&["Content-Length: 21"]));
        assert_eq!(Claims::from_head(&blob), None);
    }

    #[test]
    fn per_workspace_kinds_key_on_the_resolved_workspace_or_other() {
        let c = Counter::new();
        let both = Claims { fallthrough: None, would_refuse: true, owner_from_disk: true };
        let ws = |w: &str| (w == "billing").then(|| ("ws-1".to_string(), "billing".to_string()));
        record_with(&c, &both, Some("billing"), 10, ws);
        record_with(&c, &both, Some("nowhere"), 11, ws);
        record_with(&c, &both, None, 12, |_| panic!("no token, no lookup"));
        record_with(&c, &both, Some("  "), 13, |_| panic!("a blank token, no lookup"));
        let g = c.lock();
        let get = |kind: &'static str, key: &str| g.pending.get(&(kind, key.to_string())).copied();
        assert_eq!(get(WOULD_REFUSE, "ws-1"), Some(Tally { count: 1, first_at: 10, last_at: 10 }));
        assert_eq!(get(WOULD_REFUSE, OTHER), Some(Tally { count: 3, first_at: 11, last_at: 13 }));
        assert_eq!(get(OWNER_FROM_DISK, "ws-1").map(|t| t.count), Some(1));
        assert_eq!(get(OWNER_FROM_DISK, OTHER).map(|t| t.count), Some(3));
        assert_eq!(g.pending.len(), 4, "nothing else: {:?}", g.pending);
        assert_eq!(g.logged.iter().cloned().collect::<Vec<_>>(), vec!["other".to_string(), "ws-1".to_string()]);
    }

    #[test]
    fn the_would_refuse_line_is_once_per_workspace() {
        let c = Counter::new();
        assert!(c.first_would_refuse("ws-1"));
        assert!(!c.first_would_refuse("ws-1"));
        assert!(c.first_would_refuse("ws-2"));
    }

    #[test]
    fn past_256_nouns_a_new_one_counts_as_other_in_memory_too() {
        let c = Counter::new();
        for i in 0..MAX_NOUNS {
            c.add(FALLTHROUGH, &format!("n{i}"), 1);
        }
        c.add(FALLTHROUGH, "late", 2);
        c.add(FALLTHROUGH, "n3", 3);
        let g = c.lock();
        assert!(!g.pending.contains_key(&(FALLTHROUGH, "late".to_string())));
        assert_eq!(g.pending.get(&(FALLTHROUGH, OTHER.to_string())).map(|t| t.count), Some(1));
        assert_eq!(g.pending.get(&(FALLTHROUGH, "n3".to_string())).map(|t| t.count), Some(2));
        assert_eq!(g.pending.len(), MAX_NOUNS + 1);
    }

    #[test]
    fn stored_nouns_count_toward_the_cap() {
        let conn = table();
        let c = Counter::new();
        for i in 0..MAX_NOUNS {
            c.add(FALLTHROUGH, &format!("n{i}"), 1);
        }
        c.flush_to(&conn).unwrap();
        // A fresh boot: the table holds 256 nouns, memory none.
        let fresh = Counter::new();
        fresh.flush_to(&conn).unwrap();
        fresh.add(FALLTHROUGH, "late", 5);
        fresh.add(FALLTHROUGH, "n9", 5);
        let g = fresh.lock();
        assert_eq!(g.pending.keys().map(|(_, k)| k.as_str()).collect::<Vec<_>>(), vec!["n9", OTHER]);
    }

    #[test]
    fn flush_writes_pending_counts_once_and_keeps_the_totals() {
        let conn = table();
        let c = Counter::new();
        c.add(FALLTHROUGH, "msg", 100);
        c.add(FALLTHROUGH, "msg", 160);
        c.add(WOULD_REFUSE, "ws-1", 120);
        c.add(OWNER_FROM_DISK, OTHER, 130);
        let before = c.boot_status_value();
        assert_eq!(before, serde_json::json!({"fallthrough": {"msg": 2}, "wouldRefuse": 1, "ownerFromDisk": 1}));
        c.flush_to(&conn).unwrap();
        assert!(c.lock().pending.is_empty(), "pending is written");
        assert_eq!(c.boot_status_value(), before, "the totals survive the flush");
        // A second flush with nothing pending writes nothing more.
        c.flush_to(&conn).unwrap();
        c.add(FALLTHROUGH, "msg", 200);
        c.flush_to(&conn).unwrap();
        let rows = store::list(&conn).unwrap();
        let row = |kind: &str, key: &str| rows.iter().find(|r| r.kind == kind && r.key == key).cloned();
        assert_eq!(
            row(FALLTHROUGH, "msg"),
            Some(Row { kind: FALLTHROUGH.into(), key: "msg".into(), count: 3, first_at: 100, last_at: 200 })
        );
        assert_eq!(row(WOULD_REFUSE, "ws-1").map(|r| r.count), Some(1));
        assert_eq!(row(OWNER_FROM_DISK, OTHER).map(|r| r.count), Some(1));
        assert_eq!(rows.len(), 3);
        assert_eq!(c.boot_status_value()["fallthrough"]["msg"], 3);
    }

    #[test]
    fn a_failed_flush_keeps_the_counts_pending() {
        let conn = Connection::open_in_memory().unwrap(); // no table
        let c = Counter::new();
        c.add(WOULD_REFUSE, "ws-1", 5);
        assert!(c.flush_to(&conn).is_err());
        assert_eq!(c.lock().pending.get(&(WOULD_REFUSE, "ws-1".to_string())).map(|t| t.count), Some(1));
        conn.execute_batch(include_str!("../../k2-core/drizzle_sql/0308_cli_usage_counts.sql")).unwrap();
        c.flush_to(&conn).unwrap();
        assert_eq!(store::list(&conn).unwrap().len(), 1);
    }

    #[test]
    fn the_version_header_is_the_boot_status_version() {
        assert_eq!(SERVER_VERSION_HEADER, format!("X-K5-Server-Version: {}\r\n", env!("CARGO_PKG_VERSION")));
    }
}
