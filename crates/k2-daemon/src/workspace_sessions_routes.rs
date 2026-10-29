//! `k2 sessions` routes (prd-workspace-sessions-cli-v1 §5).
//!
//! - `GET  /cli/sessions/roster` — who is working in this workspace
//!   ([`crate::workspace_roster`], the one shared list).
//! - `GET  /cli/sessions/past`   — past conversations here, newest first
//!   (`k2_core::past_conversations`).
//! - `GET  /cli/sessions/show`   — what one was about and its last messages.
//! - `POST /cli/sessions/open`   — reopen a past conversation as a sidecar
//!   (same history); a conversation something already holds answers its
//!   address instead (one runner, WS-O4).
//! - [`msg_session`]: `POST /cli/workspace/msg` with `session=` (`k2 msg
//!   --session <id>`): deliver to whoever holds it, or reopen a past-only
//!   one with the message as its first turn (WS-M2).
//!
//! All four are agent verbs served on both doors (TCP and the per-cell
//! socket). Callers are classified like `k2 sidecar`
//! ([`crate::sidecar_routes::Caller`]); agents see their own workspace
//! only. Reads need no switch (the same Unix user can read these
//! transcripts on disk); a sandboxed session gets the roster only.
//! Opening follows the sidecar gate: "Allow hiring and managing agents",
//! own workspace, the main session only, not sandboxed, no more power.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use k2_core::log_debug;
use k2_core::past_conversations::{self as past, IdMatch, PastQuery, PastRow};
use k2_core::sidecar::{self as core, SidecarError};
use serde_json::{json, Value};

use crate::cli_response::CliResponse;
use crate::sidecar_routes::{self as sc, err, Caller, Workspace};
use crate::workspace_roster::{self, RosterOpts};

pub const ROSTER: &str = "/cli/sessions/roster";
pub const PAST: &str = "/cli/sessions/past";
pub const SHOW: &str = "/cli/sessions/show";
pub const OPEN: &str = "/cli/sessions/open";

/// The outside-K2 guard (Q5): a transcript written this recently may be
/// open in a terminal outside K2.
pub const RECENT_SECS: i64 = 120;

pub fn is_route(path: &str) -> bool {
    matches!(path, ROSTER | PAST | SHOW | OPEN)
}

pub fn is_post_route(path: &str) -> bool {
    path == OPEN
}

pub fn body_cap(path: &str) -> usize {
    if path == OPEN {
        core::NEW_BODY_MAX_BYTES
    } else {
        core::SMALL_BODY_MAX_BYTES
    }
}

pub fn too_large(path: &str) -> CliResponse {
    sc::refusal(&err(
        "body_too_large",
        413,
        format!("request bodies here are at most {} KiB", body_cap(path) / 1024),
    ))
}

/// Route one request. Blocking: call from `spawn_blocking`.
pub fn handle(
    path: &str,
    caller: Caller,
    body: &[u8],
    params: &HashMap<String, String>,
    ingress: &str,
) -> CliResponse {
    let result = match path {
        ROSTER => handle_roster(&caller, params),
        PAST => handle_past(&caller, params),
        SHOW => handle_show(&caller, params),
        OPEN => {
            let out = sc::json_body(body).and_then(|v| {
                handle_open(&caller, &v, OpenExtra::default()).map(|o| o.body)
            });
            let outcome = match &out {
                Ok(v) => format!(
                    "ok {} {}",
                    v.get("address").and_then(Value::as_str).unwrap_or("-"),
                    v.get("outcome").and_then(Value::as_str).unwrap_or("reopened")
                ),
                Err(e) => format!("refused:{}", e.code),
            };
            sc::audit("sessions-open", &caller, &outcome, ingress);
            out
        }
        _ => return CliResponse::not_found(),
    };
    match result {
        Ok(v) => CliResponse::ok_json(v.to_string()),
        Err(e) => refusal_with_candidates(&e),
    }
}

/// `SidecarError` as JSON; `id_ambiguous` carries its candidates in the hint.
fn refusal_with_candidates(e: &SidecarError) -> CliResponse {
    sc::refusal(e)
}

fn param<'a>(params: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    params.get(key).map(String::as_str).map(str::trim).filter(|s| !s.is_empty())
}

fn flag(params: &HashMap<String, String>, key: &str) -> bool {
    matches!(param(params, key), Some("1" | "true" | "yes"))
}

fn deny_sandboxed(caller: &Caller) -> Result<(), SidecarError> {
    if let Caller::Agent(a) = caller {
        if a.sandboxed {
            return Err(err(
                "sandboxed",
                403,
                "a sandboxed session cannot read host transcripts (k2 sessions still lists who is here)",
            ));
        }
    }
    Ok(())
}

fn caller_map_key(caller: &Caller) -> Option<String> {
    match caller {
        Caller::Agent(a) => a.map_key.clone(),
        _ => None,
    }
}

// ── roster ────────────────────────────────────────────────────────────

fn handle_roster(caller: &Caller, params: &HashMap<String, String>) -> Result<Value, SidecarError> {
    sc::require_credential(caller)?;
    let ws = sc::target_workspace(caller, param(params, "workspace"))?;
    let all = flag(params, "all");
    let include_api = all && matches!(caller, Caller::Owner | Caller::Login { .. });
    let roster = workspace_roster::workspace_roster(
        &ws.id,
        &RosterOpts { all, include_api, caller_map_key: caller_map_key(caller) },
    )
    .map_err(|e| err("internal", 500, e))?;
    Ok(roster.to_json())
}

