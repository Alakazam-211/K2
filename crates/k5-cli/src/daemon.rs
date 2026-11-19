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
//!   with a workspace folder in it (not the user home), else the main
//!   folder of the git checkout here when it has one, else here. "Here"
//!   is the folder as the shell's `pwd` names it ([`logical_cwd`]: `PWD`
//!   when it names this folder, so a path through a link stays as it was
//!   typed). The shell looks for its two names; this takes k2-client's
//!   list ([`k2_client::paths::WORKSPACE_DOT_DIR_NAMES`]), the K5 name too.
//! - A passport the daemon minted again for this session is used from the
//!   start ([`Settings::client`]), as the shell's prelude does.
//!
//! Where this differs from the shell: the K2 home is k2-client's
//! ([`crate::home`]: the home override counts), and the shell's COMPAT-58
//! owner token in a K2 terminal is never used with a passport (PRD 13.9
//! #4, as in [`crate::transport`]).

use crate::transport::{Auth, Client, Passport, SecretsDir};
use k2_client::brand::PRODUCT_NAME;
use k2_client::env_names;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

/// The host when `HOST` is not set.
pub const DEFAULT_HOST: &str = "127.0.0.1";

/// The shell's settings for one run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub host: String,
    pub port: Option<u16>,
    /// The port as the shell reads it: the setting, else the text of
    /// `heartbeat.port` with its trailing newlines dropped. `None` when
    /// neither is there.
    pub port_text: Option<String>,
    /// `NO_OWNER_FALLBACK=1`: the virtual desktop's guard is on, and the
    /// owner token was not read.
    pub no_owner_fallback: bool,
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
    (!t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()))
        .then(|| t.parse().ok())
        .flatten()
}

/// The nearest folder from `cwd` up (itself included, `/` not) with a
/// workspace folder in it, other than `user_home`.
fn project_root(cwd: &Path, user_home: Option<&Path>) -> Option<PathBuf> {
    let mut dir = Some(cwd);
    while let Some(d) = dir {
        // `/` itself is never the project.
        d.parent()?;
        let marked = k2_client::paths::WORKSPACE_DOT_DIR_NAMES
            .iter()
            .any(|n| d.join(n).is_dir());
        if marked && Some(d) != user_home {
            return Some(d.to_path_buf());
        }
        dir = d.parent();
    }
    None
}

/// The settings from an environment lookup, the K2 home and the folder
/// the command runs in. No git checkout is asked about.
pub fn settings(lookup: impl Fn(&str) -> Option<OsString>, home: &Path, cwd: &Path) -> Settings {
    settings_with(lookup, home, cwd, |_| None)
}

