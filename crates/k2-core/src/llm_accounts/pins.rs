//! Pins, API-key logins, and the per-session login decision.
//!
//! **Pins.** A workspace (its canonical agent, sidecars, heartbeats and
//! tabs) or one chat can run on a specific login per tool instead of the
//! pool's active one. Precedence at a fresh spawn: chat pin > workspace
//! pin > the pool.
//!
//! **Chat pins** are session pins. The UI pins a chat by its v2 session
//! key (the pinned chat: its workspace id; an extra tab: `tab-<id>`) and,
//! once its conversation id is known, by `conversation:<id>` too (one
//! pick, two rows with the same account and `created_at`). The
//! conversation row is what survives a revival under another key: a tab
//! closed and reopened from history, or restored after a restart. A
//! self-minting CLI (Codex) learns its id after the start, so the mirror
//! is written at adoption ([`mirror_to_conversation`], called from
//! `WorkspaceTabSession::follow_adopted_conversation`, which also moves
//! the Thread and the login record). When both rows exist the newer pick
//! wins ([`chat_pin`]).
//!
//! **Resume.** A chat pin applies at every revival: a resumed
//! conversation runs on its chat's pinned login, and its history is
//! copied into that login's home first when the homes differ
//! ([`carry_conversation`]; Codex homes share sessions already). With no
//! chat pin, a resume uses the login the conversation started under
//! ([`record_spawn`]); a workspace pin or a new server default never
//! moves an existing conversation.
//!
//! **Pinned subscription session** = the tool runs with the login's
//! wallet slot as its home (`CLAUDE_CONFIG_DIR=<slot>`,
//! `CODEX_HOME=<slot>`, `GROK_HOME=<slot>`), so two subscriptions run in
//! parallel. Codex slots are shadow homes: only the login file is the
//! slot's own, everything else links to the normal Codex home, so
//! conversations aren't stranded. A Claude slot keeps its own history
//! (shared settings, skills, plugins, agents and commands are linked).
//!
//! **Rotation safety.** A subscription login is never live in two
//! places: pinned anywhere ⇒ not the pool's active login and out of the
//! pool cycle, and the pool's live login can't be pinned. A pinned slot
//! with a running session is CLI-owned; K2 keep-warm skips it.
//!
//! **API keys** (`kind = api_key`) are a pasted key in `<slot>/api-key`
//! (0600), injected into the sessions that use the login via env. They
//! can be pinned anywhere and may also sit in the pool (billed per
//! token; never in the "switch to next" cycle).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use super::store;
use super::{active_id, get, kind, live_owner, lookup, now, state, update_meta, Entry, MetaUpdate, Tool, WalletError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ScopeKind {
    Workspace,
    Session,
}

impl ScopeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ScopeKind::Workspace => "workspace",
            ScopeKind::Session => "session",
        }
    }
    pub fn parse(s: &str) -> Option<ScopeKind> {
        match s.trim() {
            "workspace" => Some(ScopeKind::Workspace),
            "session" => Some(ScopeKind::Session),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Pin {
    pub scope_kind: String,
    pub scope_id: String,
    pub tool: String,
    pub account_id: String,
    pub created_by: Option<String>,
    pub created_at: i64,
}

fn map_pin(r: &rusqlite::Row<'_>) -> rusqlite::Result<Pin> {
    Ok(Pin {
        scope_kind: r.get(0)?,
        scope_id: r.get(1)?,
        tool: r.get(2)?,
        account_id: r.get(3)?,
        created_by: r.get(4)?,
        created_at: r.get(5)?,
    })
}

const PIN_COLS: &str = "scope_kind, scope_id, tool, account_id, created_by, created_at";

pub fn list_pins(conn: &Connection) -> Result<Vec<Pin>, WalletError> {
    let mut st = conn.prepare(&format!(
        "SELECT {PIN_COLS} FROM llm_account_pins ORDER BY tool, scope_kind, scope_id"
    ))?;
    let rows = st.query_map([], map_pin)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn pins_for(conn: &Connection, account_id: &str) -> Result<Vec<Pin>, WalletError> {
    Ok(list_pins(conn)?.into_iter().filter(|p| p.account_id == account_id).collect())
}

pub fn is_pinned(conn: &Connection, account_id: &str) -> Result<bool, WalletError> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM llm_account_pins WHERE account_id = ?1",
        params![account_id],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// Pin `scope` to a login for its tool. Refuses a subscription login
/// that is in the tool's live store (the pool's login): one login is
/// never live in two places.
pub fn pin(
    conn: &Connection,
    scope: ScopeKind,
    scope_id: &str,
    tool: Option<Tool>,
    account_key: &str,
    by: Option<&str>,
) -> Result<Pin, WalletError> {
    let scope_id = scope_id.trim();
    if scope_id.is_empty() {
        return Err(WalletError::InvalidLabel("scope id is empty".into()));
    }
    let e = lookup(conn, tool, account_key)?;
    let t = e.tool().ok_or_else(|| WalletError::UnknownTool(e.tool.clone()))?;
    if !e.is_api_key() {
        if live_owner(conn, t)?.as_deref() == Some(e.id.as_str()) {
            return Err(WalletError::Conflict(
                "pinned_active",
                format!(
                    "{} is the server default {} token. A subscription can't be live in two places (sign-ins rotate), so pick Server default, or make another token the server default first.",
                    e.label,
                    t.display()
                ),
            ));
        }
        if !store::slot_has_cred(t, &e.id) {
            return Err(WalletError::NotSignedIn(e.id));
        }
    }
    let ts = now();
    conn.execute(
        "INSERT INTO llm_account_pins (scope_kind, scope_id, tool, account_id, created_by, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
         ON CONFLICT(scope_kind, scope_id, tool) DO UPDATE SET account_id = excluded.account_id, \
         created_by = excluded.created_by, created_at = excluded.created_at",
        params![scope.as_str(), scope_id, t.as_str(), e.id, by, ts],
    )?;
    Ok(Pin {
        scope_kind: scope.as_str().into(),
        scope_id: scope_id.into(),
        tool: t.as_str().into(),
        account_id: e.id,
        created_by: by.map(str::to_string),
        created_at: ts,
    })
}

/// Remove a pin. `Ok(false)` when there was none.
pub fn unpin(conn: &Connection, scope: ScopeKind, scope_id: &str, tool: Tool) -> Result<bool, WalletError> {
    let n = conn.execute(
        "DELETE FROM llm_account_pins WHERE scope_kind = ?1 AND scope_id = ?2 AND tool = ?3",
        params![scope.as_str(), scope_id.trim(), tool.as_str()],
    )?;
    Ok(n > 0)
}

/// The `scope_id` prefix of a chat pin that follows one conversation.
pub const CONVERSATION_PREFIX: &str = "conversation:";

/// A provider conversation id K2 will key on and use in a file name
/// (`<id>.jsonl`, a `<id>/` folder): 1–200 of `[A-Za-z0-9._-]`, not
/// starting with a dot.
pub fn valid_conversation_id(c: &str) -> bool {
    !c.is_empty()
        && c.len() <= 200
        && !c.starts_with('.')
        && c.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.')
}

/// `conversation:<id>`, or `None` for an id K2 won't key on.
pub fn conversation_scope_id(conversation_id: &str) -> Option<String> {
    let c = conversation_id.trim();
    valid_conversation_id(c).then(|| format!("{CONVERSATION_PREFIX}{c}"))
}

fn get_pin(conn: &Connection, kind: ScopeKind, scope_id: &str, tool: Tool) -> Result<Option<Pin>, WalletError> {
    Ok(conn
        .query_row(
            &format!("SELECT {PIN_COLS} FROM llm_account_pins WHERE scope_kind = ?1 AND scope_id = ?2 AND tool = ?3"),
            params![kind.as_str(), scope_id, tool.as_str()],
            map_pin,
        )
        .optional()?)
}

/// Write one session-scope row with a given account and pick time (a
/// mirror carries its source pick's `created_at`).
fn put_session_row(
    conn: &Connection,
    scope_id: &str,
    tool: &str,
    account_id: &str,
    by: Option<&str>,
    created_at: i64,
) -> Result<(), WalletError> {
    conn.execute(
        "INSERT INTO llm_account_pins (scope_kind, scope_id, tool, account_id, created_by, created_at) \
         VALUES ('session', ?1, ?2, ?3, ?4, ?5) \
         ON CONFLICT(scope_kind, scope_id, tool) DO UPDATE SET account_id = excluded.account_id, \
         created_by = excluded.created_by, created_at = excluded.created_at",
        params![scope_id, tool, account_id, by, created_at],
    )?;
    Ok(())
}

/// The chat pin that applies to a session: the newer of its
/// conversation's pin and its session key's pin (a tie goes to the
/// session key, the chat being started).
pub fn chat_pin(
    conn: &Connection,
    tool: Tool,
    session_key: &str,
    conversation_id: Option<&str>,
) -> Result<Option<Pin>, WalletError> {
    let by_key = if session_key.is_empty() { None } else { get_pin(conn, ScopeKind::Session, session_key, tool)? };
    let by_conv = match conversation_id.and_then(conversation_scope_id) {
        Some(k) if k != session_key => get_pin(conn, ScopeKind::Session, &k, tool)?,
        _ => None,
    };
    Ok(match (by_key, by_conv) {
        (Some(k), Some(c)) => Some(if c.created_at > k.created_at { c } else { k }),
        (k, c) => k.or(c),
    })
}

/// The pin that applies to a fresh session: chat > workspace.
pub fn pin_for(
    conn: &Connection,
    tool: Tool,
    session_key: &str,
    conversation_id: Option<&str>,
    project_id: Option<&str>,
) -> Result<Option<Pin>, WalletError> {
    if let Some(p) = chat_pin(conn, tool, session_key, conversation_id)? {
        return Ok(Some(p));
    }
    if let Some(pid) = project_id.filter(|s| !s.is_empty()) {
        if let Some(p) = get_pin(conn, ScopeKind::Workspace, pid, tool)? {
            return Ok(Some(p));
        }
    }
    Ok(None)
}

/// Pin one chat: its session key, plus `conversation:<id>` when the
/// conversation is known (same account, same `created_at`). Mirrors of
/// the key's previous pick (same account and pick time) move with it, so
/// a conversation that ran under this key earlier follows the new pick.
pub fn pin_chat(
    conn: &Connection,
    session_key: &str,
    conversation_id: Option<&str>,
    tool: Option<Tool>,
    account_key: &str,
    by: Option<&str>,
) -> Result<Pin, WalletError> {
    let e = lookup(conn, tool, account_key)?;
    let t = e.tool().ok_or_else(|| WalletError::UnknownTool(e.tool.clone()))?;
    let before = get_pin(conn, ScopeKind::Session, session_key.trim(), t)?;
    let p = pin(conn, ScopeKind::Session, session_key, Some(t), &e.id, by)?;
    if let Some(old) = before {
        conn.execute(
            "UPDATE llm_account_pins SET account_id = ?1, created_by = ?2, created_at = ?3 \
             WHERE scope_kind = 'session' AND scope_id LIKE 'conversation:%' AND tool = ?4 \
             AND account_id = ?5 AND created_at = ?6",
            params![p.account_id, p.created_by, p.created_at, p.tool, old.account_id, old.created_at],
        )?;
    }
    if let Some(k) = conversation_id.and_then(conversation_scope_id) {
        if k != p.scope_id {
            put_session_row(conn, &k, &p.tool, &p.account_id, by, p.created_at)?;
        }
    }
    Ok(p)
}

/// Back to the default for one chat: its session key's pin, the
/// conversation mirrors of that pick, and `conversation:<id>` when given.
/// `Ok(false)` when there was nothing to remove.
pub fn unpin_chat(
    conn: &Connection,
    session_key: &str,
    conversation_id: Option<&str>,
    tool: Tool,
) -> Result<bool, WalletError> {
    let mut removed = false;
    if let Some(old) = get_pin(conn, ScopeKind::Session, session_key.trim(), tool)? {
        removed |= conn.execute(
            "DELETE FROM llm_account_pins WHERE scope_kind = 'session' AND scope_id LIKE 'conversation:%' \
             AND tool = ?1 AND account_id = ?2 AND created_at = ?3",
            params![tool.as_str(), old.account_id, old.created_at],
        )? > 0;
        removed |= unpin(conn, ScopeKind::Session, session_key, tool)?;
    }
    if let Some(k) = conversation_id.and_then(conversation_scope_id) {
        removed |= unpin(conn, ScopeKind::Session, &k, tool)?;
    }
    Ok(removed)
}

/// Copy the session key's pick onto `conversation:<id>` (the newer pick
/// wins; an equal one is left alone). `Ok(true)` when a row was written.
pub fn mirror_to_conversation(
    conn: &Connection,
    tool: Tool,
    session_key: &str,
    conversation_id: &str,
) -> Result<bool, WalletError> {
    let Some(k) = conversation_scope_id(conversation_id) else { return Ok(false) };
    if session_key.is_empty() || k == session_key {
        return Ok(false);
    }
    let Some(src) = get_pin(conn, ScopeKind::Session, session_key, tool)? else { return Ok(false) };
    if let Some(cur) = get_pin(conn, ScopeKind::Session, &k, tool)? {
        if cur.account_id == src.account_id || cur.created_at > src.created_at {
            return Ok(false);
        }
    }
    put_session_row(conn, &k, &src.tool, &src.account_id, src.created_by.as_deref(), src.created_at)?;
    Ok(true)
}


// ── API keys ────────────────────────────────────────────────────────

fn validate_api_key(key: &str) -> Result<String, WalletError> {
    let k = key.trim();
    if k.len() < 8 || k.len() > 512 || k.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(WalletError::InvalidLabel(
            "the API key must be 8–512 characters with no spaces".into(),
        ));
    }
    Ok(k.to_string())
}

/// Add an API-key login: the key goes to `<slot>/api-key` (0600) and is
/// never returned. Billed per token.
pub fn add_api_key(conn: &Connection, tool: Tool, label: &str, key: &str, by: Option<&str>) -> Result<Entry, WalletError> {
    let key = validate_api_key(key)?;
    let e = super::create_kind(conn, tool, label, kind::API_KEY, by)?;
    if let Err(err) = store::write_api_key(tool, &e.id, &key) {
        let _ = conn.execute("DELETE FROM llm_accounts WHERE id = ?1", params![e.id]);
        return Err(err);
    }
    update_meta(
        conn,
        &e.id,
        &MetaUpdate {
            state: Some(state::SIGNED_IN.into()),
            plan: Some("API key".into()),
            ..Default::default()
        },
    )?;
    get(conn, &e.id)?.ok_or(WalletError::NotFound(e.id))
}

/// Replace an API-key login's key.
pub fn replace_api_key(conn: &Connection, id: &str, key: &str) -> Result<Entry, WalletError> {
    let e = lookup(conn, None, id)?;
    if !e.is_api_key() {
        return Err(WalletError::Conflict("not_api_key", "that token is a subscription, not an API token".into()));
    }
    let t = e.tool().ok_or_else(|| WalletError::UnknownTool(e.tool.clone()))?;
    store::write_api_key(t, &e.id, &validate_api_key(key)?)?;
    get(conn, &e.id)?.ok_or(WalletError::NotFound(e.id))
}

fn api_key_env(tool: Tool, id: &str) -> Result<Vec<(String, String)>, WalletError> {
    let key = store::read_api_key(tool, id).ok_or_else(|| WalletError::NotSignedIn(id.to_string()))?;
    Ok(tool.api_key_env().iter().map(|v| (v.to_string(), key.clone())).collect())
}

// ── Homes for pinned subscription sessions ─────────────────────────

/// Entries of a Codex shadow home that are the slot's own.
pub const CODEX_PRIVATE_FILES: &[&str] = &["auth.json", "models_cache.json"];
/// Folders a Codex shadow home keeps local.
pub const CODEX_LOCAL_DIRS: &[&str] = &["log", "tmp", "memories"];
/// K2's own files in a slot (never linked).
const K2_SLOT_FILES: &[&str] = &[".lock", "api-key", "config.toml", "leader-login.sock"];

#[cfg(unix)]
fn symlink(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(src, dst)
}
#[cfg(not(unix))]
fn symlink(_src: &Path, _dst: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "symlinks"))
}