// ── past ──────────────────────────────────────────────────────────────

fn project_paths() -> Result<Vec<String>, SidecarError> {
    k2_core::projects_ops::projects_list()
        .map(|rows| rows.into_iter().map(|p| p.path).collect())
        .map_err(|e| err("internal", 500, format!("projects_list: {e}")))
}

/// `cursor-agent` and `cursor` are one CLI here.
fn normalize_cli(cli: &str) -> Result<String, SidecarError> {
    let c = cli.trim().to_ascii_lowercase();
    let c = if c == "cursor-agent" { "cursor".to_string() } else { c };
    if past::CLIS.contains(&c.as_str()) {
        Ok(c)
    } else {
        Err(err(
            "bad_request",
            400,
            format!("unknown cli '{cli}' (claude, codex, grok, gemini, cursor, pi, hermes)"),
        ))
    }
}

fn past_query(ws: &Workspace, params: &HashMap<String, String>) -> Result<PastQuery, SidecarError> {
    let days = match param(params, "days") {
        Some(d) => d
            .parse::<u32>()
            .ok()
            .filter(|d| (1..=365).contains(d))
            .ok_or_else(|| err("bad_request", 400, "days is a number from 1 to 365"))?,
        None => 14,
    };
    let limit = match param(params, "limit") {
        Some(l) => l
            .parse::<usize>()
            .ok()
            .filter(|l| (1..=200).contains(l))
            .ok_or_else(|| err("bad_request", 400, "limit is a number from 1 to 200"))?,
        None => 20,
    };
    let cli = param(params, "cli").map(normalize_cli).transpose()?;
    Ok(PastQuery {
        root: ws.path.clone(),
        project_paths: project_paths()?,
        days,
        cli,
        limit,
        search: param(params, "search").map(str::to_string),
        include_archived: flag(params, "archived"),
        budget: past::Budget::default(),
        now: chrono::Utc::now().timestamp(),
    })
}

/// Who holds a conversation, in the past list's words.
fn runner_for(ws: &Workspace, row: &PastRow) -> (Option<Value>, &'static str) {
    let ids = holder_ids(row);
    let owner = ids
        .iter()
        .find_map(|id| crate::conversation_owner::conversation_owner(id, Some(&ws.id), None));
    let Some(owner) = owner else {
        return (None, "outside");
    };
    let state = if owner.live { "live" } else { "asleep" };
    match owner.role {
        "main" => (Some(json!({ "address": ws.handle, "state": state })), "main"),
        "heartbeat" => (Some(json!({ "address": Value::Null, "state": state })), "heartbeat"),
        _ => {
            let (handle, created_by) = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                let handle = ids.iter().find_map(|id| core::handle_read_only(&conn, &ws.id, id));
                let created = owner
                    .pane_group_id
                    .as_deref()
                    .and_then(|pg| core::meta(&conn, &ws.id, pg))
                    .and_then(|m| m.created_by);
                (handle, created)
            };
            let address = handle.map(|h| k2_core::workspace_session_handles::format_address(&ws.handle, Some(&h)));
            (
                Some(json!({ "address": address, "state": state })),
                if created_by.is_some() { "sidecar" } else { "chat" },
            )
        }
    }
}

/// The ids K2 may hold a conversation under: its id, and for pi its
/// session file (sidecar rows store the file, WS-A6).
fn holder_ids(row: &PastRow) -> Vec<String> {
    let mut ids = vec![row.conversation_id.clone()];
    if row.cli == "pi" {
        if let Some(p) = k2_core::chat_continue::pi_session_file(&row.conversation_id, &row.cwd) {
            ids.push(p.to_string_lossy().into_owned());
        }
    }
    ids
}

fn row_json(
    ws: &Workspace,
    row: &PastRow,
    short: &str,
    branches: &mut HashMap<String, Option<String>>,
) -> Value {
    let (runner, origin) = runner_for(ws, row);
    let branch = branches
        .entry(row.cwd.clone())
        .or_insert_with(|| past::branch_of(&row.cwd))
        .clone();
    json!({
        "id": short,
        "conversationId": row.conversation_id,
        "cli": row.cli,
        "title": row.title,
        "customName": row.custom_name,
        "lastActiveAt": row.last_active_at,
        "messages": row.messages,
        "cwd": row.cwd,
        "worktree": row.worktree,
        "branch": branch,
        "origin": origin,
        "runner": runner,
        "archived": row.archived,
        "match": row.excerpt,
    })
}

fn handle_past(caller: &Caller, params: &HashMap<String, String>) -> Result<Value, SidecarError> {
    sc::require_credential(caller)?;
    let ws = sc::target_workspace(caller, param(params, "workspace"))?;
    deny_sandboxed(caller)?;
    let q = past_query(&ws, params)?;
    let result = past::list_past(&q);
    let ids: Vec<String> = result.rows.iter().map(|r| r.conversation_id.clone()).collect();
    let shorts = past::short_ids(&ids);
    let mut branches = HashMap::new();
    let rows: Vec<Value> = result
        .rows
        .iter()
        .map(|r| row_json(&ws, r, shorts.get(&r.conversation_id).map(String::as_str).unwrap_or(&r.conversation_id), &mut branches))
        .collect();
    Ok(json!({
        "ok": true,
        "workspace": ws.handle,
        "days": q.days,
        "search": q.search,
        "cli": q.cli,
        "truncated": result.truncated,
        "scanned": { "files": result.scanned_files, "bytes": result.scanned_bytes },
        "unsupported": result.unsupported.iter().map(|(c, w)| json!({ "cli": c, "why": w })).collect::<Vec<_>>(),
        "conversations": rows,
    }))
}

