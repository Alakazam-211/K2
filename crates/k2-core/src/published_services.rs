//! Durable published-service rows (`published_services`, migration 0104).
//!
//! A published service is a daemon-owned program for one workspace:
//! a shell command, a cwd, a port, a desired on/off, and an exposure
//! (`local` = `--no-tunnel`, `tunnel` = default nested hostname). Pid
//! is runtime only — `desired` is the SSOT of "should be up".
//!
//! Wire JSON (`GET /cli/publish/list`) is camelCase and never omits
//! keys (`skip_serializing_if` is banned here — empty/null still
//! serializes). Status is computed by the daemon supervisor, not stored.

use rusqlite::{params, Connection, OptionalExtension, Result};
use serde::{Deserialize, Serialize};

/// Internal exposure column. CLI has no `--expose`.
pub const EXPOSE_LOCAL: &str = "local";
pub const EXPOSE_TUNNEL: &str = "tunnel";

pub const DESIRED_RUNNING: &str = "running";
pub const DESIRED_STOPPED: &str = "stopped";

/// `published_services.kind`. Existing rows (pre-0114) are `cmd`.
pub const KIND_CMD: &str = "cmd";
/// Official Dannon skin gateway helper (not a user shell).
pub const KIND_SKIN: &str = "skin";
/// Display sentinel stored in `cmd` for `kind=skin`. Never a shell string.
pub const CMD_SKIN_SENTINEL: &str = "(skin)";
/// `cmd` of an App runtime service row (prd-k2-app-runtime-v1 AR5): the
/// App runtime (`apps` row keyed by this service's id) owns the process;
/// the publish supervisor never starts, stops or respawns it.
pub const CMD_APP_SENTINEL: &str = "(app)";

/// Computed runtime status (not a SQLite column).
pub const STATUS_RUNNING: &str = "running";
pub const STATUS_STARTING: &str = "starting";
pub const STATUS_EXITED: &str = "exited";
pub const STATUS_STOPPED: &str = "stopped";
pub const STATUS_UNHEALTHY: &str = "unhealthy";

