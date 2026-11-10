//! Put the `k2` command on the person's PATH with no admin prompt (0.46.11).
//!
//! Before 0.46.11 the macOS app only symlinked `/usr/local/bin/k2`. On a
//! fresh Apple Silicon Mac that folder doesn't exist and needs admin, so
//! the boot heal deferred to Settings → Install CLI and the person's own
//! Terminal had no `k2` (K2's agent terminals were fine: the PTY layer
//! prepends the bundled `cli/`).
//!
//! What this module does, the way rustup and bun do it:
//!
//! 1. **Links** `~/.local/bin/k2` (and the `k2so` alias) at the bundled CLI,
//!    re-pointing a link left at an old or moved bundle. `/usr/local/bin/k2`
//!    is linked as well when that folder is already writable (as before).
//! 2. **PATH:** when the person's login-shell PATH doesn't have
//!    `~/.local/bin` (and doesn't already run a K2 `k2`), appends ONE marked
//!    block to the shell's startup file (`~/.zprofile`, the bash login file,
//!    or a fish `conf.d` file). It takes effect in new Terminal windows.
//! 3. **Undo:** [`remove_path_block_at`] removes only what is between the
//!    markers and records that the person turned it off, so the boot heal
//!    doesn't put it back. An explicit install (Settings) adds it again.
//!
//! Never overwrites a `k2` that isn't K2's: a real file, a folder, or a link
//! to anything but a `cli/k2` (a K2 bundle's `…/_up_/cli/k2` or a checkout's
//! `cli/k2`) is left alone and reported as [`LinkState::Foreign`].
//!
//! Daemon-first: the daemon runs [`boot_heal`] at boot (so a headless or
//! launchd-started K2 heals too); the desktop app runs the same call as a
//! backstop and uses the status / install / undo helpers for Settings.
//! On Linux the daemon stages a copy (`k2_core::cli_stage`) and calls
//! [`ensure_path_block`] for `~/.local/bin` only.
//!
//! Everything takes an explicit home (`*_at`) so tests drive it under a
//! temp `$HOME`; the wrappers read `$HOME` through [`crate::paths::home_dir`].

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;

/// First line of the PATH block K2 writes into a shell startup file.
pub const BLOCK_BEGIN: &str = "# >>> K2 CLI >>>";
/// Last line of the PATH block.
pub const BLOCK_END: &str = "# <<< K2 CLI <<<";

/// The command's name, and its pre-0.40 alias (a shim that runs `k2`).
pub const CLI_NAME: &str = "k2";
pub const LEGACY_NAME: &str = "k2so";

/// The system-wide bin folder linked when it is already writable (macOS).
pub const SYSTEM_BIN: &str = "/usr/local/bin";

/// Under `~/.k2`: present once the person removed the PATH line, so the
/// boot heal leaves their startup file alone from then on.
pub const OPT_OUT_FILE: &str = "cli-path-line-removed";

/// The fish startup file K2 owns (the whole file is K2's block).
const FISH_CONF: &str = "k2-cli.fish";

/// `~/.local/bin`, where K2 puts `k2` without admin.
pub fn user_bin_dir(home: &Path) -> PathBuf {
    home.join(".local").join("bin")
}

// ── Which files are K2's ────────────────────────────────────────────────

/// True when a link target is a K2 CLI script by its place: a file named
/// `k2` / `k2so` in a folder named `cli` (a bundle's `…/_up_/cli/k2`, or a
/// checkout's `cli/k2`). Shape only, so a link to a deleted old bundle
/// still counts as K2's and gets re-pointed.
pub fn is_k2_cli_target(target: &Path) -> bool {
    let name = target.file_name().and_then(|n| n.to_str());
    let dir = target.parent().and_then(|p| p.file_name()).and_then(|n| n.to_str());
    matches!(name, Some(CLI_NAME) | Some(LEGACY_NAME)) && dir == Some("cli")
}