// ── resolving an <id|address> ─────────────────────────────────────────

/// What an `<id>` argument names.
#[derive(Debug, Clone)]
struct Resolved {
    row: PastRow,
}

fn not_found(token: &str) -> SidecarError {
    err(
        "conversation_not_found",
        404,
        format!("no conversation '{token}' in this workspace; find it with: k2 sessions --past --search \"<words>\" (or --days 365)"),
    )
}

/// A K2-held conversation K2 knows by an address (`k2`, `k2/3`, `k2/reviewer`).
fn by_address(ws: &Workspace, token: &str) -> Option<(String, Option<String>)> {
    let roster = workspace_roster::workspace_roster(&ws.id, &RosterOpts { all: true, ..Default::default() }).ok()?;
    let want = token.trim();
    roster
        .rows
        .into_iter()
        .find(|r| {
            r.address.as_deref() == Some(want)
                || r.handle.as_deref().is_some_and(|h| {
                    k2_core::workspace_session_handles::format_address(&ws.handle, Some(h)) == want
                })
        })
        .and_then(|r| r.conversation_id.map(|c| (c, r.cli)))
}

/// Resolve `<id|address>` (WS-A5) against this workspace's past rows
/// (14 days, then a year) and K2's own rows.
fn resolve(ws: &Workspace, token: &str) -> Result<Resolved, SidecarError> {
    let token = token.trim();
    if token.is_empty() {
        return Err(err("bad_request", 400, "name a conversation: an id from k2 sessions --past, or an address"));
    }
    let paths = project_paths()?;
    let looks_like_address = token == ws.handle || token.contains('/');
    let (wanted, cli_hint) = if looks_like_address {
        match by_address(ws, token) {
            Some((cid, cli)) => (cid, cli),
            None => return Err(not_found(token)),
        }
    } else {
        (token.to_string(), None)
    };
    let mut last_err = None;
    for days in [14u32, 365] {
        let q = PastQuery {
            root: ws.path.clone(),
            project_paths: paths.clone(),
            days,
            cli: None,
            limit: 200,
            search: None,
            include_archived: true,
            budget: past::Budget::default(),
            now: chrono::Utc::now().timestamp(),
        };
        let rows = past::list_past(&q).rows;
        // K2's own rows count too (a held conversation whose transcript
        // is older than the window).
        let mut ids: Vec<String> = rows.iter().map(|r| r.conversation_id.clone()).collect();
        let held = workspace_roster::workspace_roster(&ws.id, &RosterOpts { all: true, ..Default::default() })
            .map(|r| r.rows)
            .unwrap_or_default();
        for r in &held {
            if let Some(c) = &r.conversation_id {
                ids.push(c.clone());
            }
        }
        let uniq: HashSet<String> = ids.into_iter().collect();
        match past::match_id(&wanted, uniq.iter()) {
            IdMatch::One(id) => {
                if let Some(row) = rows.iter().find(|r| r.conversation_id == id) {
                    return Ok(Resolved { row: row.clone() });
                }
                // Held by K2 but not in the transcript lists.
                let held_row = held.iter().find(|r| r.conversation_id.as_deref() == Some(id.as_str()));
                let cli = held_row.and_then(|r| r.cli.clone()).or(cli_hint.clone()).unwrap_or_default();
                return Ok(Resolved {
                    row: PastRow {
                        conversation_id: id,
                        cli,
                        title: held_row.and_then(|r| r.title.clone()).unwrap_or_default(),
                        custom_name: None,
                        last_active_at: held_row.and_then(|r| r.last_active_at).unwrap_or(0),
                        messages: 0,
                        cwd: ws.path.clone(),
                        worktree: None,
                        archived: false,
                        excerpt: None,
                    },
                });
            }
            IdMatch::Ambiguous(c) => {
                return Err(err(
                    "id_ambiguous",
                    409,
                    format!("'{wanted}' matches more than one conversation: {}; give more characters", c.join(", ")),
                ))
            }
            IdMatch::TooShort => {
                return Err(err("id_too_short", 400, "give at least 6 characters of the id (k2 sessions --past shows them)"))
            }
            IdMatch::None => last_err = Some(not_found(token)),
        }
    }
    Err(last_err.unwrap_or_else(|| not_found(token)))
}

// ── show ──────────────────────────────────────────────────────────────

