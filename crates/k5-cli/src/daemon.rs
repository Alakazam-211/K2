//! Where the daemon is and what the binary's own requests carry to it:
//! the shell CLI's connection settings (`cli/src/prelude.sh`), read for
//! the verbs the binary answers itself.
//!
//! - The port: `PORT` (its K5_, K2_ and K2SO_ names, empty counts as
//!   unset), else the K2 home's `heartbeat.port`. None at all means no
//!   daemon to reach ([`NoPort`]).
//! - The host: `HOST`, else `127.0.0.1`.
//! - A passport: `HOOK_TOKEN`, with the cell socket at `HOOK_SOCK` and
//!   its re-minted copy in `SECRETS_DIR` ([`crate::transport::SecretsDir`]).
//! - Else the owner token from the K2 home's `heartbeat.token`, then
//!   `daemon.token` (`$(cat …)`: trailing newlines dropped, empty counts
//!   as missing), and never when `NO_OWNER_FALLBACK=1` (the virtual
//!   desktop's guard). Such a call says `X-K5-Owner-Source: disk`.
//! - Else no credential: only a public read goes out.
//! - The project: `PROJECT_PATH`, else the nearest folder up from here
//!   with a workspace folder in it (not the user home), else here. The
//!   shell looks for its two names; this takes k2-client's list
//!   ([`k2_client::paths::WORKSPACE_DOT_DIR_NAMES`]), the K5 name too.
//!
//! Where this differs from the shell: the K2 home is k2-client's
//! ([`crate::home`]: the home override counts), the shell's COMPAT-58
//! owner token in a K2 terminal is never used with a passport (PRD 13.9
//! #4, as in [`crate::transport`]), and the project search does not ask
//! git for a worktree's main folder. The verbs that use this today read
//! nothing a project changes.

use crate::transport::{Auth, Client, Passport, SecretsDir};
use k2_client::brand::PRODUCT_NAME;
use k2_client::env_names;
use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};

/// The host when `HOST` is not set.
pub const DEFAULT_HOST: &str = "127.0.0.1";

/// The shell's settings for one run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub host: String,
    pub port: Option<u16>,
    pub project: String,
    pub auth: Auth,
    /// The owner token was read from the K2 home's files.
    pub owner_from_disk: bool,
    /// `SECRETS_DIR`, where a refused passport may have been minted again.
    pub secrets_dir: Option<PathBuf>,
    /// The K2 home the files were read from.
    pub home: PathBuf,
}

/// No port: nothing to connect to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoPort {
    pub home: PathBuf,
}

impl fmt::Display for NoPort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "can't reach {PRODUCT_NAME}'s server: no port in K5_PORT or {}",
            self.home.join("heartbeat.port").display()
        )
    }
}

impl std::error::Error for NoPort {}

/// A file's text as `$(cat file)` gives it, when it is a readable file:
/// its trailing newlines dropped.
fn cat(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    Some(text.trim_end_matches('\n').to_string())
}

fn port_of(text: &str) -> Option<u16> {
    let t = text.trim();
    (!t.is_empty() && t.bytes().all(|b| b.is_ascii_digit())).then(|| t.parse().ok()).flatten()
}

/// The nearest folder from `cwd` up (itself included, `/` not) with a
/// workspace folder in it, other than `user_home`.
fn project_root(cwd: &Path, user_home: Option<&Path>) -> Option<PathBuf> {
    let mut dir = Some(cwd);
    while let Some(d) = dir {
        // `/` itself is never the project.
        d.parent()?;
        let marked = k2_client::paths::WORKSPACE_DOT_DIR_NAMES.iter().any(|n| d.join(n).is_dir());
        if marked && Some(d) != user_home {
            return Some(d.to_path_buf());
        }
        dir = d.parent();
    }
    None
}

