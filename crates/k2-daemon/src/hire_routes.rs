//! Agent hire doors (prd-agent-hire-templates-v1, "build A only": the
//! inline recipe through the v1 API). Every door calls `k2_core::hire`.
//!
//! ```text
//! /v1 (an API key with the hire scope, or the owner token):
//!   POST /v1/agents/hire           one agent or a team, all-or-nothing, idempotent
//!   GET  /v1/agents/instances      this key's hires (?instance=<id> for one)
//!   POST /v1/agents/retire         tear down a hire this key made (`retire` scope)
//!   POST /v1/agent-teams/link      "this team serves that workspace" (`link` scope)
//!   POST /v1/agent-teams/unlink
//!   GET  /v1/agent-teams/links     ?instance=<id> | ?workspace=<ws>
//! /cli (owner token, Connect logins, agent passports where noted):
//!   POST /cli/agents/hire          `k2 agent hire <dir>` (converge one path; passports too:
//!                                  the main session, with "Let this agent hire agents" on)
//!   GET  /cli/agents/hire-root     the owner-set hire root; POST sets it (Owner)
//!   POST /cli/agent-instances/hire  the recipe (Admin)
//!   GET  /cli/agent-instances       every hire (Admin)
//!   POST /cli/agent-instances/retire (Admin)
//!   POST /cli/agent-teams/link|unlink (Admin)
//!   GET  /cli/agent-teams/links     (Member; a passport sees its own workspace)
//!   GET  /cli/msg/who               sender check for a delivery id (Member; passports for their own)
//!   GET  /cli/msg/verify            the same plus the content check (`crate::msg_verify`, 0.46.16)
//! ```
//!
//! A key without the hire scope gets the uniform `/v1` 404 on every hire
//! path. Agents never hire teams, link, unlink or retire through these
//! routes (HT14). Every hire, link, unlink, retire and hire-key mint writes
//! a `hire_events` row and one `auth-audit.jsonl` line (HT19).

use std::collections::HashMap;

use k2_core::connect_users::Role;
use k2_core::hire::apply::{self, RetireOutcome};
use k2_core::hire::converge::{self, ConvergeAuthority, ConvergeOutcome, ConvergeRequest};
use k2_core::hire::limits::HireLimits;
use k2_core::hire::plan::PlanContext;
use k2_core::hire::{ledger, links, HireCaller, HireError};

use crate::cli_response::CliResponse;
use crate::compute_routes::Who;
use crate::routes::http::V1Principal;

pub const MAX_BODY: usize = 2 * 1024 * 1024;

pub const V1_HIRE: &str = "/v1/agents/hire";
pub const V1_INSTANCES: &str = "/v1/agents/instances";
pub const V1_RETIRE: &str = "/v1/agents/retire";
pub const V1_LINK: &str = "/v1/agent-teams/link";
pub const V1_UNLINK: &str = "/v1/agent-teams/unlink";
pub const V1_LINKS: &str = "/v1/agent-teams/links";

pub const CLI_HIRE: &str = "/cli/agents/hire";
pub const CLI_HIRE_ROOT: &str = "/cli/agents/hire-root";
pub const CLI_INSTANCES: &str = "/cli/agent-instances";
pub const CLI_INSTANCES_HIRE: &str = "/cli/agent-instances/hire";
pub const CLI_INSTANCES_RETIRE: &str = "/cli/agent-instances/retire";
pub const CLI_LINK: &str = "/cli/agent-teams/link";
pub const CLI_UNLINK: &str = "/cli/agent-teams/unlink";
pub const CLI_LINKS: &str = "/cli/agent-teams/links";
pub const CLI_MSG_WHO: &str = "/cli/msg/who";
/// Agent templates (prd-agent-templates-v1 T1): the picker and one card.
/// Member floor; agent passports read them too (Kessel and leads hire).
pub const CLI_TEMPLATES: &str = "/cli/agent-templates";
pub const CLI_TEMPLATES_SHOW: &str = "/cli/agent-templates/show";

/// Every `/v1` hire path (exact).
pub const V1_PATHS: &[&str] = &[V1_HIRE, V1_INSTANCES, V1_RETIRE, V1_LINK, V1_UNLINK, V1_LINKS];
/// The POST ones (`V1_POST_EXACT`).
pub const V1_POST_PATHS: &[&str] = &[V1_HIRE, V1_RETIRE, V1_LINK, V1_UNLINK];

const CLI_PATHS: &[&str] = &[
    CLI_HIRE,
    CLI_HIRE_ROOT,
    CLI_INSTANCES,
    CLI_INSTANCES_HIRE,
    CLI_INSTANCES_RETIRE,
    CLI_LINK,
    CLI_UNLINK,
    CLI_LINKS,
    CLI_MSG_WHO,
    crate::msg_verify::PATH,
    CLI_TEMPLATES,
    CLI_TEMPLATES_SHOW,
];
const CLI_POST_ONLY: &[&str] = &[CLI_HIRE, CLI_INSTANCES_HIRE, CLI_INSTANCES_RETIRE, CLI_LINK, CLI_UNLINK];

pub fn is_route(p: &str) -> bool {
    CLI_PATHS.contains(&p) || crate::template_routes::PATHS.contains(&p)
}

pub fn is_post_route(p: &str) -> bool {
    CLI_POST_ONLY.contains(&p) || crate::template_routes::PATHS.contains(&p)
}

// ── responses ────────────────────────────────────────────────────────

pub(crate) fn status_line(code: u16) -> &'static str {
    match code {
        200 => "200 OK",
        400 => "400 Bad Request",
        403 => "403 Forbidden",
        404 => "404 Not Found",
        405 => "405 Method Not Allowed",
        409 => "409 Conflict",
        413 => "413 Payload Too Large",
        422 => "422 Unprocessable Entity",
        429 => "429 Too Many Requests",
        503 => "503 Service Unavailable",
        _ => "500 Internal Server Error",
    }
}

pub fn hire_error_response(e: &HireError) -> CliResponse {
    CliResponse { status: status_line(e.status), content_type: "application/json", body: e.body().to_string() }
}

fn ok(v: serde_json::Value) -> CliResponse {
    CliResponse::ok_json(v.to_string())
}

fn err(status: u16, code: &'static str, hint: impl Into<String>) -> CliResponse {
    hire_error_response(&HireError::new(status, code, hint))
}

/// The `/v1` uniform 404 (same body as an unknown `/v1` path).
fn v1_404() -> CliResponse {
    CliResponse::not_found()
}

pub fn too_large() -> CliResponse {
    err(413, "too_large", format!("hire requests are at most {MAX_BODY} bytes"))
}

fn json_body(body: &[u8]) -> Result<serde_json::Value, HireError> {
    if body.iter().all(|b| b.is_ascii_whitespace()) {
        return Ok(serde_json::json!({}));
    }
    serde_json::from_slice(body).map_err(|e| HireError::usage(format!("invalid JSON body: {e}")))
}

fn s<'a>(v: &'a serde_json::Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(|x| x.as_str()).map(str::trim).filter(|x| !x.is_empty())
}

// ── audit ────────────────────────────────────────────────────────────

