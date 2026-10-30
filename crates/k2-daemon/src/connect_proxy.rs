//! `POST /cli/connect/proxy` — the `k2` CLI's K2 Connect control-plane
//! calls, made by the daemon with the tunnel token it holds (0.45.7).
//!
//! Since 0.45.4 a desktop keeps the tunnel token in the K2 vault and
//! `tunnel.json`'s `token` is empty, so `k2 publish status` and
//! `k2 publish subdomain list|create|point|target|rm` could no longer read
//! it. The CLI now names a method + endpoint (+ JSON body); the daemon
//! checks it against [`k2_core::tunnel::cp_proxy::check_allowed`], adds
//! the bearer and hands back the control plane's status and body. The
//! token never leaves the daemon.
//!
//! Body: `{"method","endpoint","body"?}` — `body` is the JSON the control
//! plane receives (an object, or a string holding JSON).
//!
//! 200 `{"status","body"}` = the control plane's answer, verbatim (a 4xx
//! from it is still a 200 here; the CLI renders it as before). Anything
//! else means nothing reached the control plane, or it could not be
//! reached: 400 `not_allowed` / `no_token` / `bad_request` / `bad_label` /
//! `reserved_label` / `bad_target` / `target_is_daemon` /
//! `target_metadata` / `target_not_loopback` (0.46.3, see
//! [`crate::splice_guard::check_target`]), 403 `airgap` / `primary_label` / `label_not_claimed`,
//! 409 `label_owned_by_other_workspace`, 429 `rate_limited`, 503
//! `token_unreadable` (the vault could not be read; soft, retry), 502
//! `unreachable`, 500 `config`.
//!
//! ## Who may call it
//!
//! The owner token and Owner/Admin logins get the whole allowlist. An
//! agent passport (a workspace agent, no human in Settings) manages its
//! OWN nested subdomains end to end — the same rights `k2 publish run`
//! has:
//!
//! - `GET /subdomains` (list / `k2 publish status`);
//! - `POST /subdomains` `{label,target}` — a nested label that is not the
//!   primary, not reserved (`skin`/`meet`/`chat`/`mail`) and not held by
//!   another workspace; on success the label is claimed for the agent's
//!   workspace (0074);
//! - `PUT` / `DELETE /subdomains/<label>` — only a label claimed by the
//!   agent's own workspace; a delete drops the claim.
//!
//! Agent creates and deletes are limited to [`AGENT_WRITES_PER_HOUR`] per
//! workspace. Member logins and app passes are refused (dispatcher).

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use k2_core::db::schema::SubdomainWorkspace;
use k2_core::tunnel::cp_proxy::{self, ProxyError};

use crate::cli_response::CliResponse;
use crate::label_owner;
use crate::session_token::HookPrincipal;
use crate::splice_guard;

/// Agent creates + deletes allowed per workspace per rolling hour.
pub const AGENT_WRITES_PER_HOUR: usize = 20;

fn err(status: &'static str, code: &str, msg: impl std::fmt::Display) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: serde_json::json!({ "error": msg.to_string(), "code": code }).to_string(),
    }
}

/// What to record locally after an agent write the control plane accepted.
#[derive(Debug)]
enum AfterWrite {
    Claim { label: String, target: String },
    Retarget { label: String, target: String },
    Unclaim { label: String },
}

/// Take one agent create/delete slot for `project_id`; false = over limit.
fn take_agent_write_slot(project_id: &str) -> bool {
    static SLOTS: Mutex<Option<HashMap<String, VecDeque<Instant>>>> = Mutex::new(None);
    let mut g = SLOTS.lock().unwrap_or_else(|p| p.into_inner());
    let map = g.get_or_insert_with(HashMap::new);
    let q = map.entry(project_id.to_string()).or_default();
    let now = Instant::now();
    while q.front().is_some_and(|t| now.duration_since(*t) >= Duration::from_secs(3600)) {
        q.pop_front();
    }
    if q.len() >= AGENT_WRITES_PER_HOUR {
        return false;
    }
    q.push_back(now);
    true
}

/// An agent's label: lowercase letters, digits and dashes, 1–63 chars.
fn agent_label(raw: &str) -> Result<String, CliResponse> {
    let l = raw.trim().to_ascii_lowercase();
    let ok = !l.is_empty()
        && l.len() <= 63
        && !l.starts_with('-')
        && !l.ends_with('-')
        && l.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if ok {
        Ok(l)
    } else {
        Err(err(
            "400 Bad Request",
            "bad_label",
            "Invalid label (use lowercase letters, digits, and dashes).",
        ))
    }
}

