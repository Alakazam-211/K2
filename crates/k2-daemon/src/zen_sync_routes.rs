//! Garden sync and Garden news routes (prd-zen-garden-sync-defaults-v1
//! GS25, GS30, GS31, §8.3).
//!
//! Reached through `zen_routes::handle`, so every one answers ONLY the
//! local owner token, like every `/cli/zen/*` route (SD5): an agent
//! passport, a Connect login or an app pass gets 403 `zen_local_only`.
//! Agents edit Garden files and suggest the toggle to the person (GS32).
//!
//! - `GET  /cli/zen/sync`: every Garden's page and theme state.
//! - `POST /cli/zen/garden/sync`: `{garden, part, sync}`, `{garden, undo:
//!   true}` or `{garden, keep: "previous", part?}`. GET → 405.
//! - `GET  /cli/zen/news`: unseen catalog Gardens and default updates.
//! - `POST /cli/zen/news/seen`: `{ids}` or `{all: true}`. GET → 405.
//! - `GET  /cli/zen/get?preview=page|theme|both`: the Garden as if those
//!   parts were synced; writes no sync state.
//!
//! A sync change moves the `refresh` fingerprint, so it is announced with
//! ONE `zen_changed`.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::{json, Value as J};

use k2_core::zen::grants::GrantSnapshot;
use k2_core::zen::sync::{Parts, View};
use k2_core::zen::{ZenError, ZenFiles};

/// The POST rows this module adds to `zen_routes::POST_ROUTES`.
pub const POST_ROUTES: &[&str] = &["/cli/zen/garden/sync", "/cli/zen/news/seen"];
/// The GET rows this module adds to `zen_routes::GET_ROUTES`.
pub const GET_ROUTES: &[&str] = &["/cli/zen/news", "/cli/zen/sync"];

/// Whether `path` is one of this module's routes.
pub fn is_route(path: &str) -> bool {
    POST_ROUTES.contains(&path) || GET_ROUTES.contains(&path)
}

fn body_json(body: &[u8]) -> Result<J, ZenError> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(json!({}));
    }
    serde_json::from_slice(body).map_err(|e| ZenError::BadRequest(format!("invalid JSON body: {e}")))
}

fn parse_body<T: serde::de::DeserializeOwned>(body: &[u8], shape: &str) -> Result<T, ZenError> {
    serde_json::from_value(body_json(body)?).map_err(|e| ZenError::BadRequest(format!("{shape}: {e}")))
}

fn require(f: &ZenFiles) -> Result<(), ZenError> {
    if f.is_set_up() {
        Ok(())
    } else {
        Err(ZenError::NotSetUp)
    }
}

/// `GET /cli/zen/get`, with `preview=` (GS25). Called by `zen_routes` with
/// the request's grant snapshot.
pub fn get(f: &ZenFiles, garden: Option<&str>, preview: Option<&str>, grants: &GrantSnapshot) -> Result<J, ZenError> {
    match preview {
        None => f.resolve_with(garden, grants),
        Some(p) => f.resolve_view_with(garden, grants, &View::preview(Parts::parse(p)?)),
    }
}

/// The routes that change state: owner token only, and never a passport
/// even when its holder found the owner token (`zen_routes` checks the
/// caller, GS32).
pub fn is_post_route(path: &str) -> bool {
    POST_ROUTES.contains(&path)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GardenSyncBody {
    garden: String,
    part: Option<String>,
    sync: Option<bool>,
    #[serde(default)]
    undo: bool,
    keep: Option<String>,
}

const SYNC_SHAPE: &str = "garden/sync takes {garden, part, sync} or {garden, undo: true} or {garden, keep: \"previous\", part?}";

/// `POST /cli/zen/garden/sync` (GS31).
fn garden_sync(f: &ZenFiles, body: &[u8]) -> Result<J, ZenError> {
    let b: GardenSyncBody = parse_body(body, SYNC_SHAPE)?;
    require(f)?;
    let part = b.part.as_deref().map(Parts::parse).transpose()?.unwrap_or(Parts::Both);
    let out = match (b.sync, b.undo, b.keep.as_deref()) {
        (Some(on), false, None) => {
            if b.part.is_none() {
                return Err(ZenError::BadRequest(format!("{SYNC_SHAPE}: part is required with sync")));
            }
            f.set_garden_sync(&b.garden, part, on)?
        }
        (None, true, None) => {
            if b.part.is_some() {
                return Err(ZenError::BadRequest(format!("{SYNC_SHAPE}: undo takes no part")));
            }
            f.undo_garden_sync(&b.garden)?
        }
        (None, false, Some("previous")) => f.keep_previous(&b.garden, part)?,
        (None, false, Some(other)) => {
            return Err(ZenError::BadRequest(format!("keep must be \"previous\", not \"{other}\"")))
        }
        _ => return Err(ZenError::BadRequest(SYNC_SHAPE.into())),
    };
    let changed = if out.changed { crate::zen_routes::refresh_and_emit()? } else { false };
    let list = f.sync_list()?;
    let row = list["gardens"]
        .as_array()
        .and_then(|a| a.iter().find(|g| g["id"] == out.garden.as_str()).cloned())
        .unwrap_or(J::Null);
    Ok(json!({ "ok": true, "garden": row, "changed": out.changed, "announced": changed }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SeenBody {
    #[serde(default)]
    ids: Vec<String>,
    #[serde(default)]
    all: bool,
}

/// One sync or news request (the caller already checked the owner token).
pub fn handle(path: &str, _params: &HashMap<String, String>, body: &[u8]) -> Result<J, ZenError> {
    let f = ZenFiles::local();
    match path {
        "/cli/zen/sync" => {
            require(&f)?;
            f.sync_list()
        }
        "/cli/zen/garden/sync" => garden_sync(&f, body),
        "/cli/zen/news" => {
            require(&f)?;
            f.news()
        }
        "/cli/zen/news/seen" => {
            let b: SeenBody = parse_body(body, "news/seen takes {ids: [...]} or {all: true}")?;
            require(&f)?;
            f.news_seen(&b.ids, b.all)
        }
        other => Err(ZenError::NotFound(format!("unknown zen route {other}"))),
    }
}
