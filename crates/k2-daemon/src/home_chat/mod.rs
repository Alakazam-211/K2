//! People and channels on Home, the daemon half (prd-home-people-channels-v1
//! slices S2 and S3; the ledger is `k2_core::tool_hook::admin_channels`).
//!
//! - [`sessions`]: each person's Sew session, held by the daemon only while
//!   they have a live login session here, renewed on the live socket.
//! - [`follower`]: one Sew gateway websocket per held person; unread,
//!   catch-up, and frames for that person's clients.
//! - [`state`]: the pure per-person state (unread, badges).
//! - [`routes`]: `/cli/home-chat/*` (feature `home-chat-v1`).
//! - [`events_ws`]: `WS /cli/home-chat/events`, the person's own socket.
//! - [`call_push`]: phone pushes for DM call rings (S14), from Sew's person
//!   feed.
//!
//! **Who.** Only a person's own login on this server: a Connect login
//! (`k2:connect:<uuid>`) or the owner's linked login (`k2:owner`). The bare
//! owner token, agent passports and App sessions get 403: agents use
//! `k2 chat`, and Sew chat is for people (§13).
//!
//! **Dark** (404 `home_chat_off`) while the `homeChat` switch is off. Linux
//! servers only (Sew runs on Linux).

use std::sync::{Arc, OnceLock, RwLock};

use k2_core::tool_hook::features::HomeChatFeatures;

use crate::voice::bff::{HttpSew, SewApi};

pub mod call_push;
pub mod events_ws;
pub mod follower;
pub mod people;
pub mod routes;
pub mod sessions;
pub mod state;

#[cfg(test)]
pub(crate) mod tests;

/// A pass minter: `(external id, sid)` → a signed chat pass.
pub type MintFn = dyn Fn(&str, &str) -> Result<String, String> + Send + Sync;
/// One read of a person's Sew feed: `(external id, cursor)` → its answer.
pub type FeedFn = dyn Fn(&str, Option<&str>) -> Result<serde_json::Value, String> + Send + Sync;
/// A person's push devices: external id → (person key, device logins).
pub type DevicesFn = dyn Fn(&str) -> Option<(String, Vec<String>)> + Send + Sync;

/// What Home chat talks to. Production: [`Deps::live`]; tests install a
/// fake Sew (HTTP and gateway) through [`set_deps`].
pub struct Deps {
    pub sew: Arc<dyn SewApi>,
    pub mint: Arc<MintFn>,
    /// Sew's loopback client listener (`127.0.0.1:<port>`): the gateway
    /// socket opens there, never anywhere else.
    pub gateway_addr: Arc<dyn Fn() -> String + Send + Sync>,
    /// Sew's Home flags, from the last status refresh.
    pub flags: Arc<dyn Fn() -> HomeChatFeatures + Send + Sync>,
    /// Sew runs here (Linux).
    pub platform_ok: bool,
    /// The person feed DM call pushes follow ([`call_push::live_feed`]).
    pub feed: Arc<FeedFn>,
    /// Whose phones a call push reaches ([`call_push::live_devices`]).
    pub devices: Arc<DevicesFn>,
}

fn live_mint(ext: &str, sid: &str) -> Result<String, String> {
    let install = crate::chat_pass::link::install_id_for_mint(&crate::chat_pass::link::SewRow)?;
    let claims = crate::chat_pass::ChatPassClaims::new(&install, ext, sid, crate::chat_pass::keyring::now())?;
    crate::chat_pass::sign_chat_pass(&claims).map_err(|e| e.to_string())
}

impl Deps {
    pub fn live() -> Deps {
        Deps {
            sew: Arc::new(HttpSew),
            mint: Arc::new(live_mint),
            gateway_addr: Arc::new(crate::sew::ops::client_addr),
            flags: Arc::new(|| k2_core::tool_hook::sew_home_chat_features().unwrap_or_default()),
            platform_ok: crate::sew::sew_supported(),
            feed: Arc::new(call_push::live_feed),
            devices: Arc::new(call_push::live_devices),
        }
    }
}

fn seam() -> &'static RwLock<Option<Arc<Deps>>> {
    static S: RwLock<Option<Arc<Deps>>> = RwLock::new(None);
    &S
}

