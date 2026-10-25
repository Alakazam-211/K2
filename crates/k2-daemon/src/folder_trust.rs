//! Folder trust, daemon side (prd-security-sweep-0464-v1 rev 2, SS9–SS11).
//!
//! The gate itself (scan, decisions, grants) is
//! `k2_core::cli_folder_trust::gate`. This module is what needs the
//! daemon:
//!
//! - **the card** ([`install`] registers the presenter): a one-tap action
//!   card `folder.trust` (Trust it / Always trust this folder / Don't
//!   trust) on the folder's own workspace Thread, else on the asking
//!   agent's Thread, else an owner ticket (a `/v1` hire or clone, a
//!   headless box with no pinned Chat). Only the owner answers
//!   (`require_action_card_person`, E2/A2);
//! - **the spawn gate** ([`gate_spawn`]): every harness spawn (canonical,
//!   heartbeat, wake, sidecar, v2/terminal) asks the gate first; a held
//!   folder's spawn is queued and retried once when the person trusts it
//!   ([`hold_spawn`], [`after_decision`]); heartbeats are skipped
//!   ([`heartbeat_hold`]);
//! - **the 409** ([`rewrite_refusal`]): a spawn error carrying the gate's
//!   token becomes `409 folder_trust_pending|folder_trust_denied` with the
//!   body that says how to decide, whatever route it came through;
//! - **the routes** ([`dispatch`]): `GET /cli/folder-trust/status`,
//!   `POST /cli/folder-trust/request` (agents ask), `POST
//!   /cli/folder-trust/decide` and `POST /cli/folder-trust/reset` (the
//!   owner decides; Settings → Reset);
//! - **boot** ([`boot_pass`]): the 0.46.3 upgrade marks (one owner summary)
//!   and the re-assert sweep;
//! - "Don't trust" also stops K2's running AI sessions in that folder:
//!   Claude reloads hooks mid-session (U0 finding 1), so a running session
//!   would pick up settings the person just refused.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use serde_json::{json, Value as J};

use k2_core::cli_folder_trust::gate::{self, Actor, Ask, Asked, Decision, Door, Outcome, Presenter, Refusal};
use k2_core::log_debug;

use crate::cli_response::CliResponse;

/// The action card id (`kd/action_cards.rs` `run`).
pub const CARD_ACTION: &str = "folder.trust";
pub const TRUST_IT: &str = "Trust it";
pub const ALWAYS: &str = "Always trust this folder";
pub const DONT: &str = "Don't trust";

pub const STATUS_PATH: &str = "/cli/folder-trust/status";
pub const REQUEST_PATH: &str = "/cli/folder-trust/request";
pub const DECIDE_PATH: &str = "/cli/folder-trust/decide";
pub const RESET_PATH: &str = "/cli/folder-trust/reset";

/// Findings listed on a card (the rest: `k2 workspace trust status`).
const CARD_LINES: usize = 5;

// ── Install ──────────────────────────────────────────────────────────

/// Register the card presenter. Call once after the DB is open, before
/// any spawn (idempotent).
pub fn install() {
    if gate::has_presenter() {
        return;
    }
    gate::set_presenter(Presenter { present: Box::new(present), is_open: Box::new(gate::thread_card_or_ticket_open) });
}

// ── Where the card goes ──────────────────────────────────────────────

/// The registered workspace that holds `path`: an exact `projects.path`,
/// else the longest registered folder `path` sits under.
pub(crate) fn project_for_path(path: &str) -> Option<(String, String)> {
    let projects = k2_core::projects_ops::projects_list().ok()?;
    let p = Path::new(path);
    projects
        .into_iter()
        .filter(|pr| !pr.path.is_empty() && (pr.path == path || p.starts_with(&pr.path)))
        .max_by_key(|pr| pr.path.len())
        .map(|pr| (pr.id, pr.path))
}

fn pinned_conversation(project_id: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::db::schema::WorkspaceSession::get(&conn, project_id)
        .ok()
        .flatten()
        .and_then(|s| s.session_id)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn agent_name_of(project_id: &str) -> String {
    k2_core::workspace_session_handles::workspace_address_name_shared(project_id).unwrap_or_else(|_| "An agent".to_string())
}

fn tool_display(tool: Option<&str>) -> &'static str {
    match tool.map(|t| {
        let base = Path::new(t).file_name().and_then(|s| s.to_str()).unwrap_or(t).to_ascii_lowercase();
        base.trim_end_matches(".exe").to_string()
    }) {
        Some(t) if t == "claude" => "Claude",
        Some(t) if t == "codex" => "Codex",
        Some(t) if t == "grok" => "Grok",
        Some(t) if t == "gemini" => "Gemini",
        Some(t) if t == "cursor-agent" => "Cursor",
        _ => "AI tools",
    }
}

