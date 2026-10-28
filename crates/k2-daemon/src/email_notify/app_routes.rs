//! `/cli/app/email/*`: one App's email (`k2 app email …`), for the owner,
//! Admins and the App's own agent (prd-email-notifications-core-v1 §5.3).
//! Not App-facing: an App pass, an API key or an API session is refused
//! (the App's own door is `/cli/skin/email/*` and `/v1/apps/<id>/email/*`).
//!
//! ```text
//! GET  /cli/app/email          ?app=          the owner, Admins; the App's agent ("Build and run Apps")
//! POST /cli/app/email/on       {app, sender, scope?, confirmToken, confirmAnyToken?}
//!                                             the owner in person + a fresh confirm (scope any: a second one)
//! POST /cli/app/email/off      {app}          the owner, Admins, the App's agent
//! POST /cli/app/email/test     {app}          the same: K2's fixed text to the owner's own address only
//! POST /cli/app/email/send     {app, to, subject, text|html, …}
//!                                             the owner, Admins (sent as theirs), or the App's agent with
//!                                             "…and manage them end to end" under its agentSend gate (E15)
//! GET  /cli/app/email/log      ?app=&limit=&status=&tag=
//! GET  /cli/app/email/status   ?app=&id=
//! ```

use std::collections::HashMap;

use k2_core::connect_users::Role;
use k2_core::email_notify as store;
use serde_json::{json, Value as J};

use super::{db_fail, fail, respond, Fail, R};
use crate::apps::grace::Target;
use crate::cli_response::CliResponse;
use crate::compute_routes::Who;

pub const SHOW: &str = "/cli/app/email";
pub const ON: &str = "/cli/app/email/on";
pub const OFF: &str = "/cli/app/email/off";
pub const TEST: &str = "/cli/app/email/test";
pub const SEND: &str = "/cli/app/email/send";
pub const LOG: &str = "/cli/app/email/log";
pub const STATUS: &str = "/cli/app/email/status";

pub const ROUTES: &[&str] = &[SHOW, ON, OFF, TEST, SEND, LOG, STATUS];
pub const POST_ROUTES: &[&str] = &[ON, OFF, TEST, SEND];

pub fn is_route(p: &str) -> bool {
    ROUTES.contains(&p)
}

pub fn is_post_route(p: &str) -> bool {
    POST_ROUTES.contains(&p)
}

fn from_refusal(r: crate::apps::routes::Refusal) -> Fail {
    Fail { status: r.status, code: r.code, hint: r.hint, retry_after_ms: None }
}

/// The App a call names, checked against the caller (a passport sees only
/// its own workspace's Apps; `host` is never an email App).
fn target(who: &Who, raw: Option<&str>) -> R<Target> {
    crate::apps::routes::need_credential(who).map_err(from_refusal)?;
    if raw.map(str::trim) == Some(k2_core::skin::HOST_APP_ID) {
        return Err(fail(400, "not_for_host", "App email is for a published App, never the box login (host): K2's own mail uses the system sender"));
    }
    crate::apps::grace::target(who, raw).map_err(from_refusal)
}

/// A person who sends as themselves (no agent gate): the owner token or
/// an Owner/Admin login. A plain K2 shell tab is NOT one here (security
/// review L1): an agent can type into a shell tab, so a shell passport
/// counts as its workspace's agent ([`agent_at`]).
fn person(who: &Who) -> bool {
    matches!(who, Who::Owner | Who::Login { role: Role::Owner | Role::Admin, .. })
}

fn owner_in_person(who: &Who) -> bool {
    matches!(who, Who::Owner | Who::Login { role: Role::Owner, .. })
}

/// The App's own agent at `at`: its workspace's harness passport, or a
/// plain shell tab in that workspace (L1: same switches, same gate).
fn agent_at(who: &Who, t: &Target, at: crate::apps::access::Level) -> bool {
    match who {
        Who::Shell { project_id, .. } => *project_id == t.project_id && crate::apps::access::level(project_id) >= at,
        _ => crate::app_push::owner_routes::app_agent_at(who, t, at),
    }
}

fn may_read(who: &Who, t: &Target) -> R<()> {
    if person(who) || agent_at(who, t, crate::apps::access::Level::Build) {
        return Ok(());
    }
    Err(fail(403, "forbidden", "an App's email is the owner's, Admins' and the App's own agent's (\"Build and run Apps\" on)"))
}

fn body(b: &[u8]) -> R<J> {
    if b.iter().all(|c| c.is_ascii_whitespace()) {
        return Ok(json!({}));
    }
    match serde_json::from_slice::<J>(b) {
        Ok(v @ J::Object(_)) => Ok(v),
        _ => Err(fail(400, "usage", "the body is a JSON object")),
    }
}

fn s<'a>(v: &'a J, k: &str) -> Option<&'a str> {
    v.get(k).and_then(J::as_str).map(str::trim).filter(|x| !x.is_empty())
}

