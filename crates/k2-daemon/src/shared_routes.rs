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
//! GET  /cli/shared                        {ok, role, agents:[{id,address,name,color,icon,activity}], gardens:[{id,name}]}
//! GET  /cli/shared/thread?agent=<address|id>[&limit][&after_seq|&since_seq|&before_seq]
//!                                         the agent's Thread: GET /cli/thread's answer for it
//!                                         (same thread-delta-v1 shape), plus `activity`
//! POST /cli/shared/thread/post {agent, text, clientMsgId?, via?: "compose", replyTo?: <seq>}
//!                                         POST /cli/thread/post's answer (via=compose, as this login)
//! ```
//!
//! **What this role never gets in those answers (0.46.10 follow-up):**
//! `attachments` / `attachmentCount` (the role can't open files), and the
//! agent's working state only as one coarse word, `activity: idle | working
//! | waiting`, from the guest projection of the live turn (no labels, no
//! commands, no paths). A reply is rendered here from `replyTo` (a seq in
//! the same Thread): the desktop's `↩ #N <from>:` header and the quoted
//! words are built from the stored message, never taken from the client.
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

/// One shared agent as people see it: `{id, address, name, color, icon}`
/// (`color` `#rrggbb` or null; `icon` the agent's picture as an image data
/// URL, the same checked view an App gets, or null). `None` when the
/// workspace is gone (its share then lists nothing).
pub fn agent_view(project_id: &str) -> Option<J> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let (name, color, icon): (String, Option<String>, Option<String>) = conn
        .query_row(
            "SELECT name, color, icon_url FROM projects WHERE id = ?1",
            rusqlite::params![project_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .ok()?;
    let address = k2_core::workspace_session_handles::workspace_address_name(&conn, project_id).ok()?;
    let color = color.as_deref().and_then(|c| k2_core::workspace::agent_look::normalize_color(c).ok());
    let icon = k2_core::workspace::agent_look::guest_image_view(icon.as_deref());
    Some(json!({ "id": project_id, "address": address, "name": name, "color": color, "icon": icon }))
}

/// The shared agent's working state as one coarse word, from the guest
/// projection of its pinned Chat's live turn (`thread_activity`): `waiting`
/// (on a person), `working`, or `idle` (no live turn, or it ended).
pub fn activity_of(address: &str) -> &'static str {
    let Some(conv) = crate::overlay_routes::resolve_login_addr(address) else { return "idle" };
    activity_word(crate::thread_activity::current_turn_guest(&conv).as_ref())
}

/// [`activity_of`]'s mapping over a guest turn body (pure; tests).
pub fn activity_word(turn: Option<&J>) -> &'static str {
    let Some(t) = turn else { return "idle" };
    if !t["end"].is_null() || matches!(t["state"].as_str(), Some("idle") | Some("stopped")) {
        return "idle";
    }
    if !t["waitingOn"].is_null() || t["state"] == "needs-you" || t["phase"] == "waiting" {
        return "waiting";
    }
    "working"
}

/// Drop `attachments` / `attachmentCount` anywhere in an answer: this role
/// can't open Thread files, so it is never shown a chip it can't use.
pub fn strip_attachments(v: &mut J) {
    match v {
        J::Object(o) => {
            o.remove("attachments");
            o.remove("attachmentCount");
            for x in o.values_mut() {
                strip_attachments(x);
            }
        }
        J::Array(a) => a.iter_mut().for_each(strip_attachments),
        _ => {}
    }
}

