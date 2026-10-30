//! Sew decision 13 (Rosson 2026-10-28): the owner's home-client settings,
//! K2's half. Sew 0.4.1 scopes each session by its pass's `app` claim; the
//! install's **home client** (Hearth, usually) sees every Space a person
//! is in, minus Apps the owner marked **App-only** (`hidden_from_home`).
//!
//! ```text
//! GET  /cli/sew/home                         the settings and whether Sew has them
//! POST /cli/sew/home/client {app | null}     the install's home client (null clears)
//! POST /cli/sew/home/app    {app, hidden}    an App's "App-only" mark
//! ```
//!
//! Feature flags, never versions (Sew 2026-10-28): the home client and
//! the App-only mark go once Sew reports `home_client`; each chat App's
//! signalling origin (`wss://<its public host>`, which voice for a custom
//! App needs) once it reports `app_origin` ([`refresh_origins`] keeps it
//! current from the App's bindings on every sweep).
//!
//! Gates: reads take any real credential; writes are POST-only, Admin
//! floor in `route_policy`, and:
//! - **the home client** (set or clear) is the owner's: the owner token,
//!   an Owner login or a K2 shell tab ([`need_owner_person`]). Never an
//!   Admin login and never an agent passport, "Manage Sew" or not: the home
//!   client's sessions see every Space each person is in (and Sew's A23
//!   keeps the install role only for the home client), so an agent naming
//!   its own App would hand that App every person's chat. Not requestable
//!   (no card). Every change and every refusal is audited
//!   (`sew-home-client`), and the change goes to Sew with `Sew-Actor` = who
//!   made it. Sew 0.5.1's `home_client_owner_approval` (an owner pass on
//!   the call) is later work (0.46.7).
//! - **an App's "App-only" mark**: turning it on only narrows what the
//!   home client sees, so Manage Sew (owner, an Owner/Admin login, an
//!   agent with `chat_manage`) as before; turning it off widens it again,
//!   so that is a person's (the owner token, an Owner/Admin login or a K2
//!   shell tab), never an agent's.
//!
//! Only the daemon talks to Sew: the settings are stored
//! (`k2_core::tool_hook::home`) and the S27 worker's sweep delivers them
//! over mgmt. An App names a published service (its id is the pass's `app`
//! claim and the Space binding's `app_id`).

use std::collections::HashMap;

use k2_core::connect_users::Role;
use serde_json::{json, Value};

use super::routes::{need_manage, Who};
use super::supervisor::{no, R};

pub const HOME: &str = "/cli/sew/home";
pub const HOME_CLIENT: &str = "/cli/sew/home/client";
pub const HOME_APP: &str = "/cli/sew/home/app";

pub const ROUTES: &[&str] = &[HOME, HOME_CLIENT, HOME_APP];
pub const POST_ROUTES: &[&str] = &[HOME_CLIENT, HOME_APP];

pub fn is_route(p: &str) -> bool {
    ROUTES.contains(&p)
}

pub fn is_post_route(p: &str) -> bool {
    POST_ROUTES.contains(&p)
}

