//! `k2 app auth`: the sign-in door's switches and page origins (0.46.9,
//! prd-app-hosted-sign-in-v1 vs-live V11, V23, V32).
//!
//! ```text
//! GET  /cli/app/auth       ?app=                          the box switch; with app, that App's door
//! POST /cli/app/auth/set   {door: "on"|"off"}             the box switch: the owner
//! POST /cli/app/auth/set   {app, door?, origins?}         one App: the owner, an Admin, or the App's agent ("Build and run Apps")
//! ```
//!
//! - The box switch is off until the owner turns it on (the door lands
//!   dark, V32). `K2_APP_AUTH_DOOR=1|0` overrides it (the answer says so).
//! - `door: "off"` for one App hands `/.k2/auth/*` back to that App (V23).
//! - `origins` replaces the App's extra page origins: a bundled shell's
//!   `tauri://localhost` (V11). The App's phone pairing webview origins
//!   count too, with no second command.
//! - Every change goes into the App's deploy audit (or the box's auth audit).

use std::collections::HashMap;

use k2_core::skin::auth_door as store;
use serde_json::{json, Value as J};

use super::grace::{target, Target};
use super::routes::{audit_human, json_body, need_credential, no, Refusal, Who, R};
use crate::app_push::owner_routes::{admin_human, app_agent_at, owner_in_person};

pub const STATUS: &str = "/cli/app/auth";
pub const SET: &str = "/cli/app/auth/set";

pub const ROUTES: &[&str] = &[STATUS, SET];
pub const POST_ROUTES: &[&str] = &[SET];

fn store_err(e: String) -> Refusal {
    no("store", 500, e)
}

fn box_json() -> J {
    let env = std::env::var(super::auth_door::ENV_SWITCH).ok().filter(|v| !v.trim().is_empty());
    json!({
        "on": super::auth_door::box_on(),
        "stored": store::box_on().unwrap_or(false),
        "envOverride": env,
        "door": super::auth_door::PREFIX,
    })
}

fn app_json(t: &Target) -> R<J> {
    let st = store::settings(&t.service_id).map_err(store_err)?;
    let pairing = store::allowed_origins(&t.service_id).map_err(store_err)?.into_iter().filter(|o| !st.origins.contains(o)).collect::<Vec<_>>();
    let label = crate::app_pairing::front_rewrites(&t.service_id);
    let on = super::auth_door::box_on() && !st.off && t.app_id.is_some();
    Ok(json!({
        "app": t.service_id,
        "appId": t.app_id,
        "name": t.name,
        "door": if on { "on" } else { "off" },
        "appOff": st.off,
        "origins": st.origins,
        "pairingOrigins": pairing,
        "hosts": crate::app_pairing::pairing_hosts(&t.service_id),
        // A raw App has no door on its k2.dev label; a custom name always does.
        "labelHasDoor": label.is_ok(),
        "labelNote": label.err().map(|_| "this App's manifest says host.clientIp: false, so its k2.dev label is a raw splice with no door: \
             use a custom name, or turn the client-IP front back on"),
        "notRuntime": t.app_id.is_none(),
    }))
}

/// GET `/cli/app/auth?app=`.
fn status(who: &Who, params: &HashMap<String, String>) -> R<J> {
    need_credential(who)?;
    let mut out = json!({ "ok": true, "box": box_json() });
    if let Some(raw) = params.get("app").map(String::as_str).filter(|s| !s.trim().is_empty()) {
        let t = target(who, Some(raw))?;
        may_change(who, &t)?;
        out["app"] = app_json(&t)?;
    }
    Ok(out)
}

/// The owner, an Admin, or the App's own agent with "Build and run Apps".
fn may_change(who: &Who, t: &Target) -> R<()> {
    if admin_human(who) || app_agent_at(who, t, super::access::Level::Build) {
        Ok(())
    } else {
        Err(no("forbidden", 403, "an App's sign-in door is the owner's, Admins' and this App's own agent's to change (\"Build and run Apps\" on)"))
    }
}