#[cfg(test)]
pub(crate) static AUDIT_SINK: std::sync::Mutex<Vec<(String, String, String)>> = std::sync::Mutex::new(Vec::new());

/// One `auth-audit.jsonl` line (HT19). Unit tests capture instead of
/// writing into the real home.
pub(crate) fn audit(event: &str, actor: &str, outcome: &str) {
    #[cfg(test)]
    {
        AUDIT_SINK
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push((event.to_string(), actor.to_string(), outcome.to_string()));
    }
    #[cfg(not(test))]
    {
        k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
            event,
            actor,
            outcome.to_string(),
            "-",
            "-",
            "hire",
        ));
    }
}

// ── key helpers ──────────────────────────────────────────────────────

/// A key's hire envelope, or `None` (no door / unreadable → no hire).
fn key_limits(p: &k2_core::api_keys::ApiPrincipal) -> Option<HireLimits> {
    if !p.capabilities.hire {
        return None;
    }
    HireLimits::parse_stored(k2_core::api_keys::hire_limits_for(&p.id).as_deref())
}

fn key_caller(p: &k2_core::api_keys::ApiPrincipal) -> HireCaller {
    HireCaller::ApiKey { id: p.id.clone(), label: k2_core::api_keys::key_label(&p.id) }
}

/// Quota check for one new workspace on `POST /v1/w` (hire-scoped key).
pub(crate) fn check_single_quota(key_id: &str, l: &HireLimits) -> Result<(), HireError> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let day = ledger::members_created_since(&conn, key_id, now() - 86_400);
    if day.saturating_add(1) > l.max_members_per_day {
        return Err(HireError::new(429, "quota_exceeded", format!("maxMembersPerDay is {}", l.max_members_per_day)));
    }
    let live = ledger::live_members(&conn, key_id);
    if live.saturating_add(1) > l.max_live_members {
        return Err(HireError::new(429, "quota_exceeded", format!("maxLiveMembers is {}", l.max_live_members)));
    }
    Ok(())
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Ledger a workspace a key created on `POST /v1/w` (one-member instance),
/// so quotas, `reach` and retire see it.
pub(crate) fn ledger_single_hire(caller: &HireCaller, out: &ConvergeOutcome) {
    let id = format!("inst_{}", uuid::Uuid::new_v4().simple());
    let db = k2_core::db::shared();
    let conn = db.lock();
    let body = serde_json::json!({ "ok": true, "instance": id, "status": "active", "via": "/v1/w",
        "members": [{ "workspaceId": out.id, "handle": out.handle, "path": out.path, "lead": true, "role": "agent" }] });
    if ledger::insert_applying(&conn, &id, caller, None, "v1w", None, None, 1).is_ok() {
        let _ = ledger::add_member(&conn, &id, &out.id, "agent", &out.handle, &out.path, true);
        ledger::finish(&conn, &id, "active", None, Some(&body.to_string()));
        ledger::event(&conn, "agent-hire", caller, Some(&id), None, None, "ok", Some("/v1/w"), None);
    }
    drop(conn);
    audit("agent-hire", &caller.actor(), &format!("ok {id} /v1/w {}", out.handle));
}

// ── launch + first brief ─────────────────────────────────────────────

/// Start each launched member's chat session and deliver its first brief,
/// off the request thread. Failures are ledgered; the hire stands.
fn run_launches(jobs: Vec<apply::LaunchJob>, caller: HireCaller, instance: String, on_behalf_of: Option<String>) {
    if jobs.is_empty() {
        return;
    }
    // In the daemon's runtime: starting a session needs one (`crate::rt`).
    let instance_for_log = instance.clone();
    let spawned = crate::rt::spawn("hire-launch", move || {
        for job in jobs {
            let launched = crate::canonical_session::ensure_canonical_session(&job.path);
            let (result, reason) = match (&launched, job.brief.as_deref()) {
                (Err(e), _) => ("failed", Some(format!("launch {}: {e}", job.handle))),
                (Ok(_), None) => ("ok", None),
                (Ok(_), Some(brief)) => {
                    let from = match &caller {
                        HireCaller::ApiKey { label: Some(l), .. } => format!("app:{l}"),
                        HireCaller::ApiKey { id, .. } => format!("app:{}", &id[..id.len().min(8)]),
                        // 0.46.0 stamp rule: the linked username, else `owner`.
                        _ => k2_core::person_name::sender_identity_now(k2_core::person_name::Sender::OwnerToken),
                    };
                    let r = deliver_recorded(
                        &job.handle,
                        brief,
                        &from,
                        "",
                        true,
                        crate::workspace_msg::DEFAULT_WAKE_TIMEOUT,
                        "v1",
                        &Sender::from_hire_caller(&caller, on_behalf_of.clone()),
                    );
                    if r.0.success {
                        ("ok", None)
                    } else {
                        ("failed", Some(format!("brief {}: {}", job.handle, r.0.reason.clone().unwrap_or_default())))
                    }
                }
            };
            let db = k2_core::db::shared();
            let conn = db.lock();
            ledger::event(
                &conn,
                "agent-launch",
                &caller,
                Some(&instance),
                None,
                on_behalf_of.as_deref(),
                result,
                reason.as_deref(),
                Some(&serde_json::json!({ "handle": job.handle, "brief": job.brief.is_some() })),
            );
        }
    });
    if let Err(e) = spawned {
        k2_core::log_debug!("[hire] launches for {instance_for_log} not started: {e}");
    }
}

// ── recorded delivery (S7) ───────────────────────────────────────────

/// Who is sending, from the credential (never the body).
#[derive(Debug, Clone)]
pub struct Sender {
    pub workspace_id: Option<String>,
    pub via: &'static str,
    pub actor: String,
    pub on_behalf_of: Option<String>,
}

impl Sender {
    pub fn from_hire_caller(c: &HireCaller, on_behalf_of: Option<String>) -> Self {
        match c {
            HireCaller::ApiKey { .. } => Sender { workspace_id: None, via: "api-key", actor: c.actor(), on_behalf_of },
            HireCaller::Agent { workspace_id } => Sender {
                workspace_id: Some(workspace_id.clone()),
                via: "passport",
                actor: c.actor(),
                on_behalf_of: None,
            },
            _ => Sender { workspace_id: None, via: "owner", actor: c.actor(), on_behalf_of: None },
        }
    }
}

/// `deliver_live_with_via` plus a `msg_deliveries` row: the recipient's
/// prefix carries the delivery id (`[from x · m_…]`) so it can ask
/// `k2 msg who <id>` who really sent it.
#[allow(clippy::too_many_arguments)]
pub fn deliver_recorded(
    target: &str,
    text: &str,
    from: &str,
    command: &str,
    wake: bool,
    wake_timeout: std::time::Duration,
    via: &str,
    sender: &Sender,
) -> (crate::workspace_msg::MsgResponse, Option<String>) {
    deliver_recorded_with_summary(target, text, from, command, wake, wake_timeout, via, sender, None)
}