fn handle_show(caller: &Caller, params: &HashMap<String, String>) -> Result<Value, SidecarError> {
    sc::require_credential(caller)?;
    let ws = sc::target_workspace(caller, param(params, "workspace"))?;
    deny_sandboxed(caller)?;
    let id = param(params, "id").ok_or_else(|| err("bad_request", 400, "pass id=<id|address>"))?;
    let tail = match param(params, "tail") {
        Some(t) => t
            .parse::<usize>()
            .ok()
            .filter(|t| (1..=40).contains(t))
            .ok_or_else(|| err("bad_request", 400, "tail is a number from 1 to 40"))?,
        None => 6,
    };
    let resolved = resolve(&ws, id)?;
    let row = resolved.row;
    let short = past::short_ids(std::slice::from_ref(&row.conversation_id))
        .remove(&row.conversation_id)
        .unwrap_or_else(|| row.conversation_id.clone());
    let mut branches = HashMap::new();
    let conversation = row_json(&ws, &row, &short, &mut branches);
    match k2_core::chat_continue::read_transcript_messages(&row.cli, &row.conversation_id, &row.cwd) {
        Ok(d) => {
            let first_ask = d
                .messages
                .iter()
                .find(|m| m.role == "user")
                .map(|m| k2_core::chat_continue::cap_message(&m.text));
            let total = d.messages.len();
            let start = total.saturating_sub(tail);
            let messages: Vec<Value> = d.messages[start..]
                .iter()
                .map(|m| json!({ "role": m.role, "text": k2_core::chat_continue::cap_message(&m.text), "at": m.at }))
                .collect();
            Ok(json!({
                "ok": true,
                "conversation": conversation,
                "firstAsk": first_ask,
                "messageCount": total,
                "toolCalls": d.tool_calls,
                "messages": messages,
                "unsupported": Value::Null,
            }))
        }
        Err(e) if e == k2_core::chat_continue::CURSOR_NO_TEXT => Ok(json!({
            "ok": true,
            "conversation": conversation,
            "firstAsk": Value::Null,
            "messageCount": Value::Null,
            "toolCalls": Value::Null,
            "messages": Value::Null,
            "unsupported": e,
        })),
        Err(e) => Err(err("transcript_unreadable", 404, format!("{e} for {short} ({})", row.cli))),
    }
}

// ── open ──────────────────────────────────────────────────────────────

/// What `k2 msg --session` adds to an open.
#[derive(Debug, Clone, Default)]
pub struct OpenExtra {
    /// The first turn (the message), already framed.
    pub launch_message: Option<String>,
}

/// An open's answer.
pub struct Opened {
    pub body: Value,
}

/// The write gate (WS-X3): the sidecar agent gate with this verb's words.
fn open_gate(caller: &Caller, ws: &Workspace, id: &str) -> Result<(), SidecarError> {
    let Caller::Agent(a) = caller else {
        return Ok(());
    };
    if !k2_core::workspace::settings::agents_can_manage_agents(&a.project_id) {
        return Err(err("gated", 403, sc::GATED_HINT.replace("{ws}", &ws.handle)));
    }
    if a.project_id != ws.id {
        return Err(err("forbidden", 403, "an agent may act only on its own workspace"));
    }
    if a.map_key.as_deref() != Some(a.project_id.as_str()) {
        return Err(err(
            "main_only",
            403,
            format!(
                "only the main session may open sessions; ask it: k2 msg {} \"please open {id}\"",
                ws.handle
            ),
        ));
    }
    if a.sandboxed {
        return Err(err("forbidden", 403, "a sandboxed session may not open host sessions"));
    }
    Ok(())
}

/// The preset for a reopen: `harness` if given (same CLI only); else the
/// first enabled preset of that CLI without skip-approval flags; else the
/// CLI's first preset.
fn preset_for(cli: &str, harness: Option<&str>) -> Result<core::HarnessPreset, SidecarError> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    if let Some(h) = harness {
        let p = core::resolve_preset(&conn, h)?;
        if p.provider != cli {
            return Err(err(
                "cli_mismatch",
                409,
                format!("this conversation is a {cli} chat; preset '{}' runs {}", p.label, p.provider),
            ));
        }
        return Ok(p);
    }
    let presets = k2_core::db::schema::AgentPreset::list(&conn).unwrap_or_default();
    for p in presets.iter().filter(|p| p.enabled == 1) {
        let Ok(hp) = core::resolve_preset(&conn, &p.id) else { continue };
        if hp.provider == cli && !core::carries_skip_approvals(&hp.args, &hp.danger_flags) {
            return Ok(hp);
        }
    }
    core::resolve_preset(&conn, cli).map_err(|e| {
        if e.code == "harness_unknown" {
            err("harness_not_installed", 409, format!("no enabled {cli} preset (k2 preset list)"))
        } else {
            e
        }
    })
}

/// Rule 5: an agent may not open a session with more power than itself.
fn no_more_power(caller: &Caller, args: &[String], label: &str) -> Result<(), SidecarError> {
    let Caller::Agent(a) = caller else {
        return Ok(());
    };
    let all_danger: Vec<String> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::db::schema::AgentPreset::list(&conn)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|p| p.danger_flags)
            .filter_map(|raw| serde_json::from_str::<Vec<String>>(&raw).ok())
            .flatten()
            .collect()
    };
    if core::carries_skip_approvals(args, &all_danger) && !core::carries_skip_approvals(&a.args, &all_danger) {
        return Err(err(
            "forbidden",
            403,
            format!("'{label}' skips approvals and your session does not; an agent may not open a session with more power than itself"),
        ));
    }
    Ok(())
}

