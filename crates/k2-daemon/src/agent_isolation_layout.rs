//! Agent Isolation M2, B4: the owner's switch for root-owned agent scopes
//! (`design-agent-isolation-m2-b4-nftables` "What the B4 code does":
//! `agents setup` runs only on an owner action).
//!
//! `k2 agent isolation layout on|off|status`, over
//! `POST /cli/agent-isolation/layout {mode: "on"|"off"|"status", confirmToken?}`
//! and `GET /cli/agent-isolation/layout` (status):
//!
//! - **status** — the owner token, an Owner login or an Admin login:
//!   `agents layout` (the helper's report), whether scopes are being placed
//!   now, the owner's setting, and what `/boot-status` says (why off, the
//!   P1 answer, `kernel.pid_max`, sessions still in the old layout).
//! - **on** — the owner only, with this computer's owner check
//!   (`presence::ISOLATION_LAYOUT`: a token from `/cli/vault/confirm/finish`,
//!   or this computer's own prompt where it has one). Then `agents setup`,
//!   `agents layout` (must be ready, P1 refused), the setting, and scope
//!   placement from now on: new sessions, job runs and services each get a
//!   scope at their next start; running sessions keep their place. An
//!   Admin login can't give the owner check, so it can't turn this on. An
//!   agent asking gets the `agent.isolation-layout` card's line.
//! - **off** — the owner only (it loosens isolation; as with the folder
//!   fence, an Admin may only tighten). Refused while a fenced session runs
//!   in a scope (it depends on the fenced slice). Else no new session gets a
//!   scope from now on, the setting is off, and with no scope live the
//!   slices are removed (`agents teardown`); with unfenced scopes still live
//!   the slices stay and the next daemon start with none live removes them.
//!
//! Every change and refusal of on/off is audited (`agent.isolation_layout`).
//! The owner's setting lives in its own file
//! (`k2_core::agent_scope::owner_choice`): the generic settings route can't
//! write it.

use std::collections::HashMap;

use k2_core::agent_scope::{self, OwnerChoice};
use k2_core::brand::CLI_NAME;
use serde_json::{json, Value};

use crate::cli_response::CliResponse;
use crate::sidecar_routes::Caller;
use crate::vault_routes::presence;

pub const ROUTE: &str = "/cli/agent-isolation/layout";
pub const MAX_BODY: usize = 4 * 1024;

/// The audit event of every on/off.
const AUDIT_EVENT: &str = "agent.isolation_layout";

/// Who asks, at their current role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Who {
    /// The owner token (a terminal outside K2, the desktop app).
    Owner,
    /// An Owner-role Connect login.
    OwnerLogin(String),
    /// An Admin login.
    Admin(String),
    /// A session passport (agent or shell tab), an App pass or an API key.
    NotAPerson,
}

impl Who {
    fn audit_user(&self) -> String {
        match self {
            Who::Owner => "owner".into(),
            Who::OwnerLogin(u) => format!("owner:{u}"),
            Who::Admin(u) => format!("admin:{u}"),
            Who::NotAPerson => "passport".into(),
        }
    }
}

/// What the route needs from the root helper and the daemon's placement,
/// so the tests run on stubs (never a root call, never this box's placement).
pub(crate) trait Host {
    /// `Err(why)` unless the helper serves `agents scope` here (protocol 19,
    /// `namespaces.agentscope`).
    fn helper_ok(&self) -> Result<(), String>;
    /// One `agents <verb>` call (daemon class): its JSON answer.
    fn agents(&self, verb: &str) -> Result<Value, String>;
    /// New sessions get scopes from now on (`layout`: the ready report).
    fn start_placing(&self, layout: &Value);
    /// No new session gets a scope from now on.
    fn stop_placing(&self);
    /// Scopes are being placed now.
    fn placing(&self) -> bool;
}

/// This daemon: the root helper and `k2_core::agent_scope`.
struct Live;

