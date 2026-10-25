//! `GET /cli/llm/accounts/for` and `POST /cli/llm/accounts/for/refresh`:
//! the LLM token ONE chat runs on, with that token's usage chip number
//! (boot-status feature `llm-account-for-v1`).
//!
//! The phone's chat header shows the highest usage percent next to the LLM
//! icon, like the desktop top-bar chip, but for the token this chat's
//! session actually uses (Companion, Rosson 2026-10-24).
//!
//! **Which chat.** `?addr=` takes what `k2 msg` takes: a workspace (name,
//! handle, path or id: its pinned chat) or a sidecar address `ws/2` /
//! `ws/reviewer` (that sidecar's own session), or a conversation /
//! session uuid. `?conversation=<id>` names a chat by its provider
//! conversation id.
//!
//! **Which token** ([`k2_core::llm_accounts::pins::decide`], the rules the
//! spawn itself uses):
//! - a person's own token on their personal agent (`source: pinned`,
//!   `pinScope: person`);
//! - a fresh start: the chat's own pin (its session key, or its
//!   conversation; for a sidecar that is the SIDECAR's pin) > the
//!   workspace's pin > the server default (`pinned` / `pinned` /
//!   `default`). A sidecar with no pin of its own falls back to its
//!   workspace's pin, then the default;
//! - a resume: the chat's own pin, else the token the conversation started
//!   on (`resumed`; a conversation that started on the server default
//!   follows the default's live home).
//! - A chat that is RUNNING reports the token it was started on (the spawn
//!   records it), so a pin changed after the start shows from the next
//!   start; `nextStart` then says what that will be.
//!
//! **The number** is the top-bar chip's
//! ([`crate::subscription_usage::chip_percent`]) over the same cached
//! readings: the shared cache row when the chat runs on the tool's live
//! token (always for `default`), else that token's own last reading. No
//! reading → `windows: []`, `maxPercent: null`; never an invented number.
//!
//! **Freshness.** GET returns the last cached reading (`updatedAt` = when
//! it was read). `POST …/for/refresh {addr}` re-reads just that token, at
//! most once a minute per token (`refreshed: false` + `retryAfterSecs`
//! otherwise). The phone reads on chat open and on return to the
//! foreground, and never polls.
//!
//! Callers: as `/cli/llm/accounts/list` (owner token, Connect logins and
//! paired phones, and passports read-only). Never returns a token.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use k2_core::llm_accounts::pins::{self, Source};
use k2_core::llm_accounts::{self as wallet_core, kind, state, Entry, Tool, WalletError};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::cli_response::CliResponse;
use crate::subscription_usage::{self as su, HarnessUsage};

pub const FOR: &str = "/cli/llm/accounts/for";
pub const FOR_REFRESH: &str = "/cli/llm/accounts/for/refresh";

/// One refresh per token per minute.
pub const REFRESH_EVERY: Duration = Duration::from_secs(60);

fn refuse(status: &'static str, code: &str, hint: impl Into<String>) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: json!({"error": {"code": code, "hint": hint.into()}}).to_string(),
    }
}

fn wallet_err(e: WalletError) -> CliResponse {
    refuse(e.http_status(), e.code(), e.to_string())
}

// ── Which chat ──────────────────────────────────────────────────────

/// The chat a request names, and what its session runs (or will run).
#[derive(Debug, Clone)]
pub struct Target {
    /// `ws` or `ws/handle`.
    pub address: String,
    pub workspace_id: String,
    /// The v2 session key the spawn uses: the workspace id for the pinned
    /// chat, the tab's agent name (`tab-<id>`) for a sidecar.
    pub session_key: String,
    pub sidecar: bool,
    pub program: Option<String>,
    pub args: Vec<String>,
    pub conversation: Option<String>,
    /// A session is running for this chat now.
    pub live: bool,
}

impl Target {
    pub fn tool(&self) -> Option<Tool> {
        self.program.as_deref().and_then(Tool::from_command)
    }

    /// Does (or would) the session resume a conversation? Running: its
    /// argv says so. Not running: its next start resumes when the chat has
    /// a conversation id (a wake replays the saved session with a resume).
    pub fn resume(&self) -> bool {
        match (self.live, self.tool()) {
            (true, Some(t)) => crate::llm_accounts_runtime::is_resume_argv(t, &self.args),
            (true, None) => false,
            (false, _) => self.conversation.is_some(),
        }
    }
}

fn not_found(hint: impl Into<String>) -> CliResponse {
    refuse("404 Not Found", "not_found", hint)
}

fn args_of(json: Option<&str>) -> Vec<String> {
    json.and_then(|s| serde_json::from_str(s).ok()).unwrap_or_default()
}

