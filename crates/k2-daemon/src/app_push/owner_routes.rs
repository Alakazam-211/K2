//! `/cli/app/push/*`: the owner's, Admins' and the App agent's push
//! controls (prd-custom-app-push-v1 CP36, `k2 app push …`). Not
//! App-facing: an App pass, an API key or an API session is refused.
//!
//! ```text
//! GET  /cli/app/push                ?app=          Admin; the App's agent (switch 1): state, key metadata, counts
//! POST /cli/app/push/credentials    {app, apns?: {p8, keyId, teamId, bundleId}, fcm?: {serviceAccountJson}, confirmToken}
//!                                                  the owner in person + a fresh confirm
//! POST /cli/app/push/credentials/remove {app, platform, confirmToken}   the same
//! POST /cli/app/push/check          {app}          the owner: one real call per platform to a fake token (CP16)
//! POST /cli/app/push/set            {app, enabled?, agentReplies?, previews?, maxDevices?}
//!                                                  on: the owner; off: Admin or the App's agent (switch 1);
//!                                                  agentReplies/previews: Admin or the App's agent (switch 2);
//!                                                  maxDevices: the owner
//! POST /cli/app/push/send           {app, to, title, body, …}   Admin, or the App's agent (switch 2) → 202
//! POST /cli/app/push/test           {app, to}      the same, fixed text "Test notification from K2"
//! GET  /cli/app/push/log            ?app=          Admin; the App's agent (switch 1): the last hour's sends, counts only
//! GET  /cli/app/push/devices        ?app=&guest=   Admin: platform, environment, channel, last seen; never tokens
//! POST /cli/app/push/devices/remove {app, id}      Admin
//! ```
//!
//! Keys (CP13, CP17, CP45): the owner token is readable by agents, so it is
//! not enough: every set or remove needs a fresh owner confirm (Touch ID,
//! the login or the vault password; vault action `app-push-keys`, bound to
//! the App's service id). An agent never sets, replaces, removes or reads a
//! key: it asks the owner with a permission card. Values are checked before
//! the confirm is spent, and never echoed.

use std::collections::HashMap;

use k2_core::connect_users::Role;
use k2_core::skin::app_push as store;
use serde_json::{json, Value as J};

use crate::apps::grace::{target, Target};
use crate::apps::routes::{actor, json_body, need_credential, no, respond, s, Refusal, Who, R};
use crate::cli_response::CliResponse;

pub const STATUS: &str = "/cli/app/push";
pub const CREDENTIALS: &str = "/cli/app/push/credentials";
pub const CREDENTIALS_REMOVE: &str = "/cli/app/push/credentials/remove";
pub const CHECK: &str = "/cli/app/push/check";
pub const SET: &str = "/cli/app/push/set";
pub const SEND: &str = "/cli/app/push/send";
pub const TEST: &str = "/cli/app/push/test";
pub const LOG: &str = "/cli/app/push/log";
/// The fixed text of `k2 app push test`.
pub const TEST_TEXT: &str = "Test notification from K2";
pub const DEVICES: &str = "/cli/app/push/devices";
pub const DEVICES_REMOVE: &str = "/cli/app/push/devices/remove";

pub const ROUTES: &[&str] = &[STATUS, CREDENTIALS, CREDENTIALS_REMOVE, CHECK, SET, SEND, TEST, LOG, DEVICES, DEVICES_REMOVE];
pub const POST_ROUTES: &[&str] = &[CREDENTIALS, CREDENTIALS_REMOVE, CHECK, SET, SEND, TEST, DEVICES_REMOVE];
/// Request bodies here are at most this many bytes (keys included).
pub const MAX_BODY: usize = 64 * 1024;

pub fn is_route(p: &str) -> bool {
    ROUTES.contains(&p)
}

pub fn is_post_route(p: &str) -> bool {
    POST_ROUTES.contains(&p)
}

pub fn too_large() -> CliResponse {
    respond(Err(no("too_large", 413, format!("request bodies here are at most {} KiB", MAX_BODY / 1024))))
}

// ── callers ──────────────────────────────────────────────────────────