/// The checks an agent's write must pass before anything is sent. Returns
/// the body to forward (rebuilt from the known fields only) and what to
/// record afterwards.
fn agent_gate(
    principal: &HookPrincipal,
    method: &str,
    endpoint: &str,
    body: Option<&str>,
) -> Result<(Option<String>, Option<AfterWrite>), CliResponse> {
    cp_proxy::check_allowed(method, endpoint, body)
        .map_err(|m| err("400 Bad Request", "not_allowed", m))?;
    if method == "GET" {
        return Ok((None, None));
    }
    let project_id = principal.workspace_uuid.trim().to_string();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let known: bool = conn
            .query_row(
                "SELECT 1 FROM projects WHERE id = ?1",
                rusqlite::params![project_id],
                |_| Ok(true),
            )
            .unwrap_or(false);
        if !known {
            return Err(err(
                "403 Forbidden",
                "forbidden",
                "this agent's workspace is not registered on this server",
            ));
        }
    }
    let primary = k2_core::tunnel::config::load()
        .map(|c| c.subdomain.trim().to_ascii_lowercase())
        .unwrap_or_default();

    let (label, target) = match method {
        "POST" => {
            let v: serde_json::Value = serde_json::from_str(body.unwrap_or(""))
                .map_err(|_| err("400 Bad Request", "bad_request", "body must be {\"label\",\"target\"}"))?;
            let label = v.get("label").and_then(|x| x.as_str()).unwrap_or("");
            let target = v.get("target").and_then(|x| x.as_str()).unwrap_or("").trim();
            if label.trim().is_empty() || target.is_empty() {
                return Err(err("400 Bad Request", "bad_request", "body must be {\"label\",\"target\"}"));
            }
            (agent_label(label)?, Some(target.to_string()))
        }
        "PUT" | "DELETE" => {
            let seg = endpoint.strip_prefix("/subdomains/").unwrap_or("");
            let label = agent_label(seg)?;
            let target = if method == "PUT" {
                let v: serde_json::Value = serde_json::from_str(body.unwrap_or(""))
                    .map_err(|_| err("400 Bad Request", "bad_request", "body must be {\"target\"}"))?;
                let t = v.get("target").and_then(|x| x.as_str()).unwrap_or("").trim();
                if t.is_empty() {
                    return Err(err("400 Bad Request", "bad_request", "body must be {\"target\"}"));
                }
                Some(t.to_string())
            } else {
                None
            };
            (label, target)
        }
        _ => return Err(err("400 Bad Request", "not_allowed", "not an allowed K2 Connect request")),
    };

    if !primary.is_empty() && label == primary {
        return Err(err(
            "403 Forbidden",
            "primary_label",
            format!(
                "'{label}' is this server's primary subdomain; agents manage nested labels only \
                 — ask the server owner"
            ),
        ));
    }
    if k2_core::skin::is_reserved_nested_label(&label) {
        return Err(err(
            "400 Bad Request",
            "reserved_label",
            k2_core::skin::reserved_nested_label_error(&label),
        ));
    }

    let after = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if method == "POST" {
            label_owner::decide(&conn, &label, &project_id, false, &label_owner::Caller::Agent)
                .map_err(label_owner::Refusal::into_response)?;
            AfterWrite::Claim { label: label.clone(), target: target.clone().unwrap_or_default() }
        } else {
            let owner = SubdomainWorkspace::owner_of(&conn, &label)
                .map_err(|e| err("500 Internal Server Error", "config", e))?;
            match owner {
                Some(o) if o == project_id => {}
                Some(o) => {
                    return Err(label_owner::owned_by_other(
                        &conn,
                        &label,
                        &o,
                        &label_owner::Caller::Agent,
                    )
                    .into_response())
                }
                None => {
                    return Err(err(
                        "403 Forbidden",
                        "label_not_claimed",
                        format!(
                            "'{label}' is not claimed by this workspace; an agent may only change \
                             or remove its own labels (k2 publish subdomain claim {label})"
                        ),
                    ))
                }
            }
            match target.clone() {
                Some(t) => AfterWrite::Retarget { label: label.clone(), target: t },
                None => AfterWrite::Unclaim { label: label.clone() },
            }
        }
    };

    if matches!(method, "POST" | "DELETE") && !take_agent_write_slot(&project_id) {
        return Err(err(
            "429 Too Many Requests",
            "rate_limited",
            format!(
                "this workspace's agents created or removed {AGENT_WRITES_PER_HOUR} subdomains in \
                 the last hour; try again later"
            ),
        ));
    }

    let fwd = match (method, &target) {
        ("POST", Some(t)) => Some(serde_json::json!({ "label": label, "target": t }).to_string()),
        ("PUT", Some(t)) => Some(serde_json::json!({ "target": t }).to_string()),
        _ => None,
    };
    Ok((fwd, Some(after)))
}

