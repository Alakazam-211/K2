//! Live docs: Markdown multiplayer v1 on the vendored multiplayer kit
//! (`vendor/k2-mpc-base`, `vendor/k2-mpc-markdown`).
//!
//! The kit owns the docs, their history, presence, merge rules and the file
//! I/O of disk sync; K2 owns identity, permission, time, transport and
//! watching (`docs/SESSIONS.md` in the kit). This module is K2's side:
//!
//! | File | What |
//! |---|---|
//! | `mod.rs` | the runtime: the store (`~/.k2/docs.redb`), the session manager, the upkeep timer, release on hand-off and stop, pool stats (K1) |
//! | [`watch`] | the watcher feeding the kit's disk sync: `FsChanged` outside `.k2/`, its own watch inside `.k2/` (MI13) |
//! | [`host`] | K2 as the kit's `Host`: rights per pid, the clock, the audit sink (K2) |
//! | [`caller`] | who is calling, and the participant K2 stamps for them (K2) |
//! | [`target`] | workspace + path → a jailed doc key, and the deny list (K2) |
//! | [`shares`] | cross-workspace shares on the kit's host table (MI25) |
//! | [`cores`] | Settings → Cores → Multiplayer: the per-core switches, the `core_off` gate, turning a core on or off |
//! | [`routes`] | `/cli/docs/*` (K2) |
//! | [`tier0`] | the same routes on any other file: presence, a lease, versions and restore (tier 0) |
//! | [`versions`] | a tier-0 doc's new version, told to every client (`doc_version_changed`) |
//! | [`export`] | `k2 doc export`: the kit's export verb (P24), jobs, placement, audit |
//! | [`export_worker`] | the `k2-export` worker: which binary, its hello, limits, slots |
//! | [`live`] | `WS /cli/docs/live`: the Live view's socket (plan L2), one person's tab joined to the kit's session |
//! | [`live_wire`] | that socket's JSON frames (wire 3) |
//! | [`live_watch`] | that socket's keyed watch (a deck): `keyed_snapshot`, `keyed_page`, `ops_change` |
//! | [`live_deck`] | that socket on a live deck: watched, with previews and host events |
//! | [`blob_packs`] | which pack a tier-0 file opens with (PSD/PSB files: the PSD pack, with a layer index) and its verbs |
//! | [`indexer`] | the structure indexer (jobs in the `k2-export` worker) and the blob-doc change events |
//! | [`annotations`] | comments: mention notes, the owner's purge and participant erasure |
//! | [`turn`] | the host turn of an agent's write (from its activity row), and `turn_end` |
//! | [`host_events`] | host events on the docs socket: `preview_ready`, `turn_end`, comment relay |
//! | [`preview`] | live previews: one host service, a renderer per doc type (`.pptx` today) |
//! | [`opened`] | the version an app opened: K2 notes it when it opens a doc's file in another app, so that app's save merges from it |
//!
//! **Disk sync** (kit C4): every doc is opened with `open_file`, so the file
//! on disk stays true. The kit writes changes back (1 s quiet, 5 s at most,
//! atomic, echo-suppressed), merges outside edits as `kit:external`, and
//! reconciles a reopened doc with edits made while it was closed. K2 calls
//! `tick()` every 250 ms and feeds `file_changed()` from its watcher.
//!
//! **Tier 0** (kit amendment 3): every other file in a workspace is a blob
//! doc on the kit's `BlobStore` (bytes under `~/.k2/docs-blobs/`, the index
//! in the store): presence, a lease, versions and restore, kept in sync
//! with its file the same way. Files over [`blob_max_bytes`] are declined.
//! The upkeep collects unreferenced blobs (`gc`, after the kit's 1 h grace)
//! and, when a workspace is removed, destroys its blob key with its docs
//! (`purge_space_key`).

pub mod annotations;
#[cfg(test)]
mod blob_flow_tests;
#[cfg(test)]
mod demote_tests;
pub mod blob_packs;
pub mod blend_read_only;
pub mod cad;
pub mod cad_read_only;
pub mod caller;
pub mod claims;
pub mod conflicts;
pub mod cores;
pub mod export;
pub mod export_worker;
pub mod host;
pub mod host_events;
pub mod indexer;
pub mod live;
pub mod live_deck;
pub mod live_watch;
pub mod live_wire;
pub mod opened;
pub mod preview;
pub mod routes;
pub mod say;
pub mod shares;
pub mod target;
pub mod theirs;
pub mod tier0;
pub mod turn;
pub mod versions;
pub mod watch;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Duration;

use k2_core::log_debug;
use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::session::{BlobSession, Config, Demotion, PoolStats, Session, SessionError, SessionManager};
use k2_mpc_base::store::{CommitDurability, DocKey, DocStore, StoreSettings, StoreSettingsError};

/// The vendored kit writes docs back to their files (its C4, base
/// `e7c1fd6`). `docs-md-v1` is advertised while the runtime is up.
pub const KIT_DISK_SYNC: bool = true;

/// Env override for the memory budget, in MiB.
pub const ENV_MEMORY_MIB: &str = "K2_DOCS_MEMORY_MIB";

/// Largest file that can be a tier-0 doc. Its bytes cross `/cli/docs/*`
/// as base64 JSON (a `write` body, a `show` answer), so the cap bounds
/// every buffer on the way.
pub const BLOB_MAX_BYTES: u64 = 32 << 20;

/// Env override for the tier-0 cap, in MiB (at most [`BLOB_MAX_BYTES`];
/// tests lower it).
pub const ENV_BLOB_MAX_MIB: &str = "K2_DOCS_BLOB_MAX_MIB";

/// How long a starting daemon waits for the previous one to let go of the
/// store (hand-off).
const OPEN_WAIT: Duration = Duration::from_secs(5);

/// The kit's timer: due write-backs, stat checks, idle expiry and unload.
pub const TICK_EVERY: Duration = Duration::from_millis(250);

/// Rights re-check of every open connection (PRD K-PERM-12: within 5 s).
const RECHECK_EVERY_TICKS: u64 = 8;

/// Orphan-share sweep (about a minute; the first runs on the first tick).
const SWEEP_EVERY_TICKS: u64 = 240;

/// Grouped commits (kit v0.23.0, for 0.46.17): one wait
/// for the disk per group of at most 32 changes or 100 ms. The owner's
/// "Save every change immediately" ([`save_immediately`]) turns it off.
pub const COMMIT_GROUP_CHANGES: u32 = 32;
/// A group's longest wait, ms ([`COMMIT_GROUP_CHANGES`]).
pub const COMMIT_GROUP_MS: u64 = 100;

/// Why K2 flushed the store's saves ([`Runtime::flush_saves`]): an explicit
/// save, a tab's close, a tab going out of view, the docs going idle.
pub const FLUSH_REASONS: [&str; 4] = ["save", "close", "switch_away", "idle"];

/// Flushes that succeeded since this daemon started, by [`FLUSH_REASONS`]
/// (status shows them: `saves.flushes`).
static FLUSHES: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

fn count_flush(why: &str) {
    if let Some(i) = FLUSH_REASONS.iter().position(|r| *r == why) {
        FLUSHES[i].fetch_add(1, Ordering::SeqCst);
    }
}

