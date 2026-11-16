//! The preview service: which images are current, what to draw, drawing it
//! within the host's limits, and telling the doc's sockets.
//!
//! - **Snapshots.** A doc's bytes at a rev (`file_now`) or a cut version
//!   (`file_at`) are listed by the type's renderer, and each container's raw
//!   digest is keyed: `render_hash` = 32 hex of HMAC-SHA256 under the space's
//!   host cache key for [`HASH_LABEL`] (contract Item 1: the raw digest
//!   never leaves the host). One snapshot per doc head and per version is
//!   memoised ([`MEMO`] docs at most), so a listing at an unchanged rev
//!   costs nothing.
//! - **Only dirty slides.** When a doc's head moves, the containers whose
//!   hash changed are re-drawn at the widths someone already looked at
//!   (their cached widths); untouched containers keep their images. Debounced
//!   per doc ([`DEBOUNCE`] quiet, [`DEBOUNCE_MAX`] at most): one listing per
//!   burst of changes serves every dirty container at that rev.
//! - **Slots and the host cap.** [`SLOTS`] renders at once, one worker
//!   process each (no pool), [`QUEUE`] waiting; past them a request stays
//!   `rendering` until asked again. The renderer's own wall ([`WALL`]) is
//!   enforced in its worker; the host also gives each render [`HOST_CAP`]
//!   and reports `timeout` past it, whatever the worker does.
//! - **The push** ([`super::super::host_events`]), on the doc's sockets: a
//!   render starting is `preview_rendering`; one landing is `preview_ready`
//!   with `state: "ready"`, its `render_ms` and `without_modifiers` (and a
//!   scene view's `camera`, `base_values`, `hidden_left_out` and id table,
//!   `ids`, when there); one failing is
//!   `preview_ready` with `state: "failed"` and its code. An older client
//!   that knows only `preview_ready` reads it as it always did.
//! - **Id images** ([`super::extras`]): an image that comes with one has it
//!   cached first, as its own entry beside the image ([`cache::ids_build`]),
//!   so a cached image always had its id image written with it.
//! - **Name-only changes** ([`super::reuse`]): a container whose render
//!   hash the doc already has drawn under another address (a renamed object
//!   or camera: a scene's digest leaves ids out) is answered from that
//!   image at once, `preview_ready` with `reused_from`, no worker run; its
//!   id image comes along, its table named as the listing is now
//!   ([`with_current_ids`]). The scene and camera views keep their address
//!   and hash when an object is renamed: their current images are pushed
//!   `ready` again with the new ids ([`push_renamed_ids`]), nothing drawn,
//!   and a draw that lands after the rename pushes its table named as the
//!   head lists the objects then.
//! - **Looks** ([`super::Look`]): a job is per look as well as per width;
//!   a transparent image is cached under its own name.
//! - **Shapes** ([`super::Shape`]): for a view its renderer shapes (a
//!   scene's auto view and an object's), a job is per shape too, a tall
//!   image cached under its own name, and its metadata and pushes name the
//!   shape (`shape`, wide included). Every other view is drawn once in its
//!   own aspect: the shape asked is ignored for it and it is keyed as wide
//!   ([`Snapshot::drawn_shape`]), with no `shape` in what it answers.
//! - **Purge.** [`doc_purged`], [`space_removed`], [`doc_moved`] and
//!   [`upkeep`] are the hooks' service side (memo, failures, audit), with
//!   [`super::cache`] doing the files.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use hmac::{Hmac, Mac};
use sha2::Sha256;

use k2_core::log_debug;
use k2_mpc_base::blob::BlobStore;
use k2_mpc_base::session::{ConnId, Session};
use k2_mpc_base::store::DocKey;

use super::{cache, extras, reuse, Container, FailCode, Failed, Look, NoSideFiles, Page, PreviewRenderer, Shape, Source, Width};
use crate::docs::host;
use crate::docs::host_events::{self, HostEvent};

/// The host cache key's purpose for render hashes.
pub const HASH_LABEL: &[u8] = b"k2/doc-previews/render-hash/v1";
/// Renders at once.
pub const SLOTS: usize = 2;
/// Renders waiting for a slot.
pub const QUEUE: usize = 256;
/// The renderer's own wall per render (its worker enforces it).
pub const WALL: Duration = Duration::from_secs(20);
/// The host's own cap per render, past the worker's wall.
pub const HOST_CAP: Duration = Duration::from_secs(30);
/// Snapshots kept.
pub const MEMO: usize = 32;
/// A doc's changes are listed once they have been quiet this long...
pub const DEBOUNCE: Duration = Duration::from_millis(300);
/// ...or this long after the first, whichever comes first.
pub const DEBOUNCE_MAX: Duration = Duration::from_secs(2);
/// The Access Audit event for dropped previews (counts only).
pub const AUDIT_DROPPED: &str = "doc-previews-dropped";

