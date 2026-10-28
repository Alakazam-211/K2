//! The pool of loaded sessions: open, find, rename, unload within a memory budget.

use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use super::config::Config;
use super::disk::{self, DiskCheck, Probe, ReadFile};
use super::error::{Result, SessionError};
use super::host::{AuditEvent, Host};
use super::live::{probe_error, RightsChange, Session};
use super::pack::{Declined, Pack};
use super::participant::ConnId;
use super::tier0::{BlobPack, BlobSession};
use crate::blob::{BlobError, BlobInfo, BlobStore};
use crate::store::{DocKey, DocStore, Seq, VersionId, BLOB_DOC_PACK};

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
    /// Tier 0: loaded blob docs (not in `loaded`; a few KiB each, outside the budget).
    pub blob_sessions: usize,
    /// Tier 0: blobs and their bytes in the blob store (0 without one), and the bytes no
    /// version references (collectable after the grace period).
    pub blob_count: u64,
    pub blob_bytes: u64,
    pub blob_unreferenced_bytes: u64,
}

/// Returned by [`SessionManager::tick`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct TickReport {
    /// Connections closed for being idle (as [`SessionManager::expire_idle`]).
    pub left: Vec<(DocKey, ConnId, String)>,
    /// Docs whose file was written back.
    pub written: Vec<DocKey>,
    /// Docs that merged an outside edit found by the stat backup, at that version (for a
    /// tier-0 blob doc: the version the save, or its merge, became).
    pub ingested: Vec<(DocKey, Seq)>,
    /// Sessions unloaded (as [`SessionManager::unload_idle`]).
    pub unloaded: usize,
    /// Per-doc failures (a disk error: write-back retries on the next tick).
    pub errors: Vec<(DocKey, SessionError)>,
    /// Tier 0: leases that ran out, with their holder.
    pub leases_expired: Vec<(DocKey, String)>,
    /// Tier 0: outside saves that conflicted (doc, the new current version, the conflict
    /// copy written, if any).
    pub conflicts: Vec<(DocKey, Seq, Option<PathBuf>)>,
    /// Versions cut after quiet following edits (K-HIST-4).
    pub versions_cut: Vec<(DocKey, VersionId)>,
    /// Versions removed by retention (K-HIST-7), over every doc.
    pub versions_thinned: usize,
    /// Revert records that left the revert window (K-HIST-23), over every doc.
    pub reverts_dropped: u64,
}

struct Pool {
    store: Option<Arc<DocStore>>,
    sessions: HashMap<DocKey, Arc<Session>>,
    blob_sessions: HashMap<DocKey, Arc<BlobSession>>,
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
    blobs: Option<Arc<BlobStore>>,
}

impl SessionManager {
    pub fn new(store: Arc<DocStore>, host: Arc<dyn Host>, config: Config) -> Self {
        Self {
            host,
            config: Arc::new(config),
            pool: Mutex::new(Pool {
                store: Some(store),
                sessions: HashMap::new(),
                blob_sessions: HashMap::new(),
                unloaded_total: 0,
            }),
            blobs: None,
        }
    }

    /// Tier 0: serve blob docs from `blobs` (whose index must be in this manager's store).
    pub fn with_blob_store(mut self, blobs: Arc<BlobStore>) -> Self {
        self.blobs = Some(blobs);
        self
    }

