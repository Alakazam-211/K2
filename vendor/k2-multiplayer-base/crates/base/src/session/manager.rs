//! The pool of loaded sessions: open, find, rename, unload within a memory budget.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use super::config::Config;
use super::disk::{self, DiskCheck, Probe, ReadFile};
use super::error::{Result, SessionError};
use super::host::Host;
use super::live::{probe_error, Session};
use super::pack::{Declined, Pack};
use super::participant::ConnId;
use crate::store::{DocKey, DocStore, Seq};

/// Returned by [`SessionManager::stats`] (for the host's doctor/status).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct PoolStats {
    pub loaded: usize,
    /// Sum of the loaded sessions' estimates.
    pub estimated_bytes: u64,
    pub budget_bytes: u64,
    /// Sessions unloaded since the manager started (idle or over budget).
    pub unloaded_total: u64,
}

/// Returned by [`SessionManager::tick`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct TickReport {
    /// Connections closed for being idle (as [`SessionManager::expire_idle`]).
    pub left: Vec<(DocKey, ConnId, String)>,
    /// Docs whose file was written back.
    pub written: Vec<DocKey>,
    /// Docs that merged an outside edit found by the stat backup, at that version.
    pub ingested: Vec<(DocKey, Seq)>,
    /// Sessions unloaded (as [`SessionManager::unload_idle`]).
    pub unloaded: usize,
    /// Per-doc failures (a disk error: write-back retries on the next tick).
    pub errors: Vec<(DocKey, SessionError)>,
}

struct Pool {
    store: Option<Arc<DocStore>>,
    sessions: HashMap<DocKey, Arc<Session>>,
    unloaded_total: u64,
}

/// Owns the loaded sessions of one store. One per process; thread-safe.
///
/// A session is **unloadable** when it has no connections and nobody else holds its
/// `Arc` (only the pool does). Unloading never loses anything: every change is already
/// durable. Unloadable sessions go when idle for [`Config::idle_unload_ms`]
/// ([`SessionManager::unload_idle`]) and, least recently used first, whenever the pool's
/// estimate is over [`Config::memory_budget_bytes`] (checked on every open).
///
/// Lock order: the pool, then a session. Opening holds the pool lock while it loads.
pub struct SessionManager {
    host: Arc<dyn Host>,
    config: Arc<Config>,
    pool: Mutex<Pool>,
}

impl SessionManager {
    pub fn new(store: Arc<DocStore>, host: Arc<dyn Host>, config: Config) -> Self {
        Self {
            host,
            config: Arc::new(config),
            pool: Mutex::new(Pool {
                store: Some(store),
                sessions: HashMap::new(),
                unloaded_total: 0,
            }),
        }
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    fn pool(&self) -> Result<MutexGuard<'_, Pool>> {
        self.pool
            .lock()
            .map_err(|_| SessionError::Broken("session pool lock poisoned by a panic".into()))
    }

    /// The session for `key`: already loaded, loaded from the store, or (when the store has
    /// no doc there) created from `initial_bytes`, decoded by `pack` and imported as
    /// [`super::IMPORT_PID`] with an import version. Errors: [`SessionError::Declined`] for
    /// a file the pack declines (with the reason), [`SessionError::NotFound`] when there is
    /// neither a stored doc nor bytes, [`SessionError::PackMismatch`] when the stored doc
    /// belongs to another pack.
    pub fn open(
        &self,
        pack: Arc<dyn Pack>,
        key: &DocKey,
        initial_bytes: Option<&[u8]>,
    ) -> Result<Arc<Session>> {
        let mut pool = self.pool()?;
        let store = pool.store.clone().ok_or(SessionError::Released)?;
        if let Some(s) = pool.sessions.get(key) {
            let name = pack.schema().name;
            if s.schema_name() != name {
                return Err(SessionError::PackMismatch {
                    stored: s.schema_name().to_owned(),
                    pack: name.to_owned(),
                });
            }
            return Ok(s.clone());
        }
        let session = match store.find(key)? {
            Some(id) => Session::load(self.host.clone(), self.config.clone(), pack, store, id)?,
            None => match initial_bytes {
                Some(b) => Session::import(
                    self.host.clone(),
                    self.config.clone(),
                    pack,
                    store,
                    key.clone(),
                    b,
                )?,
                None => return Err(SessionError::NotFound(key.clone())),
            },
        };
        let s = Arc::new(session);
        pool.sessions.insert(key.clone(), s.clone());
        self.enforce_budget(&mut pool, Some(key));
        Ok(s)
    }