/// A 2xx JSON answer with attachments stripped and `extra` keys added;
/// anything else as is.
fn reshape(r: CliResponse, extra: &[(&str, J)]) -> CliResponse {
    if !r.status.starts_with('2') {
        return r;
    }
    let Ok(mut v) = serde_json::from_str::<J>(&r.body) else { return r };
    strip_attachments(&mut v);
    if let Some(o) = v.as_object_mut() {
        for (k, x) in extra {
            o.insert((*k).to_string(), x.clone());
        }
    }
    CliResponse { status: r.status, content_type: r.content_type, body: v.to_string() }
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

/// A data source by name, for a share (M5). 404 `unknown_source`.
pub fn resolve_source(input: &str) -> Result<String, CliResponse> {
    let name = input.trim();
    let db = k2_core::db::shared();
    let conn = db.lock();
    match k2_core::sources::store::get_by_name(&conn, name) {
        Ok(Some(row)) => Ok(row.name),
        _ => Err(refuse("404 Not Found", "unknown_source", format!("no data source '{name}' on this server"))),
    }
}

/// Requested shares (addresses, Garden ids or names, source names) →
/// stored ids.
pub fn resolve_shares(agents: &[String], gardens: &[String], sources: &[String]) -> Result<Shares, CliResponse> {
    let mut out = Shares::default();
    for a in agents {
        out.agents.push(resolve_agent(a)?);
    }
    for g in gardens {
        out.gardens.push(resolve_garden(g)?);
    }
    for s in sources {
        out.sources.push(resolve_source(s)?);
    }
    out.normalized().map_err(|e| refuse("400 Bad Request", "too_many_shares", e))
}

/// What an agent holds that a public demo agent mustn't (security review
/// H1): connections to other agents, vault grants, a mailbox, compute
/// grants, or a harness that runs without permission prompts. A check that
/// can't be made counts as a risk. Empty = a dedicated demo agent.
pub fn agent_risks(project_id: &str) -> Vec<String> {
    let mut out = Vec::new();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let count = |sql: &str| -> Result<i64, String> {
            conn.query_row(sql, rusqlite::params![project_id], |r| r.get::<_, i64>(0)).map_err(|e| e.to_string())
        };
        for (what, sql) in [
            ("connections to other agents", "SELECT COUNT(*) FROM workspace_relations WHERE source_project_id = ?1 OR target_project_id = ?1"),
            ("a mailbox", "SELECT COUNT(*) FROM mail_addresses WHERE owner_project_id = ?1 AND status = 'active'"),
            ("compute node grants", "SELECT COUNT(*) FROM compute_node_grants WHERE workspace_id = ?1"),
            ("no permission prompts (api_skip_permissions)", "SELECT COALESCE(api_skip_permissions, 0) FROM projects WHERE id = ?1"),
        ] {
            match count(sql) {
                Ok(0) => {}
                Ok(_) => out.push(what.to_string()),
                Err(e) => out.push(format!("{what}: K2 couldn't check ({e})")),
            }
        }
    }
    match crate::vault::get() {
        Ok(v) => match v.grants_to(&k2_vault::Consumer::workspace(project_id)) {
            Ok(g) if g.is_empty() => {}
            Ok(g) => out.push(format!("vault grants ({})", g.len())),
            Err(e) => out.push(format!("vault grants: K2 couldn't check ({e})")),
        },
        // No vault on this server at all: no grant can exist. A vault that
        // exists but isn't open can't be checked: a risk.
        Err(_) if !crate::vault::vault_dir(&k2_core::paths::k2_home()).join("vault.db").exists() => {}
        Err(e) => out.push(format!("vault grants: K2 couldn't check ({e})")),
    }
    out
}

/// H1: every shared agent must be a dedicated demo agent. Refuses (409
/// `agent_not_demo`, naming each risk) unless `force`; with `force` the
/// risks come back as warnings for the answer.
pub fn demo_agent_check(shares: &Shares, force: bool) -> Result<Vec<String>, CliResponse> {
    let mut warnings = Vec::new();
    for id in &shares.agents {
        let address = agent_view(id).and_then(|v| v["address"].as_str().map(str::to_string)).unwrap_or_else(|| id.clone());
        for r in agent_risks(id) {
            warnings.push(format!("{address}: {r}"));
        }
    }
    if warnings.is_empty() || force {
        return Ok(warnings);
    }
    Err(CliResponse {
        status: "409 Conflict",
        content_type: "application/json",
        body: json!({
            "ok": false,
            "error": {
                "code": "agent_not_demo",
                "hint": "share only a dedicated demo agent: anyone with this login can talk to it. Send force (k2 users … --force) to share it anyway.",
                "risks": warnings,
            },
        })
        .to_string(),
    })
}