fn conversation_of(program: Option<&str>, args: &[String], saved: Option<&str>) -> Option<String> {
    k2_core::workspace::provider_resume::session_id_from_spawn_argv(program.unwrap_or(""), args)
        .or_else(|| saved.map(str::trim).filter(|s| !s.is_empty()).map(str::to_string))
}

fn workspace_name(project_id: &str) -> String {
    k2_core::workspace_session_handles::workspace_address_name_shared(project_id).unwrap_or_else(|_| project_id.to_string())
}

/// The workspace's pinned chat (session key = the workspace id).
pub fn canonical_target(project_id: &str) -> Target {
    let live = crate::session_lookup::lookup_any(project_id);
    let saved = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::db::schema::WorkspaceSession::get(&conn, project_id).ok().flatten()
    };
    let (program, args) = match &live {
        Some(l) => (l.command(), l.args()),
        None => (saved.as_ref().map(|s| s.harness.clone()).filter(|h| !h.trim().is_empty()), Vec::new()),
    };
    let conversation = conversation_of(
        program.as_deref(),
        if live.is_some() { &args } else { &[] },
        if live.is_some() { None } else { saved.as_ref().and_then(|s| s.session_id.as_deref()) },
    );
    Target {
        address: workspace_name(project_id),
        workspace_id: project_id.to_string(),
        session_key: crate::canonical_session::canonical_key_for(project_id),
        sidecar: false,
        program,
        args,
        conversation,
        live: live.is_some(),
    }
}

/// A sidecar of `project_id` by its conversation key: its own session.
pub fn sidecar_target(project_id: &str, conversation_key: &str, handle: &str) -> Result<Target, CliResponse> {
    let tab = crate::workspace_msg::tab_row_for_conversation(project_id, conversation_key);
    let live = crate::workspace_msg::lookup_live_for_sidecar(project_id, conversation_key);
    if tab.is_none() && live.is_none() {
        return Err(not_found(format!("no chat at {}/{handle}", workspace_name(project_id))));
    }
    let session_key = tab.as_ref().map(|t| t.agent_name.clone()).unwrap_or_else(|| {
        format!("tab-{}", k2_core::workspace_session_handles::normalize_pane_key(conversation_key))
    });
    let (program, args) = match &live {
        Some(l) => (l.command(), l.args()),
        None => (
            tab.as_ref().and_then(|t| t.command.clone()).filter(|c| !c.trim().is_empty()),
            tab.as_ref().map(|t| args_of(t.args_json.as_deref())).unwrap_or_default(),
        ),
    };
    let conversation = conversation_of(program.as_deref(), &args, tab.as_ref().and_then(|t| t.session_id.as_deref()));
    Ok(Target {
        address: k2_core::workspace_session_handles::format_address(&workspace_name(project_id), Some(handle)),
        workspace_id: project_id.to_string(),
        session_key,
        sidecar: true,
        program,
        args,
        conversation,
        live: live.is_some(),
    })
}

/// A chat by its provider conversation id (or a session uuid).
fn conversation_target(id: &str) -> Result<Target, CliResponse> {
    let id = id.trim();
    let (tab, ws) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let tab = k2_core::db::schema::WorkspaceTabSession::get_by_session_id(&conn, id).ok().flatten();
        let ws = if tab.is_none() {
            k2_core::db::schema::WorkspaceSession::get_by_session_id(&conn, id).ok().flatten()
        } else {
            None
        };
        (tab, ws)
    };
    if let Some(t) = tab {
        let handle = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::workspace_session_handles::current_handle_for(&conn, &t.project_id, id).ok().flatten()
        }
        .unwrap_or_else(|| id.chars().take(8).collect());
        return sidecar_target(&t.project_id, id, &handle);
    }
    if let Some(w) = ws {
        return Ok(canonical_target(&w.project_id));
    }
    Err(not_found(format!("no chat with conversation {id} on this server")))
}