/// The card's words (PRD §3.4).
pub fn card_prompt(agent: &str, eval: &gate::Evaluation, actor: &Actor) -> String {
    let tool = tool_display(actor.tool.as_deref());
    let mut s = if actor.changed_live {
        format!(
            "**The settings in `{}` changed while {agent} was running.** They make the AI tools run commands or send your logins elsewhere (Claude picks up new hooks without a restart):\n",
            eval.path
        )
    } else {
        format!(
            "**{agent} wants to start {tool} in `{}`.** This folder has settings that make the AI tools run commands or send your logins elsewhere when they start:\n",
            eval.path
        )
    };
    for f in eval.findings.iter().take(CARD_LINES) {
        s.push_str(&format!("• `{}`: {}\n", f.file, f.detail));
    }
    if eval.findings.len() > CARD_LINES {
        s.push_str(&format!(
            "(and {} more: `k2 workspace trust status {}`)\n",
            eval.findings.len() - CARD_LINES,
            gate::shell_quote(&eval.path)
        ));
    }
    if let Some(why) = actor.why.as_deref().map(str::trim).filter(|w| !w.is_empty()) {
        s.push_str(&format!("{agent} says: {}\n", why.chars().take(300).collect::<String>()));
    }
    s.push_str("If you didn't put these there, don't trust it.");
    s
}

/// The presenter: a Thread card where there is a Thread, else an owner
/// ticket. Never fails a spawn; where nothing can be posted, the 409 body
/// still says how to decide.
fn present(ask: &Ask<'_>) -> Asked {
    let eval = ask.eval;
    let folder = project_for_path(&eval.path).or_else(|| project_for_path(&eval.scope)).map(|(id, _)| id);
    let principal = crate::caller_workspace::request_principal().map(|p| p.workspace_uuid).filter(|w| !w.is_empty());
    let mut candidates: Vec<String> = Vec::new();
    for c in [folder.clone(), ask.actor.project_id.clone(), principal] {
        if let Some(c) = c.filter(|c| !c.is_empty()) {
            if !candidates.contains(&c) {
                candidates.push(c);
            }
        }
    }
    let agent = candidates.first().map(|p| agent_name_of(p)).unwrap_or_else(|| "An agent".to_string());
    let prompt = card_prompt(&agent, eval, ask.actor);
    let action = k2_core::overlay::CardAction {
        id: CARD_ACTION.to_string(),
        params: json!({
            "path": eval.path,
            "scope": eval.scope,
            "fingerprint": eval.fingerprint,
            "agent": agent,
        }),
        dedupe: eval.scope.clone(),
        result: None,
        view: None,
    };
    let options = [TRUST_IT, ALWAYS, DONT];
    let title = format!("Trust {}?", eval.path);
    // An owner ticket filed earlier (no Thread then) rides along with the
    // card, so the decision closes it too.
    let open_ticket = |project: &str| {
        k2_core::feedback::list_for_project(project, &k2_core::feedback::ListFilter::Open)
            .unwrap_or_default()
            .into_iter()
            .find(|i| i.title == title)
            .map(|i| i.id)
    };
    for project in &candidates {
        let Some(conversation) = pinned_conversation(project) else { continue };
        match crate::overlay_routes::present_action_card(
            Some(&conversation),
            project,
            &prompt,
            &options,
            action.clone(),
            true,
            k2_core::overlay::now_secs() + k2_core::overlay::DEFAULT_CARD_EXPIRY_SECS,
            crate::overlay_routes::CardAudience::AgentPost,
        ) {
            Ok(crate::overlay_routes::Presented::Card { id, .. }) | Ok(crate::overlay_routes::Presented::Waiting { id }) => {
                log_debug!("[folder-trust] card {id} for {} on {project}", eval.scope);
                let ticket = candidates.first().and_then(|p| open_ticket(p));
                return Asked { card: Some(id), conversation: Some(conversation), project: Some(project.clone()), ticket };
            }
            Ok(crate::overlay_routes::Presented::NoThread) => {}
            Err(e) => log_debug!("[folder-trust] card for {} on {project}: {e}", eval.scope),
        }
    }
    // No person and no Thread (A6): an owner ticket with the findings and
    // the three choices.
    let Some(project) = candidates.first() else {
        log_debug!("[folder-trust] {}: no workspace to post a card or ticket on", eval.scope);
        return Asked::default();
    };
    if let Some(id) = open_ticket(project) {
        return Asked { ticket: Some(id), project: Some(project.clone()), ..Default::default() };
    }
    let body = format!(
        "{prompt}\n\nThere is no Thread to put a card on, so K2 filed this ticket. Choose one (owner):\n\
         • Trust it (this version): k2 workspace trust {q} --yes\n\
         • Always trust this folder: k2 workspace trust {q} --always\n\
         • Don't trust: k2 workspace trust {q} --no\n\
         Until then K2 starts no AI tool in this folder. Settings → Workspaces → Folder trust shows it too.",
        q = gate::shell_quote(&eval.path)
    );
    match crate::feedback_routes::file_owners_only_notice(project, &agent, &title, &body) {
        Ok(item) => Asked { ticket: Some(item.id), project: Some(project.clone()), ..Default::default() },
        Err(e) => {
            log_debug!("[folder-trust] ticket for {}: {e}", eval.scope);
            Asked::default()
        }
    }
}

