//! `/cli/shared*`: what the owner shares with a **Voice & Chat only** login
//! (prd-voice-chat-only-members-v1, the reviewer extension for 0.46.10).
//!
//! A Voice & Chat only login (`Role::VoiceChat`) clears only the route
//! table's `VoiceChat` rows. Besides voice rooms, the owner or an Admin may
//! share, per login and by id (`connect_users::Shares`):
//! - **agents**: this login may read that agent's Thread and post in it AS
//!   ITSELF (a stamped compose post, delivered into the agent's terminal by
//!   the normal Thread compose path, which wakes it). Nothing else about the
//!   agent: no list of other agents, no settings, no terminal, no files, no
//!   cards, no other conversation;
//! - **Gardens**: read-only on a phone, through the remote Garden door
//!   (`zen_remote_routes::gate` reads [`shared_garden`]).
//!
//! The routes (all answer only a live, enabled `voice_chat` login, password
//! or paired phone; anyone else gets 403 `voice_chat_only`, the owner token
//! included, which reads Threads through `/cli/thread`):
//!
//! ```text
//! GET  /cli/shared                        {ok, role, agents:[{id,address,name}], gardens:[{id,name}]}
//! GET  /cli/shared/thread?agent=<address|id>[&limit][&after_seq|&since_seq|&before_seq]
//!                                         the agent's Thread: GET /cli/thread's answer for it
//! POST /cli/shared/thread/post {agent, text, clientMsgId?, via?: "compose"}
//!                                         POST /cli/thread/post's answer (via=compose, as this login)
//! ```
//!
//! **Why its own routes and not `/cli/thread` with a check.** The route gate
//! sees the path and the query, never a POST body, and the Thread routes
//! read the address from either, plus `from`, `via`, `origin`, `context`,
//! `command` and file attachments. Here the handler builds the inner
//! request itself from a closed list of fields: the address is always the
//! shared agent's canonical one (never the caller's string), `via` is always
//! `compose` (a `via=thread` post from a person would be stored as the
//! agent's own reply), and no slash command, attachment, widget origin or
//! context line rides along. Lines after the first are quoted (`> `), as an
//! App guest's are, so a line can't pose as the owner's or K2's own.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::{json, Value as J};

use k2_core::connect_users::{self, Shares};

use crate::cli_response::CliResponse;

pub const LIST: &str = "/cli/shared";
pub const THREAD: &str = "/cli/shared/thread";
pub const THREAD_POST: &str = "/cli/shared/thread/post";

/// `/boot-status` `features`: the server has the Voice & Chat only role,
/// its shares and these routes.
pub const FEATURE: &str = "voice-chat-role-v1";

/// Thread post bodies are small (no files ride here).
pub const MAX_BODY: usize = 64 * 1024;

pub fn is_route(path: &str) -> bool {
    matches!(path, LIST | THREAD | THREAD_POST)
}

pub fn is_post_route(path: &str) -> bool {
    path == THREAD_POST
}

/// `{ok: false, error: {code, hint}}`, the Garden door's envelope.
pub fn refuse(status: &'static str, code: &str, hint: impl Into<String>) -> CliResponse {
    let hint: String = hint.into();
    CliResponse { status, content_type: "application/json", body: json!({ "ok": false, "error": { "code": code, "hint": hint } }).to_string() }
}

/// The login a request names, when it is a live session (password or
/// paired phone). `None` for the owner token, a passport, an App pass, an
/// API key, nothing.
pub fn login_of(query: &str, bearer: Option<&str>, owner_token: &str) -> Option<String> {
    let presented = crate::routes::http::extract_token(query)
        .filter(|s| !s.is_empty())
        .or_else(|| bearer.map(str::trim).filter(|s| !s.is_empty()))?;
    if crate::routes::http::ct_eq_token(presented, owner_token) {
        return None;
    }
    connect_users::validate_session(presented)
}

/// The shares of `login` when it is a live Voice & Chat only login, else the
/// refusal every route here gives.
fn shares_of(login: Option<&str>) -> Result<(String, Shares), CliResponse> {
    let Some(login) = login else {
        return Err(refuse("403 Forbidden", "voice_chat_only", "These routes are for a Voice & Chat only login. The owner and Members read Threads through /cli/thread."));
    };
    match connect_users::voice_chat_shares(login) {
        Some(s) => Ok((login.to_string(), s)),
        None => Err(refuse("403 Forbidden", "voice_chat_only", "These routes are for a Voice & Chat only login. Members read Threads through /cli/thread.")),
    }
}

// ── resolving shares (also used by users/add and users/set-shares) ──

