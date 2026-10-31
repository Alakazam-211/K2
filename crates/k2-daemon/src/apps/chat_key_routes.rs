//! App-governed Spaces S1 (0.46.7, `prd-app-governed-spaces-k2-v1` §2):
//! an App's **chat key**, over the CLI (`k2 app grant <app> --kind
//! chat-key …`). Not App-facing: an App pass, an API key or an API session
//! is refused.
//!
//! ```text
//! GET  /cli/app/chat-key         ?app=                       the key (on, off or none)
//! POST /cli/app/chat-key         {app, caps?, maxLevel?, uploads?, maxMembers?, confirmToken?}
//! POST /cli/app/chat-key/revoke  {app}                       off now; Sew ends the App's sessions (I8)
//! ```
//!
//! The key holds no secret (K2 signs passes with its own keyring); it is the
//! owner's "K2 may sign chat passes for this App's users, up to this
//! ceiling". The record and its Sew side are `k2_core::skin::app_chat_key`.
//!
//! Who (vs-live AG-6: agents can read the owner token on shared-user
//! boxes, so turning a key on needs a person):
//!
//! | Change | Who |
//! |---|---|
//! | turn on (or back on), add a cap, raise the level or `max`, uploads on | the owner in person (owner token or an Owner login) **plus a fresh vault confirm** (action `chat-key`, bound to the App) |
//! | the same within the Admin ceiling (caps within the App's approved manifest chat caps, level `member`/`moderator`, `max` ≤ 500, uploads not newly on) | an Admin login, with the confirm |
//! | narrowing only (fewer caps, lower level or `max`, uploads off) and revoke | the owner, an Admin, a K2 shell tab, or the App's own agent with "Build and run Apps"; no confirm |
//!
//! An agent that would widen gets `owner_only` (today's plain refusal; with
//! permission cards on for its workspace, the refusal also names the App and
//! what it asked for, so it ends with the `apps.chat-key` card's Ask-with
//! line: `permission::actions::chat_cards`). Values are
//! checked before the confirm is spent. Refused: `host` (`not_for_host`),
//! the home-client App (`409 home_client_cannot_govern`), a standalone Sew
//! (`409 provider_is_standalone`), a Sew without `app_governed_spaces`
//! (`409 sew_feature_missing`); feature flags, never Sew versions.

use std::collections::HashMap;

use k2_core::connect_users::Role;
use k2_core::skin::app_chat_key::{self as ck, ChatKey, Level, Want};
use serde_json::{json, Value as J};

use super::grace::{target, Target};
use super::routes::{actor, json_body, need_credential, no, s, ws_handle, Refusal, Who, R};
use crate::app_push::owner_routes::{admin_human, app_agent_at, fresh_confirm};

pub const KEY: &str = "/cli/app/chat-key";
pub const REVOKE: &str = "/cli/app/chat-key/revoke";

/// The vault confirm action (`secretId` = the App's service id).
pub const CONFIRM_ACTION: &str = "chat-key";

/// The text the person approves on the confirm sheet, written from the
/// App's own record, never a client's label. `None` = not this action.
pub fn confirm_reason(action: &str, service_id: Option<&str>) -> Option<Result<String, &'static str>> {
    if action != CONFIRM_ACTION {
        return None;
    }
    let Some(id) = service_id.map(str::trim).filter(|s| !s.is_empty()) else { return Some(Err("secretId (the App's service id) is required")) };
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::published_services::get_by_id(&conn, id).ok().flatten()
    };
    let Some(row) = row else { return Some(Err("no published App by that service id")) };
    let name: String = row.name.chars().filter(|c| !c.is_control()).take(64).collect();
    Some(Ok(format!("let K2 sign chat passes for the users of App {name} ({}): its chat key", row.id)))
}

fn store(e: String) -> Refusal {
    no("db_error", 500, e)
}

fn owner_only(what: &str) -> Refusal {
    no(
        k2_core::deploy::codes::OWNER_ONLY,
        403,
        format!("only the server's owner, in person, {what} (the owner token or an Owner login plus a confirm; never an agent)"),
    )
}

fn app_json(t: &Target) -> J {
    json!({
        "serviceId": t.service_id,
        "name": t.name,
        "handle": format!("{}/{}", ws_handle(&t.project_id), t.name),
        "appId": t.app_id,
    })
}

