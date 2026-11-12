//! `GET /cli/docs/preview` (contract Item 8; K2's host markup §C.2).
//!
//! **Auth and rights:** the docs family's own (the dispatcher's caller
//! resolution: the owner token, a Connect login, an agent passport on this
//! machine; an App pass refused), then the doc's rights ([`super::super::routes`]):
//! the head needs **read**; a cut version (`version=`) needs **history**
//! (`file_at` needs it; K2 never implies history from read). A
//! cross-workspace agent needs a share, as for every docs route.
//!
//! **Two answers:**
//! - no `container`: the doc's previewable containers at the head (or the
//!   version): `{kind, rev, version, page, renderer, containers: [{key,
//!   position, title, hidden, render_hash, previews: {thumb, full}}]}`, each
//!   width's state `ready` (cached at this hash), `rendering`, `failed`, or
//!   `none` (not asked yet). With `want=thumb|full` the missing ones are
//!   queued.
//! - `container` (an address) and `width` (`thumb`, `thumb2x`, `full`),
//!   optionally `hash`: the image. `ready` with the image as base64 and its
//!   metadata (the contract's `Preview` table); else `202 {state:
//!   "rendering"}` (queued, and `preview_ready` follows on the docs socket),
//!   with the last image of that slide and width as `stale` when there is
//!   one; or `{state: "failed", failed: {code}}`. A `hash` that isn't the
//!   current one is served when cached (the hash names the bytes drawn),
//!   else `404 preview_gone`: list again.
//!
//! The docs routes speak JSON (every transport, Connect included), so the
//! image rides as base64 with its metadata, as Thread attachments do; a
//! client turns it into an object URL. No token ever goes in an `<img src>`.

use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};

use k2_mpc_base::session::{SessionError, Stay};

use super::cache::{self, Got};
use super::service::{self, Requested, SnapError, Snapshot};
use super::Width;
use crate::cli_response::CliResponse;
use crate::docs::caller::refusal;
use crate::docs::routes::{self, Call, OpenError};
use crate::docs::target::Tier;

pub const PREVIEW: &str = "/cli/docs/preview";

/// The route, after the docs family resolved the caller, the target and the
/// read right.
pub(crate) fn handle(rt: &Arc<crate::docs::Runtime>, c: &Call) -> CliResponse {
    let t = c.target;
    let Some(renderer) = super::renderer_for(t) else {
        let why = if crate::docs::target::is_deck_file(&t.rel) && t.tier == Tier::Blob {
            "previews are drawn for decks opened live by the Slides core, which is off on this server (k2 doc cores on slides)"
        } else {
            "K2 draws no previews of this kind of file yet"
        };
        return routes::with_exit(refusal("409 Conflict", "no_preview", why), k2_mpc_base::tools::exit::REFUSED);
    };
    let version = match c.input.u64("version") {
        Ok(v) => v,
        Err(r) => return r,
    };
    if version.is_some() && !c.rights.history {
        return routes::with_exit(
            refusal("403 Forbidden", "history_forbidden", "you may read this doc but not its history: a version's preview needs it"),
            k2_mpc_base::tools::exit::REFUSED,
        );
    }
    let bucket = |k: &str| -> Result<Option<Width>, CliResponse> {
        match c.input.str(k).map(|w| (Width::parse(&w), w)) {
            None => Ok(None),
            Some((Some(w), _)) => Ok(Some(w)),
            Some((None, w)) => Err(routes::bad(format!("{k} must be thumb, thumb2x or full (got {w})"))),
        }
    };
    let (width, want) = match (bucket("width"), bucket("want")) {
        (Ok(w), Ok(v)) => (w, v),
        (Err(r), _) | (_, Err(r)) => return r,
    };
    let Some(blobs) = rt.blobs() else {
        return refusal(
            "503 Service Unavailable",
            "previews_unavailable",
            "previews are sealed with the any-file store's keys, which aren't available on this server",
        );
    };
    let session = match routes::open(rt, t) {
        Ok(s) => s,
        Err(OpenError::CoreOff(r)) => return r,
        Err(OpenError::Session(SessionError::PackMismatch { .. } | SessionError::Declined(_))) => {
            return routes::with_exit(
                refusal("409 Conflict", "no_preview", "this deck is an any-file doc here, not a live deck: K2 draws no previews of it"),
                k2_mpc_base::tools::exit::REFUSED,
            )
        }
        Err(OpenError::Session(e)) => return routes::session_error(&e),
    };
    let joined = match session.join_shared(c.caller.participant(c.on_behalf), Stay::UntilIdle) {
        Ok(j) => j,
        Err(e) => return routes::session_error(&e),
    };
    let snap = match version {
        None => service::head(&blobs, renderer, &t.key(), &session, joined.conn),
        Some(v) => service::at_version(&blobs, renderer, &t.key(), &session, joined.conn, v),
    };
    let snap = match snap {
        Ok(s) => s,
        Err(SnapError::Session(e)) => return routes::session_error(&e),
        Err(SnapError::Failed(f)) => {
            return routes::ok(json!({ "kind": renderer.kind(), "state": "failed", "failed": { "code": f.code.as_str() } }))
        }
        Err(SnapError::NoKey(e)) => return refusal("503 Service Unavailable", "previews_unavailable", format!("no cache key: {e}")),
    };
    match (c.input.str("container"), width) {
        (None, _) => listing(&blobs, &snap, want),
        (Some(container), Some(w)) => image(&blobs, &snap, &container, c.input.str("hash").as_deref(), w),
        (Some(_), None) => routes::bad("width is required with a container: thumb, thumb2x or full"),
    }
}