/// One shared agent as people see it: `{id, address, name}`. `None` when the
/// workspace is gone (its share then lists nothing).
pub fn agent_view(project_id: &str) -> Option<J> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let name: String = conn
        .query_row("SELECT name FROM projects WHERE id = ?1", rusqlite::params![project_id], |r| r.get(0))
        .ok()?;
    let address = k2_core::workspace_session_handles::workspace_address_name(&conn, project_id).ok()?;
    Some(json!({ "id": project_id, "address": address, "name": name }))
}

/// One shared Garden: `{id, name}`, `None` when it's gone (or Zen isn't set
/// up on this server).
pub fn garden_view(id: &str) -> Option<J> {
    let f = k2_core::zen::ZenFiles::local();
    if !f.is_set_up() {
        return None;
    }
    f.gardens_json().into_iter().find(|g| g["id"].as_str() == Some(id)).map(|g| json!({ "id": g["id"], "name": g["name"] }))
}

/// An agent address (a handle, `ws`, a project id) → the workspace's project
/// id, for a share. 404 `unknown_agent` when nothing answers.
pub fn resolve_agent(input: &str) -> Result<String, CliResponse> {
    let input = input.trim();
    if input.is_empty() {
        return Err(refuse("400 Bad Request", "usage", "an agent share names an agent address"));
    }
    // A project id as itself.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let known: bool = conn
            .query_row("SELECT EXISTS (SELECT 1 FROM projects WHERE id = ?1)", rusqlite::params![input], |r| r.get(0))
            .unwrap_or(false);
        if known && !input.starts_with('_') {
            return Ok(input.to_string());
        }
    }
    match crate::overlay_routes::workspace_of_addr(input) {
        Ok(id) if !id.starts_with('_') => Ok(id),
        _ => Err(refuse("404 Not Found", "unknown_agent", format!("no agent '{input}' on this server"))),
    }
}

/// A Garden id or name → its id, for a share. 404 `unknown_garden`.
pub fn resolve_garden(input: &str) -> Result<String, CliResponse> {
    let input = input.trim();
    let f = k2_core::zen::ZenFiles::local();
    if input.is_empty() || !f.is_set_up() {
        return Err(refuse("404 Not Found", "unknown_garden", format!("no Garden '{input}' on this server")));
    }
    let list = f.gardens_json();
    let hit = list
        .iter()
        .find(|g| g["id"].as_str() == Some(input))
        .or_else(|| list.iter().find(|g| g["name"].as_str().is_some_and(|n| n.trim().eq_ignore_ascii_case(input))));
    hit.and_then(|g| g["id"].as_str().map(str::to_string))
        .ok_or_else(|| refuse("404 Not Found", "unknown_garden", format!("no Garden '{input}' on this server")))
}

/// Requested shares (addresses, Garden ids or names) → stored ids.
pub fn resolve_shares(agents: &[String], gardens: &[String]) -> Result<Shares, CliResponse> {
    let mut out = Shares::default();
    for a in agents {
        out.agents.push(resolve_agent(a)?);
    }
    for g in gardens {
        out.gardens.push(resolve_garden(g)?);
    }
    out.normalized().map_err(|e| refuse("400 Bad Request", "too_many_shares", e))
}

/// Stored shares as people see them (`GET /cli/users`, the CLI): a share
/// whose agent or Garden is gone shows `{id, missing: true}`.
pub fn shares_view(s: &Shares) -> J {
    let agents: Vec<J> = s.agents.iter().map(|id| agent_view(id).unwrap_or_else(|| json!({ "id": id, "missing": true }))).collect();
    let gardens: Vec<J> = s.gardens.iter().map(|id| garden_view(id).unwrap_or_else(|| json!({ "id": id, "missing": true }))).collect();
    json!({ "agents": agents, "gardens": gardens })
}

/// For the remote Garden door: is `garden` (an id or a name) one of the
/// Gardens shared with this live Voice & Chat only `login`? Returns its id.
pub fn shared_garden(login: &str, garden: &str) -> Option<String> {
    let shares = connect_users::voice_chat_shares(login)?;
    let id = resolve_garden(garden).ok()?;
    shares.gardens.contains(&id).then_some(id)
}

/// The shared agent `agent` names (its project id or today's address,
/// case-insensitive), as `(project id, canonical address)`.
fn shared_agent(shares: &Shares, agent: &str) -> Option<(String, String)> {
    let want = agent.trim();
    if want.is_empty() {
        return None;
    }
    shares.agents.iter().find_map(|id| {
        let v = agent_view(id)?;
        let address = v["address"].as_str()?.to_string();
        (id == want || address.eq_ignore_ascii_case(want)).then(|| (id.clone(), address))
    })
}