/// One container of a snapshot, with its keyed hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keyed {
    pub c: Container,
    pub hash: String,
}

/// A doc's previewable state at one rev (or one cut version).
pub struct Snapshot {
    pub key: DocKey,
    /// The doc version the bytes are.
    pub rev: u64,
    /// `Some` for a cut version's snapshot.
    pub version: Option<u64>,
    pub renderer: &'static dyn PreviewRenderer,
    pub build: String,
    pub page: Option<Page>,
    pub containers: Vec<Keyed>,
    pub bytes: Arc<Vec<u8>>,
}

impl Snapshot {
    pub fn container(&self, address: &str) -> Option<&Keyed> {
        self.containers.iter().find(|k| k.c.key == address)
    }

    /// The shape `container` is drawn and keyed in when `asked` is: `asked`
    /// for a view the renderer shapes, else wide ([`super::drawn_shape`]);
    /// wide for an address not listed.
    pub fn drawn_shape(&self, container: &str, asked: Shape) -> Shape {
        self.container(container).map_or(Shape::Wide, |k| super::drawn_shape(self.renderer, &k.c, asked))
    }

    /// The build `container`'s image in `look` is cached under when `asked`
    /// is the shape asked for ([`super::cache_build`] in its drawn shape).
    pub fn cache_build(&self, container: &str, look: Look, asked: Shape) -> String {
        super::cache_build(&self.build, look, self.drawn_shape(container, asked))
    }

    /// `shape` as `container`'s answers and pushes name it: only for a view
    /// the renderer shapes.
    pub fn shape_field(&self, container: &str, shape: Shape) -> Option<&'static str> {
        self.container(container).and_then(|k| super::shape_field(self.renderer, &k.c, shape))
    }

    /// The shapes `c` may be cached in: both for a view the renderer
    /// shapes, else wide alone.
    pub fn shapes_of(&self, c: &Container) -> &'static [Shape] {
        if self.renderer.shapes(c) {
            &Shape::ALL
        } else {
            &[Shape::Wide]
        }
    }
}

/// Why there is no snapshot.
#[derive(Debug)]
pub enum SnapError {
    /// The kit refused the read (rights, a version without its carrier...).
    Session(k2_mpc_base::session::SessionError),
    /// The renderer couldn't list the bytes.
    Failed(Failed),
    /// No blob store (any-file docs off): no host cache key, so no previews.
    NoKey(String),
}

fn memo() -> &'static Mutex<HashMap<(DocKey, Option<u64>), Arc<Snapshot>>> {
    static M: OnceLock<Mutex<HashMap<(DocKey, Option<u64>), Arc<Snapshot>>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 32 hex of HMAC-SHA256(key, digest).
pub fn keyed_hash(secret: &[u8; 32], digest: &[u8; 32]) -> String {
    let mut m = <Hmac<Sha256> as Mac>::new_from_slice(secret).expect("HMAC takes any key length");
    m.update(digest);
    m.finalize().into_bytes().iter().take(16).map(|b| format!("{b:02x}")).collect()
}

/// List `bytes` and key every digest: a fresh snapshot.
pub fn list(
    blobs: &BlobStore,
    renderer: &'static dyn PreviewRenderer,
    key: &DocKey,
    rev: u64,
    version: Option<u64>,
    bytes: Vec<u8>,
) -> Result<Snapshot, SnapError> {
    let listing = renderer.list(&bytes).map_err(SnapError::Failed)?;
    let k = blobs.host_cache_key(&key.space, HASH_LABEL).map_err(|e| SnapError::NoKey(e.to_string()))?;
    let containers =
        listing.containers.into_iter().map(|c| Keyed { hash: keyed_hash(k.secret_bytes(), &c.digest), c }).collect();
    Ok(Snapshot {
        key: key.clone(),
        rev,
        version,
        renderer,
        build: renderer.build(),
        page: listing.page,
        containers,
        bytes: Arc::new(bytes),
    })
}

