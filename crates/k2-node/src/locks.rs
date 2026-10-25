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

/// The pid a k2-node smoke lock names (`k2-node pid=<n> …`); `None` for
/// anything else (another tool's lock).
pub fn smoke_lock_pid(body: &str) -> Option<i32> {
    let rest = body.strip_prefix("k2-node pid=")?;
    rest.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok().filter(|p: &i32| *p > 0)
}

/// Is `pid` a live k2-node process? (A lock naming a dead pid, or a pid
/// reused by another program, is a leftover.) This very process counts as
/// live: the lock is ours, or a sibling node's in the same process (tests).
fn live_k2_node(pid: i32) -> bool {
    if pid as u32 == std::process::id() {
        return true;
    }
    // SAFETY: signal 0 only checks the pid exists.
    if unsafe { libc::kill(pid, 0) } != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
        return false;
    }
    process_name(pid).is_some_and(|n| n == "k2-node")
}

#[cfg(target_os = "linux")]
fn process_name(pid: i32) -> Option<String> {
    std::fs::read_to_string(format!("/proc/{pid}/comm")).ok().map(|s| s.trim().to_string())
}

#[cfg(not(target_os = "linux"))]
fn process_name(pid: i32) -> Option<String> {
    let out = std::process::Command::new("/bin/ps").args(["-o", "comm=", "-p", &pid.to_string()]).output().ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then(|| s.rsplit('/').next().unwrap_or(&s).to_string())
}

/// A k2-node smoke lock left by a k2-node that's gone (killed mid-job; the
/// file is in /var/tmp, which survives a reboot): its pid isn't a live
/// k2-node. Removed, so it doesn't hold this node for 12 h as a foreign
/// lock. Another tool's lock, or a live k2-node's, is never touched.
/// Returns whether it removed one.
pub fn clear_stale_smoke(path: &str) -> bool {
    let p = Path::new(path);
    let Ok(body) = std::fs::read_to_string(p) else { return false };
    let Some(pid) = smoke_lock_pid(&body) else { return false };
    if live_k2_node(pid) {
        return false;
    }
    std::fs::remove_file(p).is_ok()
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

    /// Review round 3: a k2-node smoke lock whose pid is dead, or reused by
    /// another program, is a leftover and goes; another tool's lock and a
    /// live k2-node's stay.
    #[test]
    fn a_dead_k2_nodes_smoke_lock_is_cleared_and_others_stay() {
        let d = crate::util::temp_dir("smoke-stale");
        let p = d.join("k2-smoke.lock");
        let ps = p.display().to_string();
        assert_eq!(smoke_lock_pid("k2-node pid=4242 job=j started=1 purpose=x\n"), Some(4242));
        assert_eq!(smoke_lock_pid("builder smoke\n"), None);
        // A dead pid.
        let mut c = std::process::Command::new("/bin/sh").args(["-c", "exit 0"]).spawn().unwrap();
        let dead = c.id() as i32;
        c.wait().unwrap();
        std::fs::write(&p, format!("k2-node pid={dead} job=j started=1 purpose=k2 compute job\n")).unwrap();
        assert!(clear_stale_smoke(&ps), "a dead pid's lock goes");
        assert!(!p.exists());
        // A pid reused by another program.
        let mut other = std::process::Command::new("/bin/sleep").arg("30").spawn().unwrap();
        std::fs::write(&p, format!("k2-node pid={} job=j started=1 purpose=k2 compute job\n", other.id())).unwrap();
        assert!(clear_stale_smoke(&ps), "a pid that isn't k2-node goes");
        let _ = other.kill();
        let _ = other.wait();
        // This very process's pid: ours (or a sibling node's): kept.
        std::fs::write(&p, format!("k2-node pid={} job=j started=1 purpose=x\n", std::process::id())).unwrap();
        assert!(!clear_stale_smoke(&ps));
        assert!(p.exists());
        // Another tool's lock: untouched.
        std::fs::write(&p, "builder subagent smoke\n").unwrap();
        assert!(!clear_stale_smoke(&ps));
        assert!(p.exists());
        std::fs::remove_file(&p).unwrap();
        // A live k2-node (Linux: comm is the exec'd name): untouched.
        if cfg!(target_os = "linux") {
            let link = d.join("k2-node");
            std::os::unix::fs::symlink("/bin/sleep", &link).unwrap();
            let mut node = std::process::Command::new(&link).arg("30").spawn().unwrap();
            let t0 = std::time::Instant::now();
            while process_name(node.id() as i32).as_deref() != Some("k2-node") {
                assert!(t0.elapsed() < std::time::Duration::from_secs(5), "the stand-in never showed as k2-node");
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            std::fs::write(&p, format!("k2-node pid={} job=j started=1 purpose=x\n", node.id())).unwrap();
            assert!(!clear_stale_smoke(&ps), "a live k2-node's lock stays");
            assert!(p.exists());
            let _ = node.kill();
            let _ = node.wait();
        }
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
