//! Preflight guard: never let Sew adopt a login account as its service
//! user (sewlab, 2026-10-16).
//!
//! `sew install` reuses an existing `sew` user. On sewlab ops had made a
//! `sew` login (with an SSH key) before enable; Sew adopted it, so that
//! key owned Sew's mgmt socket (full Sew admin) and sat in `sew-db` and
//! `sew-media`. Enable now refuses when the `sew` user exists and looks like
//! a login account:
//! - its shell is a login shell (not `nologin` / `false`);
//! - its password is not locked (`!` / `*`), read from `/etc/passwd` and,
//!   when the daemon can read it, `/etc/shadow` (the root helper has no
//!   read op for shadow, so an unprivileged daemon skips this one and says
//!   so);
//! - its home has `.ssh/authorized_keys` (or `authorized_keys2`), when the
//!   daemon can see into that home.
//!
//! A service account from a previous Sew install (nologin, locked, no keys)
//! passes, so re-enable and upgrade keep working. Read-only: nothing here
//! changes the box.

use std::path::Path;

use serde_json::{json, Value};

use super::ops::SewOps;

/// The account Sew runs as. Sew's installer picks it; K2's install inputs
/// carry no user name (`InstallRequest::sew_inputs`).
pub const SEW_SERVICE_USER: &str = "sew";
pub const PASSWD: &str = "/etc/passwd";
pub const SHADOW: &str = "/etc/shadow";
const MAX_DB: usize = 4 * 1024 * 1024;

/// One `/etc/passwd` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PwEntry {
    pub password: String,
    pub home: String,
    pub shell: String,
}

/// The `name` line of an `/etc/passwd` body.
pub fn passwd_entry(passwd: &str, name: &str) -> Option<PwEntry> {
    passwd.lines().find_map(|l| {
        let f: Vec<&str> = l.split(':').collect();
        (f.len() >= 7 && f[0] == name)
            .then(|| PwEntry { password: f[1].to_string(), home: f[5].to_string(), shell: f[6].trim().to_string() })
    })
}

/// The password field of the `name` line of an `/etc/shadow` body.
pub fn shadow_password(shadow: &str, name: &str) -> Option<String> {
    shadow.lines().find_map(|l| {
        let mut f = l.split(':');
        (f.next() == Some(name)).then(|| f.next().unwrap_or("").to_string())
    })
}

/// A shell that lets the account log in. Empty means `/bin/sh`.
pub fn is_login_shell(shell: &str) -> bool {
    let base = shell.rsplit('/').next().unwrap_or("");
    !matches!(base, "nologin" | "false")
}

fn locked(field: &str) -> bool {
    field.starts_with('!') || field.starts_with('*')
}

/// What the daemon learned about an existing account. `None` = it could
/// not tell (shadow or home unreadable without root).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub shell: String,
    pub home: String,
    /// The password is usable (set or empty, not `!`/`*`).
    pub unlocked: Option<bool>,
    /// `.ssh/authorized_keys` or `authorized_keys2` exists under its home.
    pub keys: Option<bool>,
}

impl Account {
    pub fn login_shell(&self) -> bool {
        is_login_shell(&self.shell)
    }
    pub fn is_login(&self) -> bool {
        self.login_shell() || self.unlocked == Some(true) || self.keys == Some(true)
    }
}

/// Look `name` up. `Err` = `/etc/passwd` unreadable; `Ok(None)` = no user.
pub fn probe(ops: &dyn SewOps, name: &str) -> Result<Option<Account>, String> {
    let passwd = ops.read_file(Path::new(PASSWD), MAX_DB)?;
    let Some(pw) = passwd_entry(&String::from_utf8_lossy(&passwd), name) else { return Ok(None) };
    let field = if pw.password == "x" {
        ops.read_file(Path::new(SHADOW), MAX_DB)
            .ok()
            .and_then(|s| shadow_password(&String::from_utf8_lossy(&s), name))
    } else {
        Some(pw.password.clone())
    };
    let unlocked = field.map(|f| !locked(&f));
    let keys = if pw.home.starts_with('/') {
        let home = pw.home.trim_end_matches('/');
        let probes: Vec<Option<bool>> = ["authorized_keys", "authorized_keys2"]
            .iter()
            .map(|f| ops.path_probe(&format!("{home}/.ssh/{f}")))
            .collect();
        if probes.contains(&Some(true)) {
            Some(true)
        } else if probes.contains(&None) {
            None
        } else {
            Some(false)
        }
    } else {
        Some(false)
    };
    Ok(Some(Account { shell: pw.shell, home: pw.home, unlocked, keys }))
}

