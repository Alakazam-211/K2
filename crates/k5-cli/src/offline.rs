//! What the shell CLI does before it prints an offline page (a study or
//! help page), so the front can print the page instead only when the
//! shell would have printed nothing else and stopped nowhere.
//!
//! The shell's prelude (`cli/src/prelude.sh`) and its gates
//! (`cli/src/cli-tools-policy.sh`) run before every command. For a page,
//! all of it is quiet except:
//!
//! - what bash itself reads at start: `BASH_ENV`, `ENV`, `SHELLOPTS`,
//!   `BASHOPTS`, `POSIXLY_CORRECT` and exported functions
//!   (`BASH_FUNC_*`) change what runs, so any of them set hands the page
//!   to the shell;
//! - `set -u` on `$HOME`, and the programs the prelude starts from `PATH`
//!   (`cat`, `dirname`, `sed`, `head`): one that is missing prints an
//!   error or stops the run;
//! - the port and token files, read as `$(cat …)` under `set -e`: one
//!   that is there but can't be read stops the run, and a NUL in one is
//!   a warning;
//! - the 0.40.4 rename heads-up: one line on stderr while a workspace's
//!   `.k2so/` to `.k2/` rename is not committed (`git ls-files .k2so`).
//!   The project is found as the shell finds it (`PROJECT_PATH`, else the
//!   nearest folder up from the shell's `pwd` with `.k2so/` or `.k2/` in
//!   it, not `$HOME`, else git's main folder for a worktree); when the
//!   line would print, the page goes to the shell;
//! - the connection gate: no port or no token is an error for most verbs,
//!   help included (`k2 study` skips it). [`Prelude`] says whether there
//!   are both, as the shell reads them;
//! - the session checks: an `id` tool in a session with `HOOK_SOCK` set
//!   (the shared Codex server, a dead passport, no passport) and the
//!   permission card probe a passport makes (`HOOK_TOKEN`). [`Prelude`]
//!   says whether each is set; the caller decides.
//!
//! Everything else the prelude does before a page sets variables only.
//! The virtual desktop's guard never stops a page: a page is an offline
//! verb, or the token it needs is there.

use k2_client::env_names;
use k2_client::paths::{HOME_DIR_NAME, WORKSPACE_DOT_DIR_NAMES};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The shell's settings and sockets, as the prelude leaves them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Prelude {
    /// `$PORT` is not empty: `PORT`, else `heartbeat.port`.
    pub port: bool,
    /// `$TOKEN` is not empty: the passport, else the owner token on disk.
    pub token: bool,
    /// A passport is set (`HOOK_TOKEN`).
    pub passport: bool,
    /// The session's socket is set (`HOOK_SOCK`).
    pub hook_sock: bool,
}

/// Why the page goes to the shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Handoff {
    /// bash reads this at start.
    Startup(OsString),
    /// `HOME` or `PATH` is not set.
    Unset(&'static str),
    /// A program the prelude starts is not on `PATH`.
    Missing(&'static str),
    /// A file the prelude reads with `$(cat …)` can't be read.
    Unreadable(PathBuf),
    /// A file the prelude reads with `$(cat …)` holds a NUL.
    Nul(PathBuf),
    /// `PWD` is a path the shell would tidy (`.`, `..`, `//`).
    Pwd,
    /// The rename heads-up would print.
    RenameNotice,
}

/// The environment, as one list.
#[derive(Debug, Clone, Default)]
pub struct Env {
    vars: Vec<(OsString, OsString)>,
}

impl Env {
    /// This process's environment.
    pub fn current() -> Env {
        Env {
            vars: std::env::vars_os().collect(),
        }
    }

    /// An environment of these pairs (tests).
    pub fn of(pairs: &[(&str, &str)]) -> Env {
        Env {
            vars: pairs
                .iter()
                .map(|(k, v)| (OsString::from(k), OsString::from(v)))
                .collect(),
        }
    }

    pub fn get(&self, name: &str) -> Option<&OsStr> {
        self.vars
            .iter()
            .rev()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_os_str())
    }

    /// The value under the first of `names` that is set and not empty,
    /// as the shell's `_envn` reads it.
    pub fn first(&self, names: &[&str]) -> Option<&OsStr> {
        names
            .iter()
            .find_map(|n| self.get(n).filter(|v| !v.is_empty()))
    }

    /// Whether some variable's name starts with `prefix`.
    pub fn any_named(&self, prefix: &str) -> bool {
        self.vars
            .iter()
            .any(|(k, _)| k.to_str().is_some_and(|k| k.starts_with(prefix)))
    }
}

