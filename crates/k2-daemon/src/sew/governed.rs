//! App-governed Spaces on the daemon side (prd-app-governed-spaces-k2-v1
//! S4 + S5; Sew 0.6.0, flag `app_governed_spaces`). The decisions live in
//! [`k2_core::tool_hook::governed`]; this module only reads what lives in
//! the main DB and serves the routes.
//!
//! - [`refresh_facts`] (every reconcile sweep, before the plan): each bound
//!   App's live manifest `chat` block, its own workspace, and the handles
//!   the block names resolved to workspace ids.
//! - `POST /cli/sew/app-spaces/governance {app, spaceId, governance,
//!   confirmToken}` ([`governance`]): the owner or an Admin login, in
//!   person, with a fresh vault confirm (action [`CONFIRM_GOVERNANCE`],
//!   bound to `<app>:<space>:<governance>`). "Manage Sew" alone is not
//!   enough (AG-7). An agent gets `person_only`, naming the command a
//!   person runs (the `sew.space-governance` card is not built yet: it
//!   needs a Sew Space value kind in the card catalog).
//! - `GET /cli/sew/app-chat?app=<app>` ([`view`]): `k2 app chat <app>`,
//!   read-only.

use std::collections::{BTreeMap, BTreeSet};

use k2_core::tool_hook::{self as hook, governed, GovernanceRefusal};
use serde_json::{json, Value};

use super::routes::Who;
use super::supervisor::{no, R};

/// Vault confirm action of the governance switch (both directions).
pub const CONFIRM_GOVERNANCE: &str = "chat-governance";

/// The id a governance confirm is bound to.
pub fn governance_secret_id(app: &str, space: &str, governance: &str) -> String {
    format!("{app}:{space}:{governance}")
}

/// Record every bound App's chat facts before the sweep plans the sync.
pub fn refresh_facts() {
    let apps: BTreeSet<String> = match hook::app_spaces() {
        Ok(v) => v.into_iter().map(|s| s.binding.app_id).collect(),
        Err(e) => {
            k2_core::log_debug!("[sew] governed facts: {e}");
            return;
        }
    };
    let facts: Vec<governed::AppFacts> = apps.into_iter().filter_map(|a| facts_for(&a)).collect();
    if let Err(e) = hook::note_app_chat_facts(&facts) {
        k2_core::log_debug!("[sew] governed facts: {e}");
    }
}

/// One App's facts from the main DB. `None`: no runtime App row (a
/// published service without a manifest): nothing to read, and what K2
/// noted before stays. A runtime App with no live Seed has no block.
pub fn facts_for(app_id: &str) -> Option<governed::AppFacts> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut f = governed::AppFacts { app_id: app_id.to_string(), ..Default::default() };
    let row = k2_core::deploy::store::get_app_by_service(&conn, app_id).ok().flatten()?;
    f.owner_workspace = Some(row.owner_workspace.clone()).filter(|w| !w.trim().is_empty());
    let Some(seed) = row.live_seed.as_deref() else { return Some(f) };
    let Ok(Some(seed)) = k2_core::deploy::store::get_seed(&conn, seed) else { return Some(f) };
    let block: Option<k2_core::deploy::manifest::ChatBlock> =
        seed.manifest.get("chat").cloned().and_then(|c| serde_json::from_value(c).ok());
    let mut handles = BTreeMap::new();
    if let Some(b) = &block {
        let names: BTreeSet<String> =
            b.channels.iter().flat_map(|c| c.rooms.iter().chain(c.agents.iter())).map(|h| h.trim().to_ascii_lowercase()).collect();
        for h in names {
            let id: Option<String> = conn
                .query_row("SELECT id FROM projects WHERE lower(trim(handle)) = ?1", rusqlite::params![h], |r| r.get(0))
                .ok();
            if let Some(id) = id {
                handles.insert(h, id);
            }
        }
    }
    f.block = block;
    f.handles = handles;
    Some(f)
}

fn s<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str).map(str::trim)
}

