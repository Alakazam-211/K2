//! `/cli/sim/*`: mobile development (prd-mobile-dev-v1, vs-live
//! M-A1–M-A28, 0.46.6). The shapes are `k2_core::sim::api`; the
//! lane-blind logic is `k2_core::sim::manager`. This file is who-may-call
//! and which-workspace only.
//!
//! **Callers** (`sidecar_routes::Caller`, from the TCP credential or the
//! cell socket):
//! - the owner token, a Connect login (its role already cleared
//!   `route_policy`), a K2 shell tab: people. Any workspace; a shell tab
//!   defaults to its own.
//! - an agent passport: only its own workspace, and only while
//!   "Allow mobile devices" (`projects.agents_can_use_sim`, 0233) is on;
//!   otherwise 403 `sim_switch_off` (exit 3; the permission-card wall
//!   adds `Ask with:`). Agents never get the stream, Take over / Hand
//!   back, the owner's setup (licence) or the settings.
//! - App passes and API keys: `wrong_credential` (403), always.
//!
//! **The licence** (M-A23): a boot that needs the Android SDK licence
//! answers `sim_needs_setup` (exit 3) and, for an agent, puts a card on
//! its workspace's Thread for a person. Only the box owner (owner token or
//! an Owner login) can accept it: on the card ([`run_card`]) or with
//! `POST /cli/sim/setup`.

use std::collections::HashMap;

use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use k2_core::sim::api::*;
use k2_core::sim::lane::{LaneKind, LaneNeed, SimText};
use k2_core::sim::manager::{self, Actor, SimError, Ws};

use crate::cli_response::CliResponse;
use crate::sidecar_routes::Caller;

pub const STATUS: &str = "/cli/sim/status";
pub const DEVICES: &str = "/cli/sim/devices";
pub const BOOT: &str = "/cli/sim/boot";
pub const STOP: &str = "/cli/sim/stop";
pub const ERASE: &str = "/cli/sim/erase";
pub const INSTALL: &str = "/cli/sim/install";
pub const LAUNCH: &str = "/cli/sim/launch";
pub const TERMINATE: &str = "/cli/sim/terminate";
pub const TREE: &str = "/cli/sim/tree";
pub const ACT: &str = "/cli/sim/act";
pub const SCREENSHOT: &str = "/cli/sim/screenshot";
pub const LOGS: &str = "/cli/sim/logs";
pub const WAIT: &str = "/cli/sim/wait";
pub const HOLD: &str = "/cli/sim/hold";
pub const SETUP: &str = "/cli/sim/setup";
pub const LICENCE: &str = "/cli/sim/licence";
pub const SETTINGS: &str = "/cli/sim/settings";
/// The WebSocket (S3); its own dispatcher arm, listed for the policy.
pub const STREAM: &str = "/cli/sim/stream";

/// GET routes (reads).
pub const READ_ROUTES: &[&str] = &[STATUS, DEVICES, TREE, LOGS, LICENCE];
/// POST-only routes (405 on GET).
pub const POST_ROUTES: &[&str] = &[BOOT, STOP, ERASE, INSTALL, LAUNCH, TERMINATE, ACT, SCREENSHOT, WAIT, HOLD, SETUP];
/// GET reads, POST writes.
pub const BOTH_ROUTES: &[&str] = &[SETTINGS];
/// What an agent passport may call (`session_token::is_agent_verb`).
pub const AGENT_ROUTES: &[&str] =
    &[STATUS, DEVICES, BOOT, STOP, ERASE, INSTALL, LAUNCH, TERMINATE, TREE, ACT, SCREENSHOT, LOGS, WAIT];

/// Request bodies are small JSON objects (a 4096-character `type` fits).
pub const MAX_BODY: usize = 64 * 1024;

/// The card action for the owner's licence acceptance.
pub const CARD_ACTION: &str = "sim.licence.accept";
pub const CARD_ACCEPT: &str = "Accept and install";
pub const CARD_NOT_NOW: &str = "Not now";

pub fn is_route(path: &str) -> bool {
    READ_ROUTES.contains(&path) || POST_ROUTES.contains(&path) || BOTH_ROUTES.contains(&path)
}

pub fn is_post_route(path: &str) -> bool {
    POST_ROUTES.contains(&path)
}

pub fn too_large() -> CliResponse {
    err(&SimError::new(413, "too_large", format!("request body over {MAX_BODY} bytes")))
}

