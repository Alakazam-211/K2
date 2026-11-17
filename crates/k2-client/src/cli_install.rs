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
//! 1. **Links** `~/.local/bin/k2` (and refreshes a `k2so` alias an earlier
//!    install left) at the bundled CLI,
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
//! `cli/k2`) or K2's k5 binary is left alone and reported as
//! [`LinkState::Foreign`].
//!
//! **What the links run** ([`Choice`]): the script, as always, unless the
//! owner turned on "New k5 command (Beta)" (`k5CliBeta`, read through
//! [`crate::app_settings_view`]) and the bundle has the k5 binary and it is
//! this release's ([`check_binary`]: its `K5_CLI_VERSION=` marker equals the
//! script's `K2_CLI_VERSION`, read from the file, never by running it). A
//! missing binary, or one of another version, means the script. With the
//! binary, `k2` and a second link, `k5`, both run it. With the script, `k5`
//! is kept up to date only where K2 already made one (turning the switch off
//! re-points it, never removes it); a new install gets `k2` alone.
//!
//! **The swap:** each link is replaced in one step (a temp link beside it,
//! renamed over it), so a path never goes missing. A flip re-points `k2`
//! first, then `k5`; stopped between the two, one name runs the binary and
//! the other the script, both of this release, and the next heal finishes
//! it. Stopped between a temp link and its rename, the old link still works
//! and the temp (`.k2.k2-link-<pid>`) is removed by a later heal once that
//! process is gone ([`sweep_temp_links`]).
//!
//! Daemon-first: the daemon runs [`boot_heal`] at boot (so a headless or
//! launchd-started K2 heals too); the desktop app runs the same call as a
//! backstop and uses the status / install / undo helpers for Settings.
//! On Linux the daemon stages a copy (`k2_core::cli_stage`) and calls
//! [`ensure_path_block`] for `~/.local/bin` only.
//!
//! Everything takes an explicit home (`*_at`) so tests drive it under a
//! temp `$HOME`; the wrappers read `$HOME` through [`crate::paths::home_dir`].
//!
//! Every name comes from the brand file (`k2-brand`) through [`Names`]: the
//! command, its aliases, the PATH block's markers and the fish file. A build
//! owns its own name and the names it is retiring (aliases in mode `notice`
//! or `tombstone`, [`Names::retired`]), and nothing else: a link, a block or
//! a fish file of another brand is left as it is. The public functions use
//! [`Names::compiled`]; the methods on [`Names`] take the table as a value,
//! so tests run the real code with another brand's table.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use k2_brand::RetiredName;
use serde::Serialize;

use crate::brand::PRODUCT_NAME;

/// First line of the PATH block this build writes into a shell startup
/// file (`# >>> K2 CLI >>>` with the default brand).
pub const BLOCK_BEGIN: &str = k2_brand::PATH_BLOCK_BEGIN;
/// Last line of the PATH block.
pub const BLOCK_END: &str = k2_brand::PATH_BLOCK_END;

/// The command's name, from the brand file.
pub use crate::brand::CLI_NAME;

/// The brand's first alias (`k2so` with the default brand), or the
/// command's own name when it has none. The install code walks every alias
/// ([`Names::aliases`]); this one name is for callers that still read one.
pub const LEGACY_NAME: &str = match crate::brand::CLI_ALIASES {
    [first, ..] => first,
    [] => CLI_NAME,
};

/// The command's system-wide path, `/usr/local/bin/<name>` (the Linux copy
/// in `k2_core::cli_stage`, and the system link here).
pub use k2_brand::SYSTEM_CLI_PATH;
/// The command's next name: a second link beside `k2`, made when the links
/// run the k5 binary (it answers to both names, and so does the script).
/// A fixed name, not from the brand file, until the rename step.
pub const K5_NAME: &str = "k5";

/// What the k5 binary carries in its bytes: this, its version, then a NUL
/// (written by the binary's build script).
pub const BINARY_VERSION_MARKER: &str = "K5_CLI_VERSION=";
/// A k5 binary larger than this is not read (it is a few MB).
const BINARY_MAX_BYTES: u64 = 256 * 1024 * 1024;

/// The system-wide bin folder linked when it is already writable (macOS).
pub const SYSTEM_BIN: &str = "/usr/local/bin";

/// Under `~/.k2`: present once the person removed the PATH line, so the
/// boot heal leaves their startup file alone from then on.
pub const OPT_OUT_FILE: &str = "cli-path-line-removed";

/// The fish startup file this build owns (the whole file is its block).
const FISH_CONF: &str = k2_brand::FISH_CONF;

/// The alias check, shared with the Windows install (every OS).
pub use crate::cli_alias::legacy_alias_present;

/// `~/.local/bin`, where K2 puts `k2` without admin.
pub fn user_bin_dir(home: &Path) -> PathBuf {
    home.join(".local").join("bin")
}

// ── The names this build owns ───────────────────────────────────────────

/// The names one build installs and owns. [`Names::compiled`] is this
/// build's, from the brand file; a test makes its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Names {
    /// The command.
    pub name: &'static str,
    /// Its other names, in the brand's order. Each gets a link where an
    /// earlier install left one.
    pub aliases: &'static [&'static str],
    /// The aliases it is retiring (mode `notice` or `tombstone`), with the
    /// PATH block and fish file their installs left: this build takes those
    /// over. A `silent` alias never had an install of its own, so it is not
    /// here.
    pub retired: &'static [RetiredName],
    /// This build's PATH block markers.
    pub block_begin: &'static str,
    pub block_end: &'static str,
    /// This build's fish startup file in `conf.d`.
    pub fish_conf: &'static str,
}

impl Names {
    /// This build's names, from the brand file.
    pub const fn compiled() -> Names {
        Names {
            name: crate::brand::CLI_NAME,
            aliases: crate::brand::CLI_ALIASES,
            retired: k2_brand::RETIRED,
            block_begin: BLOCK_BEGIN,
            block_end: BLOCK_END,
            fish_conf: FISH_CONF,
        }
    }

