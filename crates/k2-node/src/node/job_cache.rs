//! Job caches (`k2 compute run --cache NAME`, protocol 6, 0.46.6).
//!
//! One folder per (server, workspace, name) that outlives the job:
//! `<slot root>/cache/job-caches/<ws_key>/<name>`, given to the job as
//! `K2_JOB_CACHE` (fuzz corpora, a cargo target dir, …).
//!
//! - **Who owns it.** The node makes the folders; behind the Linux wall
//!   the name folder is shared with the job's group (as warm slots are)
//!   and the job's unit gets it read-write, nothing else of the cache
//!   tree. The workspace folder above it is the node's alone, so a job
//!   never swaps its cache for a link. On macOS / `--dev` the job runs as
//!   the node user.
//! - **The job user changed.** Each cache remembers the uid that last used
//!   it (`.<name>.k2meta`, node-only, beside it). A job whose user has
//!   another uid (a re-setup, a wall turned on, an owner change) gets the
//!   cache empty: files another uid wrote can't have their times or modes
//!   set by the new one (cargo, fuzzers), so a half-usable cache would
//!   fail in odd ways. Said once in the job's log.
//! - **Disk.** A tenant's caches are inside its root, so its disk share
//!   counts them (the V74 scan). All caches together may hold
//!   [`cap`] (a quarter of the policy's disk budget): when a job using one
//!   ends, idle caches are evicted, any one over the cap alone first, then
//!   the least recently used. Under the disk floor the guard evicts idle
//!   caches after the warm slots ([`super::disk`]). A cache a running job
//!   holds is never evicted or cleared.
//! - **Concurrent jobs** of one workspace may share a cache: it's a folder
//!   (cargo locks its own target dir). The node only counts holders.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use k2_node_proto::cache::{self as pcache, CacheEntry, CacheGet, CacheView};
use k2_node_proto::share::Slot;
use serde::{Deserialize, Serialize};

use super::{Node, Tenant};
use crate::wall::Wall;

/// An evicted cache is renamed to this prefix in its workspace folder,
/// then deleted; a leftover one (a crash mid-delete) goes next pass.
const EVICTED_PREFIX: &str = ".evicted-";
const META_SUFFIX: &str = ".k2meta";

/// All job caches together may hold a quarter of the disk budget.
pub fn cap(policy: &crate::config::Policy) -> u64 {
    policy.disk_budget_bytes / 4
}

/// What the node remembers about one cache (node-only, beside it).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Meta {
    /// The uid of the job user that last used it.
    #[serde(default)]
    pub uid: Option<u32>,
    /// Node clock of the last job's end (or start, while it runs).
    #[serde(default)]
    pub used_at: i64,
    /// Measured at the last job's end.
    #[serde(default)]
    pub bytes: u64,
}

fn meta_path(ws_dir: &Path, name: &str) -> PathBuf {
    ws_dir.join(format!(".{name}{META_SUFFIX}"))
}

pub fn read_meta(ws_dir: &Path, name: &str) -> Option<Meta> {
    std::fs::read(meta_path(ws_dir, name)).ok().and_then(|b| serde_json::from_slice(&b).ok())
}

fn write_meta(ws_dir: &Path, name: &str, m: &Meta) -> Result<(), String> {
    let body = serde_json::to_vec(m).map_err(|e| e.to_string())?;
    crate::util::atomic_write(&meta_path(ws_dir, name), &body, 0o600)
}

/// A real directory, not a link to one.
fn real_dir(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir())
}

/// Holders of each cache folder right now (running jobs).
#[derive(Default)]
pub struct Busy(HashMap<PathBuf, u32>);

impl Busy {
    pub fn is_busy(&self, dir: &Path) -> bool {
        self.0.get(dir).is_some_and(|n| *n > 0)
    }
    fn take(&mut self, dir: &Path) {
        *self.0.entry(dir.to_path_buf()).or_insert(0) += 1;
    }
    fn give(&mut self, dir: &Path) {
        if let Some(n) = self.0.get_mut(dir) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                self.0.remove(dir);
            }
        }
    }
}

/// A cache a job holds (give it back with [`Node::job_cache_close`]).
#[derive(Debug, Clone)]
pub struct Held {
    pub dir: PathBuf,
    pub ws_dir: PathBuf,
    pub name: String,
    pub uid: u32,
}

/// One cache on disk, as a scan finds it.
#[derive(Debug, Clone)]
struct Found {
    slot: Slot,
    ws_key: String,
    ws_dir: PathBuf,
    name: String,
    dir: PathBuf,
    meta: Meta,
}

