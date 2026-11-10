//! Recording in Apps (Sew 0.9.0; K2 PRD prd-k2-bobbin-in-apps-v1 K-c): the
//! Chat Hosting side of the settings. The decisions and the store are
//! [`k2_core::tool_hook::recording`]'s; the sweep sends each field only once
//! Sew reports its flag.
//!
//! ```text
//! POST /cli/sew/app-spaces/recording       {app, spaceId, startLevel?, required?, confirmToken?}   a Space's override (null inherits)
//! POST /cli/sew/app-spaces/recording-lock  {spaceId, lock: true|false, confirmToken}               the CUI/FCI lock
//! GET  /cli/sew/recording/policies                                                                  the org policies: wanted, seen, the flags, the locks
//! POST /cli/sew/recording/policy           {changes: {"recording.required": "multi_person", …}, confirmToken}
//! ```
//!
//! | Change | Who |
//! |---|---|
//! | a Space's `startLevel` to `none`, `required` off or inherit | the owner, an Admin, or the bound App's own agent ("…and manage them end to end") |
//! | a Space's `startLevel` above `none` | the owner or an Admin in person, with a confirm (`recording-space`, bound to `<app>:<space>`) |
//! | a Space's `required` on | the owner in person, with that confirm |
//! | the CUI/FCI lock, on or off | **the owner only**, in person, with a confirm (`recording-lock`, bound to the Space; Rosson's answer 7) |
//! | org policies | the owner or an Admin in person, with a confirm (`recording-policy`, bound to this install); `recording.required` the owner only |
//!
//! A locked Space never goes above no recording: K2 sends `start_level:
//! none` and `required: false` for it whatever anyone asks, and refuses a
//! widening override here (`space_locked`). An agent is refused
//! `person_only` for anything a person does.

use std::collections::HashMap;

use k2_core::tool_hook::features::{self, RecordingFeatures};
use k2_core::tool_hook::sew_wire::{self, recording as wire};
use serde_json::{json, Value};

use super::routes::{need_person, PersonFloor, Who};
use super::supervisor::{no, Fail, R};

pub const SPACE: &str = "/cli/sew/app-spaces/recording";
pub const LOCK: &str = "/cli/sew/app-spaces/recording-lock";
pub const POLICIES: &str = "/cli/sew/recording/policies";
pub const POLICY: &str = "/cli/sew/recording/policy";

pub const ROUTES: &[&str] = &[SPACE, LOCK, POLICIES, POLICY];
pub const POST_ROUTES: &[&str] = &[SPACE, LOCK, POLICY];

/// Vault confirm actions (`vault_routes::CONFIRM_ACTIONS`).
pub const CONFIRM_SPACE: &str = "recording-space";
pub const CONFIRM_LOCK: &str = "recording-lock";
pub const CONFIRM_POLICY: &str = "recording-policy";

pub fn is_route(p: &str) -> bool {
    ROUTES.contains(&p)
}

pub fn is_post_route(p: &str) -> bool {
    POST_ROUTES.contains(&p)
}

/// The id a Space override's confirm is bound to.
pub fn space_secret_id(app: &str, space: &str) -> String {
    format!("{app}:{space}")
}

fn store(e: String) -> Fail {
    no("db_error", 500, e)
}

fn s<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str).map(str::trim).filter(|x| !x.is_empty())
}

fn need_credential(who: &Who) -> R<()> {
    match who {
        Who::Invalid => Err(no("forbidden", 403, "invalid or missing token")),
        Who::Wrong => Err(no("wrong_credential", 403, "App passes, API keys and API sessions can't change recording settings")),
        _ => Ok(()),
    }
}

fn flags() -> R<RecordingFeatures> {
    k2_core::tool_hook::sew_recording_features().map_err(store)
}