/// The App a person names, as Sew knows it: `(service id, name)`. Takes
/// the service id (what Sew's `app_id` and the pass's `app` claim are), a
/// runtime App's id (`k2 app list`), or a published service's name when
/// exactly one has it. Anything else is a 404 naming `k2 app list`.
pub fn resolve_app(token: &str) -> R<(String, String)> {
    let t = token.trim();
    if t.is_empty() {
        return Err(no("bad_request", 400, "name the App (its id from k2 app list)"));
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    let dbe = |e: rusqlite::Error| no("db_error", 500, e.to_string());
    if let Some(r) = k2_core::published_services::get_by_id(&conn, &t.to_ascii_lowercase()).map_err(dbe)? {
        return Ok((r.id, r.name));
    }
    if let Some(a) = k2_core::deploy::store::get_app(&conn, t).map_err(dbe)? {
        if let Some(r) = k2_core::published_services::get_by_id(&conn, &a.service_id).map_err(dbe)? {
            return Ok((r.id, r.name));
        }
    }
    let named: Vec<_> = k2_core::published_services::list_all(&conn).map_err(dbe)?.into_iter().filter(|r| r.name == t).collect();
    match named.as_slice() {
        [one] => Ok((one.id.clone(), one.name.clone())),
        [] => Err(no("not_found", 404, format!("no App '{t}' on this box (its id: k2 app list)"))),
        _ => Err(no("bad_request", 400, format!("more than one App is named '{t}': name it by id (k2 app list --json, serviceId)"))),
    }
}

fn app(id: &str) -> R<(String, String)> {
    resolve_app(id)
}

/// Where a person does what an agent is refused here.
const PERSON_WHERE: &str = "Settings → Sew, or k2 hostchat home from a terminal outside K2 or a K2 shell tab";

/// Audit event of every home-client change and refusal.
pub const AUDIT_HOME_CLIENT: &str = "sew-home-client";

fn agent_refusal(what: &str) -> super::supervisor::Fail {
    no(
        "owner_only",
        403,
        format!("{what} is a person's, never an agent's (Manage Sew does not cover it). A person does it: {PERSON_WHERE}"),
    )
}

fn bad_credential(who: &Who) -> Option<super::supervisor::Fail> {
    match who {
        Who::Invalid => Some(no("forbidden", 403, "invalid or missing token")),
        Who::Wrong => Some(no("wrong_credential", 403, "App passes, API keys and API sessions can't manage Sew")),
        _ => None,
    }
}

/// The home-client gate: the owner (the owner token, an Owner login or a
/// K2 shell tab). An Admin login and every agent passport are refused.
fn need_owner_person(who: &Who) -> R<()> {
    if let Some(f) = bad_credential(who) {
        return Err(f);
    }
    match who {
        Who::Owner | Who::Login { role: Role::Owner, .. } | Who::Shell { .. } => Ok(()),
        Who::Agent { .. } => Err(agent_refusal("the home client")),
        _ => Err(no("owner_only", 403, "only the server's owner sets or clears the home client")),
    }
}

/// Turning an App's "App-only" mark off: a person (the owner token, an
/// Owner/Admin login or a K2 shell tab), never an agent.
fn need_admin_person(who: &Who) -> R<()> {
    if let Some(f) = bad_credential(who) {
        return Err(f);
    }
    match who {
        Who::Owner | Who::Login { role: Role::Owner | Role::Admin, .. } | Who::Shell { .. } => Ok(()),
        Who::Agent { .. } => Err(agent_refusal("turning an App's App-only mark off")),
        _ => Err(no("owner_only", 403, "only the server's owner or an admin turns an App's App-only mark off")),
    }
}

fn audit_home_client(who: &Who, outcome: &str) {
    crate::vault::auth_audit(AUDIT_HOME_CLIENT, &who.label(), outcome, "cli");
}

/// An App's Sew signalling origin: its primary public https origin as
/// `wss://<host>` (`None`: no public URL, so no voice from outside).
pub fn signalling_origin(service_id: &str, name: &str) -> Option<String> {
    crate::apps::origins::for_service(service_id, name)
        .into_iter()
        .next()
        .and_then(|o| o.strip_prefix("https://").map(|h| format!("wss://{h}")))
}

/// The sweep's first settings step (blocking): record every chat App's
/// current signalling origin and label, so a bound domain or a new Connect
/// subdomain reaches Sew. Errors are logged, never fatal to the sweep.
pub fn refresh_origins() {
    let apps = match k2_core::tool_hook::chat_apps() {
        Ok(a) => a,
        Err(e) => {
            k2_core::log_debug!("[sew] chat Apps: {e}");
            return;
        }
    };
    let rows: Vec<(String, Option<String>, String)> = apps
        .into_iter()
        .filter_map(|id| {
            let name = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                k2_core::published_services::get_by_id(&conn, &id).ok().flatten().map(|r| r.name)
            }?;
            Some((id.clone(), signalling_origin(&id, &name), name))
        })
        .collect();
    if let Err(e) = k2_core::tool_hook::note_app_origins(&rows) {
        k2_core::log_debug!("[sew] App origins: {e}");
    }
}

