//! App-governed Spaces (Sew 0.6.0, prd-app-governed-spaces-k2-v1 §5.4): an
//! App's own calls for the channels it makes at run time.
//!
//! ```text
//! POST /cli/sew/app/member-channels {space?, items: [{username, channels: [slug], level?}..≤500]}
//!   → 200 {ok, app, spaceId, changed, items: [{username, ok, error?}]}
//! POST /cli/sew/app/channels/bulk {space?, ops: [{op, slug?, channelId?, name?, type?, private?, userLimit?}..≤200]}
//!   → 200 {ok, app, spaceId, results: [...]}   (Sew's I6 answer, per op)
//! ```
//!
//! - **Only the App's own pass** (its gateway-link pass, or its remote
//!   pass): never a guest's session, the owner token or a login.
//! - **A chat key and Sew's flag** `app_governed_spaces`, else `409
//!   chat_key_required` / `sew_feature_missing`, before Sew is asked.
//! - **Its own Space only**: the one it names (bound to it) or its only
//!   one; another App's is `409 app_mismatch`.
//! - `member-channels` sets each person's **App-managed list in full**
//!   (K2 sends role list ∪ App list, so K2's sync and the App never
//!   overwrite each other). `username` names this App's own user; anyone
//!   else is `not_found` for that item (no oracle), an agent login is
//!   `agent_subject`. The sync goes out within seconds.
//! - `channels/bulk` proxies Sew's I6 as is; K2 records each channel's id
//!   so `member-channels` can name it by slug.
//! - 60 calls a minute per App (both routes).
//!
//! Catalog rows (v42, at integration): `chat.channels` (channels/bulk) and
//! `chat.setChannels` (member-channels), cap `chat:channels`, so the pass
//! gate admits an App's pass here. S8 does the SDK helpers and the study
//! page. Off a link, an App's remote pass reaches both.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use k2_core::skin::SkinPass;
use k2_core::tool_hook::{self as hook, governed::AppList, sew_wire};
use serde_json::{json, Value as J};

use crate::cli_response::CliResponse;
use crate::guest_rooms_routes::{app_of, error, pass_invalid, OnLink};
use crate::sew_mgmt::MgmtClient;

pub const MEMBER_CHANNELS: &str = "/cli/sew/app/member-channels";
pub const CHANNELS_BULK: &str = "/cli/sew/app/channels/bulk";

/// Calls a minute per App, both routes together.
pub const RATE_PER_MIN: usize = 60;

pub fn is_route(p: &str) -> bool {
    p == MEMBER_CHANNELS || p == CHANNELS_BULK
}

pub fn app_pass_required() -> CliResponse {
    error(
        403,
        "app_pass_required",
        "an App's chat channels take this App's own pass (K2_PASS_FILE inside the wall, or its remote pass); never a guest's pass, the owner token or a login",
        None,
    )
}

fn limiter() -> &'static Mutex<HashMap<String, VecDeque<Instant>>> {
    static L: std::sync::OnceLock<Mutex<HashMap<String, VecDeque<Instant>>>> = std::sync::OnceLock::new();
    L.get_or_init(|| Mutex::new(HashMap::new()))
}

/// `Some(retry after ms)` when `app` is over [`RATE_PER_MIN`].
fn rate_limited(app: &str) -> Option<u64> {
    let window = Duration::from_secs(60);
    let now = Instant::now();
    let mut m = limiter().lock().unwrap_or_else(|p| p.into_inner());
    let q = m.entry(app.to_string()).or_default();
    while q.front().is_some_and(|t| now.duration_since(*t) >= window) {
        q.pop_front();
    }
    if q.len() >= RATE_PER_MIN {
        let oldest = *q.front().expect("non-empty");
        return Some((window - now.duration_since(oldest)).as_millis() as u64 + 1);
    }
    q.push_back(now);
    None
}