fn on_off(v: &J) -> R<Option<bool>> {
    match v.get("door") {
        None | Some(J::Null) => Ok(None),
        Some(J::String(s)) if s == "on" => Ok(Some(true)),
        Some(J::String(s)) if s == "off" => Ok(Some(false)),
        Some(J::Bool(b)) => Ok(Some(*b)),
        _ => Err(no("usage", 400, "door is \"on\" or \"off\"")),
    }
}

/// POST `/cli/app/auth/set`.
fn set(who: &Who, body: &[u8]) -> R<J> {
    need_credential(who)?;
    let v = json_body(body)?;
    let door = on_off(&v)?;
    let app = v.get("app").and_then(J::as_str).map(str::trim).filter(|s| !s.is_empty());
    let Some(raw) = app else {
        // The box switch: the owner only.
        let Some(on) = door else { return Err(no("usage", 400, "send door: \"on\" or \"off\" (and app to change one App)")) };
        if !(owner_in_person(who) || matches!(who, Who::Shell { .. })) {
            return Err(no("owner_only", 403, "the box's sign-in door is the owner's to turn on or off: they run k2 app auth door on"));
        }
        if v.get("origins").is_some() {
            return Err(no("usage", 400, "origins belong to one App: send app too"));
        }
        let by = super::routes::actor(who);
        store::set_box_on(on, &by).map_err(store_err)?;
        super::auth_door::invalidate();
        k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
            "app-auth-door",
            &by,
            format!("box door {}", if on { "on" } else { "off" }),
            "app",
            "-",
            "apps",
        ));
        let mut out = json!({ "ok": true, "box": box_json() });
        if super::auth_door::env_override().is_some() {
            out["warnings"] = json!([format!("{} is set on this daemon and wins over the stored switch", super::auth_door::ENV_SWITCH)]);
        }
        return Ok(out);
    };
    if raw == k2_core::skin::HOST_APP_ID {
        return Err(no("not_for_host", 400, "the sign-in door is for a published App, never the box login (host)"));
    }
    let t = target(who, Some(raw))?;
    may_change(who, &t)?;
    let origins: Option<Vec<String>> = match v.get("origins") {
        None | Some(J::Null) => None,
        Some(J::Array(a)) => Some(a.iter().map(|x| x.as_str().map(str::to_string).ok_or_else(|| no("usage", 400, "origins is a list of strings"))).collect::<R<Vec<_>>>()?),
        Some(_) => return Err(no("usage", 400, "origins is a list of strings")),
    };
    if door.is_none() && origins.is_none() {
        return Err(no("usage", 400, "send door (\"on\" or \"off\") or origins"));
    }
    let by = super::routes::actor(who);
    if let Some(on) = door {
        store::set_off(&t.service_id, !on, &by).map_err(store_err)?;
    }
    if let Some(list) = origins.as_deref() {
        store::set_origins(&t.service_id, list, &by).map_err(|e| no("usage", 400, e))?;
    }
    super::auth_door::invalidate();
    if let Some(app_id) = t.app_id.as_deref() {
        audit_human(app_id, "auth_door_set", who, json!({ "door": door.map(|d| if d { "on" } else { "off" }), "origins": origins }));
    }
    let mut out = json!({ "ok": true, "box": box_json(), "app": app_json(&t)? });
    let mut warnings: Vec<String> = Vec::new();
    if !super::auth_door::box_on() {
        warnings.push("the box's sign-in door is off, so this App has none yet: the owner turns it on with k2 app auth door on".into());
    }
    if t.app_id.is_none() {
        warnings.push("this is not a runtime App: K2 only answers /.k2/auth/* in front of a runtime App (k2 app deploy)".into());
    }
    if !warnings.is_empty() {
        out["warnings"] = json!(warnings);
    }
    Ok(out)
}

/// Dispatch one call (blocking).
pub(crate) fn handle(path: &str, who: &Who, body: &[u8], params: &HashMap<String, String>) -> R<J> {
    match path {
        STATUS => status(who, params),
        SET => set(who, body),
        _ => Err(no("not_found", 404, "no such route")),
    }
}