/// Stored shares as people see them (`GET /cli/users`, the CLI): a share
/// whose agent, Garden or source is gone shows `{id|name, missing: true}`.
pub fn shares_view(s: &Shares) -> J {
    let agents: Vec<J> = s.agents.iter().map(|id| agent_view(id).unwrap_or_else(|| json!({ "id": id, "missing": true }))).collect();
    let gardens: Vec<J> = s.gardens.iter().map(|id| garden_view(id).unwrap_or_else(|| json!({ "id": id, "missing": true }))).collect();
    let sources: Vec<J> = s
        .sources
        .iter()
        .map(|n| match resolve_source(n) {
            Ok(_) => json!({ "name": n }),
            Err(_) => json!({ "name": n, "missing": true }),
        })
        .collect();
    json!({ "agents": agents, "gardens": gardens, "sources": sources })
}

/// For the Garden door's sources (M5): may this live Voice & Chat only
/// login read source `name`'s card view?
pub fn shared_source(login: &str, name: &str) -> bool {
    connect_users::voice_chat_shares(login).is_some_and(|s| s.sources.iter().any(|x| x == name))
}

// ── limits (security review H2) ────────────────────────────────────────

/// One post per this long per login.
pub const POST_EVERY: std::time::Duration = std::time::Duration::from_secs(10);
/// Posts per login per UTC day.
pub const POSTS_PER_DAY: u32 = 200;
/// Reads (`/cli/shared`, `/cli/shared/thread`) a login may burst.
pub const READ_BURST: f64 = 20.0;
/// Reads per second it refills (a 2–3 s poll and the list fit easily).
pub const READS_PER_SEC: f64 = 1.0;

/// One login's post budget.
#[derive(Debug, Clone, Default)]
pub struct PostBudget {
    last: Option<std::time::Instant>,
    day: i64,
    count: u32,
}

impl PostBudget {
    /// May one more post go now? `Err((code, retry after s))`. Nothing is
    /// spent: [`Self::spend`] does that once the post is accepted.
    pub fn check(&self, now: std::time::Instant, day: i64) -> Result<(), (&'static str, u64)> {
        if day == self.day && self.count >= POSTS_PER_DAY {
            return Err(("daily_limit", 3600));
        }
        if let Some(last) = self.last {
            let since = now.saturating_duration_since(last);
            if since < POST_EVERY {
                return Err(("rate_limited", (POST_EVERY - since).as_secs().max(1)));
            }
        }
        Ok(())
    }

    pub fn spend(&mut self, now: std::time::Instant, day: i64) {
        if day != self.day {
            self.day = day;
            self.count = 0;
        }
        self.count += 1;
        self.last = Some(now);
    }
}

/// One login's read bucket.
#[derive(Debug, Clone)]
pub struct ReadBucket {
    tokens: f64,
    at: std::time::Instant,
}

impl ReadBucket {
    pub fn new(now: std::time::Instant) -> Self {
        Self { tokens: READ_BURST, at: now }
    }

    /// Take one read; `Err(ms to wait)` when empty.
    pub fn take(&mut self, now: std::time::Instant) -> Result<(), u64> {
        let dt = now.saturating_duration_since(self.at).as_secs_f64();
        self.tokens = (self.tokens + dt * READS_PER_SEC).min(READ_BURST);
        self.at = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            Ok(())
        } else {
            Err((((1.0 - self.tokens) / READS_PER_SEC) * 1000.0).ceil() as u64)
        }
    }
}

/// How long a stored post holds its conversation busy at most while it
/// waits on the delivery lane (the hold normally ends when delivery starts
/// the agent's turn; this only frees a conversation whose job never ran).
pub const HOLD_MAX: std::time::Duration = std::time::Duration::from_secs(120);

/// Every login's post budget and every conversation's busy hold, behind
/// ONE lock (re-review M3): a post reserves its slot and its hold together
/// before anything is stored, so parallel posts can't all pass the checks.
#[derive(Default)]
struct PostState {
    budgets: HashMap<String, PostBudget>,
    /// Conversation id → when a stored post took it (cleared by
    /// [`turn_started`] once delivery opens the agent's turn).
    holds: HashMap<String, std::time::Instant>,
}

fn post_state() -> &'static std::sync::Mutex<PostState> {
    static M: std::sync::OnceLock<std::sync::Mutex<PostState>> = std::sync::OnceLock::new();
    M.get_or_init(Default::default)
}

/// A post's reserved slot and hold; [`release`] gives both back when the
/// post isn't stored.
struct Reservation {
    login: String,
    conversation: String,
    prior: Option<PostBudget>,
}