fn already(ws: &Workspace, address: Option<String>, role: &str, live: bool, cid: &str, cli: &str) -> Value {
    let address = address.unwrap_or_else(|| ws.handle.clone());
    let mut v = json!({
        "ok": true,
        "address": address,
        "role": role,
        "alreadyLive": live,
        "opened": false,
        "cli": cli,
        "conversationId": cid,
        "outcome": if live { "already_live" } else { "asleep_main" },
    });
    if role == "main" && !live {
        v["wakeWith"] = json!(format!("k2 msg {} \"…\"", ws.handle));
    }
    v
}

/// The open flow (§5.1). `extra.launch_message` is `k2 msg --session`'s
/// message (the first turn of a revived conversation).
pub fn handle_open(caller: &Caller, v: &Value, extra: OpenExtra) -> Result<Opened, SidecarError> {
    sc::require_credential(caller)?;
    let ws = sc::target_workspace(caller, sc::str_field(v, "workspace"))?;
    let token = sc::str_field(v, "id").unwrap_or("").trim().to_string();
    open_gate(caller, &ws, &token)?;
    let name = sc::str_field(v, "name").map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let harness = sc::str_field(v, "harness").map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let model = sc::str_field(v, "model").map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let force = v.get("force").and_then(Value::as_bool).unwrap_or(false);
    let brief: Option<String> = match v.get("brief") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(_) => return Err(err("brief_not_text", 400, "brief must be a string")),
    };
    if let Some(b) = brief.as_deref() {
        core::check_brief(b)?;
    }
    let brief = brief.filter(|b| !b.trim().is_empty());

    let lock = sc::workspace_lock(&ws.id);
    let _guard = lock.lock();

    let row = resolve(&ws, &token)?.row;
    let cid = row.conversation_id.clone();
    let cli = row.cli.clone();
    if cli.is_empty() || core::SUPPORTED_PROVIDERS.iter().all(|p| *p != cli) {
        return Err(err("harness_not_supported", 409, format!("conversation {cid} is not one of the seven CLIs K2 resumes")));
    }

    // WS-O4: one runner.
    let ids = holder_ids(&row);
    let owner = ids
        .iter()
        .find_map(|id| crate::conversation_owner::conversation_owner(id, Some(&ws.id), None));
    let mut asleep_row: Option<k2_core::db::schema::WorkspaceTabSession> = None;
    if let Some(o) = &owner {
        match (o.role, o.live) {
            ("main", live) => return Ok(Opened { body: already(&ws, Some(ws.handle.clone()), "main", live, &cid, &cli) }),
            (_, true) => {
                let (runner, _) = runner_for(&ws, &row);
                let address = runner.and_then(|r| r.get("address").and_then(Value::as_str).map(str::to_string));
                let role = if o.role == "heartbeat" { "heartbeat" } else { "sidecar" };
                return Ok(Opened { body: already(&ws, address, role, true, &cid, &cli) });
            }
            (_, false) => {
                if let Some(pg) = &o.pane_group_id {
                    let db = k2_core::db::shared();
                    let conn = db.lock();
                    asleep_row = k2_core::db::schema::WorkspaceTabSession::get(&conn, &ws.id, pg).ok().flatten();
                }
            }
        }
    }

    // WS-O5: written moments ago, maybe outside K2.
    let now = chrono::Utc::now().timestamp();
    let age = now - row.last_active_at;
    if !force && row.last_active_at > 0 && age < RECENT_SECS {
        return Err(err(
            "conversation_recently_active",
            409,
            format!(
                "this conversation was written {age} s ago, maybe in a terminal outside K2; opening it here would run it twice. Pass --force if it is closed."
            ),
        ));
    }

    // WS-O6: never resume in another folder.
    let cwd = row.cwd.clone();
    if !std::path::Path::new(&cwd).is_dir() {
        let code = if cwd.contains("/.worktrees/") { "worktree_gone" } else { "folder_gone" };
        return Err(err(
            code,
            410,
            format!("its folder {cwd} is gone; Chat history can continue it in a new chat"),
        ));
    }

    // WS-O7: the name.
    let conversation_key = asleep_row
        .as_ref()
        .and_then(|r| r.session_id.clone())
        .unwrap_or_else(|| ids.last().cloned().unwrap_or_else(|| cid.clone()));
    let current_custom = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::workspace_session_handles::custom_name_for_session_id(&conn, &conversation_key)
            .ok()
            .flatten()
            .or_else(|| {
                k2_core::workspace_session_handles::custom_name_for_session_id(&conn, &cid).ok().flatten()
            })
            .filter(|n| !n.trim().is_empty())
    };
    let preset = match &asleep_row {
        Some(r) if harness.is_none() => {
            // Resume that row: its own command and saved args.
            let mut p = preset_for(&cli, None)?;
            if let Some(cmd) = r.command.clone() {
                p.program = cmd;
            }
            if let Some(saved) = r.args_json.as_deref().and_then(|s| serde_json::from_str::<Vec<String>>(s).ok()) {
                p.args = saved;
            }
            p
        }
        _ => preset_for(&cli, harness.as_deref())?,
    };
    no_more_power(caller, &preset.args, &preset.label)?;

    let slug: Option<String> = match (&current_custom, &name) {
        (Some(cur), Some(n)) => {
            let cur_slug = k2_core::workspace_session_handles::slugify_custom_name(cur).unwrap_or_default();
            let want = core::validate_name(n, &ws.handle)?;
            if cur_slug != want {
                return Err(err(
                    "name_differs",
                    409,
                    format!("it is already {}/{cur_slug}; rename it in Chats or pass that name", ws.handle),
                ));
            }
            Some(cur_slug)
        }
        (Some(cur), None) => k2_core::workspace_session_handles::slugify_custom_name(cur).ok(),
        (None, Some(n)) => {
            let want = core::validate_name(n, &ws.handle)?;
            let taken = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                let hit = k2_core::workspace_session_handles::resolve_handle(&conn, &ws.id, &want).ok();
                let reserved = k2_core::workspace_session_handles::alias_owner(&conn, &ws.id, &want).ok().flatten();
                hit.or(reserved)
            };
            if let Some(k) = taken {
                if k != conversation_key && k != cid {
                    return Err(err("name_taken", 409, format!("'{want}' is already used in this workspace; pick another name")));
                }
            }
            Some(want)
        }
        (None, None) => None,
    };

    // WS-O8: the cap.
    let cap = core::cap();
    if sc::live_sidecar_count(&ws) >= cap {
        return Err(err(
            "sidecar_cap",
            409,
            format!("{} already has {cap} live sidecars; stop one first (k2 sessions stop <name>)", ws.handle),
        ));
    }
    // WS-O10: installed.
    if core::resolve_binary(&preset.program).is_none() {
        return Err(err("harness_not_installed", 409, core::install_hint(&preset.program)));
    }

    // The handle (and address) before the spawn: a name, else the
    // durable ordinal (never recycled; the register finds it).
    let handle = match &slug {
        Some(s) => {
            if current_custom.is_none() {
                let db = k2_core::db::shared();
                let conn = db.lock();
                core::bind_name(&conn, &ws.id, preset.provider, &conversation_key, s, name.as_deref().unwrap_or(s))?;
            }
            s.clone()
        }
        None => {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::workspace_session_handles::allocate_ordinal(&conn, &ws.id, &conversation_key)
                .map_err(|e| err("internal", 500, e))?
                .to_string()
        }
    };
    let address = k2_core::workspace_session_handles::format_address(&ws.handle, Some(&handle));
    let display_name = current_custom
        .clone()
        .or(name.clone())
        .or_else(|| Some(row.title.chars().take(40).collect::<String>()).filter(|t| !t.trim().is_empty()))
        .unwrap_or_else(|| address.clone());

    // Brief: written for named sessions (its folder is the name).
    let brief_info = match (&brief, &slug) {
        (Some(text), Some(s)) => Some(core::write_brief(std::path::Path::new(&ws.path), s, text).map_err(|e| err("bad_request", 400, e))?),
        (Some(_), None) => return Err(err("bad_request", 400, "a brief needs --name (the brief lives in .k2/sidecars/<name>/)")),
        _ => None,
    };
    let launch_prompt = match (&extra.launch_message, &brief_info) {
        (Some(m), _) => Some(m.clone()),
        (None, Some(b)) => Some(core::brief_changed_message(&address, &b.rel_path)),
        (None, None) => None,
    };

    // Spawn (WS-O12): the same conversation, in its own folder.
    let resume_id = ids.last().cloned().unwrap_or_else(|| cid.clone());
    let (pg, new_row) = match &asleep_row {
        Some(r) => (r.pane_group_id.clone(), false),
        None => (uuid::Uuid::new_v4().to_string(), true),
    };
    let mut args = core::resume_argv(&preset.program, &preset.args, &resume_id, &cwd);
    if let Some(m) = model.as_deref() {
        let (ws_default, force_model) = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::workspace::model_splice::load_workspace_model(&conn, &ws.path)
        };
        args = k2_core::workspace::model_splice::decide_and_apply(&preset.program, &args, Some(m), ws_default.as_deref(), true, force_model).args;
    }
    let agent_name = format!("tab-{pg}");
    crate::v2_spawn::clear_closed_tab(&agent_name);
    let outcome = crate::spawn::spawn_agent_session_v2_blocking(crate::spawn::SpawnWorkspaceSessionRequest {
        agent_name: agent_name.clone(),
        project_id: Some(ws.id.clone()),
        cwd: cwd.clone(),
        command: Some(preset.program.clone()),
        args: Some(args.clone()),
        cols: 120,
        rows: 38,
        canonical_key: Some(agent_name.clone()),
        env: preset.env.clone(),
        env_owner_set: true,
        launch_prompt: launch_prompt.clone(),
        label: Some(display_name.clone()),
    })
    .map_err(|e| sc::spawn_error(&e))?;

    {
        let created_by = caller.audit_user();
        let db = k2_core::db::shared();
        let conn = db.lock();
        core::stamp_meta(
            &conn,
            &ws.id,
            &pg,
            new_row.then_some(created_by.as_str()),
            brief_info.as_ref().map(|b| b.rel_path.as_str()),
            None,
        )
        .map_err(|e| err("spawn_failed", 500, e))?;
    }
    let delivered = match (&launch_prompt, outcome.launch_prompt_attached) {
        (None, _) => "none",
        (Some(_), true) => "launch_param",
        (Some(p), false) => {
            let sid = outcome.session_id;
            let payload = p.clone();
            let profile = k2_core::workspace::provider_resume::injection_profile_for_provider(preset.provider);
            std::thread::spawn(move || {
                let ok = crate::workspace_msg::inject_raw_into_session_with_profile(&sid, &payload, Duration::from_secs(60), &profile);
                log_debug!("[sessions-open] first-turn inject session={sid} delivered={ok}");
            });
            "inject"
        }
    };
    sc::write_layout_and_title(&ws, &pg, &display_name, &preset.program, Some(&resume_id));
    let created_by_out = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        core::meta(&conn, &ws.id, &pg).and_then(|m| m.created_by)
    };
    log_debug!("[sessions-open] {address} pg={pg} cli={cli} conversation={cid} new_row={new_row}");
    Ok(Opened {
        body: json!({
            "ok": true,
            "address": address,
            "role": "sidecar",
            "alreadyLive": false,
            "opened": true,
            "outcome": if new_row { "reopened" } else { "resumed_row" },
            "cli": cli,
            "preset": preset.label,
            "conversationId": cid,
            "sessionId": outcome.session_id.to_string(),
            "paneGroupId": pg,
            "worktree": row.worktree,
            "brief": brief_info.as_ref().map(|b| json!({ "path": b.rel_path, "bytes": b.bytes, "delivered": delivered })),
            "firstTurn": delivered,
            "createdBy": created_by_out.as_deref().map(sc::created_by_display),
        }),
    })
}