/// The doc's head snapshot, read through `conn` (needs read). At an
/// unchanged rev the memo answers without reading the file. When the head
/// moved, its dirty containers are queued at the widths already looked at.
pub fn head(
    blobs: &Arc<BlobStore>,
    renderer: &'static dyn PreviewRenderer,
    key: &DocKey,
    session: &Session,
    conn: ConnId,
) -> Result<Arc<Snapshot>, SnapError> {
    let now = session.status().map_err(SnapError::Session)?.version;
    let prev = head_now(key);
    if let Some(p) = prev.as_ref().filter(|p| p.rev == now && p.build == renderer.build()) {
        return Ok(p.clone());
    }
    let file = session.file_now(conn).map_err(SnapError::Session)?;
    let snap = Arc::new(list(blobs, renderer, key, file.version, None, file.bytes)?);
    match prev {
        Some(prev) => head_moved(blobs, &prev, &snap),
        None => remember(snap.clone()),
    }
    Ok(snap)
}

/// The doc's head snapshot as last listed, when it is memoised.
fn head_now(key: &DocKey) -> Option<Arc<Snapshot>> {
    memo().lock().unwrap_or_else(|p| p.into_inner()).get(&(key.clone(), None)).cloned()
}

/// What a doc's head moving from `prev` to `snap` sets off: `snap`
/// remembered as the head first (a draw landing from here on names its id
/// table from it), its dirty containers drawn again at the widths already
/// looked at (or reused, [`reuse`]), then the scene and camera views' id
/// tables pushed with the objects' new ids ([`push_renamed_ids`]).
pub fn head_moved(blobs: &Arc<BlobStore>, prev: &Snapshot, snap: &Arc<Snapshot>) {
    remember(snap.clone());
    for k in dirty(prev, snap) {
        for look in Look::ALL.into_iter().filter(|l| snap.renderer.draws(*l)) {
            for &shape in snap.shapes_of(&k.c) {
                for w in reuse::widths_to_redraw(prev, k, &super::cache_build(&snap.build, look, shape)) {
                    request(blobs, snap, &k.c.key, w, look, shape);
                }
            }
        }
    }
    push_renamed_ids(blobs, prev, snap);
}

/// Objects renamed in place. A scene's digest leaves ids out, so the views
/// that draw every object (the scene and the cameras) keep their address
/// and render hash, and so does a renamed object's own view when the read
/// keys it (its address is its live key): nothing is drawn, and a client
/// holding one of their id tables would keep the old ids. Each of their images cached at the view's
/// current hash whose table names an object differently from `prev`'s
/// listing is pushed `ready` again, at once: the same container, render
/// hash, width, look, shape and fields (its `rendered_at_ms` is when it was
/// drawn), at `snap`'s rev, its `ids` named as the listing is now. The same
/// render hash says the pixels are the ones the client has. The cache keeps
/// the ids it was drawn with: the listing names a current image wherever it
/// is answered ([`with_current_ids`]).
fn push_renamed_ids(blobs: &BlobStore, prev: &Snapshot, snap: &Snapshot) {
    if extras::object_ids(prev.containers.iter().map(|k| &k.c)) == extras::object_ids(snap.containers.iter().map(|k| &k.c)) {
        return;
    }
    for k in &snap.containers {
        let Some(p) = prev.container(&k.c.key).filter(|p| p.hash == k.hash) else { continue };
        if !snap.renderer.draws_ids(&k.c) {
            continue;
        }
        for (look, &shape) in Look::ALL.into_iter().filter(|l| snap.renderer.draws(*l)).flat_map(|l| snap.shapes_of(&k.c).iter().map(move |s| (l, s))) {
            let build = super::cache_build(&snap.build, look, shape);
            for w in Width::ALL {
                let cache::Got::Hit(e) = cache::get(blobs, &snap.key, &k.c.key, &k.hash, w, &build) else { continue };
                let (mut was, mut now) = (e.meta.clone(), e.meta);
                with_current_ids(prev, p, &mut was);
                with_current_ids(snap, k, &mut now);
                if now.get("ids").is_none() || was.get("ids") == now.get("ids") {
                    continue;
                }
                host_events::publish(HostEvent::PreviewReady {
                    key: snap.key.clone(),
                    container: k.c.key.clone(),
                    render_hash: k.hash.clone(),
                    width: w.as_str(),
                    state: "ready",
                    failed: None,
                    rev: snap.rev,
                    rendered_at_ms: now["rendered_at_ms"].as_u64().unwrap_or(0),
                    extras: ready_fields(&now),
                });
            }
        }
    }
}

/// A cut version's snapshot, through `conn` (the kit checks history).
pub fn at_version(
    blobs: &Arc<BlobStore>,
    renderer: &'static dyn PreviewRenderer,
    key: &DocKey,
    session: &Session,
    conn: ConnId,
    version: u64,
) -> Result<Arc<Snapshot>, SnapError> {
    if let Some(s) = memo().lock().unwrap_or_else(|p| p.into_inner()).get(&(key.clone(), Some(version))).cloned() {
        // The version's bytes never change; the kit still re-checks the right.
        session.file_at(conn, version).map_err(SnapError::Session)?;
        return Ok(s);
    }
    let file = session.file_at(conn, version).map_err(SnapError::Session)?;
    let snap = Arc::new(list(blobs, renderer, key, file.version, Some(version), file.bytes)?);
    remember(snap.clone());
    Ok(snap)
}