pub(crate) fn key_json(k: &ChatKey) -> J {
    json!({
        "kind": ck::KIND,
        "enabled": k.enabled,
        "caps": k.caps,
        "maxLevel": k.max_level.as_str(),
        "uploads": k.uploads,
        "maxMembers": k.max_members,
        "grantedBy": k.granted_by,
        "grantedAt": k.granted_at,
        "revokedAt": k.revoked_at,
    })
}

/// The App a call names (a passport: its own workspace's Apps only).
fn key_target(who: &Who, raw: Option<&str>) -> R<Target> {
    need_credential(who)?;
    if raw.map(str::trim) == Some(k2_core::skin::HOST_APP_ID) {
        return Err(no("not_for_host", 400, "a chat key is for a published App's own users, never the box login (host)"));
    }
    target(who, raw)
}

/// The install's home client (Hearth, usually): never App-scoped, never
/// governs (Sew I7).
fn home_client() -> R<Option<String>> {
    let rows = k2_core::tool_hook::home_settings().map_err(store)?;
    Ok(rows
        .into_iter()
        .find(|r| r.subject == k2_core::tool_hook::home::HOME_SUBJECT)
        .and_then(|r| r.body.get("app_id").and_then(J::as_str).map(str::to_string)))
}

/// Does the App hold an enabled chat key (the home-client setter refuses
/// such an App)?
pub(crate) fn keyed(service_id: &str) -> R<bool> {
    ck::enabled(service_id).map(|k| k.is_some()).map_err(store)
}

/// The App's approved manifest chat caps: the key's default and the
/// Admin ceiling. `None` = the App declares none (a `--cmd` service, or a
/// manifest without chat caps): only the owner names the caps then.
fn approved_chat_caps(t: &Target) -> R<Option<Vec<String>>> {
    use crate::chat_pass::identity::AppChatCeiling as C;
    match crate::apps::grants::chat_ceiling(&t.service_id).map_err(store)? {
        C::Declared(caps) => Ok(Some(caps)),
        C::NotManaged | C::NotDeclared => Ok(None),
    }
}

fn caps_arg(v: &J) -> R<Option<Vec<String>>> {
    match v.get("caps") {
        None | Some(J::Null) => Ok(None),
        Some(J::Array(a)) => a
            .iter()
            .map(|c| c.as_str().map(|c| c.trim().to_string()).ok_or_else(|| no("usage", 400, "caps is a list of chat caps")))
            .collect::<R<Vec<_>>>()
            .map(Some),
        Some(J::String(raw)) => Ok(Some(raw.split([' ', ',']).map(str::trim).filter(|c| !c.is_empty()).map(str::to_string).collect())),
        Some(_) => Err(no("usage", 400, "caps is a list of chat caps")),
    }
}

/// What the call asks for, with the key's current values (or the
/// defaults) for anything it leaves out.
fn want_of(v: &J, before: Option<&ChatKey>, approved: Option<&[String]>) -> R<Want> {
    let caps = match (caps_arg(v)?, before, approved) {
        (Some(c), _, _) => c,
        (None, Some(b), _) => b.caps.clone(),
        (None, None, Some(a)) if !a.is_empty() => a.to_vec(),
        (None, None, Some(_)) => {
            return Err(no(
                "no_chat_caps",
                409,
                "this App's chat caps aren't approved yet (or it is suspended): approve its manifest's chat caps first (k2 app approve), then give it a chat key",
            ))
        }
        (None, None, None) => {
            return Err(no("usage", 400, "this App's manifest asks for no chat caps: name them (--caps \"chat:read chat:post voice:join\")"));
        }
    };
    let caps = ck::normalize_caps(&caps).map_err(|e| no("usage", 400, e))?;
    let max_level = match s(v, "maxLevel") {
        Some(l) => Level::parse(l).ok_or_else(|| no("usage", 400, "maxLevel is member, moderator or admin"))?,
        None => before.map(|b| b.max_level).unwrap_or(Level::DEFAULT),
    };
    let uploads = match v.get("uploads") {
        None | Some(J::Null) => before.map(|b| b.uploads).unwrap_or(true),
        Some(J::Bool(b)) => *b,
        Some(_) => return Err(no("usage", 400, "uploads is true or false")),
    };
    let max_members = match v.get("maxMembers") {
        None | Some(J::Null) => before.map(|b| b.max_members).unwrap_or(ck::DEFAULT_MAX_MEMBERS),
        Some(n) => n
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| no("usage", 400, format!("maxMembers is a number, 1..={}", ck::MAX_MAX_MEMBERS)))?,
    };
    let w = Want { caps, max_level, uploads, max_members };
    ck::validate(&w).map_err(|e| no("usage", 400, e))?;
    Ok(w)
}

