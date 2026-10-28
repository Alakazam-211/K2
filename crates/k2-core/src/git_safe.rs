//! One builder for every `git` process K2 itself runs (prd-security-sweep-0464-v1
//! SS15).
//!
//! K2 runs plain `git` inside workspace folders: worktree list/add/prune,
//! rev-parse, init + the first commit, status and diff for `k2 compute run
//! --dirty`, the retire guard. A workspace folder is outside content (a
//! clone, a copied or unpacked folder), and its `.git/config` can name
//! programs git runs on its own: `core.fsmonitor` on every status/diff,
//! hooks (`post-checkout` on `worktree add`, `pre-commit`/`commit-msg` on
//! commit), `diff.external` and textconv drivers on diff. Every K2-run git
//! call goes through [`git`] (or [`command`] when there is no repo), which
//! turns those off for that one process:
//!
//! - `-c core.fsmonitor=false`, `-c core.hooksPath=/dev/null` (`NUL` on
//!   Windows): no fsmonitor daemon, no hooks;
//! - `-c protocol.ext.allow=never`, `-c protocol.file.allow=user`: no
//!   `ext::` transport, and `file://`/local-path remotes only when the
//!   person runs git themselves (submodules a repo names can't pull a
//!   local path);
//! - `GIT_TERMINAL_PROMPT=0`, `GIT_OPTIONAL_LOCKS=0`, stdin closed, and an
//!   inherited `GIT_EXTERNAL_DIFF` dropped;
//! - [`NO_EXT_DIFF`] (`--no-ext-diff --no-textconv`) goes on every
//!   `diff`/`show`/`log -p` K2 runs. These are subcommand options, so the
//!   caller adds them after the subcommand.
//!
//! What it keeps: the person's global and system config (identity,
//! credentials, LFS filters, signing). It never sets `safe.directory`, so
//! git's own refusal of a repo owned by another user still applies.
//!
//! What it doesn't stop: **smudge/clean filters** still run when
//! `worktree add` checks files out. A filter from the person's global
//! config (e.g. LFS) is theirs and fine; a repo-local `filter.*` in
//! `.git/config` is outside code, which the risky-folder scanner
//! (`cli_folder_trust::scan`, SS8) reports. K2's worktree create needs the
//! checkout, so it can't use `--no-checkout` today; once the folder-trust
//! gate (SS9) exists, an undecided folder gets `--no-checkout`.
//!
//! Hooks are off only for K2's own calls. Agents' own `git` in their
//! terminal is unchanged.
//!
//! **Which git** (0.46.4, Rosson 2026-10-26). Before building a command,
//! [`availability`] checks that a git really runs here. On macOS
//! `/usr/bin/git` is only Apple's shim: until the Command Line Tools are
//! installed every call fails AND opens Apple's install dialog. So K2
//! runs the shim only once `xcode-select -p` names a developer folder
//! that holds a real git; a Homebrew (or any other) git on PATH counts
//! too. While no git runs, [`command`] points at a path that doesn't
//! exist on macOS, so a K2 git call fails quietly instead of opening the
//! dialog. `crate::git_tools` owns what K2 does meanwhile (the deferred
//! git steps, the install button, the agents' note).
//!
//! The grep test below fails on a bare `Command::new("git")` anywhere in
//! `crates/k2-core/src` or `crates/k2-daemon/src` outside test code and
//! this file. A line that must stay raw carries the marker
//! `git_safe: allow (<reason>)` on the same line or the line above.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[cfg(windows)]
const NULL_HOOKS: &str = "core.hooksPath=NUL";
#[cfg(not(windows))]
const NULL_HOOKS: &str = "core.hooksPath=/dev/null";

/// The `-c` pairs every K2-run git gets, in order.
pub fn harden_args() -> [&'static str; 8] {
    [
        "-c",
        "core.fsmonitor=false",
        "-c",
        NULL_HOOKS,
        "-c",
        "protocol.ext.allow=never",
        "-c",
        "protocol.file.allow=user",
    ]
}

/// Put after `diff`, `show` or `log -p`: no external diff program and no
/// textconv driver from the repo's config or `.gitattributes`.
pub const NO_EXT_DIFF: [&str; 2] = ["--no-ext-diff", "--no-textconv"];

/// A hardened `git` with no working directory set (global config, or a
/// caller that passes `-C`/`--git-dir` itself).
pub fn command() -> Command {
    let mut c = Command::new(program());
    c.args(harden_args())
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env_remove("GIT_EXTERNAL_DIFF")
        .stdin(Stdio::null());
    c
}