/// The shell's folder name, for the K2 home and a workspace alike.
const NEW_NAME: &str = HOME_DIR_NAME;
/// The shell's old folder name, its fallback for both. k2-client's list
/// of a workspace's names also has the K5 name; the shell doesn't look
/// for it yet, so neither does this.
const OLD_NAME: &str = WORKSPACE_DOT_DIR_NAMES[WORKSPACE_DOT_DIR_NAMES.len() - 1];

/// What bash reads at start that changes what runs.
pub const STARTUP: &[&str] = &[
    "BASH_ENV",
    "ENV",
    "SHELLOPTS",
    "BASHOPTS",
    "POSIXLY_CORRECT",
];

/// The prelude's settings in `env`, run from `cwd` (the shell's `pwd`,
/// [`shell_pwd`]), or why the page goes to the shell.
pub fn prelude(env: &Env, cwd: &Path) -> Result<Prelude, Handoff> {
    if !cfg!(unix) {
        // Git Bash: the prelude's Windows branch; the front stays out of it.
        return Err(Handoff::Startup("windows".into()));
    }
    for name in STARTUP {
        if env.get(name).is_some() {
            return Err(Handoff::Startup((*name).into()));
        }
    }
    if env.any_named("BASH_FUNC_") {
        return Err(Handoff::Startup("BASH_FUNC_".into()));
    }
    let home = env.get("HOME").ok_or(Handoff::Unset("HOME"))?;
    let path = env.get("PATH").ok_or(Handoff::Unset("PATH"))?;
    let path = SearchPath::new(path);
    path.need("cat")?;

    let k2_home = home_in(home);

    let passport = env.first(env_names::HOOK_TOKEN).is_some();
    let hook_sock = env.first(env_names::HOOK_SOCK).is_some();
    project(env, home, &path, cwd)?;

    let mut port = env.first(env_names::PORT).is_some();
    let port_file = k2_home.join("heartbeat.port");
    if !port && port_file.is_file() {
        port = !cat(&port_file)?.is_empty();
    }
    let no_owner = env
        .first(env_names::NO_OWNER_FALLBACK)
        .is_some_and(|v| v == "1");
    let mut token = passport;
    if !no_owner {
        let mut disk = false;
        for name in ["heartbeat.token", "daemon.token"] {
            let f = k2_home.join(name);
            if !disk && access(&f, Access::Read) {
                disk = !cat(&f)?.is_empty();
            }
        }
        token = token || disk;
    }
    Ok(Prelude {
        port,
        token,
        passport,
        hook_sock,
    })
}

/// The shell's K2 home: `K2_HOME="${HOME}/.k2"; [ -d "$K2_HOME" ] ||
/// K2_HOME="${HOME}/.k2so"`. The binary's home override doesn't count:
/// the shell never reads it.
fn home_in(home: &OsStr) -> PathBuf {
    let k2_home = cat_path(home, NEW_NAME);
    if k2_home.is_dir() {
        k2_home
    } else {
        cat_path(home, OLD_NAME)
    }
}

/// The shell's K2 home in `env` ([`home_in`]), when `HOME` is set.
pub fn shell_k2_home(env: &Env) -> Option<PathBuf> {
    env.get("HOME").map(home_in)
}

/// `"$a/b"`: the shell's join.
fn cat_path(a: &OsStr, b: &str) -> PathBuf {
    let mut s = a.to_os_string();
    s.push("/");
    s.push(b);
    PathBuf::from(s)
}