fn take_confirm(who: &Who, token: Option<&str>, action: &str, binding: &str) -> R<()> {
    let stamp = match who {
        Who::Owner => "owner".to_string(),
        Who::Login { username, .. } => format!("connect:{username}"),
        _ => String::new(),
    };
    let Some(token) = token else {
        return Err(Fail {
            status: 403,
            code: "vault_confirm_required",
            hint: format!(
                "turning an App's email on needs a fresh owner confirm (Touch ID, your login or the vault password){}; \
                 k2 app email on asks for it from a terminal outside K2",
                if action == super::CONFIRM_APP_ANY { ", and a second one for the any-address scope" } else { "" }
            ),
            retry_after_ms: None,
        });
    };
    let vault = crate::vault::get().map_err(|_| fail(503, "vault_closed", "the vault isn't open yet"))?;
    vault
        .confirm
        .take(token, action, Some(binding), &stamp)
        .map_err(|_| fail(403, "vault_confirm_required", "confirm again: a confirm works once, for this App and this sender, within 60 seconds"))?;
    Ok(())
}

/// POST `/cli/app/email/on`: the owner grants the App a sender (E20: one
/// owner per address; never the system sender) and a recipient scope.
fn on(who: &Who, v: &J) -> Result<J, CliResponse> {
    let t = target(who, s(v, "app")).map_err(|f| super::fail_response(&f))?;
    if !owner_in_person(who) {
        return Err(super::fail_response(&fail(
            403,
            "owner_only",
            &format!(
                "turning an App's email on is the owner's, in person: k2 app email on {} --sender <address> from a terminal \
                 outside K2. An agent asks the owner (a permission card or a Thread message naming the sender).",
                t.name
            ),
        )));
    }
    let scope = match s(v, "scope").unwrap_or(store::SCOPE_USERS) {
        store::SCOPE_USERS => store::SCOPE_USERS,
        store::SCOPE_ANY => store::SCOPE_ANY,
        _ => return Err(super::fail_response(&fail(400, "usage", "scope is users (default) or any"))),
    };
    let sender_raw = s(v, "sender").ok_or_else(|| super::fail_response(&fail(400, "usage", "send {app, sender} (a sender from k2 notifications email sender list)")))?;
    let sender = super::senders::find_live(sender_raw).map_err(|f| super::fail_response(&f))?;
    if sender.is_system() {
        return Err(super::fail_response(&fail(409, "system_sender", "the system sender is K2's own; an App gets its own sender (k2 notifications email sender add)")));
    }
    if let Some(other) = sender.app().filter(|a| *a != t.service_id) {
        return Err(super::fail_response(&fail(409, "sender_in_use", &format!("{} is App {other}'s sender (one App per address)", sender.address))));
    }
    // N10: a workspace's notice sender is that workspace's alone.
    let agent_owner = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::agent_for_sender(&conn, &sender.id).map_err(|e| super::fail_response(&super::db_fail(e)))?
    };
    if let Some(ws) = agent_owner {
        return Err(super::fail_response(&fail(409, "sender_in_use", &format!("{} is workspace {ws}'s notice sender (one owner per address)", sender.address))));
    }
    // L11: the confirm binds the App AND this sender.
    let binding = super::app_binding(&t.service_id, &sender.address);
    let needed = |f: Fail, action: &str| -> CliResponse {
        let mut r = super::fail_response(&f);
        if f.code == "vault_confirm_required" {
            if let Ok(mut b) = serde_json::from_str::<J>(&r.body) {
                b["error"]["action"] = json!(action);
                b["error"]["serviceId"] = json!(t.service_id);
                b["error"]["secretId"] = json!(binding);
                r.body = b.to_string();
            }
        }
        r
    };
    take_confirm(who, s(v, "confirmToken"), super::CONFIRM_APP_ON, &binding).map_err(|f| needed(f, super::CONFIRM_APP_ON))?;
    if scope == store::SCOPE_ANY {
        take_confirm(who, s(v, "confirmAnyToken"), super::CONFIRM_APP_ANY, &binding).map_err(|f| needed(f, super::CONFIRM_APP_ANY))?;
    }
    let by = crate::apps::routes::actor(who);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::set_app_on(&conn, &t.service_id, &sender.id, scope, &by, super::now())
            .map_err(|e| super::fail_response(&fail(409, "sender_in_use", &e)))?;
    }
    super::audit_line("app-email-on", &by, &format!("app={} sender={} scope={scope}", t.service_id, sender.id));
    crate::apps::alerts::note(
        &t.project_id,
        "App email on",
        &format!(
            "The owner turned email on for {} from {} (scope {scope}). Send with k2 app email send or email.send; you can't read the sender's mailbox.",
            t.name, sender.address
        ),
    );
    super::app_state_json(&t.service_id).map_err(|f| super::fail_response(&f))
}

/// POST `/cli/app/email/off`: narrowing, open to the App's agent.
fn off(who: &Who, v: &J) -> R<J> {
    let t = target(who, s(v, "app"))?;
    may_read(who, &t)?;
    let by = crate::apps::routes::actor(who);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::set_app_off(&conn, &t.service_id, &format!("off by {by}"), super::now()).map_err(db_fail)?;
    }
    super::audit_line("app-email-off", &by, &format!("app={}", t.service_id));
    super::app_state_json(&t.service_id)
}

