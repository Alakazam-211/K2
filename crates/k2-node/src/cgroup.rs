//! Linux cgroup v2 under systemd `Delegate=yes` (CN24).
//!
//! At start `k2-node` moves itself into `<unit>/supervisor` (cgroup v2's
//! no-internal-process rule), enables `+cpu +memory +pids` on the unit's
//! `cgroup.subtree_control`, and gives each job a leaf `<unit>/job-<id>`
//! with `cpu.max`, `memory.max` and `pids.max`. The child joins its leaf
//! itself between fork and exec by writing `0` to the leaf's
//! `cgroup.procs`. Without delegation (old systemd, a container, macOS)
//! there is no controller and the offer says `caps.hard = false`.
//!
//! Everything works against a root path so tests run on a temp dir.

use std::ffi::CString;
use std::path::{Path, PathBuf};

pub const PIDS_MAX: u64 = 8192;

#[derive(Debug, Clone)]
pub struct Cgroups {
    /// The unit's cgroup directory (parent of `supervisor` and the leaves).
    pub base: PathBuf,
}

/// The cgroup v2 path of this process from `/proc/self/cgroup` text.
pub fn parse_self_cgroup(text: &str) -> Option<String> {
    text.lines().find_map(|l| l.strip_prefix("0::")).map(|p| p.trim().to_string()).filter(|p| p.starts_with('/'))
}

fn write(p: &Path, s: &str) -> Result<(), String> {
    std::fs::write(p, s).map_err(|e| format!("write {}: {e}", p.display()))
}

impl Cgroups {
    /// Set up delegation under `fs_root` (normally `/sys/fs/cgroup`) for
    /// the process cgroup `self_path` and `pid`. `None` when anything
    /// fails: caps stay soft.
    pub fn setup(fs_root: &Path, self_path: &str, pid: u32) -> Option<Self> {
        let rel = self_path.trim_start_matches('/');
        let mut base = fs_root.join(rel);
        if base.file_name().is_some_and(|n| n == "supervisor") {
            base = base.parent()?.to_path_buf();
        }
        if rel.is_empty() || !base.join("cgroup.subtree_control").exists() {
            return None;
        }
        let sup = base.join("supervisor");
        std::fs::create_dir_all(&sup).ok()?;
        write(&sup.join("cgroup.procs"), &pid.to_string()).ok()?;
        write(&base.join("cgroup.subtree_control"), "+cpu +memory +pids").ok()?;
        Some(Self { base })
    }

    /// The real thing on Linux.
    pub fn detect() -> Option<Self> {
        if !cfg!(target_os = "linux") {
            return None;
        }
        let text = std::fs::read_to_string("/proc/self/cgroup").ok()?;
        let path = parse_self_cgroup(&text)?;
        Self::setup(Path::new("/sys/fs/cgroup"), &path, std::process::id())
    }

    pub fn leaf(&self, job_key: &str) -> PathBuf {
        self.base.join(format!("job-{job_key}"))
    }

    /// Make the job's leaf and write its limits.
    pub fn create_leaf(&self, job_key: &str, cpu_millis: Option<u64>, mem_bytes: Option<u64>) -> Result<Leaf, String> {
        let dir = self.leaf(job_key);
        std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
        write(&dir.join("cpu.max"), &cpu_max(cpu_millis))?;
        write(&dir.join("memory.max"), &mem_bytes.map(|m| m.to_string()).unwrap_or_else(|| "max".into()))?;
        write(&dir.join("pids.max"), &PIDS_MAX.to_string())?;
        Leaf::new(dir)
    }
}

/// `cpu.max` for thousandths of a core over a 100 ms period.
pub fn cpu_max(cpu_millis: Option<u64>) -> String {
    match cpu_millis {
        Some(m) if m > 0 => format!("{} 100000", (m * 100).max(1000)),
        _ => "max 100000".to_string(),
    }
}

/// One job's leaf. `procs_path` is prebuilt so the child can join without
/// allocating after fork.
#[derive(Debug)]
pub struct Leaf {
    pub dir: PathBuf,
    pub procs_path: CString,
}

impl Leaf {
    fn new(dir: PathBuf) -> Result<Self, String> {
        let procs_path = CString::new(dir.join("cgroup.procs").to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
        Ok(Self { dir, procs_path })
    }

    /// Kill everything in the leaf (`cgroup.kill`, kernel ≥ 5.14). The
    /// caller also SIGKILLs the process group.
    pub fn kill(&self) {
        let _ = std::fs::write(self.dir.join("cgroup.kill"), "1");
    }

    /// Remove the leaf once it's empty (retried briefly).
    pub fn remove(&self) {
        for _ in 0..20 {
            if std::fs::remove_dir(&self.dir).is_ok() || !self.dir.exists() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}

/// Join the leaf from inside the forked child (async-signal-safe: open,
/// write, close on a prebuilt path).
///
/// # Safety
/// Call only between fork and exec.
pub unsafe fn join_in_child(procs_path: &CString) -> std::io::Result<()> {
    let fd = libc::open(procs_path.as_ptr(), libc::O_WRONLY);
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let n = libc::write(fd, b"0".as_ptr() as *const libc::c_void, 1);
    libc::close(fd);
    if n != 1 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_v2_line_only() {
        assert_eq!(parse_self_cgroup("0::/system.slice/k2-node.service\n").as_deref(), Some("/system.slice/k2-node.service"));
        assert_eq!(parse_self_cgroup("12:memory:/x\n1:name=systemd:/y\n"), None);
    }

    #[test]
    fn setup_and_leaf_on_a_fake_cgroupfs() {
        let root = crate::util::temp_dir("cgroupfs");
        let unit = root.join("system.slice/k2-node.service");
        std::fs::create_dir_all(&unit).unwrap();
        std::fs::write(unit.join("cgroup.subtree_control"), "").unwrap();
        let cg = Cgroups::setup(&root, "/system.slice/k2-node.service", 4242).expect("delegated");
        assert_eq!(std::fs::read_to_string(unit.join("supervisor/cgroup.procs")).unwrap(), "4242");
        assert_eq!(std::fs::read_to_string(unit.join("cgroup.subtree_control")).unwrap(), "+cpu +memory +pids");
        // A restart inside supervisor/ resolves to the same base.
        let again = Cgroups::setup(&root, "/system.slice/k2-node.service/supervisor", 4243).unwrap();
        assert_eq!(again.base, cg.base);
        let leaf = cg.create_leaf("j1-g1", Some(4000), Some(1 << 30)).unwrap();
        assert_eq!(std::fs::read_to_string(leaf.dir.join("cpu.max")).unwrap(), "400000 100000");
        assert_eq!(std::fs::read_to_string(leaf.dir.join("memory.max")).unwrap(), (1u64 << 30).to_string());
        assert_eq!(std::fs::read_to_string(leaf.dir.join("pids.max")).unwrap(), "8192");
        // Not delegated: no subtree_control → soft caps.
        assert!(Cgroups::setup(&root, "/other.slice/x", 1).is_none());
    }

    #[test]
    fn cpu_max_values() {
        assert_eq!(cpu_max(None), "max 100000");
        assert_eq!(cpu_max(Some(500)), "50000 100000");
        assert_eq!(cpu_max(Some(1)), "1000 100000", "never below 1% of a core");
    }
}
