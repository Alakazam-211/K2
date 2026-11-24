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
//!   feed, and that feed's one poller per person.
//! - [`chat_push`]: phone pushes for Home chat DMs, @mentions and replies
//!   (prd-home-chat-pushes-v1), from the same poller.
//! - [`arrivals`]: DMs and channels that arrive on their own land in the
//!   person's People group (homes-people-group-v1).
//! - [`files`]: attachments (contract-home-chat-attachments-v1): resumable
//!   uploads and every file's bytes, through the daemon as the person.
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

pub mod arrivals;
pub mod call_push;
pub mod chat_push;
pub mod events_ws;
pub mod files;
pub mod follower;
pub mod notify;
pub mod people;
pub mod routes;
pub mod sessions;
pub mod state;

#[cfg(test)]
pub(crate) mod tests;
#[cfg(test)]
mod push_tests;
#[cfg(test)]
mod files_tests;

/// A pass minter: `(external id, sid)` → a signed chat pass.
pub type MintFn = dyn Fn(&str, &str) -> Result<String, String> + Send + Sync;
/// One read of a person's Sew feed: `(external id, after, wait seconds)` →
/// the HTTP status and body (`Err` only when Sew wasn't reached).
pub type FeedFn = dyn Fn(&str, u64, u64) -> Result<(u16, serde_json::Value), String> + Send + Sync;
/// A person's push devices: external id → (person key, device logins).
pub type DevicesFn = dyn Fn(&str) -> Option<(String, Vec<String>)> + Send + Sync;
/// One mgmt call as the install: `(method, path, body)` → the HTTP status and
/// body (`Err` only when Sew wasn't reached).
pub type MgmtFn = dyn Fn(&str, &str, Option<&serde_json::Value>) -> Result<(u16, serde_json::Value), String> + Send + Sync;

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
    /// Sew's Home chat push flags (prd-home-chat-pushes-v1 §3).
    pub push_flags: Arc<dyn Fn() -> k2_core::tool_hook::features::HomePushFeatures + Send + Sync>,
    /// Mgmt calls the push path makes (the feed head, a person's level).
    pub mgmt: Arc<MgmtFn>,
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
            push_flags: Arc::new(|| k2_core::tool_hook::sew_home_push_features().unwrap_or_default()),
            mgmt: Arc::new(call_push::live_mgmt),
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

/// Sew's data went back under K2 (`k2 hostchat rollback --accept-data-loss`,
/// `k2 hostchat restore`): K2's Home chat ledger for it is dropped or sent
/// again ([`k2_core::tool_hook::admin_channels::forget_sew_ids`]: DM ids,
/// channel ids, confirmed member lists), every held person starts over
/// (session, read state, gateway) at once, and the grant hook's sweep runs
/// now, so the status and Sew's feature list follow the Sew that runs, not
/// the one before. Never fails the operation that called it: what goes
/// wrong is logged.
pub fn sew_data_reset(why: &str) {
    let forgot = k2_core::tool_hook::with_home_chat_db(|conn| {
        let r = k2_core::tool_hook::admin_channels::forget_sew_ids(conn, now_secs())?;
        // The K5 Space, its rooms and its members are checked again (made
        // again if the bundle predates them) before the sweep sends anything
        // there: `k2 voice repair`'s full check. The members map is the
        // sweep's own reconcile.
        use k2_core::tool_hook::admin_space;
        admin_space::mark_verify(conn, admin_space::VERIFY_FULL, &format!("Sew's data was reset: {why}"), now_secs(), true)?;
        Ok(r)
    });
    // HC-2: a restored Sew's seqs are lower; a stored cursor above its head
    // would push nothing until Sew caught up. Every poller reads from the
    // start again (the age guards keep the backlog quiet).
    match k2_core::tool_hook::with_home_chat_db(k2_core::tool_hook::home_push::forget_cursors) {
        Ok(n) => k2_core::log_debug!("[home-chat] Sew's data was reset: {n} feed cursors forgotten"),
        Err(e) => k2_core::log_debug!("[home-chat] Sew's data was reset; feed cursors: {e}"),
    }
    call_push::restart_all();
    crate::app_push::chat::restart_all();
    match forgot {
        Ok(r) => k2_core::log_debug!(
            "[home-chat] Sew's data was reset ({why}): {} DM ids forgotten, {} channels to make again, {} people to add again, {} lists to resend",
            r.dms,
            r.channels,
            r.members,
            r.lists
        ),
        Err(e) => k2_core::log_debug!("[home-chat] Sew's data was reset ({why}); K2's ledger: {e}"),
    }
    for ext in follower::held_people() {
        if let Some(h) = follower::get(&ext) {
            h.start_over();
        }
    }
    k2_core::tool_hook::kick_reconcile();
}

