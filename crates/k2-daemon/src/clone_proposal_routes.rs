//! Agent clone proposals (prd-clone-carries-threads-and-vault-v1 §4.4,
//! §7.1, CL21; table `clone_proposals`, 0181).
//!
//! Agents never call `/cli/clone/*` (a DENY prefix on the passport
//! allowlist). They **propose** a clone of their own workspace here; a
//! person approves it and runs it under their own credential and clone
//! gate. The routes live outside `/cli/clone/` on purpose (CL21).
//!
//! ```text
//! POST /cli/clone-proposal/new       {to, workspace?, threads?, sinceDays?, allHistory?,
//!                                     copy?, blank?, skip?, secrets?, dest?, why}
//! GET  /cli/clone-proposal/list      [status=open|all|<status>] [workspace=]
//! POST /cli/clone-proposal/withdraw  {proposal}           the proposer (or an Owner/Admin)
//! POST /cli/clone-proposal/approve   {proposal, confirmToken}   Owner/Admin person + clone gate
//! POST /cli/clone-proposal/deny      {proposal, reason?}  Owner/Admin person
//! POST /cli/clone-proposal/finish    {proposal, ok, detail?}    Owner/Admin person, after the run
//! ```
//!
//! - A passport (agent, or a person in a K2 shell tab) proposes, lists and
//!   withdraws for **its own workspace** only. `approve`, `deny`, `finish`
//!   are not on the passport allowlist and refuse passports here too.
//! - Approve needs a fresh vault confirm `clone-approve` bound to the
//!   proposal id (Touch ID, the vault password or the Connect password;
//!   60 s, single use) and the clone gate (`clone_gate::Door::
//!   ProposalApprove`). Copying secrets then needs its own `clone-carry`
//!   confirm on the clone itself (D1: Owner/Admin only).
//! - Proposing posts one line on the workspace's main Thread for its
//!   human; every decision is told to the agent (a `k2 msg`).
//! - `options` hold names and choices only (never a value).

use std::collections::HashMap;
use std::time::Duration;

use crate::cli_response::CliResponse;
use crate::compute_routes::{no, refusal_response, ws_by_id, ws_by_token, Refusal, Who, Ws, R};
use k2_core::clone_proposals::{self as cp, CloneOptions, SecretChoice};
use k2_core::connect_users::Role;
use serde_json::{json, Value as J};

pub const NEW: &str = "/cli/clone-proposal/new";
pub const LIST: &str = "/cli/clone-proposal/list";
pub const WITHDRAW: &str = "/cli/clone-proposal/withdraw";
pub const APPROVE: &str = "/cli/clone-proposal/approve";
pub const DENY: &str = "/cli/clone-proposal/deny";
pub const FINISH: &str = "/cli/clone-proposal/finish";

pub const ROUTES: &[&str] = &[NEW, LIST, WITHDRAW, APPROVE, DENY, FINISH];
pub const POST_ROUTES: &[&str] = &[NEW, WITHDRAW, APPROVE, DENY, FINISH];
/// The routes an agent passport may reach (also on `session_token`'s
/// allowlist).
pub const AGENT_ROUTES: &[&str] = &[NEW, LIST, WITHDRAW];

/// The vault confirm action for approving a proposal (`secretId` = id).
pub const CONFIRM_ACTION: &str = "clone-approve";

pub const MAX_BODY: usize = 64 * 1024;

pub mod codes {
    pub const BAD_REQUEST: &str = "bad_request";
    pub const FORBIDDEN: &str = "forbidden";
    pub const PERSON_ONLY: &str = "person_only";
    pub const NOT_FOUND: &str = "proposal_not_found";
    pub const NOT_OPEN: &str = "proposal_not_open";
    pub const EXPIRED: &str = "proposal_expired";
    pub const CONFIRM_REQUIRED: &str = "confirm_required";
    pub const VAULT_CLOSED: &str = "vault_closed";
    pub const DB: &str = "db";
}

