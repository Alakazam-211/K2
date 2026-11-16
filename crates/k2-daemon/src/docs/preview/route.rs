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
//!   else `404 preview_gone`: list again. A container the doc already has
//!   drawn under another address at the same hash (a renamed scene object)
//!   is `ready` at once, its metadata saying `reused_from`.
//! - the same with `layer=ids`: the image's id image (a scene, object or
//!   camera view's: which object is under each pixel), as base64 with
//!   `layer: "ids"`, `format: "png"` and the image's naming fields; its
//!   table is in the image's metadata (`ids`). `ready`, `rendering` (never
//!   with a stale one), `failed` or `404 preview_gone` as above, and `404
//!   no_id_image` for a container drawn without one. A cached image whose
//!   id image went (the cap) is drawn again.
//!
//! **`look`** (`opaque` or `transparent`; each type has its default: a deck
//! opaque, a scene transparent, the 3D tab's one still for all its looks).
//! A deck refuses `transparent`. Answers name `look` only when it is
//! `transparent`. **`shape`** (`wide`, the default, 16:9; or `tall`, 4:3):
//! the shape of a still a type shapes (a scene's `@scene:main` and `@obj:`
//! views). It is ignored for every other container (a scene's `@cam:` view
//! keeps its camera's aspect and a `@turntable:` frame the renderer's; a
//! slide its page; a timeline's tile its clip's), never refused: they are
//! drawn and cached once, and asking them for `tall` answers the image they
//! have. Answers about a shaped view name `shape` (wide included); a listing
//! names it when the doc has one. **Per-type extras** ride in each listed
//! container (a scene object's `bounds`, a camera's `camera` display values)
//! and in an image's metadata (a scene view's `camera` and `ids`):
//! [`super::extras`]. A current image's id table names the objects as the
//! doc does now ([`service::with_current_ids`]).
//!
//! **Not on this host.** A container the host can't draw at all (a
//! timeline's tile where there is no video decoder:
//! `PreviewRenderer::decodes_here`) is answered at once: the image is `200
//! {state: "failed", failed: {code: "not_on_this_host"}}` (as every failed
//! image), and a listing gives that container `failed` at every width plus
//! `failed: {code: "not_on_this_host"}`. Nothing is queued or pushed.
//!
//! **An agent's view.** An agent asking for a still of the head (a
//! `container`) is looking there: its connection's presence `view_address`
//! becomes that address ([`agent_viewing`]), so the roster says where it is
//! and people can Follow it. A person's view is their tab's own.
//!
//! The docs routes speak JSON (every transport, Connect included), so the
//! image rides as base64 with its metadata, as Thread attachments do; a
//! client turns it into an object URL. No token ever goes in an `<img src>`.

use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};

use k2_mpc_base::session::{SessionError, Stay};

use super::cache::{self, Got};
use super::service::{self, Keyed, Requested, SnapError, Snapshot};
use super::{extras, FailCode, Look, Shape, Width};
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
    let ids = match c.input.str("layer").as_deref().map(str::trim) {
        None => false,
        Some(extras::IDS_LAYER) => true,
        Some(l) => return routes::bad(format!("layer must be {} (or left out for the image; got {l})", extras::IDS_LAYER)),
    };
    let look = match c.input.str("look") {
        None => renderer.default_look(),
        Some(l) => match Look::parse(&l) {
            Some(l) if renderer.draws(l) => l,
            Some(l) => return routes::bad(format!("a {} preview isn't drawn {}", renderer.kind(), l.as_str())),
            None => return routes::bad(format!("look must be opaque or transparent (got {l})")),
        },
    };
    let shape = match shape_param(c.input.str("shape").as_deref()) {
        Ok(s) => s,
        Err(e) => return routes::bad(e),
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
        (None, _) => listing(&blobs, &snap, want, look, shape),
        (Some(container), Some(w)) => {
            // An agent asking for a still of the head is looking there: its
            // view address follows, so people can Follow it.
            if version.is_none() && snap.container(&container).is_some() {
                agent_viewing(&session, joined.conn, c.caller, &container);
            }
            image(&blobs, &snap, &container, c.input.str("hash").as_deref(), w, look, shape, ids)
        }
        (Some(_), None) => routes::bad("width is required with a container: thumb, thumb2x or full"),
    }
}