/// `/boot-status` lists `home-chat-v1` only when Home chat is on and Sew is
/// set up and running here.
pub fn boot_status_key() -> Option<&'static str> {
    if !k2_core::home_chat::enabled() {
        return None;
    }
    status().0.then_some(k2_core::home_chat::FEATURE)
}

/// `/boot-status` lists `home-chat-attachments-v1` whenever it lists
/// `home-chat-v1` (contract-home-chat-attachments-v1): files ride every
/// Home chat this daemon serves.
pub fn attachments_boot_status_key() -> Option<&'static str> {
    boot_status_key().map(|_| k2_core::home_chat::ATTACHMENTS_FEATURE)
}

/// `/boot-status` lists `home-chat-push-v1` when the owner's `homeChatPush`
/// is on and Home chat runs here (prd-home-chat-pushes-v1 §7).
pub fn push_boot_status_key() -> Option<&'static str> {
    if !k2_core::home_chat::push_enabled() {
        return None;
    }
    status().0.then_some(k2_core::home_chat::PUSH_FEATURE)
}

async fn answer(stream: &mut tokio::net::TcpStream, r: &crate::cli_response::CliResponse) {
    crate::routes::http::send_response(stream, r.status, r.content_type, &r.body).await;
}

fn refusal(status: u16, code: &str, hint: &str) -> crate::cli_response::CliResponse {
    crate::voice::routes::refuse(&crate::voice::bff::Refusal::new(status, code, hint))
}

/// Serve one `/cli/home-chat/*` request on the dispatcher's stream (`buf`
/// is the dispatcher's read buffer; the head was only peeked: the body
/// reader reads it). `headers` is the request's head (an upload range's
/// `Content-Range`, a file fetch's `Range` and validators).
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
    headers: &str,
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
    if p == files::FILE {
        // A file's bytes, streamed (contract-home-chat-attachments-v1 §6).
        let _ = stream.read(buf).await;
        if is_post {
            return answer(stream, &crate::voice::routes::refuse(&crate::voice::bff::Refusal::new(405, "method_not_allowed", "this route takes GET"))).await;
        }
        let caller = people::caller(query, bearer, owner_token);
        let params = crate::routes::http::parse_params(path, query);
        let fwd = files::forwarded_headers(headers);
        let opened = tokio::task::spawn_blocking(move || files::open(caller, &params, &fwd)).await;
        return match opened {
            Ok(Ok(f)) => files::stream_out(stream, f).await,
            Ok(Err(e)) => answer(stream, &crate::voice::routes::refuse(&e)).await,
            Err(e) => answer(stream, &crate::cli_response::CliResponse::internal_error(format!("worker join: {e}"))).await,
        };
    }
    if routes::is_post_route(p) && !crate::routes::http::require_post(stream, buf, is_post).await {
        return;
    }
    // An upload's bytes take a larger body than any other Home chat route.
    let cap = if p == files::DATA { files::MAX_REQUEST_BYTES } else { routes::MAX_BODY };
    let body = if is_post {
        match crate::routes::http::read_post_body_capped(stream, buf, cap).await {
            Ok(b) => b,
            Err(_) if p == files::DATA => return answer(stream, &crate::voice::routes::refuse(&files::chunk_too_large())).await,
            Err(_) => return answer(stream, &refusal(413, "too_large", "the body is too large")).await,
        }
    } else {
        let _ = stream.read(buf).await;
        Vec::new()
    };
    let caller = people::caller(query, bearer, owner_token);
    let mut params = crate::routes::http::parse_params(path, query);
    if p == files::DATA {
        // The range a native client names in its header (a browser sends
        // `offset` instead: a Content-Range header needs a CORS preflight).
        params.remove(files::RANGE_PARAM);
        if let Some(r) = crate::routes::http::extract_header(crate::routes::http::request_head(headers), "content-range") {
            params.insert(files::RANGE_PARAM.to_string(), r.trim().to_string());
        }
    }
    let p_owned = p.to_string();
    let r = tokio::task::spawn_blocking(move || routes::handle(&p_owned, caller, is_post, &params, &body))
        .await
        .unwrap_or_else(|e| crate::cli_response::CliResponse::internal_error(format!("worker join: {e}")));
    answer(stream, &r).await;
}