/// The chat `?addr=` / `?conversation=` names.
pub fn resolve(params: &HashMap<String, String>) -> Result<Target, CliResponse> {
    let pick = |k: &str| params.get(k).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    if let Some(c) = pick("conversation") {
        return conversation_target(&c);
    }
    let Some(addr) = pick("addr").or_else(|| pick("agent")) else {
        return Err(refuse("400 Bad Request", "bad_request", "addr (an agent address such as sales or sales/2) or conversation is required"));
    };
    if addr.contains("::") {
        return Err(not_found("this server's chats only (ask the agent's own server for agent::host)"));
    }
    use crate::workspace_msg::MsgTarget;
    match crate::workspace_msg::resolve_msg_target(&addr) {
        Some(MsgTarget::WorkspaceCanonical { path }) => {
            let id = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                k2_core::workspace::agent_identity::resolve_project_id(&conn, &path)
            }
            .ok_or_else(|| not_found(format!("no workspace {addr}")))?;
            Ok(canonical_target(&id))
        }
        Some(MsgTarget::Sidecar { path, conversation_key, current_handle, .. }) => {
            let id = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                k2_core::workspace::agent_identity::resolve_project_id(&conn, &path)
            }
            .ok_or_else(|| not_found(format!("no workspace for {addr}")))?;
            sidecar_target(&id, &conversation_key, &current_handle)
        }
        Some(MsgTarget::Session { session_id }) => conversation_target(&session_id),
        None => Err(not_found(
            crate::workspace_msg::explain_unresolved_sidecar(&addr).unwrap_or_else(|| format!("no agent {addr} on this server")),
        )),
    }
}

// ── Which token ─────────────────────────────────────────────────────

/// What the chat runs on, as reported.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub tool: Tool,
    /// `pinned` · `resumed` · `default`.
    pub source: &'static str,
    /// `chat` · `workspace` · `person` for `pinned`.
    pub pin_scope: Option<&'static str>,
    /// The wallet token (or a person's item, `pw_…`). `None` with
    /// `default` = the live login has no wallet entry.
    pub account_id: Option<String>,
    pub label: String,
    pub kind: Option<String>,
    /// The token is the tool's live one: its numbers are the shared cache
    /// row the top-bar chip reads.
    pub shared: bool,
    /// A resume onto a removed token: the chat can't continue on it.
    pub unavailable: Option<String>,
    /// A running chat whose next start would use another token.
    pub next_start: Option<(Option<String>, &'static str)>,
}

fn source_word(s: Source) -> &'static str {
    match s {
        Source::SessionPin | Source::WorkspacePin | Source::PersonPin => "pinned",
        Source::Resume => "resumed",
        Source::Pool => "default",
    }
}

fn pin_scope(s: Source) -> Option<&'static str> {
    match s {
        Source::SessionPin => Some("chat"),
        Source::WorkspacePin => Some("workspace"),
        Source::PersonPin => Some("person"),
        _ => None,
    }
}

/// The decision before a running chat's record is applied:
/// `(source, account id, entry)`. Person layer first, as at spawn.
fn decide_raw(
    conn: &rusqlite::Connection,
    tool: Tool,
    t: &Target,
    resume: bool,
) -> Result<(Source, Option<String>), WalletError> {
    let conv = t.conversation.as_deref();
    if let Some(c) = k2_core::person_wallet::decide::candidate(conn, tool, &t.session_key, Some(&t.workspace_id), conv, resume)? {
        return Ok((Source::PersonPin, Some(c.item_id)));
    }
    let d = pins::decide(conn, tool, &t.session_key, Some(&t.workspace_id), conv, resume)?;
    Ok((d.source, d.account.map(|e| e.id)))
}

/// Resolve the token a target's session runs on (see the module doc).
pub fn resolve_account(conn: &rusqlite::Connection, tool: Tool, t: &Target) -> Result<Resolved, WalletError> {
    let resume = t.resume();
    let (mut source, mut account) = decide_raw(conn, tool, t, resume)?;
    let mut next_start = None;
    if t.live {
        // A running chat runs on the token its spawn recorded.
        if let Some(rec) = pins::recorded_account(conn, tool, &t.session_key, t.conversation.as_deref())? {
            if rec != account {
                next_start = Some((account.clone(), source_word(source)));
                source = match (&rec, resume) {
                    (None, _) => Source::Pool,
                    (Some(_), true) => Source::Resume,
                    (Some(id), false) if k2_core::person_wallet::is_item_id(id) => Source::PersonPin,
                    (Some(_), false) => Source::WorkspacePin,
                };
                account = rec;
            }
        }
    }
    let live_owner = wallet_core::live_owner(conn, tool)?;
    let active = wallet_core::active_id(conn, tool)?;
    // The pool (default, or a resume that started in the live home).
    let pool = account.is_none();
    let id = if pool { active.clone() } else { account.clone() };
    let mut label = tool.display().to_string();
    let mut kind_s: Option<String> = None;
    let mut unavailable = None;
    if let Some(id) = id.as_deref() {
        if k2_core::person_wallet::is_item_id(id) {
            if let Some(item) = k2_core::person_wallet::get(conn, id)? {
                label = item.label.clone();
                kind_s = Some(if item.is_api_key() { kind::API_KEY } else { kind::SUBSCRIPTION }.to_string());
            }
        } else if let Some(e) = wallet_core::get(conn, id)? {
            label = e.label.clone();
            kind_s = Some(e.kind.clone());
            if e.removed_at.is_some() {
                unavailable = Some(format!(
                    "this chat started on the {} token, which was removed; continue it in a new chat",
                    e.label
                ));
            }
        }
    }
    let shared = pool || (account.is_some() && account == live_owner);
    let pin_scope_v = if source == Source::WorkspacePin && t.live && next_start.is_some() {
        // A running chat started on a pin that has changed since: which pin
        // it was is no longer known.
        None
    } else {
        pin_scope(source)
    };
    Ok(Resolved {
        tool,
        source: source_word(source),
        pin_scope: pin_scope_v,
        account_id: id,
        label,
        kind: kind_s,
        shared,
        unavailable,
        next_start,
    })
}