/// True when the file at `path` (links followed) is the K2 CLI script: a
/// `#!` first line and a `K2_CLI_VERSION=` (or pre-rename
/// `K2SO_CLI_VERSION=`) line near the top. A program that merely shares the
/// name (the Linux .deb puts the GUI binary at `/usr/bin/k2`) is not.
pub fn is_k2_cli_script(path: &Path) -> bool {
    use std::io::Read;
    let Ok(file) = fs::File::open(path) else { return false };
    let mut head = Vec::with_capacity(8192);
    if file.take(8192).read_to_end(&mut head).is_err() {
        return false;
    }
    let text = String::from_utf8_lossy(&head);
    let mut lines = text.lines();
    if !lines.next().is_some_and(|l| l.starts_with("#!")) {
        return false;
    }
    lines.take(40).any(|l| l.starts_with("K2_CLI_VERSION=") || l.starts_with("K2SO_CLI_VERSION="))
}

/// The CLI script a packaged macOS K2 ships, found from a K2 executable
/// (the app's `Contents/MacOS/k2`, or the daemon's nested helper
/// `Contents/Helpers/K2 Daemon.app/Contents/MacOS/k2-daemon`):
/// `<X>.app/Contents/Resources/_up_/cli/<name>`. `None` for a dev build
/// (`target/…`): the automatic heal never links a checkout's `cli/`, or a
/// worktree's daemon would take `~/.local/bin/k2` from the installed app.
pub fn packaged_cli_for_exe(exe: &Path, name: &str) -> Option<PathBuf> {
    let macos_dir = crate::daemon_lifecycle::bundle_sidecar_dir(exe)?;
    let contents = macos_dir.parent()?;
    if contents.file_name()? != "Contents" || contents.parent()?.extension()? != "app" {
        return None;
    }
    let cli = contents.join("Resources").join("_up_").join("cli").join(name);
    cli.is_file().then_some(cli)
}

// ── Links ───────────────────────────────────────────────────────────────

/// What sits at a `k2` link path, judged against the CLI it should run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum LinkState {
    /// Nothing there.
    Missing,
    /// A link to this CLI.
    Current,
    /// A link to another K2 CLI (an old or moved bundle, maybe deleted).
    Stale { target: PathBuf },
    /// Not K2's: a real file, a folder, or a link to something else.
    Foreign { what: String },
}

fn same_file(a: &Path, b: &Path) -> bool {
    // Canonical on both sides: `/Applications` vs
    // `/System/Volumes/Data/Applications` must not read as different (#56).
    match (dunce::canonicalize(a), dunce::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// What is at `link`, against the CLI `cli` it should point at.
pub fn link_state(link: &Path, cli: &Path) -> LinkState {
    let meta = match fs::symlink_metadata(link) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return LinkState::Missing,
        Err(e) => return LinkState::Foreign { what: format!("something K2 can't read ({e})") },
    };
    if !meta.file_type().is_symlink() {
        let what = if meta.is_dir() { "a folder" } else { "a file that isn't a link to K2" };
        return LinkState::Foreign { what: what.to_string() };
    }
    let raw = match fs::read_link(link) {
        Ok(t) => t,
        Err(e) => return LinkState::Foreign { what: format!("a link K2 can't read ({e})") },
    };
    let target = if raw.is_absolute() {
        raw
    } else {
        link.parent().map(|p| p.join(&raw)).unwrap_or(raw)
    };
    if same_file(&target, cli) {
        LinkState::Current
    } else if is_k2_cli_target(&target) {
        LinkState::Stale { target }
    } else {
        LinkState::Foreign { what: format!("a link to {}", target.display()) }
    }
}

/// What [`ensure_link`] did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum LinkOutcome {
    Created,
    Repointed { from: PathBuf },
    Current,
    /// Left alone: not K2's.
    Foreign { what: String },
    /// The folder doesn't exist (and wasn't to be made) or isn't writable.
    NotWritable { reason: String },
}

impl LinkOutcome {
    /// The link now runs this CLI.
    pub fn is_ours(&self) -> bool {
        matches!(self, Self::Created | Self::Repointed { .. } | Self::Current)
    }
}

