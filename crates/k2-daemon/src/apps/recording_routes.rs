//! Recording in Apps (Sew 0.9.0; K2 PRD prd-k2-bobbin-in-apps-v1 K-c): an
//! App's recording settings, over the CLI (`k2 app recording <app> …`) and
//! Settings → Chat Hosting → Recording.
//!
//! ```text
//! GET  /cli/app/recording      ?app=                                       as wanted, as Sew confirmed (`delivered`), its Spaces, the flags
//! POST /cli/app/recording/set  {app, startLevel?, audioMaxDays?, required?, serverRead?, confirmToken?}
//! ```
//!
//! Every field goes to Sew only once it reports `app_recording` (a Sew
//! without it is refused `sew_feature_missing` here). Each person in a call
//! still consents for themselves; `required` and `server_read` are named in
//! every consent notice.
//!
//! | Change | Who |
//! |---|---|
//! | `startLevel` lower, `audioMaxDays` lower, `required` / `serverRead` off | the owner, an Admin, or the App's own agent ("…and manage them end to end") |
//! | `startLevel` higher, `audioMaxDays` higher (or back to the org's) | the owner or an Admin in person, with a fresh confirm (`app-recording`, bound to the App) |
//! | `required` on, `serverRead` on | **the owner only**, in person, with a fresh confirm (`app-recording-owner`, bound to the App): the `apps.recording` card's Allow runs exactly this (Rosson's answer 3) |
//!
//! An agent asking for `required` or `serverRead` on gets `owner_only`; with
//! permission cards on for its workspace the refusal names the App and what
//! it asked for, so it ends with `k2 permission request apps.recording …`.
//! An agent raising `startLevel` or the keep days gets `person_only`. An
//! App's keep days above the org's `recording.audio_max_days` are refused
//! here (B23). The home client takes no recording settings (B22).

use std::collections::HashMap;

use k2_core::connect_users::Role;
use k2_core::tool_hook::features::RecordingFeatures;
use k2_core::tool_hook::sew_wire::{self, recording as wire};
use serde_json::{json, Map, Value as J};

use super::grace::{target, Target};
use super::routes::{actor, json_body, need_credential, no, s, Refusal, Who, R};
use crate::app_push::owner_routes::{admin_human, app_agent_at, fresh_confirm};

pub const READ: &str = "/cli/app/recording";
pub const SET: &str = "/cli/app/recording/set";

/// Raising the start level or the keep days (Owner or Admin), bound to the
/// App's service id.
pub const CONFIRM_WIDEN: &str = "app-recording";
/// Turning `required` or `server_read` on (the owner only), bound to the
/// App's service id.
pub const CONFIRM_OWNER: &str = "app-recording-owner";
/// The `deploy_audit` event of every change.
pub const AUDIT: &str = wire::AUDIT_SETTINGS;

/// The text the person approves on the confirm sheet, from the App's own
/// record. `None` = not these actions.
pub fn confirm_reason(action: &str, service_id: Option<&str>) -> Option<Result<String, &'static str>> {
    let what = match action {
        CONFIRM_WIDEN => "let more people start recordings, or keep audio longer, in App",
        CONFIRM_OWNER => "require recording, or let the App's server read transcripts, in App",
        _ => return None,
    };
    let Some(id) = service_id.map(str::trim).filter(|s| !s.is_empty()) else { return Some(Err("secretId (the App's service id) is required")) };
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::published_services::get_by_id(&conn, id).ok().flatten()
    };
    let Some(row) = row else { return Some(Err("no published App by that service id")) };
    let name: String = row.name.chars().filter(|c| !c.is_control()).take(64).collect();
    Some(Ok(format!("{what} {name} ({})", row.id)))
}

fn store(e: String) -> Refusal {
    no("db_error", 500, e)
}

fn rec_target(who: &Who, raw: Option<&str>) -> R<Target> {
    need_credential(who)?;
    if raw.map(str::trim) == Some(k2_core::skin::HOST_APP_ID) {
        return Err(no("not_for_host", 400, "recording settings are for a published App, never the box login (host)"));
    }
    target(who, raw)
}

fn flags() -> R<RecordingFeatures> {
    k2_core::tool_hook::sew_recording_features().map_err(store)
}

fn feature_missing(flag: &str, what: &str) -> Refusal {
    Refusal {
        extra: Some(json!({ "feature": flag })),
        ..no(
            "sew_feature_missing",
            409,
            format!("this server's chat can't {what} yet (it needs Sew 0.9.0: {flag}); the owner upgrades it with k2 hostchat upgrade"),
        )
    }
}

fn app_json(t: &Target) -> J {
    json!({ "serviceId": t.service_id, "name": t.name, "appId": t.app_id })
}