/// The proxy target a create (`POST /subdomains`) or retarget
/// (`PUT /subdomains/<label>`) sets, if any.
fn target_of(method: &str, endpoint: &str, body: Option<&str>) -> Option<String> {
    let sets_target = match method {
        "POST" => endpoint == "/subdomains",
        "PUT" => endpoint.starts_with("/subdomains/"),
        _ => false,
    };
    if !sets_target {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(body?).ok()?;
    let t = v.get("target")?.as_str()?.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// Record an accepted agent write locally (0074 claim / target / unclaim),
/// then tell the UIs and re-pull the routing map so the URL works now.
fn record_after(principal: &HookPrincipal, after: AfterWrite) {
    let project_id = principal.workspace_uuid.trim();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let res = match &after {
            AfterWrite::Claim { label, target } => label_owner::apply(
                &conn,
                label,
                project_id,
                Some(target.as_str()),
                &label_owner::Decision::Claim,
                &label_owner::Caller::Agent,
            )
            .map_err(|r| r.message()),
            AfterWrite::Retarget { label, target } => {
                SubdomainWorkspace::self_stamp(&conn, label, project_id, Some(target.as_str()))
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            }
            AfterWrite::Unclaim { label } => SubdomainWorkspace::self_unclaim(&conn, label, project_id)
                .map(|_| ())
                .map_err(|e| e.to_string()),
        };
        if let Err(e) = res {
            k2_core::log_debug!("[connect/proxy] agent write accepted but not recorded locally ({after:?}): {e}");
        }
    }
    crate::session_events::emit_tunnel_subdomains_changed();
    // Inline (we are on a blocking worker): when the agent's command
    // returns, the new routing map has landed and the URL answers. The
    // CLI's own refresh nudge is an owner route an agent can't call.
    let r = crate::misc_routes::handle_subdomain_refresh();
    if r.status != "200 OK" {
        k2_core::log_debug!(
            "[connect/proxy] map refresh after an agent write failed (converges on the next poll): {}",
            r.body
        );
    }
}

/// `k2 publish` visibility: a label made here starts private, and a removed
/// one's row goes, so a new label of that name starts private too. Before the
/// map refresh, so nothing can grandfather a label made here.
fn note_visibility(method: &str, endpoint: &str, body: Option<&str>) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let res = match method {
        "POST" if endpoint == "/subdomains" => {
            let label = body
                .and_then(|b| serde_json::from_str::<serde_json::Value>(b).ok())
                .and_then(|v| v.get("label").and_then(|l| l.as_str()).map(str::to_string));
            match label {
                Some(l) => k2_core::site_visibility::note_new_label(&conn, &l, "create"),
                None => Ok(()),
            }
        }
        "DELETE" => match endpoint.strip_prefix("/subdomains/") {
            Some(l) if !l.is_empty() => k2_core::site_visibility::forget_label(&conn, l),
            _ => Ok(()),
        },
        _ => Ok(()),
    };
    if let Err(e) = res {
        k2_core::log_debug!("[connect/proxy] visibility row: {e}");
    }
}

/// The route body. `principal` is `Some` for an agent passport (the
/// dispatcher authenticated it), `None` for the owner token or an
/// Owner/Admin login.
pub fn handle(body: &[u8], principal: Option<&HookPrincipal>) -> CliResponse {
    let v: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return err("400 Bad Request", "bad_request", format!("invalid JSON body: {e}")),
    };
    let method = v.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let endpoint = v.get("endpoint").and_then(|m| m.as_str()).unwrap_or("");
    if method.is_empty() || endpoint.is_empty() {
        return err("400 Bad Request", "bad_request", "method and endpoint are required");
    }
    let mut cp_body: Option<String> = match v.get("body") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(other) => Some(other.to_string()),
    };

    // 0.46.3: one target rule for every caller (`splice_guard`): never
    // this daemon's own port or a metadata address; agents loopback only.
    if let Some(target) = target_of(method, endpoint, cp_body.as_deref()) {
        let caller = if principal.is_some() {
            splice_guard::TargetCaller::Agent
        } else {
            splice_guard::TargetCaller::Operator
        };
        if let Err(r) = splice_guard::check_target(&target, caller) {
            return err("400 Bad Request", r.code, r.message);
        }
    }

    let mut after = None;
    if let Some(p) = principal {
        match agent_gate(p, method, endpoint, cp_body.as_deref()) {
            Ok((fwd, a)) => {
                if fwd.is_some() {
                    cp_body = fwd;
                }
                after = a;
            }
            Err(resp) => return resp,
        }
    }

    match cp_proxy::forward(method, endpoint, cp_body.as_deref()) {
        Ok(r) => {
            if (200..300).contains(&r.status) {
                note_visibility(method, endpoint, cp_body.as_deref());
            }
            if let (Some(p), Some(a)) = (principal, after) {
                if (200..300).contains(&r.status) {
                    record_after(p, a);
                }
            }
            CliResponse::ok_json(serde_json::json!({ "status": r.status, "body": r.body }).to_string())
        }
        Err(ProxyError::NotAllowed(m)) => err("400 Bad Request", "not_allowed", m),
        Err(ProxyError::Airgap(m)) => err("403 Forbidden", "airgap", m),
        Err(ProxyError::Config(m)) => err("500 Internal Server Error", "config", m),
        Err(ProxyError::TokenUnreadable(e)) => err(
            "503 Service Unavailable",
            "token_unreadable",
            format!(
                "the K2 Connect tunnel token is in the K2 vault, which could not be read \
                 right now ({e}); nothing was sent — try again once the vault is readable"
            ),
        ),
        Err(ProxyError::NoToken) => err(
            "400 Bad Request",
            "no_token",
            "no K2 Connect tunnel token on this device — select a subdomain in \
             Settings → K2 Connect (or run 'k2 tunnel start') first",
        ),
        Err(ProxyError::Unreachable(m)) => err("502 Bad Gateway", "unreachable", m),
    }
}