// ── k2 msg --session (WS-M2) and self-messages (WS-M5) ───────────────

/// The server-only params key the TCP `/cli/workspace/msg` arm stamps
/// with the caller kind when no passport was presented (`owner`,
/// `login:<user>`). Stripped from requests like the other stamps.
pub const CALLER_KIND_KEY: &str = "caller_kind";

/// The caller of a `/cli/workspace/msg` request.
pub fn msg_caller(params: &HashMap<String, String>) -> Caller {
    if let Some(p) = crate::caller_workspace::principal_from_params(params) {
        let sid = params
            .get(crate::caller_workspace::CELL_SESSION_KEY)
            .map(String::as_str)
            .unwrap_or("");
        return crate::sidecar_routes::caller_from_session(sid, &p);
    }
    match params.get(CALLER_KIND_KEY).map(String::as_str) {
        Some("owner") => Caller::Owner,
        Some(k) if k.starts_with("login:") => Caller::Login { username: k["login:".len()..].to_string() },
        // Not stamped (a door that does not classify) or anything else:
        // never the owner.
        _ => Caller::Invalid,
    }
}

/// The conversation key of the caller's own session (for the self check).
fn own_conversation(caller: &Caller) -> Option<(String, Option<String>)> {
    let Caller::Agent(a) = caller else { return None };
    let key = a.map_key.clone()?;
    let live = crate::v2_session_map::lookup_by_agent_name(&key)?;
    let cid = crate::v2_spawn::conversation_id_from_args(live.program.as_deref(), &live.args);
    Some((key, cid))
}