/// The owner in person (the owner token or an Owner login).
fn owner(who: &Who) -> bool {
    matches!(who, Who::Owner | Who::Login { role: Role::Owner, .. })
}

/// The owner or an Admin login (people who can pass a vault confirm).
fn person_admin(who: &Who) -> bool {
    matches!(who, Who::Owner | Who::Login { role: Role::Owner | Role::Admin, .. })
}

/// GET `/cli/app/recording?app=`: the owner, Admins, and the App's own agent.
pub(crate) fn read(who: &Who, params: &HashMap<String, String>) -> R<J> {
    let t = rec_target(who, params.get("app").map(String::as_str))?;
    if !admin_human(who) && !matches!(who, Who::Agent { .. }) {
        return Err(no("forbidden", 403, "an App's recording settings are the owner's, Admins' and this App's own agent's to see"));
    }
    let mut v = k2_core::tool_hook::recording_app_view(&t.service_id).map_err(store)?;
    v["ok"] = json!(true);
    v["app"] = app_json(&t);
    Ok(v)
}

/// One `set` body, parsed: `None` = not named.
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct Change {
    pub start_level: Option<String>,
    /// A number of days, or `null` (back to the org's).
    pub audio_max_days: Option<J>,
    pub required: Option<bool>,
    pub server_read: Option<bool>,
}

impl Change {
    pub(crate) fn parse(v: &J) -> R<Change> {
        let mut c = Change::default();
        if let Some(l) = v.get("startLevel").filter(|x| !x.is_null()) {
            let l = l.as_str().map(str::trim).unwrap_or("");
            if !sew_wire::is_start_level(l) {
                return Err(no("bad_request", 400, "startLevel is none, admin, moderator or member"));
            }
            c.start_level = Some(l.to_string());
        }
        match v.get("audioMaxDays") {
            None => {}
            Some(J::Null) => c.audio_max_days = Some(J::Null),
            Some(d) => match d.as_u64().filter(|d| (1..=3650).contains(d)) {
                Some(d) => c.audio_max_days = Some(json!(d)),
                None => return Err(no("bad_request", 400, "audioMaxDays is a number of days, 1 to 3650, or null for the org's")),
            },
        }
        for (k, slot) in [("required", &mut c.required), ("serverRead", &mut c.server_read)] {
            match v.get(k) {
                None | Some(J::Null) => {}
                Some(J::Bool(b)) => *slot = Some(*b),
                Some(_) => return Err(no("bad_request", 400, format!("{k} is true or false"))),
            }
        }
        if c == Change::default() {
            return Err(no("bad_request", 400, "name a change: startLevel, audioMaxDays, required or serverRead"));
        }
        Ok(c)
    }

    /// The fields this sets in the App's `recording` object.
    fn fields(&self) -> Map<String, J> {
        let mut f = Map::new();
        if let Some(l) = &self.start_level {
            f.insert("start_level".into(), json!(l));
        }
        if let Some(d) = &self.audio_max_days {
            f.insert("audio_max_days".into(), d.clone());
        }
        if let Some(b) = self.required {
            f.insert("required".into(), json!(b));
        }
        if let Some(b) = self.server_read {
            f.insert("server_read".into(), json!(b));
        }
        f
    }
}

/// What a change does against the App's settings now.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Class {
    /// `required` / `server_read` turned on (the owner only).
    pub owner_on: Vec<&'static str>,
    /// The start level or the keep days raised (Owner or Admin).
    pub widens: bool,
}

pub(crate) fn classify(before: &Map<String, J>, c: &Change, org_days: Option<u64>) -> Class {
    let mut out = Class::default();
    let on = |k: &str| before.get(k).and_then(J::as_bool) == Some(true);
    if c.required == Some(true) && !on("required") {
        out.owner_on.push("required");
    }
    if c.server_read == Some(true) && !on("server_read") {
        out.owner_on.push("server_read");
    }
    if let Some(l) = &c.start_level {
        let was = before.get("start_level").and_then(J::as_str).unwrap_or(wire::START_NONE);
        if sew_wire::start_level_rank(l) > sew_wire::start_level_rank(was) {
            out.widens = true;
        }
    }
    if let Some(d) = &c.audio_max_days {
        // The keep days in force: the App's own, else the org's.
        let was = before.get("audio_max_days").and_then(J::as_u64).or(org_days);
        let new = d.as_u64().or(org_days);
        if let (Some(n), Some(w)) = (new, was) {
            if n > w {
                out.widens = true;
            }
        } else if d.is_number() && was.is_none() {
            // Nothing known to compare with: a person decides.
            out.widens = true;
        }
    }
    out
}