/// [`deliver_recorded`] with the sender's one-line summary (`k2 msg
/// --summary`), kept on the Chatter line, never injected.
#[allow(clippy::too_many_arguments)]
pub fn deliver_recorded_with_summary(
    target: &str,
    text: &str,
    from: &str,
    command: &str,
    wake: bool,
    wake_timeout: std::time::Duration,
    via: &str,
    sender: &Sender,
    summary: Option<&str>,
) -> (crate::workspace_msg::MsgResponse, Option<String>) {
    deliver_recorded_inner(target, text, from, command, wake, wake_timeout, via, sender, summary, &Outside::default())
}

/// What a delivery's record keeps about a sender outside this server
/// (0310): its address, its key's fingerprint, and a Postal message id.
#[derive(Debug, Clone, Default)]
pub struct Outside {
    /// A Postal address or a federated `agent::host`.
    pub address: Option<String>,
    pub fingerprint: Option<String>,
    /// The Postal message id: one row each (P5, P18).
    pub postal_msg_id: Option<String>,
    /// The label is a Postal one ([`crate::workspace_msg::Sender::Postal`]).
    pub postal: bool,
}

/// A Postal message through the `/v1` door (prd-postal-in-k2-v1 S0):
/// recorded like any delivery, typed in under K2's own `[postal <address> ·
/// m_…]` label with every body line quoted ([`crate::workspace_msg::Sender::Postal`]).
/// `address` is a parsed Postal address; it is never a command.
/// `postal_msg_id` (S2) holds the row to that one Postal message.
pub fn deliver_recorded_postal(
    target: &str,
    text: &str,
    address: &str,
    wake: bool,
    sender: &Sender,
    postal_msg_id: Option<&str>,
) -> (crate::workspace_msg::MsgResponse, Option<String>) {
    let timeout = crate::workspace_msg::DEFAULT_WAKE_TIMEOUT;
    let outside = Outside {
        address: Some(address.to_string()),
        fingerprint: None,
        postal_msg_id: postal_msg_id.map(str::to_string),
        postal: true,
    };
    deliver_recorded_inner(target, text, address, "", wake, timeout, "postal", sender, None, &outside)
}

/// A message from an agent on a paired server (federation), recorded with
/// `via = federation` and the peer's pinned key (0310, P28): its
/// `[from agent::host · m_…]` stamp can be checked like any other.
pub fn deliver_recorded_federated(
    target: &str,
    text: &str,
    from_label: &str,
    peer_fingerprint: &str,
) -> (crate::workspace_msg::MsgResponse, Option<String>) {
    let sender = Sender { workspace_id: None, via: "federation", actor: format!("federation:{from_label}"), on_behalf_of: None };
    let outside = Outside {
        address: Some(from_label.to_string()),
        fingerprint: Some(peer_fingerprint.to_string()),
        ..Outside::default()
    };
    let timeout = crate::workspace_msg::DEFAULT_WAKE_TIMEOUT;
    deliver_recorded_inner(target, text, from_label, "", true, timeout, "msg", &sender, None, &outside)
}

#[allow(clippy::too_many_arguments)]
fn deliver_recorded_inner(
    target: &str,
    text: &str,
    from: &str,
    command: &str,
    wake: bool,
    wake_timeout: std::time::Duration,
    via: &str,
    sender: &Sender,
    summary: Option<&str>,
    outside: &Outside,
) -> (crate::workspace_msg::MsgResponse, Option<String>) {
    let postal = if outside.postal { outside.address.as_deref() } else { None };
    let target_ws = crate::workspace_msg::resolve_workspace(target).and_then(|path| {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::workspace::agent_identity::resolve_project_id(&conn, &path)
    });
    let label = match postal {
        Some(address) => address.to_string(),
        None => crate::workspace_msg::humanize_from(from),
    };
    let id = k2_core::hire::deliveries::new_id();
    let recorded = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::hire::deliveries::record(
            &conn,
            &k2_core::hire::deliveries::Delivery {
                id: id.clone(),
                sender_workspace_id: sender.workspace_id.clone(),
                sender_label: Some(label.clone()),
                via: sender.via.to_string(),
                actor: Some(sender.actor.clone()),
                on_behalf_of: sender.on_behalf_of.clone(),
                target_workspace_id: target_ws,
                target: target.to_string(),
                delivered: false,
                at: 0,
                body_sha256: Some(k2_core::hire::deliveries::body_sha256(text)),
                postal_msg_id: outside.postal_msg_id.clone(),
                sender_address: outside.address.clone(),
                sender_fingerprint: outside.fingerprint.clone(),
                ..Default::default()
            },
        )
        .is_ok()
    };
    let resp = match (recorded, postal) {
        (true, Some(address)) => {
            let stamp = crate::workspace_msg::postal_label(address, &id);
            let sender = crate::workspace_msg::Sender::Postal(&stamp);
            crate::workspace_msg::deliver_live_with_receipt(target, text, sender, from, &id, "", wake, wake_timeout, via, summary)
        }
        (true, None) => {
            let from_with_id = format!("{label} · {id}");
            let sender = crate::workspace_msg::Sender::Named(&from_with_id);
            crate::workspace_msg::deliver_live_with_receipt(target, text, sender, from, &id, command, wake, wake_timeout, via, summary)
        }
        // A Postal message is never typed in without its record: the
        // label's id is how the agent checks it (`k2 msg who`).
        (false, Some(_)) => {
            let mut r = crate::workspace_msg::MsgResponse::fail(crate::workspace_msg::MsgReason::SpawnFailed);
            r.reason = Some("not_recorded".into());
            r.hint = Some("the delivery record couldn't be written, so nothing was typed in; try again".into());
            return (r, None);
        }
        (false, None) => crate::workspace_msg::deliver_live_with_via(target, text, &label, command, wake, wake_timeout, via),
    };
    if recorded {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::hire::deliveries::mark_delivered(&conn, &id, resp.success);
    }
    (resp, recorded.then_some(id))
}

// ── /v1 ──────────────────────────────────────────────────────────────

/// `/v1/agents/*` + `/v1/agent-teams/*`. The dispatcher already checked the
/// `/v1` surface switch and resolved the principal.
pub(crate) fn handle_v1(
    path: &str,
    principal: &V1Principal,
    is_post: bool,
    body: &[u8],
    params: &HashMap<String, String>,
    idem_header: Option<&str>,
) -> CliResponse {
    let (caller, limits, key_has_db) = match principal {
        V1Principal::Owner => (HireCaller::Owner, None, true),
        V1Principal::Api(p) => match key_limits(p) {
            Some(l) => (key_caller(p), Some(l), p.capabilities.db),
            None => return v1_404(),
        },
    };
    let wants_post = V1_POST_PATHS.contains(&path);
    if wants_post != is_post {
        return CliResponse::method_not_allowed();
    }
    match path {
        V1_HIRE => hire_recipe(body, idem_header, PlanContext { caller, limits, key_has_db }),
        V1_INSTANCES => list_instances(&caller, params),
        V1_RETIRE => {
            if let Some(l) = limits.as_ref() {
                if !l.retire {
                    return err(403, "exceeds_key", "this key has no retire scope");
                }
            }
            retire(body, &caller, false)
        }
        V1_LINK => {
            if limits.as_ref().is_some_and(|l| !l.link) {
                return err(403, "exceeds_key", "this key has no link scope");
            }
            link(body, &caller, limits.as_ref())
        }
        V1_UNLINK => {
            if limits.as_ref().is_some_and(|l| !l.link) {
                return err(403, "exceeds_key", "this key has no link scope");
            }
            unlink(body, &caller, limits.as_ref())
        }
        V1_LINKS => links_for(&caller, limits.as_ref(), params),
        _ => v1_404(),
    }
}