/// An agent asked for a still of `container` (an address the doc lists now):
/// its connection's view address (kit v0.18.0, `Session::set_view_address`:
/// "a host sets one for any connection, an agent's when it renders a still
/// for it") becomes that address, so the roster's `view_address` says where
/// it is looking and a person can Follow it. The connection is the agent's
/// shared one (`join_shared`, as every `k2 doc` verb), so the view goes when
/// the agent leaves (`k2 doc leave`) or its connection idles out: the kit
/// drops a connection's presence with it.
///
/// A person's view is never moved here: their tab sets its own (a still the
/// app fetches for them says nothing about where they look). An address the
/// kit's grammar doesn't read as one (`@scene`, an index form like `@cam#2`)
/// can't be followed, so the agent's view is cleared rather than left on its
/// last still.
pub(crate) fn agent_viewing(session: &k2_mpc_base::session::Session, conn: k2_mpc_base::session::ConnId, caller: &crate::docs::caller::DocCaller, container: &str) {
    if caller.is_person() {
        return;
    }
    let set = match session.set_view_address(conn, Some(container)) {
        Err(SessionError::BadPosition(why)) => {
            k2_core::log_debug!("[docs/preview] {container} isn't an address presence takes ({why}); the agent's view is cleared");
            session.set_view_address(conn, None).map(drop)
        }
        r => r.map(drop),
    };
    if let Err(e) = set {
        k2_core::log_debug!("[docs/preview] the agent's view address wasn't set: {e}");
    }
}

/// The `shape` parameter: `wide` when it is left out. Taken for every
/// container and type (ignored where it doesn't apply), so only a word that
/// isn't a shape is refused.
pub(super) fn shape_param(v: Option<&str>) -> Result<Shape, String> {
    match v {
        None => Ok(Shape::Wide),
        Some(s) => Shape::parse(s).ok_or_else(|| format!("shape must be wide or tall (got {s})")),
    }
}

fn state_of(snap: &Snapshot, container: &str, hash: &str, w: Width, look: Look, shape: Shape) -> &'static str {
    if cache::entry_exists(&snap.key, container, hash, w, &snap.cache_build(container, look, shape)) {
        return "ready";
    }
    if service::rendering(&snap.key, container, hash, w, look, snap.drawn_shape(container, shape)) {
        return "rendering";
    }
    "none"
}

/// One listed container: its address, place, title, keyed `render_hash`
/// (never the raw digest), the state of each width in `look` (and `shape`,
/// when its renderer shapes it), then the type's extras.
pub fn container_json(snap: &Snapshot, k: &Keyed, look: Look, shape: Shape) -> Value {
    // A container this host can't draw at all: every width `failed`, and
    // why (`failed.code: not_on_this_host`), so the view shows its "no
    // preview" card without asking.
    let here = snap.renderer.decodes_here(&k.c);
    let mut previews = serde_json::Map::new();
    for w in [Width::Thumb, Width::Full] {
        let state = if here { state_of(snap, &k.c.key, &k.hash, w, look, shape) } else { "failed" };
        previews.insert(w.as_str().into(), json!(state));
    }
    let mut v = json!({
        "key": k.c.key,
        "position": k.c.position,
        "title": k.c.title,
        "hidden": k.c.hidden,
        "render_hash": k.hash,
        "previews": previews,
    });
    if !here {
        v["failed"] = json!({ "code": FailCode::NotOnThisHost.as_str() });
    }
    for (x, val) in &k.c.extras {
        v[x.as_str()] = val.clone();
    }
    v
}

/// `look` in an answer: only when it isn't opaque.
fn with_look(mut v: Value, look: Look) -> Value {
    if let Some(l) = service::look_field(look) {
        v["look"] = json!(l);
    }
    v
}

/// `look`, then `shape` in an answer about `container`: the shape only when
/// its renderer shapes it.
fn with_ask(v: Value, snap: &Snapshot, container: &str, look: Look, shape: Shape) -> Value {
    let mut v = with_look(v, look);
    if let Some(s) = snap.shape_field(container, shape) {
        v["shape"] = json!(s);
    }
    v
}

fn page_json(snap: &Snapshot) -> Value {
    snap.page.map_or(Value::Null, |p| json!({ "w": p.w, "h": p.h, "unit": p.unit }))
}

/// The listing: `shape` named when the doc has a view its renderer shapes.
pub(super) fn listing(blobs: &Arc<k2_mpc_base::blob::BlobStore>, snap: &Arc<Snapshot>, want: Option<Width>, look: Look, shape: Shape) -> CliResponse {
    let containers: Vec<Value> = snap
        .containers
        .iter()
        .map(|k| {
            if let Some(w) = want {
                if !cache::entry_exists(&snap.key, &k.c.key, &k.hash, w, &snap.cache_build(&k.c.key, look, shape)) {
                    service::request(blobs, snap, &k.c.key, w, look, shape);
                }
            }
            container_json(snap, k, look, shape)
        })
        .collect();
    let mut v = with_look(
        json!({
            "kind": snap.renderer.kind(),
            "rev": snap.rev,
            "version": snap.version,
            "page": page_json(snap),
            "renderer": { "build": snap.build },
            "containers": containers,
        }),
        look,
    );
    if snap.containers.iter().any(|k| snap.renderer.shapes(&k.c)) {
        v["shape"] = json!(shape.as_str());
    }
    routes::ok(v)
}