fn remember(snap: Arc<Snapshot>) {
    let mut m = memo().lock().unwrap_or_else(|p| p.into_inner());
    if m.len() >= MEMO {
        // Drop cut versions first, then anything: a snapshot is cheap to make again.
        let victim = m.keys().filter(|e| evictable(e)).find(|(_, v)| v.is_some()).or_else(|| m.keys().filter(|e| evictable(e)).next()).cloned();
        if let Some(v) = victim {
            m.remove(&v);
        }
    }
    m.insert((snap.key.clone(), snap.version), snap);
}

/// Whether the memo may drop `entry` to make room: any entry.
#[cfg(not(test))]
fn evictable(_entry: &(DocKey, Option<u64>)) -> bool {
    true
}

/// In tests, any entry but a head a test pinned ([`pin_head`]): the memo
/// is one per test process, so another test's listings could drop it.
#[cfg(test)]
fn evictable(entry: &(DocKey, Option<u64>)) -> bool {
    entry.1.is_some() || !pinned().lock().unwrap_or_else(|p| p.into_inner()).contains(&entry.0)
}

#[cfg(test)]
fn pinned() -> &'static Mutex<HashSet<DocKey>> {
    static P: OnceLock<Mutex<HashSet<DocKey>>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Keeps a doc's head snapshot in the memo while it lives (tests only).
#[cfg(test)]
pub struct HeadPin(DocKey);

/// Pin `key`'s head snapshot in the memo until the pin drops, so a test
/// that relies on it isn't at the mercy of what else the process lists.
/// Purging the doc still drops it.
#[cfg(test)]
pub fn pin_head(key: &DocKey) -> HeadPin {
    pinned().lock().unwrap_or_else(|p| p.into_inner()).insert(key.clone());
    HeadPin(key.clone())
}

#[cfg(test)]
impl Drop for HeadPin {
    fn drop(&mut self) {
        pinned().lock().unwrap_or_else(|p| p.into_inner()).remove(&self.0);
    }
}

/// The containers of `next` whose hash isn't `prev`'s for them (new ones included).
pub fn dirty<'a>(prev: &Snapshot, next: &'a Snapshot) -> Vec<&'a Keyed> {
    next.containers.iter().filter(|k| prev.container(&k.c.key).is_none_or(|p| p.hash != k.hash)).collect()
}

// ── The queue ────────────────────────────────────────────────────────

type JobId = (DocKey, String, String, Width, Look, Shape);

struct Job {
    id: JobId,
    snap: Arc<Snapshot>,
    container: Container,
    blobs: Arc<BlobStore>,
}

#[derive(Default)]
struct Queue {
    waiting: VecDeque<Job>,
    running: HashSet<JobId>,
    failed: HashMap<JobId, FailCode>,
    workers: usize,
}

fn queue() -> &'static (Mutex<Queue>, Condvar) {
    static Q: OnceLock<(Mutex<Queue>, Condvar)> = OnceLock::new();
    Q.get_or_init(|| (Mutex::new(Queue::default()), Condvar::new()))
}

/// What a request found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Requested {
    /// Queued now, or already waiting or drawing.
    Rendering,
    /// Answered from the image another container of the doc drew at the
    /// same render hash (`from`, its address): cached under this one's key
    /// now, `preview_ready` pushed, no worker run.
    Reused { from: String },
    /// The last render of exactly this failed (asked again: it isn't retried
    /// until the doc changes or the daemon restarts); or, with
    /// [`FailCode::NotOnThisHost`], this host can't draw it at all
    /// ([`PreviewRenderer::decodes_here`]): never queued or kept.
    Failed(FailCode),
    /// The queue is full: ask again later.
    Busy,
    /// No such container in the snapshot.
    NoContainer,
}