// ── The number ──────────────────────────────────────────────────────

/// The cached reading for a resolved token, decorated as served. `None`
/// = no reading (an API token, a person's item, air-gap, never read).
pub fn reading_for(r: &Resolved, now: DateTime<Utc>) -> Option<HarnessUsage> {
    if k2_core::airgap::enabled() || r.unavailable.is_some() {
        return None;
    }
    if r.shared {
        return su::served_shared_row(r.tool.as_str(), now);
    }
    let id = r.account_id.as_deref()?;
    if k2_core::person_wallet::is_item_id(id) {
        return None;
    }
    let row: HarnessUsage = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let e = wallet_core::get(&conn, id).ok().flatten()?;
        if e.is_api_key() {
            return None;
        }
        serde_json::from_str(e.usage_json.as_deref()?).ok()?
    };
    Some(su::decorate_row(row, now, &format!("acct:{id}")))
}

/// The `/for` answer.
pub fn answer_json(t: &Target, r: Option<&Resolved>, now: DateTime<Utc>) -> Value {
    let mut v = json!({
        "address": t.address,
        "workspaceId": t.workspace_id,
        "sessionKey": t.session_key,
        "sidecar": t.sidecar,
        "conversationId": t.conversation,
        "live": t.live,
        "tool": Value::Null,
        "accountId": Value::Null,
        "label": Value::Null,
        "kind": Value::Null,
        "source": Value::Null,
        "pinScope": Value::Null,
        "windows": [],
        "maxPercent": Value::Null,
        "updatedAt": Value::Null,
        "state": Value::Null,
        "airgap": k2_core::airgap::enabled(),
    });
    let Some(r) = r else { return v };
    let reading = reading_for(r, now);
    let o = v.as_object_mut().expect("object");
    o.insert("tool".into(), json!(r.tool.as_str()));
    o.insert("accountId".into(), json!(r.account_id));
    o.insert("label".into(), json!(r.label));
    o.insert("kind".into(), json!(r.kind));
    o.insert("source".into(), json!(r.source));
    o.insert("pinScope".into(), json!(r.pin_scope));
    o.insert("usageFrom".into(), json!(if r.shared { "server_default" } else { "token" }));
    if let Some(why) = &r.unavailable {
        o.insert("unavailable".into(), json!(why));
    }
    if let Some((next, src)) = &r.next_start {
        o.insert("nextStart".into(), json!({"accountId": next, "source": src}));
    }
    if let Some(row) = reading {
        let served = su::served_row_json(&row, now);
        o.insert("windows".into(), served["windows"].clone());
        o.insert("maxPercent".into(), served["maxPercent"].clone());
        o.insert("updatedAt".into(), if row.checked_at.is_empty() { Value::Null } else { json!(row.checked_at) });
        o.insert("state".into(), json!(row.state));
        if let Some(e) = &row.error {
            o.insert("error".into(), json!(e));
        }
    }
    v
}

fn resolve_all(params: &HashMap<String, String>) -> Result<(Target, Option<Resolved>), CliResponse> {
    let t = resolve(params)?;
    let Some(tool) = t.tool() else { return Ok((t, None)) };
    let r = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        resolve_account(&conn, tool, &t)
    }
    .map_err(wallet_err)?;
    Ok((t, Some(r)))
}

/// `GET /cli/llm/accounts/for`.
pub fn handle_get(params: &HashMap<String, String>) -> CliResponse {
    match resolve_all(params) {
        Ok((t, r)) => CliResponse::ok_json(answer_json(&t, r.as_ref(), Utc::now()).to_string()),
        Err(resp) => resp,
    }
}

// ── Refresh one token ───────────────────────────────────────────────