/// `$(cat file 2>/dev/null)` under `set -e`: the bytes with trailing
/// newlines dropped. A read that fails stops the shell; a NUL is a warning.
fn cat(file: &Path) -> Result<Vec<u8>, Handoff> {
    let mut bytes = std::fs::read(file).map_err(|_| Handoff::Unreadable(file.to_path_buf()))?;
    if bytes.contains(&0) {
        return Err(Handoff::Nul(file.to_path_buf()));
    }
    while bytes.last() == Some(&b'\n') {
        bytes.pop();
    }
    Ok(bytes)
}

#[derive(Clone, Copy)]
enum Access {
    Read,
    Exec,
}

#[cfg(unix)]
fn access(p: &Path, how: Access) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(p.as_os_str().as_bytes()) else {
        return false;
    };
    let mode = match how {
        Access::Read => libc::R_OK,
        Access::Exec => libc::X_OK,
    };
    // SAFETY: a valid NUL-terminated path and a plain mode.
    unsafe { libc::access(c.as_ptr(), mode) == 0 }
}

#[cfg(not(unix))]
fn access(p: &Path, _how: Access) -> bool {
    p.exists()
}

/// The shell's `PATH` search (`command -v`, and how it runs a program).
struct SearchPath<'a>(&'a OsStr);

impl<'a> SearchPath<'a> {
    fn new(p: &'a OsStr) -> Self {
        SearchPath(p)
    }

    /// The path bash runs for `name`: `<dir>/<name>` for the first entry
    /// (an empty one is `.`) where it is an executable file.
    fn find(&self, name: &str) -> Option<String> {
        let text = self.0.to_str()?;
        text.split(':').find_map(|dir| {
            let dir = if dir.is_empty() { "." } else { dir };
            let full = format!("{dir}/{name}");
            let p = Path::new(&full);
            (p.is_file() && access(p, Access::Exec)).then_some(full)
        })
    }

    fn need(&self, name: &'static str) -> Result<String, Handoff> {
        self.find(name).ok_or(Handoff::Missing(name))
    }
}

/// The shell's `pwd` at start: `PWD` when it is an absolute path to this
/// folder, else the folder's real path. A `PWD` bash would tidy first
/// (`.`, `..`, `//`, a trailing `/`) goes to the shell.
pub fn shell_pwd(env: &Env) -> Result<PathBuf, Handoff> {
    let real = || std::env::current_dir().map_err(|_| Handoff::Pwd);
    let Some(pwd) = env.get("PWD").and_then(|p| p.to_str()) else {
        return real();
    };
    if !pwd.starts_with('/') {
        return real();
    }
    if pwd != "/"
        && (pwd.ends_with('/')
            || pwd.contains("//")
            || pwd.split('/').any(|c| c == "." || c == ".."))
    {
        return Err(Handoff::Pwd);
    }
    if same_file(Path::new(pwd), Path::new(".")) {
        Ok(PathBuf::from(pwd))
    } else {
        real()
    }
}

#[cfg(unix)]
fn same_file(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(x), Ok(y)) => x.dev() == y.dev() && x.ino() == y.ino(),
        _ => false,
    }
}

#[cfg(not(unix))]
fn same_file(_a: &Path, _b: &Path) -> bool {
    false
}

fn has_dot_dir(dir: &OsStr) -> bool {
    cat_path(dir, OLD_NAME).is_dir() || cat_path(dir, NEW_NAME).is_dir()
}

/// The project as the prelude finds it, then the rename heads-up.
fn project(env: &Env, home: &OsStr, path: &SearchPath, cwd: &Path) -> Result<(), Handoff> {
    let mut git = Git::new(path);
    let project: OsString = match env.first(env_names::PROJECT_PATH) {
        Some(p) => p.to_os_string(),
        None => {
            let start = cwd.as_os_str().to_os_string();
            let mut dir = cwd.to_path_buf();
            let mut found = None;
            while dir.as_os_str() != "/" {
                if has_dot_dir(dir.as_os_str()) && dir.as_os_str() != home {
                    found = Some(dir.as_os_str().to_os_string());
                    break;
                }
                path.need("dirname")?;
                dir = match dir.parent() {
                    Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
                    _ => PathBuf::from("/"),
                };
            }
            if found.is_none() && git.usable() {
                path.need("sed")?;
                let root = git.main_root(&start);
                if !root.is_empty() && has_dot_dir(&root) {
                    found = Some(root);
                }
            }
            found.unwrap_or(start)
        }
    };
    if cat_path(&project, NEW_NAME).is_dir()
        && !cat_path(&project, OLD_NAME).is_dir()
        && git.usable()
    {
        path.need("head")?;
        if git.lists_old_folder(&project) {
            return Err(Handoff::RenameNotice);
        }
    }
    Ok(())
}