/// Check the budget, the hold and the agent's turn, and take the slot and
/// the hold, all under one lock.
fn reserve(login: &str, conversation: &str, vc: &str, now: std::time::Instant, day: i64) -> Result<Reservation, CliResponse> {
    let mut m = post_state().lock().unwrap_or_else(|p| p.into_inner());
    if let Err((code, secs)) = m.budgets.get(login).map_or(Ok(()), |b| b.check(now, day)) {
        return Err(limited(code, "too many posts from this login", secs * 1000));
    }
    let held = m.holds.get(conversation).is_some_and(|at| now.saturating_duration_since(*at) < HOLD_MAX);
    if held || activity_of(vc) != "idle" {
        return Err(limited("agent_busy", "the agent is still on your last message; send this when it answers", 5000));
    }
    if m.budgets.len() > 10_000 {
        m.budgets.clear();
    }
    if m.holds.len() > 10_000 {
        m.holds.retain(|_, at| now.saturating_duration_since(*at) < HOLD_MAX);
    }
    let prior = m.budgets.get(login).cloned();
    m.budgets.entry(login.to_string()).or_default().spend(now, day);
    m.holds.insert(conversation.to_string(), now);
    Ok(Reservation { login: login.to_string(), conversation: conversation.to_string(), prior })
}

/// The post wasn't stored: give its slot and hold back.
fn release(r: Reservation) {
    let mut m = post_state().lock().unwrap_or_else(|p| p.into_inner());
    match r.prior {
        Some(b) => {
            m.budgets.insert(r.login, b);
        }
        None => {
            m.budgets.remove(&r.login);
        }
    }
    m.holds.remove(&r.conversation);
}

/// Delivery opened the agent's turn on `conversation_id` (or, in tests,
/// reached the test sink): from here the turn itself keeps it busy.
pub(crate) fn turn_started(conversation_id: &str) {
    if !conversation_id.starts_with(crate::overlay_routes::VC_PREFIX) {
        return;
    }
    post_state().lock().unwrap_or_else(|p| p.into_inner()).holds.remove(conversation_id);
}

fn read_buckets() -> &'static std::sync::Mutex<HashMap<String, ReadBucket>> {
    static M: std::sync::OnceLock<std::sync::Mutex<HashMap<String, ReadBucket>>> = std::sync::OnceLock::new();
    M.get_or_init(Default::default)
}

fn utc_day() -> i64 {
    chrono::Utc::now().timestamp().div_euclid(86_400)
}

fn limited(code: &str, hint: &str, retry_ms: u64) -> CliResponse {
    CliResponse {
        status: "429 Too Many Requests",
        content_type: "application/json",
        body: json!({ "ok": false, "error": { "code": code, "hint": hint, "retryAfterMs": retry_ms } }).to_string(),
    }
}

fn take_read(login: &str) -> Result<(), CliResponse> {
    let now = std::time::Instant::now();
    let mut m = read_buckets().lock().unwrap_or_else(|p| p.into_inner());
    if m.len() > 10_000 {
        m.clear();
    }
    m.entry(login.to_string())
        .or_insert_with(|| ReadBucket::new(now))
        .take(now)
        .map_err(|ms| limited("rate_limited", "too many reads from this login; slow the poll down", ms))
}

/// Tests: forget every login's budgets.
#[cfg(test)]
pub(crate) fn forget_limits() {
    *post_state().lock().unwrap_or_else(|p| p.into_inner()) = PostState::default();
    read_buckets().lock().unwrap_or_else(|p| p.into_inner()).clear();
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
    if path != THREAD_POST {
        if let Err(r) = take_read(&login) {
            return r;
        }
    }
    match path {
        LIST => list(&login, &shares),
        THREAD => thread(&login, &shares, params),
        THREAD_POST => post(&login, &shares, body),
        _ => refuse("404 Not Found", "not_found", format!("no route {path}")),
    }
}

/// `GET /cli/shared`.
fn list(login: &str, shares: &Shares) -> CliResponse {
    let agents: Vec<J> = shares
        .agents
        .iter()
        .filter_map(|id| agent_view(id))
        .map(|mut a| {
            let vc = crate::overlay_routes::vc_addr(a["address"].as_str().unwrap_or_default(), login);
            a["activity"] = json!(activity_of(&vc));
            // The login's own conversation with this agent: the phone keys
            // its cache on (server, login, conversationId).
            a["conversationId"] = json!(crate::overlay_routes::vc_conversation(a["id"].as_str().unwrap_or_default(), login));
            a["conversationAddress"] = json!(vc);
            a
        })
        .collect();
    let gardens: Vec<J> = shares.gardens.iter().filter_map(|id| garden_view(id)).collect();
    CliResponse::ok_json(json!({ "ok": true, "role": "voice_chat", "agents": agents, "gardens": gardens }).to_string())
}

