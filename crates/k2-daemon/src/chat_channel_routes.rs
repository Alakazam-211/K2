//! Sew for Apps A9 (0.46.6): `chat.createChannel` on the App transport.
//! An App makes a text channel in its own bound Sew Space and puts its own
//! users in it.
//!
//! ```text
//! POST /cli/sew/app/channels {space?, slug, name, private?, members?: [username..≤100]}
//!   → 200 AppChatChannel {ok, app, spaceId, channelId, slug, name, private, created, added, notAdded, skipped}
//! ```
//!
//! - **Only the App's own pass** with `chat:channels` (the App's manifest
//!   asks for it; the owner approves it once): else `app_pass_required` /
//!   `cap_not_granted`. A guest's session pass, a platform pass, the owner
//!   token and a login never reach this.
//! - **Only its own Space**: the App's bound Space (its only one, or the one
//!   it names). Another App's Space is `409 app_mismatch`, a Space bound to
//!   no App `409 space_unbound`, before Sew is asked.
//! - **Gated on Sew's flag** `app_channel_create` (Sew 0.5.0+), never on a
//!   version: without it `409 sew_feature_missing`.
//! - Two mgmt calls over the chat target's socket and token (the grant-hook
//!   worker's client, `Sew-Actor` = the App, attribution only): first
//!   `spaces/members/add` for listed users not yet in the Space (K31's
//!   ledger), then `channels/create` with the users now in it. Sew's 409s
//!   map to `space_unbound` / `app_mismatch`.
//! - The rules (who goes in, the ledger, the audit row, 60/min) are
//!   `k2_core::skin::app_channels`.

use k2_core::skin::app_channels::{self as cc, Ask, Refusal};
use k2_core::skin::SkinPass;
use k2_core::tool_hook::{self as hook, sew_wire};
use serde_json::{json, Value as J};

use crate::cli_response::CliResponse;
use crate::guest_rooms_routes::{app_of, error, pass_invalid, OnLink};
use crate::sew_mgmt::{MgmtClient, MgmtResponse};

pub const ROUTE: &str = "/cli/sew/app/channels";

pub fn app_pass_required() -> CliResponse {
    error(
        403,
        "app_pass_required",
        "making chat channels takes this App's own pass (K2_PASS_FILE inside the wall, or its remote pass); never a guest's pass, the owner token or a login",
        None,
    )
}

pub fn refusal(r: &Refusal) -> CliResponse {
    let extra = match r {
        Refusal::RateLimited { retry_after_ms } => Some(json!({ "retryAfterMs": retry_after_ms, "limit": cc::RATE_PER_MIN })),
        Refusal::FeatureMissing => Some(json!({ "feature": hook::features::APP_CHANNEL_CREATE })),
        _ => None,
    };
    error(r.status(), r.code(), r.hint(), extra)
}

/// The audit actor and `Sew-Actor`: `app:<id>` on its link,
/// `app-remote:<service id>` off it.
fn actor(app: &str, link: Option<&OnLink>) -> String {
    match link {
        Some(l) => format!("app:{}", l.app_id),
        None => format!("app-remote:{app}"),
    }
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, Refusal> + Send + 'static) -> Result<T, Refusal> {
    tokio::task::spawn_blocking(f).await.map_err(|e| Refusal::Store(format!("task: {e}")))?
}

/// Sew's answer to a mgmt POST, as a refusal when it isn't a 2xx.
fn sew_refusal(r: &MgmtResponse, space: &str) -> Refusal {
    let e = r.error();
    let code = e.as_ref().map(|e| e.error.code.clone()).unwrap_or_default();
    let reason = e
        .as_ref()
        .and_then(|e| e.error.details.get("reason").and_then(|x| x.as_str()).map(str::to_string))
        .unwrap_or_default();
    let is = |w: &str| code == w || reason == w;
    match r.status {
        409 if is("space_unbound") => Refusal::SpaceUnbound(format!(
            "Sew says Space {space} is bound to no App (bind it again: k2 hostchat app-space bind <app> {space})"
        )),
        409 if is("app_mismatch") => {
            Refusal::AppMismatch(format!("Sew says Space {space} belongs to another App; an App makes channels in its own Space only"))
        }
        404 => Refusal::NotFound(format!("Sew has no Space {space} (k2 hostchat app-space list)")),
        400 => Refusal::Usage(format!("Sew refused the request: {}", r.error_line())),
        401 => Refusal::Unreachable("Sew refused K2's mgmt token".into()),
        s if s >= 500 => Refusal::Unreachable(format!("Sew answered {s}")),
        _ => Refusal::Sew(format!("Sew refused: {}", r.error_line())),
    }
}

