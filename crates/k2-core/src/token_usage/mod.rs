//! Host token ledger (Usage v2).
//!
//! One sqlite file at `k2_home()/usage/tokens.sqlite`, with its own
//! connection: never [`crate::db::init_database`] and never the main-DB
//! migrations. Its schema version is `PRAGMA user_version`
//! ([`LEDGER_VERSION`]); opening an older ledger wipes it once and the next
//! scan refills it (Rosson Q6, 2026-10-16).
//!
//! Claude, Codex and Grok transcripts are read **incrementally**: per file
//! the ledger keeps the byte offset after the last complete line, the
//! inode, and the parser's carried state, so a grown file resumes where it
//! stopped (B12). Rows are max-merged, so a re-read never double counts.
//! Writers take a process lock per file, never per scan (B11); readers use
//! their own WAL connection and never wait on a scan.
//!
//! Scan roots include each wallet slot home (pinned Claude / Grok logins,
//! B3) and the daemon's `CLAUDE_CONFIG_DIR` / `CODEX_HOME` / `GROK_HOME`.
//! Every row carries the login (`account_id`) and the conversation
//! (`session_id`). No access tokens and no message content are stored.

pub mod parse;
pub mod query;

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, UNIX_EPOCH};

use chrono::Datelike;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

pub use parse::{Carry, CodexLimits, Draft};
pub use query::{
    parse_tz, report, report_host, turns, turns_host, CostSummary, DayTotals, Filter, GroupBy, GroupTotals, HarnessTotals,
    LoginTotals, ModelTotals, ReportOpts, TokenTotals, TurnCursor, TurnPage, TurnRow, UsageReport, WorkspaceTotals,
    DEFAULT_TURNS_LIMIT, MAX_TURNS_LIMIT,
};

/// The ledger schema this build writes. 1 = the 0.40.150 ledger (no
/// `user_version`), 2 = Usage v2 (uncached input, per-request Codex rows,
/// accounts, sessions, offsets).
pub const LEDGER_VERSION: i64 = 2;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS turns (
    turn_key TEXT PRIMARY KEY,
    workspace_path TEXT NOT NULL DEFAULT '',
    harness TEXT NOT NULL,
    model TEXT NOT NULL DEFAULT '',
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
    cache_write_tokens INTEGER NOT NULL DEFAULT 0,
    reasoning_tokens INTEGER NOT NULL DEFAULT 0,
    recorded_at TEXT NOT NULL DEFAULT '',
    recorded_ms INTEGER,
    account_id TEXT NOT NULL DEFAULT '',
    session_id TEXT NOT NULL DEFAULT '',
    provider_cost_usd REAL
);
CREATE INDEX IF NOT EXISTS turns_recorded ON turns(recorded_ms);
CREATE INDEX IF NOT EXISTS turns_workspace ON turns(workspace_path);
CREATE INDEX IF NOT EXISTS turns_account ON turns(account_id);
CREATE TABLE IF NOT EXISTS scanned_files (
    path TEXT PRIMARY KEY,
    size INTEGER NOT NULL,
    mtime_secs INTEGER NOT NULL,
    mtime_nanos INTEGER NOT NULL,
    inode INTEGER NOT NULL DEFAULT 0,
    offset INTEGER NOT NULL DEFAULT 0,
    carry TEXT NOT NULL DEFAULT '{}'
);
CREATE TABLE IF NOT EXISTS meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
";

/// Full-scan period named for the daemon timer (T17).
pub const FULL_SCAN_INTERVAL: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Source {
    Claude,
    Codex,
    Grok,
}

impl Source {
    pub fn harness(self) -> &'static str {
        match self {
            Source::Claude => "claude",
            Source::Codex => "codex",
            Source::Grok => "grok",
        }
    }
}

/// Whose transcripts a root holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Home {
    /// The tool's normal home: rows belong to the login live there.
    Default,
    /// A wallet slot home (`~/.k2/llm-accounts/<tool>/<id>`): that login.
    Slot(String),
}

/// One transcript tree to scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Root {
    pub source: Source,
    pub dir: PathBuf,
    pub home: Home,
}

