//! `/cli/home/avatars*` (prd-home-picker-and-remote-avatars-v1 P14, P17, P18;
//! vs-live P30, P34).
//!
//! The cache of other servers' agent images lives in THIS computer's
//! `~/.k2/cache/agent-avatars/`. Only the local daemon's owner token may
//! read or write it. The policy rows are `NoLogin`, so `role_gate` turns
//! every Connect login away with `role_required`; an agent passport or an
//! app pass passes `role_gate` and gets 403 `home_avatars_local_only` here.
//!
//! - `GET  /cli/home/avatars?addresses=a,b,…` (≤ 100) → `{avatars:{addr:{dataUrl,missing,fetchedAt,sha256}}}`
//! - `POST /cli/home/avatars/put`   `{address, dataUrl|null}` → `{changed, …}`
//! - `POST /cli/home/avatars/prune` `{keep:[addr,…]}` → `{removed, kept}`

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::{json, Value as J};

use k2_core::home_avatars::{AvatarCache, AvatarError};

use crate::cli_response::CliResponse;

/// Request body cap for `put` (P15).
pub const MAX_PUT_BODY: usize = 256 * 1024;
/// Request body cap for `prune` (P15).
pub const MAX_PRUNE_BODY: usize = 64 * 1024;
/// Most addresses one GET may ask for (P14 / P30).
pub const MAX_GET_ADDRESSES: usize = 100;

pub const GET_ROUTES: &[&str] = &["/cli/home/avatars"];

/// The POST rows. A GET on either is 405.
pub const POST_ROUTES: &[&str] = &["/cli/home/avatars/prune", "/cli/home/avatars/put"];

/// Is `path` one of this module's routes?
pub fn is_route(path: &str) -> bool {
    GET_ROUTES.contains(&path) || POST_ROUTES.contains(&path)
}

/// The body cap for a POST row (GET rows read no body).
pub fn max_body(path: &str) -> usize {
    if path == "/cli/home/avatars/put" {
        MAX_PUT_BODY
    } else {
        MAX_PRUNE_BODY
    }
}

fn resp(status: &'static str, body: J) -> CliResponse {
    CliResponse { status, content_type: "application/json", body: body.to_string() }
}

pub fn local_only() -> CliResponse {
    resp(
        "403 Forbidden",
        json!({
            "error": "home_avatars_local_only",
            "message": "The Home image cache belongs to this computer. Only this computer's own K2 can read or change it.",
        }),
    )
}

pub fn too_large(path: &str) -> CliResponse {
    resp(
        "413 Payload Too Large",
        json!({
            "error": "body_too_large",
            "message": format!("{path} takes bodies of at most {} bytes.", max_body(path)),
        }),
    )
}

fn bad(code: &str, message: impl Into<String>) -> CliResponse {
    resp("400 Bad Request", json!({ "ok": false, "error": code, "message": message.into() }))
}

fn avatar_err(e: AvatarError) -> CliResponse {
    match e {
        AvatarError::Io(m) => resp(
            "500 Internal Server Error",
            json!({ "ok": false, "error": "io", "message": m }),
        ),
        other => bad(other.code(), other.to_string()),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PutBody {
    address: String,
    /// `null` records "this agent has no image" (P16).
    data_url: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PruneBody {
    keep: Vec<String>,
}

/// Handle one request. `owner` is whether the caller presented the local
/// owner token; the dispatcher already consumed the request.
pub fn handle(path: &str, owner: bool, params: &HashMap<String, String>, body: &[u8]) -> CliResponse {
    handle_in(&AvatarCache::local(), path, owner, params, body)
}

/// [`handle`] against an explicit cache folder.
pub fn handle_in(
    cache: &AvatarCache,
    path: &str,
    owner: bool,
    params: &HashMap<String, String>,
    body: &[u8],
) -> CliResponse {
    if !owner {
        return local_only();
    }
    match path {
        "/cli/home/avatars" => get(cache, params),
        "/cli/home/avatars/put" => {
            let b: PutBody = match serde_json::from_slice(body) {
                Ok(b) => b,
                Err(e) => return bad("bad_request", format!("body must be {{address, dataUrl}}: {e}")),
            };
            match cache.put(&b.address, b.data_url.as_deref()) {
                Ok(out) => resp("200 OK", json!({ "ok": true, "put": out, "changed": out.changed })),
                Err(e) => avatar_err(e),
            }
        }
        "/cli/home/avatars/prune" => {
            let b: PruneBody = match serde_json::from_slice(body) {
                Ok(b) => b,
                Err(e) => return bad("bad_request", format!("body must be {{keep: [address, …]}}: {e}")),
            };
            match cache.prune(&b.keep) {
                Ok(out) => resp(
                    "200 OK",
                    json!({ "ok": true, "removed": out.removed, "kept": out.kept, "removedServers": out.removed_servers }),
                ),
                Err(e) => avatar_err(e),
            }
        }
        _ => CliResponse::not_found(),
    }
}

fn get(cache: &AvatarCache, params: &HashMap<String, String>) -> CliResponse {
    let raw = params.get("addresses").map(String::as_str).unwrap_or("");
    let addresses: Vec<&str> = raw.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
    if addresses.len() > MAX_GET_ADDRESSES {
        return bad(
            "too_many",
            format!("ask for at most {MAX_GET_ADDRESSES} addresses at a time; got {}", addresses.len()),
        );
    }
    let mut out = serde_json::Map::new();
    for a in addresses {
        // An address that can't name a file can't have an entry: leave it out.
        let Ok((norm, _, _)) = k2_core::home_avatars::address_key(a) else { continue };
        match cache.get(&norm) {
            Ok(Some(entry)) => {
                out.insert(norm, serde_json::to_value(entry).expect("entry serializes"));
            }
            Ok(None) => {}
            Err(e) => return avatar_err(e),
        }
    }
    resp("200 OK", json!({ "ok": true, "avatars": out }))
}
