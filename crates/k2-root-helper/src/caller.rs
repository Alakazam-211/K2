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
//! 5. every held process must still be alive (no pid reuse).
//!
//! Any doubt is a refusal (exit 77), never an allow.

use serde::Deserialize;

use k2_root_proto::paths::{DAEMON_TOML, SUDO_EXES, SYSTEMCTL};

use crate::ctx::{Ctx, Fail, HResult, ProcHandle};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DaemonEntry {
    pub user: String,
    pub unit: String,
    pub scope: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DaemonToml {
    daemon: Vec<DaemonEntry>,
}

fn user_ok(u: &str) -> bool {
    let b = u.as_bytes();
    !b.is_empty()
        && b.len() <= 32
        && (b[0].is_ascii_lowercase() || b[0] == b'_')
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_' || *c == b'-')
}

fn unit_ok(u: &str) -> bool {
    let Some(stem) = u.strip_suffix(".service") else {
        return false;
    };
    !stem.is_empty()
        && stem.len() <= 64
        && stem
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'@' | b'.' | b'_' | b'-'))
}

/// Parse + validate `daemon.toml` text.
pub fn parse_daemon_toml(text: &str) -> Result<Vec<DaemonEntry>, &'static str> {
    let t: DaemonToml = toml::from_str(text).map_err(|_| "daemon.toml unreadable")?;
    if t.daemon.is_empty() || t.daemon.len() > 8 {
        return Err("daemon.toml lists no daemon");
    }
    for d in &t.daemon {
        if !user_ok(&d.user) || !unit_ok(&d.unit) || !matches!(d.scope.as_str(), "system" | "user")
        {
            return Err("daemon.toml entry invalid");
        }
    }
    Ok(t.daemon)
}

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
        .read_file(name, 16 * 1024)
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
fn walk(ctx: &Ctx) -> (i32, Vec<Box<dyn ProcHandle>>) {
    let mut held = Vec::new();
    let mut p = ctx.procs.parent_pid();
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
        p = pp;
        if let Some(h) = ctx.procs.hold(p) {
            held.push(h);
        }
    }
    (p, held)
}

/// The unit's main pid in its scope, through `systemctl show` (env cleared,
/// `LC_ALL=C`). `None` when systemd doesn't know it or it isn't running.
pub fn main_pid(ctx: &Ctx, d: &DaemonEntry) -> Option<i32> {
    let mut args: Vec<String> = Vec::new();
    if d.scope == "user" {
        args.push("--user".into());
        args.push("-M".into());
        args.push(format!("{}@", d.user));
    }
    for a in ["show", "-p", "MainPID", "--value", &d.unit] {
        args.push(a.to_string());
    }
    let out = ctx.exec.run(SYSTEMCTL, &args, None).ok()?;
    if out.status != 0 {
        return None;
    }
    let pid: i32 = String::from_utf8_lossy(&out.stdout).trim().parse().ok()?;
    (pid > 0).then_some(pid)
}

/// Identify the caller. `full` runs the MainPID check (only `daemon` verbs
/// need it; `open` verbs record pid/uid for the audit line).
pub fn identify(ctx: &Ctx, full: bool) -> Caller {
    let (pid, held) = walk(ctx);
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
    for d in &mine {
        if main_pid(ctx, d) == Some(pid) {
            if held.is_empty() || !held.iter().all(|h| h.alive()) {
                c.why_not = Some("caller process changed during the check");
                return c;
            }
            c.user = Some(d.user.clone());
            c.is_daemon = true;
            return c;
        }
    }
    c.why_not = Some("caller is not the daemon unit's main process");
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
    use super::*;

    #[test]
    fn daemon_toml_rules() {
        let ok = "[[daemon]]\nuser = \"k2\"\nunit = \"k2-daemon.service\"\nscope = \"system\"\n";
        assert_eq!(parse_daemon_toml(ok).unwrap()[0].user, "k2");
        let two = format!("{ok}[[daemon]]\nuser = \"k2std\"\nunit = \"k2-daemon-std.service\"\nscope = \"user\"\n");
        assert_eq!(parse_daemon_toml(&two).unwrap().len(), 2);
        for bad in [
            "",
            "[[daemon]]\nuser = \"root;x\"\nunit = \"k2-daemon.service\"\nscope = \"system\"\n",
            "[[daemon]]\nuser = \"K2\"\nunit = \"k2-daemon.service\"\nscope = \"system\"\n",
            "[[daemon]]\nuser = \"k2\"\nunit = \"k2-daemon\"\nscope = \"system\"\n",
            "[[daemon]]\nuser = \"k2\"\nunit = \"../x.service\"\nscope = \"system\"\n",
            "[[daemon]]\nuser = \"k2\"\nunit = \"k2-daemon.service\"\nscope = \"global\"\n",
            "[[daemon]]\nuser = \"k2\"\nunit = \"k2-daemon.service\"\nscope = \"system\"\nextra = 1\n",
            "[[daemon]]\nuser = \"k2\"\nunit = \"k2-daemon.service\"\n",
        ] {
            assert!(parse_daemon_toml(bad).is_err(), "{bad:?}");
        }
    }
}