pub(crate) fn feature_missing(flag: &str, what: &str) -> Fail {
    let mut f = no(
        "sew_feature_missing",
        409,
        format!("this server's chat can't {what} yet (it needs Sew 0.9.0: {flag}); the owner upgrades it with k2 hostchat upgrade"),
    );
    f.extra = Some(json!({ "feature": flag }));
    f
}

fn person_only(what: &str) -> Fail {
    no(
        "person_only",
        403,
        format!("nothing changed: a person does this, never an agent or a K2 terminal ({what}). Ask the server's owner or an admin to run it from a terminal outside K2"),
    )
}

/// A `startLevel` / `required` value: absent = keep, `null` = inherit.
fn patch(v: &Value, k: &str) -> R<Option<Value>> {
    match v.get(k) {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(Value::Null)),
        Some(Value::String(l)) if k == "startLevel" && sew_wire::is_start_level(l.trim()) => Ok(Some(json!(l.trim()))),
        Some(Value::Bool(b)) if k == "required" => Ok(Some(json!(b))),
        Some(_) if k == "startLevel" => Err(no("bad_request", 400, "startLevel is none, admin, moderator, member, or null to inherit")),
        Some(_) => Err(no("bad_request", 400, format!("{k} is true, false, or null to inherit"))),
    }
}

// ── a Space's override ───────────────────────────────────────────────

fn space_override(who: &Who, v: &Value) -> R<Value> {
    need_credential(who)?;
    let app_in = s(v, "app").ok_or_else(|| no("bad_request", 400, "name the App (app)"))?;
    let space = s(v, "spaceId").filter(|x| sew_wire::is_sew_id(x)).ok_or_else(|| no("bad_request", 400, "name the Space (spaceId)"))?.to_string();
    let level = patch(v, "startLevel")?;
    let required = patch(v, "required")?;
    if level.is_none() && required.is_none() {
        return Err(no("bad_request", 400, "name a change: startLevel and/or required (null inherits)"));
    }
    let app = super::home::resolve_app(app_in).map(|r| r.0).unwrap_or_else(|_| app_in.to_ascii_lowercase());
    match k2_core::tool_hook::recording_bound_app(&space).map_err(store)? {
        None => return Err(no("space_unbound", 409, format!("Space {space} is bound to no App here: k2 hostchat app-space bind <app> {space} first"))),
        Some(a) if a != app => return Err(no("app_mismatch", 409, format!("Space {space} belongs to another App ({a}), not '{app}'"))),
        Some(_) => {}
    }
    let f = flags()?;
    if level.is_some() && !f.app_recording {
        return Err(feature_missing(features::APP_RECORDING, "set who may record in a Space"));
    }
    if required.is_some() && !f.recording_required {
        return Err(feature_missing(features::RECORDING_REQUIRED, "require recording in a Space"));
    }
    let (_, _, locked) = k2_core::tool_hook::recording_space(&space).map_err(store)?;
    let level_up = level.as_ref().and_then(Value::as_str).is_some_and(|l| l != wire::START_NONE);
    let required_on = required == Some(json!(true));
    if locked && (level_up || required_on || level == Some(Value::Null) || required == Some(Value::Null)) {
        return Err(no(
            "space_locked",
            409,
            format!("Space {space} is locked against recording (CUI/FCI): only the server's owner lifts it, in person (k2 hostchat app-space recording-lock {space} off)"),
        ));
    }
    if level_up || required_on {
        if who.passport().is_some() || matches!(who, Who::Shell { .. }) {
            return Err(person_only("letting people record in a Space, or requiring it"));
        }
        let floor = if required_on { PersonFloor::Owner } else { PersonFloor::Admin };
        let method = need_person(who, v, CONFIRM_SPACE, &space_secret_id(&app, &space), floor)?;
        crate::vault::auth_audit(CONFIRM_SPACE, &who.label(), &format!("space={space} app={app} method={method}"), "cli");
    } else {
        // Narrowing: the owner, an Admin, or the bound App's own agent.
        match who {
            Who::Owner | Who::Login { role: k2_core::connect_users::Role::Owner | k2_core::connect_users::Role::Admin, .. } => {}
            Who::Agent { project_id, .. } | Who::Shell { project_id, .. }
                if k2_core::skin::ties::app_workspace(&app).as_deref() == Some(project_id.as_str())
                    && crate::apps::access::level(project_id) >= crate::apps::access::Level::Full => {}
            _ => {
                return Err(no(
                    "owner_only",
                    403,
                    "only the server's owner, an admin, or the App's own agent (\"…and manage them end to end\") changes a Space's recording",
                ))
            }
        }
    }
    let changed = k2_core::tool_hook::recording_set_space(&space, level, required, &who.label()).map_err(store)?;
    super::s32::emit("app-space");
    let (l, r, locked) = k2_core::tool_hook::recording_space(&space).map_err(store)?;
    Ok(json!({"ok": true, "app": app, "spaceId": space, "startLevel": l, "required": r, "locked": locked, "changed": changed}))
}