impl Host for Live {
    fn helper_ok(&self) -> Result<(), String> {
        let st = crate::root_helper::state();
        if crate::agent_isolation::agentscope_ok(&st) {
            Ok(())
        } else {
            Err(crate::agent_isolation::scope_helper_needed(&st))
        }
    }

    fn agents(&self, verb: &str) -> Result<Value, String> {
        let out = crate::root_helper::call(&["agents".to_string(), verb.to_string()], None)?;
        serde_json::from_slice(&out).map_err(|e| format!("agents {verb}: not JSON: {e}"))
    }

    fn start_placing(&self, layout: &Value) {
        // The running sessions' allowance first, then the placer: a session
        // in the old layout is never refused between the two.
        let mut st = agent_scope::box_state().unwrap_or_default();
        st.why_off = None;
        crate::agent_isolation::note_layout(&mut st, layout);
        st.old_layout = crate::agent_isolation::running_in_old_layout(layout);
        agent_scope::set_box_state(st);
        crate::agent_isolation::install_scope_caller();
        k2_core::log_debug!("[agents] the owner turned scopes on: the root helper places each new agent session, job run and service in its own scope");
    }

    fn stop_placing(&self) {
        let was = agent_scope::clear_caller();
        let mut st = agent_scope::box_state().unwrap_or_default();
        st.why_off = Some(crate::agent_isolation::WHY_OFF_BY_OWNER.to_string());
        agent_scope::set_box_state(st);
        k2_core::log_debug!("[agents] the owner turned scopes off (were placing: {was}): new sessions get no scope");
    }

    fn placing(&self) -> bool {
        agent_scope::scope_placement()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    On,
    Off,
    Status,
}

fn status_line(code: u16) -> &'static str {
    match code {
        200 => "200 OK",
        400 => "400 Bad Request",
        401 => "401 Unauthorized",
        403 => "403 Forbidden",
        405 => "405 Method Not Allowed",
        409 => "409 Conflict",
        413 => "413 Payload Too Large",
        503 => "503 Service Unavailable",
        _ => "500 Internal Server Error",
    }
}

fn answer(code: u16, v: Value) -> CliResponse {
    CliResponse { status: status_line(code), content_type: "application/json", body: v.to_string() }
}

fn refuse(code: u16, err: &str, hint: impl Into<String>) -> CliResponse {
    answer(code, json!({ "error": { "code": err, "hint": hint.into() } }))
}

/// The owner token, a login at its current role, or not a person.
fn who_of(caller: &Caller) -> Result<Who, CliResponse> {
    match caller {
        Caller::Owner => Ok(Who::Owner),
        Caller::Login { username } => match k2_core::connect_users::role_for_user(username) {
            Some(k2_core::connect_users::Role::Owner) => Ok(Who::OwnerLogin(username.clone())),
            Some(k2_core::connect_users::Role::Admin) => Ok(Who::Admin(username.clone())),
            _ => Err(refuse(403, "owner_only", "the agent scope layout is the server owner's (an admin may read its status)")),
        },
        Caller::Shell { .. } | Caller::Agent(_) | Caller::WrongCredential => Ok(Who::NotAPerson),
        Caller::Invalid => Err(refuse(401, "unauthorized", "the owner token or an owner or admin Connect login")),
    }
}

/// An agent's (or any passport's) refusal: the walls map adds the
/// `agent.isolation-layout` card's line to it for `on`.
fn not_a_person() -> CliResponse {
    refuse(
        403,
        "owner_only",
        format!(
            "the agent scope layout is the server's owner's to change, with this computer's owner check, from a terminal of their own ({CLI_NAME} agent isolation layout on|off). Agents: don't change logins or settings for it; ask your owner"
        ),
    )
}