/// The deps in use.
pub fn deps() -> Arc<Deps> {
    if let Some(d) = seam().read().unwrap_or_else(|p| p.into_inner()).clone() {
        return d;
    }
    static LIVE: OnceLock<Arc<Deps>> = OnceLock::new();
    LIVE.get_or_init(|| Arc::new(Deps::live())).clone()
}

/// Tests: swap the deps (`None` = live). Never called in production.
#[doc(hidden)]
pub fn set_deps(d: Option<Arc<Deps>>) {
    *seam().write().unwrap_or_else(|p| p.into_inner()) = d;
}

pub fn now_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Is Home chat usable here, and if not, why (`off`, `mac`, `sew_off`,
/// `space_pending`)?
pub fn status() -> (bool, Option<&'static str>) {
    if !k2_core::home_chat::enabled() {
        return (false, Some("off"));
    }
    if !deps().platform_ok {
        return (false, Some("mac"));
    }
    if !k2_core::tool_hook::get_target(k2_core::tool_hook::CHAT_KIND).ok().flatten().is_some_and(|t| t.enabled) {
        return (false, Some("sew_off"));
    }
    if k2_core::tool_hook::admin_space_id().ok().flatten().is_none() {
        return (false, Some("space_pending"));
    }
    (true, None)
}

/// `/boot-status` lists `home-chat-v1` only when Home chat is on and Sew is
/// set up and running here.
pub fn boot_status_key() -> Option<&'static str> {
    if !k2_core::home_chat::enabled() {
        return None;
    }
    status().0.then_some(k2_core::home_chat::FEATURE)
}

async fn answer(stream: &mut tokio::net::TcpStream, r: &crate::cli_response::CliResponse) {
    crate::routes::http::send_response(stream, r.status, r.content_type, &r.body).await;
}

fn refusal(status: u16, code: &str, hint: &str) -> crate::cli_response::CliResponse {
    crate::voice::routes::refuse(&crate::voice::bff::Refusal::new(status, code, hint))
}

/// Serve one `/cli/home-chat/*` request on the dispatcher's stream (`buf`
/// is the dispatcher's read buffer; the head is already consumed).
#[allow(clippy::too_many_arguments)]
pub async fn serve(
    stream: &mut tokio::net::TcpStream,
    buf: &mut [u8],
    p: &str,
    path: &str,
    query: &str,
    is_post: bool,
    bearer: Option<&str>,
    owner_token: &str,
) {
    use tokio::io::AsyncReadExt;
    if !k2_core::home_chat::enabled() {
        let _ = stream.read(buf).await;
        return answer(stream, &routes::dark()).await;
    }
    if p == routes::EVENTS {
        // The upgrade reads the request itself: drain it only to refuse.
        let refused = match people::caller(query, bearer, owner_token) {
            Ok(c) if status().0 => match events_ws::take_slot(&c.login) {
                Some(slot) => Ok((c, slot)),
                None => Err(refusal(429, "too_many_sockets", "at most 3 Home chat event sockets per login")),
            },
            Ok(_) => Err(refusal(503, "chat_unavailable", "chat isn't available on this server")),
            Err(e) => Err(crate::voice::routes::refuse(&e)),
        };
        let (c, _slot) = match refused {
            Ok(x) => x,
            Err(r) => {
                let _ = stream.read(buf).await;
                return answer(stream, &r).await;
            }
        };
        let token = crate::routes::http::extract_token(query).filter(|t| !t.is_empty()).or(bearer).unwrap_or("").to_string();
        return events_ws::serve(stream, c.ext, token, owner_token.to_string()).await;
    }
    if routes::is_post_route(p) && !crate::routes::http::require_post(stream, buf, is_post).await {
        return;
    }
    let body = if is_post {
        match crate::routes::http::read_post_body_capped(stream, buf, routes::MAX_BODY).await {
            Ok(b) => b,
            Err(_) => return answer(stream, &refusal(413, "too_large", "the body is too large")).await,
        }
    } else {
        let _ = stream.read(buf).await;
        Vec::new()
    };
    let caller = people::caller(query, bearer, owner_token);
    let params = crate::routes::http::parse_params(path, query);
    let p_owned = p.to_string();
    let r = tokio::task::spawn_blocking(move || routes::handle(&p_owned, caller, is_post, &params, &body))
        .await
        .unwrap_or_else(|e| crate::cli_response::CliResponse::internal_error(format!("worker join: {e}")));
    answer(stream, &r).await;
}