/// May an Admin login give `w`? Turning a key on with the defaults or
/// less: the App's approved caps (or fewer), level `member` or
/// `moderator`, the default ceiling or less, uploads not newly switched on.
fn within_admin_ceiling(before: Option<&ChatKey>, w: &Want, approved: Option<&[String]>) -> bool {
    let caps_ok = approved.is_some_and(|a| w.caps.iter().all(|c| a.contains(c)));
    let uploads_ok = !w.uploads || before.is_none_or(|b| !b.enabled || b.uploads);
    caps_ok && w.max_level <= Level::Moderator && w.max_members <= ck::DEFAULT_MAX_MEMBERS && uploads_ok
}

/// What a `POST /cli/app/chat-key` with body `v` would make of the App's
/// key: the `apps.chat-key` card's words and checks read it, so the card
/// and the route never disagree on the defaults or the Admin ceiling.
pub(crate) struct KeyPreview {
    pub before: Option<ChatKey>,
    pub want: Want,
    /// Turning it on or widening it (needs a person and a confirm).
    pub widens: bool,
    /// An Admin login may give it (else the owner only).
    pub admin_may: bool,
}

pub(crate) fn preview(service_id: &str, v: &J) -> R<KeyPreview> {
    let t = target(&Who::Owner, Some(service_id))?;
    let before = ck::get(&t.service_id).map_err(store)?;
    let approved = approved_chat_caps(&t)?;
    let want = want_of(v, before.as_ref(), approved.as_deref())?;
    let widens = ck::widens(before.as_ref(), &want);
    let admin_may = within_admin_ceiling(before.as_ref(), &want, approved.as_deref());
    Ok(KeyPreview { before, want, widens, admin_may })
}

/// An agent refused turning on or widening its own App's key: with
/// permission cards on for its workspace, the refusal also names the App and
/// what it asked for (`app`, `level`, `caps`, `uploads`), so the card wrapper
/// teaches `k2 permission request apps.chat-key …`. Cards off (or not its
/// own App): today's refusal, byte for byte.
fn with_card_fields(r: Refusal, who: &Who, t: &Target, v: &J) -> Refusal {
    let Some((ws, _)) = who.passport() else { return r };
    if t.project_id != ws || !k2_core::permission::gate::enabled_for(ws) {
        return r;
    }
    let mut extra = json!({ "app": t.name });
    if let Some(l) = s(v, "maxLevel").map(str::trim).filter(|l| !l.is_empty()) {
        extra["level"] = json!(l);
    }
    if let Ok(Some(caps)) = caps_arg(v) {
        if !caps.is_empty() {
            extra["caps"] = json!(caps.join(","));
        }
    }
    if let Some(J::Bool(b)) = v.get("uploads") {
        extra["uploads"] = json!(if *b { "on" } else { "off" });
    }
    Refusal { extra: Some(extra), ..r }
}

/// GET `/cli/app/chat-key?app=`.
pub(crate) fn read(who: &Who, params: &HashMap<String, String>) -> R<J> {
    let t = key_target(who, params.get("app").map(String::as_str))?;
    if !admin_human(who) && !matches!(who, Who::Agent { .. }) {
        return Err(no("forbidden", 403, "an App's chat key is the owner's, Admins' and this App's own agent's to see"));
    }
    let k = ck::get(&t.service_id).map_err(store)?;
    Ok(json!({ "ok": true, "app": app_json(&t), "key": k.as_ref().map(key_json) }))
}