fn status_line(code: u16) -> &'static str {
    match code {
        200 => "200 OK",
        400 => "400 Bad Request",
        403 => "403 Forbidden",
        404 => "404 Not Found",
        405 => "405 Method Not Allowed",
        408 => "408 Request Timeout",
        409 => "409 Conflict",
        413 => "413 Payload Too Large",
        501 => "501 Not Implemented",
        502 => "502 Bad Gateway",
        503 => "503 Service Unavailable",
        504 => "504 Gateway Timeout",
        _ => "500 Internal Server Error",
    }
}

fn err(e: &SimError) -> CliResponse {
    CliResponse { status: status_line(e.status), content_type: "application/json", body: e.body().to_string() }
}

fn ok<T: serde::Serialize>(v: &T) -> CliResponse {
    match serde_json::to_string(v) {
        Ok(s) => CliResponse::ok_json(s),
        Err(e) => CliResponse::internal_error(format!("encode: {e}")),
    }
}

fn parse<T: DeserializeOwned>(body: &[u8]) -> Result<T, SimError> {
    let b = if body.iter().all(u8::is_ascii_whitespace) { b"{}".as_slice() } else { body };
    serde_json::from_slice(b).map_err(|e| SimError::new(400, "bad_request", format!("invalid JSON body: {e}")))
}

fn forbidden(msg: &str) -> SimError {
    SimError::new(403, "forbidden", msg)
}

// ── who and which workspace ──────────────────────────────────────────

fn handle_of(project_id: &str) -> String {
    k2_core::workspace_session_handles::workspace_address_name_shared(project_id).unwrap_or_else(|_| project_id.to_string())
}

/// The actor for a caller that may use device verbs. Refuses app passes,
/// bad tokens and (for people-only routes) agents.
fn actor_of(c: &Caller) -> Result<Actor, SimError> {
    match c {
        Caller::Owner => Ok(Actor::Person { name: "owner".into() }),
        Caller::Login { username } => Ok(Actor::Person { name: format!("login:{username}") }),
        Caller::Shell { project_id } => Ok(Actor::Person { name: format!("shell:{}", handle_of(project_id)) }),
        Caller::Agent(a) => Ok(Actor::Agent { handle: handle_of(&a.project_id) }),
        Caller::WrongCredential => Err(SimError::new(403, "wrong_credential", "App passes and API keys can't use mobile devices")),
        Caller::Invalid => Err(SimError::new(403, "forbidden", "invalid or missing token")),
    }
}

fn own_project(c: &Caller) -> Option<&str> {
    match c {
        Caller::Shell { project_id } => Some(project_id),
        Caller::Agent(a) => Some(&a.project_id),
        _ => None,
    }
}

/// Resolve the target workspace. An agent may only name its own.
fn resolve_ws(c: &Caller, token: Option<&str>) -> Result<Ws, SimError> {
    let token = token.map(str::trim).filter(|t| !t.is_empty());
    let token = match (token, own_project(c)) {
        (Some(t), _) => t.to_string(),
        (None, Some(own)) => own.to_string(),
        (None, None) => return Err(SimError::new(400, "bad_request", "missing 'workspace'")),
    };
    let path = crate::workspace_msg::resolve_workspace(&token)
        .ok_or_else(|| SimError::new(404, "workspace_not_found", format!("no workspace '{token}'")))?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    let id = k2_core::workspace::agent_identity::resolve_project_id(&conn, &path)
        .ok_or_else(|| SimError::new(404, "workspace_not_found", format!("no workspace '{token}'")))?;
    drop(conn);
    if let Caller::Agent(a) = c {
        if a.project_id != id {
            return Err(forbidden("an agent may only drive its own workspace's device"));
        }
    }
    Ok(Ws { handle: handle_of(&id), id, path: std::path::PathBuf::from(path) })
}

/// Daemon tests that register sim lanes (a process-wide registry) take
/// this lock so they never run side by side.
#[cfg(test)]
pub(crate) fn sim_test_lock() -> std::sync::MutexGuard<'static, ()> {
    static L: std::sync::Mutex<()> = std::sync::Mutex::new(());
    L.lock().unwrap_or_else(|e| e.into_inner())
}

/// [`resolve_ws`] for the stream socket.
pub fn resolve_ws_for(c: &Caller, token: Option<&str>) -> Result<Ws, SimError> {
    resolve_ws(c, token)
}