fn view() -> R<Value> {
    let rows = k2_core::tool_hook::home_settings().map_err(|e| no("db_error", 500, e))?;
    let flags = k2_core::tool_hook::sew_features().map_err(|e| no("db_error", 500, e))?;
    let has = |f: &str| flags.iter().any(|x| x == f);
    let mut home: Value = Value::Null;
    let mut home_delivered = true;
    let mut apps = Vec::new();
    for r in rows {
        if r.subject == k2_core::tool_hook::home::HOME_SUBJECT {
            home = r.body.get("app_id").cloned().unwrap_or(Value::Null);
            home_delivered = r.delivered;
        } else if let Some(id) = r.subject.strip_prefix(k2_core::tool_hook::home::APP_PREFIX) {
            apps.push(json!({
                "app": id,
                "name": r.body.get("display_name"),
                "hiddenFromHome": r.body.get("hidden_from_home"),
                "origin": r.body.get("origin"),
                "delivered": r.delivered,
                "updatedBy": r.updated_by,
            }));
        }
    }
    use k2_core::tool_hook::features as f;
    Ok(json!({
        "ok": true,
        "homeClient": home,
        "homeClientDelivered": home_delivered,
        "apps": apps,
        "sew": {
            "features": flags,
            "appScopedSessions": has(f::APP_SCOPED_SESSIONS),
            "homeClient": has(f::HOME_CLIENT),
            "appOrigin": has(f::APP_ORIGIN),
            "k2ManagedAvatar": has(f::K2_MANAGED_AVATAR),
        },
        "note": if has(f::HOME_CLIENT) {
            "Sew takes these settings now"
        } else {
            "this Sew doesn't report home_client yet: the settings wait, and every App session sees what it did before"
        },
    }))
}

fn set_client(who: &Who, v: &Value) -> R<Value> {
    let id = match v.get("app") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if s.trim().is_empty() => None,
        Some(Value::String(s)) => Some(app(s)?.0),
        Some(_) => return Err(no("bad_request", 400, "app is an App id, or null to clear")),
    };
    // App-governed Spaces (Sew I7): the home client is never App-scoped and
    // never governs, so an App holding a chat key can't become it.
    if let Some(id) = id.as_deref() {
        if crate::apps::chat_key_routes::keyed(id).map_err(|r| no("db_error", 500, r.hint))? {
            return Err(no(
                "home_client_cannot_govern",
                409,
                "this App holds a chat key (it can govern Spaces), so it can't be the home client; revoke its key first (k2 app grant <app> --kind chat-key --revoke)",
            ));
        }
    }
    let changed = k2_core::tool_hook::set_home_client(id.as_deref(), &who.label()).map_err(|e| no("db_error", 500, e))?;
    audit_home_client(
        who,
        &format!("{} {}", if changed { "set" } else { "unchanged" }, id.as_deref().unwrap_or("none")),
    );
    super::s32::emit("home");
    let mut out = view()?;
    out["changed"] = json!(changed);
    Ok(out)
}

fn set_app(who: &Who, v: &Value) -> R<Value> {
    let raw = v.get("app").and_then(Value::as_str).ok_or_else(|| no("bad_request", 400, "name the App (app)"))?;
    let hidden = v.get("hidden").and_then(Value::as_bool).ok_or_else(|| no("bad_request", 400, "hidden is true or false"))?;
    let (id, name) = app(raw)?;
    let changed = k2_core::tool_hook::set_app_home(&id, hidden, &name, &who.label()).map_err(|e| no("db_error", 500, e))?;
    super::s32::emit("home");
    let mut out = view()?;
    out["changed"] = json!(changed);
    Ok(out)
}

/// Route a decision-13 path. `None` = not one of these.
pub fn route(path: &str, who: &Who, body: &[u8], _params: &HashMap<String, String>) -> Option<R<Value>> {
    if !is_route(path) {
        return None;
    }
    let v = if body.is_empty() {
        json!({})
    } else {
        match serde_json::from_slice::<Value>(body) {
            Ok(v) if v.is_object() => v,
            Ok(_) => return Some(Err(no("bad_request", 400, "expected a JSON object"))),
            Err(e) => return Some(Err(no("bad_request", 400, format!("invalid JSON body: {e}")))),
        }
    };
    Some(match path {
        HOME => match who {
            Who::Invalid => Err(no("forbidden", 403, "invalid or missing token")),
            Who::Wrong => Err(no("wrong_credential", 403, "App passes, API keys and API sessions can't read Sew settings")),
            _ => view(),
        },
        HOME_CLIENT => match need_owner_person(who) {
            Ok(()) => set_client(who, &v),
            Err(f) => {
                let asked = match v.get("app") {
                    Some(Value::String(s)) if !s.trim().is_empty() => s.trim().chars().take(64).collect::<String>(),
                    _ => "none".to_string(),
                };
                audit_home_client(who, &format!("refused {} {asked}", f.code));
                Err(f)
            }
        },
        // App-only on narrows (Manage Sew); off widens (a person).
        HOME_APP => match v.get("hidden") {
            Some(Value::Bool(false)) => need_admin_person(who),
            _ => need_manage(who),
        }
        .and_then(|()| set_app(who, &v)),
        _ => Err(no("not_found", 404, "no such Sew route")),
    })
}

#[cfg(test)]
#[path = "home_tests.rs"]
mod tests;