/// POST `/cli/app/chat-key`: turn on, change, or turn back on.
pub(crate) fn set(who: &Who, body: &[u8]) -> R<J> {
    need_credential(who)?;
    crate::apps::access::member_read_only(who)?;
    let v = json_body(body)?;
    let t = key_target(who, s(&v, "app"))?;
    let before = ck::get(&t.service_id).map_err(store)?;
    let approved = approved_chat_caps(&t)?;
    // Who may ask at all, before anything about the values (an agent's
    // refusal is today's plain `owner_only`).
    if !admin_human(who) && !app_agent_at(who, &t, super::access::Level::Build) {
        return Err(with_card_fields(
            owner_only("gives an App a chat key (k2 app grant <app> --kind chat-key); an agent may only narrow or revoke its own App's key"),
            who,
            &t,
            &v,
        ));
    }
    let w = want_of(&v, before.as_ref(), approved.as_deref())?;
    let widen = ck::widens(before.as_ref(), &w);
    if widen {
        match who {
            Who::Owner | Who::Login { role: Role::Owner, .. } => {}
            Who::Login { role: Role::Admin, .. } if within_admin_ceiling(before.as_ref(), &w, approved.as_deref()) => {}
            Who::Login { role: Role::Admin, .. } => {
                return Err(owner_only(
                    "gives a chat key more than the App's approved chat caps, level admin, more than 500 people, or uploads back on",
                ))
            }
            _ => {
                return Err(with_card_fields(
                    owner_only("turns an App's chat key on or widens it (k2 app grant <app> --kind chat-key from a terminal outside K2)"),
                    who,
                    &t,
                    &v,
                ))
            }
        }
    }
    // The App and the box, before a confirm is spent.
    if home_client()?.as_deref() == Some(t.service_id.as_str()) {
        return Err(no(
            "home_client_cannot_govern",
            409,
            "this App is the install's home client (it sees every Space a person is in), so it can't hold a chat key or govern a Space",
        ));
    }
    if widen {
        if let Some(row) = crate::sew::store::load() {
            if crate::sew::supervisor::row_provider(&row) != k2_root_proto::sew::PROVIDER_K2 {
                return Err(no("provider_is_standalone", 409, "a standalone Sew install takes no K2 passes, so an App on it has no chat key"));
            }
        }
        let flag = k2_core::tool_hook::features::APP_GOVERNED_SPACES;
        if !k2_core::tool_hook::sew_has(flag).map_err(store)? {
            return Err(Refusal {
                extra: Some(json!({ "feature": flag })),
                ..no(
                    "sew_feature_missing",
                    409,
                    format!("this server's Sew doesn't have App-governed Spaces yet (flag {flag}, Sew 0.6.0+): the owner upgrades Sew (k2 hostchat upgrade)"),
                )
            });
        }
        fresh_confirm(
            who,
            &t,
            &v,
            CONFIRM_ACTION,
            "an App's chat key lets K2 sign chat passes for its users: it needs a fresh confirm (Touch ID, your login or the vault password); \
             k2 app grant <app> --kind chat-key asks for it from a terminal outside K2",
            "confirm again: a confirm works once, for this App's chat key, within 60 seconds",
        )?;
    }
    let by = actor(who);
    let (before, after) = ck::set(&t.service_id, &w, &by).map_err(|e| no("usage", 400, e))?;
    if widen {
        crate::apps::alerts::note(
            &t.project_id,
            "Chat key given",
            &format!(
                "{} can now give its users chat with no setup per person ({by}): {}, up to level {}, up to {} people{}.",
                t.name,
                after.caps.join(" "),
                after.max_level.as_str(),
                after.max_members,
                if after.uploads { ", uploads on" } else { ", uploads off" }
            ),
        );
    }
    Ok(json!({
        "ok": true,
        "app": app_json(&t),
        "key": key_json(&after),
        "before": before.as_ref().map(key_json),
        "widened": widen,
    }))
}

/// POST `/cli/app/chat-key/revoke {app}`: off at once (no more passes
/// through it) and, on a Sew with `app_governed_spaces`, every Sew session
/// of this App ends (I8). Narrows: the owner, an Admin, a shell tab, or
/// the App's own agent ("Build and run Apps").
pub(crate) fn revoke(who: &Who, body: &[u8]) -> R<J> {
    need_credential(who)?;
    crate::apps::access::member_read_only(who)?;
    let v = json_body(body)?;
    let t = key_target(who, s(&v, "app"))?;
    if !admin_human(who) && !app_agent_at(who, &t, super::access::Level::Build) {
        return Err(owner_only("revokes an App's chat key over that credential"));
    }
    let r = ck::revoke(&t.service_id, &actor(who)).map_err(store)?;
    let was_on = r.before.as_ref().is_some_and(|b| b.enabled);
    Ok(json!({
        "ok": true,
        "app": app_json(&t),
        "revoked": was_on,
        "sessionsEnded": r.sessions_revoke_queued,
        "key": ck::get(&t.service_id).map_err(store)?.as_ref().map(key_json),
    }))
}

#[cfg(test)]
#[path = "chat_key_routes_tests.rs"]
mod tests;
