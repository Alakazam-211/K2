//! Phase 2.5 follow-up — daemon port stability.
//!
//! Pre-0.39.0h the daemon called `TcpListener::bind("127.0.0.1:0")` on
//! every startup, accepting whatever ephemeral port the kernel handed
//! back. Each daemon restart (app update, kickstart, launchd crash
//! recovery) therefore minted a new port — and any consumer that
//! cached the old port (the Tauri renderer's `daemon_ws_url` cache,
//! the heartbeat scheduler watchdog, …) silently routed traffic to a
//! dead socket until it noticed and re-read `~/.k2so/daemon.port`.
//!
//! For the frontend specifically this manifested as **state loss on
//! daemon restart**: every Zustand store that reads its baseline
//! through `/cli/settings/get` hit `ECONNREFUSED`, fell back to
//! defaults, then the first user interaction wrote those defaults
//! back to settings.json via the now-rebound daemon. See Phase 2.5
//! finding #547.
//!
//! Fix shape: try to reuse the previously-published port first. The
//! daemon already writes `~/.k2so/daemon.port` after a successful
//! bind, so on restart we read it back, attempt to bind that exact
//! port, and only fall through to `:0` (random) if it's taken by
//! another process (e.g. an orphan daemon a `launchctl kickstart`
//! didn't wait for). The token still rotates every boot — only the
//! port is sticky.
//!
//! This module is split out from `k2so-daemon/src/main.rs` so the
//! algorithm can be unit-tested without spawning the full daemon
//! binary, and so future remote-daemon flavors (K2SO Connect) share
//! one implementation.

use std::net::{SocketAddr, TcpListener as StdTcpListener};
use std::path::{Path, PathBuf};

/// Outcome of [`claim_port`]. The `port` is what the caller should
/// publish into `~/.k2so/daemon.port` (always equals the port the
/// returned listener actually bound to). `reused` is `true` when we
/// successfully claimed the previously-written port and `false` when
/// we fell back to a kernel-assigned ephemeral port.
#[derive(Debug)]
pub struct ClaimedPort {
    pub listener: StdTcpListener,
    pub port: u16,
    pub reused: bool,
}

/// Read a port number from `path`. Returns `None` when the file is
/// missing, unreadable, empty, or doesn't parse as a u16.
///
/// Trimmed to tolerate the trailing newline some shells append; the
/// daemon itself writes the bare digits, but a user might have hand-
/// edited the file during debugging.
pub fn read_port_file(path: &Path) -> Option<u16> {
    let raw = std::fs::read_to_string(path).ok()?;
    raw.trim().parse::<u16>().ok()
}

/// Attempt to bind `port` on loopback. Returns the bound listener on
/// success; `None` if the port is taken or otherwise unbindable.
///
/// `port = 0` would defeat the purpose of this helper — the kernel
/// would just hand back any ephemeral port — so we refuse it here.
/// The caller should pick a real previously-claimed port or fall
/// through to [`bind_ephemeral`].
fn try_bind_specific(port: u16) -> Option<StdTcpListener> {
    if port == 0 {
        return None;
    }
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    StdTcpListener::bind(addr).ok()
}

/// Bind to loopback on a kernel-assigned ephemeral port. Returns the
/// `(listener, port)` pair on success; `None` on hard failure
/// (kernel-level resource exhaustion — extremely rare in practice).
fn bind_ephemeral() -> Option<(StdTcpListener, u16)> {
    let addr = SocketAddr::from(([127, 0, 0, 1], 0));
    let listener = StdTcpListener::bind(addr).ok()?;
    let port = listener.local_addr().ok()?.port();
    Some((listener, port))
}