/// The owner token, an Owner or Admin login, or a human in a plain K2
/// shell tab.
pub(crate) fn admin_human(who: &Who) -> bool {
    matches!(who, Who::Owner | Who::Shell { .. } | Who::Login { role: Role::Owner | Role::Admin, .. })
}

/// The owner in person (can pass a vault confirm).
pub(crate) fn owner_in_person(who: &Who) -> bool {
    matches!(who, Who::Owner | Who::Login { role: Role::Owner, .. })
}

fn admin_only(_t: &Target) -> Refusal {
    no("admin_only", 403, "push devices are the owner's and Admins' to see (an agent reads counts with k2 app push status)")
}

/// The App a call names, checked against the caller (a passport sees only
/// its own workspace's Apps; host is never a push App).
pub(crate) fn push_target(who: &Who, raw: Option<&str>) -> R<Target> {
    need_credential(who)?;
    if raw.map(str::trim) == Some(k2_core::skin::HOST_APP_ID) {
        return Err(no("not_for_host", 400, "push is for a published App's own users, never the box login (host)"));
    }
    target(who, raw)
}

fn device_json(d: &store::PushDevice) -> J {
    json!({
        "id": d.id,
        "username": d.username,
        "deviceId": d.device_id,
        "platform": d.platform,
        "environment": d.environment,
        "channel": d.channel,
        "previews": d.previews,
        "mutedTopics": d.muted_topics,
        "createdAt": d.created_at,
        "lastSeenAt": d.last_seen_at,
    })
}

fn principal_of(username: &str) -> R<Option<String>> {
    let u = k2_core::skin::normalize_username(username).map_err(|e| no("usage", 400, e))?;
    let all = k2_core::skin::list_principals().map_err(|e| no("store", 500, e))?;
    Ok(all.into_iter().find(|p| p.username == u).map(|p| p.id))
}

/// GET `/cli/app/push/devices?app=&guest=`.
fn devices(who: &Who, params: &HashMap<String, String>) -> R<J> {
    let t = push_target(who, params.get("app").map(String::as_str))?;
    if !admin_human(who) {
        return Err(admin_only(&t));
    }
    let list = match params.get("guest").or_else(|| params.get("user")).map(|s| s.trim()).filter(|s| !s.is_empty()) {
        Some(u) => match principal_of(u)? {
            Some(pid) => store::devices_for_app(&t.service_id, Some(&pid)).map_err(|e| no("store", 500, e))?,
            None => Vec::new(),
        },
        None => store::devices_for_app(&t.service_id, None).map_err(|e| no("store", 500, e))?,
    };
    let devices: Vec<J> = list.iter().map(device_json).collect();
    Ok(json!({ "ok": true, "app": t.service_id, "name": t.name, "count": devices.len(), "devices": devices }))
}

/// POST `/cli/app/push/devices/remove {app, id}`.
fn devices_remove(who: &Who, body: &[u8]) -> R<J> {
    let v = json_body(body)?;
    let t = push_target(who, s(&v, "app"))?;
    if !admin_human(who) {
        return Err(admin_only(&t));
    }
    let id = s(&v, "id").ok_or_else(|| no("usage", 400, "send {app, id} (an apd_… id from k2 app push devices)"))?;
    let removed = store::remove_by_owner(&t.service_id, id).map_err(|e| no("store", 500, e))?;
    if !removed {
        return Err(no("not_found", 404, "no push device of this App by that id"));
    }
    super::audit_line("app-push-device-remove", &actor(who), &format!("app={} device={id}", t.service_id));
    Ok(json!({ "ok": true, "removed": true }))
}

/// The App's own agent (its workspace's passport) at switch 1 or above.
pub(crate) fn app_agent_at(who: &Who, t: &Target, at: crate::apps::access::Level) -> bool {
    matches!(who, Who::Agent { project_id, .. } if *project_id == t.project_id && crate::apps::access::level(project_id) >= at)
}

/// GET `/cli/app/push?app=`: state, key metadata (never a key), counts.
fn status(who: &Who, params: &HashMap<String, String>) -> R<J> {
    let t = push_target(who, params.get("app").map(String::as_str))?;
    if !admin_human(who) && !app_agent_at(who, &t, crate::apps::access::Level::Build) {
        return Err(no("forbidden", 403, "push status is the owner's, Admins' and this App's own agent's (\"Build and run Apps\" on)"));
    }
    status_json(&t)
}