fn hire_recipe(body: &[u8], idem_header: Option<&str>, ctx: PlanContext) -> CliResponse {
    let raw = match json_body(body) {
        Ok(v) => v,
        Err(e) => return hire_error_response(&e),
    };
    let caller = ctx.caller.clone();
    match apply::instantiate(&raw, idem_header, &ctx) {
        Ok(inst) => {
            if inst.changed {
                crate::v1_hire::emit_hire_watches();
                let id = inst.body["instance"].as_str().unwrap_or_default().to_string();
                let handles: Vec<String> = inst.body["members"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|m| m["handle"].as_str().map(str::to_string)).collect())
                    .unwrap_or_default();
                audit("agent-hire", &caller.actor(), &format!("ok {id} {}", handles.join(",")));
                for l in inst.body["links"].as_array().into_iter().flatten() {
                    audit("agent-link", &caller.actor(), &format!("ok {} {id}", l["id"].as_str().unwrap_or("")));
                }
                let on_behalf = inst.body["onBehalfOf"]["value"].as_str().map(str::to_string);
                let mut body = inst.body.clone();
                if !inst.launches.is_empty() {
                    body["launch"] = serde_json::json!({
                        "queued": inst.launches.iter().map(|j| j.handle.clone()).collect::<Vec<_>>(),
                    });
                }
                run_launches(inst.launches, caller, id, on_behalf);
                ok(body)
            } else {
                ok(inst.body)
            }
        }
        Err(e) => {
            if e.status != 404 {
                audit("agent-hire", &caller.actor(), &format!("deny {}: {}", e.code, e.hint.chars().take(160).collect::<String>()));
            }
            hire_error_response(&e)
        }
    }
}

fn list_instances(caller: &HireCaller, params: &HashMap<String, String>) -> CliResponse {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let key_filter = match caller {
        HireCaller::ApiKey { id, .. } => Some(id.clone()),
        _ => params.get("key").map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
    };
    if let Some(id) = params.get("instance").map(|s| s.trim()).filter(|s| !s.is_empty()) {
        return match ledger::get(&conn, id) {
            Some(inst) if key_filter.as_deref().is_none_or(|k| inst.api_key_id.as_deref() == Some(k)) => {
                let events = ledger::events_for_instance(&conn, &inst.id);
                let links = links::links_of_instance(&conn, &inst.id);
                ok(serde_json::json!({ "ok": true, "instance": inst, "links": links, "events": events }))
            }
            _ => err(404, "not_found", format!("no hire instance {id}")),
        };
    }
    let limit = params.get("limit").and_then(|s| s.parse::<usize>().ok()).unwrap_or(100).clamp(1, 500);
    let list = ledger::list(&conn, key_filter.as_deref(), limit);
    ok(serde_json::json!({ "ok": true, "instances": list }))
}

fn retire_member(path: &str, force: bool) -> RetireOutcome {
    let body = serde_json::json!({ "q": path, "force": force }).to_string();
    let r = crate::agent_retire::handle_agent_retire(body.as_bytes());
    if r.status.starts_with("200") {
        return RetireOutcome::Retired;
    }
    if r.status.starts_with("404") {
        return RetireOutcome::Gone;
    }
    let v: serde_json::Value = serde_json::from_str(&r.body).unwrap_or_default();
    let hint = v["error"]["hint"]
        .as_str()
        .or_else(|| v["error"].as_str())
        .or_else(|| v["message"].as_str())
        .unwrap_or(&r.body)
        .chars()
        .take(400)
        .collect();
    RetireOutcome::Blocked(hint)
}

/// Teardown. `force` only for the owner/Admin on `/cli` (a key never forces, HT13).
fn retire(body: &[u8], caller: &HireCaller, allow_force: bool) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(e) => return hire_error_response(&e),
    };
    let Some(instance) = s(&v, "instance") else {
        return err(400, "usage", "missing 'instance'");
    };
    let force = allow_force && v.get("force").and_then(|x| x.as_bool()).unwrap_or(false);
    match apply::teardown(instance, caller, &|p| retire_member(p, force)) {
        Ok(out) => {
            crate::v1_hire::emit_hire_watches();
            audit("agent-retire", &caller.actor(), &format!("{} {instance}", out["status"].as_str().unwrap_or("")));
            ok(out)
        }
        Err(e) => hire_error_response(&e),
    }
}

/// The team side of a link: an instance id, or any workspace token.
struct TeamSide {
    instance_id: Option<String>,
    group_id: Option<String>,
    lead: String,
    members: Vec<String>,
}

fn resolve_team(conn: &rusqlite::Connection, token: &str) -> Option<TeamSide> {
    if token.starts_with("inst_") {
        let inst = ledger::get(conn, token)?;
        if inst.status != "active" {
            return None;
        }
        let live: Vec<&ledger::InstanceMember> = inst.members.iter().filter(|m| m.status == "live").collect();
        let lead = live.iter().find(|m| m.lead).or(live.first())?.workspace_id.clone();
        let members = live.iter().filter(|m| m.workspace_id != lead).map(|m| m.workspace_id.clone()).collect();
        return Some(TeamSide { instance_id: Some(inst.id), group_id: inst.group_id, lead, members });
    }
    let ws = links::resolve_workspace(conn, token)?;
    let instance_id = ledger::instance_of_workspace(conn, &ws.id);
    let (group_id, members) = match instance_id.as_deref().and_then(|i| ledger::get(conn, i)) {
        Some(inst) => (
            inst.group_id.clone(),
            inst.members.iter().filter(|m| m.status == "live" && m.workspace_id != ws.id).map(|m| m.workspace_id.clone()).collect(),
        ),
        None => (None, Vec::new()),
    };
    Some(TeamSide { instance_id, group_id, lead: ws.id, members })
}