/// [`settings`], with `main_root` giving the main folder of the git
/// checkout a folder is in ([`git_main_root`]), asked only when no folder
/// up from `cwd` is a workspace.
pub fn settings_with(
    lookup: impl Fn(&str) -> Option<OsString>,
    home: &Path,
    cwd: &Path,
    main_root: impl FnOnce(&Path) -> Option<PathBuf>,
) -> Settings {
    let text = |names: &[&'static str]| -> Option<String> {
        env_names::first_nonempty_os_in(names, &lookup)
            .map(|(_, v)| v.to_string_lossy().into_owned())
    };
    let port_text = text(env_names::PORT).or_else(|| cat(&home.join("heartbeat.port")));
    let port = port_text.as_deref().and_then(port_of);
    let host = text(env_names::HOST).unwrap_or_else(|| DEFAULT_HOST.to_string());
    let secrets_dir = env_names::first_nonempty_os_in(env_names::SECRETS_DIR, &lookup)
        .map(|(_, v)| PathBuf::from(v));
    let no_owner_fallback = text(env_names::NO_OWNER_FALLBACK).is_some_and(|v| v == "1");
    let mut owner_from_disk = false;
    let auth = match text(env_names::HOOK_TOKEN).and_then(Passport::new) {
        Some(passport) => {
            let sock = env_names::first_nonempty_os_in(env_names::HOOK_SOCK, &lookup)
                .map(|(_, v)| PathBuf::from(v));
            Auth::Passport { passport, sock }
        }
        None if no_owner_fallback => Auth::None,
        None => {
            let token = ["heartbeat.token", "daemon.token"]
                .iter()
                .find_map(|f| cat(&home.join(f)).filter(|t| !t.is_empty()));
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
        None => project_root(cwd, user_home.as_deref())
            .or_else(|| main_root(cwd).filter(|root| marked(root)))
            .unwrap_or_else(|| cwd.to_path_buf())
            .to_string_lossy()
            .into_owned(),
    };
    Settings {
        host,
        port,
        port_text,
        no_owner_fallback,
        project,
        auth,
        owner_from_disk,
        secrets_dir,
        home: home.to_path_buf(),
    }
}

/// A folder with a workspace folder in it.
fn marked(dir: &Path) -> bool {
    k2_client::paths::WORKSPACE_DOT_DIR_NAMES
        .iter()
        .any(|n| dir.join(n).is_dir())
}

/// The folder as the shell's `pwd` names it: `PWD` when it is an absolute
/// path that, with `.`, `..` and doubled `/` taken out, names this very
/// folder (bash's start-up rule); else the folder as the system names it.
pub fn logical_cwd(pwd: Option<OsString>) -> std::io::Result<PathBuf> {
    let physical = std::env::current_dir()?;
    Ok(pwd.and_then(|p| logical(Path::new(&p))).unwrap_or(physical))
}

/// `pwd` with `.`, `..` and doubled `/` taken out by its words alone, when
/// it is absolute and names the current folder.
#[cfg(unix)]
fn logical(pwd: &Path) -> Option<PathBuf> {
    use std::os::unix::fs::MetadataExt;
    use std::path::Component;
    if !pwd.is_absolute() {
        return None;
    }
    let mut out = PathBuf::new();
    for c in pwd.components() {
        match c {
            Component::RootDir | Component::Prefix(_) => out.push(c.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(w) => out.push(w),
        }
    }
    let (a, b) = (std::fs::metadata(&out).ok()?, std::fs::metadata(".").ok()?);
    (a.dev() == b.dev() && a.ino() == b.ino()).then_some(out)
}

/// Windows: the folder as the system names it.
#[cfg(not(unix))]
fn logical(_pwd: &Path) -> Option<PathBuf> {
    None
}

/// The git the shell may run (`_k2_git_usable`, `cli/src/prelude.sh`):
/// the first `git` on `PATH`, unless it is macOS's `/usr/bin/git` with no
/// developer tools behind it (that one opens Apple's install dialog);
/// then the developer tools folder ([`developer_dir`]) must have a real
/// git in it. Found once per run.
pub fn usable_git() -> Option<&'static Path> {
    static GIT: OnceLock<Option<PathBuf>> = OnceLock::new();
    GIT.get_or_init(|| {
        let git = find_on_path("git", std::env::var_os("PATH")?)?;
        if cfg!(target_os = "macos") && git == Path::new("/usr/bin/git") {
            let dev = developer_dir()?;
            if !is_executable(&dev.join("usr/bin/git")) {
                return None;
            }
        }
        Some(git)
    })
    .as_deref()
}

/// The variable that picks the developer tools folder for this process
/// (Apple's: `xcode-select -p` and the `/usr/bin` stand-ins read it).
pub const DEVELOPER_DIR_VAR: &str = "DEVELOPER_DIR";

/// The places `xcode-select -p` looks for the developer tools folder,
/// after [`DEVELOPER_DIR_VAR`], in its order.
#[derive(Debug, Clone, Copy)]
pub struct DeveloperPlaces<'a> {
    /// The link `xcode-select -s` writes.
    pub link: &'a Path,
    /// Xcode's own folder, when nothing was chosen.
    pub xcode: &'a Path,
    /// The command line tools, when there is no Xcode.
    pub command_line_tools: &'a Path,
}

/// macOS's places.
pub fn developer_places() -> DeveloperPlaces<'static> {
    DeveloperPlaces {
        link: Path::new("/var/db/xcode_select_link"),
        xcode: Path::new("/Applications/Xcode.app/Contents/Developer"),
        command_line_tools: Path::new("/Library/Developer/CommandLineTools"),
    }
}

