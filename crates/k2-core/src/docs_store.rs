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
//!
//! **Tier 0 (any file):** the kit keeps blob bytes as files under a folder
//! the host chooses, with only the index in the store. K2 puts it beside the
//! store, `~/.k2/docs-blobs/` (so a redirected store takes its blobs with
//! it), or `K2_DOCS_BLOBS` when set. The same guard applies: a test that
//! points either at the real home's `.k2` panics. The folder holds file
//! contents, so it is owner-only ([`crate::private_home::SECRET_DIRS`]).

use std::path::PathBuf;

/// Env override for the store path (tests and headless smokes).
pub const ENV_PATH: &str = "K2_DOCS_REDB";

/// File name under `~/.k2`.
pub const FILE_NAME: &str = "docs.redb";

/// Env override for the tier-0 blob folder (tests and headless smokes).
pub const ENV_BLOBS: &str = "K2_DOCS_BLOBS";

/// The blob folder's name beside the store.
pub const BLOBS_DIR: &str = "docs-blobs";

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

/// The tier-0 blob folder this process uses: `K2_DOCS_BLOBS`, else
/// `docs-blobs/` beside [`store_path`].
pub fn blobs_path() -> PathBuf {
    if let Ok(p) = std::env::var(ENV_BLOBS) {
        let trimmed = p.trim();
        if !trimmed.is_empty() {
            let p = PathBuf::from(trimmed);
            #[cfg(any(test, feature = "test-util"))]
            crate::test_isolation::assert_store_not_in_real_home(&p);
            return p;
        }
    }
    let store = store_path();
    let dir = store.parent().map(|d| d.join(BLOBS_DIR)).unwrap_or_else(|| PathBuf::from(BLOBS_DIR));
    #[cfg(any(test, feature = "test-util"))]
    crate::test_isolation::assert_store_not_in_real_home(&dir);
    dir
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

    #[test]
    fn test_blobs_sit_beside_the_store_unless_redirected() {
        let _env = crate::test_env::lock();
        if std::env::var_os(ENV_PATH).is_some() || std::env::var_os(ENV_BLOBS).is_some() {
            panic!("{ENV_PATH} or {ENV_BLOBS} is set in this test process; unset them");
        }
        let b = blobs_path();
        assert_eq!(b.parent(), store_path().parent(), "{b:?} is not beside the store");
        assert_eq!(b.file_name().and_then(|n| n.to_str()), Some(BLOBS_DIR));
        let other = std::env::temp_dir().join(format!("k2-blobs-{}", uuid::Uuid::new_v4()));
        let got = {
            let _blobs = crate::test_env::EnvVar::set(ENV_BLOBS, &other);
            blobs_path()
        };
        assert_eq!(got, other);
    }

    #[test]
    #[should_panic(expected = "prod-isolation guard")]
    fn test_blobs_in_the_real_home_panic() {
        let _env = crate::test_env::lock();
        let home = crate::test_isolation::real_home().expect("this test needs the uid's real home from the password database");
        let r = {
            let _blobs = crate::test_env::EnvVar::set(ENV_BLOBS, home.join(".k2").join(BLOBS_DIR));
            std::panic::catch_unwind(blobs_path)
        };
        if let Err(e) = r {
            std::panic::resume_unwind(e);
        }
    }
}