    /// `file` is the command's name or one of its aliases.
    fn owns_file_name(&self, file: &str) -> bool {
        file == self.name || self.aliases.contains(&file)
    }

    /// The marker pairs this build may remove or replace: its own first,
    /// then each retired name's.
    fn marker_pairs(&self) -> Vec<(&'static str, &'static str)> {
        let mut out = vec![(self.block_begin, self.block_end)];
        out.extend(self.retired.iter().map(|r| (r.block_begin, r.block_end)));
        out
    }

    /// True when a link target is this build's CLI script by its place: a
    /// file named as the command or one of its aliases in a folder named
    /// `cli` (a bundle's `…/_up_/cli/k2`, or a checkout's `cli/k2`). Shape
    /// only, so a link to a deleted old bundle still counts and gets
    /// re-pointed. Another brand's command is never this build's. K2's k5
    /// binary by its name is ([`is_binary_target`]).
    pub fn is_cli_target(&self, target: &Path) -> bool {
        let name = target.file_name().and_then(|n| n.to_str());
        let dir = target.parent().and_then(|p| p.file_name()).and_then(|n| n.to_str());
        (name.is_some_and(|n| self.owns_file_name(n)) && dir == Some("cli")) || is_binary_target(target)
    }
}

// ── Which files are K2's ────────────────────────────────────────────────

/// [`Names::is_cli_target`] with this build's names.
pub fn is_k2_cli_target(target: &Path) -> bool {
    Names::compiled().is_cli_target(target)
}

/// The first 8 KiB of a file, links followed.
fn head_of(path: &Path) -> Option<Vec<u8>> {
    use std::io::Read;
    let file = fs::File::open(path).ok()?;
    let mut head = Vec::with_capacity(8192);
    file.take(8192).read_to_end(&mut head).ok()?;
    Some(head)
}

/// True when the file at `path` (links followed) is the K2 CLI: the script
/// (a `#!` first line and a `K2_CLI_VERSION=`, or pre-rename
/// `K2SO_CLI_VERSION=`, line near the top) or the k5 binary
/// ([`is_k5_binary`]). A program that merely shares the name (the Linux .deb
/// puts the GUI binary at `/usr/bin/k2`) is not.
pub fn is_k2_cli_script(path: &Path) -> bool {
    let Some(head) = head_of(path) else { return false };
    let text = String::from_utf8_lossy(&head);
    let mut lines = text.lines();
    if lines.next().is_some_and(|l| l.starts_with("#!")) {
        return lines.take(40).any(|l| l.starts_with("K2_CLI_VERSION=") || l.starts_with("K2SO_CLI_VERSION="));
    }
    is_k5_binary(path)
}

/// The `K2_CLI_VERSION="x.y.z"` of the script at `path`, quotes trimmed.
pub fn script_version(path: &Path) -> Option<String> {
    let head = head_of(path)?;
    let text = String::from_utf8_lossy(&head);
    let mut lines = text.lines();
    if !lines.next().is_some_and(|l| l.starts_with("#!")) {
        return None;
    }
    let line = lines.take(40).find_map(|l| l.strip_prefix("K2_CLI_VERSION="))?;
    let v = line.trim().trim_matches('"');
    (!v.is_empty()).then(|| v.to_string())
}

// ── The k5 binary ───────────────────────────────────────────────────────

/// Every version the k5 binary's bytes carry: `K5_CLI_VERSION=` then
/// letters, digits, `.`, `-` or `+`, then a NUL. A marker with no NUL
/// right after its version is not counted, so `0.46.1` never matches inside
/// `0.46.15`.
pub fn binary_versions(bytes: &[u8]) -> Vec<String> {
    let marker = BINARY_VERSION_MARKER.as_bytes();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i + marker.len() <= bytes.len() {
        if &bytes[i..i + marker.len()] != marker {
            i += 1;
            continue;
        }
        let start = i + marker.len();
        let len = bytes[start..]
            .iter()
            .take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'+'))
            .count();
        if len > 0 && bytes.get(start + len) == Some(&0) {
            let v = String::from_utf8_lossy(&bytes[start..start + len]).into_owned();
            if !out.contains(&v) {
                out.push(v);
            }
        }
        i = start;
    }
    out
}

/// True when the file at `path` (links followed) is a program carrying the
/// k5 binary's version marker (any version): an ELF or Mach-O file.
pub fn is_k5_binary(path: &Path) -> bool {
    let Some(head) = head_of(path) else { return false };
    let magic = head.get(..4).unwrap_or_default();
    let program = magic == b"\x7fELF"
        || magic == [0xcf, 0xfa, 0xed, 0xfe]
        || magic == [0xce, 0xfa, 0xed, 0xfe]
        || magic == [0xca, 0xfe, 0xba, 0xbe];
    if !program {
        return false;
    }
    match fs::metadata(path) {
        Ok(m) if m.len() <= BINARY_MAX_BYTES => {}
        _ => return false,
    }
    fs::read(path).is_ok_and(|bytes| !binary_versions(&bytes).is_empty())
}

/// `Ok` when the file at `path` is the k5 binary of version `want` (the
/// script's): read for its marker, never run. Otherwise why not, in words
/// the owner reads in Settings.
pub fn check_binary(path: &Path, want: Option<&str>) -> Result<(), String> {
    let Some(want) = want.filter(|w| !w.is_empty()) else {
        return Err("the script's version can't be read".to_string());
    };
    let meta = match fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err(format!("this release has no k5 program ({} is missing)", path.display()))
        }
        Err(e) => return Err(format!("{PRODUCT_NAME} can't read {}: {e}", path.display())),
    };
    if !meta.is_file() {
        return Err(format!("{} isn't a file", path.display()));
    }
    if meta.len() > BINARY_MAX_BYTES {
        return Err(format!("{} is too large to be the k5 program", path.display()));
    }
    let bytes = fs::read(path).map_err(|e| format!("{PRODUCT_NAME} can't read {}: {e}", path.display()))?;
    let found = binary_versions(&bytes);
    if found.iter().any(|v| v == want) {
        return Ok(());
    }
    match found.first() {
        Some(v) => Err(format!("the k5 program is version {v}, the script {want}")),
        None => Err(format!("{} carries no k5 version mark", path.display())),
    }
}

