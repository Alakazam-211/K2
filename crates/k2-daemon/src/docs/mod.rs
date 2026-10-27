//! Live docs: Markdown multiplayer v1 on the vendored multiplayer kit
//! (`vendor/k2-multiplayer-base`, `vendor/k2-multiplayer-markdown`).
//!
//! The kit owns the docs, their history, presence and merge rules; K2 owns
//! identity, permission, time, transport and files (`docs/SESSIONS.md` in
//! the kit). This module is K2's side:
//!
//! | File | What |
//! |---|---|
//! | `mod.rs` | the runtime: the store (`~/.k2/docs.redb`), the session manager, the 2 s upkeep, release on hand-off and stop, pool stats (K1) |
//! | [`host`] | K2 as the kit's `Host`: rights per pid, the clock, the audit sink (K2) |
//! | [`caller`] | who is calling, and the participant K2 stamps for them (K2) |
//! | [`target`] | workspace + path → a jailed doc key, and the deny list (K2) |
//! | [`routes`] | `/cli/docs/*` (K2) |
//!
//! **Disk sync isn't in the kit yet** (its C4). Until it lands, nothing a
//! session changes reaches the file, so edit verbs answer
//! `409 disk_sync_pending` unless the daemon runs with
//! `K2_DOCS_UNSYNCED_EDITS=1` (tests and headless smokes), and the
//! `docs-md-v1` feature key stays off ([`live_ready`]).

pub mod caller;
pub mod host;
pub mod routes;
pub mod target;

use std::collections::HashSet;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Duration;

use k2_core::log_debug;
use k2_multiplayer_base::session::{Config, PoolStats, Session, SessionError, SessionManager, TextFilePack};
use k2_multiplayer_base::store::{DocKey, DocStore};

/// Whether the vendored kit writes docs back to their files (its C4). Flip
/// with the kit bump that brings `Session::file_changed` and write-back.
pub const KIT_DISK_SYNC: bool = false;

/// Daemon env that lets edit verbs run without disk sync (tests, smokes).
pub const ENV_UNSYNCED_EDITS: &str = "K2_DOCS_UNSYNCED_EDITS";

/// Env override for the memory budget, in MiB.
pub const ENV_MEMORY_MIB: &str = "K2_DOCS_MEMORY_MIB";

/// How long a starting daemon waits for the previous one to let go of the
/// store (hand-off).
const OPEN_WAIT: Duration = Duration::from_secs(5);

/// Upkeep period: idle connections leave, idle sessions unload, open
/// connections' rights are re-checked (PRD K-PERM-12: within 5 s).
pub const UPKEEP_EVERY: Duration = Duration::from_secs(2);

/// The Markdown pack: one text root over the file's source, 4 MiB.
pub static MARKDOWN: TextFilePack = TextFilePack::new("markdown", k2_multiplayer_markdown::MAX_FILE_BYTES);

/// The loaded runtime.
pub struct Runtime {
    manager: SessionManager,
    /// K2's own handle on the store (for status); dropped at release so the
    /// file lock goes with the kit's.
    store: Mutex<Option<Arc<DocStore>>>,
    /// Docs opened in this process, for the rights re-check (the manager
    /// doesn't list its sessions; unloaded keys drop out at the next pass).
    opened: Mutex<HashSet<DocKey>>,
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

/// Open the store and start the session manager. Called once at boot, on
/// a blocking thread (the open waits for a handing-off predecessor).
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
                manager: SessionManager::new(store.clone(), Arc::new(host::K2Host), config()),
                store: Mutex::new(Some(store)),
                opened: Mutex::new(HashSet::new()),
            };
            log_debug!(
                "[docs] store open at {} (budget {} MiB, disk sync {})",
                path.display(),
                rt.manager.config().memory_budget_bytes >> 20,
                if KIT_DISK_SYNC { "on" } else { "not yet" }
            );
            *s = Slot::Running(Arc::new(rt));
            drop(s);
            spawn_upkeep();
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

/// Edit verbs may run: disk sync is in, or the daemon was started with
/// `K2_DOCS_UNSYNCED_EDITS=1`.
pub fn edits_allowed() -> bool {
    KIT_DISK_SYNC || std::env::var(ENV_UNSYNCED_EDITS).is_ok_and(|v| v.trim() == "1")
}

/// Hand-off (quiesce) and normal stop: close every session and let go of
/// the store file so the next daemon can open it. One-way: later calls get
/// [`Unavailable::Released`]. Every change is already durable (the kit
/// commits before it acks); when the kit's disk sync lands, its flush runs
/// here first. Returns whether the store was open.
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
    log_debug!("[docs] released the docs store ({why})");
    true
}

/// The 2 s upkeep thread (exits once released).
fn spawn_upkeep() {
    let _ = std::thread::Builder::new().name("k2-docs-upkeep".into()).spawn(|| loop {
        std::thread::sleep(UPKEEP_EVERY);
        match runtime() {
            Ok(rt) => rt.upkeep(),
            Err(Unavailable::Released) => return,
            Err(_) => {}
        }
    });
}

impl Runtime {
    /// The session for `key`, importing `bytes` if the store has no doc.
    pub fn open(&self, key: &DocKey, bytes: Option<&[u8]>) -> Result<Arc<Session>, SessionError> {
        let s = self.manager.open(Arc::new(MARKDOWN), key, bytes)?;
        self.opened.lock().expect("docs opened set").insert(key.clone());
        Ok(s)
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

    /// Idle connections leave, idle sessions unload, and every loaded
    /// doc's participants have their rights re-checked (losing read
    /// removes them; the kit audits it through the host).
    fn upkeep(&self) {
        if let Err(e) = self.manager.expire_idle() {
            log_debug!("[docs] WARN: expire_idle: {e}");
        }
        let keys: Vec<DocKey> = self.opened.lock().expect("docs opened set").iter().cloned().collect();
        let mut gone = Vec::new();
        for k in keys {
            match self.manager.get(&k) {
                Some(s) => match s.recheck_rights() {
                    Ok(changed) => {
                        for c in changed {
                            log_debug!("[docs] rights of {} on {} now {:?}", c.pid, host::key_label(&k), c.rights);
                        }
                    }
                    Err(SessionError::Unloaded | SessionError::Released) => gone.push(k),
                    Err(e) => log_debug!("[docs] WARN: recheck_rights on {}: {e}", host::key_label(&k)),
                },
                None => gone.push(k),
            }
        }
        if !gone.is_empty() {
            let mut set = self.opened.lock().expect("docs opened set");
            for k in gone {
                set.remove(&k);
            }
        }
        if let Err(e) = self.manager.unload_idle() {
            log_debug!("[docs] WARN: unload_idle: {e}");
        }
    }

    /// After a rename (`mv`), track the new key.
    pub fn rekeyed(&self, old: &DocKey, new: &DocKey) {
        let mut set = self.opened.lock().expect("docs opened set");
        set.remove(old);
        set.insert(new.clone());
    }
}

/// Pool stats for status and doctor: `{loaded, estimatedBytes, budgetBytes,
/// unloadedTotal, diskSync, editsAllowed}`, or why the runtime is down.
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
                "editsAllowed": edits_allowed(),
            }),
            Err(e) => serde_json::json!({ "running": false, "error": e.to_string() }),
        },
        Err(u) => serde_json::json!({ "running": false, "error": u.message() }),
    }
}