/// The read params `GET /cli/thread` takes that a shared read passes on.
const THREAD_READ_PARAMS: &[&str] = &["limit", "after_seq", "since_seq", "before_seq"];

/// `GET /cli/shared/thread?agent=`: `GET /cli/thread`'s answer for THIS
/// login's own conversation with the shared agent (`vc:<agent>:<login>`,
/// security review H1): its own posts and the agent's replies to it, never
/// the agent's Thread or anyone else's conversation.
fn thread(login: &str, shares: &Shares, params: &HashMap<String, String>) -> CliResponse {
    let agent = params.get("agent").map(String::as_str).unwrap_or("").trim().to_string();
    if agent.is_empty() {
        return refuse("400 Bad Request", "usage", "agent is required (an address from GET /cli/shared)");
    }
    let Some((_, address)) = shared_agent(shares, &agent) else {
        return not_shared(&agent);
    };
    let vc = crate::overlay_routes::vc_addr(&address, login);
    let mut inner: HashMap<String, String> = HashMap::new();
    inner.insert("addr".into(), vc.clone());
    for k in THREAD_READ_PARAMS {
        if let Some(v) = params.get(*k) {
            inner.insert((*k).to_string(), v.clone());
        }
    }
    let r = crate::overlay_routes::dispatch("/cli/thread", &inner).unwrap_or_else(|| refuse("404 Not Found", "not_found", "no Thread"));
    let conversation = shared_agent(shares, &agent).map(|(id, _)| crate::overlay_routes::vc_conversation(&id, login)).unwrap_or_default();
    reshape(
        r,
        &[
            ("activity", json!(activity_of(&vc))),
            ("conversationId", json!(conversation)),
            ("conversationAddress", json!(vc)),
        ],
    )
}

/// The desktop's reply marker (`THREAD_REPLY_MARK`).
pub const REPLY_MARK: char = '↩';

/// How much of the replied-to message a reply quotes (the desktop's
/// `THREAD_REPLY_QUOTE_MAX`).
pub const REPLY_QUOTE_MAX: usize = 600;

/// The stored message at `seq` in `address`'s Thread: `(from, body)`.
fn message_at(address: &str, seq: i64) -> Option<(String, String)> {
    let mut q: HashMap<String, String> = HashMap::new();
    q.insert("addr".into(), address.to_string());
    q.insert("before_seq".into(), (seq + 1).to_string());
    q.insert("limit".into(), "1".into());
    let r = crate::overlay_routes::dispatch("/cli/thread", &q)?;
    if !r.status.starts_with('2') {
        return None;
    }
    let v: J = serde_json::from_str(&r.body).ok()?;
    let item = v["items"].as_array()?.iter().find(|i| i["seq"].as_i64() == Some(seq))?.clone();
    let doc = &item["doc"];
    let from = doc["from"].as_str().unwrap_or_default().to_string();
    let body = doc["body"].as_str().unwrap_or_default().to_string();
    Some((from, body))
}

/// The desktop's reply text (`src/renderer/lib/thread-reply.ts`), built
/// here from the stored message: `↩ #N <from>:`, the quote as `> ` lines,
/// a blank line, then the person's words. Every line of the person's words
/// is indented two spaces, so none of them starts a line that could read as
/// a person's or K2's own (the same reason a plain post quotes its later
/// lines); the header and the quote are K2's.
pub fn reply_text(seq: i64, from: &str, quoted: &str, words: &str) -> String {
    let from: String = from
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .filter(|c| !matches!(c, '[' | ']' | ':'))
        .collect();
    let from = from.split_whitespace().collect::<Vec<_>>().join(" ");
    let from: String = from.chars().take(64).collect();
    let from = if from.is_empty() { "someone".to_string() } else { from };
    let unified = one_break(quoted);
    let q = unified.trim_matches(|c: char| c.is_whitespace());
    let q: String = if q.chars().count() > REPLY_QUOTE_MAX {
        format!("{}…", q.chars().take(REPLY_QUOTE_MAX).collect::<String>().trim_end())
    } else if q.is_empty() {
        "…".to_string()
    } else {
        q.to_string()
    };
    let quote = q.split('\n').map(|l| if l.is_empty() { ">".to_string() } else { format!("> {l}") }).collect::<Vec<_>>().join("\n");
    let w = one_break(words.trim())
        .split('\n')
        .map(|l| if l.is_empty() { String::new() } else { format!("  {l}") })
        .collect::<Vec<_>>()
        .join("\n");
    format!("↩ #{seq} {from}:\n{quote}\n\n{w}")
}

