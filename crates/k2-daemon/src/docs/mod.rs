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
//! | [`routes`] | `/cli/docs/*` (K2) |
//!
//! **Disk sync** (kit C4): every doc is opened with `open_file`, so the file
//! on disk stays true. The kit writes changes back (1 s quiet, 5 s at most,
//! atomic, echo-suppressed), merges outside edits as `kit:external`, and
//! reconciles a reopened doc with edits made while it was closed. K2 calls
//! `tick()` every 250 ms and feeds `file_changed()` from its watcher.

pub mod caller;
pub mod host;
pub mod routes;
pub mod shares;
pub mod target;
pub mod watch;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Duration;

use k2_core::log_debug;
use k2_mpc_base::session::{Config, PoolStats, Session, SessionError, SessionManager, TextFilePack};
use k2_mpc_base::store::{DocKey, DocStore};

/// The vendored kit writes docs back to their files (its C4, base
/// `e7c1fd6`). `docs-md-v1` is advertised while the runtime is up.
pub const KIT_DISK_SYNC: bool = true;

/// Env override for the memory budget, in MiB.
pub const ENV_MEMORY_MIB: &str = "K2_DOCS_MEMORY_MIB";

/// How long a starting daemon waits for the previous one to let go of the
/// store (hand-off).
const OPEN_WAIT: Duration = Duration::from_secs(5);

/// The kit's timer: due write-backs, stat checks, idle expiry and unload.
pub const TICK_EVERY: Duration = Duration::from_millis(250);

/// Rights re-check of every open connection (PRD K-PERM-12: within 5 s).
const RECHECK_EVERY_TICKS: u64 = 8;

/// Orphan-share sweep (about a minute; the first runs on the first tick).
const SWEEP_EVERY_TICKS: u64 = 240;

/// The Markdown pack: one text root over the file's source, 4 MiB.
pub static MARKDOWN: TextFilePack = TextFilePack::new("markdown", k2_mpc_markdown::MAX_FILE_BYTES);

/// The loaded runtime.
pub struct Runtime {
    manager: SessionManager,
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
    Failed(String),
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
    Failed(String),
    Released,
}

impl Unavailable {
    pub fn message(&self) -> String {
        match self {
            Unavailable::NotStarted => "live docs are still starting".to_string(),
            Unavailable::Failed(e) => format!("live docs are off: the docs store failed to open ({e})"),
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

fn config() -> Config {
    let mut c = Config::default();
    c.memory_budget_bytes = memory_budget_bytes();
    c
}

/// Open the store and start the session manager, its timer and the
/// watcher. Called once at boot, on its own thread (the open waits for a
/// handing-off predecessor).
pub fn start() {
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
    let opened = DocStore::open_with_retry(&path, OPEN_WAIT);
    let mut s = slot().write().expect("docs slot");
    match opened {
        Ok(store) => {
            let store = Arc::new(store);
            let rt = Runtime {
                manager: SessionManager::new(
                    store.clone(),
                    Arc::new(host::K2Host::new(Arc::downgrade(&store))),
                    config(),
                ),
                store: Mutex::new(Some(store)),
                opened: Mutex::new(HashMap::new()),
                watch: watch::Watch::start(),
            };
            log_debug!(
                "[docs] store open at {} (budget {} MiB, disk sync on)",
                path.display(),
                rt.manager.config().memory_budget_bytes >> 20,
            );
            *s = Slot::Running(Arc::new(rt));
            drop(s);
            spawn_timer();
            watch::spawn_fs_changed_listener();
        }
        Err(e) => {
            log_debug!("[docs] WARN: docs store at {} failed to open: {e}", path.display());
            *s = Slot::Failed(e.to_string());
        }
    }
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

/// Host-table namespace of the workspaces (spaces) K2 has opened docs in, so
/// the sweep can delete a removed workspace's docs (`delete_space`).
const SPACES_NS: &str = "k2.docs.spaces";

impl Runtime {
    /// The session for `key`, syncing with the file at `abs` inside `root`
    /// (`open_file`: import, or reconcile a stored doc with the file).
    pub fn open(&self, key: &DocKey, abs: &Path, root: &Path) -> Result<Arc<Session>, SessionError> {
        let s = self.manager.open_file(Arc::new(MARKDOWN), key, abs, root)?;
        let rel_in_k2 = key.path.starts_with(".k2/");
        let fresh = self.opened.lock().expect("docs opened set").insert(key.clone(), abs.to_path_buf()).is_none();
        if fresh {
            if rel_in_k2 {
                self.watch.watch_k2_file(abs);
            }
            self.note_space(&key.space);
        }
        Ok(s)
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
                    if let Some(store) = self.store() {
                        let _ = store.host_delete(SPACES_NS, &space);
                    }
                    log_debug!("[docs] removed {} doc(s) of removed workspace {space}", keys.len());
                    host::audit("doc-space-removed", "k2", format!("space={space} docs={} reason=workspace_removed", keys.len()));
                }
                Err(e) => log_debug!("[docs] WARN: removing the docs of {space}: {e}"),
            }
        }
    }
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
            }),
            Err(e) => serde_json::json!({ "running": false, "error": e.to_string() }),
        },
        Err(u) => serde_json::json!({ "running": false, "error": u.message() }),
    }
}