/// Who a row belongs to. The daemon resolves through the wallet; tests
/// pass a fixed answer.
pub trait AccountResolver {
    /// The wallet login in the tool's default home now (`""` = none).
    fn live_owner(&self, tool: &str) -> String;
    /// The login a pinned spawn recorded for this conversation, if any.
    fn conversation_login(&self, tool: &str, conversation_id: &str) -> Option<String>;
}

/// No wallet: every row's `account_id` is `""`.
pub struct NoAccounts;

impl AccountResolver for NoAccounts {
    fn live_owner(&self, _tool: &str) -> String {
        String::new()
    }
    fn conversation_login(&self, _tool: &str, _conversation_id: &str) -> Option<String> {
        None
    }
}

/// The wallet in the shared main DB (when the process has one).
pub struct HostAccounts;

impl AccountResolver for HostAccounts {
    fn live_owner(&self, tool: &str) -> String {
        let Some(t) = crate::llm_accounts::Tool::parse(tool) else { return String::new() };
        let Some(db) = crate::db::try_shared() else { return String::new() };
        let conn = db.lock();
        crate::llm_accounts::live_owner(&conn, t).ok().flatten().unwrap_or_default()
    }
    fn conversation_login(&self, tool: &str, conversation_id: &str) -> Option<String> {
        if conversation_id.is_empty() {
            return None;
        }
        let db = crate::db::try_shared()?;
        let conn = db.lock();
        conn.query_row(
            "SELECT account_id FROM llm_session_logins \
             WHERE tool = ?1 AND conversation_id = ?2 AND account_id IS NOT NULL AND account_id != '' \
             ORDER BY recorded_at DESC LIMIT 1",
            params![tool, conversation_id],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .ok()
        .flatten()
    }
}

/// The newest Codex `rate_limits` snapshot a scan saw for one login.
#[derive(Debug, Clone, PartialEq)]
pub struct CodexLimitsSeen {
    pub account_id: String,
    pub recorded_ms: i64,
    pub snapshot: Value,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScanStats {
    pub files_seen: u64,
    pub files_read: u64,
    pub files_skipped: u64,
    /// Rows inserted or updated.
    pub rows: u64,
    /// Newest live Codex snapshot per login (Q5), oldest first.
    pub codex_limits: Vec<CodexLimitsSeen>,
}

/// `~/.k2/usage/tokens.sqlite`. Not `k2.db`, `k2so.db`, or `skin.db`.
pub fn ledger_path() -> PathBuf {
    crate::paths::k2_home().join("usage").join("tokens.sqlite")
}

/// Canonicalize the same way [`crate::chat_history::claude_project_hash`]
/// does before it rewrites characters: existing paths go through
/// `std::fs::canonicalize`, missing paths stay literal.
pub fn canonical_workspace(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    dunce::canonicalize(&trimmed)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| trimmed.trim_end_matches('/').to_string())
}

pub fn is_sentinel_workspace(path: &str) -> bool {
    path == "_orphan" || path == "_broadcast"
}

fn store_workspace(path: &str, memo: &mut HashMap<String, String>) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() || is_sentinel_workspace(trimmed) {
        return trimmed.to_string();
    }
    if let Some(hit) = memo.get(trimmed) {
        return hit.clone();
    }
    let canon = canonical_workspace(trimmed);
    memo.insert(trimmed.to_string(), canon.clone());
    canon
}

// ── Ledger file ─────────────────────────────────────────────────────

fn writer_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().map(|s| s.to_os_string()).unwrap_or_default();
    name.push(suffix);
    path.with_file_name(name)
}

fn restrict_mode(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if path.exists() {
            let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

fn restrict_ledger(path: &Path) {
    restrict_mode(path);
    restrict_mode(&sibling(path, "-wal"));
    restrict_mode(&sibling(path, "-shm"));
}

fn connect(path: &Path) -> Result<Connection, String> {
    let conn = Connection::open(path).map_err(|e| format!("open ledger: {e}"))?;
    conn.busy_timeout(Duration::from_secs(5)).map_err(|e| format!("ledger busy_timeout: {e}"))?;
    conn.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(())).map_err(|e| format!("ledger wal: {e}"))?;
    Ok(conn)
}

fn table_exists(conn: &Connection, name: &str) -> bool {
    conn.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1", params![name], |r| {
        r.get::<_, i64>(0)
    })
    .map(|n| n > 0)
    .unwrap_or(false)
}