/// Claim a loopback listener, preferring the port stored in
/// `port_file`.
///
/// Algorithm:
/// 1. Read `port_file`. If it parses and `try_bind_specific` succeeds,
///    return `{ listener, port, reused: true }`.
/// 2. Otherwise (file missing, unparseable, or port taken), fall
///    through to `bind_ephemeral` and return `{ listener, port,
///    reused: false }`.
///
/// Returns `None` only if even the ephemeral bind fails — at that
/// point the daemon can't start so the caller should `exit(2)`.
pub fn claim_port(port_file: &Path) -> Option<ClaimedPort> {
    if let Some(preferred) = read_port_file(port_file) {
        if let Some(listener) = try_bind_specific(preferred) {
            return Some(ClaimedPort {
                listener,
                port: preferred,
                reused: true,
            });
        }
    }
    let (listener, port) = bind_ephemeral()?;
    Some(ClaimedPort {
        listener,
        port,
        reused: false,
    })
}

/// Test-only convenience: claim against a `PathBuf` so callers can
/// build a scratch path without juggling lifetimes. Production code
/// should call [`claim_port`] directly with `&Path`.
#[cfg(test)]
pub fn claim_port_for_test(port_file: PathBuf) -> Option<ClaimedPort> {
    claim_port(&port_file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn scratch_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "k2so-port-claim-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn claims_ephemeral_when_port_file_missing() {
        let dir = scratch_dir("missing");
        let port_file = dir.join("daemon.port");
        let claimed = claim_port(&port_file).expect("claim_port returned None");
        assert!(!claimed.reused, "expected ephemeral fallback");
        assert!(claimed.port > 0);
        // Listener should accept loopback connections.
        drop(claimed.listener);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn claims_ephemeral_when_port_file_is_garbage() {
        let dir = scratch_dir("garbage");
        let port_file = dir.join("daemon.port");
        std::fs::write(&port_file, b"not a number\n").unwrap();
        let claimed = claim_port(&port_file).expect("claim_port returned None");
        assert!(!claimed.reused, "expected ephemeral fallback");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reuses_previously_published_port() {
        let dir = scratch_dir("reuse");
        let port_file = dir.join("daemon.port");

        // First boot: ephemeral. Capture the port and write it to
        // the port file — simulating what the daemon would do.
        let first = claim_port(&port_file).expect("first claim");
        let first_port = first.port;
        assert!(!first.reused);
        drop(first.listener); // release before second bind
        let mut f = std::fs::File::create(&port_file).unwrap();
        write!(f, "{first_port}").unwrap();
        drop(f);

        // Second boot: should re-bind the same port.
        let second = claim_port(&port_file).expect("second claim");
        assert!(second.reused, "expected port reuse");
        assert_eq!(second.port, first_port);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn falls_back_when_preferred_port_is_taken() {
        let dir = scratch_dir("taken");
        let port_file = dir.join("daemon.port");

        // Acquire a port and HOLD the listener so it stays bound.
        let hog_addr = SocketAddr::from(([127, 0, 0, 1], 0));
        let hog = StdTcpListener::bind(hog_addr).unwrap();
        let hog_port = hog.local_addr().unwrap().port();
        let mut f = std::fs::File::create(&port_file).unwrap();
        write!(f, "{hog_port}").unwrap();
        drop(f);

        // claim_port should see the port is taken and fall through
        // to a different ephemeral port.
        let claimed = claim_port(&port_file).expect("claim_port returned None");
        assert!(!claimed.reused, "expected fallback when preferred taken");
        assert_ne!(
            claimed.port, hog_port,
            "claim_port returned the held-port"
        );

        // Cleanup
        drop(hog);
        drop(claimed.listener);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_zero_port_in_port_file() {
        // Writing "0" into the port file would defeat the purpose
        // (kernel would just pick a new port). claim_port must
        // treat that as "fall through to ephemeral".
        let dir = scratch_dir("zero");
        let port_file = dir.join("daemon.port");
        std::fs::write(&port_file, b"0\n").unwrap();

        let claimed = claim_port(&port_file).expect("claim_port returned None");
        assert!(!claimed.reused);
        assert_ne!(claimed.port, 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn claim_port_for_test_wraps_claim_port() {
        // The test-only helper should be a thin pass-through.
        let dir = scratch_dir("wrap");
        let port_file = dir.join("daemon.port");
        let claimed = claim_port_for_test(port_file).expect("claim_port_for_test None");
        assert!(claimed.port > 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