/// Point `link` at `cli` unless something that isn't K2's is there.
/// `make_dir`: create the parent folder when missing (`~/.local/bin`);
/// otherwise a missing folder is [`LinkOutcome::NotWritable`] (never
/// `mkdir /usr/local/bin`, which needs admin).
#[cfg(unix)]
pub fn ensure_link(link: &Path, cli: &Path, make_dir: bool) -> LinkOutcome {
    let from = match link_state(link, cli) {
        LinkState::Current => return LinkOutcome::Current,
        LinkState::Foreign { what } => return LinkOutcome::Foreign { what },
        LinkState::Missing => None,
        LinkState::Stale { target } => Some(target),
    };
    let Some(dir) = link.parent() else {
        return LinkOutcome::NotWritable { reason: format!("{} has no folder", link.display()) };
    };
    if !dir.is_dir() {
        if !make_dir {
            return LinkOutcome::NotWritable { reason: format!("{} doesn't exist", dir.display()) };
        }
        if let Err(e) = fs::create_dir_all(dir) {
            return LinkOutcome::NotWritable { reason: format!("create {}: {e}", dir.display()) };
        }
    }
    match replace_symlink(cli, link) {
        Ok(()) => match from {
            Some(from) => LinkOutcome::Repointed { from },
            None => LinkOutcome::Created,
        },
        Err(e) => LinkOutcome::NotWritable { reason: format!("link {}: {e}", link.display()) },
    }
}

/// Make `link` a symlink to `target` in one step: a fresh link beside it,
/// renamed over it (rename replaces a link atomically, so a `k2` running
/// in another window never finds the path missing).
#[cfg(unix)]
fn replace_symlink(target: &Path, link: &Path) -> io::Result<()> {
    let name = link.file_name().and_then(|n| n.to_str()).unwrap_or(CLI_NAME);
    let tmp = link.with_file_name(format!(".{name}.k2-link-{}", std::process::id()));
    let _ = fs::remove_file(&tmp);
    std::os::unix::fs::symlink(target, &tmp)?;
    fs::rename(&tmp, link).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

// ── Shells and startup files ────────────────────────────────────────────

/// The shells K2 knows how to put on PATH.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ShellKind {
    Zsh,
    Bash,
    Fish,
}

/// The shell kind from a shell path (`/bin/zsh`, `/opt/homebrew/bin/fish`).
pub fn shell_kind(shell: &Path) -> Option<ShellKind> {
    let name = shell.file_name()?.to_str()?.trim_start_matches('-');
    match name {
        "zsh" => Some(ShellKind::Zsh),
        "bash" => Some(ShellKind::Bash),
        "fish" => Some(ShellKind::Fish),
        _ => None,
    }
}

/// The person's login shell: `$SHELL`, else the passwd entry, else the
/// platform default (zsh on macOS, bash elsewhere). launchd may start the
/// daemon without `$SHELL`.
#[cfg(unix)]
pub fn user_shell() -> PathBuf {
    if let Some(s) = std::env::var_os("SHELL").filter(|s| !s.is_empty()) {
        return PathBuf::from(s);
    }
    // SAFETY: getpwuid returns a pointer into static storage or null; the
    // string is copied out before any other passwd call.
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if !pw.is_null() && !(*pw).pw_shell.is_null() {
            let s = std::ffi::CStr::from_ptr((*pw).pw_shell).to_string_lossy().into_owned();
            if !s.is_empty() {
                return PathBuf::from(s);
            }
        }
    }
    PathBuf::from(if cfg!(target_os = "macos") { "/bin/zsh" } else { "/bin/bash" })
}

/// `$ZDOTDIR`, when set (zsh reads `.zprofile` from there).
pub fn zdotdir() -> Option<PathBuf> {
    std::env::var_os("ZDOTDIR").filter(|s| !s.is_empty()).map(PathBuf::from)
}

/// The startup file the PATH block goes in for `kind`:
/// - zsh: `$ZDOTDIR/.zprofile` or `~/.zprofile` (read by every login
///   shell, which is what a new macOS Terminal window is);
/// - bash: the first of `~/.bash_profile`, `~/.bash_login`, `~/.profile`
///   that exists (bash reads only the first), else `~/.bash_profile`;
/// - fish: `~/.config/fish/conf.d/k2-cli.fish`.
pub fn startup_file(kind: ShellKind, home: &Path, zdotdir: Option<&Path>) -> PathBuf {
    match kind {
        ShellKind::Zsh => zdotdir.unwrap_or(home).join(".zprofile"),
        ShellKind::Bash => [".bash_profile", ".bash_login", ".profile"]
            .iter()
            .map(|n| home.join(n))
            .find(|p| p.exists())
            .unwrap_or_else(|| home.join(".bash_profile")),
        ShellKind::Fish => home.join(".config").join("fish").join("conf.d").join(FISH_CONF),
    }
}