pub fn is_route(p: &str) -> bool {
    ROUTES.contains(&p)
}

pub fn is_post_route(p: &str) -> bool {
    POST_ROUTES.contains(&p)
}

pub fn too_large() -> CliResponse {
    refusal_response(&no(codes::BAD_REQUEST, 413, format!("request bodies here are at most {} KiB", MAX_BODY / 1024)))
}

/// Serve one proposal route (blocking: DB, vault, Thread).
pub fn handle(path: &str, who: Who, body: &[u8], params: &HashMap<String, String>, ingress: &str) -> CliResponse {
    let r = match path {
        NEW => propose(&who, body),
        LIST => list(&who, params),
        WITHDRAW => withdraw(&who, body),
        APPROVE => approve(&who, body, ingress),
        DENY => deny(&who, body),
        FINISH => finish(&who, body),
        _ => Err(no(codes::NOT_FOUND, 404, "no such clone proposal route")),
    };
    match r {
        Ok(v) => CliResponse::ok_json(v.to_string()),
        Err(e) => refusal_response(&e),
    }
}

// ── helpers ──────────────────────────────────────────────────────────

fn db<T>(f: impl FnOnce(&rusqlite::Connection) -> rusqlite::Result<T>) -> R<T> {
    let h = k2_core::db::shared();
    let conn = h.lock();
    f(&conn).map_err(|e| no(codes::DB, 500, format!("database: {e}")))
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn parse(body: &[u8]) -> R<J> {
    let v: J = serde_json::from_slice(body).map_err(|e| no(codes::BAD_REQUEST, 400, format!("invalid JSON body: {e}")))?;
    if !v.is_object() {
        return Err(no(codes::BAD_REQUEST, 400, "the body is a JSON object"));
    }
    Ok(v)
}

fn s<'a>(v: &'a J, k: &str) -> Option<&'a str> {
    v.get(k).and_then(J::as_str).map(str::trim).filter(|s| !s.is_empty())
}

/// Who stamps a proposal: `agent:<handle>/<session>` for an agent (PRD
/// §5.2), else the caller label.
fn actor(who: &Who) -> String {
    match who {
        Who::Agent { project_id, session_id } => format!("agent:{}/{}", crate::compute_routes::ws_handle(project_id), session_id),
        other => other.label(),
    }
}

fn own_ws(who: &Who) -> Option<&str> {
    who.passport().map(|(ws, _)| ws)
}

fn need_person_admin(who: &Who, what: &str) -> R<()> {
    match who {
        Who::Owner | Who::Login { role: Role::Owner | Role::Admin, .. } => Ok(()),
        Who::Agent { .. } | Who::Shell { .. } => Err(no(
            codes::PERSON_ONLY,
            403,
            format!("only a person (the owner or an admin) can {what} a clone proposal, from a terminal outside K2 or the K2 app"),
        )),
        Who::Login { .. } => Err(no(codes::FORBIDDEN, 403, format!("only the owner or an admin can {what} a clone proposal"))),
        Who::Wrong | Who::Invalid => Err(no(codes::FORBIDDEN, 403, "invalid or missing token")),
    }
}

/// The vault actor stamp a confirm was minted for (matches
/// `vault::actor` for the owner token and Connect logins).
fn confirm_stamp(who: &Who) -> Option<String> {
    match who {
        Who::Owner => Some("owner".into()),
        Who::Login { username, role: Role::Owner | Role::Admin } => Some(format!("connect:{username}")),
        _ => None,
    }
}

fn find(token: Option<&str>) -> R<cp::Proposal> {
    let t = token.ok_or_else(|| no(codes::BAD_REQUEST, 400, "name the proposal (its cp_… id; k2 clone proposals)"))?;
    db(|c| cp::find(c, t))?.ok_or_else(|| no(codes::NOT_FOUND, 404, format!("no clone proposal '{t}' (k2 clone proposals)")))
}