/// An agent refused turning `required` / `server_read` on: with permission
/// cards on for its workspace, the refusal names the App and what it asked
/// for, so the card wrapper teaches `k2 permission request apps.recording …`.
fn with_card_fields(r: Refusal, who: &Who, t: &Target, c: &Change) -> Refusal {
    let Some((ws, _)) = who.passport() else { return r };
    if t.project_id != ws || !k2_core::permission::gate::enabled_for(ws) {
        return r;
    }
    let mut extra = json!({ "app": t.name });
    if c.required == Some(true) {
        extra["required"] = json!("on");
    }
    if c.server_read == Some(true) {
        extra["serverRead"] = json!("on");
    }
    Refusal { extra: Some(extra), ..r }
}

/// POST `/cli/app/recording/set`.
pub(crate) fn set(who: &Who, body: &[u8]) -> R<J> {
    need_credential(who)?;
    crate::apps::access::member_read_only(who)?;
    let v = json_body(body)?;
    let t = rec_target(who, s(&v, "app"))?;
    let c = Change::parse(&v)?;
    // Who may ask at all (a passport only for its own App, "…and manage
    // them end to end"), before anything about the values.
    let own_agent = app_agent_at(who, &t, super::access::Level::Full);
    if !admin_human(who) && !own_agent {
        return Err(no(
            k2_core::deploy::codes::OWNER_ONLY,
            403,
            "only the server's owner, an admin, or this App's own agent (\"…and manage them end to end\") changes its recording settings",
        ));
    }
    let f = flags()?;
    if !f.app_recording {
        return Err(feature_missing(k2_core::tool_hook::features::APP_RECORDING, "record in Apps"));
    }
    let home = k2_core::tool_hook::recording_app_view(&t.service_id).map_err(store)?;
    if home["homeClient"] == json!(true) {
        return Err(no(
            "home_client_cannot_govern",
            409,
            "this App is the install's home client (it sees every Space a person is in), so it takes no recording settings",
        ));
    }
    let before: Map<String, J> = home["recording"].as_object().cloned().unwrap_or_default();
    let org_days = k2_core::tool_hook::recording_org_audio_max_days().map_err(store)?;
    if let (Some(d), Some(org)) = (c.audio_max_days.as_ref().and_then(J::as_u64), org_days) {
        if d > org {
            return Err(no(
                "above_org_audio_max_days",
                400,
                format!("audio is kept at most {org} days on this server (the org's recording.audio_max_days): ask for {org} or fewer"),
            ));
        }
    }
    let class = classify(&before, &c, org_days);
    if !class.owner_on.is_empty() {
        if !owner(who) {
            let r = no(
                k2_core::deploy::codes::OWNER_ONLY,
                403,
                format!(
                    "only the server's owner, in person, turns on {} for an App (each person's consent notice names it); \
                     an agent asks on a card: k2 permission request apps.recording app={} {}--why \"…\"",
                    class.owner_on.join(" and "),
                    t.name,
                    class.owner_on.iter().map(|k| format!("{}=on ", if *k == "server_read" { "server-read" } else { k })).collect::<String>(),
                ),
            );
            return Err(with_card_fields(r, who, &t, &c));
        }
        fresh_confirm(
            who,
            &t,
            &v,
            CONFIRM_OWNER,
            "requiring recording, or letting an App's server read transcripts, needs the owner's fresh confirm (Touch ID, your login or the vault password)",
            "confirm again: a confirm works once, for this App's recording settings, within 60 seconds",
        )?;
    } else if class.widens {
        if !person_admin(who) {
            return Err(no(
                "person_only",
                403,
                format!(
                    "only the server's owner or an admin, in person with a confirm, lets more people start recordings or keeps audio longer: \
                     ask them to run k2 app recording {} set … from a terminal outside K2",
                    t.name
                ),
            ));
        }
        fresh_confirm(
            who,
            &t,
            &v,
            CONFIRM_WIDEN,
            "letting more people start recordings, or keeping audio longer, needs a fresh confirm (Touch ID, your login or the vault password)",
            "confirm again: a confirm works once, for this App's recording settings, within 60 seconds",
        )?;
    }
    let by = actor(who);
    let fields = c.fields();
    let changed = k2_core::tool_hook::recording_set_app(&t.service_id, &fields, &by).map_err(store)?;
    if changed {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let subject = t.app_id.clone().unwrap_or_else(|| t.service_id.clone());
        let payload = json!({ "service": t.service_id, "set": fields });
        if let Err(e) = k2_core::deploy::audit::append(&conn, k2_core::deploy::audit::SUBJECT_APP, &subject, AUDIT, &by, None, &payload) {
            k2_core::log_debug!("[recording] audit {}: {e}", t.service_id);
        }
    }
    let mut out = k2_core::tool_hook::recording_app_view(&t.service_id).map_err(store)?;
    out["ok"] = json!(true);
    out["app"] = app_json(&t);
    out["changed"] = json!(changed);
    Ok(out)
}

#[cfg(test)]
#[path = "recording_routes_tests.rs"]
mod tests;