/// A hardened `git` that runs in `dir`.
pub fn git(dir: impl AsRef<Path>) -> Command {
    let mut c = command();
    c.current_dir(dir.as_ref());
    c
}

/// The resolved git with none of the `-c` hardening: only for a throwaway
/// bare repo K2 just made itself (compute bundles), never a workspace.
pub fn plain() -> Command {
    let mut c = Command::new(program());
    c.env("GIT_TERMINAL_PROMPT", "0").stdin(Stdio::null());
    c
}

// ── Is git usable here? ──────────────────────────────────────────────────

/// What K2 found when it looked for a git it can run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitProbe {
    /// A git that runs. `program` is what every K2 git call uses.
    Ready { program: PathBuf, version: String },
    /// macOS with no usable git: Apple's Command Line Tools aren't
    /// installed, and no Homebrew or other git is on PATH.
    NeedsAppleClt,
    /// No git at all (Linux, Windows).
    Missing,
    /// A git is there but `--version` fails (for example Xcode installed
    /// and its licence not accepted: exit 69). `error` is what it said.
    Broken { program: PathBuf, error: String },
}

impl GitProbe {
    pub fn is_ready(&self) -> bool {
        matches!(self, GitProbe::Ready { .. })
    }
}

/// A program that runs nowhere: what [`command`] uses while git isn't
/// usable on macOS, so a K2 git call fails with "not found" instead of
/// running Apple's shim (which opens the install dialog).
pub const UNAVAILABLE: &str = "/nonexistent/k2-git-not-installed-yet/git";

/// The git program K2 runs: the probed one when git works; otherwise a
/// path that can't run on macOS, and plain `git` elsewhere (a missing
/// git there is only "not found"). Windows always runs plain `git`.
pub fn program() -> OsString {
    if cfg!(windows) {
        return OsString::from("git");
    }
    match availability() {
        GitProbe::Ready { program, .. } => program.into_os_string(),
        _ if cfg!(target_os = "macos") => OsString::from(UNAVAILABLE),
        _ => OsString::from("git"),
    }
}

/// Overrides the folders K2 searches for git (PATH syntax). For tests and
/// for checking a box by hand; unset in normal use.
pub const PROBE_PATH_ENV: &str = "K2_GIT_PROBE_PATH";

/// macOS folders searched after PATH: launchd hands the daemon a bare
/// PATH, so a Homebrew git would otherwise go unseen.
#[cfg(target_os = "macos")]
const MAC_EXTRA_DIRS: &[&str] = &["/opt/homebrew/bin", "/usr/local/bin"];

/// PATH when the daemon has none.
const DEFAULT_PATH: &str = "/usr/bin:/bin";

fn split_dirs(v: &std::ffi::OsStr) -> Vec<PathBuf> {
    std::env::split_paths(v)
        .filter(|p| !p.as_os_str().is_empty() && !is_git_wait_dir(p))
        .collect()
}

/// `…/.k2/bin/git-wait`: the agents' wrapper (`crate::git_tools`). Never
/// a git for the daemon, or the wrapper would be cached as "ready".
pub fn is_git_wait_dir(p: &Path) -> bool {
    p.ends_with(".k2/bin/git-wait")
}

/// The folders searched for git, in order.
pub fn probe_dirs() -> Vec<PathBuf> {
    if let Some(v) = std::env::var_os(PROBE_PATH_ENV) {
        return split_dirs(&v);
    }
    let path = std::env::var_os("PATH").filter(|v| !v.is_empty()).unwrap_or_else(|| DEFAULT_PATH.into());
    #[allow(unused_mut)]
    let mut dirs = split_dirs(&path);
    #[cfg(target_os = "macos")]
    dirs.extend(MAC_EXTRA_DIRS.iter().map(PathBuf::from));
    dirs
}

/// Apple's pair: the `git` shim and the `xcode-select` that says whether
/// the shim may run. Fixed paths on macOS; nothing elsewhere.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplePair {
    pub git: PathBuf,
    pub xcode_select: PathBuf,
}

impl ApplePair {
    /// Both programs in `dir` (tests).
    pub fn in_dir(dir: &Path) -> Self {
        Self { git: dir.join("git"), xcode_select: dir.join("xcode-select") }
    }
}

/// Test builds only: a folder whose `git` + `xcode-select` stand in for
/// Apple's `/usr/bin` pair (so a test never runs the real installer).
#[cfg(any(test, feature = "test-util"))]
pub const TEST_APPLE_DIR_ENV: &str = "K2_TEST_APPLE_SHIM_DIR";

