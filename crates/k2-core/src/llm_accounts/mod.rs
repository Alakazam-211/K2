//! LLM login wallet (`k2 llm accounts`, Settings → LLMs).
//!
//! Model (Rosson 2026-10-07, replaces PRD §19 on these points):
//!
//! - **One active login per tool per server.** The active login lives in
//!   the tool's normal store (Claude's keychain item or
//!   `~/.claude/.credentials.json`, `~/.codex/auth.json`,
//!   `~/.grok/auth.json`). Sessions launch exactly as before: no
//!   per-session config home, so conversations never split. A running
//!   CLI picks up a swapped token on its own.
//! - **The wallet** holds every other login under
//!   `~/.k2/llm-accounts/<tool>/<id>/` (dir 0700, credential file 0600),
//!   in the tool's own file format so the slot also works as a home for
//!   the tool's own login / refresh / usage commands.
//! - **Switching** saves the outgoing live credential back to its slot
//!   (the CLI may have refreshed it), then writes the chosen slot into
//!   the live store, under a per-tool lock. Switching affects every
//!   session on the server.
//! - **Fresh tokens.** The CLI refreshes the ACTIVE login. K2 never
//!   refreshes the active slot (refresh tokens rotate). K2 refreshes idle
//!   slots only (daemon keep-warm + lazily before a swap), never under
//!   air-gap.
//!
//! Nothing in this module returns a token to a caller outside it: the
//! public shapes ([`Entry`], [`store::CredMeta`]) carry metadata only.

pub mod pins;
pub mod store;
pub mod wallet;

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

/// Tools the wallet supports. Claude, Codex and Grok: subscription logins
/// (live store and login command verified) and API keys. Gemini: API keys
/// only. Cursor Agent, Pi and Hermes are listed on the LLMs page as "not
/// yet".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Tool {
    Claude,
    Codex,
    Grok,
    Gemini,
}

impl Tool {
    pub const ALL: [Tool; 4] = [Tool::Claude, Tool::Codex, Tool::Grok, Tool::Gemini];
    /// Tools with a subscription login K2 can swap and sign in.
    pub const SUBSCRIPTION: [Tool; 3] = [Tool::Claude, Tool::Codex, Tool::Grok];

    pub fn as_str(self) -> &'static str {
        match self {
            Tool::Claude => "claude",
            Tool::Codex => "codex",
            Tool::Grok => "grok",
            Tool::Gemini => "gemini",
        }
    }

    pub fn parse(s: &str) -> Option<Tool> {
        match s.trim().to_ascii_lowercase().as_str() {
            "claude" => Some(Tool::Claude),
            "codex" => Some(Tool::Codex),
            "grok" => Some(Tool::Grok),
            "gemini" => Some(Tool::Gemini),
            _ => None,
        }
    }

    /// The tool a spawn command runs (first token's basename).
    pub fn from_command(command: &str) -> Option<Tool> {
        let first = command.split_whitespace().next()?;
        Tool::parse(first.rsplit('/').next().unwrap_or(first))
    }

    pub fn display(self) -> &'static str {
        match self {
            Tool::Claude => "Claude",
            Tool::Codex => "Codex",
            Tool::Grok => "Grok",
            Tool::Gemini => "Gemini",
        }
    }

    /// Does K2 support subscription (CLI sign-in) logins for this tool?
    pub fn subscription_supported(self) -> bool {
        Tool::SUBSCRIPTION.contains(&self)
    }

    /// The CLI binary.
    pub fn program(self) -> &'static str {
        self.as_str()
    }

    /// The env var that points the CLI at a home: a wallet slot for the
    /// tool's own login / refresh / usage commands, and for sessions
    /// pinned to a subscription login.
    pub fn home_env_var(self) -> &'static str {
        match self {
            Tool::Claude => "CLAUDE_CONFIG_DIR",
            Tool::Codex => "CODEX_HOME",
            Tool::Grok => "GROK_HOME",
            Tool::Gemini => "GEMINI_CLI_HOME",
        }
    }

    /// The credential file name inside a home (the tool's own format).
    pub fn cred_file_name(self) -> &'static str {
        match self {
            Tool::Claude => ".credentials.json",
            Tool::Codex => "auth.json",
            Tool::Grok => "auth.json",
            Tool::Gemini => "oauth_creds.json",
        }
    }

    /// Env vars an API-key login sets on a session (verified against the
    /// installed binaries: Codex reads CODEX_API_KEY, then OPENAI_API_KEY).
    pub fn api_key_env(self) -> &'static [&'static str] {
        match self {
            Tool::Claude => &["ANTHROPIC_API_KEY"],
            Tool::Codex => &["CODEX_API_KEY", "OPENAI_API_KEY"],
            Tool::Grok => &["XAI_API_KEY"],
            Tool::Gemini => &["GEMINI_API_KEY"],
        }
    }
}

