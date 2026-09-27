//! Host token ledger.
//!
//! One sqlite file at `k2_home()/usage/tokens.sqlite`. Its own connection:
//! never [`crate::db::init_database`] and never [`crate::db::run_migrations`].
//! Claude, Codex, and Grok transcripts are streamed. A later scan skips a
//! file whose size and mtime are unchanged. No prices, no access tokens.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, UNIX_EPOCH};

use chrono::Datelike;
use rusqlite::{params, Connection};
use serde::Serialize;
use serde_json::Value;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS turns (
    turn_key TEXT PRIMARY KEY,
    workspace_path TEXT NOT NULL,
    harness TEXT NOT NULL,
    model TEXT NOT NULL,
    input_tokens INTEGER,
    output_tokens INTEGER,
    cache_read_tokens INTEGER,
    cache_write_tokens INTEGER,
    recorded_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS turns_workspace ON turns(workspace_path);
CREATE TABLE IF NOT EXISTS scanned_files (
    path TEXT PRIMARY KEY,
    size INTEGER NOT NULL,
    mtime_secs INTEGER NOT NULL,
    mtime_nanos INTEGER NOT NULL
);
";

/// Full-scan period named for the daemon timer (T17).
pub const FULL_SCAN_INTERVAL: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    Claude,
    Codex,
    Grok,
}

struct Found {
    path: PathBuf,
    source: Source,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, Default)]
pub struct TokenTotals {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub turns: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct HarnessTotals {
    pub harness: String,
    #[serde(flatten)]
    pub totals: TokenTotals,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ModelTotals {
    pub harness: String,
    pub model: String,
    #[serde(flatten)]
    pub totals: TokenTotals,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct WorkspaceTotals {
    pub path: String,
    pub outside: bool,
    #[serde(flatten)]
    pub totals: TokenTotals,
}

/// One UTC calendar day. Not a local-timezone shift of `recorded_at`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DayTotals {
    /// `YYYY-MM-DD`, UTC.
    pub day: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub turns: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct UsageReport {
    pub scope: String,
    pub workspace: Option<String>,
    pub total: TokenTotals,
    pub harnesses: Vec<HarnessTotals>,
    pub models: Vec<ModelTotals>,
    pub workspaces: Vec<WorkspaceTotals>,
    pub outside: TokenTotals,
    /// Ascending UTC days. Unparseable `recorded_at` values are omitted
    /// here and still included in `total`.
    pub days: Vec<DayTotals>,
}

/// One raw ledger turn, newest-ingested first. Unlike [`UsageReport`]
/// (aggregated), this is a single `turns` row for the live log. `rowid`
/// is the sqlite implicit rowid — a stable, monotonically increasing
/// keyset cursor (an `ON CONFLICT` update never changes it), so paging
/// by `rowid < before` is immune to the offset drift a `LIMIT/OFFSET`
/// scan would suffer as new rows arrive at the head.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TurnRow {
    pub rowid: i64,
    pub workspace_path: String,
    /// Not one of this daemon's known workspaces (or a sentinel).
    pub outside: bool,
    pub harness: String,
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    /// The raw ledger value, kept verbatim (mixed formats exist).
    pub recorded_at: String,
    /// `recorded_at` normalized to unix milliseconds when parseable
    /// (RFC3339, unix s/ms digits, or a bare `YYYY-MM-DD` at UTC
    /// midnight); `None` when it cannot be placed. Never errors.
    pub recorded_ms: Option<i64>,
}

/// A page of [`TurnRow`]s plus the keyset cursor for the next page.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TurnPage {
    pub rows: Vec<TurnRow>,
    /// Pass as `before` to fetch the next (older) page. `None` when the
    /// last page returned fewer rows than the limit (no more rows).
    pub next_cursor: Option<i64>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScanStats {
    pub files_seen: u64,
    pub files_read: u64,
    pub files_skipped: u64,
}

struct Counts {
    input: Option<i64>,
    output: Option<i64>,
    cache_read: Option<i64>,
    cache_write: Option<i64>,
}

struct Draft {
    turn_key: String,
    workspace: String,
    model: String,
    counts: Counts,
    recorded_at: String,
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
    fs::canonicalize(trimmed)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| trimmed.to_string())
}

pub fn is_sentinel_workspace(path: &str) -> bool {
    path == "_orphan" || path == "_broadcast"
}

fn store_workspace(path: &str) -> String {
    let trimmed = path.trim();
    if is_sentinel_workspace(trimmed) {
        return trimmed.to_string();
    }
    canonical_workspace(trimmed)
}

fn ledger_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|s| s.to_os_string())
        .unwrap_or_default();
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

fn open_ledger(path: &Path) -> Result<Connection, String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("usage dir: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
        }
    }
    let conn = Connection::open(path).map_err(|e| format!("open ledger: {e}"))?;
    conn.busy_timeout(Duration::from_secs(5))
        .map_err(|e| format!("ledger busy_timeout: {e}"))?;
    conn.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()))
        .map_err(|e| format!("ledger wal: {e}"))?;
    conn.execute_batch(SCHEMA)
        .map_err(|e| format!("ledger schema: {e}"))?;
    restrict_ledger(path);
    Ok(conn)
}

fn with_ledger<T>(
    path: &Path,
    f: impl FnOnce(&Connection) -> Result<T, String>,
) -> Result<T, String> {
    let _guard = ledger_lock();
    let result = {
        let conn = open_ledger(path)?;
        f(&conn)
    };
    // Chmod after the connection drops so a WAL file created on close
    // does not keep the process umask.
    restrict_ledger(path);
    result
}

fn json_i64(v: &Value) -> Option<i64> {
    if let Some(n) = v.as_i64() {
        return Some(n);
    }
    if let Some(u) = v.as_u64() {
        return i64::try_from(u).ok();
    }
    let f = v.as_f64()?;
    if f.is_finite() && f.fract() == 0.0 && (i64::MIN as f64..=i64::MAX as f64).contains(&f) {
        Some(f as i64)
    } else {
        None
    }
}

fn field_i64(obj: &Value, key: &str) -> Option<i64> {
    obj.get(key).and_then(json_i64)
}

fn counts_present(c: &Counts) -> bool {
    c.input.is_some() || c.output.is_some() || c.cache_read.is_some() || c.cache_write.is_some()
}

fn recorded_or_now(raw: &str) -> String {
    if raw.is_empty() {
        chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    } else {
        raw.to_string()
    }
}

fn timestamp_of(v: &Value) -> String {
    match v.get("timestamp") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

fn upsert(conn: &Connection, harness: &str, draft: &Draft) -> Result<(), String> {
    conn.execute(
        "INSERT INTO turns (
            turn_key, workspace_path, harness, model,
            input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
            recorded_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(turn_key) DO UPDATE SET
            workspace_path = excluded.workspace_path,
            harness = excluded.harness,
            model = excluded.model,
            input_tokens = excluded.input_tokens,
            output_tokens = excluded.output_tokens,
            cache_read_tokens = excluded.cache_read_tokens,
            cache_write_tokens = excluded.cache_write_tokens,
            recorded_at = excluded.recorded_at",
        params![
            draft.turn_key,
            draft.workspace,
            harness,
            draft.model,
            draft.counts.input,
            draft.counts.output,
            draft.counts.cache_read,
            draft.counts.cache_write,
            recorded_or_now(&draft.recorded_at),
        ],
    )
    .map_err(|e| format!("ledger upsert: {e}"))?;
    Ok(())
}

fn file_stamp(meta: &fs::Metadata) -> (i64, i64, i64) {
    let size = i64::try_from(meta.len()).unwrap_or(i64::MAX);
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok());
    let (secs, nanos) = match modified {
        Some(d) => (d.as_secs() as i64, d.subsec_nanos() as i64),
        None => (0, 0),
    };
    (size, secs, nanos)
}

fn unchanged(conn: &Connection, path: &str, size: i64, secs: i64, nanos: i64) -> bool {
    let row = conn.query_row(
        "SELECT size, mtime_secs, mtime_nanos FROM scanned_files WHERE path = ?1",
        params![path],
        |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
            ))
        },
    );
    match row {
        Ok((sz, s, n)) => sz == size && s == secs && n == nanos,
        Err(_) => false,
    }
}