/// "Allow mobile devices" for agents (exit 3 with the Settings row; the
/// permission-card wall adds `Ask with:` when cards are on).
fn agent_switch(c: &Caller, ws: &Ws) -> Result<(), SimError> {
    if let Caller::Agent(_) = c {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if !k2_core::sim::store::agents_can_use_sim(&conn, &ws.id) {
            return Err(SimError::new(
                403,
                "sim_switch_off",
                format!(
                    "mobile devices are off for {}. A person turns on \"Allow mobile devices\" in Settings → Workspaces → {} → Agent.",
                    ws.handle, ws.handle
                ),
            )
            .exit(3));
        }
    }
    Ok(())
}

fn people_only(c: &Caller) -> Result<(), SimError> {
    match c {
        Caller::Agent(_) => Err(forbidden("this is for people at the Mobile tab or in Settings, not agents")),
        _ => Ok(()),
    }
}

/// The box owner: the owner token or an Owner login (M-A23, Q7).
fn box_owner(c: &Caller) -> Result<String, SimError> {
    match c {
        Caller::Owner => Ok("owner".into()),
        Caller::Login { username } if is_owner_login(username) => Ok(format!("login:{username}")),
        _ => Err(SimError::new(
            403,
            "owner_only",
            "only the server's owner can accept a licence and set up a device lane (the card on the Thread, or Settings → K2 Server → Mobile devices)",
        )
        .exit(3)),
    }
}

fn is_owner_login(username: &str) -> bool {
    k2_core::connect_users::role_for_user(username).is_some_and(|r| r >= k2_core::connect_users::Role::Owner)
}

/// May this person send input / take over? Owner token, shell tab, and
/// Owner/Admin logins; Member logins watch only (Rosson 2026-10-27, Q13).
/// Agents drive through the verbs, not the tab (false here).
pub fn can_control(c: &Caller) -> bool {
    match c {
        Caller::Owner | Caller::Shell { .. } => true,
        Caller::Login { username } => {
            k2_core::connect_users::role_for_user(username).is_some_and(k2_core::connect_users::can_manage_users)
        }
        _ => false,
    }
}

/// People's input needs control; agents pass (their switch is checked).
fn control_gate(c: &Caller) -> Result<(), SimError> {
    match c {
        Caller::Agent(_) => Ok(()),
        _ if can_control(c) => Ok(()),
        _ => Err(SimError::new(403, "sim_watch_only", "Members can watch this device but not use it. An admin or the owner can tap and type.")),
    }
}

fn admin_or_owner(c: &Caller) -> Result<String, SimError> {
    match c {
        Caller::Owner => Ok("owner".into()),
        Caller::Login { username }
            if k2_core::connect_users::role_for_user(username).is_some_and(k2_core::connect_users::can_manage_users) =>
        {
            Ok(format!("login:{username}"))
        }
        _ => Err(forbidden("only the owner or an admin can change the device limits")),
    }
}

fn param<'a>(params: &'a HashMap<String, String>, k: &str) -> Option<&'a str> {
    params.get(k).map(String::as_str).filter(|s| !s.is_empty())
}

fn flag(params: &HashMap<String, String>, k: &str) -> bool {
    matches!(param(params, k), Some("1" | "true" | "yes"))
}

// ── the entry ────────────────────────────────────────────────────────

/// Dispatch one `/cli/sim/*` call (blocking; run it on a worker thread).
pub fn handle(path: &str, caller: Caller, is_post: bool, body: &[u8], params: &HashMap<String, String>) -> CliResponse {
    if is_post_route(path) && !is_post {
        return CliResponse::method_not_allowed();
    }
    if READ_ROUTES.contains(&path) && is_post {
        return CliResponse::method_not_allowed();
    }
    match route(path, &caller, is_post, body, params) {
        Ok(r) => r,
        Err(e) => err(&e),
    }
}