/// What the `k2` and `k5` links run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "runs", rename_all = "snake_case")]
pub enum Choice {
    /// The shell script (today's, and the default). `note`: when the switch
    /// is on, why not the binary.
    Script { note: Option<String> },
    /// The k5 binary at this path.
    Binary { binary: PathBuf },
}

impl Choice {
    /// The binary when `found` is one, else the script, with the reason
    /// only when the switch is on (off is not worth a note).
    pub fn from_found(on: bool, found: Result<PathBuf, String>) -> Self {
        match found {
            Ok(binary) => Choice::Binary { binary },
            Err(why) => Choice::Script { note: on.then_some(why) },
        }
    }

    /// The binary, when the links run it.
    pub fn binary(&self) -> Option<&Path> {
        match self {
            Choice::Binary { binary } => Some(binary),
            Choice::Script { .. } => None,
        }
    }

    /// What a link should point at: the binary when the links run it, else
    /// the bundled `script`. Settings judges a link against this, so a link
    /// to the binary is current while the switch has it.
    pub fn target<'a>(&'a self, script: &'a Path) -> &'a Path {
        self.binary().unwrap_or(script)
    }
}

/// The owner's switch (`k5CliBeta`) from `settings.json`.
pub fn switch_on() -> bool {
    crate::app_settings_view::load().k5_cli_beta
}

/// The bundle's `Contents/MacOS` folder for a bundled script at
/// `<X>.app/Contents/Resources/_up_/cli/<name>` ([`packaged_cli_for_exe`]'s
/// layout), else `None` (a checkout's `cli/k2`).
pub fn bundle_exe_dir_of_script(script: &Path) -> Option<PathBuf> {
    let cli = script.parent()?;
    let up = cli.parent()?;
    let resources = up.parent()?;
    let contents = resources.parent()?;
    let named = |p: &Path, n: &str| p.file_name().is_some_and(|x| x == n);
    let shaped = named(cli, "cli") && named(up, "_up_") && named(resources, "Resources") && named(contents, "Contents");
    (shaped && contents.parent()?.extension()? == "app").then(|| contents.join("MacOS"))
}

// k5-cli guard: begin
// tests/packaging/k5_cli_shipped_off_by_default.sh holds this file and the
// other install code to it: the binary's file name is spelled only between
// these two lines, the const below is read nowhere else, and every function
// here that returns a path starts with `if !on {`. So no install path leads
// to the binary while the owner's switch is off. The release and the
// packages ship the binary, under its own name only; this is the one place
// a link to it is decided.

/// The binary's file name.
const BINARY_FILE: &str = "k5-cli";

/// The k5 binary a bundled script's links run when the switch is `on`:
/// `<X>.app/Contents/MacOS/k5-cli`, beside the app's executable and the
/// other sidecars (from there the binary finds this same script,
/// `packaged_cli_for_exe`). It must be this release's ([`check_binary`]
/// against the script's version). Otherwise why not.
pub fn binary_for_script_if_on(on: bool, script: &Path) -> Result<PathBuf, String> {
    if !on {
        return Err("the switch is off".to_string());
    }
    let dir = bundle_exe_dir_of_script(script).ok_or_else(|| format!("{} isn't in an app bundle", script.display()))?;
    let binary = dir.join(BINARY_FILE);
    check_binary(&binary, script_version(script).as_deref())?;
    Ok(binary)
}

/// The k5 binary in `dir` when the switch is `on` (Linux: `dir` is the
/// daemon's own folder, so `/usr/bin/k2-daemon` has `/usr/bin/k5-cli`),
/// checked against `want`, the version of the script it replaces.
pub fn binary_in_dir_if_on(on: bool, dir: &Path, want: Option<&str>) -> Result<PathBuf, String> {
    if !on {
        return Err("the switch is off".to_string());
    }
    let binary = dir.join(BINARY_FILE);
    check_binary(&binary, want)?;
    Ok(binary)
}

/// True when a link target is K2's k5 binary by its name (shape only, as
/// [`is_k2_cli_target`]: a link to a deleted old bundle's binary is K2's).
pub fn is_binary_target(target: &Path) -> bool {
    target.file_name().is_some_and(|n| n == BINARY_FILE)
}
// k5-cli guard: end