/// Every file a K2 PATH block may be in, for the undo (the person may have
/// changed shells since it was written).
pub fn block_candidates(home: &Path, zdotdir: Option<&Path>) -> Vec<PathBuf> {
    let mut out = vec![home.join(".zprofile")];
    if let Some(z) = zdotdir {
        let p = z.join(".zprofile");
        if !out.contains(&p) {
            out.insert(0, p);
        }
    }
    for n in [".bash_profile", ".bash_login", ".profile"] {
        out.push(home.join(n));
    }
    out.push(startup_file(ShellKind::Fish, home, None));
    out
}

/// The marked PATH block for `kind`, newline-terminated.
pub fn path_block(kind: ShellKind) -> String {
    let body = match kind {
        ShellKind::Zsh | ShellKind::Bash => "export PATH=\"$HOME/.local/bin:$PATH\"".to_string(),
        ShellKind::Fish => {
            "if not contains -- $HOME/.local/bin $PATH\n    set -gx PATH $HOME/.local/bin $PATH\nend"
                .to_string()
        }
    };
    format!("{BLOCK_BEGIN}\n{body}\n{BLOCK_END}\n")
}

/// The file has a K2 block (its begin line).
pub fn has_block(content: &str) -> bool {
    content.lines().any(|l| l.trim_end() == BLOCK_BEGIN)
}

/// `content` with `block` appended, or `None` when a K2 block is already
/// there. Only ever appends: a newline first if the file didn't end with
/// one, nothing else changes.
pub fn add_block(content: &str, block: &str) -> Option<String> {
    if has_block(content) {
        return None;
    }
    let mut out = String::with_capacity(content.len() + block.len() + 1);
    out.push_str(content);
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(block);
    Some(out)
}

/// `content` without any complete K2 block (begin line through end line,
/// inclusive), or `None` when there is none. Lines outside the markers stay
/// byte for byte. A begin with no end is left alone (never guess where a
/// hand-edited block stops).
pub fn remove_block(content: &str) -> Option<String> {
    let lines: Vec<&str> = content.split_inclusive('\n').collect();
    let mut out = String::with_capacity(content.len());
    let mut removed = false;
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim_end() == BLOCK_BEGIN {
            if let Some(end) = (i + 1..lines.len()).find(|&j| lines[j].trim_end() == BLOCK_END) {
                removed = true;
                i = end + 1;
                continue;
            }
        }
        out.push_str(lines[i]);
        i += 1;
    }
    removed.then_some(out)
}

/// Write `content` to `path` through a temp file + rename, keeping the
/// file's mode. A startup file that is a link (a dotfiles repo) is written
/// at its target, so the link stays.
fn write_startup_file(path: &Path, content: &str) -> Result<PathBuf, String> {
    let real = if fs::symlink_metadata(path).is_ok() {
        dunce::canonicalize(path).map_err(|e| format!("resolve {}: {e}", path.display()))?
    } else {
        path.to_path_buf()
    };
    let dir = real.parent().ok_or_else(|| format!("{} has no folder", real.display()))?;
    fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let name = real.file_name().and_then(|n| n.to_str()).unwrap_or("startup");
    let tmp = dir.join(format!(".{name}.k2-tmp-{}", std::process::id()));
    fs::write(&tmp, content).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    if let Ok(meta) = fs::metadata(&real) {
        let _ = fs::set_permissions(&tmp, meta.permissions());
    }
    fs::rename(&tmp, &real).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("replace {}: {e}", real.display())
    })?;
    Ok(real)
}

// ── PATH ────────────────────────────────────────────────────────────────

/// `dir` is one of the entries of a `:`-separated PATH (a trailing `/` and
/// canonical-equal spellings count).
pub fn path_list_has_dir(path_list: &str, dir: &Path) -> bool {
    let want = dunce::canonicalize(dir).ok();
    path_list.split(':').filter(|e| !e.is_empty()).any(|e| {
        let e = Path::new(e.trim_end_matches('/'));
        e == dir || (want.is_some() && dunce::canonicalize(e).ok() == want)
    })
}