/// Flushes since start, by reason (`{save, close, switch_away, idle}`).
pub fn flush_counts() -> serde_json::Value {
    serde_json::Value::Object(
        FLUSH_REASONS.iter().zip(FLUSHES.iter()).map(|(r, n)| (r.to_string(), serde_json::json!(n.load(Ordering::SeqCst)))).collect(),
    )
}


/// The loaded runtime.
pub struct Runtime {
    manager: SessionManager,
    /// The store's commit count at the last tick: a tick that finds it
    /// unchanged with commits not yet on disk is idle, and flushes them.
    last_commits: AtomicU64,
    /// K2's own handle on the store (shares, `log`); dropped at release so
    /// the file lock goes with the kit's.
    store: Mutex<Option<Arc<DocStore>>>,
    /// Docs opened in this process: key → canonical file path. For the
    /// rights re-check and the watcher (the manager doesn't list its
    /// sessions; unloaded keys drop out at the next re-check).
    opened: Mutex<HashMap<DocKey, PathBuf>>,
    watch: watch::Watch,
}

enum Slot {
    NotStarted,
    Running(Arc<Runtime>),
    Failed(OpenFailure),
    Released,
}

fn slot() -> &'static RwLock<Slot> {
    static SLOT: OnceLock<RwLock<Slot>> = OnceLock::new();
    SLOT.get_or_init(|| RwLock::new(Slot::NotStarted))
}

/// Why the docs runtime can't serve a call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unavailable {
    NotStarted,
    Failed(OpenFailure),
    Released,
}

/// Why the docs store didn't open: the line people read, and whether a
/// newer K2 wrote the store (that line stands alone: it already says what
/// happened and what to do, so it isn't wrapped in "failed to open (…)").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenFailure {
    pub why: String,
    pub newer_k2: bool,
}

impl OpenFailure {
    pub fn from_store_error(e: &k2_mpc_base::store::StoreError) -> Self {
        OpenFailure { why: store_open_error(e), newer_k2: store_is_newer_k2s(e) }
    }
}

impl Unavailable {
    pub fn message(&self) -> String {
        match self {
            // Lazy start (MC-6): with every core off the runtime never starts.
            Unavailable::NotStarted if !cores::any_on() => "live docs are off on this server: every multiplayer core is off. \
                 The owner or an Admin turns one on in Settings → Cores → Multiplayer (k2 doc cores)"
                .to_string(),
            Unavailable::NotStarted => "live docs are still starting".to_string(),
            Unavailable::Failed(f) if f.newer_k2 => f.why.clone(),
            Unavailable::Failed(f) => format!("live docs are off: the docs store failed to open ({})", f.why),
            Unavailable::Released => "this daemon handed off; the next one serves live docs".to_string(),
        }
    }
}

/// The memory budget in bytes: `K2_DOCS_MEMORY_MIB`, else the
/// `docsMemoryBudgetMib` setting, else 256 MiB.
pub fn memory_budget_bytes() -> u64 {
    let from_env = std::env::var(ENV_MEMORY_MIB).ok().and_then(|v| v.trim().parse::<u64>().ok()).filter(|v| *v > 0);
    let mib = from_env.unwrap_or_else(|| {
        let s = k2_core::app_settings::load().docs_memory_budget_mib;
        if s == 0 {
            k2_core::app_settings::DEFAULT_DOCS_MEMORY_BUDGET_MIB
        } else {
            s
        }
    });
    mib.saturating_mul(1 << 20)
}

/// The tier-0 size cap: `K2_DOCS_BLOB_MAX_MIB` (1 MiB up to the default),
/// else [`BLOB_MAX_BYTES`].
pub fn blob_max_bytes() -> u64 {
    parse_blob_max(std::env::var(ENV_BLOB_MAX_MIB).ok().as_deref())
}

fn parse_blob_max(env: Option<&str>) -> u64 {
    env.and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|v| *v > 0)
        .map(|mib| mib.saturating_mul(1 << 20).min(BLOB_MAX_BYTES))
        .unwrap_or(BLOB_MAX_BYTES)
}

/// The blob store over `store`, in [`k2_core::docs_store::blobs_path`]
/// (owner-only). `None` (logged) when the folder can't be made: text docs
/// still work, tier-0 calls answer `NoBlobStore`.
fn open_blobs(store: &Arc<DocStore>) -> Option<Arc<BlobStore>> {
    let dir = k2_core::docs_store::blobs_path();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        log_debug!("[docs] WARN: blob folder {} can't be made ({e}); any-file docs are off", dir.display());
        return None;
    }
    if let Err(e) = k2_core::private_home::restrict_path(&dir) {
        log_debug!("[docs] WARN: blob folder {}: {e}", dir.display());
    }
    match BlobStore::open(store.clone(), &dir, blob_config(), Arc::new(host::now_ms)) {
        Ok(b) => Some(Arc::new(b)),
        Err(e) => {
            log_debug!("[docs] WARN: blob store at {} failed to open ({e}); any-file docs are off", dir.display());
            None
        }
    }
}

/// The blob store's config: K2's size cap. Kit v0.19.0's chunked versions
/// stay off (its default): the first one stamps the store format 8, which
/// kit v0.18.0 and earlier refuse to open.
fn blob_config() -> BlobConfig {
    let mut cfg = BlobConfig::default();
    cfg.max_blob_bytes = blob_max_bytes();
    cfg
}

/// The owner's "Save every change immediately" (`docsSaveImmediately`,
/// off by default): every change waits for the disk before its call
/// returns, as before kit v0.23.0. Read when the store opens, so a change
/// takes effect at the next daemon start (kit v0.24.0 brings a live setter).
pub fn save_immediately() -> bool {
    k2_core::app_settings::load().docs_save_immediately
}

/// The store's settings K2 opens it with: the kit's default cache (64 MiB)
/// and grouped commits ([`COMMIT_GROUP_CHANGES`], [`COMMIT_GROUP_MS`]), or
/// every commit waiting for the disk when `immediate`.
pub fn store_settings(immediate: bool) -> Result<StoreSettings, StoreSettingsError> {
    if immediate {
        return Ok(StoreSettings::DEFAULT.with_commit_durability(CommitDurability::Immediate));
    }
    Ok(StoreSettings::DEFAULT.with_commit_durability(CommitDurability::grouped(COMMIT_GROUP_CHANGES, COMMIT_GROUP_MS)?))
}

/// `immediate` or `grouped`, for status and the log.
pub fn durability_word(s: &StoreSettings) -> &'static str {
    match s.commit_durability() {
        CommitDurability::Immediate => "immediate",
        _ => "grouped",
    }
}