/// Bring the schema to [`LEDGER_VERSION`]. A ledger below it is wiped once
/// (its rows were counted under the old rules) and logged; at the version
/// this only ensures the tables exist. Idempotent.
fn migrate(conn: &Connection) -> Result<(), String> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).map_err(|e| format!("ledger version: {e}"))?;
    if version >= LEDGER_VERSION {
        return conn.execute_batch(SCHEMA).map_err(|e| format!("ledger schema: {e}"));
    }
    let had_rows: i64 = if table_exists(conn, "turns") {
        conn.query_row("SELECT COUNT(*) FROM turns", [], |r| r.get(0)).unwrap_or(0)
    } else {
        0
    };
    let from = if version == 0 && table_exists(conn, "turns") { 1 } else { version };
    let tx = conn.unchecked_transaction().map_err(|e| format!("ledger rebuild: {e}"))?;
    tx.execute_batch("DROP TABLE IF EXISTS turns; DROP TABLE IF EXISTS scanned_files;")
        .map_err(|e| format!("ledger rebuild: {e}"))?;
    tx.execute_batch(SCHEMA).map_err(|e| format!("ledger schema: {e}"))?;
    if from > 0 {
        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        for (k, v) in [("rebuilt_at", now), ("rebuilt_from", from.to_string()), ("rebuilt_rows", had_rows.to_string())] {
            tx.execute(
                "INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![k, v],
            )
            .map_err(|e| format!("ledger meta: {e}"))?;
        }
    }
    tx.execute_batch(&format!("PRAGMA user_version = {LEDGER_VERSION};")).map_err(|e| format!("ledger version: {e}"))?;
    tx.commit().map_err(|e| format!("ledger rebuild commit: {e}"))?;
    if from > 0 {
        crate::log_debug!(
            "[token-usage] ledger v{from} -> v{LEDGER_VERSION}: wiped {had_rows} rows; a full rescan follows"
        );
    }
    Ok(())
}

/// Open for writing: creates the folder and file owner-only and migrates.
fn open_writer(path: &Path) -> Result<Connection, String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("usage dir: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
        }
    }
    if let Err(e) = crate::private_home::prepare_private_db_file(path) {
        crate::log_debug!("[token-usage] WARN create {} owner-only: {e}", path.display());
    }
    let conn = connect(path)?;
    {
        let _g = writer_lock();
        migrate(&conn)?;
    }
    restrict_ledger(path);
    Ok(conn)
}

/// Open for a report. Readers never take the scan lock (WAL); a missing
/// or old ledger is created / migrated first.
pub(crate) fn open_reader(path: &Path) -> Result<Connection, String> {
    if !path.is_file() {
        drop(open_writer(path)?);
    }
    let conn = connect(path)?;
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).map_err(|e| format!("ledger version: {e}"))?;
    if version < LEDGER_VERSION {
        drop(conn);
        drop(open_writer(path)?);
        return connect(path);
    }
    Ok(conn)
}

/// The rebuild record (`rebuilt_at`, `rebuilt_from`, `rebuilt_rows`).
pub fn ledger_meta(path: &Path) -> Result<HashMap<String, String>, String> {
    let conn = open_reader(path)?;
    let mut stmt = conn.prepare("SELECT key, value FROM meta").map_err(|e| e.to_string())?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).map_err(|e| e.to_string())?;
    let mut out = HashMap::new();
    for row in rows {
        let (k, v) = row.map_err(|e| e.to_string())?;
        out.insert(k, v);
    }
    Ok(out)
}

// ── Times ───────────────────────────────────────────────────────────

fn is_ymd_prefix(bytes: &[u8]) -> bool {
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[..4].iter().all(|b| b.is_ascii_digit())
        && bytes[5..7].iter().all(|b| b.is_ascii_digit())
        && bytes[8..10].iter().all(|b| b.is_ascii_digit())
}