    /// Like [`SessionManager::open`], for a doc that lives in a file, which the session then
    /// keeps in sync (see `docs/SESSIONS.md`, "Disk sync"). The host resolves and jails
    /// `path`; the kit checks again that it is a regular file (not a folder or a device)
    /// that resolves, symlinks included, inside `root`, and refuses it otherwise
    /// ([`SessionError::BadPath`]).
    ///
    /// - A doc not stored yet is imported from the file (it must exist).
    /// - A stored doc is loaded and reconciled: an edit made to the file while the doc
    ///   wasn't loaded is merged in as an outside edit, and changes the file never got (a
    ///   crash before write-back) are written soon. A file that is gone opens `Missing`.
    /// - Opening and closing an untouched file never writes it.
    pub fn open_file(
        &self,
        pack: Arc<dyn Pack>,
        key: &DocKey,
        path: &Path,
        root: &Path,
    ) -> Result<Arc<Session>> {
        let root = std::fs::canonicalize(root)
            .ok()
            .filter(|r| r.is_dir())
            .ok_or_else(|| {
                SessionError::BadPath(format!("root {} is not a folder", root.display()))
            })?;
        let probed = disk::probe(path, &root)
            .map_err(|e| SessionError::Disk(format!("{}: {e}", path.display())))?;
        if matches!(probed, Probe::NotFile | Probe::Outside(_)) {
            return Err(probe_error(path, &probed));
        }
        let s = {
            let pool = self.pool()?;
            let store = pool.store.clone().ok_or(SessionError::Released)?;
            let loaded = pool.sessions.contains_key(key) || store.find(key)?.is_some();
            drop(pool);
            if loaded {
                self.open(pack, key, None)?
            } else {
                let Probe::File { real, .. } = &probed else {
                    return Err(SessionError::NotFound(key.clone()));
                };
                let bytes = match disk::read(real, self.config.disk_read_max_bytes) {
                    Ok(ReadFile::Bytes(b, _)) => b,
                    Ok(ReadFile::TooLarge(n)) => {
                        return Err(SessionError::Declined(Declined {
                            code: "too_large",
                            message: format!(
                                "file is {n} bytes; live editing reads up to {}",
                                self.config.disk_read_max_bytes
                            ),
                        }))
                    }
                    Err(e) => return Err(SessionError::Disk(format!("{}: {e}", real.display()))),
                };
                self.open(pack, key, Some(&bytes))?
            }
        };
        // outside the pool lock (file I/O); our handle keeps the session from unloading
        s.attach_file(path, &root)?;
        Ok(s)
    }

    /// The host's clock tick (every second or so): closes idle connections, runs the disk
    /// stat backup and due write-backs, and unloads idle sessions (writing back first).
    /// Time is [`super::Host::now_ms`], like every other timer in the kit.
    pub fn tick(&self) -> Result<TickReport> {
        let mut rep = TickReport {
            left: self.expire_idle()?,
            ..TickReport::default()
        };
        for (k, s) in self.loaded()? {
            match s.disk_tick() {
                Ok((check, written)) => {
                    if let Some(DiskCheck::Ingested { version }) = check {
                        rep.ingested.push((k.clone(), version));
                    }
                    if written {
                        rep.written.push(k);
                    }
                }
                Err(SessionError::Unloaded | SessionError::Released) => {}
                Err(e) => rep.errors.push((k, e)),
            }
        }
        rep.unloaded = self.unload_idle()?;
        Ok(rep)
    }

    /// Run the stat check ([`Session::check_disk`]) on every loaded session with a file now,
    /// whatever the poll interval. Returns what each found.
    pub fn poll_disk(&self) -> Result<Vec<(DocKey, Result<DiskCheck>)>> {
        let mut out = Vec::new();
        for (k, s) in self.loaded()? {
            if matches!(s.file_path(), Ok(Some(_))) {
                out.push((k, s.check_disk()));
            }
        }
        Ok(out)
    }

    fn loaded(&self) -> Result<Vec<(DocKey, Arc<Session>)>> {
        Ok(self
            .pool()?
            .sessions
            .iter()
            .map(|(k, s)| (k.clone(), s.clone()))
            .collect())
    }