/// `400 self_message` when `target` (a `k2 msg` first argument) is the
/// caller's own session.
pub fn self_message_refusal(params: &HashMap<String, String>, target: &str) -> Option<CliResponse> {
    let caller = msg_caller(params);
    let Caller::Agent(a) = &caller else { return None };
    let (own_key, own_cid) = own_conversation(&caller)?;
    let is_self = match crate::workspace_msg::resolve_msg_target(target)? {
        crate::workspace_msg::MsgTarget::WorkspaceCanonical { path } => {
            let pid = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                k2_core::workspace::agent_identity::resolve_project_id(&conn, &path)
            };
            pid.as_deref() == Some(own_key.as_str()) && own_key == a.project_id
        }
        crate::workspace_msg::MsgTarget::Sidecar { conversation_key, .. } => {
            let own_pane = own_key.strip_prefix("tab-").map(str::to_string);
            own_cid.as_deref() == Some(conversation_key.as_str()) || own_pane.as_deref() == Some(conversation_key.as_str())
        }
        crate::workspace_msg::MsgTarget::Session { session_id } => {
            own_cid.as_deref() == Some(session_id.as_str())
                || crate::v2_session_map::lookup_by_agent_name(&own_key)
                    .is_some_and(|s| s.session_id.to_string() == session_id)
        }
    };
    is_self.then(self_refusal)
}

fn self_refusal() -> CliResponse {
    CliResponse {
        status: "400 Bad Request",
        content_type: "application/json",
        body: json!({
            "success": false,
            "reason": "self_message",
            "hint": "that is you (k2 whoami); message another session from k2 sessions",
        })
        .to_string(),
    }
}

/// What `k2 msg --session <id>` resolved to.
pub enum SessionTarget {
    /// Deliver to this `k2 msg` target as usual (live or asleep holder).
    Deliver(String),
    /// Answered already (refusal, self, or a revive).
    Done(CliResponse),
}

fn msg_refusal(e: &SidecarError) -> CliResponse {
    CliResponse {
        status: sc::status_line(e.status),
        content_type: "application/json",
        body: json!({ "success": false, "reason": e.code, "hint": e.hint, "error": { "code": e.code, "hint": e.hint } }).to_string(),
    }
}