/// Nested-label charset (control-plane `bad_label`): lowercase letters,
/// digits, and dashes. Used for both tunnel labels and local-only names
/// so a later exposure flip stays valid.
pub fn is_valid_name(name: &str) -> bool {
    let s = name.trim();
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Normalize a name for a NEW claim (create / run of a new row): charset
/// check plus the reserved nested labels (`skin`, `meet`, `chat`).
pub fn normalize_name(name: &str) -> Result<String, String> {
    let s = normalize_existing_name(name)?;
    if crate::skin::is_reserved_nested_label(&s) {
        return Err(crate::skin::reserved_nested_label_error(&s));
    }
    Ok(s)
}

/// Normalize a name that addresses a row that may already exist (start,
/// stop, rm, frame, re-run): charset only. A `meet` / `chat` row created
/// before those labels were reserved keeps working; `skin` is still
/// refused because it was reserved before any row could hold it.
pub fn normalize_existing_name(name: &str) -> Result<String, String> {
    let s = name.trim().to_ascii_lowercase();
    if !is_valid_name(&s) {
        return Err(
            "bad_label: Invalid label (use lowercase letters, digits, and dashes).".into(),
        );
    }
    if crate::skin::is_reserved_nested_label(&s) && !crate::skin::is_grandfathered_reserved_label(&s)
    {
        return Err(crate::skin::reserved_nested_label_error(&s));
    }
    Ok(s)
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Durable row. `status` / `url` / `target` are computed at list time.
#[derive(Debug, Clone)]
pub struct PublishedService {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub cmd: String,
    pub cwd: String,
    pub port: i64,
    pub expose: String,
    pub desired: String,
    pub pid: Option<i64>,
    pub last_exit_code: Option<i64>,
    pub last_started_at: Option<i64>,
    pub last_exited_at: Option<i64>,
    pub error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    /// `cmd` or `skin`. Always present after 0114.
    pub kind: String,
    /// Workspace-relative UI dir. Empty = bundled login + Thread chrome.
    pub skin_root: String,
    /// Framing policy (migration 0143): `''` default, `none`, or extra
    /// origins. See [`crate::frame_policy`].
    pub frame_ancestors: String,
    /// Startup grace (migration 0177): how long the service may take to open
    /// its port, in ms. `None` = the daemon default (15 s).
    pub probe_timeout_ms: Option<i64>,
    /// Migration 0177: a tunnelled service is inside its grace and its
    /// nested hostname is not published yet.
    pub hostname_pending: bool,
}

/// Startup grace when a service stores none (`K2_PUBLISH_PROBE_MS` in the
/// daemon's environment overrides it; tests use that).
pub const PROBE_TIMEOUT_DEFAULT_MS: u64 = 15_000;

/// Longest startup grace `--probe-timeout` accepts (15 min).
pub const PROBE_TIMEOUT_MAX_MS: u64 = 15 * 60 * 1000;

/// Parse a `--probe-timeout` value: `90s`, `10m`, `1500ms`, `1m30s` or a
/// bare number of seconds. Must be > 0 and at most 15 min.
pub fn parse_probe_timeout(raw: &str) -> Result<u64, String> {
    let t = raw.trim().to_ascii_lowercase();
    let bad = || {
        format!(
            "--probe-timeout must be a duration like 90s, 10m or 1500ms (at most 15m), got '{}'",
            raw.trim()
        )
    };
    if t.is_empty() {
        return Err(bad());
    }
    let mut total: u64 = 0;
    if t.bytes().all(|b| b.is_ascii_digit()) {
        total = t.parse::<u64>().map_err(|_| bad())?.checked_mul(1000).ok_or_else(bad)?;
    } else {
        let mut rest = t.as_str();
        while !rest.is_empty() {
            let digits = rest.bytes().take_while(|b| b.is_ascii_digit()).count();
            if digits == 0 {
                return Err(bad());
            }
            let n: u64 = rest[..digits].parse().map_err(|_| bad())?;
            rest = &rest[digits..];
            let unit_len = rest.bytes().take_while(|b| b.is_ascii_alphabetic()).count();
            let mult = match &rest[..unit_len] {
                "ms" => 1,
                "s" => 1000,
                "m" => 60_000,
                "h" => 3_600_000,
                _ => return Err(bad()),
            };
            rest = &rest[unit_len..];
            total = n
                .checked_mul(mult)
                .and_then(|v| total.checked_add(v))
                .ok_or_else(bad)?;
        }
    }
    if total == 0 {
        return Err(bad());
    }
    if total > PROBE_TIMEOUT_MAX_MS {
        return Err(format!(
            "--probe-timeout is at most 15m (got {}); a service that needs longer should open its port first and build behind it",
            raw.trim()
        ));
    }
    Ok(total)
}

/// Frozen HTTP Service object (`GET /cli/publish/list`).
/// Every field is always present; nulls serialize as JSON null.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ServiceJson {
    /// `published_services.id`. The app id stored on grants and roles.
    /// Always present. Not the publish name.
    pub id: String,
    /// Workspace id (`published_services.project_id`). Always present.
    pub project_id: String,
    pub name: String,
    pub cmd: String,
    pub cwd: String,
    pub port: u16,
    pub expose: String,
    pub desired: String,
    pub status: String,
    pub pid: Option<i64>,
    pub url: Option<String>,
    pub target: String,
    pub error: Option<String>,
    pub last_exit_code: Option<i32>,
    /// Always present (`cmd` or `skin`). Existing rows serialize as `cmd`.
    pub kind: String,
    /// Workspace-relative UI dir. Empty = bundled skin chrome. Always present
    /// on the wire; `default` only so older JSON still deserializes.
    #[serde(default)]
    pub skin_root: String,
    /// Stored framing setting (FA4): `""` = default, `"none"`, or the
    /// owner's extra origins, space-separated. `--skin` Apps only; a `--cmd`
    /// App sets its own header.
    #[serde(default)]
    pub frame_ancestors: String,
    /// The `frame-ancestors` source list the App answers with, or null for
    /// a `--cmd` App (K2 does not touch its responses).
    #[serde(default)]
    pub frame_policy: Option<String>,
    /// `--cmd` Apps: set when the App's own `GET /` is HTML without
    /// `frame-ancestors` or `X-Frame-Options` (daemon probe). Null otherwise.
    #[serde(default)]
    pub frame_warning: Option<String>,
    /// Set while a service from before the vault still inherits the
    /// daemon's whole environment (`legacy` mode): that way of running
    /// ends in 0.46.1 (Rosson 2026-10-10; moved from 0.46.0 by the
    /// 2026-10-14 cross-over plan). Null for every other service.
    #[serde(default)]
    pub env_deprecation: Option<String>,
    /// `legacy` (inherits the daemon's environment) or `clean`, for a
    /// `--cmd` service; null for a `--skin` App (no process env of its
    /// own). Added in 0.45.4 so agents and scripts can branch on it.
    #[serde(default)]
    pub env_mode: Option<String>,
    /// The K2 version the `legacy` mode ends in (`0.46.1`) on a legacy
    /// service; null otherwise. `env_deprecation` keeps the full notice.
    #[serde(default)]
    pub env_removed_in: Option<String>,
    /// This daemon IS that release (or later): say "ends now", never "ends
    /// 0.46.4" on a box running 0.46.4 ([`crate::app_grace::cutover_now`]).
    /// Only sent when true.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cutover_now: bool,
    /// `--skin` Apps (S24-15): true while the running helper is an older one
    /// that still signs guests in as `host` (not bound to this App). Only
    /// sent when true.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub helper_needs_restart: bool,
    /// 0.46.0: the service runs in clean mode (the 0.46.4 behaviour, opted
    /// into early: system PATH, its own HOME, owner-token calls refused).
    /// Only sent when true.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub clean_mode: bool,
    /// True when this row's name is a reserved nested label (`meet`,
    /// `chat`) it held before the reservation. It keeps working; a new
    /// claim of that name is refused. Only sent when true.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reserved_label: bool,
    /// Stored startup grace in ms (`--probe-timeout`); null = the daemon
    /// default. Always present on the wire.
    #[serde(default)]
    pub probe_timeout_ms: Option<i64>,
    /// The grace this service gets, in ms (stored value or the default).
    #[serde(default)]
    pub probe_timeout_effective_ms: i64,
    /// While `status` is `starting`: ms left before the grace runs out.
    /// Null otherwise.
    #[serde(default)]
    pub starting_left_ms: Option<i64>,
    /// True while a tunnelled service waits inside its grace and its
    /// nested hostname is not published yet. Always present.
    #[serde(default)]
    pub hostname_pending: bool,
    /// 0.46.3: a `--cmd` service the App runtime doesn't run (no wall) says
    /// so everywhere it shows: `legacy (--cmd) — move it to an App before
    /// 0.46.4` ([`crate::app_grace::legacy_label`]). Null for Apps and
    /// `--skin`.
    #[serde(default)]
    pub legacy_label: Option<String>,
    /// With `legacy_label`: where to read how (`k2 study app-move`). Null
    /// otherwise.
    #[serde(default)]
    pub legacy_hint: Option<String>,
    /// A legacy service whose public label an App of the same workspace
    /// serves now (`k2 app label take`): that App's id. The service keeps
    /// running, unrouted (`k2 app label return` moves the label back).
    #[serde(default)]
    pub label_moved_to: Option<String>,
    /// R8 (prd-env-to-vault-v1): `.env.k2` references the vault refused at
    /// the last start (`KEY: code`, names only), e.g. a server secret not
    /// granted to this service. Omitted when none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env_refused: Vec<String>,
}

impl ServiceJson {
    pub fn from_row(
        row: &PublishedService,
        status: &str,
        url: Option<String>,
        pid: Option<i64>,
    ) -> Self {
        let port = if row.port > 0 && row.port <= u16::MAX as i64 {
            row.port as u16
        } else {
            0
        };
        Self {
            id: row.id.clone(),
            project_id: row.project_id.clone(),
            name: row.name.clone(),
            cmd: row.cmd.clone(),
            cwd: row.cwd.clone(),
            port,
            expose: row.expose.clone(),
            desired: row.desired.clone(),
            status: status.to_string(),
            pid,
            url,
            target: format!("127.0.0.1:{port}"),
            error: row.error.clone(),
            last_exit_code: row.last_exit_code.map(|c| c as i32),
            kind: if row.kind.trim().is_empty() {
                KIND_CMD.to_string()
            } else {
                row.kind.clone()
            },
            skin_root: row.skin_root.clone(),
            frame_ancestors: row.frame_ancestors.clone(),
            frame_policy: if row.kind == KIND_SKIN {
                Some(
                    crate::frame_policy::FramePolicy::from_stored(&row.frame_ancestors)
                        .map(|p| p.sources().to_string())
                        .unwrap_or_else(|e| format!("invalid ({e})")),
                )
            } else {
                None
            },
            frame_warning: None,
            env_deprecation: None,
            env_mode: None,
            env_removed_in: None,
            cutover_now: false,
            helper_needs_restart: false,
            clean_mode: false,
            reserved_label: crate::skin::is_reserved_nested_label(&row.name),
            probe_timeout_ms: row.probe_timeout_ms,
            probe_timeout_effective_ms: row
                .probe_timeout_ms
                .unwrap_or(PROBE_TIMEOUT_DEFAULT_MS as i64),
            starting_left_ms: None,
            hostname_pending: row.hostname_pending,
            legacy_label: None,
            legacy_hint: None,
            label_moved_to: None,
            env_refused: vec![],
        }
    }
}

const COLS: &str = "id, project_id, name, cmd, cwd, port, expose, desired, pid, \
     last_exit_code, last_started_at, last_exited_at, error, created_at, updated_at, \
     kind, skin_root, frame_ancestors, probe_timeout_ms, hostname_pending";

fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PublishedService> {
    Ok(PublishedService {
        id: row.get(0)?,
        project_id: row.get(1)?,
        name: row.get(2)?,
        cmd: row.get(3)?,
        cwd: row.get(4)?,
        port: row.get(5)?,
        expose: row.get(6)?,
        desired: row.get(7)?,
        pid: row.get(8)?,
        last_exit_code: row.get(9)?,
        last_started_at: row.get(10)?,
        last_exited_at: row.get(11)?,
        error: row.get(12)?,
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
        kind: row.get::<_, String>(15).unwrap_or_else(|_| KIND_CMD.to_string()),
        skin_root: row.get::<_, String>(16).unwrap_or_default(),
        frame_ancestors: row.get(17)?,
        probe_timeout_ms: row.get(18)?,
        hostname_pending: row.get::<_, i64>(19)? != 0,
    })
}

/// Insert a new service. `name` is stored lowercase. UNIQUE(project_id, name)
/// is enforced by SQLite — callers map the constraint error.
pub fn insert(
    conn: &Connection,
    project_id: &str,
    name: &str,
    cmd: &str,
    cwd: &str,
    port: u16,
    expose: &str,
    desired: &str,
    kind: &str,
    skin_root: &str,
) -> Result<PublishedService> {
    let name = name.trim().to_ascii_lowercase();
    let kind = if kind.trim().is_empty() {
        KIND_CMD
    } else {
        kind.trim()
    };
    let id = uuid::Uuid::new_v4().to_string();
    let now = now_unix();
    conn.execute(
        "INSERT INTO published_services \
         (id, project_id, name, cmd, cwd, port, expose, desired, created_at, updated_at, kind, skin_root) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9, ?10, ?11)",
        params![
            id,
            project_id,
            name,
            cmd,
            cwd,
            port as i64,
            expose,
            desired,
            now,
            kind,
            skin_root,
        ],
    )?;
    get_by_id(conn, &id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)
}

/// Set a row's exposure (`local` | `tunnel`). `k2 app label take` makes a
/// legacy service `local` while its App serves the label (so a restart of
/// the legacy service never points the label back at it); `label return`
/// makes it `tunnel` again.
pub fn set_expose(conn: &Connection, id: &str, expose: &str) -> Result<bool> {
    let n = conn.execute(
        "UPDATE published_services SET expose = ?2, hostname_pending = 0, updated_at = ?3 WHERE id = ?1",
        params![id, expose, now_unix()],
    )?;
    Ok(n > 0)
}

pub fn get_by_id(conn: &Connection, id: &str) -> Result<Option<PublishedService>> {
    conn.query_row(
        &format!("SELECT {COLS} FROM published_services WHERE id = ?1"),
        params![id],
        map_row,
    )
    .optional()
}

pub fn get_by_project_name(
    conn: &Connection,
    project_id: &str,
    name: &str,
) -> Result<Option<PublishedService>> {
    let name = name.trim().to_ascii_lowercase();
    conn.query_row(
        &format!(
            "SELECT {COLS} FROM published_services WHERE project_id = ?1 AND name = ?2"
        ),
        params![project_id, name],
        map_row,
    )
    .optional()
}

pub fn list_for_project(conn: &Connection, project_id: &str) -> Result<Vec<PublishedService>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLS} FROM published_services WHERE project_id = ?1 ORDER BY name"
    ))?;
    let rows = stmt.query_map(params![project_id], map_row)?;
    rows.collect()
}