/// Login kinds.
pub mod kind {
    /// A CLI sign-in: swapped into the live store, or a pinned home.
    pub const SUBSCRIPTION: &str = "subscription";
    /// A pasted API key: injected into sessions via env, never swapped,
    /// billed per token.
    pub const API_KEY: &str = "api_key";
}

/// Tools shown on the LLMs page without wallet support yet.
pub const NOT_YET: &[(&str, &str)] = &[
    ("cursor", "Cursor Agent"),
    ("pi", "Pi"),
    ("hermes", "Hermes"),
];

/// Entry states.
pub mod state {
    pub const NOT_SET_UP: &str = "not_set_up";
    pub const SIGNING_IN: &str = "signing_in";
    pub const SIGNED_IN: &str = "signed_in";
    pub const NEEDS_LOGIN: &str = "needs_login";
    pub const UNKNOWN: &str = "unknown";
    pub const ALL: &[&str] = &[NOT_SET_UP, SIGNING_IN, SIGNED_IN, NEEDS_LOGIN, UNKNOWN];
}

/// One wallet entry. Metadata only, never a token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: String,
    pub tool: String,
    pub label: String,
    /// `subscription` now; `api_key` later (env-injected, never swapped).
    pub kind: String,
    /// Order in the tool's pool ("switch to next login" walks it).
    pub position: i64,
    pub created_by: Option<String>,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
    pub state: String,
    pub detail: Option<String>,
    pub email: Option<String>,
    pub org: Option<String>,
    pub plan: Option<String>,
    pub expires_at: Option<i64>,
    pub refreshed_at: Option<i64>,
    pub usage_json: Option<String>,
    pub usage_checked_at: Option<i64>,
    pub removed_at: Option<i64>,
}

impl Entry {
    pub fn tool(&self) -> Option<Tool> {
        Tool::parse(&self.tool)
    }
    pub fn is_api_key(&self) -> bool {
        self.kind == kind::API_KEY
    }
}

#[derive(Debug)]
pub enum WalletError {
    UnknownTool(String),
    InvalidLabel(String),
    DuplicateLabel(String),
    NotFound(String),
    /// The slot holds no login (sign-in never finished, or it expired).
    NotSignedIn(String),
    /// Remove/rename refused on the active login.
    ActiveLogin,
    /// The live store can't be used on this machine (e.g. Codex keeps
    /// its login in the OS keyring).
    LiveStoreUnavailable(String),
    AirGap,
    LockBusy(String),
    /// A rule refusal with its own code (409): `login_pinned`,
    /// `pinned_active`, `api_key_login`, …
    Conflict(&'static str, String),
    Io(String),
    Db(rusqlite::Error),
}

impl std::fmt::Display for WalletError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WalletError::UnknownTool(t) => {
                write!(f, "unknown tool {t:?} (subscriptions: claude, codex and grok)")
            }
            WalletError::InvalidLabel(why) => write!(f, "invalid label: {why}"),
            WalletError::DuplicateLabel(l) => {
                write!(f, "a token named {l:?} already exists for that tool")
            }
            WalletError::NotFound(id) => write!(f, "no such token: {id}"),
            WalletError::NotSignedIn(id) => write!(
                f,
                "token {id} has no saved sign-in yet; sign it in on Settings → LLMs first"
            ),
            WalletError::ActiveLogin => write!(
                f,
                "that token is the server default; make another token the server default first"
            ),
            WalletError::LiveStoreUnavailable(why) => write!(f, "{why}"),
            WalletError::AirGap => write!(f, "{}", crate::airgap::TEACHING),
            WalletError::LockBusy(t) => {
                write!(f, "another change to the {t} tokens is in progress; try again")
            }
            WalletError::Conflict(_, hint) => write!(f, "{hint}"),
            WalletError::Io(e) => write!(f, "io: {e}"),
            WalletError::Db(e) => write!(f, "db: {e}"),
        }
    }
}