fn check(status: &str, detail: String, hint: &str) -> Value {
    json!({"id": "service_user", "status": status, "detail": detail, "hint": hint})
}

/// The `service_user` preflight check; `fail` is a hard stop.
pub fn preflight_check(ops: &dyn SewOps) -> Value {
    let name = SEW_SERVICE_USER;
    let acct = match probe(ops, name) {
        Err(e) => {
            return check(
                "fail",
                format!("{PASSWD} is unreadable ({e}), so K2 cannot tell whether a login user named '{name}' exists"),
                "fix the permissions on /etc/passwd, then run enable again",
            )
        }
        Ok(None) => return check("ok", format!("no '{name}' user yet (Sew creates its own service account)"), ""),
        Ok(Some(a)) => a,
    };
    if acct.is_login() {
        let mut why = Vec::new();
        if acct.login_shell() {
            why.push(format!("shell {}", if acct.shell.is_empty() { "/bin/sh" } else { &acct.shell }));
        }
        if acct.unlocked == Some(true) {
            why.push("password not locked".to_string());
        }
        if acct.keys == Some(true) {
            why.push("SSH keys present".to_string());
        }
        return check(
            "fail",
            format!("A login user named '{name}' already exists on this box ({}).", why.join(", ")),
            "Sew would adopt it as its service account and give that login full Sew admin. \
             Rename or remove that user, then run enable again.",
        );
    }
    let mut detail = format!("'{name}' is a service account (shell {}", acct.shell);
    detail.push_str(match acct.unlocked {
        Some(_) => ", password locked",
        None => ", password lock not checked: /etc/shadow needs root",
    });
    detail.push_str(match acct.keys {
        Some(_) => ", no SSH keys",
        None => ", SSH keys not checked: its home is not readable by K2",
    });
    detail.push_str("); Sew reuses it");
    check("ok", detail, "")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sew::ops::fake::FakeOps;

    const ROOT: &str = "root:x:0:0:root:/root:/bin/bash\nk2:x:1001:1001::/home/k2:/bin/bash\n";

    fn box_with(passwd_extra: &str, shadow: Option<&str>) -> FakeOps {
        let ops = FakeOps::new();
        ops.files.lock().unwrap().insert(PASSWD.into(), format!("{ROOT}{passwd_extra}").into_bytes());
        match shadow {
            Some(s) => {
                ops.files.lock().unwrap().insert(SHADOW.into(), s.as_bytes().to_vec());
            }
            None => {
                ops.files.lock().unwrap().remove(SHADOW);
            }
        }
        ops
    }

    fn status(ops: &FakeOps) -> (String, String, String) {
        let c = preflight_check(ops);
        assert_eq!(c["id"], "service_user", "{c}");
        (c["status"].as_str().unwrap().into(), c["detail"].as_str().unwrap().into(), c["hint"].as_str().unwrap().into())
    }

    #[test]
    fn parses_passwd_and_shadow_lines() {
        let pw = passwd_entry("sewx:x:1:1::/x:/bin/sh\nsew:x:999:999:Sew:/var/lib/sew:/usr/sbin/nologin\n", "sew").unwrap();
        assert_eq!(pw, PwEntry { password: "x".into(), home: "/var/lib/sew".into(), shell: "/usr/sbin/nologin".into() });
        assert!(passwd_entry("sewx:x:1:1::/x:/bin/sh\n", "sew").is_none(), "prefix is not a match");
        assert_eq!(shadow_password("sew:!:19000:0:99999:7:::\n", "sew").as_deref(), Some("!"));
        assert!(shadow_password("other:!:1::::::\n", "sew").is_none());
        for s in ["/bin/bash", "/bin/sh", "", "/usr/bin/zsh", "/usr/bin/git-shell"] {
            assert!(is_login_shell(s), "{s:?} is a login shell");
        }
        for s in ["/usr/sbin/nologin", "/sbin/nologin", "/bin/false", "/usr/bin/false"] {
            assert!(!is_login_shell(s), "{s:?} is not a login shell");
        }
    }

    /// The sewlab case: bash, a password, an authorized key.
    #[test]
    fn a_login_user_named_sew_is_refused() {
        let ops = box_with("sew:x:1002:1002::/home/sew:/bin/bash\n", Some("sew:$6$salt$hash:19000:0:99999:7:::\n"));
        ops.exists.lock().unwrap().push("/home/sew/.ssh/authorized_keys".into());
        let (st, detail, hint) = status(&ops);
        assert_eq!(st, "fail");
        assert_eq!(
            detail,
            "A login user named 'sew' already exists on this box (shell /bin/bash, password not locked, SSH keys present)."
        );
        assert_eq!(
            hint,
            "Sew would adopt it as its service account and give that login full Sew admin. \
             Rename or remove that user, then run enable again."
        );
    }

    /// Each signal refuses on its own.
    #[test]
    fn any_one_login_signal_refuses() {
        // Shell only: shadow and home unreadable (an unprivileged daemon).
        let ops = box_with("sew:x:1002:1002::/home/sew:/bin/bash\n", None);
        ops.unreadable.lock().unwrap().push("/home/sew".into());
        let (st, detail, _) = status(&ops);
        assert_eq!(st, "fail");
        assert!(detail.ends_with("(shell /bin/bash)."), "{detail}");

        // Empty shell field = /bin/sh.
        let ops = box_with("sew:x:1002:1002::/home/sew:\n", Some("sew:!:1::::::\n"));
        let (st, detail, _) = status(&ops);
        assert_eq!(st, "fail");
        assert!(detail.contains("shell /bin/sh"), "{detail}");

        // Password only.
        let ops = box_with("sew:x:999:999::/var/lib/sew:/usr/sbin/nologin\n", Some("sew:$y$j9T$abc:1::::::\n"));
        let (st, detail, _) = status(&ops);
        assert_eq!(st, "fail");
        assert!(detail.ends_with("(password not locked)."), "{detail}");

        // An empty password in /etc/passwd itself (no shadow needed).
        let ops = box_with("sew::999:999::/var/lib/sew:/usr/sbin/nologin\n", None);
        let (st, detail, _) = status(&ops);
        assert_eq!(st, "fail");
        assert!(detail.contains("password not locked"), "{detail}");

        // Keys only (nologin still allows `ssh -N -L` to the mgmt socket).
        let ops = box_with("sew:x:999:999::/var/lib/sew:/usr/sbin/nologin\n", Some("sew:!:1::::::\n"));
        ops.exists.lock().unwrap().push("/var/lib/sew/.ssh/authorized_keys2".into());
        let (st, detail, _) = status(&ops);
        assert_eq!(st, "fail");
        assert!(detail.ends_with("(SSH keys present)."), "{detail}");
    }

    /// A service account from a previous Sew install: re-enable and upgrade
    /// keep working.
    #[test]
    fn a_nologin_locked_keyless_service_account_is_allowed() {
        for (shell, pw) in [("/usr/sbin/nologin", "!"), ("/bin/false", "*"), ("/sbin/nologin", "!*"), ("/usr/sbin/nologin", "!!")] {
            let ops = box_with(&format!("sew:x:999:999:Sew:/var/lib/sew:{shell}\n"), Some(&format!("sew:{pw}:19000::::::\n")));
            let (st, detail, _) = status(&ops);
            assert_eq!(st, "ok", "{shell} {pw}: {detail}");
            assert_eq!(detail, format!("'sew' is a service account (shell {shell}, password locked, no SSH keys); Sew reuses it"));
        }
        // Unprivileged daemon: shadow and home unreadable, said plainly.
        let ops = box_with("sew:x:999:999:Sew:/var/lib/sew:/usr/sbin/nologin\n", None);
        ops.unreadable.lock().unwrap().push("/var/lib/sew".into());
        let (st, detail, _) = status(&ops);
        assert_eq!(st, "ok", "{detail}");
        assert!(detail.contains("password lock not checked: /etc/shadow needs root"), "{detail}");
        assert!(detail.contains("SSH keys not checked"), "{detail}");
    }

    #[test]
    fn no_sew_user_is_allowed() {
        let ops = box_with("sewer:x:1003:1003::/home/sewer:/bin/bash\n", None);
        let (st, detail, _) = status(&ops);
        assert_eq!(st, "ok");
        assert!(detail.starts_with("no 'sew' user yet"), "{detail}");
    }

    /// Fail closed: without /etc/passwd K2 cannot rule the account out.
    #[test]
    fn an_unreadable_passwd_is_a_hard_stop() {
        let ops = FakeOps::new();
        ops.files.lock().unwrap().remove(PASSWD);
        let (st, detail, _) = status(&ops);
        assert_eq!(st, "fail");
        assert!(detail.contains("/etc/passwd is unreadable"), "{detail}");
    }
}