/// Every published service on the box (the hire-key App picker).
pub fn list_all(conn: &Connection) -> Result<Vec<PublishedService>> {
    let mut stmt = conn.prepare(&format!("SELECT {COLS} FROM published_services ORDER BY name, created_at"))?;
    let rows = stmt.query_map([], map_row)?;
    rows.collect()
}

/// Ids of every row of one kind (`skin` | `cmd`), any desired state. The
/// per-App login backfill (S24-10) copies box logins onto each `skin` id.
pub fn ids_of_kind(conn: &Connection, kind: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT id FROM published_services WHERE kind = ?1 ORDER BY created_at, name",
    )?;
    let rows = stmt.query_map(params![kind], |r| r.get::<_, String>(0))?;
    rows.collect()
}

/// Every row with `desired = running` — boot reattach/respawn.
pub fn list_desired_running(conn: &Connection) -> Result<Vec<PublishedService>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLS} FROM published_services WHERE desired = 'running' ORDER BY created_at, name"
    ))?;
    let rows = stmt.query_map([], map_row)?;
    rows.collect()
}

pub fn update_spec(
    conn: &Connection,
    project_id: &str,
    name: &str,
    cmd: &str,
    cwd: &str,
    port: u16,
    expose: &str,
    kind: &str,
    skin_root: &str,
) -> Result<bool> {
    let name = name.trim().to_ascii_lowercase();
    let kind = if kind.trim().is_empty() {
        KIND_CMD
    } else {
        kind.trim()
    };
    let n = conn.execute(
        "UPDATE published_services SET cmd = ?1, cwd = ?2, port = ?3, expose = ?4, \
         kind = ?5, skin_root = ?6, updated_at = ?7 WHERE project_id = ?8 AND name = ?9",
        params![
            cmd,
            cwd,
            port as i64,
            expose,
            kind,
            skin_root,
            now_unix(),
            project_id,
            name
        ],
    )?;
    Ok(n > 0)
}