fn route(path: &str, c: &Caller, is_post: bool, body: &[u8], params: &HashMap<String, String>) -> Result<CliResponse, SimError> {
    let m = manager::manager();
    let actor = actor_of(c)?;
    match path {
        STATUS => {
            let ws = resolve_ws(c, param(params, "workspace"))?;
            let mut st = m.status(&ws, flag(params, "refresh"))?;
            st.can_control = can_control(c);
            Ok(ok(&st))
        }
        DEVICES => {
            let ws = resolve_ws(c, param(params, "workspace"))?;
            agent_switch(c, &ws)?;
            Ok(ok(&m.devices(param(params, "platform"))?))
        }
        BOOT => {
            let req: BootReq = parse(body)?;
            let ws = resolve_ws(c, req.workspace.as_deref())?;
            agent_switch(c, &ws)?;
            match m.boot(&ws, &actor, &req) {
                Ok(v) => Ok(ok(&v)),
                Err(e) => {
                    if e.code == "sim_needs_setup" {
                        present_licence_card(&ws, &e);
                    }
                    Err(e)
                }
            }
        }
        STOP => {
            let req: WorkspaceReq = parse(body)?;
            let ws = resolve_ws(c, req.workspace.as_deref())?;
            agent_switch(c, &ws)?;
            let reason = format!("stopped by {}", actor.audit());
            Ok(ok(&m.stop(&ws, &actor, &reason)?))
        }
        ERASE => {
            let req: WorkspaceReq = parse(body)?;
            let ws = resolve_ws(c, req.workspace.as_deref())?;
            agent_switch(c, &ws)?;
            let out = m.erase(&ws, &actor)?;
            m.measure_disk();
            Ok(ok(&out))
        }
        INSTALL => {
            let req: InstallReq = parse(body)?;
            let ws = resolve_ws(c, req.workspace.as_deref())?;
            agent_switch(c, &ws)?;
            Ok(ok(&m.install_app(&ws, &actor, &req.path)?))
        }
        LAUNCH | TERMINATE => {
            let req: AppReq = parse(body)?;
            let ws = resolve_ws(c, req.workspace.as_deref())?;
            agent_switch(c, &ws)?;
            Ok(ok(&m.launch(&ws, &actor, &req.app, path == TERMINATE)?))
        }
        TREE => {
            let ws = resolve_ws(c, param(params, "workspace"))?;
            agent_switch(c, &ws)?;
            Ok(ok(&m.tree(&ws, &actor, flag(params, "all"), param(params, "find"), flag(params, "boxes"))?))
        }
        ACT => {
            control_gate(c)?;
            let req: ActReq = parse(body)?;
            let ws = resolve_ws(c, req.workspace.as_deref())?;
            agent_switch(c, &ws)?;
            let secret = match &req.action {
                ActSpec::Type { secret: Some(name), .. } => Some(resolve_secret(c, &ws, name)?),
                _ => None,
            };
            Ok(ok(&m.act(&ws, &actor, &req, secret)?))
        }
        SCREENSHOT => {
            let req: ScreenshotReq = parse(body)?;
            let ws = resolve_ws(c, req.workspace.as_deref())?;
            agent_switch(c, &ws)?;
            Ok(ok(&m.screenshot(&ws, &actor, req.region.as_deref())?))
        }
        LOGS => {
            let ws = resolve_ws(c, param(params, "workspace"))?;
            agent_switch(c, &ws)?;
            let since = param(params, "sinceSecs").and_then(|s| s.parse::<u64>().ok()).map(std::time::Duration::from_secs);
            let max = param(params, "maxLines").and_then(|s| s.parse::<usize>().ok());
            Ok(ok(&m.logs(&ws, param(params, "app"), since, param(params, "grep"), max)?))
        }
        WAIT => {
            let req: WaitReq = parse(body)?;
            let ws = resolve_ws(c, req.workspace.as_deref())?;
            agent_switch(c, &ws)?;
            Ok(ok(&m.wait(&ws, &actor, &req)?))
        }
        HOLD => {
            people_only(c)?;
            control_gate(c)?;
            let req: HoldReq = parse(body)?;
            let ws = resolve_ws(c, req.workspace.as_deref())?;
            Ok(ok(&m.hold(&ws, &actor, req.action)?))
        }
        SETUP => {
            people_only(c)?;
            let by = box_owner(c)?;
            let req: SetupReq = parse(body)?;
            let accept = match (&req.accept_licence, &req.licence_sha1) {
                (Some(id), Some(sha)) => Some((id.as_str(), sha.as_str())),
                (Some(_), None) => return Err(SimError::new(400, "bad_request", "acceptLicence needs licenceSha1 (the text the owner read)")),
                _ => None,
            };
            m.start_setup(req.lane, accept, &by)?;
            Ok(ok(&json!({ "ok": true, "lane": req.lane, "lanes": m.probes_with_setup(false) })))
        }
        LICENCE => {
            people_only(c)?;
            let lane = param(params, "lane").and_then(LaneKind::parse).ok_or_else(|| SimError::new(400, "bad_request", "lane=android|ios"))?;
            let id = param(params, "id").ok_or_else(|| SimError::new(400, "bad_request", "id=<licence id>"))?;
            Ok(ok(&m.licence(lane, id)?))
        }
        SETTINGS => {
            people_only(c)?;
            if is_post {
                let by = admin_or_owner(c)?;
                let mut s: k2_core::sim::limits::SimSettings = parse(body)?;
                s.updated_at = Some(chrono::Utc::now().timestamp());
                s.updated_by = Some(by);
                k2_core::sim::limits::save_settings(&s).map_err(|e| SimError::new(400, "bad_request", e))?;
            }
            let s = k2_core::sim::limits::load_settings();
            let effective = s.effective(k2_core::disk_guard::box_mem().0);
            Ok(ok(&SettingsResp { settings: s, effective }))
        }
        _ => Ok(CliResponse::not_found()),
    }
}

