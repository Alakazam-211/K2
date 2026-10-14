//! "Is this process the K2 daemon's main process?" — shared by the root
//! helper (`k2-root-helper`, through sudo) and the App engine
//! (`k2-app-engine`, over its socket) (`prd-k2-app-runtime-v1` §10.6,
//! AR25).
//!
//! Both read the same root-owned `/etc/k2-root-helper/daemon.toml`
//! (written by the installer: each daemon's user, systemd unit, scope)
//! and ask systemd for that unit's `MainPID`. What differs is how each
//! finds the calling process:
//! - the helper walks up from its parent over root-owned sudo binaries and
//!   checks the daemon's session (`k2-root-helper/src/caller.rs`);
//! - the engine takes the socket peer (`SO_PEERCRED`) and holds it with a
//!   pidfd (`k2-app-engine`).
//!
//! The code here is the part they share: the config format and its rules,
//! the `MainPID` lookup, and the final match. Any doubt is a refusal.

use serde::Deserialize;

use crate::paths::{RUNUSER, SYSTEMCTL};

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

pub fn user_ok(u: &str) -> bool {
    let b = u.as_bytes();
    !b.is_empty()
        && b.len() <= 32
        && (b[0].is_ascii_lowercase() || b[0] == b'_')
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_' || *c == b'-')
}

pub fn unit_ok(u: &str) -> bool {
    let Some(stem) = u.strip_suffix(".service") else {
        return false;
    };
    !stem.is_empty()
        && stem.len() <= 64
        && stem
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'@' | b'.' | b'_' | b'-'))
}

/// Largest `daemon.toml` either side reads.
pub const DAEMON_TOML_MAX: usize = 16 * 1024;

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

/// What both callers need from the system to run the check. Programs are
/// absolute; implementations clear the environment (`LC_ALL=C`).
pub trait SystemdQuery {
    /// Run `prog args…`; `None` when it couldn't run. `(status, stdout)`.
    fn run(&self, prog: &str, args: &[String]) -> Option<(i32, Vec<u8>)>;
    fn uid_of_user(&self, user: &str) -> Option<u32>;
    /// The `runuser` fallback for user-scope daemons needs root. The
    /// engine (user `k2app`) can't, so it asks only the user bus.
    fn can_runuser(&self) -> bool;
}

/// The unit's main pid in its scope, through `systemctl show` (env
/// cleared, `LC_ALL=C`). `None` when systemd doesn't know it or it isn't
/// running.
///
/// User scope asks `systemctl --user -M <user>@` first (needs the user's
/// D-Bus session bus, `dbus-user-session`), then — root only — falls back
/// to the user manager's private socket: `runuser -u <user> -- env
/// XDG_RUNTIME_DIR=/run/user/<uid> systemctl --user show …` (server images
/// often have no user bus). Both ask the same manager — which the daemon
/// user itself controls, so user scope is weaker than system scope (any
/// process of that user can change what the unit runs).
pub fn main_pid(q: &dyn SystemdQuery, d: &DaemonEntry) -> Option<i32> {
    let show = ["show", "-p", "MainPID", "--value", d.unit.as_str()];
    let parse = |(status, stdout): (i32, Vec<u8>)| -> Option<i32> {
        if status != 0 {
            return None;
        }
        let pid: i32 = String::from_utf8_lossy(&stdout).trim().parse().ok()?;
        (pid > 0).then_some(pid)
    };
    if d.scope != "user" {
        let args: Vec<String> = show.iter().map(|s| s.to_string()).collect();
        return parse(q.run(SYSTEMCTL, &args)?);
    }
    let mut args: Vec<String> = vec!["--user".into(), "-M".into(), format!("{}@", d.user)];
    args.extend(show.iter().map(|s| s.to_string()));
    if let Some(pid) = q.run(SYSTEMCTL, &args).and_then(parse) {
        return Some(pid);
    }
    if !q.can_runuser() {
        return None;
    }
    let uid = q.uid_of_user(&d.user)?;
    let mut args: Vec<String> = vec![
        "-u".into(),
        d.user.clone(),
        "--".into(),
        "/usr/bin/env".into(),
        format!("XDG_RUNTIME_DIR=/run/user/{uid}"),
        SYSTEMCTL.into(),
        "--user".into(),
    ];
    args.extend(show.iter().map(|s| s.to_string()));
    parse(q.run(RUNUSER, &args)?)
}

/// The configured daemons whose user has this uid.
pub fn daemons_of_uid<'a>(q: &dyn SystemdQuery, daemons: &'a [DaemonEntry], uid: u32) -> Vec<&'a DaemonEntry> {
    daemons
        .iter()
        .filter(|d| q.uid_of_user(&d.user) == Some(uid))
        .collect()
}