pub fn set_desired(
    conn: &Connection,
    project_id: &str,
    name: &str,
    desired: &str,
) -> Result<bool> {
    let name = name.trim().to_ascii_lowercase();
    let n = conn.execute(
        "UPDATE published_services SET desired = ?1, updated_at = ?2 \
         WHERE project_id = ?3 AND name = ?4",
        params![desired, now_unix(), project_id, name],
    )?;
    Ok(n > 0)
}

pub fn set_runtime(
    conn: &Connection,
    project_id: &str,
    name: &str,
    pid: Option<i64>,
    error: Option<&str>,
    last_started_at: Option<i64>,
) -> Result<()> {
    let name = name.trim().to_ascii_lowercase();
    conn.execute(
        "UPDATE published_services SET pid = ?1, error = ?2, last_started_at = COALESCE(?3, last_started_at), \
         updated_at = ?4 WHERE project_id = ?5 AND name = ?6",
        params![pid, error, last_started_at, now_unix(), project_id, name],
    )?;
    Ok(())
}

/// Child exited: clear pid, stamp exit code + time. Does NOT change `desired`.
pub fn mark_exited(
    conn: &Connection,
    project_id: &str,
    name: &str,
    exit_code: Option<i64>,
) -> Result<()> {
    let name = name.trim().to_ascii_lowercase();
    let now = now_unix();
    conn.execute(
        "UPDATE published_services SET pid = NULL, last_exit_code = ?1, last_exited_at = ?2, \
         updated_at = ?2 WHERE project_id = ?3 AND name = ?4",
        params![exit_code, now, project_id, name],
    )?;
    Ok(())
}

/// [`mark_exited`] for one specific child: a no-op when the row already
/// records a different (newer) pid, so a late exit of a replaced helper
/// cannot clear its successor.
pub fn mark_exited_if_pid(
    conn: &Connection,
    project_id: &str,
    name: &str,
    pid: i64,
    exit_code: Option<i64>,
) -> Result<bool> {
    let name = name.trim().to_ascii_lowercase();
    let now = now_unix();
    let n = conn.execute(
        "UPDATE published_services SET pid = NULL, last_exit_code = ?1, last_exited_at = ?2, \
         updated_at = ?2 WHERE project_id = ?3 AND name = ?4 AND (pid IS NULL OR pid = ?5)",
        params![exit_code, now, project_id, name, pid],
    )?;
    Ok(n > 0)
}

