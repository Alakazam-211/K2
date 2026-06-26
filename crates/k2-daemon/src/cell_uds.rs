//! Per-cell Unix-domain socket bind + accept-time peer-cred attestation
//! (#58 Phase 0 — DORMANT helper, default OFF).
//!
//! ## Why this exists
//!
//! The #58 design (`.k2/prds/prd-58-token-channel-foundation.md` §3.2)
//! gives every agent cell its OWN socket — `~/.k2/run/cells/<sid>.sock`,
//! `0600` in a `0700` dir. Because the daemon CREATED that socket and owns
//! the `session_id → principal` map, **bytes arriving on socket A are cell
//! A by construction** — the principal comes from WHICH socket accepted,
//! never from the request body (nothing to forge). On accept the daemon
//! additionally reads the peer credential (`SO_PEERCRED` on Linux /
//! `LOCAL_PEERCRED`+`getpeereid` on macOS) as a belt on top of the
//! structural binding.
//!
//! ## Phase 0 scope (this module)
//!
//! ONLY the bind helper + the peer-cred reader. They are CALLED only
//! behind `K2_HOOK_SCOPED` (see [`crate::session_token::scoped_hooks_enabled`])
//! and Phase 0 never actually serves traffic on the socket — the accept
//! loop + vsock bridge + dispatch generalization are Phase 1/2. With the
//! flag OFF nothing here ever runs. UDS is unix-only; a non-unix stub
//! keeps the daemon compiling.

#[cfg(unix)]
pub use unix_impl::{bind_cell_socket, cell_socket_path, cells_dir, peer_cred, PeerCred};

#[cfg(unix)]
mod unix_impl {
    use std::path::PathBuf;

    use k2_core::session::SessionId;