/// Every kind of line break as `\n` (the same set `quote_guest_lines`
/// counts), so no break hides inside a line.
fn one_break(s: &str) -> String {
    s.replace("\r\n", "\n").replace(['\r', '\u{0b}', '\u{0c}', '\u{85}', '\u{2028}', '\u{2029}'], "\n")
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
    /// A reply: the replied-to message's seq in the same Thread. K2 renders
    /// the reply header and quote from the stored message.
    #[serde(default, rename = "replyTo")]
    reply_to: Option<i64>,
}

/// Longest post a shared Thread takes (H2).
pub const MAX_TEXT_CHARS: usize = 2000;

/// `POST /cli/shared/thread/post {agent, text, clientMsgId?, via?}`.
fn post(login: &str, shares: &Shares, body: &[u8]) -> CliResponse {
    let b: PostBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return refuse("400 Bad Request", "usage", format!("shared/thread/post takes {{agent, text, clientMsgId?}}: {e}")),
    };
    if let Some(v) = b.via.as_deref().filter(|v| *v != "compose") {
        return refuse("403 Forbidden", "compose_only", format!("a shared Thread takes only your own posts (via=compose), not via={}", v.chars().take(16).collect::<String>()));
    }
    let Some((agent_id, address)) = shared_agent(shares, &b.agent) else {
        audit("shared.thread.post", login, "refused not_shared".into());
        return not_shared(&b.agent);
    };
    let vc = crate::overlay_routes::vc_addr(&address, login);
    let conversation = crate::overlay_routes::vc_conversation(&agent_id, login);
    // M4: no control characters (but `\n`) or bidi overrides, ever.
    let cleaned = crate::overlay_routes::strip_untrusted_controls(&b.text);
    let text = cleaned.trim();
    if text.is_empty() {
        return refuse("400 Bad Request", "usage", "text is required");
    }
    if text.chars().count() > MAX_TEXT_CHARS {
        return refuse("400 Bad Request", "too_large", format!("a post is at most {MAX_TEXT_CHARS} characters"));
    }
    // A reply header is K2's to write (from `replyTo`), never the client's.
    if text.starts_with(REPLY_MARK) {
        return refuse("400 Bad Request", "reply_header_not_allowed", "send replyTo (the message's seq) instead of a reply header; K2 writes the header");
    }
    // H1: a reply quotes only this person's own conversation.
    let text = match b.reply_to {
        None => text.to_string(),
        Some(seq) if seq > 0 => match message_at(&vc, seq) {
            Some((from, body)) => reply_text(seq, &from, &body, text),
            None => return refuse("404 Not Found", "reply_not_found", format!("no message #{seq} in this Thread")),
        },
        Some(_) => return refuse("400 Bad Request", "usage", "replyTo is a message's seq (a whole number above 0)"),
    };
    // H2 / M3: one post per 10 s and 200 a day per login, and none while
    // the agent is still on this person's last message. The slot and the
    // conversation's hold are taken together under one lock BEFORE the
    // post is stored, and given back if it isn't.
    let (now, day) = (std::time::Instant::now(), utc_day());
    let reservation = match reserve(login, &conversation, &vc, now, day) {
        Ok(r) => r,
        Err(r) => return r,
    };
    let r = crate::overlay_routes::post_voice_chat(login, &vc, &text, b.client_msg_id.as_deref());
    if !r.status.starts_with('2') {
        release(reservation);
    }
    audit("shared.thread.post", login, format!("addr={vc} status={}", r.status.split(' ').next().unwrap_or("")));
    reshape(r, &[])
}

#[cfg(test)]
#[path = "shared_routes_tests.rs"]
mod tests;