/// Hostname-step fail: stop-persist so boot cannot resurrect as local-only.
pub fn mark_hostname_failed(
    conn: &Connection,
    project_id: &str,
    name: &str,
    error: &str,
) -> Result<()> {
    let name = name.trim().to_ascii_lowercase();
    let now = now_unix();
    conn.execute(
        "UPDATE published_services SET desired = 'stopped', pid = NULL, error = ?1, \
         hostname_pending = 0, last_exited_at = ?2, updated_at = ?2 \
         WHERE project_id = ?3 AND name = ?4",
        params![error, now, project_id, name],
    )?;
    Ok(())
}

/// Store the framing setting (already validated by
/// [`crate::frame_policy`]). Returns false when no such service.
pub fn set_frame_ancestors(
    conn: &Connection,
    project_id: &str,
    name: &str,
    stored: &str,
) -> Result<bool> {
    let name = name.trim().to_ascii_lowercase();
    let n = conn.execute(
        "UPDATE published_services SET frame_ancestors = ?1, updated_at = ?2 \
         WHERE project_id = ?3 AND name = ?4",
        params![stored, now_unix(), project_id, name],
    )?;
    Ok(n > 0)
}

/// Store the startup grace (`None` = back to the daemon default). The
/// caller validated it ([`parse_probe_timeout`]).
pub fn set_probe_timeout(
    conn: &Connection,
    project_id: &str,
    name: &str,
    ms: Option<u64>,
) -> Result<bool> {
    let name = name.trim().to_ascii_lowercase();
    let n = conn.execute(
        "UPDATE published_services SET probe_timeout_ms = ?1, updated_at = ?2 \
         WHERE project_id = ?3 AND name = ?4",
        params![ms.map(|v| v as i64), now_unix(), project_id, name],
    )?;
    Ok(n > 0)
}

/// Mark (or clear) "nested hostname not yet published" on a tunnelled
/// service that is inside its startup grace.
pub fn set_hostname_pending(
    conn: &Connection,
    project_id: &str,
    name: &str,
    pending: bool,
) -> Result<bool> {
    let name = name.trim().to_ascii_lowercase();
    let n = conn.execute(
        "UPDATE published_services SET hostname_pending = ?1, updated_at = ?2 \
         WHERE project_id = ?3 AND name = ?4",
        params![pending as i64, now_unix(), project_id, name],
    )?;
    Ok(n > 0)
}

pub fn set_error(conn: &Connection, project_id: &str, name: &str, error: Option<&str>) -> Result<()> {
    let name = name.trim().to_ascii_lowercase();
    conn.execute(
        "UPDATE published_services SET error = ?1, updated_at = ?2 WHERE project_id = ?3 AND name = ?4",
        params![error, now_unix(), project_id, name],
    )?;
    Ok(())
}

pub fn delete(conn: &Connection, project_id: &str, name: &str) -> Result<bool> {
    let name = name.trim().to_ascii_lowercase();
    let n = conn.execute(
        "DELETE FROM published_services WHERE project_id = ?1 AND name = ?2",
        params![project_id, name],
    )?;
    Ok(n > 0)
}