/// Make `slot` a Codex shadow of `shared` (the normal Codex home):
/// `auth.json` / `models_cache.json` stay the slot's own (a symlinked
/// `auth.json` is refused), `log` / `tmp` / `memories` stay local,
/// `config.toml` is a copy of the shared one that forces the file
/// credential store (so the OS keyring never holds the slot's login),
/// and every other entry (sessions, history, sqlite state, skills, …) is
/// a symlink into `shared`. `sessions/` is created in `shared` first so
/// new conversations always land there. Re-run before every spawn: new
/// entries Codex adds to the shared home get linked. A real folder the
/// slot grew for itself (e.g. from a sign-in run) is moved aside, never
/// deleted.
pub fn materialize_codex_shadow(slot: &Path, shared: &Path) -> Result<(), String> {
    std::fs::create_dir_all(shared).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(shared.join("sessions")).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(slot).map_err(|e| e.to_string())?;
    for f in CODEX_PRIVATE_FILES {
        let p = slot.join(f);
        if std::fs::symlink_metadata(&p).map(|m| m.file_type().is_symlink()).unwrap_or(false) {
            return Err(format!("refusing a symlinked {f} in a Codex login home"));
        }
    }
    for d in CODEX_LOCAL_DIRS {
        let p = slot.join(d);
        if std::fs::symlink_metadata(&p).map(|m| m.file_type().is_symlink()).unwrap_or(false) {
            std::fs::remove_file(&p).map_err(|e| e.to_string())?;
        }
        std::fs::create_dir_all(&p).map_err(|e| e.to_string())?;
    }
    // config.toml: the shared settings plus the file credential store.
    let shared_cfg = std::fs::read_to_string(shared.join("config.toml")).unwrap_or_default();
    let mut lines: Vec<String> = Vec::new();
    lines.push("# Written by K2 for a pinned Codex login: the shared config.toml with".into());
    lines.push("# the file credential store forced. Edit ~/.codex/config.toml instead.".into());
    lines.push("cli_auth_credentials_store = \"file\"".into());
    for l in shared_cfg.lines() {
        if l.trim_start().starts_with("cli_auth_credentials_store") {
            continue;
        }
        lines.push(l.to_string());
    }
    let cfg_path = slot.join("config.toml");
    if std::fs::symlink_metadata(&cfg_path).map(|m| m.file_type().is_symlink()).unwrap_or(false) {
        std::fs::remove_file(&cfg_path).map_err(|e| e.to_string())?;
    }
    store::write_private(&cfg_path, (lines.join("\n") + "\n").as_bytes()).map_err(|e| e.to_string())?;
    let entries = std::fs::read_dir(shared).map_err(|e| e.to_string())?;
    for ent in entries.flatten() {
        let name = ent.file_name().to_string_lossy().to_string();
        if CODEX_PRIVATE_FILES.contains(&name.as_str())
            || CODEX_LOCAL_DIRS.contains(&name.as_str())
            || K2_SLOT_FILES.contains(&name.as_str())
            || name.ends_with(".lock")
            || name.ends_with(".sock")
        {
            continue;
        }
        let dst = slot.join(&name);
        let src = shared.join(&name);
        match std::fs::symlink_metadata(&dst) {
            Ok(m) if m.file_type().is_symlink() => {
                if std::fs::read_link(&dst).ok().as_deref() == Some(src.as_path()) {
                    continue;
                }
                std::fs::remove_file(&dst).map_err(|e| e.to_string())?;
            }
            Ok(_) => {
                let aside = slot.join(format!("{name}.k2-local-{}", now()));
                std::fs::rename(&dst, &aside).map_err(|e| e.to_string())?;
            }
            Err(_) => {}
        }
        symlink(&src, &dst).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Shared Claude settings a pinned Claude home links to the normal one
/// (history — `projects/`, todos, shell snapshots — stays with the slot).
pub const CLAUDE_SHARED_ENTRIES: &[&str] =
    &["settings.json", "skills", "plugins", "agents", "commands", "CLAUDE.md", "hooks"];

/// Prepare a Claude slot for a pinned session: link the shared settings
/// entries that exist in `shared`, and mark `cwd` trusted in the slot's
/// own `.claude.json` (a new config dir would otherwise ask again).
pub fn prepare_claude_home(slot: &Path, shared: &Path, cwd: Option<&Path>) -> Result<(), String> {
    std::fs::create_dir_all(slot).map_err(|e| e.to_string())?;
    for name in CLAUDE_SHARED_ENTRIES {
        let src = shared.join(name);
        if !src.exists() {
            continue;
        }
        let dst = slot.join(name);
        match std::fs::symlink_metadata(&dst) {
            Ok(m) if m.file_type().is_symlink() => continue,
            Ok(_) => continue, // the slot has its own; leave it
            Err(_) => symlink(&src, &dst).map_err(|e| e.to_string())?,
        }
    }
    if let Some(cwd) = cwd {
        let path = slot.join(".claude.json");
        let mut v: serde_json::Value = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_else(|| serde_json::json!({}));
        let key = cwd.to_string_lossy().to_string();
        if !v.is_object() {
            v = serde_json::json!({});
        }
        let obj = v.as_object_mut().expect("object");
        let projects = obj.entry("projects").or_insert_with(|| serde_json::json!({}));
        if let Some(p) = projects.as_object_mut() {
            let entry = p.entry(key).or_insert_with(|| serde_json::json!({}));
            if let Some(e) = entry.as_object_mut() {
                if e.get("hasTrustDialogAccepted") == Some(&serde_json::Value::Bool(true)) {
                    return Ok(());
                }
                e.insert("hasTrustDialogAccepted".into(), serde_json::Value::Bool(true));
            }
        }
        let bytes = serde_json::to_vec_pretty(&v).map_err(|e| e.to_string())?;
        store::write_private(&path, &bytes).map_err(|e| e.to_string())?;
    }
    Ok(())
}

// ── The per-session decision ───────────────────────────────────────

/// Why a session runs on the login it runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// A resumed conversation: the login it started under.
    Resume,
    SessionPin,
    WorkspacePin,
    /// The pool: the tool's live store (or the pool's active API key).
    Pool,
}

/// The login a spawn uses and the env it needs.
#[derive(Debug, Clone)]
pub struct SpawnLogin {
    pub tool: Tool,
    /// `None` = the tool's live home (the pool's subscription login).
    pub account_id: Option<String>,
    pub kind: Option<String>,
    pub source: Source,
    /// The home the conversation runs in (`None` = the live home).
    pub home: Option<PathBuf>,
    /// Env pairs to set on the child (never logged: may hold an API key).
    pub env: Vec<(String, String)>,
    /// A resumed conversation moving to its chat's pinned login: copy its
    /// history from the home it last ran in before the spawn.
    pub carry: Option<Carry>,
}

/// Bring one conversation's history from one tool home to another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Carry {
    pub conversation_id: String,
    pub from: PathBuf,
    pub to: PathBuf,
}