// ── Spawn gate, held spawns, heartbeats ──────────────────────────────

/// Spawns held for a scope, retried once when the person trusts it.
static HELD: Mutex<Option<HashMap<String, Vec<crate::spawn::SpawnWorkspaceSessionRequest>>>> = Mutex::new(None);
/// At most this many held spawns per scope (the newest per map key wins).
const HELD_MAX: usize = 16;

/// The spawn door: `Err(refusal)` when the folder is held or denied. Non-
/// harness programs pass.
pub fn gate_spawn(
    program: Option<&str>,
    cwd: &str,
    env_home: Option<&str>,
    project_id: Option<&str>,
    by: &str,
) -> Result<(), Refusal> {
    let actor = Actor {
        project_id: project_id.map(str::to_string),
        by: by.to_string(),
        tool: program.map(str::to_string),
        ..Default::default()
    };
    k2_core::cli_folder_trust::gate_harness_spawn(program, Some(Path::new(cwd)), env_home, &actor).map(|_| ())
}

/// Queue a held spawn (not heartbeat lanes: those are skipped).
pub fn hold_spawn(scope: &str, req: &crate::spawn::SpawnWorkspaceSessionRequest) {
    if req.canonical_key.as_deref().is_some_and(|k| k.contains(":hb:")) {
        return;
    }
    let key = crate::spawn::canonical_key_for_request(req);
    let mut g = HELD.lock().unwrap_or_else(|e| e.into_inner());
    let list = g.get_or_insert_with(HashMap::new).entry(scope.to_string()).or_default();
    list.retain(|r| crate::spawn::canonical_key_for_request(r) != key);
    list.push(req.clone());
    if list.len() > HELD_MAX {
        list.remove(0);
    }
}

fn take_held(scope: &str) -> Vec<crate::spawn::SpawnWorkspaceSessionRequest> {
    let mut g = HELD.lock().unwrap_or_else(|e| e.into_inner());
    g.as_mut().and_then(|m| m.remove(scope)).unwrap_or_default()
}

/// How many spawns are held for `scope` (tests, status).
pub fn held_count(scope: &str) -> usize {
    let g = HELD.lock().unwrap_or_else(|e| e.into_inner());
    g.as_ref().and_then(|m| m.get(scope)).map(Vec::len).unwrap_or(0)
}

/// Heartbeats: `Some(refusal)` when the workspace's agent is a gated
/// harness (claude/codex/grok/gemini/cursor-agent) and its folder is held
/// or denied, so the fire is skipped (an activity row says `held: folder
/// trust`). A held folder still gets its card. A custom (no-model) agent
/// or another program isn't gated, as at the spawn door.
pub fn heartbeat_hold(project_path: &str, project_id: &str) -> Option<Refusal> {
    let program = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::workspace::agent_resolve::resolve_agent_command(&conn, project_path).command
    };
    heartbeat_hold_for(project_path, project_id, &program)
}

/// [`heartbeat_hold`] with the agent's program given.
pub fn heartbeat_hold_for(project_path: &str, project_id: &str, program: &str) -> Option<Refusal> {
    if !k2_core::cli_folder_trust::is_folder_trust_harness(Some(program)) {
        return None;
    }
    let actor = Actor {
        project_id: Some(project_id.to_string()),
        by: "heartbeat".into(),
        tool: Some(program.to_string()),
        ..Default::default()
    };
    match gate::ensure(project_path, Door::Spawn, &actor, None) {
        Outcome::Held(r) | Outcome::Denied(r) => Some(r),
        _ => None,
    }
}