fn link(body: &[u8], caller: &HireCaller, limits: Option<&HireLimits>) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(e) => return hire_error_response(&e),
    };
    let (Some(team_tok), Some(ws_tok)) = (s(&v, "team"), s(&v, "workspace")) else {
        return err(400, "usage", "pass {team: <instance id | workspace>, workspace: <the workspace it serves>, kind?, scope?}");
    };
    let kind = match links::parse_kind(s(&v, "kind")) {
        Ok(k) => k,
        Err(e) => return hire_error_response(&e),
    };
    let topology: &'static str = match s(&v, "topology") {
        None | Some("lead") => "lead",
        Some("mesh") => "mesh",
        Some(_) => return err(400, "usage", "topology: lead or mesh"),
    };
    let on_behalf_of = s(&v, "onBehalfOf").map(|o| o.chars().take(200).collect::<String>());
    let db = k2_core::db::shared();
    let conn = db.lock();
    let team = resolve_team(&conn, team_tok);
    let consumer = links::resolve_workspace(&conn, ws_tok);
    let (team, consumer) = match (caller, limits, team, consumer) {
        (HireCaller::ApiKey { id, .. }, Some(l), Some(t), Some(c))
            if k2_core::hire::plan::key_may_link(&conn, id, l, &t.lead)
                && k2_core::hire::plan::key_may_link(&conn, id, l, &c.id) =>
        {
            (t, c)
        }
        (HireCaller::ApiKey { .. }, _, _, _) => {
            return err(403, "exceeds_key", "this key may link only workspaces it created or the owner named (linkTargets)");
        }
        (_, _, Some(t), Some(c)) => (t, c),
        _ => return err(404, "not_found", "no such team or workspace (or the name is ambiguous)"),
    };
    let scope = v.get("scope").filter(|x| !x.is_null()).cloned();
    match links::create(
        &conn,
        caller,
        &links::NewLink {
            instance_id: team.instance_id.as_deref(),
            team_group_id: team.group_id.as_deref(),
            team_lead: &team.lead,
            team_members: team.members.clone(),
            consumer: &consumer.id,
            kind,
            topology,
            scope: scope.as_ref(),
            on_behalf_of: on_behalf_of.as_deref(),
        },
    ) {
        Ok((l, created)) => {
            if created {
                ledger::event(&conn, "agent-link", caller, l.instance_id.as_deref(), Some(&l.id), on_behalf_of.as_deref(), "ok", None, None);
                drop(conn);
                audit("agent-link", &caller.actor(), &format!("ok {}", l.id));
                refresh_rosters(&[&team.lead, &consumer.id]);
            }
            ok(serde_json::json!({ "ok": true, "changed": created, "link": l }))
        }
        Err(e) => hire_error_response(&e),
    }
}

fn refresh_rosters(ids: &[&str]) {
    let paths: Vec<String> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        ids.iter()
            .filter_map(|id| {
                conn.query_row("SELECT path FROM projects WHERE id = ?1", rusqlite::params![id], |r| r.get::<_, String>(0)).ok()
            })
            .collect()
    };
    let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
    k2_core::workspace::context_layers::refresh_roster_after_connection_change(&refs);
}

fn unlink(body: &[u8], caller: &HireCaller, limits: Option<&HireLimits>) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(e) => return hire_error_response(&e),
    };
    let Some(id) = s(&v, "link") else {
        return err(400, "usage", "missing 'link' (a link id, lnk_…)");
    };
    let db = k2_core::db::shared();
    let conn = db.lock();
    let Some(l) = links::get(&conn, id) else {
        return err(404, "not_found", format!("no link {id}"));
    };
    if let (HireCaller::ApiKey { id: key, .. }, Some(lim)) = (caller, limits) {
        let mine = l.api_key_id.as_deref() == Some(key.as_str())
            || (k2_core::hire::plan::key_may_link(&conn, key, lim, &l.team_lead_workspace_id)
                && k2_core::hire::plan::key_may_link(&conn, key, lim, &l.consumer_workspace_id));
        if !mine {
            return err(404, "not_found", format!("no link {id}"));
        }
    }
    match links::remove(&conn, &l.id) {
        Ok(changed) => {
            if changed {
                ledger::event(&conn, "agent-unlink", caller, l.instance_id.as_deref(), Some(&l.id), None, "ok", None, None);
                drop(conn);
                audit("agent-unlink", &caller.actor(), &format!("ok {}", l.id));
                refresh_rosters(&[&l.team_lead_workspace_id, &l.consumer_workspace_id]);
            }
            ok(serde_json::json!({ "ok": true, "changed": changed, "link": l.id }))
        }
        Err(e) => hire_error_response(&e),
    }
}

/// `GET …/links`: by instance, or by workspace (out = "served by", in =
/// "serves"). A key sees only its own instances/workspaces.
fn links_for(caller: &HireCaller, limits: Option<&HireLimits>, params: &HashMap<String, String>) -> CliResponse {
    let db = k2_core::db::shared();
    let conn = db.lock();
    if let Some(inst) = params.get("instance").map(|s| s.trim()).filter(|s| !s.is_empty()) {
        if let HireCaller::ApiKey { id, .. } = caller {
            if ledger::get(&conn, inst).and_then(|i| i.api_key_id).as_deref() != Some(id.as_str()) {
                return err(404, "not_found", format!("no hire instance {inst}"));
            }
        }
        return ok(serde_json::json!({ "ok": true, "instance": inst, "links": links::links_of_instance(&conn, inst) }));
    }
    let Some(tok) = params.get("workspace").map(|s| s.trim()).filter(|s| !s.is_empty()) else {
        return match caller {
            HireCaller::ApiKey { id, .. } => ok(serde_json::json!({ "ok": true, "links": links::links_of_key(&conn, id) })),
            _ => err(400, "usage", "pass ?instance=<id> or ?workspace=<ws>"),
        };
    };
    let Some(ws) = links::resolve_workspace(&conn, tok) else {
        return err(404, "not_found", format!("no workspace {tok}"));
    };
    if let (HireCaller::ApiKey { id, .. }, Some(l)) = (caller, limits) {
        if !k2_core::hire::plan::key_may_link(&conn, id, l, &ws.id) {
            return err(404, "not_found", format!("no workspace {tok}"));
        }
    }
    ok(team_view(&conn, &ws))
}

/// The "who serves me / who do I serve / my team" view of one workspace.
fn team_view(conn: &rusqlite::Connection, ws: &links::Ws) -> serde_json::Value {
    let instance = ledger::instance_of_workspace(conn, &ws.id).and_then(|i| ledger::get(conn, &i));
    serde_json::json!({
        "ok": true,
        "workspace": { "id": ws.id, "handle": ws.handle },
        "team": instance.map(|i| serde_json::json!({
            "instance": i.id,
            "name": i.team_name,
            "groupId": i.group_id,
            "members": i.members.into_iter().filter(|m| m.status == "live").collect::<Vec<_>>(),
        })),
        "servedBy": links::links_out(conn, &ws.id),
        "serves": links::links_in(conn, &ws.id),
    })
}

// ── /cli ─────────────────────────────────────────────────────────────

fn owner_only() -> CliResponse {
    err(403, "owner_only", "only a person can do this (owner token or an Owner/Admin login); an agent never hires teams, links or retires through K2")
}

fn is_admin(who: &Who) -> bool {
    matches!(who, Who::Owner | Who::Login { role: Role::Owner | Role::Admin, .. })
}

fn human_caller(who: &Who) -> Option<HireCaller> {
    match who {
        Who::Owner => Some(HireCaller::Owner),
        Who::Login { username, .. } => Some(HireCaller::Login { username: username.clone() }),
        _ => None,
    }
}