/// The mode: a GET is status only (a GET naming `on` or `off` is 405); a
/// POST says it in its JSON body.
fn parse(is_post: bool, body: &[u8], params: &HashMap<String, String>) -> Result<(Mode, Option<String>), CliResponse> {
    let mode_of = |m: &str| match m {
        "on" => Some(Mode::On),
        "off" => Some(Mode::Off),
        "status" => Some(Mode::Status),
        _ => None,
    };
    if !is_post {
        return match params.get("mode").map(String::as_str) {
            None | Some("status") => Ok((Mode::Status, None)),
            Some("on" | "off") => Err(answer(405, json!({ "error": { "code": "method_not_allowed", "hint": "turning scopes on or off is a POST" } }))),
            Some(other) => Err(refuse(400, "bad_request", format!("mode is on, off or status (got {other:?})"))),
        };
    }
    let v: Value = serde_json::from_slice(body).map_err(|_| refuse(400, "bad_request", "the body is JSON: {\"mode\": \"on\" | \"off\" | \"status\"}"))?;
    let m = v.get("mode").and_then(Value::as_str).unwrap_or("");
    let mode = mode_of(m).ok_or_else(|| refuse(400, "bad_request", format!("mode is on, off or status (got {m:?})")))?;
    let token = v.get("confirmToken").and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty()).map(str::to_string);
    Ok((mode, token))
}

/// The route (TCP; `serve_tcp`), on this daemon.
pub fn handle(caller: Caller, is_post: bool, body: &[u8], params: &HashMap<String, String>, ingress: &str) -> CliResponse {
    handle_with(&Live, caller, is_post, body, params, ingress)
}

pub(crate) fn handle_with(host: &dyn Host, caller: Caller, is_post: bool, body: &[u8], params: &HashMap<String, String>, ingress: &str) -> CliResponse {
    let who = match who_of(&caller) {
        Ok(w) => w,
        Err(r) => return r,
    };
    let (mode, token) = match parse(is_post, body, params) {
        Ok(x) => x,
        Err(r) => return r,
    };
    match mode {
        Mode::Status => status(host, &who),
        Mode::On => turn_on(host, &who, token.as_deref(), ingress),
        Mode::Off => turn_off(host, &who, ingress),
    }
}

/// The `agent.isolation-layout` card's Allow: the same entry as the person
/// who clicked, at their current role, with the confirm token the answer
/// carried (if any). The card's click is not this computer's owner check:
/// the route asks for it again (this computer's own prompt where it has one,
/// else the token), and refuses without it.
pub(crate) fn card_entry(who: &crate::permission::actions::Answerer, confirm: Option<&str>) -> CliResponse {
    let w = match who {
        crate::permission::actions::Answerer::OwnerToken => Who::Owner,
        crate::permission::actions::Answerer::Login { user, .. } => match k2_core::connect_users::role_for_user(user) {
            Some(k2_core::connect_users::Role::Owner) => Who::OwnerLogin(user.clone()),
            Some(k2_core::connect_users::Role::Admin) => Who::Admin(user.clone()),
            _ => return refuse(403, "owner_only", "only the server's owner turns on agent scopes"),
        },
    };
    turn_on(&Live, &w, confirm, "permission-card")
}

fn audit(who: &Who, outcome: impl Into<String>, ingress: &str) {
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(AUDIT_EVENT, &who.audit_user(), outcome, ingress, "-", "cli"));
}

/// The vault caller this computer's owner check is made for.
fn vault_caller(who: &Who) -> crate::vault::actor::VaultCaller {
    use crate::vault::actor::{Channel, VaultCaller};
    match who {
        Who::OwnerLogin(u) => VaultCaller { actor: k2_vault::Actor::Login { username: u.clone(), role: k2_vault::Role::Owner }, channel: Channel::Login, session_id: None },
        _ => VaultCaller { actor: k2_vault::Actor::Owner, channel: Channel::Plain, session_id: None },
    }
}