/// `type --secret <name>` (M-A20): resolve the secret from the vault for
/// THIS workspace's agent, under the same rule as `k2 vault get`
/// (`Use::Api`: the secret must be granted to this workspace for API use).
/// The value goes straight into a [`SimText::secret`] for the lane's
/// input channel; it is never returned, echoed, logged or put in argv.
/// People type secrets themselves at the Mobile tab.
fn resolve_secret(c: &Caller, _ws: &Ws, name: &str) -> Result<SimText, SimError> {
    let name = name.trim();
    if name.is_empty() || name.len() > 200 || name.chars().any(char::is_control) {
        return Err(SimError::new(400, "sim_invalid", "--secret needs a vault secret's name"));
    }
    if !matches!(c, Caller::Agent(_)) {
        return Err(SimError::new(400, "sim_invalid", "--secret is for agents; type the value yourself at the Mobile tab"));
    }
    let vault_err = |e: k2_vault::VaultError| SimError::new(e.status, e.code, format!("vault: {}", e.hint));
    let vault = crate::vault::get().map_err(vault_err)?;
    let vc = crate::vault::actor::from_caller(c.clone(), None).map_err(vault_err)?;
    let sel = crate::vault_routes::selector(
        &vc,
        // No `workspace`: an agent's selector defaults to its own (the
        // route already refused any other workspace).
        &crate::vault_routes::Sel { name: Some(name.to_string()), ..Default::default() },
    )
    .map_err(vault_err)?;
    let who = vc.requester().map_err(vault_err)?;
    let r = k2_vault::SecretStore::resolve(&*vault, &sel, k2_vault::Use::Api, &who).map_err(vault_err)?;
    let value = std::str::from_utf8(r.value.expose())
        .map_err(|_| SimError::new(400, "sim_invalid", format!("secret '{name}' is not text, so it can't be typed")))?;
    if value.chars().count() > 4096 || value.chars().any(|ch| ch.is_control() && ch != '\n' && ch != '\t') {
        return Err(SimError::new(400, "sim_invalid", format!("secret '{name}' has characters a device can't take as typing")));
    }
    Ok(SimText::secret(value))
}

// ── the owner's licence card ─────────────────────────────────────────

/// Put "Accept the Android SDK licence on this server" on the workspace's
/// Thread (for a person; only the box owner's answer counts). Deduped per
/// lane + licence, so repeated boots don't stack cards.
fn present_licence_card(ws: &Ws, e: &SimError) {
    let Some((lane, id, title)) = e.needs.iter().find_map(|n| match n {
        LaneNeed::Licence { id, title } => Some((licence_lane(e), id.clone(), title.clone())),
        _ => None,
    }) else {
        return;
    };
    let Some(lane) = lane else { return };
    let text = match manager::manager().licence(lane, &id) {
        Ok(t) => t,
        Err(err) => {
            k2_core::log_debug!("[sim] licence card: {}", err.message);
            return;
        }
    };
    let Some(conversation) = pinned_conversation(&ws.id) else { return };
    let url = text.url.clone().unwrap_or_default();
    let prompt = format!(
        "{} wants to run an {} device on this server. That needs the server's owner to accept the {} (sha1 {}){}. \
         Accepting downloads the {} tools and one system image (about 5 GB) into ~/.k2/sim and lets agents with \
         \"Allow mobile devices\" boot devices here. Only the owner's answer counts.",
        ws.handle,
        lane.platform(),
        title,
        &text.sha1[..12.min(text.sha1.len())],
        if url.is_empty() { String::new() } else { format!(": {url}") },
        lane.platform(),
    );
    let action = k2_core::overlay::CardAction {
        id: CARD_ACTION.to_string(),
        params: json!({ "lane": lane, "licence": id, "sha1": text.sha1 }),
        dedupe: format!("sim-licence-{lane}-{id}"),
        result: None,
        view: None,
    };
    match crate::overlay_routes::present_action_card(
        Some(&conversation),
        &ws.id,
        &prompt,
        &[CARD_ACCEPT, CARD_NOT_NOW],
        action,
        true,
        k2_core::overlay::now_secs() + k2_core::overlay::DEFAULT_CARD_EXPIRY_SECS,
        crate::overlay_routes::CardAudience::AgentPost,
    ) {
        Ok(_) => {}
        Err(err) => k2_core::log_debug!("[sim] licence card on {}: {err}", ws.handle),
    }
}