    /// The loaded session for `key`, if any (never loads).
    pub fn get(&self, key: &DocKey) -> Option<Arc<Session>> {
        self.pool().ok()?.sessions.get(key).cloned()
    }

    /// Rename a doc (`mv`): moves its store record, and the loaded session if there is one.
    pub fn rekey(&self, old: &DocKey, new: DocKey) -> Result<()> {
        let mut pool = self.pool()?;
        let store = pool.store.clone().ok_or(SessionError::Released)?;
        if let Some(s) = pool.sessions.get(old).cloned() {
            s.rekey(new.clone())?;
            pool.sessions.remove(old);
            pool.sessions.insert(new, s);
            return Ok(());
        }
        let id = store
            .find(old)?
            .ok_or_else(|| SessionError::NotFound(old.clone()))?;
        store.rekey(id, new)?;
        Ok(())
    }

    /// Close idle connections in every loaded session ([`Session::expire_idle`]).
    /// Returns the connections that left, with doc and pid. Driven by the host's clock.
    pub fn expire_idle(&self) -> Result<Vec<(DocKey, ConnId, String)>> {
        let sessions: Vec<(DocKey, Arc<Session>)> = self
            .pool()?
            .sessions
            .iter()
            .map(|(k, s)| (k.clone(), s.clone()))
            .collect();
        let mut out = Vec::new();
        for (k, s) in sessions {
            match s.expire_idle() {
                Ok(gone) => out.extend(gone.into_iter().map(|(c, p)| (k.clone(), c, p))),
                Err(SessionError::Unloaded | SessionError::Released) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(out)
    }

    /// Unload sessions unused for [`Config::idle_unload_ms`] that are unloadable (no
    /// participants, no outside handle). Compacts a long log first. Returns how many went.
    pub fn unload_idle(&self) -> Result<usize> {
        let now = self.host.now_ms();
        let mut pool = self.pool()?;
        let idle: Vec<DocKey> = pool
            .sessions
            .iter()
            .filter(|(_, s)| {
                Self::unloadable(s)
                    && now.saturating_sub(s.last_used_ms()) >= self.config.idle_unload_ms
            })
            .map(|(k, _)| k.clone())
            .collect();
        for k in &idle {
            self.unload(&mut pool, k);
        }
        Ok(idle.len())
    }

    /// Pool totals, for the host's doctor/status.
    pub fn stats(&self) -> Result<PoolStats> {
        let pool = self.pool()?;
        Ok(PoolStats {
            loaded: pool.sessions.len(),
            estimated_bytes: pool
                .sessions
                .values()
                .filter_map(|s| s.pool_info())
                .map(|(_, b)| b)
                .sum(),
            budget_bytes: self.config.memory_budget_bytes,
            unloaded_total: pool.unloaded_total,
        })
    }

    /// Process hand-off: write back every file, close every session (handles the host
    /// still holds fail with [`SessionError::Released`]) and drop the kit's store handles. Once the host drops its
    /// own `Arc<DocStore>`, the store file is unlocked for the next process.
    pub fn release(&self) -> Result<()> {
        let mut pool = self.pool()?;
        for (_, s) in pool.sessions.drain() {
            s.close(SessionError::Released, false);
        }
        pool.store = None;
        Ok(())
    }

    fn unloadable(s: &Arc<Session>) -> bool {
        // only the pool holds it, and nobody is in it
        Arc::strong_count(s) == 1 && s.pool_info().is_some_and(|(n, _)| n == 0)
    }

    fn unload(&self, pool: &mut Pool, key: &DocKey) {
        if let Some(s) = pool.sessions.remove(key) {
            s.close(SessionError::Unloaded, true);
            pool.unloaded_total += 1;
        }
    }

    /// While over budget, unload the least recently used unloadable session (never `keep`).
    fn enforce_budget(&self, pool: &mut Pool, keep: Option<&DocKey>) {
        loop {
            let total: u64 = pool
                .sessions
                .values()
                .filter_map(|s| s.pool_info())
                .map(|(_, b)| b)
                .sum();
            if total <= self.config.memory_budget_bytes {
                return;
            }
            let victim = pool
                .sessions
                .iter()
                .filter(|(k, s)| Some(*k) != keep && Self::unloadable(s))
                .min_by_key(|(_, s)| s.last_used_ms())
                .map(|(k, _)| k.clone());
            match victim {
                Some(k) => self.unload(pool, &k),
                None => return, // over budget, but everything loaded is in use
            }
        }
    }
}