fn mark_scanned(
    conn: &Connection,
    path: &str,
    size: i64,
    secs: i64,
    nanos: i64,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO scanned_files (path, size, mtime_secs, mtime_nanos)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(path) DO UPDATE SET
            size = excluded.size,
            mtime_secs = excluded.mtime_secs,
            mtime_nanos = excluded.mtime_nanos",
        params![path, size, secs, nanos],
    )
    .map_err(|e| format!("ledger stamp: {e}"))?;
    Ok(())
}

fn for_each_line(path: &Path, mut f: impl FnMut(&str) -> Result<(), String>) -> Result<(), String> {
    let file = File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut buf = String::new();
    loop {
        buf.clear();
        let n = reader
            .read_line(&mut buf)
            .map_err(|e| format!("read {}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        let line = buf.trim();
        if line.is_empty() {
            continue;
        }
        f(line)?;
    }
    Ok(())
}

fn claude_usage(v: &Value) -> Option<&Value> {
    let on_message = v.get("message").and_then(|m| m.get("usage"));
    if on_message.is_some_and(|u| {
        field_i64(u, "input_tokens").is_some()
            || field_i64(u, "output_tokens").is_some()
            || field_i64(u, "cache_read_input_tokens").is_some()
            || field_i64(u, "cache_creation_input_tokens").is_some()
    }) {
        return on_message;
    }
    let top = v.get("usage");
    if top.is_some_and(|u| {
        field_i64(u, "input_tokens").is_some()
            || field_i64(u, "output_tokens").is_some()
            || field_i64(u, "cache_read_input_tokens").is_some()
            || field_i64(u, "cache_creation_input_tokens").is_some()
    }) {
        return top;
    }
    None
}

fn claude_draft(v: &Value, carried_cwd: &mut Option<String>) -> Option<Draft> {
    let usage = claude_usage(v)?;
    let counts = Counts {
        input: field_i64(usage, "input_tokens"),
        output: field_i64(usage, "output_tokens"),
        cache_read: field_i64(usage, "cache_read_input_tokens"),
        cache_write: field_i64(usage, "cache_creation_input_tokens"),
    };
    if !counts_present(&counts) {
        return None;
    }
    // Provider message id only. A forked copy of the line is the same row.
    let turn_key = v
        .get("message")
        .and_then(|m| m.get("id"))
        .and_then(|id| id.as_str())
        .filter(|s| !s.is_empty())?
        .to_string();
    let model = v
        .get("message")
        .and_then(|m| m.get("model"))
        .and_then(|m| m.as_str())
        .unwrap_or("")
        .to_string();
    if let Some(cwd) = v
        .get("cwd")
        .and_then(|c| c.as_str())
        .filter(|s| !s.is_empty())
    {
        *carried_cwd = Some(cwd.to_string());
    }
    let workspace = carried_cwd.clone().unwrap_or_default();
    Some(Draft {
        turn_key,
        workspace: store_workspace(&workspace),
        model,
        counts,
        recorded_at: timestamp_of(v),
    })
}

fn ingest_claude(conn: &Connection, path: &Path) -> Result<(), String> {
    let mut carried: Option<String> = None;
    for_each_line(path, |line| {
        let v: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => return Ok(()),
        };
        if let Some(draft) = claude_draft(&v, &mut carried) {
            upsert(conn, "claude", &draft)?;
        }
        Ok(())
    })
}

struct CodexCarry {
    session_id: String,
    turn_id: String,
    model: String,
    cwd: String,
}

fn codex_draft(v: &Value, carry: &mut CodexCarry) -> Option<Draft> {
    let kind = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    let payload = v.get("payload");
    if kind == "session_meta" {
        if let Some(p) = payload {
            if carry.session_id.is_empty() {
                if let Some(id) = p.get("id").and_then(|s| s.as_str()) {
                    carry.session_id = id.to_string();
                }
            }
            if carry.cwd.is_empty() {
                if let Some(cwd) = p.get("cwd").and_then(|s| s.as_str()) {
                    carry.cwd = cwd.to_string();
                }
            }
        }
        return None;
    }
    if kind == "turn_context" {
        if let Some(p) = payload {
            if let Some(id) = p
                .get("turn_id")
                .and_then(|s| s.as_str())
                .filter(|s| !s.is_empty())
            {
                carry.turn_id = id.to_string();
            }
            if let Some(model) = p.get("model").and_then(|s| s.as_str()) {
                carry.model = model.to_string();
            }
            if let Some(cwd) = p
                .get("cwd")
                .and_then(|s| s.as_str())
                .filter(|s| !s.is_empty())
            {
                carry.cwd = cwd.to_string();
            }
        }
        return None;
    }
    if kind != "event_msg" {
        return None;
    }
    let payload = payload?;
    if payload.get("type").and_then(|t| t.as_str()) != Some("token_count") {
        return None;
    }
    let last = payload.pointer("/info/last_token_usage")?;
    if !last.is_object() {
        return None;
    }
    // Codex input already includes cache. Do not add cache read on top.
    // Reasoning stays out of output — that fold is Grok-only.
    let counts = Counts {
        input: field_i64(last, "input_tokens"),
        output: field_i64(last, "output_tokens"),
        cache_read: field_i64(last, "cached_input_tokens"),
        cache_write: None,
    };
    if !counts_present(&counts) {
        return None;
    }
    let turn_key = if !carry.turn_id.is_empty() {
        carry.turn_id.clone()
    } else if !carry.session_id.is_empty() {
        format!("{}:pending", carry.session_id)
    } else {
        return None;
    };
    Some(Draft {
        turn_key,
        workspace: store_workspace(&carry.cwd),
        model: carry.model.clone(),
        counts,
        recorded_at: timestamp_of(v),
    })
}

fn ingest_codex(conn: &Connection, path: &Path) -> Result<(), String> {
    let mut carry = CodexCarry {
        session_id: String::new(),
        turn_id: String::new(),
        model: String::new(),
        cwd: String::new(),
    };
    for_each_line(path, |line| {
        let v: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => return Ok(()),
        };
        if let Some(draft) = codex_draft(&v, &mut carry) {
            // Same turn_key each snapshot: the last cumulative write wins.
            upsert(conn, "codex", &draft)?;
        }
        Ok(())
    })
}

fn codex_file_cwd(path: &Path) -> Option<String> {
    let file = File::open(path).ok()?;
    let reader = BufReader::new(file);
    for (i, line) in reader.lines().enumerate() {
        if i > 80 {
            break;
        }
        let line = line.ok()?;
        let v: Value = match serde_json::from_str(line.trim()) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let kind = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if kind == "session_meta" || kind == "turn_context" {
            if let Some(cwd) = v
                .pointer("/payload/cwd")
                .and_then(|c| c.as_str())
                .filter(|s| !s.is_empty())
            {
                return Some(canonical_workspace(cwd));
            }
        }
    }
    None
}

fn grok_output(output: Option<i64>, reasoning: Option<i64>) -> Option<i64> {
    match (output, reasoning) {
        (Some(o), Some(r)) => Some(o.saturating_add(r)),
        (Some(o), None) => Some(o),
        (None, _) => None,
    }
}

fn grok_counts(obj: &Value) -> Counts {
    Counts {
        input: field_i64(obj, "inputTokens"),
        output: grok_output(
            field_i64(obj, "outputTokens"),
            field_i64(obj, "reasoningTokens"),
        ),
        cache_read: field_i64(obj, "cachedReadTokens"),
        cache_write: field_i64(obj, "cacheCreationTokens"),
    }
}

fn grok_drafts(v: &Value, workspace: &str) -> Vec<Draft> {
    let method = v.get("method").and_then(|m| m.as_str()).unwrap_or("");
    if method != "session/update" && method != "_x.ai/session/update" {
        return Vec::new();
    }
    let Some(update) = v.pointer("/params/update") else {
        return Vec::new();
    };
    let Some(usage) = update.get("usage").filter(|u| u.is_object()) else {
        return Vec::new();
    };
    let turn_id = update
        .get("prompt_id")
        .and_then(|s| s.as_str())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            v.pointer("/params/_meta/eventId")
                .and_then(|s| s.as_str())
                .filter(|s| !s.is_empty())
        });
    let Some(turn_id) = turn_id else {
        return Vec::new();
    };
    let recorded = timestamp_of(v);
    let ws = store_workspace(workspace);
    if let Some(models) = usage.get("modelUsage").and_then(|m| m.as_object()) {
        if !models.is_empty() {
            let mut out = Vec::new();
            for (model, body) in models {
                if !body.is_object() {
                    continue;
                }
                let counts = grok_counts(body);
                if !counts_present(&counts) {
                    continue;
                }
                out.push(Draft {
                    turn_key: format!("{turn_id}\t{model}"),
                    workspace: ws.clone(),
                    model: model.clone(),
                    counts,
                    recorded_at: recorded.clone(),
                });
            }
            return out;
        }
    }
    let model = update
        .get("model")
        .and_then(|s| s.as_str())
        .or_else(|| usage.get("model").and_then(|s| s.as_str()))
        .unwrap_or("")
        .to_string();
    let counts = grok_counts(usage);
    if !counts_present(&counts) {
        return Vec::new();
    }
    vec![Draft {
        turn_key: format!("{turn_id}\t{model}"),
        workspace: ws,
        model,
        counts,
        recorded_at: recorded,
    }]
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