/// Queue a render of `container` at `width` in `look` and `shape` (for a
/// view its renderer shapes; ignored for any other, which is one job
/// whatever shape is asked) from `snap`, unless the doc already has its
/// image under another address ([`reuse`]).
pub fn request(blobs: &Arc<BlobStore>, snap: &Arc<Snapshot>, container: &str, width: Width, look: Look, shape: Shape) -> Requested {
    let Some(k) = snap.container(container) else { return Requested::NoContainer };
    // A host that can't draw it at all (no decoder for it here) says so at
    // once: nothing queued, nothing pushed, no failure kept.
    if !snap.renderer.decodes_here(&k.c) {
        return Requested::Failed(FailCode::NotOnThisHost);
    }
    let shape = snap.drawn_shape(container, shape);
    let id: JobId = (snap.key.clone(), k.c.key.clone(), k.hash.clone(), width, look, shape);
    {
        let q = queue().0.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(code) = q.failed.get(&id) {
            return Requested::Failed(*code);
        }
        if q.running.contains(&id) || q.waiting.iter().any(|j| j.id == id) {
            return Requested::Rendering;
        }
    }
    if let Some(from) = reuse::try_reuse(blobs, snap, k, width, look, shape) {
        return Requested::Reused { from };
    }
    let (lock, cv) = queue();
    let mut q = lock.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(code) = q.failed.get(&id) {
        return Requested::Failed(*code);
    }
    if q.running.contains(&id) || q.waiting.iter().any(|j| j.id == id) {
        return Requested::Rendering;
    }
    if q.waiting.len() >= QUEUE {
        return Requested::Busy;
    }
    q.waiting.push_back(Job { id, snap: snap.clone(), container: k.c.clone(), blobs: blobs.clone() });
    if q.workers < SLOTS {
        q.workers += 1;
        let spawned = std::thread::Builder::new().name("k2-docs-preview".into()).spawn(worker);
        if let Err(e) = spawned {
            q.workers -= 1;
            log_debug!("[docs/preview] WARN: no render thread: {e}");
        }
    }
    cv.notify_one();
    Requested::Rendering
}

/// Whether a render of exactly this is waiting or drawing (`shape` the one
/// it is drawn in, [`Snapshot::drawn_shape`]).
pub fn rendering(key: &DocKey, container: &str, hash: &str, width: Width, look: Look, shape: Shape) -> bool {
    let id: JobId = (key.clone(), container.to_string(), hash.to_string(), width, look, shape);
    let q = queue().0.lock().unwrap_or_else(|p| p.into_inner());
    q.running.contains(&id) || q.waiting.iter().any(|j| j.id == id)
}

/// A worker thread: draws until the queue has been empty for a minute.
fn worker() {
    let (lock, cv) = queue();
    loop {
        let job = {
            let mut q = lock.lock().unwrap_or_else(|p| p.into_inner());
            let deadline = Instant::now() + Duration::from_secs(60);
            loop {
                if let Some(j) = q.waiting.pop_front() {
                    q.running.insert(j.id.clone());
                    break Some(j);
                }
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    q.workers -= 1;
                    break None;
                }
                q = cv.wait_timeout(q, left).unwrap_or_else(|p| p.into_inner()).0;
            }
        };
        let Some(job) = job else { return };
        let outcome = draw(&job);
        let mut q = lock.lock().unwrap_or_else(|p| p.into_inner());
        q.running.remove(&job.id);
        match outcome {
            Ok(()) => {
                q.failed.remove(&job.id);
            }
            Err(code) => {
                if q.failed.len() >= 4096 {
                    q.failed.clear();
                }
                q.failed.insert(job.id.clone(), code);
            }
        }
    }
}