impl From<rusqlite::Error> for WalletError {
    fn from(e: rusqlite::Error) -> Self {
        WalletError::Db(e)
    }
}

impl WalletError {
    pub fn code(&self) -> &'static str {
        match self {
            WalletError::UnknownTool(_) => "unknown_tool",
            WalletError::InvalidLabel(_) => "invalid_label",
            WalletError::DuplicateLabel(_) => "duplicate_label",
            WalletError::NotFound(_) => "not_found",
            WalletError::NotSignedIn(_) => "not_signed_in",
            WalletError::ActiveLogin => "active_login",
            WalletError::LiveStoreUnavailable(_) => "live_store_unavailable",
            WalletError::AirGap => "airgap",
            WalletError::LockBusy(_) => "busy",
            WalletError::Conflict(code, _) => code,
            WalletError::Io(_) => "io",
            WalletError::Db(_) => "db",
        }
    }
    pub fn http_status(&self) -> &'static str {
        match self {
            WalletError::DuplicateLabel(_)
            | WalletError::ActiveLogin
            | WalletError::NotSignedIn(_)
            | WalletError::LockBusy(_)
            | WalletError::Conflict(..) => "409 Conflict",
            WalletError::NotFound(_) => "404 Not Found",
            WalletError::AirGap => "403 Forbidden",
            WalletError::LiveStoreUnavailable(_) => "422 Unprocessable Entity",
            WalletError::Io(_) | WalletError::Db(_) => "500 Internal Server Error",
            _ => "400 Bad Request",
        }
    }
}

pub(crate) fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

const COLS: &str = "id, tool, label, created_by, created_at, last_used_at, state, detail, \
                    email, org, plan, expires_at, refreshed_at, usage_json, usage_checked_at, removed_at, \
                    kind, position";

fn map_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Entry> {
    Ok(Entry {
        id: r.get(0)?,
        tool: r.get(1)?,
        label: r.get(2)?,
        created_by: r.get(3)?,
        created_at: r.get(4)?,
        last_used_at: r.get(5)?,
        state: r.get(6)?,
        detail: r.get(7)?,
        email: r.get(8)?,
        org: r.get(9)?,
        plan: r.get(10)?,
        expires_at: r.get(11)?,
        refreshed_at: r.get(12)?,
        usage_json: r.get(13)?,
        usage_checked_at: r.get(14)?,
        removed_at: r.get(15)?,
        kind: r.get(16)?,
        position: r.get(17)?,
    })
}

/// Live entries (not removed), by tool, then pool order, then label.
pub fn list(conn: &Connection) -> Result<Vec<Entry>, WalletError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLS} FROM llm_accounts WHERE removed_at IS NULL \
         ORDER BY tool, position, label COLLATE NOCASE"
    ))?;
    let rows = stmt.query_map([], map_row)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn list_for(conn: &Connection, tool: Tool) -> Result<Vec<Entry>, WalletError> {
    Ok(list(conn)?.into_iter().filter(|e| e.tool == tool.as_str()).collect())
}

pub fn get(conn: &Connection, id: &str) -> Result<Option<Entry>, WalletError> {
    Ok(conn
        .query_row(
            &format!("SELECT {COLS} FROM llm_accounts WHERE id = ?1"),
            params![id],
            map_row,
        )
        .optional()?)
}