/// The first `name` a shell with this PATH would run.
pub fn first_on_path(path_list: &str, name: &str) -> Option<PathBuf> {
    path_list.split(':').filter(|e| !e.is_empty()).map(|e| Path::new(e).join(name)).find(|p| {
        fs::metadata(p).is_ok_and(|m| m.is_file())
    })
}

/// What [`ensure_path_block`] decided.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum PathOutcome {
    /// The login PATH already has `~/.local/bin`.
    OnPath,
    /// The login PATH already runs a K2 `k2` (e.g. `/usr/local/bin/k2`, a
    /// distro package's `/usr/bin/k2`): nothing to add.
    FoundElsewhere { path: PathBuf },
    /// The block was added to this file.
    BlockAdded { file: PathBuf },
    /// The block was already in this file.
    BlockPresent { file: PathBuf },
    /// The person removed the PATH line; left alone.
    OptedOut,
    /// Nothing written, and why (unknown shell, the login shell didn't
    /// answer, `~/.local/bin/k2` isn't K2's).
    Skipped { reason: String },
}

fn opt_out_path(home: &Path) -> PathBuf {
    home.join(".k2").join(OPT_OUT_FILE)
}

/// The person removed the PATH line from Settings (the boot heal leaves it).
pub fn path_opted_out(home: &Path) -> bool {
    opt_out_path(home).exists()
}

/// Add the PATH block for `shell` unless it isn't needed.
///
/// - `login_path`: the person's login-shell PATH ([`crate::process::login_shell_path`]);
///   `None` when the shell didn't answer. The boot heal then writes nothing;
///   an explicit install (`force`) writes the block anyway.
/// - `force`: an explicit install from Settings: also clears the opt-out.
///
/// Serialized across processes (the app and the daemon both run it at
/// first launch) by a lock under `~/.k2`, so the block is never doubled.
pub fn ensure_path_block(
    home: &Path,
    shell: &Path,
    zdotdir: Option<&Path>,
    login_path: Option<&str>,
    force: bool,
) -> Result<PathOutcome, String> {
    let Some(kind) = shell_kind(shell) else {
        return Ok(PathOutcome::Skipped {
            reason: format!("K2 doesn't know how to set PATH for {}", shell.display()),
        });
    };
    let _lock = HomeLock::take(home)?;
    if force {
        let _ = fs::remove_file(opt_out_path(home));
    } else if path_opted_out(home) {
        return Ok(PathOutcome::OptedOut);
    }
    let file = startup_file(kind, home, zdotdir);
    let content = match fs::read_to_string(&file) {
        Ok(c) => c,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("read {}: {e}", file.display())),
    };
    if has_block(&content) {
        return Ok(PathOutcome::BlockPresent { file });
    }
    let bin = user_bin_dir(home);
    match login_path {
        Some(p) if path_list_has_dir(p, &bin) => return Ok(PathOutcome::OnPath),
        Some(p) => {
            if let Some(found) = first_on_path(p, CLI_NAME).filter(|f| is_k2_cli_script(f)) {
                return Ok(PathOutcome::FoundElsewhere { path: found });
            }
        }
        None if !force => {
            return Ok(PathOutcome::Skipped {
                reason: "the login shell didn't report its PATH".to_string(),
            })
        }
        None => {}
    }
    let Some(next) = add_block(&content, &path_block(kind)) else {
        return Ok(PathOutcome::BlockPresent { file });
    };
    write_startup_file(&file, &next)?;
    Ok(PathOutcome::BlockAdded { file })
}

/// Remove the K2 PATH block from every startup file that has one (only the
/// lines between and including the markers) and record the opt-out so the
/// boot heal doesn't add it back. Returns the files changed; a fish file
/// left empty is deleted (the whole file was K2's).
pub fn remove_path_block_at(home: &Path, zdotdir: Option<&Path>) -> Result<Vec<PathBuf>, String> {
    let _lock = HomeLock::take(home)?;
    let mut changed = Vec::new();
    for file in block_candidates(home, zdotdir) {
        let Ok(content) = fs::read_to_string(&file) else { continue };
        let Some(next) = remove_block(&content) else { continue };
        let is_fish_conf = file.file_name().and_then(|n| n.to_str()) == Some(FISH_CONF);
        if is_fish_conf && next.trim().is_empty() {
            fs::remove_file(&file).map_err(|e| format!("remove {}: {e}", file.display()))?;
        } else {
            write_startup_file(&file, &next)?;
        }
        changed.push(file);
    }
    let marker = opt_out_path(home);
    if let Some(dir) = marker.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    }
    fs::write(&marker, "The person removed K2's PATH line from Settings; K2 won't add it back by itself.\n")
        .map_err(|e| format!("write {}: {e}", marker.display()))?;
    Ok(changed)
}