/// This computer's owner check for turning scopes on (mirrors
/// `connect_users_routes::owner_login_gate`): refused with the confirm the
/// CLI gets (`confirm.action`, `confirm.secretId`).
fn owner_check(who: &Who, token: Option<&str>, ingress: &str) -> Result<(), CliResponse> {
    presence::isolation_layout_change(&vault_caller(who), token, ingress).map_err(|e| {
        answer(
            e.status,
            json!({
                "error": { "code": e.code, "hint": e.hint },
                "confirm": { "action": presence::ISOLATION_LAYOUT, "secretId": presence::ISOLATION_LAYOUT_BINDING },
            }),
        )
    })
}

fn by_of(who: &Who) -> String {
    who.audit_user()
}

fn turn_on(host: &dyn Host, who: &Who, token: Option<&str>, ingress: &str) -> CliResponse {
    match who {
        Who::Owner | Who::OwnerLogin(_) => {}
        Who::Admin(_) => {
            audit(who, "refused on: admin", ingress);
            return refuse(
                403,
                "owner_only",
                format!(
                    "only the server's owner turns on a root-owned scope for every agent session: it takes this computer's owner check, which an admin login can't give. Ask the owner to run {CLI_NAME} agent isolation layout on (an admin may read {CLI_NAME} agent isolation layout status)"
                ),
            );
        }
        Who::NotAPerson => {
            audit(who, "refused on: not a person", ingress);
            return not_a_person();
        }
    }
    if host.placing() && agent_scope::owner_choice() == OwnerChoice::On {
        return answer(200, json!({ "ok": true, "changed": false, "placing": true, "setting": "on", "message": "already on: every new agent session gets its own root-owned scope" }));
    }
    // Before this computer's prompt: a box that can't place scopes says so
    // without asking the owner for anything.
    if let Err(why) = host.helper_ok() {
        audit(who, format!("refused on: {why}"), ingress);
        return refuse(409, "helper_needed", why);
    }
    if let Err(r) = owner_check(who, token, ingress) {
        audit(who, "refused on: no owner check", ingress);
        return r;
    }
    let setup = match host.agents("setup") {
        Ok(v) => v,
        Err(e) => {
            audit(who, format!("refused on: agents setup: {e}"), ingress);
            return refuse(409, "setup_failed", format!("agents setup: {e}. Nothing was turned on"));
        }
    };
    let layout = match host.agents("layout") {
        Ok(v) => v,
        Err(e) => {
            audit(who, format!("refused on: agents layout: {e}"), ingress);
            return refuse(409, "layout_unreadable", format!("agents setup ran, but agents layout failed: {e}. Nothing places scopes; run {CLI_NAME} agent isolation layout status"));
        }
    };
    if let Err(why) = crate::agent_isolation::layout_ready(&layout) {
        audit(who, format!("refused on: layout not ready: {why}"), ingress);
        return answer(
            409,
            json!({
                "error": {
                    "code": "layout_not_ready",
                    "hint": format!("{why}. The slices agents setup made stay in place, and nothing places scopes; {CLI_NAME} agent isolation layout status shows the report"),
                },
                "setup": setup,
                "layout": layout,
            }),
        );
    }
    if let Err(e) = agent_scope::set_owner_choice(true, &by_of(who)) {
        audit(who, format!("refused on: the setting: {e}"), ingress);
        return refuse(500, "store_unavailable", format!("the layout is ready, but the setting could not be kept: {e}. Nothing places scopes"));
    }
    host.start_placing(&layout);
    let old = crate::agent_isolation::running_in_old_layout(&layout).len();
    audit(who, format!("ok on (agents running in the old layout: {old})"), ingress);
    answer(
        200,
        json!({
            "ok": true,
            "changed": true,
            "placing": true,
            "setting": "on",
            "appliesTo": "new sessions, job runs and services: each gets its own root-owned scope at its next start. Running sessions keep their place until they restart",
            "agentsInOldLayout": old,
            "setup": setup,
            "layout": layout,
        }),
    )
}