/// `POST /cli/sew/app-spaces/governance`.
pub fn governance(who: &Who, v: &Value) -> R<Value> {
    let app_in = s(v, "app").filter(|a| !a.is_empty()).ok_or_else(|| no("bad_request", 400, "name the App (app)"))?;
    let space = s(v, "spaceId").filter(|a| !a.is_empty()).ok_or_else(|| no("bad_request", 400, "name the Space (spaceId)"))?.to_string();
    let to = s(v, "governance").unwrap_or("");
    if to != "app" && to != "sew" {
        return Err(no("bad_request", 400, "governance is app or sew"));
    }
    let app = super::home::resolve_app(app_in).map(|r| r.0).unwrap_or_else(|_| app_in.to_ascii_lowercase());
    if who.passport().is_some() {
        // AG-7: "Manage Sew" is not enough to hand an App a Space.
        return Err(no(
            "person_only",
            403,
            format!(
                "nothing changed: only the server's owner or an admin switches who governs a Space, in person with a confirm \
                 (Manage Sew does not cover it). Ask them to run, from a terminal outside K2: \
                 k2 hostchat app-space governance {app} {space} {to}"
            ),
        ));
    }
    let method = super::routes::need_person(
        who,
        v,
        CONFIRM_GOVERNANCE,
        &governance_secret_id(&app, &space, to),
        super::routes::PersonFloor::Admin,
    )?;
    let changed = hook::set_space_governance(&app, &space, to, &who.label()).map_err(|r| match r {
        GovernanceRefusal::SpaceUnbound => no("space_unbound", 409, format!("Space {space} is bound to no App here: k2 hostchat app-space bind <app> {space} first")),
        GovernanceRefusal::AppMismatch(a) => no("app_mismatch", 409, format!("Space {space} belongs to App '{a}', not '{app}'")),
        GovernanceRefusal::ChatKeyRequired => no(
            "chat_key_required",
            409,
            format!("an App governs its Spaces only with a chat key: k2 app grant {app} --kind chat-key (the owner, in person)"),
        ),
        GovernanceRefusal::FeatureMissing => {
            let mut f = no("sew_feature_missing", 409, "this box's Sew can't do App-governed Spaces yet (it needs Sew 0.6.0: app_governed_spaces)");
            f.extra = Some(json!({"feature": hook::features::APP_GOVERNED_SPACES}));
            f
        }
        GovernanceRefusal::Usage(e) => no("bad_request", 400, e),
        GovernanceRefusal::Store(e) => no("db_error", 500, e),
    })?;
    crate::vault::auth_audit(CONFIRM_GOVERNANCE, &who.label(), &format!("space={space} app={app} governance={to} method={method}"), "cli");
    super::s32::emit("app-space");
    let hint = if to == "app" {
        "Sew makes the App's service owner the Space's owner; box users in the Space leave first (k2 app chat shows them), \
         and their messages stay. K2 then keeps every App user's private channels in step with their role."
    } else {
        "Back to Sew's own rules: people manage the Space in chat; private channels and levels stay as they are."
    };
    Ok(json!({"ok": true, "app": app, "spaceId": space, "governance": to, "changed": changed, "hint": hint}))
}

/// `GET /cli/sew/app-chat?app=<app>`.
pub fn view(app_in: &str) -> R<Value> {
    if app_in.trim().is_empty() {
        return Err(no("bad_request", 400, "name the App (app=<app>)"));
    }
    let (app, name) = super::home::resolve_app(app_in)?;
    let v = hook::governed_view(&app).map_err(|e| no("db_error", 500, e))?;
    Ok(json!({"ok": true, "name": name, "chat": v}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use k2_core::connect_users::Role;

    const APP: &str = "7d4e2a10-1111-4222-8333-444455556666";

    fn body(to: &str) -> Value {
        json!({"app": APP, "spaceId": "5101", "governance": to})
    }

    /// AG-7: "Manage Sew" is never enough; an agent asks on the card.
    #[test]
    fn an_agent_is_refused_person_only_with_the_card_it_asks_on() {
        let _db = k2_core::db::scoped_for_test();
        for who in [
            Who::Agent { project_id: "w-it".into(), session_id: "s1".into() },
            Who::Shell { project_id: "w-it".into(), session_id: "s2".into() },
        ] {
            let f = governance(&who, &body("app")).expect_err("an agent never switches governance");
            assert_eq!((f.code, f.status), ("person_only", 403));
            assert!(f.hint.contains(&format!("k2 hostchat app-space governance {APP} 5101 app")), "{}", f.hint);
        }
    }

    #[test]
    fn a_member_login_is_refused_and_the_owner_needs_a_fresh_confirm() {
        let _db = k2_core::db::scoped_for_test();
        let f = governance(&Who::Login { username: "ana".into(), role: Role::Member }, &body("app")).expect_err("member");
        assert_eq!((f.code, f.status), ("owner_only", 403));
        for who in [Who::Owner, Who::Login { username: "ada".into(), role: Role::Admin }] {
            let f = governance(&who, &body("sew")).expect_err("no confirm token");
            assert_eq!((f.code, f.status), ("owner_confirm_required", 403), "{who:?}");
        }
        let f = governance(&Who::Owner, &body("maybe")).expect_err("bad value");
        assert_eq!((f.code, f.status), ("bad_request", 400));
        assert_eq!(governance_secret_id(APP, "5101", "app"), format!("{APP}:5101:app"));
    }
}