/// What a heal of the bundled `script` links: the binary beside it when the
/// switch is on and the binary passes, else the script.
pub fn choice_for_script(on: bool, script: &Path) -> Choice {
    Choice::from_found(on, binary_for_script_if_on(on, script))
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

/// What is at `link`, against the CLI `cli` it should point at
/// ([`Names::link_state`] with this build's names).
pub fn link_state(link: &Path, cli: &Path) -> LinkState {
    Names::compiled().link_state(link, cli)
}

impl Names {
    /// What is at `link`, against the CLI `cli` it should point at. A link
    /// to another CLI is [`LinkState::Stale`] only when that CLI is one of
    /// these names ([`Names::is_cli_target`]).
    pub fn link_state(&self, link: &Path, cli: &Path) -> LinkState {
        link_state_by(link, cli, |t| self.is_cli_target(t))
    }

    /// Point `link` at `cli` unless something that isn't this build's is
    /// there. See [`ensure_link`].
    #[cfg(unix)]
    pub fn ensure_link(&self, link: &Path, cli: &Path, make_dir: bool) -> LinkOutcome {
        let from = match self.link_state(link, cli) {
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
}

fn link_state_by(link: &Path, cli: &Path, is_ours: impl Fn(&Path) -> bool) -> LinkState {
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
    } else if is_ours(&target) {
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
/// `mkdir /usr/local/bin`, which needs admin). With this build's names.
#[cfg(unix)]
pub fn ensure_link(link: &Path, cli: &Path, make_dir: bool) -> LinkOutcome {
    Names::compiled().ensure_link(link, cli, make_dir)
}

/// Make `link` a symlink to `target` in one step: a fresh link beside it,
/// renamed over it (rename replaces a link atomically, so a `k2` running
/// in another window never finds the path missing).
#[cfg(unix)]
fn replace_symlink(target: &Path, link: &Path) -> io::Result<()> {
    let tmp = begin_link_swap(target, link)?;
    finish_swap(&tmp, link)
}

/// The temp name beside `link` that this process swaps in through:
/// `.<name>.k2-link-<pid>`.
pub fn swap_temp_path(link: &Path) -> PathBuf {
    let name = link.file_name().and_then(|n| n.to_str()).unwrap_or(CLI_NAME);
    link.with_file_name(format!(".{name}.k2-link-{}", std::process::id()))
}

/// The swap's first half: a link to `target` at [`swap_temp_path`]. Until
/// [`finish_swap`] renames it, `link` is untouched (tests stop here to show
/// an interrupted swap leaves a working link).
#[cfg(unix)]
pub fn begin_link_swap(target: &Path, link: &Path) -> io::Result<PathBuf> {
    let tmp = swap_temp_path(link);
    let _ = fs::remove_file(&tmp);
    std::os::unix::fs::symlink(target, &tmp)?;
    Ok(tmp)
}

/// The swap's second half: `tmp` renamed over `link` (one step).
pub fn finish_swap(tmp: &Path, link: &Path) -> io::Result<()> {
    fs::rename(tmp, link).inspect_err(|_| {
        let _ = fs::remove_file(tmp);
    })
}

/// Remove the temp entries a swap left in `dir` when its process stopped
/// between the two halves: `.<name>.k2-link-<pid>` for the command, `k5` or
/// an alias, whose process is gone. A running process's temp is its own,
/// still in use. Returns what went. With this build's names
/// ([`Names::sweep_temp_links`]).
#[cfg(unix)]
pub fn sweep_temp_links(dir: &Path) -> Vec<PathBuf> {
    Names::compiled().sweep_temp_links(dir)
}

impl Names {
    /// [`sweep_temp_links`] with these names.
    #[cfg(unix)]
    pub fn sweep_temp_links(&self, dir: &Path) -> Vec<PathBuf> {
        let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
        let names: Vec<&str> = [self.name, K5_NAME].into_iter().chain(self.aliases.iter().copied()).collect();
        let mut gone = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let pid = names
                .iter()
                .find_map(|n| name.strip_prefix(&format!(".{n}.k2-link-")))
                .and_then(|p| p.parse::<i32>().ok());
            let Some(pid) = pid else { continue };
            if pid <= 0 || process_running(pid) {
                continue;
            }
            let path = entry.path();
            let plain = fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink() || m.is_file());
            if plain && fs::remove_file(&path).is_ok() {
                gone.push(path);
            }
        }
        gone
    }
}

#[cfg(unix)]
fn process_running(pid: i32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    if unsafe { libc::kill(pid, 0) } == 0 {
        return true;
    }
    io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// The `k5` link beside `k2`: made and kept at `target` when the links run
/// the binary; with the script, re-pointed only where K2 already made one
/// (`None` when there is none, or one that isn't K2's). With this build's
/// names ([`Names::ensure_k5_link`]).
#[cfg(unix)]
pub fn ensure_k5_link(link: &Path, target: &Path, binary: bool, make_dir: bool) -> Option<LinkOutcome> {
    Names::compiled().ensure_k5_link(link, target, binary, make_dir)
}

impl Names {
    /// [`ensure_k5_link`] with these names.
    #[cfg(unix)]
    pub fn ensure_k5_link(&self, link: &Path, target: &Path, binary: bool, make_dir: bool) -> Option<LinkOutcome> {
        if !binary && matches!(self.link_state(link, target), LinkState::Missing | LinkState::Foreign { .. }) {
            return None;
        }
        Some(self.ensure_link(link, target, make_dir))
    }
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
/// - fish: `~/.config/fish/conf.d/k2-cli.fish` (this build's fish file).
pub fn startup_file(kind: ShellKind, home: &Path, zdotdir: Option<&Path>) -> PathBuf {
    Names::compiled().startup_file(kind, home, zdotdir)
}

/// Every file a K2 PATH block may be in, for the undo (the person may have
/// changed shells since it was written).
pub fn block_candidates(home: &Path, zdotdir: Option<&Path>) -> Vec<PathBuf> {
    Names::compiled().block_candidates(home, zdotdir)
}

/// The marked PATH block for `kind`, newline-terminated.
pub fn path_block(kind: ShellKind) -> String {
    Names::compiled().path_block(kind)
}

/// The file has this build's block (its begin line).
pub fn has_block(content: &str) -> bool {
    Names::compiled().has_block(content)
}

/// `content` with `block` appended, or `None` when this build's block is
/// already there. Only ever appends: a newline first if the file didn't end
/// with one, nothing else changes.
pub fn add_block(content: &str, block: &str) -> Option<String> {
    if has_block(content) {
        return None;
    }
    Some(append_block(content, block))
}

fn append_block(content: &str, block: &str) -> String {
    let mut out = String::with_capacity(content.len() + block.len() + 1);
    out.push_str(content);
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(block);
    out
}

/// `content` without any complete block of this build or of a name it is
/// retiring ([`Names::remove_block`]), or `None` when there is none.
pub fn remove_block(content: &str) -> Option<String> {
    Names::compiled().remove_block(content)
}

/// fish's `conf.d`, where each command's fish file lives.
fn fish_conf_dir(home: &Path) -> PathBuf {
    home.join(".config").join("fish").join("conf.d")
}

/// The first complete marked block in `lines` whose begin line is one of
/// `pairs`: its first and last line indexes. A begin with no end of its
/// own pair after it is not a block.
fn find_block(lines: &[&str], from: usize, pairs: &[(&str, &str)]) -> Option<(usize, usize)> {
    (from..lines.len()).find_map(|i| {
        let (_, end) = pairs.iter().find(|(begin, _)| lines[i].trim_end() == *begin)?;
        let j = (i + 1..lines.len()).find(|&j| lines[j].trim_end() == *end)?;
        Some((i, j))
    })
}

/// `content` without any complete block of `pairs` (begin line through
/// its end line, inclusive), or `None` when there is none. Lines outside
/// the markers stay byte for byte. A begin with no end is left alone
/// (never guess where a hand-edited block stops).
fn remove_marked(content: &str, pairs: &[(&str, &str)]) -> Option<String> {
    let lines: Vec<&str> = content.split_inclusive('\n').collect();
    let mut out = String::with_capacity(content.len());
    let mut removed = false;
    let mut i = 0;
    while let Some((begin, end)) = find_block(&lines, i, pairs) {
        out.extend(lines[i..begin].iter().copied());
        removed = true;
        i = end + 1;
    }
    out.extend(lines[i..].iter().copied());
    removed.then_some(out)
}

impl Names {
    /// The startup file the PATH block goes in for `kind` ([`startup_file`]).
    pub fn startup_file(&self, kind: ShellKind, home: &Path, zdotdir: Option<&Path>) -> PathBuf {
        match kind {
            ShellKind::Zsh => zdotdir.unwrap_or(home).join(".zprofile"),
            ShellKind::Bash => [".bash_profile", ".bash_login", ".profile"]
                .iter()
                .map(|n| home.join(n))
                .find(|p| p.exists())
                .unwrap_or_else(|| home.join(".bash_profile")),
            ShellKind::Fish => fish_conf_dir(home).join(self.fish_conf),
        }
    }

    /// Every file this build's block, or a retired name's, may be in, for
    /// the undo: the zsh and bash startup files, this build's fish file,
    /// then each retired name's fish file.
    pub fn block_candidates(&self, home: &Path, zdotdir: Option<&Path>) -> Vec<PathBuf> {
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
        out.push(self.startup_file(ShellKind::Fish, home, None));
        out.extend(self.retired.iter().map(|r| fish_conf_dir(home).join(r.fish_conf)));
        out
    }

    /// The marked PATH block for `kind`, newline-terminated.
    pub fn path_block(&self, kind: ShellKind) -> String {
        let body = match kind {
            ShellKind::Zsh | ShellKind::Bash => "export PATH=\"$HOME/.local/bin:$PATH\"",
            ShellKind::Fish => {
                "if not contains -- $HOME/.local/bin $PATH\n    set -gx PATH $HOME/.local/bin $PATH\nend"
            }
        };
        format!("{}\n{body}\n{}\n", self.block_begin, self.block_end)
    }

    /// The file has this build's block (its begin line). A retired name's
    /// block does not count: it is this build's to replace.
    pub fn has_block(&self, content: &str) -> bool {
        content.lines().any(|l| l.trim_end() == self.block_begin)
    }

    /// `content` without any complete block of this build or of a name it
    /// is retiring (begin line through end line, inclusive), or `None` when
    /// there is none. Lines outside the markers stay byte for byte; a
    /// block with any other marker stays. A begin with no end is left alone
    /// (never guess where a hand-edited block stops).
    pub fn remove_block(&self, content: &str) -> Option<String> {
        remove_marked(content, &self.marker_pairs())
    }

    /// `content` with the first complete block of a retired name replaced
    /// by `block`, in the same place, or `None` when there is none.
    fn replace_retired_block(&self, content: &str, block: &str) -> Option<String> {
        let pairs: Vec<(&str, &str)> = self.retired.iter().map(|r| (r.block_begin, r.block_end)).collect();
        let lines: Vec<&str> = content.split_inclusive('\n').collect();
        let (begin, end) = find_block(&lines, 0, &pairs)?;
        let mut out = String::with_capacity(content.len() + block.len());
        out.extend(lines[..begin].iter().copied());
        out.push_str(block);
        out.extend(lines[end + 1..].iter().copied());
        Some(out)
    }
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
    /// A block of a name this build is retiring was replaced by this
    /// build's, in the same place (fish: this build's file took the old
    /// one's place).
    BlockReplaced { file: PathBuf },
    /// The person removed the PATH line; left alone.
    OptedOut,
    /// Nothing written, and why (unknown shell, the login shell didn't
    /// answer, `~/.local/bin/k2` isn't K2's).
    Skipped { reason: String },
}

fn opt_out_path(home: &Path) -> PathBuf {
    crate::paths::k2_home_in(home).join(OPT_OUT_FILE)
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
/// With this build's names ([`Names::ensure_path_block`]).
pub fn ensure_path_block(
    home: &Path,
    shell: &Path,
    zdotdir: Option<&Path>,
    login_path: Option<&str>,
    force: bool,
) -> Result<PathOutcome, String> {
    Names::compiled().ensure_path_block(home, shell, zdotdir, login_path, force)
}

/// Remove this build's PATH block, and a retired name's, from every startup
/// file that has one (only the lines between and including the markers)
/// and record the opt-out so the boot heal doesn't add it back. Returns the
/// files changed; a fish file left empty is deleted (the whole file was
/// the block). With this build's names ([`Names::remove_path_block_at`]).
pub fn remove_path_block_at(home: &Path, zdotdir: Option<&Path>) -> Result<Vec<PathBuf>, String> {
    Names::compiled().remove_path_block_at(home, zdotdir)
}

fn read_or_empty(file: &Path) -> Result<String, String> {
    match fs::read_to_string(file) {
        Ok(c) => Ok(c),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(format!("read {}: {e}", file.display())),
    }
}

impl Names {
    /// [`ensure_path_block`] with these names. This build's block already
    /// there: nothing to do. A retired name's block there: it is replaced
    /// by this build's in the same place, with no login PATH needed (the
    /// line was already on PATH). A block with any other marker is never
    /// read or touched.
    pub fn ensure_path_block(
        &self,
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
        let file = self.startup_file(kind, home, zdotdir);
        let content = read_or_empty(&file)?;
        if self.has_block(&content) {
            return Ok(PathOutcome::BlockPresent { file });
        }
        let block = self.path_block(kind);
        if let Some(next) = self.replace_retired_block(&content, &block) {
            write_startup_file(&file, &next)?;
            return Ok(PathOutcome::BlockReplaced { file });
        }
        if kind == ShellKind::Fish && self.take_retired_fish_file(home, &file, &content, &block)? {
            return Ok(PathOutcome::BlockReplaced { file });
        }
        let bin = user_bin_dir(home);
        match login_path {
            Some(p) if path_list_has_dir(p, &bin) => return Ok(PathOutcome::OnPath),
            Some(p) => {
                if let Some(found) = first_on_path(p, self.name).filter(|f| is_k2_cli_script(f)) {
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
        write_startup_file(&file, &append_block(&content, &block))?;
        Ok(PathOutcome::BlockAdded { file })
    }

    /// fish: a retired name's fish file with its block becomes this
    /// build's `file`. This build's block is written first, then the old
    /// file loses its block (and goes when nothing else is left in it), so
    /// PATH never misses the line. True when one was taken over.
    fn take_retired_fish_file(&self, home: &Path, file: &Path, content: &str, block: &str) -> Result<bool, String> {
        for r in self.retired {
            let old = fish_conf_dir(home).join(r.fish_conf);
            let old_content = read_or_empty(&old)?;
            let Some(rest) = remove_marked(&old_content, &[(r.block_begin, r.block_end)]) else { continue };
            write_startup_file(file, &append_block(content, block))?;
            if rest.trim().is_empty() {
                fs::remove_file(&old).map_err(|e| format!("remove {}: {e}", old.display()))?;
            } else {
                write_startup_file(&old, &rest)?;
            }
            return Ok(true);
        }
        Ok(false)
    }

    /// [`remove_path_block_at`] with these names: this build's block and a
    /// retired name's go, from every candidate file ([`Names::block_candidates`]).
    pub fn remove_path_block_at(&self, home: &Path, zdotdir: Option<&Path>) -> Result<Vec<PathBuf>, String> {
        let _lock = HomeLock::take(home)?;
        let mut changed = Vec::new();
        for file in self.block_candidates(home, zdotdir) {
            let Ok(content) = fs::read_to_string(&file) else { continue };
            let Some(next) = self.remove_block(&content) else { continue };
            let name = file.file_name().and_then(|n| n.to_str());
            let is_fish_conf = name == Some(self.fish_conf) || self.retired.iter().any(|r| name == Some(r.fish_conf));
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
}

/// An exclusive `flock` on `~/.k2/.cli-install.lock` for the life of the
/// value.
struct HomeLock(#[allow(dead_code)] fs::File);

impl HomeLock {
    fn take(home: &Path) -> Result<Self, String> {
        let dir = crate::paths::k2_home_in(home);
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
    /// The bundled `k2` script (the links run it, or the k5 binary beside
    /// it: [`Choice`]).
    pub cli: &'a Path,
    /// The bundled shim the alias links run (`k2so` with the default
    /// brand), when the bundle has one.
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
    /// What the links run.
    pub runs: Choice,
    pub user_link_path: PathBuf,
    pub user_link: LinkOutcome,
    /// `~/.local/bin/k5` ([`ensure_k5_link`]).
    pub user_k5: Option<LinkOutcome>,
    /// The first alias's link in `~/.local/bin` (`k2so` with the default
    /// brand), when an earlier install left one. Every alias's link is in
    /// [`HealReport::user_aliases`].
    pub user_legacy: Option<LinkOutcome>,
    pub system_link: Option<LinkOutcome>,
    pub system_k5: Option<LinkOutcome>,
    /// The same in the system bin folder.
    pub system_legacy: Option<LinkOutcome>,
    /// Every alias link refreshed in `~/.local/bin`, in the brand's order.
    pub user_aliases: Vec<AliasLink>,
    /// Every alias link refreshed in the system bin folder.
    pub system_aliases: Vec<AliasLink>,
    pub path: PathOutcome,
    /// Temp links an interrupted swap left, removed.
    pub swept: Vec<PathBuf>,
}

/// One alias link a heal refreshed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AliasLink {
    pub path: PathBuf,
    pub outcome: LinkOutcome,
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
        let k5 = self.user_link_path.with_file_name(K5_NAME);
        match &self.user_k5 {
            Some(LinkOutcome::Created) => out.push(format!("k5 linked at {}", k5.display())),
            Some(LinkOutcome::Repointed { from }) => {
                out.push(format!("k5 at {} re-pointed (was {})", k5.display(), from.display()))
            }
            Some(LinkOutcome::Foreign { what }) => {
                out.push(format!("left {} alone: it is {what}, not {PRODUCT_NAME}'s", k5.display()))
            }
            Some(LinkOutcome::NotWritable { reason }) => out.push(format!("couldn't link {}: {reason}", k5.display())),
            Some(LinkOutcome::Current) | None => {}
        }
        if let Choice::Script { note: Some(note) } = &self.runs {
            out.push(format!("the new k5 command is on, but the links run the script: {note}"));
        }
        for p in &self.swept {
            out.push(format!("removed {}, left by an interrupted swap", p.display()));
        }
        if let Some(LinkOutcome::Foreign { what }) = &self.system_link {
            out.push(format!("left {SYSTEM_CLI_PATH} alone: it is {what}, not K2's"));
        }
        if let Some(LinkOutcome::Created | LinkOutcome::Repointed { .. }) = &self.system_link {
            out.push(format!("k2 linked at {SYSTEM_CLI_PATH}"));
        }
        match &self.path {
            PathOutcome::BlockAdded { file } => out.push(format!(
                "added the K2 PATH line to {} (new Terminal windows find k2)",
                file.display()
            )),
            PathOutcome::BlockReplaced { file } => {
                out.push(format!("replaced the old command's PATH line in {}", file.display()))
            }
            PathOutcome::Skipped { reason } => out.push(format!("PATH line not added: {reason}")),
            _ => {}
        }
        out
    }
}

/// Link `~/.local/bin/k2` (and `k5`, [`ensure_k5_link`]) at what `runs`
/// says, link `/usr/local/bin/k2` (and `k5`) when that folder is writable,
/// and add the PATH block when new Terminal windows wouldn't find `k2`
/// otherwise. An alias (`k2so`) is only refreshed where an earlier install
/// left one ([`legacy_alias_present`]); a new install gets `k2` alone.
/// Idempotent: a second run changes nothing. With this build's names
/// ([`Names::heal_at`]).
#[cfg(unix)]
pub fn heal_at(home: &Path, input: &HealInput, runs: &Choice) -> Result<HealReport, String> {
    Names::compiled().heal_at(home, input, runs)
}

impl Names {
    /// [`heal_at`] with these names: the command's link (and `k5`) at what
    /// `runs` says, then each alias an earlier install left, in
    /// `~/.local/bin` and then in the system bin folder (its `k5` and
    /// aliases only over this build's own link there). The aliases run the
    /// shim either way.
    #[cfg(unix)]
    pub fn heal_at(&self, home: &Path, input: &HealInput, runs: &Choice) -> Result<HealReport, String> {
        let target = runs.target(input.cli);
        let binary = runs.binary().is_some();
        let bin = user_bin_dir(home);
        let mut swept = self.sweep_temp_links(&bin);
        let user_link_path = bin.join(self.name);
        let user_link = self.ensure_link(&user_link_path, target, true);
        // A build whose own name is `k5` has no second link to make.
        let k5_beside = self.name != K5_NAME;
        let user_k5 = (k5_beside && user_link.is_ours())
            .then(|| self.ensure_k5_link(&bin.join(K5_NAME), target, binary, true))
            .flatten();
        let user_aliases = self.refresh_aliases(&bin, input.legacy_shim, true);

        let (system_link, system_k5, system_aliases) = match input.system_bin {
            Some(sys) if sys.is_dir() => {
                swept.extend(self.sweep_temp_links(sys));
                let link = self.ensure_link(&sys.join(self.name), target, false);
                let k5 = (k5_beside && link.is_ours())
                    .then(|| self.ensure_k5_link(&sys.join(K5_NAME), target, binary, false))
                    .flatten();
                let aliases =
                    if link.is_ours() { self.refresh_aliases(sys, input.legacy_shim, false) } else { Vec::new() };
                (Some(link), k5, aliases)
            }
            _ => (None, None, Vec::new()),
        };

        let path = if user_link.is_ours() {
            self.ensure_path_block(home, input.shell, input.zdotdir, input.login_path, input.force_path)?
        } else {
            PathOutcome::Skipped {
                reason: format!("{} isn't K2's link", user_link_path.display()),
            }
        };
        Ok(HealReport {
            runs: runs.clone(),
            user_legacy: self.first_alias_in(&user_aliases),
            system_legacy: self.first_alias_in(&system_aliases),
            user_link_path,
            user_link,
            user_k5,
            system_link,
            system_k5,
            user_aliases,
            system_aliases,
            path,
            swept,
        })
    }

    /// Each alias in `dir` that an earlier install left, pointed at `shim`,
    /// in the brand's order. Nothing without a shim.
    #[cfg(unix)]
    fn refresh_aliases(&self, dir: &Path, shim: Option<&Path>, make_dir: bool) -> Vec<AliasLink> {
        let Some(shim) = shim else { return Vec::new() };
        self.aliases
            .iter()
            .map(|alias| dir.join(alias))
            .filter(|path| legacy_alias_present(path))
            .map(|path| AliasLink { outcome: self.ensure_link(&path, shim, make_dir), path })
            .collect()
    }

    /// The first alias's outcome among `links`.
    #[cfg(unix)]
    fn first_alias_in(&self, links: &[AliasLink]) -> Option<LinkOutcome> {
        let first = self.aliases.first()?;
        links
            .iter()
            .find(|l| l.path.file_name().and_then(|n| n.to_str()) == Some(*first))
            .map(|l| l.outcome.clone())
    }
}

/// [`heal_at`] under `$HOME`, with the links' target from the owner's
/// switch and the bundle ([`choice_for_script`]).
#[cfg(unix)]
pub fn heal(input: &HealInput) -> Result<HealReport, String> {
    let home = crate::paths::home_dir().ok_or_else(|| "HOME is not set".to_string())?;
    heal_at(&home, input, &choice_for_script(switch_on(), input.cli))
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
    let names = Names::compiled();
    let cli = packaged_cli_for_exe(exe, names.name)?;
    if crate::daemon_lifecycle::is_transient_exe_location(&cli) {
        return None;
    }
    // The bundle's alias shim: the first alias the bundle has a file for.
    let shim = names.aliases.iter().find_map(|alias| packaged_cli_for_exe(exe, alias));
    ensure_executable(&cli);
    if let Some(s) = &shim {
        ensure_executable(s);
    }
    if let Some(binary) = choice_for_script(switch_on(), &cli).binary() {
        ensure_executable(binary);
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

// ── Uninstall (Settings) ────────────────────────────────────────────────

/// The names Settings' uninstall removes from `~/.local/bin`: `k2`, `k5`
/// and the `k2so` alias, each only when it is K2's ([`is_our_link`]).
pub const UNINSTALL_NAMES: [&str; 3] = [CLI_NAME, K5_NAME, LEGACY_NAME];

/// True when `path` is a link K2 made: its target is K2's CLI by its place
/// ([`is_k2_cli_target`]: a bundle's or a checkout's `cli/k2`, or the k5
/// binary), even when that target is gone. A file, a folder, or a link to
/// anything else (someone else's `k5`) is not.
pub fn is_our_link(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
        && fs::read_link(path).is_ok_and(|t| is_k2_cli_target(&t))
}

/// The `names` in `dir` that are K2's links ([`is_our_link`]), in order.
pub fn our_links_in(dir: &Path, names: &[&str]) -> Vec<PathBuf> {
    names.iter().map(|n| dir.join(n)).filter(|p| is_our_link(p)).collect()
}

/// Remove K2's links among `names` in `dir` (the links themselves, never
/// what they point at). Returns what went; stops at the first that can't be
/// removed.
pub fn remove_our_links(dir: &Path, names: &[&str]) -> Result<Vec<PathBuf>, String> {
    let mut gone = Vec::new();
    for link in our_links_in(dir, names) {
        fs::remove_file(&link).map_err(|e| format!("remove {}: {e}", link.display()))?;
        gone.push(link);
    }
    Ok(gone)
}

// ── Status (Settings) ───────────────────────────────────────────────────

/// What an installed `k2` runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CliKind {
    Script,
    Binary,
}

/// What runs at an installed `k2`, and its version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledCli {
    pub runs: CliKind,
    pub version: Option<String>,
}

/// What runs at `path` (links followed), for Settings: the script and its
/// `K2_CLI_VERSION` (or a pre-rename `K2SO_CLI_VERSION`), or the k5 binary
/// and the version its mark carries (`want`, the bundled script's, when the
/// binary carries it, else the first). Read, never run. `None` when it isn't
/// the K2 CLI.
pub fn installed_cli(path: &Path, want: Option<&str>) -> Option<InstalledCli> {
    if is_k5_binary(path) {
        let found = fs::read(path).map(|b| binary_versions(&b)).unwrap_or_default();
        let version = match want {
            Some(w) if found.iter().any(|v| v == w) => Some(w.to_string()),
            _ => found.into_iter().next(),
        };
        return Some(InstalledCli { runs: CliKind::Binary, version });
    }
    if !is_k2_cli_script(path) {
        return None;
    }
    let head = head_of(path)?;
    let text = String::from_utf8_lossy(&head);
    let version = text.lines().skip(1).take(40).find_map(|l| {
        let v = l.strip_prefix("K2_CLI_VERSION=").or_else(|| l.strip_prefix("K2SO_CLI_VERSION="))?;
        let v = v.trim().trim_matches('"');
        (!v.is_empty()).then(|| v.to_string())
    });
    Some(InstalledCli { runs: CliKind::Script, version })
}

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
    /// when it is the K2 CLI (the script or the k5 binary).
    pub new_terminals_run: Option<PathBuf>,
    /// A new Terminal window finds a K2 `k2`.
    pub new_terminals_find_it: bool,
    /// The person removed the PATH line.
    pub opted_out: bool,
}

/// [`PathStatus`] for `home` with this shell and login PATH, with this
/// build's names ([`Names::path_status_at`]).
pub fn path_status_at(
    home: &Path,
    shell: &Path,
    zdotdir: Option<&Path>,
    login_path: Option<&str>,
) -> PathStatus {
    Names::compiled().path_status_at(home, shell, zdotdir, login_path)
}

impl Names {
    /// [`path_status_at`] with these names.
    pub fn path_status_at(
        &self,
        home: &Path,
        shell: &Path,
        zdotdir: Option<&Path>,
        login_path: Option<&str>,
    ) -> PathStatus {
        let user_bin = user_bin_dir(home);
        let startup = shell_kind(shell).map(|k| self.startup_file(k, home, zdotdir));
        let block_present = startup
            .as_ref()
            .and_then(|f| fs::read_to_string(f).ok())
            .is_some_and(|c| self.has_block(&c));
        let on_login_path = login_path.map(|p| path_list_has_dir(p, &user_bin));
        // What a new window's PATH will be: the block runs in the login
        // shell's startup, so it puts ~/.local/bin in front.
        let effective = match (login_path, block_present) {
            (Some(p), true) => Some(format!("{}:{p}", user_bin.display())),
            (Some(p), false) => Some(p.to_string()),
            (None, true) => Some(user_bin.display().to_string()),
            (None, false) => None,
        };
        let new_terminals_run = effective
            .as_deref()
            .and_then(|p| first_on_path(p, self.name))
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
    fn a_version_counts_only_with_its_nul() {
        assert_eq!(binary_versions(b"..K5_CLI_VERSION=0.46.15\0.."), vec!["0.46.15".to_string()]);
        assert_eq!(binary_versions(b"K5_CLI_VERSION=0.46.15"), Vec::<String>::new(), "no NUL, no version");
        assert_eq!(binary_versions(b"K5_CLI_VERSION=\0"), Vec::<String>::new(), "empty");
        assert_eq!(binary_versions(b"K5_CLI_VERSION=build.rs writes"), Vec::<String>::new(), "a bare prefix");
        assert_eq!(
            binary_versions(b"K5_CLI_VERSION=K5_CLI_VERSION=1.2.3\0 K5_CLI_VERSION=1.2.3\0 K5_CLI_VERSION=2.0.0-rc1+x\0"),
            vec!["1.2.3".to_string(), "2.0.0-rc1+x".to_string()]
        );
    }

    #[test]
    fn the_bundle_exe_dir_comes_from_the_script_s_place_only() {
        assert_eq!(
            bundle_exe_dir_of_script(Path::new("/Applications/K2.app/Contents/Resources/_up_/cli/k2")),
            Some(PathBuf::from("/Applications/K2.app/Contents/MacOS"))
        );
        assert_eq!(bundle_exe_dir_of_script(Path::new("/Users/x/K2/cli/k2")), None);
        assert_eq!(bundle_exe_dir_of_script(Path::new("/opt/K2/Contents/Resources/_up_/cli/k2")), None, "not an .app");
        assert_eq!(bundle_exe_dir_of_script(Path::new("k2")), None);
    }

    #[test]
    fn choice_off_never_names_the_binary_and_carries_no_note() {
        let script = Path::new("/no-such-dir-k2-test/K2.app/Contents/Resources/_up_/cli/k2");
        assert_eq!(choice_for_script(false, script), Choice::Script { note: None });
        assert!(binary_for_script_if_on(false, script).is_err());
        assert!(binary_in_dir_if_on(false, Path::new("/usr/bin"), Some("1.0.0")).is_err());
        // On, with nothing there: the script, and why.
        match choice_for_script(true, script) {
            Choice::Script { note: Some(n) } => assert_eq!(n, "the script's version can't be read"),
            other => panic!("{other:?}"),
        }
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

#[cfg(all(test, unix))]
#[path = "cli_install_names_tests.rs"]
mod names_tests;