fn decide_err(code: &'static str) -> Refusal {
    match code {
        "expired" => no(codes::EXPIRED, 410, "this proposal expired (7 days); the agent can propose again"),
        "not_open" => no(codes::NOT_OPEN, 409, "this proposal was already decided"),
        "not_approved" => no(codes::NOT_OPEN, 409, "only an approved proposal can be finished"),
        "not_found" => no(codes::NOT_FOUND, 404, "no such proposal"),
        _ => no(codes::DB, 500, "could not record the decision"),
    }
}

/// One proposal as the routes and the CLI see it.
fn proposal_json(p: &cp::Proposal) -> J {
    let mut v = serde_json::to_value(p).unwrap_or(J::Null);
    let ws = ws_by_id(&p.project_id);
    v["workspaceHandle"] = json!(ws.as_ref().map(|w| w.handle.clone()));
    v["workspacePath"] = json!(ws.as_ref().map(|w| w.path.clone()));
    v["summary"] = json!(summary_line(p, ws.as_ref()));
    v
}

fn summary_line(p: &cp::Proposal, ws: Option<&Ws>) -> String {
    let name = ws.map(|w| w.handle.as_str()).unwrap_or(p.project_id.as_str());
    let to = if p.target == "here" { "a second copy on this server".to_string() } else { p.target.clone() };
    format!("clone {name} to {to}, {}", p.options.summary())
}

/// Tell the proposing agent (a `k2 msg`, never blocks). Tests install a
/// recording sink.
pub(crate) mod notify {
    use std::sync::RwLock;
    pub type Sink = Box<dyn Fn(&str, &str) + Send + Sync>;
    static SINK: RwLock<Option<Sink>> = RwLock::new(None);

    #[cfg(test)]
    pub fn set_sink(s: Option<Sink>) {
        *SINK.write().unwrap_or_else(|e| e.into_inner()) = s;
    }

    pub fn agent(project_id: &str, text: &str) {
        if let Some(s) = SINK.read().unwrap_or_else(|e| e.into_inner()).as_ref() {
            s(project_id, text);
            return;
        }
        let ws = project_id.to_string();
        let text = text.to_string();
        let _ = std::thread::Builder::new().name("clone-proposal".into()).spawn(move || {
            let r = crate::workspace_msg::deliver_live(&ws, &text, "k2-clone", "", true, super::Duration::from_secs(60));
            k2_core::log_debug!("[clone-proposal] told {ws}: {}", serde_json::to_string(&r).unwrap_or_default());
        });
    }
}

fn audit(by: &str, outcome: String) {
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new("clone-proposal", by, outcome, "cli", "-", "clone"));
}

fn expire_and_tell() -> R<()> {
    let gone = db(|c| cp::expire_due(c, now()))?;
    for p in &gone {
        notify::agent(&p.project_id, &format!("[k2 clone] proposal {} expired after 7 days without an answer", p.id));
    }
    Ok(())
}

/// The per-secret choices from `copy` / `blank` / `skip` lists and/or a
/// `secrets` map. A name in two places is refused.
fn secret_choices(v: &J) -> R<std::collections::BTreeMap<String, SecretChoice>> {
    let mut out = std::collections::BTreeMap::new();
    let mut put = |name: &str, c: SecretChoice| -> R<()> {
        let n = name.trim();
        if n.is_empty() {
            return Ok(());
        }
        if let Some(prev) = out.insert(n.to_string(), c) {
            if prev != c {
                return Err(no(codes::BAD_REQUEST, 400, format!("'{n}' is named twice with different choices")));
            }
        }
        Ok(())
    };
    for (key, c) in [("copy", SecretChoice::Copy), ("blank", SecretChoice::Blank), ("skip", SecretChoice::Skip)] {
        match v.get(key) {
            None | Some(J::Null) => {}
            Some(J::Array(a)) => {
                for n in a {
                    let n = n.as_str().ok_or_else(|| no(codes::BAD_REQUEST, 400, format!("{key} is a list of secret names")))?;
                    put(n, c)?;
                }
            }
            Some(J::String(csv)) => {
                for n in csv.split(',') {
                    put(n, c)?;
                }
            }
            Some(_) => return Err(no(codes::BAD_REQUEST, 400, format!("{key} is a list of secret names"))),
        }
    }
    if let Some(m) = v.get("secrets") {
        let m = m.as_object().ok_or_else(|| no(codes::BAD_REQUEST, 400, "secrets is {NAME: copy|blank|skip}"))?;
        for (name, c) in m {
            let c = c
                .as_str()
                .and_then(SecretChoice::parse)
                .ok_or_else(|| no(codes::BAD_REQUEST, 400, format!("'{name}': choose copy, blank or skip")))?;
            put(name, c)?;
        }
    }
    Ok(out)
}

