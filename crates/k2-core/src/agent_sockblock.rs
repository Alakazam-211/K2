//! Agent Isolation M2: the Unix-socket block for fenced sessions
//! (`review-agent-isolation-m2-unix-socket-block`, sections 2 and 3;
//! `design-agent-isolation-m2-unix-socket-block` rev 2).
//!
//! Landlock on the supported kernels doesn't govern `connect()` to a
//! pathname Unix socket, so a fenced session could reach other programs of
//! the same user (systemd's buses, the owner's tmux) and have them run
//! commands outside its fence (kernel rows 4, P2, P7). The block:
//! - a seccomp filter in the session ([`filter`]): every `connect()`
//!   notifies; `io_uring`, a notification listener of its own, Unix datagram
//!   sockets and other architectures' entries are refused;
//! - a supervisor ([`supervisor`]): the exec helper forks, the child installs
//!   the filter and runs the program, the helper stays as the session's
//!   first process and answers each `connect()` by connecting the caller's
//!   own socket (a copy taken with `pidfd_getfd`) to the address it copied
//!   out once, when the rule allows it.
//!
//! The Unix rule: only (1) the session's hook socket, by its fixed path, no
//! link on the way, a socket owned by K2's user; (2) a socket under the
//! session's own temp folder, opened beneath a folder handle held from
//! spawn, one link, same device, connected through `/proc/self/fd/<n>`.
//! Abstract and relative names are refused. Other families are connected as
//! asked.
//!
//! Two modes ([`Mode`]): `record` (the default for now) decides as enforce
//! would, writes every decision to `~/.k2/agents/<ws>/sockblock.log`, and
//! allows; `enforce` refuses. The daemon's `K2_AGENT_SOCKBLOCK`
//! (`enforce|record|off`) overrides it.
//!
//! Linux (x86-64, aarch64) only; elsewhere the daemon never sets it.

use std::path::{Component, Path};

use serde::{Deserialize, Serialize};

pub mod filter;
#[cfg(target_os = "linux")]
pub mod supervisor;
#[cfg(target_os = "linux")]
pub mod sys;

#[cfg(all(test, target_os = "linux"))]
mod run_tests;
#[cfg(test)]
mod tests;

/// The daemon's switch: `enforce`, `record` (the default) or `off`.
pub const ENV_SWITCH: &str = "K2_AGENT_SOCKBLOCK";
/// The decision log, in the agent's own folder (`~/.k2/agents/<ws>/`),
/// outside its view.
pub const LOG_NAME: &str = "sockblock.log";
/// The log is moved aside (`.1`) at a session's start past this size.
pub const LOG_ROTATE_AT: u64 = 8 << 20;
/// A session stops writing to the log past this many bytes.
pub const LOG_SESSION_CAP: u64 = 8 << 20;

/// What the supervisor does with a connect its rule refuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Log it and allow it.
    Record,
    /// Refuse it.
    Enforce,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Record => "record",
            Mode::Enforce => "enforce",
        }
    }
}

/// The switch's value: the mode, or off (`None`). Unset or empty: record.
pub fn mode_from(v: Option<&str>) -> Option<Mode> {
    match v.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        None | Some("") | Some("record") | Some("1") | Some("on") => Some(Mode::Record),
        Some("enforce") => Some(Mode::Enforce),
        Some("off" | "0" | "false" | "no") => None,
        // Unknown words: the safe default, said in /boot-status.
        Some(_) => Some(Mode::Record),
    }
}

/// The daemon's mode for fenced sessions now.
pub fn mode() -> Option<Mode> {
    mode_from(std::env::var(ENV_SWITCH).ok().as_deref())
}

/// The block's part of the exec helper's plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockSpec {
    pub mode: Mode,
    /// The session's hook socket (`$K2_HOOK_SOCK`), its folder resolved by
    /// the daemon; `None`: the session has none (a published service).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hook_sock: Option<String>,
    /// The user K2 runs as: the hook socket must be theirs.
    pub k2_uid: u32,
    /// The session's private temp folder (its `TMPDIR`), resolved.
    pub tmp_dir: String,
    /// The decision log (`~/.k2/agents/<ws>/sockblock.log`).
    pub log: String,
}

/// The block for a fenced session of agent `ws` (`None` with the switch off
/// or off Linux). `hook_sock`: the session's `$K2_HOOK_SOCK`.
pub fn spec_for(ws_dir: &Path, tmp_dir: &Path, hook_sock: Option<&str>) -> Option<BlockSpec> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let mode = mode()?;
    let resolve = |p: &Path| -> String {
        let r = match (p.parent(), p.file_name()) {
            (Some(dir), Some(name)) => std::fs::canonicalize(dir).map(|d| d.join(name)).unwrap_or_else(|_| p.to_path_buf()),
            _ => p.to_path_buf(),
        };
        r.to_string_lossy().into_owned()
    };
    #[cfg(unix)]
    // SAFETY: geteuid has no failure mode.
    let k2_uid = unsafe { libc::geteuid() };
    #[cfg(not(unix))]
    let k2_uid = 0;
    Some(BlockSpec {
        mode,
        hook_sock: hook_sock.filter(|h| !h.is_empty()).map(|h| resolve(Path::new(h))),
        k2_uid,
        tmp_dir: std::fs::canonicalize(tmp_dir).unwrap_or_else(|_| tmp_dir.to_path_buf()).to_string_lossy().into_owned(),
        log: ws_dir.join(LOG_NAME).to_string_lossy().into_owned(),
    })
}