/// A live entry by id, or by label within `tool`.
pub fn lookup(conn: &Connection, tool: Option<Tool>, key: &str) -> Result<Entry, WalletError> {
    let key = key.trim();
    if let Some(e) = get(conn, key)? {
        if e.removed_at.is_none() && tool.map(|t| t.as_str() == e.tool).unwrap_or(true) {
            return Ok(e);
        }
    }
    if let Some(t) = tool {
        if let Some(e) = conn
            .query_row(
                &format!(
                    "SELECT {COLS} FROM llm_accounts WHERE tool = ?1 AND label = ?2 COLLATE NOCASE \
                     AND removed_at IS NULL"
                ),
                params![t.as_str(), key],
                map_row,
            )
            .optional()?
        {
            return Ok(e);
        }
    }
    Err(WalletError::NotFound(key.to_string()))
}

/// 1–40 chars of letters, digits, space, `.`, `_`, `-`, `@`, `+`.
pub fn validate_label(label: &str) -> Result<String, WalletError> {
    let l = label.trim();
    if l.is_empty() {
        return Err(WalletError::InvalidLabel("label is empty".into()));
    }
    if l.chars().count() > 40 {
        return Err(WalletError::InvalidLabel("label is longer than 40 characters".into()));
    }
    if !l.chars().all(|c| {
        c.is_alphanumeric() || matches!(c, ' ' | '.' | '_' | '-' | '@' | '+')
    }) {
        return Err(WalletError::InvalidLabel(
            "use letters, digits, spaces and . _ - @ +".into(),
        ));
    }
    Ok(l.to_string())
}

fn label_taken(conn: &Connection, tool: Tool, label: &str, except: Option<&str>) -> Result<bool, WalletError> {
    let id: Option<String> = conn
        .query_row(
            "SELECT id FROM llm_accounts WHERE tool = ?1 AND label = ?2 COLLATE NOCASE AND removed_at IS NULL",
            params![tool.as_str(), label],
            |r| r.get(0),
        )
        .optional()?;
    Ok(match (id, except) {
        (Some(found), Some(ex)) => found != ex,
        (Some(_), None) => true,
        (None, _) => false,
    })
}

fn new_id() -> String {
    let u = uuid::Uuid::new_v4().simple().to_string();
    format!("acc_{}", &u[..16])
}

/// Insert a new subscription entry row (no slot file yet: `not_set_up`).
pub fn create(conn: &Connection, tool: Tool, label: &str, created_by: Option<&str>) -> Result<Entry, WalletError> {
    if !tool.subscription_supported() {
        return Err(WalletError::LiveStoreUnavailable(format!(
            "{} subscriptions aren't supported yet; add an API token instead",
            tool.display()
        )));
    }
    create_kind(conn, tool, label, kind::SUBSCRIPTION, created_by)
}

pub(crate) fn create_kind(
    conn: &Connection,
    tool: Tool,
    label: &str,
    kind_: &str,
    created_by: Option<&str>,
) -> Result<Entry, WalletError> {
    let label = validate_label(label)?;
    if label_taken(conn, tool, &label, None)? {
        return Err(WalletError::DuplicateLabel(label));
    }
    let id = new_id();
    // New logins join the end of the tool's pool.
    conn.execute(
        "INSERT INTO llm_accounts (id, tool, label, kind, position, created_by, created_at, state) \
         VALUES (?1, ?2, ?3, ?6, \
                 (SELECT COALESCE(MAX(position), -1) + 1 FROM llm_accounts WHERE tool = ?2), \
                 ?4, ?5, 'not_set_up')",
        params![id, tool.as_str(), label, created_by, now(), kind_],
    )?;
    get(conn, &id)?.ok_or(WalletError::NotFound(id))
}

pub fn rename(conn: &Connection, id: &str, new_label: &str) -> Result<Entry, WalletError> {
    let e = lookup(conn, None, id)?;
    let tool = e.tool().ok_or_else(|| WalletError::UnknownTool(e.tool.clone()))?;
    let label = validate_label(new_label)?;
    if label_taken(conn, tool, &label, Some(&e.id))? {
        return Err(WalletError::DuplicateLabel(label));
    }
    conn.execute("UPDATE llm_accounts SET label = ?2 WHERE id = ?1", params![e.id, label])?;
    get(conn, &e.id)?.ok_or(WalletError::NotFound(e.id))
}