/// The audit actor and `Sew-Actor`: `app:<id>` on its link,
/// `app-remote:<service id>` off it.
fn actor(app: &str, link: Option<&OnLink>) -> String {
    match link {
        Some(l) => format!("app:{}", l.app_id),
        None => format!("app-remote:{app}"),
    }
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, CliResponse> + Send + 'static) -> Result<T, CliResponse> {
    tokio::task::spawn_blocking(f).await.map_err(|e| error(500, "internal", format!("task: {e}"), None))?
}

fn store(e: String) -> CliResponse {
    error(503, "chat_unavailable", e, None)
}

/// One call with an already-resolved pass (`None` = a `k2skn_` that didn't
/// resolve).
pub async fn handle(route: &str, pass: Option<SkinPass>, link: Option<OnLink>, is_post: bool, body: Vec<u8>) -> CliResponse {
    if !is_post {
        return error(405, "usage", format!("{route} is a POST"), None);
    }
    let Some(pass) = pass else { return pass_invalid() };
    let Some(app) = app_of(&pass, link.as_ref()) else { return app_pass_required() };
    let v: J = match serde_json::from_slice::<J>(&body) {
        Ok(v @ J::Object(_)) => v,
        _ => return error(400, "usage", "the body is a JSON object", None),
    };
    if let Some(ms) = rate_limited(&app) {
        return error(429, "rate_limited", format!("at most {RATE_PER_MIN} chat channel calls a minute per App"), Some(json!({"retryAfterMs": ms})));
    }
    let space_in = v.get("space").and_then(J::as_str).map(str::to_string);
    let space = {
        let (app, space_in) = (app.clone(), space_in.clone());
        match blocking(move || hook::governed_space_for_app(&app, space_in.as_deref()).map_err(store)).await {
            Ok(Ok(s)) => s,
            Ok(Err((status, code, hint))) => return error(status, code, hint, None),
            Err(r) => return r,
        }
    };
    let actor = actor(&app, link.as_ref());
    if route == MEMBER_CHANNELS {
        member_channels(app, space, actor, v).await
    } else {
        channels_bulk(app, space, actor, v).await
    }
}

async fn member_channels(app: String, space: String, actor: String, v: J) -> CliResponse {
    let Some(items) = v.get("items").and_then(J::as_array).filter(|a| (1..=sew_wire::MEMBER_CHANNELS_BULK_MAX).contains(&a.len())) else {
        return error(400, "usage", format!("items: 1 to {} of {{username, channels: [slug], level?}}", sew_wire::MEMBER_CHANNELS_BULK_MAX), None);
    };
    struct Item {
        username: String,
        slugs: Vec<String>,
        level: Option<String>,
    }
    let mut parsed = Vec::new();
    for (i, it) in items.iter().enumerate() {
        let username = it.get("username").and_then(J::as_str).map(str::trim).unwrap_or("").to_string();
        let slugs: Option<Vec<String>> =
            it.get("channels").and_then(J::as_array).map(|a| a.iter().filter_map(J::as_str).map(str::to_string).collect());
        let level = it.get("level").and_then(J::as_str).map(str::to_string);
        let Some(slugs) = slugs.filter(|s| s.len() <= sew_wire::MEMBER_CHANNELS_MAX && s.iter().all(|x| sew_wire::is_channel_slug(x))) else {
            return error(400, "usage", format!("items[{i}].channels: up to {} channel slugs", sew_wire::MEMBER_CHANNELS_MAX), None);
        };
        if username.is_empty() {
            return error(400, "usage", format!("items[{i}].username: name this App's user"), None);
        }
        if level.as_deref().is_some_and(|l| sew_wire::level_rank(l).is_none()) {
            return error(400, "usage", format!("items[{i}].level: member, moderator or admin"), None);
        }
        parsed.push(Item { username, slugs, level });
    }
    let out = blocking(move || {
        let mut lists = Vec::new();
        let mut answers = Vec::new();
        for it in parsed {
            match hook::app_user_for_call(&app, &it.username).map_err(store)? {
                Ok(principal_id) => {
                    lists.push(AppList { principal_id, space_id: space.clone(), slugs: it.slugs, level: it.level });
                    answers.push(json!({"username": it.username, "ok": true}));
                }
                Err(code) => answers.push(json!({"username": it.username, "ok": false, "error": code})),
            }
        }
        let changed = hook::set_app_member_lists(&app, &lists, &actor).map_err(store)?;
        Ok(json!({"ok": true, "app": app, "spaceId": space, "changed": changed, "items": answers}))
    })
    .await;
    match out {
        Ok(v) => CliResponse::ok_json(v.to_string()),
        Err(r) => r,
    }
}

