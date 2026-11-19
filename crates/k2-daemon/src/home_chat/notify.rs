//! A person's own push choices for Home chat (prd-home-chat-pushes-v1 §7;
//! vs-live HC-14, HC-18). Rosson 11-18: every human role (Owner, Admin,
//! Member, Voice & Chat only) picks, per channel, All messages or @mentions
//! only; there is no Off (mute stays Sew's, and isn't touched here); DMs
//! always notify.
//!
//! | Route | Answer |
//! |---|---|
//! | `GET /cli/home-chat/notify?conv=` | `{conv, convKind, level, effective, inherited, fixed, muted, allMessagesLive}` |
//! | `POST /cli/home-chat/notify/set {conv, level: all\|mentions}` | the same |
//! | `GET /cli/home-chat/push-prefs` | `{on, previews, agents, serverPreviews, bell, devices, allMessagesLive}` |
//! | `POST /cli/home-chat/push-prefs/set {previews?, agents?}` | the same |
//!
//! Levels live in Sew: K2 reads and sets them on the person's own held
//! session (`/api/v1/channels/{id}/notify[/set]`), or over mgmt once Sew
//! has `person_channel_notify` (0.10.6, P2). A DM answers `fixed: true`
//! ("DMs always notify") and a set on one is 409 `dm_always_notifies`; any
//! level but the two is 400 `usage`; a channel the person isn't in answers
//! Sew's own refusal (422 `not_a_member`). `level` is the conversation's own
//! level, or the one in effect when it inherits (`inherited: true`); `none`
//! (set elsewhere) shows as such. `allMessagesLive` is false until Sew has
//! `feed_all_messages` (0.11.0): until then All messages is saved, but pushes
//! come for @mentions.

use serde::Deserialize;
use serde_json::{json, Value};

use super::chat_push::Env as _;
use super::people::Caller;
use super::routes::{ok_or_refuse, param, sew_as, sew_id};
use super::{deps, follower};
use crate::voice::bff::{sew_refusal, Refusal};

type R<T> = Result<T, Refusal>;

fn all_messages_live() -> bool {
    (deps().push_flags)().feed_all_messages
}

fn is_dm(c: &Caller, conv: &str) -> bool {
    super::chat_push::LiveEnv.is_home_dm(&c.ext, conv)
}

/// Sew's `ChannelNotifyPrefs` (`{channel_id, level, effective_level,
/// muted}`) as the route answers it.
fn answer(conv: &str, dm: bool, prefs: &Value) -> Value {
    let effective = prefs["effective_level"].as_str().unwrap_or(if dm { "all" } else { "mentions" });
    let own = prefs["level"].as_str();
    json!({
        "ok": true,
        "conv": conv,
        "convKind": if dm { "dm" } else { "channel" },
        "level": if dm { "all" } else { own.unwrap_or(effective) },
        "effective": effective,
        "inherited": !dm && own.is_none(),
        "fixed": dm,
        "muted": prefs["muted"].as_bool().unwrap_or(false),
        "allMessagesLive": all_messages_live(),
    })
}

fn mgmt_path(c: &Caller, conv: &str) -> String {
    format!("/mgmt/v1/people/{}/channels/{conv}/notify", k2_core::tool_hook::sew_wire::pct_path(&c.ext))
}

/// The person's prefs for `conv`, from Sew.
fn read(c: &Caller, conv: &str) -> R<Value> {
    if (deps().push_flags)().person_channel_notify {
        let (status, body) = (deps().mgmt)("GET", &mgmt_path(c, conv), None).map_err(|e| Refusal::new(503, "chat_unavailable", e))?;
        return mgmt_ok(status, body);
    }
    ok_or_refuse(sew_as(&c.ext, "GET", &format!("/api/v1/channels/{conv}/notify"), None)?)
}

fn mgmt_ok(status: u16, body: Value) -> R<Value> {
    if (200..300).contains(&status) {
        return Ok(body);
    }
    Err(sew_refusal(&crate::voice::bff::SewAnswer { status, body }))
}