/// macOS: the developer tools folder (Xcode's or the command line
/// tools'), as `xcode-select -p` names it, or `None` when there is none
/// (they are not installed). Found by looking, not by starting
/// `xcode-select` (about 3 ms on a Mac): [`developer_dir_in`] with this
/// process's [`DEVELOPER_DIR_VAR`]. Nothing in `/usr/bin` that stands in
/// for a developer tool is ever run to find out. Found once per run.
pub fn developer_dir() -> Option<&'static Path> {
    static DEV: OnceLock<Option<PathBuf>> = OnceLock::new();
    DEV.get_or_init(|| {
        developer_dir_in(
            std::env::var_os(DEVELOPER_DIR_VAR).as_deref(),
            &developer_places(),
        )
    })
    .as_deref()
}

/// The developer tools folder for `var` (the value of
/// [`DEVELOPER_DIR_VAR`]) and `places`, as `xcode-select -p` names it:
///
/// 1. `var` when it is set and not empty, as written (a last `/` taken
///    off), whether or not it is a folder: `xcode-select -p` prints it
///    as it is, and the stand-ins use it as it is;
/// 2. else the target of `places.link` when there is that link, as it is;
/// 3. else `places.xcode`, then `places.command_line_tools`, the first
///    that is a folder;
/// 4. else none.
pub fn developer_dir_in(var: Option<&OsStr>, places: &DeveloperPlaces) -> Option<PathBuf> {
    if let Some(v) = var.filter(|v| !v.is_empty()) {
        return Some(without_last_slashes(v));
    }
    if let Ok(target) = std::fs::read_link(places.link) {
        return Some(match places.link.parent() {
            Some(parent) if target.is_relative() => parent.join(target),
            _ => target,
        });
    }
    [places.xcode, places.command_line_tools]
        .into_iter()
        .find(|p| p.is_dir())
        .map(Path::to_path_buf)
}

/// `v` with its last `/`s taken off (`/` alone stays).
fn without_last_slashes(v: &OsStr) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let b = v.as_bytes();
        let mut end = b.len();
        while end > 1 && b[end - 1] == b'/' {
            end -= 1;
        }
        PathBuf::from(OsStr::from_bytes(&b[..end]))
    }
    #[cfg(not(unix))]
    {
        PathBuf::from(v)
    }
}

/// The first executable file `name` in the folders of `path` (an empty
/// folder name is the current folder), as `command -v` finds it.
pub fn find_on_path(name: &str, path: OsString) -> Option<PathBuf> {
    std::env::split_paths(&path)
        .map(|dir| {
            if dir.as_os_str().is_empty() {
                PathBuf::from(".").join(name)
            } else {
                dir.join(name)
            }
        })
        .find(|p| is_executable(p))
}

/// A file this user may run, as `command -v` takes it.
#[cfg(unix)]
pub fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// A file (Windows has no mode bits to read).
#[cfg(not(unix))]
pub fn is_executable(p: &Path) -> bool {
    p.is_file()
}

/// The main folder of the git checkout `dir` is in, as the shell finds it:
/// `git -C <dir> rev-parse --path-format=absolute --git-common-dir`, with
/// a last `/` + one character + `git` taken off each line (its `sed
/// 's|/.git$||'`), when [`usable_git`] says git may run.
pub fn git_main_root(dir: &Path) -> Option<PathBuf> {
    let git = usable_git()?;
    let out = Command::new(git)
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = text.split('\n').map(strip_dot_git).collect();
    let root = lines.join("\n");
    let root = root.trim_end_matches('\n');
    (!root.is_empty()).then(|| PathBuf::from(root))
}

/// `sed 's|/.git$||'` on one line: `.` is any one character.
fn strip_dot_git(line: &str) -> &str {
    let chars: Vec<(usize, char)> = line.char_indices().collect();
    let n = chars.len();
    if n >= 5 && chars[n - 5].1 == '/' && line[chars[n - 3].0..] == *"git" {
        &line[..chars[n - 5].0]
    } else {
        line
    }
}

impl Settings {
    /// A client for these settings. `would_refuse`: the passport rule's
    /// count ([`crate::passport`]). A passport minted again for this
    /// session is taken at once ([`Client::use_healed_passport`]), as the
    /// shell's prelude does.
    pub fn client(&self, would_refuse: bool) -> Result<Client, NoPort> {
        let port = self.port.ok_or_else(|| NoPort {
            home: self.home.clone(),
        })?;
        Ok(self.client_on(port, would_refuse))
    }