fn last_refresh() -> &'static Mutex<HashMap<String, Instant>> {
    static M: std::sync::OnceLock<Mutex<HashMap<String, Instant>>> = std::sync::OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Take the token's refresh slot: `Err(seconds to wait)` within a minute
/// of the last one.
pub fn take_refresh_slot(key: &str, now: Instant) -> Result<(), u64> {
    let mut m = last_refresh().lock().unwrap_or_else(|p| p.into_inner());
    if let Some(prev) = m.get(key) {
        let since = now.saturating_duration_since(*prev);
        if since < REFRESH_EVERY {
            return Err((REFRESH_EVERY - since).as_secs().max(1));
        }
    }
    m.insert(key.to_string(), now);
    Ok(())
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RefreshBody {
    addr: Option<String>,
    conversation: Option<String>,
}

/// `POST /cli/llm/accounts/for/refresh {addr}`: re-read just this chat's
/// token, then answer as GET plus `refreshed` (and `reason` /
/// `retryAfterSecs` when it did not).
pub fn handle_refresh(body: &[u8]) -> CliResponse {
    let b: RefreshBody = if body.iter().all(|c| c.is_ascii_whitespace()) {
        RefreshBody::default()
    } else {
        match serde_json::from_slice(body) {
            Ok(b) => b,
            Err(_) => return refuse("400 Bad Request", "bad_request", "the body must be a JSON object"),
        }
    };
    let mut params = HashMap::new();
    if let Some(a) = b.addr {
        params.insert("addr".to_string(), a);
    }
    if let Some(c) = b.conversation {
        params.insert("conversation".to_string(), c);
    }
    let (t, r) = match resolve_all(&params) {
        Ok(x) => x,
        Err(resp) => return resp,
    };
    let (refreshed, reason, retry): (bool, Option<&str>, Option<u64>) = match &r {
        None => (false, Some("no_llm"), None),
        Some(r) => refresh_one(r),
    };
    // Re-resolve: a probe can't move a pin, but read the new numbers.
    let r = match resolve_all(&params) {
        Ok((_, r2)) => r2.or(r),
        Err(_) => r,
    };
    let mut v = answer_json(&t, r.as_ref(), Utc::now());
    let o = v.as_object_mut().expect("object");
    o.insert("refreshed".into(), json!(refreshed));
    if let Some(why) = reason {
        o.insert("reason".into(), json!(why));
    }
    if let Some(s) = retry {
        o.insert("retryAfterSecs".into(), json!(s));
    }
    CliResponse::ok_json(v.to_string())
}

/// Probe the one token, rate-limited per token. `(refreshed, reason,
/// retryAfterSecs)`.
fn refresh_one(r: &Resolved) -> (bool, Option<&'static str>, Option<u64>) {
    if k2_core::airgap::enabled() {
        return (false, Some("airgap"), None);
    }
    if r.unavailable.is_some() {
        return (false, Some("unavailable"), None);
    }
    let key = if r.shared {
        format!("shared:{}", r.tool.as_str())
    } else {
        match r.account_id.as_deref() {
            Some(id) => format!("acct:{id}"),
            None => return (false, Some("no_usage"), None),
        }
    };
    let entry: Option<Entry> = if r.shared {
        None
    } else {
        let id = r.account_id.as_deref().unwrap_or_default();
        if k2_core::person_wallet::is_item_id(id) {
            return (false, Some("no_usage"), None);
        }
        let db = k2_core::db::shared();
        let conn = db.lock();
        match wallet_core::get(&conn, id).ok().flatten() {
            Some(e) if !e.is_api_key() && e.removed_at.is_none() => Some(e),
            _ => return (false, Some("no_usage"), None),
        }
    };
    if !r.tool.subscription_supported() {
        return (false, Some("no_usage"), None);
    }
    if let Err(wait) = take_refresh_slot(&key, Instant::now()) {
        return (false, Some("rate_limited"), Some(wait));
    }
    // Tests (temp HOME / shims) and denied probes never call a vendor.
    if !crate::llm_accounts_runtime::vendor_calls_allowed() {
        return (false, Some("probes_off"), None);
    }
    let res = match entry {
        None => su::refresh_shared(Some(r.tool.as_str())).map(|_| ()),
        Some(e) if e.state == state::SIGNED_IN => crate::llm_accounts_runtime::probe_idle_usage(&e),
        Some(_) => return (false, Some("not_signed_in"), None),
    };
    match res {
        Ok(()) => (true, None, None),
        Err(e) => {
            k2_core::log_debug!("[llm-accounts/for] usage {key}: {e}");
            (false, Some("probe_failed"), None)
        }
    }
}
