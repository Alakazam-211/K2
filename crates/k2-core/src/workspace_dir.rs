//! Per-workspace dot-dir resolution (`<project>/.k2` vs `<project>/.k2so`).
//!
//! 0.40.x: NEW workspaces create `.k2/`. Existing `.k2so/` workspaces are
//! matched and kept exactly as-is — the resolver checks for a real `.k2/`
//! dir, then a real `.k2so/` dir, and only falls back to creating `.k2/`
//! when neither exists yet. So nothing in a user's existing repo renames
//! underneath them; only brand-new workspaces pick up the K2 name. The
//! whole-tree migration of legacy `.k2so/` dirs lands before 1.0.0.
//!
//! Every production path that anchors on the workspace dot-dir MUST go
//! through [`workspace_dot_dir`] — a literal `.join(".k2so")` is only
//! correct in (a) tests pinning the legacy branch, and (b) the 0.37
//! unification migration, which operates on the historical layout by
//! definition.

use std::path::{Path, PathBuf};

/// Resolve the workspace dot-dir for a project root:
/// 1. `<root>/.k2` if it exists as a real directory (preferred name), else
/// 2. `<root>/.k2so` if THAT exists as a real directory (existing
///    workspaces keep their dir — never renamed underneath the user), else
/// 3. `<root>/.k2` — the 0.40.x+ default for brand-new workspaces.
///
/// The one guard: if a non-directory `.k2` *file* is squatting the path
/// (pathological), fall back to `.k2so/` so `create_dir_all` won't collide.
pub fn workspace_dot_dir(project_root: impl AsRef<Path>) -> PathBuf {
    let root = project_root.as_ref();
    let k2 = root.join(".k2");
    if k2.is_dir() {
        return k2;
    }
    let k2so = root.join(".k2so");
    if k2so.is_dir() {
        return k2so;
    }
    // Neither dot-DIR exists yet → brand-new workspace → create `.k2/`,
    // unless a non-dir `.k2` file is in the way.
    if k2.exists() {
        k2so
    } else {
        k2
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "k2-wsdir-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn defaults_to_k2_when_neither_exists() {
        // 0.40.x+ flip: a brand-new workspace creates `.k2/`.
        let root = tmp("fresh");
        assert_eq!(workspace_dot_dir(&root), root.join(".k2"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn keeps_k2so_when_only_k2so_exists() {
        // Existing `.k2so/` workspaces are never renamed underneath the user.
        let root = tmp("legacy");
        std::fs::create_dir_all(root.join(".k2so")).unwrap();
        assert_eq!(workspace_dot_dir(&root), root.join(".k2so"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn prefers_k2_when_present() {
        let root = tmp("optin");
        std::fs::create_dir_all(root.join(".k2")).unwrap();
        assert_eq!(workspace_dot_dir(&root), root.join(".k2"));
        // Even if a stale .k2so also exists, .k2 wins.
        std::fs::create_dir_all(root.join(".k2so")).unwrap();
        assert_eq!(workspace_dot_dir(&root), root.join(".k2"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_k2_file_is_not_a_dot_dir() {
        let root = tmp("notdir");
        std::fs::write(root.join(".k2"), b"not a dir").unwrap();
        assert_eq!(workspace_dot_dir(&root), root.join(".k2so"));
        std::fs::remove_dir_all(&root).unwrap();
    }
}
