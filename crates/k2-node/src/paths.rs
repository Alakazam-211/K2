//! Where everything lives (§8.2). Two roots:
//!
//! - **home** (`/var/k2node` on macOS, `/var/lib/k2node` on Linux), owned
//!   by the node user: key, ledger, jobs, mirrors, warm slots, toolchains.
//! - **config dir** (`/etc/k2-node`), owned by root with group
//!   `k2nodectl`: the machine owner's `policy.toml` and `control.toml`.
//!   The node user can't write it, so a job (same uid as `k2-node`) can't
//!   un-pause the node or widen its caps.

use std::path::{Path, PathBuf};

pub const DEFAULT_CONFIG_DIR: &str = "/etc/k2-node";

pub fn default_home() -> PathBuf {
    if cfg!(target_os = "macos") {
        PathBuf::from("/var/k2node")
    } else {
        PathBuf::from("/var/lib/k2node")
    }
}

#[derive(Debug, Clone)]
pub struct Layout {
    pub home: PathBuf,
    pub config: PathBuf,
}

impl Layout {
    pub fn new(home: impl Into<PathBuf>, config: impl Into<PathBuf>) -> Self {
        Self { home: home.into(), config: config.into() }
    }

    pub fn state(&self) -> PathBuf {
        self.home.join("state")
    }
    pub fn key(&self) -> PathBuf {
        self.state().join("node-key.pem")
    }
    pub fn pin(&self) -> PathBuf {
        self.state().join("controller.json")
    }
    pub fn ledger(&self) -> PathBuf {
        self.state().join("ledger.sqlite")
    }
    pub fn jobs(&self) -> PathBuf {
        self.home.join("jobs")
    }
    /// One attempt's directory: `jobs/<job_id>-g<generation>`.
    pub fn job_dir(&self, job_id: &str, generation: u32) -> PathBuf {
        self.jobs().join(format!("{job_id}-g{generation}"))
    }
    pub fn repos(&self) -> PathBuf {
        self.home.join("repos")
    }
    pub fn mirror(&self, project_key: &str) -> PathBuf {
        self.repos().join(format!("{project_key}.git"))
    }
    pub fn cache(&self) -> PathBuf {
        self.home.join("cache")
    }
    pub fn project_cache(&self, project_key: &str) -> PathBuf {
        self.cache().join(project_key)
    }
    pub fn toolchains(&self) -> PathBuf {
        self.home.join("toolchains")
    }
    pub fn run(&self) -> PathBuf {
        self.home.join("run")
    }
    pub fn status(&self) -> PathBuf {
        self.run().join("status.json")
    }
    pub fn git_home(&self) -> PathBuf {
        self.run().join("git-home")
    }
    pub fn policy(&self) -> PathBuf {
        self.config.join("policy.toml")
    }
    pub fn control(&self) -> PathBuf {
        self.config.join("control.toml")
    }

    /// Create the home tree (not the config dir; the installer owns it).
    pub fn ensure(&self) -> Result<(), String> {
        crate::util::mkdir_mode(&self.state(), 0o700)?;
        for d in [self.jobs(), self.repos(), self.cache(), self.toolchains(), self.git_home(), self.home.join("log")] {
            std::fs::create_dir_all(&d).map_err(|e| format!("create {}: {e}", d.display()))?;
        }
        crate::util::mkdir_mode(&self.run(), 0o755)?;
        Ok(())
    }

    /// Where non-owner tenants' job-facing roots live: beside the home,
    /// `<home>-t/<slot>` (`/var/lib/k2node-t/<slot>` on Linux,
    /// `/var/k2node-t/<slot>` on macOS; prd-compute-shared-queue-v1 §8.2).
    pub fn tenants_root(&self) -> PathBuf {
        let name = self.home.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "k2node".into());
        self.home.with_file_name(format!("{name}-t"))
    }

    /// One tenant's paths (V48). Slot 0, the owner server, keeps today's.
    pub fn tenant(&self, slot: u8) -> TenantLayout {
        TenantLayout { slot, layout: self.clone() }
    }
}

/// One tenant's node-side paths. Every name a tenant can touch carries its
/// slot (V48); the owner (slot 0) keeps every 0.46.2 path, so an older
/// `k2-node` after a rollback still finds the owner's state (§10.3).
///
/// - node-only state (pin, ledger, journals, artifacts): under `state/`
///   (0700, the node user), `state/tenants/<slot>/` for tenants;
/// - the job-facing root (jobs, mirrors, warm slots): the home for the
///   owner, [`Layout::tenants_root`]`/<slot>` for tenants (created by the
///   executor's `tenant.prepare`, node-owned, V54).
#[derive(Debug, Clone)]
pub struct TenantLayout {
    pub slot: u8,
    layout: Layout,
}

