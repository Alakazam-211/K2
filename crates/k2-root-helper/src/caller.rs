//! Caller is the daemon (§6).
//!
//! `/etc/k2-root-helper/daemon.toml` (root:root 0644, written by the
//! installer) names each daemon: user, systemd unit, scope. A `daemon`
//! verb runs only when the process that ran `sudo` is that unit's main
//! process:
//!
//! 1. take the parent pid and hold it (pidfd) at once;
//! 2. while that process is a root-owned sudo (`/usr/bin/sudo`, sudo-rs),
//!    step to its parent, holding each, up to 3 hops (covers sudo as the
//!    parent with a PAM session or `use_pty` monitor, and sudo exec'ing);
//! 3. its real uid must be a configured daemon user's;
//! 4. it must equal the unit's `MainPID` (systemd, right scope);
//! 5. the process just below it (the outermost sudo, or the helper when
//!    sudo exec'd) must be in the daemon's session (sid == MainPID): a
//!    terminal the daemon spawned has its own session (setsid), so a shell
//!    tab that `exec sudo …` is refused;
//! 6. every held process must still be alive (no pid reuse).
//!
//! Any doubt is a refusal (exit 77), never an allow.

use k2_root_proto::caller::{match_main_pid, parse_daemon_toml, SystemdQuery, DAEMON_TOML_MAX};
use k2_root_proto::paths::{DAEMON_TOML, SUDO_EXES};

pub use k2_root_proto::caller::DaemonEntry;

use crate::ctx::{Ctx, Fail, HResult, ProcHandle};

/// Read the config: a regular file, never through a link, and (in
/// production) root-owned and not writable by group or other.
pub fn load_daemons(ctx: &Ctx) -> Result<Vec<DaemonEntry>, &'static str> {
    let dir = ctx
        .dir("/etc/k2-root-helper")
        .map_err(|_| "daemon.toml missing")?;
    let name = std::ffi::OsStr::new("daemon.toml");
    let st = dir
        .lstat(name)
        .map_err(|_| "daemon.toml unreadable")?
        .ok_or("daemon.toml missing")?;
    if crate::fsx::kind_of(st.st_mode as u32) != crate::fsx::Kind::File {
        return Err("daemon.toml is not a regular file");
    }
    if (st.st_mode as u32) & 0o022 != 0 {
        return Err("daemon.toml is group/other-writable");
    }
    if ctx.real_owners && st.st_uid != 0 {
        return Err("daemon.toml is not root-owned");
    }
    let bytes = dir
        .read_file(name, DAEMON_TOML_MAX)
        .map_err(|_| "daemon.toml unreadable")?
        .ok_or("daemon.toml missing")?;
    let text = std::str::from_utf8(&bytes).map_err(|_| "daemon.toml unreadable")?;
    let _ = DAEMON_TOML;
    parse_daemon_toml(text)
}

/// Who called.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caller {
    /// The first non-sudo ancestor.
    pub pid: i32,
    pub uid: Option<u32>,
    /// The configured daemon user that uid belongs to, if any.
    pub user: Option<String>,
    /// Passed the full check (§6.2).
    pub is_daemon: bool,
    /// Why not, when it isn't (audit only).
    pub why_not: Option<&'static str>,
}

/// Steps 1–2: the first non-sudo ancestor plus every handle taken.
/// Steps 1–2. Returns (first non-sudo ancestor, the process just below it
/// — the outermost sudo, or `None` when the helper's own parent is it —,
/// every handle taken).
fn walk(ctx: &Ctx) -> (i32, Option<i32>, Vec<Box<dyn ProcHandle>>) {
    let mut held = Vec::new();
    let mut p = ctx.procs.parent_pid();
    let mut below: Option<i32> = None;
    if let Some(h) = ctx.procs.hold(p) {
        held.push(h);
    }
    for _ in 0..3 {
        let is_sudo = match ctx.procs.exe(p) {
            Some((exe, owner)) => owner == 0 && SUDO_EXES.iter().any(|s| exe.as_os_str() == *s),
            None => false,
        };
        if !is_sudo {
            break;
        }
        let Some(pp) = ctx.procs.ppid_of(p) else {
            break;
        };
        below = Some(p);
        p = pp;
        if let Some(h) = ctx.procs.hold(p) {
            held.push(h);
        }
    }
    (p, below, held)
}