/// Unix milliseconds for a transcript timestamp, or `None` when it cannot
/// be placed. Accepts RFC3339, all-digit unix seconds (`>= 1e9`) or
/// milliseconds (`>= 1e12`), and a bare `YYYY-MM-DD` (UTC midnight).
/// Anything else (the fixture stamp `10`) is `None`. Never errors.
pub fn epoch_ms(raw: &str) -> Option<i64> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    if s.bytes().all(|b| b.is_ascii_digit()) {
        let n: i64 = s.parse().ok()?;
        if n >= 1_000_000_000_000 {
            return Some(n);
        }
        if n >= 1_000_000_000 {
            return Some(n * 1000);
        }
        return None;
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(dt.timestamp_millis());
    }
    let bytes = s.as_bytes();
    if bytes.len() >= 10 && is_ymd_prefix(&bytes[..10]) {
        let date = chrono::NaiveDate::parse_from_str(&s[..10], "%Y-%m-%d").ok()?;
        return Some(date.and_hms_opt(0, 0, 0)?.and_utc().timestamp_millis());
    }
    None
}

/// A `since` / `until` value: RFC3339, unix ms or s, `YYYY-MM-DD` (UTC),
/// or a relative `7d` / `24h` / `90m` / `30s` back from `now_ms`.
pub fn parse_time_bound(raw: &str, now_ms: i64) -> Result<i64, String> {
    let s = raw.trim();
    if s.is_empty() {
        return Err("empty time".into());
    }
    let (num, unit) = s.split_at(s.len() - 1);
    if !num.is_empty() && num.bytes().all(|b| b.is_ascii_digit()) {
        let mult: Option<i64> = match unit {
            "d" => Some(86_400_000),
            "h" => Some(3_600_000),
            "m" => Some(60_000),
            "s" => Some(1000),
            "w" => Some(7 * 86_400_000),
            _ => None,
        };
        if let Some(mult) = mult {
            let n: i64 = num.parse().map_err(|_| format!("bad time {s:?}"))?;
            return Ok(now_ms.saturating_sub(n.saturating_mul(mult)));
        }
    }
    epoch_ms(s).ok_or_else(|| format!("bad time {s:?} (RFC3339, unix ms, YYYY-MM-DD, or 7d / 24h / 90m)"))
}

// ── Roots ───────────────────────────────────────────────────────────

fn forbidden_component(path: &Path) -> bool {
    path.components().any(|c| {
        let s = c.as_os_str().to_string_lossy();
        s == "memtrace" || s == "logs"
    })
}

fn walk_files(dir: &Path, out: &mut Vec<PathBuf>, want: &impl Fn(&Path) -> bool) {
    if forbidden_component(dir) || !dir.is_dir() {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else { continue };
        // Never follow links: a Codex slot's `sessions` (and shared Claude
        // settings) link back to the normal home and must count once.
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            walk_files(&path, out, want);
        } else if meta.is_file() && want(&path) {
            out.push(path);
        }
    }
}

fn is_jsonl(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some("jsonl")
}

fn is_history_jsonl(path: &Path) -> bool {
    path.file_name().and_then(|s| s.to_str()) == Some("history.jsonl")
}

fn root_files(root: &Root, dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    match root.source {
        Source::Claude | Source::Codex => walk_files(dir, &mut out, &|p| is_jsonl(p) && !is_history_jsonl(p)),
        Source::Grok => walk_files(dir, &mut out, &|p| p.file_name().and_then(|s| s.to_str()) == Some("updates.jsonl")),
    }
    out.sort();
    out
}

fn push_root(out: &mut Vec<Root>, root: Root) {
    if root.dir.as_os_str().is_empty() {
        return;
    }
    let key = dunce::canonicalize(&root.dir).unwrap_or_else(|_| root.dir.clone());
    if let Some(existing) = out.iter_mut().find(|r| dunce::canonicalize(&r.dir).unwrap_or_else(|_| r.dir.clone()) == key) {
        // A slot wins over "default" for the same folder.
        if matches!(root.home, Home::Slot(_)) {
            existing.home = root.home;
        }
        return;
    }
    out.push(root);
}