/// The kit's settings K2 runs with. K2 sets no live size limit per file
/// type (`keyed_max_entries_by_pack`): every keyed type (a deck) takes the
/// host-wide `keyed_max_entries` unless its pack recommends a lower one.
/// Writers: K2 caps them at 128 per document (`writers_max`; the kit's
/// default 0 is no cap, which let a misbehaving client mint writers without
/// end, and some checks grow with them). Past the cap the kit refuses a new
/// writer's edits with `MPC-E908` writers_doc_cap (the connection reads),
/// and it sends `WritersNearCap` once per load at 80% (103 writers).
/// Person-writer reuse stays the kit's default (on), which changes nothing
/// while K2 sends no device key (`JoinOptions::device`; it sends none).
/// Kit v0.21.0's hold after the version an app opened is noted
/// (`opened_hold_ms`, [`opened`]) stays at the kit's 10 s.
///
/// Kit v0.23.0: `store` is what the store opens with ([`store_settings`]);
/// the manager reports it as it runs (`SessionManager::new` sets it from
/// the store).
///
/// Kit v0.24.0 returns freed memory to the OS after write-backs, at most
/// once per `release_after_write_ms` (the kit's 30 s, kept). K2 sets no
/// `Config::release`: the daemon runs on the system allocator (no
/// `#[global_allocator]`, no mimalloc), so the kit's own call is the right
/// one: glibc's `malloc_trim(0)` on Linux, and nothing on macOS and
/// Windows, whose allocators return memory on their own.
fn kit_config(memory_budget_bytes: u64, store: StoreSettings) -> Config {
    let mut c = Config::default();
    c.memory_budget_bytes = memory_budget_bytes;
    c.store = store;
    c.writers_max = WRITERS_MAX;
    c
}

/// The most writers a multiplayer document holds (`Config::writers_max`).
pub(crate) const WRITERS_MAX: usize = 128;

/// Open the store and start the session manager, its timer and the
/// watcher. Called on its own thread (the open waits for a handing-off
/// predecessor): at boot when a multiplayer core is on, and when one is
/// turned on (`cores`). A no-op while running; starts are serialized, so a
/// second caller never opens the store twice.
pub fn start() {
    static STARTING: Mutex<()> = Mutex::new(());
    let _one = STARTING.lock().unwrap_or_else(|p| p.into_inner());
    {
        let s = slot().read().expect("docs slot");
        if matches!(*s, Slot::Running(_) | Slot::Released) {
            return;
        }
    }
    let path = k2_core::docs_store::store_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let settings = match store_settings(save_immediately()) {
        Ok(s) => s,
        Err(e) => {
            log_debug!("[docs] WARN: the docs store's settings were refused: {e}");
            *slot().write().expect("docs slot") = Slot::Failed(OpenFailure { why: e.to_string(), newer_k2: false });
            return;
        }
    };
    let opened = DocStore::open_with_retry_with(&path, OPEN_WAIT, settings);
    let mut s = slot().write().expect("docs slot");
    match opened {
        Ok(store) => {
            let store = Arc::new(store);
            let mut manager = SessionManager::new(
                store.clone(),
                Arc::new(host::K2Host::new(Arc::downgrade(&store))),
                kit_config(memory_budget_bytes(), settings),
            );
            let blobs = open_blobs(&store);
            if let Some(b) = &blobs {
                manager = manager.with_blob_store(b.clone());
            }
            let rt = Runtime {
                manager,
                last_commits: AtomicU64::new(0),
                store: Mutex::new(Some(store)),
                opened: Mutex::new(HashMap::new()),
                watch: watch::Watch::start(),
            };
            log_debug!(
                "[docs] store open at {} (budget {} MiB, disk sync on, any-file docs {}, commits {})",
                path.display(),
                rt.manager.config().memory_budget_bytes >> 20,
                if blobs.is_some() { "on" } else { "off" },
                durability_word(&rt.manager.config().store),
            );
            *s = Slot::Running(Arc::new(rt));
            drop(s);
            spawn_timer();
            watch::spawn_fs_changed_listener();
            // Exports: a previous run's leftover jobs, and the worker's hello.
            export::sweep_leftovers();
            export_worker::check_in_background();
        }
        Err(e) => {
            let failure = OpenFailure::from_store_error(&e);
            log_debug!("[docs] WARN: docs store at {} failed to open: {}", path.display(), failure.why);
            *s = Slot::Failed(failure);
        }
    }
}

/// What a failed store open says. A store written by a newer K2 (a newer
/// store format) can't be opened by this one, and there is no way back:
/// say exactly that (0.46.7 moved stores to format 5; kit v0.9.6 keeps it).
/// A newer kit may also add a table without a new format (v0.9.7's
/// `authored`): that store still opens, reads and edits here, and only a
/// purge or rewrite is refused (`store_newer`, `routes::session_error`).
/// "Reads up to" names the highest format the kit reads
/// (`STORE_FORMAT_READ`: 8 since kit v0.19.0), not the one it stamps.
/// A format-8 store whose required features this K2 doesn't know, or that
/// sets the native feature but holds no engine store, is also a newer K2's:
/// the kit refuses it before reading anything, so the store and the files
/// on disk are untouched. Since kit v0.20.0 a store holding native-engine
/// documents (a Beta) opens here, and only those documents are refused,
/// each on its own (`MPC-E169`, `routes::session_error`).
pub fn store_open_error(e: &k2_mpc_base::store::StoreError) -> String {
    use k2_mpc_base::store::StoreError;
    match e {
        StoreError::Newer { found } => format!(
            "the docs store was written by a newer K2 (store format {found}; this K2 reads up to {}). \
             A box that opened documents with a newer K2 can't go back to an older K2: update K2 again",
            k2_mpc_base::store::STORE_FORMAT_READ
        ),
        StoreError::NativeWithoutEngine | StoreError::UnknownFeatures { .. } => NATIVE_STORE_FROM_NEWER_K2.to_string(),
        other => other.to_string(),
    }
}

/// What a store a newer K2 wrote with native-engine documents says.
const NATIVE_STORE_FROM_NEWER_K2: &str = "This box's document store was written by a newer K2 \
     (it has native-engine documents, a Beta). This K2 can't open it: update K2 to open live documents again. \
     Nothing in the store was changed, and the files on disk hold their latest text.";

/// Whether a newer K2 wrote the store (its line stands alone).
fn store_is_newer_k2s(e: &k2_mpc_base::store::StoreError) -> bool {
    use k2_mpc_base::store::StoreError;
    matches!(e, StoreError::Newer { .. } | StoreError::NativeWithoutEngine | StoreError::UnknownFeatures { .. })
}

/// The running runtime, or why there is none.
pub fn runtime() -> Result<Arc<Runtime>, Unavailable> {
    match &*slot().read().expect("docs slot") {
        Slot::Running(rt) => Ok(rt.clone()),
        Slot::NotStarted => Err(Unavailable::NotStarted),
        Slot::Failed(e) => Err(Unavailable::Failed(e.clone())),
        Slot::Released => Err(Unavailable::Released),
    }
}

/// `docs-md-v1` is advertised only when a client can rely on it: the
/// runtime is up and the kit writes docs back to their files.
pub fn live_ready() -> bool {
    KIT_DISK_SYNC && runtime().is_ok()
}

/// Hand-off (quiesce) and normal stop: write back every doc with unwritten
/// changes, close every session and let go of the store file so the next
/// daemon can open it (the kit's `release()` flushes first). One-way:
/// later calls get [`Unavailable::Released`]. Returns whether the store was
/// open.
pub fn release(why: &str) -> bool {
    let rt = {
        let mut s = slot().write().expect("docs slot");
        match std::mem::replace(&mut *s, Slot::Released) {
            Slot::Running(rt) => rt,
            _ => return false,
        }
    };
    if let Err(e) = rt.manager.release() {
        log_debug!("[docs] WARN: release ({why}): {e}");
    }
    if let Some(store) = rt.store() {
        claims::flush(&store);
    }
    rt.store.lock().expect("docs store handle").take();
    rt.watch.stop();
    log_debug!("[docs] released the docs store ({why})");
    true
}