/// Refuse token-shaped text in any metadata column.
pub fn looks_like_secret(v: &str) -> bool {
    let lower = v.to_ascii_lowercase();
    if lower.contains("accesstoken")
        || lower.contains("access_token")
        || lower.contains("refreshtoken")
        || lower.contains("refresh_token")
        || lower.contains("authorization")
        || lower.contains("bearer ")
        || v.contains("sk-ant-")
    {
        return true;
    }
    v.split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | '"' | '\''))
        .any(|w| {
            w.len() >= 40
                && w.chars().all(|c| {
                    c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '=' | '/' | '+')
                })
                && !w.starts_with('/')
        })
}

/// Metadata update. `None` leaves a column as it is.
#[derive(Debug, Clone, Default)]
pub struct MetaUpdate {
    pub state: Option<String>,
    /// `Some(None)` clears the detail.
    pub detail: Option<Option<String>>,
    pub email: Option<String>,
    pub org: Option<String>,
    pub plan: Option<String>,
    pub expires_at: Option<Option<i64>>,
    pub refreshed_at: Option<i64>,
    pub last_used_at: Option<i64>,
    pub usage_json: Option<String>,
    pub usage_checked_at: Option<i64>,
}

pub fn update_meta(conn: &Connection, id: &str, m: &MetaUpdate) -> Result<(), WalletError> {
    if let Some(s) = &m.state {
        if !state::ALL.contains(&s.as_str()) {
            return Err(WalletError::InvalidLabel(format!("unknown state {s:?}")));
        }
    }
    let texts = [
        m.detail.clone().flatten(),
        m.email.clone(),
        m.org.clone(),
        m.plan.clone(),
    ];
    for t in texts.iter().flatten() {
        if looks_like_secret(t) {
            return Err(WalletError::Io(
                "refusing to store token metadata that looks like a credential".into(),
            ));
        }
    }
    if let Some(u) = &m.usage_json {
        if u.contains("accessToken") || u.contains("refreshToken") || u.contains("Authorization") {
            return Err(WalletError::Io(
                "refusing to store a usage snapshot that contains credentials".into(),
            ));
        }
    }
    let tx_cols: Vec<(&str, rusqlite::types::Value)> = {
        use rusqlite::types::Value;
        let mut v: Vec<(&str, Value)> = Vec::new();
        if let Some(s) = &m.state {
            v.push(("state", Value::Text(s.clone())));
        }
        if let Some(d) = &m.detail {
            v.push(("detail", d.clone().map(Value::Text).unwrap_or(Value::Null)));
        }
        if let Some(e) = &m.email {
            v.push(("email", Value::Text(e.clone())));
        }
        if let Some(o) = &m.org {
            v.push(("org", Value::Text(o.clone())));
        }
        if let Some(p) = &m.plan {
            v.push(("plan", Value::Text(p.clone())));
        }
        if let Some(x) = &m.expires_at {
            v.push(("expires_at", x.map(Value::Integer).unwrap_or(Value::Null)));
        }
        if let Some(x) = m.refreshed_at {
            v.push(("refreshed_at", Value::Integer(x)));
        }
        if let Some(x) = m.last_used_at {
            v.push(("last_used_at", Value::Integer(x)));
        }
        if let Some(u) = &m.usage_json {
            v.push(("usage_json", Value::Text(u.clone())));
        }
        if let Some(x) = m.usage_checked_at {
            v.push(("usage_checked_at", Value::Integer(x)));
        }
        v
    };
    if tx_cols.is_empty() {
        return Ok(());
    }
    let sets: Vec<String> = tx_cols
        .iter()
        .enumerate()
        .map(|(i, (c, _))| format!("{c} = ?{}", i + 2))
        .collect();
    let sql = format!("UPDATE llm_accounts SET {} WHERE id = ?1", sets.join(", "));
    let mut values: Vec<rusqlite::types::Value> = vec![rusqlite::types::Value::Text(id.to_string())];
    values.extend(tx_cols.into_iter().map(|(_, v)| v));
    conn.execute(&sql, rusqlite::params_from_iter(values))?;
    Ok(())
}