impl SpawnLogin {
    /// A subscription login running from its slot (needs the slot lock
    /// across the spawn, and makes the slot CLI-owned while it runs).
    pub fn pinned_slot(&self) -> Option<(Tool, &str)> {
        match (&self.account_id, self.kind.as_deref()) {
            (Some(id), Some(k)) if k == kind::SUBSCRIPTION => Some((self.tool, id.as_str())),
            _ => None,
        }
    }
}

/// The recorded `(account_id, home)` a conversation (or, for ids the CLI
/// minted itself, its session key) last started under.
type Recorded = (Option<String>, Option<String>);

fn recorded(
    conn: &Connection,
    tool: Tool,
    session_key: &str,
    conversation_id: Option<&str>,
) -> Result<Option<Recorded>, WalletError> {
    let map = |r: &rusqlite::Row<'_>| -> rusqlite::Result<Recorded> { Ok((r.get(0)?, r.get(1)?)) };
    if let Some(cid) = conversation_id.filter(|c| !c.is_empty()) {
        let r = conn
            .query_row(
                "SELECT account_id, home FROM llm_session_logins WHERE tool = ?1 AND conversation_id = ?2 \
                 ORDER BY recorded_at DESC, rowid DESC LIMIT 1",
                params![tool.as_str(), cid],
                map,
            )
            .optional()?;
        if r.is_some() {
            return Ok(r);
        }
    }
    Ok(conn
        .query_row(
            "SELECT account_id, home FROM llm_session_logins WHERE session_key = ?1 AND tool = ?2 AND conversation_id = ''",
            params![session_key, tool.as_str()],
            map,
        )
        .optional()?)
}