/// Every transcript tree on this host: the default homes, the daemon's
/// `CLAUDE_CONFIG_DIR` / `CODEX_HOME` / `GROK_HOME`, and each wallet slot
/// home for Claude and Grok (B3). Codex slots link `sessions/` to the
/// normal home, so they are not separate roots.
pub fn host_roots() -> Vec<Root> {
    use crate::llm_accounts::{store, Tool};
    let mut out = Vec::new();
    let home = crate::paths::home_dir().unwrap_or_default();
    let defaults = [
        (Source::Claude, Tool::Claude, home.join(".claude"), "projects"),
        (Source::Codex, Tool::Codex, home.join(".codex"), "sessions"),
        (Source::Grok, Tool::Grok, home.join(".grok"), "sessions"),
    ];
    for (source, tool, fallback, sub) in defaults {
        let (live, _) = store::live_home(tool);
        push_root(&mut out, Root { source, dir: live.join(sub), home: Home::Default });
        push_root(&mut out, Root { source, dir: fallback.join(sub), home: Home::Default });
    }
    for (source, tool, sub) in [(Source::Claude, Tool::Claude, "projects"), (Source::Grok, Tool::Grok, "sessions")] {
        let Ok(entries) = fs::read_dir(store::tool_dir(tool)) else { continue };
        let mut ids: Vec<String> = entries
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .filter_map(|e| e.file_name().to_str().map(str::to_string))
            .filter(|id| !id.starts_with('.'))
            .collect();
        ids.sort();
        for id in ids {
            push_root(&mut out, Root { source, dir: store::slot_dir(tool, &id).join(sub), home: Home::Slot(id) });
        }
    }
    out
}

// ── Ingest ──────────────────────────────────────────────────────────

struct Stamp {
    size: i64,
    secs: i64,
    nanos: i64,
    inode: i64,
}

fn stamp_of(meta: &fs::Metadata) -> Stamp {
    let size = i64::try_from(meta.len()).unwrap_or(i64::MAX);
    let modified = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok());
    let (secs, nanos) = modified.map(|d| (d.as_secs() as i64, d.subsec_nanos() as i64)).unwrap_or((0, 0));
    #[cfg(unix)]
    let inode = {
        use std::os::unix::fs::MetadataExt;
        meta.ino() as i64
    };
    #[cfg(not(unix))]
    let inode = 0;
    Stamp { size, secs, nanos, inode }
}

struct Scanned {
    size: i64,
    secs: i64,
    nanos: i64,
    inode: i64,
    offset: i64,
    carry: String,
}

fn load_scanned(conn: &Connection, path: &str) -> Option<Scanned> {
    conn.query_row(
        "SELECT size, mtime_secs, mtime_nanos, inode, offset, carry FROM scanned_files WHERE path = ?1",
        params![path],
        |r| {
            Ok(Scanned {
                size: r.get(0)?,
                secs: r.get(1)?,
                nanos: r.get(2)?,
                inode: r.get(3)?,
                offset: r.get(4)?,
                carry: r.get(5)?,
            })
        },
    )
    .optional()
    .ok()
    .flatten()
}

fn mark_scanned(conn: &Connection, path: &str, st: &Stamp, offset: i64, carry: &Carry) -> Result<(), String> {
    let carry = serde_json::to_string(carry).map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO scanned_files (path, size, mtime_secs, mtime_nanos, inode, offset, carry)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(path) DO UPDATE SET
            size = excluded.size, mtime_secs = excluded.mtime_secs, mtime_nanos = excluded.mtime_nanos,
            inode = excluded.inode, offset = excluded.offset, carry = excluded.carry",
        params![path, st.size, st.secs, st.nanos, st.inode, offset, carry],
    )
    .map_err(|e| format!("ledger stamp: {e}"))?;
    Ok(())
}