/// Resolve `session=` for `/cli/workspace/msg` (WS-M2):
/// 1–3. a runner or a tab row holds it → its address (deliver as today);
/// 4. past only → reopen it with the message as the first turn (the open
///    gate, cap and refusals apply); `wake=false` never revives.
pub fn msg_session(
    params: &HashMap<String, String>,
    token: &str,
    text: &str,
    from: &str,
    command: &str,
    wake: bool,
) -> SessionTarget {
    let caller = msg_caller(params);
    if let Err(e) = sc::require_credential(&caller) {
        return SessionTarget::Done(msg_refusal(&e));
    }
    let requested = params.get("workspace").map(String::as_str).filter(|w| !w.trim().is_empty());
    let ws = match sc::target_workspace(&caller, requested) {
        Ok(ws) => ws,
        Err(e) => return SessionTarget::Done(msg_refusal(&e)),
    };
    if let Err(e) = deny_sandboxed(&caller) {
        return SessionTarget::Done(msg_refusal(&e));
    }
    let row = match resolve(&ws, token) {
        Ok(r) => r.row,
        Err(e) => return SessionTarget::Done(msg_refusal(&e)),
    };
    let ids = holder_ids(&row);
    let owner = ids
        .iter()
        .find_map(|id| crate::conversation_owner::conversation_owner(id, Some(&ws.id), None));
    if let Some(o) = owner {
        if let (Some(own), Some(agent)) = (own_conversation(&caller), o.agent_name.as_deref()) {
            if own.0 == agent {
                return SessionTarget::Done(self_refusal());
            }
        }
        if o.role == "main" {
            return SessionTarget::Deliver(ws.handle.clone());
        }
        let (runner, _) = runner_for(&ws, &row);
        if let Some(addr) = runner.and_then(|r| r.get("address").and_then(Value::as_str).map(str::to_string)) {
            return SessionTarget::Deliver(addr);
        }
        // A live runner with no address (a heartbeat): its PTY id.
        if let Some(live) = o.agent_name.as_deref().and_then(crate::v2_session_map::lookup_by_agent_name) {
            return SessionTarget::Deliver(live.session_id.to_string());
        }
        return SessionTarget::Deliver(row.conversation_id.clone());
    }
    if !wake {
        return SessionTarget::Done(CliResponse::ok_json(
            json!({
                "success": false,
                "reason": "dormant_no_wake",
                "hint": format!("{} is a past conversation; without --no-wake k2 msg --session reopens it", row.conversation_id),
            })
            .to_string(),
        ));
    }
    // A revive is an open: a Connect login needs Admin+ (the open route's
    // floor; `k2 msg` itself is Member).
    if let Caller::Login { username } = &caller {
        let role = k2_core::connect_users::role_for_user(username);
        if !matches!(role, Some(k2_core::connect_users::Role::Owner | k2_core::connect_users::Role::Admin)) {
            return SessionTarget::Done(msg_refusal(&err(
                "forbidden",
                403,
                "reopening a past conversation needs an Admin or Owner login",
            )));
        }
    }
    let message = crate::workspace_msg::format_message(from, text, command);
    let body = json!({ "workspace": ws.id, "id": row.conversation_id });
    match handle_open(&caller, &body, OpenExtra { launch_message: Some(message) }) {
        Ok(o) => {
            let addr = o.body.get("address").and_then(Value::as_str).unwrap_or_default().to_string();
            sc::audit("sessions-open", &caller, &format!("ok {addr} revived"), "msg");
            if o.body.get("opened").and_then(Value::as_bool) == Some(true) {
                crate::overlay_routes::record_inject_chatter(&addr, from, text, "msg", None, None);
                SessionTarget::Done(CliResponse::ok_json(
                    json!({
                        "success": true,
                        "revived": true,
                        "addr": addr,
                        "target_session_id": o.body.get("sessionId"),
                        "attempts": 1,
                        "firstTurn": o.body.get("firstTurn"),
                    })
                    .to_string(),
                ))
            } else {
                // Something took it between the check and the open.
                SessionTarget::Deliver(addr)
            }
        }
        Err(e) => {
            sc::audit("sessions-open", &caller, &format!("refused:{}", e.code), "msg");
            SessionTarget::Done(msg_refusal(&e))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_shapes() {
        for p in [ROSTER, PAST, SHOW, OPEN] {
            assert!(is_route(p), "{p}");
        }
        assert!(!is_route("/cli/sessions/list-for-workspace"));
        assert!(is_post_route(OPEN));
        assert!(!is_post_route(ROSTER));
        assert_eq!(body_cap(OPEN), 96 * 1024);
        assert_eq!(body_cap(PAST), 4 * 1024);
    }

    #[test]
    fn cli_names_normalize() {
        assert_eq!(normalize_cli("cursor-agent").expect("cursor"), "cursor");
        assert_eq!(normalize_cli("Claude").expect("claude"), "claude");
        assert_eq!(normalize_cli("vim").expect_err("unknown").code, "bad_request");
    }

    #[test]
    fn msg_caller_without_passport_is_the_stamped_kind() {
        let mut p = HashMap::new();
        assert_eq!(msg_caller(&p), Caller::Invalid, "unstamped is never the owner");
        p.insert(CALLER_KIND_KEY.to_string(), "owner".to_string());
        assert_eq!(msg_caller(&p), Caller::Owner);
        p.insert(CALLER_KIND_KEY.to_string(), "login:ana".to_string());
        assert_eq!(msg_caller(&p), Caller::Login { username: "ana".into() });
        p.insert(CALLER_KIND_KEY.to_string(), "other".to_string());
        assert_eq!(msg_caller(&p), Caller::Invalid);
    }
}