// ── new ──────────────────────────────────────────────────────────────

fn propose(who: &Who, body: &[u8]) -> R<J> {
    if matches!(who, Who::Wrong | Who::Invalid) {
        return Err(no(codes::FORBIDDEN, 403, "invalid or missing token"));
    }
    let v = parse(body)?;
    let requested = s(&v, "workspace");
    let ws = match own_ws(who) {
        Some(own) => {
            let w = ws_by_id(own).ok_or_else(|| no(codes::FORBIDDEN, 404, "your workspace is not registered"))?;
            if let Some(req) = requested {
                if ws_by_token(req).is_none_or(|o| o.id != w.id) {
                    return Err(no(codes::FORBIDDEN, 403, "an agent proposes clones of its own workspace only"));
                }
            }
            w
        }
        None => {
            let req = requested.ok_or_else(|| no(codes::BAD_REQUEST, 400, "name the workspace to clone (workspace)"))?;
            ws_by_token(req).ok_or_else(|| no(codes::BAD_REQUEST, 404, format!("no workspace '{req}'")))?
        }
    };
    let target = s(&v, "to").ok_or_else(|| no(codes::BAD_REQUEST, 400, "say where: to = a host (luzz.k2.dev) or 'here'"))?;
    let why = s(&v, "why");
    if who.passport().is_some() && why.is_none() {
        return Err(no(codes::BAD_REQUEST, 400, "say why for your human: --why \"…\""));
    }
    let since_days = match v.get("sinceDays") {
        None | Some(J::Null) => None,
        Some(n) => Some(
            n.as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| no(codes::BAD_REQUEST, 400, "sinceDays is a whole number of days"))?,
        ),
    };
    let options = CloneOptions {
        threads: v.get("threads").and_then(J::as_bool).unwrap_or(true),
        since_days,
        secrets: secret_choices(&v)?,
        all_history: v.get("allHistory").and_then(J::as_bool).unwrap_or(true),
        dest: s(&v, "dest").map(str::to_string),
    };
    let by = actor(who);
    let p = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        cp::create(
            &conn,
            cp::NewProposal { project_id: &ws.id, proposed_by: &by, target, options, why },
            now(),
        )
        .map_err(|e| no(codes::BAD_REQUEST, 400, e))?
    };
    let line = format!(
        "Proposed: {}. Review in K2 or `k2 clone approve {}`{}",
        summary_line(&p, Some(&ws)),
        p.id,
        p.why.as_deref().map(|w| format!(" (why: {w})")).unwrap_or_default()
    );
    let notice = crate::overlay_routes::post_workspace_notice(&ws.handle, &ws.handle, &line);
    if let Err(e) = &notice {
        k2_core::log_debug!("[clone-proposal] Thread notice for {} not posted: {e}", ws.handle);
    }
    audit(&by, format!("propose {} {} -> {}", p.id, ws.handle, p.target));
    Ok(json!({
        "ok": true,
        "status": "held",
        "proposal": proposal_json(&p),
        "threadNotice": notice.is_ok(),
        "hint": format!(
            "held for a person: they approve it with k2 clone approve {} (or in K2), or deny it. Check with k2 clone proposals --mine.",
            p.id
        ),
    }))
}