    /// The blob store, if one was given.
    pub fn blob_store(&self) -> Option<&Arc<BlobStore>> {
        self.blobs.as_ref()
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
        if pool.blob_sessions.contains_key(key) {
            return Err(SessionError::PackMismatch {
                stored: BLOB_DOC_PACK.to_owned(),
                pack: pack.schema().name.to_owned(),
            });
        }
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
            match s.history_tick() {
                Ok(h) => {
                    if let Some(v) = h.cut {
                        rep.versions_cut.push((k.clone(), v));
                    }
                    rep.versions_thinned += h.thinned;
                    rep.reverts_dropped += h.reverts_dropped;
                }
                Err(SessionError::Unloaded | SessionError::Released) => {}
                Err(e) => rep.errors.push((k.clone(), e)),
            }
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
        for (k, s) in self.loaded_blobs()? {
            match s.tick() {
                Ok((check, written, expired)) => {
                    match check {
                        Some(DiskCheck::Ingested { version } | DiskCheck::Merged { version }) => {
                            rep.ingested.push((k.clone(), version))
                        }
                        Some(DiskCheck::Conflict { version, copy, .. }) => {
                            rep.ingested.push((k.clone(), version));
                            rep.conflicts.push((k.clone(), version, copy));
                        }
                        _ => {}
                    }
                    if written {
                        rep.written.push(k.clone());
                    }
                    if let Some(pid) = expired {
                        rep.leases_expired.push((k, pid));
                    }
                }
                Err(SessionError::Unloaded | SessionError::Released) => {}
                Err(e) => rep.errors.push((k, e)),
            }
        }
        rep.unloaded = self.unload_idle()?;
        Ok(rep)
    }

    fn loaded_blobs(&self) -> Result<Vec<(DocKey, Arc<BlobSession>)>> {
        Ok(self
            .pool()?
            .blob_sessions
            .iter()
            .map(|(k, s)| (k.clone(), s.clone()))
            .collect())
    }

    // ----- tier 0: blob docs -----

    fn blobs(&self) -> Result<Arc<BlobStore>> {
        self.blobs.clone().ok_or(SessionError::NoBlobStore)
    }

    /// Tier 0: the blob doc at `key`, already loaded, loaded from the store, or (when the
    /// store has none) created from the bytes `initial` yields, streamed into the blob store
    /// (outside the pool lock) and imported as [`super::IMPORT_PID`]. Errors as
    /// [`SessionManager::open`]; a text doc at `key` is [`SessionError::PackMismatch`];
    /// [`SessionError::NoBlobStore`] without [`SessionManager::with_blob_store`].
    pub fn open_blob(
        &self,
        pack: Arc<dyn BlobPack>,
        key: &DocKey,
        initial: Option<&mut dyn Read>,
    ) -> Result<Arc<BlobSession>> {
        let blobs = self.blobs()?;
        if let Some(s) = self.loaded_or_stored_blob(&pack, key)? {
            return Ok(s);
        }
        let src = initial.ok_or_else(|| SessionError::NotFound(key.clone()))?;
        let info = blobs.put(&key.space, src)?;
        self.import_blob(pack, key, info, None)
    }

    fn loaded_or_stored_blob(
        &self,
        pack: &Arc<dyn BlobPack>,
        key: &DocKey,
    ) -> Result<Option<Arc<BlobSession>>> {
        let blobs = self.blobs()?;
        let mut pool = self.pool()?;
        let store = pool.store.clone().ok_or(SessionError::Released)?;
        if let Some(s) = pool.blob_sessions.get(key) {
            return Ok(Some(s.clone()));
        }
        if let Some(s) = pool.sessions.get(key) {
            return Err(SessionError::PackMismatch {
                stored: s.schema_name().to_owned(),
                pack: BLOB_DOC_PACK.to_owned(),
            });
        }
        let Some(id) = store.find(key)? else {
            return Ok(None);
        };
        let s = Arc::new(BlobSession::load(
            self.host.clone(),
            self.config.clone(),
            blobs,
            pack.clone(),
            store,
            id,
        )?);
        pool.blob_sessions.insert(key.clone(), s.clone());
        Ok(Some(s))
    }

    fn import_blob(
        &self,
        pack: Arc<dyn BlobPack>,
        key: &DocKey,
        info: BlobInfo,
        from_file: Option<super::disk::FileStat>,
    ) -> Result<Arc<BlobSession>> {
        // someone may have created it while we uploaded: theirs wins, ours is collected
        if let Some(s) = self.loaded_or_stored_blob(&pack, key)? {
            return Ok(s);
        }
        let mut pool = self.pool()?;
        let store = pool.store.clone().ok_or(SessionError::Released)?;
        let s = Arc::new(BlobSession::import(
            self.host.clone(),
            self.config.clone(),
            self.blobs()?,
            pack,
            store,
            key.clone(),
            info,
            from_file,
        )?);
        pool.blob_sessions.insert(key.clone(), s.clone());
        Ok(s)
    }