/// The home a resumed conversation's history is in: its recorded home,
/// else the tool's live home (pool and API-key sessions run there, and
/// so did every conversation from before K2 tracked logins).
fn recorded_home(rec: &Option<Recorded>, tool: Tool) -> PathBuf {
    match rec {
        Some((_, Some(h))) if !h.is_empty() => PathBuf::from(h),
        _ => store::live_home(tool).0,
    }
}

fn login_env(tool: Tool, e: &Entry, source: Source) -> Result<SpawnLogin, WalletError> {
    if e.is_api_key() {
        return Ok(SpawnLogin {
            tool,
            account_id: Some(e.id.clone()),
            kind: Some(kind::API_KEY.into()),
            source,
            home: None,
            env: api_key_env(tool, &e.id)?,
            carry: None,
        });
    }
    let slot = store::slot_dir(tool, &e.id);
    Ok(SpawnLogin {
        tool,
        account_id: Some(e.id.clone()),
        kind: Some(kind::SUBSCRIPTION.into()),
        source,
        home: Some(slot.clone()),
        env: vec![(tool.home_env_var().to_string(), slot.to_string_lossy().into_owned())],
        carry: None,
    })
}

fn pool(conn: &Connection, tool: Tool) -> Result<SpawnLogin, WalletError> {
    // The pool's active login is an API key → inject it; otherwise the
    // live home, no env (sessions launch exactly as before).
    if let Some(id) = active_id(conn, tool)? {
        if let Some(e) = get(conn, &id)? {
            if e.is_api_key() && e.removed_at.is_none() {
                let mut l = login_env(tool, &e, Source::Pool)?;
                l.account_id = None;
                return Ok(l);
            }
        }
    }
    Ok(SpawnLogin { tool, account_id: None, kind: None, source: Source::Pool, home: None, env: Vec::new(), carry: None })
}