/// `/cli/*` hire routes. `who` is the classified caller.
pub fn handle_cli(
    path: &str,
    who: Who,
    is_post: bool,
    body: &[u8],
    params: &HashMap<String, String>,
    idem_header: Option<&str>,
) -> CliResponse {
    if is_post_route(path) && !is_post {
        return CliResponse::method_not_allowed();
    }
    if matches!(who, Who::Invalid | Who::Wrong) {
        return CliResponse::forbidden();
    }
    match path {
        CLI_HIRE => cli_converge(&who, body),
        CLI_HIRE_ROOT => hire_root(&who, is_post, body),
        CLI_MSG_WHO => msg_who(&who, params),
        crate::msg_verify::PATH => crate::msg_verify::handle(&who, is_post, params),
        // prd-agent-templates-v1 T6/T5: save, add, remove, upgrade, diff.
        p if crate::template_routes::PATHS.contains(&p) => crate::template_routes::handle(p, &who, is_post, body),
        CLI_TEMPLATES | CLI_TEMPLATES_SHOW => {
            if is_post {
                return CliResponse::method_not_allowed();
            }
            templates_read(path, params)
        }
        CLI_LINKS => {
            if is_post {
                return CliResponse::method_not_allowed();
            }
            match who.passport() {
                // An agent sees its own workspace's links only.
                Some((ws, _)) => {
                    let db = k2_core::db::shared();
                    let conn = db.lock();
                    match links::resolve_workspace(&conn, ws) {
                        Some(w) => ok(team_view(&conn, &w)),
                        None => err(404, "not_found", "your workspace isn't registered"),
                    }
                }
                None => links_for(&HireCaller::Owner, None, params),
            }
        }
        _ => {
            // Admin-floor team routes: people only.
            if !is_admin(&who) {
                return owner_only();
            }
            let caller = human_caller(&who).unwrap_or(HireCaller::Owner);
            match path {
                CLI_INSTANCES => {
                    if is_post {
                        return CliResponse::method_not_allowed();
                    }
                    list_instances(&caller, params)
                }
                CLI_INSTANCES_HIRE => hire_recipe(body, idem_header, PlanContext { caller, limits: None, key_has_db: true }),
                CLI_INSTANCES_RETIRE => retire(body, &caller, true),
                CLI_LINK => link(body, &caller, None),
                CLI_UNLINK => unlink(body, &caller, None),
                _ => CliResponse::not_found(),
            }
        }
    }
}

/// `POST /cli/agents/hire` — `k2 agent hire <dir>`'s daemon half: converge
/// one workspace at a path on the shared core. Owner, any login, or a
/// passport (in-cell): a passport never raises `dbAccess` (the 0.44.4
/// self-grant check, now daemon-side).
fn cli_converge(who: &Who, body: &[u8]) -> CliResponse {
    if let Err(r) = agent_hire_gate(who, body) {
        return r;
    }
    let caller = match who {
        Who::Owner => HireCaller::Owner,
        Who::Login { username, .. } => HireCaller::Login { username: username.clone() },
        Who::Agent { project_id, .. } | Who::Shell { project_id, .. } => {
            HireCaller::Agent { workspace_id: project_id.clone() }
        }
        _ => return CliResponse::forbidden(),
    };
    let req: ConvergeRequest = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => return err(400, "usage", format!("invalid JSON body: {e}")),
    };
    // 0.46.5 `--take-app` (prd-app-handoff-v1 §4.13, A14): never inline.
    // The hire converges first; only a hire that succeeded files the App's
    // handover to the new agent (auto-accepted, Q1). A failed hire files
    // nothing.
    let extra: serde_json::Value = serde_json::from_slice(body).unwrap_or(serde_json::Value::Null);
    let take_app = extra.get("takeApp").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let brief = extra.get("brief").and_then(|v| v.as_str()).map(str::to_string);
    // prd-agent-templates-v1 T2: a template hire runs `apply` (the one
    // front door: optional path, dry run as the review, the lock).
    if req.template.as_deref().is_some_and(|t| !t.trim().is_empty()) {
        return cli_template_hire(who, &caller, &req, &extra, take_app.as_deref(), brief.as_deref());
    }
    // A plain dry run (`dryRun: true`, no template; 0.46.16): the gate
    // above is the whole answer. It refused already (hire_off / main_only,
    // taught like the real hire) or the real hire would pass it. Nothing
    // else in the body is read and nothing is written; `k2 agent hire
    // --dry-run` sends it without a path, so a daemon from before this
    // answers `usage: missing path` instead of hiring.
    if extra.get("dryRun").and_then(|v| v.as_bool()) == Some(true) {
        return ok(serde_json::json!({ "ok": true, "dryRun": true, "hire": "allowed" }));
    }
    match converge::converge(&req, &caller, &ConvergeAuthority::unrestricted()) {
        Ok(out) => {
            if out.changed || out.created {
                crate::v1_hire::emit_hire_watches();
            }
            let mut v = serde_json::json!({
                "ok": true,
                "path": out.path,
                "id": out.id,
                "handle": out.handle,
                "name": out.name,
                "changed": out.changed,
                "created": out.created,
                "steps": out.steps,
            });
            if let Some(app) = take_app {
                v["handover"] = match crate::apps::handover::hire_take(who, &app, &out.id, brief.as_deref(), out.created) {
                    Ok(h) => h,
                    Err(e) => serde_json::json!({ "error": { "code": e.code, "hint": e.hint } }),
                };
            }
            ok(v)
        }
        Err(e) => hire_error_response(&e),
    }
}

/// The refusal an agent gets when its workspace's "Let this agent hire
/// agents" is off (`hire_off`; the walls map teaches `agents.hire`).
pub(crate) const HIRE_OFF_HINT: &str = "Ask your human to turn on 'Let this agent hire agents' in Settings → Workspaces / Agents → {ws} → Permissions (or run k2 agent access set {ws} agents_hire on as the owner). Opening your own sidecars is a separate switch.";

/// The card an agent asks with for "Let this agent hire agents".
const HIRE_ACTION: &str = "agents.hire";

/// The `hire_off` hint for workspace `workspace_id` (address `ws`). With
/// permission cards on for that agent (both switches) the refusal wrapper
/// adds `Ask with: k2 permission request agents.hire …` under it. Off,
/// `k2 permission` is an unknown command for it (`k2 study permissions`,
/// "Off"), so the hint says what the person clicks and how they turn cards
/// on so the next ask is a card (0.46.16: an agent that had hired before
/// 0.46.12 got `hire_off` with neither).
pub(crate) fn hire_off_hint(workspace_id: &str, ws: &str) -> String {
    let base = HIRE_OFF_HINT.replace("{ws}", ws);
    if k2_core::permission::gate::enabled_for(workspace_id) {
        base
    } else {
        format!(
            "{base} Permission cards are off for you, so no card can go up: to approve from a card instead, your human turns them on ({}) and you ask with k2 permission request {HIRE_ACTION}.",
            crate::apps::user_hold::PERMISSION_CARDS_ON_PATH
        )
    }
}

/// What the real hire still needs, for a template dry run that skipped the
/// switch (`hireSwitch` in its answer): `None` when the caller isn't an
/// agent or its switch is on. `command` is the card to ask with, present
/// only while permission cards are on for that agent.
fn hire_switch_note(who: &Who) -> Option<serde_json::Value> {
    let Who::Agent { project_id, .. } = who else {
        return None;
    };
    if k2_core::workspace::settings::agents_can_hire_agents(project_id) {
        return None;
    }
    let ws = k2_core::workspace_session_handles::workspace_address_name_shared(project_id).unwrap_or_else(|_| project_id.clone());
    let mut v = serde_json::json!({
        "on": false,
        "code": "hire_off",
        "hint": hire_off_hint(project_id, &ws),
    });
    if k2_core::permission::gate::enabled_for(project_id) {
        v["command"] = serde_json::json!(crate::permission::walls::ask_command(HIRE_ACTION, &serde_json::Map::new()));
    }
    Some(v)
}