/// Insert a request, or merge a copy of one (P6, B10): keep the first
/// workspace, time, login and session; keep the max of every count.
fn upsert(conn: &Connection, harness: &str, d: &Draft, account: &str) -> Result<(), String> {
    conn.execute(
        "INSERT INTO turns (
            turn_key, workspace_path, harness, model,
            input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens,
            recorded_at, recorded_ms, account_id, session_id, provider_cost_usd
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
         ON CONFLICT(turn_key) DO UPDATE SET
            workspace_path = CASE WHEN turns.workspace_path = '' THEN excluded.workspace_path ELSE turns.workspace_path END,
            model = CASE WHEN turns.model = '' THEN excluded.model ELSE turns.model END,
            input_tokens = MAX(turns.input_tokens, excluded.input_tokens),
            output_tokens = MAX(turns.output_tokens, excluded.output_tokens),
            cache_read_tokens = MAX(turns.cache_read_tokens, excluded.cache_read_tokens),
            cache_write_tokens = MAX(turns.cache_write_tokens, excluded.cache_write_tokens),
            reasoning_tokens = MAX(turns.reasoning_tokens, excluded.reasoning_tokens),
            recorded_at = CASE WHEN turns.recorded_ms IS NULL AND excluded.recorded_ms IS NOT NULL
                               THEN excluded.recorded_at ELSE turns.recorded_at END,
            recorded_ms = COALESCE(turns.recorded_ms, excluded.recorded_ms),
            account_id = CASE WHEN turns.account_id = '' THEN excluded.account_id ELSE turns.account_id END,
            session_id = CASE WHEN turns.session_id = '' THEN excluded.session_id ELSE turns.session_id END,
            provider_cost_usd = CASE
                WHEN turns.provider_cost_usd IS NULL THEN excluded.provider_cost_usd
                WHEN excluded.provider_cost_usd IS NULL THEN turns.provider_cost_usd
                ELSE MAX(turns.provider_cost_usd, excluded.provider_cost_usd) END",
        params![
            d.turn_key,
            d.workspace,
            harness,
            d.model,
            d.input,
            d.output,
            d.cache_read,
            d.cache_write,
            d.reasoning,
            d.recorded_at,
            d.recorded_ms,
            account,
            d.session_id,
            d.provider_cost_usd,
        ],
    )
    .map_err(|e| format!("ledger upsert: {e}"))?;
    Ok(())
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3]) {
                if let Ok(v) = u8::from_str_radix(hex, 16) {
                    out.push(v);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn percent_encode_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// A Grok session's cwd: `summary.json` `.info.cwd`, else the decoded
/// session-folder parent. `current_model_id` never stamps turns.
fn grok_workspace(updates: &Path) -> String {
    if let Some(summary) = updates.parent().map(|p| p.join("summary.json")) {
        if let Ok(text) = fs::read_to_string(&summary) {
            if let Ok(v) = serde_json::from_str::<Value>(&text) {
                if let Some(cwd) = v.pointer("/info/cwd").and_then(|c| c.as_str()).filter(|s| !s.is_empty()) {
                    return cwd.to_string();
                }
            }
        }
    }
    let encoded = updates
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let decoded = percent_decode(&encoded);
    if decoded.starts_with('/') {
        decoded
    } else {
        String::new()
    }
}

fn initial_carry(root: &Root, path: &Path) -> Carry {
    match root.source {
        Source::Grok => Carry {
            cwd: grok_workspace(path),
            session_id: path
                .parent()
                .and_then(|p| p.file_name())
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
            model: String::new(),
        },
        _ => Carry::default(),
    }
}

/// Per-scan memo: canonical paths and resolved logins.
struct ScanCtx<'a> {
    accounts: &'a dyn AccountResolver,
    canon: HashMap<String, String>,
    owners: HashMap<&'static str, String>,
    codex_limits: HashMap<String, CodexLimitsSeen>,
}

impl ScanCtx<'_> {
    fn owner(&mut self, source: Source) -> String {
        let h = source.harness();
        if let Some(o) = self.owners.get(h) {
            return o.clone();
        }
        let o = self.accounts.live_owner(h);
        self.owners.insert(h, o.clone());
        o
    }
}