fn grok_workspace(updates: &Path) -> String {
    if let Some(summary) = updates.parent().map(|p| p.join("summary.json")) {
        if let Ok(text) = fs::read_to_string(&summary) {
            if let Ok(v) = serde_json::from_str::<Value>(&text) {
                // `.info.cwd` only. `current_model_id` is the end-of-session
                // model and must not stamp turns.
                if let Some(cwd) = v
                    .pointer("/info/cwd")
                    .and_then(|c| c.as_str())
                    .filter(|s| !s.is_empty())
                {
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

fn ingest_grok(conn: &Connection, path: &Path) -> Result<(), String> {
    let workspace = grok_workspace(path);
    for_each_line(path, |line| {
        let v: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => return Ok(()),
        };
        for draft in grok_drafts(&v, &workspace) {
            upsert(conn, "grok", &draft)?;
        }
        Ok(())
    })
}

fn ingest(conn: &Connection, found: &Found) -> Result<(), String> {
    match found.source {
        Source::Claude => ingest_claude(conn, &found.path),
        Source::Codex => ingest_codex(conn, &found.path),
        Source::Grok => ingest_grok(conn, &found.path),
    }
}

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
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let meta = match fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(_) => continue,
        };
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

fn collect_claude(dir: &Path, out: &mut Vec<Found>) {
    let mut paths = Vec::new();
    walk_files(dir, &mut paths, &|p| is_jsonl(p) && !is_history_jsonl(p));
    for path in paths {
        out.push(Found {
            path,
            source: Source::Claude,
        });
    }
}

fn collect_codex(dir: &Path, out: &mut Vec<Found>) {
    let mut paths = Vec::new();
    walk_files(dir, &mut paths, &|p| is_jsonl(p) && !is_history_jsonl(p));
    for path in paths {
        out.push(Found {
            path,
            source: Source::Codex,
        });
    }
}

fn collect_grok(dir: &Path, out: &mut Vec<Found>) {
    let mut paths = Vec::new();
    walk_files(dir, &mut paths, &|p| {
        p.file_name().and_then(|s| s.to_str()) == Some("updates.jsonl")
    });
    for path in paths {
        out.push(Found {
            path,
            source: Source::Grok,
        });
    }
}

fn percent_encode_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn scan_found(ledger: &Path, found: &[Found]) -> Result<ScanStats, String> {
    with_ledger(ledger, |conn| {
        let mut stats = ScanStats::default();
        for item in found {
            stats.files_seen += 1;
            let meta = match fs::metadata(&item.path) {
                Ok(m) => m,
                Err(_) => continue,
            };
            let (size, secs, nanos) = file_stamp(&meta);
            let path_str = item.path.to_string_lossy().into_owned();
            if unchanged(conn, &path_str, size, secs, nanos) {
                stats.files_skipped += 1;
                continue;
            }
            conn.execute("BEGIN IMMEDIATE", [])
                .map_err(|e| format!("ledger begin: {e}"))?;
            if let Err(e) =
                ingest(conn, item).and_then(|_| mark_scanned(conn, &path_str, size, secs, nanos))
            {
                let _ = conn.execute("ROLLBACK", []);
                return Err(e);
            }
            conn.execute("COMMIT", [])
                .map_err(|e| format!("ledger commit: {e}"))?;
            stats.files_read += 1;
        }
        Ok(stats)
    })
}

/// Scan explicit roots. Missing roots are empty, not an error.
pub fn scan_roots(
    ledger: &Path,
    claude_projects: &Path,
    codex_sessions: &Path,
    grok_sessions: &Path,
) -> Result<ScanStats, String> {
    let mut found = Vec::new();
    collect_claude(claude_projects, &mut found);
    collect_codex(codex_sessions, &mut found);
    collect_grok(grok_sessions, &mut found);
    scan_found(ledger, &found)
}

pub fn scan_host() -> Result<ScanStats, String> {
    let home = dirs::home_dir().ok_or_else(|| "no home directory".to_string())?;
    scan_roots(
        &ledger_path(),
        &home.join(".claude").join("projects"),
        &home.join(".codex").join("sessions"),
        &home.join(".grok").join("sessions"),
    )
}

fn recent_day_dirs(root: &Path) -> Vec<PathBuf> {
    let today = chrono::Local::now().date_naive();
    let mut days = vec![today];
    if let Some(prev) = today.checked_sub_days(chrono::Days::new(1)) {
        days.push(prev);
    }
    days.into_iter()
        .map(|day| {
            root.join(format!("{:04}", day.year()))
                .join(format!("{:02}", day.month()))
                .join(format!("{:02}", day.day()))
        })
        .collect()
}

/// Idle scan: the Claude project dir for this cwd, the Grok session dir
/// encoded from this cwd, and today's Codex rollouts whose cwd matches.
/// Not a walk of every transcript tree.
pub fn scan_workspace_host(cwd: &str) -> Result<ScanStats, String> {
    let home = dirs::home_dir().ok_or_else(|| "no home directory".to_string())?;
    let mut found = Vec::new();
    let canon = canonical_workspace(cwd);
    for path in [cwd.to_string(), canon.clone()] {
        let hash = crate::chat_history::claude_project_hash(&path);
        collect_claude(
            &home.join(".claude").join("projects").join(hash),
            &mut found,
        );
        let enc = percent_encode_path(&path);
        collect_grok(&home.join(".grok").join("sessions").join(enc), &mut found);
    }
    found.sort_by(|a, b| a.path.cmp(&b.path));
    found.dedup_by(|a, b| a.path == b.path);
    let mut codex = Vec::new();
    for day in recent_day_dirs(&home.join(".codex").join("sessions")) {
        collect_codex(&day, &mut codex);
    }
    for item in codex {
        if codex_file_cwd(&item.path).as_deref() == Some(canon.as_str()) {
            found.push(item);
        }
    }
    scan_found(&ledger_path(), &found)
}

#[derive(Debug)]
struct Group {
    workspace: String,
    harness: String,
    model: String,
    totals: TokenTotals,
}

fn add_totals(into: &mut TokenTotals, add: &TokenTotals) {
    into.input_tokens += add.input_tokens;
    into.output_tokens += add.output_tokens;
    into.cache_read_tokens += add.cache_read_tokens;
    into.cache_write_tokens += add.cache_write_tokens;
    into.turns += add.turns;
}

fn load_groups(conn: &Connection, workspace: Option<&str>) -> Result<Vec<Group>, String> {
    let sql = "SELECT workspace_path, harness, model,
            COALESCE(SUM(input_tokens), 0),
            COALESCE(SUM(output_tokens), 0),
            COALESCE(SUM(cache_read_tokens), 0),
            COALESCE(SUM(cache_write_tokens), 0),
            COUNT(*)
         FROM turns
         WHERE (?1 IS NULL OR workspace_path = ?1)
         GROUP BY workspace_path, harness, model";
    let mut stmt = conn
        .prepare(sql)
        .map_err(|e| format!("ledger query: {e}"))?;
    let rows = stmt
        .query_map(params![workspace], |r| {
            Ok(Group {
                workspace: r.get(0)?,
                harness: r.get(1)?,
                model: r.get(2)?,
                totals: TokenTotals {
                    input_tokens: r.get(3)?,
                    output_tokens: r.get(4)?,
                    cache_read_tokens: r.get(5)?,
                    cache_write_tokens: r.get(6)?,
                    turns: r.get(7)?,
                },
            })
        })
        .map_err(|e| format!("ledger query: {e}"))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| format!("ledger row: {e}"))?);
    }
    Ok(out)
}