/// An exclusive `flock` on `~/.k2/.cli-install.lock` for the life of the
/// value.
struct HomeLock(#[allow(dead_code)] fs::File);

impl HomeLock {
    fn take(home: &Path) -> Result<Self, String> {
        let dir = home.join(".k2");
        fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
        let path = dir.join(".cli-install.lock");
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(|e| format!("open {}: {e}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            // SAFETY: a valid open fd; the lock goes when the file closes.
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
                return Err(format!("lock {}: {}", path.display(), io::Error::last_os_error()));
            }
        }
        Ok(Self(file))
    }
}

// ── The whole heal ──────────────────────────────────────────────────────

/// Inputs to [`heal_at`].
pub struct HealInput<'a> {
    /// The bundled `k2` script the links point at.
    pub cli: &'a Path,
    /// The bundled `k2so` shim, when the bundle has one.
    pub legacy_shim: Option<&'a Path>,
    /// `/usr/local/bin` (linked only when it exists and is writable), or
    /// `None` to leave it.
    pub system_bin: Option<&'a Path>,
    pub shell: &'a Path,
    pub zdotdir: Option<&'a Path>,
    /// The login-shell PATH, `None` when the shell didn't answer.
    pub login_path: Option<&'a str>,
    /// An explicit install (Settings): write the PATH block even without a
    /// login PATH, and clear the opt-out.
    pub force_path: bool,
}

/// What [`heal_at`] did, for logs and Settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealReport {
    pub user_link_path: PathBuf,
    pub user_link: LinkOutcome,
    pub user_legacy: Option<LinkOutcome>,
    pub system_link: Option<LinkOutcome>,
    pub system_legacy: Option<LinkOutcome>,
    pub path: PathOutcome,
}

impl HealReport {
    /// One log line per thing worth saying (a foreign `k2`, a new block).
    pub fn log_lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        let user = self.user_link_path.display();
        match &self.user_link {
            LinkOutcome::Created => out.push(format!("k2 linked at {user}")),
            LinkOutcome::Repointed { from } => {
                out.push(format!("k2 at {user} re-pointed (was {})", from.display()))
            }
            LinkOutcome::Foreign { what } => {
                out.push(format!("left {user} alone: it is {what}, not K2's"))
            }
            LinkOutcome::NotWritable { reason } => out.push(format!("couldn't link {user}: {reason}")),
            LinkOutcome::Current => {}
        }
        if let Some(LinkOutcome::Foreign { what }) = &self.system_link {
            out.push(format!("left {SYSTEM_BIN}/k2 alone: it is {what}, not K2's"));
        }
        if let Some(LinkOutcome::Created | LinkOutcome::Repointed { .. }) = &self.system_link {
            out.push(format!("k2 linked at {SYSTEM_BIN}/k2"));
        }
        match &self.path {
            PathOutcome::BlockAdded { file } => out.push(format!(
                "added the K2 PATH line to {} (new Terminal windows find k2)",
                file.display()
            )),
            PathOutcome::Skipped { reason } => out.push(format!("PATH line not added: {reason}")),
            _ => {}
        }
        out
    }
}