impl TenantLayout {
    fn owner(&self) -> bool {
        self.slot == 0
    }

    /// This tenant's node-only state dir.
    pub fn state(&self) -> PathBuf {
        if self.owner() {
            self.layout.state()
        } else {
            self.layout.state().join("tenants").join(self.slot.to_string())
        }
    }
    /// The server pin: `state/controller.json` for the owner,
    /// `state/servers/<slot>.json` otherwise (V45).
    pub fn pin(&self) -> PathBuf {
        if self.owner() {
            self.layout.pin()
        } else {
            self.layout.state().join("servers").join(format!("{}.json", self.slot))
        }
    }
    pub fn ledger(&self) -> PathBuf {
        if self.owner() {
            self.layout.ledger()
        } else {
            self.state().join("ledger.sqlite")
        }
    }
    /// The job-facing root.
    pub fn root(&self) -> PathBuf {
        if self.owner() {
            self.layout.home.clone()
        } else {
            self.layout.tenants_root().join(self.slot.to_string())
        }
    }
    pub fn jobs(&self) -> PathBuf {
        self.root().join("jobs")
    }
    /// One attempt's directory: `jobs/<job_id>-g<generation>` in this root.
    pub fn job_dir(&self, job_id: &str, generation: u32) -> PathBuf {
        self.jobs().join(format!("{job_id}-g{generation}"))
    }
    pub fn repos(&self) -> PathBuf {
        self.root().join("repos")
    }
    pub fn mirror(&self, project_key: &str) -> PathBuf {
        self.repos().join(format!("{project_key}.git"))
    }
    pub fn cache(&self) -> PathBuf {
        self.root().join("cache")
    }
    pub fn project_cache(&self, project_key: &str) -> PathBuf {
        self.cache().join(project_key)
    }
    /// The slot's shared scratch (`<root>/tmp`): its sccache server socket
    /// lives here, so one server's builds never reach another's sccache
    /// (V63). Group-writable for the slot's job user only.
    pub fn tmp(&self) -> PathBuf {
        self.root().join("tmp")
    }
    /// The slot's sccache server socket (`SCCACHE_SERVER_UDS`).
    pub fn sccache_socket(&self) -> PathBuf {
        self.tmp().join("sccache.sock")
    }
    /// An attempt's log journal. The owner's stays in its job dir (today);
    /// a tenant's is node-only, never in a dir its job can write (§8.2).
    pub fn journal(&self, job_id: &str, generation: u32) -> PathBuf {
        if self.owner() {
            self.job_dir(job_id, generation).join("log")
        } else {
            self.state().join("journal").join(format!("{job_id}-g{generation}.log"))
        }
    }
    /// Kept artifacts: `artifacts/` under the home for the owner, node-only
    /// state for tenants.
    pub fn artifacts(&self) -> PathBuf {
        if self.owner() {
            self.layout.home.join("artifacts")
        } else {
            self.state().join("artifacts")
        }
    }

    /// The node-only dirs (0700). The job-facing root is the executor's.
    pub fn ensure_state(&self) -> Result<(), String> {
        if self.owner() {
            return Ok(());
        }
        crate::util::mkdir_mode(&self.layout.state().join("servers"), 0o700)?;
        crate::util::mkdir_mode(&self.layout.state().join("tenants"), 0o700)?;
        crate::util::mkdir_mode(&self.state(), 0o700)?;
        for d in [self.state().join("journal"), self.artifacts()] {
            crate::util::mkdir_mode(&d, 0o700)?;
        }
        Ok(())
    }
}

/// The K2 daemon files that mark a user who runs K2 itself (CN25). A
/// compute node must never run as (or be installed for) such a user.
pub const PROD_FILES: &[&str] = &["daemon.port", "daemon.token", "heartbeat.port", "heartbeat.token"];

/// Which of [`PROD_FILES`] exist under `<home>/.k2`.
pub fn k2_files_in(home: &Path) -> Vec<String> {
    PROD_FILES
        .iter()
        .map(|f| home.join(".k2").join(f))
        .filter(|p| p.exists())
        .map(|p| p.display().to_string())
        .collect()
}