/// The match both sides end on: `pid` (real uid `uid`) is the main
/// process of one configured daemon unit. `Ok(entry)` or a static reason.
pub fn match_main_pid<'a>(
    q: &dyn SystemdQuery,
    daemons: &'a [DaemonEntry],
    pid: i32,
    uid: u32,
) -> Result<&'a DaemonEntry, &'static str> {
    let mine = daemons_of_uid(q, daemons, uid);
    if mine.is_empty() {
        return Err("caller uid is not a daemon user");
    }
    mine.into_iter()
        .find(|d| main_pid(q, d) == Some(pid))
        .ok_or("caller is not the daemon unit's main process")
}

/// Read `daemon.toml` from a path with plain `std` I/O (the engine; the
/// helper reads it through its root-relative, no-follow `Dir`): a regular
/// file, never through a link, root-owned, not group/other-writable.
#[cfg(unix)]
pub fn load_daemon_toml_file(path: &std::path::Path) -> Result<Vec<DaemonEntry>, &'static str> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let f = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|_| "daemon.toml missing")?;
    let meta = f.metadata().map_err(|_| "daemon.toml unreadable")?;
    if !meta.is_file() {
        return Err("daemon.toml is not a regular file");
    }
    if meta.mode() & 0o022 != 0 {
        return Err("daemon.toml is group/other-writable");
    }
    if meta.uid() != 0 {
        return Err("daemon.toml is not root-owned");
    }
    let mut buf = Vec::new();
    f.take(DAEMON_TOML_MAX as u64 + 1)
        .read_to_end(&mut buf)
        .map_err(|_| "daemon.toml unreadable")?;
    if buf.len() > DAEMON_TOML_MAX {
        return Err("daemon.toml too large");
    }
    let text = std::str::from_utf8(&buf).map_err(|_| "daemon.toml unreadable")?;
    parse_daemon_toml(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

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

    struct Q {
        pids: HashMap<String, i32>,
        users: HashMap<String, u32>,
        user_bus: bool,
        root: bool,
        calls: std::cell::RefCell<Vec<(String, Vec<String>)>>,
    }

    impl SystemdQuery for Q {
        fn run(&self, prog: &str, args: &[String]) -> Option<(i32, Vec<u8>)> {
            self.calls.borrow_mut().push((prog.into(), args.to_vec()));
            if prog == SYSTEMCTL && args.iter().any(|a| a == "-M") && !self.user_bus {
                return Some((1, vec![]));
            }
            let unit = args.last()?;
            Some((0, format!("{}\n", self.pids.get(unit).copied().unwrap_or(0)).into_bytes()))
        }
        fn uid_of_user(&self, user: &str) -> Option<u32> {
            self.users.get(user).copied()
        }
        fn can_runuser(&self) -> bool {
            self.root
        }
    }

    fn q(user_bus: bool, root: bool) -> Q {
        Q {
            pids: [("k2-daemon.service".to_string(), 4242), ("k2-user.service".to_string(), 777)].into(),
            users: [("k2".to_string(), 1001), ("k2u".to_string(), 1002)].into(),
            user_bus,
            root,
            calls: Default::default(),
        }
    }

    #[test]
    fn main_pid_match() {
        let daemons = parse_daemon_toml(
            "[[daemon]]\nuser = \"k2\"\nunit = \"k2-daemon.service\"\nscope = \"system\"\n\
             [[daemon]]\nuser = \"k2u\"\nunit = \"k2-user.service\"\nscope = \"user\"\n",
        )
        .unwrap();
        let sys = q(false, false);
        assert_eq!(match_main_pid(&sys, &daemons, 4242, 1001).unwrap().unit, "k2-daemon.service");
        assert_eq!(match_main_pid(&sys, &daemons, 4243, 1001), Err("caller is not the daemon unit's main process"));
        assert_eq!(match_main_pid(&sys, &daemons, 4242, 1003), Err("caller uid is not a daemon user"));
        assert_eq!(match_main_pid(&sys, &daemons, 4242, 1002).map(|d| d.unit.as_str()), Err("caller is not the daemon unit's main process"));
        // User scope: the user bus, else runuser (root only).
        assert!(match_main_pid(&sys, &daemons, 777, 1002).is_err(), "no user bus, not root: refused");
        assert!(sys.calls.borrow().iter().all(|(p, _)| p == SYSTEMCTL), "never runuser without root");
        assert_eq!(match_main_pid(&q(true, false), &daemons, 777, 1002).unwrap().user, "k2u");
        let root = q(false, true);
        assert_eq!(match_main_pid(&root, &daemons, 777, 1002).unwrap().user, "k2u");
        assert!(root.calls.borrow().iter().any(|(p, _)| p == RUNUSER));
    }
}