// ── the CUI/FCI lock ─────────────────────────────────────────────────

fn lock(who: &Who, v: &Value) -> R<Value> {
    need_credential(who)?;
    let space = s(v, "spaceId").filter(|x| sew_wire::is_sew_id(x)).ok_or_else(|| no("bad_request", 400, "name the Space (spaceId)"))?.to_string();
    let on = match v.get("lock") {
        Some(Value::Bool(b)) => *b,
        _ => return Err(no("bad_request", 400, "lock is true (no recording in this Space, ever) or false (lift it)")),
    };
    if who.passport().is_some() || matches!(who, Who::Shell { .. }) {
        let side = if on { "on" } else { "off" };
        return Err(no(
            "person_only",
            403,
            format!(
                "nothing changed: only the server's owner sets or lifts a Space's CUI/FCI recording lock, in person with a confirm. \
                 Ask them to run, from a terminal outside K2: k2 hostchat app-space recording-lock {space} {side}"
            ),
        ));
    }
    let method = need_person(who, v, CONFIRM_LOCK, &space, PersonFloor::Owner)?;
    let changed = k2_core::tool_hook::recording_set_lock(&space, on, &who.label()).map_err(store)?;
    crate::vault::auth_audit(CONFIRM_LOCK, &who.label(), &format!("space={space} lock={on} method={method}"), "cli");
    super::s32::emit("app-space");
    let hint = if on {
        "No one records in this Space now: K2 tells Sew start_level none and required off for it, whatever its App asks, and keeps doing so after an unbind."
    } else {
        "Lifted: the Space follows its App's and the org's recording rules again."
    };
    Ok(json!({"ok": true, "spaceId": space, "locked": on, "changed": changed, "hint": hint}))
}

// ── org policies ─────────────────────────────────────────────────────

fn policies_view(who: &Who) -> R<Value> {
    need_credential(who)?;
    let rows = k2_core::tool_hook::recording_policies().map_err(store)?;
    let locks = k2_core::tool_hook::recording_locks().map_err(store)?;
    Ok(json!({"ok": true, "flags": flags()?, "policies": rows, "locks": locks, "k5SpaceId": k2_core::tool_hook::recording_k5_space().map_err(store)?}))
}