/// Validate a job's relative working directory: relative, no `..`, no NUL.
pub fn safe_relative(p: &str) -> Option<PathBuf> {
    let path = Path::new(p);
    if p.is_empty() || p.contains('\0') || path.is_absolute() {
        return None;
    }
    for c in path.components() {
        match c {
            std::path::Component::Normal(_) | std::path::Component::CurDir => {}
            _ => return None,
        }
    }
    Some(path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_relative_refuses_escapes() {
        assert!(safe_relative("crates/k2-core").is_some());
        assert!(safe_relative("./a").is_some());
        assert!(safe_relative("../a").is_none());
        assert!(safe_relative("a/../../b").is_none());
        assert!(safe_relative("/etc").is_none());
        assert!(safe_relative("").is_none());
    }

    #[test]
    fn k2_files_found_under_dot_k2() {
        let d = crate::util::temp_dir("prodfiles");
        assert!(k2_files_in(&d).is_empty());
        std::fs::create_dir_all(d.join(".k2")).unwrap();
        std::fs::write(d.join(".k2/heartbeat.token"), "x").unwrap();
        assert_eq!(k2_files_in(&d), vec![d.join(".k2/heartbeat.token").display().to_string()]);
    }

    #[test]
    fn layout_paths() {
        let l = Layout::new("/h", "/c");
        assert_eq!(l.job_dir("j", 2), PathBuf::from("/h/jobs/j-g2"));
        assert_eq!(l.mirror("pk"), PathBuf::from("/h/repos/pk.git"));
        assert_eq!(l.control(), PathBuf::from("/c/control.toml"));
    }

    /// Slot 0 keeps every 0.46.2 path (a rolled-back node still finds the
    /// owner's state); a tenant's are all namespaced by its slot (V48).
    #[test]
    fn owner_keeps_todays_paths_and_tenants_are_namespaced() {
        let l = Layout::new("/var/lib/k2node", "/etc/k2-node");
        let o = l.tenant(0);
        assert_eq!(o.pin(), l.pin());
        assert_eq!(o.ledger(), l.ledger());
        assert_eq!(o.state(), l.state());
        assert_eq!(o.job_dir("j", 2), l.job_dir("j", 2));
        assert_eq!(o.journal("j", 2), l.job_dir("j", 2).join("log"));
        assert_eq!(o.mirror("pk"), l.mirror("pk"));
        assert_eq!(o.project_cache("pk"), l.project_cache("pk"));
        assert_eq!(o.cache(), l.cache());
        assert_eq!(o.artifacts(), PathBuf::from("/var/lib/k2node/artifacts"));
        assert_eq!(l.tenants_root(), PathBuf::from("/var/lib/k2node-t"));
        let t = l.tenant(3);
        assert_eq!(t.pin(), PathBuf::from("/var/lib/k2node/state/servers/3.json"));
        assert_eq!(t.ledger(), PathBuf::from("/var/lib/k2node/state/tenants/3/ledger.sqlite"));
        assert_eq!(t.job_dir("j", 2), PathBuf::from("/var/lib/k2node-t/3/jobs/j-g2"));
        assert_eq!(t.mirror("pk"), PathBuf::from("/var/lib/k2node-t/3/repos/pk.git"));
        assert_eq!(t.project_cache("pk"), PathBuf::from("/var/lib/k2node-t/3/cache/pk"));
        assert_eq!(t.journal("j", 2), PathBuf::from("/var/lib/k2node/state/tenants/3/journal/j-g2.log"));
        assert_eq!(t.artifacts(), PathBuf::from("/var/lib/k2node/state/tenants/3/artifacts"));
        assert_eq!(t.sccache_socket(), PathBuf::from("/var/lib/k2node-t/3/tmp/sccache.sock"));
        assert_eq!(o.sccache_socket(), PathBuf::from("/var/lib/k2node/tmp/sccache.sock"));
        assert!(t.sccache_socket().as_os_str().len() < 100, "fits sun_path");
        // Journals and artifacts are node-only state, never in the root a job writes.
        for p in [t.journal("j", 2), t.artifacts(), t.ledger(), t.pin()] {
            assert!(p.starts_with(l.state()) && !p.starts_with(t.root()), "{}", p.display());
        }
        assert_eq!(Layout::new("/var/k2node", "/etc/k2-node").tenants_root(), PathBuf::from("/var/k2node-t"));
    }

    #[test]
    fn tenant_state_dirs_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let d = crate::util::temp_dir("tenant-state");
        let l = Layout::new(d.join("home"), d.join("cfg"));
        l.ensure().unwrap();
        l.tenant(0).ensure_state().unwrap();
        assert!(!l.state().join("tenants").exists(), "the owner needs no tenant dirs");
        let t = l.tenant(1);
        t.ensure_state().unwrap();
        for p in [l.state().join("servers"), t.state(), t.state().join("journal"), t.artifacts()] {
            let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700, "{}", p.display());
        }
    }
}