    /// `~/.k2/run/cells` — the per-cell socket directory (`0700`).
    pub fn cells_dir() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".k2")
            .join("run")
            .join("cells")
    }

    /// `~/.k2/run/cells/<session_id>.sock`.
    pub fn cell_socket_path(session_id: &SessionId) -> PathBuf {
        cells_dir().join(format!("{session_id}.sock"))
    }

    /// Bind the per-cell `UnixListener` at [`cell_socket_path`], creating
    /// the `0700` parent dir and removing any stale socket file first, then
    /// chmod the socket to `0600`.
    ///
    /// Returns a `std::os::unix::net::UnixListener` (sync — `handle_v2_spawn`
    /// is a sync handler). Phase 1 converts this into the tokio accept loop
    /// via `tokio::net::UnixListener::from_std`. In Phase 0 the caller binds
    /// + immediately drops it (no accept loop yet), so this is exercised for
    /// its filesystem + permission side effects only, behind the flag.
    pub fn bind_cell_socket(
        session_id: &SessionId,
    ) -> std::io::Result<std::os::unix::net::UnixListener> {
        use std::os::unix::fs::PermissionsExt;

        let dir = cells_dir();
        std::fs::create_dir_all(&dir)?;
        // 0700 on the directory — only this uid may traverse to the sockets.
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;

        let path = cell_socket_path(session_id);
        // A stale socket file from a prior daemon would make bind() fail
        // with EADDRINUSE; remove it (best-effort — a missing file is fine).
        if path.exists() {
            let _ = std::fs::remove_file(&path);
        }

        let listener = std::os::unix::net::UnixListener::bind(&path)?;
        // 0600 on the socket — only this uid may connect.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        Ok(listener)
    }

    /// The credential of the process on the other end of an accepted
    /// connection. `pid` is best-effort (reliable on Linux `SO_PEERCRED`;
    /// weaker on macOS `LOCAL_PEERPID`, hence `Option`).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct PeerCred {
        pub uid: u32,
        pub gid: u32,
        pub pid: Option<i32>,
    }

    /// Read the peer credential of an accepted `UnixStream` — the
    /// accept-time attestation belt. The STRUCTURAL socket binding (one
    /// socket per cell) is the primary guarantee; this asserts the
    /// connecting uid/pid is the expected privilege-dropped worker.
    #[cfg(target_os = "linux")]
    pub fn peer_cred(stream: &std::os::unix::net::UnixStream) -> std::io::Result<PeerCred> {
        use std::os::unix::io::AsRawFd;
        let fd = stream.as_raw_fd();
        let mut ucred = libc::ucred {
            pid: 0,
            uid: 0,
            gid: 0,
        };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        // SAFETY: fd is a valid connected socket for the call's duration;
        // we pass a correctly-sized ucred + its length.
        let rc = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                &mut ucred as *mut _ as *mut libc::c_void,
                &mut len,
            )
        };
        if rc != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(PeerCred {
            uid: ucred.uid,
            gid: ucred.gid,
            pid: Some(ucred.pid),
        })
    }

    /// macOS: `getpeereid` gives uid/gid; pid via `LOCAL_PEERPID` (less
    /// reliable than Linux `SO_PEERCRED`, so it's optional — PRD §6
    /// open-Q "macOS peer-cred"). The structural socket binding holds
    /// regardless of how strict the pid match is.
    #[cfg(target_os = "macos")]
    pub fn peer_cred(stream: &std::os::unix::net::UnixStream) -> std::io::Result<PeerCred> {
        use std::os::unix::io::AsRawFd;
        let fd = stream.as_raw_fd();

        let mut uid: libc::uid_t = 0;
        let mut gid: libc::gid_t = 0;
        // SAFETY: fd is a valid connected socket; out-params are sized.
        let rc = unsafe { libc::getpeereid(fd, &mut uid, &mut gid) };
        if rc != 0 {
            return Err(std::io::Error::last_os_error());
        }

        // Best-effort pid via LOCAL_PEERPID. A failure leaves pid = None.
        let mut pid: libc::c_int = 0;
        let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
        const LOCAL_PEERPID: libc::c_int = 0x002; // sys/un.h
        // SAFETY: same fd; out-param + length are correctly sized.
        let prc = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_LOCAL,
                LOCAL_PEERPID,
                &mut pid as *mut _ as *mut libc::c_void,
                &mut len,
            )
        };
        let pid = if prc == 0 { Some(pid) } else { None };

        Ok(PeerCred {
            uid: uid as u32,
            gid: gid as u32,
            pid,
        })
    }

    /// Other unixes: uid/gid via `getpeereid`, no portable pid.
    #[cfg(all(unix, not(target_os = "linux"), not(target_os = "macos")))]
    pub fn peer_cred(stream: &std::os::unix::net::UnixStream) -> std::io::Result<PeerCred> {
        use std::os::unix::io::AsRawFd;
        let fd = stream.as_raw_fd();
        let mut uid: libc::uid_t = 0;
        let mut gid: libc::gid_t = 0;
        // SAFETY: fd is a valid connected socket; out-params are sized.
        let rc = unsafe { libc::getpeereid(fd, &mut uid, &mut gid) };
        if rc != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(PeerCred {
            uid: uid as u32,
            gid: gid as u32,
            pid: None,
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn cell_socket_path_is_under_run_cells_and_named_for_session() {
            let sid = SessionId::new();
            let path = cell_socket_path(&sid);
            assert!(
                path.ends_with(format!("{sid}.sock")),
                "socket file must be <session_id>.sock, got {path:?}",
            );
            assert!(
                path.to_string_lossy().contains("/.k2/run/cells/"),
                "socket must live under ~/.k2/run/cells/, got {path:?}",
            );
        }

        #[test]
        fn bind_creates_a_0600_socket_in_a_0700_dir() {
            use std::os::unix::fs::PermissionsExt;
            // Isolate $HOME so we don't touch the real ~/.k2. The HOME MUST
            // be short: a Unix socket path is capped at `SUN_LEN` (~104
            // bytes on macOS), and the real `/var/folders/...` temp dir
            // blows past it once `/.k2/run/cells/<uuid>.sock` is appended.
            // A short `/tmp` base keeps the bound path under the limit.
            let prev = std::env::var_os("HOME");
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let tmp = std::path::PathBuf::from(format!(
                "/tmp/k2u{}{}",
                std::process::id(),
                nanos % 100_000
            ));
            std::fs::create_dir_all(&tmp).expect("create temp HOME");
            std::env::set_var("HOME", &tmp);

            let sid = SessionId::new();
            let listener = bind_cell_socket(&sid).expect("bind per-cell socket");
            let path = cell_socket_path(&sid);
            assert!(path.exists(), "socket file must exist after bind");

            let sock_mode = std::fs::metadata(&path)
                .expect("stat socket")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(sock_mode, 0o600, "socket must be 0600, got {sock_mode:o}");

            let dir_mode = std::fs::metadata(cells_dir())
                .expect("stat cells dir")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(dir_mode, 0o700, "cells dir must be 0700, got {dir_mode:o}");

            drop(listener);
            // Restore HOME + clean up.
            match prev {
                Some(p) => std::env::set_var("HOME", p),
                None => std::env::remove_var("HOME"),
            }
            let _ = std::fs::remove_dir_all(&tmp);
        }
    }
}

// Non-unix stub: UDS + peer-cred are unavailable; the daemon still
// compiles. The flag-gated call sites never reach these on Windows.
#[cfg(not(unix))]
mod stub {
    use k2_core::session::SessionId;
    use std::path::PathBuf;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct PeerCred {
        pub uid: u32,
        pub gid: u32,
        pub pid: Option<i32>,
    }

    pub fn cells_dir() -> PathBuf {
        PathBuf::from(".")
    }
    pub fn cell_socket_path(_session_id: &SessionId) -> PathBuf {
        PathBuf::from(".")
    }
    pub fn bind_cell_socket(_session_id: &SessionId) -> std::io::Result<()> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "per-cell UDS is not supported on this platform",
        ))
    }
}

#[cfg(not(unix))]
pub use stub::{bind_cell_socket, cell_socket_path, cells_dir, PeerCred};
