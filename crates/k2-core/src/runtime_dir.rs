//! A private, memory-backed runtime directory for files that briefly hold
//! secrets (the rendered frpc config, K2 vault §10.4). Never `/tmp` or the
//! home folder: Linux `$XDG_RUNTIME_DIR/k2`, else `/run/user/<uid>/k2`,
//! else `/dev/shm/k2-<uid>`; macOS the per-user `$TMPDIR/k2-runtime`
//! (private, 0700). `None` when none is usable.

use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

fn ensure_private(dir: &Path) -> bool {
    // SAFETY: geteuid has no preconditions.
    let me = unsafe { libc::geteuid() };
    match std::fs::symlink_metadata(dir) {
        Ok(m) => {
            if m.file_type().is_symlink() || !m.is_dir() || m.uid() != me {
                return false;
            }
            if m.permissions().mode() & 0o077 != 0 {
                return std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).is_ok();
            }
            true
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::DirBuilder::new().mode(0o700).create(dir).is_ok()
        }
        Err(_) => false,
    }
}

pub fn private_runtime_dir() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = vec![];
    #[cfg(target_os = "linux")]
    {
        // SAFETY: geteuid has no preconditions.
        let uid = unsafe { libc::geteuid() };
        if let Some(x) = std::env::var_os("XDG_RUNTIME_DIR").filter(|x| !x.is_empty()) {
            candidates.push(PathBuf::from(x).join("k2"));
        }
        candidates.push(PathBuf::from(format!("/run/user/{uid}/k2")));
        candidates.push(PathBuf::from(format!("/dev/shm/k2-{uid}")));
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(t) = std::env::var_os("TMPDIR").filter(|x| !x.is_empty()) {
            candidates.push(PathBuf::from(t).join("k2-runtime"));
        }
    }
    candidates
        .into_iter()
        .find(|c| c.parent().is_some_and(|p| p.is_dir()) && ensure_private(c))
}
