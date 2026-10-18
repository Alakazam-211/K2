//! Paths (PRD §7.1, §7.2, P88). Everything resolves from the K2 home that
//! the daemon passes in (`K2_HOME`), never a literal `~/.k2`.

use std::path::{Path, PathBuf};

/// macOS `sun_path` holds 104 bytes including the NUL; Linux 108. Use the
/// smaller one everywhere so behaviour matches across OSes.
pub const SUN_PATH_MAX: usize = 103;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub home: PathBuf,
    /// `<home>/run/ptyd` (0700).
    pub dir: PathBuf,
    /// The control socket. `<dir>/ctl.sock`, or the short fallback.
    pub sock: PathBuf,
    /// Written only when `sock` is the short fallback (V30).
    pub endpoint_file: PathBuf,
    pub key: PathBuf,
    pub lock: PathBuf,
    pub log: PathBuf,
}

fn hash12(home: &Path) -> String {
    let h = crate::auth::sha256_hex(home.as_os_str().to_string_lossy().as_bytes());
    h[..12].to_string()
}

impl Paths {
    /// Resolve every path for a K2 home. `tmpdir` is the short-path base
    /// used when `<home>/run/ptyd/ctl.sock` would not fit in `sun_path`.
    pub fn resolve(home: &Path, tmpdir: &Path) -> Paths {
        let dir = home.join("run").join("ptyd");
        let default_sock = dir.join("ctl.sock");
        let sock = if default_sock.as_os_str().len() > SUN_PATH_MAX {
            tmpdir.join(format!("k2p-{}", hash12(home))).join("ctl.sock")
        } else {
            default_sock
        };
        Paths {
            endpoint_file: dir.join("endpoint"),
            key: dir.join("key"),
            lock: dir.join("lock"),
            log: home.join("logs").join("ptyd.log"),
            home: home.to_path_buf(),
            dir,
            sock,
        }
    }

    /// Resolve with `$TMPDIR` (or `/tmp`) as the short-path base.
    pub fn from_home(home: &Path) -> Paths {
        let tmp = std::env::var_os("TMPDIR")
            .map(PathBuf::from)
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        Paths::resolve(home, &tmp)
    }

    /// For clients: the socket ptyd actually listens on. The endpoint file
    /// (written by ptyd when it used the fallback) wins over the computed
    /// default, so a client with a different `$TMPDIR` still finds it.
    pub fn client_sock(&self) -> PathBuf {
        match std::fs::read_to_string(&self.endpoint_file) {
            Ok(s) if !s.trim().is_empty() => PathBuf::from(s.trim()),
            _ => self.sock.clone(),
        }
    }

    pub fn uses_fallback(&self) -> bool {
        self.sock.parent() != Some(self.dir.as_path())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_home_uses_run_ptyd() {
        let p = Paths::resolve(Path::new("/Users/a/.k2"), Path::new("/tmp"));
        assert_eq!(p.sock, Path::new("/Users/a/.k2/run/ptyd/ctl.sock"));
        assert_eq!(p.key, Path::new("/Users/a/.k2/run/ptyd/key"));
        assert_eq!(p.log, Path::new("/Users/a/.k2/logs/ptyd.log"));
        assert!(!p.uses_fallback());
    }

    #[test]
    fn long_home_falls_back_to_a_short_hashed_path() {
        let home = PathBuf::from(format!("/var/folders/{}/T/.tmpXYZ/home/.k2", "x".repeat(80)));
        let p = Paths::resolve(&home, Path::new("/tmp"));
        assert!(p.uses_fallback());
        assert!(p.sock.as_os_str().len() <= SUN_PATH_MAX, "{:?}", p.sock);
        assert!(p.sock.starts_with("/tmp"));
        // Deterministic per home, different across homes.
        assert_eq!(p.sock, Paths::resolve(&home, Path::new("/tmp")).sock);
        let other = Paths::resolve(&home.join("b"), Path::new("/tmp"));
        assert_ne!(p.sock, other.sock);
    }
}
