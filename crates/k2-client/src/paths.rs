//! Canonical K2 home-directory paths (PRD `prd-k2so-cleanup-v1.md` §3.1).
//!
//! `~/.k2` is the ONE home dir. The legacy `~/.k2so` is a compatibility
//! SYMLINK maintained by `migration_home` for external tools that
//! memorized the old path — no K2 code may construct a `.k2so` path
//! (enforced by `scripts/k2so-gate.sh` in CI). Every subsystem that needs
//! "the K2 home" resolves it here so the answer can never drift again.

use std::path::PathBuf;

/// The K2 home directory: `$HOME/.k2`.
///
/// Falls back to a relative `.k2` when no home directory can be resolved
/// — the same degrade `db::db_dir` has always used, so a pathological
/// environment behaves identically across subsystems instead of panicking
/// in some and degrading in others.
pub fn k2_home() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".k2")
}

/// `~/.k2/bin` — staged sidecar binaries (frpc, k2-open, xdg-open).
pub fn k2_bin() -> PathBuf {
    k2_home().join("bin")
}

/// `~/.k2/hooks` — agent lifecycle hook scripts (notify.sh).
pub fn k2_hooks() -> PathBuf {
    k2_home().join("hooks")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn k2_home_ends_with_dot_k2_never_k2so() {
        let p = k2_home();
        assert!(p.ends_with(".k2"), "canonical home must be ~/.k2: {p:?}");
        assert!(
            !p.to_string_lossy().contains(".k2so"),
            "legacy name must never appear in the canonical path"
        );
    }

    #[test]
    fn helpers_derive_from_k2_home() {
        assert_eq!(k2_bin(), k2_home().join("bin"));
        assert_eq!(k2_hooks(), k2_home().join("hooks"));
    }
}