    /// Tier 0: like [`SessionManager::open_blob`] for a file the session then keeps in sync
    /// (`docs/SESSIONS.md`, "Tier 0"). Path rules as [`SessionManager::open_file`]. A new
    /// doc is imported from the file (streamed; a file over the blob limit is
    /// `Declined { code: "too_large" }`); a stored doc is reconciled with the file.
    pub fn open_blob_file(
        &self,
        pack: Arc<dyn BlobPack>,
        key: &DocKey,
        path: &Path,
        root: &Path,
    ) -> Result<Arc<BlobSession>> {
        let blobs = self.blobs()?;
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
        let s = match self.loaded_or_stored_blob(&pack, key)? {
            Some(s) => s,
            None => {
                let Probe::File { real, .. } = &probed else {
                    return Err(SessionError::NotFound(key.clone()));
                };
                let disk_err =
                    |e: std::io::Error| SessionError::Disk(format!("{}: {e}", real.display()));
                let mut f = File::open(real).map_err(disk_err)?;
                let stat = disk::FileStat::of(&f.metadata().map_err(disk_err)?);
                let info = match blobs.put(&key.space, &mut f) {
                    Ok(i) => i,
                    Err(BlobError::TooLarge { limit }) => {
                        return Err(SessionError::Declined(Declined {
                            code: "too_large",
                            message: format!("file is over the {limit}-byte blob limit"),
                        }))
                    }
                    Err(e) => return Err(e.into()),
                };
                self.import_blob(pack, key, info, Some(stat))?
            }
        };
        s.attach_file(path, &root)?;
        Ok(s)
    }