/// Decide the login for a spawn. `conversation_id` + `is_resume`: a
/// resume runs on its chat's pin (session key or conversation, the newer
/// pick) when there is one, with [`SpawnLogin::carry`] set when the
/// conversation's history lives in another home; otherwise on the login
/// its conversation (or, for ids the CLI minted itself, its session key)
/// started under, even when workspace pins or the server default have
/// changed since; a conversation with no record started in the live
/// home. A fresh spawn takes chat pin > workspace pin > pool.
pub fn decide_spawn(
    conn: &Connection,
    tool: Tool,
    session_key: &str,
    project_id: Option<&str>,
    conversation_id: Option<&str>,
    is_resume: bool,
) -> Result<SpawnLogin, WalletError> {
    if is_resume {
        let rec = recorded(conn, tool, session_key, conversation_id)?;
        if let Some(p) = chat_pin(conn, tool, session_key, conversation_id)? {
            if let Some(e) = get(conn, &p.account_id)?.filter(|e| e.removed_at.is_none()) {
                let mut l = login_env(tool, &e, Source::SessionPin)?;
                if let Some(cid) = conversation_id.map(str::trim).filter(|c| valid_conversation_id(c)) {
                    let from = recorded_home(&rec, tool);
                    let to = l.home.clone().unwrap_or_else(|| store::live_home(tool).0);
                    if from != to {
                        l.carry = Some(Carry { conversation_id: cid.to_string(), from, to });
                    }
                }
                return Ok(l);
            }
        }
        return match rec.map(|(a, _)| a) {
            Some(Some(id)) => {
                let e = get(conn, &id)?.ok_or_else(|| WalletError::NotFound(id.clone()))?;
                if e.removed_at.is_some() {
                    return Err(WalletError::Conflict(
                        "account_unavailable",
                        format!("this chat started on the {} token, which was removed; continue it in a new chat", e.label),
                    ));
                }
                login_env(tool, &e, Source::Resume)
            }
            // Started in the live home (or before K2 tracked logins).
            _ => {
                let mut l = pool(conn, tool)?;
                l.source = Source::Resume;
                Ok(l)
            }
        };
    }
    if let Some(p) = pin_for(conn, tool, session_key, conversation_id, project_id)? {
        if let Some(e) = get(conn, &p.account_id)? {
            if e.removed_at.is_none() {
                let src = if p.scope_kind == "session" { Source::SessionPin } else { Source::WorkspacePin };
                return login_env(tool, &e, src);
            }
        }
    }
    pool(conn, tool)
}