async fn post<T: serde::de::DeserializeOwned>(client: &MgmtClient, path: &str, body: &impl serde::Serialize, actor: &str, space: &str) -> Result<T, Refusal> {
    let bytes = serde_json::to_vec(body).map_err(|e| Refusal::Store(format!("body: {e}")))?;
    let key = format!("k2ch_{}", uuid::Uuid::new_v4().simple());
    let r = client.post(path, &bytes, &key, &hook::sew_actor_header(actor)).await.map_err(|e| Refusal::Unreachable(e.0))?;
    if !r.is_success() {
        return Err(sew_refusal(&r, space));
    }
    serde_json::from_slice(&r.body).map_err(|e| Refusal::Unreachable(format!("{path}: unexpected answer ({e})")))
}

async fn create(app: String, link: Option<OnLink>, ask: Ask) -> Result<cc::Created, Refusal> {
    let mut prep = {
        let app = app.clone();
        blocking(move || cc::prepare(&app, ask)).await?
    };
    let target = blocking(|| hook::get_target(hook::CHAT_KIND).map_err(Refusal::Store)).await?.ok_or(Refusal::ChatOff)?;
    let client = MgmtClient::new(&target.socket, &target.token_file);
    let actor = actor(&app, link.as_ref());
    let space = prep.space_id.clone();
    if let Some(body) = prep.space_add() {
        let resp: sew_wire::SpaceMembersAddResponse = post(&client, sew_wire::MGMT_SPACE_MEMBERS_ADD, &body, &actor, &space).await?;
        prep = blocking(move || {
            let mut p = prep;
            cc::after_space_add(&mut p, &resp).map(|()| p)
        })
        .await?;
    }
    let resp: sew_wire::ChannelCreateResponse = post(&client, sew_wire::MGMT_CHANNELS_CREATE, &prep.channel_create(), &actor, &space).await?;
    let via = if link.is_some() { "link" } else { "remote" };
    blocking(move || cc::finish(prep, resp, &actor, via)).await
}

/// One call with an already-resolved pass (`None` = a `k2skn_` that didn't
/// resolve).
pub async fn handle(pass: Option<SkinPass>, link: Option<OnLink>, is_post: bool, body: Vec<u8>) -> CliResponse {
    if !is_post {
        return error(405, "usage", format!("{ROUTE} is a POST"), None);
    }
    let Some(pass) = pass else { return pass_invalid() };
    let Some(app) = app_of(&pass, link.as_ref()) else { return app_pass_required() };
    if !pass.has_cap(cc::CAP) {
        return error(
            403,
            "cap_not_granted",
            "this App's pass doesn't carry chat:channels (add it to the manifest's caps; the owner approves it once)",
            Some(json!({ "cap": cc::CAP })),
        );
    }
    let v: J = match serde_json::from_slice::<J>(&body) {
        Ok(v @ J::Object(_)) => v,
        _ => return error(400, "usage", "the body is a JSON object {space?, slug, name, private?, members?}", None),
    };
    let ask = match Ask::parse(&v) {
        Ok(a) => a,
        Err(e) => return refusal(&e),
    };
    match create(app, link, ask).await {
        Ok(out) => CliResponse::ok_json(serde_json::to_string(&out).unwrap_or_else(|_| "{}".into())),
        Err(e) => refusal(&e),
    }
}

#[cfg(test)]
#[path = "chat_channel_routes_tests.rs"]
mod tests;