pub(crate) fn status_json(t: &Target) -> R<J> {
    let cfg = store::config(&t.service_id).map_err(|e| no("store", 500, e))?;
    let state = super::state(&t.service_id);
    let counts = store::device_counts(&t.service_id).map_err(|e| no("store", 500, e))?;
    let devices = store::devices_for_app(&t.service_id, None).map_err(|e| no("store", 500, e))?;
    let n = |f: &dyn Fn(&store::PushDevice) -> bool| devices.iter().filter(|d| f(d)).count();
    Ok(json!({
        "ok": true,
        "app": t.service_id,
        "name": t.name,
        "state": if state.is_ok() { "on" } else { "off" },
        "off": state.err().map(super::Off::as_str),
        "enabled": cfg.enabled,
        "ownerOn": cfg.owner_on,
        "approved": super::push_send_approved(&t.service_id),
        "agentReplies": cfg.agent_replies,
        "previews": cfg.previews,
        "maxDevices": cfg.max_devices.unwrap_or(store::DEFAULT_MAX_DEVICES_PER_APP),
        "apns": cfg.apns,
        "fcm": cfg.fcm,
        "credError": cfg.cred_error,
        "credErrorAt": cfg.cred_error_at,
        "devices": {
            "total": devices.len(),
            "guests": counts.len(),
            "apns": n(&|d| d.platform == store::PLATFORM_APNS),
            "fcm": n(&|d| d.platform == store::PLATFORM_FCM),
            "sandbox": n(&|d| d.environment == store::ENV_SANDBOX),
            "production": n(&|d| d.environment == store::ENV_PRODUCTION),
        },
    }))
}

fn key_refusal(e: super::creds::KeyError) -> Refusal {
    match e {
        super::creds::KeyError::VaultClosed => no("vault_closed", 503, "the vault isn't open yet; try again when K2 has finished starting"),
        super::creds::KeyError::NotConfigured => no("push_not_configured", 409, "this App has no key for that platform"),
        super::creds::KeyError::Vault(c) => no("vault_error", 500, format!("the vault refused ({c})")),
    }
}

/// The owner in person, with a fresh confirm bound to this App. Anyone
/// else: `owner_only` (an agent asks with a permission card).
fn keys_confirm(who: &Who, t: &Target, v: &J) -> R<()> {
    if !owner_in_person(who) {
        return Err(no(
            "owner_only",
            403,
            "an App's push keys are the owner's: set or remove them in person (k2 app push setup from a terminal outside K2). \
             An agent asks with a permission card.",
        ));
    }
    let stamp = match who {
        Who::Owner => "owner".to_string(),
        Who::Login { username, .. } => format!("connect:{username}"),
        _ => String::new(),
    };
    let Some(token) = s(v, "confirmToken") else {
        return Err(Refusal {
            extra: Some(json!({ "serviceId": t.service_id })),
            ..no(
                "vault_confirm_required",
                403,
                "push keys are credentials: setting or removing them needs a fresh owner confirm (Touch ID, your login or vault password). \
                 k2 app push setup asks for it from a terminal outside K2.",
            )
        });
    };
    let vault = crate::vault::get().map_err(|_| no("vault_closed", 503, "the vault isn't open yet"))?;
    vault
        .confirm
        .take(token, super::creds::CONFIRM_ACTION, Some(&t.service_id), &stamp)
        .map_err(|_| no("vault_confirm_required", 403, "confirm again: a confirm works once, for this App's push keys, within 60 seconds"))?;
    Ok(())
}

fn field<'a>(v: &'a J, k: &str) -> &'a str {
    v.get(k).and_then(J::as_str).unwrap_or("")
}