/// `_k2_git_usable` and the two git calls, as the prelude makes them.
struct Git<'a> {
    path: &'a SearchPath<'a>,
    usable: Option<Option<String>>,
}

impl<'a> Git<'a> {
    fn new(path: &'a SearchPath<'a>) -> Self {
        Git { path, usable: None }
    }

    /// The git to run, when the shell would run one: the one on `PATH`,
    /// except Apple's `/usr/bin/git` shim with no developer folder (it
    /// opens an install dialog). `uname` missing reads as not macOS.
    fn usable(&mut self) -> bool {
        if self.usable.is_none() {
            let git = self.path.find("git").filter(|g| {
                let darwin = cfg!(target_os = "macos") && self.path.find("uname").is_some();
                if !(darwin && g == "/usr/bin/git") {
                    return true;
                }
                let dev = Command::new("/usr/bin/xcode-select")
                    .arg("-p")
                    .stdin(Stdio::null())
                    .stderr(Stdio::null())
                    .output()
                    .map(|o| trim_newlines(o.stdout))
                    .unwrap_or_default();
                !dev.is_empty() && {
                    let mut p = dev;
                    p.extend_from_slice(b"/usr/bin/git");
                    access(&bytes_path(p), Access::Exec)
                }
            });
            self.usable = Some(git);
        }
        self.usable.as_ref().is_some_and(|g| g.is_some())
    }

    fn run(&self, args: &[&OsStr]) -> Vec<u8> {
        let Some(Some(git)) = &self.usable else {
            return Vec::new();
        };
        Command::new(git)
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .map(|o| o.stdout)
            .unwrap_or_default()
    }

    /// `git -C <dir> rev-parse --path-format=absolute --git-common-dir |
    /// sed 's|/.git$||'`, trailing newlines dropped.
    fn main_root(&self, dir: &OsStr) -> OsString {
        let out = self.run(&[
            OsStr::new("-C"),
            dir,
            OsStr::new("rev-parse"),
            OsStr::new("--path-format=absolute"),
            OsStr::new("--git-common-dir"),
        ]);
        let lines: Vec<Vec<u8>> = out
            .split(|b| *b == b'\n')
            .map(|l| strip_dot_git(l).to_vec())
            .collect();
        bytes_path(trim_newlines(lines.join(&b'\n'))).into_os_string()
    }

    /// `git -C <dir> ls-files .k2so | head -1` is not empty.
    fn lists_old_folder(&self, dir: &OsStr) -> bool {
        let out = self.run(&[
            OsStr::new("-C"),
            dir,
            OsStr::new("ls-files"),
            OsStr::new(OLD_NAME),
        ]);
        out.split(|b| *b == b'\n')
            .next()
            .is_some_and(|l| !l.is_empty())
    }
}

/// sed's `s|/.git$||` on one line: a `/`, any one character, `git`, at
/// the end.
fn strip_dot_git(line: &[u8]) -> &[u8] {
    let Some(rest) = line.strip_suffix(b"git") else {
        return line;
    };
    // The one character before `git`: a UTF-8 sequence, else one byte.
    let width = (1..=4.min(rest.len()))
        .find(|n| {
            std::str::from_utf8(&rest[rest.len() - n..]).is_ok_and(|s| s.chars().count() == 1)
        })
        .unwrap_or(1);
    if rest.len() > width && rest[rest.len() - width - 1] == b'/' {
        &line[..rest.len() - width - 1]
    } else {
        line
    }
}

fn trim_newlines(mut b: Vec<u8>) -> Vec<u8> {
    while b.last() == Some(&b'\n') {
        b.pop();
    }
    b
}