/// Whether this kernel has what the block needs (seccomp user
/// notification with the wait-killable flag: 5.19+), or why not.
pub fn support() -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        if filter::Arch::native().is_none() {
            return Err("the Unix-socket block is built for x86-64 and aarch64 only".into());
        }
        let rel = std::fs::read_to_string("/proc/sys/kernel/osrelease").map_err(|e| format!("kernel release: {e}"))?;
        let (maj, min) = kernel_version(&rel).ok_or_else(|| format!("kernel release {rel:?} not understood"))?;
        if (maj, min) < (5, 19) {
            return Err(format!("kernel {maj}.{min}: the Unix-socket block needs 5.19 or later"));
        }
        if !sys::action_avail(filter::RET_USER_NOTIF) {
            return Err("this kernel has no seccomp user notification".into());
        }
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err("the Unix-socket block is Linux only".into())
    }
}

/// `(major, minor)` from a kernel release string (`6.8.0-149-generic`).
pub fn kernel_version(rel: &str) -> Option<(u32, u32)> {
    let mut it = rel.trim().split(['.', '-']);
    Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
}

/// For `/boot-status` `agentIsolation.sockblock`.
pub fn status_json() -> serde_json::Value {
    let raw = std::env::var(ENV_SWITCH).ok();
    let m = mode_from(raw.as_deref());
    serde_json::json!({
        "mode": m.map(Mode::as_str).unwrap_or("off"),
        "appliesTo": "fenced sessions, their job runs and published services",
        "switch": format!("{ENV_SWITCH}={}", raw.as_deref().filter(|v| !v.is_empty()).unwrap_or("unset (record)")),
        "kernel": match support() { Ok(()) => serde_json::Value::from("ok"), Err(e) => serde_json::Value::from(e) },
    })
}

// ── addresses ────────────────────────────────────────────────────────

/// A Unix address as the caller gave it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnixName {
    /// A name in the file system (bytes up to the first NUL).
    Path(Vec<u8>),
    /// An abstract name (leading NUL).
    Abstract(Vec<u8>),
    /// Not a valid Unix address for `connect()` (the kernel says `EINVAL`).
    Invalid,
}

/// `sizeof(struct sockaddr_un)`.
pub const SUN_LEN_MAX: usize = 110;

/// Parse a `sockaddr_un` copied out of the caller (any bytes).
pub fn parse_unix(addr: &[u8]) -> UnixName {
    if addr.len() <= 2 || addr.len() > SUN_LEN_MAX {
        return UnixName::Invalid;
    }
    if u16::from_ne_bytes([addr[0], addr[1]]) != 1 {
        return UnixName::Invalid;
    }
    let p = &addr[2..];
    if p[0] == 0 {
        return UnixName::Abstract(p[1..].to_vec());
    }
    let end = p.iter().position(|b| *b == 0).unwrap_or(p.len());
    UnixName::Path(p[..end].to_vec())
}

/// A path the rule may name: absolute, no `.` or `..`, no empty part.
pub fn plain_absolute(p: &str) -> bool {
    let path = Path::new(p);
    path.is_absolute()
        && !p.contains('\0')
        && !p.contains("//")
        && path.components().all(|c| matches!(c, Component::RootDir | Component::Normal(_)))
}

/// One decision, as the log writes it.
#[derive(Debug, Clone, Serialize)]
pub struct Record<'a> {
    pub t: String,
    pub pid: u32,
    pub call: &'a str,
    pub family: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// What enforce does: `allow` or `refuse`.
    pub verdict: &'a str,
    pub mode: &'a str,
    /// What happened: `allowed` or `refused`.
    pub applied: &'a str,
    pub reason: String,
}

/// A family's name for the log.
pub fn family_name(f: i32) -> String {
    match f {
        1 => "unix".into(),
        2 => "inet".into(),
        10 => "inet6".into(),
        16 => "netlink".into(),
        17 => "packet".into(),
        40 => "vsock".into(),
        n => format!("family {n}"),
    }
}

/// A Unix name for the log.
pub fn shown(n: &UnixName) -> Option<String> {
    match n {
        UnixName::Path(p) => Some(String::from_utf8_lossy(p).into_owned()),
        UnixName::Abstract(p) => Some(format!("@{}", String::from_utf8_lossy(p))),
        UnixName::Invalid => None,
    }
}