/// Link `~/.local/bin/k2` (+ `k2so`), link `/usr/local/bin/k2` when that
/// folder is writable, and add the PATH block when new Terminal windows
/// wouldn't find `k2` otherwise. Idempotent: a second run changes nothing.
#[cfg(unix)]
pub fn heal_at(home: &Path, input: &HealInput) -> Result<HealReport, String> {
    let bin = user_bin_dir(home);
    let user_link_path = bin.join(CLI_NAME);
    let user_link = ensure_link(&user_link_path, input.cli, true);
    let user_legacy = input.legacy_shim.map(|shim| ensure_link(&bin.join(LEGACY_NAME), shim, true));

    let (system_link, system_legacy) = match input.system_bin {
        Some(sys) if sys.is_dir() => {
            let link = ensure_link(&sys.join(CLI_NAME), input.cli, false);
            let legacy = match (&link, input.legacy_shim) {
                (l, Some(shim)) if l.is_ours() => Some(ensure_link(&sys.join(LEGACY_NAME), shim, false)),
                _ => None,
            };
            (Some(link), legacy)
        }
        _ => (None, None),
    };

    let path = if user_link.is_ours() {
        ensure_path_block(home, input.shell, input.zdotdir, input.login_path, input.force_path)?
    } else {
        PathOutcome::Skipped {
            reason: format!("{} isn't K2's link", user_link_path.display()),
        }
    };
    Ok(HealReport { user_link_path, user_link, user_legacy, system_link, system_legacy, path })
}

/// [`heal_at`] under `$HOME`.
#[cfg(unix)]
pub fn heal(input: &HealInput) -> Result<HealReport, String> {
    let home = crate::paths::home_dir().ok_or_else(|| "HOME is not set".to_string())?;
    heal_at(&home, input)
}

/// The boot heal for a packaged macOS K2, from the running exe (the app's
/// or the daemon's). Silent: never an admin prompt. `None` when there is
/// nothing to do here (a dev build, a DMG / translocated copy, not macOS).
/// Probes the login shell, so run it off the boot path (a thread).
#[cfg(unix)]
pub fn boot_heal(exe: &Path) -> Option<Result<HealReport, String>> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let cli = packaged_cli_for_exe(exe, CLI_NAME)?;
    if crate::daemon_lifecycle::is_transient_exe_location(&cli) {
        return None;
    }
    let shim = packaged_cli_for_exe(exe, LEGACY_NAME);
    ensure_executable(&cli);
    if let Some(s) = &shim {
        ensure_executable(s);
    }
    let shell = user_shell();
    let login = crate::process::login_shell_path(&shell);
    let zdot = zdotdir();
    Some(heal(&HealInput {
        cli: &cli,
        legacy_shim: shim.as_deref(),
        system_bin: Some(Path::new(SYSTEM_BIN)),
        shell: &shell,
        zdotdir: zdot.as_deref(),
        login_path: login.as_deref(),
        force_path: false,
    }))
}

/// Linux boot step after `cli_stage` put a copy at `staged`: when that is
/// `~/.local/bin/k2`, add the PATH block if the login PATH needs it.
/// `None` when the copy went elsewhere (`/usr/local/bin`, already on PATH).
#[cfg(unix)]
pub fn boot_path_for_staged(staged: &Path) -> Option<Result<PathOutcome, String>> {
    let home = crate::paths::home_dir()?;
    if staged != user_bin_dir(&home).join(CLI_NAME) {
        return None;
    }
    let shell = user_shell();
    let login = crate::process::login_shell_path(&shell);
    let zdot = zdotdir();
    Some(ensure_path_block(&home, &shell, zdot.as_deref(), login.as_deref(), false))
}

/// chmod 0755 (best effort): a bundle copied without modes still runs.
#[cfg(unix)]
pub fn ensure_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = fs::metadata(path) {
        if meta.permissions().mode() & 0o111 != 0o111 {
            let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o755));
        }
    }
}

// ── Status (Settings) ───────────────────────────────────────────────────

/// Where `k2` is and whether a new Terminal window will find it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PathStatus {
    /// `~/.local/bin`.
    pub user_bin: PathBuf,
    /// The person's shell, and the startup file K2's line goes in (`None`
    /// for a shell K2 doesn't know).
    pub shell: PathBuf,
    pub startup_file: Option<PathBuf>,
    /// The K2 block is in that file.
    pub block_present: bool,
    /// The login PATH has `~/.local/bin`; `None` when the shell didn't say.
    pub on_login_path: Option<bool>,
    /// The `k2` a new Terminal window runs (login PATH, plus the block),
    /// when it is the K2 CLI.
    pub new_terminals_run: Option<PathBuf>,
    /// A new Terminal window finds a K2 `k2`.
    pub new_terminals_find_it: bool,
    /// The person removed the PATH line.
    pub opted_out: bool,
}