/// Every cache under one slot's `cache/job-caches/` (real folders only,
/// plain names only; links and odd names are never touched).
fn scan_slot(t: &Tenant) -> Vec<Found> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(t.paths.job_caches()) else { return out };
    for ws in rd.flatten() {
        let ws_key = ws.file_name().to_string_lossy().into_owned();
        if !pcache::valid_ws_key(&ws_key) || !ws.file_type().is_ok_and(|f| f.is_dir()) {
            continue;
        }
        let Ok(inner) = std::fs::read_dir(ws.path()) else { continue };
        for e in inner.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !pcache::valid_name(&name) || !e.file_type().is_ok_and(|f| f.is_dir()) {
                continue;
            }
            let meta = read_meta(&ws.path(), &name).unwrap_or_default();
            out.push(Found { slot: t.slot, ws_key: ws_key.clone(), ws_dir: ws.path(), name, dir: e.path(), meta });
        }
    }
    out.sort_by(|a, b| (&a.ws_key, &a.name).cmp(&(&b.ws_key, &b.name)));
    out
}

impl Node {
    /// The uid `t`'s jobs run as: the job user behind the Linux wall (the
    /// slot's own for a tenant), else this process's. `None` when the job
    /// user is missing (the start fails on its own then).
    pub(crate) fn job_uid(&self, t: &Tenant) -> Option<u32> {
        match &self.wall {
            Wall::JobUser { user } => {
                let name = if t.is_owner() { user.clone() } else { crate::admin::jobs::tenant_user_name(t.slot) };
                crate::admin::server::user_ids(&name).map(|(u, _)| u)
            }
            // SAFETY: geteuid can't fail.
            _ => Some(unsafe { libc::geteuid() }),
        }
    }