/// One live scope from `agents layout`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct LiveScope {
    ws: String,
    id: String,
    fenced: bool,
}

fn live_scopes(layout: &Value) -> Vec<LiveScope> {
    layout["scopes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| {
            Some(LiveScope { ws: s["ws"].as_str()?.to_string(), id: s["id"].as_str()?.to_string(), fenced: s["kind"].as_str() == Some("fenced") })
        })
        .collect()
}

/// The agent's name for a workspace id, else the id.
fn handle_of(ws: &str) -> String {
    k2_core::workspace_session_handles::workspace_address_name_shared(ws).unwrap_or_else(|_| ws.to_string())
}

fn turn_off(host: &dyn Host, who: &Who, ingress: &str) -> CliResponse {
    match who {
        Who::Owner | Who::OwnerLogin(_) => {}
        Who::Admin(_) => {
            audit(who, "refused off: admin", ingress);
            return refuse(
                403,
                "owner_only",
                format!(
                    "only the server's owner turns agent scopes off: it loosens agent isolation (as with an agent's folder fence, an admin may only tighten). An admin may read {CLI_NAME} agent isolation layout status"
                ),
            );
        }
        Who::NotAPerson => {
            audit(who, "refused off: not a person", ingress);
            return not_a_person();
        }
    }
    let layout = match host.helper_ok() {
        Ok(()) => match host.agents("layout") {
            Ok(v) => Some(v),
            Err(e) => {
                audit(who, format!("refused off: agents layout: {e}"), ingress);
                return refuse(409, "layout_unreadable", format!("agents layout: {e}. Nothing was changed: the live scopes can't be listed"));
            }
        },
        Err(why) if host.placing() => {
            audit(who, format!("refused off: {why}"), ingress);
            return refuse(409, "helper_needed", format!("{why}. Nothing was changed: scopes are being placed, and the live ones can't be listed without the helper"));
        }
        Err(_) => None,
    };
    let scopes = layout.as_ref().map(live_scopes).unwrap_or_default();
    let fenced: Vec<Value> = scopes
        .iter()
        .filter(|s| s.fenced)
        .map(|s| json!({ "agent": handle_of(&s.ws), "workspaceId": s.ws, "scope": format!("k2-agent-{}:{}.scope", s.ws, s.id) }))
        .collect();
    if !fenced.is_empty() {
        audit(who, format!("refused off: {} fenced scope(s) live", fenced.len()), ingress);
        let names: Vec<String> = fenced.iter().map(|f| format!("{} ({})", f["agent"].as_str().unwrap_or(""), f["scope"].as_str().unwrap_or(""))).collect();
        return answer(
            409,
            json!({
                "error": {
                    "code": "fenced_scopes_live",
                    "hint": format!(
                        "{} fenced agent session(s) run in a scope, and a fenced session depends on the fenced slice: {}. End them first (restart or stop those agents), then run {CLI_NAME} agent isolation layout off again. Nothing was changed",
                        fenced.len(),
                        names.join(", ")
                    ),
                    "fenced": fenced,
                },
            }),
        );
    }
    if let Err(e) = agent_scope::set_owner_choice(false, &by_of(who)) {
        audit(who, format!("refused off: the setting: {e}"), ingress);
        return refuse(500, "store_unavailable", format!("the setting could not be kept: {e}. Nothing was changed"));
    }
    let was_placing = host.placing();
    host.stop_placing();
    let mut out = json!({
        "ok": true,
        "changed": true,
        "placing": false,
        "setting": "off",
        "wasPlacing": was_placing,
        "appliesTo": "new sessions, job runs and services start without a scope from now on (the layout from before; the next daemon start says which in k2 app doctor)",
        "liveScopes": scopes.len(),
    });
    match &layout {
        Some(l) if crate::agent_isolation::teardown_due(l) => match host.agents("teardown") {
            Ok(v) => {
                out["teardown"] = v;
                out["message"] = json!("no scope was live: the agent slices were removed (agents teardown)");
            }
            Err(e) => {
                out["teardownError"] = json!(e);
                out["message"] = json!("no scope was live, but agents teardown failed; the next daemon start tries again");
            }
        },
        Some(_) if !scopes.is_empty() => {
            out["message"] = json!(format!(
                "{} unfenced session(s) keep their scopes until they end. The agent slices stay until the next daemon start with none live, which removes them",
                scopes.len()
            ));
        }
        _ => {
            out["message"] = json!("the agent slices aren't set up: nothing to remove");
        }
    }
    let outcome = format!("ok off (scopes live: {}, teardown: {})", scopes.len(), out.get("teardown").map_or("no", |_| "yes"));
    audit(who, outcome, ingress);
    answer(200, out)
}