fn policy_set(who: &Who, v: &Value) -> R<Value> {
    need_credential(who)?;
    let Some(changes) = v.get("changes").and_then(Value::as_object).filter(|c| !c.is_empty()) else {
        return Err(no("bad_request", 400, "changes is an object of recording policies, e.g. {\"recording.required\": \"multi_person\"}"));
    };
    let f = flags()?;
    for (k, val) in changes {
        k2_core::tool_hook::recording::policy_value_ok(k, val).map_err(|e| no("bad_request", 400, e))?;
        let flag = k2_core::tool_hook::recording::policy_flag(k);
        let have = if flag == features::RECORDING_REQUIRED { f.recording_required } else { f.app_recording };
        if !have {
            return Err(feature_missing(flag, &format!("set {k}")));
        }
    }
    if who.passport().is_some() || matches!(who, Who::Shell { .. }) {
        return Err(person_only("the server's recording policies"));
    }
    let floor = if changes.contains_key(wire::POLICY_REQUIRED) { PersonFloor::Owner } else { PersonFloor::Admin };
    let method = need_person(who, v, CONFIRM_POLICY, &super::routes::confirm_secret_id(), floor)?;
    let mut changed = Vec::new();
    for (k, val) in changes {
        if k2_core::tool_hook::recording_set_policy(k, val, &who.label()).map_err(store)? {
            changed.push(k.clone());
        }
    }
    crate::vault::auth_audit(CONFIRM_POLICY, &who.label(), &format!("set {} method={method}", changed.join(",")), "cli");
    super::s32::emit("recording");
    let rows = k2_core::tool_hook::recording_policies().map_err(store)?;
    Ok(json!({"ok": true, "changed": changed, "policies": rows}))
}

/// Route a recording-settings path. `None` = not one.
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
        SPACE => space_override(who, &v),
        LOCK => lock(who, &v),
        POLICIES => policies_view(who),
        POLICY => policy_set(who, &v),
        _ => Err(no("not_found", 404, "no such Sew route")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use k2_core::connect_users::Role;

    /// PRD §8 K-c (Q7): the CUI/FCI lock is the owner's alone, in person:
    /// an agent (Manage Sew or not) and a K2 terminal get `person_only`, an
    /// Admin `owner_only`, the owner without a confirm
    /// `owner_confirm_required`; nothing is stored.
    #[test]
    fn only_the_owner_sets_or_lifts_the_lock() {
        let _home = k2_core::test_env::TempHome::new();
        let _db = k2_core::db::scoped_for_test();
        for on in [true, false] {
            let body = json!({"spaceId": "5101", "lock": on});
            for who in [Who::Agent { project_id: "w".into(), session_id: "s".into() }, Who::Shell { project_id: "w".into(), session_id: "s".into() }] {
                let f = lock(&who, &body).expect_err("an agent never locks or lifts");
                assert_eq!((f.code, f.status), ("person_only", 403));
                assert!(f.hint.contains(&format!("k2 hostchat app-space recording-lock 5101 {}", if on { "on" } else { "off" })), "{}", f.hint);
            }
            let f = lock(&Who::Login { username: "ada".into(), role: Role::Admin }, &body).expect_err("an Admin");
            assert_eq!((f.code, f.status), ("owner_only", 403));
            let f = lock(&Who::Owner, &body).expect_err("no confirm");
            assert_eq!((f.code, f.status), ("owner_confirm_required", 403));
        }
        assert!(k2_core::tool_hook::recording_locks().unwrap().is_empty(), "nothing stored");
        let f = lock(&Who::Owner, &json!({"spaceId": "5101"})).expect_err("no side");
        assert_eq!(f.status, 400);
    }

    /// An agent can't raise a Space's recording or set org policies; a
    /// policy Sew can't take yet is `sew_feature_missing`, before any
    /// confirm.
    #[test]
    fn agents_never_widen_and_flags_come_first() {
        let _home = k2_core::test_env::TempHome::new();
        let _db = k2_core::db::scoped_for_test();
        let agent = Who::Agent { project_id: "w".into(), session_id: "s".into() };
        let f = policy_set(&Who::Owner, &json!({"changes": {"recording.required": "multi_person"}})).expect_err("Sew 0.8.x");
        assert_eq!((f.code, f.status), ("sew_feature_missing", 409));
        let f = policy_set(&agent, &json!({"changes": {"recording.required": "always"}})).expect_err("bad value");
        assert_eq!(f.status, 400);
        let f = space_override(&agent, &json!({"app": "x", "spaceId": "5101", "startLevel": "member"})).expect_err("unbound");
        assert_eq!(f.code, "space_unbound");
    }
}
