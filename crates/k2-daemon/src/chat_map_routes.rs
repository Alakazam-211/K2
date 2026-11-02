//! Sew for Apps A6 (0.46.6): the identity map on the App transport. An
//! App resolves its own users to Sew members (for DMs, calls, mentions)
//! and Sew members back to its users (for display and permissions),
//! without asking Sew and without seeing anyone else's.
//!
//! ```text
//! GET  /cli/sew/app/member ?principal=<id>|username=<name>[&app=<own id>]   chat.member
//!   → 200 AppChatMember {ok, app, username, principalId, externalId, memberId, state}
//! POST /cli/sew/app/whois {memberIds: [..≤100], app?}                       chat.whois
//! GET  /cli/sew/app/space [?app=<own id>]                                    chat.space
//!   → 200 AppChatWhois {ok, app, members: [{memberId, kind, isThisAppUser, principalId?, username?, displayName?, imageHash?, deleted}]}
//! ```
//!
//! - **Only the App's own pass**, carrying `chat:read` (the App's manifest
//!   asks for it and the owner approved it): else `app_pass_required` /
//!   `cap_not_granted`. A guest's session pass, a platform pass, the owner
//!   token and a login never reach this.
//! - **Filtered per App**: `chat.member` answers only for this App's users
//!   (anyone else is the same 404 as nobody); `chat.whois` gives a member
//!   who isn't this App's user only `{memberId, kind, isThisAppUser:
//!   false}`, never their principal, name or other Apps. The App's own room
//!   agent is the one agent it may name.
//! - Never Sew's mgmt socket: the answers come from K2's durable mirror
//!   (`k2_core::tool_hook::members`), written by the S27 worker.

use std::collections::HashMap;

use k2_core::skin::app_chat::{self as ac, Refusal};
use k2_core::skin::SkinPass;
use serde_json::{json, Value as J};

use crate::cli_response::CliResponse;
use crate::guest_rooms_routes::{app_of, error, pass_invalid, OnLink};

pub const MEMBER: &str = "/cli/sew/app/member";
pub const WHOIS: &str = "/cli/sew/app/whois";
/// `chat.space` (0.46.8, feature `app-chat-space-v1`): this App's own Space
/// and manifest channels. The App's own pass, no cap: it says only what the
/// App's own binding and manifest hold.
pub const SPACE: &str = "/cli/sew/app/space";

pub fn is_route(p: &str) -> bool {
    p == MEMBER || p == WHOIS || p == SPACE
}

pub fn app_pass_required() -> CliResponse {
    error(
        403,
        "app_pass_required",
        "the chat identity map takes this App's own pass (K2_PASS_FILE inside the wall, or its remote pass); never a guest's pass, the owner token or a login",
        None,
    )
}

fn field<'a>(v: &'a J, k: &str) -> Option<&'a str> {
    v.get(k).and_then(J::as_str).map(str::trim).filter(|s| !s.is_empty())
}

/// One call with an already-resolved pass. Blocking: run on a blocking task.
pub fn handle(path: &str, pass: Option<SkinPass>, link: Option<&OnLink>, is_post: bool, body: &[u8], params: &HashMap<String, String>) -> CliResponse {
    let want_post = path == WHOIS;
    if is_post != want_post {
        return error(405, "usage", format!("{path} is a {}", if want_post { "POST" } else { "GET" }), None);
    }
    let Some(pass) = pass else { return pass_invalid() };
    let Some(app) = app_of(&pass, link) else { return app_pass_required() };
    if path == SPACE {
        return space(&app, params);
    }
    if !pass.has_cap(k2_core::skin::CAP_CHAT_READ) {
        return error(
            403,
            "cap_not_granted",
            "this App's pass doesn't carry chat:read (ask for it in the manifest's caps; the owner approves it once)",
            Some(json!({ "cap": k2_core::skin::CAP_CHAT_READ })),
        );
    }
    let v: J = if is_post {
        match serde_json::from_slice::<J>(body) {
            Ok(v @ J::Object(_)) => v,
            _ => return error(400, "usage", "the body is a JSON object {memberIds, app?}", None),
        }
    } else {
        json!(params)
    };
    if field(&v, "app").is_some_and(|a| a != app) {
        return refusal(&Refusal::NotFound);
    }
    let out = if path == MEMBER {
        let Some(who) = field(&v, "principal").or_else(|| field(&v, "username")) else {
            return error(400, "usage", "name the user: principal=<id> or username=<name>", None);
        };
        ac::member_of(&app, who).and_then(|m| serde_json::to_string(&m).map_err(|e| Refusal::Store(e.to_string())))
    } else {
        let Some(ids) = v.get("memberIds").and_then(J::as_array) else {
            return error(400, "usage", "memberIds is a list of Sew member ids", None);
        };
        let ids: Option<Vec<String>> = ids
            .iter()
            .map(|x| match x {
                J::String(s) => Some(s.clone()),
                J::Number(n) => Some(n.to_string()),
                _ => None,
            })
            .collect();
        let Some(ids) = ids else { return error(400, "usage", "memberIds holds strings (or numbers)", None) };
        ac::whois(&app, &ids).and_then(|w| serde_json::to_string(&w).map_err(|e| Refusal::Store(e.to_string())))
    };
    match out {
        Ok(body) => CliResponse::ok_json(body),
        Err(e) => refusal(&e),
    }
}

/// `GET /cli/sew/app/space[?app=<own id>]`: `{ok, app, spaceId, spaces,
/// manifestChannels}`. Only additions under `app-chat-space-v1`.
fn space(app: &str, params: &HashMap<String, String>) -> CliResponse {
    if params.get("app").map(|a| a.trim()).is_some_and(|a| !a.is_empty() && a != app) {
        return error(404, "not_found", "this call answers only for the App whose pass this is: leave app out", None);
    }
    let spaces = match k2_core::tool_hook::spaces_of_app(app) {
        Ok(s) => s,
        Err(e) => return error(503, "chat_unavailable", e, None),
    };
    let block = crate::sew::governed::facts_for(app).and_then(|f| f.block);
    let manifest_channels: Vec<J> = block
        .map(|b| b.channels)
        .unwrap_or_default()
        .into_iter()
        .map(|c| {
            // A manifest channel names its Space when the App has several.
            let found = spaces
                .iter()
                .filter(|s| c.space.as_deref().is_none_or(|want| want == s.space_id))
                .find_map(|s| s.channels.iter().find(|r| r.slug == c.slug).map(|r| (s.space_id.clone(), r.channel_id.clone())));
            let (space_id, channel_id) = found.map_or((None, None), |(s, ch)| (Some(s), ch));
            json!({ "slug": c.slug, "name": c.name, "type": c.kind, "private": c.private, "spaceId": space_id, "channelId": channel_id })
        })
        .collect();
    let only = match spaces.as_slice() {
        [one] => Some(one.space_id.clone()),
        _ => None,
    };
    CliResponse::ok_json(json!({ "ok": true, "app": app, "spaceId": only, "spaces": spaces, "manifestChannels": manifest_channels }).to_string())
}

fn refusal(r: &Refusal) -> CliResponse {
    let extra = match r {
        Refusal::RateLimited { retry_after_ms } => Some(json!({ "retryAfterMs": retry_after_ms })),
        _ => None,
    };
    error(r.status(), r.code(), r.hint(), extra)
}

#[cfg(test)]
#[path = "chat_map_routes_tests.rs"]
mod tests;