fn state_of(snap: &Snapshot, container: &str, hash: &str, w: Width) -> &'static str {
    if cache::entry_exists(&snap.key, container, hash, w, &snap.build) {
        return "ready";
    }
    if service::rendering(&snap.key, container, hash, w) {
        return "rendering";
    }
    "none"
}

fn page_json(snap: &Snapshot) -> Value {
    snap.page.map_or(Value::Null, |p| json!({ "w": p.w, "h": p.h, "unit": p.unit }))
}

fn listing(blobs: &Arc<k2_mpc_base::blob::BlobStore>, snap: &Arc<Snapshot>, want: Option<Width>) -> CliResponse {
    let containers: Vec<Value> = snap
        .containers
        .iter()
        .map(|k| {
            if let Some(w) = want {
                if !cache::entry_exists(&snap.key, &k.c.key, &k.hash, w, &snap.build) {
                    service::request(blobs, snap, &k.c.key, w);
                }
            }
            let mut previews = serde_json::Map::new();
            for w in [Width::Thumb, Width::Full] {
                previews.insert(w.as_str().into(), json!(state_of(snap, &k.c.key, &k.hash, w)));
            }
            json!({
                "key": k.c.key,
                "position": k.c.position,
                "title": k.c.title,
                "hidden": k.c.hidden,
                "render_hash": k.hash,
                "previews": previews,
            })
        })
        .collect();
    routes::ok(json!({
        "kind": snap.renderer.kind(),
        "rev": snap.rev,
        "version": snap.version,
        "page": page_json(snap),
        "renderer": { "build": snap.build },
        "containers": containers,
    }))
}

fn with_image(state: &str, e: cache::Entry) -> Value {
    let mut v = e.meta;
    if let Some(o) = v.as_object_mut() {
        o.insert("state".into(), json!(state));
        o.insert("base64".into(), json!(base64::engine::general_purpose::STANDARD.encode(&e.image)));
    }
    v
}

fn image(blobs: &Arc<k2_mpc_base::blob::BlobStore>, snap: &Arc<Snapshot>, container: &str, hash: Option<&str>, w: Width) -> CliResponse {
    let Some(current) = snap.container(container) else {
        return routes::with_exit(
            refusal("404 Not Found", "no_such_container", format!("no {container} in this doc now (list it again)")),
            k2_mpc_base::tools::exit::NOT_FOUND,
        );
    };
    let hash = hash.map(str::trim).filter(|h| !h.is_empty()).unwrap_or(&current.hash);
    match cache::get(blobs, &snap.key, container, hash, w, &snap.build) {
        Got::Hit(e) => return routes::ok(with_image("ready", e)),
        Got::KeyDestroyed | Got::Miss => {}
    }
    if hash != current.hash {
        return routes::with_exit(
            refusal("404 Not Found", "preview_gone", "that preview isn't cached and the slide has changed since: list it again"),
            k2_mpc_base::tools::exit::NOT_FOUND,
        );
    }
    match service::request(blobs, snap, container, w) {
        Requested::Failed(code) => routes::ok(json!({
            "kind": snap.renderer.kind(),
            "container": container,
            "render_hash": hash,
            "width": w.as_str(),
            "rev": snap.rev,
            "page": page_json(snap),
            "state": "failed",
            "failed": { "code": code.as_str() },
        })),
        Requested::NoContainer => routes::bad(format!("no {container} in this doc")),
        Requested::Rendering | Requested::Busy => {
            let body = match cache::newest_other(blobs, &snap.key, container, hash, w, &snap.build) {
                Some(e) => {
                    let mut v = with_image("stale", e);
                    v["current_hash"] = json!(hash);
                    v
                }
                None => json!({
                    "kind": snap.renderer.kind(),
                    "container": container,
                    "render_hash": hash,
                    "width": w.as_str(),
                    "rev": snap.rev,
                    "page": page_json(snap),
                    "state": "rendering",
                }),
            };
            let mut r = routes::ok(body);
            r.status = "202 Accepted";
            r
        }
    }
}