#[cfg(unix)]
fn bytes_path(b: Vec<u8>) -> PathBuf {
    use std::os::unix::ffi::OsStringExt;
    PathBuf::from(OsString::from_vec(b))
}

#[cfg(not(unix))]
fn bytes_path(b: Vec<u8>) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(&b).into_owned())
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn scratch(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "k5-offline-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).unwrap();
        }
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::canonicalize(dir).unwrap()
    }

    /// A folder with stand-ins for the programs the prelude starts.
    fn bin(dir: &Path, names: &[&str]) -> String {
        let b = dir.join("bin");
        std::fs::create_dir_all(&b).unwrap();
        for n in names {
            let p = b.join(n);
            std::fs::write(&p, "#!/bin/sh\nexit 0\n").unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        b.to_str().unwrap().to_string()
    }

    fn s(p: &Path) -> &str {
        p.to_str().unwrap()
    }

    /// The names are the shell's, as its prelude spells them.
    #[test]
    fn the_folder_names_are_the_shell_s() {
        assert_eq!((NEW_NAME, OLD_NAME), (".k2", ".k2so"));
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/prelude.sh");
        let prelude = std::fs::read_to_string(p).unwrap();
        let fallback = format!("[ -d \"$K2_HOME\" ] || K2_HOME=\"${{HOME}}/{OLD_NAME}\"");
        for line in [
            "K2_HOME=\"${HOME}/.k2\"",
            fallback.as_str(),
            "        if { [ -d \"$_dir/.k2so\" ] || [ -d \"$_dir/.k2\" ]; } && [ \"$_dir\" != \"$HOME\" ]; then",
            "if [ -d \"$PROJECT/.k2\" ] && [ ! -d \"$PROJECT/.k2so\" ] && _k2_git_usable; then",
            "    _k2so_pending=$(git -C \"$PROJECT\" ls-files .k2so 2>/dev/null | head -1 || true)",
            "            _main_root=$(git -C \"$(pwd)\" rev-parse --path-format=absolute --git-common-dir 2>/dev/null | sed 's|/.git$||' || true)",
            "    PORT=\"${PORT:-$(cat \"${K2_HOME}/heartbeat.port\" 2>/dev/null)}\"",
            "if [ -z \"$_k2_no_owner_fallback\" ] && [ -r \"${K2_HOME}/heartbeat.token\" ]; then",
            "if [ -z \"$_k2_no_owner_fallback\" ] && [ -z \"$DISK_OWNER_TOKEN\" ] && [ -r \"${K2_HOME}/daemon.token\" ]; then",
        ] {
            assert!(prelude.lines().any(|l| l == line), "prelude.sh changed: no line {line:?}");
        }
    }

    #[test]
    fn what_bash_reads_at_start_hands_off() {
        let d = scratch("start");
        let path = bin(&d, &["cat", "dirname"]);
        for (k, v) in [
            ("BASH_ENV", "/x"),
            ("ENV", ""),
            ("SHELLOPTS", "xtrace"),
            ("BASHOPTS", ""),
            ("POSIXLY_CORRECT", "1"),
            ("BASH_FUNC_cat%%", "() { :; }"),
        ] {
            let env = Env::of(&[("HOME", s(&d)), ("PATH", &path), (k, v)]);
            assert!(matches!(prelude(&env, &d), Err(Handoff::Startup(_))), "{k}");
        }
        assert_eq!(
            prelude(&Env::of(&[("PATH", &path)]), &d),
            Err(Handoff::Unset("HOME"))
        );
        assert_eq!(
            prelude(&Env::of(&[("HOME", s(&d))]), &d),
            Err(Handoff::Unset("PATH"))
        );
        let none = bin(&d.join("none"), &[]);
        assert_eq!(
            prelude(&Env::of(&[("HOME", s(&d)), ("PATH", &none)]), &d),
            Err(Handoff::Missing("cat"))
        );
    }

    #[test]
    fn the_port_and_token_as_the_shell_reads_them() {
        let d = scratch("files");
        let path = bin(&d, &["cat", "dirname"]);
        let hd = d.join("hd");
        std::fs::create_dir_all(hd.join(".k2so")).unwrap();
        let env = Env::of(&[("HOME", s(&hd)), ("PATH", &path)]);
        let p = prelude(&env, &d).unwrap();
        assert_eq!(
            (p.port, p.token, p.passport, p.hook_sock),
            (false, false, false, false)
        );
        // No .k2 folder: the old name is the hd.
        std::fs::write(hd.join(".k2so/heartbeat.port"), "\n\n").unwrap();
        std::fs::write(hd.join(".k2so/daemon.token"), "tok\n").unwrap();
        let p = prelude(&env, &d).unwrap();
        assert_eq!(
            (p.port, p.token),
            (false, true),
            "a port file of newlines is no port"
        );
        std::fs::write(hd.join(".k2so/heartbeat.port"), "4100\n").unwrap();
        assert!(prelude(&env, &d).unwrap().port);
        // With .k2 a folder, its files count, not the old name's.
        std::fs::create_dir_all(hd.join(".k2")).unwrap();
        let p = prelude(&env, &d).unwrap();
        assert_eq!((p.port, p.token), (false, false));
        let env2 = Env::of(&[
            ("HOME", s(&hd)),
            ("PATH", &path),
            ("K2_PORT", "1"),
            ("K5_HOOK_TOKEN", "s.x"),
            ("K2SO_HOOK_SOCK", "/s"),
        ]);
        let p = prelude(&env2, &d).unwrap();
        assert_eq!(
            (p.port, p.token, p.passport, p.hook_sock),
            (true, true, true, true)
        );
        // The virtual desktop reads no token file.
        std::fs::write(hd.join(".k2/heartbeat.token"), "t").unwrap();
        assert!(prelude(&env, &d).unwrap().token);
        let vd = Env::of(&[
            ("HOME", s(&hd)),
            ("PATH", &path),
            ("K2_NO_OWNER_FALLBACK", "1"),
        ]);
        assert!(!prelude(&vd, &d).unwrap().token);
        // A file the shell can't read stops it; a NUL is a warning.
        std::fs::remove_file(hd.join(".k2/heartbeat.token")).unwrap();
        std::fs::create_dir_all(hd.join(".k2/heartbeat.token")).unwrap();
        assert!(matches!(prelude(&env, &d), Err(Handoff::Unreadable(_))));
        std::fs::remove_dir(hd.join(".k2/heartbeat.token")).unwrap();
        std::fs::write(hd.join(".k2/heartbeat.port"), "41\x0000").unwrap();
        assert!(matches!(prelude(&env, &d), Err(Handoff::Nul(_))));
    }

    #[test]
    fn the_rename_notice_hands_off_only_when_git_lists_the_old_folder() {
        let d = scratch("rename");
        let ws = d.join("ws");
        std::fs::create_dir_all(ws.join(".k2")).unwrap();
        std::fs::create_dir_all(ws.join("a/b")).unwrap();
        let path = bin(&d, &["cat", "dirname", "sed", "head", "uname"]);
        // A git that lists a file for ls-files and fails anything else.
        let git = d.join("bin/git");
        std::fs::write(
            &git,
            "#!/bin/sh\ncase \"$3\" in ls-files) echo .k2so/x ;; *) exit 128 ;; esac\n",
        )
        .unwrap();
        std::fs::set_permissions(&git, std::fs::Permissions::from_mode(0o755)).unwrap();
        let env = Env::of(&[("HOME", s(&d)), ("PATH", &path)]);
        assert_eq!(prelude(&env, &ws.join("a/b")), Err(Handoff::RenameNotice));
        // Not a workspace (.k2so there too, or no .k2): no notice.
        std::fs::create_dir_all(ws.join(".k2so")).unwrap();
        assert!(prelude(&env, &ws.join("a/b")).is_ok());
        std::fs::remove_dir(ws.join(".k2so")).unwrap();
        // The user home is never the project.
        let home_is_ws = Env::of(&[("HOME", s(&ws)), ("PATH", &path)]);
        assert!(prelude(&home_is_ws, &ws.join("a/b")).is_ok());
        // PROJECT_PATH wins.
        let other = Env::of(&[("HOME", s(&d)), ("PATH", &path), ("K2_PROJECT_PATH", s(&d))]);
        assert!(prelude(&other, &ws.join("a/b")).is_ok());
        // A git that lists nothing: no notice.
        std::fs::write(&git, "#!/bin/sh\nexit 0\n").unwrap();
        assert!(prelude(&env, &ws.join("a/b")).is_ok());
        // No head on PATH while the check would run: the shell's error.
        let nohead = bin(&d.join("nohead"), &["cat", "dirname", "sed", "git"]);
        let env = Env::of(&[("HOME", s(&d)), ("PATH", &nohead)]);
        assert_eq!(
            prelude(&env, &ws.join("a/b")),
            Err(Handoff::Missing("head"))
        );
    }

    #[test]
    fn a_worktree_s_main_folder_by_git() {
        let d = scratch("wt");
        let main = d.join("main");
        std::fs::create_dir_all(main.join(".k2")).unwrap();
        std::fs::create_dir_all(d.join("wt/x")).unwrap();
        let path = bin(&d, &["cat", "dirname", "sed", "head", "uname"]);
        let git = d.join("bin/git");
        let script = format!(
            "#!/bin/sh\ncase \"$3\" in rev-parse) echo '{}/.git' ;; ls-files) [ \"$2\" = '{}' ] && echo .k2so/x ;; esac\nexit 0\n",
            s(&main),
            s(&main)
        );
        std::fs::write(&git, script).unwrap();
        std::fs::set_permissions(&git, std::fs::Permissions::from_mode(0o755)).unwrap();
        let env = Env::of(&[("HOME", s(&d)), ("PATH", &path)]);
        assert_eq!(prelude(&env, &d.join("wt/x")), Err(Handoff::RenameNotice));
    }

    #[test]
    fn sed_drops_a_slash_any_character_and_git() {
        assert_eq!(strip_dot_git(b"/r/.git"), b"/r");
        assert_eq!(strip_dot_git(b"/r/agit"), b"/r");
        assert_eq!(strip_dot_git("/r/\u{e9}git".as_bytes()), b"/r");
        assert_eq!(strip_dot_git(b"/r/foo.git"), b"/r/foo.git");
        assert_eq!(strip_dot_git(b"/.git"), b"");
        assert_eq!(strip_dot_git(b".git"), b".git");
        assert_eq!(strip_dot_git(b""), b"");
    }

    #[test]
    fn the_shell_s_pwd() {
        let here = std::env::current_dir().unwrap();
        let real = std::fs::canonicalize(&here).unwrap();
        assert_eq!(shell_pwd(&Env::of(&[])).unwrap(), here);
        let h = here.to_str().unwrap();
        assert_eq!(
            shell_pwd(&Env::of(&[("PWD", h)])).unwrap(),
            PathBuf::from(h)
        );
        assert_eq!(
            shell_pwd(&Env::of(&[("PWD", "/")])).unwrap(),
            here,
            "/ is another folder"
        );
        assert_eq!(shell_pwd(&Env::of(&[("PWD", "rel")])).unwrap(), here);
        for odd in [
            format!("{h}/"),
            format!("{h}/."),
            format!("{h}/../x"),
            "//x".to_string(),
        ] {
            assert_eq!(
                shell_pwd(&Env::of(&[("PWD", &odd)])),
                Err(Handoff::Pwd),
                "{odd}"
            );
        }
        assert!(real.is_absolute());
    }

    #[test]
    fn the_path_search_is_bash_s() {
        let d = scratch("path");
        let a = bin(&d.join("a"), &["git"]);
        let plain = d.join("b/bin");
        std::fs::create_dir_all(&plain).unwrap();
        std::fs::write(plain.join("git"), "x").unwrap();
        let joined = format!("{}:{a}", s(&plain));
        let p = SearchPath(OsStr::new(&joined));
        assert_eq!(
            p.find("git"),
            Some(format!("{a}/git")),
            "a file that is not executable is passed over"
        );
        assert_eq!(p.find("nope"), None);
        let slash = format!("{a}/");
        assert_eq!(
            SearchPath(OsStr::new(&slash)).find("git"),
            Some(format!("{a}//git"))
        );
    }
}