/// Which lane a `sim_needs_setup` came from: the first lane whose probe
/// lists that need.
fn licence_lane(e: &SimError) -> Option<LaneKind> {
    let ids: Vec<&str> = e.needs.iter().filter_map(|n| if let LaneNeed::Licence { id, .. } = n { Some(id.as_str()) } else { None }).collect();
    manager::manager()
        .probes()
        .into_iter()
        .find(|p| p.needs.iter().any(|n| matches!(n, LaneNeed::Licence { id, .. } if ids.contains(&id.as_str()))))
        .map(|p| p.lane)
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

/// The card's answer (`action_cards::run`). `by` is `owner` or a login
/// name; only the box owner may accept.
pub fn run_card(params: &Value, answer: &str, by: &str) -> Result<Value, String> {
    if answer != CARD_ACCEPT {
        return Ok(json!({ "ok": true, "message": "Not accepted. Devices stay off until the owner accepts." }));
    }
    let who = if by == "owner" {
        "owner".to_string()
    } else if is_owner_login(by) {
        format!("login:{by}")
    } else {
        return Err("Only the server's owner can accept this licence.".to_string());
    };
    let lane = params.get("lane").and_then(Value::as_str).and_then(LaneKind::parse).ok_or("the card has no lane")?;
    let id = params.get("licence").and_then(Value::as_str).ok_or("the card has no licence id")?;
    let sha = params.get("sha1").and_then(Value::as_str).ok_or("the card has no licence hash")?;
    manager::manager().start_setup(lane, Some((id, sha)), &who).map_err(|e| e.message)?;
    Ok(json!({ "ok": true, "message": format!("Accepted. K2 is downloading the {} tools; devices can boot when it finishes.", lane.platform()) }))
}

// ── daemon lifecycle ─────────────────────────────────────────────────

/// At daemon start: register this OS's lanes, wire the event hook, probe
/// (for the boot-status key), re-adopt recorded devices, and start the
/// reaper.
pub fn start() {
    k2_core::sim::register_builtin_lanes();
    manager::set_event_hook(crate::session_events::emit_sim);
    std::thread::Builder::new()
        .name("k2-sim-reaper".into())
        .spawn(|| {
            let m = manager::manager();
            let probes = m.refresh_probes();
            for p in &probes {
                k2_core::log_debug!("[sim] lane {} usable={} ready={} {}", p.lane, p.usable, p.ready, p.detail.clone().unwrap_or_default());
            }
            for line in m.adopt_all() {
                k2_core::log_debug!("[sim] {line}");
            }
            m.measure_disk();
            let mut ticks = 0u64;
            loop {
                std::thread::sleep(std::time::Duration::from_secs(30));
                ticks += 1;
                for line in m.sweep(&ws_for_project) {
                    k2_core::log_debug!("[sim] {line}");
                }
                if ticks % 20 == 0 {
                    m.measure_disk();
                }
            }
        })
        .map(|_| ())
        .unwrap_or_else(|e| k2_core::log_debug!("[sim] WARN reaper thread: {e}"));
}

fn ws_for_project(project_id: &str) -> Option<Ws> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let path: String = conn.query_row("SELECT path FROM projects WHERE id = ?1", [project_id], |r| r.get(0)).ok()?;
    drop(conn);
    Some(Ws { id: project_id.to_string(), handle: handle_of(project_id), path: std::path::PathBuf::from(path) })
}

/// The boot-status key (M-A13): at least one lane can run here.
pub fn boot_status_key() -> Option<&'static str> {
    manager::manager().any_lane_usable().then_some(k2_core::sim::BOOT_STATUS_KEY)
}

#[cfg(test)]
#[path = "sim_routes_tests.rs"]
mod tests;