/// Record the login a session / conversation started under.
pub fn record_spawn(
    conn: &Connection,
    login: &SpawnLogin,
    session_key: &str,
    conversation_id: Option<&str>,
) -> Result<(), WalletError> {
    let ts = now();
    let home = login.home.as_ref().map(|h| h.to_string_lossy().into_owned());
    // API keys from the pool aren't a home; record only pinned logins.
    let acct = login.account_id.clone();
    for cid in [Some(""), conversation_id.filter(|c| !c.is_empty())].into_iter().flatten() {
        conn.execute(
            "INSERT INTO llm_session_logins (session_key, tool, conversation_id, account_id, home, recorded_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
             ON CONFLICT(session_key, tool, conversation_id) DO UPDATE SET account_id = excluded.account_id, \
             home = excluded.home, recorded_at = excluded.recorded_at",
            params![session_key, login.tool.as_str(), cid, acct, home, ts],
        )?;
    }
    Ok(())
}

/// Where a tool keeps conversations inside its home, one folder per
/// project: Claude `projects/<slug>/<id>.jsonl` (plus `<id>/`), Grok
/// `sessions/<cwd>/<id>/`. Codex shadow homes share `sessions/` with the
/// normal home and Gemini tokens are API keys run in the live home, so
/// they have nothing to carry.
fn conversation_root(tool: Tool) -> Option<&'static str> {
    match tool {
        Tool::Claude => Some("projects"),
        Tool::Grok => Some("sessions"),
        Tool::Codex | Tool::Gemini => None,
    }
}