/// The 250 ms timer thread (exits once released).
fn spawn_timer() {
    let _ = std::thread::Builder::new().name("k2-docs-tick".into()).spawn(|| {
        let mut n: u64 = 0;
        loop {
            std::thread::sleep(TICK_EVERY);
            match runtime() {
                Ok(rt) => {
                    rt.tick();
                    if n % RECHECK_EVERY_TICKS == 0 {
                        rt.recheck_rights();
                    }
                    if n % SWEEP_EVERY_TICKS == 0 {
                        rt.sweep_orphans();
                    }
                }
                Err(Unavailable::Released) => return,
                Err(_) => {}
            }
            n = n.wrapping_add(1);
        }
    });
}

/// Whether a project id is still a registered workspace. Only a definite
/// "no such row" says no: a DB error keeps the share (never erase on doubt).
fn workspace_exists(id: &str) -> bool {
    let db = k2_core::db::shared();
    let conn = db.lock();
    match conn.query_row("SELECT 1 FROM projects WHERE id = ?1", rusqlite::params![id], |_| Ok(())) {
        Ok(()) => true,
        Err(rusqlite::Error::QueryReturnedNoRows) => false,
        Err(_) => true,
    }
}

/// The tier-0 open of `key` (`open_blob_file` with its file type's pack),
/// and the kit's demote when the doc is stored as live text under a name
/// that now routes to tier 0 by itself ([`target::Tier::of`] isn't Markdown
/// or text: the CAD, BIM and circuit-board files the text pack moved to
/// not-text, K2's own design list, and the timeline and fab files, which the
/// text pack took as text before their cores; an `.otio` served at tier 0
/// while its core is off too). `demote_text_file` runs once: the same key and doc
/// id become a tier-0 doc with every text version, the live state current
/// and written to the file, an outside save since the last sync kept beside
/// it (keep-both, never merged by lines), the change log readable as the
/// live log. The text session closes as `demoted`. Later opens find a tier-0
/// doc and never demote again.
///
/// Any other mismatch stays the kit's `PackMismatch`: a text file served at
/// tier 0 only because its pack refuses it today (it turned binary, or too
/// large) keeps its live history. `Some` only when this call demoted.
pub(crate) fn open_blob_or_demote(
    manager: &SessionManager,
    key: &DocKey,
    abs: &Path,
    root: &Path,
) -> Result<(Arc<BlobSession>, Option<Demotion>), SessionError> {
    let pack = blob_packs::pack_for(&key.path);
    match manager.open_blob_file(pack.clone(), key, abs, root) {
        Err(SessionError::PackMismatch { stored, .. })
            if stored == k2_mpc_text::SCHEMA.name && !matches!(target::Tier::of(&key.path), target::Tier::Markdown | target::Tier::Text(_)) =>
        {
            let (s, d) = manager.demote_text_file(Arc::new(k2_mpc_text::PACK), pack, key, abs, root)?;
            Ok((s, (!d.already).then_some(d)))
        }
        r => r.map(|s| (s, None)),
    }
}

/// Host-table namespace of the workspaces (spaces) K2 has opened docs in, so
/// the sweep can delete a removed workspace's docs (`delete_space`).
const SPACES_NS: &str = "k2.docs.spaces";

impl Runtime {
    /// The session for `key`, syncing with the file at `abs` inside `root`
    /// (`open_file` with `pack`, Markdown's or the text pack's: import, or
    /// reconcile a stored doc with the file).
    pub fn open(
        &self,
        key: &DocKey,
        abs: &Path,
        root: &Path,
        pack: Arc<dyn k2_mpc_base::session::Pack>,
    ) -> Result<Arc<Session>, SessionError> {
        let s = self.manager.open_file(pack, key, abs, root)?;
        self.note_opened(key, abs);
        Ok(s)
    }

    /// Tier 0: the blob doc for any other file at `abs` inside `root`
    /// (`open_blob_file`: import, or reconcile a stored doc with the file),
    /// with its file type's pack ([`blob_packs::pack_for`]: PSD/PSB files
    /// get the PSD pack and its layer index). A version with no index yet
    /// is queued for the indexer.
    ///
    /// A doc stored as live text under a name that now routes to tier 0 is
    /// demoted first, once ([`open_blob_or_demote`]); the Live sockets that
    /// were in its text session are told to reopen it ([`live::doc_demoted`]).
    pub fn open_blob(&self, key: &DocKey, abs: &Path, root: &Path) -> Result<Arc<BlobSession>, SessionError> {
        let (s, demoted) = open_blob_or_demote(&self.manager, key, abs, root)?;
        if let Some(d) = demoted {
            let told = live::doc_demoted(key, &d.connections, d.written);
            log_debug!(
                "[docs] {} was live text and is now an any-file doc: {} versions, {} changes kept, written={}, kept_both={}, {} of {} connections told",
                host::key_label(key),
                d.versions.len(),
                d.changes,
                d.written,
                d.kept_both,
                told,
                d.connections.len(),
            );
        }
        self.note_opened(key, abs);
        indexer::note_root(&key.space, root);
        if blob_packs::indexes(&key.path) {
            indexer::on_open(&s, key);
        }
        Ok(s)
    }

    /// A doc was loaded: the watcher index, the `.k2/` watch, the space row.
    fn note_opened(&self, key: &DocKey, abs: &Path) {
        let fresh = self.opened.lock().expect("docs opened set").insert(key.clone(), abs.to_path_buf()).is_none();
        if fresh {
            if key.path.starts_with(".k2/") {
                self.watch.watch_k2_file(abs);
            }
            self.note_space(&key.space);
        }
    }

    /// A session closed by a turn-off left: the watcher index and its
    /// `.k2/` watch drop it now.
    fn forget(&self, key: &DocKey) {
        if let Some(p) = self.opened.lock().expect("docs opened set").remove(key) {
            if key.path.starts_with(".k2/") {
                self.watch.unwatch_k2_file(&p);
            }
        }
    }

    /// Loaded tier-0 doc for `key`, if any (never loads).
    pub fn get_blob(&self, key: &DocKey) -> Option<Arc<BlobSession>> {
        self.manager.get_blob(key)
    }

    /// The blob store, when any-file docs are on.
    pub fn blobs(&self) -> Option<Arc<BlobStore>> {
        self.manager.blob_store().cloned()
    }

    /// Remember that `space` holds docs (once per space).
    fn note_space(&self, space: &str) {
        let Some(store) = self.store() else { return };
        match store.host_get(SPACES_NS, space) {
            Ok(Some(_)) => {}
            Ok(None) => {
                if let Err(e) = store.host_put(SPACES_NS, space, b"") {
                    log_debug!("[docs] WARN: can't record space {space}: {e}");
                }
            }
            Err(e) => log_debug!("[docs] WARN: can't read space {space}: {e}"),
        }
    }

