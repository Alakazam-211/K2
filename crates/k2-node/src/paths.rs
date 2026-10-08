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
    fn layout_paths() {
        let l = Layout::new("/h", "/c");
        assert_eq!(l.job_dir("j", 2), PathBuf::from("/h/jobs/j-g2"));
        assert_eq!(l.mirror("pk"), PathBuf::from("/h/repos/pk.git"));
        assert_eq!(l.control(), PathBuf::from("/c/control.toml"));
    }
}