fn not_shared(agent: &str) -> CliResponse {
    refuse("403 Forbidden", "not_shared", format!("'{}' isn't shared with you on this server", agent.chars().take(64).collect::<String>()))
}

fn audit(event: &str, login: &str, outcome: String) {
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(event, login, outcome, "shared", "-", "voice_chat"));
}

// ── the routes ──────────────────────────────────────────────────────

/// One `/cli/shared*` request. `login` is [`login_of`]'s answer.
pub fn handle(path: &str, login: Option<&str>, params: &HashMap<String, String>, body: &[u8]) -> CliResponse {
    let (login, shares) = match shares_of(login) {
        Ok(x) => x,
        Err(r) => return r,
    };
    match path {
        LIST => list(&shares),
        THREAD => thread(&shares, params),
        THREAD_POST => post(&login, &shares, body),
        _ => refuse("404 Not Found", "not_found", format!("no route {path}")),
    }
}

/// `GET /cli/shared`.
fn list(shares: &Shares) -> CliResponse {
    let agents: Vec<J> = shares.agents.iter().filter_map(|id| agent_view(id)).collect();
    let gardens: Vec<J> = shares.gardens.iter().filter_map(|id| garden_view(id)).collect();
    CliResponse::ok_json(json!({ "ok": true, "role": "voice_chat", "agents": agents, "gardens": gardens }).to_string())
}

/// The read params `GET /cli/thread` takes that a shared read passes on.
const THREAD_READ_PARAMS: &[&str] = &["limit", "after_seq", "since_seq", "before_seq"];

/// `GET /cli/shared/thread?agent=`: `GET /cli/thread`'s answer for the
/// shared agent's canonical address, and nothing else the caller sent.
fn thread(shares: &Shares, params: &HashMap<String, String>) -> CliResponse {
    let agent = params.get("agent").map(String::as_str).unwrap_or("").trim().to_string();
    if agent.is_empty() {
        return refuse("400 Bad Request", "usage", "agent is required (an address from GET /cli/shared)");
    }
    let Some((_, address)) = shared_agent(shares, &agent) else {
        return not_shared(&agent);
    };
    let mut inner: HashMap<String, String> = HashMap::new();
    inner.insert("addr".into(), address.clone());
    for k in THREAD_READ_PARAMS {
        if let Some(v) = params.get(*k) {
            inner.insert((*k).to_string(), v.clone());
        }
    }
    crate::overlay_routes::dispatch("/cli/thread", &inner).unwrap_or_else(|| refuse("404 Not Found", "not_found", "no Thread"))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PostBody {
    agent: String,
    text: String,
    #[serde(default, rename = "clientMsgId")]
    client_msg_id: Option<String>,
    /// Only `"compose"` (the person's own words, delivered to the agent).
    #[serde(default)]
    via: Option<String>,
}

/// Longest post a shared Thread takes.
pub const MAX_TEXT_CHARS: usize = 8000;

/// `POST /cli/shared/thread/post {agent, text, clientMsgId?, via?}`.
fn post(login: &str, shares: &Shares, body: &[u8]) -> CliResponse {
    let b: PostBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return refuse("400 Bad Request", "usage", format!("shared/thread/post takes {{agent, text, clientMsgId?}}: {e}")),
    };
    if let Some(v) = b.via.as_deref().filter(|v| *v != "compose") {
        return refuse("403 Forbidden", "compose_only", format!("a shared Thread takes only your own posts (via=compose), not via={}", v.chars().take(16).collect::<String>()));
    }
    let Some((_, address)) = shared_agent(shares, &b.agent) else {
        audit("shared.thread.post", login, "refused not_shared".into());
        return not_shared(&b.agent);
    };
    let text = b.text.trim();
    if text.is_empty() {
        return refuse("400 Bad Request", "usage", "text is required");
    }
    if text.chars().count() > MAX_TEXT_CHARS {
        return refuse("400 Bad Request", "too_large", format!("a post is at most {MAX_TEXT_CHARS} characters"));
    }
    let mut inner = json!({
        "addr": address,
        "text": crate::workspace_msg::quote_guest_lines(text),
        "via": "compose",
    });
    if let Some(id) = b.client_msg_id {
        inner[crate::client_msg_id::PARAM] = json!(id);
    }
    let r = crate::overlay_routes::dispatch_post_as("/cli/thread/post", &HashMap::new(), inner.to_string().as_bytes(), login);
    audit("shared.thread.post", login, format!("addr={address} status={}", r.status.split(' ').next().unwrap_or("")));
    r
}

#[cfg(test)]
#[path = "shared_routes_tests.rs"]
mod tests;