struct TurnStamp {
    recorded_at: String,
    totals: TokenTotals,
}

fn recorded_text(row: &rusqlite::Row<'_>, idx: usize) -> rusqlite::Result<String> {
    Ok(match row.get_ref(idx)? {
        rusqlite::types::ValueRef::Text(bytes) => String::from_utf8_lossy(bytes).into_owned(),
        rusqlite::types::ValueRef::Integer(n) => n.to_string(),
        _ => String::new(),
    })
}

fn load_turn_stamps(conn: &Connection, workspace: Option<&str>) -> Result<Vec<TurnStamp>, String> {
    let sql = "SELECT recorded_at,
            COALESCE(input_tokens, 0),
            COALESCE(output_tokens, 0),
            COALESCE(cache_read_tokens, 0),
            COALESCE(cache_write_tokens, 0)
         FROM turns
         WHERE (?1 IS NULL OR workspace_path = ?1)";
    let mut stmt = conn.prepare(sql).map_err(|e| format!("ledger days: {e}"))?;
    let rows = stmt
        .query_map(params![workspace], |r| {
            Ok(TurnStamp {
                recorded_at: recorded_text(r, 0)?,
                totals: TokenTotals {
                    input_tokens: r.get(1)?,
                    output_tokens: r.get(2)?,
                    cache_read_tokens: r.get(3)?,
                    cache_write_tokens: r.get(4)?,
                    turns: 1,
                },
            })
        })
        .map_err(|e| format!("ledger days: {e}"))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| format!("ledger day row: {e}"))?);
    }
    Ok(out)
}

fn is_ymd_prefix(bytes: &[u8]) -> bool {
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[..4].iter().all(|b| b.is_ascii_digit())
        && bytes[5..7].iter().all(|b| b.is_ascii_digit())
        && bytes[8..10].iter().all(|b| b.is_ascii_digit())
}

/// UTC `YYYY-MM-DD` for a ledger `recorded_at`, or `None` when the value
/// is not a day we can place. A date prefix is kept as written (no local
/// timezone shift). Digits are unix milliseconds at `>= 1_000_000_000_000`
/// and unix seconds at `>= 1_000_000_000`. Anything else, including the
/// fixture timestamp `10`, is omitted. Never errors.
fn utc_day(raw: &str) -> Option<String> {
    let s = raw.trim();
    let bytes = s.as_bytes();
    if bytes.len() >= 10 && is_ymd_prefix(&bytes[..10]) {
        return Some(s[..10].to_string());
    }
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: i64 = s.parse().ok()?;
    let secs = if n >= 1_000_000_000_000 {
        n / 1000
    } else if n >= 1_000_000_000 {
        n
    } else {
        return None;
    };
    let date = chrono::DateTime::<chrono::Utc>::from_timestamp(secs, 0)?.date_naive();
    Some(format!(
        "{:04}-{:02}-{:02}",
        date.year(),
        date.month(),
        date.day()
    ))
}

