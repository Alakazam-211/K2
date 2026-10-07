//! Pins, API-key logins, and the per-session login decision.
//!
//! **Pins.** A workspace (its canonical agent, sidecars, heartbeats and
//! tabs) or one session (its v2 session key) can run on a specific login
//! per tool instead of the pool's active one. Precedence at a fresh
//! spawn: session pin > workspace pin > the pool. A resume always uses
//! the login the conversation started under ([`record_spawn`]).
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
                    "{} is the pool's active {} login. A login can't be live in two places (sign-ins rotate), so switch the pool to another login first, or pin a different one.",
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

/// The pin that applies to a fresh session: session > workspace.
pub fn pin_for(
    conn: &Connection,
    tool: Tool,
    session_key: &str,
    project_id: Option<&str>,
) -> Result<Option<Pin>, WalletError> {
    let one = |kind: ScopeKind, id: &str| -> Result<Option<Pin>, WalletError> {
        Ok(conn
            .query_row(
                &format!(
                    "SELECT {PIN_COLS} FROM llm_account_pins WHERE scope_kind = ?1 AND scope_id = ?2 AND tool = ?3"
                ),
                params![kind.as_str(), id, tool.as_str()],
                map_pin,
            )
            .optional()?)
    };
    if !session_key.is_empty() {
        if let Some(p) = one(ScopeKind::Session, session_key)? {
            return Ok(Some(p));
        }
    }
    if let Some(pid) = project_id.filter(|s| !s.is_empty()) {
        if let Some(p) = one(ScopeKind::Workspace, pid)? {
            return Ok(Some(p));
        }
    }
    Ok(None)
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
        return Err(WalletError::Conflict("not_api_key", "that login is a sign-in, not an API key".into()));
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

fn recorded(
    conn: &Connection,
    tool: Tool,
    session_key: &str,
    conversation_id: Option<&str>,
) -> Result<Option<Option<String>>, WalletError> {
    if let Some(cid) = conversation_id.filter(|c| !c.is_empty()) {
        let r: Option<Option<String>> = conn
            .query_row(
                "SELECT account_id FROM llm_session_logins WHERE tool = ?1 AND conversation_id = ?2 \
                 ORDER BY recorded_at DESC LIMIT 1",
                params![tool.as_str(), cid],
                |r| r.get(0),
            )
            .optional()?;
        if r.is_some() {
            return Ok(r);
        }
    }
    Ok(conn
        .query_row(
            "SELECT account_id FROM llm_session_logins WHERE session_key = ?1 AND tool = ?2 AND conversation_id = ''",
            params![session_key, tool.as_str()],
            |r| r.get(0),
        )
        .optional()?)
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
    Ok(SpawnLogin { tool, account_id: None, kind: None, source: Source::Pool, home: None, env: Vec::new() })
}

/// Decide the login for a spawn. `conversation_id` + `is_resume`: a
/// resume always uses the login its conversation (or, for ids the CLI
/// minted itself, its session key) started under, even when pins have
/// changed since; a conversation with no record started in the live
/// home. A fresh spawn takes session pin > workspace pin > pool.
pub fn decide_spawn(
    conn: &Connection,
    tool: Tool,
    session_key: &str,
    project_id: Option<&str>,
    conversation_id: Option<&str>,
    is_resume: bool,
) -> Result<SpawnLogin, WalletError> {
    if is_resume {
        return match recorded(conn, tool, session_key, conversation_id)? {
            Some(Some(id)) => {
                let e = get(conn, &id)?.ok_or_else(|| WalletError::NotFound(id.clone()))?;
                if e.removed_at.is_some() {
                    return Err(WalletError::Conflict(
                        "account_unavailable",
                        format!("this conversation started on the {} login, which was removed; continue it in a new chat", e.label),
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
    if let Some(p) = pin_for(conn, tool, session_key, project_id)? {
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