async fn channels_bulk(app: String, space: String, actor: String, v: J) -> CliResponse {
    let ops: Vec<sew_wire::ChannelOp> = match v.get("ops").cloned().map(serde_json::from_value::<Vec<sew_wire::ChannelOp>>) {
        Some(Ok(o)) if (1..=sew_wire::CHANNELS_BULK_MAX).contains(&o.len()) => o,
        Some(Err(e)) => return error(400, "usage", format!("ops: {e}"), None),
        _ => return error(400, "usage", format!("ops: 1 to {} of {{op, slug?, channelId?, name?, type?, private?, userLimit?}}", sew_wire::CHANNELS_BULK_MAX), None),
    };
    for (i, o) in ops.iter().enumerate() {
        let ok = match o.op.as_str() {
            "ensure" => o.slug.as_deref().is_some_and(sew_wire::is_channel_slug) && o.name.as_deref().is_some_and(|n| (1..=100).contains(&n.trim().chars().count())) && matches!(o.kind.as_deref(), None | Some("text") | Some("voice")),
            "rename" => (o.slug.is_some() || o.channel_id.is_some()) && o.name.is_some(),
            "archive" | "unarchive" => o.slug.is_some() || o.channel_id.is_some(),
            _ => false,
        };
        if !ok {
            return error(400, "usage", format!("ops[{i}]: ensure {{slug, name, type: text|voice}}, rename {{slug|channelId, name}}, archive|unarchive {{slug|channelId}}"), None);
        }
    }
    let req = sew_wire::ChannelsBulkRequest { space_id: space.clone(), app_id: app.clone(), ops };
    let target = match blocking(|| hook::get_target(hook::CHAT_KIND).map_err(store)).await {
        Ok(Some(t)) => t,
        Ok(None) => return error(409, "chat_unavailable", "chat is not set up on this box", None),
        Err(r) => return r,
    };
    let client = MgmtClient::new(&target.socket, &target.token_file);
    let bytes = match serde_json::to_vec(&req) {
        Ok(b) => b,
        Err(e) => return error(500, "internal", format!("body: {e}"), None),
    };
    let key = format!("k2cb_{}", uuid::Uuid::new_v4().simple());
    let r = match client.post(sew_wire::MGMT_CHANNELS_BULK, &bytes, &key, &hook::sew_actor_header(&actor)).await {
        Ok(r) => r,
        Err(e) => return error(503, "sew_unreachable", e.0, None),
    };
    if !r.is_success() {
        let e = r.error();
        let reason = e.as_ref().and_then(|e| e.error.detail_str("reason").map(str::to_string)).unwrap_or_default();
        let code = if reason.is_empty() { "sew_refused".to_string() } else { reason };
        let status = if (400..500).contains(&r.status) { r.status } else { 502 };
        return error(status, &code, format!("Sew refused: {}", r.error_line()), None);
    }
    let resp: sew_wire::ChannelsBulkResponse = match serde_json::from_slice(&r.body) {
        Ok(a) => a,
        Err(e) => return error(502, "sew_refused", format!("Sew's answer is not a bulk channel answer ({e})"), None),
    };
    let rec = {
        let (space, app, req, resp) = (space.clone(), app.clone(), req.clone(), resp.clone());
        blocking(move || hook::note_app_channel_ops(&space, &app, &req, &resp).map_err(store)).await
    };
    if let Err(r) = rec {
        return r;
    }
    CliResponse::ok_json(json!({"ok": true, "app": app, "spaceId": space, "results": resp.results}).to_string())
}