/// "Let this agent hire agents" (0299, 0.46.12; prd-agent-templates-v1
/// AT5): an agent's own `k2 agent hire` needs its workspace's switch, and
/// only the workspace's main session may hire (a sidecar never can, switch
/// or not). A template dry run writes nothing, so it needs only the main
/// session: the agent can show its person the hire before asking (its
/// answer's `hireSwitch` says the real hire needs the switch). A plain dry
/// run (no template) answers exactly as the real hire would here, so
/// `k2 agent hire --dry-run` never shows "would apply" for a hire the
/// daemon refuses. A plain shell tab is a person typing (as for sidecars);
/// the owner token and logins have their route floor; keys use `/v1`.
fn agent_hire_gate(who: &Who, body: &[u8]) -> Result<(), CliResponse> {
    let Who::Agent { project_id, session_id } = who else {
        return Ok(());
    };
    let v: serde_json::Value = serde_json::from_slice(body).unwrap_or(serde_json::Value::Null);
    let template = v.get("template").and_then(|t| t.as_str()).is_some_and(|t| !t.trim().is_empty());
    let dry_run = v.get("dryRun").and_then(|d| d.as_bool()).unwrap_or(false);
    // A dry run is audited as one, not as a refused hire.
    let outcome = |code: &str| if dry_run { format!("dry-run:{code}") } else { format!("refused:{code}") };
    let ws = k2_core::workspace_session_handles::workspace_address_name_shared(project_id).unwrap_or_else(|_| project_id.clone());
    if !(template && dry_run) && !k2_core::workspace::settings::agents_can_hire_agents(project_id) {
        audit("agent-hire", &who.label(), &outcome("hire_off"));
        return Err(err(403, "hire_off", hire_off_hint(project_id, &ws)));
    }
    if !is_main_session(project_id, session_id) {
        audit("agent-hire", &who.label(), &outcome("main_only"));
        return Err(err(
            403,
            "main_only",
            format!("only the workspace's main session may hire agents; a sidecar can't. Ask it: k2 msg {ws} \"please hire …\""),
        ));
    }
    Ok(())
}

/// Is this passport's session the workspace's main session (its live
/// `v2_session_map` key is the workspace id)? A sidecar's key is `tab-…`; a
/// session that is gone fails closed (as `sidecar_routes::agent_gate` does).
fn is_main_session(project_id: &str, session_id: &str) -> bool {
    #[cfg(test)]
    {
        if TEST_MAIN_SESSIONS.with(|m| m.borrow().contains(session_id)) {
            return true;
        }
    }
    crate::v2_session_map::snapshot()
        .into_iter()
        .any(|(key, s)| key == project_id && s.session_id.to_string() == session_id)
}

#[cfg(test)]
thread_local! {
    static TEST_MAIN_SESSIONS: std::cell::RefCell<std::collections::HashSet<String>> = std::cell::RefCell::new(std::collections::HashSet::new());
}

/// Unit tests: treat this passport session as its workspace's main session
/// (no PTY is spawned; the integration tests use real sessions).
#[cfg(test)]
pub(crate) fn test_main_session(session_id: &str) {
    TEST_MAIN_SESSIONS.with(|m| m.borrow_mut().insert(session_id.to_string()));
}

/// The daemon's V1 action catalog, for the template loader (AT6).
pub fn template_action_kind(id: &str) -> k2_core::hire::template::ActionKind {
    use k2_core::hire::template::ActionKind;
    use k2_core::permission::catalog::Grant;
    match crate::permission::actions::find(id) {
        Some(r) if r.spec.grant == Grant::Standing => ActionKind::Standing,
        Some(_) => ActionKind::OneTime,
        None => ActionKind::Unknown,
    }
}

pub fn install_template_action_check() {
    k2_core::hire::template::install_action_check(template_action_kind);
}

/// `GET /cli/agent-templates[?all=1]` and `GET /cli/agent-templates/show?id=<id>[&files=1]`.
fn templates_read(path: &str, params: &HashMap<String, String>) -> CliResponse {
    let flag = |k: &str| params.get(k).is_some_and(|v| matches!(v.as_str(), "1" | "true" | "yes"));
    let r = if path == CLI_TEMPLATES {
        k2_core::hire::template::list_json(flag("all"))
    } else {
        match params.get("id").map(|s| s.trim()).filter(|s| !s.is_empty()) {
            Some(id) => k2_core::hire::template::show_json(id, flag("files")),
            None => Err(HireError::usage("missing ?id=<template id> (see: k2 agent template list)")),
        }
    };
    match r {
        Ok(v) => ok(v),
        Err(e) => hire_error_response(&e),
    }
}

/// `POST /cli/agents/hire {template, params?, answers?, path?, …}` (T2). `path` is
/// optional: `<hire root>/<handle>`. `dryRun` returns the review rows and
/// writes nothing. The response carries `hello` on the first create; the
/// caller launches the agent and delivers it.
fn cli_template_hire(
    who: &Who,
    caller: &HireCaller,
    req: &ConvergeRequest,
    extra: &serde_json::Value,
    take_app: Option<&str>,
    brief: Option<&str>,
) -> CliResponse {
    let flag = |k: &str| extra.get(k).and_then(|v| v.as_bool()).unwrap_or(false);
    let opt = |s: &Option<String>| s.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let areq = k2_core::hire::template::ApplyRequest {
        template: req.template.clone().unwrap_or_default(),
        params: req.params.clone().unwrap_or_default(),
        answers: req.answers.clone().unwrap_or_default(),
        path: opt(&Some(req.path.clone())),
        name: opt(&req.name),
        handle: extra.get("handle").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string),
        tool: opt(&req.preset),
        model: opt(&req.default_model),
        dry_run: flag("dryRun"),
        db_access: opt(&req.db_access),
        no_wiki: req.no_wiki.unwrap_or(false),
        no_agents_md: req.no_agents_md.unwrap_or(false),
        fanout: req.fanout.unwrap_or(false),
    };
    let has = |s: &Option<String>| s.as_deref().is_some_and(|s| !s.trim().is_empty());
    if has(&req.persona) || has(&req.persona_raw) {
        return err(400, "usage", "template and persona are mutually exclusive — pass one or the other");
    }
    match k2_core::hire::template::apply(&areq, caller) {
        Ok(out) => {
            if !out.dry_run && (out.changed || out.created) {
                crate::v1_hire::emit_hire_watches();
            }
            let mut v = out.to_json();
            v["id"] = serde_json::json!(out.workspace_id);
            // The dry run skipped "Let this agent hire agents": say the
            // real hire needs it (and the card to ask with).
            if out.dry_run {
                if let Some(note) = hire_switch_note(who) {
                    v["hireSwitch"] = note;
                }
            }
            // Daemon-first hello (PRD §3, the Hello row): with `launch: true`,
            // on the first create the daemon starts the agent and delivers its
            // hello, then the hirer's `onboard` message after it. Absent or
            // false: nothing starts (agents hire from scripts; a surprise
            // launch spends someone's tokens) and the hello is only returned.
            let launch = extra.get("launch").and_then(|v| v.as_bool()) == Some(true);
            let onboard = extra.get("onboard").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
            // T9: the hello waits in the lock until the agent first starts.
            // A launch takes it (first create or a later re-run) BEFORE the
            // session starts, so the start hook finds nothing and it goes
            // once, ahead of `onboard`.
            let mut delivered = false;
            if !out.dry_run && launch {
                if let Some(hello) = crate::template_hello::take(&out.path) {
                    queue_hello(&out.path, &out.handle, hello, onboard.clone(), who, caller);
                    delivered = true;
                }
            }
            v["helloDelivered"] = serde_json::json!(delivered);
            if delivered && onboard.is_some() {
                v["onboardQueued"] = serde_json::json!(true);
            }
            if let (Some(app), false) = (take_app, out.dry_run) {
                v["handover"] = match crate::apps::handover::hire_take(who, app, &out.workspace_id, brief, out.created) {
                    Ok(h) => h,
                    Err(e) => serde_json::json!({ "error": { "code": e.code, "hint": e.hint } }),
                };
            }
            ok(v)
        }
        Err(e) => hire_error_response(&e),
    }
}