    /// Make (or reuse) the cache `name` of `workspace_id` for one of `t`'s
    /// jobs and hold it. Returns the hold and lines for the job's log.
    /// Blocking (folder walks; the executor for a discard).
    pub(crate) fn job_cache_open(&self, t: &Tenant, workspace_id: &str, name: &str, unit_mode: bool) -> Result<(Held, Vec<String>), String> {
        if !pcache::valid_name(name) {
            return Err(format!("{}: {name:?} isn't a cache name (1–64 of a-z 0-9 . _ -)", pcache::codes::CACHE_INVALID));
        }
        let fp = t.pin.lock().unwrap().as_ref().map(|p| p.controller_fp.clone()).ok_or("no server pin for this slot")?;
        let ws_key = pcache::ws_key(&fp, workspace_id);
        let ws_dir = t.paths.job_cache_ws(&ws_key);
        let dir = t.paths.job_cache(&ws_key, name);
        let uid = self.job_uid(t).ok_or("the job user is missing")?;
        let mut notes = Vec::new();
        // The parents are the node's: made here, never through a link.
        for p in [t.paths.cache(), t.paths.job_caches(), ws_dir.clone()] {
            match std::fs::symlink_metadata(&p) {
                Ok(m) if m.is_dir() => {}
                Ok(_) => return Err(format!("{} isn't a plain folder", p.display())),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    std::fs::create_dir(&p).map_err(|e| format!("create {}: {e}", p.display()))?;
                }
                Err(e) => return Err(format!("{}: {e}", p.display())),
            }
        }
        // The workspace folder: the node writes it (meta files), the job
        // only passes through it.
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = if unit_mode { 0o750 } else { 0o700 };
            std::fs::set_permissions(&ws_dir, std::fs::Permissions::from_mode(mode)).map_err(|e| format!("chmod {}: {e}", ws_dir.display()))?;
        }
        let mut busy = self.job_caches_busy.lock().unwrap();
        let held_now = busy.is_busy(&dir);
        match std::fs::symlink_metadata(&dir) {
            Ok(m) if !m.is_dir() => {
                // Only the node writes this folder; anything else here is
                // left over from no known run: it goes.
                std::fs::remove_file(&dir).map_err(|e| format!("remove {}: {e}", dir.display()))?;
            }
            Ok(_) => {
                let meta = read_meta(&ws_dir, name).unwrap_or_default();
                if let Some(old) = meta.uid.filter(|old| *old != uid) {
                    if held_now {
                        notes.push(format!("cache {name}: another running job (uid {old}) holds it; shared as it is"));
                    } else {
                        self.discard_blocking(t.slot, &ws_dir, name, &dir)?;
                        notes.push(format!("cache {name}: it was written by another job user (uid {old}, now {uid}); it starts empty"));
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("{}: {e}", dir.display())),
        }
        let fresh = !real_dir(&dir);
        if fresh {
            std::fs::create_dir(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
            notes.push(format!("cache {name}: new (empty)"));
        } else if notes.is_empty() {
            let m = read_meta(&ws_dir, name).unwrap_or_default();
            notes.push(format!("cache {name}: kept from an earlier run ({} MB)", m.bytes >> 20));
        }
        if unit_mode {
            // The job (its group) writes it, like a warm slot.
            crate::wall::group_dir(&dir)?;
        }
        let prev = read_meta(&ws_dir, name).unwrap_or_default();
        write_meta(&ws_dir, name, &Meta { uid: Some(uid), used_at: self.now(), bytes: if fresh { 0 } else { prev.bytes } })?;
        busy.take(&dir);
        Ok((Held { dir, ws_dir, name: name.to_string(), uid }, notes))
    }

    /// A job that held a cache ended: measure it, note its use, give it
    /// back, then keep all caches under [`cap`].
    pub(crate) async fn job_cache_close(self: &Arc<Self>, h: Held) {
        let d = h.dir.clone();
        let bytes = tokio::task::spawn_blocking(move || super::run::disk_usage(&d)).await.unwrap_or(0);
        if real_dir(&h.dir) {
            if let Err(e) = write_meta(&h.ws_dir, &h.name, &Meta { uid: Some(h.uid), used_at: self.now(), bytes }) {
                crate::nlog!("cache {}: {e}", h.dir.display());
            }
        }
        self.job_caches_busy.lock().unwrap().give(&h.dir);
        let n = self.clone();
        let _ = tokio::task::spawn_blocking(move || n.trim_job_caches()).await;
    }

    /// Remove one cache (not held): renamed aside under the busy lock's
    /// caller, its meta dropped, then deleted (the executor gives the job
    /// user's files back first behind the Linux wall). Blocking.
    fn discard_blocking(&self, slot: Slot, ws_dir: &Path, name: &str, dir: &Path) -> Result<(), String> {
        let aside = ws_dir.join(format!("{EVICTED_PREFIX}{name}-{}", k2_node_proto::crypto::random_id().unwrap_or_else(|_| self.now().to_string())));
        std::fs::rename(dir, &aside).map_err(|e| format!("move {} aside: {e}", dir.display()))?;
        let _ = std::fs::remove_file(meta_path(ws_dir, name));
        self.delete_aside(slot, &aside);
        Ok(())
    }

    fn delete_aside(&self, slot: Slot, aside: &Path) {
        if !real_dir(aside) {
            let _ = std::fs::remove_file(aside);
            return;
        }
        self.reclaim_blocking(slot, aside);
        if let Err(e) = std::fs::remove_dir_all(aside) {
            crate::nlog!("remove {}: {e}", aside.display());
        }
    }

    /// Evict one idle cache. False when a job holds it (now).
    fn evict_job_cache(&self, f: &Found) -> bool {
        {
            let busy = self.job_caches_busy.lock().unwrap();
            if busy.is_busy(&f.dir) {
                return false;
            }
            // Renamed under the lock: no open races the delete.
            let aside = f.ws_dir.join(format!("{EVICTED_PREFIX}{}-{}", f.name, k2_node_proto::crypto::random_id().unwrap_or_else(|_| self.now().to_string())));
            if let Err(e) = std::fs::rename(&f.dir, &aside) {
                crate::nlog!("evict {}: {e}", f.dir.display());
                return false;
            }
            let _ = std::fs::remove_file(meta_path(&f.ws_dir, &f.name));
            drop(busy);
            self.delete_aside(f.slot, &aside);
        }
        true
    }

    /// Every cache on this node, every slot.
    fn all_job_caches(&self) -> Vec<Found> {
        self.tenants().iter().flat_map(|t| scan_slot(t)).collect()
    }

    /// Leftover `.evicted-…` folders (a delete cut short). Blocking.
    pub(crate) fn sweep_evicted_job_caches(&self) {
        for t in self.tenants() {
            let Ok(rd) = std::fs::read_dir(t.paths.job_caches()) else { continue };
            for ws in rd.flatten().filter(|e| e.file_type().is_ok_and(|f| f.is_dir())) {
                let Ok(inner) = std::fs::read_dir(ws.path()) else { continue };
                for e in inner.flatten() {
                    if e.file_name().to_string_lossy().starts_with(EVICTED_PREFIX) {
                        self.delete_aside(t.slot, &e.path());
                    }
                }
            }
        }
    }

    /// Keep all idle caches under [`cap`]: any one over the cap alone
    /// first, then the least recently used. Returns what went. Blocking.
    pub(crate) fn trim_job_caches(&self) -> Vec<(PathBuf, u64)> {
        let cap = cap(&self.local.lock().unwrap().policy);
        let all = self.all_job_caches();
        let mut total: u64 = all.iter().map(|f| f.meta.bytes).sum();
        let mut idle: Vec<&Found> = {
            let busy = self.job_caches_busy.lock().unwrap();
            all.iter().filter(|f| !busy.is_busy(&f.dir)).collect()
        };
        idle.sort_by_key(|f| (f.meta.bytes <= cap, f.meta.used_at));
        let mut out = Vec::new();
        for f in idle {
            if total <= cap && f.meta.bytes <= cap {
                continue;
            }
            if self.evict_job_cache(f) {
                crate::nlog!("job cache {} evicted: {} MB (cap {} GB)", f.dir.display(), f.meta.bytes >> 20, cap >> 30);
                total = total.saturating_sub(f.meta.bytes);
                out.push((f.dir.clone(), f.meta.bytes));
            }
        }
        out
    }

    /// The disk guard (under the floor plus its margin, after the warm
    /// slots): evict idle caches, least recently used first, until
    /// `enough()` says so. Returns how many went. Blocking.
    pub(crate) fn evict_job_caches_for_disk(&self, enough: impl Fn() -> bool) -> usize {
        let mut all = self.all_job_caches();
        all.sort_by_key(|f| f.meta.used_at);
        let mut n = 0;
        for f in &all {
            if enough() {
                break;
            }
            if self.evict_job_cache(f) {
                crate::nlog!("disk low: evicted job cache {}", f.dir.display());
                n += 1;
            }
        }
        n
    }

    /// `cache_get`: `t`'s own caches on this node (never another
    /// server's), after clearing the ones asked for. Blocking.
    pub fn cache_view(&self, t: &Tenant, g: &CacheGet) -> CacheView {
        let at = self.now();
        let refused = |code: &str| CacheView { request_id: g.request_id.clone(), at, caches: vec![], cleared: vec![], refused: Some(code.to_string()) };
        if g.name.as_deref().is_some_and(|n| !pcache::valid_name(n)) {
            return refused(pcache::codes::CACHE_INVALID);
        }
        let Some(fp) = t.pin.lock().unwrap().as_ref().map(|p| p.controller_fp.clone()) else { return refused("not_paired") };
        let want_ws = g.workspace_id.as_deref().map(|w| pcache::ws_key(&fp, w));
        let matches = |f: &Found| want_ws.as_deref().is_none_or(|k| f.ws_key == k) && g.name.as_deref().is_none_or(|n| f.name == n);
        let entry = |f: &Found, busy: bool| {
            // A cache never measured (a job still holds it, or it's from
            // before its first end) is measured now.
            let bytes = if f.meta.bytes == 0 && real_dir(&f.dir) { super::run::disk_usage(&f.dir) } else { f.meta.bytes };
            CacheEntry { ws_key: f.ws_key.clone(), name: f.name.clone(), bytes, used_at: f.meta.used_at, busy }
        };
        let mut cleared = Vec::new();
        if g.clear {
            for f in scan_slot(t).iter().filter(|f| matches(f)) {
                let e = entry(f, false);
                if self.evict_job_cache(f) {
                    crate::nlog!("job cache {} cleared by its server", f.dir.display());
                    cleared.push(e);
                }
            }
        }
        let busy = self.job_caches_busy.lock().unwrap().0.clone();
        let caches = scan_slot(t).iter().filter(|f| matches(f)).map(|f| entry(f, busy.get(&f.dir).is_some_and(|n| *n > 0))).collect();
        CacheView { request_id: g.request_id.clone(), at, caches, cleared, refused: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_counts_holders() {
        let mut b = Busy::default();
        let d = PathBuf::from("/x/fuzz");
        assert!(!b.is_busy(&d));
        b.take(&d);
        b.take(&d);
        b.give(&d);
        assert!(b.is_busy(&d), "one holder left");
        b.give(&d);
        assert!(!b.is_busy(&d));
        b.give(&d);
        assert!(!b.is_busy(&d), "never below zero");
    }

    #[test]
    fn meta_round_trips_beside_the_cache_and_is_never_a_cache_name() {
        let d = crate::util::temp_dir("jc-meta");
        let m = Meta { uid: Some(501), used_at: 7, bytes: 4096 };
        write_meta(&d, "fuzz", &m).unwrap();
        assert_eq!(read_meta(&d, "fuzz"), Some(m));
        assert!(!pcache::valid_name(".fuzz.k2meta"));
        assert!(!pcache::valid_name(&format!("{EVICTED_PREFIX}fuzz-x")));
        let _ = std::fs::remove_dir_all(&d);
    }
}