fn with_image(state: &str, e: cache::Entry) -> Value {
    let mut v = e.meta;
    if let Some(o) = v.as_object_mut() {
        o.insert("state".into(), json!(state));
        o.insert("base64".into(), json!(base64::engine::general_purpose::STANDARD.encode(&e.image)));
    }
    v
}

/// A cached image (or with `ids`, its id image) as answered: a current
/// image's id table named as the doc is now.
fn answer(snap: &Snapshot, current: &Keyed, mut e: cache::Entry, ids: bool) -> Value {
    if !ids {
        service::with_current_ids(snap, current, &mut e.meta);
    }
    with_image("ready", e)
}

/// The image of `container` at `w` in `look` and `shape` (ignored for a
/// container its renderer doesn't shape; with `ids`, its id image): the
/// route's second answer.
#[allow(clippy::too_many_arguments)]
pub(super) fn image(
    blobs: &Arc<k2_mpc_base::blob::BlobStore>,
    snap: &Arc<Snapshot>,
    container: &str,
    hash: Option<&str>,
    w: Width,
    look: Look,
    shape: Shape,
    ids: bool,
) -> CliResponse {
    let Some(current) = snap.container(container) else {
        return routes::with_exit(
            refusal("404 Not Found", "no_such_container", format!("no {container} in this doc now (list it again)")),
            k2_mpc_base::tools::exit::NOT_FOUND,
        );
    };
    if ids && !snap.renderer.draws_ids(&current.c) {
        return routes::with_exit(
            refusal("404 Not Found", "no_id_image", format!("{container} is drawn without an id image")),
            k2_mpc_base::tools::exit::NOT_FOUND,
        );
    }
    let hash = hash.map(str::trim).filter(|h| !h.is_empty()).unwrap_or(&current.hash);
    let image_build = snap.cache_build(container, look, shape);
    let build = if ids { cache::ids_build(&image_build) } else { image_build };
    match cache::get(blobs, &snap.key, container, hash, w, &build) {
        Got::Hit(e) => return routes::ok(answer(snap, current, e, ids)),
        Got::KeyDestroyed | Got::Miss => {}
    }
    if hash != current.hash {
        return routes::with_exit(
            refusal("404 Not Found", "preview_gone", "that preview isn't cached and the slide has changed since: list it again"),
            k2_mpc_base::tools::exit::NOT_FOUND,
        );
    }
    match service::request(blobs, snap, container, w, look, shape) {
        Requested::Reused { from } => match cache::get(blobs, &snap.key, container, hash, w, &build) {
            Got::Hit(e) => routes::ok(answer(snap, current, e, ids)),
            Got::KeyDestroyed | Got::Miss => {
                k2_core::log_debug!("[docs/preview] the image reused from {from} went before it was read");
                let mut r = routes::ok(with_ask(rendering_body(snap, container, hash, w), snap, container, look, shape));
                r.status = "202 Accepted";
                r
            }
        },
        Requested::Failed(code) => routes::ok(with_ask(json!({
            "kind": snap.renderer.kind(),
            "container": container,
            "render_hash": hash,
            "width": w.as_str(),
            "rev": snap.rev,
            "page": page_json(snap),
            "state": "failed",
            "failed": { "code": code.as_str() },
        }), snap, container, look, shape)),
        Requested::NoContainer => routes::bad(format!("no {container} in this doc")),
        Requested::Rendering | Requested::Busy => {
            let stale = if ids { None } else { cache::newest_other(blobs, &snap.key, container, hash, w, &build) };
            let body = match stale {
                Some(e) => {
                    let mut v = with_image("stale", e);
                    v["current_hash"] = json!(hash);
                    v
                }
                None => with_ask(rendering_body(snap, container, hash, w), snap, container, look, shape),
            };
            let mut r = routes::ok(body);
            r.status = "202 Accepted";
            r
        }
    }
}