/// Deliver a template agent's hello as the hirer (see
/// [`crate::template_hello`]): start its session if needed, then the hello,
/// then the hirer's `onboard` message after it, off the request thread.
fn queue_hello(path: &str, handle: &str, hello: String, onboard: Option<String>, who: &Who, caller: &HireCaller) {
    let from = match who {
        Who::Login { username, .. } => k2_core::person_name::sender_identity_now(k2_core::person_name::Sender::Login(username)),
        Who::Agent { project_id, .. } | Who::Shell { project_id, .. } => crate::compute_routes::ws_handle(project_id),
        _ => k2_core::person_name::sender_identity_now(k2_core::person_name::Sender::OwnerToken),
    };
    crate::template_hello::send(path, handle, hello, onboard, from, Sender::from_hire_caller(caller, None));
}

fn hire_root(who: &Who, is_post: bool, body: &[u8]) -> CliResponse {
    if !is_post {
        if who.passport().is_some() {
            return owner_only();
        }
        let stored = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::hire::root::stored_root(&conn)
        };
        return match k2_core::hire::root::hire_root() {
            Ok(r) => ok(serde_json::json!({ "ok": true, "root": r, "default": stored.is_none() })),
            Err(e) => hire_error_response(&e),
        };
    }
    if !who.is_owner() {
        return err(403, "owner_only", "only the server's owner sets the hire root");
    }
    let v = match json_body(body) {
        Ok(v) => v,
        Err(e) => return hire_error_response(&e),
    };
    let Some(raw) = s(&v, "root") else {
        return err(400, "usage", "missing 'root'");
    };
    match k2_core::hire::root::set_hire_root(raw) {
        Ok(p) => {
            audit("hire-root-set", &who.label(), &format!("ok {}", p.display()));
            ok(serde_json::json!({ "ok": true, "root": p }))
        }
        Err(e) => hire_error_response(&e),
    }
}

/// `GET /cli/msg/who?id=m_…` — who really sent a delivery. A passport may
/// look up only deliveries to or from its own workspace.
fn msg_who(who: &Who, params: &HashMap<String, String>) -> CliResponse {
    let Some(id) = params.get("id").map(|s| s.trim()).filter(|s| !s.is_empty()) else {
        return err(400, "usage", "missing ?id=<delivery id> (the m_… in [from … · m_…])");
    };
    if !k2_core::hire::deliveries::is_delivery_id(id) {
        return err(404, "not_found", format!("no delivery {id}: real deliveries look like m_ and 10 letters/digits"));
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    let Some(d) = k2_core::hire::deliveries::get(&conn, id) else {
        return err(404, "not_found", format!("no delivery {id}: K2 never sent it (text that looks like a prefix proves nothing)"));
    };
    if let Some((ws, _)) = who.passport() {
        let mine = d.target_workspace_id.as_deref() == Some(ws) || d.sender_workspace_id.as_deref() == Some(ws);
        if !mine {
            return err(404, "not_found", format!("no delivery {id} addressed to you"));
        }
    }
    let sender_handle = d.sender_workspace_id.as_deref().and_then(|w| {
        conn.query_row("SELECT handle FROM projects WHERE id = ?1", rusqlite::params![w], |r| r.get::<_, Option<String>>(0))
            .ok()
            .flatten()
    });
    let link_rows: Vec<links::Link> = match (d.sender_workspace_id.as_deref(), d.target_workspace_id.as_deref()) {
        (Some(a), Some(b)) => links::links_out(&conn, a)
            .into_iter()
            .chain(links::links_out(&conn, b))
            .filter(|l| {
                (l.consumer_workspace_id == a && team_side_has(&conn, l, b)) || (l.consumer_workspace_id == b && team_side_has(&conn, l, a))
            })
            .collect(),
        _ => Vec::new(),
    };
    ok(serde_json::json!({
        "ok": true,
        "id": d.id,
        "via": d.via,
        "sender": { "workspaceId": d.sender_workspace_id, "handle": sender_handle, "label": d.sender_label },
        "actor": d.actor,
        "onBehalfOf": d.on_behalf_of.as_ref().map(|v| serde_json::json!({ "value": v, "verified": false })),
        "target": { "workspaceId": d.target_workspace_id, "token": d.target },
        "delivered": d.delivered,
        "at": d.at,
        "linked": !link_rows.is_empty(),
        "links": link_rows.iter().map(|l| serde_json::json!({ "id": l.id, "kind": l.kind, "scope": l.scope })).collect::<Vec<_>>(),
    }))
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::path::PathBuf;
    use std::sync::OnceLock;

    /// One hire root (and trash dir) for the whole test binary: the DB is
    /// shared, so every hire test must agree on it. Never the real home.
    pub(crate) fn ensure_test_root() -> PathBuf {
        static ROOT: OnceLock<PathBuf> = OnceLock::new();
        ROOT.get_or_init(|| {
            let base = k2_core::test_env::stable_temp_dir().join(format!("k2-hire-tests-{}", std::process::id()));
            let root = base.join("agents");
            std::fs::create_dir_all(&root).expect("mkdir hire test root");
            k2_core::hire::apply::set_trash_override(Some(base.join("trash")));
            k2_core::db::init_for_tests();
            k2_core::hire::root::set_hire_root(&root.to_string_lossy()).expect("set hire root")
        })
        .clone()
    }
}

#[cfg(test)]
#[path = "hire_routes_tests.rs"]
mod tests;

fn team_side_has(conn: &rusqlite::Connection, l: &links::Link, ws: &str) -> bool {
    if l.team_lead_workspace_id == ws {
        return true;
    }
    l.instance_id
        .as_deref()
        .and_then(|i| ledger::get(conn, i))
        .is_some_and(|i| i.members.iter().any(|m| m.workspace_id == ws && m.status == "live"))
}