/// The fixed text of `k2 app email test`.
pub const TEST_SUBJECT: &str = "Test email from K2";

/// POST `/cli/app/email/test`: K2's fixed text from the App's sender to the
/// owner's own address (never anyone else's).
fn test(who: &Who, v: &J) -> R<J> {
    let t = target(who, s(v, "app"))?;
    may_read(who, &t)?;
    let (st, sender, owner_email) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let st = store::app_state(&conn, &t.service_id).map_err(db_fail)?;
        let sender = match st.sender_id.as_deref() {
            Some(id) => store::get_sender(&conn, id).map_err(db_fail)?,
            None => None,
        };
        (st, sender, store::settings(&conn).map_err(db_fail)?.owner_email)
    };
    if !st.enabled {
        return Err(fail(403, "email_off", "this App's email is off (k2 app email on)"));
    }
    let Some(sender) = sender.filter(|s| s.is_live()) else { return Err(fail(409, "no_sender", "this App has no sender")) };
    let to = owner_email.ok_or_else(|| fail(409, "no_owner_email", "the owner hasn't set their own address (k2 notifications email settings --owner-email); tests only ever go there"))?;
    let c = super::send::Composed { subject: TEST_SUBJECT.to_string(), text: super::relay::TEST_TEXT.to_string(), html: None };
    let warm = super::budget::warming_up(&sender, super::now()).then_some(sender.domain.as_str());
    let id = super::send::send_fixed(&sender, store::ORIGIN_AGENT, "test", Some(&t.service_id), &to, &c, &super::budget::Kind::App { app: &t.service_id, scope_any: false, warmup_domain: warm })?;
    super::audit_line("app-email-test", &crate::apps::routes::actor(who), &format!("app={} log={id}", t.service_id));
    Ok(json!({ "ok": true, "id": id, "to": "the owner's own address", "result": "accepted for delivery" }))
}

/// POST `/cli/app/email/send`.
fn send(who: &Who, v: &J) -> R<J> {
    let mut v = v.clone();
    let t = target(who, s(&v, "app"))?;
    if let Some(o) = v.as_object_mut() {
        o.remove("app");
    }
    let actor = crate::apps::routes::actor(who);
    let caller = if person(who) {
        super::send::Caller::Person { app: t.service_id.clone(), actor }
    } else if agent_at(who, &t, crate::apps::access::Level::Full) {
        let path = crate::workspace_msg::resolve_workspace(&t.project_id).ok_or_else(|| fail(404, "not_found", "the App's workspace is gone"))?;
        super::send::Caller::Agent { app: t.service_id.clone(), actor, project_id: t.project_id.clone(), project_path: path }
    } else {
        return Err(fail(
            403,
            "forbidden",
            "sending is for the owner, Admins, or this App's own agent with \"…and manage them end to end\" on (under its email sending setting)",
        ));
    };
    super::send::send(&caller, &v, super::now())
}

fn log(who: &Who, params: &HashMap<String, String>) -> R<J> {
    let t = target(who, params.get("app").map(String::as_str))?;
    may_read(who, &t)?;
    let limit = params.get("limit").and_then(|l| l.parse().ok());
    let before = params.get("before").and_then(|l| l.parse().ok());
    super::send::log(&t.service_id, limit, before, params.get("status").map(String::as_str), params.get("tag").map(String::as_str))
}

fn status(who: &Who, params: &HashMap<String, String>) -> R<J> {
    let t = target(who, params.get("app").map(String::as_str))?;
    may_read(who, &t)?;
    let id = params.get("id").map(|s| s.trim()).filter(|s| !s.is_empty()).ok_or_else(|| fail(400, "usage", "send ?id= (from email.send)"))?;
    super::send::status(&t.service_id, id)
}

fn show(who: &Who, params: &HashMap<String, String>) -> R<J> {
    let t = target(who, params.get("app").map(String::as_str))?;
    may_read(who, &t)?;
    let mut v = super::app_state_json(&t.service_id)?;
    v["ok"] = json!(true);
    v["name"] = json!(t.name);
    Ok(v)
}

pub fn handle(path: &str, who: Who, is_post: bool, body_bytes: &[u8], params: &HashMap<String, String>) -> CliResponse {
    if is_post_route(path) && !is_post {
        return CliResponse::method_not_allowed();
    }
    if is_post {
        if let Err(e) = crate::apps::access::member_read_only(&who) {
            return crate::apps::routes::refusal_response(&e);
        }
    }
    let v = if is_post {
        match body(body_bytes) {
            Ok(v) => v,
            Err(f) => return super::fail_response(&f),
        }
    } else {
        J::Null
    };
    match path {
        SHOW => respond(show(&who, params)),
        ON => match on(&who, &v) {
            Ok(j) => respond(Ok(j)),
            Err(r) => r,
        },
        OFF => respond(off(&who, &v)),
        TEST => respond(test(&who, &v)),
        SEND => respond(send(&who, &v)),
        LOG => respond(log(&who, params)),
        STATUS => respond(status(&who, params)),
        _ => super::fail_response(&fail(404, "not_found", "no such App email route")),
    }
}