fn status(host: &dyn Host, who: &Who) -> CliResponse {
    if *who == Who::NotAPerson {
        return refuse(403, "owner_only", "the agent scope layout's status is for the server's owner or an admin");
    }
    let (layout, layout_error) = match host.helper_ok().and_then(|()| host.agents("layout")) {
        Ok(v) => (Some(v), None),
        Err(e) => (None, Some(e)),
    };
    let placing = host.placing();
    let choice = agent_scope::owner_choice();
    let b = agent_scope::box_state().unwrap_or_default();
    let why_off = if placing {
        None
    } else {
        b.why_off.clone().or_else(|| {
            Some(match choice {
                OwnerChoice::NotSet => crate::agent_isolation::why_off_not_turned_on(),
                OwnerChoice::Off => crate::agent_isolation::WHY_OFF_BY_OWNER.to_string(),
                OwnerChoice::On => format!("turned on, but not placing scopes on this daemon ({CLI_NAME} app doctor says why)"),
            })
        })
    };
    answer(
        200,
        json!({
            "ok": true,
            "placing": placing,
            "setting": choice.as_str(),
            "whyOff": why_off,
            "p1": b.p1,
            "pidMax": b.pid_max,
            "oldLayout": b.old_layout.iter().map(|(ws, why)| json!({ "agent": ws, "why": why })).collect::<Vec<_>>(),
            "layout": layout,
            "layoutError": layout_error,
        }),
    )
}

/// The TCP door's arm for [`ROUTE`] (`routes/dispatcher.rs`). `true`: the
/// dispatcher ends the connection (an over-cap body).
pub(crate) async fn serve_tcp(
    stream: &mut tokio::net::TcpStream,
    buf: &mut [u8],
    is_post: bool,
    path: &str,
    query: &str,
    bearer: Option<&str>,
    owner_token: &str,
) -> bool {
    use tokio::io::AsyncReadExt;
    let body = if is_post {
        match crate::routes::http::read_post_body_capped(&mut *stream, buf, MAX_BODY).await {
            Ok(b) => b,
            Err(refused) => {
                let r = refuse(413, "body_too_large", "request bodies here are at most 4 KiB");
                crate::routes::http::refuse_body_too_large(stream, &refused, r.status, r.content_type, &r.body).await;
                return true;
            }
        }
    } else {
        let _ = stream.read(buf).await;
        Vec::new()
    };
    let caller = crate::sidecar_routes::caller_from_tcp(ROUTE, query, bearer, owner_token);
    let params = crate::routes::http::parse_params(path, query);
    let r = tokio::task::spawn_blocking(move || handle(caller, is_post, &body, &params, "cli"))
        .await
        .unwrap_or_else(|e| CliResponse::internal_error(format!("worker join: {e}")));
    crate::routes::http::send_response(stream, r.status, r.content_type, &r.body).await;
    false
}

#[cfg(test)]
#[path = "agent_isolation_layout_tests.rs"]
mod tests;