/// POST `/cli/app/push/credentials`.
fn credentials(who: &Who, body: &[u8]) -> R<J> {
    let v = json_body(body)?;
    let t = push_target(who, s(&v, "app"))?;
    if !owner_in_person(who) {
        return keys_confirm(who, &t, &v).map(|_| J::Null);
    }
    let apns = match v.get("apns") {
        None | Some(J::Null) => None,
        Some(a @ J::Object(_)) => Some(
            super::creds::validate_apns(field(a, "p8"), field(a, "keyId"), field(a, "teamId"), field(a, "bundleId")).map_err(|e| no("usage", 400, e))?,
        ),
        Some(_) => return Err(no("usage", 400, "apns is {p8, keyId, teamId, bundleId}")),
    };
    let fcm = match v.get("fcm") {
        None | Some(J::Null) => None,
        Some(f @ J::Object(_)) => Some(super::creds::validate_fcm(field(f, "serviceAccountJson")).map_err(|e| no("usage", 400, e))?),
        Some(_) => return Err(no("usage", 400, "fcm is {serviceAccountJson}")),
    };
    if apns.is_none() && fcm.is_none() {
        return Err(no("usage", 400, "send apns {p8, keyId, teamId, bundleId} and/or fcm {serviceAccountJson}"));
    }
    keys_confirm(who, &t, &v)?;
    let by = actor(who);
    let mut set = Vec::new();
    if let Some(k) = &apns {
        super::creds::store_apns(&t.service_id, k, &by).map_err(key_refusal)?;
        set.push(format!("APNs key {} (team {}, bundle {}, fingerprint {})", k.key_id, k.team_id, k.bundle_id, k.fingerprint()));
    }
    if let Some(k) = &fcm {
        super::creds::store_fcm(&t.service_id, k, &by).map_err(key_refusal)?;
        set.push(format!("Firebase service account {} (project {}, fingerprint {})", k.client_email, k.project_id, k.fingerprint()));
    }
    super::sender::global().reset_app(&t.service_id);
    super::audit_line("app-push-keys-set", &by, &format!("app={} {}", t.service_id, set.join("; ")));
    crate::apps::alerts::note(
        &t.project_id,
        "App push keys changed",
        &format!("The owner set push keys for {} ({}): {}. You can send with k2 app push send; you can't read the keys.", t.name, by, set.join("; ")),
    );
    let mut out = status_json(&t)?;
    out["set"] = json!(set);
    Ok(out)
}

/// POST `/cli/app/push/credentials/remove {app, platform}`.
fn credentials_remove(who: &Who, body: &[u8]) -> R<J> {
    let v = json_body(body)?;
    let t = push_target(who, s(&v, "app"))?;
    let platforms: Vec<&str> = match s(&v, "platform") {
        Some("apns") => vec![store::PLATFORM_APNS],
        Some("fcm") => vec![store::PLATFORM_FCM],
        Some("all") => vec![store::PLATFORM_APNS, store::PLATFORM_FCM],
        _ => return Err(no("usage", 400, "send {app, platform: apns|fcm|all}")),
    };
    keys_confirm(who, &t, &v)?;
    let by = actor(who);
    let mut removed = Vec::new();
    for p in platforms {
        if super::creds::remove(&t.service_id, p, &by).map_err(key_refusal)? {
            removed.push(p);
        }
    }
    super::sender::global().reset_app(&t.service_id);
    super::audit_line("app-push-keys-remove", &by, &format!("app={} removed={}", t.service_id, removed.join(",")));
    if !removed.is_empty() {
        crate::apps::alerts::note(
            &t.project_id,
            "App push keys changed",
            &format!("The owner removed {} push key(s) for {} ({by}); sends to those phones answer push_not_configured.", removed.join(" and "), t.name),
        );
    }
    let mut out = status_json(&t)?;
    out["removed"] = json!(removed);
    Ok(out)
}