    /// The loaded blob doc for `key`, if any (never loads).
    pub fn get_blob(&self, key: &DocKey) -> Option<Arc<BlobSession>> {
        self.pool().ok()?.blob_sessions.get(key).cloned()
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
        if let Some(s) = pool.blob_sessions.get(old).cloned() {
            s.rekey(new.clone())?;
            pool.blob_sessions.remove(old);
            pool.blob_sessions.insert(new, s);
            return Ok(());
        }
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

    /// The keys of every loaded doc, text and tier 0, sorted by space then path. Never
    /// loads anything. (Empty if the pool is broken, as after a panic inside it.)
    pub fn loaded_keys(&self) -> Vec<DocKey> {
        let Ok(pool) = self.pool() else {
            return Vec::new();
        };
        let mut keys: Vec<DocKey> = pool
            .sessions
            .keys()
            .chain(pool.blob_sessions.keys())
            .cloned()
            .collect();
        keys.sort_by(|a, b| (&a.space, &a.path).cmp(&(&b.space, &b.path)));
        keys
    }

    /// The host's periodic rights re-check (PRD §11.2) over every loaded doc, text and tier
    /// 0: each session's `recheck_rights` ([`Session::recheck_rights`],
    /// [`BlobSession::recheck_rights`]), so participants who lost read leave. Returns each
    /// pid whose rights changed, with its doc, sorted by doc. A session that fails is
    /// skipped and reported as [`AuditEvent::RecheckFailed`]; one that closed meanwhile
    /// (unloaded, released, deleted) is simply skipped. Never loads anything.
    pub fn recheck_rights(&self) -> Vec<(DocKey, RightsChange)> {
        let (text, blobs) = match (self.loaded(), self.loaded_blobs()) {
            (Ok(t), Ok(b)) => (t, b),
            _ => return Vec::new(),
        };
        let mut done: Vec<(DocKey, Result<Vec<RightsChange>>)> = text
            .into_iter()
            .map(|(k, s)| (k, s.recheck_rights()))
            .chain(blobs.into_iter().map(|(k, s)| (k, s.recheck_rights())))
            .collect();
        done.sort_by(|a, b| (&a.0.space, &a.0.path).cmp(&(&b.0.space, &b.0.path)));
        let mut out = Vec::new();
        for (key, r) in done {
            match r {
                Ok(changes) => out.extend(changes.into_iter().map(|c| (key.clone(), c))),
                Err(SessionError::Unloaded | SessionError::Released | SessionError::Deleted) => {}
                Err(e) => self.host.on_event(&AuditEvent::RecheckFailed {
                    key,
                    error: e.to_string(),
                }),
            }
        }
        out
    }

    /// Delete the doc at `key` (text or tier 0) for good: a loaded session is closed first
    /// (its participants get [`AuditEvent::DocDeleted`]; later calls on it fail with
    /// [`SessionError::Deleted`]), then every kit row of the doc is erased in one store
    /// transaction ([`DocStore::delete_doc`]: a blob doc's blob references are released,
    /// so blob GC removes unreferenced blobs after its grace). The file on disk, if any,
    /// and the host's table are the host's: the kit neither writes nor removes them.
    /// [`SessionError::NotFound`] when there is no doc at `key`. On a store error nothing is
    /// erased (the closed session reopens on the next `open`).
    pub fn delete_doc(&self, key: &DocKey) -> Result<()> {
        let mut pool = self.pool()?;
        let store = pool.store.clone().ok_or(SessionError::Released)?;
        let id = store
            .find(key)?
            .ok_or_else(|| SessionError::NotFound(key.clone()))?;
        if let Some(s) = pool.sessions.remove(key) {
            s.close_deleted();
        }
        if let Some(s) = pool.blob_sessions.remove(key) {
            s.close_deleted();
        }
        store.delete_doc(id, self.host.now_ms())?;
        Ok(())
    }

    /// Delete every doc in `space`, as [`SessionManager::delete_doc`] does: loaded sessions
    /// close first, then one store transaction erases them all
    /// ([`DocStore::delete_space`]). The space's blob key stays
    /// ([`BlobStore::purge_space_key`] destroys it and every blob in the space at once).
    /// Returns the keys deleted.
    pub fn delete_space(&self, space: &str) -> Result<Vec<DocKey>> {
        let mut pool = self.pool()?;
        let store = pool.store.clone().ok_or(SessionError::Released)?;
        let in_space = |k: &DocKey| k.space == space;
        let text: Vec<DocKey> = pool
            .sessions
            .keys()
            .filter(|k| in_space(k))
            .cloned()
            .collect();
        for k in text {
            if let Some(s) = pool.sessions.remove(&k) {
                s.close_deleted();
            }
        }
        let blobs: Vec<DocKey> = pool
            .blob_sessions
            .keys()
            .filter(|k| in_space(k))
            .cloned()
            .collect();
        for k in blobs {
            if let Some(s) = pool.blob_sessions.remove(&k) {
                s.close_deleted();
            }
        }
        Ok(store.delete_space(space, self.host.now_ms())?)
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
        for (k, s) in self.loaded_blobs()? {
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
        let idle_blobs: Vec<DocKey> = pool
            .blob_sessions
            .iter()
            .filter(|(_, s)| {
                Arc::strong_count(s) == 1
                    && s.pool_info() == Some(0)
                    && now.saturating_sub(s.last_used_ms()) >= self.config.idle_unload_ms
            })
            .map(|(k, _)| k.clone())
            .collect();
        for k in &idle_blobs {
            if let Some(s) = pool.blob_sessions.remove(k) {
                s.close(SessionError::Unloaded);
                pool.unloaded_total += 1;
            }
        }
        Ok(idle.len() + idle_blobs.len())
    }

    /// Pool totals, for the host's doctor/status.
    pub fn stats(&self) -> Result<PoolStats> {
        let blob = match &self.blobs {
            Some(b) => b.stats()?,
            None => Default::default(),
        };
        let pool = self.pool()?;
        Ok(PoolStats {
            blob_sessions: pool.blob_sessions.len(),
            blob_count: blob.blobs,
            blob_bytes: blob.bytes,
            blob_unreferenced_bytes: blob.unreferenced_bytes,
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
        for (_, s) in pool.blob_sessions.drain() {
            s.close(SessionError::Released);
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