pub fn is_unique_violation(err: &rusqlite::Error) -> bool {
    match err {
        rusqlite::Error::SqliteFailure(e, _) => {
            e.code == rusqlite::ErrorCode::ConstraintViolation
                || e.extended_code == 2067 // SQLITE_CONSTRAINT_UNIQUE
                || e.extended_code == 1555 // SQLITE_CONSTRAINT_PRIMARYKEY
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn fresh() -> Connection {
        crate::db::isolated_test_connection()
    }

    fn project(conn: &Connection, path: &str) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT OR IGNORE INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            params![id, "pub-test", path],
        )
        .expect("insert project");
        id
    }

    #[test]
    fn migration_applies_and_roundtrips() {
        let conn = fresh();
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='published_services'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1, "0104 must create published_services");

        let pid = project(&conn, "/tmp/pub-rt");
        let row = insert(
            &conn,
            &pid,
            "Web",
            "npm start",
            "/tmp/pub-rt",
            3000,
            EXPOSE_TUNNEL,
            DESIRED_STOPPED,
            KIND_CMD,
            "",
        )
        .expect("insert");
        assert_eq!(row.name, "web", "name is stored lowercase");
        assert_eq!(row.port, 3000);
        assert_eq!(row.expose, EXPOSE_TUNNEL);
        assert_eq!(row.desired, DESIRED_STOPPED);
        assert_eq!(row.kind, KIND_CMD);
        assert_eq!(row.skin_root, "");
        assert!(row.pid.is_none());
        assert!(row.error.is_none());

        let listed = list_for_project(&conn, &pid).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].cmd, "npm start");
        assert_eq!(listed[0].cwd, "/tmp/pub-rt");
    }

    #[test]
    fn unique_project_name_is_enforced() {
        let conn = fresh();
        let a = project(&conn, "/tmp/pub-a");
        let b = project(&conn, "/tmp/pub-b");
        insert(&conn, &a, "web", "a", "/a", 1, EXPOSE_LOCAL, DESIRED_STOPPED, KIND_CMD, "").unwrap();
        let dup = insert(&conn, &a, "web", "a2", "/a", 2, EXPOSE_LOCAL, DESIRED_STOPPED, KIND_CMD, "");
        assert!(dup.is_err(), "same project+name must UNIQUE-fail");
        assert!(is_unique_violation(dup.as_ref().unwrap_err()));
        // Same name on another workspace is fine (local-only may collide).
        insert(&conn, &b, "web", "b", "/b", 3, EXPOSE_LOCAL, DESIRED_STOPPED, KIND_CMD, "")
            .expect("cross-workspace name collision is allowed");
        assert_eq!(list_for_project(&conn, &a).unwrap().len(), 1);
        assert_eq!(list_for_project(&conn, &b).unwrap().len(), 1);
    }

    #[test]
    fn list_does_not_hide_fields() {
        let conn = fresh();
        let pid = project(&conn, "/tmp/pub-json");
        let row = insert(
            &conn,
            &pid,
            "api",
            "python -m http.server",
            "/tmp/pub-json",
            8090,
            EXPOSE_LOCAL,
            DESIRED_RUNNING,
            KIND_CMD,
            "",
        )
        .unwrap();
        set_runtime(&conn, &pid, "api", Some(4242), None, Some(now_unix())).unwrap();
        let row = get_by_id(&conn, &row.id).unwrap().unwrap();
        let json = ServiceJson::from_row(&row, STATUS_RUNNING, None, Some(4242));
        let v = serde_json::to_value(&json).unwrap();
        for key in [
            "id",
            "projectId",
            "name",
            "cmd",
            "cwd",
            "port",
            "expose",
            "desired",
            "status",
            "pid",
            "url",
            "target",
            "error",
            "lastExitCode",
            "kind",
            "skinRoot",
            "frameAncestors",
            "framePolicy",
            "frameWarning",
            "envDeprecation",
            "envMode",
            "envRemovedIn",
            "probeTimeoutMs",
            "probeTimeoutEffectiveMs",
            "startingLeftMs",
            "hostnamePending",
        ] {
            assert!(v.get(key).is_some(), "Service JSON must carry {key}");
        }
        assert!(v["probeTimeoutMs"].is_null(), "no stored grace = null");
        assert_eq!(v["probeTimeoutEffectiveMs"], PROBE_TIMEOUT_DEFAULT_MS);
        assert_eq!(v["hostnamePending"], false);
        assert_eq!(v["id"], row.id, "id is published_services.id");
        assert_eq!(v["projectId"], pid);
        assert!(v.get("project_id").is_none(), "wire key is projectId");
        assert_eq!(v["kind"], KIND_CMD);
        assert_eq!(v["skinRoot"], "", "empty skinRoot is present, never omitted");
        assert!(v["url"].is_null(), "local-only url is explicit null");
        assert!(v["error"].is_null());
        assert!(v["lastExitCode"].is_null());
        assert_eq!(v["pid"], 4242);
        assert_eq!(v["target"], "127.0.0.1:8090");
        assert_eq!(v["expose"], "local");
        assert_eq!(v["desired"], "running");
        assert_eq!(v["status"], "running");
    }

    #[test]
    fn probe_timeout_parses_units_and_enforces_the_cap() {
        assert_eq!(parse_probe_timeout("90s").unwrap(), 90_000);
        assert_eq!(parse_probe_timeout("10m").unwrap(), 600_000);
        assert_eq!(parse_probe_timeout("1500ms").unwrap(), 1_500);
        assert_eq!(parse_probe_timeout("1m30s").unwrap(), 90_000);
        assert_eq!(parse_probe_timeout("45").unwrap(), 45_000, "bare number = seconds");
        assert_eq!(parse_probe_timeout(" 15M ").unwrap(), PROBE_TIMEOUT_MAX_MS);
        let over = parse_probe_timeout("16m").unwrap_err();
        assert!(over.contains("at most 15m"), "{over}");
        assert!(parse_probe_timeout("1h").unwrap_err().contains("at most 15m"));
        for bad in ["", "0", "0s", "abc", "10x", "s", "-5s", "1.5m", "10m junk"] {
            let e = parse_probe_timeout(bad).expect_err(bad);
            assert!(e.contains("--probe-timeout"), "{bad}: {e}");
        }
    }

    #[test]
    fn probe_timeout_and_hostname_pending_round_trip() {
        let conn = fresh();
        let pid = project(&conn, "/tmp/pub-grace");
        insert(&conn, &pid, "hearth", "sh publish.sh", "/tmp/pub-grace", 3047, EXPOSE_TUNNEL, DESIRED_STOPPED, KIND_CMD, "")
            .unwrap();
        let row = get_by_project_name(&conn, &pid, "hearth").unwrap().unwrap();
        assert_eq!(row.probe_timeout_ms, None);
        assert!(!row.hostname_pending);

        assert!(set_probe_timeout(&conn, &pid, "hearth", Some(600_000)).unwrap());
        assert!(set_hostname_pending(&conn, &pid, "hearth", true).unwrap());
        let row = get_by_project_name(&conn, &pid, "hearth").unwrap().unwrap();
        assert_eq!(row.probe_timeout_ms, Some(600_000));
        assert!(row.hostname_pending);
        let v = serde_json::to_value(ServiceJson::from_row(&row, STATUS_STARTING, None, None)).unwrap();
        assert_eq!(v["probeTimeoutMs"], 600_000);
        assert_eq!(v["probeTimeoutEffectiveMs"], 600_000);
        assert_eq!(v["hostnamePending"], true);

        // A failed hostname step clears the pending mark with the row.
        mark_hostname_failed(&conn, &pid, "hearth", "probe failed").unwrap();
        let row = get_by_project_name(&conn, &pid, "hearth").unwrap().unwrap();
        assert!(!row.hostname_pending);
        assert_eq!(row.probe_timeout_ms, Some(600_000), "the grace survives a failure");

        assert!(set_probe_timeout(&conn, &pid, "hearth", None).unwrap());
        let row = get_by_project_name(&conn, &pid, "hearth").unwrap().unwrap();
        assert_eq!(row.probe_timeout_ms, None, "None = back to the default");
    }

    #[test]
    fn mark_exited_clears_pid_keeps_desired() {
        let conn = fresh();
        let pid = project(&conn, "/tmp/pub-exit");
        insert(
            &conn,
            &pid,
            "worker",
            "sleep 1",
            "/tmp/pub-exit",
            9,
            EXPOSE_LOCAL,
            DESIRED_RUNNING,
            KIND_CMD,
            "",
        )
        .unwrap();
        set_runtime(&conn, &pid, "worker", Some(99), None, Some(now_unix())).unwrap();
        mark_exited(&conn, &pid, "worker", Some(1)).unwrap();
        let row = get_by_project_name(&conn, &pid, "worker").unwrap().unwrap();
        assert!(row.pid.is_none());
        assert_eq!(row.last_exit_code, Some(1));
        assert_eq!(row.desired, DESIRED_RUNNING, "P4: desired stays running");
        assert!(row.last_exited_at.is_some());
    }

    #[test]
    fn late_exit_of_a_replaced_child_keeps_the_new_pid() {
        let conn = fresh();
        let pid = project(&conn, "/tmp/pub-exit-pid");
        insert(
            &conn, &pid, "agents", CMD_SKIN_SENTINEL, "/tmp/pub-exit-pid", 9,
            EXPOSE_LOCAL, DESIRED_RUNNING, KIND_SKIN, "",
        )
        .unwrap();
        set_runtime(&conn, &pid, "agents", Some(200), None, Some(now_unix())).unwrap();
        // The old child (pid 100) reports its exit after pid 200 took over.
        assert!(!mark_exited_if_pid(&conn, &pid, "agents", 100, Some(0)).unwrap());
        let row = get_by_project_name(&conn, &pid, "agents").unwrap().unwrap();
        assert_eq!(row.pid, Some(200), "a late exit must not clear the successor");
        assert!(mark_exited_if_pid(&conn, &pid, "agents", 200, Some(1)).unwrap());
        let row = get_by_project_name(&conn, &pid, "agents").unwrap().unwrap();
        assert_eq!(row.pid, None);
        assert_eq!(row.last_exit_code, Some(1));
    }

    #[test]
    fn hostname_fail_does_not_leave_desired_running() {
        let conn = fresh();
        let pid = project(&conn, "/tmp/pub-hostfail");
        insert(
            &conn,
            &pid,
            "web",
            "npm start",
            "/tmp/pub-hostfail",
            3000,
            EXPOSE_TUNNEL,
            DESIRED_RUNNING,
            KIND_CMD,
            "",
        )
        .unwrap();
        mark_hostname_failed(&conn, &pid, "web", "claim did not land").unwrap();
        let row = get_by_project_name(&conn, &pid, "web").unwrap().unwrap();
        assert_eq!(row.desired, DESIRED_STOPPED);
        assert_eq!(row.expose, EXPOSE_TUNNEL, "expose stays tunnel");
        assert!(row.pid.is_none());
        assert_eq!(row.error.as_deref(), Some("claim did not land"));
        let boot = list_desired_running(&conn).unwrap();
        assert!(
            !boot.iter().any(|r| r.project_id == pid && r.name == "web"),
            "boot must not resurrect a hostname-failed row"
        );
    }

    #[test]
    fn name_charset_matches_bad_label() {
        assert!(normalize_name("Web").unwrap() == "web");
        assert!(normalize_name("a-b-1").is_ok());
        assert!(normalize_name("").is_err());
        assert!(normalize_name("Has_Underscore").is_err());
        assert!(normalize_name("ok!").is_err());
    }

    #[test]
    fn reserved_nested_label_skin_is_400_loud() {
        let err = normalize_name("skin").expect_err("skin is reserved");
        assert!(
            err.contains("reserved_label"),
            "must fail loud with reserved_label, got {err}"
        );
        assert!(
            err.contains("Pick another nested label"),
            "must tell them to pick another name, not a Caddy port: {err}"
        );
        assert!(
            !err.contains("38472"),
            "must not teach 38472 as a publish target: {err}"
        );
        assert!(normalize_name("Skin").is_err());
        assert!(normalize_name("staging").is_ok());
        assert!(normalize_existing_name("skin").is_err(), "skin stays strict");
    }

    #[test]
    fn reserved_nested_labels_meet_and_chat_refuse_new_claims() {
        for name in ["meet", "Meet", " chat ", "CHAT"] {
            let err = normalize_name(name).expect_err("meet/chat are reserved");
            assert!(err.starts_with("reserved_label: '"), "{err}");
            assert!(err.contains("Pick another nested label"), "{err}");
        }
        assert!(normalize_name("meeting").is_ok());
        assert!(normalize_name("chats").is_ok());
        // A row made before the reservation is still addressable.
        assert_eq!(normalize_existing_name(" Chat ").unwrap(), "chat");
        assert_eq!(normalize_existing_name("meet").unwrap(), "meet");
        assert!(normalize_existing_name("Bad_Name").is_err());
    }

    #[test]
    fn preexisting_chat_row_lists_and_deletes() {
        let conn = fresh();
        let pid = project(&conn, "/tmp/pub-chat-old");
        insert(
            &conn,
            &pid,
            "chat",
            "python3 -m http.server 4100",
            "/tmp",
            4100,
            EXPOSE_TUNNEL,
            DESIRED_RUNNING,
            KIND_CMD,
            "",
        )
        .expect("rows written before the reservation still load");
        let rows = list_for_project(&conn, &pid).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "chat");
        assert!(
            list_desired_running(&conn).unwrap().iter().any(|r| r.name == "chat"),
            "boot still resurrects it"
        );
        let name = normalize_existing_name("chat").unwrap();
        assert!(delete(&conn, &pid, &name).unwrap(), "rm of the old row works");
        assert!(list_for_project(&conn, &pid).unwrap().is_empty());
    }

    #[test]
    fn kind_defaults_cmd_and_skin_roundtrips() {
        let conn = fresh();
        let pid = project(&conn, "/tmp/pub-kind");
        let cmd_row = insert(
            &conn,
            &pid,
            "web",
            "npm start",
            "/tmp/pub-kind",
            3000,
            EXPOSE_LOCAL,
            DESIRED_STOPPED,
            KIND_CMD,
            "",
        )
        .unwrap();
        assert_eq!(cmd_row.kind, KIND_CMD);
        assert_eq!(cmd_row.skin_root, "");
        let json = ServiceJson::from_row(&cmd_row, STATUS_STOPPED, None, None);
        let v = serde_json::to_value(&json).unwrap();
        assert_eq!(v["kind"], "cmd", "kind is always present camelCase");
        assert_eq!(v["skinRoot"], "", "cmd rows still serialize skinRoot");

        let skin_row = insert(
            &conn,
            &pid,
            "agents",
            CMD_SKIN_SENTINEL,
            "/tmp/pub-kind",
            8788,
            EXPOSE_LOCAL,
            DESIRED_STOPPED,
            KIND_SKIN,
            "ui",
        )
        .unwrap();
        assert_eq!(skin_row.kind, KIND_SKIN);
        assert_eq!(skin_row.cmd, CMD_SKIN_SENTINEL);
        assert_eq!(skin_row.skin_root, "ui");
        let json = ServiceJson::from_row(&skin_row, STATUS_STOPPED, None, None);
        let v = serde_json::to_value(&json).unwrap();
        assert_eq!(v["kind"], "skin");
        assert_eq!(v["cmd"], CMD_SKIN_SENTINEL);
        assert_eq!(v["skinRoot"], "ui");

        let bad = insert(
            &conn,
            &pid,
            "nope",
            "x",
            "/tmp/pub-kind",
            1,
            EXPOSE_LOCAL,
            DESIRED_STOPPED,
            "python",
            "",
        );
        assert!(bad.is_err(), "CHECK (kind IN cmd|skin) must reject other kinds");
    }

    /// 0143 + FA4: the framing setting defaults to '' (the App default),
    /// round-trips, and the list JSON shows the raw value and the policy.
    #[test]
    fn frame_ancestors_defaults_roundtrips_and_shows_policy() {
        let conn = fresh();
        let pid = project(&conn, "/tmp/pub-frame");
        let skin = insert(
            &conn, &pid, "agents", CMD_SKIN_SENTINEL, "/tmp/pub-frame", 8788,
            EXPOSE_LOCAL, DESIRED_STOPPED, KIND_SKIN, "",
        )
        .unwrap();
        assert_eq!(skin.frame_ancestors, "", "existing and new rows start at the default");
        let v = serde_json::to_value(ServiceJson::from_row(&skin, STATUS_STOPPED, None, None))
            .unwrap();
        assert_eq!(v["frameAncestors"], "");
        assert_eq!(v["framePolicy"], "'self' tauri://localhost http://tauri.localhost");
        assert!(v["frameWarning"].is_null());

        assert!(set_frame_ancestors(&conn, &pid, "AGENTS", "https://partner.example").unwrap());
        let row = get_by_project_name(&conn, &pid, "agents").unwrap().unwrap();
        assert_eq!(row.frame_ancestors, "https://partner.example");
        let v = serde_json::to_value(ServiceJson::from_row(&row, STATUS_STOPPED, None, None))
            .unwrap();
        assert_eq!(
            v["framePolicy"],
            "'self' tauri://localhost http://tauri.localhost https://partner.example"
        );

        assert!(set_frame_ancestors(&conn, &pid, "agents", "none").unwrap());
        let row = get_by_project_name(&conn, &pid, "agents").unwrap().unwrap();
        let v = serde_json::to_value(ServiceJson::from_row(&row, STATUS_STOPPED, None, None))
            .unwrap();
        assert_eq!(v["framePolicy"], "'none'");

        assert!(!set_frame_ancestors(&conn, &pid, "missing", "").unwrap());

        let cmd = insert(
            &conn, &pid, "web", "npm start", "/tmp/pub-frame", 3000,
            EXPOSE_LOCAL, DESIRED_STOPPED, KIND_CMD, "",
        )
        .unwrap();
        let v = serde_json::to_value(ServiceJson::from_row(&cmd, STATUS_STOPPED, None, None))
            .unwrap();
        assert!(v["framePolicy"].is_null(), "K2 does not frame-guard a --cmd App's responses");
    }
}
