//! A small per-user PTY host: it owns terminal sessions in its own process,
//! so they survive restarts, updates and crashes of the program that drives
//! them.
//!
//! ptyd is a long-lived, single-threaded Unix process. One **owner** client
//! (the program that drives the sessions, such as a terminal server)
//! connects over a private Unix socket, authenticated
//! both ways: the server checks the peer uid and an HMAC challenge over a
//! 0600 key file; the client checks the socket directory and the server's
//! uid. The owner asks ptyd to spawn programs on fresh PTYs; ptyd reaps
//! them, always reads every master into a per-session ring with monotonic
//! byte offsets, and streams output to **data** connections from any offset.
//! When the owner goes away, the sessions keep running; a new owner takes
//! over, lists them and re-attaches where it left off. ptyd can also hold
//! listening sockets and other fds by name across an owner restart, and can
//! replace its own binary in place (`upgrade`) without dropping a session.
//!
//! The crate is both the server binary (`k2-ptyd`) and a library:
//!
//! - [`proto`]: the wire protocol (length-prefixed JSON frames, `Request` /
//!   `Reply`);
//! - [`auth`]: the key file and the HMAC challenge;
//! - [`paths`]: the socket, key, lock and log layout under a state home;
//! - [`ring`]: the per-session output ring;
//! - [`client`]: a small blocking client (handshake, requests, fd passing,
//!   data-stream attach);
//! - [`launch`], [`server`], [`spawn`], [`sys`], [`fsutil`]: the server side
//!   (Unix only).
//!
//! The dependency surface is deliberately tiny: no async runtime, no
//! terminal emulator, no network or TLS crates. Windows is not supported
//! yet.
//!
//! Its runtime contract still uses the names of K2, the product it was
//! built for: the state home comes from `K2_HOME`, the orphan timeout from
//! `K2_PTYD_ORPHAN_SECS`, and the environment is cut down to
//! [`ENV_ALLOWLIST`]. They become configuration before this crate is
//! published under its own name.
//!
//! A runnable owner (launch, connect, spawn bash, attach from offset 0,
//! read) is in `examples/spawn_bash.rs`.

pub mod auth;
pub mod paths;
pub mod proto;
pub mod ring;

#[cfg(unix)]
pub mod client;
#[cfg(unix)]
pub mod fsutil;
#[cfg(unix)]
pub mod launch;
#[cfg(unix)]
pub mod server;
#[cfg(unix)]
pub mod spawn;
#[cfg(unix)]
pub mod sys;

/// Env allowlist ptyd is launched with. Everything else is
/// cleared; in particular never `NOTIFY_SOCKET`, `LISTEN_*`,
/// `K2_HANDOFF_FD`, `K2_HOOK_*` or an owner token.
pub const ENV_ALLOWLIST: &[&str] = &[
    "HOME",
    "USER",
    "LOGNAME",
    "PATH",
    "LANG",
    "TMPDIR",
    "XDG_RUNTIME_DIR",
    "K2_HOME",
    "K2_PTYD_ORPHAN_SECS",
];

/// True for names ptyd may inherit (`LC_*` included).
pub fn env_allowed(name: &str) -> bool {
    ENV_ALLOWLIST.contains(&name) || name.starts_with("LC_")
}

#[cfg(test)]
mod tests {
    #[test]
    fn allowlist_never_passes_daemon_secrets() {
        for bad in ["NOTIFY_SOCKET", "LISTEN_FDS", "LISTEN_PID", "K2_HANDOFF_FD", "K2_HOOK_TOKEN", "K2_HOOK_SOCK", "K2_TOKEN", "K2_PTYD"] {
            assert!(!super::env_allowed(bad), "{bad}");
        }
        assert!(super::env_allowed("LC_ALL"));
        assert!(super::env_allowed("K2_HOME"));
    }
}