/// POST `/cli/app/push/check {app}`: the owner only (it calls Apple and
/// Google). Never in tests against the vendors: tests fake the transport.
fn check(who: &Who, body: &[u8]) -> R<J> {
    let v = json_body(body)?;
    let t = push_target(who, s(&v, "app"))?;
    if !owner_in_person(who) && !matches!(who, Who::Shell { .. }) {
        return Err(no("owner_only", 403, "checking an App's push keys calls Apple and Google: the owner runs k2 app push check"));
    }
    if crate::push_routes::airgap_blocks() {
        return Err(no("airgap", 503, "air-gap is on: nothing leaves this server"));
    }
    crate::vault::get().map_err(|_| no("vault_closed", 503, "the vault isn't open yet"))?;
    let cfg = store::config(&t.service_id).map_err(|e| no("store", 500, e))?;
    let mut results = serde_json::Map::new();
    for (platform, set) in [(store::PLATFORM_APNS, cfg.apns.is_some()), (store::PLATFORM_FCM, cfg.fcm.is_some())] {
        let limits = super::sender::CHECK_LIMITS.iter().find(|(p, _)| *p == platform).map(|(_, t)| *t).unwrap_or("");
        let r = if !set {
            json!({ "configured": false })
        } else {
            match super::sender::global().check(&t.service_id, platform) {
                Ok(_) => json!({ "configured": true, "ok": true, "limits": limits }),
                Err(code) => json!({ "configured": true, "ok": false, "code": code, "limits": limits }),
            }
        };
        results.insert(platform.to_string(), r);
    }
    super::audit_line("app-push-check", &actor(who), &format!("app={} {}", t.service_id, J::Object(results.clone())));
    Ok(json!({ "ok": true, "app": t.service_id, "name": t.name, "results": results }))
}