/// `GET /cli/home-chat/notify?conv=`.
pub fn get(c: &Caller, p: &std::collections::HashMap<String, String>) -> R<Value> {
    let conv = sew_id(param(p, "conv").ok_or_else(|| Refusal::new(400, "usage", "conv is required"))?, "conv")?;
    let dm = is_dm(c, &conv);
    let prefs = match read(c, &conv) {
        Ok(v) => v,
        // A DM always notifies: the answer stands without Sew's mute.
        Err(_) if dm => Value::Null,
        Err(e) => return Err(e),
    };
    Ok(answer(&conv, dm, &prefs))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetBody {
    conv: String,
    level: String,
}

/// `POST /cli/home-chat/notify/set {conv, level}`.
pub fn set(c: &Caller, b: SetBody) -> R<Value> {
    let conv = sew_id(&b.conv, "conv")?;
    let level = match b.level.trim() {
        "all" => "all",
        "mentions" => "mentions",
        _ => return Err(Refusal::new(400, "usage", "level is all or mentions: All messages or @mentions only")),
    };
    if is_dm(c, &conv) {
        return Err(Refusal::new(409, "dm_always_notifies", "DMs always notify; there is no level to set"));
    }
    let body = json!({"level": level});
    let prefs = if (deps().push_flags)().person_channel_notify {
        let (status, got) = (deps().mgmt)("POST", &mgmt_path(c, &conv), Some(&body)).map_err(|e| Refusal::new(503, "chat_unavailable", e))?;
        mgmt_ok(status, got)?
    } else {
        ok_or_refuse(sew_as(&c.ext, "POST", &format!("/api/v1/channels/{conv}/notify/set"), Some(&body))?)?
    };
    // The person's other windows stay in step at once (Sew's own
    // `notification_prefs_updated` follows on the held session).
    if let Some(h) = follower::get(&c.ext) {
        h.publish(&[super::state::notify_frame(&conv, &prefs)]);
    }
    Ok(answer(&conv, false, &prefs))
}

fn prefs_answer(c: &Caller) -> R<Value> {
    let p = k2_core::tool_hook::with_home_chat_db(|conn| k2_core::tool_hook::home_push::prefs(conn, &c.ext))
        .map_err(|e| Refusal::new(503, "chat_unavailable", e))?;
    let (bell, devices) = match (deps().devices)(&c.ext) {
        Some((principal, logins)) => {
            let bell = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                !k2_core::notify_prefs::tables_exist(&conn)
                    || k2_core::notify_prefs::PrefsSnapshot::load(&conn).map(|s| s.bell(&principal).on).unwrap_or(true)
            };
            (bell, k2_core::push_devices::list_devices_for_usernames(&logins).map(|d| d.len()).unwrap_or(0))
        }
        None => (false, 0),
    };
    let server = k2_core::home_chat::push_enabled();
    Ok(json!({
        "ok": true,
        // HC-18: on = the owner's switch, the person's bell, and a phone.
        "on": server && bell && devices > 0,
        "previews": p.previews,
        "agents": p.agents,
        "serverPreviews": k2_core::home_chat::push_previews_allowed(),
        "bell": bell,
        "devices": devices,
        "allMessagesLive": all_messages_live(),
    }))
}

/// `GET /cli/home-chat/push-prefs`.
pub fn prefs_get(c: &Caller) -> R<Value> {
    prefs_answer(c)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrefsBody {
    #[serde(default)]
    previews: Option<bool>,
    #[serde(default)]
    agents: Option<bool>,
}

/// `POST /cli/home-chat/push-prefs/set {previews?, agents?}`: the caller's
/// own choices only.
pub fn prefs_set(c: &Caller, b: PrefsBody) -> R<Value> {
    if b.previews.is_none() && b.agents.is_none() {
        return Err(Refusal::new(400, "usage", "set previews or agents (or both)"));
    }
    k2_core::tool_hook::with_home_chat_db(|conn| {
        k2_core::tool_hook::home_push::set_prefs(conn, &c.ext, b.previews, b.agents, &c.login, super::now_secs())
    })
    .map_err(|e| Refusal::new(503, "chat_unavailable", e))?;
    prefs_answer(c)
}