/// Draw one job under the host cap, cache it, and push `preview_rendering`
/// before and `preview_ready` after.
fn draw(job: &Job) -> Result<(), FailCode> {
    let (key, _, hash, width, look, shape) = &job.id;
    let snap = job.snap.clone();
    let container = job.container.clone();
    let (w, look, shape) = (*width, *look, *shape);
    let shape_named = super::shape_field(job.snap.renderer, &job.container, shape);
    host_events::publish(HostEvent::PreviewRendering {
        key: key.clone(),
        container: job.container.key.clone(),
        render_hash: hash.clone(),
        width: w.as_str(),
        look: look_field(look),
        shape: shape_named,
        rev: job.snap.rev,
    });
    let (tx, rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new().name("k2-docs-preview-render".into()).spawn(move || {
        let src = Source { doc: &snap.bytes, side: &NoSideFiles };
        let _ = tx.send(snap.renderer.render(&src, &container, w, look, shape, WALL));
    });
    let result = match spawned {
        Ok(_) => match rx.recv_timeout(HOST_CAP) {
            Ok(r) => r,
            Err(_) => Err(Failed::new(FailCode::Timeout, format!("no answer within the host's {} s", HOST_CAP.as_secs()))),
        },
        Err(e) => Err(Failed::new(FailCode::RendererError, format!("no render thread: {e}"))),
    };
    let rendered_at_ms = host::now_ms();
    let mut ready = Vec::new();
    let outcome = match result {
        Ok(img) => match store(&job.blobs, &job.snap, &job.container.key, hash, w, look, shape, &img, rendered_at_ms) {
            Ok(mut meta) => {
                // An object renamed while this was drawn keeps the view's
                // hash: the push names the table as the head lists it now
                // (the cache keeps the drawn names, as for every image).
                if let Some(head) = head_now(key) {
                    if let Some(k) = head.container(&job.container.key) {
                        with_current_ids(&head, k, &mut meta);
                    }
                }
                ready = ready_fields(&meta);
                Ok(())
            }
            Err(e) => {
                log_debug!("[docs/preview] WARN: caching a preview of {}: {e}", host::key_label(key));
                Err(FailCode::RendererError)
            }
        },
        Err(f) => {
            log_debug!("[docs/preview] {} {} at {}: {} ({})", host::key_label(key), job.container.key, w.as_str(), f.code.as_str(), f.detail);
            Err(f.code)
        }
    };
    host_events::publish(HostEvent::PreviewReady {
        key: key.clone(),
        container: job.container.key.clone(),
        render_hash: hash.clone(),
        width: w.as_str(),
        state: if outcome.is_ok() { "ready" } else { "failed" },
        failed: outcome.err().map(FailCode::as_str),
        rev: job.snap.rev,
        rendered_at_ms,
        extras: if outcome.is_ok() { ready } else { ask_fields(look, shape_named) },
    });
    outcome
}

/// Cache an image drawn of `container` at `width` in `look` and `shape`
/// (as [`request`] takes it) from `snap`, its id image first (beside it,
/// [`cache::ids_build`]), so a cached image always has its id image
/// written. Returns the image's metadata.
#[allow(clippy::too_many_arguments)]
pub fn store(blobs: &BlobStore, snap: &Snapshot, container: &str, hash: &str, width: Width, look: Look, shape: Shape, img: &super::Image, rendered_at_ms: u64) -> Result<serde_json::Value, String> {
    let shape = snap.drawn_shape(container, shape);
    let meta = meta_json(snap, container, hash, width, look, shape, img, rendered_at_ms);
    let build = super::cache_build(&snap.build, look, shape);
    if let Some(ids) = &img.ids {
        cache::put(blobs, &snap.key, container, hash, width, &cache::ids_build(&build), &ids_entry_meta(&meta), ids)?;
    }
    cache::put(blobs, &snap.key, container, hash, width, &build, &meta, &img.bytes)?;
    Ok(meta)
}

/// A look as the frames and the metadata name it: left out for opaque (what
/// every older answer was), `"transparent"` otherwise.
pub fn look_field(look: Look) -> Option<&'static str> {
    (look != Look::Opaque).then_some(look.as_str())
}

/// What a failed push names of what was asked: `look` (left out for
/// opaque), then `shape` (for a view its renderer shapes).
fn ask_fields(look: Look, shape: Option<&'static str>) -> Vec<(String, k2_mpc_base::errors::Value)> {
    let mut f = Vec::new();
    if let Some(l) = look_field(look) {
        f.push(("look".to_string(), k2_mpc_base::errors::Value::from(l)));
    }
    if let Some(s) = shape {
        f.push(("shape".to_string(), k2_mpc_base::errors::Value::from(s)));
    }
    f
}

/// The fields a `ready` push carries after the old ones, from the image's
/// metadata: `render_ms`, `without_modifiers`, then what is there of
/// `look`, `shape`, `reused_from`, `camera`, `base_values`, `hidden_left_out`
/// and `ids` (its objects listed only up to [`extras::PUSH_IDS_MAX`]).
pub fn ready_fields(meta: &serde_json::Value) -> Vec<(String, k2_mpc_base::errors::Value)> {
    use k2_mpc_base::errors::Value;
    let mut f = vec![
        ("render_ms".to_string(), Value::from(meta["render_ms"].as_u64().unwrap_or(0))),
        ("without_modifiers".to_string(), Value::from(meta["without_modifiers"].as_bool().unwrap_or(false))),
    ];
    for k in ["look", "shape", "reused_from", "camera", "base_values", "hidden_left_out"] {
        if let Some(v) = meta.get(k) {
            f.push((k.to_string(), extras::to_wire(v)));
        }
    }
    if let Some(v) = meta.get("ids") {
        f.push(("ids".to_string(), extras::to_wire(&extras::ids_for_push(v))));
    }
    f
}