/// The settings from an environment lookup, the K2 home and the folder
/// the command runs in.
pub fn settings(lookup: impl Fn(&str) -> Option<OsString>, home: &Path, cwd: &Path) -> Settings {
    let text = |names: &[&'static str]| -> Option<String> {
        env_names::first_nonempty_os_in(names, &lookup).map(|(_, v)| v.to_string_lossy().into_owned())
    };
    let port = match text(env_names::PORT) {
        Some(p) => port_of(&p),
        None => cat(&home.join("heartbeat.port")).as_deref().and_then(port_of),
    };
    let host = text(env_names::HOST).unwrap_or_else(|| DEFAULT_HOST.to_string());
    let secrets_dir = env_names::first_nonempty_os_in(env_names::SECRETS_DIR, &lookup).map(|(_, v)| PathBuf::from(v));
    let no_owner_fallback = text(env_names::NO_OWNER_FALLBACK).is_some_and(|v| v == "1");
    let mut owner_from_disk = false;
    let auth = match text(env_names::HOOK_TOKEN).and_then(Passport::new) {
        Some(passport) => {
            let sock = env_names::first_nonempty_os_in(env_names::HOOK_SOCK, &lookup).map(|(_, v)| PathBuf::from(v));
            Auth::Passport { passport, sock }
        }
        None if no_owner_fallback => Auth::None,
        None => {
            let token = ["heartbeat.token", "daemon.token"].iter().find_map(|f| cat(&home.join(f)).filter(|t| !t.is_empty()));
            match token {
                Some(t) => {
                    owner_from_disk = true;
                    Auth::Owner(t)
                }
                None => Auth::None,
            }
        }
    };
    let user_home = lookup("HOME").filter(|h| !h.is_empty()).map(PathBuf::from);
    let project = match text(env_names::PROJECT_PATH) {
        Some(p) => p,
        None => project_root(cwd, user_home.as_deref()).unwrap_or_else(|| cwd.to_path_buf()).to_string_lossy().into_owned(),
    };
    Settings { host, port, project, auth, owner_from_disk, secrets_dir, home: home.to_path_buf() }
}

impl Settings {
    /// A client for these settings. `would_refuse`: the passport rule's
    /// count ([`crate::passport`]).
    pub fn client(&self, would_refuse: bool) -> Result<Client, NoPort> {
        let port = self.port.ok_or_else(|| NoPort { home: self.home.clone() })?;
        let mut c = Client::new(self.host.clone(), port, self.project.clone(), self.auth.clone());
        c.would_refuse = would_refuse;
        c.owner_from_disk = self.owner_from_disk;
        c.port_file = Some(self.home.join("daemon.port"));
        c.healer = self.secrets_dir.clone().map(|d| Box::new(SecretsDir(d)) as Box<dyn crate::transport::Healer>);
        Ok(c)
    }
}

/// Why the settings can't be read.
#[derive(Debug)]
pub enum SettingsError {
    Home(crate::home::HomeError),
    Cwd(std::io::Error),
}

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SettingsError::Home(e) => e.fmt(f),
            SettingsError::Cwd(e) => write!(f, "can't read the current folder: {e}"),
        }
    }
}

impl std::error::Error for SettingsError {}

