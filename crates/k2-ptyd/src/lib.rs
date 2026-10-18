//! k2-ptyd: K2's PTY host (PRD `prd-k2-ptyd-v1`).
//!
//! A small, long-lived per-user process that owns agent PTYs, so agents
//! survive daemon restarts, updates and crashes. The daemon is its single
//! owner client over a Unix socket (peer-uid check + HMAC challenge). ptyd
//! spawns and reaps agents, always reads every master into a per-session
//! ring, and streams bytes to the daemon by offset.
//!
//! The library half is shared with the daemon: the wire types ([`proto`]),
//! auth ([`auth`]), paths ([`paths`]) and a small blocking client
//! ([`client`]). No tokio, no alacritty (P1).

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

/// Env allowlist ptyd is launched with (PRD §7.1, P5). Everything else is
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