/// The metadata of an image's id image entry, from the image's: what names
/// it (kind, container, render hash, width, look, shape, rev, size, when it was
/// drawn, `reused_from`), `layer: "ids"`, and its format. Its table stays in
/// the image's metadata only.
pub fn ids_entry_meta(meta: &serde_json::Value) -> serde_json::Value {
    let mut v = serde_json::Map::new();
    for k in ["kind", "container", "render_hash", "width", "look", "shape", "rev", "size", "render_ms", "rendered_at_ms", "reused_from"] {
        if let Some(x) = meta.get(k) {
            v.insert(k.to_string(), x.clone());
        }
    }
    v.insert("layer".into(), serde_json::json!(extras::IDS_LAYER));
    v.insert("format".into(), serde_json::json!("png"));
    v.insert("mime".into(), serde_json::json!("image/png"));
    serde_json::Value::Object(v)
}

/// `meta` (an image of `k`'s) with its id table named as `snap` lists the
/// objects now, when it is `k`'s current image: a rename changes no render
/// hash, so a cached image may carry ids from before it. An image at
/// another hash keeps the ids it was drawn with.
pub fn with_current_ids(snap: &Snapshot, k: &Keyed, meta: &mut serde_json::Value) {
    if meta.get("render_hash").and_then(serde_json::Value::as_str) != Some(k.hash.as_str()) {
        return;
    }
    if let Some(ids) = meta.get_mut("ids") {
        extras::current_ids(ids, &k.c, snap.containers.iter().map(|x| &x.c));
    }
}

/// The metadata kept with an image and answered with it (contract Item 8's
/// `Preview` table), `shape` the one it was drawn in.
#[allow(clippy::too_many_arguments)]
pub fn meta_json(snap: &Snapshot, container: &str, hash: &str, width: Width, look: Look, shape: Shape, img: &super::Image, rendered_at_ms: u64) -> serde_json::Value {
    let mut v = serde_json::json!({
        "kind": snap.renderer.kind(),
        "container": container,
        "render_hash": hash,
        "page": snap.page.map(|p| serde_json::json!({ "w": p.w, "h": p.h, "unit": p.unit })),
        "width": width.as_str(),
        "size": { "w": img.width, "h": img.height },
        "format": img.format,
        "mime": img.mime,
        "rev": snap.rev,
        "fonts_substituted": img.fonts_substituted.iter().map(|(a, d)| serde_json::json!({ "asked": a, "drawn": d })).collect::<Vec<_>>(),
        "render_ms": img.render_ms,
        "rendered_at_ms": rendered_at_ms,
    });
    // A scene drawn from base meshes says so (the view shows "shown without modifiers").
    if img.without_modifiers {
        v["without_modifiers"] = serde_json::json!(true);
    }
    if let Some(l) = look_field(look) {
        v["look"] = serde_json::json!(l);
    }
    if let Some(s) = snap.shape_field(container, shape) {
        v["shape"] = serde_json::json!(s);
    }
    for (k, x) in &img.extras {
        v[k.as_str()] = x.clone();
    }
    v
}

// ── Debounced re-listing on change (a socket watching the doc) ───────

struct Pending {
    session: Arc<Session>,
    conn: ConnId,
    blobs: Arc<BlobStore>,
    renderer: &'static dyn PreviewRenderer,
    first: Instant,
    last: Instant,
}

fn pending() -> &'static Mutex<HashMap<DocKey, Pending>> {
    static P: OnceLock<Mutex<HashMap<DocKey, Pending>>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A watched doc changed (a docs socket forwarded a change): list it again
/// once the burst is over, through that socket's connection (it has read),
/// and queue its dirty containers.
pub fn doc_changed(blobs: Arc<BlobStore>, renderer: &'static dyn PreviewRenderer, key: &DocKey, session: Arc<Session>, conn: ConnId) {
    let now = Instant::now();
    let mut p = pending().lock().unwrap_or_else(|p| p.into_inner());
    let start = p.is_empty();
    p.entry(key.clone())
        .and_modify(|e| {
            e.last = now;
            e.session = session.clone();
            e.conn = conn;
        })
        .or_insert(Pending { session, conn, blobs, renderer, first: now, last: now });
    if start {
        let _ = std::thread::Builder::new().name("k2-docs-preview-debounce".into()).spawn(debounce);
    }
}

fn debounce() {
    loop {
        std::thread::sleep(Duration::from_millis(100));
        let due: Vec<(DocKey, Pending)> = {
            let mut p = pending().lock().unwrap_or_else(|p| p.into_inner());
            let now = Instant::now();
            let keys: Vec<DocKey> = p
                .iter()
                .filter(|(_, e)| now.duration_since(e.last) >= DEBOUNCE || now.duration_since(e.first) >= DEBOUNCE_MAX)
                .map(|(k, _)| k.clone())
                .collect();
            let due = keys.into_iter().filter_map(|k| p.remove(&k).map(|e| (k, e))).collect();
            if p.is_empty() {
                // This thread ends; the next change starts another. Run what is due first.
                drop(p);
                run_due(due);
                return;
            }
            due
        };
        run_due(due);
    }
}