    /// `mv`: the doc at `old` now lives at `new` (`new_abs`, already renamed
    /// on disk by the caller). Moves the store record and the loaded
    /// session's key; the watcher index and `.k2/` watches follow.
    pub fn rekey(&self, old: &DocKey, new: &DocKey, new_abs: &Path) -> Result<(), SessionError> {
        self.manager.rekey(old, new.clone())?;
        claims::rekey(self.store().as_deref(), old, new);
        live::doc_moved(old, new);
        // Previews follow the doc (contract Item 8: a rename never leaves
        // them at the old key).
        preview::service::doc_moved(old, new);
        // An outside save kept for "Use their version" was the old file's.
        theirs::forget(old);
        host_events::forget_saves(old);
        let mut set = self.opened.lock().expect("docs opened set");
        if let Some(p) = set.remove(old) {
            if old.path.starts_with(".k2/") {
                self.watch.unwatch_k2_file(&p);
            }
        }
        set.insert(new.clone(), new_abs.to_path_buf());
        if new.path.starts_with(".k2/") {
            self.watch.watch_k2_file(new_abs);
        }
        Ok(())
    }

    /// Loaded session for `key`, if any (never loads).
    pub fn get(&self, key: &DocKey) -> Option<Arc<Session>> {
        self.manager.get(key)
    }

    pub fn stats(&self) -> Result<PoolStats, SessionError> {
        self.manager.stats()
    }

    pub fn config(&self) -> &Config {
        self.manager.config()
    }

    /// The store, while open.
    pub fn store(&self) -> Option<Arc<DocStore>> {
        self.store.lock().expect("docs store handle").clone()
    }

    /// The loaded docs whose file is `path`, or (for a folder) whose file is
    /// directly inside it: `(key, is_the_file)`.
    pub(crate) fn docs_at(&self, path: &Path) -> Vec<(DocKey, bool)> {
        self.opened
            .lock()
            .expect("docs opened set")
            .iter()
            .filter_map(|(k, f)| {
                if f == path {
                    Some((k.clone(), true))
                } else if f.parent() == Some(path) {
                    Some((k.clone(), false))
                } else {
                    None
                }
            })
            .collect()
    }

    /// The kit's timer: due write-backs, the stat backup, idle expiry and
    /// unload, version cuts and retention. Failures are per doc
    /// (write-back retries next tick).
    fn tick(&self) {
        match self.manager.tick() {
            Ok(rep) => {
                for (k, e) in &rep.errors {
                    log_debug!("[docs] WARN: {}: {e}", host::key_label(k));
                }
            }
            Err(SessionError::Released) => {}
            Err(e) => log_debug!("[docs] WARN: tick: {e}"),
        }
        // Agents' last reads, written behind (they survive a restart).
        if let Some(store) = self.store() {
            claims::flush(&store);
        }
        self.flush_if_idle();
    }

    /// Grouped commits: every change so far made durable now
    /// (`SessionManager::flush`; nothing to do when every commit is on disk,
    /// or with "Save every change immediately"). K2 calls it at a person's
    /// or agent's explicit save (`k2 doc flush`), when a tab closes, when it
    /// goes out of view (its `visible: false`), and when the docs go idle.
    /// Returns whether every commit is on disk after it. A failure is
    /// logged: a commit that failed at the disk rolls the store back at the
    /// next tick or flush (`AuditEvent::StoreRolledBack`).
    pub fn flush_saves(&self, why: &str) -> bool {
        match self.manager.flush() {
            Ok(()) => {
                count_flush(why);
                true
            }
            Err(SessionError::Released) => false,
            Err(e) => {
                log_debug!("[docs] WARN: flush ({why}): {e}");
                false
            }
        }
    }

    /// Commits not yet on disk, and none since the last tick: the docs are
    /// idle, so flush them ([`Runtime::flush_saves`]). Returns whether it
    /// flushed.
    fn flush_if_idle(&self) -> bool {
        let Some(store) = self.store() else { return false };
        let commits = store.commits();
        let before = self.last_commits.swap(commits, Ordering::SeqCst);
        if commits != before || store.durable_through() >= commits {
            return false;
        }
        self.flush_saves("idle")
    }

    /// Every loaded doc's participants have their rights re-checked (the
    /// kit's `recheck_rights`; losing read removes them, audited through the
    /// host). The watcher's index drops what unloaded, with its `.k2/`
    /// watch.
    fn recheck_rights(&self) {
        for (k, c) in self.manager.recheck_rights() {
            log_debug!("[docs] rights of {} on {} now {:?}", c.pid, host::key_label(&k), c.rights);
        }
        let loaded: HashSet<DocKey> = self.manager.loaded_keys().into_iter().collect();
        let mut set = self.opened.lock().expect("docs opened set");
        let gone: Vec<DocKey> = set.keys().filter(|k| !loaded.contains(*k)).cloned().collect();
        for k in gone {
            if let Some(p) = set.remove(&k) {
                if k.path.starts_with(".k2/") {
                    self.watch.unwatch_k2_file(&p);
                }
            }
        }
    }

    /// A removed workspace's records: its shares (as owner or sharee, K2's
    /// host rows) and its docs (`delete_space`, every kit table). This is
    /// housekeeping, not privacy erasure: redb doesn't zero freed pages
    /// until the kit's erasure slice. Never on a DB error.
    fn sweep_orphans(&self) {
        let Some(store) = self.store() else { return };
        match shares::sweep_orphans(&store, workspace_exists) {
            Ok(0) => {}
            Ok(n) => {
                log_debug!("[docs] removed {n} share(s) of removed workspaces");
                host::audit("doc-share-removed", "k2", format!("count={n} reason=workspace_removed"));
            }
            Err(e) => log_debug!("[docs] WARN: share sweep: {e}"),
        }
        let spaces = match store.host_scan(SPACES_NS, "") {
            Ok(rows) => rows,
            Err(e) => {
                log_debug!("[docs] WARN: space sweep: {e}");
                return;
            }
        };
        drop(store);
        self.collect_blobs();
        export::sweep_leftovers();
        // The preview cache's cap (contract Item 8, hook 5).
        preview::service::upkeep();
        for (space, _) in spaces {
            if workspace_exists(&space) {
                continue;
            }
            match self.manager.delete_space(&space) {
                Ok(keys) => {
                    let mut set = self.opened.lock().expect("docs opened set");
                    for k in &keys {
                        if let Some(p) = set.remove(k) {
                            if k.path.starts_with(".k2/") {
                                self.watch.unwatch_k2_file(&p);
                            }
                        }
                    }
                    drop(set);
                    log_debug!("[docs] removed {} doc(s) of removed workspace {space}", keys.len());
                    host::audit("doc-space-removed", "k2", format!("space={space} docs={} reason=workspace_removed", keys.len()));
                    // The space's blob key goes with it, and every blob
                    // stored under it: the same bytes stored again get an
                    // unrelated id. The space row stays until this worked,
                    // so a failure retries on the next sweep.
                    // Its previews too, before the space row goes (contract
                    // Item 8, hook 3: a failure retries on the next sweep).
                    if !self.purge_space_key(&space)
                        || !claims::forget_space(self.store().as_deref(), &space)
                        || !preview::service::space_removed(&space)
                    {
                        continue;
                    }
                    if let Some(store) = self.store() {
                        let _ = store.host_delete(SPACES_NS, &space);
                    }
                }
                Err(e) => log_debug!("[docs] WARN: removing the docs of {space}: {e}"),
            }
        }
    }