fn rendering_body(snap: &Snapshot, container: &str, hash: &str, w: Width) -> Value {
    json!({
        "kind": snap.renderer.kind(),
        "container": container,
        "render_hash": hash,
        "width": w.as_str(),
        "rev": snap.rev,
        "page": page_json(snap),
        "state": "rendering",
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use k2_mpc_base::session::{ConnId, Participant, ParticipantKind, RosterEvent, Session, SessionManager, Stay, TextFilePack};
    use k2_mpc_base::store::{DocKey, DocStore};

    use super::agent_viewing;
    use crate::docs::caller::DocCaller;
    use crate::docs::host::K2Host;
    use crate::test_support::Scratch;

    /// A live doc in workspace `p1` (K2's own host: the agent `agent:p1`
    /// has every right in its own workspace, the owner everywhere).
    fn doc(s: &Scratch) -> (SessionManager, Arc<Session>) {
        let store = Arc::new(DocStore::open(s.path().join("docs.redb")).expect("store"));
        let mgr = SessionManager::new(store, Arc::new(K2Host::default()), k2_mpc_base::session::Config::default());
        let key = DocKey { space: "p1".into(), path: "scene.md".into() };
        let session = mgr.open(Arc::new(TextFilePack::new("markdown", 4 << 20)), &key, Some(b"# scene\n")).expect("open");
        (mgr, session)
    }

    /// `pid`'s view address as a fresh roster snapshot shows it (after a
    /// flush), or `None` when they aren't on the roster.
    fn view_of(mgr: &SessionManager, s: &Session, observer: ConnId, pid: &str) -> Option<Option<String>> {
        mgr.flush_rosters();
        let rx = s.subscribe_roster(observer).expect("roster");
        match rx.try_recv().expect("a snapshot first") {
            RosterEvent::Snapshot { members, .. } => {
                members.iter().find(|m| m.entry.participant.pid == pid).map(|m| m.entry.view_address.clone())
            }
            other => panic!("not a snapshot: {other:?}"),
        }
    }

    /// An agent's still sets its view address (Follow can follow it); a
    /// person's never moves; an address presence can't take clears it
    /// rather than leaving the last one; and it goes when the agent leaves.
    #[test]
    fn an_agents_still_is_where_it_is_looking_until_it_leaves() {
        let s = Scratch::new();
        let (mgr, session) = doc(&s);
        let owner = DocCaller::Person { key: "owner".into(), via: crate::docs::caller::VIA_CLI };
        let agent = DocCaller::Agent { project_id: "p1".into(), session: None };
        // Participants as K2 stamps them (pid, kind, via), named here so the
        // test reads no people list or workspace record.
        let person = Participant::new("person:owner", "Ada", "#2f9e8f", ParticipantKind::Person, "tab");
        let bot = || Participant::new("agent:p1", "Atlas", "#6d5bd0", ParticipantKind::Agent, crate::docs::caller::VIA_CLI);
        let me = session.join(person, Stay::Held).expect("the owner's tab").conn;
        // The route's join: the agent's shared connection, as every k2 doc verb.
        let a = session.join_shared(bot(), Stay::UntilIdle).expect("the agent").conn;
        assert_eq!(view_of(&mgr, &session, me, "agent:p1"), Some(None), "no view before a still");

        agent_viewing(&session, a, &agent, "@cam:c1");
        assert_eq!(view_of(&mgr, &session, me, "agent:p1"), Some(Some("@cam:c1".into())));
        agent_viewing(&session, a, &agent, "@turntable:3");
        assert_eq!(view_of(&mgr, &session, me, "agent:p1"), Some(Some("@turntable:3".into())), "the latest still");
        // The same connection as the agent's next verb (k2 doc), not a new one.
        let again = session.join_shared(bot(), Stay::UntilIdle).expect("again").conn;
        assert_eq!(again, a, "one shared connection a participant");

        // A person's still (the app fetching for its tab) never moves their view.
        agent_viewing(&session, me, &owner, "@cam:c2");
        assert_eq!(view_of(&mgr, &session, me, "person:owner"), Some(None), "a person's view is their tab's");

        // `@scene` isn't an address the kit's presence takes: cleared, never stale.
        agent_viewing(&session, a, &agent, "@scene");
        assert_eq!(view_of(&mgr, &session, me, "agent:p1"), Some(None), "not left on @turntable:3");
        agent_viewing(&session, a, &agent, "@obj:Lamp");
        assert_eq!(view_of(&mgr, &session, me, "agent:p1"), Some(Some("@obj:Lamp".into())));

        // The agent leaves (k2 doc leave, or its connection idles out): its view goes with it.
        session.leave(a).expect("leave");
        assert_eq!(view_of(&mgr, &session, me, "agent:p1"), None, "gone with its connection");
    }
}
