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
//! ONLY the bind helper (the peer-cred reader — `PeerCred`/`peer_cred` —
//! was never wired to a caller and was deleted in the 0.40.31 warning-zero
//! pass; recover from git history when the accept loop lands). It is CALLED only
//! behind `K2_HOOK_SCOPED` (see [`crate::session_token::scoped_hooks_enabled`])
//! and Phase 0 never actually serves traffic on the socket — the accept
//! loop + vsock bridge + dispatch generalization are Phase 1/2. With the
//! flag OFF nothing here ever runs. UDS is unix-only; a non-unix stub
//! keeps the daemon compiling.

#[cfg(unix)]
pub use unix_impl::{bind_cell_socket, cell_socket_path};

// Sandbox B2 (microVM cell socket perms) — internal to the daemon crate.
// P4-H6: `resolve_sandbox_cell_uid` is no longer the live path (the spawn door
// allocates a PER-SESSION uid from `cell_uid_pool` and passes it in); it is kept
// (the worker still has a legacy shared-`k2cell` fallback, and its own unit tests
// pin the resolver) but no longer re-exported, since nothing in the daemon
// resolves a shared cell uid anymore. `set_cell_socket_owner` still chowns the
// cell socket — now to the per-session uid.
#[cfg(unix)]
pub(crate) use unix_impl::set_cell_socket_owner;

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
        // 0600 on the socket — only the daemon uid may connect. A microVM-
        // backed cell additionally chowns this inode to the dedicated `k2cell`
        // uid AFTER bind (see [`set_cell_socket_owner`]); a bare-PTY cell never
        // does, so it stays daemon-only — byte-identical to pre-B2.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        Ok(listener)
    }

    /// Resolve the dedicated sandbox-cell uid (`k2cell`) the SAME way the
    /// microVM worker's preflight + [`crate::cell_server`]'s peer-cred belt
    /// do: an explicit `K2_SANDBOX_CELL_UID` override first, then a passwd
    /// lookup by name. Returns `None` when neither resolves — on a host
    /// without the dedicated user (macOS dev, any flag-off box) the socket
    /// chown is simply skipped and the socket stays `0600` daemon-only.
    ///
    /// **Fail-closed:** a uid of 0 is REFUSED (mirrors the worker, which
    /// refuses to "drop" to root) — we never chown the socket to root, which
    /// would be a no-op widening rather than the intended scoping.
    ///
    /// P4-H6: superseded by the per-session [`crate::cell_uid_pool`] allocator —
    /// kept as the legacy shared-`k2cell` resolver (worker fallback parity + the
    /// unit tests below) but no longer on any live daemon path.
    #[allow(dead_code)]
    pub(crate) fn resolve_sandbox_cell_uid() -> Option<u32> {
        if let Ok(s) = std::env::var("K2_SANDBOX_CELL_UID") {
            return match s.trim().parse::<u32>() {
                Ok(0) | Err(_) => None,
                Ok(uid) => Some(uid),
            };
        }
        // getpwnam("k2cell"): the daemon runs OUTSIDE any jail, so NSS is
        // available here (unlike the worker's post-pivot drop, which is why it
        // caches numerically). One lookup at serve time per microVM cell.
        let name = std::ffi::CString::new("k2cell").ok()?;
        // SAFETY: `name` is a valid NUL-terminated C string kept alive across
        // the call; `getpwnam` returns a pointer into a static buffer (or
        // null). We read `pw_uid` out immediately and copy it.
        let pw = unsafe { libc::getpwnam(name.as_ptr()) };
        if pw.is_null() {
            return None;
        }
        let uid = unsafe { (*pw).pw_uid };
        if uid == 0 {
            None
        } else {
            Some(uid)
        }
    }

    /// Sandbox B2: make a **microVM-backed** cell's socket connectable by
    /// EXACTLY the dedicated `k2cell` uid (+ root) — scoped, never world.
    ///
    /// `bind_cell_socket` leaves the inode `0600` owned by the daemon (root).
    /// libkrun's unix-proxy performs the host-side `connect()` AS `k2cell`
    /// (the VMM priv-dropped to that uid; there is no guest→host idmap), so a
    /// root-owned `0600` inode gives it EACCES and the proxy silently drops
    /// bytes. We chown the inode to `cell_uid` while KEEPING mode `0600`: the
    /// owner (`k2cell`) now passes the rw check on `connect()`, and root (the
    /// daemon, which still holds the listener fd and runs `accept()`) bypasses
    /// perms regardless. The scope stays exactly `{k2cell, root}` — we never
    /// touch the mode bits, so this can never become a world/group widening.
    ///
    /// `cell_uid` MUST be the resolved dedicated uid from
    /// [`resolve_sandbox_cell_uid`] (uid 0 already refused there — fail-closed:
    /// the caller leaves the socket daemon-only when it doesn't resolve). The
    /// group is left unchanged (gid `-1`).
    ///
    /// Unix-only but compiles everywhere `#[cfg(unix)]` (libc::chown is
    /// portable across unixes); on macOS dev it is reachable only by an
    /// on-box test with a real second uid.
    pub(crate) fn set_cell_socket_owner(
        session_id: &SessionId,
        cell_uid: u32,
    ) -> std::io::Result<()> {
        use std::os::unix::ffi::OsStrExt;
        let path = cell_socket_path(session_id);
        let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, e)
        })?;
        // SAFETY: `c_path` is a valid NUL-terminated path kept alive across the
        // call. gid `-1` (cast from u32::MAX) means "don't change the group" —
        // only the owner uid is set; the mode bits (0600) are untouched.
        let rc = unsafe {
            libc::chown(
                c_path.as_ptr(),
                cell_uid as libc::uid_t,
                u32::MAX as libc::gid_t,
            )
        };
        if rc != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
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
            // Serialize with every other $HOME mutator in the binary —
            // HOME is process-global, and an unlocked swap here raced the
            // TempHome tests (path re-derived from HOME mid-test).
            let _home_lock = crate::test_support::lock_home();
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

        #[test]
        fn resolve_sandbox_cell_uid_honors_override_and_refuses_root() {
            // The shared resolver (reused by both cell_server's peer-cred belt
            // and the cell-socket chown) reads K2_SANDBOX_CELL_UID first.
            let prev = std::env::var_os("K2_SANDBOX_CELL_UID");

            // A concrete non-zero uid is taken verbatim.
            std::env::set_var("K2_SANDBOX_CELL_UID", "9001");
            assert_eq!(
                resolve_sandbox_cell_uid(),
                Some(9001),
                "explicit override uid must be returned"
            );

            // uid 0 is REFUSED — never chown the socket to root (fail-closed).
            std::env::set_var("K2_SANDBOX_CELL_UID", "0");
            assert_eq!(
                resolve_sandbox_cell_uid(),
                None,
                "uid 0 must be refused — never widen to root"
            );

            // Garbage parses to None (fail-closed), not a panic.
            std::env::set_var("K2_SANDBOX_CELL_UID", "not-a-number");
            assert_eq!(
                resolve_sandbox_cell_uid(),
                None,
                "unparseable override must fail-closed to None"
            );

            match prev {
                Some(p) => std::env::set_var("K2_SANDBOX_CELL_UID", p),
                None => std::env::remove_var("K2_SANDBOX_CELL_UID"),
            }
        }

        #[test]
        fn set_cell_socket_owner_to_self_uid_is_a_noop_chown() {
            // We can't chown to a *different* uid without privilege, but
            // chowning the socket to our OWN euid is always permitted and
            // exercises the real `libc::chown` path + leaves the 0600 mode
            // bits untouched (the scope-preserving invariant). A real
            // cross-uid `k2cell` chown is only assertable on-box (Linux, with
            // the dedicated user) — noted in the report.
            use std::os::unix::fs::PermissionsExt;
            // Serialize with every other $HOME mutator (see the sibling
            // bind test): set_cell_socket_owner re-derives the socket
            // path from HOME, so a concurrent swap = chown on a path
            // that no longer exists (the exact flake this fixes).
            let _home_lock = crate::test_support::lock_home();
            let prev = std::env::var_os("HOME");
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let tmp = std::path::PathBuf::from(format!(
                "/tmp/k2c{}{}",
                std::process::id(),
                nanos % 100_000
            ));
            std::fs::create_dir_all(&tmp).expect("create temp HOME");
            std::env::set_var("HOME", &tmp);

            let sid = SessionId::new();
            let listener = bind_cell_socket(&sid).expect("bind per-cell socket");
            let path = cell_socket_path(&sid);

            // SAFETY: geteuid is always safe.
            let euid = unsafe { libc::geteuid() };
            set_cell_socket_owner(&sid, euid).expect("chown socket to own euid");

            // Mode bits MUST remain 0600 — the chown never widens perms.
            let mode = std::fs::metadata(&path)
                .expect("stat socket")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600, "chown must leave mode 0600, got {mode:o}");

            drop(listener);
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
pub use stub::{bind_cell_socket, cell_socket_path};