/// After a person's decision: retry held spawns once on trust; on "Don't
/// trust", drop them and stop K2's running AI sessions in the folder.
/// Returns how many sessions were stopped.
pub fn after_decision(scope: &str, keys: &[String], decision: Decision, granted: bool) -> usize {
    match decision {
        Decision::Once | Decision::Always if granted => {
            let held = take_held(scope);
            if !held.is_empty() {
                // The spawn registers the session and emits session events,
                // which need the daemon's tokio runtime: carry its handle
                // into the retry thread (a request worker has one).
                let rt = tokio::runtime::Handle::try_current().ok();
                std::thread::Builder::new()
                    .name("k2-folder-trust-retry".into())
                    .spawn(move || {
                        let _rt = rt.as_ref().map(|h| h.enter());
                        for req in held {
                            let key = crate::spawn::canonical_key_for_request(&req);
                            match crate::spawn::spawn_agent_session_v2_blocking(req) {
                                Ok(o) => log_debug!("[folder-trust] held spawn {key} started (session {})", o.session_id),
                                Err(e) => log_debug!("[folder-trust] held spawn {key}: {e}"),
                            }
                        }
                    })
                    .ok();
            }
            0
        }
        Decision::Deny => {
            take_held(scope);
            stop_sessions_in(scope, keys)
        }
        _ => 0,
    }
}

/// Stop K2's live harness sessions in a folder: cwd under one of
/// `folders`, or in another work tree of the same repo (`scope`).
pub fn stop_sessions_in(scope: &str, folders: &[String]) -> usize {
    let mut n = 0;
    for (key, s) in crate::v2_session_map::snapshot() {
        if !k2_core::cli_folder_trust::is_folder_trust_harness(s.program.as_deref()) {
            continue;
        }
        let Some(cwd) = s.cwd.as_ref().and_then(|c| c.canonicalize().ok().or_else(|| Some(c.clone()))) else { continue };
        let inside = folders.iter().any(|f| cwd.starts_with(f))
            || gate::scope_for(&cwd.to_string_lossy()).as_deref() == Some(scope);
        if inside && crate::v2_session_map::unregister(&key).is_some() {
            log_debug!("[folder-trust] stopped {key} ({}): folder not trusted", cwd.display());
            n += 1;
        }
    }
    n
}

// ── 409 ──────────────────────────────────────────────────────────────

/// A non-2xx response whose body carries the gate's token becomes the 409
/// the PRD names. `None` = leave the response alone.
pub fn rewrite_refusal(status: &str, body: &str) -> Option<(&'static str, String)> {
    if status.starts_with('2') || !gate::has_refusal_token(body) {
        return None;
    }
    let r = gate::refusal_in_text(body)?;
    Some(("409 Conflict", r.to_json().to_string()))
}

/// A spawn error for a route: the refusal's 409, else the old 400.
pub fn spawn_failed_response(e: &str) -> CliResponse {
    match gate::refusal_in_text(e) {
        Some(r) => CliResponse { status: "409 Conflict", content_type: "application/json", body: r.to_json().to_string() },
        None => CliResponse::bad_request(format!("spawn failed: {e}")),
    }
}

// ── The card's answer (action_cards::run) ────────────────────────────

/// A person tapped an option on a `folder.trust` card (already checked to
/// be the owner by `require_action_card_person`). `by`: `owner` or a login.
pub fn run_card(params: &J, answer: &str, by: &str) -> Result<J, String> {
    let path = params["path"].as_str().filter(|p| !p.is_empty()).ok_or("the card names no folder")?;
    let card_fp = params["fingerprint"].as_str();
    let decision = match answer {
        TRUST_IT => Decision::Once,
        ALWAYS => Decision::Always,
        DONT => Decision::Deny,
        other => return Err(format!("'{other}' isn't an answer to this card")),
    };
    let pending = gate::scope_for(path).as_deref().and_then(gate::pending_of);
    let r = gate::decide(path, decision, by, if decision == Decision::Once { card_fp } else { None })?;
    if let Some(p) = pending {
        close_pending(&p, false);
    }
    let keys = gate::evaluate(path).map(|e| e.keys).unwrap_or_else(|| vec![r.path.clone()]);
    let stopped = after_decision(&r.scope, &keys, decision, r.granted);
    let mut message = r.message.clone();
    if stopped > 0 {
        message.push_str(&format!(" Stopped {stopped} running session{} there.", if stopped == 1 { "" } else { "s" }));
    }
    Ok(json!({
        "decision": decision.as_str(),
        "path": r.path,
        "granted": r.granted,
        "leftover": r.leftover,
        "stopped": stopped,
        "message": message,
    }))
}

