//! The retired `k2so` CLI alias, on every OS.
//!
//! Installs put `k2` alone on a machine that never had the alias, and
//! refresh one an earlier install made, so scripts that call `k2so` keep
//! working. macOS and Linux check `~/.local/bin/k2so` (and the system
//! bin); Windows checks `%LOCALAPPDATA%\K2\cli\k2so` and
//! `%LOCALAPPDATA%\K2\bin\k2so.cmd`. The unix PATH heal lives in
//! [`crate::cli_install`], which re-exports this.

use std::path::Path;

/// The `k2so` alias at `path` is there (a file, or a link, even a broken
/// one).
pub fn legacy_alias_present(path: &Path) -> bool {
    path.exists() || path.is_symlink()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "k2-cli-alias-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock after 1970")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn a_missing_alias_is_absent_and_a_file_is_present() {
        let dir = temp_dir("file");
        let alias = dir.join("k2so");
        assert!(!legacy_alias_present(&alias));
        std::fs::write(&alias, "#!/bin/bash\n").expect("write alias");
        assert!(legacy_alias_present(&alias));
        std::fs::remove_dir_all(&dir).expect("clean temp dir");
    }

    /// Windows: the `k2so.cmd` launcher an earlier install left in
    /// `%LOCALAPPDATA%\K2\bin` counts, so Install CLI refreshes it.
    #[test]
    fn a_cmd_launcher_left_by_an_earlier_install_is_present() {
        let dir = temp_dir("cmd");
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).expect("create bin");
        let cmd = bin.join("k2so.cmd");
        assert!(!legacy_alias_present(&cmd));
        std::fs::write(&cmd, "@echo off\r\n").expect("write k2so.cmd");
        assert!(legacy_alias_present(&cmd));
        std::fs::remove_dir_all(&dir).expect("clean temp dir");
    }

    #[cfg(unix)]
    #[test]
    fn a_broken_link_is_present() {
        let dir = temp_dir("link");
        let alias = dir.join("k2so");
        std::os::unix::fs::symlink(dir.join("gone"), &alias).expect("symlink");
        assert!(legacy_alias_present(&alias));
        std::fs::remove_dir_all(&dir).expect("clean temp dir");
    }
}