/// Read one file from its stored offset; returns whether it was read.
fn ingest_file(conn: &Connection, root: &Root, path: &Path, ctx: &mut ScanCtx<'_>, stats: &mut ScanStats) -> Result<bool, String> {
    let Ok(meta) = fs::metadata(path) else { return Ok(false) };
    let st = stamp_of(&meta);
    let path_s = path.to_string_lossy().into_owned();
    let _guard = writer_lock();
    let prev = load_scanned(conn, &path_s);
    if let Some(p) = &prev {
        if p.size == st.size && p.secs == st.secs && p.nanos == st.nanos && p.inode == st.inode {
            stats.files_skipped += 1;
            return Ok(false);
        }
    }
    // Resume a grown file at its offset; a replaced or shrunk file is read
    // again from the top (the max-merge makes that harmless).
    let (start, mut carry) = match prev {
        Some(p) if p.inode == st.inode && st.size >= p.offset && p.offset > 0 => {
            (p.offset, serde_json::from_str::<Carry>(&p.carry).unwrap_or_else(|_| initial_carry(root, path)))
        }
        _ => (0, initial_carry(root, path)),
    };
    let file_session = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let mut file = File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    file.seek(SeekFrom::Start(start as u64)).map_err(|e| format!("seek {}: {e}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut pos = start;
    let mut buf: Vec<u8> = Vec::new();
    let mut codex_account: Option<(String, String)> = None;
    conn.execute_batch("BEGIN IMMEDIATE").map_err(|e| format!("ledger begin: {e}"))?;
    let body = (|| -> Result<(), String> {
        loop {
            buf.clear();
            let n = reader.read_until(b'\n', &mut buf).map_err(|e| format!("read {}: {e}", path.display()))?;
            if n == 0 || buf.last() != Some(&b'\n') {
                // EOF, or a line still being written: stop before it.
                break;
            }
            let line_at = pos;
            pos += n as i64;
            let text = String::from_utf8_lossy(&buf);
            let line = text.trim();
            if line.is_empty() {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
            let mut drafts: Vec<Draft> = Vec::new();
            match root.source {
                Source::Claude => drafts.extend(parse::claude(&v, &mut carry, &file_session)),
                Source::Grok => drafts.extend(parse::grok(&v, &carry)),
                Source::Codex => {
                    let (d, limits) = parse::codex(&v, &mut carry, line_at as u64, &path_s);
                    drafts.extend(d);
                    if let Some(l) = limits {
                        let acct = codex_login(root, &carry.session_id, &mut codex_account, ctx);
                        let newer = ctx.codex_limits.get(&acct).map_or(true, |seen| l.recorded_ms >= seen.recorded_ms);
                        if newer {
                            ctx.codex_limits.insert(
                                acct.clone(),
                                CodexLimitsSeen { account_id: acct, recorded_ms: l.recorded_ms, snapshot: l.snapshot },
                            );
                        }
                    }
                }
            }
            for mut d in drafts {
                d.workspace = store_workspace(&d.workspace, &mut ctx.canon);
                let account = match (&root.home, root.source) {
                    (Home::Slot(id), _) => id.clone(),
                    (Home::Default, Source::Codex) => codex_login(root, &d.session_id, &mut codex_account, ctx),
                    (Home::Default, s) => ctx.owner(s),
                };
                upsert(conn, root.source.harness(), &d, &account)?;
                stats.rows += 1;
            }
        }
        mark_scanned(conn, &path_s, &st, pos, &carry)
    })();
    if let Err(e) = body {
        let _ = conn.execute_batch("ROLLBACK");
        return Err(e);
    }
    conn.execute_batch("COMMIT").map_err(|e| format!("ledger commit: {e}"))?;
    Ok(true)
}

/// A Codex conversation's login: its pinned-spawn record, else the live
/// login. Memoized per file.
fn codex_login(root: &Root, session_id: &str, memo: &mut Option<(String, String)>, ctx: &mut ScanCtx<'_>) -> String {
    if let Home::Slot(id) = &root.home {
        return id.clone();
    }
    if let Some((sid, acct)) = memo {
        if sid == session_id {
            return acct.clone();
        }
    }
    let acct = ctx.accounts.conversation_login("codex", session_id).unwrap_or_else(|| ctx.owner(Source::Codex));
    *memo = Some((session_id.to_string(), acct.clone()));
    acct
}

/// Scan explicit files (grouped by root). Missing files are skipped.
fn scan_files(ledger: &Path, items: &[(Root, PathBuf)], accounts: &dyn AccountResolver) -> Result<ScanStats, String> {
    let conn = open_writer(ledger)?;
    let mut ctx = ScanCtx { accounts, canon: HashMap::new(), owners: HashMap::new(), codex_limits: HashMap::new() };
    let mut stats = ScanStats::default();
    for (root, path) in items {
        stats.files_seen += 1;
        if ingest_file(&conn, root, path, &mut ctx, &mut stats)? {
            stats.files_read += 1;
        }
    }
    drop(conn);
    restrict_ledger(ledger);
    let mut limits: Vec<CodexLimitsSeen> = ctx.codex_limits.into_values().collect();
    limits.sort_by_key(|l| l.recorded_ms);
    stats.codex_limits = limits;
    Ok(stats)
}

/// Scan every file under `roots`. Missing roots are empty, not an error.
pub fn scan(ledger: &Path, roots: &[Root], accounts: &dyn AccountResolver) -> Result<ScanStats, String> {
    let mut items = Vec::new();
    for root in roots {
        for path in root_files(root, &root.dir) {
            items.push((root.clone(), path));
        }
    }
    scan_files(ledger, &items, accounts)
}

/// Test/legacy helper: the three default-home roots.
pub fn scan_roots(ledger: &Path, claude_projects: &Path, codex_sessions: &Path, grok_sessions: &Path) -> Result<ScanStats, String> {
    let roots = [
        Root { source: Source::Claude, dir: claude_projects.to_path_buf(), home: Home::Default },
        Root { source: Source::Codex, dir: codex_sessions.to_path_buf(), home: Home::Default },
        Root { source: Source::Grok, dir: grok_sessions.to_path_buf(), home: Home::Default },
    ];
    scan(ledger, &roots, &NoAccounts)
}

pub fn scan_host() -> Result<ScanStats, String> {
    scan(&ledger_path(), &host_roots(), &HostAccounts)
}

fn recent_day_dirs(root: &Path) -> Vec<PathBuf> {
    let today = chrono::Local::now().date_naive();
    let mut days = vec![today];
    if let Some(prev) = today.checked_sub_days(chrono::Days::new(1)) {
        days.push(prev);
    }
    days.into_iter()
        .map(|day| root.join(format!("{:04}", day.year())).join(format!("{:02}", day.month())).join(format!("{:02}", day.day())))
        .collect()
}

fn codex_file_cwd(path: &Path) -> Option<String> {
    let file = File::open(path).ok()?;
    for (i, line) in BufReader::new(file).lines().enumerate() {
        if i > 80 {
            break;
        }
        let line = line.ok()?;
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else { continue };
        let kind = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if kind == "session_meta" || kind == "turn_context" {
            if let Some(cwd) = v.pointer("/payload/cwd").and_then(|c| c.as_str()).filter(|s| !s.is_empty()) {
                return Some(canonical_workspace(cwd));
            }
        }
    }
    None
}

fn within(path: &str, ws: &str) -> bool {
    path == ws || (path.starts_with(ws) && path.as_bytes().get(ws.len()) == Some(&b'/'))
}

/// Idle scan for one workspace: its Claude project folder and Grok session
/// folder in every root (slots included), and today's / yesterday's Codex
/// rollouts whose cwd is in it. Not a walk of every tree.
pub fn scan_workspace(ledger: &Path, roots: &[Root], cwd: &str, accounts: &dyn AccountResolver) -> Result<ScanStats, String> {
    let canon = canonical_workspace(cwd);
    let mut items: Vec<(Root, PathBuf)> = Vec::new();
    for root in roots {
        match root.source {
            Source::Claude => {
                for path in [cwd.to_string(), canon.clone()] {
                    let dir = root.dir.join(crate::chat_history::claude_project_hash(&path));
                    items.extend(root_files(root, &dir).into_iter().map(|p| (root.clone(), p)));
                }
            }
            Source::Grok => {
                for path in [cwd.to_string(), canon.clone()] {
                    let dir = root.dir.join(percent_encode_path(&path));
                    items.extend(root_files(root, &dir).into_iter().map(|p| (root.clone(), p)));
                }
            }
            Source::Codex => {
                for day in recent_day_dirs(&root.dir) {
                    for p in root_files(root, &day) {
                        if codex_file_cwd(&p).is_some_and(|c| within(&c, &canon)) {
                            items.push((root.clone(), p));
                        }
                    }
                }
            }
        }
    }
    items.sort_by(|a, b| a.1.cmp(&b.1));
    items.dedup_by(|a, b| a.1 == b.1);
    scan_files(ledger, &items, accounts)
}

pub fn scan_workspace_host(cwd: &str) -> Result<ScanStats, String> {
    scan_workspace(&ledger_path(), &host_roots(), cwd, &HostAccounts)
}

#[cfg(test)]
mod tests;