/// After a decision: resolve the owner ticket and (when the decision came
/// from elsewhere than the card) void the waiting card.
fn close_pending(p: &gate::Pending, void_card: bool) {
    if void_card {
        if let (Some(conv), Some(card), Some(project)) = (p.conversation.as_deref(), p.card.as_deref(), p.project.as_deref()) {
            match k2_core::overlay::void_card(conv, project, card, None) {
                Ok(cb) => crate::overlay_routes::publish_action_frame(&cb),
                Err(e) => log_debug!("[folder-trust] void card {card}: {e}"),
            }
        }
    }
    if let Some(t) = p.ticket.as_deref() {
        if let Err(e) = k2_core::feedback::set_status(t, "resolved") {
            log_debug!("[folder-trust] resolve ticket {t}: {e}");
        }
    }
}

// ── Routes ───────────────────────────────────────────────────────────

fn param(params: &HashMap<String, String>, body: &[u8], key: &str) -> Option<String> {
    if let Some(v) = params.get(key).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        return Some(v);
    }
    let v: J = serde_json::from_slice(body).ok()?;
    v.get(key).and_then(|x| x.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn err(status: &'static str, code: &str, message: impl std::fmt::Display) -> CliResponse {
    CliResponse { status, content_type: "application/json", body: json!({ "error": code, "message": message.to_string() }).to_string() }
}

fn folder_param(params: &HashMap<String, String>, body: &[u8]) -> Result<String, CliResponse> {
    param(params, body, "path")
        .or_else(|| param(params, body, "folder"))
        .ok_or_else(|| err("400 Bad Request", "usage", "path required (the folder)"))
}

/// The workspace a passport caller belongs to (for the card's place).
fn caller_project(params: &HashMap<String, String>) -> Option<String> {
    crate::caller_workspace::principal_from_params(params)
        .map(|p| p.workspace_uuid)
        .or_else(|| crate::caller_workspace::request_principal().map(|p| p.workspace_uuid))
        .filter(|w| !w.is_empty())
}

/// `/cli/folder-trust/*`. `session_author`: `owner` or a login name (the
/// dispatcher's), used by the person check on decide/reset.
pub fn dispatch(is_post: bool, path: &str, params: &HashMap<String, String>, body: &[u8], session_author: &str) -> CliResponse {
    match path {
        STATUS_PATH => status_route(params, body),
        REQUEST_PATH => {
            if !is_post {
                return CliResponse::method_not_allowed();
            }
            request_route(params, body)
        }
        DECIDE_PATH => {
            if !is_post {
                return CliResponse::method_not_allowed();
            }
            if let Err(r) = crate::overlay_routes::require_action_card_person(params, session_author, "answers") {
                return r;
            }
            decide_route(params, body, session_author)
        }
        RESET_PATH => {
            if !is_post {
                return CliResponse::method_not_allowed();
            }
            if let Err(r) = crate::overlay_routes::require_action_card_person(params, session_author, "answers") {
                return r;
            }
            reset_route(params, body, session_author)
        }
        _ => CliResponse::not_found(),
    }
}

fn status_route(params: &HashMap<String, String>, body: &[u8]) -> CliResponse {
    match param(params, body, "path").or_else(|| param(params, body, "folder")) {
        Some(p) => match gate::status(&p) {
            Ok(v) => CliResponse::ok_json(serde_json::to_string(&v).unwrap_or_default()),
            Err(e) => err("400 Bad Request", "usage", e),
        },
        None => {
            // Every registered workspace (Settings → Workspaces).
            let projects = match k2_core::projects_ops::projects_list() {
                Ok(p) => p,
                Err(e) => return CliResponse::internal_error(e),
            };
            let folders: Vec<J> = projects
                .iter()
                .filter_map(|p| gate::status(&p.path).ok().map(|s| {
                    let mut v = serde_json::to_value(&s).unwrap_or(J::Null);
                    v["projectId"] = json!(p.id);
                    v
                }))
                .collect();
            CliResponse::ok_json(json!({ "folders": folders }).to_string())
        }
    }
}

fn request_route(params: &HashMap<String, String>, body: &[u8]) -> CliResponse {
    let folder = match folder_param(params, body) {
        Ok(f) => f,
        Err(r) => return r,
    };
    let actor = Actor {
        project_id: caller_project(params),
        by: "request".into(),
        why: param(params, body, "why"),
        ..Default::default()
    };
    let out = gate::ensure(&folder, Door::Spawn, &actor, None);
    CliResponse::ok_json(out.to_json().to_string())
}

fn decision_param(params: &HashMap<String, String>, body: &[u8]) -> Result<Decision, CliResponse> {
    let raw = param(params, body, "decision").unwrap_or_default();
    Decision::parse(&raw).ok_or_else(|| err("400 Bad Request", "usage", "decision must be once (--yes), always (--always) or deny (--no)"))
}

fn decide_route(params: &HashMap<String, String>, body: &[u8], by: &str) -> CliResponse {
    let folder = match folder_param(params, body) {
        Ok(f) => f,
        Err(r) => return r,
    };
    let decision = match decision_param(params, body) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let scope = gate::scope_for(&folder);
    let pending = scope.as_deref().and_then(gate::pending_of);
    let r = match gate::decide(&folder, decision, by, None) {
        Ok(r) => r,
        Err(e) => return err("400 Bad Request", "usage", e),
    };
    // The card that was waiting no longer asks anything.
    if let Some(p) = pending {
        close_pending(&p, true);
    }
    let keys = gate::evaluate(&folder).map(|e| e.keys).unwrap_or_else(|| vec![r.path.clone()]);
    let stopped = after_decision(&r.scope, &keys, decision, r.granted);
    let mut v = serde_json::to_value(&r).unwrap_or(J::Null);
    v["ok"] = json!(true);
    v["stopped"] = json!(stopped);
    CliResponse::ok_json(v.to_string())
}

fn reset_route(params: &HashMap<String, String>, body: &[u8], by: &str) -> CliResponse {
    let folder = match folder_param(params, body) {
        Ok(f) => f,
        Err(r) => return r,
    };
    match gate::reset(&folder, by) {
        Ok(st) => {
            take_held(&st.scope);
            let mut v = serde_json::to_value(&st).unwrap_or(J::Null);
            v["ok"] = json!(true);
            CliResponse::ok_json(v.to_string())
        }
        Err(e) => err("400 Bad Request", "usage", e),
    }
}

// ── Boot ─────────────────────────────────────────────────────────────

/// After ready: finish the 0.46.3 upgrade (one owner summary listing the
/// folders that were trusted before K2 checked and are risky now), then
/// re-assert decisions that grant (the sweep never asks in bulk).
pub fn boot_pass() {
    let upgraded = gate::upgrade_pass();
    if !upgraded.is_empty() {
        file_upgrade_summary(&upgraded);
    }
    k2_core::cli_folder_trust::trust_all_registered_projects();
}

fn file_upgrade_summary(folders: &[gate::UpgradedFolder]) {
    let Some((project, _)) = folders.iter().find_map(|f| project_for_path(&f.path)) else {
        log_debug!("[folder-trust] upgrade summary: no workspace to file it on");
        return;
    };
    let title = "Folders trusted before K2 checked them".to_string();
    let mut body = String::from(
        "K2 now looks inside a folder before it tells Claude, Codex, Grok, Gemini or Cursor to trust it. \
         These folders were trusted before K2 checked, and they have settings that run commands when the AI tools start. \
         They stay trusted as they are; K2 asks again if these settings change.\n",
    );
    for f in folders {
        body.push_str(&format!("\n{}\n", f.path));
        for line in f.findings.iter().take(CARD_LINES) {
            body.push_str(&format!("  • {}: {}\n", line.file, line.detail));
        }
        if f.findings.len() > CARD_LINES || f.more > 0 {
            body.push_str(&format!("  (more: k2 workspace trust status {})\n", gate::shell_quote(&f.path)));
        }
        body.push_str(&format!("  Review: k2 workspace trust {} --yes|--always|--no\n", gate::shell_quote(&f.path)));
    }
    body.push_str("\nSettings → Workspaces → Folder trust shows each one, with Reset.");
    let agent = agent_name_of(&project);
    if let Err(e) = crate::feedback_routes::file_owners_only_notice(&project, &agent, &title, &body) {
        log_debug!("[folder-trust] upgrade summary: {e}");
    }
}

#[cfg(test)]
#[path = "folder_trust_tests.rs"]
mod tests;