/// The settings of this process.
pub fn current() -> Result<Settings, SettingsError> {
    let home = crate::home::k2_home().map_err(SettingsError::Home)?;
    let cwd = std::env::current_dir().map_err(SettingsError::Cwd)?;
    Ok(settings(|name| std::env::var_os(name), &home, &cwd))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn scratch(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!("k5-daemon-{tag}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).unwrap();
        }
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let map: HashMap<String, OsString> = pairs.iter().map(|(k, v)| (k.to_string(), OsString::from(v))).collect();
        move |name: &str| map.get(name).cloned()
    }

    #[test]
    fn the_port_from_the_setting_then_the_file() {
        let dir = scratch("port");
        let s = settings(env(&[]), &dir, &dir);
        assert_eq!(s.port, None);
        assert_eq!(s.client(false).err(), Some(NoPort { home: dir.clone() }));
        std::fs::write(dir.join("heartbeat.port"), "4100\n").unwrap();
        assert_eq!(settings(env(&[]), &dir, &dir).port, Some(4100));
        // The setting wins, under its newest name first; empty is unset.
        assert_eq!(settings(env(&[("K2_PORT", "4200")]), &dir, &dir).port, Some(4200));
        assert_eq!(settings(env(&[("K5_PORT", "4300"), ("K2_PORT", "4200")]), &dir, &dir).port, Some(4300));
        assert_eq!(settings(env(&[("K5_PORT", ""), ("K2_PORT", "4200")]), &dir, &dir).port, Some(4200));
        // A setting that is not a port is no port: the file is not read.
        assert_eq!(settings(env(&[("K2_PORT", "42a")]), &dir, &dir).port, None);
        std::fs::write(dir.join("heartbeat.port"), "99999").unwrap();
        assert_eq!(settings(env(&[]), &dir, &dir).port, None);
        assert_eq!(settings(env(&[]), &dir, &dir).host, DEFAULT_HOST);
        assert_eq!(settings(env(&[("K2_HOST", "10.0.0.2")]), &dir, &dir).host, "10.0.0.2");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_passport_never_reads_the_owner_token() {
        let dir = scratch("pass");
        std::fs::write(dir.join("heartbeat.token"), "owner-tok\n").unwrap();
        let s = settings(env(&[("K2_HOOK_TOKEN", "s.x"), ("K2_HOOK_SOCK", "/tmp/s.sock"), ("K2_SECRETS_DIR", "/sec")]), &dir, &dir);
        assert_eq!(s.auth, Auth::Passport { passport: Passport::new("s.x").unwrap(), sock: Some(PathBuf::from("/tmp/s.sock")) });
        assert!(!s.owner_from_disk);
        assert_eq!(s.secrets_dir, Some(PathBuf::from("/sec")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_owner_token_from_disk() {
        let dir = scratch("owner");
        assert_eq!(settings(env(&[]), &dir, &dir).auth, Auth::None);
        std::fs::write(dir.join("daemon.token"), "daemon-tok\n\n").unwrap();
        let s = settings(env(&[("K2_HOOK_TOKEN", "")]), &dir, &dir);
        assert_eq!(s.auth, Auth::Owner("daemon-tok".into()));
        assert!(s.owner_from_disk);
        // heartbeat.token first; an empty one counts as missing.
        std::fs::write(dir.join("heartbeat.token"), "\n").unwrap();
        assert_eq!(settings(env(&[]), &dir, &dir).auth, Auth::Owner("daemon-tok".into()));
        std::fs::write(dir.join("heartbeat.token"), "hb-tok").unwrap();
        assert_eq!(settings(env(&[]), &dir, &dir).auth, Auth::Owner("hb-tok".into()));
        // The virtual desktop's guard: no owner token, no credential.
        let s = settings(env(&[("K5_NO_OWNER_FALLBACK", "1")]), &dir, &dir);
        assert_eq!(s.auth, Auth::None);
        assert!(!s.owner_from_disk);
        assert_eq!(settings(env(&[("K2_NO_OWNER_FALLBACK", "0")]), &dir, &dir).auth, Auth::Owner("hb-tok".into()));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_client_carries_the_settings() {
        let dir = scratch("client");
        std::fs::write(dir.join("heartbeat.token"), "hb-tok").unwrap();
        let s = settings(env(&[("K2_PORT", "4100"), ("K2_PROJECT_PATH", "/w")]), &dir, &dir);
        let c = s.client(true).unwrap();
        assert_eq!((c.host.as_str(), c.port, c.project.as_str()), (DEFAULT_HOST, 4100, "/w"));
        assert!(c.would_refuse && c.owner_from_disk);
        assert_eq!(c.port_file, Some(dir.join("daemon.port")));
        assert!(c.healer.is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_project_is_the_nearest_marked_folder() {
        let dir = std::fs::canonicalize(scratch("proj")).unwrap();
        let deep = dir.join("ws/a/b");
        std::fs::create_dir_all(&deep).unwrap();
        assert_eq!(settings(env(&[]), &dir, &deep).project, deep.to_string_lossy());
        std::fs::create_dir_all(dir.join("ws/.k2")).unwrap();
        assert_eq!(settings(env(&[]), &dir, &deep).project, dir.join("ws").to_string_lossy());
        std::fs::create_dir_all(dir.join("ws/a/.k2so")).unwrap();
        assert_eq!(settings(env(&[]), &dir, &deep).project, dir.join("ws/a").to_string_lossy());
        // The user home is never the project; the search goes on above it.
        let home = dir.join("ws/a");
        let home_s = home.to_string_lossy().into_owned();
        assert_eq!(settings(env(&[("HOME", &home_s)]), &dir, &deep).project, dir.join("ws").to_string_lossy());
        assert_eq!(settings(env(&[("K2_PROJECT_PATH", "/set")]), &dir, &deep).project, "/set");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
