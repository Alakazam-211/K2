//! Foreign locks and the smoke lock (§9.2). Other tools on the machine
//! (Sew's CI, a builder's hand-run smoke test) announce themselves with
//! lock files. A lock younger than 12 h holds new starts; an older one is
//! reported and never deleted. While jobs run the node writes its own
//! smoke lock and removes it after, only if it wrote it.

use std::path::{Path, PathBuf};

pub const STALE_SECS: i64 = 12 * 3600;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LockScan {
    /// The first young foreign lock.
    pub holding: Option<String>,
    /// Old locks, reported only.
    pub stale: Vec<String>,
}

fn mtime(p: &Path) -> Option<i64> {
    let m = std::fs::metadata(p).ok()?;
    let t = m.modified().ok()?;
    Some(t.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64)
}

/// Scan `paths` (plus the smoke lock unless we wrote it).
pub fn scan(paths: &[String], smoke: Option<&str>, we_own_smoke: bool, now: i64) -> LockScan {
    let mut out = LockScan::default();
    let mut all: Vec<&str> = paths.iter().map(|s| s.as_str()).collect();
    if let (Some(s), false) = (smoke, we_own_smoke) {
        all.push(s);
    }
    for p in all {
        let Some(t) = mtime(Path::new(p)) else { continue };
        if now - t < STALE_SECS {
            if out.holding.is_none() {
                out.holding = Some(p.to_string());
            }
        } else {
            out.stale.push(p.to_string());
        }
    }
    out
}

/// The node's own smoke lock while jobs run.
#[derive(Debug, Default)]
pub struct SmokeLock {
    written: Option<PathBuf>,
}

impl SmokeLock {
    pub fn owned(&self) -> bool {
        self.written.is_some()
    }

    /// Write it if nobody else holds the path. Returns whether we own it.
    pub fn acquire(&mut self, path: &str, job_id: &str, now: i64) -> bool {
        if self.written.is_some() {
            return true;
        }
        let p = PathBuf::from(path);
        if p.exists() {
            return false;
        }
        let body = format!("k2-node pid={} job={job_id} started={now} purpose=k2 compute job\n", std::process::id());
        let ok = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&p)
            .and_then(|mut f| std::io::Write::write_all(&mut f, body.as_bytes()))
            .is_ok();
        if ok {
            self.written = Some(p);
        }
        ok
    }

    /// Remove it if we wrote it and it's still ours.
    pub fn release(&mut self) {
        if let Some(p) = self.written.take() {
            let mine = std::fs::read_to_string(&p)
                .map(|s| s.starts_with(&format!("k2-node pid={} ", std::process::id())))
                .unwrap_or(false);
            if mine {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set_mtime(p: &Path, t: i64) {
        let c = std::ffi::CString::new(p.to_string_lossy().as_bytes()).unwrap();
        let tv = [libc::timeval { tv_sec: t as _, tv_usec: 0 }, libc::timeval { tv_sec: t as _, tv_usec: 0 }];
        assert_eq!(unsafe { libc::utimes(c.as_ptr(), tv.as_ptr()) }, 0);
    }

    #[test]
    fn young_lock_holds_old_lock_is_reported_not_deleted() {
        let d = crate::util::temp_dir("locks");
        let young = d.join("young.lock");
        let old = d.join("old.lock");
        std::fs::write(&young, "x").unwrap();
        std::fs::write(&old, "x").unwrap();
        let now = crate::util::now();
        set_mtime(&old, now - STALE_SECS - 10);
        let s = scan(&[old.display().to_string(), young.display().to_string()], None, false, now);
        assert_eq!(s.holding.as_deref(), Some(young.to_str().unwrap()));
        assert_eq!(s.stale, vec![old.display().to_string()]);
        std::fs::remove_file(&young).unwrap();
        let s = scan(&[old.display().to_string()], None, false, now);
        assert_eq!(s.holding, None);
        assert!(old.exists(), "stale locks are never deleted");
    }

    #[test]
    fn smoke_lock_written_and_removed_only_if_ours() {
        let d = crate::util::temp_dir("smoke");
        let p = d.join("k2-smoke.lock").display().to_string();
        let mut l = SmokeLock::default();
        assert!(l.acquire(&p, "job1", 5));
        assert!(std::fs::read_to_string(&p).unwrap().contains("job=job1"));
        let s = scan(&[], Some(&p), l.owned(), crate::util::now());
        assert_eq!(s.holding, None, "our own smoke lock doesn't hold us");
        l.release();
        assert!(!Path::new(&p).exists());
        // Someone else's smoke lock: we neither take it nor delete it.
        std::fs::write(&p, "builder subagent smoke\n").unwrap();
        let mut l = SmokeLock::default();
        assert!(!l.acquire(&p, "job2", 6));
        l.release();
        assert!(Path::new(&p).exists());
        let s = scan(&[], Some(&p), l.owned(), crate::util::now());
        assert_eq!(s.holding.as_deref(), Some(p.as_str()));
    }
}
