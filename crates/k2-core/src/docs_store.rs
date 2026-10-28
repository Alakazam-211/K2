//! Where the live-docs store lives (Markdown multiplayer v1, K1).
//!
//! The store itself is the multiplayer kit's (`k2_mpc_base::store`,
//! one redb file, linked by the daemon). K2 only chooses the path:
//! `~/.k2/docs.redb`, or `K2_DOCS_REDB` when set. Under test the default is
//! a per-process temp file, and a test that points `K2_DOCS_REDB` at the
//! real home's store panics ([`crate::test_isolation::assert_store_not_in_real_home`]).
//!
//! `.redb` files under `~/.k2` are tightened to owner-only at boot
//! ([`crate::private_home`]), so this file needs no rule of its own.

use std::path::PathBuf;

/// Env override for the store path (tests and headless smokes).
pub const ENV_PATH: &str = "K2_DOCS_REDB";

/// File name under `~/.k2`.
pub const FILE_NAME: &str = "docs.redb";

/// The store path this process uses.
pub fn store_path() -> PathBuf {
    if let Ok(p) = std::env::var(ENV_PATH) {
        let trimmed = p.trim();
        if !trimmed.is_empty() {
            let p = PathBuf::from(trimmed);
            #[cfg(any(test, feature = "test-util"))]
            crate::test_isolation::assert_store_not_in_real_home(&p);
            return p;
        }
    }
    #[cfg(any(test, feature = "test-util"))]
    {
        test_store_path()
    }
    #[cfg(not(any(test, feature = "test-util")))]
    {
        crate::paths::k2_home().join(FILE_NAME)
    }
}

#[cfg(any(test, feature = "test-util"))]
fn test_store_path() -> PathBuf {
    static PATH: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    PATH.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!(
            "k2-docs-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).expect("docs store test dir");
        dir.join(FILE_NAME)
    })
    .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_is_a_temp_file_never_the_home_store() {
        let _env = crate::test_env::lock();
        if std::env::var_os(ENV_PATH).is_some() {
            panic!("{ENV_PATH} is set in this test process; unset it");
        }
        let p = store_path();
        assert!(p.starts_with(std::env::temp_dir()), "{p:?} is not under the temp dir");
        assert_eq!(p.file_name().and_then(|n| n.to_str()), Some(FILE_NAME));
        assert_eq!(store_path(), p, "one path per process");
    }
}