/// `/usr/bin/git` + `/usr/bin/xcode-select` on macOS, `None` elsewhere.
pub fn apple_pair() -> Option<ApplePair> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    #[cfg(any(test, feature = "test-util"))]
    if let Some(d) = std::env::var_os(TEST_APPLE_DIR_ENV) {
        return Some(ApplePair::in_dir(Path::new(&d)));
    }
    Some(ApplePair { git: PathBuf::from("/usr/bin/git"), xcode_select: PathBuf::from("/usr/bin/xcode-select") })
}

pub(crate) fn is_exec_file(p: &Path) -> bool {
    let Ok(md) = std::fs::metadata(p) else { return false };
    if !md.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        md.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// `<git> --version`: the version line, or what went wrong.
fn version_of(git: &Path) -> Result<String, String> {
    let out = Command::new(git)
        .arg("--version")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("couldn't run {}: {e}", git.display()))?;
    let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if out.status.success() && v.starts_with("git version") {
        return Ok(v);
    }
    let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
    let said = if err.is_empty() { v } else { err };
    let code = out.status.code().map(|c| format!("exit {c}")).unwrap_or_else(|| "killed".into());
    Err(if said.is_empty() { code } else { format!("{said} ({code})") })
}

/// The developer folder `xcode-select -p` names, when it names one.
/// (`xcode-select -p` never opens the install dialog.)
pub fn apple_developer_dir(xcode_select: &Path) -> Option<PathBuf> {
    let out = Command::new(xcode_select).arg("-p").stdin(Stdio::null()).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let d = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!d.is_empty()).then(|| PathBuf::from(d))
}

/// Look for a git that runs in `dirs` (PATH order). `apple` (macOS) names
/// Apple's pair: that exact `git` is run only once its `xcode-select -p`
/// names a developer folder holding `usr/bin/git`; before that it is
/// skipped without ever being run. A git that is there but fails
/// `--version` is [`GitProbe::Broken`] unless a later one works.
pub fn probe_in(dirs: &[PathBuf], apple: Option<&ApplePair>) -> GitProbe {
    let exe = if cfg!(windows) { "git.exe" } else { "git" };
    let mut seen: Vec<&PathBuf> = Vec::new();
    let mut broken: Option<GitProbe> = None;
    for d in dirs {
        if seen.contains(&d) {
            continue;
        }
        seen.push(d);
        let git = d.join(exe);
        if !is_exec_file(&git) {
            continue;
        }
        if let Some(pair) = apple.filter(|p| p.git == git) {
            let tools_git = apple_developer_dir(&pair.xcode_select).map(|dev| dev.join("usr/bin/git"));
            if !tools_git.is_some_and(|g| is_exec_file(&g)) {
                continue;
            }
        }
        match version_of(&git) {
            Ok(version) => return GitProbe::Ready { program: git, version },
            Err(error) => {
                broken.get_or_insert(GitProbe::Broken { program: git, error });
            }
        }
    }
    match (broken, apple) {
        (Some(b), _) => b,
        (None, Some(_)) => GitProbe::NeedsAppleClt,
        (None, None) => GitProbe::Missing,
    }
}

/// Probe now with this machine's folders (no cache).
pub fn probe_now() -> GitProbe {
    probe_in(&probe_dirs(), apple_pair().as_ref())
}

/// The cached answer, without probing.
pub fn cached() -> Option<GitProbe> {
    CACHE.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|(p, _)| p.clone())
}

/// While git isn't usable, look again at most this often.
const RECHECK_EVERY: Duration = Duration::from_secs(10);

static CACHE: Mutex<Option<(GitProbe, Instant)>> = Mutex::new(None);

/// Is git usable here? Cached: a working git stays cached while its
/// program is still there; a missing one is probed again after
/// [`RECHECK_EVERY`] (or at once by [`refresh`]).
pub fn availability() -> GitProbe {
    if let Some((p, at)) = CACHE.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        if cache_still_good(p, at.elapsed()) {
            return p.clone();
        }
    }
    refresh()
}

/// Pure: may a cached answer stand? A ready git only while its program is
/// still an executable file (0.46.5, first-run review item 10: a cached
/// Homebrew git that was uninstalled used to fail every K2 git call with
/// "not found" until the daemon restarted; now the next call probes again
/// and finds `/usr/bin/git` or whatever else runs). Anything else for
/// [`RECHECK_EVERY`].
pub fn cache_still_good(p: &GitProbe, age: Duration) -> bool {
    match p {
        GitProbe::Ready { program, .. } => is_exec_file(program),
        _ => age < RECHECK_EVERY,
    }
}