// ── list ─────────────────────────────────────────────────────────────

fn list(who: &Who, params: &HashMap<String, String>) -> R<J> {
    if matches!(who, Who::Wrong | Who::Invalid) {
        return Err(no(codes::FORBIDDEN, 403, "invalid or missing token"));
    }
    let status = match params.get("status").map(|s| s.trim()) {
        None | Some("") | Some("open") => Some(cp::STATUS_OPEN),
        Some("all") => None,
        Some(st) if cp::STATUSES.contains(&st) => Some(st),
        Some(other) => {
            return Err(no(codes::BAD_REQUEST, 400, format!("status is open, all or one of {}, not '{other}'", cp::STATUSES.join(", "))))
        }
    };
    let project = match own_ws(who) {
        Some(own) => Some(own.to_string()),
        None => match params.get("workspace").map(|s| s.trim()).filter(|s| !s.is_empty()) {
            Some(w) => Some(ws_by_token(w).ok_or_else(|| no(codes::BAD_REQUEST, 404, format!("no workspace '{w}'")))?.id),
            None => None,
        },
    };
    expire_and_tell()?;
    let rows = db(|c| cp::list(c, project.as_deref(), status, 200))?;
    Ok(json!({ "ok": true, "proposals": rows.iter().map(proposal_json).collect::<Vec<_>>() }))
}

// ── withdraw ─────────────────────────────────────────────────────────

fn withdraw(who: &Who, body: &[u8]) -> R<J> {
    if matches!(who, Who::Wrong | Who::Invalid) {
        return Err(no(codes::FORBIDDEN, 403, "invalid or missing token"));
    }
    let v = parse(body)?;
    let p = find(s(&v, "proposal"))?;
    let by = actor(who);
    let allowed = match who {
        Who::Agent { project_id, .. } | Who::Shell { project_id, .. } => *project_id == p.project_id,
        Who::Owner | Who::Login { role: Role::Owner | Role::Admin, .. } => true,
        Who::Login { .. } => p.proposed_by == by,
        _ => false,
    };
    if !allowed {
        // Never confirm another workspace's proposal exists to a passport.
        return Err(no(codes::NOT_FOUND, 404, "no such proposal of yours"));
    }
    let done = db(|c| Ok(cp::decide(c, &p.id, cp::STATUS_WITHDRAWN, &by, now())))?.map_err(decide_err)?;
    audit(&by, format!("withdraw {}", done.id));
    Ok(json!({ "ok": true, "proposal": proposal_json(&done) }))
}

// ── approve ──────────────────────────────────────────────────────────

fn vault() -> R<std::sync::Arc<k2_vault::LocalVault>> {
    crate::vault::get().map_err(|e| no(codes::VAULT_CLOSED, 503, format!("the vault isn't open ({}): {}", e.code, e.hint)))
}