    /// Destroy a removed workspace's tier-0 key and blobs (K-T0-4), audited
    /// as metadata. True when done (or there is no blob store).
    fn purge_space_key(&self, space: &str) -> bool {
        let Some(blobs) = self.blobs() else { return true };
        match blobs.purge_space_key(space) {
            Ok(r) => {
                // Since kit v0.15.0 the kit rewrites the store file after
                // every purge that destroyed a key, so the key's bytes (and
                // the index rows', which hold layer names and text) leave it.
                // When that rewrite failed, try once more and say so.
                let mut rewritten = r.store_rewritten;
                if rewrite_again(&r) {
                    match self.store().map(|st| st.rewrite_file()) {
                        Some(Ok(_)) => rewritten = true,
                        Some(Err(e)) => log_debug!(
                            "[docs] WARN: {space}'s blob key was destroyed but the store file wasn't rewritten ({e}); its bytes stay in freed pages until a later rewrite"
                        ),
                        None => {}
                    }
                }
                if r.key_destroyed || r.blobs_removed > 0 || r.index_rows_removed > 0 {
                    host::audit(
                        host::AUDIT_SPACE_KEY_PURGED,
                        "k2",
                        format!(
                            "space={space} key_destroyed={} blobs={} bytes={} index_rows={} store_rewritten={rewritten} reason=workspace_removed",
                            r.key_destroyed, r.blobs_removed, r.bytes_removed, r.index_rows_removed
                        ),
                    );
                }
                true
            }
            Err(e) => {
                log_debug!("[docs] WARN: purging the blob key of {space}: {e}");
                false
            }
        }
    }

    /// Blob GC: unreferenced blobs past the kit's grace (1 h), files a crash
    /// left without an index row, purged spaces' folders.
    fn collect_blobs(&self) {
        let Some(blobs) = self.blobs() else { return };
        if let Err(e) = blobs.gc() {
            log_debug!("[docs] WARN: blob gc: {e}");
        }
    }
}

/// Run the orphan sweep now (removed workspaces' docs, keys and previews),
/// rather than on its minute tick. For headless tests.
#[doc(hidden)]
pub fn sweep_orphans_now() {
    if let Ok(rt) = runtime() {
        rt.sweep_orphans();
    }
}

/// Whether a removed workspace's purge left the store file unrewritten with
/// something in it to scrub: the destroyed key (kit v0.15.0 always rewrites
/// then, so `store_rewritten` false means that rewrite failed) or index rows.
fn rewrite_again(r: &k2_mpc_base::blob::PurgeReport) -> bool {
    (r.key_destroyed || r.index_rows_removed > 0) && !r.store_rewritten
}

/// How the store saves (kit v0.23.0): `commits` is what it runs with
/// (`grouped` or `immediate`), `saveEveryChangeImmediately` the owner's
/// setting, and `restartNeeded` when they differ (the setting takes effect
/// at the next daemon start); `commitsMade` and `commitsOnDisk` since the
/// store opened, and the flushes by reason.
fn saves_json(rt: &Runtime) -> serde_json::Value {
    let store = rt.config().store;
    let setting = save_immediately();
    let running_immediate = matches!(store.commit_durability(), CommitDurability::Immediate);
    let (made, on_disk) = rt.store().map_or((0, 0), |s| (s.commits(), s.durable_through()));
    let mut v = serde_json::json!({
        "commits": durability_word(&store),
        "saveEveryChangeImmediately": setting,
        "restartNeeded": setting != running_immediate,
        "commitsMade": made,
        "commitsOnDisk": on_disk,
        "flushes": flush_counts(),
    });
    if let CommitDurability::Grouped { max_changes, max_ms } = store.commit_durability() {
        v["groupMaxChanges"] = serde_json::json!(max_changes);
        v["groupMaxMs"] = serde_json::json!(max_ms);
    }
    v
}