/// The active entry id for a tool, if any.
pub fn active_id(conn: &Connection, tool: Tool) -> Result<Option<String>, WalletError> {
    Ok(conn
        .query_row(
            "SELECT a.account_id FROM llm_active a JOIN llm_accounts e ON e.id = a.account_id \
             WHERE a.tool = ?1 AND e.removed_at IS NULL",
            params![tool.as_str()],
            |r| r.get(0),
        )
        .optional()?)
}

pub(crate) fn set_active(conn: &Connection, tool: Tool, id: &str, by: Option<&str>) -> Result<(), WalletError> {
    set_active_with_live(conn, tool, id, None, by)
}

/// `live_account_id`: the subscription login left in the live store
/// while `id` (an API key) is the pool's active login.
pub(crate) fn set_active_with_live(
    conn: &Connection,
    tool: Tool,
    id: &str,
    live_account_id: Option<&str>,
    by: Option<&str>,
) -> Result<(), WalletError> {
    let t = now();
    conn.execute(
        "INSERT INTO llm_active (tool, account_id, switched_at, switched_by, live_account_id) VALUES (?1, ?2, ?3, ?4, ?5) \
         ON CONFLICT(tool) DO UPDATE SET account_id = excluded.account_id, \
         switched_at = excluded.switched_at, switched_by = excluded.switched_by, \
         live_account_id = excluded.live_account_id",
        params![tool.as_str(), id, t, by, live_account_id],
    )?;
    conn.execute("UPDATE llm_accounts SET last_used_at = ?2 WHERE id = ?1", params![id, t])?;
    Ok(())
}

/// The subscription login whose credential is in the tool's live store
/// right now (the CLI owns its refresh): the pool's active login, or,
/// while an API key is active, the login left behind in the live store.
pub fn live_owner(conn: &Connection, tool: Tool) -> Result<Option<String>, WalletError> {
    let row: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT account_id, live_account_id FROM llm_active WHERE tool = ?1",
            params![tool.as_str()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((active, live)) = row else { return Ok(None) };
    if let Some(l) = live {
        return Ok(Some(l));
    }
    match get(conn, &active)? {
        Some(e) if !e.is_api_key() && e.removed_at.is_none() => Ok(Some(e.id)),
        _ => Ok(None),
    }
}

/// Soft-delete the row (the slot folder is moved by [`wallet::remove`]).
pub(crate) fn mark_removed(conn: &Connection, id: &str) -> Result<(), WalletError> {
    conn.execute(
        "UPDATE llm_accounts SET removed_at = ?2 WHERE id = ?1",
        params![id, now()],
    )?;
    Ok(())
}

/// The entry after the active one (label order, wrapping) that has a
/// saved sign-in: the one-click "switch to next login". Never automatic.
pub fn next_signed_in(conn: &Connection, tool: Tool) -> Result<Option<Entry>, WalletError> {
    let entries = list_for(conn, tool)?;
    if entries.len() < 2 {
        return Ok(None);
    }
    let active = active_id(conn, tool)?;
    let start = active
        .as_deref()
        .and_then(|a| entries.iter().position(|e| e.id == a))
        .unwrap_or(entries.len() - 1);
    for step in 1..entries.len() {
        let e = &entries[(start + step) % entries.len()];
        // The pool cycle skips API keys (billed per token: only an
        // explicit pick) and any login pinned somewhere (never live in
        // two places).
        if Some(e.id.as_str()) != active.as_deref()
            && !e.is_api_key()
            && store::slot_has_cred(tool, &e.id)
            && !pins::is_pinned(conn, &e.id)?
        {
            return Ok(Some(e.clone()));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod pins_tests;