fn newer_or_missing(src: &Path, dst: &Path) -> bool {
    let Ok(dm) = std::fs::metadata(dst) else { return true };
    match (std::fs::metadata(src).and_then(|m| m.modified()), dm.modified()) {
        (Ok(s), Ok(d)) => s > d,
        _ => false,
    }
}

/// Copy `src` (a file or a folder, never through a symlink) to `dst`,
/// file by file, replacing only files that are missing or older there.
fn copy_entry(src: &Path, dst: &Path) -> Result<usize, String> {
    let meta = std::fs::symlink_metadata(src).map_err(|e| e.to_string())?;
    if meta.file_type().is_symlink() {
        return Ok(0);
    }
    if meta.is_dir() {
        std::fs::create_dir_all(dst).map_err(|e| e.to_string())?;
        let mut n = 0;
        for ent in std::fs::read_dir(src).map_err(|e| e.to_string())?.flatten() {
            n += copy_entry(&ent.path(), &dst.join(ent.file_name()))?;
        }
        return Ok(n);
    }
    if !newer_or_missing(src, dst) {
        return Ok(0);
    }
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::copy(src, dst).map_err(|e| format!("{}: {e}", dst.display()))?;
    Ok(1)
}

/// Bring one conversation's history from the home it last ran in to the
/// home its chat's token runs in, so `--resume <id>` finds it there.
/// Copies (never moves or deletes) the conversation's files under the
/// same project folder names; a file already in `to` is replaced only by
/// a newer one. Returns the number of files copied (0 for tools whose
/// homes share conversations).
pub fn carry_conversation(tool: Tool, from: &Path, to: &Path, conversation_id: &str) -> Result<usize, String> {
    let Some(root) = conversation_root(tool) else { return Ok(0) };
    if !valid_conversation_id(conversation_id) {
        return Err(format!("not a conversation id: {conversation_id:?}"));
    }
    if from == to {
        return Ok(0);
    }
    let Ok(dirs) = std::fs::read_dir(from.join(root)) else { return Ok(0) };
    let mut n = 0;
    for d in dirs.flatten() {
        // `file_type` does not follow links: a linked project folder is skipped.
        if !d.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        for name in [format!("{conversation_id}.jsonl"), conversation_id.to_string()] {
            let src = d.path().join(&name);
            if std::fs::symlink_metadata(&src).is_err() {
                continue;
            }
            n += copy_entry(&src, &to.join(root).join(d.file_name()).join(&name))?;
        }
    }
    Ok(n)
}

/// Get a pinned subscription slot ready before the spawn: Codex shadow
/// home, Claude shared settings + folder trust. The caller holds the
/// slot lock.
pub fn prepare_slot_home(tool: Tool, id: &str, cwd: Option<&Path>) -> Result<(), WalletError> {
    let slot = store::slot_dir(tool, id);
    let (live, _) = store::live_home(tool);
    match tool {
        Tool::Codex => materialize_codex_shadow(&slot, &live).map_err(WalletError::Io),
        Tool::Claude => prepare_claude_home(&slot, &live, cwd).map_err(WalletError::Io),
        _ => Ok(()),
    }
}

/// Every pin, grouped by login (for the LLMs page "Pinned to: …").
pub fn pins_by_account(conn: &Connection) -> Result<HashMap<String, Vec<Pin>>, WalletError> {
    let mut m: HashMap<String, Vec<Pin>> = HashMap::new();
    for p in list_pins(conn)? {
        m.entry(p.account_id.clone()).or_default().push(p);
    }
    Ok(m)
}