/// Probe again now and cache the answer.
pub fn refresh() -> GitProbe {
    let p = probe_now();
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Some((p.clone(), Instant::now()));
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[cfg(unix)]
    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("k2-git-safe-{tag}-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).expect("mkdir temp");
        d
    }

    /// Plain git for building fixtures (not K2's own path): isolated from
    /// the machine's config so the fixture is the only config in play.
    #[cfg(unix)]
    fn fixture_git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git") // git_safe: allow (test fixture)
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", "-c", "init.defaultBranch=main"])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("run git");
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[cfg(unix)]
    fn write_exec(path: &Path, body: &str) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(path, body).expect("write script");
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod script");
    }

    /// T13: a repo whose own config names an fsmonitor program, a
    /// post-checkout hook, a textconv driver and an external diff runs none
    /// of them through K2's `rev-parse`, `status`, `diff` and `worktree add`.
    #[cfg(unix)]
    #[test]
    fn repo_config_programs_do_not_run_through_git_safe() {
        let d = temp("t13");
        let repo = d.join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir repo");
        fixture_git(&repo, &["init", "-q"]);
        std::fs::write(repo.join("a.bin"), "one\n").expect("write a.bin");
        std::fs::write(repo.join(".gitattributes"), "*.bin diff=evil\n").expect("write attrs");
        fixture_git(&repo, &["add", "."]);
        fixture_git(&repo, &["commit", "-q", "-m", "base"]);
        std::fs::write(repo.join("a.bin"), "two\n").expect("change a.bin");

        let marks = d.join("marks");
        std::fs::create_dir_all(&marks).expect("mkdir marks");
        let prog = |name: &str| {
            let p = d.join(format!("{name}.sh"));
            write_exec(&p, &format!("#!/bin/sh\ntouch '{}'\n", marks.join(name).display()));
            p
        };
        let fsmon = prog("fsmonitor");
        let textconv = prog("textconv");
        let extdiff = prog("extdiff");
        let hooks = repo.join(".git/hooks");
        write_exec(&hooks.join("post-checkout"), &format!("#!/bin/sh\ntouch '{}'\n", marks.join("post-checkout").display()));
        fixture_git(&repo, &["config", "core.fsmonitor", fsmon.to_str().expect("utf8")]);
        fixture_git(&repo, &["config", "diff.evil.textconv", textconv.to_str().expect("utf8")]);
        fixture_git(&repo, &["config", "diff.external", extdiff.to_str().expect("utf8")]);

        // The fixture is armed: plain git runs the hook on worktree add and
        // the external diff on diff. (If this half fails, the test proves
        // nothing, so it fails loudly instead.)
        fixture_git(&repo, &["worktree", "add", "-q", "-b", "plain", d.join("wt-plain").to_str().expect("utf8")]);
        assert!(marks.join("post-checkout").exists(), "fixture: plain git must run post-checkout");
        let plain_diff = Command::new("git") // git_safe: allow (test fixture)
            .arg("-C")
            .arg(&repo)
            .args(["diff"])
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("plain diff");
        assert!(plain_diff.status.success(), "fixture: plain diff: {}", String::from_utf8_lossy(&plain_diff.stderr));
        assert!(marks.join("extdiff").exists(), "fixture: plain git must run diff.external");
        for m in ["post-checkout", "extdiff", "fsmonitor", "textconv"] {
            let _ = std::fs::remove_file(marks.join(m));
        }

        // K2's path.
        let run = |args: &[&str]| {
            let out = git(&repo)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .args(args)
                .output()
                .expect("run git_safe");
            assert!(out.status.success(), "git_safe {args:?}: {}", String::from_utf8_lossy(&out.stderr));
            String::from_utf8_lossy(&out.stdout).to_string()
        };
        run(&["rev-parse", "--show-toplevel"]);
        let st = run(&["status", "--porcelain"]);
        assert!(st.contains("a.bin"), "status sees the change: {st:?}");
        let mut diff_args = vec!["diff"];
        diff_args.extend(NO_EXT_DIFF);
        let diff = run(&diff_args);
        assert!(diff.contains("-one") && diff.contains("+two"), "a real diff, not a textconv one: {diff:?}");
        run(&["worktree", "add", "-q", "-b", "k2", d.join("wt-k2").to_str().expect("utf8")]);
        assert!(d.join("wt-k2/a.bin").exists(), "worktree add still checks out");

        let ran: Vec<String> = std::fs::read_dir(&marks)
            .expect("read marks")
            .map(|e| e.expect("entry").file_name().to_string_lossy().to_string())
            .collect();
        assert!(ran.is_empty(), "repo-named programs ran through git_safe: {ran:?}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn command_carries_the_hardening_flags_first() {
        let c = git("/tmp");
        let args: Vec<String> = c.get_args().map(|a| a.to_string_lossy().to_string()).collect();
        assert_eq!(&args[..8], &harden_args().map(String::from)[..]);
        assert_eq!(c.get_current_dir(), Some(Path::new("/tmp")));
        let envs: Vec<(String, Option<String>)> = c
            .get_envs()
            .map(|(k, v)| (k.to_string_lossy().to_string(), v.map(|v| v.to_string_lossy().to_string())))
            .collect();
        assert!(envs.contains(&("GIT_TERMINAL_PROMPT".into(), Some("0".into()))), "{envs:?}");
        assert!(envs.contains(&("GIT_OPTIONAL_LOCKS".into(), Some("0".into()))), "{envs:?}");
        assert!(envs.contains(&("GIT_EXTERNAL_DIFF".into(), None)), "{envs:?}");
        assert!(!args.iter().any(|a| a.starts_with("safe.directory")), "never safe.directory: {args:?}");
    }

    // ── probe (0.46.4: first run on a Mac without developer tools) ───────

    /// A stub `git` in `dir` that touches `mark` whenever it runs, so a
    /// test can prove Apple's shim was never run. `works` = `--version`
    /// answers like a real git; otherwise it fails like the shim does.
    #[cfg(unix)]
    fn stub_git(dir: &Path, mark: &Path, works: bool) {
        std::fs::create_dir_all(dir).expect("mkdir stub dir");
        let tail = if works {
            "echo 'git version 9.9.9-stub'"
        } else {
            "echo 'xcode-select: note: No developer tools were found, requesting install.' >&2; exit 1"
        };
        // Written by a /bin/sh child: this process execs it (ETXTBSY).
        crate::test_env::write_executable(&dir.join("git"), &format!("#!/bin/sh\n: > '{}'\n{tail}\n", mark.display()));
    }

    /// A stub `xcode-select` whose `-p` names `dev`, or fails the way it
    /// does before the Command Line Tools are installed.
    #[cfg(unix)]
    fn stub_xcode_select(dir: &Path, dev: Option<&Path>) {
        std::fs::create_dir_all(dir).expect("mkdir stub dir");
        let body = match dev {
            Some(d) => format!("#!/bin/sh\n[ \"$1\" = -p ] || exit 9\necho '{}'\n", d.display()),
            None => "#!/bin/sh\necho 'xcode-select: error: unable to get active developer directory' >&2\nexit 2\n".to_string(),
        };
        crate::test_env::write_executable(&dir.join("xcode-select"), &body);
    }

    #[cfg(unix)]
    #[test]
    fn apple_shim_without_tools_is_never_run() {
        let d = temp("probe-noclt");
        let usr_bin = d.join("usr-bin");
        let mark = d.join("shim-ran");
        stub_git(&usr_bin, &mark, true);
        stub_xcode_select(&usr_bin, None);
        let apple = ApplePair::in_dir(&usr_bin);
        assert_eq!(probe_in(&[usr_bin.clone()], Some(&apple)), GitProbe::NeedsAppleClt);
        assert!(!mark.exists(), "the probe ran Apple's shim (that opens the install dialog)");
        // A developer folder without a git in it doesn't count either.
        let dev = d.join("dev");
        std::fs::create_dir_all(&dev).expect("mkdir dev");
        stub_xcode_select(&usr_bin, Some(&dev));
        assert_eq!(probe_in(&[usr_bin.clone()], Some(&apple)), GitProbe::NeedsAppleClt);
        assert!(!mark.exists(), "the probe ran Apple's shim with no git in the developer folder");
        // Only Apple's exact git path is the shim: a git beside some other
        // xcode-select is an ordinary git.
        let elsewhere = d.join("elsewhere");
        stub_git(&elsewhere, &d.join("elsewhere-ran"), true);
        stub_xcode_select(&elsewhere, None);
        assert!(probe_in(&[usr_bin, elsewhere], Some(&apple)).is_ready());
        assert!(!mark.exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn apple_shim_runs_once_the_tools_are_installed() {
        let d = temp("probe-clt");
        let usr_bin = d.join("usr-bin");
        let dev = d.join("CommandLineTools");
        stub_git(&usr_bin, &d.join("shim-ran"), true);
        stub_git(&dev.join("usr/bin"), &d.join("tools-git-ran"), true);
        stub_xcode_select(&usr_bin, Some(&dev));
        assert_eq!(
            probe_in(&[usr_bin.clone()], Some(&ApplePair::in_dir(&usr_bin))),
            GitProbe::Ready { program: usr_bin.join("git"), version: "git version 9.9.9-stub".into() }
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Xcode installed, licence not accepted: the shim runs and fails with
    /// exit 69. That is its own state with git's words, not "install".
    #[cfg(unix)]
    #[test]
    fn apple_shim_that_fails_is_broken_not_missing() {
        let d = temp("probe-licence");
        let usr_bin = d.join("usr-bin");
        let dev = d.join("Xcode.app/Contents/Developer");
        stub_git(&dev.join("usr/bin"), &d.join("tools-git-ran"), true);
        stub_xcode_select(&usr_bin, Some(&dev));
        crate::test_env::write_executable(
            &usr_bin.join("git"),
            "#!/bin/sh\necho 'You have not agreed to the Xcode license agreements. Please run '\\''sudo xcodebuild -license'\\'' from within a Terminal window to review and agree to the Xcode and Apple SDKs license.' >&2\nexit 69\n",
        );
        match probe_in(&[usr_bin.clone()], Some(&ApplePair::in_dir(&usr_bin))) {
            GitProbe::Broken { program, error } => {
                assert_eq!(program, usr_bin.join("git"));
                assert!(error.contains("sudo xcodebuild -license") && error.ends_with("(exit 69)"), "{error}");
            }
            other => panic!("want Broken, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_agents_wrapper_folder_is_never_searched() {
        let sep = if cfg!(windows) { ";" } else { ":" };
        let path = ["/home/me/.k2/bin/git-wait", "/usr/bin", "/home/me/.k2/bin"].join(sep);
        let dirs = split_dirs(std::ffi::OsStr::new(&path));
        assert_eq!(dirs, vec![PathBuf::from("/usr/bin"), PathBuf::from("/home/me/.k2/bin")]);
        assert!(is_git_wait_dir(Path::new("/Users/x/.k2/bin/git-wait")));
        assert!(!is_git_wait_dir(Path::new("/Users/x/.k2/bin")));
    }

    /// 0.46.5 (first-run review item 10): a cached ready git stands only
    /// while its program is there. Uninstall it (Homebrew's, say) and the
    /// cache gives way, so the next call probes again instead of failing
    /// "not found" until the daemon restarts; then the probe finds the git
    /// left on PATH. A not-ready answer still waits [`RECHECK_EVERY`].
    #[cfg(unix)]
    #[test]
    fn a_cached_git_that_was_uninstalled_is_probed_again() {
        let d = temp("probe-gone");
        let brew = d.join("brew-bin");
        let usr = d.join("usr-bin");
        stub_git(&brew, &d.join("brew-ran"), true);
        stub_git(&usr, &d.join("usr-ran"), true);
        let first = probe_in(&[brew.clone(), usr.clone()], None);
        assert_eq!(first, GitProbe::Ready { program: brew.join("git"), version: "git version 9.9.9-stub".into() });
        let long_ago = Duration::from_secs(24 * 60 * 60);
        assert!(cache_still_good(&first, long_ago), "a ready git that is still there stays cached");
        std::fs::remove_file(brew.join("git")).expect("uninstall the cached git");
        assert!(!cache_still_good(&first, Duration::ZERO), "the cached git is gone: probe again at once");
        assert_eq!(
            probe_in(&[brew, usr.clone()], None),
            GitProbe::Ready { program: usr.join("git"), version: "git version 9.9.9-stub".into() },
            "the probe falls back to the git that is left"
        );
        assert!(cache_still_good(&GitProbe::Missing, Duration::from_secs(1)));
        assert!(!cache_still_good(&GitProbe::Missing, RECHECK_EVERY));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn homebrew_git_is_used_while_the_tools_are_missing() {
        let d = temp("probe-brew");
        let usr_bin = d.join("usr-bin");
        let brew = d.join("brew-bin");
        let shim_mark = d.join("shim-ran");
        stub_git(&usr_bin, &shim_mark, true);
        stub_xcode_select(&usr_bin, None);
        stub_git(&brew, &d.join("brew-ran"), true);
        let apple = ApplePair::in_dir(&usr_bin);
        let p = probe_in(&[usr_bin, brew.clone()], Some(&apple));
        assert_eq!(p, GitProbe::Ready { program: brew.join("git"), version: "git version 9.9.9-stub".into() });
        assert!(!shim_mark.exists(), "Apple's shim ran although Homebrew's git was there");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn missing_or_broken_git_is_not_ready() {
        let d = temp("probe-linux");
        let empty = d.join("empty");
        std::fs::create_dir_all(&empty).expect("mkdir empty");
        assert_eq!(probe_in(&[empty.clone()], None), GitProbe::Missing);
        assert_eq!(probe_in(&[], None), GitProbe::Missing);
        // Not a Mac: an `xcode-select` beside git means nothing.
        let bin = d.join("bin");
        stub_git(&bin, &d.join("ran"), true);
        stub_xcode_select(&bin, None);
        assert!(probe_in(&[empty.clone(), bin.clone()], None).is_ready());
        // A git that fails `--version` is Broken, with what it said...
        let broken = d.join("broken");
        stub_git(&broken, &d.join("broken-ran"), false);
        match probe_in(&[broken.clone()], None) {
            GitProbe::Broken { program, error } => {
                assert_eq!(program, broken.join("git"));
                assert!(error.contains("No developer tools were found") && error.ends_with("(exit 1)"), "{error}");
            }
            other => panic!("want Broken, got {other:?}"),
        }
        // ...unless a later git works.
        assert!(probe_in(&[broken, bin], None).is_ready());
        let _ = std::fs::remove_dir_all(&d);
    }

    // ── grep test ────────────────────────────────────────────────────────

    const RAW: &str = concat!("Command::new(", "\"git\")");
    const MARK: &str = "git_safe: allow (";

    fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display())) {
            let p = e.expect("dir entry").path();
            if p.is_dir() {
                if p.file_name().is_some_and(|n| n == "tests") {
                    continue;
                }
                rs_files(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }

    fn is_test_file(p: &Path) -> bool {
        let name = p.file_name().expect("file name").to_string_lossy();
        name == "tests.rs" || name.ends_with("_tests.rs")
    }

    fn ident_byte(c: u8) -> bool {
        c.is_ascii_alphanumeric() || c == b'_'
    }

    /// `r"…"`, `r#"…"#`, `br"…"` start at `i` (and not `r#ident`).
    fn raw_string_at(b: &[u8], i: usize) -> bool {
        if i > 0 && ident_byte(b[i - 1]) {
            let byte_prefix = b[i - 1] == b'b' && (i < 2 || !ident_byte(b[i - 2]));
            if !byte_prefix {
                return false;
            }
        }
        let mut j = i + 1;
        while b.get(j) == Some(&b'#') {
            j += 1;
        }
        b.get(j) == Some(&b'"')
    }

    fn utf8_len(first: Option<&u8>) -> usize {
        match first {
            Some(&c) if c >= 0xF0 => 4,
            Some(&c) if c >= 0xE0 => 3,
            Some(&c) if c >= 0x80 => 2,
            _ => 1,
        }
    }

    /// Byte offset just past the `}` closing the first block that opens at
    /// or after `start`. Comments, strings, raw strings and char literals
    /// are skipped, so a `"{"` in a test doesn't count. `None` = it never
    /// closes.
    fn block_end(src: &str, start: usize) -> Option<usize> {
        let b = src.as_bytes();
        let (mut i, mut depth, mut opened) = (start, 0i64, false);
        while i < b.len() {
            match b[i] {
                b'/' if b.get(i + 1) == Some(&b'/') => {
                    while i < b.len() && b[i] != b'\n' {
                        i += 1;
                    }
                    continue;
                }
                b'/' if b.get(i + 1) == Some(&b'*') => {
                    let mut nest = 1;
                    i += 2;
                    while i < b.len() && nest > 0 {
                        if b[i] == b'/' && b.get(i + 1) == Some(&b'*') {
                            nest += 1;
                            i += 2;
                        } else if b[i] == b'*' && b.get(i + 1) == Some(&b'/') {
                            nest -= 1;
                            i += 2;
                        } else {
                            i += 1;
                        }
                    }
                    continue;
                }
                b'r' if raw_string_at(b, i) => {
                    let mut j = i + 1;
                    let mut hashes = 0;
                    while b[j] == b'#' {
                        hashes += 1;
                        j += 1;
                    }
                    j += 1;
                    loop {
                        if j >= b.len() {
                            return None;
                        }
                        if b[j] == b'"' && b.len() >= j + 1 + hashes && b[j + 1..j + 1 + hashes].iter().all(|&c| c == b'#') {
                            j += 1 + hashes;
                            break;
                        }
                        j += 1;
                    }
                    i = j;
                    continue;
                }
                b'"' => {
                    i += 1;
                    while i < b.len() && b[i] != b'"' {
                        if b[i] == b'\\' {
                            i += 1;
                        }
                        i += 1;
                    }
                    i += 1;
                    continue;
                }
                b'\'' => {
                    if b.get(i + 1) == Some(&b'\\') {
                        i += 2;
                        while i < b.len() && b[i] != b'\'' {
                            i += 1;
                        }
                        i += 1;
                        continue;
                    }
                    let n = utf8_len(b.get(i + 1));
                    if b.get(i + 1 + n) == Some(&b'\'') {
                        i += 2 + n;
                        continue;
                    }
                    // A lifetime.
                }
                b'{' => {
                    depth += 1;
                    opened = true;
                }
                b'}' => {
                    depth -= 1;
                    if opened && depth == 0 {
                        return Some(i + 1);
                    }
                }
                _ => {}
            }
            i += 1;
        }
        None
    }

    /// Lines of `src` outside `#[cfg(test)] mod … { … }` blocks, with their
    /// 1-based numbers. A block that never closes panics, so a lexing miss
    /// can't silently hide production code.
    fn non_test_lines(file: &Path, src: &str) -> Vec<(usize, String)> {
        let mut starts = Vec::new();
        let mut off = 0;
        for l in src.split('\n') {
            starts.push(off);
            off += l.len() + 1;
        }
        let lines: Vec<&str> = src.split('\n').collect();
        let mut out = Vec::new();
        let mut i = 0;
        while i < lines.len() {
            if lines[i].trim().starts_with("#[cfg(test)]") {
                let mut j = i + 1;
                while j < lines.len() && (lines[j].trim().is_empty() || lines[j].trim().starts_with("#[")) {
                    j += 1;
                }
                let item = lines.get(j).map(|l| l.trim()).unwrap_or("");
                let inline_mod = (item.starts_with("mod ") || item.starts_with("pub mod ") || item.starts_with("pub(crate) mod ")) && item.contains('{');
                if inline_mod {
                    let end = block_end(src, starts[j]).unwrap_or_else(|| panic!("{}:{}: test module never closes", file.display(), j + 1));
                    let mut k = j;
                    while k < lines.len() && starts[k] < end {
                        k += 1;
                    }
                    i = k;
                    continue;
                }
            }
            out.push((i + 1, lines[i].to_string()));
            i += 1;
        }
        out
    }

    #[test]
    fn no_raw_git_command_outside_git_safe() {
        let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut files = Vec::new();
        for root in ["k2-core/src", "k2-daemon/src"] {
            rs_files(&crates.join(root), &mut files);
        }
        assert!(files.len() > 100, "the scan found the sources ({} files)", files.len());
        let mut bad = Vec::new();
        let mut allowed = Vec::new();
        for f in &files {
            if is_test_file(f) || f.ends_with("k2-core/src/git_safe.rs") {
                continue;
            }
            let src = std::fs::read_to_string(f).unwrap_or_else(|e| panic!("read {}: {e}", f.display()));
            let kept = non_test_lines(f, &src);
            for (idx, (n, line)) in kept.iter().enumerate() {
                if !line.contains(RAW) {
                    continue;
                }
                let prev = if idx > 0 { kept[idx - 1].1.as_str() } else { "" };
                if line.contains(MARK) || prev.contains(MARK) {
                    allowed.push(format!("{}:{n}", f.display()));
                } else {
                    bad.push(format!("{}:{n}: {}", f.display(), line.trim()));
                }
            }
        }
        assert!(
            bad.is_empty(),
            "raw git commands outside crate::git_safe (use git_safe::git(dir) / git_safe::command(), or mark the line `git_safe: allow (<reason>)`):\n{}",
            bad.join("\n")
        );
        // The allow-list is empty since 0.46.4: the two temp-bare-repo calls
        // in compute/sync.rs moved to `git_safe::plain()`, so they too run
        // the probed git and never Apple's shim.
        assert!(allowed.is_empty(), "allow-listed raw git lines came back: {allowed:?}");
    }

    #[test]
    fn non_test_lines_skips_inline_test_modules_only() {
        let src = concat!(
            "fn a() {}\n",
            "#[cfg(test)]\n",
            "mod tests {\n",
            "    fn b<'a>(x: &'a str) { let _ = (\"{\", '{', r#\"}}\"#, x); } // }\n",
            "    /* { */\n",
            "}\n",
            "fn c() {}\n",
            "#[cfg(test)]\n",
            "mod ext_tests;\n",
        );
        let kept: Vec<String> = non_test_lines(Path::new("x.rs"), src).into_iter().map(|(_, l)| l).collect();
        assert_eq!(kept, vec!["fn a() {}", "fn c() {}", "#[cfg(test)]", "mod ext_tests;", ""]);
        let never = "#[cfg(test)]\nmod tests {\n fn x() {\n";
        assert!(std::panic::catch_unwind(|| non_test_lines(Path::new("y.rs"), never)).is_err(), "an unclosed module panics");
    }
}