fn approve(who: &Who, body: &[u8], ingress: &str) -> R<J> {
    need_person_admin(who, "approve")?;
    // The clone gate: approving runs a clone under this person's credential.
    if let Err(resp) = crate::clone_gate::check(who, crate::clone_gate::Door::ProposalApprove, ingress) {
        let v: J = serde_json::from_str(&resp.body).unwrap_or(J::Null);
        let code = match v["code"].as_str() {
            Some(crate::clone_gate::codes::PERSONAL_OFF) => crate::clone_gate::codes::PERSONAL_OFF,
            _ => crate::clone_gate::codes::NOT_ALLOWED,
        };
        return Err(no(code, 403, v["hint"].as_str().unwrap_or("cloning isn't allowed for you here").to_string()));
    }
    let stamp = confirm_stamp(who).ok_or_else(|| no(codes::FORBIDDEN, 403, "only the owner or an admin approves"))?;
    let v = parse(body)?;
    let p = find(s(&v, "proposal"))?;
    let Some(token) = s(&v, "confirmToken") else {
        return Err(no(
            codes::CONFIRM_REQUIRED,
            403,
            "approving needs a fresh confirm (Touch ID, the vault password or your login password): k2 clone approve <id> asks for it",
        ));
    };
    let proof = vault()?
        .confirm
        .take(token, CONFIRM_ACTION, Some(&p.id), &stamp)
        .map_err(|_| no(codes::CONFIRM_REQUIRED, 403, "confirm again: a confirm works once, for this proposal, within 60 seconds"))?;
    let via = match proof {
        k2_vault::HumanProof::Confirmed { method } => method,
        k2_vault::HumanProof::None => return Err(no(codes::CONFIRM_REQUIRED, 403, "no confirm")),
    };
    let by = actor(who);
    let done = db(|c| Ok(cp::decide(c, &p.id, cp::STATUS_APPROVED, &by, now())))?.map_err(decide_err)?;
    audit(&by, format!("approve {} via {via}", done.id));
    notify::agent(
        &done.project_id,
        &format!("[k2 clone] {by} approved proposal {}; they are running the clone now", done.id),
    );
    let ws = ws_by_id(&done.project_id);
    let copies = done.options.secrets.values().filter(|c| **c == SecretChoice::Copy).count();
    Ok(json!({
        "ok": true,
        "proposal": proposal_json(&done),
        "via": via,
        "run": {
            "workspacePath": ws.as_ref().map(|w| w.path.clone()),
            "workspaceHandle": ws.as_ref().map(|w| w.handle.clone()),
            "target": done.target,
            "options": done.options,
            "proposalId": done.id,
        },
        "hint": if copies > 0 {
            "approved: run it now (k2 clone approve runs it for you). Copying secrets asks you to confirm once more."
        } else {
            "approved: run it now (k2 clone approve runs it for you)."
        },
    }))
}

// ── deny ─────────────────────────────────────────────────────────────

fn deny(who: &Who, body: &[u8]) -> R<J> {
    need_person_admin(who, "deny")?;
    let v = parse(body)?;
    let p = find(s(&v, "proposal"))?;
    let by = actor(who);
    let done = db(|c| Ok(cp::decide(c, &p.id, cp::STATUS_DENIED, &by, now())))?.map_err(decide_err)?;
    let reason = s(&v, "reason").map(|r| r.chars().take(cp::MAX_WHY).collect::<String>());
    audit(&by, format!("deny {}", done.id));
    notify::agent(
        &done.project_id,
        &format!(
            "[k2 clone] {by} denied proposal {}{}",
            done.id,
            reason.as_deref().map(|r| format!(": {r}")).unwrap_or_default()
        ),
    );
    Ok(json!({ "ok": true, "proposal": proposal_json(&done) }))
}

// ── finish ───────────────────────────────────────────────────────────

fn finish(who: &Who, body: &[u8]) -> R<J> {
    need_person_admin(who, "finish")?;
    let v = parse(body)?;
    let p = find(s(&v, "proposal"))?;
    let ok = v.get("ok").and_then(J::as_bool).ok_or_else(|| no(codes::BAD_REQUEST, 400, "ok is true or false"))?;
    let detail = s(&v, "detail").map(|d| d.chars().take(300).collect::<String>());
    let done = db(|c| Ok(cp::finish(c, &p.id, ok)))?.map_err(decide_err)?;
    let by = actor(who);
    audit(&by, format!("finish {} {}", done.id, done.status));
    notify::agent(
        &done.project_id,
        &format!(
            "[k2 clone] proposal {} {}{}",
            done.id,
            if ok { "is done: the clone is in place" } else { "failed" },
            detail.as_deref().map(|d| format!(" ({d})")).unwrap_or_default()
        ),
    );
    Ok(json!({ "ok": true, "proposal": proposal_json(&done) }))
}

#[cfg(test)]
#[path = "clone_proposal_routes_tests.rs"]
mod tests;