fn bool_opt(v: &J, k: &str) -> R<Option<bool>> {
    match v.get(k) {
        None | Some(J::Null) => Ok(None),
        Some(J::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(no("usage", 400, format!("{k} must be true or false"))),
    }
}

/// POST `/cli/app/push/set`.
fn set(who: &Who, body: &[u8]) -> R<J> {
    use crate::apps::access::Level;
    let v = json_body(body)?;
    let t = push_target(who, s(&v, "app"))?;
    let enabled = bool_opt(&v, "enabled")?;
    let agent_replies = bool_opt(&v, "agentReplies")?;
    let previews = bool_opt(&v, "previews")?;
    let max_devices = match v.get("maxDevices") {
        None | Some(J::Null) => None,
        Some(J::Number(n)) => Some(n.as_i64().ok_or_else(|| no("usage", 400, "maxDevices is a whole number"))?),
        Some(_) => return Err(no("usage", 400, "maxDevices is a whole number")),
    };
    if enabled.is_none() && agent_replies.is_none() && previews.is_none() && max_devices.is_none() {
        return Err(no("usage", 400, "send {app} with enabled, agentReplies, previews and/or maxDevices"));
    }
    // On (and raising the device ceiling) is the owner's approval of push
    // for this App (CP36): never an agent, never an Admin.
    let owner = owner_in_person(who) || matches!(who, Who::Shell { .. });
    if (enabled == Some(true) || max_devices.is_some()) && !owner {
        return Err(no(
            "owner_only",
            403,
            "turning an App's notifications on is the owner's approval (k2 app push on <app>); an agent asks with a permission card",
        ));
    }
    if enabled == Some(false) && !admin_human(who) && !app_agent_at(who, &t, Level::Build) {
        return Err(no("forbidden", 403, "turning notifications off is for the owner, Admins and this App's own agent"));
    }
    if (agent_replies.is_some() || previews.is_some()) && !admin_human(who) && !app_agent_at(who, &t, Level::Full) {
        return Err(no(
            "forbidden",
            403,
            "agent replies and previews are set by the owner, Admins, or this App's own agent with \"…and manage them end to end\" on",
        ));
    }
    let by = actor(who);
    let before = store::config(&t.service_id).map_err(|e| no("store", 500, e))?;
    let sw = store::Switches {
        enabled,
        owner_on: if enabled == Some(true) { Some(true) } else { None },
        agent_replies,
        previews,
    };
    store::set_switches(&t.service_id, sw, &by).map_err(|e| no("usage", 400, e))?;
    if let Some(m) = max_devices {
        store::set_max_devices(&t.service_id, Some(m), &by).map_err(|e| no("usage", 400, e))?;
    }
    if let Some(e) = enabled.filter(|e| *e != (before.enabled && (before.owner_on || super::push_send_approved(&t.service_id)))) {
        super::audit_line("app-push-state", &by, &format!("app={} enabled={e}", t.service_id));
        crate::apps::alerts::note(
            &t.project_id,
            "App notifications changed",
            &format!("Notifications for {} are now {} (by {by}).", t.name, if e { "on" } else { "off" }),
        );
    }
    if agent_replies.is_some_and(|a| a != before.agent_replies) || previews.is_some_and(|p| p != before.previews) {
        super::audit_line("app-push-settings", &by, &format!("app={} agentReplies={:?} previews={:?}", t.service_id, agent_replies, previews));
    }
    status_json(&t)
}

/// May `who` send for this App (CP36: Admin, or the App's agent at switch 2)?
fn may_send(who: &Who, t: &Target) -> R<()> {
    if admin_human(who) || app_agent_at(who, t, crate::apps::access::Level::Full) {
        return Ok(());
    }
    Err(no(
        "forbidden",
        403,
        "sending is for the owner, Admins, or this App's own agent with \"…and manage them end to end\" on",
    ))
}

fn send_refusal(e: super::send::SendErr) -> Refusal {
    Refusal { code: e.code, status: e.status, hint: e.hint, extra: Some(J::Object(e.extra)) }
}

/// POST `/cli/app/push/send {app, to, title, body, …}`.
fn send(who: &Who, body: &[u8]) -> R<J> {
    let mut v = json_body(body)?;
    let t = push_target(who, s(&v, "app"))?;
    may_send(who, &t)?;
    if let Some(o) = v.as_object_mut() {
        o.remove("app");
    }
    let to = super::send::parse_to(&v).map_err(send_refusal)?;
    let n = super::payload::parse(&v).map_err(|r| send_refusal(r.into()))?;
    let caller = super::send::Caller { actor: actor(who), kind: super::payload::KIND_APP };
    let mut out = super::send::send(&t.service_id, &caller, &to, n).map_err(send_refusal)?;
    out["status"] = json!("queued");
    Ok(out)
}

/// POST `/cli/app/push/test {app, to}`: the fixed text.
fn test(who: &Who, body: &[u8]) -> R<J> {
    let v = json_body(body)?;
    let t = push_target(who, s(&v, "app"))?;
    may_send(who, &t)?;
    let to = super::send::parse_to(&v).map_err(send_refusal)?;
    let n = super::payload::parse(&json!({ "title": TEST_TEXT, "body": TEST_TEXT, "topic": "k2.test" })).map_err(|r| send_refusal(r.into()))?;
    let caller = super::send::Caller { actor: actor(who), kind: super::payload::KIND_APP };
    let mut out = super::send::send(&t.service_id, &caller, &to, n).map_err(send_refusal)?;
    out["status"] = json!("queued");
    Ok(out)
}

/// GET `/cli/app/push/log?app=`.
fn log(who: &Who, params: &HashMap<String, String>) -> R<J> {
    let t = push_target(who, params.get("app").map(String::as_str))?;
    if !admin_human(who) && !app_agent_at(who, &t, crate::apps::access::Level::Build) {
        return Err(no("forbidden", 403, "the send log is the owner's, Admins' and this App's own agent's"));
    }
    let sends: Vec<J> = super::sendlog::list(&t.service_id).iter().map(super::send::record_json).collect();
    Ok(json!({ "ok": true, "app": t.service_id, "name": t.name, "pending": super::worker::pending(&t.service_id), "sends": sends }))
}

pub fn handle(path: &str, who: Who, body: &[u8], params: &HashMap<String, String>) -> CliResponse {
    if is_post_route(path) {
        if let Err(e) = crate::apps::access::member_read_only(&who) {
            return respond(Err(e));
        }
    }
    let r = match path {
        STATUS => status(&who, params),
        CREDENTIALS => credentials(&who, body),
        CREDENTIALS_REMOVE => credentials_remove(&who, body),
        CHECK => check(&who, body),
        SET => set(&who, body),
        SEND => send(&who, body),
        TEST => test(&who, body),
        LOG => log(&who, params),
        DEVICES => devices(&who, params),
        DEVICES_REMOVE => devices_remove(&who, body),
        _ => Err(no("not_found", 404, "no such App push route")),
    };
    match r {
        Ok(v) if v.get("status").and_then(J::as_str) == Some("queued") => {
            CliResponse { status: "202 Accepted", content_type: "application/json", body: v.to_string() }
        }
        other => respond(other),
    }
}

#[cfg(test)]
#[path = "owner_routes_tests.rs"]
mod tests;