/// Pool stats for status and doctor: `{running, loaded, estimatedBytes,
/// budgetBytes, unloadedTotal, diskSync}`, or why the runtime is down.
pub fn pool_json() -> serde_json::Value {
    match runtime() {
        Ok(rt) => match rt.stats() {
            Ok(p) => serde_json::json!({
                "running": true,
                "loaded": p.loaded,
                "estimatedBytes": p.estimated_bytes,
                "budgetBytes": p.budget_bytes,
                "unloadedTotal": p.unloaded_total,
                "diskSync": KIT_DISK_SYNC,
                "anyFile": {
                    "on": rt.blobs().is_some(),
                    "loaded": p.blob_sessions,
                    "blobs": p.blob_count,
                    "bytes": p.blob_bytes,
                    "unreferencedBytes": p.blob_unreferenced_bytes,
                    "maxFileBytes": blob_max_bytes(),
                },
                "saves": saves_json(&rt),
            }),
            Err(e) => serde_json::json!({ "running": false, "error": e.to_string() }),
        },
        Err(u) => serde_json::json!({ "running": false, "error": u.message() }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Kit v0.18.0: a keyed doc's live size limit is resolved per file type
    /// (the host's limit for the type, else the lower of the pack's own
    /// default and the host-wide limit). K2 sets none per type, and the deck
    /// pack recommends none, so a deck keeps K2's host-wide limit of 25,000
    /// elements and fields. A pack that starts recommending a lower one
    /// fails here first.
    #[test]
    fn a_deck_takes_k2s_host_wide_live_size_limit() {
        use k2_mpc_base::session::{KeyedLimit, LimitSource};
        let c = kit_config(256 << 20, StoreSettings::DEFAULT);
        assert!(c.keyed_max_entries_by_pack.is_empty(), "K2 sets no limit per file type: {:?}", c.keyed_max_entries_by_pack);
        assert_eq!(c.keyed_max_entries, 25_000, "K2's host-wide live size limit");
        let deck = k2_mpc_pptx::PptxLive::default();
        assert_eq!(c.keyed_limit(&deck), KeyedLimit::new(25_000, LimitSource::HostWide));
    }

    /// Kit v0.20.0's writer ids: K2 leaves the writers cap off and sends no
    /// device key, so person-writer reuse (on by default) changes nothing.
    /// Agents count toward the participant cap as persons do; K2 keeps the
    /// kit's cap. A watcher past it takes no writer slot until promoted.
    #[test]
    fn k2_caps_writers_at_128_and_keeps_the_other_writer_defaults() {
        let c = kit_config(256 << 20, StoreSettings::DEFAULT);
        assert_eq!(c.writers_max, 128, "K2's writers cap per document");
        assert_ne!(Config::default().writers_max, c.writers_max, "the kit's default (0, no cap) is not K2's");
        assert_eq!(c.participants_max, Config::default().participants_max, "the kit's participant cap");
        assert!(c.reuse_person_writers, "the kit's default; nothing changes without a device key");
        // With a cap the near-cap notice comes at 80% of it, never against
        // the kit's no-cap ceiling (WRITERS_NOTICE_MAX), which stays as it is.
        assert!(c.writers_max < k2_mpc_base::session::WRITERS_NOTICE_MAX, "the cap is the notice's ceiling, not 8,192");
        assert_eq!(k2_mpc_base::session::WRITERS_NOTICE_MAX, 8_192, "the kit's ceiling with no cap, unused while K2 caps");
    }

    /// Kit v0.21.0: after K2 notes the file an app opened, the kit holds its
    /// write-backs for the kit's default (10 s), so the app reads the
    /// version noted; K2 doesn't change it.
    #[test]
    fn k2_keeps_the_kits_opened_hold() {
        let c = kit_config(256 << 20, StoreSettings::DEFAULT);
        assert_eq!(c.opened_hold_ms, Config::default().opened_hold_ms, "the kit's hold");
        assert_eq!(c.opened_hold_ms, 10_000, "10 s");
    }

    /// Kit v0.24.0: freed memory goes back to the OS after write-backs at the
    /// kit's pace (30 s), through the kit's own call: the daemon runs on the
    /// system allocator, so K2 hands it no allocator call of its own.
    #[test]
    fn k2_keeps_the_kits_memory_release_after_write_backs() {
        let c = kit_config(256 << 20, StoreSettings::DEFAULT);
        assert_eq!(c.release_after_write_ms, Config::default().release_after_write_ms, "the kit's pace");
        assert_eq!(c.release_after_write_ms, 30_000, "30 s");
        assert!(c.release.is_none(), "no allocator call of K2's: the kit's malloc_trim on glibc");
    }

    #[test]
    fn a_too_new_store_says_there_is_no_way_back() {
        // Kit v0.19.0 reads up to format 8 (the required-features word):
        // a store above it is a newer K2's.
        let m = store_open_error(&k2_mpc_base::store::StoreError::Newer { found: 9 });
        assert!(m.contains("store format 9; this K2 reads up to 8"), "{m}");
        assert!(m.contains("can't go back to an older K2"), "{m}");
        assert_eq!(k2_mpc_base::store::STORE_FORMAT_READ, 8, "kit v0.19.0 reads up to store format 8");
        // Kit v0.10.0.x reads up to format 6 (structured docs). A store is
        // created at 5 and stamped 6 only by its first structured doc (a
        // grid), which K2 0.46.7 never opens (no CSV grid core): every
        // store 0.46.7 writes stays format 5 (WHATS_NEW says so). Format 8
        // comes only with a first chunked version, which K2 leaves off.
        assert_eq!(k2_mpc_base::store::STORE_FORMAT, 6, "0.46.7 reads up to store format 6");
        assert_eq!(k2_mpc_base::store::STORE_FORMAT_BASE, 5, "0.46.7 writes store format 5");
    }

    /// A store the kit refuses at open, before reading anything, as a newer
    /// K2's: unknown required features, or the native feature with no engine
    /// store (since kit v0.20.0 a store holding native-engine documents
    /// opens, and only those documents are refused). K2 says so plainly,
    /// in one line people can act on, not the kit's own words and not
    /// wrapped in "the docs store failed to open (…)": that is the line the
    /// CLI's 503 `docs_unavailable` and the Live view's closed frame carry.
    #[test]
    fn a_store_with_native_engine_documents_says_update_k2() {
        use k2_mpc_base::store::StoreError;
        let want = "This box's document store was written by a newer K2 (it has native-engine documents, a Beta). \
                    This K2 can't open it: update K2 to open live documents again. \
                    Nothing in the store was changed, and the files on disk hold their latest text.";
        for e in [StoreError::NativeWithoutEngine, StoreError::UnknownFeatures { found: 8, unknown: 1 << 40 }] {
            assert_eq!(store_open_error(&e), want, "{e:?}");
            let f = OpenFailure::from_store_error(&e);
            assert!(f.newer_k2, "{e:?} is a newer K2's store");
            assert_eq!(Unavailable::Failed(f).message(), want, "{e:?}: the line stands alone");
        }
    }

    /// A newer store format also stands alone; any other open failure keeps
    /// the "failed to open (…)" wrapper with the kit's words inside.
    #[test]
    fn only_a_newer_k2s_store_drops_the_failed_to_open_wrapper() {
        use k2_mpc_base::store::StoreError;
        let newer = Unavailable::Failed(OpenFailure::from_store_error(&StoreError::Newer { found: 9 })).message();
        assert!(newer.starts_with("the docs store was written by a newer K2 (store format 9;"), "{newer}");
        let db = Unavailable::Failed(OpenFailure::from_store_error(&StoreError::Db("disk I/O error".into()))).message();
        assert_eq!(db, "live docs are off: the docs store failed to open (store: disk I/O error)");
    }

    /// Kit v0.19.0's chunked versions stay off: the first chunked version
    /// stamps the store format 8, which a K2 on kit v0.18.0 or earlier
    /// refuses to open, so turning it on would take away the way back to
    /// the release before.
    #[test]
    fn chunked_versions_stay_off() {
        let cfg = blob_config();
        assert!(!cfg.chunked_versions, "chunked versions stay off until the release K2 would roll back to reads format 8");
        assert_eq!(cfg.max_blob_bytes, blob_max_bytes(), "K2's size cap");
    }

    /// Kit v0.15.0: a purge that destroyed a key rewrites the store file
    /// itself; K2 rewrites again only when that failed (the key's or the
    /// index rows' bytes would stay in freed pages).
    #[test]
    fn a_purge_is_rewritten_again_only_when_the_kits_rewrite_failed() {
        use k2_mpc_base::blob::PurgeReport;
        let report = |key_destroyed: bool, index_rows_removed: u64, store_rewritten: bool| {
            let mut r = PurgeReport::default();
            r.key_destroyed = key_destroyed;
            r.index_rows_removed = index_rows_removed;
            r.store_rewritten = store_rewritten;
            r
        };
        assert!(rewrite_again(&report(true, 0, false)), "a destroyed key the kit's rewrite didn't scrub");
        assert!(rewrite_again(&report(true, 3, false)));
        assert!(rewrite_again(&report(false, 3, false)), "index rows alone");
        assert!(!rewrite_again(&report(true, 0, true)), "the kit rewrote the file");
        assert!(!rewrite_again(&report(true, 3, true)));
        assert!(!rewrite_again(&report(false, 0, false)), "the space had no key: nothing to scrub");
    }

    #[test]
    fn the_tier_0_cap_is_32_mib_and_the_env_only_lowers_it() {
        assert_eq!(parse_blob_max(None), BLOB_MAX_BYTES);
        assert_eq!(BLOB_MAX_BYTES, 32 << 20);
        assert_eq!(parse_blob_max(Some("4")), 4 << 20);
        assert_eq!(parse_blob_max(Some(" 1 ")), 1 << 20);
        assert_eq!(parse_blob_max(Some("64")), BLOB_MAX_BYTES, "never past the default (it sizes the body cap)");
        for junk in ["0", "", "x", "-3"] {
            assert_eq!(parse_blob_max(Some(junk)), BLOB_MAX_BYTES, "{junk:?}");
        }
    }

    /// Kit v0.23.0: K2 opens the store with grouped
    /// saves (32 changes or 100 ms per wait for the disk) and the kit's
    /// cache; the owner's "Save every change immediately" opens it with every
    /// change waiting for the disk.
    #[test]
    fn grouped_saves_are_the_default_and_the_owner_setting_turns_them_off() {
        let grouped = store_settings(false).expect("grouped settings");
        assert_eq!(grouped, StoreSettings::DEFAULT.with_commit_durability(CommitDurability::grouped(32, 100).expect("in range")));
        assert_eq!(grouped.commit_durability(), CommitDurability::Grouped { max_changes: 32, max_ms: 100 });
        assert_eq!(grouped.cache_bytes(), StoreSettings::DEFAULT.cache_bytes(), "the kit's 64 MiB cache");
        assert_eq!(durability_word(&grouped), "grouped");
        let immediate = store_settings(true).expect("immediate settings");
        assert_eq!(immediate.commit_durability(), CommitDurability::Immediate);
        assert_eq!(immediate, StoreSettings::DEFAULT);
        assert_eq!(durability_word(&immediate), "immediate");
        assert_eq!(kit_config(256 << 20, grouped).store, grouped, "the config reports the store as it runs");
        assert!(!k2_core::app_settings::AppSettings::default().docs_save_immediately, "the setting is off by default");
        crate::test_support::with_temp_home(|| {
            assert!(!save_immediately(), "a new server saves in groups");
            k2_core::app_settings::update(serde_json::json!({ "docsSaveImmediately": true })).expect("turn it on");
            assert!(save_immediately(), "the setting is read from app settings");
        });
    }

    /// A runtime over a temp store with grouped saves whose group never
    /// fills or times out by itself (the host's clock stands still, and the
    /// doc's seq floor is already raised), so only K2's barriers put it on
    /// disk.
    struct Grouped {
        dir: std::path::PathBuf,
        rt: Runtime,
        s: Arc<Session>,
        conn: k2_mpc_base::session::ConnId,
    }

    impl Drop for Grouped {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    struct StillHost;

    impl k2_mpc_base::session::Host for StillHost {
        fn rights(&self, _who: &k2_mpc_base::session::Participant, _key: &DocKey) -> k2_mpc_base::session::Rights {
            k2_mpc_base::session::Rights::editor()
        }
        fn now_ms(&self) -> u64 {
            1_700_000_000_000
        }
        fn on_event(&self, _ev: &k2_mpc_base::session::AuditEvent) {}
    }

    impl Grouped {
        fn new() -> Grouped {
            use k2_mpc_base::session::{Participant, ParticipantKind, Stay, TextFilePack};
            let dir = std::env::temp_dir().join(format!("k2-docs-grouped-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&dir).expect("dir");
            let settings = StoreSettings::DEFAULT.with_commit_durability(CommitDurability::grouped(1024, 10_000).expect("in range"));
            let store = Arc::new(DocStore::open_with(dir.join("docs.redb"), settings).expect("store"));
            let mut config = kit_config(256 << 20, settings);
            config.edit_rate_per_s = 1_000_000;
            config.edit_burst = 1_000_000;
            let manager = SessionManager::new(store.clone(), Arc::new(StillHost), config);
            let s = manager
                .open(Arc::new(TextFilePack::new("markdown", 4 << 20)), &DocKey { space: "ws".into(), path: "notes.md".into() }, Some(b"one\n"))
                .expect("open");
            let conn = s.join(Participant::new("agent:writer", "Writer", "#16a34a", ParticipantKind::Agent, "cli"), Stay::Held).expect("join").conn;
            let rt = Runtime {
                manager,
                last_commits: AtomicU64::new(0),
                store: Mutex::new(Some(store)),
                opened: Mutex::new(HashMap::new()),
                watch: watch::Watch::start(),
            };
            let g = Grouped { dir, rt, s, conn };
            // The doc's first versions raise its seq floor (the kit's
            // `seq_floor.rs`), and that commit waits for the disk; the next
            // ones (up to the group's 1,024) don't.
            g.edit("warm\n");
            g
        }

        fn edit(&self, line: &str) {
            self.s
                .run_tool(self.conn, k2_mpc_base::session::TextFilePack::ROOT, &["append".to_owned()], line.as_bytes())
                .expect("append");
        }

        /// (commits made, commits on disk).
        fn counts(&self) -> (u64, u64) {
            let st = self.rt.store().expect("store");
            (st.commits(), st.durable_through())
        }

        fn flushes(&self, why: &str) -> u64 {
            flush_counts()[why].as_u64().expect("a count")
        }
    }

    /// Grouped saves wait for a barrier: K2's flush (an explicit save, a
    /// tab's close or switch-away, all through `flush_saves`) puts every
    /// change on disk and is counted by its reason.
    #[test]
    fn a_flush_puts_every_grouped_change_on_disk() {
        let g = Grouped::new();
        g.edit("two\n");
        let (made, on_disk) = g.counts();
        assert!(on_disk < made, "a grouped change waits for its group: {on_disk} of {made} on disk");
        for why in ["save", "close", "switch_away"] {
            let before = g.flushes(why);
            g.edit(&format!("{why}\n"));
            assert!(g.counts().1 < g.counts().0, "{why}: the change waits");
            assert!(g.rt.flush_saves(why), "{why}: the flush succeeds");
            let (made, on_disk) = g.counts();
            assert_eq!(on_disk, made, "{why}: every change is on disk after the flush");
            assert_eq!(g.flushes(why), before + 1, "{why}: counted");
        }
        assert!(g.rt.flush_saves("save"), "a flush with nothing waiting is a no-op that succeeds");
    }

    /// The idle barrier: a tick that finds commits waiting and none since the
    /// last tick flushes them; a tick right after a change doesn't.
    #[test]
    fn an_idle_tick_flushes_and_a_busy_one_waits() {
        let g = Grouped::new();
        g.edit("two\n");
        let idle_before = g.flushes("idle");
        assert!(!g.rt.flush_if_idle(), "a change since the last tick: not idle yet");
        assert!(g.counts().1 < g.counts().0, "still waiting");
        assert!(g.rt.flush_if_idle(), "no change since the last tick: idle, so it flushes");
        let (made, on_disk) = g.counts();
        assert_eq!(on_disk, made, "every change on disk once idle");
        assert_eq!(g.flushes("idle"), idle_before + 1);
        assert!(!g.rt.flush_if_idle(), "nothing waiting: no flush");
        g.rt.tick();
        g.edit("three\n");
        g.rt.tick();
        assert!(g.counts().1 < g.counts().0, "the tick right after a change leaves it waiting (the kit's group hasn't timed out)");
        g.rt.tick();
        let (made, on_disk) = g.counts();
        assert_eq!(on_disk, made, "the next quiet tick puts it on disk");
    }

    #[test]
    fn a_write_body_fits_a_whole_file_at_the_cap_as_base64() {
        let at_cap = (BLOB_MAX_BYTES as usize).div_ceil(3) * 4;
        assert!(routes::max_body(routes::WRITE) > at_cap + 1024, "room for the JSON around it");
        assert!(routes::max_body("/cli/docs/write?token=x") > at_cap, "the query string is ignored");
        assert_eq!(routes::max_body(routes::EDIT), routes::MAX_BODY);
        assert_eq!(routes::max_body(routes::LEASE), routes::MAX_BODY);
    }
}