/// [`PathStatus`] for `home` with this shell and login PATH.
pub fn path_status_at(
    home: &Path,
    shell: &Path,
    zdotdir: Option<&Path>,
    login_path: Option<&str>,
) -> PathStatus {
    let user_bin = user_bin_dir(home);
    let startup = shell_kind(shell).map(|k| startup_file(k, home, zdotdir));
    let block_present = startup
        .as_ref()
        .and_then(|f| fs::read_to_string(f).ok())
        .is_some_and(|c| has_block(&c));
    let on_login_path = login_path.map(|p| path_list_has_dir(p, &user_bin));
    // What a new window's PATH will be: the block runs in the login shell's
    // startup, so it puts ~/.local/bin in front.
    let effective = match (login_path, block_present) {
        (Some(p), true) => Some(format!("{}:{p}", user_bin.display())),
        (Some(p), false) => Some(p.to_string()),
        (None, true) => Some(user_bin.display().to_string()),
        (None, false) => None,
    };
    let new_terminals_run = effective
        .as_deref()
        .and_then(|p| first_on_path(p, CLI_NAME))
        .filter(|f| is_k2_cli_script(f));
    PathStatus {
        user_bin,
        shell: shell.to_path_buf(),
        startup_file: startup,
        block_present,
        on_login_path,
        new_terminals_find_it: new_terminals_run.is_some(),
        new_terminals_run,
        opted_out: path_opted_out(home),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn k2_cli_target_is_a_k2_or_k2so_in_a_cli_folder() {
        assert!(is_k2_cli_target(Path::new("/Applications/K2.app/Contents/Resources/_up_/cli/k2")));
        assert!(is_k2_cli_target(Path::new("/Applications/K2SO.app/Contents/Resources/_up_/cli/k2so")));
        assert!(is_k2_cli_target(Path::new("/Users/x/K2/cli/k2")));
        assert!(!is_k2_cli_target(Path::new("/opt/homebrew/bin/k2")));
        assert!(!is_k2_cli_target(Path::new("/Users/x/K2/cli/k2-widget-shot.mjs")));
        assert!(!is_k2_cli_target(Path::new("k2")));
    }

    #[test]
    fn shell_kinds() {
        assert_eq!(shell_kind(Path::new("/bin/zsh")), Some(ShellKind::Zsh));
        assert_eq!(shell_kind(Path::new("-zsh")), Some(ShellKind::Zsh));
        assert_eq!(shell_kind(Path::new("/opt/homebrew/bin/bash")), Some(ShellKind::Bash));
        assert_eq!(shell_kind(Path::new("/usr/bin/fish")), Some(ShellKind::Fish));
        assert_eq!(shell_kind(Path::new("/bin/tcsh")), None);
    }

    #[test]
    fn add_block_appends_once_and_terminates_the_last_line() {
        let block = path_block(ShellKind::Zsh);
        let once = add_block("eval \"$(brew shellenv)\"", &block).expect("first add");
        assert_eq!(once, format!("eval \"$(brew shellenv)\"\n{block}"));
        assert_eq!(add_block(&once, &block), None, "a second add must change nothing");
        assert_eq!(add_block("", &block).as_deref(), Some(block.as_str()));
    }

    #[test]
    fn remove_block_takes_only_the_marked_lines() {
        let block = path_block(ShellKind::Bash);
        let before = "# mine\nexport A=1\n";
        let after = "alias ll='ls -l'\n";
        let file = format!("{before}{block}{after}");
        assert_eq!(remove_block(&file).as_deref(), Some(format!("{before}{after}").as_str()));
        assert_eq!(remove_block(before), None);
        // A begin with no end is a hand edit: left alone.
        let broken = format!("{before}{BLOCK_BEGIN}\nexport PATH=x\n");
        assert_eq!(remove_block(&broken), None);
    }

    #[test]
    fn path_list_has_dir_ignores_a_trailing_slash() {
        let dir = Path::new("/Users/x/.local/bin");
        assert!(path_list_has_dir("/usr/bin:/Users/x/.local/bin/:/bin", dir));
        assert!(!path_list_has_dir("/usr/bin:/bin", dir));
        assert!(!path_list_has_dir("", dir));
    }
}
