//! argv + env parity with alacritty's `tty::new` (PRD P15, V31).
//!
//! In-process, alacritty spawns the child with `std::process::Command`, so
//! the child gets the **daemon's environment**, plus `ALACRITTY_WINDOW_ID`,
//! `USER`, `HOME` and `WINDOWID`, then the caller's env on top, minus
//! `XDG_ACTIVATION_TOKEN` / `DESKTOP_STARTUP_ID`
//! (`alacritty_terminal-0.26.0/src/tty/unix.rs:229-241`). ptyd passes its
//! env through verbatim and inherits nothing, so the daemon must send that
//! complete set. With no program, alacritty runs the user's login shell
//! (on macOS through `/usr/bin/login -flp`); [`default_shell_argv`] mirrors
//! it.

use std::collections::{BTreeMap, HashMap};
use std::ffi::CStr;

/// alacritty's `ShellUser`: USER / HOME / SHELL from the env, falling back
/// to the passwd entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellUser {
    pub user: String,
    pub home: String,
    pub shell: String,
}

impl ShellUser {
    pub fn from_env() -> std::io::Result<ShellUser> {
        let pw = passwd();
        let pick = |var: &str, f: fn(&Pw) -> String| -> std::io::Result<String> {
            match std::env::var(var) {
                Ok(v) => Ok(v),
                Err(_) => pw.as_ref().map(f).map_err(|e| std::io::Error::new(e.kind(), e.to_string())),
            }
        };
        Ok(ShellUser { user: pick("USER", |p| p.name.clone())?, home: pick("HOME", |p| p.dir.clone())?, shell: pick("SHELL", |p| p.shell.clone())? })
    }
}

struct Pw {
    name: String,
    dir: String,
    shell: String,
}

fn passwd() -> std::io::Result<Pw> {
    // SAFETY: getpwuid_r into stack buffers; strings copied out.
    unsafe {
        let mut buf = vec![0 as libc::c_char; 4096];
        let mut ent: libc::passwd = std::mem::zeroed();
        let mut res: *mut libc::passwd = std::ptr::null_mut();
        let rc = libc::getpwuid_r(libc::getuid(), &mut ent, buf.as_mut_ptr(), buf.len(), &mut res);
        if rc != 0 || res.is_null() {
            return Err(std::io::Error::other("getpwuid_r failed"));
        }
        let s = |p: *const libc::c_char| CStr::from_ptr(p).to_string_lossy().into_owned();
        Ok(Pw { name: s(ent.pw_name), dir: s(ent.pw_dir), shell: s(ent.pw_shell) })
    }
}

/// alacritty's `default_shell_command` (no program given).
pub fn default_shell_argv(user: &ShellUser) -> Vec<String> {
    #[cfg(target_os = "macos")]
    {
        let shell_name = user.shell.rsplit('/').next().unwrap_or(&user.shell);
        let exec = format!("exec -a -{} {}", shell_name, user.shell);
        let hush = std::path::Path::new(&user.home).join(".hushlogin").exists();
        let flags = if hush { "-qflp" } else { "-flp" };
        vec!["/usr/bin/login".into(), flags.into(), user.user.clone(), "/bin/zsh".into(), "-fc".into(), exec]
    }
    #[cfg(not(target_os = "macos"))]
    {
        vec![user.shell.clone()]
    }
}

/// The complete child env alacritty would produce, as sorted pairs.
/// `daemon_env` is the daemon's own environment (`std::env::vars_os`);
/// non-UTF-8 entries are dropped (the ptyd protocol is UTF-8 JSON).
pub fn child_env(
    daemon_env: impl IntoIterator<Item = (std::ffi::OsString, std::ffi::OsString)>,
    user: &ShellUser,
    config_env: &HashMap<String, String>,
) -> Vec<(String, String)> {
    let mut env: BTreeMap<String, String> = daemon_env
        .into_iter()
        .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
        .collect();
    env.insert("ALACRITTY_WINDOW_ID".into(), "0".into());
    env.insert("USER".into(), user.user.clone());
    env.insert("HOME".into(), user.home.clone());
    env.insert("WINDOWID".into(), "0".into());
    for (k, v) in config_env {
        env.insert(k.clone(), v.clone());
    }
    env.remove("XDG_ACTIVATION_TOKEN");
    env.remove("DESKTOP_STARTUP_ID");
    env.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn user() -> ShellUser {
        ShellUser { user: "u".into(), home: "/nonexistent-home".into(), shell: "/bin/zsh".into() }
    }

    #[test]
    fn additions_overrides_and_removals_match_alacritty_order() {
        let daemon = vec![
            (OsString::from("PATH"), OsString::from("/daemon")),
            (OsString::from("XDG_ACTIVATION_TOKEN"), OsString::from("x")),
            (OsString::from("DESKTOP_STARTUP_ID"), OsString::from("y")),
            (OsString::from("USER"), OsString::from("daemon-user")),
        ];
        let mut cfg = HashMap::new();
        cfg.insert("PATH".to_string(), "/child".to_string());
        cfg.insert("HOME".to_string(), "/caller-home".to_string());
        cfg.insert("DESKTOP_STARTUP_ID".to_string(), "from-caller".to_string());
        let env: HashMap<String, String> = child_env(daemon, &user(), &cfg).into_iter().collect();
        assert_eq!(env["PATH"], "/child", "caller env overrides the daemon env");
        assert_eq!(env["HOME"], "/caller-home", "caller env overrides alacritty's HOME");
        assert_eq!(env["USER"], "u", "alacritty's USER overrides the daemon's");
        assert_eq!(env["ALACRITTY_WINDOW_ID"], "0");
        assert_eq!(env["WINDOWID"], "0");
        assert!(!env.contains_key("XDG_ACTIVATION_TOKEN"));
        assert!(!env.contains_key("DESKTOP_STARTUP_ID"), "removed even when the caller set it (removal runs last)");
    }

    #[test]
    fn default_shell_matches_alacritty() {
        let argv = default_shell_argv(&user());
        if cfg!(target_os = "macos") {
            assert_eq!(argv, vec!["/usr/bin/login", "-flp", "u", "/bin/zsh", "-fc", "exec -a -zsh /bin/zsh"]);
        } else {
            assert_eq!(argv, vec!["/bin/zsh"]);
        }
    }
}