    /// [`Settings::client`] on `port`, whatever the settings say: for a
    /// call that goes on the cell socket with no port set, or a port its
    /// caller read by its own rule.
    pub fn client_on(&self, port: u16, would_refuse: bool) -> Client {
        let mut c = Client::new(
            self.host.clone(),
            port,
            self.project.clone(),
            self.auth.clone(),
        );
        c.would_refuse = would_refuse;
        c.owner_from_disk = self.owner_from_disk;
        c.port_file = Some(self.home.join("daemon.port"));
        c.healer = self
            .secrets_dir
            .clone()
            .map(|d| Box::new(SecretsDir(d)) as Box<dyn crate::transport::Healer>);
        c.use_healed_passport();
        c
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
    let cwd = logical_cwd(std::env::var_os("PWD")).map_err(SettingsError::Cwd)?;
    Ok(settings_with(
        |name| std::env::var_os(name),
        &home,
        &cwd,
        git_main_root,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn scratch(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "k5-daemon-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).unwrap();
        }
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let map: HashMap<String, OsString> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), OsString::from(v)))
            .collect();
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
        assert_eq!(
            settings(env(&[("K2_PORT", "4200")]), &dir, &dir).port,
            Some(4200)
        );
        assert_eq!(
            settings(env(&[("K5_PORT", "4300"), ("K2_PORT", "4200")]), &dir, &dir).port,
            Some(4300)
        );
        assert_eq!(
            settings(env(&[("K5_PORT", ""), ("K2_PORT", "4200")]), &dir, &dir).port,
            Some(4200)
        );
        // A setting that is not a port is no port: the file is not read.
        assert_eq!(settings(env(&[("K2_PORT", "42a")]), &dir, &dir).port, None);
        std::fs::write(dir.join("heartbeat.port"), "99999").unwrap();
        assert_eq!(settings(env(&[]), &dir, &dir).port, None);
        assert_eq!(settings(env(&[]), &dir, &dir).host, DEFAULT_HOST);
        assert_eq!(
            settings(env(&[("K2_HOST", "10.0.0.2")]), &dir, &dir).host,
            "10.0.0.2"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_passport_never_reads_the_owner_token() {
        let dir = scratch("pass");
        std::fs::write(dir.join("heartbeat.token"), "owner-tok\n").unwrap();
        let s = settings(
            env(&[
                ("K2_HOOK_TOKEN", "s.x"),
                ("K2_HOOK_SOCK", "/tmp/s.sock"),
                ("K2_SECRETS_DIR", "/sec"),
            ]),
            &dir,
            &dir,
        );
        assert_eq!(
            s.auth,
            Auth::Passport {
                passport: Passport::new("s.x").unwrap(),
                sock: Some(PathBuf::from("/tmp/s.sock"))
            }
        );
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
        assert_eq!(
            settings(env(&[]), &dir, &dir).auth,
            Auth::Owner("daemon-tok".into())
        );
        std::fs::write(dir.join("heartbeat.token"), "hb-tok").unwrap();
        assert_eq!(
            settings(env(&[]), &dir, &dir).auth,
            Auth::Owner("hb-tok".into())
        );
        // The virtual desktop's guard: no owner token, no credential.
        let s = settings(env(&[("K5_NO_OWNER_FALLBACK", "1")]), &dir, &dir);
        assert_eq!(s.auth, Auth::None);
        assert!(!s.owner_from_disk);
        assert_eq!(
            settings(env(&[("K2_NO_OWNER_FALLBACK", "0")]), &dir, &dir).auth,
            Auth::Owner("hb-tok".into())
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_client_carries_the_settings() {
        let dir = scratch("client");
        std::fs::write(dir.join("heartbeat.token"), "hb-tok").unwrap();
        let s = settings(
            env(&[("K2_PORT", "4100"), ("K2_PROJECT_PATH", "/w")]),
            &dir,
            &dir,
        );
        let c = s.client(true).unwrap();
        assert_eq!(
            (c.host.as_str(), c.port, c.project.as_str()),
            (DEFAULT_HOST, 4100, "/w")
        );
        assert!(c.would_refuse && c.owner_from_disk);
        assert_eq!(c.port_file, Some(dir.join("daemon.port")));
        assert!(c.healer.is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_port_as_the_shell_reads_it() {
        let dir = scratch("porttext");
        assert_eq!(settings(env(&[]), &dir, &dir).port_text, None);
        std::fs::write(dir.join("heartbeat.port"), "4100\n\n").unwrap();
        let s = settings(env(&[]), &dir, &dir);
        assert_eq!((s.port_text.as_deref(), s.port), (Some("4100"), Some(4100)));
        let s = settings(env(&[("K2_PORT", "42a")]), &dir, &dir);
        assert_eq!((s.port_text.as_deref(), s.port), (Some("42a"), None));
        assert!(settings(env(&[("K2_NO_OWNER_FALLBACK", "1")]), &dir, &dir).no_owner_fallback);
        assert!(
            !settings(
                env(&[("K5_NO_OWNER_FALLBACK", "0"), ("K2_NO_OWNER_FALLBACK", "1")]),
                &dir,
                &dir
            )
            .no_owner_fallback
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_sed_that_drops_dot_git() {
        for (line, want) in [
            ("/repo/.git", "/repo"),
            ("/repo/xgit", "/repo"),
            ("/repo/.gitx", "/repo/.gitx"),
            ("/.git", ""),
            ("/repo/é.git", "/repo/é.git"),
            ("/repo/égit", "/repo"),
            ("git", "git"),
            ("", ""),
        ] {
            assert_eq!(strip_dot_git(line), want, "{line:?}");
        }
    }

    /// No workspace folder up from here: the git checkout's main folder,
    /// when it has one; else here.
    #[test]
    fn a_worktree_finds_its_main_folder() {
        let dir = std::fs::canonicalize(scratch("wt")).unwrap();
        let main = dir.join("main");
        let wt = dir.join("wt/sub");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::create_dir_all(&main).unwrap();
        let ask = |root: Option<PathBuf>| settings_with(env(&[]), &dir, &wt, move |_| root).project;
        assert_eq!(
            ask(Some(main.clone())),
            wt.to_string_lossy(),
            "the main folder is no workspace"
        );
        std::fs::create_dir_all(main.join(".k2")).unwrap();
        assert_eq!(ask(Some(main.clone())), main.to_string_lossy());
        assert_eq!(ask(None), wt.to_string_lossy());
        // A workspace up from here wins; git is not asked.
        std::fs::create_dir_all(dir.join("wt/.k2so")).unwrap();
        let s = settings_with(env(&[]), &dir, &wt, |_| panic!("asked git"));
        assert_eq!(s.project, dir.join("wt").to_string_lossy());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn pwd_counts_when_it_names_this_folder() {
        let here = std::env::current_dir().unwrap();
        assert_eq!(logical_cwd(None).unwrap(), here);
        assert_eq!(logical_cwd(Some("relative".into())).unwrap(), here);
        assert_eq!(
            logical_cwd(Some("/nonexistent-k5-dir".into())).unwrap(),
            here
        );
        let dotted = format!("{}/./x/..//", here.display());
        assert_eq!(logical_cwd(Some(dotted.into())).unwrap(), here);
        // A link to this folder is kept as written.
        let dir = scratch("pwdlink");
        let link = dir.join("link");
        std::os::unix::fs::symlink(&here, &link).unwrap();
        assert_eq!(logical_cwd(Some(link.clone().into())).unwrap(), link);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_project_is_the_nearest_marked_folder() {
        let dir = std::fs::canonicalize(scratch("proj")).unwrap();
        let deep = dir.join("ws/a/b");
        std::fs::create_dir_all(&deep).unwrap();
        assert_eq!(
            settings(env(&[]), &dir, &deep).project,
            deep.to_string_lossy()
        );
        std::fs::create_dir_all(dir.join("ws/.k2")).unwrap();
        assert_eq!(
            settings(env(&[]), &dir, &deep).project,
            dir.join("ws").to_string_lossy()
        );
        std::fs::create_dir_all(dir.join("ws/a/.k2so")).unwrap();
        assert_eq!(
            settings(env(&[]), &dir, &deep).project,
            dir.join("ws/a").to_string_lossy()
        );
        // The user home is never the project; the search goes on above it.
        let home = dir.join("ws/a");
        let home_s = home.to_string_lossy().into_owned();
        assert_eq!(
            settings(env(&[("HOME", &home_s)]), &dir, &deep).project,
            dir.join("ws").to_string_lossy()
        );
        assert_eq!(
            settings(env(&[("K2_PROJECT_PATH", "/set")]), &dir, &deep).project,
            "/set"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The places of one test, all under `dir`.
    fn places(dir: &Path) -> (PathBuf, PathBuf, PathBuf) {
        (
            dir.join("db/xcode_select_link"),
            dir.join("Xcode.app/Contents/Developer"),
            dir.join("CommandLineTools"),
        )
    }

    #[cfg(unix)]
    #[test]
    fn the_developer_folder_in_xcode_select_s_order() {
        let dir = scratch("developer");
        let (link, xcode, clt) = places(&dir);
        let at = DeveloperPlaces {
            link: &link,
            xcode: &xcode,
            command_line_tools: &clt,
        };
        let find = |var: Option<&str>| developer_dir_in(var.map(OsStr::new), &at);
        // Nothing installed: none.
        assert_eq!(find(None), None);
        // The command line tools, then Xcode before them.
        std::fs::create_dir_all(&clt).unwrap();
        assert_eq!(find(None), Some(clt.clone()));
        std::fs::create_dir_all(&xcode).unwrap();
        assert_eq!(find(None), Some(xcode.clone()));
        // The link `xcode-select -s` writes beats both; a relative target
        // is read from the link's folder.
        let chosen = dir.join("chosen");
        std::fs::create_dir_all(&chosen).unwrap();
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&chosen, &link).unwrap();
        assert_eq!(find(None), Some(chosen.clone()));
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink("../chosen", &link).unwrap();
        assert_eq!(find(None), Some(dir.join("db/../chosen")));
        // A link to nothing is still the answer, as `xcode-select -p`
        // prints it: no Xcode or tools folder is tried after it.
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(dir.join("gone"), &link).unwrap();
        assert_eq!(find(None), Some(dir.join("gone")));
        // DEVELOPER_DIR beats them all, as written (a last `/` off),
        // folder or not; empty counts as unset.
        assert_eq!(find(Some(clt.to_str().unwrap())), Some(clt.clone()));
        let slash = format!("{}//", clt.to_str().unwrap());
        assert_eq!(find(Some(&slash)), Some(clt.clone()));
        let missing = dir.join("missing");
        assert_eq!(find(Some(missing.to_str().unwrap())), Some(missing));
        assert_eq!(
            find(Some("relative/dir")),
            Some(PathBuf::from("relative/dir"))
        );
        assert_eq!(find(Some("/")), Some(PathBuf::from("/")));
        assert_eq!(find(Some("")), Some(dir.join("gone")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// On a Mac, the lookup names the folder `xcode-select -p` prints, for
    /// this machine and for each kind of DEVELOPER_DIR. `xcode-select -p`
    /// never opens Apple's install dialog, so running it here is safe.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_developer_folder_is_the_one_xcode_select_prints() {
        let dir = scratch("xcode-select");
        let missing = dir.join("missing");
        let cases: Vec<Option<&str>> = vec![
            None,
            Some(""),
            Some(dir.to_str().unwrap()),
            Some(missing.to_str().unwrap()),
            Some("relative/dir"),
            Some("/Library/Developer/CommandLineTools/"),
        ];
        for var in cases {
            let mut cmd = Command::new("/usr/bin/xcode-select");
            cmd.arg("-p").stdin(Stdio::null()).stderr(Stdio::null());
            cmd.env_remove(DEVELOPER_DIR_VAR);
            if let Some(v) = var {
                cmd.env(DEVELOPER_DIR_VAR, v);
            }
            let out = cmd.output().expect("xcode-select runs");
            let printed = String::from_utf8(out.stdout).unwrap();
            let printed = printed.trim_end_matches('\n');
            let want = (!printed.is_empty()).then(|| PathBuf::from(printed));
            assert_eq!(
                developer_dir_in(var.map(OsStr::new), &developer_places()),
                want,
                "DEVELOPER_DIR {var:?}"
            );
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