/// Unix milliseconds for a ledger `recorded_at`, or `None` when it cannot
/// be placed. Accepts RFC3339, all-digit unix seconds (`>= 1e9`) or
/// milliseconds (`>= 1e12`), and a bare `YYYY-MM-DD` (UTC midnight). This
/// is the millisecond sibling of [`utc_day`]; it never errors, so an
/// unparseable stamp just yields `None` and the row still lists.
fn epoch_ms(raw: &str) -> Option<i64> {
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

fn bucket_days(stamps: &[TurnStamp]) -> Vec<DayTotals> {
    let mut by_day: BTreeMap<String, TokenTotals> = BTreeMap::new();
    for stamp in stamps {
        let Some(day) = utc_day(&stamp.recorded_at) else {
            continue;
        };
        add_totals(by_day.entry(day).or_default(), &stamp.totals);
    }
    by_day
        .into_iter()
        .map(|(day, totals)| DayTotals {
            day,
            input_tokens: totals.input_tokens,
            output_tokens: totals.output_tokens,
            cache_read_tokens: totals.cache_read_tokens,
            cache_write_tokens: totals.cache_write_tokens,
            turns: totals.turns,
        })
        .collect()
}

/// `workspace` is an exact path after [`canonical_workspace`]. `None` is
/// the machine total. Known paths are this daemon's workspaces
/// (`projects_list` paths and `workspaces.worktree_path`). Sentinels are
/// never their own bucket.
pub fn query(
    ledger: &Path,
    workspace: Option<&str>,
    known: &[String],
) -> Result<UsageReport, String> {
    let filter = workspace.map(canonical_workspace).filter(|s| !s.is_empty());
    let known_set: BTreeSet<String> = known
        .iter()
        .filter(|p| !is_sentinel_workspace(p))
        .map(|p| canonical_workspace(p))
        .filter(|p| !p.is_empty())
        .collect();
    let (groups, stamps) = with_ledger(ledger, |conn| {
        let groups = load_groups(conn, filter.as_deref())?;
        let stamps = load_turn_stamps(conn, filter.as_deref())?;
        Ok((groups, stamps))
    })?;
    let days = bucket_days(&stamps);
    let mut total = TokenTotals::default();
    let mut outside = TokenTotals::default();
    let mut by_ws: BTreeMap<String, TokenTotals> = BTreeMap::new();
    let mut by_h: BTreeMap<String, TokenTotals> = BTreeMap::new();
    let mut by_m: BTreeMap<(String, String), TokenTotals> = BTreeMap::new();
    for g in &groups {
        add_totals(&mut total, &g.totals);
        add_totals(by_h.entry(g.harness.clone()).or_default(), &g.totals);
        add_totals(
            by_m.entry((g.harness.clone(), g.model.clone()))
                .or_default(),
            &g.totals,
        );
        let outside_row = is_sentinel_workspace(&g.workspace) || !known_set.contains(&g.workspace);
        if filter.is_none() && outside_row {
            add_totals(&mut outside, &g.totals);
        } else {
            add_totals(by_ws.entry(g.workspace.clone()).or_default(), &g.totals);
        }
    }
    if let Some(ref path) = filter {
        if !by_ws.contains_key(path) {
            by_ws.insert(path.clone(), TokenTotals::default());
        }
    }
    let workspaces = by_ws
        .into_iter()
        .map(|(path, totals)| WorkspaceTotals {
            outside: !known_set.contains(&path),
            path,
            totals,
        })
        .collect();
    let harnesses = by_h
        .into_iter()
        .map(|(harness, totals)| HarnessTotals { harness, totals })
        .collect();
    let models = by_m
        .into_iter()
        .map(|((harness, model), totals)| ModelTotals {
            harness,
            model,
            totals,
        })
        .collect();
    Ok(UsageReport {
        scope: if filter.is_some() {
            "workspace".to_string()
        } else {
            "machine".to_string()
        },
        workspace: filter,
        total,
        harnesses,
        models,
        workspaces,
        outside,
        days,
    })
}

pub fn query_host(workspace: Option<&str>, known: &[String]) -> Result<UsageReport, String> {
    query(&ledger_path(), workspace, known)
}

/// Clamp bounds for a turn-log page. `limit` is capped so one request can
/// never scan the whole ledger; the daemon route defaults an absent limit
/// to [`DEFAULT_TURNS_LIMIT`] before calling.
pub const MAX_TURNS_LIMIT: i64 = 200;
pub const DEFAULT_TURNS_LIMIT: i64 = 50;

/// Raw newest-first turns for the live log. `workspace` is an exact path
/// (canonicalized here); `None` is machine-wide. `before` is an exclusive
/// `rowid` keyset cursor (`None` = newest page). `limit` is clamped to
/// `1..=`[`MAX_TURNS_LIMIT`]. `known` marks each row's `outside` flag the
/// same way [`query`] does. Rows are ordered by `rowid DESC` (ingestion
/// order), which for the ledger tracks arrival of new turns.
pub fn query_turns(
    ledger: &Path,
    workspace: Option<&str>,
    known: &[String],
    before: Option<i64>,
    limit: i64,
) -> Result<TurnPage, String> {
    let filter = workspace.map(canonical_workspace).filter(|s| !s.is_empty());
    let known_set: BTreeSet<String> = known
        .iter()
        .filter(|p| !is_sentinel_workspace(p))
        .map(|p| canonical_workspace(p))
        .filter(|p| !p.is_empty())
        .collect();
    let limit = limit.clamp(1, MAX_TURNS_LIMIT);
    let rows = with_ledger(ledger, |conn| {
        let sql = "SELECT rowid, workspace_path, harness, model,
                COALESCE(input_tokens, 0),
                COALESCE(output_tokens, 0),
                COALESCE(cache_read_tokens, 0),
                COALESCE(cache_write_tokens, 0),
                recorded_at
             FROM turns
             WHERE (?1 IS NULL OR workspace_path = ?1)
               AND (?2 IS NULL OR rowid < ?2)
             ORDER BY rowid DESC
             LIMIT ?3";
        let mut stmt = conn.prepare(sql).map_err(|e| format!("ledger turns: {e}"))?;
        let mapped = stmt
            .query_map(params![filter, before, limit], |r| {
                let workspace_path: String = r.get(1)?;
                let recorded_at = recorded_text(r, 8)?;
                Ok(TurnRow {
                    rowid: r.get(0)?,
                    outside: is_sentinel_workspace(&workspace_path)
                        || !known_set.contains(&workspace_path),
                    workspace_path,
                    harness: r.get(2)?,
                    model: r.get(3)?,
                    input_tokens: r.get(4)?,
                    output_tokens: r.get(5)?,
                    cache_read_tokens: r.get(6)?,
                    cache_write_tokens: r.get(7)?,
                    recorded_ms: epoch_ms(&recorded_at),
                    recorded_at,
                })
            })
            .map_err(|e| format!("ledger turns: {e}"))?;
        let mut out = Vec::new();
        for row in mapped {
            out.push(row.map_err(|e| format!("ledger turn row: {e}"))?);
        }
        Ok(out)
    })?;
    // A short page means the ledger is exhausted; a full page hands back
    // the last rowid so the next call resumes strictly below it.
    let next_cursor = if (rows.len() as i64) < limit {
        None
    } else {
        rows.last().map(|r| r.rowid)
    };
    Ok(TurnPage { rows, next_cursor })
}

pub fn query_turns_host(
    workspace: Option<&str>,
    known: &[String],
    before: Option<i64>,
    limit: i64,
) -> Result<TurnPage, String> {
    query_turns(&ledger_path(), workspace, known, before, limit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

    struct Tmp(PathBuf);
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn tmp() -> Tmp {
        let n = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!("k2-token-usage-{}-{n}", std::process::id()));
        fs::create_dir_all(&p).expect("tmpdir");
        Tmp(p)
    }

    fn write_jsonl(path: &Path, lines: &[Value]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("parent");
        }
        let mut body = String::new();
        for line in lines {
            body.push_str(&serde_json::to_string(line).expect("json line"));
            body.push('\n');
        }
        fs::write(path, body).expect("write jsonl");
    }

    fn insert_turns(ledger: &Path, rows: &[(&str, &str, &str, i64, i64, i64, i64)]) {
        let conn = open_ledger(ledger).expect("open ledger");
        for (key, workspace, recorded_at, input, output, cache_read, cache_write) in rows {
            conn.execute(
                "INSERT INTO turns (
                    turn_key, workspace_path, harness, model,
                    input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
                    recorded_at
                 ) VALUES (?1, ?2, 'grok', 'grok-test', ?3, ?4, ?5, ?6, ?7)",
                params![
                    key,
                    workspace,
                    input,
                    output,
                    cache_read,
                    cache_write,
                    recorded_at
                ],
            )
            .expect("insert turn");
        }
    }

    fn canon_dir(root: &Path, name: &str) -> String {
        let path = root.join(name);
        fs::create_dir_all(&path).expect("workspace dir");
        canonical_workspace(&path.to_string_lossy())
    }

    #[test]
    fn turns_paged_newest_first_with_keyset() {
        let tmp = tmp();
        let ledger = tmp.0.join("tokens.sqlite");
        let a = canon_dir(&tmp.0, "ws-a");
        let b = canon_dir(&tmp.0, "ws-b");
        // Inserted oldest→newest, so rowid ascends k1<k2<k3<k4.
        insert_turns(
            &ledger,
            &[
                ("k1", &a, "2026-09-26T00:00:00Z", 1, 1, 0, 0),
                ("k2", &a, "2026-09-26T00:01:00Z", 2, 2, 0, 0),
                ("k3", &b, "1758844800", 3, 3, 0, 0), // unix seconds
                ("k4", &a, "2026-09-26T00:03:00Z", 4, 4, 0, 0),
            ],
        );
        let known = vec![a.clone()];

        // Rows are identified by their distinct input_tokens (1..=4).
        let ins = |rows: &[TurnRow]| -> Vec<i64> { rows.iter().map(|r| r.input_tokens).collect() };

        // Page 1: newest two, full page → a cursor comes back.
        let p1 = query_turns(&ledger, None, &known, None, 2).expect("page 1");
        assert_eq!(ins(&p1.rows), vec![4, 3], "newest first");
        assert!(p1.next_cursor.is_some(), "full page yields a cursor");
        assert!(p1.rows[0].recorded_ms.is_some(), "rfc3339 parsed to ms");
        assert!(p1.rows[1].recorded_ms.is_some(), "unix seconds parsed to ms");
        // `outside` reflects the known set: ws-b is not known.
        assert!(p1.rows[1].outside, "ws-b turn is outside");
        assert!(!p1.rows[0].outside, "ws-a turn is not outside");

        // Page 2 resumes strictly below the cursor; short page → no cursor.
        let p2 = query_turns(&ledger, None, &known, p1.next_cursor, 2).expect("page 2");
        assert_eq!(ins(&p2.rows), vec![2, 1]);
        assert!(p2.next_cursor.is_none(), "exhausted ledger clears the cursor");

        // Workspace filter narrows to ws-a's three turns, newest first.
        let only_a = query_turns(&ledger, Some(&a), &known, None, 50).expect("ws-a");
        assert_eq!(ins(&only_a.rows), vec![4, 2, 1]);
        assert!(only_a.rows.iter().all(|r| !r.outside));
    }

    struct Stored {
        turn_key: String,
        workspace_path: String,
        harness: String,
        model: String,
        input_tokens: Option<i64>,
        output_tokens: Option<i64>,
        cache_read_tokens: Option<i64>,
        cache_write_tokens: Option<i64>,
    }

    fn stored_rows(ledger: &Path) -> Vec<Stored> {
        let conn = Connection::open(ledger).expect("reopen ledger");
        let mut stmt = conn
            .prepare(
                "SELECT turn_key, workspace_path, harness, model,
                    input_tokens, output_tokens, cache_read_tokens, cache_write_tokens
                 FROM turns ORDER BY turn_key",
            )
            .expect("prepare");
        let rows = stmt
            .query_map([], |r| {
                Ok(Stored {
                    turn_key: r.get(0)?,
                    workspace_path: r.get(1)?,
                    harness: r.get(2)?,
                    model: r.get(3)?,
                    input_tokens: r.get(4)?,
                    output_tokens: r.get(5)?,
                    cache_read_tokens: r.get(6)?,
                    cache_write_tokens: r.get(7)?,
                })
            })
            .expect("query");
        rows.map(|r| r.expect("row")).collect()
    }

    fn claude_line(
        id: &str,
        model: &str,
        cwd: &str,
        input: i64,
        output: i64,
        cache_read: i64,
        cache_write: i64,
    ) -> Value {
        json!({
            "type": "assistant",
            "cwd": cwd,
            "timestamp": "2026-09-26T00:00:00Z",
            "message": {
                "id": id,
                "model": model,
                "role": "assistant",
                "content": [{"type": "text", "text": "not tokens"}],
                "usage": {
                    "input_tokens": input,
                    "output_tokens": output,
                    "cache_read_input_tokens": cache_read,
                    "cache_creation_input_tokens": cache_write
                }
            }
        })
    }

    #[test]
    fn ledger_path_is_usage_tokens_sqlite() {
        let p = ledger_path();
        assert_eq!(
            p.file_name().and_then(|s| s.to_str()),
            Some("tokens.sqlite")
        );
        assert_eq!(
            p.parent()
                .and_then(|s| s.file_name())
                .and_then(|s| s.to_str()),
            Some("usage")
        );
        let home = p.parent().and_then(|s| s.parent()).expect("k2 home");
        assert_eq!(home.file_name().and_then(|s| s.to_str()), Some(".k2"));
        assert!(!p.to_string_lossy().contains("k2so.db"));
        assert!(!p.to_string_lossy().ends_with("k2.db"));
        assert!(!p.to_string_lossy().ends_with("skin.db"));
    }

    #[test]
    fn claude_same_message_id_is_one_row() {
        let dir = tmp();
        let cwd = dir.0.join("ws");
        fs::create_dir_all(&cwd).expect("cwd");
        let cwd_s = cwd.to_string_lossy().into_owned();
        let projects = dir.0.join("claude");
        let session = projects.join("abc.jsonl");
        write_jsonl(
            &session,
            &[
                claude_line("msg_same", "claude-a", &cwd_s, 3, 4, 10, 2),
                claude_line("msg_same", "claude-a", &cwd_s, 3, 4, 10, 2),
            ],
        );
        // Forked copy of the same provider id must not insert a second row.
        write_jsonl(
            &projects.join("fork.jsonl"),
            &[claude_line("msg_same", "claude-a", &cwd_s, 3, 4, 10, 2)],
        );
        write_jsonl(
            &projects.join("history.jsonl"),
            &[claude_line(
                "msg_history_only",
                "claude-a",
                &cwd_s,
                999,
                999,
                999,
                999,
            )],
        );
        let ledger = dir.0.join("tokens.sqlite");
        scan_roots(
            &ledger,
            &projects,
            &dir.0.join("no-codex"),
            &dir.0.join("no-grok"),
        )
        .expect("scan");
        let rows = stored_rows(&ledger);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].turn_key, "msg_same");
        assert_eq!(rows[0].input_tokens, Some(3));
        assert_eq!(rows[0].output_tokens, Some(4));
        assert_eq!(rows[0].cache_read_tokens, Some(10));
        assert_eq!(rows[0].cache_write_tokens, Some(2));
        // Claude input is the uncached part. Cache is not subtracted.
        assert_ne!(rows[0].input_tokens, Some(3 - 10));
        scan_roots(
            &ledger,
            &projects,
            &dir.0.join("no-codex"),
            &dir.0.join("no-grok"),
        )
        .expect("rescan");
        assert_eq!(stored_rows(&ledger).len(), 1);
    }

    #[test]
    fn claude_later_usage_replaces_placeholder() {
        let dir = tmp();
        let cwd = dir.0.join("ws");
        fs::create_dir_all(&cwd).expect("cwd");
        let cwd_s = cwd.to_string_lossy().into_owned();
        let projects = dir.0.join("claude");
        write_jsonl(
            &projects.join("s.jsonl"),
            &[
                claude_line("msg_grow", "claude-a", &cwd_s, 1, 1, 5, 0),
                claude_line("msg_grow", "claude-a", &cwd_s, 80, 40, 900, 12),
            ],
        );
        let ledger = dir.0.join("tokens.sqlite");
        scan_roots(
            &ledger,
            &projects,
            &dir.0.join("no-codex"),
            &dir.0.join("no-grok"),
        )
        .expect("scan");
        let rows = stored_rows(&ledger);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].input_tokens, Some(80));
        assert_eq!(rows[0].output_tokens, Some(40));
        assert_eq!(rows[0].cache_read_tokens, Some(900));
        assert_eq!(rows[0].cache_write_tokens, Some(12));
        assert_ne!(rows[0].input_tokens, Some(1));
        assert_ne!(rows[0].input_tokens, Some(80 + 900));
        assert_ne!(rows[0].input_tokens, Some(80 - 900));
    }

    #[test]
    fn model_change_keeps_early_turns_on_the_first_model() {
        let dir = tmp();
        let cwd = dir.0.join("ws");
        fs::create_dir_all(&cwd).expect("cwd");
        let cwd_s = cwd.to_string_lossy().into_owned();
        let projects = dir.0.join("claude");
        write_jsonl(
            &projects.join("s.jsonl"),
            &[
                claude_line("msg_early", "claude-first", &cwd_s, 10, 1, 4, 0),
                claude_line("msg_late", "claude-second", &cwd_s, 7, 2, 8, 0),
            ],
        );
        let ledger = dir.0.join("tokens.sqlite");
        scan_roots(
            &ledger,
            &projects,
            &dir.0.join("no-codex"),
            &dir.0.join("no-grok"),
        )
        .expect("scan");
        let known = [canonical_workspace(&cwd_s)];
        let report = query(&ledger, None, &known).expect("report");
        assert_eq!(report.models.len(), 2);
        let early = report
            .models
            .iter()
            .find(|m| m.model == "claude-first")
            .expect("first model");
        let late = report
            .models
            .iter()
            .find(|m| m.model == "claude-second")
            .expect("second model");
        assert_eq!(early.totals.input_tokens, 10);
        assert_eq!(late.totals.input_tokens, 7);
        let rows = stored_rows(&ledger);
        let early_row = rows
            .iter()
            .find(|r| r.turn_key == "msg_early")
            .expect("early");
        assert_eq!(early_row.model, "claude-first");
        assert_ne!(early_row.model, "claude-second");
    }

    #[test]
    fn codex_last_snapshot_once_and_cache_not_added_to_input() {
        let dir = tmp();
        let cwd = dir.0.join("ws");
        fs::create_dir_all(&cwd).expect("cwd");
        let cwd_s = canonical_workspace(&cwd.to_string_lossy());
        let sessions = dir.0.join("codex").join("2026").join("09").join("26");
        let rollout = sessions.join("rollout.jsonl");
        write_jsonl(
            &rollout,
            &[
                json!({"type": "session_meta", "payload": {"id": "sess-1", "cwd": cwd_s}}),
                json!({"type": "turn_context", "payload": {"turn_id": "turn-1", "model": "gpt-first", "cwd": cwd_s}}),
                json!({"type": "event_msg", "payload": {"type": "token_count", "info": null}}),
                json!({"type": "event_msg", "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 9999, "cached_input_tokens": 9999, "output_tokens": 9999}, "last_token_usage": {"input_tokens": 10, "cached_input_tokens": 4, "output_tokens": 1, "reasoning_output_tokens": 9}}}}),
                json!({"type": "event_msg", "payload": {"type": "token_count", "info": {"last_token_usage": {"input_tokens": 20, "cached_input_tokens": 8, "output_tokens": 2, "reasoning_output_tokens": 9}}}}),
                json!({"type": "event_msg", "payload": {"type": "token_count", "info": {"last_token_usage": {"input_tokens": 30, "cached_input_tokens": 11, "output_tokens": 3, "reasoning_output_tokens": 9}}}}),
                json!({"type": "turn_context", "payload": {"turn_id": "turn-2", "model": "gpt-second", "cwd": cwd_s}}),
                json!({"type": "event_msg", "payload": {"type": "token_count", "info": {"last_token_usage": {"input_tokens": 5, "cached_input_tokens": 1, "output_tokens": 1, "reasoning_output_tokens": 50}}}}),
            ],
        );
        write_jsonl(
            &dir.0.join("codex").join("history.jsonl"),
            &[
                json!({"type": "event_msg", "payload": {"type": "token_count", "info": {"last_token_usage": {"input_tokens": 123456, "cached_input_tokens": 1, "output_tokens": 1}}}}),
            ],
        );
        let ledger = dir.0.join("tokens.sqlite");
        scan_roots(
            &ledger,
            &dir.0.join("no-claude"),
            &dir.0.join("codex"),
            &dir.0.join("no-grok"),
        )
        .expect("scan");
        let rows = stored_rows(&ledger);
        assert_eq!(rows.len(), 2);
        let first = rows
            .iter()
            .find(|r| r.turn_key == "turn-1")
            .expect("turn 1");
        let second = rows
            .iter()
            .find(|r| r.turn_key == "turn-2")
            .expect("turn 2");
        assert_eq!(first.input_tokens, Some(30));
        assert_eq!(first.output_tokens, Some(3));
        assert_eq!(first.cache_read_tokens, Some(11));
        assert_eq!(first.model, "gpt-first");
        assert_ne!(first.input_tokens, Some(10 + 20 + 30));
        assert_ne!(first.input_tokens, Some(30 + 11));
        assert_ne!(first.output_tokens, Some(3 + 9));
        assert_eq!(second.input_tokens, Some(5));
        assert_eq!(second.cache_read_tokens, Some(1));
        assert_eq!(second.model, "gpt-second");
        assert_ne!(second.input_tokens, Some(30 + 5));
        assert_ne!(first.model, "gpt-second");
    }

    #[test]
    fn grok_skips_lines_without_usage_and_adds_reasoning_once() {
        let dir = tmp();
        let cwd = dir.0.join("ws");
        fs::create_dir_all(&cwd).expect("cwd");
        let cwd_s = cwd.to_string_lossy().into_owned();
        let session = dir.0.join("grok").join("encoded").join("sess");
        fs::create_dir_all(&session).expect("session");
        fs::write(
            session.join("summary.json"),
            serde_json::to_string(&json!({
                "current_model_id": "end-of-session-model",
                "session_kind": "subagent",
                "info": {"cwd": cwd_s}
            }))
            .expect("summary"),
        )
        .expect("write summary");
        fs::write(
            session.join("chat_history.jsonl"),
            "{\"usage\":{\"inputTokens\":999,\"outputTokens\":999}}\n",
        )
        .expect("chat history");
        write_jsonl(
            &session.join("updates.jsonl"),
            &[
                json!({"method": "session/update", "params": {"update": {"sessionUpdate": "user_message_chunk", "content": "hello"}}}),
                json!({"method": "_x.ai/session/update", "params": {"update": {"sessionUpdate": "turn_completed", "prompt_id": "bare"}}}),
                json!({
                    "method": "_x.ai/session/update",
                    "timestamp": 10,
                    "params": {
                        "update": {
                            "sessionUpdate": "turn_completed",
                            "prompt_id": "p9",
                            "model": "grok-test",
                            "usage": {
                                "inputTokens": 10,
                                "outputTokens": 4,
                                "reasoningTokens": 6,
                                "cachedReadTokens": 2,
                                "cacheCreationTokens": 1,
                                "costUsdTicks": 999
                            }
                        }
                    }
                }),
                json!({
                    "method": "session/update",
                    "timestamp": 11,
                    "params": {
                        "update": {
                            "sessionUpdate": "turn_completed",
                            "prompt_id": "p9",
                            "model": "grok-test",
                            "usage": {
                                "inputTokens": 10,
                                "outputTokens": 4,
                                "reasoningTokens": 6,
                                "cachedReadTokens": 2,
                                "cacheCreationTokens": 1
                            }
                        }
                    }
                }),
            ],
        );
        let ledger = dir.0.join("tokens.sqlite");
        scan_roots(
            &ledger,
            &dir.0.join("no-claude"),
            &dir.0.join("no-codex"),
            &dir.0.join("grok"),
        )
        .expect("scan");
        let rows = stored_rows(&ledger);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].turn_key, "p9\tgrok-test");
        assert_eq!(rows[0].model, "grok-test");
        assert_ne!(rows[0].model, "end-of-session-model");
        assert_eq!(rows[0].input_tokens, Some(10));
        assert_eq!(rows[0].output_tokens, Some(4 + 6));
        assert_ne!(rows[0].output_tokens, Some(4 + 6 + 6));
        assert_eq!(rows[0].cache_read_tokens, Some(2));
        assert_eq!(rows[0].cache_write_tokens, Some(1));
        assert_eq!(rows[0].workspace_path, canonical_workspace(&cwd_s));
        assert_eq!(rows[0].harness, "grok");
    }

    #[test]
    fn grok_model_usage_splits_without_the_parent_row() {
        let dir = tmp();
        let cwd = dir.0.join("ws");
        fs::create_dir_all(&cwd).expect("cwd");
        let cwd_s = cwd.to_string_lossy().into_owned();
        let session = dir.0.join("grok").join("enc").join("sess");
        fs::create_dir_all(&session).expect("session");
        fs::write(
            session.join("summary.json"),
            serde_json::to_string(&json!({"info": {"cwd": cwd_s}, "current_model_id": "nope"}))
                .expect("summary"),
        )
        .expect("summary");
        write_jsonl(
            &session.join("updates.jsonl"),
            &[json!({
                "method": "_x.ai/session/update",
                "params": {
                    "update": {
                        "prompt_id": "split",
                        "usage": {
                            "inputTokens": 100,
                            "outputTokens": 10,
                            "reasoningTokens": 1,
                            "modelUsage": {
                                "m-a": {"inputTokens": 40, "outputTokens": 3, "reasoningTokens": 1, "cachedReadTokens": 5, "cacheCreationTokens": 0},
                                "m-b": {"inputTokens": 60, "outputTokens": 7, "reasoningTokens": 2, "cachedReadTokens": 0, "cacheCreationTokens": 4}
                            }
                        }
                    }
                }
            })],
        );
        let ledger = dir.0.join("tokens.sqlite");
        scan_roots(
            &ledger,
            &dir.0.join("no-claude"),
            &dir.0.join("no-codex"),
            &dir.0.join("grok"),
        )
        .expect("scan");
        let rows = stored_rows(&ledger);
        assert_eq!(rows.len(), 2);
        let a = rows.iter().find(|r| r.model == "m-a").expect("m-a");
        let b = rows.iter().find(|r| r.model == "m-b").expect("m-b");
        assert_eq!(a.output_tokens, Some(3 + 1));
        assert_eq!(b.output_tokens, Some(7 + 2));
        assert_eq!(a.input_tokens, Some(40));
        assert_eq!(b.input_tokens, Some(60));
        let output_sum = a.output_tokens.expect("a out") + b.output_tokens.expect("b out");
        assert_eq!(output_sum, 4 + 9);
        assert_ne!(output_sum, 10 + 1 + 4 + 9);
    }

    #[test]
    fn machine_total_is_workspaces_plus_outside() {
        let dir = tmp();
        let root = dir.0.join("root");
        let worktree = dir.0.join("worktree");
        let other = dir.0.join("other");
        fs::create_dir_all(&root).expect("root");
        fs::create_dir_all(&worktree).expect("wt");
        fs::create_dir_all(&other).expect("other");
        let root_s = root.to_string_lossy().into_owned();
        let wt_s = worktree.to_string_lossy().into_owned();
        let other_s = other.to_string_lossy().into_owned();
        let projects = dir.0.join("claude");
        write_jsonl(
            &projects.join("s.jsonl"),
            &[
                claude_line("r", "m", &root_s, 10, 1, 0, 0),
                claude_line("w", "m", &wt_s, 20, 1, 0, 0),
                claude_line("o", "m", &other_s, 5, 1, 0, 0),
                claude_line("s", "m", "_orphan", 7, 1, 0, 0),
                claude_line("b", "m", "_broadcast", 3, 1, 0, 0),
            ],
        );
        let ledger = dir.0.join("tokens.sqlite");
        scan_roots(
            &ledger,
            &projects,
            &dir.0.join("no-codex"),
            &dir.0.join("no-grok"),
        )
        .expect("scan");
        let known = [
            canonical_workspace(&root_s),
            canonical_workspace(&wt_s),
            "_orphan".to_string(),
        ];
        let report = query(&ledger, None, &known).expect("report");
        let ws_input: i64 = report
            .workspaces
            .iter()
            .map(|w| w.totals.input_tokens)
            .sum();
        assert_eq!(
            report.total.input_tokens,
            ws_input + report.outside.input_tokens
        );
        assert_eq!(report.total.input_tokens, 10 + 20 + 5 + 7 + 3);
        assert_eq!(report.outside.input_tokens, 5 + 7 + 3);
        assert!(report.workspaces.iter().all(|w| w.path != "_orphan"));
        assert!(report.workspaces.iter().all(|w| w.path != "_broadcast"));
        let root_bucket = report
            .workspaces
            .iter()
            .find(|w| w.path == canonical_workspace(&root_s))
            .expect("root");
        let wt_bucket = report
            .workspaces
            .iter()
            .find(|w| w.path == canonical_workspace(&wt_s))
            .expect("worktree");
        assert_eq!(root_bucket.totals.input_tokens, 10);
        assert_eq!(wt_bucket.totals.input_tokens, 20);
        assert!(!root_bucket.outside);
        assert!(!wt_bucket.outside);
        let only_root = query(&ledger, Some(&root_s), &known).expect("root query");
        assert_eq!(only_root.total.input_tokens, 10);
        assert!(only_root
            .workspaces
            .iter()
            .all(|w| w.path == canonical_workspace(&root_s)));
        assert_eq!(only_root.workspaces.len(), 1);
        let only_wt = query(&ledger, Some(&wt_s), &known).expect("wt query");
        assert_eq!(only_wt.total.input_tokens, 20);
        assert_ne!(only_wt.total.input_tokens, only_root.total.input_tokens);
    }

    #[test]
    fn sqlite_file_has_no_planted_access_token_and_is_mode_0600() {
        let dir = tmp();
        let cwd = dir.0.join("ws");
        fs::create_dir_all(&cwd).expect("cwd");
        let cwd_s = cwd.to_string_lossy().into_owned();
        let token = "sk-provider-access-TOKEN-do-not-store";
        let projects = dir.0.join("claude");
        let mut line = claude_line("msg_tok", "claude-a", &cwd_s, 4, 5, 6, 7);
        line["message"]["content"][0]["text"] = json!(token);
        line["access_token"] = json!(token);
        line["authorization"] = json!(format!("Bearer {token}"));
        write_jsonl(&projects.join("s.jsonl"), &[line]);
        let ledger = dir.0.join("usage").join("tokens.sqlite");
        scan_roots(
            &ledger,
            &projects,
            &dir.0.join("no-codex"),
            &dir.0.join("no-grok"),
        )
        .expect("scan");
        assert!(ledger.is_file());
        let mut hay = Vec::new();
        hay.extend(fs::read(&ledger).expect("read db"));
        for extra in [sibling(&ledger, "-wal"), sibling(&ledger, "-shm")] {
            if extra.is_file() {
                hay.extend(fs::read(&extra).expect("read sidecar"));
            }
        }
        assert!(
            !hay.windows(token.len()).any(|w| w == token.as_bytes()),
            "ledger bytes contain the planted access token"
        );
        // SQLite deletes an empty WAL on clean close. Hold a connection,
        // write, and chmod the live -wal/-shm the same way open_ledger does.
        let conn = open_ledger(&ledger).expect("reopen");
        conn.execute(
            "INSERT INTO scanned_files (path, size, mtime_secs, mtime_nanos)
             VALUES ('mode-probe', 1, 1, 1)
             ON CONFLICT(path) DO UPDATE SET size = excluded.size",
            [],
        )
        .expect("wal write");
        restrict_ledger(&ledger);
        let journal: String = conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .expect("journal mode");
        assert_eq!(journal, "wal");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&ledger).expect("meta").permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
            let wal = sibling(&ledger, "-wal");
            assert!(wal.is_file(), "wal file missing while the ledger is open");
            let wal_mode = fs::metadata(&wal).expect("wal meta").permissions().mode() & 0o777;
            assert_eq!(wal_mode, 0o600);
            let shm = sibling(&ledger, "-shm");
            assert!(shm.is_file(), "shm file missing while the ledger is open");
            let shm_mode = fs::metadata(&shm).expect("shm meta").permissions().mode() & 0o777;
            assert_eq!(shm_mode, 0o600);
            hay.extend(fs::read(&wal).expect("read live wal"));
            hay.extend(fs::read(&shm).expect("read live shm"));
            assert!(
                !hay.windows(token.len()).any(|w| w == token.as_bytes()),
                "wal or shm contains the planted access token"
            );
        }
        let projects_table: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'projects'",
                [],
                |r| r.get(0),
            )
            .expect("projects probe");
        assert_eq!(projects_table, 0);
        drop(conn);
    }

    #[test]
    fn two_turns_on_the_same_utc_day_sum_into_one_days_row() {
        let dir = tmp();
        let ws = canon_dir(&dir.0, "ws");
        let ledger = dir.0.join("tokens.sqlite");
        insert_turns(
            &ledger,
            &[
                ("a", &ws, "2026-09-18T02:00:00Z", 10, 1, 4, 2),
                ("b", &ws, "2026-09-18T23:00:00Z", 4, 5, 6, 7),
            ],
        );
        let report = query(&ledger, None, std::slice::from_ref(&ws)).expect("report");
        assert_eq!(report.days.len(), 1);
        assert_eq!(report.days[0].day, "2026-09-18");
        assert_eq!(report.days[0].input_tokens, 14);
        assert_eq!(report.days[0].output_tokens, 6);
        assert_eq!(report.days[0].cache_read_tokens, 10);
        assert_eq!(report.days[0].cache_write_tokens, 9);
        assert_eq!(report.days[0].turns, 2);
        assert_eq!(report.total.input_tokens, 14);
        assert_eq!(report.total.turns, 2);
    }

    #[test]
    fn rfc3339_and_unix_seconds_on_the_same_utc_day_sum() {
        let dir = tmp();
        let ws = canon_dir(&dir.0, "ws");
        let ledger = dir.0.join("tokens.sqlite");
        // 1789735446 is 2026-09-18 12:44:06 UTC, the shape of a real Grok line.
        // 1789735446000 is that instant in unix milliseconds.
        // An offset RFC3339 keeps the date written in the string.
        insert_turns(
            &ledger,
            &[
                ("rfc", &ws, "2026-09-18T00:30:00Z", 3, 1, 0, 0),
                ("secs", &ws, "1789735446", 7, 2, 1, 0),
                ("millis", &ws, "1789735446000", 1, 1, 1, 0),
                ("offset", &ws, "2026-09-17T20:00:00-07:00", 100, 0, 0, 0),
            ],
        );
        let report = query(&ledger, None, std::slice::from_ref(&ws)).expect("report");
        assert_eq!(report.days.len(), 2);
        assert_eq!(report.days[0].day, "2026-09-17");
        assert_eq!(report.days[0].input_tokens, 100);
        assert_eq!(report.days[1].day, "2026-09-18");
        assert_eq!(report.days[1].input_tokens, 3 + 7 + 1);
        assert_eq!(report.days[1].output_tokens, 1 + 2 + 1);
        assert_eq!(report.days[1].cache_read_tokens, 2);
        assert_eq!(report.days[1].turns, 3);
        assert_ne!(report.days[1].day, "2026-09-17");
        assert_eq!(report.total.input_tokens, 3 + 7 + 1 + 100);
    }

    #[test]
    fn workspace_query_omits_another_paths_day() {
        let dir = tmp();
        let a = canon_dir(&dir.0, "a");
        let b = canon_dir(&dir.0, "b");
        let ledger = dir.0.join("tokens.sqlite");
        insert_turns(
            &ledger,
            &[
                ("a", &a, "2026-09-01T00:00:00Z", 10, 1, 0, 0),
                ("b", &b, "2026-09-02T00:00:00Z", 20, 1, 0, 0),
            ],
        );
        let known = [a.clone(), b.clone()];
        let only_a = query(&ledger, Some(&a), &known).expect("workspace report");
        assert_eq!(only_a.days.len(), 1);
        assert_eq!(only_a.days[0].day, "2026-09-01");
        assert_eq!(only_a.days[0].input_tokens, 10);
        assert_eq!(only_a.total.input_tokens, 10);
        assert!(only_a.days.iter().all(|d| d.day != "2026-09-02"));
        assert_ne!(only_a.total.input_tokens, 30);
        let machine = query(&ledger, None, &known).expect("machine report");
        assert_eq!(machine.days.len(), 2);
        assert_eq!(machine.days[0].day, "2026-09-01");
        assert_eq!(machine.days[0].input_tokens, 10);
        assert_eq!(machine.days[1].day, "2026-09-02");
        assert_eq!(machine.days[1].input_tokens, 20);
        assert_eq!(machine.total.input_tokens, 30);
    }

    #[test]
    fn timestamp_10_does_not_create_a_day_row_and_stays_in_total() {
        let dir = tmp();
        let ws = canon_dir(&dir.0, "ws");
        let ledger = dir.0.join("tokens.sqlite");
        insert_turns(
            &ledger,
            &[
                ("fixture", &ws, "10", 42, 7, 9, 11),
                ("real", &ws, "2026-09-18T00:00:00Z", 8, 1, 2, 3),
            ],
        );
        let report = query(&ledger, None, std::slice::from_ref(&ws)).expect("report");
        assert_eq!(report.days.len(), 1);
        assert_eq!(report.days[0].day, "2026-09-18");
        assert_eq!(report.days[0].input_tokens, 8);
        assert_eq!(report.days[0].turns, 1);
        assert!(report.days.iter().all(|d| d.day != "10"));
        assert_eq!(report.total.input_tokens, 42 + 8);
        assert_eq!(report.total.output_tokens, 7 + 1);
        assert_eq!(report.total.cache_read_tokens, 9 + 2);
        assert_eq!(report.total.cache_write_tokens, 11 + 3);
        assert_eq!(report.total.turns, 2);
        assert_ne!(report.days[0].input_tokens, report.total.input_tokens);
    }
}