/// The helper's view of systemd for the shared check: its `Exec`
/// (absolute programs, env cleared) and process table; runs as root, so
/// the `runuser` fallback for user-scope daemons is available.
struct HelperQuery<'a>(&'a Ctx);

impl SystemdQuery for HelperQuery<'_> {
    fn run(&self, prog: &str, args: &[String]) -> Option<(i32, Vec<u8>)> {
        let out = self.0.exec.run(prog, args, None).ok()?;
        Some((out.status, out.stdout))
    }
    fn uid_of_user(&self, user: &str) -> Option<u32> {
        self.0.procs.uid_of_user(user)
    }
    fn can_runuser(&self) -> bool {
        true
    }
}

/// The unit's main pid in its scope (`k2_root_proto::caller::main_pid`).
pub fn main_pid(ctx: &Ctx, d: &DaemonEntry) -> Option<i32> {
    k2_root_proto::caller::main_pid(&HelperQuery(ctx), d)
}

/// Identify the caller. `full` runs the MainPID check (only `daemon` verbs
/// need it; `open` verbs record pid/uid for the audit line).
pub fn identify(ctx: &Ctx, full: bool) -> Caller {
    let (pid, below, held) = walk(ctx);
    let uid = ctx.procs.ruid_of(pid);
    let daemons = load_daemons(ctx);
    let mut c = Caller {
        pid,
        uid,
        user: None,
        is_daemon: false,
        why_not: None,
    };
    let daemons = match daemons {
        Ok(d) => d,
        Err(e) => {
            c.why_not = Some(e);
            return c;
        }
    };
    let Some(uid) = uid else {
        c.why_not = Some("caller uid unknown");
        return c;
    };
    let mine: Vec<&DaemonEntry> = daemons
        .iter()
        .filter(|d| ctx.procs.uid_of_user(&d.user) == Some(uid))
        .collect();
    let Some(first) = mine.first() else {
        c.why_not = Some("caller uid is not a daemon user");
        return c;
    };
    c.user = Some(first.user.clone());
    if !full {
        return c;
    }
    let d = match match_main_pid(&HelperQuery(ctx), &daemons, pid, uid) {
        Ok(d) => d,
        Err(why) => {
            c.why_not = Some(why);
            return c;
        }
    };
    // The process that the daemon spawned (the outermost sudo, or the
    // helper itself when sudo exec'd it) must still be in the daemon's
    // session. systemd makes every service's main process a session
    // leader; the daemon's own `sudo` calls inherit that session, while
    // terminals and published services `setsid()`. Without this, a K2
    // shell tab (a direct child of the daemon) that `exec sudo …` would be
    // "the daemon's child sudo".
    let sid = match below {
        Some(s) => ctx.procs.sid_of(s),
        None => ctx.procs.own_sid(),
    };
    if sid != Some(pid) {
        c.why_not = Some("caller is not in the daemon's session (setsid child of the daemon)");
        return c;
    }
    if held.is_empty() || !held.iter().all(|h| h.alive()) {
        c.why_not = Some("caller process changed during the check");
        return c;
    }
    c.user = Some(d.user.clone());
    c.is_daemon = true;
    c
}

/// For `daemon` verbs: the daemon's user, or exit 77.
pub fn require_daemon(c: &Caller) -> HResult<String> {
    match (&c.user, c.is_daemon) {
        (Some(u), true) => Ok(u.clone()),
        _ => Err(Fail::not_daemon()),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn daemon_toml_rules_are_the_shared_ones() {
        let ok = "[[daemon]]\nuser = \"k2\"\nunit = \"k2-daemon.service\"\nscope = \"system\"\n";
        assert_eq!(super::parse_daemon_toml(ok).unwrap()[0].user, "k2");
        assert!(super::parse_daemon_toml("[[daemon]]\nuser = \"K2\"\nunit = \"x.service\"\nscope = \"system\"\n").is_err());
    }
}