fn run_due(due: Vec<(DocKey, Pending)>) {
    for (k, e) in due {
        if let Err(err) = head(&e.blobs, e.renderer, &k, &e.session, e.conn) {
            log_debug!("[docs/preview] {} not listed after a change: {err:?}", host::key_label(&k));
        }
    }
}

// ── Purge hooks (service side) ───────────────────────────────────────

/// Audit a drop, counts only, never content.
pub fn audit_drop(reason: &str, space: &str, key: Option<&DocKey>, entries: usize) {
    if entries == 0 {
        return;
    }
    let doc = key.map(host::key_label).unwrap_or_else(|| "-".into());
    host::audit(AUDIT_DROPPED, "k2", format!("space={space} doc={doc} entries={entries} reason={reason}"));
}

fn forget(pred: impl Fn(&DocKey) -> bool) {
    memo().lock().unwrap_or_else(|p| p.into_inner()).retain(|(k, _), _| !pred(k));
    let mut q = queue().0.lock().unwrap_or_else(|p| p.into_inner());
    q.waiting.retain(|j| !pred(&j.id.0));
    q.failed.retain(|id, _| !pred(&id.0));
    pending().lock().unwrap_or_else(|p| p.into_inner()).retain(|k, _| !pred(k));
}

/// A doc's content was purged or erased (any scope: purged text is in the
/// pixels), or the doc was deleted: its previews go. Returns the count.
pub fn doc_purged(key: &DocKey, reason: &str) -> usize {
    forget(|k| k == key);
    let n = cache::drop_doc(key);
    audit_drop(reason, &key.space, Some(key), n);
    n
}

/// A removed workspace: its whole preview folder, before its space row
/// goes. False when the folder is still there (the sweep retries).
pub fn space_removed(space: &str) -> bool {
    forget(|k| k.space == space);
    match cache::drop_space(space) {
        Ok(n) => {
            audit_drop("workspace_removed", space, None, n);
            true
        }
        Err(e) => {
            log_debug!("[docs/preview] WARN: {e}");
            false
        }
    }
}

/// A rename: the previews follow the doc (a renamed doc keeps its hashes).
pub fn doc_moved(old: &DocKey, new: &DocKey) {
    forget(|k| k == old);
    cache::move_doc(old, new);
}

/// The cap, on the docs upkeep.
pub fn upkeep() {
    let (n, bytes) = cache::prune(cache::cap_bytes());
    if n > 0 {
        host::audit(AUDIT_DROPPED, "k2", format!("entries={n} bytes={bytes} reason=cache_cap"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_keyed_hash_depends_on_the_key_and_hides_the_digest() {
        let d = [7u8; 32];
        let a = keyed_hash(&[1u8; 32], &d);
        assert_eq!(a.len(), 32);
        assert_eq!(a, keyed_hash(&[1u8; 32], &d));
        assert_ne!(a, keyed_hash(&[2u8; 32], &d), "another space's key, another hash");
        assert_ne!(a, keyed_hash(&[1u8; 32], &[8u8; 32]));
        assert!(!a.contains(&"07".repeat(4)), "not the raw digest");
    }

    #[test]
    fn dirty_is_the_containers_whose_hash_moved_or_that_are_new() {
        let c = |key: &str, hash: &str| Keyed {
            c: Container { key: key.into(), position: 1, title: String::new(), hidden: false, digest: [0; 32], locator: "1".into(), extras: Default::default() },
            hash: hash.into(),
        };
        let snap = |cs: Vec<Keyed>| Snapshot {
            key: DocKey { space: "s".into(), path: "d.pptx".into() },
            rev: 1,
            version: None,
            renderer: &super::super::deck::DECK,
            build: "b".into(),
            page: None,
            containers: cs,
            bytes: Arc::new(Vec::new()),
        };
        let a = snap(vec![c("@sld:1", "x"), c("@sld:2", "y"), c("@sld:3", "z")]);
        let b = snap(vec![c("@sld:1", "x"), c("@sld:2", "Y"), c("@sld:4", "w")]);
        let d: Vec<&str> = dirty(&a, &b).iter().map(|k| k.c.key.as_str()).collect();
        assert_eq!(d, ["@sld:2", "@sld:4"]);
        assert!(dirty(&a, &a).is_empty());
    }
}
